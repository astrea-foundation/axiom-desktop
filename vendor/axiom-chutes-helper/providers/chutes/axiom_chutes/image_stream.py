"""Request-authenticated image progress and bounded final-result streaming."""
import asyncio
import hashlib
import hmac

from .wire import canonical, decode_json
from .image_generation import generate_image, image_input, MAX_IMAGE_JSON, ImageInputRejected

IMAGE_STREAM_DOMAIN = b"axiom-chutes-image-stream-v1\0"
IMAGE_STREAM_FRAME_BYTES = 128 * 1024
IMAGE_STREAM_CHUNK_BYTES = 64 * 1024
IMAGE_STREAM_MAX_FRAMES = 2048
IMAGE_STAGES = {"queued": 0, "generating": 1, "finishing": 2}


def validate_image_progress(value, previous=None):
    if (not isinstance(value, dict) or set(value) != {"stage", "percent"}
            or not isinstance(value.get("stage"), str) or value["stage"] not in IMAGE_STAGES or type(value.get("percent")) is not int
            or not 0 <= value["percent"] <= 99
            or value["stage"] == "queued" and value["percent"] != 0
            or value["stage"] == "finishing" and value["percent"] != 99
            or previous is not None and (IMAGE_STAGES[value["stage"]] < IMAGE_STAGES[previous["stage"]]
                                        or value["percent"] < previous["percent"])):
        raise ValueError("invalid or regressing image progress")
    return value


class ImageStreamEncoder:
    def __init__(self, secret, request):
        self.secret, self.request_id = secret, request["request_id"]
        self.request_hash = hashlib.sha256(canonical(request)).hexdigest()
        self.sequence, self.previous = 0, "00" * 32

    def frame(self, kind, data):
        body = {"request_id": self.request_id,
                "request_sha256": self.request_hash,
                "sequence": self.sequence, "previous": self.previous, "kind": kind, "data": data}
        self.previous = hmac.new(self.secret, IMAGE_STREAM_DOMAIN + canonical(body), hashlib.sha256).hexdigest()
        self.sequence += 1
        if self.sequence > IMAGE_STREAM_MAX_FRAMES:
            raise ValueError("image stream frame limit")
        encoded = canonical({**body, "mac": self.previous})
        if len(encoded) > IMAGE_STREAM_FRAME_BYTES:
            raise ValueError("image stream frame size")
        return encoded.decode() + "\n"

    def complete(self, reply):
        raw = canonical(reply)
        if len(raw) > MAX_IMAGE_JSON:
            raise ValueError("image result size")
        text = raw.decode()
        for offset in range(0, len(text), IMAGE_STREAM_CHUNK_BYTES):
            yield self.frame("result_chunk", text[offset:offset + IMAGE_STREAM_CHUNK_BYTES])
        yield self.frame("end", {"sha256": hashlib.sha256(raw).hexdigest()})


class ImageStreamVerifier:
    def __init__(self, secret, request):
        self.secret, self.request_id = secret, request["request_id"]
        self.request_hash = hashlib.sha256(canonical(request)).hexdigest()
        self.sequence, self.previous = 0, "00" * 32
        self.progress, self.chunks, self.bytes, self.ended = None, [], 0, False

    def accept(self, frame):
        if (self.ended or self.sequence >= IMAGE_STREAM_MAX_FRAMES or not isinstance(frame, dict)
                or set(frame) != {"request_id", "request_sha256", "sequence", "previous", "kind", "data", "mac"}
                or type(frame["sequence"]) is not int or frame["sequence"] != self.sequence
                or frame["previous"] != self.previous or frame["request_id"] != self.request_id
                or frame["request_sha256"] != self.request_hash
                or not isinstance(frame["mac"], str) or len(canonical(frame)) > IMAGE_STREAM_FRAME_BYTES):
            raise ValueError("image stream order or request mismatch")
        body = {key: value for key, value in frame.items() if key != "mac"}
        if not hmac.compare_digest(frame["mac"], hmac.new(self.secret, IMAGE_STREAM_DOMAIN + canonical(body), hashlib.sha256).hexdigest()):
            raise ValueError("image stream authentication failed")
        kind, data = frame["kind"], frame["data"]
        if kind == "progress":
            if self.chunks or self.progress is None and data != {"stage": "queued", "percent": 0}:
                raise ValueError("invalid image progress ordering")
            self.progress = validate_image_progress(data, self.progress)
        elif kind == "result_chunk":
            if (self.progress != {"stage": "finishing", "percent": 99} or not isinstance(data, str)
                    or not data or len(data.encode()) > IMAGE_STREAM_CHUNK_BYTES):
                raise ValueError("invalid image result chunk")
            self.bytes += len(data.encode())
            if self.bytes > MAX_IMAGE_JSON:
                raise ValueError("image result size")
            self.chunks.append(data)
        elif kind == "end":
            if not self.chunks or data != {"sha256": hashlib.sha256("".join(self.chunks).encode()).hexdigest()}:
                raise ValueError("incomplete image result")
            self.ended = True
        else:
            raise ValueError("unknown image stream frame")
        self.previous, self.sequence = frame["mac"], self.sequence + 1
        return kind, data

    def finish(self):
        if not self.ended:
            raise ValueError("image stream ended without completion")
        return decode_json("".join(self.chunks).encode(), MAX_IMAGE_JSON)


async def image_stream(value, meter, fence, gate, *, http=None, on_failure=None):
    request = image_input(value)
    encoder = ImageStreamEncoder(bytes.fromhex(value["confirmation_secret"]), request)
    last = {"stage": "queued", "percent": 0}
    yield encoder.frame("progress", last)

    async def dispatch():
        output = asyncio.Queue(maxsize=16)
        async def report(progress):
            await output.put(("progress", progress))
        async def execute():
            try:
                reply = await generate_image(value, meter, fence, http=http, on_progress=report, admitted_request=request)
                await output.put(("complete", reply))
            except ImageInputRejected:
                raise
            except BaseException:
                if on_failure:
                    await on_failure()
                raise
        task = asyncio.create_task(execute())
        try:
            while not task.done() or not output.empty():
                try:
                    yield await asyncio.wait_for(output.get(), .25)
                except TimeoutError:
                    pass
            await task
        finally:
            if not task.done():
                task.cancel()
            await asyncio.gather(task, return_exceptions=True)

    stream = gate.stream("image", dispatch)
    pending = asyncio.create_task(anext(stream))
    try:
        while True:
            ready, _ = await asyncio.wait({pending}, timeout=5)
            if not ready:
                yield encoder.frame("progress", last)
                continue
            try:
                kind, data = pending.result()
            except StopAsyncIteration:
                raise ValueError("image operation ended without a result") from None
            if kind == "complete":
                # Drain the owned task before reporting successful completion.
                try:
                    await anext(stream)
                except StopAsyncIteration:
                    pass
                else:
                    raise ValueError("extra image operation output")
                for frame in encoder.complete(data):
                    yield frame
                return
            last = validate_image_progress(data, last)
            yield encoder.frame("progress", last)
            pending = asyncio.create_task(anext(stream))
    finally:
        if not pending.done():
            pending.cancel()
        await asyncio.gather(pending, return_exceptions=True)
        await stream.aclose()
