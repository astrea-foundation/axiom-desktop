//! Native-only encrypted local storage. Database keys are independent of login
//! credentials and never enter configuration, IPC, environment variables or logs.

use std::{path::Path, sync::Arc};

use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit, Payload},
};
use hkdf::Hkdf;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

use crate::{AxiomError, Result};

mod database;
mod keys;

pub(crate) use database::backup_database;
pub(crate) use keys::{KEYRING_SERVICE, load_key_for_path, save_key_for_path, scope_prefix};

const ASSET_MAGIC: [u8; 8] = *b"AXAST001";
const RECOVERY_MAGIC: [u8; 8] = *b"AXKEY001";
const NONCE_BYTES: usize = 24;
const TAG_BYTES: usize = 16;

/// Deliberately has no Debug, Serialize, Display or secret-export implementation.
/// Every store has an independently generated 256-bit root. Derivation separates
/// `SQLite`, asset encryption and portable recovery wrapping.
pub(crate) struct StorageKey {
    secret: Zeroizing<[u8; 32]>,
}

impl StorageKey {
    /// Explicit native injection for recovery and synthetic fixtures. Production
    /// opening obtains the key exclusively from the system credential store.
    pub(crate) fn from_bytes(bytes: [u8; 32]) -> Self {
        Self {
            secret: Zeroizing::new(bytes),
        }
    }

    fn generate() -> Result<Self> {
        let mut secret = Zeroizing::new([0; 32]);
        getrandom::fill(secret.as_mut()).map_err(|_| failure("Secure random generation failed"))?;
        Ok(Self { secret })
    }

    fn derive(&self, domain: &[u8]) -> Result<Zeroizing<[u8; 32]>> {
        derive(&self.secret, domain)
    }

    fn matches(&self, other: &Self) -> bool {
        bool::from(self.secret.as_ref().ct_eq(other.secret.as_ref()))
    }

    fn database_key(&self) -> Result<Zeroizing<String>> {
        let key = self.derive(b"axiom-local-storage/sqlcipher/v1")?;
        let encoded = Zeroizing::new(hex::encode(key.as_ref()));
        Ok(Zeroizing::new(format!("x'{}'", encoded.as_str())))
    }

    pub(crate) fn wrap_for_recovery(&self, recovery_key: &[u8; 32], aad: &[u8]) -> Result<Vec<u8>> {
        let key = derive(recovery_key, b"axiom-local-storage/recovery/v1")?;
        seal(&key, RECOVERY_MAGIC, aad, self.secret.as_ref())
    }

    pub(crate) fn from_recovery(
        recovery_key: &[u8; 32],
        aad: &[u8],
        wrapped: &[u8],
    ) -> Result<Self> {
        if wrapped.len() != RECOVERY_MAGIC.len() + NONCE_BYTES + 32 + TAG_BYTES {
            return Err(failure("Invalid storage recovery key envelope"));
        }
        let key = derive(recovery_key, b"axiom-local-storage/recovery/v1")?;
        let raw = Zeroizing::new(unseal(&key, RECOVERY_MAGIC, aad, wrapped)?);
        let mut secret = Zeroizing::new([0; 32]);
        secret.copy_from_slice(&raw);
        Ok(Self { secret })
    }
}

pub(crate) struct OpenedDatabase {
    pub connection: rusqlite::Connection,
    pub key: Arc<StorageKey>,
    pub maintenance: Option<crate::factory_reset::MaintenanceGuard>,
}

pub(crate) fn open_database(path: &Path) -> Result<OpenedDatabase> {
    check_restore_marker(path)?;
    let paths = database::StorePaths::prepare(path)?;
    let _lock = paths.lock()?;
    check_restore_marker(&paths.database)?;
    let mut maintenance = initial_maintenance(&paths)?;
    let key = Arc::new(keys::obtain(&paths)?);
    let connection = database::open_locked(&paths, &key)?;
    finish_open_maintenance(&paths, &connection, &mut maintenance)?;
    Ok(OpenedDatabase {
        connection,
        key,
        maintenance,
    })
}

fn check_restore_marker(path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(restore_marker_for_database(path)) {
        Ok(_) => Err(failure(
            "An encrypted restore is incomplete. Finish recovery before opening this account's local storage",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn restore_marker_for_database(path: &Path) -> std::path::PathBuf {
    let directory = path.parent().unwrap_or(Path::new("."));
    let mut name = std::ffi::OsString::from(".");
    name.push(directory.file_name().unwrap_or_default());
    name.push(".restore-pending");
    directory.with_file_name(name)
}

pub(crate) fn open_database_with_key(path: &Path, key: Arc<StorageKey>) -> Result<OpenedDatabase> {
    let paths = database::StorePaths::prepare(path)?;
    let _lock = paths.lock()?;
    let mut maintenance = initial_maintenance(&paths)?;
    let connection = database::open_locked(&paths, &key)?;
    finish_open_maintenance(&paths, &connection, &mut maintenance)?;
    Ok(OpenedDatabase {
        connection,
        key,
        maintenance,
    })
}

fn migration_lease(paths: &database::StorePaths) -> Result<crate::factory_reset::MaintenanceGuard> {
    crate::factory_reset::maintenance_for_database(&paths.database)
        .map_err(|error| failure(&error.to_string()))
}

fn initial_maintenance(
    paths: &database::StorePaths,
) -> Result<Option<crate::factory_reset::MaintenanceGuard>> {
    if paths.needs_initial_maintenance()? {
        Ok(Some(migration_lease(paths)?))
    } else {
        Ok(None)
    }
}

fn finish_open_maintenance(
    paths: &database::StorePaths,
    connection: &rusqlite::Connection,
    maintenance: &mut Option<crate::factory_reset::MaintenanceGuard>,
) -> Result<()> {
    if maintenance.is_none() && crate::session::needs_maintenance(connection)? {
        *maintenance = Some(migration_lease(paths)?);
    }
    Ok(())
}

fn derive(secret: &[u8; 32], domain: &[u8]) -> Result<Zeroizing<[u8; 32]>> {
    let mut key = Zeroizing::new([0; 32]);
    Hkdf::<Sha256>::new(Some(b"axiom-local-storage-v1"), secret)
        .expand(domain, key.as_mut())
        .map_err(|_| failure("Storage key derivation failed"))?;
    Ok(key)
}

fn asset_aad(key: &StorageKey, asset_id: &str) -> Result<Vec<u8>> {
    if asset_id.is_empty() || asset_id.len() > 1024 {
        return Err(failure("Invalid encrypted asset identity"));
    }
    // Store identity is stable under portable restore, but independent stores
    // cannot substitute ciphertext, even when they use the same asset filename.
    let identity = key.derive(b"axiom-local-storage/store-identity/v1")?;
    let mut aad = Vec::with_capacity(64 + asset_id.len());
    aad.extend_from_slice(b"axiom-local-asset-v1\0");
    aad.extend_from_slice(&Sha256::digest(identity.as_ref()));
    aad.extend_from_slice(&(asset_id.len() as u64).to_be_bytes());
    aad.extend_from_slice(asset_id.as_bytes());
    Ok(aad)
}

pub(crate) fn encrypt_asset(key: &StorageKey, asset_id: &str, bytes: &[u8]) -> Result<Vec<u8>> {
    let aad = asset_aad(key, asset_id)?;
    let secret = key.derive(b"axiom-local-storage/asset/v1")?;
    seal(&secret, ASSET_MAGIC, &aad, bytes)
}

pub(crate) fn decrypt_asset(key: &StorageKey, asset_id: &str, bytes: &[u8]) -> Result<Vec<u8>> {
    let aad = asset_aad(key, asset_id)?;
    let secret = key.derive(b"axiom-local-storage/asset/v1")?;
    unseal(&secret, ASSET_MAGIC, &aad, bytes)
}

fn seal(key: &[u8; 32], magic: [u8; 8], aad: &[u8], bytes: &[u8]) -> Result<Vec<u8>> {
    let mut nonce = [0; NONCE_BYTES];
    getrandom::fill(&mut nonce).map_err(|_| failure("Secure random generation failed"))?;
    let mut associated = Vec::with_capacity(magic.len() + aad.len());
    associated.extend_from_slice(&magic);
    associated.extend_from_slice(aad);
    let cipher = XChaCha20Poly1305::new(key.into());
    let encrypted = cipher
        .encrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: bytes,
                aad: &associated,
            },
        )
        .map_err(|_| failure("Local storage encryption failed"))?;
    let mut output = Vec::with_capacity(magic.len() + nonce.len() + encrypted.len());
    output.extend_from_slice(&magic);
    output.extend_from_slice(&nonce);
    output.extend_from_slice(&encrypted);
    Ok(output)
}

fn unseal(key: &[u8; 32], magic: [u8; 8], aad: &[u8], bytes: &[u8]) -> Result<Vec<u8>> {
    if bytes.len() < magic.len() + NONCE_BYTES + TAG_BYTES || !bytes.starts_with(&magic) {
        return Err(failure("Invalid encrypted local storage format"));
    }
    let mut associated = Vec::with_capacity(magic.len() + aad.len());
    associated.extend_from_slice(&magic);
    associated.extend_from_slice(aad);
    XChaCha20Poly1305::new(key.into())
        .decrypt(
            XNonce::from_slice(&bytes[8..32]),
            Payload {
                msg: &bytes[32..],
                aad: &associated,
            },
        )
        .map_err(|_| failure("Encrypted local storage authentication failed"))
}

fn failure(message: &str) -> AxiomError {
    AxiomError::Storage(message.into())
}

#[cfg(test)]
mod tests;
