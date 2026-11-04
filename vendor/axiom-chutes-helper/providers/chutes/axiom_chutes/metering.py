"""Metadata-only, independently signed usage for the managed Chutes channel.

The public signing key is obtained through a fresh attested SDK exchange. This
is a channel endorsement under the Chutes runtime trust model, not an image
measurement. A client-known stream MAC is never accepted as billing evidence.
"""
import hashlib
import hmac
import os
from pathlib import Path
import re
import threading

from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey, Ed25519PublicKey
from cryptography.hazmat.primitives.serialization import Encoding, PublicFormat

from .wire import canonical, decode_json

DOMAIN = b"axiom-chutes-usage-v1\0"
SESSION_DOMAIN = b"axiom-chutes-metering-session-v1\0"
HEX = re.compile(r"^[0-9a-f]{64}$")
FIELDS = {"reservation_id", "billing_nonce", "price_version", "cache_namespace"}
RATES = {"input_microusd_per_million", "cached_input_microusd_per_million",
         "output_microusd_per_million", "image_microusd_per_megapixel_step", "request_microusd"}
IMAGE_MODEL = "chutes:qwen-image-2.1-turbo"
IMAGE_EDIT_MODEL = "chutes:qwen-image-2.1-turbo-edit"
IMAGE_PIXELS, IMAGE_STEPS = 1536 * 1536, 8


def price_charge(price, counts=None, *, operation="chat", image_steps=IMAGE_STEPS):
    if (not isinstance(price, dict) or set(price) != RATES | {"version", "currency"}
            or price["currency"] != "USD" or not isinstance(price["version"], str)
            or not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]{0,63}", price["version"])
            or any(type(price[k]) is not int or not 0 <= price[k] <= 9007199254740991 for k in RATES)):
        raise ValueError("invalid price snapshot")
    def scaled(value):
        return (value + 999999) // 1000000
    if operation in {"image", "image_edit"}:
        pixels = counts["image_pixels"] if counts is not None else IMAGE_PIXELS
        steps = counts["image_steps"] if counts is not None else image_steps
        if (type(pixels) is not int or not 0 < pixels <= IMAGE_PIXELS
                or type(steps) is not int or not 1 <= steps <= 64):
            raise ValueError("image usage exceeds reservation")
        cost = {"input_microusd": 0, "output_microusd": 0,
                "image_microusd": scaled(pixels * steps * price["image_microusd_per_megapixel_step"]),
                "request_microusd": price["request_microusd"]}
        total = sum(cost.values())
        if total > 9007199254740991:
            raise ValueError("maximum image price exceeds integer bound")
        return total if counts is None else {**cost, "total_microusd": total}
    if operation != "chat":
        raise ValueError("unsupported price operation")
    if counts is None:
        total = (scaled(32768 * max(price["input_microusd_per_million"], price["cached_input_microusd_per_million"]))
                 + scaled(4096 * price["output_microusd_per_million"]) + price["request_microusd"])
        if total > 9007199254740991:
            raise ValueError("maximum price exceeds integer bound")
        return total
    result = {"input_microusd": scaled((counts["input_tokens"] - counts["cached_input_tokens"])
                    * price["input_microusd_per_million"] + counts["cached_input_tokens"]
                    * price["cached_input_microusd_per_million"]),
              "output_microusd": scaled(counts["output_tokens"] * price["output_microusd_per_million"]),
              "image_microusd": 0, "request_microusd": price["request_microusd"]}
    return {**result, "total_microusd": sum(result.values())}


def billing_context(value):
    if (not isinstance(value, dict) or set(value) != FIELDS
            or any(not isinstance(value[k], str) or not HEX.fullmatch(value[k])
                   for k in ("reservation_id", "billing_nonce", "cache_namespace"))
            or not isinstance(value["price_version"], str)
            or not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]{0,63}", value["price_version"])):
        raise ValueError("invalid billing context")
    return value


def verify_session(value, secret, challenge):
    if not isinstance(value, dict) or set(value) != {"session", "confirmation"}:
        raise ValueError("invalid metering session")
    session = value["session"]
    expected = hmac.new(secret, SESSION_DOMAIN + canonical(session), hashlib.sha256).hexdigest()
    if (not isinstance(value["confirmation"], str)
            or not hmac.compare_digest(expected, value["confirmation"])
            or not isinstance(session, dict)
            or set(session) != {"challenge", "boot_id", "signing_key"}
            or session["challenge"] != challenge
            or any(not isinstance(session[k], str) or not HEX.fullmatch(session[k])
                   for k in ("boot_id", "signing_key"))):
        raise ValueError("metering session authentication failed")
    return session


def verify_receipt(document, signing_key, boot_id, expected):
    if not isinstance(document, dict) or set(document) != {"payload", "signature"}:
        raise ValueError("signed usage required")
    payload = document["payload"]
    if (not isinstance(payload, dict) or set(payload) != FIELDS | {
            "schema", "boot_id", "request_id", "model", "input_tokens", "output_tokens",
            "cached_input_tokens", "reasoning_tokens"}
            or payload["schema"] != "axiom-chutes-usage-v1" or payload["boot_id"] != boot_id
            or payload["model"] != "Qwen3.8-27B-Uncensored"
            or not isinstance(payload["request_id"], str) or not HEX.fullmatch(payload["request_id"])
            or any(payload.get(k) != v for k, v in expected.items())
            or any(type(payload[k]) is not int or not 0 <= payload[k] <= 65536
                   for k in ("input_tokens", "output_tokens", "cached_input_tokens", "reasoning_tokens"))
            or payload["cached_input_tokens"] > payload["input_tokens"]
            or payload["reasoning_tokens"] > payload["output_tokens"]):
        raise ValueError("usage identity or counts differ")
    billing_context({k: payload[k] for k in FIELDS})
    signature = document["signature"]
    if not isinstance(signature, str) or not re.fullmatch(r"[0-9a-f]{128}", signature):
        raise ValueError("usage signature required")
    Ed25519PublicKey.from_public_bytes(bytes.fromhex(signing_key)).verify(
        bytes.fromhex(signature), DOMAIN + canonical(payload))
    return payload


def verify_image_receipt(document, signing_key, boot_id, expected):
    if not isinstance(document, dict) or set(document) != {"payload", "signature"}:
        raise ValueError("signed image usage required")
    p = document["payload"]
    edit = isinstance(p, dict) and p.get("model") == IMAGE_EDIT_MODEL
    fields = {"source_image_pixels"} if edit else set()
    if (not isinstance(p, dict) or set(p) - {"runtime_policy_sha256"} != FIELDS | {"schema", "boot_id", "request_id", "model", "image_pixels", "image_steps"} | fields
            or p["schema"] != ("axiom-chutes-image-edit-usage-v1" if edit else "axiom-chutes-image-usage-v1")
            or p["boot_id"] != boot_id or p["model"] != (IMAGE_EDIT_MODEL if edit else IMAGE_MODEL)
            or not isinstance(p["request_id"], str) or not HEX.fullmatch(p["request_id"])
            or any(p.get(k) != v for k, v in expected.items())
            or type(p["image_pixels"]) is not int or not 0 < p["image_pixels"] <= IMAGE_PIXELS
            or type(p["image_steps"]) is not int or not 1 <= p["image_steps"] <= 64
            or "runtime_policy_sha256" not in p and p["image_steps"] != IMAGE_STEPS
            or "runtime_policy_sha256" in p and (not isinstance(p["runtime_policy_sha256"], str) or not HEX.fullmatch(p["runtime_policy_sha256"]))
            or edit and (type(p["source_image_pixels"]) is not int or not 0 < p["source_image_pixels"] <= 16 * 1024 * 1024)):
        raise ValueError("image usage identity or counts differ")
    billing_context({k: p[k] for k in FIELDS})
    if not isinstance(document["signature"], str) or not re.fullmatch(r"[0-9a-f]{128}", document["signature"]):
        raise ValueError("image usage signature required")
    Ed25519PublicKey.from_public_bytes(bytes.fromhex(signing_key)).verify(
        bytes.fromhex(document["signature"]), DOMAIN + canonical(p))
    return p


class MeteringIdentity:
    def __init__(self, directory):
        self.key = Ed25519PrivateKey.generate()
        self.boot_id = os.urandom(32).hex()
        self.signing_key = self.key.public_key().public_bytes(Encoding.Raw, PublicFormat.Raw).hex()
        self.directory = Path(directory)
        self.directory.mkdir(mode=0o700, parents=True, exist_ok=True)
        if self.directory.is_symlink() or self.directory.stat().st_mode & 0o077:
            raise ValueError("private usage outbox required")
        self.lock = threading.Lock()
        self.attempted = set()

    async def admit(self, request, operation, origin, *, image_steps=IMAGE_STEPS, http=None):
        """Check the dispatched hold with its fixed controller before model use.

        Only operational billing metadata leaves the TEE. HTTPS authenticates
        the configured account service; the boot key authenticates this probe.
        Generation ciphertext and plaintext never reach this endpoint.
        """
        if origin not in {"https://api.axiom.stream", "https://api-staging.axiom.stream"}:
            raise ValueError("installed account controller required")
        context = billing_context(request.get("billing"))
        if (operation not in {"chat", "image", "image_edit"}
                or not isinstance(request.get("request_id"), str)
                or not HEX.fullmatch(request["request_id"])):
            raise ValueError("bounded admission identity required")
        payload = {**context, "schema": "axiom-chutes-admission-v1", "boot_id": self.boot_id,
            "request_id": request["request_id"], "operation": operation,
            "challenge": os.urandom(32).hex(),
            "runtime_policy_sha256": request.get("runtime_policy_sha256"),
            "image_steps": image_steps if operation != "chat" else None}
        document = {"payload": payload,
            "signature": self.key.sign(b"axiom-chutes-admission-v1\0" + canonical(payload)).hex()}
        import httpx
        owned = http is None
        http = http or httpx.AsyncClient(trust_env=False, follow_redirects=False, timeout=15)
        try:
            response = await http.post(origin + "/api/v1/chutes/admissions", json=document)
            if (response.status_code != 200
                    or decode_json(response.content, 1024) != {"admitted": True, "challenge": payload["challenge"]}):
                raise ValueError("dispatched account hold not confirmed")
        finally:
            if owned:
                await http.aclose()

    def begin(self, context):
        billing_context(context)
        with self.lock:
            identity = context["reservation_id"]
            if (identity in self.attempted or len(self.attempted) >= 100000
                    or len(list(self.directory.glob("*.json"))) >= 1024):
                raise ValueError("usage reservation replayed or signer capacity exhausted")
            self.attempted.add(identity)

    def session(self, challenge, secret):
        if not isinstance(challenge, str) or not HEX.fullmatch(challenge) or len(secret) != 32:
            raise ValueError("fresh metering challenge required")
        value = {"challenge": challenge, "boot_id": self.boot_id, "signing_key": self.signing_key}
        return {"session": value, "confirmation": hmac.new(
            secret, SESSION_DOMAIN + canonical(value), hashlib.sha256).hexdigest()}

    def finish(self, context, request_id, usage):
        billing_context(context)
        payload = {**context, "schema": "axiom-chutes-usage-v1", "boot_id": self.boot_id,
            "request_id": request_id, "model": "Qwen3.8-27B-Uncensored",
            "input_tokens": usage["prompt_tokens"], "output_tokens": usage["completion_tokens"],
            "cached_input_tokens": usage.get("cached_input_tokens", 0),
            "reasoning_tokens": usage.get("reasoning_tokens", 0)}
        document = {"payload": payload, "signature": self.key.sign(DOMAIN + canonical(payload)).hex()}
        verify_receipt(document, self.signing_key, self.boot_id, context)
        return self._record(document, context)

    def finish_image(self, context, request_id, *, pixels=1024 * 1024, source_pixels=0, steps=IMAGE_STEPS, runtime_policy_sha256=None):
        billing_context(context)
        payload = {**context, "schema": "axiom-chutes-image-edit-usage-v1" if source_pixels else "axiom-chutes-image-usage-v1", "boot_id": self.boot_id,
                   "request_id": request_id, "model": IMAGE_EDIT_MODEL if source_pixels else IMAGE_MODEL,
                   "image_pixels": pixels, "image_steps": steps,
                   **({"runtime_policy_sha256": runtime_policy_sha256} if runtime_policy_sha256 is not None else {}),
                   **({"source_image_pixels": source_pixels} if source_pixels else {})}
        document = {"payload": payload, "signature": self.key.sign(DOMAIN + canonical(payload)).hex()}
        verify_image_receipt(document, self.signing_key, self.boot_id, context)
        return self._record(document, context)

    def _record(self, document, context):
        raw = canonical(document)
        with self.lock:
            if len(list(self.directory.glob("*.json"))) >= 1024:
                raise RuntimeError("usage outbox full")
            path = self.directory / (context["reservation_id"] + ".json")
            descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
            with os.fdopen(descriptor, "wb") as out:
                out.write(raw)
                out.flush()
                os.fsync(out.fileno())
            directory_fd = os.open(self.directory, os.O_RDONLY)
            try:
                os.fsync(directory_fd)
            finally:
                os.close(directory_fd)
        return document

    async def deliver(self, origin):
        """Retry metadata only; delivery never reruns generation."""
        # The retained bootstrap image's historical controller was the Chutes
        # API. Never send account metering there, or to a caller-selected host.
        # Local staging uses the native client's independent delivery path.
        if origin not in {"https://api.axiom.stream", "https://api-staging.axiom.stream"}:
            return
        import asyncio
        import httpx
        async with httpx.AsyncClient(trust_env=False, follow_redirects=False, timeout=15) as http:
            while True:
                for path in sorted(self.directory.glob("*.json")):
                    try:
                        value = decode_json(path.read_bytes(), 16 * 1024)
                        response = await http.post(origin + "/api/v1/chutes/receipts", json=value)
                        if response.status_code == 200 and response.json().get("settled") is True:
                            path.unlink()
                    except Exception:
                        pass
                await asyncio.sleep(15)
