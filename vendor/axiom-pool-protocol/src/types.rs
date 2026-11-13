use crate::{AUDIENCE, PROTOCOL, SignedDocument, hex_bytes, origin, sha256};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerIdentity {
    pub worker_id: String,
    pub boot_id: String,
    pub encryption_key: String,
    pub identity_key: String,
    pub model_manifest: String,
    pub logical_origin: String,
    pub evidence_profile: String,
    pub deployment_digest: String,
}
impl WorkerIdentity {
    pub fn validate(&self, fixture: bool) -> Result<()> {
        hex_bytes(&self.worker_id, 16)?;
        hex_bytes(&self.boot_id, 32)?;
        hex_bytes(&self.encryption_key, 32)?;
        hex_bytes(&self.identity_key, 32)?;
        hex_bytes(&self.model_manifest, 32)?;
        hex_bytes(&self.deployment_digest, 32)?;
        origin(&self.logical_origin, fixture)?;
        ensure!(
            self.evidence_profile == "tinfoil-gpu-v3"
                || (fixture && self.evidence_profile == "test-fixture-only"),
            "unsupported worker evidence profile"
        );
        Ok(())
    }
    pub fn encryption_digest(&self) -> Result<String> {
        Ok(sha256(hex_bytes(&self.encryption_key, 32)?))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Chat,
    Image,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    pub context_tokens: u64,
    pub output_tokens: u64,
    pub input_bytes: u64,
    pub output_bytes: u64,
    pub image_pixels: u64,
    pub image_steps: u64,
    pub deadline: u64,
}
impl Limits {
    pub fn validate(&self, now: u64) -> Result<()> {
        ensure!(
            (1..=131_072).contains(&self.context_tokens)
                && (1..=32_768).contains(&self.output_tokens)
                && self.output_tokens <= self.context_tokens,
            "invalid token budget"
        );
        ensure!(
            (1..=crate::MAX_REQUEST as u64).contains(&self.input_bytes)
                && (1..=crate::MAX_RESPONSE as u64).contains(&self.output_bytes),
            "invalid byte budget"
        );
        ensure!(
            self.image_pixels <= 1024 * 1024
                && self.image_steps <= 8
                && self.deadline > now
                && self.deadline <= now.saturating_add(600),
            "invalid execution budget"
        );
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub account: String,
    pub account_session: String,
    pub client_jkt: String,
    pub request_id: String,
    pub attempt_id: String,
    pub reservation_id: String,
    pub fencing: u64,
    pub worker: WorkerIdentity,
    pub grant_jti: String,
    pub operation: Operation,
    pub limits: Limits,
}
impl Binding {
    pub fn validate(&self, now: u64, fixture: bool) -> Result<()> {
        for id in [&self.account, &self.account_session, &self.client_jkt] {
            ensure!(!id.is_empty() && id.len() <= 128, "invalid principal");
        }
        for id in [
            &self.request_id,
            &self.attempt_id,
            &self.reservation_id,
            &self.grant_jti,
        ] {
            hex_bytes(id, 16)?;
        }
        ensure!(
            self.fencing > 0 && self.fencing <= 9_007_199_254_740_991,
            "invalid fence"
        );
        self.worker.validate(fixture)?;
        self.limits.validate(now)?;
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Grant {
    pub protocol: String,
    pub issuer: String,
    pub audience: String,
    pub issued_at: u64,
    pub expires_at: u64,
    pub binding: Binding,
    pub price_version: String,
    pub max_charge_microusd: u64,
}
impl Grant {
    pub fn validate(
        &self,
        issuer: &str,
        worker: &WorkerIdentity,
        now: u64,
        fixture: bool,
    ) -> Result<()> {
        ensure!(
            self.protocol == PROTOCOL && self.issuer == issuer && self.audience == AUDIENCE,
            "grant contract mismatch"
        );
        ensure!(
            self.issued_at <= now.saturating_add(30)
                && self.expires_at > now
                && self.expires_at > self.issued_at
                && self.expires_at - self.issued_at <= 120,
            "expired grant"
        );
        ensure!(
            &self.binding.worker == worker
                && !self.price_version.is_empty()
                && self.price_version.len() <= 64,
            "wrong grant recipient"
        );
        self.binding.validate(now, fixture)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    pub protocol: String,
    pub grant: String,
    pub proof: String,
    pub payload: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Payload {
    pub binding: String,
    pub input: Input,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum Input {
    Chat {
        messages: Vec<Message>,
        max_tokens: u64,
        reasoning_effort: String,
        tools: Vec<serde_json::Value>,
    },
    Image {
        prompt: String,
        width: u32,
        height: u32,
        steps: u32,
        seed: u32,
    },
}
impl Input {
    pub fn validate(&self, binding: &Binding) -> Result<()> {
        match self {
            Self::Chat {
                messages,
                max_tokens,
                reasoning_effort,
                tools,
            } => {
                ensure!(
                    binding.operation == Operation::Chat
                        && !messages.is_empty()
                        && messages.len() <= 4096
                        && *max_tokens > 0
                        && *max_tokens <= binding.limits.output_tokens,
                    "invalid chat"
                );
                ensure!(
                    ["low", "medium", "high"].contains(&reasoning_effort.as_str())
                        && tools.is_empty(),
                    "invalid reasoning/tools"
                );
                for m in messages {
                    ensure!(
                        ["system", "developer", "user", "assistant"].contains(&m.role.as_str())
                            && m.content.len() <= 3 * 1024 * 1024,
                        "invalid message"
                    );
                }
            }
            Self::Image {
                prompt,
                width,
                height,
                steps,
                ..
            } => {
                ensure!(
                    binding.operation == Operation::Image
                        && !prompt.is_empty()
                        && prompt.len() <= 64 * 1024
                        && *width >= 256
                        && *height >= 256
                        && width.is_multiple_of(64)
                        && height.is_multiple_of(64)
                        && *width <= 1024
                        && *height <= 1024
                        && u64::from(*width) * u64::from(*height) <= binding.limits.image_pixels
                        && *steps > 0
                        && u64::from(*steps) <= binding.limits.image_steps,
                    "invalid image budget"
                );
            }
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Message {
    pub role: String,
    pub content: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub image_pixels: u64,
    pub image_steps: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub protocol: String,
    pub binding_sha256: String,
    pub request_wire_sha256: String,
    pub response_prefix_sha256: String,
    pub binding: Binding,
    pub status: String,
    pub usage: Usage,
    pub price_version: String,
    pub charge_microusd: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ticket {
    pub protocol: String,
    pub binding: Binding,
    pub expires_at: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerEvidence {
    pub protocol: String,
    pub challenge: String,
    pub identity: SignedDocument,
    pub document: serde_json::Value,
    pub manifest: SignedDocument,
    pub policy: SignedDocument,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdentityStatement {
    pub worker: WorkerIdentity,
    pub challenge: String,
    pub issued_at: u64,
    pub expires_at: u64,
}
