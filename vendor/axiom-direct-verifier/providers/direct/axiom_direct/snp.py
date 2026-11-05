"""Strict Turin SNP v5 appraisal using installed AMD roots and fixed KDS URLs."""

from datetime import datetime, timezone
import struct

from cryptography import x509
from cryptography.hazmat.primitives import hashes
from cryptography.hazmat.primitives.asymmetric import ec, rsa, utils
from cryptography.x509.oid import ObjectIdentifier

from .strict import hex_bytes, integer, require, unbase64

KDS = "https://kdsintf.amd.com/vcek/v1/Turin/"
TCB_OIDS = {"fmc": 9, "bootloader": 1, "tee": 2, "snp": 3, "microcode": 8}


def _tcb(raw):
    require(len(raw) == 8 and raw[4:7] == bytes(3), "invalid Turin TCB layout")
    return dict(zip(("fmc", "bootloader", "tee", "snp", "microcode"), (raw[0], raw[1], raw[2], raw[3], raw[7])))


def parse_report(raw):
    require(isinstance(raw, bytes) and len(raw) == 1184, "SNP report size")
    require(struct.unpack_from("<I", raw)[0] == 5 and struct.unpack_from("<I", raw, 52)[0] == 1,
            "only SNP v5 ECDSA-P384 reports supported")
    require(raw[392] == 0x1A and raw[393] <= 0x11 and raw[424:480] == bytes(56), "Turin report required")
    signer = struct.unpack_from("<I", raw, 72)[0]
    require(signer == 0, "unmasked VCEK-signed evidence required")
    policy = struct.unpack_from("<Q", raw, 8)[0]
    require(policy & (1 << 17) and not policy & ((1 << 18) | (1 << 19) | (1 << 21)),
            "SNP debug, migration agents and CXL are forbidden")
    require(struct.unpack_from("<I", raw, 48)[0] == 0, "SNP VMPL0 required")
    require(struct.unpack_from("<Q", raw, 64)[0] & (1 << 5), "AMD platform alias check must be complete")
    tcbs = [raw[a:a+8] for a in (56, 384, 480, 496)]
    require(len(set(tcbs)) == 1, "SNP current/reported/committed/launch TCB mismatch")
    return {"measurement": raw[144:192].hex(), "report_data": raw[80:144],
            "chip_id": raw[416:424], "tcb": _tcb(tcbs[0]), "policy": policy}


def _extension(cert, name):
    return cert.extensions.get_extension_for_oid(ObjectIdentifier(name)).value.value


def _der_integer(raw):
    require(len(raw) in (3, 4) and raw[:2] == bytes((2, len(raw)-2)), "invalid AMD SPL extension")
    value = int.from_bytes(raw[2:], "big", signed=True)
    return integer(value, 0, 255)


def verify_report(raw, expected_data, policy, vcek_der, chain_pem, crl_der, now):
    info = parse_report(raw)
    require(set(policy) == {"product", "root_sha256", "minimum_tcb"} and policy["product"] == "Turin",
            "installed Turin AMD policy required")
    hex_bytes(policy["root_sha256"], 32)
    minimum = policy["minimum_tcb"]
    require(isinstance(minimum, dict) and set(minimum) == set(TCB_OIDS), "complete AMD minimum TCB required")
    for field, minimum_value in minimum.items():
        require(info["tcb"][field] >= integer(minimum_value, 0, 255), "AMD firmware below minimum TCB")
    cert = x509.load_der_x509_certificate(vcek_der)
    chain = x509.load_pem_x509_certificates(chain_pem)
    require(len(chain) == 2, "exact AMD ASK/ARK chain required")
    roots = [c for c in chain if c.subject == c.issuer]
    require(len(roots) == 1, "one AMD ARK required")
    root = roots[0]
    intermediate = next(c for c in chain if c is not root)
    require(root.fingerprint(hashes.SHA256()).hex() == policy["root_sha256"], "unapproved AMD root")
    instant = datetime.fromtimestamp(now, timezone.utc)
    for item in (cert, intermediate, root):
        require(item.not_valid_before_utc <= instant < item.not_valid_after_utc, "expired AMD certificate")
    for item in (intermediate, root):
        require(item.extensions.get_extension_for_class(x509.BasicConstraints).value.ca, "invalid AMD CA")
    cert.verify_directly_issued_by(intermediate)
    intermediate.verify_directly_issued_by(root)
    root.verify_directly_issued_by(root)
    require(_extension(cert, "1.3.6.1.4.1.3704.1.2") == b"\x16\x05Turin", "AMD certificate product mismatch")
    require(_extension(cert, "1.3.6.1.4.1.3704.1.4") == info["chip_id"], "AMD certificate chip mismatch")
    for field, oid in TCB_OIDS.items():
        require(_der_integer(_extension(cert, "1.3.6.1.4.1.3704.1.3." + str(oid))) == info["tcb"][field],
                "AMD VCEK/report TCB mismatch")
    crl = x509.load_der_x509_crl(crl_der)
    issuers = [c for c in (root, intermediate) if c.subject == crl.issuer]
    require(len(issuers) == 1 and crl.next_update_utc is not None
            and crl.last_update_utc <= instant < crl.next_update_utc, "stale AMD revocation collateral")
    key = issuers[0].public_key()
    if isinstance(key, rsa.RSAPublicKey):
        key.verify(crl.signature, crl.tbs_certlist_bytes, crl.signature_algorithm_parameters, crl.signature_hash_algorithm)
    else:
        require(isinstance(key, ec.EllipticCurvePublicKey), "unsupported AMD CRL key")
        key.verify(crl.signature, crl.tbs_certlist_bytes, ec.ECDSA(crl.signature_hash_algorithm))
    for item in (cert, intermediate):
        if item.issuer == crl.issuer:
            require(crl.get_revoked_certificate_by_serial_number(item.serial_number) is None, "revoked AMD certificate")
    public = cert.public_key()
    require(isinstance(public, ec.EllipticCurvePublicKey) and isinstance(public.curve, ec.SECP384R1),
            "AMD P384 VCEK required")
    r, s = [int.from_bytes(raw[offset:offset+72], "little") for offset in (672, 744)]
    require(raw[720:744] == bytes(24) and raw[792:816] == bytes(24) and raw[816:] == bytes(368),
            "noncanonical SNP signature")
    public.verify(utils.encode_dss_signature(r, s), raw[:672], ec.ECDSA(hashes.SHA384()))
    require(info["report_data"] == expected_data, "SNP challenge/workload/key/device binding mismatch")
    info['collateral_expires_at']=int(min(*(item.not_valid_after_utc for item in (cert,intermediate,root)),
        crl.next_update_utc).timestamp())
    return info


async def fetch_collateral(info, http):
    # These untrusted fields only select bounded collateral at fixed AMD URLs.
    tcb = info["tcb"]
    params = {"fmcSPL": tcb["fmc"], "blSPL": tcb["bootloader"], "teeSPL": tcb["tee"],
              "snpSPL": tcb["snp"], "ucodeSPL": tcb["microcode"]}
    import asyncio
    async def get(path, params=None):
        for attempt in range(4):
            async with http.stream("GET", KDS + path, params=params) as response:
                if response.status_code == 200:
                    collateral = bytearray()
                    async for chunk in response.aiter_bytes():
                        collateral.extend(chunk)
                        require(len(collateral) <= 2 * 1024 * 1024, "AMD collateral limit")
                    return bytes(collateral)
                require(attempt < 3 and response.status_code in {429, 500, 502, 503, 504},
                        "AMD collateral unavailable")
                retry_after = response.headers.get("retry-after", "")
                delay = min(10, int(retry_after)) if retry_after.isascii() and retry_after.isdigit() and len(retry_after) <= 3 else 2 ** attempt
            # Only a public collateral GET is repeated, before inference. All
            # signatures, revocation, firmware and request bindings below are
            # still checked against the complete returned collateral.
            await asyncio.sleep(max(1, delay))
    return await asyncio.gather(get(info["chip_id"].hex(), params), get("cert_chain"), get("crl"))


def decode_collateral(value):
    require(isinstance(value,dict) and set(value)=={'vcek','chain','crl'},'complete AMD collateral required')
    values=[unbase64(value[key],2*1024*1024) for key in ('vcek','chain','crl')]
    require(sum(map(len,values))<=1024*1024,'AMD collateral aggregate limit')
    return values


async def appraise(raw, expected_data, policy, now, http, collateral=None):
    # Supplied manufacturer collateral is untrusted. The exact same complete
    # signature, CRL, date, chip/TCB and fresh report binding checks run below.
    if collateral is None:
        vcek,chain,crl=await fetch_collateral(parse_report(raw),http)
    else:
        vcek,chain,crl=decode_collateral(collateral)
    return verify_report(raw, expected_data, policy, vcek, chain, crl, now)
