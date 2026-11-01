#!/usr/bin/env python3
"""Native stdio bridge for an explicitly enabled local Chutes development test.

Messages and response secrets travel over anonymous native pipes and Chutes
ML-KEM E2EE, never the Axiom backend. Hardware evidence is checked locally on
every fresh lease. Observed runtime measurements are development pins, not
reviewed application provenance. No production VerifiedWorker is constructed.
"""
import argparse
import asyncio
import hashlib
import hmac
import json
import math
import os
from pathlib import Path
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "providers/chutes"))
sys.path.insert(0, str(ROOT / "scripts"))
from axiom_chutes.client import ChutesClient, read_limited
from axiom_chutes.wire import StreamCipher, canonical, decode_json, open_response, seal, unbase64
from axiom_chutes.desktop_stream import MAX_FRAME, StreamVerifier, sse_data, chat_messages, chat_tools, MAX_CHAT_BYTES
from chutes_native_install import bridge_config, hosted_release_config

MAX_PIPE = 40 * 1024 * 1024


class Bridge:
    def __init__(self, path):
        self.cfg = bridge_config(path)
        self.hosted = self.cfg.get("hosted_relay") is True
        if not self.hosted and (self.cfg.get("development_preview") is not True
                or self.cfg.get("local_api_origin") != "https://127.0.0.1:9443"):
            raise ValueError("explicit local development configuration required")
        self.client = None
        if not self.hosted:
            key_path = Path(self.cfg["key_file"])
            if key_path.stat().st_mode & 0o077:
                raise ValueError("private provider credential permissions required")
            self.client = ChutesClient(key_path.read_text().strip())
        elif "key_file" in self.cfg or not self.cfg.get("release_verification_config"):
            raise ValueError("hosted native configuration requires a release authority and no provider credential")
        self.hardware = None
        self.verified_at = None
        self.rows = []
        self.release = None
        self.release_checked_at = 0
        self.release_metadata = None
        self.metering = None
        self.maximum_charge = 0
        self.pending_billing = None

    def configure(self, value):
        from axiom_chutes.relay import RelayChutesClient
        if (not self.hosted or self.client is not None or set(value) != {
                "op", "relay_origin", "access_token", "maximum_charge_microusd"}
                or type(value["maximum_charge_microusd"]) is not int
                or not 0 <= value["maximum_charge_microusd"] <= 9007199254740991):
            raise ValueError("one native credential handoff required")
        self.client = RelayChutesClient(value["relay_origin"], value["access_token"],
                                       ca_file=self.cfg.get("tls_ca_file"))
        self.maximum_charge = value["maximum_charge_microusd"]

    def _development_heartbeat(self):
        # Hosted helpers are relocatable clients, including against a local
        # staging relay. Deployment state and its SDK belong to the operator.
        if getattr(self, "hosted", False):
            return
        from chutes_preview import update_state
        update_state(last_heartbeat_epoch=int(time.time()), testing_finished=False)

    async def refresh_release(self):
        if self.client is None:
            raise ValueError("native authentication required")
        release_config = self.cfg.get("release_verification_config")
        if release_config and (self.release is None or self.release.expires_at <= time.time() + 10
                or self.release_checked_at + 30 <= time.time()):
            if not isinstance(release_config, str) or not Path(release_config).is_absolute():
                raise ValueError("absolute local release verification config required")
            previous = self.release
            if self.hosted:
                from axiom_chutes.publisher import verify_metadata
                from axiom_chutes.release import RollbackState
                authority, state = hosted_release_config(Path(release_config))
                metadata = await self.client.api("GET", "/release")
                self.release = await asyncio.to_thread(verify_metadata, authority, canonical(metadata),
                                                      RollbackState(state))
                self.release_metadata = metadata
            else:
                from inference_release import verify_files
                self.release = await asyncio.to_thread(verify_files, Path(release_config))
            if getattr(self.release, "hardware", None):
                if not self.hosted and self.release.chute_id != self.cfg["chute_id"]:
                    raise ValueError("signed release selects another Chute")
                if previous is None or previous.manifest_sha256 != self.release.manifest_sha256:
                    self.hardware, self.rows = None, []
                    self.metering = None
            self.release_checked_at = time.time()

    async def capabilities(self):
        from axiom_chutes.runtime_policy import default_runtime, runtime_digest
        await self.refresh_release()
        if not self.hosted or self.release is None:
            raise ValueError("signed hosted capabilities required")
        runtime = getattr(self.release, "runtime", None)
        return {"runtime": runtime if runtime is not None else default_runtime(),
                "runtime_policy_sha256": runtime_digest(runtime) if runtime is not None else None,
                "release_authorization": self.release.public_report()}

    async def attest(self):
        await self.refresh_release()
        if self.hardware is None or self.hardware.expires_at <= time.time() + 10:
            self.rows = await self.client.discover(getattr(self.release, "chute_id", self.cfg.get("chute_id")))
            hardware_policy = getattr(self.release, "hardware", None)
            self.hardware = await self.client.attest_hardware(self.rows[0],
                hardware_policy["measurements"] if hardware_policy else self.cfg["development_measurements"],
                hardware_policy["nvidia_anchors"] if hardware_policy else self.cfg["nvidia_anchors"])
            self.verified_at = int(time.time())
            if self.hosted:
                self.metering = await self.client.metering_session(self.hardware)
                used = self.hardware.instance.invocation_nonce
                self.rows = [r for r in self.rows if r.invocation_nonce != used]
        instance = self.hardware.instance
        report = {"instance_id": instance.instance_id, "expires_at": self.hardware.expires_at,
                "verified_at": self.verified_at, "quote_sha256": self.hardware.quote_sha256,
                "key_sha256": hashlib.sha256(unbase64(instance.public_key)).hexdigest(),
                "hardware_verified": True, "workload_binding_verified": False}
        if self.release is not None:
            # Authorization is independently checked but does not establish
            # which application possesses this hardware-attested SDK key.
            report["release_authorization"] = self.release.public_report()
            report["expires_at"] = min(report["expires_at"], self.release.expires_at)
            if getattr(self.release, "runtime", None) is not None:
                from axiom_chutes.runtime_policy import runtime_digest
                report["runtime"] = self.release.runtime
                report["runtime_policy_sha256"] = runtime_digest(self.release.runtime)
        return report

    async def prepare_request(self, value, path, stream):
        if (not isinstance(value, dict)
                or not {"op", "messages", "max_tokens"} <= set(value)
                or set(value) - {"op", "messages", "max_tokens", "request_id", "tools"}
                or type(value["max_tokens"]) is not int or not 1 <= value["max_tokens"] <= 4096
                or not isinstance(value["messages"], list) or not 1 <= len(value["messages"]) <= 128
                or len(canonical(value["messages"])) > MAX_CHAT_BYTES):
            raise ValueError("bounded text conversation required")
        chat_messages(value["messages"])
        if "tools" in value:
            chat_tools(value["tools"])
        evidence = await self.attest()
        instance = self.hardware.instance
        self.rows = [r for r in self.rows if r.instance_id == instance.instance_id
                     and r.public_key == instance.public_key and r.nonce_expires_at > time.time() + 5]
        if not self.rows:
            self.rows = await self.client.discover(instance.chute_id)
            if any(r.public_key != instance.public_key for r in self.rows if r.instance_id == instance.instance_id):
                raise ValueError("recipient key changed")
            self.rows = [r for r in self.rows if r.instance_id == instance.instance_id
                         and r.public_key == instance.public_key]
        if not self.rows:
            raise ValueError("attested instance disappeared")
        row = self.rows.pop(0)  # Never replay an attempted nonce or request.
        secret = os.urandom(32)
        request = {"request_id": os.urandom(32).hex(), "messages": value["messages"],
                   "max_tokens": value["max_tokens"], "reasoning_effort": "medium"}
        if value.get("tools"):
            request["tools"] = value["tools"]
        self.invocation_started_ms = int(time.time() * 1000)
        reservation = None
        if getattr(self, "hosted", False):
            from axiom_chutes.metering import billing_context, price_charge
            import re
            native_id = value.get("request_id")
            if not isinstance(native_id, str) or not re.fullmatch(r"[0-9a-f]{32}", native_id):
                raise ValueError("canonical native invocation ID required")
            request["request_id"] = native_id + os.urandom(16).hex()
            reservation = await self.client.api("POST", "/reservations", json={
                "instance_id": row.instance_id, "maximum_charge_microusd": self.maximum_charge, "request_id": native_id})
            if (reservation["signing_key"] != self.metering["signing_key"]
                    or reservation["boot_id"] != self.metering["boot_id"]
                    or reservation["request_id"] != native_id
                    or reservation["price_version"] != reservation["price"]["version"]
                    or type(reservation["maximum_charge_microusd"]) is not int
                    or reservation["maximum_charge_microusd"] != price_charge(reservation["price"])
                    or not 0 <= reservation["maximum_charge_microusd"] <= self.maximum_charge):
                await self.client.api("POST", "/reservations/" + reservation["reservation_id"] + "/cancel")
                raise ValueError("metering key or native cost ceiling changed")
            context = {k: reservation[k] for k in ("reservation_id", "billing_nonce", "price_version", "cache_namespace")}
            request["billing"] = billing_context(context)
            self.pending_billing = reservation
        payload = {**request, "confirmation_secret": secret.hex()}
        # The API header selects forwarding, but the SDK selects response
        # encryption from this field inside the encrypted body. The SDK adds
        # its fixed usage options before validating the cord's input model.
        if stream:
            payload["stream"] = True
        blob, response_key = seal(unbase64(row.public_key), payload, limit=24 * 1024 * 1024)
        self._development_heartbeat()
        headers = {**self.client._headers, "X-Chute-Id": row.chute_id,
                   "X-Instance-Id": row.instance_id, "X-E2E-Nonce": row.invocation_nonce,
                   "X-E2E-Path": path, "X-E2E-Stream": "true" if stream else "false",
                   "Content-Type": "application/octet-stream"}
        if reservation:
            headers.update({"X-Axiom-Reservation": reservation["reservation_id"],
                            "X-Axiom-Request-Id": request["request_id"]})
        return evidence, secret, request, response_key, headers, blob

    async def complete(self, value):
        evidence, secret, request, response_key, headers, blob = await self.prepare_request(value, "/desktop-chat", False)
        import httpx
        async with self.client._http.stream("POST", self.client.invocation_url, headers=headers, content=blob) as response:
                if response.status_code != 200:
                    raise RuntimeError("encrypted invocation failed; request was not replayed")
                result = open_response(await read_limited(response), response_key)
        answer = validate_response(result, secret, request)
        return {"evidence": evidence, **answer}

    async def image(self, value, *, on_progress=None):
        """Native tool-only operation; never receives a provider credential."""
        import re
        from axiom_chutes.image_generation import image_source, image_references, verify_image_response, requested_dimensions
        from axiom_chutes.metering import IMAGE_EDIT_MODEL, IMAGE_MODEL, billing_context, price_charge, verify_image_receipt
        edit = value.get("op") == "image_edit"
        operation, model_id = ("image_edit", IMAGE_EDIT_MODEL) if edit else ("image", IMAGE_MODEL)
        if (not self.hosted or set(value) - {"width", "height", "stream", "runtime_policy_sha256"} != {"op", "request_id", "prompt", "references", "uncensored"}
                or type(value.get("uncensored")) is not bool
                or "stream" in value and value["stream"] is not True
                or not isinstance(value["request_id"], str) or not re.fullmatch(r"[0-9a-f]{32}", value["request_id"])
                or not isinstance(value["prompt"], str) or not value["prompt"].strip()
                or len(value["prompt"].encode()) > 8192):
            raise ValueError("native image tool request required")
        references = image_references(value)
        if edit != bool(references):
            raise ValueError("reference list and billing operation differ")
        requested_dimensions(value, references[0] if references else None)
        evidence = await self.attest()
        runtime = getattr(self.release, "runtime", None)
        runtime_fields = {}
        if runtime is not None:
            from axiom_chutes.runtime_policy import runtime_digest
            policy_digest = runtime_digest(runtime)
            if value.get("runtime_policy_sha256") != policy_digest:
                raise ValueError("runtime policy changed before dispatch; refresh required")
            runtime_fields = {"runtime_policy": self.release_metadata, "runtime_policy_sha256": policy_digest}
        elif "runtime_policy_sha256" in value:
            raise ValueError("runtime policy was removed before dispatch")
        # Check advertised limits before reserving credit or consuming a nonce.
        image_references({**value, **runtime_fields})
        requested_dimensions({**value, **runtime_fields}, references[0] if references else None)
        instance = self.hardware.instance
        rows = await self.client.discover(instance.chute_id)
        rows = [r for r in rows if r.instance_id == instance.instance_id and r.public_key == instance.public_key
                and r.nonce_expires_at > time.time() + 5]
        if not rows:
            raise ValueError("verified image recipient unavailable")
        row = rows[0]
        reservation = await self.client.api("POST", "/reservations", json={"instance_id": row.instance_id,
            "maximum_charge_microusd": self.maximum_charge, "request_id": value["request_id"], "operation": operation,
            **({"runtime_policy_sha256": policy_digest} if runtime is not None else {})})
        steps = len(runtime["image"]["sigmas"]) - 1 if runtime is not None else 8
        if (reservation["signing_key"] != self.metering["signing_key"] or reservation["boot_id"] != self.metering["boot_id"]
                or reservation["request_id"] != value["request_id"]
                or reservation["price_version"] != reservation["price"]["version"]
                or type(reservation["maximum_charge_microusd"]) is not int
                or reservation["maximum_charge_microusd"] != price_charge(reservation["price"], operation=operation, image_steps=steps)
                or not 0 <= reservation["maximum_charge_microusd"] <= self.maximum_charge):
            await self.client.api("POST", "/reservations/" + reservation["reservation_id"] + "/cancel")
            raise ValueError("image usage key or cost ceiling differs")
        secret = os.urandom(32)
        context = billing_context({k: reservation[k] for k in ("reservation_id", "billing_nonce", "price_version", "cache_namespace")})
        request = {"request_id": value["request_id"] + os.urandom(16).hex(), "prompt": value["prompt"],
                   "seed": int.from_bytes(os.urandom(4), "big"), "billing": context,
                   "references": references, "uncensored": value["uncensored"],
                   **runtime_fields,
                   **({"width": value["width"], "height": value["height"]} if "width" in value else {})}
        streaming = value.get("stream") is True
        blob, response_key = seal(unbase64(row.public_key), {**request, "confirmation_secret": secret.hex(),
            **({"stream": True} if streaming else {})}, limit=36 * 1024 * 1024)
        path = "/desktop-image-edit" if edit else "/desktop-image"
        if streaming:
            path += "-stream"
        headers = {**self.client._headers, "X-Chute-Id": row.chute_id, "X-Instance-Id": row.instance_id,
            "X-E2E-Nonce": row.invocation_nonce, "X-E2E-Path": path, "X-E2E-Stream": "true" if streaming else "false",
            "X-Axiom-Reservation": reservation["reservation_id"], "X-Axiom-Request-Id": request["request_id"],
            "Content-Type": "application/octet-stream"}
        started = int(time.time() * 1000)
        self._development_heartbeat()
        async with self.client._http.stream("POST", self.client.invocation_url, headers=headers, content=blob) as response:
            if response.status_code != 200:
                raise RuntimeError("encrypted image invocation failed; do not replay")
            if streaming:
                reply = await self.image_stream_response(response, response_key, secret, request, on_progress)
            else:
                reply = open_response(await read_limited(response, 24 * 1024 * 1024), response_key, limit=16 * 1024 * 1024)
            answer = verify_image_response(reply, secret, request)
        counts = verify_image_receipt(answer["billing_receipt"], self.metering["signing_key"], self.metering["boot_id"],
                                      {**context, "request_id": request["request_id"], "model": model_id,
                                       **({"runtime_policy_sha256": request["runtime_policy_sha256"]} if runtime is not None else {})})
        if (counts["image_pixels"] != answer["width"] * answer["height"]
                or counts.get("source_image_pixels", 0) != sum(image_source(source)[1] for source in references)):
            raise ValueError("image receipt dimensions differ")
        billing = await self.client.api("POST", "/receipts", json=answer["billing_receipt"])
        if (billing.get("settled") is not True or billing.get("cost") != price_charge(reservation["price"], counts, operation=operation)
                or not 0 <= billing["cost"]["total_microusd"] <= self.maximum_charge):
            raise ValueError("image usage settlement not confirmed")
        return {"evidence": evidence, "response_authenticated": True, "billing_receipt_verified": True,
            "request_id": value["request_id"], "model_id": model_id, "billing": billing,
            "references": answer["references"],
            **{key: answer[key] for key in ("uncensored", "lora")},
            **({"runtime_policy_sha256": answer["runtime_policy_sha256"]} if runtime is not None else {}),
            **{k: answer[k] for k in ("image_base64", "image_sha256", "mime_type", "width", "height", "steps")},
            "accounting": {"requestId": value["request_id"], "modelId": model_id, "providerId": "chutes",
                "state": "completed", "completeness": "final", "inputTokens": "0", "outputTokens": "0",
                "cachedInputTokens": "0", "reasoningTokens": "0", "costMicrousd": str(billing["cost"]["total_microusd"]),
                "settled": True, "responseVerified": False, "startedAtMs": str(started),
                "finishedAtMs": str(int(time.time() * 1000)), "finishReason": "stop"}}

    async def image_stream_response(self, response, response_key, secret, request, on_progress):
        from axiom_chutes.image_stream import ImageStreamVerifier, IMAGE_STREAM_FRAME_BYTES
        verifier, cipher = ImageStreamVerifier(secret, request), StreamCipher(response_key)
        buffer, transport_done = b"", False
        async for raw in sse_data(response, max_bytes=48 * 1024 * 1024):
            if transport_done:
                raise ValueError("transport data followed image completion")
            if raw == b"[DONE]":
                if not verifier.ended:
                    raise ValueError("unauthenticated image completion")
                transport_done = True
                continue
            buffer += cipher.open(decode_json(raw, 512 * 1024))
            while b"\n" in buffer:
                line, buffer = buffer.split(b"\n", 1)
                if not line.strip():
                    continue
                kind, data = verifier.accept(decode_json(line, IMAGE_STREAM_FRAME_BYTES))
                if kind == "progress" and on_progress:
                    await on_progress(data)
            if len(buffer) > IMAGE_STREAM_FRAME_BYTES:
                raise ValueError("image stream frame size")
        if buffer.strip() or cipher.stream_key is None:
            raise ValueError("incomplete encrypted image framing")
        return verifier.finish()

    async def stream(self, value):
        evidence, secret, request, response_key, headers, blob = await self.prepare_request(value, "/desktop-stream", True)
        verifier, cipher = StreamVerifier(secret, request), StreamCipher(response_key)
        buffer, transport_done = b"", False
        import httpx
        async with self.client._http.stream("POST", self.client.invocation_url, headers=headers, content=blob) as response:
                if response.status_code != 200:
                    raise RuntimeError("encrypted stream failed; request was not replayed")
                async for raw in sse_data(response):
                    if transport_done:
                        raise ValueError("transport data followed completion")
                    if raw == b"[DONE]":
                        if not verifier.ended:
                            raise ValueError("unauthenticated outer completion")
                        transport_done = True
                        continue
                    buffer += cipher.open(decode_json(raw, 512 * 1024))
                    while b"\n" in buffer:
                        line, buffer = buffer.split(b"\n", 1)
                        if not line.strip():
                            continue
                        record = decode_json(line, MAX_FRAME)
                        kind, data = verifier.accept(record)
                        sequence = record["sequence"]
                        if kind == "start":
                            yield {"ok": True, "event": "start", "sequence": sequence, "evidence": evidence}
                        elif kind in ("text", "reasoning"):
                            yield {"ok": True, "event": kind, "sequence": sequence, "text": data}
                        # Do not mark success at the terminal frame: the
                        # complete HTTP stream must also end without extra data.
                    if len(buffer) > MAX_FRAME:
                        raise ValueError("inner stream frame limit")
        if buffer.strip() or cipher.stream_key is None:
            raise ValueError("incomplete encrypted stream framing")
        answer = verifier.finish()
        if self.hosted:
            from axiom_chutes.metering import verify_receipt, price_charge
            receipt = answer.get("billing_receipt")
            counts = verify_receipt(receipt, self.metering["signing_key"], self.metering["boot_id"],
                {**request["billing"], "request_id": request["request_id"]})
            if (counts["input_tokens"] != answer["input_tokens"]
                    or counts["output_tokens"] != answer["output_tokens"]):
                raise ValueError("signed usage differs from the authenticated stream")
            billing = await self.client.api("POST", "/receipts", json=receipt)
            if (billing.get("settled") is not True
                    or billing.get("cost") != price_charge(self.pending_billing["price"], counts)
                    or type(billing.get("cost", {}).get("total_microusd")) is not int
                    or not 0 <= billing["cost"]["total_microusd"] <= self.maximum_charge):
                raise ValueError("usage settlement not confirmed")
            answer.update(billing_receipt_verified=True, billing=billing)
            answer["accounting"] = {"requestId": self.pending_billing["request_id"],
                "modelId": "chutes:qwen3.8-27b", "providerId": "chutes", "state": "completed",
                "completeness": "final", "inputTokens": str(counts["input_tokens"]),
                "cachedInputTokens": str(counts["cached_input_tokens"]), "outputTokens": str(counts["output_tokens"]),
                "reasoningTokens": str(counts["reasoning_tokens"]), "costMicrousd": str(billing["cost"]["total_microusd"]),
                "settled": True, "responseVerified": False, "startedAtMs": str(self.invocation_started_ms),
                "finishedAtMs": str(int(time.time() * 1000)), "finishReason": answer["finish_reason"]}
            self.pending_billing = None
        yield {"ok": True, "event": "complete", "sequence": verifier.sequence - 1,
               "evidence": evidence, **answer}


def validate_response(value, secret, request):
    if (not isinstance(value, dict)
            or set(value) not in ({"result", "confirmation"}, {"result", "confirmation", "queue_seconds"})):
        raise ValueError("invalid reply envelope")
    # The shared GPU gate appends queue timing outside the authenticated result.
    # Never use that operational hint as evidence, output, or billing data.
    if "queue_seconds" in value and (type(value["queue_seconds"]) not in (int, float)
            or not math.isfinite(value["queue_seconds"]) or not 0 <= value["queue_seconds"] <= 660):
        raise ValueError("invalid optional queue timing")
    result = value["result"]
    expected = hmac.new(secret, b"axiom-chutes-desktop-v1\0" + canonical(result), hashlib.sha256).hexdigest()
    if (not isinstance(value["confirmation"], str) or not hmac.compare_digest(expected, value["confirmation"])
            or result.get("request_id") != request["request_id"]
            or result.get("request_sha256") != hashlib.sha256(canonical(request)).hexdigest()
            or result.get("development_preview") is not True
            or result.get("workload_binding_verified") is not False
            or result.get("finish_reason") not in ("stop", "length")):
        raise ValueError("reply authentication or request binding failed")
    message, usage = result["message"], result["usage"]
    if (not isinstance(message, dict) or message.get("role") != "assistant"
            or message.get("tool_calls") or message.get("refusal")
            or any(message.get(k) is not None and not isinstance(message[k], str)
                   for k in ("content", "reasoning", "reasoning_content"))
            or any(type(usage.get(k)) is not int or not 0 <= usage[k] <= 65536
                   for k in ("prompt_tokens", "completion_tokens"))):
        raise ValueError("invalid authenticated text reply")
    return {"text": message.get("content") or "",
            "reasoning": message.get("reasoning_content") or message.get("reasoning") or "",
            "input_tokens": usage["prompt_tokens"], "output_tokens": usage["completion_tokens"],
            "finish_reason": result["finish_reason"], "response_authenticated": True}


async def run(path):
    bridge = Bridge(path)
    try:
        while True:
            raw = await asyncio.to_thread(sys.stdin.buffer.readline, MAX_PIPE + 1)
            if not raw:
                break
            if len(raw) > MAX_PIPE or not raw.endswith(b"\n"):
                raise ValueError("native input limit")
            try:
                value = decode_json(raw, MAX_PIPE)
                if value.get("op") == "configure":
                    bridge.configure(value)
                    reply = {"ok": True}
                elif value == {"op": "attest"}:
                    reply = {"ok": True, "evidence": await bridge.attest()}
                elif value == {"op": "capabilities"}:
                    reply = {"ok": True, **await bridge.capabilities()}
                elif value.get("op") == "stream":
                    async for event in bridge.stream(value):
                        write_reply(event)
                    continue
                elif value.get("op") == "complete":
                    reply = {"ok": True, **await bridge.complete(value)}
                elif value.get("op") in {"image", "image_edit"}:
                    sequence = 0
                    async def image_progress(progress):
                        nonlocal sequence
                        write_reply({"ok": True, "event": "image_progress", "request_id": value["request_id"],
                                     "sequence": sequence, "progress": progress})
                        sequence += 1
                    result = await bridge.image(value, on_progress=image_progress)
                    reply = {"ok": True, **result}
                    if value.get("stream") is True:
                        reply.update(event="image_complete", sequence=sequence)
                else:
                    raise ValueError("unsupported native operation")
            except Exception as error:
                # Provider errors may include sensitive request data. Expose
                # only the exception class, never its message or a traceback.
                reply = {"ok": False, "error": type(error).__name__}
            finally:
                # A reusable helper retains public verification and its account
                # transport, not the preceding prompt, reference images or
                # output. This also runs for streamed chat's `continue` above.
                raw = value = result = event = image_progress = None
            try:
                write_reply(reply)
            finally:
                reply = None
    finally:
        if bridge.client is not None:
            await bridge.client.close()


def write_reply(reply):
    encoded = canonical(reply)
    if len(encoded) > MAX_PIPE:
        raise ValueError("native output limit")
    sys.stdout.buffer.write(encoded + b"\n")
    sys.stdout.buffer.flush()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", type=Path, required=True)
    args = parser.parse_args()
    try:
        asyncio.run(run(args.config))
    except Exception:
        sys.exit(1)
