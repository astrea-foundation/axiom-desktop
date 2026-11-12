//! `SQLCipher` connection setup and resumable plaintext migration. Only our
//! private staging files are discarded; unreadable user databases are preserved.

use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

use rusqlite::{Connection, OpenFlags, Transaction, TransactionBehavior, types::ValueRef};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use super::{StorageKey, decrypt_asset, encrypt_asset, failure};
use crate::Result;

const SQLITE_HEADER: &[u8; 16] = b"SQLite format 3\0";
const MIGRATION_ID: &str = "database-encryption-migration-v1";
// Check the linked engine, not the Rust crate version: libsqlite3-sys 0.39.1
// has a patched normal SQLite bundle but an affected SQLCipher bundle.
const MINIMUM_SQLITE_VERSION: i32 = 3_054_001;

pub(super) fn require_supported_sqlite_version(version: i32) -> Result<()> {
    if version < MINIMUM_SQLITE_VERSION {
        return Err(failure(
            "Local storage requires SQLite 3.54.1 or newer. Update Axiom before opening this database; existing data was preserved",
        ));
    }
    Ok(())
}

pub(super) struct StorePaths {
    pub database: PathBuf,
    candidate: PathBuf,
    previous: PathBuf,
    marker: PathBuf,
    marker_staging: PathBuf,
    lock: PathBuf,
}

impl StorePaths {
    pub fn prepare(path: &Path) -> Result<Self> {
        let name = path
            .file_name()
            .ok_or_else(|| failure("Local database filename required"))?;
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        if !parent.exists() {
            let mut builder = fs::DirBuilder::new();
            builder.recursive(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder.create(parent)?;
        }
        let database = parent.canonicalize()?.join(name);
        let paths = Self {
            candidate: sibling(&database, ".encrypted-migration"),
            previous: sibling(&database, ".plaintext-migration"),
            marker: sibling(&database, ".encryption-migration"),
            marker_staging: sibling(&database, ".encryption-marker-new"),
            lock: sibling(&database, ".storage-lock"),
            database,
        };
        for path in [
            &paths.database,
            &paths.candidate,
            &paths.previous,
            &paths.marker,
            &paths.marker_staging,
            &paths.lock,
        ] {
            regular_if_present(path)?;
        }
        Ok(paths)
    }

    pub fn lock(&self) -> Result<File> {
        let mut options = OpenOptions::new();
        options.create(true).truncate(false).read(true).write(true);
        private_options(&mut options);
        let file = options.open(&self.lock)?;
        fs2::FileExt::lock_exclusive(&file)
            .map_err(|_| failure("Could not lock local storage initialization"))?;
        Ok(file)
    }

    pub fn needs_existing_key(&self) -> Result<bool> {
        Ok(self.marker.exists()
            || self.marker_staging.exists()
            || self.previous.exists()
            || self.candidate.exists()
            || matches!(kind(&self.database)?, DatabaseKind::Encrypted))
    }

    pub fn needs_initial_maintenance(&self) -> Result<bool> {
        Ok(self.marker.exists()
            || self.marker_staging.exists()
            || self.previous.exists()
            || self.candidate.exists()
            || kind(&self.database)? != DatabaseKind::Encrypted)
    }

    pub fn has_migration(&self) -> Result<bool> {
        Ok(self.marker.exists()
            || self.marker_staging.exists()
            || self.previous.exists()
            || self.candidate.exists()
            || kind(&self.database)? == DatabaseKind::Plaintext)
    }
}

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = OsString::from(path.file_name().unwrap_or_default());
    name.push(suffix);
    path.with_file_name(name)
}

pub(super) fn regular_if_present(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(failure("Local storage requires regular files, not links"));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if metadata.nlink() != 1 {
                    return Err(failure("Local storage does not support hard-linked files"));
                }
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn private_options(options: &mut OpenOptions) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(nix::libc::O_NOFOLLOW);
    }
    #[cfg(not(unix))]
    let _ = options;
}

fn create_private(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    private_options(&mut options);
    Ok(options.open(path)?)
}

fn private_permissions(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn sync_parent(path: &Path) -> Result<()> {
    // Windows rename durability is followed by syncing the resulting file;
    // opening a directory as File is not supported by std on Windows.
    #[cfg(unix)]
    File::open(
        path.parent()
            .ok_or_else(|| failure("Database directory missing"))?,
    )?
    .sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn sync_file(path: &Path) -> Result<()> {
    regular_if_present(path)?;
    let mut options = OpenOptions::new();
    // FlushFileBuffers requires a writable handle on Windows.
    options.read(true).write(true);
    private_options(&mut options);
    options.open(path)?.sync_all()?;
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum DatabaseKind {
    Missing,
    Empty,
    Plaintext,
    Encrypted,
}

pub(super) fn kind(path: &Path) -> Result<DatabaseKind> {
    regular_if_present(path)?;
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(DatabaseKind::Missing);
        }
        Err(error) => return Err(error.into()),
    };
    if file.metadata()?.len() == 0 {
        return Ok(DatabaseKind::Empty);
    }
    let mut header = [0; 16];
    let count = file.read(&mut header)?;
    Ok(if count == 16 && header == *SQLITE_HEADER {
        DatabaseKind::Plaintext
    } else {
        DatabaseKind::Encrypted
    })
}

fn open_connection(path: &Path, key: Option<&StorageKey>) -> Result<Connection> {
    // Runs before SQLite receives any database/WAL path. Validation of file
    // contents cannot compensate for a vulnerable linked SQLite engine.
    require_supported_sqlite_version(rusqlite::version_number())?;
    regular_if_present(path)?;
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(|_| failure("Could not open local database; existing data was preserved"))?;
    connection
        .busy_timeout(Duration::from_secs(5))
        .map_err(database_error)?;
    // Unknown PRAGMAs are silently accepted by ordinary SQLite. Require a
    // positive SQLCipher identity before doing any database reads or writes.
    let version: String = connection
        .query_row("PRAGMA cipher_version", [], |row| row.get(0))
        .map_err(|_| failure("This application is missing encrypted storage support"))?;
    if version.is_empty() {
        return Err(failure(
            "This application is missing encrypted storage support",
        ));
    }
    if let Some(key) = key {
        let encoded = key.database_key()?;
        connection
            .pragma_update(None, "key", encoded.as_str())
            .map_err(database_error)?;
    }
    connection
        .execute_batch("PRAGMA temp_store=MEMORY; PRAGMA secure_delete=ON;")
        .map_err(database_error)?;
    let _: i64 = connection
        .query_row("SELECT count(*) FROM sqlite_schema", [], |row| row.get(0))
        .map_err(|_| {
            failure(
                "Local database could not be unlocked or is damaged; existing data was preserved",
            )
        })?;
    Ok(connection)
}

pub(super) fn validate_key(path: &Path, key: &StorageKey) -> Result<()> {
    if matches!(kind(path)?, DatabaseKind::Encrypted) {
        let connection = open_connection(path, Some(key))?;
        integrity(&connection)?;
    } else if !matches!(kind(path)?, DatabaseKind::Missing | DatabaseKind::Empty) {
        return Err(failure("Recovery requires an encrypted database"));
    }
    Ok(())
}

pub(super) fn open_locked(paths: &StorePaths, key: &StorageKey) -> Result<Connection> {
    recover(paths, key)?;
    match kind(&paths.database)? {
        DatabaseKind::Plaintext => migrate(paths, key)?,
        DatabaseKind::Missing => {
            create_private(&paths.database)?.sync_all()?;
            sync_parent(&paths.database)?;
        }
        DatabaseKind::Empty | DatabaseKind::Encrypted => {}
    }
    let connection = open_connection(&paths.database, Some(key))?;
    // Materialize the encryption header before the opener returns even for a
    // brand-new database; a lost key can never be mistaken for a fresh store.
    if fs::metadata(&paths.database)?.len() == 0 {
        connection
            .execute_batch("PRAGMA user_version=0;")
            .map_err(database_error)?;
    }
    private_permissions(&paths.database)?;
    Ok(connection)
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Snapshot {
    application_id: i64,
    user_version: i64,
    digest: String,
}

fn integrity(connection: &Connection) -> Result<()> {
    let mut check = connection
        .prepare("PRAGMA integrity_check")
        .map_err(database_error)?;
    let mut rows = check.query([]).map_err(database_error)?;
    let row = rows
        .next()
        .map_err(database_error)?
        .ok_or_else(|| failure("Database integrity check was incomplete"))?;
    if row.get::<_, String>(0).map_err(database_error)? != "ok"
        || rows.next().map_err(database_error)?.is_some()
    {
        return Err(failure(
            "Database integrity check failed; existing data was preserved",
        ));
    }
    let foreign: bool = connection
        .prepare("PRAGMA foreign_key_check")
        .map_err(database_error)?
        .exists([])
        .map_err(database_error)?;
    if foreign {
        return Err(failure(
            "Database reference integrity check failed; existing data was preserved",
        ));
    }
    Ok(())
}

fn hash_value(hash: &mut Sha256, value: ValueRef<'_>) {
    match value {
        ValueRef::Null => hash.update([0]),
        ValueRef::Integer(value) => {
            hash.update([1]);
            hash.update(value.to_be_bytes());
        }
        ValueRef::Real(value) => {
            hash.update([2]);
            hash.update(value.to_bits().to_be_bytes());
        }
        ValueRef::Text(value) => {
            hash.update([3]);
            hash.update((value.len() as u64).to_be_bytes());
            hash.update(value);
        }
        ValueRef::Blob(value) => {
            hash.update([4]);
            hash.update((value.len() as u64).to_be_bytes());
            hash.update(value);
        }
    }
}

fn snapshot(connection: &Connection) -> Result<Snapshot> {
    let application_id = connection
        .query_row("PRAGMA application_id", [], |row| row.get(0))
        .map_err(database_error)?;
    let user_version = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(database_error)?;
    let mut hash = Sha256::new();
    let mut schema = connection.prepare("SELECT type,name,tbl_name,sql FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%' ORDER BY type,name").map_err(database_error)?;
    let mut rows = schema.query([]).map_err(database_error)?;
    let mut tables = Vec::new();
    while let Some(row) = rows.next().map_err(database_error)? {
        for index in 0..4 {
            hash_value(&mut hash, row.get_ref(index).map_err(database_error)?);
        }
        if row.get::<_, String>(0).map_err(database_error)? == "table" {
            tables.push(row.get::<_, String>(1).map_err(database_error)?);
        }
    }
    if connection
        .query_row(
            "SELECT count(*) FROM sqlite_schema WHERE name='sqlite_sequence'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .map_err(database_error)?
        != 0
        && connection
            .query_row("SELECT count(*) FROM sqlite_sequence", [], |r| {
                r.get::<_, i64>(0)
            })
            .map_err(database_error)?
            != 0
    {
        // SQLite retains this empty internal table after the last AUTOINCREMENT
        // table is dropped, whereas export omits it. All actual sequence rows
        // remain part of the content digest.
        tables.push("sqlite_sequence".into());
    }
    for table in tables {
        let sql = format!("SELECT * FROM \"{}\"", table.replace('"', "\"\""));
        let mut statement = connection.prepare(&sql).map_err(database_error)?;
        let columns = statement.column_count();
        let mut rows = statement.query([]).map_err(database_error)?;
        let mut digests = Vec::new();
        while let Some(row) = rows.next().map_err(database_error)? {
            let mut row_hash = Sha256::new();
            row_hash.update((columns as u64).to_be_bytes());
            for index in 0..columns {
                let value = row.get_ref(index).map_err(database_error)?;
                row_hash.update([u8::from(matches!(value, ValueRef::Blob(_)))]);
                hash_value(&mut row_hash, value);
            }
            digests.push(<[u8; 32]>::from(row_hash.finalize()));
        }
        digests.sort_unstable();
        hash.update((table.len() as u64).to_be_bytes());
        hash.update(table.as_bytes());
        hash.update((digests.len() as u64).to_be_bytes());
        for digest in digests {
            hash.update(digest);
        }
    }
    Ok(Snapshot {
        application_id,
        user_version,
        digest: hex::encode(hash.finalize()),
    })
}

/// Export under a consistent `SQLite` transaction into a new, encrypted file.
/// Refuses overwrite; does not use rusqlite's unkeyed convenience backup API.
pub(crate) fn backup_database(
    source: &Connection,
    key: &StorageKey,
    destination: &Path,
) -> Result<()> {
    export(source, key, destination).map(|_| ())
}

fn export(source: &Connection, key: &StorageKey, destination: &Path) -> Result<Snapshot> {
    if !source.is_autocommit() {
        return Err(failure("Storage export requires an idle connection"));
    }
    let name = destination
        .file_name()
        .ok_or_else(|| failure("Storage export filename required"))?;
    let parent = destination
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let canonical_destination = parent.canonicalize()?.join(name);
    let destination = canonical_destination.as_path();
    create_private(destination)?.sync_all()?;
    let key_literal = key.database_key()?;
    let path = destination
        .to_str()
        .ok_or_else(|| failure("Storage export path must be Unicode"))?;
    source
        .execute(
            "ATTACH DATABASE ?1 AS axiom_export KEY ?2",
            rusqlite::params![path, key_literal.as_str()],
        )
        .map_err(database_error)?;
    let result: Result<Snapshot> = (|| {
        let transaction = Transaction::new_unchecked(source, TransactionBehavior::Exclusive)
            .map_err(database_error)?;
        integrity(source)?;
        let expected = snapshot(source)?;
        source
            .query_row("SELECT sqlcipher_export('axiom_export')", [], |_| Ok(()))
            .map_err(database_error)?;
        source
            .execute_batch(&format!(
                "PRAGMA axiom_export.application_id={}; PRAGMA axiom_export.user_version={};",
                expected.application_id, expected.user_version
            ))
            .map_err(database_error)?;
        transaction.commit().map_err(database_error)?;
        Ok(expected)
    })();
    let detached = source
        .execute_batch("DETACH DATABASE axiom_export")
        .map_err(database_error);
    let expected = result?;
    detached?;
    let target = open_connection(destination, Some(key))?;
    integrity(&target)?;
    if snapshot(&target)? != expected {
        return Err(failure(
            "Encrypted export validation failed; existing data was preserved",
        ));
    }
    target.close().map_err(|(_, e)| database_error(e))?;
    sync_file(destination)?;
    sync_parent(destination)?;
    Ok(expected)
}

fn migrate(paths: &StorePaths, key: &StorageKey) -> Result<()> {
    let source = open_connection(&paths.database, None)?;
    integrity(&source)?;
    // DELETE mode requires other WAL readers/writers to leave. Exclusive mode
    // then retains the source lock through the copy and verification.
    let mode: String = source
        .query_row("PRAGMA journal_mode=DELETE", [], |r| r.get(0))
        .map_err(database_error)?;
    if !mode.eq_ignore_ascii_case("delete") {
        return Err(failure(
            "Close other Axiom processes before migrating local storage",
        ));
    }
    source
        .execute_batch("PRAGMA locking_mode=EXCLUSIVE;")
        .map_err(database_error)?;
    if paths.candidate.exists() {
        remove_stage(&paths.candidate)?;
    }
    let expected = export(&source, key, &paths.candidate)?;
    write_marker(paths, key, &expected)?;
    source.close().map_err(|(_, e)| database_error(e))?;
    finish_replacement(paths, key, &expected)
}

fn write_marker(paths: &StorePaths, key: &StorageKey, expected: &Snapshot) -> Result<()> {
    // The final marker is the commit record: publish only a fully written and
    // synced envelope. A crash before this rename leaves the original database
    // authoritative and the incomplete marker can be discarded on retry.
    remove_stage(&paths.marker_staging)?;
    let marker = encrypt_asset(key, MIGRATION_ID, &serde_json::to_vec(expected)?)?;
    let mut file = create_private(&paths.marker_staging)?;
    file.write_all(&marker)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&paths.marker_staging, &paths.marker)?;
    sync_parent(&paths.marker)
}

fn read_marker(paths: &StorePaths, key: &StorageKey) -> Result<Snapshot> {
    if fs::metadata(&paths.marker)?.len() > 4096 {
        return Err(failure("Invalid local storage migration record"));
    }
    let raw = Zeroizing::new(decrypt_asset(key, MIGRATION_ID, &fs::read(&paths.marker)?)?);
    serde_json::from_slice(&raw).map_err(|_| failure("Invalid local storage migration record"))
}

fn verify_snapshot(path: &Path, key: Option<&StorageKey>, expected: &Snapshot) -> Result<()> {
    let connection = open_connection(path, key)?;
    integrity(&connection)?;
    if snapshot(&connection)? != *expected {
        return Err(failure(
            "Migration files changed; existing data was preserved",
        ));
    }
    connection.close().map_err(|(_, e)| database_error(e))?;
    Ok(())
}

fn recover(paths: &StorePaths, key: &StorageKey) -> Result<()> {
    if paths.marker.exists() {
        let expected = read_marker(paths, key)?;
        finish_replacement(paths, key, &expected)
    } else if paths.previous.exists() {
        Err(failure(
            "Unrecognized storage migration files; existing data was preserved",
        ))
    } else if (paths.candidate.exists() || paths.marker_staging.exists())
        && kind(&paths.database)? != DatabaseKind::Plaintext
    {
        Err(failure(
            "Incomplete storage export requires recovery; existing data was preserved",
        ))
    } else {
        Ok(())
    }
}

fn finish_replacement(paths: &StorePaths, key: &StorageKey, expected: &Snapshot) -> Result<()> {
    match kind(&paths.database)? {
        DatabaseKind::Plaintext => {
            if paths.previous.exists() {
                return Err(failure(
                    "Ambiguous local storage migration; existing data was preserved",
                ));
            }
            verify_snapshot(&paths.database, None, expected)?;
            verify_snapshot(&paths.candidate, Some(key), expected)?;
            fs::rename(&paths.database, &paths.previous)?;
            sync_parent(&paths.database)?;
        }
        DatabaseKind::Missing => {
            if !paths.previous.exists() {
                return Err(failure("Migration source is missing; restore a backup"));
            }
            verify_snapshot(&paths.previous, None, expected)?;
            verify_snapshot(&paths.candidate, Some(key), expected)?;
        }
        DatabaseKind::Encrypted => {
            verify_snapshot(&paths.database, Some(key), expected)?;
            // A crash after installation can leave the verified plaintext copy.
            cleanup(paths)?;
            return Ok(());
        }
        DatabaseKind::Empty => {
            return Err(failure(
                "Migration source is empty; existing data was preserved",
            ));
        }
    }
    fs::rename(&paths.candidate, &paths.database)?;
    sync_file(&paths.database)?;
    sync_parent(&paths.database)?;
    verify_snapshot(&paths.database, Some(key), expected)?;
    cleanup(paths)
}

fn remove_stage(path: &Path) -> Result<()> {
    for file in [
        path.to_owned(),
        sibling(path, "-wal"),
        sibling(path, "-shm"),
        sibling(path, "-journal"),
    ] {
        regular_if_present(&file)?;
        match fs::remove_file(file) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

fn cleanup(paths: &StorePaths) -> Result<()> {
    remove_stage(&paths.previous)?;
    remove_stage(&paths.candidate)?;
    remove_stage(&paths.marker_staging)?;
    fs::remove_file(&paths.marker)?;
    sync_parent(&paths.database)
}

#[allow(clippy::needless_pass_by_value)] // Used directly as a Result::map_err converter.
fn database_error(_: rusqlite::Error) -> crate::AxiomError {
    // Never include SQL text, payloads or key literals in a storage error.
    failure("Local database operation failed; existing data was preserved")
}

#[cfg(test)]
pub(super) mod test_support {
    use super::*;
    pub fn stage(path: &Path, key: &StorageKey, move_old: bool, install: bool) -> Result<()> {
        let paths = StorePaths::prepare(path)?;
        let source = open_connection(&paths.database, None)?;
        let expected = export(&source, key, &paths.candidate)?;
        source.close().map_err(|(_, e)| database_error(e))?;
        write_marker(&paths, key, &expected)?;
        if move_old {
            fs::rename(&paths.database, &paths.previous)?;
        }
        if install {
            fs::rename(&paths.candidate, &paths.database)?;
        }
        Ok(())
    }
}
