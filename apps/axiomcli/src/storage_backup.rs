//! Offline, portable backups of encrypted databases and external asset files.
//! The archive contains ciphertext and an authenticated, recovery-wrapped key.

use std::{
    collections::HashSet,
    fs::{self, File},
    io::{IsTerminal as _, Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context as _, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use zeroize::Zeroizing;

use crate::{
    local_storage::{self, StorageKey},
    paths::{AxiomPaths, FrontendKind},
    session::{SessionStore, asset_directory_for_database},
};

const MAGIC: &[u8; 8] = b"AXBKP001";
const DOMAIN: &[u8] = b"axiom-encrypted-backup-manifest-v1\0";
const MAX_HEADER_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, clap::Args)]
pub struct Arguments {
    #[command(subcommand)]
    action: Action,
}

#[derive(Debug, clap::Subcommand)]
enum Action {
    /// Back up one local store. Close Axiom first; keep the printed recovery key separately.
    Backup {
        #[arg(long)]
        account: String,
        #[arg(long, value_enum)]
        frontend: FrontendKind,
        #[arg(long)]
        output: PathBuf,
    },
    /// Restore into an empty account/frontend store. Existing local data is never overwritten.
    Restore {
        #[arg(long)]
        input: PathBuf,
        /// Read the 64-character recovery key from a pipe, never a command argument.
        #[arg(long, required = true)]
        recovery_key_stdin: bool,
    },
    /// Recover the OS-store key for an existing database without replacing chat files.
    RecoverKey {
        #[arg(long)]
        input: PathBuf,
        #[arg(long, required = true)]
        recovery_key_stdin: bool,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    path: String,
    bytes: u64,
    sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    version: u32,
    account: String,
    frontend: FrontendKind,
    files: Vec<Entry>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    manifest: Manifest,
    wrapped_key: String,
}

struct OpenArchive {
    file: File,
    manifest: Manifest,
    key: Arc<StorageKey>,
    identity: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RestoreIntent {
    version: u32,
    archive: String,
    activation: String,
}

pub fn command(arguments: &Arguments) -> anyhow::Result<()> {
    let paths = AxiomPaths::discover()?;
    let _lease = crate::factory_reset::maintenance_lease(&paths)?;
    match &arguments.action {
        Action::Backup {
            account,
            frontend,
            output,
        } => {
            let database = paths.account_state_database(account, *frontend)?;
            ensure!(
                database.is_file(),
                "The selected local database does not exist"
            );
            let key = local_storage::load_key_for_path(&database)?;
            // This also upgrades an older asset representation before taking
            // the consistent encrypted snapshot. No inference is performed.
            let store = SessionStore::open_protected_with_key(&database, key.clone())?;
            store.verify_asset_integrity()?;
            drop(store);
            let mut recovery = Zeroizing::new([0; 32]);
            getrandom::fill(recovery.as_mut())
                .context("Could not generate a backup recovery key")?;
            create_archive(&database, account, *frontend, output, &key, &recovery)?;
            println!("Encrypted backup created at {}", output.display());
            println!(
                "Keep this recovery key separately from the backup. It is shown only for this explicit export:"
            );
            println!("{}", hex::encode(recovery.as_ref()));
        }
        Action::Restore {
            input,
            recovery_key_stdin,
        } => {
            ensure!(*recovery_key_stdin, "Recovery key input is required");
            let recovery = read_recovery_key()?;
            let archive = open_archive(input, &recovery)?;
            restore_archive(&paths, archive, local_storage::save_key_for_path)?;
            println!(
                "Encrypted local store restored. Open Axiom and sign in to the owning account."
            );
        }
        Action::RecoverKey {
            input,
            recovery_key_stdin,
        } => {
            ensure!(*recovery_key_stdin, "Recovery key input is required");
            let recovery = read_recovery_key()?;
            let archive = open_archive(input, &recovery)?;
            let database = paths
                .account_state_database(&archive.manifest.account, archive.manifest.frontend)?;
            recover_existing_key(&database, &archive.key, local_storage::save_key_for_path)?;
            println!(
                "The existing local store's key was recovered into the system credential store."
            );
        }
    }
    Ok(())
}

fn recover_existing_key(
    database: &Path,
    key: &StorageKey,
    install: impl FnOnce(&Path, &StorageKey) -> crate::Result<()>,
) -> anyhow::Result<()> {
    ensure!(
        database.is_file(),
        "There is no existing database at this account/frontend path; use restore instead"
    );
    let mut header = [0u8; 16];
    let mut file = regular_file(database)?;
    ensure!(
        file.read(&mut header)? == header.len() && &header != b"SQLite format 3\0",
        "Key-only recovery requires an existing encrypted database; local files were preserved"
    );
    // Key-only recovery must not initialize, migrate or interrupt chats. The
    // installer validates the encrypted database and key before writing.
    install(database, key)?;
    Ok(())
}

/// Confirmed offline reset shares the maintenance fence with backup/recovery.
pub fn reset_local_state(
    account: &str,
    frontend: FrontendKind,
    confirm: bool,
) -> anyhow::Result<()> {
    ensure!(confirm, "refusing to delete local state without --confirm");
    let paths = AxiomPaths::discover()?;
    let _maintenance = crate::factory_reset::maintenance_lease(&paths)?;
    paths.prepare()?;
    let removed = paths.reset_account_frontend_state(account, frontend)?;
    println!(
        "removed {} local state file(s) for account {account}, frontend {frontend}",
        removed.len()
    );
    Ok(())
}

fn read_recovery_key() -> anyhow::Result<Zeroizing<[u8; 32]>> {
    ensure!(
        !std::io::stdin().is_terminal(),
        "Pipe the recovery key through standard input; it is not accepted as a command argument"
    );
    let mut text = Zeroizing::new(String::new());
    std::io::stdin()
        .lock()
        .take(1024)
        .read_to_string(&mut text)?;
    let text = text.trim();
    ensure!(
        text.len() == 64,
        "The recovery key must contain 64 hexadecimal characters"
    );
    let mut key = Zeroizing::new([0; 32]);
    hex::decode_to_slice(text, key.as_mut()).context("Invalid recovery key")?;
    Ok(key)
}

fn authenticated_manifest(manifest: &Manifest) -> anyhow::Result<Vec<u8>> {
    let mut aad = DOMAIN.to_vec();
    aad.extend_from_slice(&serde_json::to_vec(manifest)?);
    Ok(aad)
}

fn regular_file(path: &Path) -> anyhow::Result<File> {
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_file() && !metadata.is_symlink(),
        "Backup inputs must be regular files"
    );
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(nix::libc::O_NOFOLLOW);
    }
    Ok(options.open(path)?)
}

fn entry(path: &Path, relative: String) -> anyhow::Result<Entry> {
    validate_entry_path(&relative)?;
    let mut file = regular_file(path)?;
    let mut sha = Sha256::new();
    let mut bytes = 0u64;
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        bytes = bytes
            .checked_add(count as u64)
            .context("Backup file length overflow")?;
        sha.update(&buffer[..count]);
    }
    Ok(Entry {
        path: relative,
        bytes,
        sha256: hex::encode(sha.finalize()),
    })
}

fn asset_files(root: &Path) -> anyhow::Result<Vec<(String, PathBuf)>> {
    fn visit(
        root: &Path,
        directory: &Path,
        files: &mut Vec<(String, PathBuf)>,
    ) -> anyhow::Result<()> {
        let metadata = fs::symlink_metadata(directory)?;
        ensure!(
            metadata.is_dir() && !metadata.is_symlink(),
            "Invalid encrypted asset directory"
        );
        for item in fs::read_dir(directory)? {
            let item = item?;
            let metadata = fs::symlink_metadata(item.path())?;
            ensure!(
                !metadata.is_symlink(),
                "Encrypted asset directories must not contain symlinks"
            );
            if metadata.is_dir() {
                visit(root, &item.path(), files)?;
            } else {
                ensure!(metadata.is_file(), "Invalid encrypted asset file");
                let relative = item
                    .path()
                    .strip_prefix(root)?
                    .components()
                    .map(|part| {
                        part.as_os_str()
                            .to_str()
                            .map(str::to_owned)
                            .context("Invalid asset filename")
                    })
                    .collect::<anyhow::Result<Vec<_>>>()?
                    .join("/");
                let name = format!("assets/{relative}");
                validate_entry_path(&name)?;
                files.push((name, item.path()));
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    if root.try_exists()? {
        visit(root, root, &mut files)?;
    }
    files.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    Ok(files)
}

fn create_archive(
    database: &Path,
    account: &str,
    frontend: FrontendKind,
    output: &Path,
    key: &Arc<StorageKey>,
    recovery: &[u8; 32],
) -> anyhow::Result<()> {
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    ensure!(!output.try_exists()?, "Backup output already exists");
    let staging = tempfile::tempdir_in(parent)?;
    let snapshot = staging.path().join("snapshot.sqlite3");
    let opened = local_storage::open_database_with_key(database, key.clone())?;
    local_storage::backup_database(&opened.connection, key, &snapshot)?;
    drop(opened);
    let mut sources = vec![("database".to_owned(), snapshot)];
    sources.extend(asset_files(&asset_directory_for_database(database))?);
    let files = sources
        .iter()
        .map(|(name, path)| entry(path, name.clone()))
        .collect::<anyhow::Result<Vec<_>>>()?;
    let manifest = Manifest {
        version: 1,
        account: account.to_owned(),
        frontend,
        files,
    };
    let wrapped_key =
        STANDARD.encode(key.wrap_for_recovery(recovery, &authenticated_manifest(&manifest)?)?);
    let header = serde_json::to_vec(&Envelope {
        manifest: manifest.clone(),
        wrapped_key,
    })?;
    ensure!(
        header.len() as u64 <= MAX_HEADER_BYTES,
        "Encrypted backup manifest exceeds its metadata budget"
    );
    let mut pending = tempfile::NamedTempFile::new_in(parent)?;
    pending.write_all(MAGIC)?;
    pending.write_all(&(header.len() as u64).to_le_bytes())?;
    pending.write_all(&header)?;
    for ((_, source), expected) in sources.iter().zip(&manifest.files) {
        let mut source = regular_file(source)?;
        copy_verified(&mut source, pending.as_file_mut(), expected)?;
        ensure!(
            source.read(&mut [0u8; 1])? == 0,
            "A backup input changed while being copied"
        );
    }
    pending.as_file().sync_all()?;
    pending
        .persist_noclobber(output)
        .map_err(|error| error.error)?;
    sync_directory(parent)?;
    Ok(())
}

fn validate_entry_path(path: &str) -> anyhow::Result<()> {
    if path == "database" {
        return Ok(());
    }
    ensure!(
        path.starts_with("assets/"),
        "Invalid encrypted backup entry"
    );
    for component in path[7..].split('/') {
        ensure!(
            !component.is_empty()
                && component != "."
                && component != ".."
                && component.len() <= 255,
            "Invalid encrypted asset path"
        );
        ensure!(
            component.bytes().all(|c| c.is_ascii_lowercase()
                || c.is_ascii_digit()
                || matches!(c, b'.' | b'-' | b'_'))
                && !component.ends_with('.'),
            "Invalid encrypted asset path"
        );
        let stem = component.split('.').next().unwrap_or_default();
        ensure!(
            !matches!(stem, "con" | "prn" | "aux" | "nul")
                && !(stem.len() == 4
                    && (stem.starts_with("com") || stem.starts_with("lpt"))
                    && stem.as_bytes()[3].is_ascii_digit()),
            "Invalid encrypted asset path"
        );
    }
    Ok(())
}

fn open_archive(path: &Path, recovery: &[u8; 32]) -> anyhow::Result<OpenArchive> {
    let mut file = regular_file(path)?;
    let mut magic = [0; 8];
    file.read_exact(&mut magic)?;
    ensure!(&magic == MAGIC, "Unsupported encrypted backup format");
    let mut length = [0; 8];
    file.read_exact(&mut length)?;
    let length = u64::from_le_bytes(length);
    ensure!(
        length > 0
            && length <= MAX_HEADER_BYTES
            && length <= file.metadata()?.len().saturating_sub(16),
        "Invalid encrypted backup metadata size"
    );
    let mut header = vec![0; usize::try_from(length)?];
    file.read_exact(&mut header)?;
    let envelope: Envelope =
        serde_json::from_slice(&header).context("Invalid encrypted backup manifest")?;
    let manifest = envelope.manifest;
    ensure!(
        manifest.version == 1,
        "Unsupported encrypted backup version"
    );
    let wrapped = STANDARD
        .decode(envelope.wrapped_key)
        .context("Invalid recovery key envelope")?;
    let key = Arc::new(StorageKey::from_recovery(
        recovery,
        &authenticated_manifest(&manifest)?,
        &wrapped,
    )?);
    let mut seen = HashSet::new();
    let mut expected_bytes = 16u64.checked_add(length).context("Backup size overflow")?;
    for entry in &manifest.files {
        validate_entry_path(&entry.path)?;
        ensure!(
            seen.insert(entry.path.clone()),
            "Duplicate encrypted backup entry"
        );
        ensure!(
            entry.sha256.len() == 64 && entry.sha256.bytes().all(|b| b.is_ascii_hexdigit()),
            "Invalid backup digest"
        );
        expected_bytes = expected_bytes
            .checked_add(entry.bytes)
            .context("Backup size overflow")?;
    }
    ensure!(
        seen.contains("database") && expected_bytes == file.metadata()?.len(),
        "Incomplete encrypted backup"
    );
    Ok(OpenArchive {
        file,
        manifest,
        key,
        identity: hex::encode(Sha256::digest(&header)),
    })
}

fn copy_verified(
    source: &mut impl Read,
    target: &mut impl Write,
    expected: &Entry,
) -> anyhow::Result<()> {
    let mut remaining = expected.bytes;
    let mut sha = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    while remaining > 0 {
        let limit = usize::try_from(remaining.min(buffer.len() as u64))?;
        let count = source.read(&mut buffer[..limit])?;
        ensure!(count > 0, "Truncated encrypted backup entry");
        target.write_all(&buffer[..count])?;
        sha.update(&buffer[..count]);
        remaining -= count as u64;
    }
    ensure!(
        hex::encode(sha.finalize()) == expected.sha256,
        "Encrypted backup entry failed integrity verification"
    );
    Ok(())
}

fn extract_archive(archive: &mut OpenArchive, destination: &Path) -> anyhow::Result<PathBuf> {
    let database = destination.join("state.sqlite3");
    let assets = asset_directory_for_database(&database);
    for entry in &archive.manifest.files {
        let path = if entry.path == "database" {
            database.clone()
        } else {
            assets.join(&entry.path[7..])
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut target = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            target.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        copy_verified(&mut archive.file, &mut target, entry)?;
        target.sync_all()?;
    }
    ensure!(
        archive.file.read(&mut [0u8; 1])? == 0,
        "Unexpected encrypted backup trailer"
    );
    sync_directory_tree(destination)?;
    Ok(database)
}

fn sync_directory_tree(directory: &Path) -> anyhow::Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let metadata = fs::symlink_metadata(entry.path())?;
        ensure!(!metadata.is_symlink(), "Invalid staged restore path");
        if metadata.is_dir() {
            sync_directory_tree(&entry.path())?;
        }
    }
    sync_directory(directory)
}

fn stamp_restore(
    database: &Path,
    key: Arc<StorageKey>,
    intent: &RestoreIntent,
) -> anyhow::Result<()> {
    let opened = local_storage::open_database_with_key(database, key)?;
    opened.connection.execute_batch("CREATE TABLE IF NOT EXISTS native_restore_identity(id INTEGER PRIMARY KEY CHECK(id=1), archive TEXT NOT NULL, activation TEXT NOT NULL);")?;
    opened.connection.execute("INSERT INTO native_restore_identity(id,archive,activation) VALUES(1,?1,?2) ON CONFLICT(id) DO UPDATE SET archive=excluded.archive,activation=excluded.activation", [&intent.archive, &intent.activation])?;
    let (busy, _, _): (i64, i64, i64) =
        opened
            .connection
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?;
    ensure!(busy == 0, "Could not make the restore activation durable");
    drop(opened);
    // Windows FlushFileBuffers requires a writable handle; a read-only
    // handle fails with ERROR_ACCESS_DENIED. Opening for write never truncates.
    fs::OpenOptions::new()
        .write(true)
        .open(database)?
        .sync_all()?;
    Ok(())
}

fn verify_restore_identity(
    database: &Path,
    key: Arc<StorageKey>,
    intent: &RestoreIntent,
) -> anyhow::Result<()> {
    let mut header = [0u8; 16];
    let mut file = regular_file(database)?;
    ensure!(
        file.read(&mut header)? == header.len() && &header != b"SQLite format 3\0",
        "The restore destination contains unrelated plaintext or incomplete data; it was preserved"
    );
    let opened = local_storage::open_database_with_key(database, key)?;
    let saved: (String, String) = opened
        .connection
        .query_row(
            "SELECT archive,activation FROM native_restore_identity WHERE id=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .context("The destination is not the staged backup; existing local data was preserved")?;
    ensure!(
        saved == (intent.archive.clone(), intent.activation.clone()),
        "The destination does not match the pending restore; existing data was preserved"
    );
    Ok(())
}

fn verify_store(database: &Path, key: Arc<StorageKey>) -> anyhow::Result<()> {
    let store = SessionStore::open_protected_with_key(database, key.clone())?;
    store.verify_asset_integrity()?;
    drop(store);
    let opened = local_storage::open_database_with_key(database, key)?;
    let (busy, _, _): (i64, i64, i64) =
        opened
            .connection
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?;
    ensure!(
        busy == 0,
        "Could not finish the restored database checkpoint"
    );
    Ok(())
}

fn restore_archive(
    paths: &AxiomPaths,
    mut archive: OpenArchive,
    save_key: impl FnOnce(&Path, &StorageKey) -> crate::Result<()>,
) -> anyhow::Result<()> {
    paths.prepare()?;
    paths.prepare_account(&archive.manifest.account)?;
    let database =
        paths.account_state_database(&archive.manifest.account, archive.manifest.frontend)?;
    let directory = database.parent().context("Invalid local store directory")?;
    let account_directory = directory.parent().context("Invalid account directory")?;
    let marker = local_storage::restore_marker_for_database(&database);
    let previous_intent = match fs::symlink_metadata(&marker) {
        Ok(metadata) => {
            ensure!(
                metadata.is_file() && !metadata.is_symlink(),
                "Invalid restore recovery marker"
            );
            let saved: RestoreIntent = serde_json::from_slice(&fs::read(&marker)?)?;
            ensure!(
                saved.version == 1
                    && saved.archive == archive.identity
                    && uuid::Uuid::parse_str(&saved.activation).is_ok(),
                "Finish the pending restore using its original backup"
            );
            Some(saved)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let pending = previous_intent.is_some();
    let intent = previous_intent.unwrap_or_else(|| RestoreIntent {
        version: 1,
        archive: archive.identity.clone(),
        activation: uuid::Uuid::new_v4().to_string(),
    });
    if pending && database.try_exists()? {
        verify_restore_identity(&database, archive.key.clone(), &intent)?;
        verify_store(&database, archive.key.clone())?;
        save_key(&database, &archive.key)?;
        fs::remove_file(&marker)?;
        sync_directory(account_directory)?;
        return Ok(());
    }
    if let Ok(metadata) = fs::symlink_metadata(directory) {
        ensure!(
            metadata.is_dir() && !metadata.is_symlink(),
            "Invalid restore destination"
        );
        clear_restore_coordination(directory)?;
    }
    fs::create_dir_all(account_directory)?;
    let staging = tempfile::Builder::new()
        .prefix(".axiom-encrypted-restore-")
        .tempdir_in(account_directory)?;
    let staged_database = extract_archive(&mut archive, staging.path())?;
    verify_store(&staged_database, archive.key.clone())?;
    stamp_restore(&staged_database, archive.key.clone(), &intent)?;
    sync_directory_tree(staging.path())?;
    // Persist intent before touching the credential store. A new process must
    // not turn an interrupted restore into an unrelated empty conversation DB.
    if !pending {
        let mut pending_marker = tempfile::NamedTempFile::new_in(account_directory)?;
        pending_marker.write_all(&serde_json::to_vec(&intent)?)?;
        pending_marker.as_file().sync_all()?;
        pending_marker
            .persist_noclobber(&marker)
            .map_err(|error| error.error)?;
        sync_directory(account_directory)?;
    }
    fs::create_dir_all(directory)?;
    save_key(&database, &archive.key)?;
    // Key installation creates only this empty coordination file. The global
    // maintenance lease excludes every normal application process here.
    clear_restore_coordination(directory)?;
    fs::remove_dir(directory)?; // Only the verified empty destination is removed.
    fs::rename(staging.path(), directory)?;
    sync_directory(account_directory)?;
    // Validate again at the final path; asset authentication must be portable.
    verify_store(&database, archive.key)?;
    fs::remove_file(marker)?;
    sync_directory(account_directory)?;
    Ok(())
}

fn clear_restore_coordination(directory: &Path) -> anyhow::Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let metadata = fs::symlink_metadata(entry.path())?;
        ensure!(
            entry.file_name() == "state.sqlite3.storage-lock"
                && metadata.is_file()
                && !metadata.is_symlink()
                && metadata.len() == 0,
            "The destination already contains local data; restore never overwrites existing chats"
        );
        fs::remove_file(entry.path())?;
    }
    Ok(())
}

fn sync_directory(path: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[cfg(test)]
mod tests;
