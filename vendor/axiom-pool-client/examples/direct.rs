//! Production direct client. No fixture keys or unchecked-recipient mode.
//! The first TLS exchange carries only a public attestation challenge. After
//! independent verification, all subsequent requests pin its CPU-bound cert.
use anyhow::{Context, Result, ensure};
use axiom_pool_client::attestation::{VerifierConfig, bounded_json};
use axiom_pool_protocol::{
    GRANT_DOMAIN, PROTOCOL,
    authorization::{self, ClientKey},
    crypto::RequestContext,
    frames::FrameVerifier,
    sign, strict,
    types::{
        Binding, Envelope, Grant, Input, Limits, Message, Operation, Payload, WorkerEvidence,
        WorkerIdentity,
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
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    origin: String,
    /// Optional SSH forwarding transport; only an exact HTTP loopback origin.
    transport_origin: Option<String>,
    /// Optional route for a stable logical DNS origin. TLS still validates
    /// that logical hostname against the independently CPU-bound certificate.
    connect_ip: Option<IpAddr>,
    model_manifest: String,
    issuer_key_file: PathBuf,
    verifier: VerifierConfig,
    operation: String,
    prompt: Option<String>,
    image_output: Option<PathBuf>,
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
        tokio::time::sleep(Duration::from_secs(10)).await;
    }
    unreachable!("bounded admission attempts return on the final response")
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("direct request failed: {error:#}; inference was not replayed");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let path = std::env::args()
        .nth(1)
        .context("direct client configuration path required")?;
    let raw = std::fs::read(path)?;
    ensure!(raw.len() <= 64 * 1024, "direct client config limit");
    let cfg: Config = strict::decode(&raw)?;
    axiom_pool_protocol::origin(&cfg.origin, false)?;
    axiom_pool_protocol::hex_bytes(&cfg.model_manifest, 32)?;
    let transport = cfg.transport_origin.as_deref().unwrap_or(&cfg.origin);
    if transport != cfg.origin {
        ensure!(
            cfg.connect_ip.is_none(),
            "forwarding and IP routing cannot be combined"
        );
        let url = reqwest::Url::parse(transport)?;
        ensure!(
            url.scheme() == "http"
                && matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))
                && url.origin().ascii_serialization() == transport,
            "forwarding transport must be exact HTTP loopback"
        );
    }
    ensure!(
        matches!(cfg.operation.as_str(), "attest" | "chat" | "image"),
        "unknown direct operation"
    );
    ensure!(
        cfg.operation != "image" || cfg.image_output.is_some(),
        "image output path required before dispatch"
    );
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
    let verified = cfg
        .verifier
        .verify(
            &client,
            evidence,
            &challenge,
            &expected,
            cfg.verifier.minimum_policy_sequence,
            &CancellationToken::new(),
        )
        .await?;
    if cfg.operation == "attest" {
        println!(
            "{}",
            json!({"hardware_verified":true,"workload_binding_verified":true,
            "evidence_profile":expected.evidence_profile,"worker":expected,"expires_at":verified.expires_at(),
            "policy_sequence":verified.policy_sequence()})
        );
        return Ok(());
    }
    use std::os::unix::fs::PermissionsExt;
    ensure!(
        cfg.issuer_key_file.metadata()?.permissions().mode() & 0o077 == 0,
        "private operator issuer key permissions required"
    );
    let pem = Zeroizing::new(std::fs::read_to_string(&cfg.issuer_key_file)?);
    let issuer = ed25519_dalek::SigningKey::from_pkcs8_pem(&pem)?;
    let lease = sign(
        &Lease {
            protocol: PROTOCOL,
            worker: &expected,
            expires_at: verified.expires_at().min(now() + 200),
            max_sequences: 4,
            token_budget: 65536,
        },
        &issuer,
        "axiom-pool-admission-v1",
    )?;
    admit(&client, transport, &lease).await?;
    let key = loop {
        if let Ok(k) = p256::ecdsa::SigningKey::from_bytes((&rand::random::<[u8; 32]>()).into()) {
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
        operation: if cfg.operation == "chat" {
            Operation::Chat
        } else {
            Operation::Image
        },
        limits: Limits {
            context_tokens: 8192,
            output_tokens: 768,
            input_bytes: 1024 * 1024,
            output_bytes: 32 * 1024 * 1024,
            image_pixels: 1024 * 1024,
            image_steps: 8,
            deadline: now() + 180,
        },
    };
    let input = if cfg.operation == "chat" {
        Input::Chat{messages:vec![Message{role:"user".into(),
            content:cfg.prompt.unwrap_or_else(||"Write a Python function that adds two integers, and one assert. Keep the answer short.".into())}],
            max_tokens:768,reasoning_effort:"medium".into(),tools:vec![]}
    } else {
        Input::Image {
            prompt: cfg
                .prompt
                .unwrap_or_else(|| "A blue square centered on a white background.".into()),
            width: 512,
            height: 512,
            steps: 8,
            seed: 42,
        }
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
        verified.expires_at() > now() + 180,
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
    while let Some(bytes) = stream.next().await {
        ensure!(verified.expires_at() > now(), "proof expired during stream");
        for fragment in bytes?.chunks(axiom_pool_protocol::MAX_FRAME) {
            for opened in reader.push(fragment)? {
                let frame = frames.push(&opened)?;
                match frame.kind.as_str() {
                    "text_delta" => {
                        text.push_str(frame.data["text"].as_str().context("text delta")?)
                    }
                    "reasoning_delta" => {
                        reasoning.push_str(frame.data["text"].as_str().context("reasoning delta")?)
                    }
                    "image_chunk" => image.extend(
                        STANDARD
                            .decode(frame.data["bytes_base64"].as_str().context("image bytes")?)?,
                    ),
                    _ => {}
                }
            }
        }
    }
    reader.finish()?;
    let terminal = frames.finish()?;
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
    if cfg.operation == "image" {
        ensure!(
            image.len() >= 24 && image[..8] == *b"\x89PNG\r\n\x1a\n",
            "PNG result missing"
        );
        std::fs::write(
            cfg.image_output.context("image output path required")?,
            &image,
        )?;
    } else {
        ensure!(!text.is_empty(), "text answer missing");
    }
    println!(
        "{}",
        json!({"hardware_verified":true,"workload_binding_verified":true,"authenticated_completion":true,
        "evidence_profile":expected.evidence_profile,"text":text,"reasoning":reasoning,"image_bytes":image.len(),
        "usage":terminal.usage,"receipt":terminal.receipt})
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

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
