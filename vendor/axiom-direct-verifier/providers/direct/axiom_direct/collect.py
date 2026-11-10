"""Collect local CPU/GPU evidence for the measured worker's immutable identity.

This is a stdin helper for the worker, not a public arbitrary-report-data API.
The approved measured guest must contain only the approved loader/services and
must not expose administrative shells or another report oracle.
"""

import base64
import ctypes as c
import hashlib
import json
import os
from pathlib import Path
import struct
import sys

from . import PROFILE
from .strict import canonical, decode_json, hex_bytes, require, sha256, unbase64, worker_identity

REPORT_DOMAIN = b"axiom-direct-snp-key-device-binding-v1\0"


def report_data(challenge, worker, gpu_evidence, tls_certificate):
    hex_bytes(challenge, 32)
    worker_identity(worker)
    return hashlib.sha512(REPORT_DOMAIN + canonical({"challenge": challenge, "worker": worker,
        "gpu_evidence_sha256": sha256(canonical(gpu_evidence)),
        "tls_certificate_sha256": sha256(unbase64(tls_certificate, 16*1024))})).digest()


class _SnpRequest(c.Structure):
    _fields_ = [("data", c.c_ubyte * 64), ("vmpl", c.c_uint32), ("reserved", c.c_ubyte * 28)]


class _SnpIoctl(c.Structure):
    _fields_ = [("version", c.c_ubyte), ("request", c.c_uint64), ("response", c.c_uint64),
                ("exitinfo2", c.c_uint64)]


def snp_report(data):
    require(len(data) == 64, "SNP report data size")
    request = _SnpRequest()
    request.data[:] = data
    response = (c.c_ubyte * 4000)()
    args = _SnpIoctl(1, c.addressof(request), c.addressof(response), 0)
    require(c.sizeof(args) == 32 and c.sizeof(request) == 96, "unsupported SNP ioctl ABI")
    number = (3 << 30) | (c.sizeof(args) << 16) | (ord("S") << 8)
    libc = c.CDLL(None, use_errno=True)
    libc.ioctl.argtypes = [c.c_int, c.c_ulong, c.c_void_p]
    libc.ioctl.restype = c.c_int
    fd = os.open("/dev/sev-guest", os.O_RDWR | os.O_CLOEXEC)
    try:
        code = libc.ioctl(fd, number, c.byref(args))
    finally:
        os.close(fd)
    require(code == 0 and args.exitinfo2 == 0, "SNP report request failed")
    raw = bytes(response)
    status, size = struct.unpack_from("<II", raw)
    require(status == 0 and size == 1184, "invalid SNP report response")
    report = raw[32:32+size]
    require(report[80:144] == data, "SNP report data mismatch")
    return report


class _GpuCertificate(c.Structure):
    _fields_ = [("cert_size", c.c_uint), ("attestation_size", c.c_uint),
                ("cert", c.c_ubyte * 0x1000), ("attestation", c.c_ubyte * 0x1400)]


class _GpuReport(c.Structure):
    _fields_ = [("cec_present", c.c_uint), ("size", c.c_uint), ("cec_size", c.c_uint),
                ("nonce", c.c_ubyte * 32), ("report", c.c_ubyte * 0x2000), ("cec", c.c_ubyte * 0x1000)]


class _GpuSettings(c.Structure):
    _fields_ = [("version", c.c_uint), ("environment", c.c_uint), ("feature", c.c_uint),
                ("devtools", c.c_uint), ("multigpu", c.c_uint)]


def gpu_evidence(challenge):
    nonce = hex_bytes(challenge, 32)
    nvml = c.CDLL("libnvidia-ml.so.1")
    def checked(code):
        require(code == 0, "NVML evidence query failed")
    checked(nvml.nvmlInit_v2())
    try:
        settings = _GpuSettings()
        settings.version = c.sizeof(settings) | (1 << 24)
        checked(nvml.nvmlSystemGetConfComputeSettings(c.byref(settings)))
        # These checks execute in the approved, measured collector/driver.
        # Vendor report appraisal independently authenticates the GPU firmware.
        require(settings.environment == 2 and settings.feature == 1 and settings.devtools == 0,
                "production GPU confidential computing with DevTools off required")
        count = c.c_uint()
        checked(nvml.nvmlDeviceGetCount_v2(c.byref(count)))
        require(count.value == 1, "exactly one confidential GPU required")
        device = c.c_void_p()
        checked(nvml.nvmlDeviceGetHandleByIndex_v2(0, c.byref(device)))
        architecture = c.c_uint()
        checked(nvml.nvmlDeviceGetArchitecture(device, c.byref(architecture)))
        require(architecture.value == 10, "Blackwell GPU required")
        cert, report = _GpuCertificate(), _GpuReport()
        checked(nvml.nvmlDeviceGetConfComputeGpuCertificate(device, c.byref(cert)))
        report.nonce[:] = nonce
        checked(nvml.nvmlDeviceGetConfComputeGpuAttestationReport(device, c.byref(report)))
        # This driver returns the CEC buffer capacity even when its presence
        # flag is zero. Only the flagged primary report is sent to NRAS.
        require(0 < cert.attestation_size <= 0x1400 and 0 < report.size <= 0x2000
                and report.cec_present == 0 and report.cec_size <= 0x1000, "unsupported GPU report layout")
        b64 = lambda value: base64.b64encode(value).decode()
        return [{"arch": "BLACKWELL", "evidence": b64(bytes(report.report[:report.size])),
                 "certificate": b64(bytes(cert.attestation[:cert.attestation_size]))}]
    finally:
        nvml.nvmlShutdown()


def collect(challenge, worker):
    hex_bytes(challenge, 32)
    worker_identity(worker)
    gpu = gpu_evidence(challenge)
    certificate_path = Path("/run/axiom-state/tls-certificate.der")
    certificate = certificate_path.read_bytes()
    require(0 < len(certificate) <= 16*1024, "boot TLS certificate required")
    tls = base64.b64encode(certificate).decode()
    report = snp_report(report_data(challenge, worker, gpu, tls))
    from .collateral import shared_collateral
    collateral=shared_collateral(report)
    return {"profile": PROFILE, "challenge": challenge, "worker": worker,
            "snp_report": base64.b64encode(report).decode(), "gpu_evidence": gpu, "tls_certificate": tls,
            "amd_collateral":collateral}


def main():
    try:
        value = decode_json(sys.stdin.buffer.read(16 * 1024 + 1), 16 * 1024)
        require(isinstance(value, dict) and set(value) == {"challenge", "worker"}, "invalid collector input")
        print(json.dumps(collect(value["challenge"], value["worker"]), separators=(",", ":")))
    except Exception:
        print("direct hardware evidence unavailable", file=sys.stderr)
        raise SystemExit(1)


if __name__ == "__main__":
    main()
