//! Synthetic credentials only. The integration test uses a disposable Keychain
//! explicitly, with UI disabled; never read the user's login Keychain here.
use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

const LOGIN: &str = crate::auth::KEYRING_SERVICE;
const STORAGE: &str = crate::local_storage::KEYRING_SERVICE;
const TOKEN: &[u8] = b"synthetic-refresh-token-not-a-real-credential";
const KEY: &[u8] = &[0x57; 32];

#[derive(Default)]
struct Memory {
    revision: Option<String>,
    bytes: Vec<u8>,
    legacy: BTreeMap<(String, String), Vec<u8>>,
}

#[derive(Default)]
struct BackendFixture {
    state: Mutex<Memory>,
    reads: AtomicUsize,
    legacy_reads: AtomicUsize,
    deny_reads: AtomicBool,
    deny_legacy: AtomicBool,
    fail_writes: AtomicBool,
}

impl BackendFixture {
    fn legacy(&self, service: &str, account: &str, value: &[u8]) {
        self.state
            .lock()
            .unwrap()
            .legacy
            .insert((service.into(), account.into()), value.to_vec());
    }
}

impl Backend for BackendFixture {
    fn revision(&self) -> Result<Option<String>> {
        Ok(self.state.lock().unwrap().revision.clone())
    }
    fn read(&self) -> Result<Zeroizing<Vec<u8>>> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        if self.deny_reads.load(Ordering::SeqCst) {
            return Err("synthetic Keychain denied".into());
        }
        Ok(Zeroizing::new(self.state.lock().unwrap().bytes.clone()))
    }
    fn write(&self, revision: &str, bytes: &[u8], _: bool) -> Result<()> {
        if self.fail_writes.load(Ordering::SeqCst) {
            return Err("synthetic Keychain write failed".into());
        }
        let mut state = self.state.lock().unwrap();
        state.revision = Some(revision.into());
        state.bytes = bytes.to_vec();
        Ok(())
    }
    fn legacy_contains(&self, service: &str, account: &str) -> Result<bool> {
        Ok(self
            .state
            .lock()
            .unwrap()
            .legacy
            .contains_key(&(service.into(), account.into())))
    }
    fn legacy_read(&self, service: &str, account: &str) -> Result<Option<Zeroizing<Vec<u8>>>> {
        self.legacy_reads.fetch_add(1, Ordering::SeqCst);
        if self.deny_legacy.load(Ordering::SeqCst) {
            return Err("synthetic legacy Keychain denied".into());
        }
        Ok(self
            .state
            .lock()
            .unwrap()
            .legacy
            .get(&(service.into(), account.into()))
            .cloned()
            .map(Zeroizing::new))
    }
    fn legacy_delete(&self, service: &str, account: &str) -> Result<()> {
        self.state
            .lock()
            .unwrap()
            .legacy
            .remove(&(service.into(), account.into()));
        Ok(())
    }
}

fn migrated(root: &Path, backend: &BackendFixture) -> Store {
    backend.legacy(LOGIN, "production", TOKEN);
    backend.legacy(STORAGE, "account-a:desktop", KEY);
    let store = Store::new(root).unwrap();
    assert_eq!(
        store
            .read(backend, LOGIN, "production")
            .unwrap()
            .unwrap()
            .as_slice(),
        TOKEN
    );
    assert_eq!(
        store
            .read(backend, STORAGE, "account-a:desktop")
            .unwrap()
            .unwrap()
            .as_slice(),
        KEY
    );
    store
}

#[test]
fn one_protected_read_unlocks_signin_and_storage_and_rotation_needs_no_second_read() {
    let root = tempfile::tempdir().unwrap();
    let backend = BackendFixture::default();
    drop(migrated(root.path(), &backend));
    assert_eq!(
        backend.legacy_reads.load(Ordering::SeqCst),
        2,
        "one-time migration reads both old items"
    );
    assert!(
        backend
            .legacy_contains(STORAGE, "account-a:desktop")
            .unwrap()
    );
    backend.deny_legacy.store(true, Ordering::SeqCst);
    backend.reads.store(0, Ordering::SeqCst);

    let reopened = Store::new(root.path()).unwrap();
    assert_eq!(
        reopened
            .read(&backend, LOGIN, "production")
            .unwrap()
            .unwrap()
            .as_slice(),
        TOKEN
    );
    reopened
        .write(&backend, LOGIN, "production", b"rotated-synthetic-token")
        .unwrap();
    assert_eq!(
        reopened
            .read(&backend, STORAGE, "account-a:desktop")
            .unwrap()
            .unwrap()
            .as_slice(),
        KEY
    );
    assert_eq!(backend.reads.load(Ordering::SeqCst), 1);
    for entry in fs::read_dir(root.path().join("shared")).unwrap() {
        let path = entry.unwrap().path();
        assert_eq!(
            fs::read(path).unwrap(),
            b"",
            "disk contains only empty coordination files"
        );
    }
}

#[test]
fn logout_keeps_keys_other_origins_and_tombstones_old_credentials() {
    let root = tempfile::tempdir().unwrap();
    let backend = BackendFixture::default();
    let store = migrated(root.path(), &backend);
    store
        .write(&backend, LOGIN, "staging", b"staging-synthetic-token")
        .unwrap();
    store.delete(&backend, LOGIN, "production").unwrap();
    assert!(!backend.legacy_contains(LOGIN, "production").unwrap());
    // An older process must not resurrect the signed-out credential.
    backend.legacy(LOGIN, "production", TOKEN);
    let reopened = Store::new(root.path()).unwrap();
    assert!(
        reopened
            .read(&backend, LOGIN, "production")
            .unwrap()
            .is_none()
    );
    assert_eq!(
        reopened
            .read(&backend, STORAGE, "account-a:desktop")
            .unwrap()
            .unwrap()
            .as_slice(),
        KEY
    );
    assert_eq!(
        reopened
            .read(&backend, LOGIN, "staging")
            .unwrap()
            .unwrap()
            .as_slice(),
        b"staging-synthetic-token"
    );
}

#[test]
fn concurrent_native_processes_merge_rotations_and_independent_keys() {
    let root = tempfile::tempdir().unwrap();
    let backend = Arc::new(BackendFixture::default());
    drop(migrated(root.path(), &backend));
    let workers: Vec<_> = (0..6)
        .map(|index| {
            let backend = Arc::clone(&backend);
            let path = root.path().to_owned();
            std::thread::spawn(move || {
                let store = Store::new(&path).unwrap();
                for generation in 0..8 {
                    store
                        .write(
                            backend.as_ref(),
                            LOGIN,
                            &format!("origin-{index}"),
                            format!("token-{index}-{generation}").as_bytes(),
                        )
                        .unwrap();
                    store
                        .write(
                            backend.as_ref(),
                            STORAGE,
                            &format!("account-{index}"),
                            &[index; 32],
                        )
                        .unwrap();
                }
            })
        })
        .collect();
    for worker in workers {
        worker.join().unwrap();
    }
    let store = Store::new(root.path()).unwrap();
    for index in 0..6 {
        assert_eq!(
            store
                .read(backend.as_ref(), LOGIN, &format!("origin-{index}"))
                .unwrap()
                .unwrap()
                .as_slice(),
            format!("token-{index}-7").as_bytes()
        );
        assert_eq!(
            store
                .read(backend.as_ref(), STORAGE, &format!("account-{index}"))
                .unwrap()
                .unwrap()
                .as_slice(),
            &[index; 32]
        );
    }
    assert_eq!(
        store
            .read(backend.as_ref(), STORAGE, "account-a:desktop")
            .unwrap()
            .unwrap()
            .as_slice(),
        KEY
    );
}

#[test]
fn denied_migration_and_failed_writes_preserve_all_prior_secrets() {
    let root = tempfile::tempdir().unwrap();
    let backend = BackendFixture::default();
    let store = migrated(root.path(), &backend);
    let original = backend.state.lock().unwrap().bytes.clone();
    backend.fail_writes.store(true, Ordering::SeqCst);
    assert!(
        store
            .write(&backend, LOGIN, "production", b"replacement-token")
            .is_err()
    );
    assert_eq!(backend.state.lock().unwrap().bytes, original);
    assert_eq!(
        store
            .read(&backend, LOGIN, "production")
            .unwrap()
            .unwrap()
            .as_slice(),
        TOKEN
    );
    backend.fail_writes.store(false, Ordering::SeqCst);
    backend.deny_legacy.store(true, Ordering::SeqCst);
    assert!(store.read(&backend, STORAGE, "unmigrated-account").is_err());
    assert_eq!(backend.state.lock().unwrap().bytes, original);

    backend.deny_reads.store(true, Ordering::SeqCst);
    let reopened = Store::new(root.path()).unwrap();
    assert!(reopened.read(&backend, LOGIN, "production").is_err());
    assert_eq!(backend.state.lock().unwrap().bytes, original);
}

#[test]
fn missing_corrupt_or_other_profile_items_never_fall_back_or_replace_keys() {
    for failure in ["missing", "corrupt", "scope", "version", "revision"] {
        let root = tempfile::tempdir().unwrap();
        let backend = BackendFixture::default();
        drop(migrated(root.path(), &backend));
        {
            let mut state = backend.state.lock().unwrap();
            match failure {
                "missing" => {
                    state.revision = None;
                    state.bytes.clear();
                }
                "corrupt" => state.bytes = b"unreadable protected payload".to_vec(),
                _ => {
                    let mut value: serde_json::Value =
                        serde_json::from_slice(&state.bytes).unwrap();
                    match failure {
                        "scope" => value["scope"] = serde_json::json!("other-profile"),
                        "version" => value["version"] = serde_json::json!(99),
                        _ => {
                            value["revision"] = serde_json::json!(uuid::Uuid::new_v4().to_string());
                        }
                    }
                    state.bytes = serde_json::to_vec(&value).unwrap();
                }
            }
        }
        let prior = backend.state.lock().unwrap().bytes.clone();
        let reopened = Store::new(root.path()).unwrap();
        assert!(
            reopened
                .read(&backend, STORAGE, "account-a:desktop")
                .is_err(),
            "{failure}"
        );
        assert!(
            reopened
                .write(&backend, LOGIN, "production", TOKEN)
                .is_err(),
            "{failure}"
        );
        assert_eq!(backend.state.lock().unwrap().bytes, prior);
        assert!(
            backend
                .legacy_contains(STORAGE, "account-a:desktop")
                .unwrap()
        );
        assert_eq!(
            backend.legacy_reads.load(Ordering::SeqCst),
            2,
            "must not silently adopt legacy keys again"
        );
    }
}

#[test]
fn explicit_recovery_can_restore_a_missing_item_without_reviving_login_or_replacing_other_secrets()
{
    let root = tempfile::tempdir().unwrap();
    let backend = BackendFixture::default();
    drop(migrated(root.path(), &backend));
    backend.legacy(LOGIN, "production", TOKEN);
    {
        let mut state = backend.state.lock().unwrap();
        state.revision = None;
        state.bytes.clear();
    }
    let recovered = Store::new(root.path()).unwrap();
    assert!(
        recovered
            .restore_storage_key(&backend, "account-a:desktop", &[0x58; 32])
            .is_err()
    );
    assert!(backend.revision().unwrap().is_none());
    recovered
        .restore_storage_key(&backend, "account-a:desktop", KEY)
        .unwrap();
    assert_eq!(
        recovered
            .read(&backend, STORAGE, "account-a:desktop")
            .unwrap()
            .unwrap()
            .as_slice(),
        KEY
    );
    assert!(
        recovered
            .read(&backend, LOGIN, "production")
            .unwrap()
            .is_none()
    );
    assert!(backend.legacy_contains(LOGIN, "production").unwrap());
    recovered
        .write(&backend, LOGIN, "production", b"new-signin-token")
        .unwrap();
    assert_eq!(
        recovered
            .read(&backend, LOGIN, "production")
            .unwrap()
            .unwrap()
            .as_slice(),
        b"new-signin-token"
    );
    let saved = backend.state.lock().unwrap().bytes.clone();
    assert!(
        recovered
            .restore_storage_key(&backend, "account-a:desktop", &[0x59; 32])
            .is_err()
    );
    assert_eq!(backend.state.lock().unwrap().bytes, saved);
    {
        let mut state = backend.state.lock().unwrap();
        state.bytes = b"damaged combined Keychain item".to_vec();
        state.revision = Some(uuid::Uuid::new_v4().to_string());
    }
    assert!(
        Store::new(root.path())
            .unwrap()
            .restore_storage_key(&backend, "account-a:desktop", KEY)
            .is_err(),
        "recovery must never overwrite a damaged item containing other accounts' secrets"
    );
}

#[test]
fn legacy_login_is_removed_only_after_a_verified_combined_copy() {
    let root = tempfile::tempdir().unwrap();
    let backend = BackendFixture::default();
    backend.legacy(LOGIN, "production", TOKEN);
    backend.fail_writes.store(true, Ordering::SeqCst);
    let store = Store::new(root.path()).unwrap();
    assert!(store.read(&backend, LOGIN, "production").is_err());
    assert!(backend.revision().unwrap().is_none());
    assert!(backend.legacy_contains(LOGIN, "production").unwrap());
    backend.fail_writes.store(false, Ordering::SeqCst);
    backend.deny_reads.store(true, Ordering::SeqCst);
    assert!(store.read(&backend, LOGIN, "production").is_err());
    assert!(backend.legacy_contains(LOGIN, "production").unwrap());
    backend.deny_reads.store(false, Ordering::SeqCst);
    assert_eq!(
        store
            .read(&backend, LOGIN, "production")
            .unwrap()
            .unwrap()
            .as_slice(),
        TOKEN
    );
    assert!(!backend.legacy_contains(LOGIN, "production").unwrap());
}

#[test]
fn profile_symlink_and_linked_coordination_files_fail_before_secret_access() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    symlink(other.path(), root.path().join("shared")).unwrap();
    assert!(Store::new(root.path()).is_err());
    fs::remove_file(root.path().join("shared")).unwrap();
    let store = Store::new(root.path()).unwrap();
    let backend = BackendFixture::default();
    let outside = other.path().join("keep");
    fs::write(&outside, b"unrelated data").unwrap();
    symlink(&outside, store.directory.join("native-secrets.lock")).unwrap();
    assert!(store.write(&backend, LOGIN, "production", TOKEN).is_err());
    assert_eq!(fs::read(&outside).unwrap(), b"unrelated data");
    fs::remove_file(store.directory.join("native-secrets.lock")).unwrap();
    fs::hard_link(&outside, store.directory.join("native-secrets.lock")).unwrap();
    assert!(store.write(&backend, LOGIN, "production", TOKEN).is_err());
    assert_eq!(backend.reads.load(Ordering::SeqCst), 0);
}
