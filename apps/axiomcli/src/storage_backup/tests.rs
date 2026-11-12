use super::*;
use serde_json::{Value, json};

const IMAGE: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGP4z8AAAAMBAQDJ/pLvAAAAAElFTkSuQmCC";

fn fixture() -> (tempfile::TempDir, PathBuf, Arc<StorageKey>, Value) {
    let root = tempfile::tempdir().unwrap();
    let database = root.path().join("source/state.sqlite3");
    let key = Arc::new(StorageKey::from_bytes([0x71; 32]));
    let store = SessionStore::open_protected_with_key(&database, key.clone()).unwrap();
    let payload = json!({"version":1,"kind":"prompt","payload":{
        "text":"Synthetic private backup text",
        "attachments":[{"kind":"image","name":"fixture.png","image":{"mimeType":"image/png","data":IMAGE}}]
    }});
    store.set_local_payload("draft", &payload).unwrap();
    store.verify_asset_integrity().unwrap();
    drop(store);
    (root, database, key, payload)
}

fn paths(root: &Path) -> AxiomPaths {
    AxiomPaths::from_roots(root.join("config"), root.join("data"))
}

fn assert_absent(bytes: &[u8], secret: &[u8]) {
    assert!(!bytes.windows(secret.len()).any(|part| part == secret));
}

#[test]
fn encrypted_backup_restores_database_and_external_assets_to_another_directory() {
    let (root, database, key, payload) = fixture();
    let archive = root.path().join("portable.axbackup");
    let recovery = [0x51; 32];
    create_archive(
        &database,
        "account-a",
        FrontendKind::DesktopChat,
        &archive,
        &key,
        &recovery,
    )
    .unwrap();
    let raw = fs::read(&archive).unwrap();
    assert_absent(&raw, b"Synthetic private backup text");
    assert_absent(&raw, IMAGE.as_bytes());
    assert_absent(&raw, &STANDARD.decode(IMAGE).unwrap());
    let destination = paths(&root.path().join("destination"));
    restore_archive(
        &destination,
        open_archive(&archive, &recovery).unwrap(),
        |_, _| Ok(()),
    )
    .unwrap();
    let restored_path = destination
        .account_state_database("account-a", FrontendKind::DesktopChat)
        .unwrap();
    let restored = SessionStore::open_protected_with_key(&restored_path, key.clone()).unwrap();
    assert_eq!(restored.local_payload("draft").unwrap(), Some(payload));
    restored.verify_asset_integrity().unwrap();
    assert_eq!(
        fs::read_dir(asset_directory_for_database(&restored_path))
            .unwrap()
            .count(),
        1
    );
    drop(restored);
    let before = fs::read(&restored_path).unwrap();
    assert!(
        restore_archive(
            &destination,
            open_archive(&archive, &recovery).unwrap(),
            |_, _| Ok(())
        )
        .is_err()
    );
    assert_eq!(fs::read(restored_path).unwrap(), before);
    assert!(
        create_archive(
            &database,
            "account-a",
            FrontendKind::DesktopChat,
            &archive,
            &key,
            &recovery
        )
        .is_err()
    );
}

#[test]
fn wrong_recovery_key_and_modified_ciphertext_fail_without_installing_a_store() {
    let (root, database, key, _) = fixture();
    let archive = root.path().join("portable.axbackup");
    let recovery = [0x52; 32];
    create_archive(
        &database,
        "account-a",
        FrontendKind::Cli,
        &archive,
        &key,
        &recovery,
    )
    .unwrap();
    assert!(open_archive(&archive, &[0x53; 32]).is_err());
    let mut raw = fs::read(&archive).unwrap();
    *raw.last_mut().unwrap() ^= 1;
    fs::write(&archive, raw).unwrap();
    let destination = paths(&root.path().join("destination"));
    let mut saved = false;
    assert!(
        restore_archive(
            &destination,
            open_archive(&archive, &recovery).unwrap(),
            |_, _| {
                saved = true;
                Ok(())
            }
        )
        .is_err()
    );
    assert!(!saved);
    assert!(
        !destination
            .account_state_database("account-a", FrontendKind::Cli)
            .unwrap()
            .exists()
    );
}

#[test]
fn failed_key_install_is_recoverable_and_blocks_normal_opening_until_retry() {
    let (root, database, key, payload) = fixture();
    let archive = root.path().join("portable.axbackup");
    let recovery = [0x54; 32];
    create_archive(
        &database,
        "account-a",
        FrontendKind::Cli,
        &archive,
        &key,
        &recovery,
    )
    .unwrap();
    let destination = paths(&root.path().join("destination"));
    assert!(
        restore_archive(
            &destination,
            open_archive(&archive, &recovery).unwrap(),
            |_, _| {
                Err(crate::AxiomError::Storage(
                    "Synthetic locked credential store".into(),
                ))
            }
        )
        .is_err()
    );
    let target = destination
        .account_state_database("account-a", FrontendKind::Cli)
        .unwrap();
    assert!(local_storage::restore_marker_for_database(&target).exists());
    assert!(local_storage::open_database(&target).is_err());
    restore_archive(
        &destination,
        open_archive(&archive, &recovery).unwrap(),
        |_, _| Ok(()),
    )
    .unwrap();
    assert!(!local_storage::restore_marker_for_database(&target).exists());
    assert_eq!(
        SessionStore::open_protected_with_key(&target, key)
            .unwrap()
            .local_payload("draft")
            .unwrap(),
        Some(payload)
    );
}

#[test]
fn restore_retries_after_directory_activation_without_overwriting_data() {
    let (root, database, key, _) = fixture();
    let archive = root.path().join("portable.axbackup");
    let recovery = [0x55; 32];
    create_archive(
        &database,
        "account-a",
        FrontendKind::Cli,
        &archive,
        &key,
        &recovery,
    )
    .unwrap();
    let destination = paths(&root.path().join("destination"));
    restore_archive(
        &destination,
        open_archive(&archive, &recovery).unwrap(),
        |_, _| Ok(()),
    )
    .unwrap();
    let target = destination
        .account_state_database("account-a", FrontendKind::Cli)
        .unwrap();
    let identity = open_archive(&archive, &recovery).unwrap().identity;
    let opened = local_storage::open_database_with_key(&target, key.clone()).unwrap();
    let activation = opened
        .connection
        .query_row(
            "SELECT activation FROM native_restore_identity WHERE id=1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    drop(opened);
    let intent = RestoreIntent {
        version: 1,
        archive: identity,
        activation,
    };
    fs::write(
        local_storage::restore_marker_for_database(&target),
        serde_json::to_vec(&intent).unwrap(),
    )
    .unwrap();
    assert!(local_storage::open_database(&target).is_err());
    restore_archive(
        &destination,
        open_archive(&archive, &recovery).unwrap(),
        |_, _| Ok(()),
    )
    .unwrap();
    assert!(!local_storage::restore_marker_for_database(&target).exists());
    SessionStore::open_protected_with_key(&target, key)
        .unwrap()
        .verify_asset_integrity()
        .unwrap();
}

#[test]
fn pending_restore_rejects_unrelated_plaintext_created_by_an_older_process() {
    let (root, database, key, _) = fixture();
    let archive = root.path().join("portable.axbackup");
    let recovery = [0x56; 32];
    create_archive(
        &database,
        "account-a",
        FrontendKind::Cli,
        &archive,
        &key,
        &recovery,
    )
    .unwrap();
    let destination = paths(&root.path().join("destination"));
    assert!(
        restore_archive(
            &destination,
            open_archive(&archive, &recovery).unwrap(),
            |_, _| { Err(crate::AxiomError::Storage("Synthetic key failure".into())) }
        )
        .is_err()
    );
    let target = destination
        .account_state_database("account-a", FrontendKind::Cli)
        .unwrap();
    let other = rusqlite::Connection::open(&target).unwrap();
    other.execute_batch("CREATE TABLE unrelated(value TEXT); INSERT INTO unrelated VALUES('preserve this older process');").unwrap();
    drop(other);
    let before = fs::read(&target).unwrap();
    let mut installed = false;
    assert!(
        restore_archive(
            &destination,
            open_archive(&archive, &recovery).unwrap(),
            |_, _| {
                installed = true;
                Ok(())
            }
        )
        .is_err()
    );
    assert!(!installed);
    assert_eq!(fs::read(&target).unwrap(), before);
    assert!(local_storage::restore_marker_for_database(&target).exists());
}

#[test]
fn key_only_recovery_never_opens_or_changes_the_chat_store_or_assets() {
    let (root, database, key, _) = fixture();
    for entry in fs::read_dir(asset_directory_for_database(&database)).unwrap() {
        fs::remove_file(entry.unwrap().path()).unwrap();
    }
    let before = fs::read(&database).unwrap();
    let mut installed = false;
    recover_existing_key(&database, &key, |path, recovered| {
        assert_eq!(path, database);
        let cipher =
            local_storage::encrypt_asset(recovered, "fixture", b"synthetic secret").unwrap();
        assert_eq!(
            local_storage::decrypt_asset(&key, "fixture", &cipher).unwrap(),
            b"synthetic secret"
        );
        installed = true;
        Ok(())
    })
    .unwrap();
    assert!(installed);
    assert_eq!(fs::read(&database).unwrap(), before);
    assert_eq!(
        fs::read_dir(asset_directory_for_database(&database))
            .unwrap()
            .count(),
        0
    );
    drop(root);
}

#[test]
fn key_only_recovery_rejects_empty_and_plaintext_files_before_installing_a_key() {
    let root = tempfile::tempdir().unwrap();
    let key = StorageKey::from_bytes([0x76; 32]);
    let database = root.path().join("state.sqlite3");
    for bytes in [
        b"".as_slice(),
        b"SQLite format 3\0unrelated existing data".as_slice(),
    ] {
        fs::write(&database, bytes).unwrap();
        let mut installed = false;
        assert!(
            recover_existing_key(&database, &key, |_, _| {
                installed = true;
                Ok(())
            })
            .is_err()
        );
        assert!(!installed);
        assert_eq!(fs::read(&database).unwrap(), bytes);
    }
}

#[test]
fn malformed_or_unsafe_archive_entries_are_rejected() {
    for path in [
        "../outside",
        "assets/../outside",
        "assets//file",
        "assets/./file",
        "assets/C:/file",
        "assets/file\\outside",
        "assets/con",
        "assets/nul.png",
        "assets/name.",
    ] {
        assert!(validate_entry_path(path).is_err(), "accepted {path}");
    }
    assert!(validate_entry_path("database").is_ok());
    assert!(validate_entry_path("assets/91fc2e43-1d97-450f-b552-2c58ef67ce18").is_ok());
}
