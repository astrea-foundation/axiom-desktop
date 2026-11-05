"""Bounded public metadata reads; no proxy or attester-selected verifier URLs."""

from .strict import require


async def read_limited(response, limit):
    out = bytearray()
    async for chunk in response.aiter_bytes():
        out.extend(chunk)
        require(len(out) <= limit, "metadata response limit")
    return bytes(out)
