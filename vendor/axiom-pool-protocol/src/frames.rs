use crate::{
    PROTOCOL, RECEIPT_DOMAIN, SignedDocument,
    crypto::OpenedFrame,
    sha256, strict,
    types::{Binding, Receipt, Usage},
};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Frame {
    pub protocol: String,
    pub binding_sha256: String,
    pub request_wire_sha256: String,
    pub sequence: u64,
    pub kind: String,
    pub data: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Terminal {
    pub success: bool,
    pub transcript_sha256: String,
    pub frame_count: u64,
    pub finish_reason: String,
    pub usage: Usage,
    pub receipt: SignedDocument,
}
pub fn prefix_hash(request_digest: &str, nonce: &[u8]) -> Result<Sha256> {
    let mut hash = Sha256::new();
    hash.update(b"axiom-pool-response-prefix-v1\0");
    hash.update(crate::hex_bytes(request_digest, 32)?);
    hash.update(nonce);
    Ok(hash)
}
pub struct Transcript {
    pub binding_hash: String,
    pub request_hash: String,
    sequence: u64,
    hash: Sha256,
    bytes: usize,
    terminal: bool,
}
impl Transcript {
    pub fn new(binding: &str, request_hash: &str) -> Self {
        Self {
            binding_hash: sha256(binding),
            request_hash: request_hash.into(),
            sequence: 0,
            hash: Sha256::new(),
            bytes: 0,
            terminal: false,
        }
    }
    pub fn count(&self) -> u64 {
        self.sequence
    }
    pub fn digest(&self) -> String {
        hex::encode(self.hash.clone().finalize())
    }
    pub fn frame(&mut self, kind: &str, data: Value) -> Result<Vec<u8>> {
        ensure!(!self.terminal && self.sequence < 65536, "stream ended");
        let frame = Frame {
            protocol: PROTOCOL.into(),
            binding_sha256: self.binding_hash.clone(),
            request_wire_sha256: self.request_hash.clone(),
            sequence: self.sequence,
            kind: kind.into(),
            data,
        };
        let mut raw = serde_json::to_vec(&frame)?;
        raw.push(b'\n');
        ensure!(
            raw.len() <= crate::MAX_FRAME && self.bytes + raw.len() <= crate::MAX_RESPONSE,
            "stream limit"
        );
        if kind == "terminal" {
            self.terminal = true;
        } else {
            self.hash.update(&raw);
            self.sequence += 1;
        }
        self.bytes += raw.len();
        Ok(raw)
    }
}

/// Own one of these per originating request; never select it from a frame ID.
pub struct FrameVerifier {
    binding: Binding,
    transcript: Transcript,
    prefix: Sha256,
    key: ed25519_dalek::VerifyingKey,
    terminal: Option<Terminal>,
    max_bytes: usize,
    image: Option<ImageState>,
    failed: bool,
}
struct ImageState {
    id: String,
    bytes: usize,
    chunks: u64,
    expected_bytes: usize,
    expected_chunks: u64,
    digest: String,
    hash: Sha256,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageManifest {
    pub artifact_id: String,
    pub media_type: String,
    pub bytes: usize,
    pub width: u32,
    pub height: u32,
    pub chunks: u64,
    pub sha256: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageChunk {
    pub artifact_id: String,
    pub index: u64,
    pub bytes_base64: String,
}
impl FrameVerifier {
    pub fn new(
        binding: Binding,
        exact_binding: &str,
        request_hash: &str,
        nonce: &[u8],
    ) -> Result<Self> {
        let decoded: Binding = strict::decode(exact_binding.as_bytes())?;
        ensure!(
            decoded == binding && nonce.len() == 32,
            "wrong originating binding"
        );
        let key = ed25519_dalek::VerifyingKey::from_bytes(
            &crate::hex_bytes(&binding.worker.identity_key, 32)?
                .try_into()
                .map_err(|_| anyhow::anyhow!("key length"))?,
        )?;
        Ok(Self {
            max_bytes: binding.limits.output_bytes as usize,
            binding,
            transcript: Transcript::new(exact_binding, request_hash),
            prefix: prefix_hash(request_hash, nonce)?,
            key,
            terminal: None,
            image: None,
            failed: false,
        })
    }
    pub fn push(&mut self, opened: &OpenedFrame) -> Result<Frame> {
        ensure!(!self.failed, "frame verifier failed");
        let result = self.read(opened);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn read(&mut self, opened: &OpenedFrame) -> Result<Frame> {
        ensure!(
            self.terminal.is_none()
                && self.transcript.sequence < 65536
                && opened.plaintext.len() <= crate::MAX_FRAME
                && opened.plaintext.last() == Some(&b'\n'),
            "invalid or trailing frame"
        );
        let f: Frame = strict::decode(&opened.plaintext)?;
        ensure!(
            f.protocol == PROTOCOL
                && f.binding_sha256 == self.transcript.binding_hash
                && f.request_wire_sha256 == self.transcript.request_hash
                && f.sequence == self.transcript.sequence,
            "response binding/order mismatch"
        );
        self.transcript.bytes += opened.plaintext.len();
        ensure!(
            self.transcript.bytes <= self.max_bytes,
            "response budget exceeded"
        );
        if f.kind == "terminal" {
            let terminal: Terminal = strict::decode(&serde_json::to_vec(&f.data)?)?;
            ensure!(
                terminal.success
                    && terminal.frame_count == self.transcript.count()
                    && terminal.transcript_sha256 == self.transcript.digest()
                    && ["stop", "length", "tool_calls"].contains(&terminal.finish_reason.as_str()),
                "invalid completion"
            );
            let receipt: Receipt = crate::verify(&terminal.receipt, &self.key, RECEIPT_DOMAIN)?;
            ensure!(
                receipt.protocol == PROTOCOL
                    && receipt.binding == self.binding
                    && receipt.binding_sha256 == self.transcript.binding_hash
                    && receipt.request_wire_sha256 == self.transcript.request_hash
                    && receipt.response_prefix_sha256
                        == hex::encode(self.prefix.clone().finalize())
                    && receipt.status == "succeeded"
                    && receipt.usage == terminal.usage,
                "receipt binding mismatch"
            );
            ensure!(
                terminal
                    .usage
                    .input_tokens
                    .checked_add(terminal.usage.output_tokens)
                    .is_some_and(|n| n <= self.binding.limits.context_tokens)
                    && terminal.usage.output_tokens <= self.binding.limits.output_tokens,
                "usage exceeds budget"
            );
            ensure!(
                terminal.usage.image_pixels <= self.binding.limits.image_pixels
                    && terminal.usage.image_steps <= self.binding.limits.image_steps,
                "image usage exceeds budget"
            );
            if let Some(image) = &self.image {
                ensure!(
                    image.bytes == image.expected_bytes
                        && image.chunks == image.expected_chunks
                        && hex::encode(image.hash.clone().finalize()) == image.digest,
                    "incomplete image"
                );
            }
            ensure!(
                (self.binding.operation != crate::types::Operation::Image) || self.image.is_some(),
                "missing image"
            );
            self.terminal = Some(terminal);
        } else {
            self.validate_kind(&f)?;
            self.transcript.hash.update(&opened.plaintext);
            self.transcript.sequence += 1;
            self.prefix.update(&opened.wire);
        }
        Ok(f)
    }
    fn validate_kind(&mut self, f: &Frame) -> Result<()> {
        match f.kind.as_str() {
            "accepted" => ensure!(f.sequence == 0, "late acceptance"),
            "heartbeat" => ensure!(f.data == json!({"provisional":true}), "invalid heartbeat"),
            "text_delta" | "reasoning_delta" => {
                ensure!(
                    self.binding.operation == crate::types::Operation::Chat
                        && f.data.get("text").and_then(Value::as_str).is_some()
                        && f.data.get("provisional") == Some(&Value::Bool(true)),
                    "invalid text delta"
                );
            }
            // Tool calls need a complete validated tool contract; v1 does not
            // advertise tool support until that parser is implemented.
            "usage" => {
                let _: Usage = strict::decode(&serde_json::to_vec(&f.data)?)?;
            }
            "image_manifest" => {
                let m: ImageManifest = strict::decode(&serde_json::to_vec(&f.data)?)?;
                crate::hex_bytes(&m.artifact_id, 16)?;
                crate::hex_bytes(&m.sha256, 32)?;
                ensure!(
                    self.binding.operation == crate::types::Operation::Image
                        && self.image.is_none()
                        && m.media_type == "image/png"
                        && m.width > 0
                        && m.height > 0
                        && u64::from(m.width) * u64::from(m.height)
                            <= self.binding.limits.image_pixels
                        && m.bytes > 0
                        && m.bytes <= 64 * 1024 * 1024
                        && m.chunks > 0
                        && m.chunks <= 1024,
                    "invalid image manifest"
                );
                self.image = Some(ImageState {
                    id: m.artifact_id,
                    bytes: 0,
                    chunks: 0,
                    expected_bytes: m.bytes,
                    expected_chunks: m.chunks,
                    digest: m.sha256,
                    hash: Sha256::new(),
                });
            }
            "image_chunk" => {
                use base64::{Engine, engine::general_purpose::STANDARD};
                let c: ImageChunk = strict::decode(&serde_json::to_vec(&f.data)?)?;
                ensure!(c.bytes_base64.len() <= 256 * 1024, "large image chunk");
                let raw = STANDARD.decode(&c.bytes_base64)?;
                let image = self
                    .image
                    .as_mut()
                    .ok_or_else(|| anyhow::anyhow!("missing image manifest"))?;
                ensure!(
                    c.artifact_id == image.id
                        && c.index == image.chunks
                        && !raw.is_empty()
                        && image.bytes + raw.len() <= image.expected_bytes
                        && image.chunks < image.expected_chunks,
                    "invalid image order/length"
                );
                image.hash.update(&raw);
                image.bytes += raw.len();
                image.chunks += 1;
            }
            _ => anyhow::bail!("unsupported frame or authenticated failure"),
        }
        Ok(())
    }
    pub fn finish(self) -> Result<Terminal> {
        ensure!(!self.failed, "frame verifier failed");
        self.terminal
            .ok_or_else(|| anyhow::anyhow!("missing authenticated completion"))
    }
}

pub fn text_delta(text: &str) -> Value {
    json!({"text":text,"provisional":true})
}
