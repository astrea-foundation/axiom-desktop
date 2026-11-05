"""Chutes ingress only relays Axiom ciphertext to the existing Rust worker.

There is no chat-completions, image engine, raw prompt, private-key export, or
arbitrary upstream route exposed by this adapter. Engine children and the Rust
worker are supervised together. The Rust worker still requires independent
controller admission; a healthy Chutes pod cannot bypass it.
"""

import asyncio
import base64
import hashlib
import hmac
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import tempfile

import httpx

from .wire import canonical, decode_json, unbase64

MAX_REQUEST = 16 * 1024 * 1024
MAX_RESPONSE = 96 * 1024 * 1024


def validate_ciphertext(value):
    if not isinstance(value, dict) or set(value) != {"encapsulated_key", "body"}:
        raise ValueError("only an Axiom encrypted request is accepted")
    if not isinstance(value["encapsulated_key"], str) or not re.fullmatch(r"[0-9a-f]{64}", value["encapsulated_key"]):
        raise ValueError("invalid encapsulated key")
    body = unbase64(value["body"], MAX_REQUEST + 32)
    if len(body) < 20 or int.from_bytes(body[:4], "big") != len(body) - 4:
        raise ValueError("invalid Axiom encrypted request framing")
    return value


async def exchange(value, *, http=None):
    validate_ciphertext(value)
    # Chutes runs user generators on a different event loop. Never share an
    # AsyncClient from an on_startup hook with this request's loop.
    owns_http = http is None
    http = http or httpx.AsyncClient(trust_env=False, follow_redirects=False,
                                    timeout=httpx.Timeout(610, connect=3))
    try:
        async with http.stream("POST", "http://127.0.0.1:8005/v1/exchange", json=value) as response:
            if response.status_code != 200:
                raise RuntimeError("encrypted worker unavailable; do not replay the request")
            nonce = response.headers.get("axiom-pool-response-nonce", "")
            if not re.fullmatch(r"[0-9a-f]{64}", nonce):
                raise ValueError("missing worker response nonce")
            # Explicit transport records preserve the header through Chutes'
            # cord API. Only the inner Axiom AEAD/terminal authenticates it.
            yield canonical({"type": "response", "nonce": nonce}).decode() + "\n"
            total = 0
            async for chunk in response.aiter_bytes(chunk_size=64 * 1024):
                total += len(chunk)
                if total > MAX_RESPONSE:
                    raise ValueError("encrypted response size limit")
                yield canonical({"type": "chunk", "body": base64.b64encode(chunk).decode()}).decode() + "\n"
            yield '{"type":"eof"}\n'
    finally:
        if owns_http:
            await http.aclose()


async def bootstrap(value, *, http=None):
    if (not isinstance(value, dict) or set(value) != {"challenge", "confirmation_secret"}
            or any(not isinstance(v, str) or not re.fullmatch(r"[0-9a-f]{64}", v) for v in value.values())):
        raise ValueError("invalid bootstrap challenge")
    owns_http = http is None
    http = http or httpx.AsyncClient(trust_env=False, follow_redirects=False,
                                    timeout=httpx.Timeout(30, connect=3))
    try:
        async with http.stream("POST", "http://127.0.0.1:8006/v1/attestation",
                               json={"challenge": value["challenge"]}) as response:
            if response.status_code != 200:
                raise RuntimeError("worker evidence unavailable")
            from .client import read_limited
            evidence = decode_json(await read_limited(response))
        confirmation = hmac.new(bytes.fromhex(value["confirmation_secret"]),
                                b"axiom-chutes-bootstrap-v1\0" + canonical(evidence),
                                hashlib.sha256).hexdigest()
        return {"evidence": evidence, "confirmation": confirmation}
    finally:
        if owns_http:
            await http.aclose()


async def health():
    async with httpx.AsyncClient(trust_env=False, follow_redirects=False, timeout=3) as http:
        try:
            result = await http.get("http://127.0.0.1:8005/healthz")
            ready = result.status_code == 200
        except httpx.HTTPError:
            ready = False
    return {"ready": ready, "provider": "chutes", "workload_binding_qualified": False}


class Runtime:
    def __init__(self, *, bootstrap_only=False):
        self.bootstrap_only = bootstrap_only
        self.process = None
        self.broker = None
        self.monitor = None
        self.state = None

    async def start(self):
        if self.state is not None or self.process is not None:
            raise RuntimeError("replace the complete Chutes pod after a worker epoch")
        from aiohttp import web
        from chutes.entrypoint._shared import is_tee_env
        from chutes.entrypoint.verify import TeeEvidenceService
        if not is_tee_env():
            raise RuntimeError("a Chutes TEE deployment is required")
        raw = unbase64(os.environ["AXIOM_WORKER_CONFIG_B64"], 64 * 1024)
        cfg = decode_json(raw, 64 * 1024)
        if cfg.get("provider") != "chutes" or cfg.get("fixture", False) or cfg["listen"] != "127.0.0.1:8005":
            raise ValueError("Chutes production worker configuration required")
        if cfg.get("encryption_key_file") or cfg.get("identity_key_file"):
            raise ValueError("Chutes boot keys must be generated inside the worker")
        self.state = Path(tempfile.mkdtemp(prefix="axiom-chutes-"))
        os.chmod(self.state, 0o700)
        socket = self.state / "attestation.sock"
        cfg["attestation_socket"] = str(socket)
        cfg["boot_state_file"] = str(self.state / "worker.boot")

        async def attest(request):
            value = decode_json(await request.read(), 1024)
            if (not isinstance(value, dict) or set(value) != {"challenge"}
                    or not re.fullmatch(r"[0-9a-f]{64}", value["challenge"])):
                raise web.HTTPBadRequest()
            service = TeeEvidenceService()
            public_key = getattr(service, "_e2e_pubkey", None)
            if not public_key:
                raise web.HTTPServiceUnavailable()
            result = await service._fetch_evidence(value["challenge"])
            document = {"profile": "chutes-gpu-v1", "e2e_pubkey": public_key,
                        "runtime_evidence": result, "workload_binding": None}
            raw = canonical(document)
            if len(raw) > 8 * 1024 * 1024:
                raise web.HTTPBadGateway()
            return web.Response(body=raw, content_type="application/json")

        app = web.Application(client_max_size=1024)
        app.router.add_post("/attest", attest)
        self.broker = web.AppRunner(app, access_log=None)
        await self.broker.setup()
        await web.UnixSite(self.broker, str(socket)).start()
        os.chmod(socket, 0o600)
        env = dict(os.environ)
        env["AXIOM_WORKER_CONFIG_JSON"] = canonical(cfg).decode()
        env["AXIOM_WORKER_STATE_DIR"] = str(self.state)
        if self.bootstrap_only:
            env["AXIOM_CHUTES_BOOTSTRAP_ONLY"] = "1"
        else:
            env.pop("AXIOM_CHUTES_BOOTSTRAP_ONLY", None)
        try:
            log = None
            try:
                if self.bootstrap_only:
                    descriptor = os.open(self.state / "supervisor.log", os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
                    log = os.fdopen(descriptor, "wb", buffering=0)
                self.process = subprocess.Popen([sys.executable, "/opt/axiom/supervise.py"], env=env,
                    stdin=subprocess.DEVNULL, stdout=log if log is not None else subprocess.DEVNULL,
                    stderr=subprocess.STDOUT if log is not None else subprocess.DEVNULL,
                    # Preserve the provider's inheritable authorization handle.
                    # Python sockets/files remain non-inheritable by default.
                    close_fds=False, start_new_session=True)
            finally:
                if log is not None:
                    log.close()
        except BaseException:
            await self.close()
            raise
        self.monitor = asyncio.create_task(self._monitor())
        # Match the SDK's engine startup contract: activation and Aegis arming
        # happen after the engines and worker have completed startup. A staging
        # failure still returns with its diagnostics available; production's
        # monitor terminates the complete pod on any child failure.
        port = 8006 if self.bootstrap_only else 8005
        try:
            async with httpx.AsyncClient(trust_env=False, follow_redirects=False, timeout=2) as http:
                while self.process is not None and self.process.poll() is None:
                    try:
                        if (await http.get(f"http://127.0.0.1:{port}/healthz")).status_code == 200:
                            break
                    except httpx.HTTPError:
                        pass
                    await asyncio.sleep(1)
        except BaseException:
            await self.close()
            raise

    async def _monitor(self):
        while self.process is not None and self.process.poll() is None:
            await asyncio.sleep(1)
        if self.bootstrap_only:
            # Keep the private staging metadata/diagnostic cords available to
            # inspect a failed boot. There is no worker or engine-only restart.
            return
        # There is no frontend-only restart. Let Chutes replace the entire
        # pod/worker epoch; its old boot/key is never reused for a new run.
        os.kill(os.getpid(), signal.SIGTERM)

    async def close(self):
        if self.monitor is not None:
            self.monitor.cancel()
            await asyncio.gather(self.monitor, return_exceptions=True)
            self.monitor = None
        if self.process is not None:
            process, self.process = self.process, None
            if process.poll() is None:
                os.killpg(process.pid, signal.SIGTERM)
                try:
                    await asyncio.wait_for(asyncio.to_thread(process.wait), timeout=30)
                except TimeoutError:
                    os.killpg(process.pid, signal.SIGKILL)
                    await asyncio.to_thread(process.wait)
        if self.broker is not None:
            await self.broker.cleanup()
            self.broker = None
