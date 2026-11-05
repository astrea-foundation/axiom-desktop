"""One metadata-only, freshly verified health request per active Chutes worker.

Scheduling belongs to the operator's server. Importing this module never starts
a task, changes a deployment, renews a preview heartbeat, or runs inference.
"""
import argparse
import asyncio
from dataclasses import dataclass
import json
import os
from pathlib import Path
import stat
import sys
import time
from urllib.parse import urlsplit

from .client import ChutesClient, identifier, read_limited
from .publisher import PublisherAuthority, verify_files
from .release import read_file, require
from .wire import decode_json, open_response, seal, unbase64

SCHEMA = "axiom-chutes-keepalive-v1"
HEALTH_LIMIT = 64 * 1024


@dataclass(frozen=True)
class KeepaliveConfig:
    chute_id: str
    api_key_file: Path
    authority_file: Path
    state_file: Path
    expected_interval_seconds: int
    health_field: str
    release_source: dict

    def summary(self):
        return {"chute_id": self.chute_id,
                "expected_interval_seconds": self.expected_interval_seconds,
                "health_field": self.health_field,
                "inference_requested": False, "deployment_modified": False}


def absolute_path(value):
    require(isinstance(value, str) and value and Path(value).is_absolute()
            and ".." not in Path(value).parts, "absolute installed path required")
    return Path(value)


def load_config(path):
    value = decode_json(read_file(path, 16 * 1024), 16 * 1024)
    fields = {"schema", "chute_id", "api_key_file", "authority_file", "state_file",
              "expected_interval_seconds", "health_field"}
    require(isinstance(value, dict) and set(value) in (
        fields | {"release_directory"}, fields | {"metadata_url"}), "exact keepalive configuration required")
    require(value["schema"] == SCHEMA, "unsupported keepalive configuration")
    chute_id = identifier(value["chute_id"])
    interval = value["expected_interval_seconds"]
    require(type(interval) is int and 60 <= interval <= 86400, "keepalive interval must be 60..86400 seconds")
    require(value["health_field"] in ("ready", "staging_ready"), "explicit health readiness field required")
    source = {}
    if "release_directory" in value:
        source["directory"] = str(absolute_path(value["release_directory"]))
    else:
        url = value["metadata_url"]
        require(isinstance(url, str) and len(url) <= 2048, "fixed publisher URL required")
        parsed = urlsplit(url)
        require(parsed.scheme == "https" and parsed.hostname and not parsed.username
                and not parsed.password and not parsed.query and not parsed.fragment,
                "fixed HTTPS publisher URL required")
        source["metadata_url"] = url
    return KeepaliveConfig(chute_id, absolute_path(value["api_key_file"]),
        absolute_path(value["authority_file"]), absolute_path(value["state_file"]),
        interval, value["health_field"], source)


def read_key(path):
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(descriptor, "rb") as stream:
        info = os.fstat(stream.fileno())
        require(stat.S_ISREG(info.st_mode) and not stat.S_IMODE(info.st_mode) & 0o077,
                "private regular API key file required")
        raw = stream.read(4097)
    require(0 < len(raw) <= 4096, "bounded API key required")
    return raw.decode("ascii").strip()


def active_instances(metadata, config, *, allow_empty=False):
    require(isinstance(metadata, dict) and metadata.get("chute_id") == config.chute_id
            and metadata.get("public") is False and metadata.get("tee") is True
            and metadata.get("preemptible") is False and metadata.get("disabled") is False,
            "enabled private non-preemptible TEE deployment required")
    idle = metadata.get("shutdown_after_seconds")
    require(type(idle) is int and idle >= 2 * config.expected_interval_seconds,
            "idle timeout must be at least twice the scheduled keepalive interval")
    rows = metadata.get("instances")
    require(isinstance(rows, list) and len(rows) <= 100, "bounded instance inventory required")
    active = set()
    for row in rows:
        require(isinstance(row, dict), "instance metadata required")
        if row.get("active") is True and row.get("verified") is True:
            identity = identifier(row.get("instance_id"))
            require(identity not in active, "duplicate active worker")
            active.add(identity)
    require((0 if allow_empty else 1) <= len(active) <= 5,
            "one to five active verified workers required")
    return active, idle


async def ping(client, instance, release, health_field):
    hardware = await client.attest_hardware(instance, release.hardware["measurements"],
                                            release.hardware["nvidia_anchors"])
    require(hardware.instance == instance, "verified recipient differs from discovered worker")
    # Verification can use most of a discovery nonce's lifetime. Refresh only
    # for this exact worker/key; a changed key requires a fresh scheduled run.
    hardware = await client.refresh_invocation_nonce(hardware)
    row = hardware.instance
    require((row.chute_id, row.instance_id, row.public_key)
            == (instance.chute_id, instance.instance_id, instance.public_key),
            "refreshed nonce selects another verified recipient")
    require(hardware.expires_at > time.time() and row.nonce_expires_at > time.time(),
            "fresh hardware proof and invocation nonce required")
    require(release.expires_at > time.time() + 30, "publisher policy expires too soon for health request")
    client._take_nonce(row)  # Consume before dispatch; an ambiguous call is never replayed.
    blob, response_key = seal(unbase64(row.public_key), {})
    headers = {**client._headers, "X-Chute-Id": row.chute_id, "X-Instance-Id": row.instance_id,
               "X-E2E-Nonce": row.invocation_nonce, "X-E2E-Path": "/health",
               "X-E2E-Stream": "false", "Content-Type": "application/octet-stream"}
    async with asyncio.timeout(30):
        async with client._http.stream("POST", client.invocation_url, headers=headers, content=blob) as response:
            require(response.status_code == 200, "encrypted worker health request failed")
            health = open_response(await read_limited(response, HEALTH_LIMIT), response_key, limit=HEALTH_LIMIT)
    require(hardware.expires_at > time.time(), "health evidence expired before completion")
    require(release.expires_at > time.time(), "publisher policy expired before health completion")
    require(isinstance(health, dict) and health.get(health_field) is True, "worker health check failed")
    return {"instance_id": row.instance_id, "healthy": True, "hardware_verified": True,
            "checked_at_epoch": int(time.time())}


async def run_once(config, *, warm_if_missing=False):
    authority = PublisherAuthority.load(config.authority_file)
    source = {**config.release_source, "state": str(config.state_file)}
    release = await asyncio.to_thread(verify_files, source, authority)
    require(release.chute_id == config.chute_id, "signed release selects another deployment")
    client = ChutesClient(read_key(config.api_key_file))
    try:
        metadata = await client._json("GET", "/chutes/" + config.chute_id)
        active, idle = active_instances(metadata, config, allow_empty=warm_if_missing)
        if not active:
            # Explicit operator opt-in: request capacity for this same signed
            # private registration, without enabling or replacing it. A
            # pending instance is left to finish its startup. Neither status
            # claims health or authorizes inference.
            requested = not metadata["instances"]
            if requested:
                await client._json("GET", "/chutes/warmup/" + config.chute_id,
                                   params={"quick": "true"})
            return {"status": "waiting_for_gpu" if requested else "starting_worker",
                    **config.summary(), "channel": release.channel,
                    "idle_timeout_seconds": idle, "warmup_requested": requested,
                    "workers": []}
        rows = await client.discover(config.chute_id)
        selected = {}
        for row in rows:
            if row.instance_id in active:
                previous = selected.setdefault(row.instance_id, row)
                require(previous.public_key == row.public_key, "ambiguous worker recipient key")
        require(set(selected) == active, "active worker disappeared from encrypted routing")
        # Keep each instance warm, rather than letting a load balancer deliver
        # every health call to the same replica. No GPU job or credit reservation.
        reports = await asyncio.gather(*(ping(client, selected[identity], release, config.health_field)
                                         for identity in sorted(selected)), return_exceptions=True)
        # Wait for all attempts before closing the shared transport, including
        # when one worker rejects verification. Health checks carry no job data.
        for report in reports:
            if isinstance(report, BaseException):
                raise report
        return {"status": "healthy", **config.summary(), "channel": release.channel,
                "idle_timeout_seconds": idle, "workers": reports}
    finally:
        await client.close()


async def run_command(args):
    config = load_config(args.config)
    if args.check_config:
        authority = PublisherAuthority.load(config.authority_file)
        return {"status": "configuration_valid", **config.summary(), "channel": authority.channel,
                "network_requests": 0, "credentials_read": False}
    return await run_once(config, warm_if_missing=getattr(args, "warm_if_missing", False))


def add_arguments(parser):
    parser.add_argument("--config", type=Path, required=True)
    parser.add_argument("--check-config", action="store_true",
                        help="validate local public settings without network calls or reading the API key")
    parser.add_argument("--warm-if-missing", action="store_true",
                        help="request capacity for the same enabled signed deployment when it has no instances")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    add_arguments(parser)
    args = parser.parse_args()
    try:
        print(json.dumps(asyncio.run(run_command(args))), flush=True)
    except KeyboardInterrupt:
        sys.exit(130)
    except Exception as error:
        # Do not expose provider bodies, URLs with credentials, or key data.
        print(json.dumps({"status": "failed", "error_type": type(error).__name__,
                          "inference_requested": False, "deployment_modified": False}), file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    main()
