"""Fixed staging workloads. No caller-selected prompt, model, URL, or grant.

Only the separately named private preview definition exposes these functions.
Production ingress and its admission verifier do not use this module.
"""

import asyncio
import base64
import hashlib
import importlib.util
import importlib.machinery
import os
import re
import struct
import time

import httpx

from .client import read_limited
from .wire import canonical, decode_json
from .performance import performance_generate


async def health():
    endpoints = {"llm": "http://127.0.0.1:10101/health",
                 "image": "http://127.0.0.1:8080/axiom/v1/health",
                 "bootstrap": "http://127.0.0.1:8006/healthz"}
    checks = {}
    async with httpx.AsyncClient(trust_env=False, follow_redirects=False, timeout=3) as http:
        for name, url in endpoints.items():
            try:
                checks[name] = (await http.get(url)).status_code == 200
            except httpx.HTTPError:
                checks[name] = False
    return {"ready": False, "staging_ready": all(checks.values()), "components": checks,
            "image_edit_codec_available": (importlib.util.find_spec("PIL") is not None
                or importlib.machinery.PathFinder.find_spec("PIL", ["/opt/axiom/vllm/lib/python3.12/site-packages"]) is not None),
            "provider": "chutes", "bootstrap_only": True, "pool_admission": False,
            "workload_binding_qualified": False}


async def self_test(value, *, http=None):
    if os.environ.get("AXIOM_CHUTES_BOOTSTRAP_ONLY") != "1":
        raise ValueError("fixed self tests are only available in the staging image")
    if (not isinstance(value, dict) or set(value) != {"challenge", "workload"}
            or not re.fullmatch(r"[0-9a-f]{64}", value.get("challenge", ""))
            or value.get("workload") not in ("llm", "image", "image-fresh", "benchmark")):
        raise ValueError("a fixed staging workload and diagnostic challenge are required")
    owns = http is None
    http = http or httpx.AsyncClient(trust_env=False, follow_redirects=False,
                                    timeout=httpx.Timeout(600, connect=5))
    started = time.monotonic()
    result = {**value, "pool_admission": False, "synthetic_fixed_workload": True}
    request_id = "axiom-selftest-" + value["challenge"]

    async def get_json(method, url, **kwargs):
        async with http.stream(method, url, **kwargs) as response:
            allowed = (200, 202) if method == "POST" else (200,)
            if response.status_code not in allowed:
                raise RuntimeError("private staging engine unavailable")
            return decode_json(await read_limited(response, 8 * 1024 * 1024))

    try:
        if value["workload"] == "benchmark":
            result.update(await performance_generate(value["challenge"], longer=True, http=http))
        elif value["workload"] == "llm":
            payload = {"model": "Qwen3.8-27B-Uncensored", "temperature": 0,
                "max_tokens": 1024, "reasoning_effort": "medium", "stream": False,
                "messages": [{"role": "user", "content": "Write a Python function add(a, b) that returns their sum. Include one assert example. Return the code."}]}
            async with http.stream("POST", "http://127.0.0.1:10101/v1/chat/completions",
                    headers={"X-Request-Id": request_id}, json=payload) as response:
                if response.status_code != 200 or response.headers.get("x-request-id") != request_id:
                    raise RuntimeError("staging engine request identity mismatch")
                answer = decode_json(await read_limited(response, 1024 * 1024))
            result.update(request_id=request_id, message=answer["choices"][0]["message"],
                          usage=answer.get("usage"), finish_reason=answer["choices"][0].get("finish_reason"))
        else:
            prompt = ("A studio photograph of one glossy cobalt blue ceramic coffee mug on a light oak table, soft daylight, realistic materials, no text."
                if value["workload"] == "image" else
                "A realistic studio photograph of one red glass vase holding three white tulips on a dark walnut table, soft daylight, no text.")
            from .image_generation import comfy_generate, ImageJobFence
            encoded, width, height = await comfy_generate({"request_id": value["challenge"], "prompt": prompt,
                "seed": 42, "references": [], "width": 1024, "height": 1024}, ImageJobFence(), http=http)
            raw = base64.b64decode(encoded, validate=True)
            result.update(image_base64=encoded, image_sha256=hashlib.sha256(raw).hexdigest(), image_bytes=len(raw),
                          prompt_sha256=hashlib.sha256(prompt.encode()).hexdigest(), resolution=[width, height], steps=8)
        result["seconds"] = time.monotonic() - started
        return result
    finally:
        if owns:
            await http.aclose()


async def preview_desktop_chat(value, *, http=None):
    """Bounded text-only interactive preview, authenticated as one whole reply.

    This is deliberately separate from the qualified Rust pool endpoint.
    It requires private Chutes E2EE ingress and is labeled development-only.
    """
    import hmac
    if os.environ.get("AXIOM_CHUTES_BOOTSTRAP_ONLY") != "1":
        raise ValueError("desktop preview requires the private staging image")
    if (not isinstance(value, dict)
            or set(value) != {"request_id", "confirmation_secret", "messages", "max_tokens", "reasoning_effort"}
            or any(not isinstance(value[k], str) or not re.fullmatch(r"[0-9a-f]{64}", value[k])
                   for k in ("request_id", "confirmation_secret"))
            or type(value["max_tokens"]) is not int or not 1 <= value["max_tokens"] <= 4096
            or value["reasoning_effort"] != "medium"
            or not isinstance(value["messages"], list) or not 1 <= len(value["messages"]) <= 128
            or len(canonical(value["messages"])) > 1048576):
        raise ValueError("invalid desktop preview input")
    for message in value["messages"]:
        if (not isinstance(message, dict) or set(message) != {"role", "content"}
                or message["role"] not in ("system", "user", "assistant")
                or not isinstance(message["content"], str)):
            raise ValueError("desktop preview accepts text messages only")
    request = {k: value[k] for k in ("request_id", "messages", "max_tokens", "reasoning_effort")}
    request_hash = hashlib.sha256(canonical(request)).hexdigest()
    request_id = "axiom-desktop-" + value["request_id"]
    owns = http is None
    http = http or httpx.AsyncClient(trust_env=False, follow_redirects=False,
                                    timeout=httpx.Timeout(600, connect=5))
    started = time.monotonic()
    try:
        payload = {"model": "Qwen3.8-27B-Uncensored", "stream": False,
                   "messages": value["messages"], "max_tokens": value["max_tokens"],
                   "reasoning_effort": "medium", "temperature": 0.6}
        async with http.stream("POST", "http://127.0.0.1:10101/v1/chat/completions",
                               json=payload, headers={"X-Request-Id": request_id}) as response:
            if response.status_code != 200 or response.headers.get("x-request-id") != request_id:
                raise RuntimeError("desktop engine request identity mismatch")
            answer = decode_json(await read_limited(response, 2 * 1024 * 1024))
        choice = answer["choices"][0]
        if choice.get("finish_reason") not in ("stop", "length"):
            raise ValueError("incomplete desktop generation")
        result = {"request_id": value["request_id"], "request_sha256": request_hash,
                  "message": choice["message"], "usage": answer["usage"],
                  "finish_reason": choice["finish_reason"], "seconds": time.monotonic() - started,
                  "development_preview": True, "workload_binding_verified": False}
        tag = hmac.new(bytes.fromhex(value["confirmation_secret"]),
                       b"axiom-chutes-desktop-v1\0" + canonical(result), hashlib.sha256).hexdigest()
        return {"result": result, "confirmation": tag}
    finally:
        if owns:
            await http.aclose()
