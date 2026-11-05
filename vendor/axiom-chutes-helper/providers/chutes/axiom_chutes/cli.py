"""Read-only diagnostics and explicit keep-warm operation."""

import argparse
import asyncio
import json
import os
from pathlib import Path
import re
import sys
import time

from .client import API, ChutesClient, identifier
from .config import load_deployment
from .wire import decode_json


def credentials(path):
    if path:
        file = Path(path)
        if file.is_symlink() or file.stat().st_mode & 0o077:
            raise ValueError("credential file must be private (mode 0600)")
        raw = file.read_bytes()
        if len(raw) > 4096:
            raise ValueError("credential size limit")
        return raw.decode().strip()
    value = os.environ.get("CHUTES_API_KEY")
    if not value:
        raise ValueError("set CHUTES_API_KEY or use --api-key-file; no deployment has been made")
    return value


async def run(args):
    if args.command == "keepalive":
        from .keepalive import run_command
        print(json.dumps(await run_command(args)), flush=True)
        return
    if args.command == "plan":
        value = load_deployment(args.deployment)
        print(json.dumps({"name": value["name"], "base_worker_image": value["base_worker_image"],
                          "gpu": "pro_6000", "gpu_count": 1, "tee": True,
                          "max_instances": 1, "shutdown_after_seconds": 604800,
                          "production_admission": "blocked_pending_workload_key_binding",
                          "deploy_command": "chutes deploy deploy.chutes.chute:chute --accept-fee"}, indent=2))
        return
    client = ChutesClient(credentials(args.api_key_file))
    try:
        if args.command == "discover":
            rows = await client.discover(identifier(args.chute_id))
            unique = {r.instance_id: r for r in rows}
            print(json.dumps({"instances": [{"instance_id": r.instance_id,
                "recipient_sha256": __import__("hashlib").sha256(r.public_key.encode()).hexdigest()}
                for r in unique.values()], "attestation_verified": False}, indent=2))
        elif args.command == "probe":
            trust = decode_json(Path(args.trust_file).read_bytes(), 128 * 1024)
            rows = await client.discover(identifier(args.chute_id))
            reports, seen = [], set()
            for instance in rows:
                if instance.instance_id in seen:
                    continue
                seen.add(instance.instance_id)
                result = await client.attest_hardware(instance, trust["measurements"], trust["nvidia_anchor_sha256"])
                reports.append({"instance_id": instance.instance_id, "quote_sha256": result.quote_sha256,
                                "expires_at": result.expires_at, "hardware_verified": True,
                                "workload_binding_verified": False, "worker_key_custody_verified": False,
                                "pool_admission": False})
            print(json.dumps({"reports": reports}, indent=2))
        elif args.command == "keep-warm":
            ids = [identifier(v) for v in args.chute_id]
            if len(set(ids)) != len(ids) or not 1 <= len(ids) <= 8 or not 15 <= args.interval <= 60:
                raise ValueError("one to eight distinct chutes and interval 15..60 required")
            if not args.forever and not 1 <= args.duration_minutes <= 120:
                raise ValueError("duration must be 1..120 minutes, or explicitly use --forever")
            until = None if args.forever else time.monotonic() + args.duration_minutes * 60
            while True:
                # No retry, model invocation, deployment, or auto-creation.
                # Successful metadata cords count as use in the current provider
                # implementation. This remains best effort, not an uptime SLA.
                for chute_id in ids:
                    metadata = await client._json("GET", f"/chutes/{chute_id}")
                    if metadata.get("public") is not False or metadata.get("preemptible") is not False or metadata.get("tee") is not True or metadata.get("max_instances") != 1:
                        raise ValueError("keep-warm requires a private non-preemptible single-instance TEE chute")
                    slug = metadata.get("slug", "")
                    if not isinstance(slug, str) or not re.fullmatch(r"[a-z0-9][a-z0-9-]{0,61}[a-z0-9]", slug):
                        raise ValueError("invalid Chutes public route")
                    # The generic /chutes/{id}/{cord} endpoint is the SDK's
                    # serialized RPC, not its public JSON API. Use the actual
                    # public cord and never follow a metadata-supplied URL.
                    async with client._http.stream("POST", f"https://{slug}.chutes.ai/health",
                                                   headers=client._headers, json={}) as response:
                        if response.status_code != 200:
                            raise RuntimeError("Chutes worker health request failed")
                        from .client import read_limited
                        result = decode_json(await read_limited(response, 16 * 1024))
                    if result.get("ready") is not True:
                        raise RuntimeError("worker is not admitted/healthy; keep-warm stopped")
                print(json.dumps({"healthy_chutes": ids, "checked_at": int(time.time())}), flush=True)
                if args.once or (until is not None and time.monotonic() + args.interval >= until):
                    break
                await asyncio.sleep(args.interval)
    finally:
        await client.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--api-key-file")
    sub = parser.add_subparsers(dest="command", required=True)
    plan = sub.add_parser("plan", help="validate public config without deploying")
    plan.add_argument("deployment")
    for name in ("discover", "probe"):
        command = sub.add_parser(name)
        command.add_argument("--chute-id", required=True)
        if name == "probe":
            command.add_argument("--trust-file", required=True)
    warm = sub.add_parser("keep-warm", help="explicit billable metadata calls to already-admitted workers")
    warm.add_argument("--chute-id", action="append", required=True)
    warm.add_argument("--interval", type=int, default=30)
    warm.add_argument("--duration-minutes", type=int, default=120)
    warm.add_argument("--once", action="store_true")
    warm.add_argument("--forever", action="store_true", help="explicit continuous billable keepalive until stopped")
    keepalive = sub.add_parser("keepalive", help="one encrypted, freshly verified health call per active worker")
    from .keepalive import add_arguments
    add_arguments(keepalive)
    args = parser.parse_args()
    try:
        asyncio.run(run(args))
    except KeyboardInterrupt:
        sys.exit(130)
    except Exception:
        # Provider error bodies and exception strings can contain request or
        # credential data. Keep public diagnostics bounded and content-free.
        print("Chutes operation rejected; check credentials, pins, admission, and capacity. No automatic replay.", file=sys.stderr)
        sys.exit(1)
