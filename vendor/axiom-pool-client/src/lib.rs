//! Native pool client. Recipient keys are accepted only by the installed verifier.
#![forbid(unsafe_code)]
pub mod attestation;

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

pub struct PoolClient {
    http: reqwest::Client,
    origin: String,
    issuer_key: ed25519_dalek::VerifyingKey,
    credential: SecretString,
    key: p256::ecdsa::SigningKey,
    verifier: attestation::VerifierConfig,
    minimum_sequence: Arc<AtomicU64>,
    fixture: bool,
}
pub struct VerifiedOutput {
    pub text: String,
    pub reasoning: String,
    pub image: Option<Vec<u8>>,
    pub terminal: Terminal,
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
        Self::construct(origin, issuer_key, credential, verifier, false)
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
        Self::construct(origin, issuer_key, credential, verifier, true)
    }
    fn construct(
        origin: String,
        issuer_key: &str,
        credential: SecretString,
        verifier: attestation::VerifierConfig,
        fixture: bool,
    ) -> Result<Self> {
        axiom_pool_protocol::origin(&origin, fixture)?;
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
            http: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(std::time::Duration::from_secs(10))
                .timeout(std::time::Duration::from_secs(610))
                .build()?,
            origin,
            issuer_key: public,
            credential,
            key,
            minimum_sequence: Arc::new(AtomicU64::new(verifier.minimum_policy_sequence)),
            verifier,
            fixture,
        })
    }
    async fn metadata(
        &self,
        route: &str,
        body: Value,
        cancel: &CancellationToken,
    ) -> Result<Value> {
        let response = tokio::select! {_ = cancel.cancelled()=>anyhow::bail!("cancelled"),response=self.http.post(format!("{}/api/v1/pool/{route}",self.origin)).bearer_auth(self.credential.expose_secret()).json(&body).send()=>response?};
        ensure!(
            response.status().is_success(),
            "pool metadata request rejected"
        );
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
        // The preview is free. Never silently accept a changed price from a
        // mutable catalog; a paid client needs an explicit accepted cost cap.
        ensure!(
            grant.max_charge_microusd == 0 && grant.price_version == "pool-free-preview-v1",
            "unaccepted price"
        );
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
        self.execute_sealed(
            prepared.grant,
            prepared.grant_document,
            prepared.binding_bytes,
            payload,
            prepared.ticket,
            prepared.reservation,
            prepared.worker,
            &mut provisional,
            cancel,
        )
        .await
    }
    async fn execute_sealed<F, Fut>(
        &self,
        grant: Grant,
        doc: SignedDocument,
        binding_bytes: String,
        payload: Zeroizing<String>,
        ticket: SignedDocument,
        reservation: String,
        mut worker: attestation::VerifiedWorker,
        provisional: &mut F,
        cancel: CancellationToken,
    ) -> Result<VerifiedOutput>
    where
        F: FnMut(Frame) -> Fut,
        Fut: std::future::Future<Output = Result<()>>,
    {
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
        ensure!(
            response.status().is_success(),
            "pool exchange rejected; status required before retry"
        );
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
                && receipt.charge_microusd <= grant.max_charge_microusd,
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
        Ok(VerifiedOutput {
            text,
            reasoning,
            image,
            terminal,
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
