"""Relocatable, public native configuration for the managed Chutes helper."""
import os
from pathlib import Path
import sys

from axiom_chutes.publisher import PublisherAuthority
from axiom_chutes.release import read_file, require
from axiom_chutes.wire import decode_json


def installed_path(value, directory):
    require(isinstance(value, str) and value, "installed path required")
    path = Path(value)
    if path.is_absolute():
        return path
    require(all(part not in (".", "..") for part in path.parts) and path.as_posix() == value,
            "canonical installed relative path required")
    path = (directory / path).resolve(strict=True)
    require(path.is_relative_to(directory.resolve()), "installed path escapes its bundle")
    return path


def bridge_config(path):
    path = Path(path)
    value = decode_json(read_file(path, 64 * 1024), 64 * 1024)
    require(isinstance(value, dict), "native bridge configuration required")
    if value.get("hosted_relay") is True:
        require(set(value) <= {"hosted_relay", "chute_id", "release_verification_config", "tls_ca_file"},
                "hosted client configuration must not contain a provider credential")
        value["release_verification_config"] = str(installed_path(value["release_verification_config"], path.parent))
    return value


def hosted_release_config(path):
    path = Path(path)
    value = decode_json(read_file(path, 64 * 1024), 64 * 1024)
    require(isinstance(value, dict) and set(value) in ({"authority", "state"}, {"authority", "user_state"}),
            "installed release authority and rollback location required")
    authority_path = installed_path(value["authority"], path.parent)
    authority = PublisherAuthority.load(authority_path)
    if "user_state" in value:
        require(value["user_state"] is True, "explicit private user rollback state required")
        if sys.platform == "darwin":
            directory = Path.home() / "Library/Application Support/Axiom/chutes"
        elif sys.platform.startswith("linux"):
            state_root = Path(os.environ.get("XDG_STATE_HOME", str(Path.home() / ".local/state")))
            require(state_root.is_absolute(), "absolute XDG state root required")
            directory = state_root / "axiom/chutes"
        elif sys.platform == "win32":
            state_root = Path(os.environ.get("LOCALAPPDATA", str(Path.home() / "AppData/Local")))
            require(state_root.is_absolute(), "absolute Windows state root required")
            directory = state_root / "Axiom/chutes"
        else:
            raise ValueError("native helper platform is not qualified")
        state = directory / authority.identity / "publisher.json"
    else:
        state = Path(value["state"])
        require(state.is_absolute(), "absolute private rollback state required")
    return authority, state
