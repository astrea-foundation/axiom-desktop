//! Binary assets live outside `SQLite`. Only versioned references and ownership
//! records are persisted in SQL; files contain authenticated ciphertext only.
use super::codec::storage_error;
use crate::{
    AxiomError, Result,
    local_storage::{StorageKey, decrypt_asset, encrypt_asset},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use rusqlite::{Connection, OptionalExtension as _, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

const MAX_ASSET_BYTES: usize = 128 * 1024 * 1024;
#[derive(Clone)]
pub(super) struct AssetManager {
    backing: Backing,
}
#[derive(Clone)]
enum Backing {
    Disk { root: PathBuf, key: Arc<StorageKey> },
    Memory(Arc<Mutex<HashMap<String, Vec<u8>>>>),
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AssetRef {
    version: u8,
    id: String,
    sha256: String,
    bytes: u64,
}
pub(super) fn thread_owner(id: &str) -> String {
    format!("thread:{id}")
}
pub(super) fn payload_owner(id: &str) -> String {
    format!("payload:{id}")
}
fn invalid(message: &str) -> AxiomError {
    AxiomError::Storage(message.into())
}

impl AssetManager {
    pub(super) fn memory() -> Self {
        Self {
            backing: Backing::Memory(Arc::new(Mutex::new(HashMap::new()))),
        }
    }
    pub(super) fn disk(database: &Path, key: Arc<StorageKey>) -> Result<Self> {
        // Canonical database identity also makes symlink aliases share assets.
        let root = super::asset_directory_for_database(database);
        fs::create_dir_all(&root)?;
        if fs::symlink_metadata(&root)?.file_type().is_symlink() {
            return Err(invalid("asset directory cannot be a symlink"));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
        }
        Ok(Self {
            backing: Backing::Disk { root, key },
        })
    }
    pub(super) fn intern(
        &self,
        connection: &Connection,
        owner: &str,
        data: &str,
    ) -> Result<AssetRef> {
        if data.len() > MAX_ASSET_BYTES.div_ceil(3) * 4 {
            return Err(invalid("asset exceeds storage limit"));
        }
        let bytes = STANDARD
            .decode(data)
            .map_err(|_| invalid("asset base64 is invalid"))?;
        if STANDARD.encode(&bytes) != data || bytes.len() > MAX_ASSET_BYTES {
            return Err(invalid("asset encoding is not canonical"));
        }
        let digest = hex::encode(Sha256::digest(&bytes));
        let existing: Option<(String, u64)> = connection
            .query_row(
                "SELECT id,byte_length FROM binary_assets WHERE sha256=?1",
                [&digest],
                |r| Ok((r.get(0)?, super::codec::checked_u64_sql(r.get(1)?)?)),
            )
            .optional()
            .map_err(storage_error)?;
        let reference = if let Some((id, length)) = existing {
            let reference = AssetRef {
                version: 1,
                id,
                sha256: digest,
                bytes: length,
            };
            if self.read_bytes(&reference)? != bytes {
                return Err(invalid("saved asset content differs"));
            }
            reference
        } else {
            let reference = AssetRef {
                version: 1,
                id: uuid::Uuid::new_v4().to_string(),
                sha256: digest,
                bytes: bytes.len() as u64,
            };
            // File durability precedes SQL commit. A failed transaction can leave
            // only unreferenced ciphertext, reclaimed under the next write lock.
            self.write_bytes(&reference, &bytes)?;
            connection
                .execute(
                    "INSERT INTO binary_assets(id,sha256,byte_length) VALUES (?1,?2,?3)",
                    params![
                        reference.id,
                        reference.sha256,
                        i64::try_from(reference.bytes)
                            .map_err(|_| invalid("asset size exceeds SQLite range"))?
                    ],
                )
                .map_err(storage_error)?;
            reference
        };
        connection
            .execute(
                "INSERT OR IGNORE INTO asset_owners(owner,asset_id) VALUES (?1,?2)",
                params![owner, reference.id],
            )
            .map_err(storage_error)?;
        Ok(reference)
    }
    pub(super) fn resolve(
        &self,
        connection: &Connection,
        owner: &str,
        reference: &AssetRef,
    ) -> Result<String> {
        // Authorize before touching the filesystem, including guessed asset IDs.
        let saved: Option<(String,u64)> = connection.query_row("SELECT a.sha256,a.byte_length FROM binary_assets a JOIN asset_owners o ON o.asset_id=a.id WHERE o.owner=?1 AND a.id=?2", params![owner,reference.id], |r| Ok((r.get(0)?,super::codec::checked_u64_sql(r.get(1)?)?))).optional().map_err(storage_error)?;
        if saved != Some((reference.sha256.clone(), reference.bytes)) {
            return Err(invalid("asset does not belong to this saved record"));
        }
        Ok(STANDARD.encode(self.read_bytes(reference)?))
    }
    fn read_bytes(&self, reference: &AssetRef) -> Result<Vec<u8>> {
        if reference.version != 1
            || reference.bytes > MAX_ASSET_BYTES as u64
            || uuid::Uuid::parse_str(&reference.id).is_err()
            || reference.id.len() != 36
        {
            return Err(invalid("invalid stored asset reference"));
        }
        let bytes = match &self.backing {
            Backing::Memory(values) => values
                .lock()
                .map_err(|_| invalid("asset memory lock failed"))?
                .get(&reference.id)
                .cloned()
                .ok_or_else(|| invalid("saved asset is missing"))?,
            Backing::Disk { root, key } => {
                let path = root.join(&reference.id);
                let metadata =
                    fs::symlink_metadata(&path).map_err(|_| invalid("saved asset is missing"))?;
                if !metadata.is_file()
                    || metadata.file_type().is_symlink()
                    || metadata.len() > reference.bytes + 1024
                {
                    return Err(invalid("saved asset file is invalid"));
                }
                decrypt_asset(key, &reference.id, &fs::read(path)?)?
            }
        };
        if bytes.len() as u64 != reference.bytes
            || hex::encode(Sha256::digest(&bytes)) != reference.sha256
        {
            return Err(invalid("saved asset integrity check failed"));
        }
        Ok(bytes)
    }
    fn write_bytes(&self, reference: &AssetRef, bytes: &[u8]) -> Result<()> {
        match &self.backing {
            Backing::Memory(values) => {
                values
                    .lock()
                    .map_err(|_| invalid("asset memory lock failed"))?
                    .insert(reference.id.clone(), bytes.to_vec());
            }
            Backing::Disk { root, key } => {
                let ciphertext = encrypt_asset(key, &reference.id, bytes)?;
                let temporary = root.join(format!("{}.pending", reference.id));
                let mut options = OpenOptions::new();
                options.write(true).create_new(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt as _;
                    options.mode(0o600);
                }
                let mut file = options.open(&temporary)?;
                file.write_all(&ciphertext)?;
                file.sync_all()?;
                drop(file);
                fs::rename(&temporary, root.join(&reference.id))?;
                #[cfg(unix)]
                fs::File::open(root)?.sync_all()?;
            }
        }
        Ok(())
    }
    pub(super) fn verify(&self, connection: &Connection) -> Result<()> {
        self.verify_with_progress(connection, &|_| {})
    }

    fn verify_with_progress(
        &self,
        connection: &Connection,
        progress: &crate::storage_progress::Observer<'_>,
    ) -> Result<()> {
        use crate::storage_progress::{self, Phase};
        let total: u64 = connection
            .query_row("SELECT COUNT(*) FROM binary_assets", [], |row| {
                super::codec::checked_u64_sql(row.get(0)?)
            })
            .map_err(storage_error)?;
        let mut completed = 0;
        storage_progress::count(progress, Phase::Checking, completed, total);
        let mut statement = connection
            .prepare("SELECT id,sha256,byte_length FROM binary_assets")
            .map_err(storage_error)?;
        let mut rows = statement.query([]).map_err(storage_error)?;
        while let Some(row) = rows.next().map_err(storage_error)? {
            let reference = AssetRef {
                version: 1,
                id: row.get(0).map_err(storage_error)?,
                sha256: row.get(1).map_err(storage_error)?,
                bytes: super::codec::checked_u64_sql(row.get(2).map_err(storage_error)?)
                    .map_err(storage_error)?,
            };
            self.read_bytes(&reference)?;
            completed += 1;
            storage_progress::count(progress, Phase::Checking, completed, total);
        }
        Ok(())
    }
    /// Called only while holding `SQLite`'s IMMEDIATE write transaction. No other
    /// writer can have an uncommitted asset file when orphan cleanup runs.
    pub(super) fn verify_and_clean_with_progress(
        &self,
        connection: &Connection,
        progress: &crate::storage_progress::Observer<'_>,
    ) -> Result<()> {
        self.verify_with_progress(connection, progress)?;
        self.clean_orphans(connection)
    }
    fn clean_orphans(&self, connection: &Connection) -> Result<()> {
        let mut statement = connection
            .prepare("SELECT id FROM binary_assets")
            .map_err(storage_error)?;
        let retained: HashSet<String> = statement
            .query_map([], |r| r.get(0))
            .map_err(storage_error)?
            .collect::<std::result::Result<_, _>>()
            .map_err(storage_error)?;
        if let Backing::Memory(values) = &self.backing {
            values
                .lock()
                .map_err(|_| invalid("asset memory lock failed"))?
                .retain(|id, _| retained.contains(id));
        }
        if let Backing::Disk { root, .. } = &self.backing {
            for entry in fs::read_dir(root)? {
                let entry = entry?;
                let name = entry.file_name();
                let name = name.to_string_lossy();
                let id = name.strip_suffix(".pending").unwrap_or(&name);
                if uuid::Uuid::parse_str(id).is_ok() && !retained.contains(name.as_ref()) {
                    if entry.file_type()?.is_dir() {
                        return Err(invalid("unexpected asset directory"));
                    }
                    fs::remove_file(entry.path())?;
                }
            }
        }
        Ok(())
    }
    /// Reclaim only after the transaction that removed ownership committed.
    /// The second write lock excludes concurrent file producers while unlinking.
    pub(super) fn collect_garbage(&self, connection: &mut Connection) -> Result<()> {
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let removed = transaction.execute("DELETE FROM binary_assets WHERE NOT EXISTS (SELECT 1 FROM asset_owners WHERE asset_id=binary_assets.id)", []).map_err(storage_error)?;
        transaction.commit().map_err(storage_error)?;
        if removed == 0 {
            return Ok(());
        }
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        self.clean_orphans(&transaction)?;
        transaction.commit().map_err(storage_error)
    }
    pub(super) fn collect_after_commit(&self, connection: &mut Connection) {
        if self.collect_garbage(connection).is_err() {
            // The user write already committed. Retrying cleanup on next open
            // is safe; reporting the write as failed could resend an outbox.
            tracing::warn!("Encrypted asset cleanup deferred until next store open");
        }
    }
}
