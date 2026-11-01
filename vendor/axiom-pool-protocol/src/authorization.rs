use crate::{PROTOCOL, hex_bytes, sha256, strict};
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use p256::ecdsa::{
    Signature, SigningKey, VerifyingKey,
    signature::{Signer, Verifier},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientKey {
    pub kty: String,
    pub crv: String,
    pub x: String,
    pub y: String,
}
impl ClientKey {
    pub fn from_signing(key: &SigningKey) -> Self {
        let point = key.verifying_key().to_encoded_point(false);
        Self {
            kty: "EC".into(),
            crv: "P-256".into(),
            x: URL_SAFE_NO_PAD.encode(point.x().expect("P-256 x")),
            y: URL_SAFE_NO_PAD.encode(point.y().expect("P-256 y")),
        }
    }
    pub fn public(&self) -> Result<VerifyingKey> {
        ensure!(
            self.kty == "EC" && self.crv == "P-256" && self.x.len() == 43 && self.y.len() == 43,
            "invalid client key"
        );
        let x = URL_SAFE_NO_PAD.decode(&self.x)?;
        let y = URL_SAFE_NO_PAD.decode(&self.y)?;
        ensure!(
            x.len() == 32
                && y.len() == 32
                && URL_SAFE_NO_PAD.encode(&x) == self.x
                && URL_SAFE_NO_PAD.encode(&y) == self.y,
            "noncanonical client key"
        );
        Ok(VerifyingKey::from_sec1_bytes(
            &[&[4], x.as_slice(), y.as_slice()].concat(),
        )?)
    }
    pub fn thumbprint(&self) -> Result<String> {
        self.public()?;
        let canonical = serde_json::json!({"crv":self.crv,"kty":self.kty,"x":self.x,"y":self.y});
        Ok(token_hash(&serde_json::to_string(&canonical)?))
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    typ: String,
    alg: String,
    jwk: ClientKey,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Claims {
    pub htm: String,
    pub htu: String,
    pub iat: u64,
    pub jti: String,
    pub ath: String,
    pub pool_protocol: String,
    pub worker_key: String,
    pub request_digest: String,
}
pub fn token_hash(token: &str) -> String {
    use sha2::{Digest, Sha256};
    URL_SAFE_NO_PAD.encode(Sha256::digest(token.as_bytes()))
}
pub fn proof(
    key: &SigningKey,
    token: &str,
    uri: &str,
    worker_key: &str,
    payload: &str,
    now: u64,
) -> Result<String> {
    let header = Header {
        typ: "dpop+jwt".into(),
        alg: "ES256".into(),
        jwk: ClientKey::from_signing(key),
    };
    let claims = Claims {
        htm: "POST".into(),
        htu: uri.into(),
        iat: now,
        jti: crate::random_id(),
        ath: token_hash(token),
        pool_protocol: PROTOCOL.into(),
        worker_key: worker_key.into(),
        request_digest: sha256(payload),
    };
    let signed = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(&header)?),
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims)?)
    );
    let signature: Signature = key.sign(signed.as_bytes());
    Ok(format!(
        "{signed}.{}",
        URL_SAFE_NO_PAD.encode(signature.to_bytes())
    ))
}
pub fn verify(
    proof: &str,
    jkt: &str,
    token: &str,
    uri: &str,
    worker_key: &str,
    payload: &str,
    now: u64,
) -> Result<Claims> {
    ensure!(proof.len() <= 8192, "oversized proof");
    let parts = proof.split('.').collect::<Vec<_>>();
    ensure!(parts.len() == 3, "invalid proof");
    let header: Header = strict::decode(&URL_SAFE_NO_PAD.decode(parts[0])?)?;
    ensure!(
        header.typ == "dpop+jwt" && header.alg == "ES256" && header.jwk.thumbprint()? == jkt,
        "wrong proof key/type"
    );
    let signature = Signature::from_slice(&URL_SAFE_NO_PAD.decode(parts[2])?)?;
    header.jwk.public()?.verify(
        proof
            .rsplit_once('.')
            .context("invalid proof")?
            .0
            .as_bytes(),
        &signature,
    )?;
    let claims: Claims = strict::decode(&URL_SAFE_NO_PAD.decode(parts[1])?)?;
    hex_bytes(&claims.jti, 16)?;
    ensure!(
        claims.htm == "POST"
            && claims.htu == uri
            && claims.iat <= now.saturating_add(30)
            && claims.iat.saturating_add(60) >= now
            && claims.ath == token_hash(token)
            && claims.pool_protocol == PROTOCOL
            && claims.worker_key == worker_key
            && claims.request_digest == sha256(payload),
        "proof binding mismatch"
    );
    Ok(claims)
}
