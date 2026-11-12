//! Synthetic stores only: no user database or operating-system keyring access.
use super::{SessionStore, asset_directory_for_database};
use crate::{
    app::{AppEvent, PermissionProfile, SessionId, TurnId},
    local_storage::StorageKey,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use std::{path::Path, sync::Arc};
use tempfile::tempdir;

const ORIGINAL: &[u8] = b"\x89PNG\r\n\x1a\nsynthetic-original-asset-unique-sentinel";
const PREVIEW: &[u8] = b"\x89PNG\r\n\x1a\nsynthetic-exact-preview-unique-sentinel";
const FILE: &[u8] = b"%PDF-1.7\nsynthetic-file-asset-unique-sentinel";
fn key() -> Arc<StorageKey> {
    Arc::new(StorageKey::from_bytes([0x71; 32]))
}

#[test]
fn migration_reports_phases_and_exact_counts_only_for_existing_upgrades() {
    use crate::storage_progress::Phase;
    use std::cell::RefCell;
    let root = tempdir().unwrap();
    let path = root.path().join("state.sqlite3");
    let _legacy = legacy(&path, 6);
    let updates = RefCell::new(Vec::new());
    let observe = |progress| updates.borrow_mut().push(progress);
    let migrated =
        SessionStore::open_protected_with_key_and_progress(&path, key(), &observe).unwrap();
    let progress = updates.take();
    assert_eq!(progress.first().unwrap().phase, Phase::Encrypting);
    assert_eq!(progress.last().unwrap().phase, Phase::Finalizing);
    let mut previous_phase = 0;
    for update in &progress {
        let phase = match update.phase {
            Phase::Encrypting => 0,
            Phase::MovingMedia => 1,
            Phase::Checking => 2,
            Phase::Finalizing => 3,
            Phase::Failed => panic!("successful migration cannot report failure"),
        };
        assert!(phase >= previous_phase);
        previous_phase = phase;
        if let (Some(completed), Some(total)) = (update.completed, update.total) {
            assert!(completed <= total);
        }
    }
    for phase in [Phase::MovingMedia, Phase::Checking] {
        let counts: Vec<_> = progress.iter().filter(|p| p.phase == phase).collect();
        assert_eq!(counts.first().unwrap().completed, Some(0));
        let last = counts.last().unwrap();
        assert_eq!(last.completed, last.total);
        assert!(last.total.unwrap() > 0);
        assert!(
            counts
                .windows(2)
                .all(|pair| pair[0].completed <= pair[1].completed)
        );
    }
    assert_no_embedded_bytes(&migrated);
    drop(migrated);
    drop(SessionStore::open_protected_with_key_and_progress(&path, key(), &observe).unwrap());
    assert!(
        updates.borrow().is_empty(),
        "current stores must not claim to migrate again"
    );
    let fresh = root.path().join("new.sqlite3");
    drop(SessionStore::open_protected_with_key_and_progress(&fresh, key(), &observe).unwrap());
    assert!(
        updates.borrow().is_empty(),
        "a new account has nothing to migrate"
    );
}
fn image(bytes: &[u8]) -> Value {
    json!({"mimeType":"image/png","data":STANDARD.encode(bytes)})
}
fn attachment() -> Value {
    json!({"kind":"image","name":"upload.png","image":image(ORIGINAL)})
}
fn payload() -> Value {
    json!({"version":1,"kind":"prompt","payload":{"text":"saved draft","attachments":[attachment()]}})
}
fn generated() -> axiom_secure_client::GeneratedImage {
    serde_json::from_value(json!({
        "requestId":"11".repeat(16),"data":STANDARD.encode(ORIGINAL),"mimeType":"image/png","width":1024,"height":1024,
        "sha256":hex::encode(Sha256::digest(ORIGINAL)),"sourceImageIds":["img_parent_b","img_parent_a"],
        "evidence":{"state":"verified","provider_id":"axiom-direct","model_id":"direct:qwen3.8-27b","attestation_protocol":"synthetic-unit-fixture","e2ee_protocol":"synthetic-unit-fixture","e2ee_encryption_version":1,"trust_policy_version":"unit-test","verified_at_unix_seconds":1,"model_key_fingerprint":"00".repeat(32),"tls_spki_fingerprint":null,"checks":[],"provider_claims":[]},
        "usage":{"requestId":"11".repeat(16),"providerId":"axiom-direct","modelId":"direct:qwen3.8-27b","settled":true,"responseVerified":true}
    })).unwrap()
}
struct Legacy {
    id: SessionId,
    turn: TurnId,
    attachments: Value,
    messages: Value,
    image: axiom_secure_client::GeneratedImage,
    digests: Vec<(i64, String)>,
}
fn legacy(path: &Path, version: i64) -> Legacy {
    let connection = Connection::open(path).unwrap();
    super::schema::initialize_schema(&connection, 0).unwrap();
    let id = SessionId::new();
    let turn = TurnId::new();
    let time = "2026-11-11T00:00:00Z";
    let (encoding, cwd) = super::codec::encode_path(Path::new("/tmp")).unwrap();
    connection.execute("INSERT INTO threads(id,cwd,cwd_encoding,origin,profile,created_at,updated_at,next_timeline_sequence) VALUES(?1,?2,?5,'test',?3,?4,?4,2)",params![id.to_string(),cwd,PermissionProfile::Confirm.to_string(),time,encoding]).unwrap();
    connection.execute("INSERT INTO turns(thread_id,id,status,user_item_id,started_at,completed_at) VALUES(?1,?2,'completed','user',?3,?3)",params![id.to_string(),turn.to_string(),time]).unwrap();
    let file = json!({"name":"doc.pdf","mimeType":"application/pdf","data":STANDARD.encode(FILE)});
    let attachments = json!([attachment(),{"kind":"file","name":"doc.pdf","file":file}]);
    connection.execute("INSERT INTO timeline_items(id,thread_id,turn_id,sequence,kind,status,content,metadata,created_at,updated_at) VALUES('user',?1,?2,1,'user_message','completed','look',?3,?4,?4)",params![id.to_string(),turn.to_string(),json!({"attachments":[{"kind":"image","name":"upload.png"}]}).to_string(),time]).unwrap();
    connection
        .execute(
            "INSERT INTO prompt_attachments(user_item_id,payload) VALUES('user',?1)",
            [attachments.to_string()],
        )
        .unwrap();
    let generated = generated();
    let mut proof = serde_json::to_value(&generated).unwrap();
    proof.as_object_mut().unwrap().remove("data");
    let metadata = json!({"name":"generate","arguments":{"prompt":"exact arguments remain"},"success":true,"generated_image":proof,"content":[{"type":"content","content":{"type":"image","mimeType":"image/png","data":STANDARD.encode(ORIGINAL)}}]});
    connection.execute("INSERT INTO timeline_items(id,thread_id,turn_id,sequence,kind,status,external_id,metadata,created_at,updated_at) VALUES('tool',?1,?2,2,'tool_call','completed','call',?3,?4,?4)",params![id.to_string(),turn.to_string(),metadata.to_string(),time]).unwrap();
    let messages = json!([
        {"role":"user","content":"look","images":[image(ORIGINAL)],"files":[file]},
        {"role":"tool","content":"preview","tool_call_id":"view","images":[image(PREVIEW)]}
    ]);
    let mut digests = Vec::new();
    if version == 6 {
        connection
            .execute(
                "INSERT INTO model_history_threads(thread_id,version) VALUES(?1,1)",
                [id.to_string()],
            )
            .unwrap();
        for (index, message) in messages.as_array().unwrap().iter().enumerate() {
            let message_id = 41 + i64::try_from(index).unwrap();
            let digest = hex::encode(Sha256::digest(message.to_string().as_bytes()));
            connection
                .execute(
                    "INSERT INTO model_messages(id,thread_id,digest,payload) VALUES(?1,?2,?3,?4)",
                    params![message_id, id.to_string(), digest, message.to_string()],
                )
                .unwrap();
            digests.push((message_id, digest));
        }
        for event in [
            json!({"kind":"prompt_accepted","turn_id":turn,"text":"look","attachments":attachments}),
            json!({"kind":"image_generated","call_id":"call","image":generated}),
            json!({"kind":"model_history_checkpoint","version":1,"messages":[41,42],"_axiom_message_refs":true}),
        ] {
            connection.execute("INSERT INTO model_history(thread_id,version,turn_id,occurred_at,origin,payload) VALUES(?1,1,?2,?3,'test',?4)",params![id.to_string(),turn.to_string(),time,event.to_string()]).unwrap();
        }
    }
    connection.execute_batch("DROP TABLE asset_maintenance; DROP TABLE local_payloads; DROP TABLE asset_owners; DROP TABLE binary_assets; PRAGMA user_version=6;").unwrap();
    if version == 2 {
        connection.execute_batch("DROP TABLE model_messages; DROP TABLE model_history; DROP TABLE model_history_threads; ALTER TABLE turns DROP COLUMN owner_id; DROP TABLE desktop_mcp_threads; DROP TABLE desktop_mcp_state; ALTER TABLE threads DROP COLUMN title_generation_id; PRAGMA user_version=2;").unwrap();
    }
    Legacy {
        id,
        turn,
        attachments,
        messages,
        image: generated,
        digests,
    }
}
fn assert_no_embedded_bytes(store: &SessionStore) {
    let connection = store.lock().unwrap();
    for sql in [
        "SELECT payload FROM prompt_attachments",
        "SELECT metadata FROM timeline_items",
        "SELECT payload FROM model_history",
        "SELECT payload FROM model_messages",
        "SELECT payload FROM local_payloads",
    ] {
        let mut stmt = connection.prepare(sql).unwrap();
        let rows = stmt.query_map([], |r| r.get::<_, String>(0)).unwrap();
        for row in rows {
            let text = row.unwrap();
            for bytes in [ORIGINAL, PREVIEW, FILE] {
                assert!(
                    !text.contains(&STANDARD.encode(bytes)),
                    "inline data persisted"
                );
                assert!(
                    !text.contains(&serde_json::to_string(bytes).unwrap()),
                    "byte array persisted"
                );
            }
        }
    }
    let data_columns: i64 = connection
        .query_row(
            "SELECT count(*) FROM pragma_table_info('binary_assets') WHERE upper(type)='BLOB'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(data_columns, 0);
}
#[test]
fn legacy_v2_and_v6_externalize_all_known_slots_and_replay_exactly() {
    for version in [2, 6] {
        let root = tempdir().unwrap();
        let path = root.path().join("state.sqlite3");
        let legacy = legacy(&path, version);
        let store = SessionStore::open_protected_with_key(&path, key()).unwrap();
        assert_no_embedded_bytes(&store);
        assert_eq!(
            serde_json::to_value(store.prompt_attachments(&legacy.id, "user").unwrap()).unwrap(),
            legacy.attachments
        );
        assert_eq!(
            store
                .generated_image(&legacy.id, "call", &legacy.image.sha256)
                .unwrap(),
            legacy.image
        );
        let original_catalog = crate::image_tool::ImageCatalog::default();
        let restored_catalog = crate::image_tool::ImageCatalog::default();
        original_catalog
            .add_uploads(
                &legacy.id.to_string(),
                &legacy.turn.to_string(),
                &serde_json::from_value::<Vec<axiom_inference::PromptAttachment>>(
                    legacy.attachments.clone(),
                )
                .unwrap(),
            )
            .unwrap();
        restored_catalog
            .add_uploads(
                &legacy.id.to_string(),
                &legacy.turn.to_string(),
                &store.prompt_attachments(&legacy.id, "user").unwrap(),
            )
            .unwrap();
        assert_eq!(
            original_catalog.manifest().unwrap(),
            restored_catalog.manifest().unwrap()
        );
        assert_eq!(
            restored_catalog.add_generated(&legacy.image).unwrap(),
            format!("img_{}", legacy.image.request_id)
        );
        let events = store.load(&legacy.id).unwrap();
        assert!(events.iter().any(|e|matches!(&e.event,AppEvent::PromptAccepted{attachments,..} if serde_json::to_value(attachments).unwrap()==legacy.attachments)));
        if version == 6 {
            assert!(events.iter().any(|e|matches!(&e.event,AppEvent::ModelHistoryCheckpoint{messages,..} if messages==&legacy.messages)));
            let connection = store.lock().unwrap();
            for (id, digest) in &legacy.digests {
                assert_eq!(
                    connection
                        .query_row("SELECT digest FROM model_messages WHERE id=?1", [id], |r| r
                            .get::<_, String>(0))
                        .unwrap(),
                    *digest
                );
            }
        }
        let count: i64 = store
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM binary_assets", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, if version == 6 { 3 } else { 2 });
        let export = store.export(&legacy.id).unwrap();
        assert!(export.contains(&STANDARD.encode(ORIGINAL)));
        for entry in std::fs::read_dir(asset_directory_for_database(&path)).unwrap() {
            let entry = entry.unwrap();
            assert!(uuid::Uuid::parse_str(&entry.file_name().to_string_lossy()).is_ok());
            let ciphertext = std::fs::read(entry.path()).unwrap();
            assert!(ciphertext.starts_with(b"AXAST001"));
            for bytes in [ORIGINAL, PREVIEW, FILE] {
                assert!(!ciphertext.windows(bytes.len()).any(|w| w == bytes));
            }
        }
        drop(store);
        let reopened = SessionStore::open_protected_with_key(&path, key()).unwrap();
        assert_eq!(
            reopened
                .load(&legacy.id)
                .unwrap()
                .iter()
                .map(|e| &e.event)
                .collect::<Vec<_>>(),
            events.iter().map(|e| &e.event).collect::<Vec<_>>()
        );
        assert_no_embedded_bytes(&reopened);
    }
}

#[test]
fn legacy_v6_recovers_unrelated_broken_metadata_but_rejects_invalid_image_data() {
    for invalid_image in [false, true] {
        let root = tempdir().unwrap();
        let path = root.path().join("state.sqlite3");
        let legacy = legacy(&path, 6);
        let raw = Connection::open(&path).unwrap();
        raw.execute("INSERT INTO timeline_items(id,thread_id,turn_id,sequence,kind,status,content,metadata,created_at,updated_at) SELECT 'recoverable',thread_id,turn_id,3,'assistant_message','completed','recoverable assistant text','broken json',created_at,updated_at FROM timeline_items WHERE id='user'", []).unwrap();
        if invalid_image {
            raw.execute("UPDATE timeline_items SET metadata=json_set(metadata,'$.content[0].content.data','!invalid!') WHERE id='tool'", []).unwrap();
        }
        drop(raw);
        let result = SessionStore::open_protected_with_key(&path, key());
        if invalid_image {
            let error = result.expect_err("recognized invalid image must fail closed");
            assert!(error.to_string().contains("base64"), "{error}");
            let opened = crate::local_storage::open_database_with_key(&path, key()).unwrap();
            assert_eq!(
                opened
                    .connection
                    .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                    .unwrap(),
                6
            );
            assert_eq!(
                opened
                    .connection
                    .query_row(
                        "SELECT payload FROM prompt_attachments WHERE user_item_id='user'",
                        [],
                        |r| r.get::<_, String>(0)
                    )
                    .unwrap(),
                legacy.attachments.to_string()
            );
            continue;
        }
        let store = result.unwrap();
        let page = store.thread_page(&legacy.id, None, None, 100).unwrap();
        let recovered = page
            .items
            .iter()
            .find(|item| item.id == "recoverable")
            .unwrap();
        assert_eq!(recovered.content, "recoverable assistant text");
        assert!(recovered.metadata["recovery_warning"].is_string());
        let events = store.load(&legacy.id).unwrap();
        assert!(events.iter().any(|e| matches!(&e.event, AppEvent::PromptAccepted { attachments, .. } if serde_json::to_value(attachments).unwrap() == legacy.attachments)));
        assert!(events.iter().any(|e| matches!(&e.event, AppEvent::ImageGenerated { image, .. } if image.as_ref() == &legacy.image)));
        assert!(events.iter().any(|e| matches!(&e.event, AppEvent::ModelHistoryCheckpoint { messages, .. } if messages == &legacy.messages)));
        {
            let connection = store.lock().unwrap();
            for (id, digest) in &legacy.digests {
                assert_eq!(
                    connection
                        .query_row("SELECT digest FROM model_messages WHERE id=?1", [id], |r| r
                            .get::<_, String>(0))
                        .unwrap(),
                    *digest
                );
            }
            let checkpoint: String = connection
                .query_row(
                    "SELECT payload FROM model_history WHERE sequence=3",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            let checkpoint: Value = serde_json::from_str(&checkpoint).unwrap();
            assert_eq!(checkpoint["messages"], json!([41, 42]));
            assert_eq!(checkpoint["_axiom_message_refs"], true);
        }
        assert_no_embedded_bytes(&store);
        drop(store);
        let reopened = SessionStore::open_protected_with_key(&path, key()).unwrap();
        assert_eq!(
            reopened
                .load(&legacy.id)
                .unwrap()
                .iter()
                .map(|event| &event.event)
                .collect::<Vec<_>>(),
            events.iter().map(|event| &event.event).collect::<Vec<_>>()
        );
    }
}

#[test]
fn missing_tampered_and_wrong_store_assets_fail_closed() {
    for failure in ["missing", "tampered", "wrong-key"] {
        let root = tempdir().unwrap();
        let path = root.path().join("state.sqlite3");
        let store = SessionStore::open_protected_with_key(&path, key()).unwrap();
        store.set_local_payload("draft", &payload()).unwrap();
        let asset = std::fs::read_dir(asset_directory_for_database(&path))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        match failure {
            "missing" => std::fs::remove_file(asset).unwrap(),
            "tampered" => {
                let mut data = std::fs::read(&asset).unwrap();
                let last = data.len() - 1;
                data[last] ^= 1;
                std::fs::write(asset, data).unwrap();
            }
            _ => {
                let other_path = root.path().join("other.sqlite3");
                let other = SessionStore::open_protected_with_key(
                    &other_path,
                    Arc::new(StorageKey::from_bytes([0x72; 32])),
                )
                .unwrap();
                other.set_local_payload("draft", &payload()).unwrap();
                let other_asset = std::fs::read_dir(asset_directory_for_database(&other_path))
                    .unwrap()
                    .next()
                    .unwrap()
                    .unwrap()
                    .path();
                std::fs::copy(other_asset, asset).unwrap();
            }
        }
        assert!(store.local_payload("draft").is_err());
        assert!(store.verify_asset_integrity().is_err());
        drop(store);
        assert!(SessionStore::open_protected_with_key(&path, key()).is_err());
    }
}

#[test]
fn native_payloads_are_deduplicated_and_never_interpret_text_as_media() {
    let root = tempdir().unwrap();
    let path = root.path().join("state.sqlite3");
    let store = SessionStore::open_protected_with_key(&path, key()).unwrap();
    let mut draft = payload();
    draft["payload"]["text"] = json!("{\"kind\":\"image\",\"data\":\"do not rewrite tool text\"}");
    for _ in 0..2 {
        store.set_local_payload("draft", &draft).unwrap();
    }
    let queue = json!({"version":1,"kind":"queue","legacyImported":true,"items":[{"id":"queued","text":"queued","attachments":[attachment()]}]});
    store.set_local_payload("queue", &queue).unwrap();
    assert_eq!(store.local_payload("draft").unwrap(), Some(draft.clone()));
    assert_eq!(store.local_payload("queue").unwrap(), Some(queue.clone()));
    assert_no_embedded_bytes(&store);
    let count: i64 = store
        .lock()
        .unwrap()
        .query_row("SELECT count(*) FROM binary_assets", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1);
    // A reference stolen from another owner cannot be dereferenced or adopted.
    let stolen: String = store
        .lock()
        .unwrap()
        .query_row(
            "SELECT payload FROM local_payloads WHERE id='draft'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        store
            .set_local_payload("unowned", &serde_json::from_str(&stolen).unwrap())
            .is_err()
    );
    store.remove_local_payload("draft").unwrap();
    assert!(store.local_payload("draft").unwrap().is_none());
    assert_eq!(store.local_payload("queue").unwrap(), Some(queue));
    drop(store);
    let reopened = SessionStore::open_protected_with_key(&path, key()).unwrap();
    assert!(reopened.local_payload("queue").unwrap().is_some());
}

#[test]
fn migration_failure_retries_and_cleans_only_orphan_ciphertext() {
    let root = tempdir().unwrap();
    let path = root.path().join("state.sqlite3");
    let legacy = legacy(&path, 6);
    // First attachment is valid and gets staged; the second forces rollback.
    let raw = Connection::open(&path).unwrap();
    let mut malformed = legacy.attachments.clone();
    malformed[1]["file"]["data"] = json!("!invalid!");
    raw.execute(
        "UPDATE prompt_attachments SET payload=?1",
        [malformed.to_string()],
    )
    .unwrap();
    drop(raw);
    assert!(SessionStore::open_protected_with_key(&path, key()).is_err());
    let opened = crate::local_storage::open_database_with_key(&path, key()).unwrap();
    assert_eq!(
        opened
            .connection
            .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        6
    );
    opened
        .connection
        .execute(
            "UPDATE prompt_attachments SET payload=?1",
            [legacy.attachments.to_string()],
        )
        .unwrap();
    drop(opened);
    let store = SessionStore::open_protected_with_key(&path, key()).unwrap();
    assert_no_embedded_bytes(&store);
    assert_eq!(
        std::fs::read_dir(asset_directory_for_database(&path))
            .unwrap()
            .count(),
        3
    );
}

#[test]
fn replacement_and_deletion_collect_only_unowned_assets_after_commit() {
    let root = tempdir().unwrap();
    let path = root.path().join("state.sqlite3");
    let legacy = legacy(&path, 6);
    let store = SessionStore::open_protected_with_key(&path, key()).unwrap();
    store.set_local_payload("draft", &payload()).unwrap();
    let mut invalid = payload();
    invalid["payload"]["attachments"][0]["image"]["data"] = json!("invalid!");
    assert!(store.set_local_payload("draft", &invalid).is_err());
    assert_eq!(store.local_payload("draft").unwrap(), Some(payload()));
    let empty = json!({"version":1,"kind":"prompt","payload":{"text":"","attachments":[]}});
    store.set_local_payload("draft", &empty).unwrap();
    // Removing a draft must retain every original/preview used by history.
    assert_eq!(
        std::fs::read_dir(asset_directory_for_database(&path))
            .unwrap()
            .count(),
        3
    );
    store.set_local_payload("draft", &payload()).unwrap();
    store.delete_sessions(&[legacy.id]).unwrap();
    // Only the original is still owned by the draft.
    assert_eq!(
        std::fs::read_dir(asset_directory_for_database(&path))
            .unwrap()
            .count(),
        1
    );
    store.set_local_payload("draft", &empty).unwrap();
    assert_eq!(
        std::fs::read_dir(asset_directory_for_database(&path))
            .unwrap()
            .count(),
        0
    );
    store.verify_asset_integrity().unwrap();
}

#[test]
fn pending_physical_migration_cleanup_retries_on_reopen() {
    let root = tempdir().unwrap();
    let path = root.path().join("state.sqlite3");
    let store = SessionStore::open_protected_with_key(&path, key()).unwrap();
    store.set_local_payload("draft", &payload()).unwrap();
    store
        .lock()
        .unwrap()
        .execute("UPDATE asset_maintenance SET purge_pending=1", [])
        .unwrap();
    drop(store);
    let store = SessionStore::open_protected_with_key(&path, key()).unwrap();
    let pending: bool = store
        .lock()
        .unwrap()
        .query_row("SELECT purge_pending FROM asset_maintenance", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert!(!pending);
    assert_eq!(store.local_payload("draft").unwrap(), Some(payload()));
}
