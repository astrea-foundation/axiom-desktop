//! Native pool client. Recipient keys are accepted only by the installed verifier.
#![forbid(unsafe_code)]
pub mod attestation;
pub mod direct;

use anyhow::{Context, Result, ensure};
use axiom_pool_protocol::{
    GRANT_DOMAIN, PROTOCOL, SignedDocument,
    authorization::{self, ClientKey},
    crypto::RequestContext,
    frames::{Frame, FrameVerifier, Terminal},
    strict,
    types::{Envelope, Grant, Input, Limits, Payload, Ticket, WorkerEvidence},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use futures_util::StreamExt;
use secrecy::{ExposeSecret, SecretString};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

#[derive(Debug)]
pub struct InsufficientCredit;

#[derive(Debug)]
pub struct AuthenticationRejected;
impl std::fmt::Display for AuthenticationRejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Your Axiom session is no longer valid. Sign in again.")
    }
}
impl std::error::Error for AuthenticationRejected {}

impl std::fmt::Display for InsufficientCredit {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Insufficient balance. Top up your Axiom balance to continue.")
    }
}

impl std::error::Error for InsufficientCredit {}

#[derive(Debug)]
pub struct TrialCapacity {
    pub retry_at: Option<i64>,
}

impl std::fmt::Display for TrialCapacity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Free trials are currently at capacity. Please try again later.")
    }
}

impl std::error::Error for TrialCapacity {}

fn unavailable_error(body: &Value) -> anyhow::Error {
    if body["detail"]["code"] == "trial_capacity" {
        TrialCapacity {
            retry_at: body["detail"]["retry_at_unix_seconds"]
                .as_i64()
                .filter(|at| *at > 0),
        }
        .into()
    } else {
        anyhow::anyhow!("pool metadata request rejected")
    }
}

fn check_status(status: reqwest::StatusCode, detail: &'static str) -> Result<()> {
    if status == reqwest::StatusCode::PAYMENT_REQUIRED {
        return Err(InsufficientCredit.into());
    }
    ensure!(status.is_success(), detail);
    Ok(())
}

pub struct PoolClient {
    http: reqwest::Client,
    origin: String,
    issuer_key: ed25519_dalek::VerifyingKey,
    credential: SecretString,
    key: p256::ecdsa::SigningKey,
    verifier: attestation::VerifierConfig,
    minimum_sequence: Arc<AtomicU64>,
    fixture: bool,
    maximum_charge_microusd: u64,
}
pub struct VerifiedOutput {
    pub tool_calls: Vec<axiom_pool_protocol::tools::ToolCall>,
    pub direct_timings: Option<direct::DirectTimings>,
    pub text: String,
    pub reasoning: String,
    pub image: Option<Vec<u8>>,
    pub terminal: Terminal,
    pub billing: VerifiedBilling,
}
#[derive(Clone, Debug, serde::Serialize)]
pub struct VerifiedBilling {
    pub price_version: String,
    pub charge_microusd: u64,
    pub maximum_charge_microusd: u64,
    pub pricing: Option<axiom_pool_protocol::pricing::Price>,
    pub settled: bool,
}
pub struct PreparedRequest {
    grant: Grant,
    binding_bytes: String,
    input: Input,
    reservation: String,
    ticket: SignedDocument,
    worker: attestation::VerifiedWorker,
    grant_document: SignedDocument,
}
impl PoolClient {
    pub fn new(
        origin: String,
        issuer_key: &str,
        credential: SecretString,
        verifier: attestation::VerifierConfig,
    ) -> Result<Self> {
        Self::construct(origin, issuer_key, credential, verifier, false, None)
    }
    /// A trusted development caller may add a CA for an exact HTTPS loopback
    /// relay. This does not relax worker attestation or provider trust.
    pub fn new_with_local_relay_ca(
        origin: String,
        issuer_key: &str,
        credential: SecretString,
        verifier: attestation::VerifierConfig,
        ca_pem: &[u8],
    ) -> Result<Self> {
        Self::construct(
            origin,
            issuer_key,
            credential,
            verifier,
            false,
            Some(ca_pem),
        )
    }
    #[cfg(feature = "test-fixture")]
    pub fn fixture(
        origin: String,
        issuer_key: &str,
        credential: SecretString,
        verifier: attestation::VerifierConfig,
    ) -> Result<Self> {
        ensure!(
            std::env::var("AXIOM_POOL_TEST_ONLY").as_deref() == Ok("1"),
            "explicit test process required"
        );
        Self::construct(origin, issuer_key, credential, verifier, true, None)
    }
    fn construct(
        origin: String,
        issuer_key: &str,
        credential: SecretString,
        verifier: attestation::VerifierConfig,
        fixture: bool,
        ca_pem: Option<&[u8]>,
    ) -> Result<Self> {
        axiom_pool_protocol::origin(&origin, fixture)?;
        let mut http = reqwest::Client::builder()
            .user_agent("axiom-native/1")
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(610));
        if let Some(pem) = ca_pem {
            // Do not allow a local development root to authenticate a remote
            // service, or accept an unbounded certificate bundle.
            let url = reqwest::Url::parse(&origin)?;
            ensure!(
                pem.len() <= 64 * 1024
                    && url.scheme() == "https"
                    && matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]")),
                "local relay CA requires a loopback HTTPS origin"
            );
            let mut certificates = reqwest::Certificate::from_pem_bundle(pem)?;
            ensure!(
                certificates.len() == 1,
                "one local relay CA certificate required"
            );
            http = http.add_root_certificate(certificates.remove(0));
        }
        let public = ed25519_dalek::VerifyingKey::from_bytes(
            &axiom_pool_protocol::hex_bytes(issuer_key, 32)?
                .try_into()
                .map_err(|_| anyhow::anyhow!("key length"))?,
        )?;
        let key = loop {
            if let Ok(k) = p256::ecdsa::SigningKey::from_bytes((&rand::random::<[u8; 32]>()).into())
            {
                break k;
            }
        };
        Ok(Self {
            http: http.build()?,
            origin,
            issuer_key: public,
            credential,
            key,
            minimum_sequence: Arc::new(AtomicU64::new(verifier.minimum_policy_sequence)),
            verifier,
            fixture,
            maximum_charge_microusd: 0,
        })
    }

    /// Native financial policy. The renderer and the worker cannot raise it.
    /// Zero retains the existing free-only behavior.
    pub fn with_maximum_charge(mut self, maximum_microusd: u64) -> Result<Self> {
        ensure!(
            maximum_microusd <= axiom_pool_protocol::pricing::MAX_MONEY,
            "invalid accepted charge limit"
        );
        self.maximum_charge_microusd = maximum_microusd;
        Ok(self)
    }
    async fn metadata(
        &self,
        route: &str,
        body: Value,
        cancel: &CancellationToken,
    ) -> Result<Value> {
        let response = tokio::select! {_ = cancel.cancelled()=>anyhow::bail!("cancelled"),response=self.http.post(format!("{}/api/v1/pool/{route}",self.origin)).bearer_auth(self.credential.expose_secret()).json(&body).send()=>response?};
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(AuthenticationRejected.into());
        }
        if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(direct::CapacityUnavailable::Preparation.into());
        }
        if response.status() == reqwest::StatusCode::SERVICE_UNAVAILABLE {
            let body = tokio::select! {
                _ = cancel.cancelled() => anyhow::bail!("cancelled"),
                body = attestation::bounded_json(response, 4096) => body.unwrap_or(Value::Null),
            };
            return Err(unavailable_error(&body));
        }
        check_status(response.status(), "pool metadata request rejected")?;
        attestation::bounded_json(response, 8 * 1024 * 1024).await
    }
    pub async fn prepare(
        &self,
        model: &str,
        input: Input,
        limits: Limits,
        request_id: Option<String>,
        cancel: &CancellationToken,
    ) -> Result<PreparedRequest> {
        self.prepare_pinned(model, input, limits, request_id, None, cancel)
            .await
    }
    pub async fn probe(
        &self,
        model: &str,
        cancel: &CancellationToken,
    ) -> Result<attestation::VerifiedWorker> {
        let challenge = axiom_pool_protocol::random_challenge();
        let value = self
            .metadata(
                "probe",
                json!({"model_manifest":model,"challenge":challenge}),
                cancel,
            )
            .await?;
        let expected: axiom_pool_protocol::types::WorkerIdentity =
            strict::decode(&serde_json::to_vec(&value["worker"])?)?;
        ensure!(expected.model_manifest == model, "probe model mismatch");
        let evidence: WorkerEvidence = strict::decode(&serde_json::to_vec(&value["evidence"])?)?;
        let worker = self
            .verifier
            .verify(
                &self.http,
                evidence,
                &challenge,
                &expected,
                self.minimum_sequence.load(Ordering::Acquire),
                cancel,
            )
            .await?;
        self.minimum_sequence
            .fetch_max(worker.policy_sequence(), Ordering::AcqRel);
        Ok(worker)
    }
    pub async fn prepare_pinned(
        &self,
        model: &str,
        input: Input,
        limits: Limits,
        request_id: Option<String>,
        pinned: Option<&attestation::VerifiedWorker>,
        cancel: &CancellationToken,
    ) -> Result<PreparedRequest> {
        let operation = match &input {
            Input::Chat { .. } => "chat",
            Input::Image { .. } => "image",
        };
        let challenge = axiom_pool_protocol::random_challenge();
        let key = ClientKey::from_signing(&self.key);
        let jkt = key.thumbprint()?;
        let request_id = request_id.unwrap_or_else(axiom_pool_protocol::random_id);
        let value=self.metadata("reserve",json!({"request_id":request_id,"model_manifest":model,"operation":operation,"limits":limits,"client_key":key,"challenge":challenge,"preferred_worker":pinned.map(|p|p.identity())}),cancel).await?;
        let ticket: SignedDocument = strict::decode(&serde_json::to_vec(&value["ticket"])?)?;
        let route: Ticket = axiom_pool_protocol::verify(
            &ticket,
            &self.issuer_key,
            axiom_pool_protocol::TICKET_DOMAIN,
        )?;
        ensure!(
            route.protocol == PROTOCOL
                && route.expires_at > now()
                && route.binding.request_id == request_id
                && route.binding.client_jkt == jkt
                && route.binding.limits == limits
                && route.binding.worker.model_manifest == model,
            "reservation identity mismatch"
        );
        if let Some(pinned) = pinned {
            ensure!(
                pinned.identity() == &route.binding.worker,
                "worker changed; establish a new session"
            );
        }
        let evidence: WorkerEvidence = strict::decode(&serde_json::to_vec(&value["evidence"])?)?;
        let worker = self
            .verifier
            .verify(
                &self.http,
                evidence,
                &challenge,
                &route.binding.worker,
                self.minimum_sequence.load(Ordering::Acquire),
                cancel,
            )
            .await?;
        self.minimum_sequence
            .fetch_max(worker.policy_sequence(), Ordering::AcqRel);
        let reservation = route.binding.reservation_id.clone();
        let value = self
            .metadata("grants", json!({"reservation_id":reservation}), cancel)
            .await?;
        let doc: SignedDocument = strict::decode(&serde_json::to_vec(&value["grant"])?)?;
        let grant: Grant = axiom_pool_protocol::verify(&doc, &self.issuer_key, GRANT_DOMAIN)?;
        grant.validate(&self.origin, &route.binding.worker, now(), self.fixture)?;
        ensure!(grant.binding == route.binding, "grant/reservation mismatch");
        input.validate(&grant.binding)?;
        // Prices are signed by the controller, independently of worker trust.
        // A native caller must opt into a positive per-request cost ceiling.
        if grant.max_charge_microusd > self.maximum_charge_microusd
            || (grant.pricing.is_none() && grant.price_version != "pool-free-preview-v1")
        {
            let _ = self
                .metadata(
                    "control",
                    json!({"reservation_id":reservation,"action":"cancel"}),
                    cancel,
                )
                .await;
            anyhow::bail!("unaccepted price");
        }
        let binding_bytes = serde_json::to_string(&grant.binding)?;
        Ok(PreparedRequest {
            grant,
            binding_bytes,
            input,
            reservation,
            ticket,
            worker,
            grant_document: doc,
        })
    }
    pub async fn execute(
        &self,
        prepared: PreparedRequest,
        mut provisional: impl FnMut(Frame),
        cancel: CancellationToken,
    ) -> Result<VerifiedOutput> {
        self.execute_with(
            prepared,
            move |frame| {
                provisional(frame);
                std::future::ready(Ok(()))
            },
            cancel,
        )
        .await
    }
    pub async fn execute_with<F, Fut>(
        &self,
        prepared: PreparedRequest,
        mut provisional: F,
        cancel: CancellationToken,
    ) -> Result<VerifiedOutput>
    where
        F: FnMut(Frame) -> Fut,
        Fut: std::future::Future<Output = Result<()>>,
    {
        ensure!(prepared.worker.expires_at() > now(), "worker proof expired");
        let payload = Zeroizing::new(serde_json::to_string(&Payload {
            binding: prepared.binding_bytes.clone(),
            input: prepared.input,
        })?);
        let PreparedRequest {
            grant,
            grant_document: doc,
            binding_bytes,
            ticket,
            reservation,
            mut worker,
            ..
        } = prepared;
        let token = Zeroizing::new(serde_json::to_string(&doc)?);
        let proof = authorization::proof(
            &self.key,
            &token,
            &format!("{}/v1/exchange", worker.identity().logical_origin),
            &worker.identity().encryption_key,
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
        let (sealed, context) =
            RequestContext::seal(&worker.identity().encryption_key, &plaintext)?;
        let wire_hash = sealed.digest()?;
        let response = tokio::select! {_=cancel.cancelled()=>anyhow::bail!("cancelled before dispatch"),r=self.http.post(format!("{}/api/v1/pool/exchange",self.origin)).bearer_auth(self.credential.expose_secret()).json(&json!({"reservation_id":reservation,"ticket":ticket,"sealed":sealed})).send()=>r?};
        check_status(
            response.status(),
            "pool exchange rejected; status required before retry",
        )?;
        let nonce = response
            .headers()
            .get("axiom-pool-response-nonce")
            .and_then(|v| v.to_str().ok())
            .context("missing encrypted response")?;
        let mut verifier = FrameVerifier::new(
            grant.binding.clone(),
            &binding_bytes,
            &wire_hash,
            &axiom_pool_protocol::hex_bytes(nonce, 32)?,
        )?;
        let mut reader = context.response(nonce)?;
        let mut stream = response.bytes_stream();
        let mut text = String::new();
        let mut reasoning = String::new();
        let mut image = None;
        let mut image_dimensions = None;
        loop {
            if worker.expires_at() <= now() + 30 {
                let challenge = axiom_pool_protocol::random_challenge();
                let value = self
                    .metadata(
                        "attestation",
                        json!({"reservation_id":reservation,"challenge":challenge}),
                        &cancel,
                    )
                    .await?;
                let evidence: WorkerEvidence = strict::decode(&serde_json::to_vec(&value)?)?;
                worker = self
                    .verifier
                    .verify(
                        &self.http,
                        evidence,
                        &challenge,
                        &grant.binding.worker,
                        self.minimum_sequence.load(Ordering::Acquire),
                        &cancel,
                    )
                    .await?;
                self.minimum_sequence
                    .fetch_max(worker.policy_sequence(), Ordering::AcqRel);
            }
            let next = tokio::select! {_=cancel.cancelled()=>{let _=self.metadata("control",json!({"reservation_id":reservation,"action":"cancel"}),&CancellationToken::new()).await;anyhow::bail!("cancelled")},v=stream.next()=>v};
            let Some(bytes) = next else { break };
            let bytes = bytes?;
            ensure!(
                worker.expires_at() > now(),
                "worker proof expired during stream"
            );
            for fragment in bytes.chunks(axiom_pool_protocol::MAX_FRAME) {
                for opened in reader.push(fragment)? {
                    let frame = verifier.push(&opened)?;
                    match frame.kind.as_str() {
                        "text_delta" => {
                            text.push_str(frame.data["text"].as_str().context("delta")?)
                        }
                        "reasoning_delta" => {
                            reasoning.push_str(frame.data["text"].as_str().context("delta")?)
                        }
                        "image_manifest" => {
                            image = Some(Vec::new());
                            image_dimensions = Some((
                                frame.data["width"].as_u64().context("width")?,
                                frame.data["height"].as_u64().context("height")?,
                            ));
                        }
                        "image_chunk" => {
                            image
                                .as_mut()
                                .context("image missing")?
                                .extend(STANDARD.decode(
                                    frame.data["bytes_base64"].as_str().context("image bytes")?,
                                )?)
                        }
                        _ => {}
                    }
                    if frame.kind != "terminal" {
                        tokio::select! {_=cancel.cancelled()=>anyhow::bail!("cancelled"),result=provisional(frame)=>result?};
                    }
                }
            }
        }
        ensure!(
            worker.expires_at() > now(),
            "worker proof expired before response EOF"
        );
        reader.finish()?;
        let terminal = verifier.finish()?;
        let receipt: axiom_pool_protocol::types::Receipt = axiom_pool_protocol::verify(
            &terminal.receipt,
            &ed25519_dalek::VerifyingKey::from_bytes(
                &axiom_pool_protocol::hex_bytes(&grant.binding.worker.identity_key, 32)?
                    .try_into()
                    .map_err(|_| anyhow::anyhow!("key length"))?,
            )?,
            axiom_pool_protocol::RECEIPT_DOMAIN,
        )?;
        ensure!(
            receipt.price_version == grant.price_version
                && receipt.charge_microusd == grant.charge(&receipt.usage)?,
            "receipt cost mismatch"
        );
        if let Some(image) = &image {
            let (w, h) = image_dimensions.context("image dimensions")?;
            ensure!(
                image.len() >= 24
                    && image[..8] == *b"\x89PNG\r\n\x1a\n"
                    && u32::from_be_bytes(image[16..20].try_into()?) as u64 == w
                    && u32::from_be_bytes(image[20..24].try_into()?) as u64 == h,
                "image dimensions mismatch"
            );
        }
        // The independently verified worker signature is sufficient for the
        // controller to reconcile accounting if its callback was interrupted.
        // This submits public receipt metadata, never a prompt or response.
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            self.metadata(
                "receipts",
                serde_json::to_value(&terminal.receipt)?,
                &cancel,
            ),
        )
        .await;
        Ok(VerifiedOutput {
            tool_calls: Vec::new(),
            direct_timings: None,
            text,
            reasoning,
            image,
            terminal,
            billing: VerifiedBilling {
                price_version: receipt.price_version,
                charge_microusd: receipt.charge_microusd,
                maximum_charge_microusd: grant.max_charge_microusd,
                pricing: grant.pricing,
                settled: false,
            },
        })
    }
}
impl PreparedRequest {
    pub fn worker(&self) -> &attestation::VerifiedWorker {
        &self.worker
    }
}
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |v| v.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trial_capacity_retains_only_the_typed_retry_timestamp() {
        let error = unavailable_error(&json!({"detail": {"code": "trial_capacity",
            "message": "private upstream diagnostic", "retry_at_unix_seconds": 1791849600}}));
        assert_eq!(
            error.downcast_ref::<TrialCapacity>().unwrap().retry_at,
            Some(1_791_849_600)
        );
        assert!(!error.to_string().contains("private"));
        assert!(!unavailable_error(&json!({"detail":{"code":"other"}})).is::<TrialCapacity>());
        let invalid = unavailable_error(
            &json!({"detail":{"code":"trial_capacity","retry_at_unix_seconds":-1}}),
        );
        assert_eq!(
            invalid.downcast_ref::<TrialCapacity>().unwrap().retry_at,
            None
        );
    }

    #[test]
    fn only_payment_required_is_a_typed_balance_error() {
        let error =
            check_status(reqwest::StatusCode::PAYMENT_REQUIRED, "metadata rejected").unwrap_err();
        assert!(error.is::<InsufficientCredit>());
        assert!(error.to_string().contains("Top up your Axiom balance"));
        for status in [
            reqwest::StatusCode::CONFLICT,
            reqwest::StatusCode::BAD_GATEWAY,
        ] {
            let error = check_status(status, "metadata rejected").unwrap_err();
            assert!(!error.is::<InsufficientCredit>());
            assert!(!error.to_string().contains("Top up"));
        }
    }

    #[test]
    fn local_ca_cannot_authorize_remote_origins_or_empty_certificate_fallback() {
        let issuer = ed25519_dalek::SigningKey::from_bytes(&[7; 32]);
        let issuer_key = hex::encode(issuer.verifying_key().as_bytes());
        let verifier: attestation::VerifierConfig = serde_json::from_value(json!({
            "binary": "/not-installed", "binary_sha256": "11".repeat(32),
            "publisher_key": "22".repeat(32), "minimum_policy_sequence": 1
        }))
        .unwrap();
        for origin in [
            "https://api.example",
            "https://localhost.attacker.example",
            "http://127.0.0.1:9443",
            "https://127.0.0.1:9443",
        ] {
            assert!(
                PoolClient::new_with_local_relay_ca(
                    origin.into(),
                    &issuer_key,
                    SecretString::new("test-only".into()),
                    verifier.clone(),
                    b""
                )
                .is_err()
            );
        }
    }
}
