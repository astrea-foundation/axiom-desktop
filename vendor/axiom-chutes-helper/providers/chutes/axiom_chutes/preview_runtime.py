"""Self-contained staging supervisor, inlined into the authenticated Chute code.

This lets a code update reuse the existing immutable model/engine image. It
does not modify image files, turn off Aegis, or admit an Axiom inference Core.
"""

import asyncio
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import tempfile
import threading
import time

import httpx

from axiom_chutes.wire import canonical, decode_json, unbase64
from axiom_chutes.performance import performance_command, performance_generate, performance_image_command, performance_vision_warmup


RUNTIME_FAILURE_EXIT_SECONDS = 35


class PreviewRuntime:
    def __init__(self, config_b64, performance_profile="graphs-marlin", warm_image=None):
        self.llm_arguments = performance_command(performance_profile)
        self.performance_profile = performance_profile
        self.performance = {"profile": performance_profile, "warmup_complete": False}
        self.warm_image = warm_image
        self.config_b64 = config_b64
        self.state = None
        self.children = []
        self.broker = None
        self.monitor = None
        self.exit_code = None
        self.active = False
        self.failure_lock = asyncio.Lock()
        # The diagnostics interface reports this in-process supervisor's state.
        self.process = self

    def poll(self):
        return self.exit_code

    def _child(self, name, arguments, env):
        descriptor = os.open(self.state / (name + ".log"), os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(descriptor, "wb", buffering=0) as log:
            child = subprocess.Popen(arguments, env=env, stdin=subprocess.DEVNULL,
                stdout=subprocess.DEVNULL if name == "image" else log, stderr=subprocess.STDOUT, close_fds=False, start_new_session=True)
        self.children.append((name, child))

    async def _ready(self, endpoints, seconds):
        until = time.monotonic() + seconds
        async with httpx.AsyncClient(trust_env=False, follow_redirects=False, timeout=2) as http:
            while time.monotonic() < until:
                for name, child in self.children:
                    code = child.poll()
                    if code is not None:
                        raise RuntimeError(f"{name} exited during startup ({code})")
                checks = []
                for endpoint in endpoints:
                    try:
                        checks.append((await http.get(endpoint)).status_code == 200)
                    except httpx.HTTPError:
                        checks.append(False)
                if all(checks):
                    return
                await asyncio.sleep(1)
        raise TimeoutError("fixed staging readiness deadline exceeded")

    async def start(self):
        from aiohttp import web
        from chutes.entrypoint._shared import is_tee_env
        from chutes.entrypoint.verify import TeeEvidenceService
        if not is_tee_env():
            raise RuntimeError("a Chutes TEE deployment is required")
        cfg = decode_json(unbase64(self.config_b64, 64 * 1024), 64 * 1024)
        if (cfg.get("provider") != "chutes" or cfg.get("fixture", False)
                or cfg.get("listen") != "127.0.0.1:8005"
                or cfg.get("encryption_key_file") or cfg.get("identity_key_file")
                or cfg.get("model_root") != "/models"
                or cfg.get("model_manifest_file") != "/opt/axiom/models.json"):
            raise ValueError("fixed non-fixture staging configuration required")
        self.state = Path(tempfile.mkdtemp(prefix="axiom-chutes-"))
        os.chmod(self.state, 0o700)
        socket = self.state / "attestation.sock"
        cfg.update(attestation_socket=str(socket), boot_state_file=str(self.state / "worker.boot"),
                   llm_origin="http://127.0.0.1:10101", image_origin="http://127.0.0.1:8080")

        async def attest(request):
            value = decode_json(await request.read(), 1024)
            if (not isinstance(value, dict) or set(value) != {"challenge"}
                    or not isinstance(value["challenge"], str)
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
        path = self.state / "worker.json"
        if "runtime_authority" in cfg:
            authority_path = self.state / "runtime-authority.json"
            authority_path.write_bytes(canonical(cfg.pop("runtime_authority")))
            authority_path.chmod(0o600)
            from axiom_chutes.publisher import PublisherAuthority
            PublisherAuthority.load(authority_path)
            os.environ["AXIOM_RUNTIME_AUTHORITY_FILE"] = str(authority_path)
            os.environ["AXIOM_RUNTIME_STATE_FILE"] = str(self.state / "runtime-policy-state.json")
            os.environ["AXIOM_RUNTIME_CHUTE_ID"] = cfg["runtime_chute_id"]
        cfg.pop("runtime_chute_id", None)
        descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(descriptor, "wb") as target:
            target.write(canonical(cfg))
        os.environ["AXIOM_IMAGE_ENGINE_TOKEN"] = os.urandom(32).hex()
        env = {**os.environ, "AXIOM_RUNTIME_PROVIDER": "chutes", "AXIOM_CHUTES_BOOTSTRAP_ONLY": "1",
               "HF_HUB_OFFLINE": "1", "HF_HUB_DISABLE_TELEMETRY": "1", "DO_NOT_TRACK": "1",
               "TOKENIZERS_PARALLELISM": "false", "FLASHINFER_NO_DOWNLOAD": "1",
               "VLLM_USE_V2_MODEL_RUNNER": "0",
               # FlashInfer 0.7 defaults SM120 to the CUDA-12.9-only family
               # target. CUDA 12.8 supports this architecture-specific target.
               "FLASHINFER_CUDA_ARCH_LIST": "12.0a"}
        env.pop("VLLM_API_KEY", None)
        env.pop("VLLM_MARLIN_INPUT_DTYPE", None)
        # Console scripts without a .py suffix are not bytecode-manifest entries.
        # Match the provider's module launch and preserve its authorization FDs.
        try:
            self._child("llm", self.llm_arguments, {**env, "OMP_NUM_THREADS": "1", "MKL_NUM_THREADS": "1"})
            self._child("image", performance_image_command(), env)
            await self._ready(["http://127.0.0.1:10101/health",
                               "http://127.0.0.1:8080/axiom/v1/health"], 1800)
            # Compile/capture and verify real decode batches before activation.
            # The Rust worker has not created its boot keys or opened metadata.
            warmups = []
            for count in (1, 4):
                batch = await asyncio.gather(*(performance_generate(os.urandom(32).hex())
                    for _ in range(count)), return_exceptions=True)
                if any(isinstance(value, BaseException) for value in batch):
                    self.performance["warmup_failures"] = [type(value).__name__
                        for value in batch if isinstance(value, BaseException)]
                    raise RuntimeError("fixed coding warmup failed before activation")
                warmups.append({"batch_size": count,
                    "seconds": [value["seconds"] for value in batch],
                    "completion_tokens": [value["usage"]["completion_tokens"] for value in batch]})
            self.performance.update(warmup_complete=True, warmup_batches=warmups)
            if self.warm_image is not None:
                image = await self.warm_image({"challenge": os.urandom(32).hex(), "workload": "image"})
                self.performance["image_warmup"] = {key: image[key]
                    for key in ("seconds", "image_bytes", "image_sha256")}
            self.performance["vision_warmup"] = [await performance_vision_warmup(count) for count in (1, 2)]
            (self.state / "performance.json").write_bytes(canonical(self.performance))
            os.chmod(self.state / "performance.json", 0o600)
            self._child("worker", ["/usr/local/bin/axiom-worker", str(path), "--bootstrap-only"], env)
            await self._ready(["http://127.0.0.1:8006/healthz"], 1200)
        except asyncio.CancelledError:
            await self.close()
            raise
        except Exception as error:
            await self._fail(error)
            # A completed startup hook activates the SDK instance. Propagate
            # engine and warmup failures so a dead runtime cannot become hot.
            raise
        # As in the provider's own LLM template, startup completes before Aegis
        # arms. The engines and real Rust metadata worker are already ready.
        self.active = True
        self.monitor = asyncio.create_task(self._monitor())

    async def _stop_children(self):
        for _, child in reversed(self.children):
            if child.poll() is None:
                try:
                    os.killpg(child.pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
        for _, child in self.children:
            if child.poll() is None:
                try:
                    await asyncio.wait_for(asyncio.to_thread(child.wait), timeout=10)
                except TimeoutError:
                    try:
                        os.killpg(child.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                    await asyncio.to_thread(child.wait)

    async def _fail(self, error):
        async with self.failure_lock:
            if self.exit_code is not None:
                return
            await self._fail_once(error)

    async def _fail_once(self, error):
        self.exit_code = 1
        # Exiting a request coroutine (or setting exit_code) leaves the SDK's
        # liveness ping running. End the entire serving process after cleanup.
        # A separate timer bounds shutdown even if the event loop gets stuck.
        watchdog = None
        if self.active:
            try:
                watchdog = threading.Timer(RUNTIME_FAILURE_EXIT_SECONDS, os._exit, args=(1,))
                watchdog.daemon = True
                watchdog.start()
            except (RuntimeError, MemoryError):
                # RAM/thread exhaustion may prevent a timer from starting.
                # Container teardown must then reclaim the children directly.
                os._exit(1)
        try:
            try:
                report = {"error_type": type(error).__name__,
                          "children": [{"name": name, "returncode": child.poll()} for name, child in self.children]}
                # Only locally constructed stages belong in the failure report.
                if isinstance(error, (RuntimeError, TimeoutError)):
                    report["stage"] = str(error)
                path = self.state / "supervisor-failure.json"
                descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
                with os.fdopen(descriptor, "wb") as target:
                    target.write(canonical(report))
            except (OSError, MemoryError):
                # A full filesystem or exhausted RAM must not prevent cleanup.
                pass
        finally:
            try:
                await self._stop_children()
            finally:
                if watchdog is not None:
                    try:
                        os._exit(1)
                    finally:
                        watchdog.cancel()

    async def _monitor(self):
        while True:
            for name, child in self.children:
                if child.poll() is not None:
                    await self._fail(RuntimeError(f"{name} exited; replace the complete worker epoch"))
                    return
            await asyncio.sleep(1)

    async def close(self):
        if self.monitor is not None:
            self.monitor.cancel()
            await asyncio.gather(self.monitor, return_exceptions=True)
            self.monitor = None
        await self._stop_children()
        if self.broker is not None:
            await self.broker.cleanup()
            self.broker = None
        if self.exit_code is None:
            self.exit_code = 0
