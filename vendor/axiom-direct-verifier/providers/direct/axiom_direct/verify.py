"""Independent verifier for the Rust pool's direct SNP/NVIDIA evidence profile.

The publisher key, verifier digest and sequence floor are installed by the
client/controller. They are never selected by an attester or a model catalog.
"""

import asyncio
import json
import sys
import time

import httpx

from . import PROFILE
from .collect import report_data
from .nvidia import appraise as appraise_gpu
from .release import verify_release, capabilities
from .snp import appraise as appraise_cpu
from .strict import decode_json, hex_bytes, integer, require, signed_payload, unbase64, worker_identity


async def verify(value):
    require(isinstance(value, dict) and set(value) == {"evidence", "context", "nvidia"}, "invalid verifier input")
    require(value["nvidia"] == [], "direct verifier performs its own vendor appraisal")
    context, evidence = value["context"], value["evidence"]
    require(isinstance(context, dict) and set(context) == {"challenge", "origin", "publisher_key", "model_manifest",
            "now", "minimum_policy_sequence"}, "invalid installed verification context")
    local_now = int(time.time())
    now = integer(context["now"])
    require(abs(now-local_now) <= 30, "verification clock mismatch")
    hex_bytes(context["challenge"], 32)
    hex_bytes(context["publisher_key"], 32)
    require(isinstance(evidence, dict) and set(evidence) == {"protocol", "challenge", "identity", "document", "manifest", "policy"}
            and evidence["protocol"] == "axiom-pool-e2ee-v1"
            and evidence["challenge"] == context["challenge"], "evidence challenge/protocol mismatch")
    document = evidence["document"]
    require(isinstance(document, dict) and set(document) == {"profile", "challenge", "worker", "snp_report", "gpu_evidence", "tls_certificate", "amd_collateral"}
            and document["profile"] == PROFILE and document["challenge"] == context["challenge"], "direct document mismatch")
    worker = worker_identity(document["worker"])
    require(worker["logical_origin"] == context["origin"] and worker["model_manifest"] == context["model_manifest"],
            "unexpected worker model/origin")
    statement, _ = signed_payload(evidence["identity"], worker["identity_key"], "axiom-pool-identity-v1")
    require(isinstance(statement, dict) and set(statement) == {"worker", "challenge", "issued_at", "expires_at"}
            and statement["worker"] == worker and statement["challenge"] == context["challenge"], "identity statement mismatch")
    issued, expires = integer(statement["issued_at"]), integer(statement["expires_at"])
    require(issued <= now+30 and now < expires and 0 < expires-issued <= 240, "expired worker identity")
    manifest, policy, accepted = verify_release(evidence["manifest"], evidence["policy"], context["publisher_key"],
        worker, now, integer(context["minimum_policy_sequence"], 1))
    gpu = document["gpu_evidence"]
    require(isinstance(gpu, list) and len(gpu) == 1 and isinstance(gpu[0], dict)
            and set(gpu[0]) == {"arch", "evidence", "certificate"} and gpu[0]["arch"] == "BLACKWELL",
            "exactly one Blackwell GPU required")
    unbase64(gpu[0]["evidence"], 8192)
    unbase64(gpu[0]["certificate"], 32*1024)
    report = unbase64(document["snp_report"], 1184)
    async with httpx.AsyncClient(trust_env=False, follow_redirects=False, timeout=25) as http:
        cpu_info, gpu_expiry = await asyncio.gather(
            appraise_cpu(report, report_data(context["challenge"], worker, gpu, document["tls_certificate"]), policy["amd"], now, http, document['amd_collateral']),
            appraise_gpu(gpu, context["challenge"], policy["nvidia_anchors"]))
    require(cpu_info["measurement"] in accepted, "unapproved guest launch measurement")
    expires = min(expires, gpu_expiry, cpu_info['collateral_expires_at'], now+240)
    if policy["expires_at"] is not None:
        expires = min(expires, policy["expires_at"])
    require(expires > int(time.time()), "evidence expired during appraisal")
    return {"worker": worker, "expires_at": expires, "policy_sequence": policy["sequence"],
            "generation": manifest["generation"], "capabilities": capabilities(manifest)}


def main():
    try:
        value = decode_json(sys.stdin.buffer.read(8*1024*1024+1), 8*1024*1024)
        result = asyncio.run(asyncio.wait_for(verify(value), 55))
        print(json.dumps(result, separators=(",", ":")))
    except Exception:
        # The wire protocol exposes a bounded failure, never credentials or a
        # provider-selected exception string. CLI diagnostics can call verify.
        print("direct workload attestation rejected", file=sys.stderr)
        raise SystemExit(1)


if __name__ == "__main__":
    main()
