//! Public synthetic vectors; request-secret export exists only in test builds.
use axiom_pool_protocol::{
    PROTOCOL, RECEIPT_DOMAIN,
    crypto::Identity,
    frames::{Terminal, Transcript, prefix_hash, text_delta},
    sign,
    test_support::Fixture,
    types::{Receipt, Usage},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use sha2::Digest;
fn main() -> anyhow::Result<()> {
    let f = Fixture::new();
    let grant = f.grant("fixture-account-A", 1_800_000_000);
    let (request, context, binding) = f.request(&grant, "synthetic-fixture-request", 1_800_000_000);
    let (secret, enc) = context.fixture_response_token();
    let identity = Identity::from_bytes(&[10; 32])?;
    let (_, mut writer) = identity.open(&request)?;
    let nonce = hex::encode(writer.nonce);
    let digest = request.digest()?;
    let mut transcript = Transcript::new(&binding, &digest);
    let mut prefix = prefix_hash(&digest, &writer.nonce)?;
    let mut frames = Vec::new();
    for (kind, data) in [
        ("accepted", serde_json::json!({"provisional":true})),
        ("text_delta", text_delta("synthetic-fixture-answer")),
    ] {
        let raw = transcript.frame(kind, data)?;
        let wire = writer.frame(&raw)?;
        prefix.update(&wire);
        frames.push(wire);
    }
    let usage = Usage {
        input_tokens: 2,
        output_tokens: 3,
        ..Usage::default()
    };
    let receipt = Receipt {
        protocol: PROTOCOL.into(),
        binding_sha256: transcript.binding_hash.clone(),
        request_wire_sha256: digest.clone(),
        response_prefix_sha256: hex::encode(prefix.finalize()),
        binding: grant.binding.clone(),
        status: "succeeded".into(),
        usage: usage.clone(),
        price_version: grant.price_version.clone(),
        charge_microusd: 0,
    };
    let terminal = Terminal {
        success: true,
        transcript_sha256: transcript.digest(),
        frame_count: transcript.count(),
        finish_reason: "stop".into(),
        usage,
        receipt: sign(&receipt, &f.worker_key, RECEIPT_DOMAIN)?,
    };
    frames.push(writer.frame(&transcript.frame("terminal", serde_json::to_value(terminal)?)?)?);
    let value = serde_json::json!({"description":"PUBLIC TEST FIXTURE. No hardware attestation. Secret material is synthetic and MUST NOT be used outside tests.","binding":binding,"request":request,"request_digest":digest,"worker_public_key":hex::encode(f.worker_key.verifying_key().as_bytes()),"response_nonce":nonce,"exported_secret":hex::encode(secret),"request_enc":hex::encode(enc),"response_frames":frames.into_iter().map(|b|STANDARD.encode(b)).collect::<Vec<_>>()});
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}
