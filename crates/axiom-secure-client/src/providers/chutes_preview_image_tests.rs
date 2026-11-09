//! Exercise real stdio helpers, including cross-provider image requests.
#![cfg(unix)]

use super::*;

struct Fixture {
    directory: tempfile::TempDir,
    native: NativeConfig,
    config: SecureClientConfig,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let script = directory.path().join("helper.py");
        let config = directory.path().join("config.json");
        std::fs::write(&script, HELPER).unwrap();
        std::fs::write(&config, b"{}").unwrap();
        Self {
            native: NativeConfig {
                python: Some("/usr/bin/python3".into()),
                script: Some(script),
                helper: None,
                helper_sha256: None,
                config,
                release_authority_fingerprint: Some("56".repeat(32)),
                script_sha256: Some(hex::encode(Sha256::digest(HELPER))),
                maximum_charge_microusd: 100_000,
            },
            config: SecureClientConfig::new("https://relay.example").unwrap(),
            directory,
        }
    }

    fn mode(&self, mode: &str) {
        std::fs::write(self.directory.path().join("mode"), mode).unwrap();
    }

    fn events(&self) -> Vec<Value> {
        std::fs::read_to_string(self.directory.path().join("events"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    async fn wait_images(&self, count: usize) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while self.events().iter().filter(|v| v["op"] == "image").count() < count {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    }

    async fn image(
        &self,
        cache: &ImageSessionCache,
        token: &str,
        cancel: CancellationToken,
        stream: bool,
    ) -> Result<GeneratedImage> {
        self.image_with_dimensions(
            cache,
            token,
            cancel,
            stream,
            Some(axiom_inference::ImageDimensions {
                width: 256,
                height: 256,
            }),
        )
        .await
    }

    async fn image_with_dimensions(
        &self,
        cache: &ImageSessionCache,
        token: &str,
        cancel: CancellationToken,
        stream: bool,
        dimensions: Option<axiom_inference::ImageDimensions>,
    ) -> Result<GeneratedImage> {
        self.image_for_model(
            cache,
            token,
            cancel,
            stream,
            dimensions,
            "tinfoil:chat-model",
        )
        .await
    }

    async fn image_for_model(
        &self,
        cache: &ImageSessionCache,
        token: &str,
        cancel: CancellationToken,
        stream: bool,
        dimensions: Option<axiom_inference::ImageDimensions>,
        model: &str,
    ) -> Result<GeneratedImage> {
        let (progress, mut updates) = mpsc::channel(8);
        let result = image_operation_on_installation(
            &self.config,
            Arc::new(ApiCredential::new(token)),
            cache,
            self.native.clone(),
            axiom_inference::ImageGenerationRequest {
                chat_model_id: model,
                prompt: "fixed public regression fixture",
                references: &[],
                dimensions,
            },
            // Production derives this from the chat model's catalog tag; the
            // fixture models are the bundled uncensored offerings or not.
            is_uncensored_model(model),
            stream.then_some(progress),
            cancel,
        )
        .await;
        if stream && result.is_ok() {
            assert_eq!(updates.recv().await.unwrap().percent, 0);
            assert_eq!(updates.recv().await.unwrap().percent, 50);
            assert_eq!(updates.recv().await.unwrap().percent, 99);
            assert!(updates.recv().await.is_none());
        }
        result
    }
}

#[tokio::test]
async fn signed_runtime_changes_defaults_adapter_and_steps_on_the_same_helper() {
    let fixture = Fixture::new();
    let cache = ImageSessionCache::default();
    let mut runtime = axiom_inference::ImageRuntimePolicy::default();
    for (ratio, strength, sigmas) in [
        ("1:1", 0.8, vec![1.0, 0.6, 0.0]),
        ("3:2", 0.7, vec![1.0, 0.8, 0.4, 0.0]),
    ] {
        runtime.image.default_aspect_ratio = ratio.into();
        runtime.image.lora.as_mut().unwrap().strength = strength;
        runtime.image.sigmas = sigmas;
        std::fs::write(
            fixture.directory.path().join("runtime.json"),
            serde_json::to_vec(&runtime).unwrap(),
        )
        .unwrap();
        let image = fixture
            .image_for_model(
                &cache,
                "account-a",
                CancellationToken::new(),
                true,
                None,
                "chutes:qwen3.8-27b",
            )
            .await
            .unwrap();
        let expected = runtime.dimensions(ratio).unwrap();
        assert_eq!(
            (image.width, image.height),
            (expected.width, expected.height)
        );
    }
    assert_eq!(
        fixture
            .events()
            .iter()
            .filter(|e| e["op"] == "configure")
            .count(),
        1
    );
    fixture.mode("wrong_runtime_binding");
    assert!(
        fixture
            .image_for_model(
                &cache,
                "account-a",
                CancellationToken::new(),
                false,
                None,
                "chutes:qwen3.8-27b"
            )
            .await
            .is_err()
    );
    assert!(cache.idle.lock().await.is_none());
}

#[tokio::test]
async fn unsupported_runtime_is_rejected_before_image_dispatch() {
    let fixture = Fixture::new();
    let cache = ImageSessionCache::default();
    let mut runtime = axiom_inference::ImageRuntimePolicy::default();
    runtime.required_features.push("future-protocol".into());
    std::fs::write(
        fixture.directory.path().join("runtime.json"),
        serde_json::to_vec(&runtime).unwrap(),
    )
    .unwrap();
    assert!(
        fixture
            .image(&cache, "account-a", CancellationToken::new(), false)
            .await
            .is_err()
    );
    assert!(!fixture.events().iter().any(|e| e["op"] == "image"));
}

#[tokio::test]
async fn image_policy_changes_per_request_on_a_reused_helper_and_rejects_wrong_adapters() {
    let fixture = Fixture::new();
    let cache = ImageSessionCache::default();
    for model in [
        "chutes:qwen3.8-27b",
        "tinfoil:chat-model",
        "chutes-preview:qwen3.8-27b",
    ] {
        let image = fixture
            .image_for_model(
                &cache,
                "account-a",
                CancellationToken::new(),
                true,
                None,
                model,
            )
            .await
            .unwrap();
        assert_eq!(image.chat_model_id.as_deref(), Some(model));
    }
    let events = fixture.events();
    assert_eq!(events.iter().filter(|v| v["op"] == "configure").count(), 1);
    assert_eq!(
        events
            .iter()
            .filter(|v| v["op"] == "image")
            .map(|v| v["uncensored"].as_bool().unwrap())
            .collect::<Vec<_>>(),
        [true, false, true]
    );
    for mode in [
        "wrong_strength",
        "wrong_policy",
        "missing_policy",
        "wrong_adapter",
    ] {
        fixture.mode(mode);
        assert!(
            fixture
                .image_for_model(
                    &cache,
                    "account-a",
                    CancellationToken::new(),
                    false,
                    None,
                    "chutes:qwen3.8-27b"
                )
                .await
                .is_err(),
            "{mode}"
        );
        assert!(cache.idle.lock().await.is_none());
    }
}

#[tokio::test]
async fn default_images_send_portrait_dimensions_to_retained_helpers_and_reject_smaller_results() {
    let fixture = Fixture::new();
    let cache = ImageSessionCache::default();
    for stream in [false, true] {
        let image = fixture
            .image_with_dimensions(&cache, "account-a", CancellationToken::new(), stream, None)
            .await
            .unwrap();
        assert_eq!((image.width, image.height), (1216, 1824));
    }
    let events = fixture.events();
    let images = events.iter().filter(|event| event["op"] == "image");
    for image in images {
        assert_eq!(
            (image["width"].as_u64(), image["height"].as_u64()),
            (Some(1216), Some(1824))
        );
    }
    let smaller = fixture
        .image(&cache, "account-a", CancellationToken::new(), false)
        .await
        .unwrap();
    assert_eq!((smaller.width, smaller.height), (256, 256));

    fixture.mode("wrong_dimensions");
    assert!(
        fixture
            .image_with_dimensions(&cache, "account-a", CancellationToken::new(), false, None)
            .await
            .is_err()
    );
    assert!(cache.idle.lock().await.is_none());
}

#[tokio::test]
async fn images_from_tinfoil_reuse_one_helper_and_verify_the_image_recipient_before_every_send() {
    let fixture = Fixture::new();
    let cache = ImageSessionCache::default();
    let first = fixture
        .image(&cache, "account-a", CancellationToken::new(), false)
        .await
        .unwrap();
    let second = fixture
        .image(&cache, "account-a", CancellationToken::new(), true)
        .await
        .unwrap();
    assert_ne!(first.request_id, second.request_id);
    assert_eq!(second.evidence.provider_id, "chutes");
    assert_eq!(second.evidence.model_id, "chutes:qwen-image-2.1-turbo");
    assert_eq!(second.evidence.state, SecurityState::Verified);
    let events = fixture.events();
    assert_eq!(
        events
            .iter()
            .map(|v| v["op"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["configure", "attest", "image", "attest", "image"]
    );
    assert!(events.iter().all(|v| v["pid"] == events[0]["pid"]));
    assert!(cache.idle.lock().await.is_some());

    fixture.mode("expired");
    assert!(
        fixture
            .image(&cache, "account-a", CancellationToken::new(), false)
            .await
            .is_err()
    );
    assert_eq!(
        fixture
            .events()
            .iter()
            .filter(|v| v["op"] == "image")
            .count(),
        2,
        "expired image evidence must prevent dispatch even with a verified chat provider"
    );
    assert!(cache.idle.lock().await.is_none());
}

#[tokio::test]
async fn image_helpers_are_isolated_between_credentials_and_die_with_the_owner() {
    let fixture = Fixture::new();
    let account_a = ImageSessionCache::default();
    let account_b = ImageSessionCache::default();
    fixture
        .image(&account_a, "account-a", CancellationToken::new(), false)
        .await
        .unwrap();
    fixture
        .image(&account_b, "account-b", CancellationToken::new(), false)
        .await
        .unwrap();
    let configured = fixture
        .events()
        .into_iter()
        .filter(|v| v["op"] == "configure")
        .collect::<Vec<_>>();
    assert_eq!(configured.len(), 2);
    assert_ne!(configured[0]["pid"], configured[1]["pid"]);
    assert_ne!(
        configured[0]["credential_fingerprint"],
        configured[1]["credential_fingerprint"]
    );
    let pid = account_a
        .idle
        .lock()
        .await
        .as_ref()
        .unwrap()
        .bridge
        .child
        .id()
        .unwrap();
    let alive = || {
        std::process::Command::new("/bin/kill")
            .args(["-0", &pid.to_string()])
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success()
    };
    assert!(alive());
    let weak = Arc::downgrade(&account_a.idle);
    drop(account_a);
    assert!(weak.upgrade().is_none());
    tokio::time::timeout(Duration::from_secs(2), async {
        while alive() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert!(account_b.idle.lock().await.is_some());
}

#[tokio::test]
async fn cancelled_and_aborted_images_discard_the_helper_without_replay() {
    for abort in [false, true] {
        let fixture = Arc::new(Fixture::new());
        let cache = Arc::new(ImageSessionCache::default());
        fixture.mode("hang");
        let cancellation = CancellationToken::new();
        let task_fixture = fixture.clone();
        let task_cache = cache.clone();
        let token = cancellation.clone();
        let task = tokio::spawn(async move {
            task_fixture
                .image(&task_cache, "account-a", token, false)
                .await
        });
        fixture.wait_images(1).await;
        // A queued caller can cancel without taking the in-use helper.
        let queued = CancellationToken::new();
        queued.cancel();
        assert!(matches!(
            fixture.image(&cache, "account-a", queued, false).await,
            Err(error) if error.code() == crate::SecureErrorCode::Cancelled
        ));
        if abort {
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
        } else {
            cancellation.cancel();
            assert!(matches!(
                task.await.unwrap(),
                Err(error) if error.code() == crate::SecureErrorCode::Cancelled
            ));
        }
        assert!(cache.idle.lock().await.is_none());
        fixture.mode("");
        fixture
            .image(&cache, "account-a", CancellationToken::new(), false)
            .await
            .unwrap();
        let events = fixture.events();
        assert_eq!(events.iter().filter(|v| v["op"] == "image").count(), 2);
        assert_eq!(events.iter().filter(|v| v["op"] == "configure").count(), 2);
        let ids = events
            .iter()
            .filter(|v| v["op"] == "image")
            .map(|v| &v["request_id"])
            .collect::<Vec<_>>();
        assert_ne!(ids[0], ids[1]);
    }
}

#[tokio::test]
async fn failures_and_worker_mismatches_are_never_retained_or_replayed() {
    for mode in ["exit", "invalid", "changed_key"] {
        let fixture = Fixture::new();
        let cache = ImageSessionCache::default();
        fixture
            .image(&cache, "account-a", CancellationToken::new(), false)
            .await
            .unwrap();
        fixture.mode(mode);
        assert!(
            fixture
                .image(&cache, "account-a", CancellationToken::new(), false)
                .await
                .is_err()
        );
        assert!(cache.idle.lock().await.is_none());
        assert_eq!(
            fixture
                .events()
                .iter()
                .filter(|v| v["op"] == "configure")
                .count(),
            1
        );
        fixture.mode("");
        fixture
            .image(&cache, "account-a", CancellationToken::new(), false)
            .await
            .unwrap();
        assert_eq!(
            fixture
                .events()
                .iter()
                .filter(|v| v["op"] == "configure")
                .count(),
            2
        );
    }
}

#[tokio::test]
async fn configuration_changes_and_idle_expiry_replace_helpers() {
    let fixture = Fixture::new();
    let cache = ImageSessionCache::default();
    fixture
        .image(&cache, "account-a", CancellationToken::new(), false)
        .await
        .unwrap();
    std::fs::write(&fixture.native.config, b"{\"changed\":true}").unwrap();
    fixture
        .image(&cache, "account-a", CancellationToken::new(), false)
        .await
        .unwrap();
    cache.idle.lock().await.as_mut().unwrap().idle_since -= IMAGE_HELPER_IDLE_TIMEOUT;
    fixture
        .image(&cache, "account-a", CancellationToken::new(), false)
        .await
        .unwrap();
    assert_eq!(
        fixture
            .events()
            .iter()
            .filter(|v| v["op"] == "configure")
            .count(),
        3
    );
}

#[tokio::test]
async fn concurrent_images_serialize_one_helper_without_crossing_streams() {
    let fixture = Fixture::new();
    let cache = ImageSessionCache::default();
    let (first, second) = tokio::join!(
        fixture.image(&cache, "account-a", CancellationToken::new(), true),
        fixture.image(&cache, "account-a", CancellationToken::new(), true),
    );
    assert_ne!(first.unwrap().request_id, second.unwrap().request_id);
    assert_eq!(
        fixture
            .events()
            .iter()
            .filter(|v| v["op"] == "configure")
            .count(),
        1
    );
}

#[tokio::test(start_paused = true)]
async fn idle_reaper_does_not_keep_a_helper_alive_indefinitely() {
    let fixture = Fixture::new();
    let cache = ImageSessionCache::default();
    // Avoid paused-time auto-advance while waiting for the external process.
    tokio::time::resume();
    fixture
        .image(&cache, "account-a", CancellationToken::new(), false)
        .await
        .unwrap();
    tokio::time::pause();
    tokio::task::yield_now().await;
    tokio::time::advance(IMAGE_HELPER_IDLE_TIMEOUT + Duration::from_secs(1)).await;
    tokio::task::yield_now().await;
    assert!(cache.idle.lock().await.is_none());
}

const HELPER: &str = r"
import base64, hashlib, json, os, struct, sys, time, zlib
from pathlib import Path
root = Path(sys.argv[2]).parent
def chunk(kind, raw):
    return struct.pack('>I', len(raw)) + kind + raw + struct.pack('>I', zlib.crc32(kind + raw))
for line in sys.stdin:
    value = json.loads(line)
    op = value['op']
    entry = {'op': 'image' if op in ('image', 'image_edit') else op, 'pid': os.getpid()}
    if op == 'configure':
        entry['credential_fingerprint'] = hashlib.sha256(value['access_token'].encode()).hexdigest()
    if op in ('image', 'image_edit'):
        entry['request_id'] = value['request_id']
        entry['width'] = value.get('width')
        entry['height'] = value.get('height')
        entry['uncensored'] = value['uncensored']
    with (root / 'events').open('a') as events:
        events.write(json.dumps(entry) + '\n')
    mode = (root / 'mode').read_text() if (root / 'mode').exists() else ''
    if op == 'configure':
        reply = {'ok': True}
    elif op == 'attest':
        now = int(time.time())
        evidence = {'instance_id': 'image-worker', 'expires_at': now - 1 if mode == 'expired' else now + 120,
            'verified_at': now, 'quote_sha256': 'ab' * 32, 'key_sha256': 'cd' * 32,
            'hardware_verified': True, 'workload_binding_verified': False,
            'release_authorization': {'repository': 'astrea-foundation/axiom-inference', 'manifest_sha256': '34' * 32,
                'policy_sequence': 42, 'expires_at': now + 3600, 'method': 'ed25519-publisher',
                'authority_fingerprint': '56' * 32, 'channel': 'staging', 'release_id': 'fixture'}}
        runtime = json.loads((root / 'runtime.json').read_text()) if (root / 'runtime.json').exists() else None
        if runtime is not None:
            evidence.update(runtime=runtime, runtime_policy_sha256=hashlib.sha256(json.dumps(runtime, sort_keys=True, separators=(',', ':')).encode()).hexdigest())
        reply = {'ok': True, 'evidence': evidence}
    else:
        if mode == 'exit':
            sys.exit(0)
        while mode == 'hang':
            time.sleep(.01)
        request_id = value['request_id']
        model = 'chutes:qwen-image-2.1-turbo'
        # Match retained workers' old fallback so omitted dimensions are caught.
        width, height = value.get('width', 1024), value.get('height', 1024)
        if mode == 'wrong_dimensions':
            width = height = 1024
        png = b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', width, height, 8, 2, 0, 0, 0))
        png += chunk(b'IDAT', zlib.compress(bytes(height * (width * 3 + 1)))) + chunk(b'IEND', b'')
        reply = {'ok': True, 'response_authenticated': mode != 'invalid', 'billing_receipt_verified': True,
            'billing': {'settled': True}, 'request_id': request_id, 'model_id': model, 'mime_type': 'image/png',
            'width': width, 'height': height, 'references': [], 'steps': 8,
            'evidence': {**evidence, 'key_sha256': 'ef' * 32 if mode == 'changed_key' else evidence['key_sha256']},
            'image_base64': base64.b64encode(png).decode(), 'image_sha256': hashlib.sha256(png).hexdigest(),
            'accounting': {'requestId': request_id, 'providerId': 'chutes', 'modelId': model, 'state': 'completed',
                'completeness': 'final', 'inputTokens': '0', 'outputTokens': '0', 'costMicrousd': '1', 'settled': True}}
        reply.update(uncensored=value['uncensored'], lora={
            'name': 'uncensored.safetensors',
            'sha256': '83aba822ad8ee10a1e836828f5088259d93bce7b5f3419b46dbc0258c2be46a9',
            'strength': 0.9} if value['uncensored'] else None)
        if runtime is not None:
            reply.update(steps=len(runtime['image']['sigmas']) - 1, runtime_policy_sha256=evidence['runtime_policy_sha256'])
            spec = runtime['image']['lora'] if value['uncensored'] else None
            reply['lora'] = {k:spec[k] for k in ('name','sha256','strength')} if spec else None
        if mode == 'wrong_runtime_binding': reply['runtime_policy_sha256'] = 'ff' * 32
        if mode == 'wrong_strength': reply['lora']['strength'] = 0.8
        if mode == 'wrong_adapter': reply['lora']['sha256'] = '00' * 32
        if mode == 'wrong_policy': reply.update(uncensored=False, lora=None)
        if mode == 'missing_policy':
            del reply['uncensored']
            del reply['lora']
        if value.get('stream'):
            for seq, (stage, percent) in enumerate([('queued', 0), ('generating', 50), ('finishing', 99)]):
                print(json.dumps({'ok': True, 'event': 'image_progress', 'request_id': request_id,
                    'sequence': seq, 'progress': {'stage': stage, 'percent': percent}}), flush=True)
            reply.update(event='image_complete', sequence=3)
    print(json.dumps(reply), flush=True)
";
