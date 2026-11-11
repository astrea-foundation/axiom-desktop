use anyhow::{Context, Result, ensure};
use x509_parser::{certificate::X509Certificate, prelude::*};

pub fn parse(raw: &[u8]) -> Result<X509Certificate<'_>> {
    let (rest, cert) =
        X509Certificate::from_der(raw).map_err(|_| anyhow::anyhow!("invalid certificate"))?;
    ensure!(rest.is_empty(), "trailing certificate data");
    Ok(cert)
}
pub fn validity(cert: &X509Certificate<'_>, now: u64) -> Result<u64> {
    let now = i64::try_from(now)?;
    ensure!(
        cert.validity().not_before.timestamp() <= now
            && now < cert.validity().not_after.timestamp(),
        "expired certificate"
    );
    Ok(u64::try_from(cert.validity().not_after.timestamp())?)
}
pub fn ca(cert: &X509Certificate<'_>, path: Option<usize>) -> Result<()> {
    let constraints = cert
        .basic_constraints()?
        .context("CA constraints required")?;
    ensure!(
        constraints.value.ca
            && path.is_none_or(|p| constraints
                .value
                .path_len_constraint
                .is_none_or(|n| u64::from(n) >= p as u64)),
        "invalid CA"
    );
    Ok(())
}
pub fn issued(cert: &X509Certificate<'_>, issuer: &X509Certificate<'_>) -> Result<()> {
    ensure!(
        cert.issuer() == issuer.subject(),
        "certificate issuer mismatch"
    );
    ensure!(
        cert.signature_algorithm == cert.tbs_certificate.signature,
        "certificate algorithm mismatch"
    );
    cert.verify_signature(Some(issuer.public_key()))?;
    Ok(())
}
pub fn extension<'a>(cert: &'a X509Certificate<'_>, oid: &str) -> Result<&'a [u8]> {
    let mut matches = cert
        .extensions()
        .iter()
        .filter(|e| e.oid.to_id_string() == oid);
    let result = matches.next().context("missing AMD extension")?;
    ensure!(matches.next().is_none(), "duplicate AMD extension");
    Ok(result.value)
}

pub fn p384_key(cert: &X509Certificate<'_>) -> Result<p384::ecdsa::VerifyingKey> {
    let algorithm = &cert.public_key().algorithm;
    ensure!(
        algorithm.algorithm.to_id_string() == "1.2.840.10045.2.1"
            && algorithm
                .parameters
                .as_ref()
                .and_then(|p| p.as_oid().ok())
                .is_some_and(|oid| oid.to_id_string() == "1.3.132.0.34"),
        "P384 key required"
    );
    Ok(p384::ecdsa::VerifyingKey::from_sec1_bytes(
        cert.public_key().subject_public_key.data.as_ref(),
    )?)
}
