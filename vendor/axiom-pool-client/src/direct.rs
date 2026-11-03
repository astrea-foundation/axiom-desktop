//! Operator-controlled direct SNP/NVIDIA worker client. No fixture or unchecked recipient path.
use crate::{
    VerifiedBilling, VerifiedOutput,
    attestation::{VerifiedWorker, VerifierConfig, bounded_json},
};
use anyhow::{Context, Result, ensure};
use axiom_pool_protocol::{
    GRANT_DOMAIN, PROTOCOL,
    authorization::{self, ClientKey},
    crypto::RequestContext,
    frames::FrameVerifier,
    sign, strict,
    types::{
        Binding, Envelope, Grant, Input, Limits, Operation, Payload, WorkerEvidence, WorkerIdentity,
    },
};
use base64::{Engine, engine::general_purpose::STANDARD};
use ed25519_dalek::pkcs8::DecodePrivateKey;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectConfig {
    pub origin: String,
    pub connect_ip: Option<IpAddr>,
    pub model_manifest: String,
    pub issuer_key_file: PathBuf,
    pub verifier: VerifierConfig,
    #[serde(default)]
    pub vision: bool,
    #[serde(default)]
    pub tools: bool,
    #[serde(default)]
    pub image_generation: bool,
}

pub struct DirectClient {
    config: DirectConfig,
    connection: tokio::sync::RwLock<Option<(VerifiedWorker, reqwest::Client)>>,
    verification_lock: tokio::sync::Mutex<()>,
    admission: tokio::sync::RwLock<Option<(WorkerIdentity, u64)>>,
    renewal_lock: tokio::sync::Mutex<()>,
    epoch: std::sync::atomic::AtomicU64,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct DirectTimings {
    pub verification_ms: u64,
    pub admission_ms: u64,
    pub first_frame_ms: Option<u64>,
    pub first_reasoning_ms: Option<u64>,
    pub first_text_ms: Option<u64>,
    pub total_ms: u64,
    pub proof_reused: bool,
    pub admission_reused: bool,
}

fn elapsed_ms(start: Instant) -> u64 {
    start.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
}
#[derive(Serialize)]
struct Lease<'a> {
    protocol: &'a str,
    worker: &'a WorkerIdentity,
    expires_at: u64,
    max_sequences: u32,
    token_budget: u32,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn route(
    builder: reqwest::ClientBuilder,
    origin: &str,
    connect_ip: Option<IpAddr>,
) -> Result<reqwest::ClientBuilder> {
    let Some(ip) = connect_ip else {
        return Ok(builder);
    };
    let url = reqwest::Url::parse(origin)?;
    let host = url
        .domain()
        .context("IP routing requires a logical DNS origin")?;
    Ok(builder.resolve(
        host,
        SocketAddr::new(ip, url.port_or_known_default().unwrap_or(443)),
    ))
}

async fn admit(
    client: &reqwest::Client,
    transport: &str,
    lease: &axiom_pool_protocol::SignedDocument,
) -> Result<()> {
    // Only the operator lease is repeated. Each attempt runs the guest's
    // full checks; no grant, prompt, or inference ciphertext exists yet.
    for attempt in 0..3 {
        let response = client
            .post(format!("{transport}/v1/admission"))
            .json(lease)
            .send()
            .await?;
        let status = response.status();
        drop(response);
        if status.is_success() {
            return Ok(());
        }
        ensure!(
            status == reqwest::StatusCode::FORBIDDEN && attempt < 2,
            "worker rejected local full attestation or admission"
        );
        // Guest-side full appraisal fetches AMD collateral too. Its current
        // measured verifier rejects a throttled fetch, so allow the vendor's
        // observed ten-second window before resubmitting this public lease.
        tokio::time::sleep(Duration::from_secs(10)).await;
    }
    unreachable!("bounded admission attempts return on the final response")
}

impl DirectClient {
    pub fn new(config: DirectConfig) -> Result<Self> {
        axiom_pool_protocol::origin(&config.origin, false)?;
        axiom_pool_protocol::hex_bytes(&config.model_manifest, 32)?;
        ensure!(
            config.issuer_key_file.is_absolute(),
            "absolute operator key path required"
        );
        Ok(Self {
            config,
            connection: tokio::sync::RwLock::new(None),
            verification_lock: tokio::sync::Mutex::new(()),
            admission: tokio::sync::RwLock::new(None),
            renewal_lock: tokio::sync::Mutex::new(()),
            epoch: std::sync::atomic::AtomicU64::new(0),
        })
    }
    /// Public, independently verified hardware and signed release material.
    pub async fn public_evidence(&self) -> Option<serde_json::Value> {
        self.connection
            .read()
            .await
            .as_ref()
            .and_then(|(worker, _)| worker.public_evidence())
    }
    pub async fn invalidate(&self) {
        self.epoch.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        *self.connection.write().await = None;
        *self.admission.write().await = None;
    }
    async fn verified_until(
        &self,
        deadline: u64,
        cancel: &CancellationToken,
    ) -> Result<(VerifiedWorker, reqwest::Client, bool)> {
        // A usable proof never waits behind an independent background refresh.
        if let Some((worker, client)) = self.connection.read().await.as_ref()
            && worker.expires_at() > deadline.saturating_add(5)
        {
            return Ok((worker.clone(), client.clone(), true));
        }
        let _refresh = self.verification_lock.lock().await;
        if let Some((worker, client)) = self.connection.read().await.as_ref()
            && worker.expires_at() > deadline.saturating_add(5)
        {
            return Ok((worker.clone(), client.clone(), true));
        }
        let epoch = self.epoch.load(std::sync::atomic::Ordering::Acquire);
        let result = self.verify_fresh(cancel).await;
        let mut slot = self.connection.write().await;
        ensure!(
            self.epoch.load(std::sync::atomic::Ordering::Acquire) == epoch,
            "verification invalidated during refresh"
        );
        match result {
            Ok((worker, client)) => {
                ensure!(
                    worker.expires_at() > deadline,
                    "fresh proof cannot cover request deadline"
                );
                *slot = Some((worker.clone(), client.clone()));
                Ok((worker, client, false))
            }
            Err(error) => {
                *slot = None;
                Err(error)
            }
        }
    }
    async fn verify_fresh(
        &self,
        cancel: &CancellationToken,
    ) -> Result<(VerifiedWorker, reqwest::Client)> {
        let cfg = &self.config;
        let transport = &cfg.origin;
        let challenge = axiom_pool_protocol::random_challenge();
        // This client is deliberately scoped to one public challenge request.
        // It is never used to send a grant, prompt, key, or inference ciphertext.
        let bootstrap = route(reqwest::Client::builder(), &cfg.origin, cfg.connect_ip)?
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .danger_accept_invalid_certs(true)
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(70))
            .build()?;
        let response = bootstrap
            .post(format!("{transport}/v1/attestation"))
            .json(&json!({"challenge":challenge}))
            .send()
            .await?
            .error_for_status()?;
        let value = bounded_json(response, 8 * 1024 * 1024).await?;
        drop(bootstrap);
        let evidence: WorkerEvidence = strict::decode(&serde_json::to_vec(&value)?)?;
        let expected: WorkerIdentity =
            strict::decode(&serde_json::to_vec(&evidence.document["worker"])?)?;
        ensure!(
            expected.evidence_profile == "axiom-snp-nvidia-v1"
                && expected.logical_origin == cfg.origin
                && expected.model_manifest == cfg.model_manifest,
            "unexpected direct recipient"
        );
        let tls = STANDARD.decode(
            evidence.document["tls_certificate"]
                .as_str()
                .context("CPU-bound TLS certificate missing")?,
        )?;
        ensure!(
            !tls.is_empty() && tls.len() <= 16 * 1024,
            "TLS certificate limit"
        );
        let client = route(reqwest::Client::builder(), &cfg.origin, cfg.connect_ip)?
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .tls_built_in_root_certs(false)
            .add_root_certificate(reqwest::Certificate::from_der(&tls)?)
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(200))
            .build()?;
        let public_evidence = serde_json::to_value(&evidence)?;
        let verified = cfg
            .verifier
            .verify(
                &client,
                evidence,
                &challenge,
                &expected,
                cfg.verifier.minimum_policy_sequence,
                cancel,
            )
            .await?;
        ensure!(
            !cfg.vision || verified.supports("vision"),
            "vision is not authorized by the signed workload release"
        );
        ensure!(
            !cfg.tools || verified.supports("tools"),
            "tools are not authorized by the signed workload release"
        );
        Ok((verified.with_public_evidence(public_evidence), client))
    }
    pub async fn probe(&self, cancel: &CancellationToken) -> Result<VerifiedWorker> {
        tokio::select! { _=cancel.cancelled()=>anyhow::bail!("cancelled"),
        result=self.verified_until(now()+180,cancel)=>Ok(result?.0) }
    }
    /// Prewarm the same bounded attestation and admission used by inference.
    /// This performs no inference and does not create a request grant.
    pub async fn prepare(&self, cancel: &CancellationToken) -> Result<VerifiedWorker> {
        self.prepare_until(now() + 180, cancel).await
    }
    /// Renew with thirty seconds of extra headroom before a foreground request
    /// would have to wait. This sends only public evidence and admission.
    pub async fn prewarm(&self, cancel: &CancellationToken) -> Result<VerifiedWorker> {
        self.prepare_until(now() + 210, cancel).await
    }
    async fn prepare_until(
        &self,
        deadline: u64,
        cancel: &CancellationToken,
    ) -> Result<VerifiedWorker> {
        tokio::select! {_=cancel.cancelled()=>anyhow::bail!("cancelled"),result=async{
            let (worker,client,_)=self.verified_until(deadline,cancel).await?;
            self.ensure_admitted(&worker,&client,deadline).await?;Ok(worker)
        }=>result}
    }
    fn issuer(&self) -> Result<ed25519_dalek::SigningKey> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            ensure!(
                self.config.issuer_key_file.metadata()?.permissions().mode() & 0o077 == 0,
                "private operator issuer key permissions required"
            );
        }
        #[cfg(not(unix))]
        anyhow::bail!("direct operator mode requires protected Unix key storage");
        let pem = Zeroizing::new(std::fs::read_to_string(&self.config.issuer_key_file)?);
        Ok(ed25519_dalek::SigningKey::from_pkcs8_pem(&pem)?)
    }
    async fn ensure_admitted(
        &self,
        worker: &VerifiedWorker,
        client: &reqwest::Client,
        deadline: u64,
    ) -> Result<bool> {
        let covers = |value: &Option<(WorkerIdentity, u64)>| {
            value.as_ref().is_some_and(|(identity, expiry)| {
                identity == worker.identity() && *expiry > deadline.saturating_add(5)
            })
        };
        if covers(&*self.admission.read().await) {
            return Ok(true);
        }
        let _renew = self.renewal_lock.lock().await;
        if covers(&*self.admission.read().await) {
            return Ok(true);
        }
        let epoch = self.epoch.load(std::sync::atomic::Ordering::Acquire);
        let expiry = worker.expires_at().min(now() + 240);
        let lease = sign(
            &Lease {
                protocol: PROTOCOL,
                worker: worker.identity(),
                expires_at: expiry,
                max_sequences: 4,
                token_budget: 65536,
            },
            &self.issuer()?,
            "axiom-pool-admission-v1",
        )?;
        let result = admit(client, &self.config.origin, &lease).await;
        let mut slot = self.admission.write().await;
        ensure!(
            self.epoch.load(std::sync::atomic::Ordering::Acquire) == epoch,
            "admission invalidated during renewal"
        );
        if let Err(error) = result {
            *slot = None;
            return Err(error);
        }
        ensure!(expiry > deadline, "admission cannot cover request deadline");
        *slot = Some((worker.identity().clone(), expiry));
        Ok(false)
    }
    pub async fn execute_with<F, Fut>(
        &self,
        input: Input,
        limits: Limits,
        pinned: Option<&VerifiedWorker>,
        on_frame: F,
        cancel: CancellationToken,
    ) -> Result<(VerifiedWorker, VerifiedOutput)>
    where
        F: FnMut(axiom_pool_protocol::frames::Frame) -> Fut + Send,
        Fut: std::future::Future<Output = Result<()>> + Send,
    {
        let result = tokio::select! { _=cancel.cancelled()=>Err(anyhow::anyhow!("cancelled")),
        result=self.execute_inner(input,limits,pinned,on_frame,&cancel)=>result };
        // Discard a failed transport/session, but never replay its inference.
        if result.is_err() {
            self.invalidate().await;
        }
        result
    }
    async fn execute_inner<F, Fut>(
        &self,
        input: Input,
        limits: Limits,
        pinned: Option<&VerifiedWorker>,
        mut on_frame: F,
        cancel: &CancellationToken,
    ) -> Result<(VerifiedWorker, VerifiedOutput)>
    where
        F: FnMut(axiom_pool_protocol::frames::Frame) -> Fut + Send,
        Fut: std::future::Future<Output = Result<()>> + Send,
    {
        let started = Instant::now();
        let mut timings = DirectTimings::default();
        limits.validate(now())?;
        ensure!(
            limits.deadline <= now() + 180,
            "direct request deadline exceeds fresh proof window"
        );
        let image_operation = matches!(&input, Input::Image { .. });
        let (verified, client, reused) = self.verified_until(limits.deadline, cancel).await?;
        timings.verification_ms = elapsed_ms(started);
        timings.proof_reused = reused;
        let has_images = matches!(&input, Input::Chat { messages, .. }
            if messages.iter().any(|message| !message.images.is_empty()))
            || matches!(&input, Input::Image { references, .. } if !references.is_empty());
        ensure!(
            !has_images || (self.config.vision && verified.supports("vision")),
            "vision requires installed capability and fresh signed workload authorization"
        );
        let tool_names = match &input {
            Input::Chat { tools, .. } => axiom_pool_protocol::tools::definitions(tools)?,
            _ => Default::default(),
        };
        let has_tools = matches!(&input,Input::Chat{messages,..} if messages.iter().any(|m| !m.tool_calls.is_empty() || m.tool_call_id.is_some()))
            || !tool_names.is_empty();
        ensure!(
            !has_tools || (self.config.tools && verified.supports("tools")),
            "tools require installed and signed authorization"
        );
        let expected = verified.identity().clone();
        if let Some(previous) = pinned {
            ensure!(
                previous.identity() == &expected,
                "direct worker changed; fresh session required"
            );
        }
        let cfg = &self.config;
        let transport = &cfg.origin;
        let issuer = self.issuer()?;
        let admission_started = Instant::now();
        timings.admission_reused = self
            .ensure_admitted(&verified, &client, limits.deadline)
            .await?;
        timings.admission_ms = elapsed_ms(admission_started);
        let key = loop {
            if let Ok(k) = p256::ecdsa::SigningKey::from_bytes((&rand::random::<[u8; 32]>()).into())
            {
                break k;
            }
        };
        let binding = Binding {
            account: axiom_pool_protocol::random_id(),
            account_session: axiom_pool_protocol::random_id(),
            client_jkt: ClientKey::from_signing(&key).thumbprint()?,
            request_id: axiom_pool_protocol::random_id(),
            attempt_id: axiom_pool_protocol::random_id(),
            reservation_id: axiom_pool_protocol::random_id(),
            fencing: 1,
            worker: expected.clone(),
            grant_jti: axiom_pool_protocol::random_id(),
            operation: if image_operation {
                Operation::Image
            } else {
                Operation::Chat
            },
            limits,
        };
        let grant = Grant {
            protocol: PROTOCOL.into(),
            issuer: cfg.origin.clone(),
            audience: axiom_pool_protocol::AUDIENCE.into(),
            issued_at: now(),
            expires_at: now() + 120,
            binding: binding.clone(),
            price_version: "pool-free-preview-v1".into(),
            max_charge_microusd: 0,
            pricing: None,
        };
        grant.validate(&cfg.origin, &expected, now(), false)?;
        input.validate(&binding)?;
        let expected_input_pixels: u64 = match &input {
            Input::Chat { messages, .. } => messages
                .iter()
                .flat_map(|message| &message.images)
                .map(|image| image.pixels())
                .sum(),
            Input::Image { references, .. } => references.iter().map(|image| image.pixels()).sum(),
        };
        let grant_doc = sign(&grant, &issuer, GRANT_DOMAIN)?;
        let token = Zeroizing::new(serde_json::to_string(&grant_doc)?);
        let exact_binding = serde_json::to_string(&binding)?;
        let payload = Zeroizing::new(serde_json::to_string(&Payload {
            binding: exact_binding.clone(),
            input,
        })?);
        let proof = authorization::proof(
            &key,
            &token,
            &format!("{}/v1/exchange", cfg.origin),
            &expected.encryption_key,
            &payload,
            now(),
        )?;
        let envelope = Envelope {
            protocol: PROTOCOL.into(),
            grant: token.to_string(),
            proof,
            payload: payload.to_string(),
        };
        let plaintext = Zeroizing::new(serde_json::to_vec(&envelope)?);
        ensure!(
            verified.expires_at() > grant.binding.limits.deadline,
            "fresh evidence required for the complete bounded request"
        );
        let (sealed, context) = RequestContext::seal(&expected.encryption_key, &plaintext)?;
        let wire = sealed.digest()?;
        let response = client
            .post(format!("{transport}/v1/exchange"))
            .json(&sealed)
            .send()
            .await?
            .error_for_status()?;
        let nonce = response
            .headers()
            .get("axiom-pool-response-nonce")
            .and_then(|v| v.to_str().ok())
            .context("response nonce missing")?;
        let mut frames = FrameVerifier::new(
            binding,
            &exact_binding,
            &wire,
            &axiom_pool_protocol::hex_bytes(nonce, 32)?,
        )?;
        let mut reader = context.response(nonce)?;
        let mut stream = response.bytes_stream();
        let (mut text, mut reasoning, mut image) = (String::new(), String::new(), Vec::new());
        let mut tool_calls = Vec::new();
        while let Some(bytes) = stream.next().await {
            ensure!(verified.expires_at() > now(), "proof expired during stream");
            for fragment in bytes?.chunks(axiom_pool_protocol::MAX_FRAME) {
                for opened in reader.push(fragment)? {
                    let frame = frames.push(&opened)?;
                    timings
                        .first_frame_ms
                        .get_or_insert_with(|| elapsed_ms(started));
                    match frame.kind.as_str() {
                        "tool_call" => {
                            tool_calls.push(strict::decode::<axiom_pool_protocol::tools::ToolCall>(
                                &serde_json::to_vec(&frame.data)?,
                            )?)
                        }
                        "text_delta" => {
                            timings
                                .first_text_ms
                                .get_or_insert_with(|| elapsed_ms(started));
                            text.push_str(frame.data["text"].as_str().context("text delta")?)
                        }
                        "reasoning_delta" => {
                            timings
                                .first_reasoning_ms
                                .get_or_insert_with(|| elapsed_ms(started));
                            reasoning
                                .push_str(frame.data["text"].as_str().context("reasoning delta")?);
                        }
                        "image_chunk" => {
                            image.extend(STANDARD.decode(
                                frame.data["bytes_base64"].as_str().context("image bytes")?,
                            )?)
                        }
                        _ => {}
                    }
                    on_frame(frame).await?;
                }
            }
        }
        reader.finish()?;
        let terminal = frames.finish()?;
        ensure!(
            terminal.usage.input_image_pixels == expected_input_pixels,
            "authenticated input image accounting differs from the originating request"
        );
        ensure!(
            terminal.success && verified.expires_at() > now(),
            "authenticated inference did not complete"
        );
        let receipt: axiom_pool_protocol::types::Receipt = axiom_pool_protocol::verify(
            &terminal.receipt,
            &ed25519_dalek::VerifyingKey::from_bytes(
                &axiom_pool_protocol::hex_bytes(&expected.identity_key, 32)?
                    .try_into()
                    .map_err(|_| anyhow::anyhow!("identity key size"))?,
            )?,
            axiom_pool_protocol::RECEIPT_DOMAIN,
        )?;
        ensure!(
            receipt.price_version == grant.price_version
                && receipt.charge_microusd == grant.charge(&receipt.usage)?,
            "receipt price differs"
        );
        if image_operation {
            ensure!(
                image.len() >= 24 && image[..8] == *b"\x89PNG\r\n\x1a\n",
                "PNG result missing"
            );
        } else {
            ensure!(
                !text.is_empty() || !reasoning.is_empty() || !tool_calls.is_empty(),
                "text response missing"
            );
        }
        ensure!(
            tool_calls
                .iter()
                .all(|call| tool_names.contains(&call.function.name)),
            "worker returned an unadvertised tool call"
        );
        timings.total_ms = elapsed_ms(started);
        Ok((
            verified,
            VerifiedOutput {
                text,
                reasoning,
                image: if image_operation { Some(image) } else { None },
                terminal,
                tool_calls,
                direct_timings: Some(timings),
                billing: VerifiedBilling {
                    price_version: grant.price_version,
                    charge_microusd: 0,
                    maximum_charge_microusd: 0,
                    pricing: None,
                },
            },
        ))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    #[tokio::test]
    async fn verified_cache_covers_the_entire_deadline_and_never_falls_back_after_refresh_failure()
    {
        let f = axiom_pool_protocol::test_support::Fixture::new();
        let cfg:DirectConfig=serde_json::from_value(json!({"origin":"https://worker.invalid:9","connect_ip":"127.0.0.1",
            "model_manifest":"11".repeat(32),"issuer_key_file":"/not-read-by-probe.pem","verifier":{"binary":"/not-run","binary_sha256":"22".repeat(32),"publisher_key":"33".repeat(32),"minimum_policy_sequence":1}})).unwrap();
        let client = DirectClient::new(cfg).unwrap();
        let expiry = now() + 240;
        *client.connection.write().await = Some((
            VerifiedWorker::for_cache_test(f.worker.clone(), expiry),
            reqwest::Client::new(),
        ));
        let pending_refresh = client.verification_lock.lock().await;
        let (worker, _, reused) = tokio::time::timeout(
            Duration::from_millis(100),
            client.verified_until(now() + 180, &CancellationToken::new()),
        )
        .await
        .unwrap()
        .unwrap();
        drop(pending_refresh);
        assert!(reused);
        assert_eq!(worker.identity(), &f.worker);
        assert_eq!(worker.expires_at(), expiry);
        assert!(
            client
                .verified_until(expiry, &CancellationToken::new())
                .await
                .is_err()
        );
        assert!(client.connection.write().await.is_none());
        *client.admission.write().await = Some((f.worker, expiry));
        client.invalidate().await;
        assert!(client.admission.write().await.is_none());
        assert!(client.public_evidence().await.is_none());
    }

    fn admission_server(statuses: &'static [u16]) -> (String, std::thread::JoinHandle<()>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let task = std::thread::spawn(move || {
            for status in statuses {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                let mut request = Vec::new();
                let mut buffer = [0u8; 4096];
                loop {
                    let count = stream.read(&mut buffer).unwrap();
                    assert!(count > 0);
                    request.extend_from_slice(&buffer[..count]);
                    if let Some(end) = request.windows(4).position(|v| v == b"\r\n\r\n") {
                        let headers = std::str::from_utf8(&request[..end]).unwrap();
                        assert!(headers.starts_with("POST /v1/admission HTTP/1.1\r\n"));
                        let length: usize = headers
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length: ")
                                    .map(|value| value.parse().unwrap())
                            })
                            .unwrap();
                        if request.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                write!(
                    stream,
                    "HTTP/1.1 {status} Test\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                )
                .unwrap();
            }
        });
        (origin, task)
    }

    async fn check_admission(statuses: &'static [u16], success: bool) {
        let (origin, server) = admission_server(statuses);
        let lease = axiom_pool_protocol::SignedDocument {
            payload: "test-only-lease".into(),
            signature: "test-only-signature".into(),
        };
        let result = admit(
            &reqwest::Client::builder().no_proxy().build().unwrap(),
            &origin,
            &lease,
        )
        .await;
        assert_eq!(result.is_ok(), success);
        server.join().unwrap();
    }

    #[tokio::test]
    async fn rejected_admission_can_recover_before_dispatch() {
        check_admission(&[403, 200], true).await;
    }

    #[tokio::test]
    async fn persistent_admission_rejection_is_bounded() {
        check_admission(&[403, 403, 403], false).await;
    }

    #[tokio::test]
    async fn other_errors_are_not_retried() {
        check_admission(&[400], false).await;
    }
}
