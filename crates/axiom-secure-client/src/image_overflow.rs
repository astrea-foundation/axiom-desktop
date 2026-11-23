//! Native opt-in to the existing attested gateway for text-only image overflow.
mod transport;

use std::{
    io::{Cursor, Read as _},
    path::PathBuf,
    process::Stdio,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context as _, Result as AnyResult, ensure};
use axiom_inference::{
    ImageDimensions, ImageGenerationProgress, ImageGenerationRequest, InvocationState,
    MAX_GENERATED_IMAGE_BYTES, ProviderFailureKind, SecurityState, UsageCompleteness,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use p256::ecdsa::SigningKey;
use rand_core::OsRng;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    process::Command,
    sync::mpsc,
    time::timeout,
};
use tokio_util::sync::CancellationToken;
use url::Url;
use zeroize::Zeroizing;

use crate::{
    ApiCredential, EvidenceClaim, GeneratedImage, Result, SecureClientConfig, SecureClientError,
    SecureErrorCode,
    catalog::relay_endpoint,
    http::{bounded_body, relay_client},
};
use transport::Transport;

const PROTOCOL: &str = "axiom-gateway-v2";
const CHUTES_MODEL: &str = "chutes:qwen-image-2512";
const MAX_EVIDENCE_BYTES: usize = 8 * 1024 * 1024;
const MAX_IMAGE_DATA: usize = MAX_GENERATED_IMAGE_BYTES.div_ceil(3) * 4;
const MAX_IMAGE_RESPONSE: usize = MAX_IMAGE_DATA + 2 * 1024 * 1024;

#[derive(Debug)]
pub(crate) struct Config {
    pins: ConfigFile,
    policy_sequence: AtomicU64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    gateway_origin: String,
    publisher_key: String,
    verifier: PathBuf,
    verifier_sha256: String,
}

impl Config {
    pub(crate) fn installed() -> Result<Option<Self>> {
        let Some(path) = std::env::var_os("AXIOM_IMAGE_OVERFLOW_CONFIG_FILE") else {
            return Ok(None);
        };
        let path = PathBuf::from(path);
        let invalid =
            || SecureClientError::configuration("invalid installed image overflow configuration");
        if !path.is_absolute() {
            return Err(invalid());
        }
        let mut raw = Vec::new();
        std::fs::File::open(path)
            .and_then(|file| file.take(16_385).read_to_end(&mut raw))
            .map_err(|_| invalid())?;
        if raw.len() > 16_384 {
            return Err(invalid());
        }
        let config: ConfigFile = serde_json::from_slice(&raw).map_err(|_| invalid())?;
        let origin = Url::parse(&config.gateway_origin).map_err(|_| invalid())?;
        if origin.scheme() != "https"
            || origin.origin().ascii_serialization() != config.gateway_origin
            || !config.verifier.is_absolute()
            || !is_hex(&config.verifier_sha256, 32)
            || !is_hex(&config.publisher_key, 32)
        {
            return Err(invalid());
        }
        Ok(Some(Self {
            pins: config,
            policy_sequence: AtomicU64::new(0),
        }))
    }

    async fn verify(&self, evidence: Value, challenge: &str) -> AnyResult<VerifiedGateway> {
        let input = serde_json::to_vec(&json!({
            "evidence": evidence,
            "context": {
                "challenge": challenge,
                "origin": self.pins.gateway_origin,
                "publisher_key": self.pins.publisher_key,
                "now": now(),
                "minimum_policy_sequence": self.policy_sequence.load(Ordering::Acquire),
            },
        }))?;
        ensure!(
            input.len() <= MAX_EVIDENCE_BYTES,
            "gateway evidence exceeds limit"
        );
        let path = self.pins.verifier.clone();
        let expected = self.pins.verifier_sha256.clone();
        tokio::task::spawn_blocking(move || -> AnyResult<()> {
            let metadata = std::fs::metadata(&path)?;
            ensure!(
                metadata.is_file() && metadata.len() <= 128 * 1024 * 1024,
                "invalid gateway verifier file"
            );
            let file = std::fs::File::open(path)?;
            let mut digest = Sha256::new();
            let size = std::io::copy(&mut file.take(128 * 1024 * 1024 + 1), &mut digest)?;
            ensure!(
                size == metadata.len() && hex::encode(digest.finalize()) == expected,
                "gateway verifier digest differs"
            );
            Ok(())
        })
        .await??;

        let mut command = Command::new(&self.pins.verifier);
        command
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        #[cfg(windows)]
        if let Some(root) = std::env::var_os("SystemRoot") {
            command.env("SystemRoot", root);
        }
        let mut child = command.spawn()?;
        let mut stdin = child
            .stdin
            .take()
            .context("missing gateway verifier input")?;
        let mut output = child
            .stdout
            .take()
            .context("missing gateway verifier output")?
            .take(16_385);
        let mut raw = Vec::new();
        let ((), _) = tokio::try_join!(
            async move {
                stdin.write_all(&input).await?;
                drop(stdin);
                Ok::<_, std::io::Error>(())
            },
            output.read_to_end(&mut raw),
        )?;
        ensure!(
            raw.len() <= 16_384 && child.wait().await?.success(),
            "gateway attestation failed"
        );
        let result: VerifiedGateway = serde_json::from_slice(&raw)?;
        ensure!(
            result.origin == self.pins.gateway_origin
                && is_hex(&result.encryption_key, 32)
                && is_hex(&result.source_revision, 20)
                && is_hex(&result.config_digest, 32)
                && result.policy_sequence > 0
                && result.policy_expires_at > now()
                && result.evidence_expires_at > now()
                && result.policy_sequence
                    >= self
                        .policy_sequence
                        .fetch_max(result.policy_sequence, Ordering::AcqRel),
            "invalid or expired gateway proof"
        );
        Ok(result)
    }
}

#[derive(Deserialize)]
struct VerifiedGateway {
    origin: String,
    encryption_key: String,
    source_revision: String,
    config_digest: String,
    policy_sequence: u64,
    policy_expires_at: u64,
    evidence_expires_at: u64,
}

struct Session {
    id: Zeroizing<String>,
    transport: Transport,
    proof: VerifiedGateway,
}

fn random_id() -> String {
    hex::encode(rand::random::<[u8; 16]>())
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn is_hex(value: &str, bytes: usize) -> bool {
    value.len() == bytes * 2
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn eligible(request: &ImageGenerationRequest<'_>) -> Option<ImageDimensions> {
    let size = request.dimensions?;
    (request.allow_long_queue
        && request.references.is_empty()
        && !request.prompt.trim().is_empty()
        && request.prompt.len() <= 8192
        && !request.chat_model_id.trim().is_empty()
        && request.chat_model_id.len() <= 256
        && !request.chat_model_id.chars().any(char::is_control)
        && size.validate().is_ok()
        && size.width <= 2048
        && size.height <= 2048
        && u64::from(size.width) * u64::from(size.height) <= 1536 * 1536)
        .then_some(size)
}

pub(crate) async fn try_image(
    client: &SecureClientConfig,
    authority: &ApiCredential,
    request: &ImageGenerationRequest<'_>,
    progress: &mpsc::Sender<ImageGenerationProgress>,
    cancel: &CancellationToken,
) -> Result<Option<GeneratedImage>> {
    let (Some(config), Some(size)) = (client.image_overflow.as_deref(), eligible(request)) else {
        return Ok(None);
    };
    // A gateway delegation must stay on the direct provider when it falls back.
    if authority.expose().starts_with("axg_") {
        return Ok(None);
    }
    let preparation = tokio::select! {
        biased;
        () = cancel.cancelled() => return Err(SecureClientError::cancelled()),
        value = timeout(client.attestation_timeout, Session::prepare(client, config, authority)) => value,
    };
    let Ok(Ok(Some(session))) = preparation else {
        // No image RPC has been sent. Any preflight failure leaves direct admission available.
        return Ok(None);
    };
    let request_id = random_id();
    let result = tokio::select! {
        biased;
        () = cancel.cancelled() => Err(SecureClientError::cancelled()),
        value = timeout(Duration::from_secs(220), session.image(&request_id, request, size, progress)) => {
            value.ok().and_then(AnyResult::ok).ok_or_else(|| {
                SecureClientError::with_code(
                    ProviderFailureKind::InvalidResponse,
                    SecureErrorCode::ReceiptUnavailable,
                    "Chutes image attempt has no authenticated completion",
                    false,
                )
            })
        }
    };
    let cleanup = async {
        if result.is_err() {
            session
                .control(json!({"op":"cancel", "target_request_id":request_id}))
                .await;
        }
        session.control(json!({"op":"session_close"})).await;
    };
    let _ = timeout(Duration::from_secs(3), cleanup).await;
    result
}

async fn json_response(response: reqwest::Response, limit: usize) -> AnyResult<Value> {
    ensure!(response.status().is_success(), "gateway preflight rejected");
    Ok(serde_json::from_slice(
        &bounded_body(response, limit).await?,
    )?)
}

impl Session {
    async fn prepare(
        client: &SecureClientConfig,
        config: &Config,
        authority: &ApiCredential,
    ) -> AnyResult<Option<Self>> {
        let signer = SigningKey::random(&mut OsRng);
        let browser_key = transport::public_key(&signer)?;
        let base = &client.relay_base_url;
        let route = relay_endpoint(base, "/api/v1/web-gateway/grants")?;
        let backend = relay_client(client, &route).await?;
        let grant = json_response(
            backend
                .post(route)
                .bearer_auth(authority.expose())
                .json(&json!({"browser_key":browser_key,"image_overflow":true}))
                .send()
                .await?,
            4096,
        )
        .await?;
        let Some(grant) = grant["grant"].as_str() else {
            return Ok(None);
        };
        ensure!(
            grant.len() == 47
                && grant.starts_with("axw_")
                && grant
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-')),
            "invalid gateway grant"
        );

        let http = reqwest::Client::builder()
            .https_only(true)
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .connect_timeout(Duration::from_secs(10))
            .build()?;
        let challenge = hex::encode(rand::random::<[u8; 32]>());
        let (mut evidence, policy) = tokio::try_join!(
            async {
                json_response(
                    http.post(format!("{}/v1/attestation", config.pins.gateway_origin))
                        .json(&json!({"challenge":challenge}))
                        .send()
                        .await?,
                    MAX_EVIDENCE_BYTES,
                )
                .await
            },
            async {
                let route = relay_endpoint(base, "/api/v1/web-gateway/trust-policy")?;
                json_response(backend.get(route).send().await?, 128 * 1024).await
            }
        )?;
        evidence
            .as_object_mut()
            .context("invalid gateway evidence")?
            .insert("policy".into(), policy);
        let proof = config.verify(evidence, &challenge).await?;
        let transport = Transport {
            http,
            origin: config.pins.gateway_origin.clone(),
            encryption_key: proof.encryption_key.clone(),
            signer,
            public_key: browser_key,
        };
        let mut response = None;
        transport
            .exchange(
                "/v1/session",
                None,
                grant,
                &json!({"grant":grant}),
                16_384,
                |frame| {
                    ensure!(response.is_none(), "unexpected gateway session frame");
                    response = Some(serde_json::from_slice::<Value>(frame)?);
                    Ok(())
                },
            )
            .await?;
        let value = response.context("missing gateway session response")?;
        let id = value["data"]["session_id"]
            .as_str()
            .context("gateway session refused")?;
        ensure!(
            value["success"] == true
                && value["data"]["protocol"] == PROTOCOL
                && is_hex(id, 32)
                && value["data"]["expires_at"]
                    .as_u64()
                    .is_some_and(|expiry| expiry > now()),
            "invalid gateway session"
        );
        let session = Self {
            id: Zeroizing::new(id.into()),
            transport,
            proof,
        };
        if value["data"]["image_overflow"] != 1 {
            session.control(json!({"op":"session_close"})).await;
            return Ok(None);
        }
        Ok(Some(session))
    }

    async fn control(&self, mut payload: Value) {
        payload["request_id"] = json!(random_id());
        let _ = self
            .transport
            .exchange(
                "/v1/rpc",
                Some(&self.id),
                &self.id,
                &payload,
                16_384,
                |_| Ok(()),
            )
            .await;
    }

    async fn image(
        &self,
        id: &str,
        request: &ImageGenerationRequest<'_>,
        size: ImageDimensions,
        progress: &mpsc::Sender<ImageGenerationProgress>,
    ) -> AnyResult<Option<GeneratedImage>> {
        ensure!(
            self.proof.evidence_expires_at > now() && self.proof.policy_expires_at > now(),
            "gateway proof expired before dispatch"
        );
        let mut reply = Reply::default();
        self.transport.exchange("/v1/rpc", Some(&self.id), &self.id, &json!({
            "op": "image", "request_id": id, "model": request.chat_model_id, "prompt": request.prompt,
            "dimensions": size, "image_queue": true, "overflow_only": true,
        }), MAX_IMAGE_RESPONSE, |frame| {
            if let Some(update) = reply.frame(id, frame)? {
                let _ = progress.try_send(update);
            }
            Ok(())
        }).await?;
        let Some(mut image) = reply.finish(id, request.chat_model_id, size)? else {
            return Ok(None);
        };
        image.evidence.provider_claims.push(EvidenceClaim {
            name: "Axiom gateway source revision".into(),
            value: self.proof.source_revision.clone(),
        });
        image.evidence.provider_claims.push(EvidenceClaim {
            name: "Axiom gateway measured configuration".into(),
            value: self.proof.config_digest.clone(),
        });
        Ok(Some(image))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Frame {
    protocol: String,
    request_id: String,
    sequence: usize,
    kind: String,
    data: Value,
}

#[derive(Default)]
struct Reply {
    transcript: Sha256,
    sequence: usize,
    accepted: bool,
    data: Zeroizing<String>,
    progress: Option<ImageGenerationProgress>,
    terminal: Option<Value>,
}

impl Reply {
    fn frame(&mut self, id: &str, raw: &[u8]) -> AnyResult<Option<ImageGenerationProgress>> {
        let frame: Frame = serde_json::from_slice(raw)?;
        ensure!(
            self.terminal.is_none()
                && frame.protocol == PROTOCOL
                && frame.request_id == id
                && frame.sequence == self.sequence,
            "gateway image frame binding differs"
        );
        self.sequence += 1;
        match frame.kind.as_str() {
            "accepted" => {
                ensure!(
                    self.sequence == 1 && frame.data["request_id"] == id,
                    "gateway image admission differs"
                );
                self.accepted = true;
            }
            "image_progress" => {
                let update: ImageGenerationProgress = serde_json::from_value(frame.data)?;
                ensure!(
                    self.accepted && update.valid_after(self.progress),
                    "invalid gateway image progress"
                );
                self.progress = Some(update);
                self.transcript.update(raw);
                return Ok(Some(update));
            }
            "image_chunk" => {
                let data = frame.data["data"].as_str().context("missing image chunk")?;
                ensure!(
                    self.accepted
                        && !data.is_empty()
                        && data.len() % 4 == 0
                        && frame.data["offset"].as_u64() == Some(self.data.len() as u64)
                        && self.data.len() + data.len() <= MAX_IMAGE_DATA,
                    "invalid gateway image chunk"
                );
                self.data.push_str(data);
            }
            "terminal" => {
                ensure!(
                    self.accepted
                        && frame.data["success"] == true
                        && frame.data["transcript_sha256"]
                            == hex::encode(self.transcript.clone().finalize()),
                    "gateway image completion was not authenticated"
                );
                self.terminal = Some(frame.data);
                return Ok(None);
            }
            _ => anyhow::bail!("unexpected gateway image frame"),
        }
        self.transcript.update(raw);
        Ok(None)
    }

    fn finish(
        mut self,
        id: &str,
        model: &str,
        size: ImageDimensions,
    ) -> AnyResult<Option<GeneratedImage>> {
        let terminal = self
            .terminal
            .take()
            .context("missing authenticated image completion")?;
        let mut result = terminal
            .get("result")
            .cloned()
            .context("missing gateway image result")?;
        if result == json!({"overflow_admitted":false}) {
            ensure!(
                self.data.is_empty() && self.progress.is_none(),
                "gateway declined after image execution"
            );
            return Ok(None);
        }
        ensure!(
            result["upstream_response_verified"] == true
                && !self.data.is_empty()
                && result["image_data_length"].as_u64() == Some(self.data.len() as u64),
            "invalid gateway image completion"
        );
        let mut image = result
            .get_mut("image")
            .context("missing gateway image metadata")?
            .take();
        image
            .as_object_mut()
            .context("invalid gateway image metadata")?
            .insert(
                "data".into(),
                Value::String(std::mem::take(&mut *self.data)),
            );
        let image: GeneratedImage = serde_json::from_value(image)?;
        ensure!(
            image.request_id == id
                && image.chat_model_id.as_deref() == Some(model)
                && image.source_image_ids.is_empty()
                && image.mime_type == "image/png"
                && (image.width, image.height) == (size.width, size.height)
                && image.evidence.state == SecurityState::Verified
                && image.evidence.provider_id == "chutes"
                && image.evidence.model_id == CHUTES_MODEL
                && image.evidence.attestation_protocol == "chutes-intel-tdx-nvidia-v1"
                && image.evidence.e2ee_protocol == "chutes-mlkem768-v1"
                && image.evidence.e2ee_encryption_version == 1
                && image
                    .evidence
                    .hard_expires_at_unix_seconds
                    .is_some_and(|expiry| expiry > now())
                && image.usage.request_id == id
                && image.usage.provider_id == "chutes"
                && image.usage.model_id == CHUTES_MODEL
                && image.usage.settled
                && image.usage.response_verified
                && image.usage.state == InvocationState::Completed
                && image.usage.completeness == UsageCompleteness::Final,
            "gateway image and settlement binding differs"
        );
        let bytes = Zeroizing::new(STANDARD.decode(&image.data)?);
        ensure!(
            bytes.len() <= MAX_GENERATED_IMAGE_BYTES
                && image.sha256 == hex::encode(Sha256::digest(&*bytes))
                && image::ImageReader::with_format(Cursor::new(&*bytes), image::ImageFormat::Png)
                    .into_dimensions()?
                    == (image.width, image.height),
            "gateway image content binding differs"
        );
        Ok(Some(image))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "0123456789abcdef0123456789abcdef";
    const SIZE: ImageDimensions = ImageDimensions {
        width: 1536,
        height: 1536,
    };

    fn frame(sequence: usize, kind: &str, data: &Value) -> Vec<u8> {
        let mut raw = serde_json::to_vec(&json!({
            "protocol":PROTOCOL, "request_id":ID, "sequence":sequence, "kind":kind, "data":data,
        }))
        .unwrap();
        raw.push(b'\n');
        raw
    }

    fn decline(reply: &Reply) -> Vec<u8> {
        frame(
            reply.sequence,
            "terminal",
            &json!({
                "success":true, "transcript_sha256":hex::encode(reply.transcript.clone().finalize()),
                "result":{"overflow_admitted":false},
            }),
        )
    }

    #[test]
    fn direct_admission_requires_a_bound_decline_before_generation() {
        let accepted = frame(0, "accepted", &json!({"request_id":ID}));
        let mut reply = Reply::default();
        reply.frame(ID, &accepted).unwrap();
        assert!(reply.finish(ID, "fixture-model", SIZE).is_err());

        let mut reply = Reply::default();
        reply.frame(ID, &accepted).unwrap();
        let bound = decline(&reply);
        let mut wrong: Value = serde_json::from_slice(&bound).unwrap();
        wrong["data"]["transcript_sha256"] = json!("00".repeat(32));
        assert!(
            reply
                .frame(ID, &serde_json::to_vec(&wrong).unwrap())
                .is_err()
        );

        let mut reply = Reply::default();
        reply.frame(ID, &accepted).unwrap();
        reply.frame(ID, &bound).unwrap();
        assert!(reply.finish(ID, "fixture-model", SIZE).unwrap().is_none());

        let mut reply = Reply::default();
        reply.frame(ID, &accepted).unwrap();
        reply
            .frame(
                ID,
                &frame(
                    1,
                    "image_progress",
                    &json!({"stage":"generating","percent":1}),
                ),
            )
            .unwrap();
        let late = decline(&reply);
        reply.frame(ID, &late).unwrap();
        assert!(reply.finish(ID, "fixture-model", SIZE).is_err());
    }

    #[test]
    fn desktop_requests_with_references_or_large_output_stay_direct() {
        let image = axiom_inference::ImageContent {
            mime_type: "image/png".into(),
            data: String::new(),
        };
        let references = [("reference", &image)];
        let mut request = ImageGenerationRequest {
            chat_model_id: "fixture-model",
            prompt: "A quiet lake",
            references: &[],
            dimensions: Some(SIZE),
            allow_long_queue: true,
        };
        assert!(eligible(&request).is_some());
        request.references = &references;
        assert!(eligible(&request).is_none());
        request.references = &[];
        request.dimensions = Some(ImageDimensions {
            width: 2048,
            height: 2048,
        });
        assert!(eligible(&request).is_none());
        request.dimensions = Some(SIZE);
        request.allow_long_queue = false;
        assert!(eligible(&request).is_none());
    }

    #[test]
    fn bound_terminal_returns_a_verified_settled_chutes_image() {
        let size = ImageDimensions {
            width: 256,
            height: 256,
        };
        let mut png = Cursor::new(Vec::new());
        image::RgbImage::new(size.width, size.height)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let bytes = png.into_inner();
        let data = STANDARD.encode(&bytes);
        let usage = axiom_inference::RequestUsage {
            request_id: ID.into(),
            model_id: CHUTES_MODEL.into(),
            provider_id: "chutes".into(),
            state: InvocationState::Completed,
            completeness: UsageCompleteness::Final,
            settled: true,
            response_verified: true,
            cost_microusd: Some("8000".into()),
            ..Default::default()
        };
        let metadata = json!({
            "requestId":ID, "chatModelId":"fixture-model", "mimeType":"image/png",
            "width":size.width, "height":size.height, "sha256":hex::encode(Sha256::digest(&bytes)),
            "usage":usage,
            "evidence":{
                "state":"verified", "provider_id":"chutes", "model_id":CHUTES_MODEL,
                "attestation_protocol":"chutes-intel-tdx-nvidia-v1", "e2ee_protocol":"chutes-mlkem768-v1",
                "e2ee_encryption_version":1, "trust_policy_version":"fixture", "verified_at_unix_seconds":now(),
                "hard_expires_at_unix_seconds":now()+180, "model_key_fingerprint":"01".repeat(32),
                "tls_spki_fingerprint":null, "checks":[], "provider_claims":[],
            }
        });
        let mut reply = Reply::default();
        reply
            .frame(ID, &frame(0, "accepted", &json!({"request_id":ID})))
            .unwrap();
        reply
            .frame(
                ID,
                &frame(1, "image_chunk", &json!({"offset":0, "data":data})),
            )
            .unwrap();
        let terminal = frame(
            2,
            "terminal",
            &json!({
                "success":true, "transcript_sha256":hex::encode(reply.transcript.clone().finalize()),
                "result":{"upstream_response_verified":true, "image":metadata, "image_data_length":data.len()}
            }),
        );
        reply.frame(ID, &terminal).unwrap();
        let image = reply.finish(ID, "fixture-model", size).unwrap().unwrap();
        assert_eq!(image.data, data);
        assert_eq!(image.usage.cost_microusd.as_deref(), Some("8000"));
    }
}
