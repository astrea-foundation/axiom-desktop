"""Private, memory-only ComfyUI execution of the pinned Qwen 2.1 recipe.

No PromptServer, graph executor, history, uploads, output files or custom nodes
are started. The Axiom scheduler owns admission; this process admits one job
and drains its GPU task even if its IPC reader disconnects.
"""
import asyncio
import base64
import contextlib
import gc
import hashlib
import hmac
import io
import json
import math
import os
from pathlib import Path
import re
import sys
import tempfile
from .image_policy import (image_policy, UNCENSORED_LORA_FILE, UNCENSORED_LORA_BYTES,
                           UNCENSORED_LORA_SHA256, UNCENSORED_LORA_STRENGTH)
from .runtime_policy import default_runtime, validate_runtime, runtime_digest, selected_image_policy

COMFY_REVISION = "19ae172bff6c9e4c738e7f2e00fe087f5b5b46ab"
MODEL_FILES = {
    "diffusion_models/qwen_image_2.1_turbo_int8_convrot.safetensors": (7256787829, "8144a0182964aa8403ad6dbc011db837304167d235161b9aabaa8532e967314c"),
    "text_encoders/qwen3vl_8b_int8_convrot.safetensors": (9350798360, "8bfd0f6e12abf2d2d697ecc888e5e90b0d6741d6708f05799f53afa560452e8f"),
    "vae/qwen_image_2.1_vae_bf16.safetensors": (675509688, "bb21f7473051e1ac368515dd3f2e15cd44d7a11748ee8823e1ddca3e4876b7c9"),
    UNCENSORED_LORA_FILE: (UNCENSORED_LORA_BYTES, UNCENSORED_LORA_SHA256),
}
SIGMAS = "1,0.978453,0.95418,0.926626,0.89508,0.845148,0.704534,0.414568,0"
MAX_REQUEST = 36 * 1024 * 1024
MAX_PNG = 10 * 1024 * 1024


def engine_input(value):
    if (not isinstance(value, dict) or set(value) - {"uncensored", "runtime", "runtime_policy_sha256"} != {"job_id", "request_sha256", "prompt", "seed", "width", "height", "references"}
            or type(value.get("uncensored", False)) is not bool
            or any(not isinstance(value[k], str) or not re.fullmatch(r"[0-9a-f]{64}", value[k]) for k in ("job_id", "request_sha256"))
            or not isinstance(value["prompt"], str) or not value["prompt"].strip() or len(value["prompt"].encode()) > 8192
            or type(value["seed"]) is not int or not 0 <= value["seed"] <= 0xffffffff
            or any(type(value[k]) is not int or not 256 <= value[k] <= 8192 or value[k] % 32 for k in ("width", "height"))
            or value["width"] * value["height"] > 1536 ** 2
            or not isinstance(value["references"], list) or len(value["references"]) > 5
            or any(not isinstance(r, str) or len(r) > 7 * 1024 * 1024 for r in value["references"])):
        raise ValueError("invalid private engine request")
    if "runtime" in value or "runtime_policy_sha256" in value:
        runtime = validate_runtime(value.get("runtime"))
        if value.get("runtime_policy_sha256") != runtime_digest(runtime):
            raise ValueError("private runtime policy binding differs")
        if len(value["references"]) > runtime["image"]["max_references"] or value["width"] * value["height"] > runtime["image"]["max_pixels"]:
            raise ValueError("private runtime resource limit")
    return value


class ComfyRecipe:
    def __init__(self):
        # Comfy dependencies live in a separate, locked interpreter environment.
        root = Path(os.environ.get("AXIOM_COMFY_ROOT", "/opt/ComfyUI"))
        model_root = Path(os.environ.get("AXIOM_COMFY_MODEL_ROOT", "/models/image"))
        marker = root / "AXIOM_REVISION"
        if marker.is_file():
            revision = marker.read_text().strip()
        else:
            import subprocess
            revision = subprocess.check_output(["git", "-C", str(root), "rev-parse", "HEAD"], text=True).strip()
        if revision != COMFY_REVISION:
            raise RuntimeError("Comfy source revision differs")
        for name, (size, digest) in MODEL_FILES.items():
            p = model_root / name
            if p.is_symlink() or p.stat().st_size != size:
                raise RuntimeError("Comfy model inventory differs")
            with p.open("rb") as source:
                if hashlib.file_digest(source, "sha256").hexdigest() != digest:
                    raise RuntimeError("Comfy model hash differs")
        self.workspace = tempfile.TemporaryDirectory(prefix="axiom-comfy-")
        sys.path.insert(0, str(root))
        import comfy.options
        comfy.options.enable_args_parsing()
        sys.argv = [str(root / "main.py"), "--base-directory", self.workspace.name,
                    "--preview-method", "none", "--reserve-vram", "2",
                    "--disable-all-custom-nodes", "--disable-partner-nodes", "--disable-metadata", "--disable-assets"]
        import torch
        import nodes
        import folder_paths
        import comfy.model_management
        import comfy.utils
        from comfy_extras import nodes_custom_sampler, nodes_qwen
        for kind in ("diffusion_models", "text_encoders", "vae", "loras"):
            folder_paths.folder_names_and_paths[kind] = ([str(model_root / kind)], {".safetensors"})
        folder_paths.folder_names_and_paths["embeddings"] = ([], {".safetensors"})
        torch.set_num_threads(4)
        torch.set_num_interop_threads(1)
        self.torch, self.nodes = torch, nodes
        self.model_root = model_root
        self.sampling, self.qwen = nodes_custom_sampler, nodes_qwen
        self.utils, self.management = comfy.utils, comfy.model_management
        # Only weights survive a request. No graph/node/conditioning cache exists.
        self.model = nodes.UNETLoader().load_unet("qwen_image_2.1_turbo_int8_convrot.safetensors", "default")[0]
        self.uncensored_model = nodes.LoraLoaderModelOnly().load_lora_model_only(
            self.model, "uncensored.safetensors", UNCENSORED_LORA_STRENGTH)[0]
        if self.model.patches or len(self.uncensored_model.patches) != 192:
            raise RuntimeError("uncensored adapter did not match the pinned image model")
        self.clip = nodes.CLIPLoader().load_clip("qwen3vl_8b_int8_convrot.safetensors", "qwen_image", "default")[0]
        self.vae = nodes.VAELoader().load_vae("qwen_image_2.1_vae_bf16.safetensors")[0]
        self.runtime = default_runtime()
        self.runtime_key = runtime_digest(self.runtime)

    def configure_runtime(self, runtime):
        """Only EngineService's exclusively owned job may replace this patcher."""
        runtime = validate_runtime(runtime)
        digest = runtime_digest(runtime)
        if digest == self.runtime_key:
            return
        spec = runtime["image"]["lora"]
        if spec == self.runtime["image"]["lora"]:
            self.runtime, self.runtime_key = runtime, digest
            return
        model = self.model
        if spec is not None:
            path = self.model_root / "loras" / spec["name"]
            if path.is_symlink() or not path.is_file():
                raise ValueError("configured adapter is not installed")
            with path.open("rb") as stream:
                if hashlib.file_digest(stream, "sha256").hexdigest() != spec["sha256"]:
                    raise ValueError("configured adapter hash differs")
            if spec["strength"] != 0:
                try:
                    model = self.nodes.LoraLoaderModelOnly().load_lora_model_only(self.model, spec["name"], spec["strength"])[0]
                except Exception:
                    raise ValueError("configured adapter could not be loaded") from None
            if self.model.patches or spec["strength"] != 0 and len(model.patches) != spec["patches"]:
                raise ValueError("configured adapter does not match the model")
        # Commit only after validation. Base weights and the GPU process survive.
        self.uncensored_model, self.runtime, self.runtime_key = model, runtime, digest

    def generate(self, value, report):
        # Library prints must not put prompt fragments in supervisor diagnostics.
        with open(os.devnull, "w") as sink, contextlib.redirect_stdout(sink), contextlib.redirect_stderr(sink):
            return self._generate(value, report)

    def _generate(self, value, report):
        import numpy as np
        from PIL import Image
        images = {}
        for i, encoded in enumerate(value["references"], 1):
            raw = base64.b64decode(encoded, validate=True)
            if len(raw) > 5 * 1024 * 1024:
                raise ValueError("reference byte limit")
            with Image.open(io.BytesIO(raw)) as image:
                if image.format != "PNG" or getattr(image, "n_frames", 1) != 1 or image.width * image.height > 1024 ** 2:
                    raise ValueError("normalized PNG reference required")
                ratio = image.width / image.height
                w = max(32, round(math.sqrt(1024 ** 2 * ratio) / 32) * 32)
                h = max(32, round(math.sqrt(1024 ** 2 / ratio) / 32) * 32)
                if max(w, h) > 8192 or w * h > 1152 ** 2:
                    raise ValueError("reference conditioning geometry limit")
                images[f"image_{i}"] = self.torch.from_numpy(np.array(image.convert("RGBA"), dtype=np.float32) / 255.0).unsqueeze(0)
        with self.torch.inference_mode():
            return self._sample(value, images, report)

    def _sample(self, value, images, report):
        import numpy as np
        from PIL import Image
        # Each job selects an immutable patcher. Never append adapters to a
        # previously patched model or carry the previous request's mode forward.
        selected = self.uncensored_model if value.get("uncensored", False) else self.model
        model = self.qwen.QwenImage21Cache.execute(selected, "auto", "default")[0]
        clip = self.clip.clone()
        positive, negative, _ = self.qwen.TextEncodeQwenImage21.execute(
            clip, value["prompt"], "", vae=self.vae, resolution=1024, images=images).result
        settings = self.runtime["image"]
        guider = self.sampling.CFGGuider.execute(model, positive, negative, settings["cfg"])[0]
        noise = self.sampling.RandomNoise.execute(value["seed"])[0]
        sampler = self.sampling.KSamplerSelect.execute(settings["sampler"])[0]
        sigmas = self.sampling.ManualSigmas.execute(",".join(str(s) for s in settings["sigmas"]))[0]
        steps = len(settings["sigmas"]) - 1
        latent = self.nodes.EmptyLatentImage().generate(value["width"], value["height"], 1)[0]
        def progress(current, total, preview=None, **kwargs):
            if type(current) is not int or type(total) is not int or total != steps or not 0 <= current <= total:
                raise RuntimeError("unexpected sampler counters")
            report({"kind": "progress", "value": current, "max": total})
        self.utils.set_progress_bar_global_hook(progress)
        try:
            sampled = self.sampling.SamplerCustomAdvanced.execute(noise, guider, sampler, sigmas, latent)[0]
        finally:
            self.utils.set_progress_bar_global_hook(None)
        pixels = self.nodes.VAEDecode().decode(self.vae, sampled)[0]
        if pixels.shape[0] != 1:
            raise RuntimeError("unexpected output batch")
        array = np.clip(255.0 * pixels[0].cpu().numpy(), 0, 255).astype(np.uint8)
        output = io.BytesIO()
        Image.fromarray(array).save(output, format="PNG", compress_level=4)
        if output.tell() > MAX_PNG:
            raise RuntimeError("output image byte limit")
        return output.getvalue()

    def cleanup(self):
        self.utils.set_progress_bar_global_hook(None)
        self.model.model.current_patcher = None  # Clears Qwen's per-run prefix cache.
        stage = self.clip.cond_stage_model
        for module in stage.modules():
            if hasattr(module, "image_spans"):
                module.image_spans = []
        gc.collect()
        self.management.cleanup_models()
        self.torch.cuda.synchronize()
        self.torch.cuda.empty_cache()


class EngineService:
    def __init__(self, recipe, token):
        if not re.fullmatch(r"[0-9a-f]{64}", token):
            raise ValueError("private engine token required")
        self.recipe, self.token = recipe, token
        self.active, self.healthy = False, True

    async def generate(self, request):
        from aiohttp import web
        if not hmac.compare_digest(request.headers.get("Authorization", ""), "Bearer " + self.token):
            raise web.HTTPForbidden()
        if self.active or not self.healthy:
            raise web.HTTPServiceUnavailable()
        self.active = True  # Claim before awaiting body reads, including concurrent submissions.
        try:
            value = engine_input(await request.json())
            if hasattr(self.recipe, "configure_runtime"):
                configure = asyncio.create_task(asyncio.to_thread(self.recipe.configure_runtime, value.get("runtime", default_runtime())))
                try:
                    await asyncio.shield(configure)
                except BaseException:
                    # Cancellation cannot transfer ownership while a CPU loader
                    # is still constructing or committing this job's patcher.
                    while not configure.done():
                        try:
                            await asyncio.shield(configure)
                        except asyncio.CancelledError:
                            continue
                        except BaseException:
                            break
                    raise
        except (ValueError, TypeError, KeyError):
            self.active = False
            raise web.HTTPBadRequest() from None
        except BaseException:
            self.active = False
            raise
        queue = asyncio.Queue(maxsize=16)
        loop = asyncio.get_running_loop()
        def report(event):
            loop.call_soon_threadsafe(queue.put_nowait, event)
        def execute():
            try:
                return self.recipe.generate(value, report)
            finally:
                self.recipe.cleanup()
        task = asyncio.create_task(asyncio.to_thread(execute))
        response = web.StreamResponse(headers={"Content-Type": "application/x-ndjson", "Cache-Control": "no-store"})
        async def send(event):
            event = {"job_id": value["job_id"], "request_sha256": value["request_sha256"], **event}
            await response.write(json.dumps(event, separators=(",", ":")).encode() + b"\n")
        try:
            await response.prepare(request)
            while not task.done() or not queue.empty():
                try:
                    event = await asyncio.wait_for(queue.get(), .25)
                except TimeoutError:
                    continue
                await send(event)
            raw = await asyncio.shield(task)  # Cleanup has completed before result delivery.
            await send({"kind": "result", "image_base64": base64.b64encode(raw).decode(),
                        "image_sha256": hashlib.sha256(raw).hexdigest(),
                        **selected_image_policy(value.get("uncensored", False), value.get("runtime")),
                        **({"runtime_policy_sha256": value["runtime_policy_sha256"]} if "runtime" in value else {})})
            await response.write_eof()
        except (ConnectionError, asyncio.CancelledError):
            raise
        except Exception:
            self.healthy = False
            # Do not expose exception text or a traceback containing request data.
            try:
                await send({"kind": "error"})
            except ConnectionError:
                pass
        finally:
            # Reader cancellation never transfers the running GPU to another job.
            try:
                await asyncio.shield(task)
            except BaseException:
                self.healthy = False
            self.active = False
            value.clear()
        return response

    def application(self):
        from aiohttp import web
        app = web.Application(client_max_size=MAX_REQUEST)
        async def health(request):
            runtime = getattr(self.recipe, "runtime", default_runtime())
            return web.json_response({"engine": "comfyui", "ready": self.healthy,
                "references": runtime["image"]["max_references"],
                "uncensored_lora": selected_image_policy(True, runtime)["lora"],
                "runtime_policy_sha256": runtime_digest(runtime)}, status=200 if self.healthy else 503)
        app.router.add_get("/axiom/v1/health", health)
        app.router.add_post("/axiom/v1/generate", self.generate)
        return app


def main():
    import logging
    import resource
    from aiohttp import web
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
    logging.disable(logging.CRITICAL)
    service = EngineService(ComfyRecipe(), os.environ["AXIOM_IMAGE_ENGINE_TOKEN"])
    web.run_app(service.application(), host="127.0.0.1", port=int(os.environ.get("AXIOM_IMAGE_PORT", "8080")), access_log=None, print=None, handler_cancellation=False)


if __name__ == "__main__":
    main()
