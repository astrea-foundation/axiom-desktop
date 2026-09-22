use chacha20poly1305::{
    KeyInit, XChaCha20Poly1305, XNonce,
    aead::{Aead, Payload},
};
use curve25519_dalek::edwards::CompressedEdwardsY;
use ed25519_dalek::SigningKey;
use hkdf::Hkdf;
use rand_core::{OsRng, RngCore};
use sha2::{Digest, Sha256, Sha512};
use x25519_dalek::{PublicKey as X25519PublicKey, StaticSecret};
use zeroize::Zeroizing;

use crate::{Result, SecureClientError};

const HKDF_INFO: &[u8] = b"ed25519_encryption";
const EPHEMERAL_PUBLIC_BYTES: usize = 32;
const NONCE_BYTES: usize = 24;
const TAG_BYTES: usize = 16;
const WIRE_PREFIX_BYTES: usize = EPHEMERAL_PUBLIC_BYTES + NONCE_BYTES;
const MINIMUM_WIRE_BYTES: usize = WIRE_PREFIX_BYTES + TAG_BYTES;
const MAXIMUM_WIRE_BYTES: usize = 20 * 1024 * 1024 + MINIMUM_WIRE_BYTES;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct Ed25519PublicKey([u8; 32]);

impl Ed25519PublicKey {
    pub(crate) fn from_hex(value: &str) -> Result<Self> {
        let normalized = value.trim().strip_prefix("0x").unwrap_or(value.trim());
        if normalized.len() != 64 {
            return Err(crypto_error("public key has an invalid length"));
        }
        let decoded =
            hex::decode(normalized).map_err(|_| crypto_error("public key is not valid hex"))?;
        let bytes: [u8; 32] = decoded
            .try_into()
            .map_err(|_| crypto_error("public key has an invalid length"))?;
        // Conversion also rejects invalid compressed Edwards points.
        ed25519_public_to_x25519(&bytes)?;
        Ok(Self(bytes))
    }

    pub(crate) fn to_hex(self) -> String {
        hex::encode(self.0)
    }

    pub(crate) const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl std::fmt::Debug for Ed25519PublicKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Ed25519PublicKey([PUBLIC KEY])")
    }
}

pub(crate) struct ClientIdentity {
    public_key: Ed25519PublicKey,
    x25519_secret: StaticSecret,
}

impl ClientIdentity {
    pub(crate) fn generate() -> Self {
        let mut seed = Zeroizing::new([0_u8; 32]);
        OsRng.fill_bytes(seed.as_mut());
        let signing = SigningKey::from_bytes(&seed);
        let public_key = Ed25519PublicKey(signing.verifying_key().to_bytes());
        let mut expanded = Zeroizing::new([0_u8; 64]);
        expanded[..32].copy_from_slice(&signing.to_bytes());
        expanded[32..].copy_from_slice(&public_key.0);
        let x25519_secret = StaticSecret::from(
            ed25519_secret_to_x25519(&expanded[..])
                .expect("a generated Ed25519 secret has the required length"),
        );
        Self {
            public_key,
            x25519_secret,
        }
    }

    pub(crate) fn public_key_hex(&self) -> String {
        self.public_key.to_hex()
    }

    pub(crate) fn decrypt_hex(&self, wire_hex: &str) -> Result<Vec<u8>> {
        decrypt_hex(&self.x25519_secret, wire_hex)
    }
}

impl std::fmt::Debug for ClientIdentity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ClientIdentity")
            .field("public_key", &self.public_key)
            .field("x25519_secret", &"[REDACTED]")
            .finish()
    }
}

pub(crate) fn encrypt_hex(recipient: Ed25519PublicKey, plaintext: &[u8]) -> Result<String> {
    if plaintext.len() > MAXIMUM_WIRE_BYTES - MINIMUM_WIRE_BYTES {
        return Err(crypto_error("plaintext exceeds the protocol limit"));
    }
    let recipient_x25519 = X25519PublicKey::from(ed25519_public_to_x25519(&recipient.0)?);
    let ephemeral_secret = StaticSecret::random_from_rng(OsRng);
    let ephemeral_public = X25519PublicKey::from(&ephemeral_secret);
    let shared = ephemeral_secret.diffie_hellman(&recipient_x25519);
    let key = derive_key(shared.as_bytes())?;
    let cipher = XChaCha20Poly1305::new((&*key).into());

    let mut nonce = [0_u8; NONCE_BYTES];
    OsRng.fill_bytes(&mut nonce);
    let ciphertext = cipher
        .encrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: plaintext,
                aad: &[],
            },
        )
        .map_err(|_| crypto_error("payload encryption failed"))?;

    let mut wire = Vec::with_capacity(WIRE_PREFIX_BYTES + ciphertext.len());
    wire.extend_from_slice(ephemeral_public.as_bytes());
    wire.extend_from_slice(&nonce);
    wire.extend_from_slice(&ciphertext);
    Ok(hex::encode(wire))
}

fn decrypt_hex(recipient_secret: &StaticSecret, wire_hex: &str) -> Result<Vec<u8>> {
    if wire_hex.len() > MAXIMUM_WIRE_BYTES.saturating_mul(2) {
        return Err(decryption_error("ciphertext exceeds the protocol limit"));
    }
    let normalized = wire_hex
        .trim()
        .strip_prefix("0x")
        .unwrap_or(wire_hex.trim());
    let wire =
        hex::decode(normalized).map_err(|_| decryption_error("ciphertext is not valid hex"))?;
    if !(MINIMUM_WIRE_BYTES..=MAXIMUM_WIRE_BYTES).contains(&wire.len()) {
        return Err(decryption_error("ciphertext has an invalid length"));
    }
    let ephemeral_bytes: [u8; EPHEMERAL_PUBLIC_BYTES] =
        wire[..EPHEMERAL_PUBLIC_BYTES]
            .try_into()
            .map_err(|_| decryption_error("ciphertext prefix is invalid"))?;
    let ephemeral_public = X25519PublicKey::from(ephemeral_bytes);
    let nonce = &wire[EPHEMERAL_PUBLIC_BYTES..WIRE_PREFIX_BYTES];
    let ciphertext = &wire[WIRE_PREFIX_BYTES..];
    let shared = recipient_secret.diffie_hellman(&ephemeral_public);
    let key = derive_key(shared.as_bytes())?;
    let cipher = XChaCha20Poly1305::new((&*key).into());
    cipher
        .decrypt(
            XNonce::from_slice(nonce),
            Payload {
                msg: ciphertext,
                aad: &[],
            },
        )
        .map_err(|_| decryption_error("ciphertext authentication failed"))
}

fn ed25519_public_to_x25519(ed25519: &[u8; 32]) -> Result<[u8; 32]> {
    CompressedEdwardsY(*ed25519)
        .decompress()
        .map(|point| point.to_montgomery().to_bytes())
        .ok_or_else(|| crypto_error("public key is not a valid Ed25519 point"))
}

fn ed25519_secret_to_x25519(secret: &[u8]) -> Result<[u8; 32]> {
    if secret.len() != 64 {
        return Err(crypto_error("Ed25519 secret has an invalid length"));
    }
    let digest = Sha512::digest(&secret[..32]);
    let mut converted = [0_u8; 32];
    converted.copy_from_slice(&digest[..32]);
    converted[0] &= 0xf8;
    converted[31] &= 0x7f;
    converted[31] |= 0x40;
    Ok(converted)
}

fn derive_key(shared_secret: &[u8]) -> Result<Zeroizing<[u8; 32]>> {
    let hkdf = Hkdf::<Sha256>::new(None, shared_secret);
    let mut output = Zeroizing::new([0_u8; 32]);
    hkdf.expand(HKDF_INFO, output.as_mut())
        .map_err(|_| crypto_error("key derivation failed"))?;
    Ok(output)
}

fn crypto_error(detail: &'static str) -> SecureClientError {
    SecureClientError::new(axiom_inference::ProviderFailureKind::Encryption, detail)
}

fn decryption_error(detail: &'static str) -> SecureClientError {
    SecureClientError::new(axiom_inference::ProviderFailureKind::Decryption, detail)
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use serde::Deserialize;

    use super::*;

    #[derive(Deserialize)]
    struct Vectors {
        hkdf_info: String,
        key_conversions: Vec<KeyConversion>,
        secret_conversions: Vec<SecretConversion>,
        #[serde(rename = "hkdf_vectors")]
        hkdf_cases: Vec<HkdfVector>,
        encrypt_roundtrips: Vec<Roundtrip>,
    }

    #[derive(Deserialize)]
    struct KeyConversion {
        ed25519_public_hex: String,
        x25519_public_hex: String,
    }

    #[derive(Deserialize)]
    struct SecretConversion {
        ed25519_secret_hex: String,
        x25519_secret_hex: String,
    }

    #[derive(Deserialize)]
    struct HkdfVector {
        shared_secret_hex: String,
        derived_key_hex: String,
    }

    #[derive(Deserialize)]
    struct Roundtrip {
        recipient_x25519_secret_hex: String,
        recipient_ed25519_public_hex: String,
        wire_hex: String,
        expected_plaintext: String,
    }

    fn vectors() -> Vectors {
        serde_json::from_str(include_str!("../../../../../fixtures/e2ee/near-v2.json")).unwrap()
    }

    #[test]
    fn authoritative_conversion_and_hkdf_vectors_match() {
        let vectors = vectors();
        assert_eq!(vectors.hkdf_info.as_bytes(), HKDF_INFO);
        for vector in vectors.key_conversions {
            let key = Ed25519PublicKey::from_hex(&vector.ed25519_public_hex).unwrap();
            assert_eq!(
                hex::encode(ed25519_public_to_x25519(&key.0).unwrap()),
                vector.x25519_public_hex
            );
        }
        for vector in vectors.secret_conversions {
            let secret = hex::decode(vector.ed25519_secret_hex).unwrap();
            assert_eq!(
                hex::encode(ed25519_secret_to_x25519(&secret).unwrap()),
                vector.x25519_secret_hex
            );
        }
        for vector in vectors.hkdf_cases {
            let shared = hex::decode(vector.shared_secret_hex).unwrap();
            assert_eq!(
                hex::encode(*derive_key(&shared).unwrap()),
                vector.derived_key_hex
            );
        }
    }

    #[test]
    fn decrypts_python_and_round_trips_rust_ciphertext() {
        for vector in vectors().encrypt_roundtrips {
            let secret = StaticSecret::from(
                <[u8; 32]>::try_from(hex::decode(&vector.recipient_x25519_secret_hex).unwrap())
                    .unwrap(),
            );
            assert_eq!(
                decrypt_hex(&secret, &vector.wire_hex).unwrap(),
                vector.expected_plaintext.as_bytes()
            );
            let public = Ed25519PublicKey::from_hex(&vector.recipient_ed25519_public_hex).unwrap();
            let encrypted = encrypt_hex(public, vector.expected_plaintext.as_bytes()).unwrap();
            assert_eq!(
                decrypt_hex(&secret, &encrypted).unwrap(),
                vector.expected_plaintext.as_bytes()
            );
        }
    }

    #[test]
    fn generated_identity_is_fresh_and_redacted() {
        let first = ClientIdentity::generate();
        let second = ClientIdentity::generate();
        assert_ne!(first.public_key_hex(), second.public_key_hex());
        let debug = format!("{first:?}");
        assert!(debug.contains("REDACTED"));
        assert!(!debug.contains(&first.public_key_hex()));

        let first_public = Ed25519PublicKey::from_hex(&first.public_key_hex()).unwrap();
        let wire = encrypt_hex(first_public, b"cherry").unwrap();
        assert_eq!(first.decrypt_hex(&wire).unwrap(), b"cherry");
        assert!(second.decrypt_hex(&wire).is_err());
    }

    #[test]
    fn tampering_truncation_wrong_key_and_limits_fail_closed() {
        let recipient = ClientIdentity::generate();
        let public = Ed25519PublicKey::from_hex(&recipient.public_key_hex()).unwrap();
        let wire = encrypt_hex(public, b"private prompt").unwrap();

        let mut tampered = hex::decode(&wire).unwrap();
        *tampered.last_mut().unwrap() ^= 1;
        assert!(recipient.decrypt_hex(&hex::encode(tampered)).is_err());
        assert!(recipient.decrypt_hex("00").is_err());
        assert!(recipient.decrypt_hex("not-hex").is_err());
        assert!(
            encrypt_hex(
                public,
                &vec![0; MAXIMUM_WIRE_BYTES - MINIMUM_WIRE_BYTES + 1]
            )
            .is_err()
        );
    }

    #[test]
    fn boundary_sized_plaintext_round_trips() {
        let recipient = ClientIdentity::generate();
        let public = Ed25519PublicKey::from_hex(&recipient.public_key_hex()).unwrap();
        for plaintext in [
            Vec::new(),
            vec![b'x'; 4_096],
            vec![b'y'; axiom_inference::MAX_MESSAGE_TEXT_BYTES],
        ] {
            let wire = encrypt_hex(public, &plaintext).unwrap();
            assert_eq!(recipient.decrypt_hex(&wire).unwrap(), plaintext);
        }
    }

    #[test]
    #[ignore = "requires AXIOM_BACKEND_SOURCE pointing at the authoritative Axiom backend"]
    fn python_decrypts_rust_ciphertext() {
        let backend = std::env::var("AXIOM_BACKEND_SOURCE").expect("AXIOM_BACKEND_SOURCE");
        let vector = vectors().encrypt_roundtrips.remove(0);
        let public = Ed25519PublicKey::from_hex(&vector.recipient_ed25519_public_hex).unwrap();
        let wire = encrypt_hex(public, b"rust to python cherry").unwrap();
        let script = r"
import sys
from axiom_api.inference.e2ee import decrypt_response_hex
plaintext = decrypt_response_hex(sys.argv[1], bytes.fromhex(sys.argv[2]))
sys.stdout.write(plaintext)
";
        let output = std::process::Command::new("python3")
            .arg("-c")
            .arg(script)
            .arg(wire)
            .arg(vector.recipient_x25519_secret_hex)
            .env("PYTHONPATH", format!("{backend}/src"))
            .output()
            .expect("python3 must run");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, b"rust to python cherry");
    }

    proptest! {
        #[test]
        fn arbitrary_ciphertext_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..2048)) {
            let recipient = ClientIdentity::generate();
            let _ = recipient.decrypt_hex(&hex::encode(bytes));
        }
    }
}
