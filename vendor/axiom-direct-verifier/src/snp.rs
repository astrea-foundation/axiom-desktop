use crate::{cert, strict::*};
use anyhow::{Context, Result, ensure};
use p384::ecdsa::{Signature, signature::Verifier};
use serde_json::Value;
use sev::{firmware::guest::AttestationReport, parser::ByteParser};
use x509_parser::prelude::*;

const TCB: [(&str, u8); 5] = [
    ("fmc", 9),
    ("bootloader", 1),
    ("tee", 2),
    ("snp", 3),
    ("microcode", 8),
];

pub fn parse(raw: &[u8]) -> Result<AttestationReport> {
    ensure!(raw.len() == 1184, "SNP report size");
    let report = AttestationReport::from_bytes(raw)?;
    ensure!(
        report.version == 5 && report.sig_algo == 1,
        "SNP v5 P384 required"
    );
    ensure!(
        raw[392] == 0x1a && raw[393] <= 0x11 && raw[424..480] == [0; 56],
        "Turin required"
    );
    ensure!(raw[72..76] == [0; 4], "unmasked VCEK required");
    let policy = u64::from_le_bytes(raw[8..16].try_into()?);
    ensure!(
        policy & (1 << 17) != 0 && policy & ((1 << 18) | (1 << 19) | (1 << 21)) == 0,
        "SNP debug/migration/CXL forbidden"
    );
    ensure!(
        report.vmpl == 0 && raw[64] & (1 << 5) != 0,
        "VMPL0 and alias check required"
    );
    for start in [384, 480, 496] {
        ensure!(raw[56..64] == raw[start..start + 8], "SNP TCB mismatch");
    }
    ensure!(raw[60..63] == [0; 3], "invalid Turin TCB layout");
    Ok(report)
}
pub fn verify(
    raw: &[u8],
    expected: &[u8; 64],
    policy: &Value,
    collateral: &Value,
    now: u64,
) -> Result<(String, u64)> {
    let report = parse(raw)?;
    exact(policy, &["product", "root_sha256", "minimum_tcb"])?;
    ensure!(
        policy["product"] == "Turin",
        "installed Turin policy required"
    );
    hex_bytes(&policy["root_sha256"], 32)?;
    exact(&policy["minimum_tcb"], &TCB.map(|(f, _)| f))?;
    let values = [raw[56], raw[57], raw[58], raw[59], raw[63]];
    for (index, (field, _)) in TCB.iter().enumerate() {
        ensure!(
            u64::from(values[index]) >= integer(&policy["minimum_tcb"][field], 0, 255)?,
            "AMD firmware below minimum TCB"
        );
    }
    exact(collateral, &["vcek", "chain", "crl"])?;
    let vcek = b64(&collateral["vcek"], 2 * 1024 * 1024)?;
    let chain = b64(&collateral["chain"], 2 * 1024 * 1024)?;
    let crl = b64(&collateral["crl"], 2 * 1024 * 1024)?;
    ensure!(
        vcek.len() + chain.len() + crl.len() <= 1024 * 1024,
        "AMD collateral aggregate limit"
    );
    let chain = ::pem::parse_many(chain)?;
    ensure!(
        chain.len() == 2 && chain.iter().all(|p| p.tag() == "CERTIFICATE"),
        "exact AMD ASK/ARK chain required"
    );
    let certificates = chain
        .iter()
        .map(|p| cert::parse(p.contents()))
        .collect::<Result<Vec<_>>>()?;
    let roots = certificates
        .iter()
        .enumerate()
        .filter(|(_, c)| c.subject() == c.issuer())
        .map(|(i, _)| i)
        .collect::<Vec<_>>();
    ensure!(roots.len() == 1, "one ARK required");
    let root_index = roots[0];
    let root = &certificates[root_index];
    let ask = &certificates[1 - root_index];
    let leaf = cert::parse(&vcek)?;
    ensure!(
        sha256(chain[root_index].contents()) == text(&policy["root_sha256"])?,
        "unapproved AMD root"
    );
    let mut expires = u64::MAX;
    for c in [&leaf, ask, root] {
        expires = expires.min(cert::validity(c, now)?);
    }
    cert::ca(ask, None)?;
    cert::ca(root, None)?;
    cert::issued(&leaf, ask)?;
    cert::issued(ask, root)?;
    cert::issued(root, root)?;
    ensure!(
        cert::extension(&leaf, "1.3.6.1.4.1.3704.1.2")? == b"\x16\x05Turin",
        "AMD product mismatch"
    );
    ensure!(
        cert::extension(&leaf, "1.3.6.1.4.1.3704.1.4")? == &raw[416..424],
        "AMD chip mismatch"
    );
    for (index, (_, oid)) in TCB.iter().enumerate() {
        let ext = cert::extension(&leaf, &format!("1.3.6.1.4.1.3704.1.3.{oid}"))?;
        ensure!(
            (ext.len() == 3 || ext.len() == 4)
                && ext[0] == 2
                && usize::from(ext[1]) + 2 == ext.len(),
            "invalid AMD SPL DER"
        );
        let value = if ext.len() == 3 {
            ensure!(ext[2] < 128, "negative AMD SPL");
            ext[2]
        } else {
            ensure!(ext[2] == 0 && ext[3] >= 128, "noncanonical AMD SPL");
            ext[3]
        };
        ensure!(value == values[index], "AMD VCEK/report TCB mismatch");
    }
    let (rest, crl) = CertificateRevocationList::from_der(&crl)
        .map_err(|_| anyhow::anyhow!("invalid AMD CRL"))?;
    ensure!(rest.is_empty(), "trailing CRL data");
    let issuers = [root, ask]
        .into_iter()
        .filter(|c| c.subject() == crl.issuer())
        .collect::<Vec<_>>();
    let next = crl
        .next_update()
        .context("CRL next update required")?
        .timestamp();
    ensure!(
        issuers.len() == 1
            && crl.last_update().timestamp() <= i64::try_from(now)?
            && i64::try_from(now)? < next,
        "stale AMD revocation collateral"
    );
    ensure!(
        crl.signature_algorithm == crl.tbs_cert_list.signature,
        "CRL algorithm mismatch"
    );
    crl.verify_signature(issuers[0].public_key())?;
    for c in [&leaf, ask] {
        if c.issuer() == crl.issuer() {
            ensure!(
                !crl.iter_revoked_certificates()
                    .any(|r| r.raw_serial() == c.raw_serial()),
                "revoked AMD certificate"
            );
        }
    }
    expires = expires.min(u64::try_from(next)?);
    ensure!(
        raw[720..744] == [0; 24] && raw[792..816] == [0; 24] && raw[816..] == [0; 368],
        "noncanonical SNP signature"
    );
    let mut sig = [0u8; 96];
    for i in 0..48 {
        sig[i] = raw[719 - i];
        sig[48 + i] = raw[791 - i];
    }
    // Verify the original signed bytes. Re-encoding a parsed report could erase
    // reserved fields that are also covered by the hardware signature.
    let public = cert::p384_key(&leaf)?;
    public.verify(&raw[..672], &Signature::from_slice(&sig)?)?;
    ensure!(
        &report.report_data == expected,
        "SNP challenge/workload/key/device/TLS binding mismatch"
    );
    Ok((hex::encode(report.measurement), expires))
}
