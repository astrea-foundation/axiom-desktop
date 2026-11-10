"""Bounded JSON/base64 metadata parsing. Inference uses the Rust EHBP codec."""

import base64
import json

MAX_METADATA = 8 * 1024 * 1024


def unique(items):
    result = {}
    for key, value in items:
        if key in result:
            raise ValueError("duplicate JSON field")
        result[key] = value
    return result


def decode_json(raw, limit=MAX_METADATA):
    if not isinstance(raw, bytes) or len(raw) > limit:
        raise ValueError("metadata size limit")
    def no_constant(_):
        raise ValueError("invalid JSON number")
    return json.loads(raw, object_pairs_hook=unique, parse_constant=no_constant)


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), allow_nan=False).encode()


def unbase64(value, limit=MAX_METADATA):
    if not isinstance(value, str) or len(value) > (limit + 2) // 3 * 4:
        raise ValueError("base64 size limit")
    raw = base64.b64decode(value, validate=True)
    if len(raw) > limit or base64.b64encode(raw).decode() != value:
        raise ValueError("noncanonical base64")
    return raw
