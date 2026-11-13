//! Deterministic identities for adversarial tests; absent from default builds.
use crate::{
    AUDIENCE, GRANT_DOMAIN, PROTOCOL,
    authorization::{self, ClientKey},
    crypto::{Identity, RequestContext, SealedRequest},
    sign,
    types::*,
};
pub struct Fixture {
    pub issuer: ed25519_dalek::SigningKey,
    pub worker_key: ed25519_dalek::SigningKey,
    pub client: p256::ecdsa::SigningKey,
    pub worker: WorkerIdentity,
}
impl Fixture {
    pub fn new() -> Self {
        let issuer = ed25519_dalek::SigningKey::from_bytes(&[7; 32]);
        let worker_key = ed25519_dalek::SigningKey::from_bytes(&[8; 32]);
        let client = p256::ecdsa::SigningKey::from_bytes((&[9; 32]).into()).unwrap();
        let crypto = Identity::from_bytes(&[10; 32]).unwrap();
        let worker = WorkerIdentity {
            worker_id: "11".repeat(16),
            boot_id: "12".repeat(32),
            encryption_key: crypto.public_key,
            identity_key: hex::encode(worker_key.verifying_key().as_bytes()),
            model_manifest: "13".repeat(32),
            logical_origin: "https://worker.example".into(),
            evidence_profile: "test-fixture-only".into(),
            deployment_digest: "14".repeat(32),
        };
        Self {
            issuer,
            worker_key,
            client,
            worker,
        }
    }
    pub fn grant(&self, account: &str, now: u64) -> Grant {
        Grant {
            protocol: PROTOCOL.into(),
            issuer: "https://issuer.example".into(),
            audience: AUDIENCE.into(),
            issued_at: now,
            expires_at: now + 120,
            binding: Binding {
                account: account.into(),
                account_session: "session".into(),
                client_jkt: ClientKey::from_signing(&self.client).thumbprint().unwrap(),
                request_id: crate::random_id(),
                attempt_id: crate::random_id(),
                reservation_id: crate::random_id(),
                fencing: 1,
                worker: self.worker.clone(),
                grant_jti: crate::random_id(),
                operation: Operation::Chat,
                limits: Limits {
                    context_tokens: 1024,
                    output_tokens: 128,
                    input_bytes: 3 * 1024 * 1024,
                    output_bytes: 1024 * 1024,
                    image_pixels: 1024 * 1024,
                    image_steps: 8,
                    deadline: now + 120,
                },
            },
            price_version: "test-free-v1".into(),
            max_charge_microusd: 0,
        }
    }
    pub fn request(
        &self,
        grant: &Grant,
        text: &str,
        now: u64,
    ) -> (SealedRequest, RequestContext, String) {
        let binding = serde_json::to_string(&grant.binding).unwrap();
        let payload = serde_json::to_string(&Payload {
            binding: binding.clone(),
            input: Input::Chat {
                messages: vec![Message {
                    role: "user".into(),
                    content: text.into(),
                }],
                max_tokens: 128,
                reasoning_effort: "medium".into(),
                tools: vec![],
            },
        })
        .unwrap();
        let token =
            serde_json::to_string(&sign(grant, &self.issuer, GRANT_DOMAIN).unwrap()).unwrap();
        let proof = authorization::proof(
            &self.client,
            &token,
            &format!("{}/v1/exchange", self.worker.logical_origin),
            &self.worker.encryption_key,
            &payload,
            now,
        )
        .unwrap();
        let envelope = Envelope {
            protocol: PROTOCOL.into(),
            grant: token,
            proof,
            payload,
        };
        let (request, context) = RequestContext::seal(
            &self.worker.encryption_key,
            &serde_json::to_vec(&envelope).unwrap(),
        )
        .unwrap();
        (request, context, binding)
    }
}
impl Default for Fixture {
    fn default() -> Self {
        Self::new()
    }
}
