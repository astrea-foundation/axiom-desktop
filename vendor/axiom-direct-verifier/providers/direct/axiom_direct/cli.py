"""Build/sign/inspect a direct worker without enrolling observed measurements."""

import argparse
import asyncio
import base64
import hashlib
import json
import os
from pathlib import Path
import secrets
import subprocess
import sys
import time

import httpx
from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

from .collect import gpu_evidence, snp_report
from .nvidia import appraise as appraise_gpu
from .release import MANIFEST_DOMAIN, POLICY_DOMAIN, policy_expiration
from .snp import appraise as appraise_cpu
from .strict import canonical, decode_json, hex_bytes, require, sha256, sign_document, signed_payload


def read(path, limit=8*1024*1024):
    return decode_json(Path(path).read_bytes(), limit)


def private_key(path):
    path = Path(path)
    require(path.stat().st_mode & 0o077 == 0, "private signing key permissions required")
    key = serialization.load_pem_private_key(path.read_bytes(), password=None)
    require(isinstance(key, Ed25519PrivateKey), "Ed25519 signing key required")
    return key


def write_json(path, value):
    Path(path).write_bytes(canonical(value)+b"\n")


async def hardware_check(roots, accepted_measurements=None):
    nonce = secrets.token_hex(32)
    gpu = gpu_evidence(nonce)
    data = hashlib.sha512(b"axiom-direct-hardware-probe-v1\0" + bytes.fromhex(nonce) + canonical(gpu)).digest()
    report = snp_report(data)
    now = int(time.time())
    async with httpx.AsyncClient(trust_env=False, follow_redirects=False, timeout=25) as http:
        cpu, expiry = await asyncio.gather(appraise_cpu(report, data, roots["amd"], now, http),
            appraise_gpu(gpu, nonce, roots["nvidia_anchors"]))
    if accepted_measurements is not None:
        require(cpu["measurement"] in accepted_measurements, "unapproved startup launch measurement")
    return {"hardware_verified": True, "workload_keys_verified": False, "measurement": cpu["measurement"],
            "tcb": cpu["tcb"], "expires_at": min(expiry, cpu["collateral_expires_at"], now+240)}


def measure(firmware, kernel, initrd, command_line, vcpus, vcpu_sig, guest_features):
    executable = Path(sys.executable).parent / "sev-snp-measure"
    require(executable.is_file(), "install the pinned direct build extra for sev-snp-measure")
    args = [str(executable), "--mode", "snp", "--ovmf", str(firmware), "--kernel", str(kernel),
        "--initrd", str(initrd), "--append", command_line, "--vcpus", str(vcpus),
        "--vcpu-sig", str(vcpu_sig), "--guest-features", hex(guest_features), "--vmm-type", "QEMU"]
    result = subprocess.run(args, check=True, capture_output=True, text=True, timeout=120)
    digest = result.stdout.strip().lower()
    hex_bytes(digest, 48)
    return digest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    p = commands.add_parser("keygen")
    p.add_argument("--directory", type=Path, required=True)
    p = commands.add_parser("hardware-check")
    p.add_argument("--roots", type=Path, required=True)
    p = commands.add_parser("sign-release")
    p.add_argument("--key", type=Path, required=True)
    p.add_argument("--input", type=Path, required=True)
    p.add_argument("--output", type=Path, required=True)
    p = commands.add_parser("measure")
    for name in ("firmware", "kernel", "initrd", "command-line-file", "output"):
        p.add_argument("--"+name, type=Path, required=True)
    p.add_argument("--vcpus", type=int, required=True)
    p.add_argument("--vcpu-sig", type=lambda v:int(v,0), required=True)
    p.add_argument("--guest-features", type=lambda v:int(v,0), default=1)
    p = commands.add_parser("sign-policy")
    for name in ("key", "release", "roots", "measurement", "output"):
        p.add_argument("--"+name, type=Path, required=True)
    p.add_argument("--sequence", type=int, required=True)
    p.add_argument("--lifetime", type=int, default=0, help="seconds until expiry; 0 keeps approval until explicitly revoked (default)")
    p = commands.add_parser("verify")
    p.add_argument("--input", type=Path, required=True)
    args = parser.parse_args()
    if args.command == "keygen":
        args.directory.mkdir(mode=0o700, parents=True, exist_ok=True)
        authority = {}
        for name in ("publisher", "issuer"):
            key = Ed25519PrivateKey.generate()
            raw = key.private_bytes(serialization.Encoding.PEM, serialization.PrivateFormat.PKCS8,
                serialization.NoEncryption())
            fd = os.open(args.directory/(name+".pem"), os.O_WRONLY|os.O_CREAT|os.O_EXCL, 0o600)
            with os.fdopen(fd,"wb") as output: output.write(raw)
            authority[name+"_key"] = key.public_key().public_bytes_raw().hex()
        write_json(args.directory/"authority.json",authority)
        print(json.dumps(authority))
    elif args.command == "hardware-check":
        print(json.dumps(asyncio.run(hardware_check(read(args.roots)))))
    elif args.command == "sign-release":
        value = read(args.input)
        require(value.get("schema") == MANIFEST_DOMAIN, "direct manifest schema required")
        document = sign_document(value,private_key(args.key),MANIFEST_DOMAIN)
        write_json(args.output,document)
        print(json.dumps({"deployment_digest":sha256(canonical(value)),"generation":value["generation"]}))
    elif args.command == "measure":
        command_line = args.command_line_file.read_text().strip()
        require("\n" not in command_line and "\0" not in command_line, "one kernel command line required")
        digest = measure(args.firmware,args.kernel,args.initrd,command_line,args.vcpus,args.vcpu_sig,args.guest_features)
        out = {"measurement":digest,"vcpus":args.vcpus,"vcpu_sig":args.vcpu_sig,"guest_features":args.guest_features,
            "firmware_sha256":sha256(args.firmware.read_bytes()),"kernel_sha256":sha256(args.kernel.read_bytes()),
            "initrd_sha256":sha256(args.initrd.read_bytes()),"command_line_sha256":sha256(command_line.encode())}
        write_json(args.output,out)
        print(json.dumps(out))
    elif args.command == "sign-policy":
        key = private_key(args.key)
        release, raw = signed_payload(read(args.release),key.public_key().public_bytes_raw().hex(),MANIFEST_DOMAIN)
        roots, measured = read(args.roots), read(args.measurement)
        require(measured["firmware_sha256"] == release["boot"]["firmware_sha256"]
            and measured["kernel_sha256"] == release["boot"]["kernel_sha256"]
            and all(measured[k] == release["boot"][k] for k in ("vcpus","vcpu_sig","guest_features")),
            "candidate boot and signed release differ")
        require(1 <= args.sequence, "positive signed policy sequence required")
        hex_bytes(measured["measurement"],48)
        now = int(time.time())
        value = {"schema":POLICY_DOMAIN,"sequence":args.sequence,"issued_at":now,"expires_at":policy_expiration(now,args.lifetime),
            "minimum_generation":release["generation"],"releases":[{"deployment_digest":sha256(raw),
                "measurement":measured["measurement"]}],"revoked_deployments":[],**roots}
        write_json(args.output,sign_document(value,key,POLICY_DOMAIN))
        print(json.dumps({"sequence":args.sequence,"expires_at":value["expires_at"],"measurement":measured["measurement"]}))
    elif args.command == "verify":
        from .verify import verify
        print(json.dumps(asyncio.run(verify(read(args.input)))))


if __name__ == "__main__":
    main()
