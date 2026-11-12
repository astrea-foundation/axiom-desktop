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

const ORIGINAL: &[u8] = include_bytes!("fixtures/storage-original.png");
const PREVIEW: &[u8] = include_bytes!("fixtures/storage-preview.png");
const FILE: &[u8] = b"%PDF-1.7\nsynthetic-file-asset-unique-sentinel";
fn key() -> Arc<StorageKey> {
    Arc::new(StorageKey::from_bytes([0x71; 32]))
}
fn damaged_images() -> [(&'static str, &'static [u8], &'static str); 4] {
    [
        ("signature", b"\x89PNG\r\n\x1a\n", "image/png"),
        (
            "truncated",
            include_bytes!("fixtures/storage-truncated.png"),
            "image/png",
        ),
        (
            "bounds",
            include_bytes!("fixtures/storage-too-wide.png"),
            "image/png",
        ),
        ("mime", ORIGINAL, "image/jpeg"),
    ]
}

#[derive(Default)]
struct DecodingProvider {
    requests: tokio::sync::Mutex<Vec<axiom_inference::InferenceRequest>>,
}
#[async_trait::async_trait]
impl crate::provider::InferenceProvider for DecodingProvider {
    async fn models(
        &self,
        _: tokio_util::sync::CancellationToken,
    ) -> crate::Result<Vec<axiom_inference::ModelInfo>> {
        Ok(vec![axiom_inference::ModelInfo {
            id: "storage-vision-fixture".into(),
            supports_images: true,
            context_window_tokens: 65_536,
            max_output_tokens: 4096,
            ..Default::default()
        }])
    }
    async fn stream(
        &self,
        request: axiom_inference::InferenceRequest,
        events: tokio::sync::mpsc::Sender<axiom_inference::ProviderEvent>,
        _: tokio_util::sync::CancellationToken,
    ) -> crate::Result<axiom_inference::AssistantTurn> {
        request
            .validate()
            .map_err(|error| crate::AxiomError::Provider(error.to_string()))?;
        for image in request.messages.iter().flat_map(|message| &message.images) {
            let raw = STANDARD.decode(&image.data).unwrap();
            let format = image::ImageFormat::from_mime_type(&image.mime_type).unwrap();
            let mut reader = image::ImageReader::with_format(std::io::Cursor::new(raw), format);
            let mut limits = image::Limits::default();
            limits.max_image_width = Some(8192);
            limits.max_image_height = Some(8192);
            limits.max_alloc = Some(128 * 1024 * 1024);
            reader.limits(limits);
            reader.decode().map_err(|error| {
                crate::AxiomError::Provider(format!(
                    "synthetic provider cannot decode saved image: {error}"
                ))
            })?;
        }
        self.requests.lock().await.push(request);
        events
            .send(axiom_inference::ProviderEvent::ResponseVerified)
            .await
            .unwrap();
        Ok(axiom_inference::AssistantTurn {
            text: "Synthetic usable continuation and summary.".into(),
            ..Default::default()
        })
    }
}

async fn continuation_and_compaction(
    id: &SessionId,
    history: &[crate::app::EventEnvelope],
) -> Vec<AppEvent> {
    use crate::agent::{AgentEngine, TurnContext, TurnRunner as _};
    use tokio_util::sync::CancellationToken;
    let provider = Arc::new(DecodingProvider::default());
    let engine = AgentEngine::new(
        provider.clone(),
        Arc::new(crate::tools::ToolRegistry::new()),
        "storage-vision-fixture",
        3,
    );
    engine.restore_session(id, history).await.unwrap();
    let (events, mut receiver) = tokio::sync::mpsc::channel(256);
    engine
        .run(
            TurnContext {
                session_id: id.clone(),
                turn_id: TurnId::new(),
                cwd: "/tmp".into(),
                permission_profile: PermissionProfile::Web,
                web_enabled: false,
                image_tools_available: false,
                attachments: vec![],
                mcp: None,
                steering: None,
                approval: None,
                questions: None,
            },
            "Continue using readable text".into(),
            events,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let mut updates = Vec::new();
    while let Ok(event) = receiver.try_recv() {
        if matches!(event, AppEvent::ModelHistoryCheckpoint { .. }) {
            updates.push(event);
        }
    }
    let (events, mut receiver) = tokio::sync::mpsc::channel(256);
    engine
        .compact(id, None, events, CancellationToken::new())
        .await
        .unwrap();
    let requests = provider.requests.lock().await;
    assert!(
        requests.len() >= 2,
        "continuation and compaction both reached the synthetic provider"
    );
    assert_eq!(
        requests[0]
            .messages
            .iter()
            .map(|message| message.images.len())
            .sum::<usize>(),
        1
    );
    assert!(
        requests
            .iter()
            .flat_map(|request| &request.messages)
            .flat_map(|message| &message.images)
            .all(|image| image.data == STANDARD.encode(PREVIEW)),
        "healthy original image bytes remain exact"
    );
    while let Ok(event) = receiver.try_recv() {
        if matches!(
            event,
            AppEvent::ModelHistoryCheckpoint { .. } | AppEvent::ContextCompacted { .. }
        ) {
            updates.push(event);
        }
    }
    updates
}

#[tokio::test]
async fn authenticated_external_bad_images_are_omitted_before_continuation_and_compaction() {
    // Header dimensions alone are insufficient: this PNG has a real IHDR and
    // IDAT header but its pixel stream is truncated.
    let truncated = include_bytes!("fixtures/storage-truncated.png");
    assert_eq!(&truncated[16..24], &ORIGINAL[16..24]);
    for (case, raw, mime) in damaged_images() {
        let root = tempdir().unwrap();
        let path = root.path().join("state.sqlite3");
        let fixture = legacy(&path, 6);
        let store = SessionStore::open_protected_with_key(&path, key()).unwrap();
        append(
            &store,
            &fixture.id,
            AppEvent::ModelHistoryCheckpoint {
                version: 1,
                messages: json!([{"role":"system","content":"system"},{"role":"user","content":"Preserve readable text","images":[image(ORIGINAL),image(PREVIEW)]}]),
            },
        );
        let owner = super::assets::thread_owner(&fixture.id.to_string());
        let (bad_path, saved_payload, saved_digest) = {
            let mut connection = store.lock().unwrap();
            let assets = connection.assets().clone();
            let transaction = connection.transaction().unwrap();
            // Simulate an older schema-7 writer: cryptographically valid stored
            // bytes, which had only been checked for MIME magic at that time.
            let bad_ref = serde_json::to_value(
                assets
                    .intern(&transaction, &owner, &STANDARD.encode(raw))
                    .unwrap(),
            )
            .unwrap();
            let preview = serde_json::to_value(
                assets
                    .intern(&transaction, &owner, &STANDARD.encode(PREVIEW))
                    .unwrap(),
            )
            .unwrap();
            let bad_image = json!({"mimeType":mime,"_axiom_asset":bad_ref});
            let good_image = json!({"mimeType":"image/png","_axiom_asset":preview});
            let attachments = json!([{"kind":"image","name":"damaged.png","image":bad_image},{"kind":"image","name":"surviving.png","image":good_image}]);
            transaction
                .execute(
                    "UPDATE prompt_attachments SET payload=?1",
                    [attachments.to_string()],
                )
                .unwrap();
            transaction.execute("UPDATE model_history SET payload=json_set(payload,'$.attachments',json(?1)) WHERE json_extract(payload,'$.kind')='prompt_accepted'",[attachments.to_string()]).unwrap();
            transaction.execute("UPDATE model_history SET payload=json_set(payload,'$.image.mimeType',?1,'$.image._axiom_asset',json(?2)) WHERE json_extract(payload,'$.kind')='image_generated'",params![mime,bad_ref.to_string()]).unwrap();
            let mut block = bad_image.clone();
            block["type"] = json!("image");
            transaction.execute("UPDATE timeline_items SET metadata=json_set(metadata,'$.content[0].content',json(?1)) WHERE id='tool'",[block.to_string()]).unwrap();
            let mut message = json!({"role":"user","content":"Preserve readable text","images":[bad_image,good_image]});
            let stored = message.to_string();
            message["images"] =
                json!([{"mimeType":mime,"data":STANDARD.encode(raw)},image(PREVIEW)]);
            let digest = hex::encode(Sha256::digest(message.to_string().as_bytes()));
            transaction.execute("UPDATE model_messages SET payload=?1,digest=?2 WHERE json_extract(payload,'$.content')='Preserve readable text'",params![stored,digest]).unwrap();
            let draft_owner = super::assets::payload_owner("previous-draft");
            assets
                .intern(&transaction, &draft_owner, &STANDARD.encode(raw))
                .unwrap();
            let mut draft = payload();
            draft["payload"]["attachments"][0]["image"] = bad_image;
            transaction
                .execute(
                    "INSERT INTO local_payloads(id,payload) VALUES('previous-draft',?1)",
                    [draft.to_string()],
                )
                .unwrap();
            transaction.commit().unwrap();
            (
                asset_directory_for_database(&path).join(bad_ref["id"].as_str().unwrap()),
                stored,
                digest,
            )
        };
        let ciphertext = std::fs::read(&bad_path).unwrap();
        store.verify_asset_integrity().unwrap();
        drop(store);
        let store = SessionStore::open_protected_with_key(&path, key()).unwrap();
        let outcome = store.load_recovering(&fixture.id).unwrap();
        assert!(!outcome.warnings.is_empty(), "{case}");
        assert!(
            !outcome
                .events
                .iter()
                .any(|event| matches!(event.event, AppEvent::ImageGenerated { .. })),
            "{case}"
        );
        let attachments = store.prompt_attachments(&fixture.id, "user").unwrap();
        assert_eq!(
            serde_json::to_value(attachments).unwrap(),
            json!([{"kind":"image","name":"surviving.png","image":image(PREVIEW)}]),
            "{case}"
        );
        let draft = store.local_payload("previous-draft").unwrap().unwrap();
        assert_eq!(draft["payload"]["text"], "saved draft");
        assert_eq!(draft["payload"]["attachments"], json!([]));
        assert!(
            store
                .generated_image(&fixture.id, "call", &fixture.image.sha256)
                .is_err()
        );
        let updates = continuation_and_compaction(&fixture.id, &outcome.events).await;
        let connection = store.lock().unwrap();
        let unchanged:(String,String) = connection.query_row("SELECT payload,digest FROM model_messages WHERE json_extract(payload,'$.content')='Preserve readable text'",[],|row| Ok((row.get(0)?,row.get(1)?))).unwrap();
        assert_eq!(
            unchanged,
            (saved_payload, saved_digest),
            "reads preserve original references/digest"
        );
        assert_eq!(
            std::fs::read(&bad_path).unwrap(),
            ciphertext,
            "reads retain original ciphertext"
        );
        drop(connection);
        let mut rejected = payload();
        rejected["payload"]["attachments"][0]["image"] =
            json!({"mimeType":mime,"data":STANDARD.encode(raw)});
        assert!(
            store.set_local_payload("new-damaged", &rejected).is_err(),
            "{case}"
        );
        for update in updates {
            append(&store, &fixture.id, update);
        }
        drop(store);
        let reopened = SessionStore::open_protected_with_key(&path, key()).unwrap();
        assert!(reopened.load(&fixture.id).is_ok());
        assert_eq!(std::fs::read(&bad_path).unwrap(), ciphertext);
    }
}

#[tokio::test]
async fn large_valid_generated_original_is_omitted_only_from_five_mib_input_slots() {
    let root = tempdir().unwrap();
    let path = root.path().join("state.sqlite3");
    let fixture = legacy(&path, 6);
    let mut large = ORIGINAL.to_vec();
    large.resize(6 * 1024 * 1024, 0);
    let data = STANDARD.encode(&large);
    let mut generated = fixture.image.clone();
    generated.data = data.clone();
    generated.sha256 = hex::encode(Sha256::digest(&large));
    let mut proof = serde_json::to_value(&generated).unwrap();
    proof.as_object_mut().unwrap().remove("data");
    let attachments = json!([{"kind":"image","name":"large-upload.png","image":image(&large)},
        {"kind":"image","name":"surviving.png","image":image(PREVIEW)},fixture.attachments[1]]);
    let message = json!({"role":"user","content":"Large input retains readable text","images":[image(&large),image(PREVIEW)]});
    let raw = Connection::open(&path).unwrap();
    raw.execute(
        "UPDATE prompt_attachments SET payload=?1",
        [attachments.to_string()],
    )
    .unwrap();
    raw.execute("UPDATE model_history SET payload=json_set(payload,'$.attachments',json(?1)) WHERE json_extract(payload,'$.kind')='prompt_accepted'",[attachments.to_string()]).unwrap();
    raw.execute("UPDATE model_history SET payload=json_set(payload,'$.image',json(?1)) WHERE json_extract(payload,'$.kind')='image_generated'",[serde_json::to_string(&generated).unwrap()]).unwrap();
    raw.execute("UPDATE timeline_items SET metadata=json_set(metadata,'$.generated_image',json(?1),'$.content[0].content.data',?2) WHERE id='tool'",params![proof.to_string(),data]).unwrap();
    raw.execute(
        "UPDATE model_messages SET payload=?1,digest=?2 WHERE id=41",
        params![
            message.to_string(),
            hex::encode(Sha256::digest(message.to_string().as_bytes()))
        ],
    )
    .unwrap();
    let system = json!({"role":"system","content":"system"});
    raw.execute(
        "UPDATE model_messages SET payload=?1,digest=?2 WHERE id=42",
        params![
            system.to_string(),
            hex::encode(Sha256::digest(system.to_string().as_bytes()))
        ],
    )
    .unwrap();
    raw.execute("UPDATE model_history SET payload=json_set(payload,'$.messages',json('[42,41]')) WHERE json_extract(payload,'$.kind')='model_history_checkpoint'",[]).unwrap();
    drop(raw);
    let store = SessionStore::open_protected_with_key(&path, key()).unwrap();
    assert!(
        recovery_records(&store, &fixture.id)
            .iter()
            .any(|record| record["sourceKind"] == 3)
    );
    for phase in ["migrated", "external"] {
        if phase == "external" {
            let connection = store.lock().unwrap();
            let metadata: String = connection
                .query_row(
                    "SELECT metadata FROM timeline_items WHERE id='tool'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            let metadata: Value = serde_json::from_str(&metadata).unwrap();
            let reference = metadata["content"][0]["content"]["_axiom_asset"].clone();
            let original: String = connection
                .query_row(
                    "SELECT payload FROM model_messages WHERE id=41",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            let mut external: Value = serde_json::from_str(&original).unwrap();
            external["images"]
                .as_array_mut()
                .unwrap()
                .insert(0, json!({"mimeType":"image/png","_axiom_asset":reference}));
            connection
                .execute(
                    "UPDATE model_messages SET payload=?1 WHERE id=41",
                    [external.to_string()],
                )
                .unwrap();
        }
        let outcome = store.load_recovering(&fixture.id).unwrap();
        let messages = outcome
            .events
            .iter()
            .find_map(|event| match &event.event {
                AppEvent::ModelHistoryCheckpoint { messages, .. } => Some(messages),
                _ => None,
            })
            .unwrap();
        assert_eq!(messages[1]["images"], json!([image(PREVIEW)]), "{phase}");
        assert_eq!(messages[1]["content"], "Large input retains readable text");
        assert_eq!(
            store
                .generated_image(&fixture.id, "call", &generated.sha256)
                .unwrap()
                .data,
            data,
            "10 MiB generated-original slot remains available"
        );
        let catalog =
            crate::image_tool::ImageCatalog::from_history(&fixture.id, &outcome.events).unwrap();
        assert_eq!(
            catalog
                .resolve(&format!("img_{}", generated.request_id))
                .unwrap()
                .image
                .data,
            data
        );
        continuation_and_compaction(&fixture.id, &outcome.events).await;
    }
    let mut draft = payload();
    draft["payload"]["attachments"][0]["image"] = image(&large);
    assert!(store.set_local_payload("oversized-upload", &draft).is_err());
    assert!(
        store
            .append(&crate::app::EventEnvelope {
                schema_version: 1,
                sequence: 1,
                turn_id: None,
                occurred_at: chrono::Utc::now(),
                correlation_id: crate::app::CorrelationId::new(),
                origin: crate::app::Origin::Test,
                session_id: fixture.id.clone(),
                event: AppEvent::ModelHistoryCheckpoint {
                    version: 1,
                    messages: json!([system, message])
                }
            })
            .is_err()
    );
    store.verify_asset_integrity().unwrap();
}
fn recovery_records(store: &SessionStore, id: &SessionId) -> Vec<Value> {
    let connection = store.lock().unwrap();
    let owner = super::assets::thread_owner(&id.to_string());
    let mut statement = connection.prepare("SELECT a.id,a.sha256,a.byte_length FROM binary_assets a JOIN asset_owners o ON o.asset_id=a.id WHERE o.owner=?1").unwrap();
    let references = statement.query_map([&owner], |row| Ok(json!({"version":1,"id":row.get::<_,String>(0)?,"sha256":row.get::<_,String>(1)?,"bytes":row.get::<_,i64>(2)?}))).unwrap();
    references
        .filter_map(|reference| {
            let reference = serde_json::from_value(reference.unwrap()).unwrap();
            let encoded = connection
                .assets()
                .resolve(&connection, &owner, &reference)
                .unwrap();
            let value: Value = serde_json::from_slice(&STANDARD.decode(encoded).unwrap()).ok()?;
            (value["kind"] == "legacy_media_recovery").then_some(value)
        })
        .collect()
}
fn append(store: &SessionStore, id: &SessionId, event: AppEvent) {
    store
        .append(&crate::app::EventEnvelope {
            schema_version: 1,
            sequence: 1,
            turn_id: event.turn_id().cloned(),
            occurred_at: chrono::Utc::now(),
            correlation_id: crate::app::CorrelationId::new(),
            origin: crate::app::Origin::Test,
            session_id: id.clone(),
            event,
        })
        .unwrap();
}
fn asset_path(store: &SessionStore, path: &Path, bytes: &[u8]) -> std::path::PathBuf {
    let asset: String = store
        .lock()
        .unwrap()
        .query_row(
            "SELECT id FROM binary_assets WHERE sha256=?1",
            [hex::encode(Sha256::digest(bytes))],
            |r| r.get(0),
        )
        .unwrap();
    asset_directory_for_database(path).join(asset)
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
        "requestId":"11".repeat(16),"data":STANDARD.encode(ORIGINAL),"mimeType":"image/png","width":256,"height":256,
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
        if version == 2 {
            assert!(events.iter().any(|e|matches!(&e.event,AppEvent::PromptAccepted{attachments,..} if serde_json::to_value(attachments).unwrap()==legacy.attachments)));
        } else {
            assert!(events.iter().any(|e| matches!(&e.event, AppEvent::PromptAccepted { text, attachments, .. }
                if text.is_empty() && serde_json::to_value(&attachments[0]).unwrap() == legacy.attachments[0]
                && matches!(attachments[1], axiom_inference::PromptAttachment::Text { .. }))));
        }
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
fn legacy_v6_recovers_unrelated_broken_metadata_and_omits_invalid_image_data() {
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
        assert!(events.iter().any(|e| matches!(&e.event, AppEvent::PromptAccepted { attachments, .. } if serde_json::to_value(&attachments[0]).unwrap() == legacy.attachments[0])));
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
        if invalid_image {
            let tool = page.items.iter().find(|item| item.id == "tool").unwrap();
            assert!(tool.metadata.get("generated_image").is_none());
            assert_eq!(tool.metadata["success"], true);
            assert_eq!(
                tool.metadata["arguments"]["prompt"],
                "exact arguments remain"
            );
            assert!(tool.metadata["recovery_warning"].is_string());
            assert!(
                store
                    .generated_image(&legacy.id, "call", &legacy.image.sha256)
                    .is_err()
            );
            assert!(recovery_records(&store, &legacy.id).iter().any(|record| {
                record["sourceKind"] == 1
                    && record["sourceId"] == "tool"
                    && String::from_utf8(
                        STANDARD
                            .decode(record["originalPayloadBase64"].as_str().unwrap())
                            .unwrap(),
                    )
                    .unwrap()
                    .contains("!invalid!")
            }));
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
fn missing_tampered_and_wrong_store_assets_omit_bytes_and_reopen_usable_accounts() {
    for failure in ["missing", "tampered", "wrong-key"] {
        let root = tempdir().unwrap();
        let path = root.path().join("state.sqlite3");
        let store = SessionStore::open_protected_with_key(&path, key()).unwrap();
        let unrelated = SessionId::new();
        append(
            &store,
            &unrelated,
            AppEvent::SessionCreated {
                cwd: "/tmp".into(),
                origin: crate::app::Origin::Test,
                profile: PermissionProfile::Confirm,
            },
        );
        let unrelated_turn = TurnId::new();
        append(
            &store,
            &unrelated,
            AppEvent::PromptAccepted {
                turn_id: unrelated_turn.clone(),
                text: "unrelated usable conversation".into(),
                attachments: Vec::new(),
            },
        );
        append(
            &store,
            &unrelated,
            AppEvent::TurnCompleted {
                turn_id: unrelated_turn,
            },
        );
        store.set_local_payload("draft", &payload()).unwrap();
        let asset = std::fs::read_dir(asset_directory_for_database(&path))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        match failure {
            "missing" => std::fs::remove_file(&asset).unwrap(),
            "tampered" => {
                let mut data = std::fs::read(&asset).unwrap();
                let last = data.len() - 1;
                data[last] ^= 1;
                std::fs::write(&asset, data).unwrap();
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
                std::fs::copy(other_asset, &asset).unwrap();
            }
        }
        let damaged_ciphertext = asset.exists().then(|| std::fs::read(&asset).unwrap());
        let mut recovered = payload();
        recovered["payload"]["attachments"] = json!([]);
        assert_eq!(
            store.local_payload("draft").unwrap(),
            Some(recovered.clone())
        );
        assert!(store.verify_asset_integrity().is_err());
        drop(store);
        let reopened = SessionStore::open_protected_with_key(&path, key()).unwrap();
        assert_eq!(reopened.local_payload("draft").unwrap(), Some(recovered));
        assert!(reopened.thread_summary(&unrelated).is_ok());
        assert!(
            reopened
                .load(&unrelated)
                .unwrap()
                .iter()
                .any(|event| matches!(&event.event,
            AppEvent::PromptAccepted { text, .. } if text == "unrelated usable conversation"))
        );
        assert_eq!(
            asset.exists().then(|| std::fs::read(&asset).unwrap()),
            damaged_ciphertext,
            "opening must preserve damaged ciphertext for recovery"
        );
        assert!(reopened.verify_asset_integrity().is_err());
        let identity_before: (String, i64) = reopened.lock().unwrap().query_row(
            "SELECT id,(SELECT COUNT(*) FROM asset_owners WHERE asset_id=binary_assets.id) FROM binary_assets", [], |r| Ok((r.get(0)?,r.get(1)?)),
        ).unwrap();
        // Reuploading the exact original bytes repairs the deduplicated asset;
        // old references become readable again, without adopting damaged data.
        reopened.set_local_payload("reupload", &payload()).unwrap();
        assert_eq!(reopened.local_payload("draft").unwrap(), Some(payload()));
        reopened.verify_asset_integrity().unwrap();
        let identity_after: (String, i64) = reopened.lock().unwrap().query_row(
            "SELECT id,(SELECT COUNT(*) FROM asset_owners WHERE asset_id=binary_assets.id) FROM binary_assets", [], |r| Ok((r.get(0)?,r.get(1)?)),
        ).unwrap();
        assert_eq!(identity_after, (identity_before.0, identity_before.1 + 1));
        if let Some(damaged) = damaged_ciphertext {
            let saved = std::fs::read_dir(asset_directory_for_database(&path))
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .find(|path| {
                    path.file_name()
                        .unwrap()
                        .to_string_lossy()
                        .contains(".unavailable-")
                })
                .unwrap();
            assert_eq!(std::fs::read(saved).unwrap(), damaged);
        }
        drop(reopened);
        let reopened = SessionStore::open_protected_with_key(&path, key()).unwrap();
        assert_eq!(reopened.local_payload("draft").unwrap(), Some(payload()));
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
    // Stage a valid attachment and an omission/recovery archive, then simulate
    // an actual storage-write failure. Media damage itself must not abort.
    let raw = Connection::open(&path).unwrap();
    let mut malformed = legacy.attachments.clone();
    malformed[1]["file"]["data"] = json!("!invalid!");
    raw.execute(
        "UPDATE prompt_attachments SET payload=?1",
        [malformed.to_string()],
    )
    .unwrap();
    raw.execute_batch("CREATE TRIGGER fail_asset_migration BEFORE UPDATE ON model_messages BEGIN SELECT RAISE(ABORT,'injected migration storage failure'); END;").unwrap();
    drop(raw);
    assert!(SessionStore::open_protected_with_key(&path, key()).is_err());
    let staged = std::fs::read_dir(asset_directory_for_database(&path))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    assert!(
        !staged.is_empty(),
        "the failure occurs after ciphertext was staged"
    );
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
        malformed.to_string(),
        "failed conversion preserves the original damaged record"
    );
    opened
        .connection
        .execute_batch("DROP TRIGGER fail_asset_migration;")
        .unwrap();
    drop(opened);
    let store = SessionStore::open_protected_with_key(&path, key()).unwrap();
    assert_eq!(
        serde_json::to_value(store.prompt_attachments(&legacy.id, "user").unwrap()).unwrap(),
        json!([legacy.attachments[0]])
    );
    assert_no_embedded_bytes(&store);
    assert_eq!(recovery_records(&store, &legacy.id).len(), 1);
    let live: i64 = store
        .lock()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM binary_assets", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        std::fs::read_dir(asset_directory_for_database(&path))
            .unwrap()
            .count(),
        usize::try_from(live).unwrap()
    );
    assert!(
        staged.iter().all(|path| !path.exists()),
        "rolled-back media and recovery archives are reclaimed only after a successful retry"
    );
    store.verify_asset_integrity().unwrap();
}

#[test]
fn legacy_inline_media_damage_preserves_usable_history_positions_and_recovery_records() {
    for version in [2, 6] {
        for damage in [
            "base64",
            "missing",
            "shape",
            "magic",
            "signature",
            "truncated",
            "bounds",
            "mime",
        ] {
            let root = tempdir().unwrap();
            let path = root.path().join("state.sqlite3");
            let fixture = legacy(&path, version);
            let mut bad_image = attachment();
            let mut bad_file = fixture.attachments[1].clone();
            match damage {
                "base64" => {
                    bad_image["image"]["data"] = json!("!invalid-image!");
                    bad_file["file"]["data"] = json!("!invalid-file!");
                }
                "missing" => {
                    bad_image["image"].as_object_mut().unwrap().remove("data");
                    bad_file["file"].as_object_mut().unwrap().remove("data");
                }
                "shape" => {
                    bad_image["image"] = json!({"data":17,"mimeType":false});
                    bad_file["file"] = json!({"name":null,"data":[],"mimeType":false});
                }
                "magic" => {
                    bad_image["image"]["data"] = json!(STANDARD.encode(b"not image bytes"));
                    bad_file["file"]["mimeType"] = json!("not/a-supported-file");
                }
                _ => {
                    let (_, bytes, mime) = damaged_images()
                        .into_iter()
                        .find(|(name, _, _)| *name == damage)
                        .unwrap();
                    bad_image["image"]["data"] = json!(STANDARD.encode(bytes));
                    bad_image["image"]["mimeType"] = json!(mime);
                    bad_file["file"]["data"] = json!("!invalid-file!");
                }
            }
            let good_image = json!({"kind":"image","name":"surviving.png","image":image(PREVIEW)});
            let attachments = json!([bad_image, bad_file, good_image, fixture.attachments[1]]);
            let original_payload = attachments.to_string();
            let raw = Connection::open(&path).unwrap();
            raw.execute(
                "UPDATE prompt_attachments SET payload=?1",
                [attachments.to_string()],
            )
            .unwrap();
            let original_tool: String = raw
                .query_row(
                    "SELECT metadata FROM timeline_items WHERE id='tool'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            let mut tool: Value = serde_json::from_str(&original_tool).unwrap();
            tool["content"][0]["content"] = attachments[0]["image"].clone();
            tool["content"][0]["content"]["type"] = json!("image");
            raw.execute(
                "UPDATE timeline_items SET metadata=?1 WHERE id='tool'",
                [tool.to_string()],
            )
            .unwrap();
            if version == 6 {
                raw.execute("UPDATE model_history SET payload=json_set(payload,'$.attachments',json(?1)) WHERE json_extract(payload,'$.kind')='prompt_accepted'", [attachments.to_string()]).unwrap();
                let mut generated = serde_json::to_value(&fixture.image).unwrap();
                generated.as_object_mut().unwrap().remove("data");
                if let Some(data) = attachments[0]["image"].get("data") {
                    generated["data"] = data.clone();
                }
                generated["mimeType"] = attachments[0]["image"]["mimeType"].clone();
                raw.execute("UPDATE model_history SET payload=json_set(payload,'$.image',json(?1)) WHERE json_extract(payload,'$.kind')='image_generated'", [generated.to_string()]).unwrap();
                let mut message = fixture.messages[0].clone();
                message["images"] = json!([attachments[0]["image"]]);
                message["files"] = json!([attachments[1]["file"]]);
                raw.execute(
                    "UPDATE model_messages SET payload=?1 WHERE id=41",
                    [message.to_string()],
                )
                .unwrap();
            }
            drop(raw);
            let store = SessionStore::open_protected_with_key(&path, key()).unwrap();
            assert_eq!(
                serde_json::to_value(store.prompt_attachments(&fixture.id, "user").unwrap())
                    .unwrap(),
                json!([attachments[2], attachments[3]])
            );
            let history = store.load_presentation(&fixture.id).unwrap().events;
            assert!(history.iter().any(|event| matches!(&event.event, AppEvent::PromptAccepted { text, attachments, .. }
                if text == "look" && attachments.len() == 4 && matches!(attachments[0],axiom_inference::PromptAttachment::Text { .. })
                && matches!(attachments[1],axiom_inference::PromptAttachment::Text { .. }))));
            assert!(
                !history
                    .iter()
                    .any(|event| matches!(event.event, AppEvent::ImageGenerated { .. }))
            );
            let catalog =
                crate::image_tool::ImageCatalog::from_history(&fixture.id, &history).unwrap();
            let identity = serde_json::to_vec(&(
                "axiom-local-image-v1",
                fixture.id.to_string(),
                fixture.turn.to_string(),
                2,
                STANDARD.encode(PREVIEW),
            ))
            .unwrap();
            assert!(
                catalog
                    .resolve(&format!(
                        "img_{}",
                        &hex::encode(Sha256::digest(identity))[..32]
                    ))
                    .is_ok()
            );
            let snapshot = store.thread_snapshot(&fixture.id, None, 100).unwrap();
            let tool = snapshot
                .items
                .iter()
                .find(|item| item.id == "tool")
                .unwrap();
            assert_eq!(tool.metadata["success"], true);
            assert_eq!(
                tool.metadata["arguments"]["prompt"],
                "exact arguments remain"
            );
            assert!(tool.metadata["generated_image"].is_null());
            assert!(tool.metadata["recovery_warning"].is_string());
            let records = recovery_records(&store, &fixture.id);
            assert!(records.iter().any(|record| {
                record["sourceKind"] == 0
                    && String::from_utf8(
                        STANDARD
                            .decode(record["originalPayloadBase64"].as_str().unwrap())
                            .unwrap(),
                    )
                    .unwrap()
                        == original_payload
            }));
            if version == 6 {
                let checkpoint = history
                    .iter()
                    .find_map(|event| match &event.event {
                        AppEvent::ModelHistoryCheckpoint { messages, .. } => Some(messages),
                        _ => None,
                    })
                    .unwrap();
                assert_eq!(checkpoint[0]["content"], "look");
                assert_eq!(checkpoint[0]["images"], json!([]));
                assert_eq!(checkpoint[0]["files"], json!([]));
                assert_eq!(checkpoint[1], fixture.messages[1]);
                assert!(records.iter().any(|record| record["sourceKind"] == 3
                    && record["originalDigest"] == fixture.digests[0].1));
                // The omitted message's old digest must not block a later
                // authenticated write of that exact original valid message.
                append(
                    &store,
                    &fixture.id,
                    AppEvent::ModelHistoryCheckpoint {
                        version: 1,
                        messages: fixture.messages.clone(),
                    },
                );
            }
            let mut forged = payload();
            forged["payload"]["attachments"][0]["image"]["_axiom_unavailable"] = json!(true);
            assert!(store.set_local_payload("new-write", &forged).is_err());
            assert_no_embedded_bytes(&store);
            store.verify_asset_integrity().unwrap();
            drop(store);
            let reopened = SessionStore::open_protected_with_key(&path, key()).unwrap();
            assert_eq!(
                serde_json::to_value(reopened.prompt_attachments(&fixture.id, "user").unwrap())
                    .unwrap(),
                json!([attachments[2], attachments[3]])
            );
            assert!(!recovery_records(&reopened, &fixture.id).is_empty());
            assert!(
                reopened
                    .export(&fixture.id)
                    .unwrap()
                    .contains("exact arguments remain")
            );
            reopened.delete_sessions(&[fixture.id]).unwrap();
            assert_eq!(
                std::fs::read_dir(asset_directory_for_database(&path))
                    .unwrap()
                    .count(),
                0,
                "thread deletion also releases its encrypted recovery records"
            );
        }
    }
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

#[test]
fn damaged_media_is_omitted_from_history_checkpoints_attachments_queue_and_export_after_restart() {
    let root = tempdir().unwrap();
    let path = root.path().join("state.sqlite3");
    let fixture = legacy(&path, 6);
    let store = SessionStore::open_protected_with_key(&path, key()).unwrap();
    store.set_local_payload("draft", &payload()).unwrap();
    let queue = json!({"version":1,"kind":"queue","items":[
        {"id":"damaged","threadId":fixture.id,"text":"keep queued text","attachments":[attachment()],"status":"sending","preparing":true,"webEnabled":false,"agentRevision":0},
        {"id":"unaffected","threadId":fixture.id,"text":"other queue entry","attachments":[],"status":"queued","webEnabled":false,"agentRevision":0}
    ]});
    store.set_local_payload("queue", &queue).unwrap();
    let valid_preview = asset_path(&store, &path, PREVIEW);
    let valid_ciphertext = std::fs::read(&valid_preview).unwrap();
    let original = asset_path(&store, &path, ORIGINAL);
    let mut damaged = std::fs::read(&original).unwrap();
    let end = damaged.len() - 1;
    damaged[end] ^= 1;
    std::fs::write(&original, &damaged).unwrap();
    drop(store);
    let store = SessionStore::open_protected_with_key(&path, key()).unwrap();
    let outcome = store.load_recovering(&fixture.id).unwrap();
    assert_eq!(outcome.warnings, vec![super::asset_codec::MEDIA_OMITTED]);
    assert!(
        !outcome
            .events
            .iter()
            .any(|event| matches!(event.event, AppEvent::ImageGenerated { .. }))
    );
    assert!(outcome.events.iter().any(|event| matches!(&event.event,
        AppEvent::PromptAccepted { attachments, .. } if matches!(attachments[0], axiom_inference::PromptAttachment::Text { .. }))));
    let checkpoint = outcome
        .events
        .iter()
        .find_map(|event| match &event.event {
            AppEvent::ModelHistoryCheckpoint { messages, .. } => Some(messages),
            _ => None,
        })
        .unwrap();
    assert_eq!(checkpoint[0]["content"], "look");
    assert_eq!(checkpoint[0]["images"], json!([]));
    assert_eq!(checkpoint[0]["files"], fixture.messages[0]["files"]);
    assert_eq!(
        checkpoint[1], fixture.messages[1],
        "valid exact preview remains unchanged"
    );
    let attachments = store.prompt_attachments(&fixture.id, "user").unwrap();
    assert_eq!(
        serde_json::to_value(attachments).unwrap(),
        json!([fixture.attachments[1]])
    );
    assert!(
        store
            .generated_image(&fixture.id, "call", &fixture.image.sha256)
            .is_err()
    );
    let snapshot = store.thread_snapshot(&fixture.id, None, 100).unwrap();
    let tool = snapshot
        .items
        .iter()
        .find(|item| item.id == "tool")
        .unwrap();
    assert!(tool.metadata.get("generated_image").is_none());
    assert_eq!(tool.metadata["success"], true);
    assert_eq!(
        tool.metadata["arguments"]["prompt"],
        "exact arguments remain"
    );
    assert!(tool.metadata["recovery_warning"].is_string());
    let queue_after = store.local_payload("queue").unwrap().unwrap();
    assert_eq!(queue_after["items"][0]["text"], "keep queued text");
    assert_eq!(queue_after["items"][0]["attachments"], json!([]));
    assert_eq!(queue_after["items"][0]["status"], "queued");
    assert_eq!(queue_after["items"][0]["preparing"], true);
    assert!(
        queue_after["items"][0]["error"]
            .as_str()
            .unwrap()
            .contains("omitted")
    );
    assert_eq!(queue_after["items"][1], queue["items"][1]);
    let export = store.export(&fixture.id).unwrap();
    assert!(export.contains("look") && export.contains("exact arguments remain"));
    assert!(!export.contains(&STANDARD.encode(ORIGINAL)));
    assert_eq!(std::fs::read(&valid_preview).unwrap(), valid_ciphertext);
    assert_eq!(std::fs::read(&original).unwrap(), damaged);
    // A canonical checkpoint written after recovery persists only usable media.
    let checkpoint = checkpoint.clone();
    append(
        &store,
        &fixture.id,
        AppEvent::ModelHistoryCheckpoint {
            version: 1,
            messages: checkpoint.clone(),
        },
    );
    drop(store);
    let reopened = SessionStore::open_protected_with_key(&path, key()).unwrap();
    assert!(
        reopened
            .load(&fixture.id)
            .unwrap()
            .iter()
            .any(|event| matches!(&event.event,
        AppEvent::ModelHistoryCheckpoint { messages, .. } if messages == &checkpoint))
    );
    assert_eq!(std::fs::read(&valid_preview).unwrap(), valid_ciphertext);
}

#[test]
fn malformed_unrelated_metadata_and_unowned_asset_markers_never_become_media() {
    let store = SessionStore::in_memory().unwrap();
    store.set_local_payload("owner", &payload()).unwrap();
    let raw: String = store
        .lock()
        .unwrap()
        .query_row(
            "SELECT payload FROM local_payloads WHERE id='owner'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let raw: Value = serde_json::from_str(&raw).unwrap();
    let mut stolen = raw.clone();
    stolen["payload"]["text"] = json!("preserve unowned record text");
    store
        .lock()
        .unwrap()
        .execute(
            "INSERT INTO local_payloads(id,payload) VALUES('unowned',?1)",
            [stolen.to_string()],
        )
        .unwrap();
    let recovered = store.local_payload("unowned").unwrap().unwrap();
    assert_eq!(recovered["payload"]["text"], "preserve unowned record text");
    assert_eq!(recovered["payload"]["attachments"], json!([]));
    assert_eq!(store.local_payload("owner").unwrap(), Some(payload()));
    let connection = store.lock().unwrap();
    let codec = super::asset_codec::MediaCodec {
        assets: connection.assets(),
        connection: &connection,
        owner: "unowned",
        direction: super::asset_codec::Direction::Load,
    };
    let mut metadata = json!({"generated_image":"unrelated malformed metadata", "content":[
        {"type":"image","_axiom_asset":raw["payload"]["attachments"][0]["image"]["_axiom_asset"]}
    ], "arguments":{"images":[{"_axiom_asset":{"id":"not-an-asset"}}]}});
    let original = metadata.clone();
    assert_eq!(codec.timeline(&mut metadata).unwrap(), 0);
    assert_eq!(
        metadata, original,
        "unrecognized metadata must not trigger media lookup"
    );
    let mut message = json!({"role":"tool", "content":original.to_string(), "tool_calls":[{"function":{"arguments":original}}]});
    let original = message.clone();
    assert_eq!(codec.message(&mut message).unwrap(), 0);
    assert_eq!(message, original);
}

#[test]
fn semantic_restore_selects_checkpoint_without_old_bodies_files_or_obsolete_media() {
    let root = tempdir().unwrap();
    let path = root.path().join("state.sqlite3");
    let fixture = legacy(&path, 6);
    // A non-image preceding the upload is significant: image IDs include its
    // original attachment index, even when that file is no longer hydrated.
    let raw = Connection::open(&path).unwrap();
    let attachments = json!([fixture.attachments[1], fixture.attachments[0]]);
    raw.execute(
        "UPDATE prompt_attachments SET payload=?1",
        [attachments.to_string()],
    )
    .unwrap();
    raw.execute("UPDATE model_history SET payload=json_set(payload,'$.attachments',json(?1)) WHERE json_extract(payload,'$.kind')='prompt_accepted'", [attachments.to_string()]).unwrap();
    drop(raw);
    let store = SessionStore::open_protected_with_key(&path, key()).unwrap();
    let catalog = crate::image_tool::ImageCatalog::default();
    catalog
        .add_uploads(
            &fixture.id.to_string(),
            &fixture.turn.to_string(),
            &serde_json::from_value::<Vec<axiom_inference::PromptAttachment>>(attachments.clone())
                .unwrap(),
        )
        .unwrap();
    catalog.add_generated(&fixture.image).unwrap();
    let expected_manifest = catalog.manifest().unwrap();
    let current = json!([
        {"role":"system","content":"canonical system"},
        {"role":"user","content":"compacted summary with exact facts"}
    ]);
    {
        let mut connection = store.lock().unwrap();
        let transaction = connection.transaction().unwrap();
        let text = "old token-dense text ".repeat(200);
        for _ in 0..1_000 {
            transaction.execute("INSERT INTO model_history(thread_id,version,turn_id,occurred_at,origin,payload) VALUES(?1,1,?2,'2026-11-11T00:00:00Z','test',?3)", params![fixture.id.to_string(),fixture.turn.to_string(),json!({"kind":"text_delta","turn_id":fixture.turn,"text":text}).to_string()]).unwrap();
        }
        transaction.commit().unwrap();
    }
    for event in [
        AppEvent::ModelChanged {
            model: "selected-model".into(),
        },
        AppEvent::ThinkingLevelChanged {
            level: crate::app::ThinkingLevel::High,
        },
        AppEvent::PermissionProfileChanged {
            profile: PermissionProfile::FullAccess,
        },
        AppEvent::WorkspaceChanged {
            paths: vec!["/tmp/changed-file".into()],
        },
        AppEvent::ToolStarted {
            call_id: "unknown-side-effect".into(),
            name: "synthetic-tool".into(),
        },
        AppEvent::ModelHistoryCheckpoint {
            version: 1,
            messages: current.clone(),
        },
    ] {
        append(&store, &fixture.id, event);
    }
    std::fs::remove_file(asset_path(&store, &path, PREVIEW)).unwrap();
    std::fs::remove_file(asset_path(&store, &path, FILE)).unwrap();
    drop(store);
    let store = SessionStore::open_protected_with_key(&path, key()).unwrap();
    let outcome = store.load_recovering(&fixture.id).unwrap();
    assert!(
        outcome.warnings.is_empty(),
        "media used only in superseded model state is not hydrated"
    );
    assert_eq!(
        super::permission_for_resume(&outcome.events, PermissionProfile::Confirm),
        PermissionProfile::FullAccess
    );
    assert_eq!(
        super::thinking_for_resume(&outcome.events, crate::app::ThinkingLevel::Low),
        crate::app::ThinkingLevel::High
    );
    assert!(
        outcome.events.len() < 15,
        "text deltas before the checkpoint must not scale runtime replay"
    );
    assert!(
        !outcome
            .events
            .iter()
            .any(|event| matches!(event.event, AppEvent::TextDelta { .. }))
    );
    assert_eq!(
        outcome
            .events
            .iter()
            .filter(|event| matches!(event.event, AppEvent::ModelHistoryCheckpoint { .. }))
            .count(),
        1
    );
    assert!(outcome.events.iter().any(|event| matches!(&event.event, AppEvent::ModelHistoryCheckpoint { messages, .. } if messages == &current)));
    assert!(outcome.events.iter().any(|event| matches!(&event.event, AppEvent::ToolStarted { call_id, .. } if call_id == "unknown-side-effect")));
    assert!(outcome.events.iter().any(|event| matches!(&event.event, AppEvent::WorkspaceChanged { paths } if paths == &vec![std::path::PathBuf::from("/tmp/changed-file")])));
    let loaded_catalog =
        crate::image_tool::ImageCatalog::from_history(&fixture.id, &outcome.events).unwrap();
    assert_eq!(loaded_catalog.manifest().unwrap(), expected_manifest);
    let page = store.thread_snapshot(&fixture.id, None, 100).unwrap();
    assert!(page.items.iter().any(|item| item.content == "look"));
    assert!(
        store
            .export(&fixture.id)
            .unwrap()
            .contains("exact arguments remain")
    );
    assert_eq!(store.lock().unwrap().query_row("SELECT COUNT(*) FROM model_history WHERE json_extract(payload,'$.kind')='text_delta'", [], |r| r.get::<_, i64>(0)).unwrap(), 1_000,
        "selective restore must not delete the saved journal");
}

#[test]
fn unavailable_attachment_preserves_surviving_uploaded_image_identity() {
    for canonical in [false, true] {
        let root = tempdir().unwrap();
        let path = root.path().join("state.sqlite3");
        let store = SessionStore::open_protected_with_key(&path, key()).unwrap();
        let id = SessionId::new();
        let turn = TurnId::new();
        append(
            &store,
            &id,
            AppEvent::SessionCreated {
                cwd: "/tmp".into(),
                origin: crate::app::Origin::Test,
                profile: PermissionProfile::Confirm,
            },
        );
        let attachments = vec![
            serde_json::from_value::<axiom_inference::PromptAttachment>(attachment()).unwrap(),
            serde_json::from_value(
                json!({"kind":"image","name":"preview.png","image":image(PREVIEW)}),
            )
            .unwrap(),
        ];
        let expected = crate::image_tool::ImageCatalog::default();
        expected
            .add_uploads(&id.to_string(), &turn.to_string(), &attachments)
            .unwrap();
        let expected: Value = serde_json::from_str(&expected.manifest().unwrap()).unwrap();
        append(
            &store,
            &id,
            AppEvent::PromptAccepted {
                turn_id: turn.clone(),
                text: "keep this text".into(),
                attachments,
            },
        );
        append(
            &store,
            &id,
            AppEvent::TurnCompleted {
                turn_id: turn.clone(),
            },
        );
        if !canonical {
            store.lock().unwrap().execute_batch("DELETE FROM model_history; DELETE FROM model_history_threads; DELETE FROM model_messages;").unwrap();
        }
        std::fs::remove_file(asset_path(&store, &path, ORIGINAL)).unwrap();
        drop(store);
        let reopened = SessionStore::open_protected_with_key(&path, key()).unwrap();
        let history = reopened.load(&id).unwrap();
        assert!(history.iter().any(|event| matches!(&event.event, AppEvent::PromptAccepted { text, attachments, turn_id } if text == "keep this text" && turn_id == &turn && attachments.len() == 2)));
        let restored = crate::image_tool::ImageCatalog::from_history(&id, &history).unwrap();
        let restored: Value = serde_json::from_str(&restored.manifest().unwrap()).unwrap();
        assert_eq!(restored.as_array().unwrap().len(), 1);
        assert_eq!(restored[0]["image_id"], expected[1]["image_id"]);
        assert_eq!(restored[0]["name"], "preview.png");
        append(
            &reopened,
            &id,
            AppEvent::ModelChanged {
                model: "model-after-recovery".into(),
            },
        );
        drop(reopened);
        let reopened = SessionStore::open_protected_with_key(&path, key()).unwrap();
        let catalog =
            crate::image_tool::ImageCatalog::from_history(&id, &reopened.load(&id).unwrap())
                .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&catalog.manifest().unwrap()).unwrap()[0]["image_id"],
            expected[1]["image_id"]
        );
    }
}

#[test]
fn selective_restore_retains_only_context_reports_that_still_cover_the_checkpoint() {
    for changed in [false, true] {
        let store = SessionStore::in_memory().unwrap();
        let id = SessionId::new();
        append(
            &store,
            &id,
            AppEvent::SessionCreated {
                cwd: "/tmp".into(),
                origin: crate::app::Origin::Test,
                profile: PermissionProfile::Confirm,
            },
        );
        let messages =
            json!([{"role":"system","content":"system"},{"role":"assistant","content":"reply"}]);
        append(
            &store,
            &id,
            AppEvent::ModelHistoryCheckpoint {
                version: 1,
                messages: messages.clone(),
            },
        );
        let usage = axiom_acp_extension::ContextUsage {
            input_tokens: 70_000,
            output_tokens: 1_000,
            model_id: "model".into(),
            reported_at: "2026-11-12T00:00:00Z".into(),
            context_window_tokens: Some(100_000),
            auto_compact_threshold_tokens: Some(85_000),
        };
        append(
            &store,
            &id,
            AppEvent::ContextUsageUpdated {
                usage: usage.clone(),
            },
        );
        if changed {
            append(
                &store,
                &id,
                AppEvent::ContextCompacted {
                    summary: "different state".into(),
                    messages_before: 2,
                },
            );
        }
        append(
            &store,
            &id,
            AppEvent::ModelHistoryCheckpoint {
                version: 1,
                messages,
            },
        );
        let loaded = store.load(&id).unwrap();
        let report = loaded
            .iter()
            .position(|event| matches!(event.event, AppEvent::ContextUsageUpdated { .. }));
        if changed {
            assert!(
                report.is_none(),
                "compaction invalidates the earlier report"
            );
        } else {
            let checkpoint = loaded
                .iter()
                .position(|event| matches!(event.event, AppEvent::ModelHistoryCheckpoint { .. }))
                .unwrap();
            assert_eq!(report, Some(checkpoint + 1));
            assert!(
                matches!(&loaded[checkpoint + 1].event, AppEvent::ContextUsageUpdated { usage: value } if value == &usage)
            );
        }
    }
}
