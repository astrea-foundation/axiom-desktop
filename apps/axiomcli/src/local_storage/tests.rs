use super::*;
use rusqlite::Connection;
use std::{fs, path::PathBuf};

const CANARY: &str = "synthetic-private-storage-canary-58dd399e";

#[test]
fn linked_sqlcipher_has_patched_sqlite_and_openssl_provider() {
    assert!(database::require_supported_sqlite_version(3_053_004).is_err());
    assert!(database::require_supported_sqlite_version(3_054_000).is_err());
    database::require_supported_sqlite_version(rusqlite::version_number()).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let opened = open_database_with_key(&directory.path().join("state.sqlite3"), key()).unwrap();
    let sqlite: String = opened
        .connection
        .query_row("SELECT sqlite_version()", [], |row| row.get(0))
        .unwrap();
    let cipher: String = opened
        .connection
        .query_row("PRAGMA cipher_version", [], |row| row.get(0))
        .unwrap();
    let provider: String = opened
        .connection
        .query_row("PRAGMA cipher_provider", [], |row| row.get(0))
        .unwrap();
    let provider_version: String = opened
        .connection
        .query_row("PRAGMA cipher_provider_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(sqlite, "3.54.1");
    assert_eq!(cipher.split_whitespace().next(), Some("4.20.0"));
    assert_eq!(provider, "openssl");
    assert_eq!(provider_version, "OpenSSL 4.0.3 29 Sep 2026");
}

fn key() -> Arc<StorageKey> {
    Arc::new(StorageKey::from_bytes([0x71; 32]))
}

#[test]
fn decryption_observation_requires_an_existing_encrypted_database() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("encrypted.sqlite3");
    let observations = std::cell::Cell::new(0);
    let decrypting = || observations.set(observations.get() + 1);
    let opened = open_database_with_key_and_progress(&path, key(), &|_| {}, &decrypting).unwrap();
    assert_eq!(
        observations.get(),
        0,
        "creating a fresh store is not decrypting saved chats"
    );
    fixture(&opened.connection);
    drop(opened);
    let reopened = open_database_with_key_and_progress(&path, key(), &|_| {}, &decrypting).unwrap();
    assert_eq!(observations.get(), 1);
    verify_fixture(&reopened.connection);
    drop(reopened);
    assert!(
        open_database_with_key_and_progress(
            &path,
            Arc::new(StorageKey::from_bytes([0x72; 32])),
            &|_| {},
            &decrypting,
        )
        .is_err()
    );
    assert_eq!(
        observations.get(),
        2,
        "failed decryption still reports the work attempted"
    );

    let legacy = directory.path().join("plaintext.sqlite3");
    let source = Connection::open(&legacy).unwrap();
    fixture(&source);
    drop(source);
    let migration =
        open_database_with_key_and_progress(&legacy, key(), &|_| {}, &decrypting).unwrap();
    assert_eq!(
        observations.get(),
        2,
        "legacy encryption keeps its migration progress"
    );
    verify_fixture(&migration.connection);
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

fn fixture(connection: &Connection) {
    connection.execute_batch(
        "PRAGMA application_id=1096302899; PRAGMA user_version=6;
         CREATE TABLE parent(id INTEGER PRIMARY KEY AUTOINCREMENT, text TEXT NOT NULL, binary BLOB);
         CREATE TABLE child(id INTEGER PRIMARY KEY, parent_id INTEGER REFERENCES parent(id), score REAL);
         CREATE INDEX child_parent ON child(parent_id);
         CREATE TRIGGER parent_audit AFTER INSERT ON parent BEGIN INSERT INTO child(parent_id,score) VALUES(new.id,1.25); END;
         CREATE VIEW saved_text AS SELECT text FROM parent;"
    ).unwrap();
    connection
        .execute(
            "INSERT INTO parent(text,binary) VALUES(?1,?2)",
            rusqlite::params![CANARY, &[0u8, 1, 0xff, 0, 42][..]],
        )
        .unwrap();
    // Exercise sqlite_sequence preservation beyond the highest surviving row.
    connection
        .execute("INSERT INTO parent(text) VALUES('deleted')", [])
        .unwrap();
    connection
        .execute("DELETE FROM child WHERE parent_id=2", [])
        .unwrap();
    connection
        .execute("DELETE FROM parent WHERE id=2", [])
        .unwrap();
}

fn verify_fixture(connection: &Connection) {
    assert_eq!(
        connection
            .query_row("PRAGMA application_id", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1_096_302_899
    );
    assert_eq!(
        connection
            .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        6
    );
    assert_eq!(
        connection
            .query_row("SELECT text FROM saved_text", [], |r| r.get::<_, String>(0))
            .unwrap(),
        CANARY
    );
    assert_eq!(
        connection
            .query_row("SELECT binary FROM parent", [], |r| r.get::<_, Vec<u8>>(0))
            .unwrap(),
        [0, 1, 0xff, 0, 42]
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT seq FROM sqlite_sequence WHERE name='parent'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        2
    );
    assert_eq!(
        connection
            .query_row("SELECT score FROM child", [], |r| r.get::<_, f64>(0))
            .unwrap()
            .to_bits(),
        1.25_f64.to_bits()
    );
    assert_eq!(
        connection
            .query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
}

fn assert_ciphertext(path: &Path) {
    let bytes = fs::read(path).unwrap();
    assert!(!bytes.is_empty());
    assert!(!bytes.starts_with(b"SQLite format 3\0"));
    assert!(!bytes.windows(CANARY.len()).any(|v| v == CANARY.as_bytes()));
}

fn assert_migration_clean(path: &Path) {
    for suffix in [
        ".encrypted-migration",
        ".plaintext-migration",
        ".encryption-migration",
        ".encryption-marker-new",
    ] {
        assert!(!sidecar(path, suffix).exists(), "leftover {suffix}");
    }
}

#[test]
fn new_database_and_wal_are_encrypted_and_wrong_key_preserves_database() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.sqlite3");
    let opened = open_database_with_key(&path, key()).unwrap();
    opened
        .connection
        .execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;")
        .unwrap();
    fixture(&opened.connection);
    assert_ciphertext(&path);
    let wal = fs::read(sidecar(&path, "-wal")).unwrap();
    assert!(!wal.windows(CANARY.len()).any(|v| v == CANARY.as_bytes()));
    assert_eq!(
        opened
            .connection
            .query_row("PRAGMA temp_store", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
    opened.connection.close().unwrap();
    let before = fs::read(&path).unwrap();
    assert!(open_database_with_key(&path, Arc::new(StorageKey::from_bytes([0x72; 32]))).is_err());
    assert_eq!(before, fs::read(&path).unwrap());
    let unkeyed = Connection::open(&path).unwrap();
    assert!(
        unkeyed
            .query_row("SELECT count(*) FROM sqlite_schema", [], |r| r
                .get::<_, i64>(0))
            .is_err()
    );
    unkeyed.close().unwrap();
    verify_fixture(&open_database_with_key(&path, key()).unwrap().connection);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[test]
fn plaintext_migration_preserves_schema_metadata_and_every_value() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.sqlite3");
    let plain = Connection::open(&path).unwrap();
    fixture(&plain);
    plain.close().unwrap();
    assert!(fs::read(&path).unwrap().starts_with(b"SQLite format 3\0"));
    let opened = open_database_with_key(&path, key()).unwrap();
    verify_fixture(&opened.connection);
    assert_ciphertext(&path);
    assert_migration_clean(&path);
    opened
        .connection
        .execute("INSERT INTO parent(text) VALUES('after migration')", [])
        .unwrap();
    assert_eq!(opened.connection.last_insert_rowid(), 3);
    assert_eq!(
        opened
            .connection
            .query_row("SELECT count(*) FROM child", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
}

#[test]
fn migration_includes_committed_uncheckpointed_plaintext_wal() {
    let original = tempfile::tempdir().unwrap();
    let crashed = tempfile::tempdir().unwrap();
    let original_path = original.path().join("state.sqlite3");
    let path = crashed.path().join("state.sqlite3");
    let plain = Connection::open(&original_path).unwrap();
    plain
        .execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;")
        .unwrap();
    fixture(&plain);
    // Copy an idle committed database + WAL while its connection remains open:
    // the destination is the state left by a process exiting without cleanup.
    fs::copy(&original_path, &path).unwrap();
    fs::copy(sidecar(&original_path, "-wal"), sidecar(&path, "-wal")).unwrap();
    plain.close().unwrap();
    assert!(
        fs::read(sidecar(&path, "-wal"))
            .unwrap()
            .windows(CANARY.len())
            .any(|v| v == CANARY.as_bytes())
    );
    let opened = open_database_with_key(&path, key()).unwrap();
    verify_fixture(&opened.connection);
    assert_ciphertext(&path);
    assert!(!sidecar(&path, "-wal").exists());
    assert!(!sidecar(&path, "-shm").exists());
    assert_migration_clean(&path);
}

#[test]
fn migration_recovers_each_durable_replacement_boundary() {
    for (move_old, install) in [(false, false), (true, false), (true, true)] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.sqlite3");
        let plain = Connection::open(&path).unwrap();
        fixture(&plain);
        plain.close().unwrap();
        database::test_support::stage(&path, &key(), move_old, install).unwrap();
        // Authentication must happen before any rename or plaintext removal.
        let before: Vec<_> = [
            "",
            ".encrypted-migration",
            ".plaintext-migration",
            ".encryption-migration",
        ]
        .map(|suffix| fs::read(sidecar(&path, suffix)).ok())
        .into();
        assert!(
            open_database_with_key(&path, Arc::new(StorageKey::from_bytes([0x44; 32]))).is_err()
        );
        let after: Vec<_> = [
            "",
            ".encrypted-migration",
            ".plaintext-migration",
            ".encryption-migration",
        ]
        .map(|suffix| fs::read(sidecar(&path, suffix)).ok())
        .into();
        assert_eq!(before, after);
        verify_fixture(&open_database_with_key(&path, key()).unwrap().connection);
        assert_ciphertext(&path);
        assert_migration_clean(&path);
    }
}

#[test]
fn incomplete_export_and_unpublished_marker_retry_from_intact_plaintext() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.sqlite3");
    let plain = Connection::open(&path).unwrap();
    fixture(&plain);
    plain.close().unwrap();
    fs::write(
        sidecar(&path, ".encrypted-migration"),
        b"partial encrypted export",
    )
    .unwrap();
    fs::write(sidecar(&path, ".encryption-marker-new"), b"partial marker").unwrap();
    verify_fixture(&open_database_with_key(&path, key()).unwrap().connection);
    assert_migration_clean(&path);
}

#[test]
fn corrupt_source_and_tampered_migration_copies_are_never_replaced() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.sqlite3");
    for content in [
        b"not a valid database".as_slice(),
        b"SQLite format 3\0broken".as_slice(),
    ] {
        fs::write(&path, content).unwrap();
        assert!(open_database_with_key(&path, key()).is_err());
        assert_eq!(fs::read(&path).unwrap(), content);
        assert!(!sidecar(&path, ".encrypted-migration").exists());
    }
    fs::remove_file(&path).unwrap();
    let plain = Connection::open(&path).unwrap();
    fixture(&plain);
    plain.close().unwrap();
    database::test_support::stage(&path, &key(), false, false).unwrap();
    let before = fs::read(&path).unwrap();
    fs::write(
        sidecar(&path, ".encrypted-migration"),
        b"damaged ciphertext",
    )
    .unwrap();
    assert!(open_database_with_key(&path, key()).is_err());
    assert_eq!(fs::read(&path).unwrap(), before);
    assert!(sidecar(&path, ".encryption-migration").exists());
}

#[test]
fn encrypted_backup_is_consistent_and_refuses_overwrite() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.sqlite3");
    let destination = directory.path().join("backup.sqlite3");
    let opened = open_database_with_key(&path, key()).unwrap();
    opened
        .connection
        .execute_batch("PRAGMA journal_mode=WAL;")
        .unwrap();
    fixture(&opened.connection);
    backup_database(&opened.connection, &opened.key, &destination).unwrap();
    assert_ciphertext(&destination);
    verify_fixture(
        &open_database_with_key(&destination, key())
            .unwrap()
            .connection,
    );
    let before = fs::read(&destination).unwrap();
    assert!(backup_database(&opened.connection, &opened.key, &destination).is_err());
    assert_eq!(fs::read(destination).unwrap(), before);
}

#[test]
fn asset_aead_binds_store_identity_filename_version_and_contents() {
    let key = key();
    let encrypted = encrypt_asset(&key, "opaque-asset-id", CANARY.as_bytes()).unwrap();
    let again = encrypt_asset(&key, "opaque-asset-id", CANARY.as_bytes()).unwrap();
    assert_ne!(encrypted, again);
    assert!(
        !encrypted
            .windows(CANARY.len())
            .any(|v| v == CANARY.as_bytes())
    );
    assert_eq!(
        decrypt_asset(&key, "opaque-asset-id", &encrypted).unwrap(),
        CANARY.as_bytes()
    );
    assert!(decrypt_asset(&key, "substituted-id", &encrypted).is_err());
    assert!(
        decrypt_asset(
            &StorageKey::from_bytes([0x22; 32]),
            "opaque-asset-id",
            &encrypted
        )
        .is_err()
    );
    for index in [0, 8, 31, 32, encrypted.len() - 1] {
        let mut tampered = encrypted.clone();
        tampered[index] ^= 1;
        assert!(decrypt_asset(&key, "opaque-asset-id", &tampered).is_err());
    }
    for length in [0, 7, 31, 47, encrypted.len() - 1] {
        assert!(decrypt_asset(&key, "opaque-asset-id", &encrypted[..length]).is_err());
    }
    assert!(encrypt_asset(&key, "", b"value").is_err());
}

#[test]
fn recovery_key_wrapping_authenticates_manifest_and_restores_same_store() {
    let key = key();
    let recovery_key = [0x31; 32];
    let aad = b"synthetic authenticated archive manifest";
    let wrapped = key.wrap_for_recovery(&recovery_key, aad).unwrap();
    let recovered = StorageKey::from_recovery(&recovery_key, aad, &wrapped).unwrap();
    assert!(key.matches(&recovered));
    assert!(StorageKey::from_recovery(&[0x32; 32], aad, &wrapped).is_err());
    assert!(StorageKey::from_recovery(&recovery_key, b"another manifest", &wrapped).is_err());
    let asset = encrypt_asset(&key, "asset-id", CANARY.as_bytes()).unwrap();
    assert_eq!(
        decrypt_asset(&recovered, "asset-id", &asset).unwrap(),
        CANARY.as_bytes()
    );
    assert!(StorageKey::from_recovery(&recovery_key, aad, &asset).is_err());
}

#[test]
fn pending_restore_fails_before_keychain_and_explicit_validation_can_open() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("account/desktop/state.sqlite3");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let marker = restore_marker_for_database(&path);
    assert_eq!(
        marker,
        directory.path().join("account/.desktop.restore-pending")
    );
    fs::write(&marker, b"synthetic restore manifest").unwrap();
    assert!(
        open_database(&path)
            .err()
            .unwrap()
            .to_string()
            .contains("restore is incomplete")
    );
    assert!(!path.exists());
    open_database_with_key(&path, key())
        .unwrap()
        .connection
        .close()
        .unwrap();
    assert!(open_database(&path).is_err());
    assert!(marker.exists());
}

#[cfg(unix)]
#[test]
fn links_are_rejected_and_dangling_restore_marker_still_blocks_open() {
    use std::os::unix::fs::symlink;
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("outside.sqlite3");
    let path = directory.path().join("state.sqlite3");
    fs::write(&target, "unrelated data").unwrap();
    symlink(&target, &path).unwrap();
    assert!(open_database_with_key(&path, key()).is_err());
    assert_eq!(fs::read(&target).unwrap(), b"unrelated data");
    let restore_path = directory.path().join("account/desktop/state.sqlite3");
    fs::create_dir_all(restore_path.parent().unwrap()).unwrap();
    symlink(
        directory.path().join("missing"),
        restore_marker_for_database(&restore_path),
    )
    .unwrap();
    assert!(
        open_database(&restore_path)
            .err()
            .unwrap()
            .to_string()
            .contains("restore is incomplete")
    );
}

#[test]
fn migration_requires_exclusive_runtime_lease_but_current_store_allows_other_readers() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let paths =
        crate::paths::AxiomPaths::from_roots(root.join("config/axiom"), root.join("data/axiom"));
    let path = paths.data_root().join("accounts/one/desktop/state.sqlite3");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let plain = Connection::open(&path).unwrap();
    fixture(&plain);
    plain.close().unwrap();
    let _runtime = crate::factory_reset::runtime_lease(&paths).unwrap();
    let other = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(root.join("data/.axiom-factory-reset.lock"))
        .unwrap();
    fs2::FileExt::try_lock_shared(&other).unwrap();
    let before = fs::read(&path).unwrap();
    assert!(open_database_with_key(&path, key()).is_err());
    assert_eq!(before, fs::read(&path).unwrap());
    assert_migration_clean(&path);
    fs2::FileExt::unlock(&other).unwrap();
    let opened = open_database_with_key(&path, key()).unwrap();
    assert!(opened.maintenance.is_some());
    assert!(fs2::FileExt::try_lock_shared(&other).is_err());
    verify_fixture(&opened.connection);
    opened.connection.execute_batch("PRAGMA user_version=7; CREATE TABLE asset_maintenance(id INTEGER PRIMARY KEY, purge_pending INTEGER); INSERT INTO asset_maintenance VALUES(1,1);").unwrap();
    drop(opened);
    fs2::FileExt::try_lock_shared(&other).unwrap();
    // An encrypted v7 store still needs the fence when purge is incomplete.
    assert!(open_database_with_key(&path, key()).is_err());
    fs2::FileExt::unlock(&other).unwrap();
    let opened = open_database_with_key(&path, key()).unwrap();
    assert!(opened.maintenance.is_some());
    opened
        .connection
        .execute("UPDATE asset_maintenance SET purge_pending=0", [])
        .unwrap();
    drop(opened);
    fs2::FileExt::try_lock_shared(&other).unwrap();
    let opened = open_database_with_key(&path, key()).unwrap();
    assert!(opened.maintenance.is_none());
}
