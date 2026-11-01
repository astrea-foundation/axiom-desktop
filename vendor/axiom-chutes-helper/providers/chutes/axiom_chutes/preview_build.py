"""Deterministic public Chutes source rendering, shared by CI and deployment."""
import base64
import json
from pathlib import Path
import re


def render_preview_source(root, username, tag, worker, performance_profile, *, reference_only=True,
                          deployment_name="axiom-preview-a", idle_shutdown_seconds=7200):
    if (not isinstance(deployment_name, str)
            or not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_-]{0,63}", deployment_name)):
        raise ValueError("a bounded deployment name is required")
    if type(idle_shutdown_seconds) is not int or not 60 <= idle_shutdown_seconds <= 604800:
        raise ValueError("provider idle timeout must be 60..604800 seconds")
    root = Path(root)
    source = (root / "deploy/chutes/worker_preview.py").read_text()
    if deployment_name != "axiom-preview-a":
        marker = 'name="axiom-preview-a", image=image'
        if source.count(marker) != 1:
            raise ValueError("unique deployment name marker required")
        source = source.replace(marker, "name=" + json.dumps(deployment_name) + ", image=image")
    if idle_shutdown_seconds != 7200:
        marker = "shutdown_after_seconds=7200"
        if source.count(marker) != 1:
            raise ValueError("unique provider idle timeout marker required")
        source = source.replace(marker, "shutdown_after_seconds=" + str(idle_shutdown_seconds))
    for marker, value in [("__AXIOM_USERNAME__", username), ("__AXIOM_IMAGE_TAG__", tag),
        ("__AXIOM_IMAGE_REFERENCE_ONLY__", reference_only),
        ("__AXIOM_LLM_PROFILE__", performance_profile),
        ("__AXIOM_WORKER_CONFIG_B64__", base64.b64encode(json.dumps(worker, sort_keys=True, separators=(",", ":")).encode()).decode())]:
        source = source.replace(json.dumps(marker), repr(value) if isinstance(value, bool) else json.dumps(value))
    source = source.replace("# __AXIOM_INLINE_PERFORMANCE__",
                            (root / "providers/chutes/axiom_chutes/performance.py").read_text())
    runtime_source = (root / "providers/chutes/axiom_chutes/preview_runtime.py").read_text()
    runtime_source = runtime_source.replace("from axiom_chutes.performance import performance_command, performance_generate, performance_image_command, performance_vision_warmup", "")
    source = source.replace("# __AXIOM_INLINE_PREVIEW_RUNTIME__", runtime_source)
    preview_api = (root / "providers/chutes/axiom_chutes/preview.py").read_text()
    preview_api = preview_api.replace("from .client import", "from axiom_chutes.client import")
    preview_api = preview_api.replace("from .wire import", "from axiom_chutes.wire import")
    preview_api = preview_api.replace("from .image_generation import comfy_generate, ImageJobFence", "from axiom_chutes.image_generation import comfy_generate, ImageJobFence")
    preview_api = preview_api.replace("from .performance import performance_generate", "")
    preview_api = preview_api.replace("async def health(", "async def preview_health(")
    preview_api = preview_api.replace("async def self_test(", "async def preview_self_test(")
    source = source.replace("# __AXIOM_INLINE_PREVIEW_API__", preview_api)
    desktop_stream = (root / "providers/chutes/axiom_chutes/desktop_stream.py").read_text()
    desktop_stream = desktop_stream.replace("from .wire import", "from axiom_chutes.wire import")
    desktop_stream = desktop_stream.replace("from .metering import billing_context", "")
    desktop_stream = desktop_stream.replace("from .image_generation import image_source", "")
    metering = (root / "providers/chutes/axiom_chutes/metering.py").read_text()
    metering = metering.replace("from .wire import", "from axiom_chutes.wire import")
    source = source.replace("# __AXIOM_INLINE_METERING__", metering)
    source = source.replace("# __AXIOM_INLINE_DESKTOP_STREAM__", desktop_stream)
    image_generation = (root / "providers/chutes/axiom_chutes/image_generation.py").read_text()
    image_generation = image_generation.replace("from .client import", "from axiom_chutes.client import")
    image_generation = image_generation.replace("from .wire import", "from axiom_chutes.wire import")
    image_generation = image_generation.replace("from .image_policy import", "from axiom_chutes.image_policy import")
    image_generation = image_generation.replace("from .runtime_policy import", "from axiom_chutes.runtime_policy import")
    image_generation = image_generation.replace("from .metering import IMAGE_EDIT_MODEL, IMAGE_MODEL, IMAGE_PIXELS, IMAGE_STEPS, billing_context", "")
    source = source.replace("# __AXIOM_INLINE_IMAGE_GENERATION__", image_generation)
    image_stream = (root / "providers/chutes/axiom_chutes/image_stream.py").read_text()
    image_stream = image_stream.replace("from .wire import", "from axiom_chutes.wire import")
    image_stream = image_stream.replace("from .image_generation import generate_image, image_input, MAX_IMAGE_JSON, ImageInputRejected", "")
    source = source.replace("# __AXIOM_INLINE_IMAGE_STREAM__", image_stream)
    if "__AXIOM_" in source:
        raise ValueError("unresolved release source placeholder")
    compile(source, "worker_preview.py", "exec")
    return source
