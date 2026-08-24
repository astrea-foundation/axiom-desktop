#!/usr/bin/env python3
"""Minimal ACP client for the protected direct AxiomCLI live lane."""

from __future__ import annotations

import json
import select
import subprocess
import sys
import time
from pathlib import Path


def fail(message: str) -> None:
    raise RuntimeError(message)


def main() -> None:
    if len(sys.argv) != 6:
        fail("expected AXIOMCLI WORKSPACE PROMPT OUTPUT STDERR")
    binary, workspace, prompt, output_path, stderr_path = sys.argv[1:]
    stderr_handle = open(stderr_path, "w", encoding="utf-8")
    process = subprocess.Popen(
        [binary, "acp"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=stderr_handle,
        text=True,
        bufsize=1,
    )
    assert process.stdin is not None
    assert process.stdout is not None
    messages: list[dict[str, object]] = []

    def send(value: dict[str, object]) -> None:
        process.stdin.write(json.dumps(value, separators=(",", ":")) + "\n")
        process.stdin.flush()

    def receive(deadline: float) -> dict[str, object]:
        remaining = deadline - time.monotonic()
        if remaining <= 0 or not select.select([process.stdout], [], [], remaining)[0]:
            fail("timed out waiting for ACP output")
        line = process.stdout.readline()
        if not line:
            fail(f"unexpected ACP EOF with status {process.poll()}")
        value = json.loads(line)
        if not isinstance(value, dict):
            fail("ACP message was not an object")
        messages.append(value)
        return value

    def await_response(identifier: int, deadline: float) -> dict[str, object]:
        while True:
            value = receive(deadline)
            if value.get("id") == identifier:
                return value

    deadline = time.monotonic() + 300
    send({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": 1,
            "clientCapabilities": {},
            "clientInfo": {"name": "axiomcli-live-gate", "version": "1"},
        },
    })
    initialized = await_response(1, deadline)
    if initialized.get("result", {}).get("protocolVersion") != 1:
        fail("AxiomCLI did not negotiate ACP v1")

    send({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "session/new",
        "params": {"cwd": str(Path(workspace).resolve()), "mcpServers": []},
    })
    session_response = await_response(2, deadline)
    session_id = session_response.get("result", {}).get("sessionId")
    if not isinstance(session_id, str) or not session_id:
        fail("AxiomCLI returned no ACP session ID")

    send({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "session/prompt",
        "params": {
            "sessionId": session_id,
            "prompt": [{"type": "text", "text": prompt}],
        },
    })
    approvals = 0
    while True:
        value = receive(deadline)
        if value.get("method") == "session/request_permission":
            options = value.get("params", {}).get("options", [])
            option_ids = {
                option.get("optionId") for option in options if isinstance(option, dict)
            }
            if "allow_once" not in option_ids:
                fail("ACP permission request omitted allow_once")
            approvals += 1
            value["liveGateSelectedOption"] = "allow_once"
            send({
                "jsonrpc": "2.0",
                "id": value["id"],
                "result": {
                    "outcome": {"outcome": "selected", "optionId": "allow_once"}
                },
            })
        if value.get("method") == "elicitation/create":
            send({
                "jsonrpc": "2.0",
                "id": value["id"],
                "result": {"action": "decline"},
            })
        if value.get("id") == 3:
            result = value.get("result", {})
            if result.get("stopReason") != "end_turn":
                fail(f"ACP prompt did not complete normally: {value}")
            break

    process.stdin.close()
    try:
        status = process.wait(timeout=15)
    except subprocess.TimeoutExpired:
        process.terminate()
        fail("AxiomCLI did not exit after ACP EOF")
    stderr_handle.close()
    if status != 0:
        fail(f"AxiomCLI exited with status {status}")

    transcript = json.dumps(messages, separators=(",", ":"))
    if '"status":"completed"' not in transcript:
        fail("ACP events contained no completed tool")
    if "Verifying model attestation and secure endpoint binding" not in transcript:
        fail("ACP events contained no secure verification status")
    if approvals:
        messages.append({"liveGateApprovals": approvals, "selected": "allow_once"})
    with open(output_path, "w", encoding="utf-8") as output:
        for value in messages:
            output.write(json.dumps(value, separators=(",", ":")) + "\n")


if __name__ == "__main__":
    main()
