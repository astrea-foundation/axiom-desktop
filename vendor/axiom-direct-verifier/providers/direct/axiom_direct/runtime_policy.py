"""Signed, data-only backend settings within the installed client's resource limits."""
import copy
import hashlib
import math
import re

from .wire import canonical

CLIENT_PROTOCOL = 1
MAX_PIXELS = 1536 ** 2
MAX_REFERENCES = 5
SIGMAS = [1, .978453, .95418, .926626, .89508, .845148, .704534, .414568, 0]


class RuntimeUpdateRequired(ValueError):
    """Named native error; no provider exception text needs to cross the pipe."""


def default_runtime():
    return {"protocol": 1, "minimum_client": 1, "required_features": [],
        "image": {"default_aspect_ratio": "2:3",
            "aspect_ratios": ["1:1", "2:3", "3:2", "3:4", "4:3", "9:16", "16:9", "1:2", "2:1", "9:21", "21:9"],
            "max_pixels": MAX_PIXELS, "max_references": MAX_REFERENCES,
            "sampler": "euler", "cfg": 1, "sigmas": SIGMAS.copy(),
            "uncensored_models": ["direct:qwen3.8-27b", "direct-preview:qwen3.8-27b"],
            "lora": {"name": "uncensored.safetensors",
                "sha256": "83aba822ad8ee10a1e836828f5088259d93bce7b5f3419b46dbc0258c2be46a9",
                "strength": .9, "patches": 192}}}


def _require(condition, message="unsupported runtime policy; client update required"):
    if not condition:
        raise ValueError(message)


def aspect_dimensions(ratio, max_pixels=MAX_PIXELS):
    _require(isinstance(ratio, str) and re.fullmatch(r"[1-9][0-9]?:[1-9][0-9]?", ratio), "invalid aspect ratio")
    w, h = map(int, ratio.split(":"))
    _require(1 / 4 <= w / h <= 4, "unsupported aspect ratio")
    divisor = math.gcd(w, h)
    w, h = w // divisor, h // divisor
    # Both dimensions are multiples of 32 and the ratio remains exact.
    scale = math.isqrt(max_pixels // (1024 * w * h))
    _require(scale > 0 and min(w, h) * scale * 32 >= 256, "aspect ratio does not fit resource limits")
    return w * scale * 32, h * scale * 32


def validate_runtime(value):
    _require(isinstance(value, dict) and set(value) - {"extensions"} == {"protocol", "minimum_client", "required_features", "image"})
    extensions = value.get("extensions", {})
    _require(isinstance(extensions, dict) and len(extensions) <= 32 and len(canonical(extensions)) <= 8192)
    if not (type(value["protocol"]) is int and value["protocol"] == CLIENT_PROTOCOL
        and type(value["minimum_client"]) is int and 1 <= value["minimum_client"] <= CLIENT_PROTOCOL
        and value["required_features"] == []):
        raise RuntimeUpdateRequired("unsupported runtime policy; client update required")
    image = value["image"]
    _require(isinstance(image, dict) and set(image) == {"default_aspect_ratio", "aspect_ratios", "max_pixels",
        "max_references", "sampler", "cfg", "sigmas", "uncensored_models", "lora"})
    _require(type(image["max_pixels"]) is int and 256 ** 2 <= image["max_pixels"] <= MAX_PIXELS
        and type(image["max_references"]) is int and 0 <= image["max_references"] <= MAX_REFERENCES)
    ratios = image["aspect_ratios"]
    _require(isinstance(ratios, list) and 1 <= len(ratios) <= 32 and all(isinstance(r, str) for r in ratios)
        and len(set(ratios)) == len(ratios) and image["default_aspect_ratio"] in ratios)
    for ratio in ratios:
        aspect_dimensions(ratio, image["max_pixels"])
    _require(isinstance(image["sampler"], str) and image["sampler"] in {"euler", "heun", "dpmpp_2m"}
        and type(image["cfg"]) in (int, float) and math.isfinite(image["cfg"]) and 0 <= image["cfg"] <= 20)
    sigmas = image["sigmas"]
    _require(isinstance(sigmas, list) and 2 <= len(sigmas) <= 65
        and all(type(s) in (int, float) and math.isfinite(s) and 0 <= s <= 1 for s in sigmas)
        and sigmas[0] == 1 and sigmas[-1] == 0 and all(a > b for a, b in zip(sigmas, sigmas[1:])))
    models = image["uncensored_models"]
    _require(isinstance(models, list) and len(models) <= 32 and all(isinstance(m, str)
        and re.fullmatch(r"[a-z0-9-]+:[A-Za-z0-9._-]{1,128}", m) for m in models) and len(set(models)) == len(models))
    lora = image["lora"]
    if lora is not None:
        _require(isinstance(lora, dict) and set(lora) == {"name", "sha256", "strength", "patches"}
            and isinstance(lora["name"], str) and re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]{0,110}\.safetensors", lora["name"])
            and isinstance(lora["sha256"], str) and re.fullmatch(r"[0-9a-f]{64}", lora["sha256"])
            and type(lora["strength"]) in (int, float) and math.isfinite(lora["strength"]) and 0 <= lora["strength"] <= 2
            and type(lora["patches"]) is int and 1 <= lora["patches"] <= 4096)
    return copy.deepcopy(value)


def runtime_digest(value):
    return hashlib.sha256(canonical(validate_runtime(value))).hexdigest()


def image_settings(value=None):
    return validate_runtime(value if value is not None else default_runtime())["image"]


def request_runtime(request):
    envelope = request.get("runtime_policy")
    if envelope is None:
        return None
    try:
        runtime = envelope["manifest"]["payload"]["runtime"]
        _require(request["runtime_policy_sha256"] == runtime_digest(runtime), "runtime policy binding differs")
        return validate_runtime(runtime)
    except (KeyError, TypeError) as error:
        raise ValueError("signed runtime policy required") from error


def selected_image_policy(uncensored=False, runtime=None):
    _require(type(uncensored) is bool, "native image model policy must be a boolean")
    spec = image_settings(runtime)["lora"] if uncensored else None
    return {"uncensored": uncensored, "lora": {"name": spec["name"], "sha256": spec["sha256"], "strength": float(spec["strength"])} if spec else None}


def runtime_binding(request):
    if "runtime_policy" not in request:
        return {}
    request_runtime(request)
    return {"runtime_policy_sha256": request["runtime_policy_sha256"]}


def verify_worker_runtime(request):
    """Verify once before admission; queued jobs retain their own immutable snapshot."""
    runtime = request_runtime(request)
    if runtime is None:
        return None
    import os
    from pathlib import Path
    from .publisher import PublisherAuthority, verify_metadata, verify_document, MANIFEST_DOMAIN
    from .release import RollbackState
    authority_file = os.environ.get("AXIOM_RUNTIME_AUTHORITY_FILE")
    state_file = os.environ.get("AXIOM_RUNTIME_STATE_FILE")
    _require(authority_file and state_file, "worker runtime authority is not installed")
    authority = PublisherAuthority.load(Path(authority_file))
    expected_chute = os.environ.get("AXIOM_RUNTIME_CHUTE_ID")
    if expected_chute:
        manifest, _ = verify_document(authority, request["runtime_policy"]["manifest"], MANIFEST_DOMAIN)
        _require(manifest.get("chute_id") == expected_chute, "runtime policy selects another worker deployment")
    release = verify_metadata(authority, canonical(request["runtime_policy"]), RollbackState(Path(state_file)))
    _require(release.runtime == runtime, "runtime release differs")
    return runtime
