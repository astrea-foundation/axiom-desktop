"""Request-bound, ordered streaming for the explicit Chutes desktop preview.

Chutes encrypts each transport chunk. A secret sent only in request ciphertext
authenticates our frames, their order, and the final transcript/usage. Runtime
and application qualification remain distinct from reply authentication.
"""
import hashlib
import hmac
import os
import re

import httpx

from .wire import canonical, decode_json
from .metering import billing_context
from .image_generation import image_source

STREAM_DOMAIN = b"axiom-chutes-desktop-stream-v1\0"
MAX_TEXT = 2 * 1024 * 1024
MAX_FRAME = 128 * 1024
MAX_FRAMES = 16384
MAX_CHAT_BYTES = 20 * 1024 * 1024


class PreviewInputRejected(ValueError):
    """Input was rejected before a GPU job could remain running."""


def tool_calls(value, names=None):
    if not isinstance(value, list) or not 1 <= len(value) <= 32 or len(canonical(value)) > 256 * 1024:
        raise ValueError("bounded tool calls required")
    seen = set()
    for call in value:
        if (not isinstance(call, dict) or set(call) != {"id", "type", "function"} or call["type"] != "function"
                or not isinstance(call["id"], str) or not re.fullmatch(r"[A-Za-z0-9_-]{1,128}", call["id"])
                or call["id"] in seen or not isinstance(call["function"], dict)
                or set(call["function"]) != {"name", "arguments"}
                or not isinstance(call["function"]["name"], str)
                or not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]{0,127}", call["function"]["name"])
                or names is not None and call["function"]["name"] not in names
                or not isinstance(call["function"]["arguments"], str)
                or len(call["function"]["arguments"].encode()) > 65536):
            raise ValueError("invalid or unadvertised tool call")
        seen.add(call["id"])
    return value


def chat_tools(value):
    if not isinstance(value, list) or len(value) > 128 or len(canonical(value)) > 512 * 1024:
        raise PreviewInputRejected("bounded tools required")
    names = set()
    for tool in value:
        fn = tool.get("function") if isinstance(tool, dict) else None
        if (not isinstance(tool, dict) or set(tool) != {"type", "function"} or tool["type"] != "function"
                or not isinstance(fn, dict) or not {"name", "description", "parameters"} <= set(fn)
                or set(fn) - {"name", "description", "parameters", "strict"}
                or not isinstance(fn["name"], str) or not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]{0,127}", fn["name"])
                or fn["name"] in names or not isinstance(fn["description"], str)
                or not isinstance(fn["parameters"], dict) or fn["parameters"].get("type") != "object"):
            raise PreviewInputRejected("invalid function definition")
        names.add(fn["name"])
    return value


def chat_messages(messages):
    if not isinstance(messages, list) or not 1 <= len(messages) <= 128 or len(canonical(messages)) > MAX_CHAT_BYTES:
        raise PreviewInputRejected("bounded messages required")
    pending, used, images = set(), set(), 0
    for m in messages:
        if (not isinstance(m, dict) or not {"role", "content"} <= set(m)
                or set(m) - {"role", "content", "tool_calls", "tool_call_id"}
                or m["role"] not in {"system", "user", "assistant", "tool"}):
            raise PreviewInputRejected("text and tool messages required")
        content = m["content"]
        if not isinstance(content, str):
            if m["role"] != "user" or not isinstance(content, list) or not 1 <= len(content) <= 11:
                raise PreviewInputRejected("bounded user image content required")
            for block in content:
                if isinstance(block, dict) and set(block) == {"type", "text"} and block["type"] == "text" and isinstance(block["text"], str):
                    continue
                if (not isinstance(block, dict) or set(block) != {"type", "image_url"}
                        or block["type"] != "image_url" or not isinstance(block["image_url"], dict)
                        or set(block["image_url"]) != {"url"} or not isinstance(block["image_url"]["url"], str)):
                    raise PreviewInputRejected("inline private image required")
                images += 1
                if images > 10:
                    raise PreviewInputRejected("at most ten images in the model context")
                url = block["image_url"]["url"]
                match = re.fullmatch(r"data:(image/(?:png|jpeg|webp|gif));base64,([A-Za-z0-9+/]*={0,2})", url)
                if match is None or len(match[2]) > 7_000_000:
                    raise PreviewInputRejected("bounded inline image data required")
                import base64
                try:
                    raw = base64.b64decode(match[2], validate=True)
                    image_source({"id":"img_" + "0" * 32, "mime_type":match[1], "data":match[2],
                                  "sha256":hashlib.sha256(raw).hexdigest()})
                except ValueError as error:
                    raise PreviewInputRejected("invalid private image") from error
        if "tool_calls" in m:
            if m["role"] != "assistant" or pending:
                raise PreviewInputRejected("invalid assistant tool history")
            calls = tool_calls(m["tool_calls"])
            if any(c["id"] in used for c in calls):
                raise PreviewInputRejected("duplicate tool history")
            pending = {c["id"] for c in calls}
            used.update(pending)
        if m["role"] == "tool":
            if set(m) != {"role", "content", "tool_call_id"} or m["tool_call_id"] not in pending:
                raise PreviewInputRejected("orphan tool result")
            pending.remove(m["tool_call_id"])
        elif "tool_call_id" in m or pending and m["role"] != "assistant":
            raise PreviewInputRejected("incomplete tool history")
    if pending:
        raise PreviewInputRejected("unanswered tool call")
    return messages


def desktop_stream_input(value):
    if (os.environ.get("AXIOM_CHUTES_BOOTSTRAP_ONLY") != "1"
            or not isinstance(value, dict)
            or not {"request_id", "confirmation_secret", "messages", "max_tokens", "reasoning_effort"} <= set(value)
            or set(value) - {"request_id", "confirmation_secret", "messages", "max_tokens", "reasoning_effort", "billing", "tools"}
            or any(not isinstance(value[k], str) or not re.fullmatch(r"[0-9a-f]{64}", value[k])
                   for k in ("request_id", "confirmation_secret"))
            or type(value["max_tokens"]) is not int or not 1 <= value["max_tokens"] <= 4096
            or value["reasoning_effort"] != "medium"
            or not isinstance(value["messages"], list) or not 1 <= len(value["messages"]) <= 128
            or len(canonical(value["messages"])) > MAX_CHAT_BYTES):
        raise PreviewInputRejected("bounded private desktop input required")
    try:
        chat_messages(value["messages"])
        if "tools" in value:
            chat_tools(value["tools"])
    except ValueError as error:
        raise PreviewInputRejected("invalid tool conversation") from error
    request = {k: value[k] for k in ("request_id", "messages", "max_tokens", "reasoning_effort")}
    if value.get("billing") is not None:
        billing_context(value["billing"])
        request["billing"] = value["billing"]
    if "tools" in value:
        request["tools"] = value["tools"]
    return request


def stream_frame(secret, request, sequence, previous, kind, data, *, request_hash=None):
    body = {"request_id": request["request_id"], "request_sha256": request_hash or hashlib.sha256(canonical(request)).hexdigest(),
            "sequence": sequence, "previous": previous, "kind": kind, "data": data}
    tag = hmac.new(secret, STREAM_DOMAIN + canonical(body), hashlib.sha256).hexdigest()
    return {**body, "mac": tag}


def stream_usage(value):
    if (not isinstance(value, dict) or any(type(value.get(k)) is not int or not 0 <= value[k] <= 65536
                                         for k in ("prompt_tokens", "completion_tokens"))):
        raise ValueError("invalid authenticated usage")
    result = {k: value[k] for k in ("prompt_tokens", "completion_tokens")}
    for target, details, field, parent in (
            ("cached_input_tokens", "prompt_tokens_details", "cached_tokens", "prompt_tokens"),
            ("reasoning_tokens", "completion_tokens_details", "reasoning_tokens", "completion_tokens")):
        count = value.get(target, (value.get(details) or {}).get(field, 0))
        if count is None:
            count = 0
        if type(count) is not int or not 0 <= count <= result[parent]:
            raise ValueError("invalid engine usage details")
        result[target] = count
    return result


class StreamVerifier:
    def __init__(self, secret, request):
        if len(secret) != 32:
            raise ValueError("invalid stream secret")
        self.secret, self.request = secret, request
        self.request_hash = hashlib.sha256(canonical(request)).hexdigest()
        self.sequence, self.previous = 0, "00" * 32
        self.started = self.ended = False
        self.text, self.reasoning = [], []
        self.text_bytes = 0
        self.terminal = None

    def accept(self, frame):
        if (self.ended or self.sequence >= MAX_FRAMES or not isinstance(frame, dict)
                or set(frame) != {"request_id", "request_sha256", "sequence", "previous", "kind", "data", "mac"}
                or type(frame["sequence"]) is not int or frame["sequence"] != self.sequence
                or frame["previous"] != self.previous or frame["request_id"] != self.request["request_id"]
                or frame["request_sha256"] != self.request_hash or not isinstance(frame["mac"], str)
                or len(canonical(frame)) > MAX_FRAME):
            raise ValueError("stream order or request mismatch")
        body = {k: v for k, v in frame.items() if k != "mac"}
        expected = hmac.new(self.secret, STREAM_DOMAIN + canonical(body), hashlib.sha256).hexdigest()
        if not hmac.compare_digest(expected, frame["mac"]):
            raise ValueError("stream frame authentication failed")
        kind, data = frame["kind"], frame["data"]
        if kind == "start":
            if self.started or self.sequence != 0 or data != {
                    "model": "Qwen3.8-27B-Uncensored", "development_preview": True,
                    "workload_binding_verified": False}:
                raise ValueError("invalid stream start")
            self.started = True
        elif kind in ("text", "reasoning"):
            if not self.started or not isinstance(data, str) or not data:
                raise ValueError("invalid stream delta")
            self.text_bytes += len(data.encode())
            if self.text_bytes > MAX_TEXT:
                raise ValueError("stream text limit")
            (self.text if kind == "text" else self.reasoning).append(data)
        elif kind == "end":
            if (not self.started or not isinstance(data, dict)
                    or not {"finish_reason", "usage", "text_sha256", "reasoning_sha256"} <= set(data)
                    or set(data) - {"finish_reason", "usage", "text_sha256", "reasoning_sha256", "billing_receipt", "tool_calls"}
                    or data["finish_reason"] not in ("stop", "length", "tool_calls")
                    or data["text_sha256"] != hashlib.sha256("".join(self.text).encode()).hexdigest()
                    or data["reasoning_sha256"] != hashlib.sha256("".join(self.reasoning).encode()).hexdigest()):
                raise ValueError("incomplete authenticated transcript")
            stream_usage(data["usage"])
            if data["finish_reason"] == "tool_calls":
                tool_calls(data.get("tool_calls"), {t["function"]["name"] for t in self.request.get("tools", [])})
            elif data.get("tool_calls"):
                raise ValueError("tool calls require authenticated tool completion")
            self.ended, self.terminal = True, data
        else:
            raise ValueError("unsupported stream frame")
        self.previous, self.sequence = frame["mac"], self.sequence + 1
        return kind, data

    def finish(self):
        if not self.ended:
            raise ValueError("stream ended without authenticated completion")
        return {"text": "".join(self.text), "reasoning": "".join(self.reasoning),
                "input_tokens": self.terminal["usage"]["prompt_tokens"],
                "output_tokens": self.terminal["usage"]["completion_tokens"],
                "finish_reason": self.terminal["finish_reason"], "response_authenticated": True,
                **({"tool_calls": self.terminal["tool_calls"]} if "tool_calls" in self.terminal else {}),
                **({"billing_receipt": self.terminal["billing_receipt"]} if "billing_receipt" in self.terminal else {})}


async def sse_data(response, *, max_bytes=16 * 1024 * 1024, max_event=256 * 1024):
    """Bound complete SSE events before decoding; reject unterminated data."""
    buffer, lines, total, event_size = b"", [], 0, 0
    # A requested chunk_size makes httpx accumulate bytes and delays deltas.
    async for chunk in response.aiter_bytes():
        total += len(chunk)
        if total > max_bytes:
            raise ValueError("SSE response limit")
        buffer += chunk
        while b"\n" in buffer:
            line, buffer = buffer.split(b"\n", 1)
            line = line.rstrip(b"\r")
            event_size += len(line) + 1
            if event_size > max_event:
                raise ValueError("SSE event limit")
            if not line:
                if lines:
                    yield b"\n".join(lines)
                lines, event_size = [], 0
            elif line.startswith(b"data:"):
                lines.append(line[5:].removeprefix(b" "))
            elif not line.startswith((b":", b"event:", b"id:")):
                raise ValueError("unexpected SSE field")
        if len(buffer) + event_size > max_event:
            raise ValueError("SSE event limit")
    if buffer.strip() or lines:
        raise ValueError("truncated SSE event")


async def preview_desktop_stream(value, *, http=None, meter=None):
    request = desktop_stream_input(value)
    if "billing" in request:
        if meter is None:
            raise PreviewInputRejected("usage signer unavailable")
        meter.begin(request["billing"])
    secret, sequence, previous = bytes.fromhex(value["confirmation_secret"]), 0, "00" * 32
    verifier = StreamVerifier(secret, request)

    def frame(kind, data):
        nonlocal sequence, previous
        record = stream_frame(secret, request, sequence, previous, kind, data, request_hash=verifier.request_hash)
        verifier.accept(record)
        previous, sequence = record["mac"], sequence + 1
        return record

    owns = http is None
    http = http or httpx.AsyncClient(trust_env=False, follow_redirects=False, timeout=httpx.Timeout(600, connect=5))
    request_id = "axiom-desktop-" + request["request_id"]
    payload = {"model": "Qwen3.8-27B-Uncensored", "stream": True, "stream_options": {"include_usage": True},
               "messages": request["messages"], "max_tokens": request["max_tokens"],
               "reasoning_effort": "medium", "temperature": 0.6}
    if "billing" in request:
        payload["cache_salt"] = request["billing"]["cache_namespace"]
    if request.get("tools"):
        payload.update(tools=request["tools"], tool_choice="auto", parallel_tool_calls=False)
    finish = usage = None
    calls = {}
    done = False
    try:
        async with http.stream("POST", "http://127.0.0.1:10101/v1/chat/completions", json=payload,
                               headers={"X-Request-Id": request_id}) as response:
            if response.status_code in (400, 413, 422):
                raise PreviewInputRejected("engine rejected the desktop input")
            if response.status_code != 200 or response.headers.get("x-request-id") != request_id:
                raise RuntimeError("desktop engine identity mismatch")
            yield frame("start", {"model": payload["model"], "development_preview": True,
                                  "workload_binding_verified": False})
            async for raw in sse_data(response, max_bytes=8 * 1024 * 1024, max_event=MAX_FRAME):
                if done:
                    raise ValueError("engine data followed completion")
                if raw == b"[DONE]":
                    done = True
                    continue
                chunk = decode_json(raw, MAX_FRAME)
                if chunk.get("usage") is not None:
                    if usage is not None:
                        raise ValueError("duplicate engine usage")
                    usage = stream_usage(chunk["usage"])
                choices = chunk.get("choices", [])
                if not isinstance(choices, list) or len(choices) > 1:
                    raise ValueError("unexpected engine choices")
                if not choices:
                    continue
                choice = choices[0]
                delta = choice.get("delta", {})
                if choice.get("index", 0) != 0 or not isinstance(delta, dict):
                    raise ValueError("invalid engine delta")
                if delta.get("refusal") or delta.get("function_call"):
                    raise ValueError("unsupported engine output")
                for part in delta.get("tool_calls") or []:
                    index = part.get("index")
                    if type(index) is not int or not 0 <= index < 32:
                        raise ValueError("invalid tool delta index")
                    call = calls.setdefault(index, {"id":"", "type":"function", "function":{"name":"", "arguments":""}})
                    if part.get("type", "function") != "function":
                        raise ValueError("unsupported tool kind")
                    for field in ("id",):
                        if part.get(field) is not None:
                            if not isinstance(part[field], str):
                                raise ValueError("invalid tool identity")
                            call[field] += part[field]
                    fn = part.get("function") or {}
                    for field in ("name", "arguments"):
                        if fn.get(field) is not None:
                            if not isinstance(fn[field], str):
                                raise ValueError("invalid tool delta")
                            call["function"][field] += fn[field]
                    if len(canonical(calls)) > 256 * 1024:
                        raise ValueError("tool delta limit")
                if finish is not None:
                    raise ValueError("engine delta followed finish")
                for name, kind in (("reasoning", "reasoning"), ("content", "text")):
                    text = delta.get(name)
                    if name == "reasoning" and text is None:
                        text = delta.get("reasoning_content")
                    if text is not None:
                        if not isinstance(text, str):
                            raise ValueError("invalid engine text")
                        if text:
                            yield frame(kind, text)
                if choice.get("finish_reason") is not None:
                    finish = choice["finish_reason"]
        if not done or finish not in ("stop", "length", "tool_calls") or usage is None:
            raise ValueError("engine stream is incomplete")
        receipt = {}
        if "billing" in request:
            if meter is None:
                raise ValueError("independent usage signer required")
            receipt["billing_receipt"] = meter.finish(request["billing"], request["request_id"], usage)
        yield frame("end", {"finish_reason": finish, "usage": usage, **receipt,
                            **({"tool_calls": [calls[k] for k in sorted(calls)]} if calls else {}),
                            "text_sha256": hashlib.sha256("".join(verifier.text).encode()).hexdigest(),
                            "reasoning_sha256": hashlib.sha256("".join(verifier.reasoning).encode()).hexdigest()})
    finally:
        if owns:
            await http.aclose()
