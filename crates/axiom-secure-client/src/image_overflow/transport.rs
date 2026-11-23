use anyhow::{Context, Result, ensure};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use futures_util::StreamExt as _;
use hpke::{
    Deserializable as _, Serializable as _, aead::AesGcm256, kdf::HkdfSha256, kem::X25519HkdfSha256,
};
use p256::ecdsa::{Signature, SigningKey, signature::Signer as _};
use rand::{SeedableRng as _, rngs::StdRng};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use zeroize::Zeroizing;

use super::{PROTOCOL, now, random_id};

const MAX_FRAME_BYTES: usize = 1024 * 1024;
type Kem = X25519HkdfSha256;

pub(super) struct Transport {
    pub http: reqwest::Client,
    pub origin: String,
    pub encryption_key: String,
    pub signer: SigningKey,
    pub public_key: Value,
}

pub(super) fn public_key(signer: &SigningKey) -> Result<Value> {
    let point = signer.verifying_key().to_encoded_point(false);
    Ok(json!({
        "kty": "EC",
        "crv": "P-256",
        "x": URL_SAFE_NO_PAD.encode(point.x().context("missing P-256 coordinate")?),
        "y": URL_SAFE_NO_PAD.encode(point.y().context("missing P-256 coordinate")?),
    }))
}

impl Transport {
    fn authorize(&self, uri: &str, authority: &str, payload: &str) -> Result<Zeroizing<String>> {
        let header = json!({"alg":"ES256", "typ":"dpop+jwt", "jwk":self.public_key});
        let claims = json!({
            "htm": "POST",
            "htu": uri,
            "iat": now(),
            "jti": random_id(),
            "ath": URL_SAFE_NO_PAD.encode(Sha256::digest(authority.as_bytes())),
            "gateway_key": self.encryption_key,
            "request_digest": hex::encode(Sha256::digest(payload.as_bytes())),
        });
        let signed = format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&header)?),
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims)?),
        );
        let signature: Signature = self.signer.sign(signed.as_bytes());
        Ok(Zeroizing::new(format!(
            "{signed}.{}",
            URL_SAFE_NO_PAD.encode(signature.to_bytes())
        )))
    }

    pub(super) async fn exchange(
        &self,
        path: &str,
        session: Option<&str>,
        authority: &str,
        payload: &Value,
        limit: usize,
        mut receive: impl FnMut(&[u8]) -> Result<()>,
    ) -> Result<()> {
        let uri = format!("{}{path}", self.origin);
        let payload = Zeroizing::new(serde_json::to_string(payload)?);
        let proof = self.authorize(&uri, authority, &payload)?;
        let plaintext = Zeroizing::new(serde_json::to_vec(&json!({
            "protocol": PROTOCOL,
            "session_id": session,
            "proof": proof.as_str(),
            "payload": payload.as_str(),
        }))?);
        let peer = <Kem as hpke::Kem>::PublicKey::from_bytes(&hex::decode(&self.encryption_key)?)
            .map_err(|_| anyhow::anyhow!("invalid gateway encryption key"))?;
        let (encapsulated, mut sender) = hpke::setup_sender::<AesGcm256, HkdfSha256, Kem, _>(
            &hpke::OpModeS::Base,
            &peer,
            tinfoil_ehbp::HPKE_REQUEST_INFO,
            &mut StdRng::from_os_rng(),
        )
        .map_err(|_| anyhow::anyhow!("gateway encryption failed"))?;
        let ciphertext = sender
            .seal(&plaintext, &[])
            .map_err(|_| anyhow::anyhow!("gateway encryption failed"))?;
        let mut body = u32::try_from(ciphertext.len())?.to_be_bytes().to_vec();
        body.extend_from_slice(&ciphertext);
        let mut export = vec![0; tinfoil_ehbp::EXPORT_LENGTH];
        sender
            .export(tinfoil_ehbp::EXPORT_LABEL, &mut export)
            .map_err(|_| anyhow::anyhow!("gateway response key export failed"))?;
        let enc = encapsulated.to_bytes();
        let recovery = tinfoil_ehbp::SessionRecoveryToken::new(export, enc.to_vec())?;
        drop(plaintext);

        // The public shim leaves these application EHBP headers intact.
        let response = self
            .http
            .post(uri)
            .header("content-type", "application/octet-stream")
            .header("axiom-encapsulated-key", hex::encode(enc))
            .body(body)
            .send()
            .await?;
        ensure!(response.status().is_success(), "gateway request rejected");
        let nonce = response
            .headers()
            .get("axiom-response-nonce")
            .context("gateway response encryption required")?
            .to_str()?;
        ensure!(
            super::is_hex(nonce, tinfoil_ehbp::RESPONSE_NONCE_LENGTH),
            "invalid gateway response nonce"
        );
        let mut decryptor = recovery.response_decryptor(&hex::decode(nonce)?)?;
        let mut stream = response.bytes_stream();
        let mut count = 0usize;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            count = count.saturating_add(chunk.len());
            ensure!(count <= limit, "gateway response exceeds limit");
            for frame in decryptor.push(&chunk)? {
                let frame = Zeroizing::new(frame);
                ensure!(
                    frame.len() <= MAX_FRAME_BYTES && frame.last() == Some(&b'\n'),
                    "invalid gateway frame"
                );
                receive(&frame)?;
            }
        }
        decryptor.finish()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use aes_gcm::{
        Aes256Gcm, Nonce,
        aead::{Aead as _, KeyInit as _},
    };
    use p256::ecdsa::signature::Verifier as _;
    use std::collections::BTreeMap;
    use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader};

    use super::*;

    #[tokio::test]
    async fn encrypted_gateway_exchange_binds_request_authority_and_reply() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let (private, public) = <Kem as hpke::Kem>::gen_keypair(&mut StdRng::from_os_rng());
        let signer = SigningKey::random(&mut rand_core::OsRng);
        let key = *signer.verifying_key();
        let transport = Transport {
            http: reqwest::Client::builder().no_proxy().build().unwrap(),
            origin: origin.clone(),
            encryption_key: hex::encode(public.to_bytes()),
            public_key: public_key(&signer).unwrap(),
            signer,
        };
        let payload = json!({"op":"image", "prompt":"Synthetic image test", "overflow_only":true});
        let expected_payload = serde_json::to_string(&payload).unwrap();
        let receiver = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut stream = BufReader::new(stream);
            let mut line = String::new();
            stream.read_line(&mut line).await.unwrap();
            assert_eq!(line.trim_end(), "POST /v1/rpc HTTP/1.1");
            let mut headers = BTreeMap::new();
            loop {
                line.clear();
                stream.read_line(&mut line).await.unwrap();
                if line == "\r\n" {
                    break;
                }
                let (name, value) = line.split_once(':').unwrap();
                headers.insert(name.to_ascii_lowercase(), value.trim().to_owned());
            }
            assert!(!headers.contains_key("authorization") && !headers.contains_key("cookie"));
            let mut request = vec![0; headers["content-length"].parse().unwrap()];
            stream.read_exact(&mut request).await.unwrap();
            assert_eq!(
                u32::from_be_bytes(request[..4].try_into().unwrap()) as usize,
                request.len() - 4
            );
            let enc = hex::decode(&headers["axiom-encapsulated-key"]).unwrap();
            let encapsulated = <Kem as hpke::Kem>::EncappedKey::from_bytes(&enc).unwrap();
            let mut peer = hpke::setup_receiver::<AesGcm256, HkdfSha256, Kem>(
                &hpke::OpModeR::Base,
                &private,
                &encapsulated,
                tinfoil_ehbp::HPKE_REQUEST_INFO,
            )
            .unwrap();
            let envelope: Value =
                serde_json::from_slice(&peer.open(&request[4..], &[]).unwrap()).unwrap();
            assert_eq!(envelope["payload"], expected_payload);
            let (jwt_input, sig) = envelope["proof"]
                .as_str()
                .unwrap()
                .rsplit_once('.')
                .unwrap();
            let signature = Signature::from_slice(&URL_SAFE_NO_PAD.decode(sig).unwrap()).unwrap();
            key.verify(jwt_input.as_bytes(), &signature).unwrap();
            let claims: Value = serde_json::from_slice(
                &URL_SAFE_NO_PAD
                    .decode(jwt_input.split_once('.').unwrap().1)
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(
                claims["request_digest"],
                hex::encode(Sha256::digest(expected_payload))
            );
            assert_eq!(claims["htu"], format!("{origin}/v1/rpc"));
            assert_eq!(
                claims["ath"],
                URL_SAFE_NO_PAD.encode(Sha256::digest(b"fixture-session"))
            );

            let nonce = [17; 32];
            let mut export = [0; 32];
            peer.export(tinfoil_ehbp::EXPORT_LABEL, &mut export)
                .unwrap();
            let material = tinfoil_ehbp::derive_response_keys(&export, &enc, &nonce).unwrap();
            let ciphertext = Aes256Gcm::new_from_slice(&material.key)
                .unwrap()
                .encrypt(
                    Nonce::from_slice(&material.nonce_base),
                    b"{\"fixture\":true}\n".as_slice(),
                )
                .unwrap();
            let header = format!(
                "HTTP/1.1 200 OK\r\ncontent-length: {}\r\naxiom-response-nonce: {}\r\nconnection: close\r\n\r\n",
                ciphertext.len() + 4,
                hex::encode(nonce),
            );
            stream.write_all(header.as_bytes()).await.unwrap();
            stream
                .write_all(&u32::try_from(ciphertext.len()).unwrap().to_be_bytes())
                .await
                .unwrap();
            stream.write_all(&ciphertext).await.unwrap();
        });
        let mut replies = Vec::new();
        transport
            .exchange(
                "/v1/rpc",
                Some("fixture-session"),
                "fixture-session",
                &payload,
                1024,
                |frame| {
                    replies.push(serde_json::from_slice::<Value>(frame)?);
                    Ok(())
                },
            )
            .await
            .unwrap();
        assert_eq!(replies, [json!({"fixture":true})]);
        receiver.await.unwrap();
    }
}
