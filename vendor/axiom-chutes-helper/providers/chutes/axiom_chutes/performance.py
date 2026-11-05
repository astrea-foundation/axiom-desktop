"""Fixed GPU profiles and synthetic benchmarks; never accepts user prompts.

The deployer chooses a profile before a fresh worker epoch starts. These
helpers neither change an active engine nor establish production admission.
"""

import ast
import asyncio
from contextlib import asynccontextmanager
import json
import re
import time

import httpx


PERFORMANCE_PROFILES = (
    "eager-marlin", "graphs-marlin", "graphs-native",
    "graphs-marlin-mtp-1", "graphs-marlin-mtp-2", "graphs-marlin-mtp-3",
    "graphs-native-mtp-1", "graphs-native-mtp-2", "graphs-native-mtp-3",
)


class CodingQualityError(ValueError):
    """Generation completed, but its fixed synthetic answer was incorrect."""


def performance_command(profile):
    if profile not in PERFORMANCE_PROFILES:
        raise ValueError("unknown fixed performance profile")
    args = ["/opt/axiom/vllm/bin/python", "-m", "vllm.entrypoints.openai.api_server",
        "--model", "/models/llm", "--served-model-name", "Qwen3.8-27B-Uncensored",
        "--host", "127.0.0.1", "--port", "10101", "--gpu-memory-utilization", "0.70",
        "--dtype", "bfloat16", "--linear-backend", "auto" if "native" in profile else "marlin",
        "--kv-cache-dtype", "auto", "--kv-cache-memory-bytes", "12884901888",
        "--max-model-len", "32768", "--max-num-seqs", "4", "--max-num-batched-tokens", "2048",
        "--no-enable-prefix-caching", "--enable-request-id-headers",
        "--limit-mm-per-prompt", '{"image":{"count":10,"width":1024,"height":1024},"video":0}',
        "--mm-processor-kwargs", '{"size":{"longest_edge":1048576,"shortest_edge":65536}}',
        "--mm-processor-cache-gb", "0",
        "--no-enable-log-requests", "--no-enable-log-outputs", "--enable-prompt-tokens-details",
        "--enable-auto-tool-choice", "--tool-call-parser", "qwen3_xml",
        "--reasoning-parser", "qwen3", "--default-chat-template-kwargs", '{"reasoning_effort":"medium"}']
    args += ["--enforce-eager" if profile.startswith("eager-") else "--no-enforce-eager"]
    if "-mtp-" in profile:
        args += ["--speculative-config", json.dumps({"method": "mtp",
            "num_speculative_tokens": int(profile.rsplit("-", 1)[1])}, separators=(",", ":"))]
    return args


def performance_image_command():
    return ["/opt/axiom/comfy/bin/python", "-m", "axiom_chutes.comfy_engine"]



async def performance_vision_warmup(count=1):
    """Compile only fixed synthetic vision shapes before accepting user jobs."""
    import base64
    import struct
    import zlib
    def chunk(kind, value):
        return struct.pack(">I", len(value)) + kind + value + struct.pack(">I", zlib.crc32(kind + value) & 0xffffffff)
    rows = (b"\0" + b"\x20\x60\xff" * 1024) * 1024
    png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 1024, 1024, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b"")
    url = "data:image/png;base64," + base64.b64encode(png).decode()
    content = [{"type":"text", "text":"What is the dominant color of these images? Reply with only the color name."}]
    content += [{"type":"image_url","image_url":{"url":url}} for _ in range(count)]
    start = time.monotonic()
    async with httpx.AsyncClient(trust_env=False, follow_redirects=False, timeout=300) as client:
        response = await client.post("http://127.0.0.1:10101/v1/chat/completions", json={
            "model":"Qwen3.8-27B-Uncensored", "messages":[{"role":"user","content":content}],
            "max_tokens":256, "reasoning_effort":"medium", "temperature":0})
        response.raise_for_status()
        value = response.json()
    choice = value["choices"][0]
    visual = value["usage"].get("prompt_tokens_details", {}).get("multimodal_tokens", {}).get("image")
    if (choice["finish_reason"] != "stop" or "blue" not in (choice["message"].get("content") or "").lower()
            or visual != count * 1024):
        raise RuntimeError("fixed vision warmup did not verify the expected image input")
    return {"image_count":count,"visual_tokens":visual,"seconds":time.monotonic()-start,"verified":True}


def _python_trees(content):
    if not isinstance(content, str) or not content.strip():
        raise CodingQualityError("coding output has no Python answer")
    blocks = re.findall(r"```(?:python)?\s*\n(.*?)```", content, re.DOTALL)
    candidates = (["\n".join(blocks)] + blocks) if blocks else [content]
    for candidate in candidates:
        try:
            yield ast.parse(candidate.strip())
        except (SyntaxError, ValueError):
            continue


def _sum_terms(node):
    if isinstance(node, ast.BinOp) and isinstance(node.op, ast.Add):
        return _sum_terms(node.left) + _sum_terms(node.right)
    if isinstance(node, ast.Name):
        return [node.id]
    if isinstance(node, ast.Constant) and type(node.value) is int:
        return [node.value]
    raise ValueError("unexpected arithmetic in fixed coding answer")


def _addition(function, offset=0):
    args = function.args
    if (len(args.args) != 2 or args.posonlyargs or args.kwonlyargs or args.vararg
            or args.kwarg or args.defaults or args.kw_defaults or function.decorator_list
            or len({arg.arg for arg in args.args}) != 2):
        return False
    body = function.body
    if (body and isinstance(body[0], ast.Expr) and isinstance(body[0].value, ast.Constant)
            and isinstance(body[0].value.value, str)):
        body = body[1:]
    if len(body) != 1 or not isinstance(body[0], ast.Return):
        return False
    try:
        terms = _sum_terms(body[0].value)
    except ValueError:
        return False
    names = [term for term in terms if isinstance(term, str)]
    return sorted(names) == sorted(arg.arg for arg in args.args) and sum(
        term for term in terms if type(term) is int) == offset


def _assertion(node, name, offset=0, fixed=False):
    if not isinstance(node, ast.Assert) or not isinstance(node.test, ast.Compare):
        return False
    comparison = node.test
    if len(comparison.ops) != 1 or not isinstance(comparison.ops[0], ast.Eq):
        return False
    call, expected = comparison.left, comparison.comparators[0]
    if (not isinstance(call, ast.Call) or not isinstance(call.func, ast.Name)
            or call.func.id != name or len(call.args) != 2 or call.keywords
            or not all(isinstance(arg, ast.Constant) and type(arg.value) in (int, float) for arg in call.args)
            or not isinstance(expected, ast.Constant) or type(expected.value) not in (int, float)):
        return False
    values = [arg.value for arg in call.args]
    return (not fixed or values == [2, 3]) and sum(values) + offset == expected.value


def verify_coding_answer(message):
    """Validate the simple addition task by AST; never execute generated code."""
    for tree in _python_trees(message.get("content")):
        functions = [node for node in tree.body if isinstance(node, ast.FunctionDef) and node.name == "add"]
        if any(_addition(function) for function in functions) and any(
                _assertion(node, "add") for node in tree.body):
            return
    raise CodingQualityError("coding output does not contain the requested addition function and correct assert")


def benchmark_prompt(challenge):
    if not isinstance(challenge, str) or not re.fullmatch(r"[0-9a-f]{64}", challenge):
        raise ValueError("a fixed hexadecimal benchmark challenge is required")
    return ("Return only Python code. Start with the literal assignment request_tag = '" + challenge
        + "'. Then write eight separate functions named add_0 through add_7, each taking a and b. "
        "Function add_i must return a + b + i, with i replaced by its integer suffix. "
        "Write each function explicitly; do not use loops, factories, decorators or imports. "
        "After the functions, write eight asserts, one per function, checking add_i(2, 3) == 5 + i, "
        "with both i and the expected result replaced by literal integers.")


def verify_benchmark_answer(message, challenge):
    benchmark_prompt(challenge)  # Validate before comparing a caller-derived marker.
    for tree in _python_trees(message.get("content")):
        tags = [node for node in tree.body if isinstance(node, ast.Assign)
            and len(node.targets) == 1 and isinstance(node.targets[0], ast.Name)
            and node.targets[0].id == "request_tag"]
        if (len(tags) != 1 or not isinstance(tags[0].value, ast.Constant)
                or tags[0].value.value != challenge):
            continue
        functions = [node for node in tree.body if isinstance(node, ast.FunctionDef)]
        if len(functions) != 8 or len({node.name for node in functions}) != 8:
            continue
        if any(not any(node.name == f"add_{index}" and _addition(node, index)
                for node in functions) for index in range(8)):
            continue
        if any(not any(_assertion(node, f"add_{index}", index, fixed=True)
                for node in tree.body) for index in range(8)):
            continue
        if any(not isinstance(node, (ast.FunctionDef, ast.Assert)) and node is not tags[0]
                for node in tree.body):
            continue
        return
    raise CodingQualityError("benchmark code or per-request response marker is incorrect")


async def performance_generate(challenge, *, longer=False, http=None):
    """Measure the fixed loopback SSE stream including exact delta token IDs."""
    benchmark_prompt(challenge)
    prompt = benchmark_prompt(challenge) if longer else (
        "Write a Python function add(a, b) that returns their sum. Include one assert example. Return the code.")
    request_id = "axiom-benchmark-" + challenge
    payload = {"model": "Qwen3.8-27B-Uncensored", "temperature": 0,
        "max_tokens": 2048 if longer else 1024, "reasoning_effort": "medium",
        "messages": [{"role": "user", "content": prompt}], "stream": True,
        "stream_options": {"include_usage": True}, "return_token_ids": True}
    owns = http is None
    http = http or httpx.AsyncClient(trust_env=False, follow_redirects=False, timeout=600)
    started = time.monotonic()
    content, reasoning = [], []
    first = last = None
    first_chunk_tokens = token_count = body_bytes = 0
    usage = finish = None
    done = False
    try:
        async with http.stream("POST", "http://127.0.0.1:10101/v1/chat/completions",
                headers={"X-Request-Id": request_id}, json=payload) as response:
            if response.status_code != 200 or response.headers.get("x-request-id") != request_id:
                raise RuntimeError("benchmark engine request identity mismatch")
            async for line in response.aiter_lines():
                body_bytes += len(line.encode())
                if body_bytes > 2 * 1024 * 1024:
                    raise ValueError("fixed benchmark response limit exceeded")
                if not line.startswith("data:"):
                    continue
                value = line[5:].strip()
                if value == "[DONE]":
                    done = True
                    break
                chunk = json.loads(value)
                if isinstance(chunk.get("usage"), dict):
                    usage = chunk["usage"]
                choices = chunk.get("choices", [])
                if not choices:
                    continue
                choice = choices[0]
                ids = choice.get("token_ids") or []
                if not isinstance(ids, list) or not all(type(token) is int for token in ids):
                    raise ValueError("invalid benchmark token IDs")
                if ids:
                    tick = time.monotonic()
                    if first is None:
                        first, first_chunk_tokens = tick, len(ids)
                    last = tick
                    token_count += len(ids)
                delta = choice.get("delta", {})
                for name, parts in (("content", content), ("reasoning", reasoning)):
                    text = delta.get(name)
                    if name == "reasoning" and text is None:
                        text = delta.get("reasoning_content")
                    if text is not None:
                        if not isinstance(text, str):
                            raise ValueError("invalid fixed benchmark text")
                        parts.append(text)
                finish = choice.get("finish_reason") or finish
        elapsed = time.monotonic() - started
        if (not done or finish != "stop" or not usage or type(usage.get("completion_tokens")) is not int
                or usage["completion_tokens"] <= 0 or token_count <= 0 or first is None or last is None
                or abs(token_count - usage["completion_tokens"]) > 1):
            raise ValueError("fixed coding benchmark did not finish normally")
        message = {"role": "assistant", "content": "".join(content), "reasoning": "".join(reasoning)}
        if longer:
            verify_benchmark_answer(message, challenge)
        else:
            verify_coding_answer(message)
        decoded = token_count - first_chunk_tokens
        return {"challenge": challenge, "request_id": request_id, "message": message,
            "usage": usage, "finish_reason": finish, "seconds": elapsed,
            "time_to_first_token_seconds": first - started, "streamed_token_count": token_count,
            "first_chunk_tokens": first_chunk_tokens, "decode_seconds": last - first,
            "decode_tokens_per_second": decoded / (last - first) if decoded and last > first else None,
            "engine_request_tokens_per_second": usage["completion_tokens"] / elapsed,
            "timing_scope": "loopback_engine_stream",
            "code_and_request_marker_verified": bool(longer), "production_admission": False}
    finally:
        if owns:
            await http.aclose()


class PriorityGate:
    """Keep image jobs serial and avoid GPU overlap; queued LLMs go first.

    Images get a turn after two seconds of waiting once active LLM jobs drain.
    Running GPU jobs are never preempted. A disconnected caller's owned job
    keeps its slot until completion; queued cancellation removes its ticket.
    """
    def __init__(self, image_grace=0.05):
        # Chutes creates and closes a separate event loop for each cord call.
        # Keep shared synchronization and owned GPU jobs on the startup loop.
        self.owner_loop = asyncio.get_running_loop()
        self.condition = asyncio.Condition()
        self.llm_active = 0
        self.image_active = False
        self.llm_waiting = 0
        self.image_waiting = []
        self.image_grace = image_grace
        self.jobs = set()

    async def run(self, workload, operation):
        if asyncio.get_running_loop() is not self.owner_loop:
            if self.owner_loop.is_closed():
                raise RuntimeError("GPU scheduler event loop is closed")
            pending = asyncio.run_coroutine_threadsafe(self._run(workload, operation), self.owner_loop)
            return await asyncio.wrap_future(pending)
        return await self._run(workload, operation)

    async def stream(self, workload, operation):
        """Bridge bounded output across cord loops; disconnected jobs drain.

        Only the owned producer touches the engine. Cancellation closes the
        subscriber, while an already running GPU task retains its slot.
        """
        import queue
        import threading
        output, detached = queue.Queue(maxsize=32), threading.Event()

        async def emit(item):
            while not detached.is_set():
                try:
                    output.put_nowait(item)
                    return
                except queue.Full:
                    await asyncio.sleep(0.01)

        async def dispatch():
            try:
                async for item in operation():
                    await emit(("data", item))
            except asyncio.CancelledError:
                # Shutdown already stopped the engines. Do not wait for a
                # stalled subscriber to make room for an EOF sentinel.
                detached.set()
                raise
            finally:
                await emit(("eof", None))
            return {}

        pending = asyncio.create_task(self.run(workload, dispatch))
        try:
            while True:
                try:
                    kind, item = await asyncio.to_thread(output.get, True, 0.25)
                except queue.Empty:
                    if pending.done():
                        await pending
                        return
                    continue
                if kind == "eof":
                    await pending
                    return
                yield item
        finally:
            detached.set()
            if not pending.done():
                pending.cancel()
            await asyncio.gather(pending, return_exceptions=True)

    async def _run(self, workload, operation):
        if len(self.jobs) >= 8:
            raise RuntimeError("fixed staging GPU queue is full")
        started = False
        async def owned():
            nonlocal started
            async with self.slot(workload) as queue_seconds:
                started = True
                result = await operation()
                result["queue_seconds"] = queue_seconds
                return result
        task = asyncio.create_task(owned())
        self.jobs.add(task)
        def complete(done):
            self.jobs.discard(done)
            if not done.cancelled():
                done.exception()  # Retrieve errors even after a caller disconnects.
        task.add_done_callback(complete)
        # A disconnected caller must not release a GPU slot while its native
        # image job is still running. The owned task drains independently.
        try:
            return await asyncio.shield(task)
        except asyncio.CancelledError:
            if not started:
                task.cancel()
                await asyncio.gather(task, return_exceptions=True)
            raise

    async def close_after_engines_stopped(self):
        pending = list(self.jobs)
        for task in pending:
            task.cancel()
        await asyncio.gather(*pending, return_exceptions=True)

    @asynccontextmanager
    async def slot(self, workload):
        if asyncio.get_running_loop() is not self.owner_loop:
            raise RuntimeError("GPU slots belong to the persistent startup event loop")
        if workload not in ("llm", "image"):
            raise ValueError("unknown fixed scheduler workload")
        began = time.monotonic()
        acquired = False
        ticket = object()
        if workload == "image":
            await asyncio.sleep(self.image_grace)
        async with self.condition:
            if workload == "llm":
                self.llm_waiting += 1
            else:
                self.image_waiting.append((ticket, began))
            try:
                def allowed():
                    if self.image_active:
                        return False
                    if workload == "llm":
                        image_turn = self.image_waiting and time.monotonic() - self.image_waiting[0][1] >= 2
                        return self.llm_active < 4 and not image_turn
                    image_turn = time.monotonic() - began >= 2
                    return (self.llm_active == 0 and self.image_waiting[0][0] is ticket
                        and (self.llm_waiting == 0 or image_turn))
                while not allowed():
                    # Once the GPU is idle, advancing the image-fairness clock
                    # must wake waiters even if no job remains to notify them.
                    # Busy jobs already notify on completion; avoid polling.
                    seconds = None
                    if self.image_waiting and not self.image_active and self.llm_active == 0:
                        seconds = max(0.001, 2 - (time.monotonic() - self.image_waiting[0][1]))
                    if seconds is None:
                        await self.condition.wait()
                    else:
                        try:
                            await asyncio.wait_for(self.condition.wait(), timeout=seconds)
                        except TimeoutError:
                            self.condition.notify_all()
                if workload == "llm":
                    self.llm_active += 1
                else:
                    self.image_active = True
                acquired = True
            finally:
                if workload == "llm":
                    self.llm_waiting -= 1
                else:
                    self.image_waiting = [item for item in self.image_waiting if item[0] is not ticket]
                self.condition.notify_all()
        try:
            yield time.monotonic() - began
        finally:
            if acquired:
                async with self.condition:
                    if workload == "llm":
                        self.llm_active -= 1
                    else:
                        self.image_active = False
                    self.condition.notify_all()
