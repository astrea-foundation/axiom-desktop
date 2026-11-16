//! One protected macOS Keychain item per native data profile. Independent
//! login credentials and database keys stay inside Keychain, never in files.
//! Only a coordination lock and a nonsecret migration marker live on disk.

use std::{
    collections::{BTreeMap, HashMap},
    fs::{self, File, OpenOptions},
    os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use zeroize::{Zeroize as _, Zeroizing};

mod macos;
#[cfg(test)]
mod tests;

pub(crate) const KEYRING_SERVICE: &str = "stream.axiom.native-secrets";
const MAX_BYTES: usize = 1024 * 1024;
const MAX_SECRET_BYTES: usize = 16 * 1024;
const MAX_ENTRIES: usize = 4096;
type Result<T> = std::result::Result<T, String>;

trait Backend {
    /// Metadata only: this must not decrypt the protected item.
    fn revision(&self) -> Result<Option<String>>;
    fn read(&self) -> Result<Zeroizing<Vec<u8>>>;
    /// Data and revision are changed atomically, preserving existing ACLs.
    fn write(&self, revision: &str, bytes: &[u8], exists: bool) -> Result<()>;
    fn legacy_contains(&self, service: &str, account: &str) -> Result<bool>;
    fn legacy_read(&self, service: &str, account: &str) -> Result<Option<Zeroizing<Vec<u8>>>>;
    fn legacy_delete(&self, service: &str, account: &str) -> Result<()>;
}

// No Debug/Display implementation: these values must never enter diagnostics.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Secrets {
    version: u8,
    scope: String,
    revision: String,
    legacy_login: bool,
    // None is a deletion tombstone, so logout cannot resurrect an old item.
    entries: BTreeMap<String, Option<String>>,
}

impl Drop for Secrets {
    fn drop(&mut self) {
        for value in self.entries.values_mut().flatten() {
            value.zeroize();
        }
    }
}

struct Snapshot {
    secrets: Secrets,
    persisted: bool,
}

struct Store {
    scope: String,
    directory: PathBuf,
    cache: Mutex<Option<Snapshot>>,
}

fn invalid() -> String {
    "Saved native secrets are damaged or unavailable; existing keys were preserved".into()
}

fn slot(service: &str, account: &str) -> Result<String> {
    if !matches!(
        service,
        crate::auth::KEYRING_SERVICE | crate::local_storage::KEYRING_SERVICE
    ) || account.is_empty()
        || account.len() > 1024
        || account.contains('\0')
    {
        return Err("Invalid native credential scope".into());
    }
    Ok(format!("{service}\0{account}"))
}

fn directory(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.is_symlink() => {}
        Ok(_) => return Err("Native secret coordination requires a private directory".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            use std::os::unix::fs::DirBuilderExt as _;
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(path)
                .map_err(|_| "Could not prepare native secret coordination")?;
        }
        Err(_) => return Err("Could not inspect native secret coordination".into()),
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|_| "Could not protect native secret coordination".into())
}

fn private_file(path: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(nix::libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| "Could not open native secret coordination")?;
    let metadata = file
        .metadata()
        .map_err(|_| "Could not inspect native secret coordination")?;
    if !metadata.is_file() || metadata.nlink() != 1 {
        return Err("Native secret coordination requires a private regular file".into());
    }
    file.set_permissions(fs::Permissions::from_mode(0o600))
        .map_err(|_| "Could not protect native secret coordination")?;
    Ok(file)
}

impl Store {
    fn new(root: &Path) -> Result<Self> {
        directory(root)?;
        let root = root
            .canonicalize()
            .map_err(|_| "Could not identify native data profile")?;
        let directory = root.join("shared");
        crate::native_secrets::directory(&directory)?;
        Ok(Self {
            scope: format!(
                "{}:native-secrets-v1",
                hex::encode(Sha256::digest(root.as_os_str().as_encoded_bytes()))
            ),
            directory,
            cache: Mutex::new(None),
        })
    }

    fn marker(&self) -> PathBuf {
        self.directory.join("native-secrets.migrated")
    }

    fn migrated(&self) -> Result<bool> {
        match fs::symlink_metadata(self.marker()) {
            Ok(metadata)
                if metadata.is_file() && !metadata.is_symlink() && metadata.nlink() == 1 =>
            {
                Ok(true)
            }
            Ok(_) => Err("Invalid native secret migration marker".into()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(_) => Err("Could not inspect native secret migration".into()),
        }
    }

    fn mark_migrated(&self) -> Result<()> {
        if !self.migrated()? {
            private_file(&self.marker())?
                .sync_all()
                .map_err(|_| "Could not record native secret migration")?;
            File::open(&self.directory)
                .and_then(|file| file.sync_all())
                .map_err(|_| "Could not commit native secret migration")?;
        }
        Ok(())
    }

    fn refresh(&self, backend: &impl Backend, cached: &mut Option<Snapshot>) -> Result<()> {
        let revision = backend.revision()?;
        if let Some(revision) = revision {
            if cached
                .as_ref()
                .is_none_or(|snapshot| !snapshot.persisted || snapshot.secrets.revision != revision)
            {
                let bytes = backend.read()?;
                if bytes.len() > MAX_BYTES {
                    return Err(invalid());
                }
                let secrets: Secrets = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
                if secrets.version != 1
                    || secrets.scope != self.scope
                    || secrets.revision != revision
                    || uuid::Uuid::parse_str(&revision).is_err()
                    || secrets.entries.len() > MAX_ENTRIES
                {
                    return Err(invalid());
                }
                for (name, value) in &secrets.entries {
                    let (service, account) = name.split_once('\0').ok_or_else(invalid)?;
                    slot(service, account)?;
                    if let Some(value) = value {
                        let decoded =
                            Zeroizing::new(STANDARD.decode(value).map_err(|_| invalid())?);
                        if decoded.len() > MAX_SECRET_BYTES {
                            return Err(invalid());
                        }
                    }
                }
                *cached = Some(Snapshot {
                    secrets,
                    persisted: true,
                });
            }
            self.mark_migrated()?;
        } else {
            if cached.as_ref().is_some_and(|snapshot| snapshot.persisted) || self.migrated()? {
                return Err(
                    "The native secrets Keychain item is missing; restore access before continuing"
                        .into(),
                );
            }
            *cached = Some(Snapshot {
                secrets: self.empty(true),
                persisted: false,
            });
        }
        Ok(())
    }

    fn empty(&self, legacy_login: bool) -> Secrets {
        Secrets {
            version: 1,
            scope: self.scope.clone(),
            revision: uuid::Uuid::new_v4().to_string(),
            legacy_login,
            entries: BTreeMap::new(),
        }
    }

    fn commit(
        &self,
        backend: &impl Backend,
        snapshot: &mut Snapshot,
        name: String,
        value: Option<&[u8]>,
    ) -> Result<()> {
        if value.is_some_and(|bytes| bytes.len() > MAX_SECRET_BYTES) {
            return Err("Native secret exceeds storage limit".into());
        }
        let mut next = snapshot.secrets.clone();
        next.entries
            .insert(name, value.map(|bytes| STANDARD.encode(bytes)));
        if next.entries.len() > MAX_ENTRIES {
            return Err("Too many native credentials in this profile".into());
        }
        next.revision = uuid::Uuid::new_v4().to_string();
        let bytes = Zeroizing::new(serde_json::to_vec(&next).map_err(|_| invalid())?);
        if bytes.len() > MAX_BYTES {
            return Err("Native credentials exceed storage limit".into());
        }
        backend.write(&next.revision, &bytes, snapshot.persisted)?;
        *snapshot = Snapshot {
            secrets: next,
            persisted: true,
        };
        self.mark_migrated()
    }

    fn locked<T>(
        &self,
        backend: &impl Backend,
        operation: impl FnOnce(&mut Snapshot) -> Result<T>,
    ) -> Result<T> {
        let mut cached = self
            .cache
            .lock()
            .map_err(|_| "Native secret access was interrupted")?;
        let lock = private_file(&self.directory.join("native-secrets.lock"))?;
        fs2::FileExt::lock_exclusive(&lock)
            .map_err(|_| "Could not coordinate native secret access")?;
        // A different Desktop/CLI process may have rotated a token or added a
        // key. Its atomic revision changes invalidate only this memory cache.
        if let Err(error) = self.refresh(backend, &mut cached) {
            cached.take();
            return Err(error);
        }
        let result = operation(cached.as_mut().ok_or_else(invalid)?);
        if result.is_err() {
            cached.take();
        }
        result
    }

    fn read(
        &self,
        backend: &impl Backend,
        service: &str,
        account: &str,
    ) -> Result<Option<Zeroizing<Vec<u8>>>> {
        let name = slot(service, account)?;
        self.locked(backend, |snapshot| {
            if let Some(value) = snapshot.secrets.entries.get(&name) {
                let value = value
                    .as_ref()
                    .map(|value| {
                        STANDARD
                            .decode(value)
                            .map(Zeroizing::new)
                            .map_err(|_| invalid())
                    })
                    .transpose()?;
                if service == crate::auth::KEYRING_SERVICE {
                    backend.legacy_delete(service, account)?;
                }
                return Ok(value);
            }
            if service == crate::auth::KEYRING_SERVICE && !snapshot.secrets.legacy_login {
                return Ok(None);
            }
            let value = backend.legacy_read(service, account)?;
            if let Some(value) = &value {
                // Keep the old protected item for recovery. Only a successful
                // Keychain commit makes the combined copy authoritative.
                self.commit(backend, snapshot, name, Some(value))?;
                if service == crate::auth::KEYRING_SERVICE {
                    use subtle::ConstantTimeEq as _;
                    // Verify the protected copy before removing a legacy
                    // rotating token. An older client must not reuse that
                    // stale token and revoke the current session family.
                    let expected = Zeroizing::new(
                        serde_json::to_vec(&snapshot.secrets).map_err(|_| invalid())?,
                    );
                    let saved = backend.read()?;
                    if !bool::from(saved.as_slice().ct_eq(expected.as_slice())) {
                        return Err(invalid());
                    }
                    backend.legacy_delete(service, account)?;
                }
            }
            Ok(value)
        })
    }

    fn write(
        &self,
        backend: &impl Backend,
        service: &str,
        account: &str,
        value: &[u8],
    ) -> Result<()> {
        let name = slot(service, account)?;
        self.locked(backend, |snapshot| {
            self.commit(backend, snapshot, name, Some(value))
        })
    }

    fn delete(&self, backend: &impl Backend, service: &str, account: &str) -> Result<()> {
        let name = slot(service, account)?;
        let result = self.locked(backend, |snapshot| {
            self.commit(backend, snapshot, name, None)?;
            backend.legacy_delete(service, account)
        });
        // Sign-out/revocation also releases the unlocked profile snapshot.
        // The independent chat keys remain protected inside Keychain.
        if let Ok(mut cached) = self.cache.lock() {
            cached.take();
        }
        result
    }

    /// Only the explicit backup-recovery path calls this, after validating the
    /// supplied key against the existing encrypted database. A missing whole
    /// item can be recreated, but an unreadable/damaged item is never replaced.
    fn restore_storage_key(&self, backend: &impl Backend, account: &str, key: &[u8]) -> Result<()> {
        use subtle::ConstantTimeEq as _;
        if key.len() != 32 {
            return Err("Invalid recovered storage key".into());
        }
        let name = slot(crate::local_storage::KEYRING_SERVICE, account)?;
        let mut cached = self
            .cache
            .lock()
            .map_err(|_| "Native secret access was interrupted")?;
        let lock = private_file(&self.directory.join("native-secrets.lock"))?;
        fs2::FileExt::lock_exclusive(&lock)
            .map_err(|_| "Could not coordinate native secret recovery")?;
        if backend.revision()?.is_some() {
            self.refresh(backend, &mut cached)?;
        } else {
            // Restoring a database key does not authorize reviving old login
            // tokens. Fresh sign-in can install a new credential afterward.
            *cached = Some(Snapshot {
                secrets: self.empty(false),
                persisted: false,
            });
        }
        let snapshot = cached.as_mut().ok_or_else(invalid)?;
        let current = if let Some(value) = snapshot.secrets.entries.get(&name) {
            value
                .as_ref()
                .map(|value| {
                    STANDARD
                        .decode(value)
                        .map(Zeroizing::new)
                        .map_err(|_| invalid())
                })
                .transpose()?
        } else {
            backend.legacy_read(crate::local_storage::KEYRING_SERVICE, account)?
        };
        if current
            .as_ref()
            .is_some_and(|current| !bool::from(current.as_slice().ct_eq(key)))
        {
            return Err("A different storage key already exists at the restore destination".into());
        }
        self.commit(backend, snapshot, name, Some(key))
    }
}

fn with_store<T>(
    root: &Path,
    operation: impl FnOnce(&Store, &macos::Keychain) -> Result<T>,
) -> Result<T> {
    static STORES: OnceLock<Mutex<HashMap<PathBuf, Arc<Store>>>> = OnceLock::new();
    directory(root)?;
    let root = root
        .canonicalize()
        .map_err(|_| "Could not identify native data profile")?;
    let store = {
        let mut stores = STORES
            .get_or_init(Mutex::default)
            .lock()
            .map_err(|_| "Native secret access was interrupted")?;
        if let Some(store) = stores.get(&root) {
            Arc::clone(store)
        } else {
            let store = Arc::new(Store::new(&root)?);
            stores.insert(root, Arc::clone(&store));
            store
        }
    };
    let backend = macos::Keychain::user(&store.scope)?;
    operation(&store, &backend)
}

pub(crate) fn contains(root: &Path, service: &str, account: &str) -> Result<bool> {
    slot(service, account)?;
    with_store(root, |store, backend| {
        if backend.revision()?.is_some() {
            return Ok(true);
        }
        if store.migrated()? {
            return Err(invalid());
        }
        backend.legacy_contains(service, account)
    })
}

pub(crate) fn read(
    root: &Path,
    service: &str,
    account: &str,
) -> Result<Option<Zeroizing<Vec<u8>>>> {
    with_store(root, |store, backend| store.read(backend, service, account))
}

pub(crate) fn write(root: &Path, service: &str, account: &str, value: &[u8]) -> Result<()> {
    with_store(root, |store, backend| {
        store.write(backend, service, account, value)
    })
}

pub(crate) fn delete(root: &Path, service: &str, account: &str) -> Result<()> {
    with_store(root, |store, backend| {
        store.delete(backend, service, account)
    })
}

pub(crate) fn restore_storage_key(root: &Path, account: &str, key: &[u8]) -> Result<()> {
    with_store(root, |store, backend| {
        store.restore_storage_key(backend, account, key)
    })
}
