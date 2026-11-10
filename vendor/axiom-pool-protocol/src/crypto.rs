use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, KeyInit},
};
use anyhow::{Context, Result, ensure};
use hpke::kem::Kem as _;
use hpke::{Deserializable, Serializable, aead::AesGcm256, kdf::HkdfSha256, kem::X25519HkdfSha256};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;
type Kem = X25519HkdfSha256;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SealedRequest {
    pub encapsulated_key: String,
    pub body: String,
}
impl SealedRequest {
    pub fn bytes(&self) -> Result<(Vec<u8>, Vec<u8>)> {
        use base64::{Engine, engine::general_purpose::STANDARD};
        ensure!(
            self.body.len() <= (crate::MAX_REQUEST + 32).div_ceil(3) * 4,
            "request exceeds limit"
        );
        let enc = crate::hex_bytes(&self.encapsulated_key, 32)?;
        let body = STANDARD.decode(&self.body)?;
        ensure!(STANDARD.encode(&body) == self.body, "noncanonical base64");
        Ok((enc, body))
    }
    pub fn digest(&self) -> Result<String> {
        let (enc, body) = self.bytes()?;
        let mut hash = Sha256::new();
        hash.update(b"axiom-pool-wire-v1\0");
        hash.update((enc.len() as u32).to_be_bytes());
        hash.update(enc);
        hash.update((body.len() as u64).to_be_bytes());
        hash.update(body);
        Ok(hex::encode(hash.finalize()))
    }
}

pub struct Identity {
    private: <Kem as hpke::kem::Kem>::PrivateKey,
    pub public_key: String,
}
impl Identity {
    pub fn from_pkcs8_pem(pem: &str) -> Result<Self> {
        ensure!(pem.len() <= 4096, "private key limit");
        let (label, doc) = pkcs8::SecretDocument::from_pem(pem)?;
        let info = pkcs8::PrivateKeyInfo::try_from(doc.as_bytes())?;
        ensure!(
            label == "PRIVATE KEY"
                && info.algorithm.oid == pkcs8::ObjectIdentifier::new_unwrap("1.3.101.110")
                && info.algorithm.parameters.is_none()
                && info.private_key.len() == 34
                && info.private_key[..2] == [4, 32],
            "invalid endorsed X25519 key"
        );
        Self::from_bytes(&info.private_key[2..])
    }
    pub fn from_bytes(raw: &[u8]) -> Result<Self> {
        let private = <Kem as hpke::kem::Kem>::PrivateKey::from_bytes(raw)
            .map_err(|_| anyhow::anyhow!("invalid encryption key"))?;
        let public_key = hex::encode(Kem::sk_to_pk(&private).to_bytes());
        Ok(Self {
            private,
            public_key,
        })
    }
    pub fn open(&self, request: &SealedRequest) -> Result<(Zeroizing<Vec<u8>>, ResponseWriter)> {
        let (enc, body) = request.bytes()?;
        ensure!(
            body.len() >= 20 && body.len() <= crate::MAX_REQUEST + 20,
            "invalid request length"
        );
        let len = u32::from_be_bytes(body[..4].try_into()?) as usize;
        ensure!(
            len == body.len() - 4,
            "incomplete or multiple request frames"
        );
        let encapsulated = <Kem as hpke::kem::Kem>::EncappedKey::from_bytes(&enc)
            .map_err(|_| anyhow::anyhow!("invalid encapsulated key"))?;
        let mut receiver = hpke::setup_receiver::<AesGcm256, HkdfSha256, Kem>(
            &hpke::OpModeR::Base,
            &self.private,
            &encapsulated,
            ehbp::HPKE_REQUEST_INFO,
        )
        .map_err(|_| anyhow::anyhow!("HPKE setup failed"))?;
        let plaintext = Zeroizing::new(
            receiver
                .open(&body[4..], &[])
                .map_err(|_| anyhow::anyhow!("request authentication failed"))?,
        );
        let mut secret = Zeroizing::new([0u8; 32]);
        receiver
            .export(ehbp::EXPORT_LABEL, secret.as_mut())
            .map_err(|_| anyhow::anyhow!("export failed"))?;
        let nonce = rand::random::<[u8; 32]>();
        Ok((
            plaintext,
            ResponseWriter {
                material: ehbp::derive_response_keys(secret.as_ref(), &enc, &nonce)?,
                nonce,
                sequence: 0,
                bytes: 0,
            },
        ))
    }
}

pub struct RequestContext {
    secret: Zeroizing<[u8; 32]>,
    enc: Vec<u8>,
}
impl RequestContext {
    #[cfg(feature = "test-fixture")]
    pub fn fixture_response_token(&self) -> (Vec<u8>, Vec<u8>) {
        (self.secret.to_vec(), self.enc.clone())
    }
    pub fn seal(recipient: &str, plaintext: &[u8]) -> Result<(SealedRequest, Self)> {
        use base64::{Engine, engine::general_purpose::STANDARD};
        ensure!(
            plaintext.len() <= crate::MAX_REQUEST,
            "request exceeds limit"
        );
        let recipient =
            <Kem as hpke::kem::Kem>::PublicKey::from_bytes(&crate::hex_bytes(recipient, 32)?)
                .map_err(|_| anyhow::anyhow!("invalid recipient"))?;
        let (enc, mut sender) = hpke::setup_sender::<AesGcm256, HkdfSha256, Kem, _>(
            &hpke::OpModeS::Base,
            &recipient,
            ehbp::HPKE_REQUEST_INFO,
            &mut rand::rng(),
        )
        .map_err(|_| anyhow::anyhow!("HPKE setup failed"))?;
        let ciphertext = sender
            .seal(plaintext, &[])
            .map_err(|_| anyhow::anyhow!("request encryption failed"))?;
        let mut secret = Zeroizing::new([0u8; 32]);
        sender
            .export(ehbp::EXPORT_LABEL, secret.as_mut())
            .map_err(|_| anyhow::anyhow!("export failed"))?;
        let enc = enc.to_bytes().to_vec();
        let wire = [
            (ciphertext.len() as u32).to_be_bytes().as_slice(),
            ciphertext.as_slice(),
        ]
        .concat();
        Ok((
            SealedRequest {
                encapsulated_key: hex::encode(&enc),
                body: STANDARD.encode(wire),
            },
            Self { secret, enc },
        ))
    }
    pub fn response(self, nonce: &str) -> Result<ResponseReader> {
        let material = ehbp::derive_response_keys(
            self.secret.as_ref(),
            &self.enc,
            &crate::hex_bytes(nonce, 32)?,
        )?;
        Ok(ResponseReader {
            material,
            sequence: 0,
            pending: Vec::new(),
            bytes: 0,
            failed: false,
        })
    }
}
pub struct ResponseWriter {
    material: ehbp::ResponseKeyMaterial,
    pub nonce: [u8; 32],
    sequence: u64,
    bytes: usize,
}
impl ResponseWriter {
    pub fn frame(&mut self, plaintext: &[u8]) -> Result<Vec<u8>> {
        ensure!(
            plaintext.len() <= crate::MAX_FRAME
                && self.bytes + plaintext.len() <= crate::MAX_RESPONSE,
            "response limit"
        );
        let nonce = ehbp::compute_nonce(&self.material.nonce_base, self.sequence);
        let cipher = Aes256Gcm::new_from_slice(&self.material.key)?;
        let body = cipher
            .encrypt(Nonce::from_slice(&nonce), plaintext)
            .map_err(|_| anyhow::anyhow!("response encryption failed"))?;
        self.sequence = self.sequence.checked_add(1).context("sequence exhausted")?;
        self.bytes += plaintext.len();
        Ok([
            (body.len() as u32).to_be_bytes().as_slice(),
            body.as_slice(),
        ]
        .concat())
    }
}
pub struct ResponseReader {
    material: ehbp::ResponseKeyMaterial,
    sequence: u64,
    pending: Vec<u8>,
    bytes: usize,
    failed: bool,
}
pub struct OpenedFrame {
    pub plaintext: Zeroizing<Vec<u8>>,
    pub wire: Vec<u8>,
}
impl ResponseReader {
    pub fn push(&mut self, input: &[u8]) -> Result<Vec<OpenedFrame>> {
        ensure!(!self.failed, "reader failed");
        let result = self.read(input);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn read(&mut self, input: &[u8]) -> Result<Vec<OpenedFrame>> {
        self.bytes = self
            .bytes
            .checked_add(input.len())
            .context("response overflow")?;
        ensure!(
            self.bytes <= crate::MAX_RESPONSE + crate::MAX_RESPONSE / 16,
            "response exceeds limit"
        );
        // Network chunks are bounded by callers; consume progressively to avoid
        // growing the pending frame from an attacker-controlled length prefix.
        ensure!(
            input.len() <= 2 * crate::MAX_FRAME,
            "network chunk exceeds limit"
        );
        self.pending.extend_from_slice(input);
        let mut out = Vec::new();
        let mut offset = 0;
        while self.pending.len() - offset >= 4 {
            let length = u32::from_be_bytes(self.pending[offset..offset + 4].try_into()?) as usize;
            ensure!(
                (16..=crate::MAX_FRAME + 16).contains(&length),
                "invalid encrypted frame size"
            );
            if self.pending.len() - offset < length + 4 {
                break;
            }
            let nonce = ehbp::compute_nonce(&self.material.nonce_base, self.sequence);
            let cipher = Aes256Gcm::new_from_slice(&self.material.key)?;
            let bytes = cipher
                .decrypt(
                    Nonce::from_slice(&nonce),
                    &self.pending[offset + 4..offset + 4 + length],
                )
                .map_err(|_| anyhow::anyhow!("response authentication failed"))?;
            self.sequence = self.sequence.checked_add(1).context("sequence exhausted")?;
            out.push(OpenedFrame {
                plaintext: Zeroizing::new(bytes),
                wire: self.pending[offset..offset + length + 4].to_vec(),
            });
            offset += length + 4;
        }
        self.pending.drain(..offset);
        ensure!(
            self.pending.len() <= crate::MAX_FRAME + 20,
            "partial frame exceeds limit"
        );
        Ok(out)
    }
    pub fn finish(self) -> Result<()> {
        ensure!(
            !self.failed && self.pending.is_empty() && self.sequence > 0,
            "truncated encrypted response"
        );
        Ok(())
    }
}
