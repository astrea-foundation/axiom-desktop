//! OS-keystore-only production keys. The database initialization lock serializes
//! first creation across processes; `NoEntry` is not permission to replace a key
//! belonging to an encrypted database or interrupted migration.

use sha2::{Digest, Sha256};
use std::{path::Path, sync::Arc};
use zeroize::Zeroizing;

use super::{
    StorageKey,
    database::{self, StorePaths},
    failure,
};
use crate::Result;

pub(crate) const KEYRING_SERVICE: &str = "stream.axiom.local-storage";

pub(crate) fn scope_prefix(root: &Path) -> String {
    format!(
        "{}:",
        hex::encode(Sha256::digest(root.as_os_str().as_encoded_bytes()))
    )
}

pub(crate) fn credential_name(path: &Path) -> String {
    let parent = path.parent().unwrap_or(path);
    let root = parent
        .ancestors()
        .find(|p| p.file_name().is_some_and(|n| n == "accounts"))
        .and_then(Path::parent)
        .unwrap_or(parent);
    format!(
        "{}{}",
        scope_prefix(root),
        hex::encode(Sha256::digest(path.as_os_str().as_encoded_bytes()))
    )
}

trait KeyStore {
    fn read(&self, name: &str) -> Result<Option<StorageKey>>;
    fn write(&self, name: &str, key: &StorageKey) -> Result<()>;
}

struct SystemKeyStore;

impl KeyStore for SystemKeyStore {
    fn read(&self, name: &str) -> Result<Option<StorageKey>> {
        let entry = keyring::Entry::new(KEYRING_SERVICE, name).map_err(|_| {
            failure("System credential store is unavailable; unlock it and try again")
        })?;
        let bytes = match entry.get_secret() {
            Ok(value) => Zeroizing::new(value),
            Err(keyring::Error::NoEntry) => return Ok(None),
            Err(_) => {
                return Err(failure(
                    "Local storage key is locked or unavailable; existing data was preserved",
                ));
            }
        };
        if bytes.len() != 32 {
            return Err(failure(
                "Stored local storage key has an invalid format; restore a backup",
            ));
        }
        let mut secret = Zeroizing::new([0; 32]);
        secret.copy_from_slice(&bytes);
        Ok(Some(StorageKey { secret }))
    }

    fn write(&self, name: &str, key: &StorageKey) -> Result<()> {
        keyring::Entry::new(KEYRING_SERVICE, name)
            .map_err(|_| {
                failure("System credential store is unavailable; unlock it and try again")
            })?
            .set_secret(key.secret.as_ref())
            .map_err(|_| {
                failure("Could not save the local storage key; existing data was preserved")
            })
    }
}

pub(super) fn obtain(paths: &StorePaths) -> Result<StorageKey> {
    obtain_with(paths, &SystemKeyStore)
}

fn obtain_with(paths: &StorePaths, store: &impl KeyStore) -> Result<StorageKey> {
    let name = credential_name(&paths.database);
    if let Some(key) = store.read(&name)? {
        return Ok(key);
    }
    if paths.needs_existing_key()? {
        return Err(failure(
            "The local database encryption key is missing. Restore an encrypted backup with its recovery key; existing data was preserved",
        ));
    }
    let key = StorageKey::generate()?;
    store.write(&name, &key)?;
    let verified = store
        .read(&name)?
        .ok_or_else(|| failure("The credential store did not retain the local storage key"))?;
    if !key.matches(&verified) {
        return Err(failure("The local storage key changed during creation"));
    }
    Ok(verified)
}

pub(crate) fn load_key_for_path(path: &Path) -> Result<Arc<StorageKey>> {
    let paths = StorePaths::prepare(path)?;
    let _lock = paths.lock()?;
    SystemKeyStore
        .read(&credential_name(&paths.database))?
        .map(Arc::new)
        .ok_or_else(|| failure("The local storage key is missing; restore a recovery backup"))
}

/// Explicit recovery installation only. Never silently rotates or replaces a
/// different credential, and validates an already-installed database first.
pub(crate) fn save_key_for_path(path: &Path, key: &StorageKey) -> Result<()> {
    let paths = StorePaths::prepare(path)?;
    let _lock = paths.lock()?;
    save_with(&paths, key, &SystemKeyStore)
}

fn save_with(paths: &StorePaths, key: &StorageKey, store: &impl KeyStore) -> Result<()> {
    let name = credential_name(&paths.database);
    if let Some(existing) = store.read(&name)? {
        if !existing.matches(key) {
            return Err(failure(
                "A different storage key already exists at the restore destination",
            ));
        }
        database::validate_key(&paths.database, key)?;
        return Ok(());
    }
    database::validate_key(&paths.database, key)?;
    store.write(&name, key)?;
    let saved = store
        .read(&name)?
        .ok_or_else(|| failure("The credential store did not retain the recovery key"))?;
    if !saved.matches(key) {
        return Err(failure("The recovery key could not be verified"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::HashMap,
        sync::{
            Mutex,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        },
    };

    #[derive(Default)]
    struct MemoryStore {
        values: Mutex<HashMap<String, [u8; 32]>>,
        locked: AtomicBool,
        writes: AtomicUsize,
    }
    impl KeyStore for MemoryStore {
        fn read(&self, name: &str) -> Result<Option<StorageKey>> {
            if self.locked.load(Ordering::SeqCst) {
                return Err(failure("Synthetic key store locked"));
            }
            Ok(self
                .values
                .lock()
                .unwrap()
                .get(name)
                .map(|k| StorageKey::from_bytes(*k)))
        }
        fn write(&self, name: &str, key: &StorageKey) -> Result<()> {
            if self.locked.load(Ordering::SeqCst) {
                return Err(failure("Synthetic key store locked"));
            }
            self.writes.fetch_add(1, Ordering::SeqCst);
            self.values.lock().unwrap().insert(name.into(), *key.secret);
            Ok(())
        }
    }

    #[test]
    fn concurrent_creation_keeps_one_key_and_locked_missing_stores_fail_closed() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.sqlite3");
        let store = Arc::new(MemoryStore::default());
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let store = store.clone();
                let path = path.clone();
                std::thread::spawn(move || {
                    let paths = StorePaths::prepare(&path).unwrap();
                    let _lock = paths.lock().unwrap();
                    let key = obtain_with(&paths, &*store).unwrap();
                    let connection = database::open_locked(&paths, &key, &|| {}).unwrap();
                    connection.close().unwrap();
                    key
                })
            })
            .collect();
        let keys: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
        assert!(keys.iter().all(|k| k.matches(&keys[0])));
        assert_eq!(store.writes.load(Ordering::SeqCst), 1);
        let original = std::fs::read(&path).unwrap();
        let paths = StorePaths::prepare(&path).unwrap();
        store.locked.store(true, Ordering::SeqCst);
        assert!(obtain_with(&paths, &*store).is_err());
        store.locked.store(false, Ordering::SeqCst);
        store.values.lock().unwrap().clear();
        assert!(obtain_with(&paths, &*store).is_err());
        assert_eq!(std::fs::read(path).unwrap(), original);
        assert_eq!(store.writes.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn recovery_refuses_existing_key_replacement_and_checks_database() {
        let directory = tempfile::tempdir().unwrap();
        let paths = StorePaths::prepare(&directory.path().join("state.sqlite3")).unwrap();
        let key = StorageKey::from_bytes([7; 32]);
        let wrong = StorageKey::from_bytes([8; 32]);
        database::open_locked(&paths, &key, &|| {})
            .unwrap()
            .close()
            .unwrap();
        let store = MemoryStore::default();
        assert!(save_with(&paths, &wrong, &store).is_err());
        assert_eq!(store.writes.load(Ordering::SeqCst), 0);
        save_with(&paths, &key, &store).unwrap();
        assert!(save_with(&paths, &wrong, &store).is_err());
        assert_eq!(store.writes.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn missing_key_never_regenerates_over_an_interrupted_plaintext_migration() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.sqlite3");
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch("CREATE TABLE saved(value TEXT); INSERT INTO saved VALUES('synthetic');")
            .unwrap();
        connection.close().unwrap();
        database::test_support::stage(&path, &StorageKey::from_bytes([3; 32]), false, false)
            .unwrap();
        let before = std::fs::read(&path).unwrap();
        let store = MemoryStore::default();
        let paths = StorePaths::prepare(&path).unwrap();
        assert!(obtain_with(&paths, &store).is_err());
        assert_eq!(store.writes.load(Ordering::SeqCst), 0);
        assert_eq!(before, std::fs::read(path).unwrap());
    }

    #[test]
    fn credential_scopes_separate_profiles_accounts_and_frontends() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let profile = root.join("data/axiom");
        let other_profile = root.join("other/data/axiom");
        let desktop = credential_name(&profile.join("accounts/one/desktop/state.sqlite3"));
        let cli = credential_name(&profile.join("accounts/one/cli/state.sqlite3"));
        let account = credential_name(&profile.join("accounts/two/desktop/state.sqlite3"));
        let other = credential_name(&other_profile.join("accounts/one/desktop/state.sqlite3"));
        assert!(desktop.starts_with(&scope_prefix(&profile)));
        assert!(cli.starts_with(&scope_prefix(&profile)));
        assert!(account.starts_with(&scope_prefix(&profile)));
        assert!(!other.starts_with(&scope_prefix(&profile)));
        assert_ne!(desktop, cli);
        assert_ne!(desktop, account);
    }
}
