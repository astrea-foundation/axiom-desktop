# NEAR worker sessions

Contract: `near-v3`, encryption wire version `2`, attestation
`near-tdx-nvidia-v2`, inference envelope `provider_e2ee_v2`.

This contract uses one freshly attested Ed25519 identity for recipient-key
conversion and response signatures. It replaces NEAR v2's independently fetched
Ed25519/ECDSA reports. Only direct model workers are supported by this contract.

## Establishment

The native client creates a random 32-byte challenge and calls the authenticated
`GET /api/v1/provider/attestation/report` with `nonce`, `model_id`, and the exact
three contract selectors. The backend opens a direct worker connection and
requests one Ed25519 report with that challenge and `include_tls_fingerprint=true`.
It verifies Intel TDX, NVIDIA evidence, the measured workload manifest, and the
report's binding to the TLS SPKI on that exact connection.

The response's `evidence` contains the raw quote, GPU evidence, workload manifest,
normalized key/TLS/challenge fields, a `worker_session_id`, and a `lease`.
The lease binds the model, endpoint, protocol, generation, key fingerprint,
response-signing key, and hard expiry. Both keys and fingerprints must agree.

The native client independently verifies the vendor evidence and its own nonce.
The TDX report data is `SHA256(ed25519_key_bytes || tls_spki_sha256_bytes) || nonce`.
The measured configuration binds the exact manifest; NVIDIA verification must
pass for the same nonce. Intel status defaults to the signed local `UpToDate` policy. A user may explicitly
accept the exact `OutOfDate` status for NEAR until the native runtime restarts.
The client then sends `allow_outdated_tcb=true` on establishment and independently
permits that status only after all quote, key, nonce, workload and GPU checks pass.
Accepted evidence is marked `degraded`, with the real Intel status and advisories;
all other rejected statuses and authentication failures still fail closed. The
backend records `outdated_tcb_accepted` on that account-bound worker session and
keeps its original expiry and TLS pin. Consent never extends a lease or changes
the signed trust-policy document.
The server's `verified` flag and supplied collateral are not trust roots.

The native client does **not** claim to have observed the provider's TLS connection:
its security comes from the locally attested encryption key and authenticated
response. The backend's TLS pin keeps routing on the accepted worker. This is an
explicit protocol change, not removal of a check from the old registered profile.

## Routing and lifetime

`provider_worker_sessions` stores account ownership, an immutable verification
snapshot, and expiry (at most five minutes). The client bounds its verified cache
to its own policy and cache lifetime too. Different sessions for one model coexist;
a new worker never overwrites another client's accepted recipient.

The relay context contains exactly `request_body_hash`, `response_signing_address`,
and `worker_session_id`. The backend looks up the account-owned session rather than
the model's latest lease. Expired/mismatched sessions fail before provider dispatch.

A bounded in-process connection cache is only an optimization. Another API process
can load the snapshot and reconnect, but every TLS handshake must match its SPKI
before HTTP headers or ciphertext are written. A public preflight GET checks the
accepted worker before billing admission. The HTTP transport versions are pinned,
and real-socket tests cover certificate rotation and zero request bytes to a wrong
worker. Redirects, environment proxies, and HTTP/2 pooling are disabled here.
The durable dispatch record is written after the TLS pin check, immediately before
the POST headers. A rejected connection releases its unused billing hold and request
ID, so the existing one-time retry can reuse that ID. Once dispatch is recorded,
an ambiguous send or response failure retains the ID and cannot replay inference.

## Inference and completion

Messages, reasoning, tools, and tool results retain NEAR's v2 field encryption:
Ed25519-to-X25519 conversion, ephemeral X25519 ECDH, HKDF-SHA256 with
`ed25519_encryption`, and XChaCha20-Poly1305. Ciphertext is hex-encoded
`ephemeral_public_key || nonce || ciphertext_and_tag`. Provider credentials stay
on the backend. Prompts and plaintext responses stay on the native client.

After completion the backend requests `/signature/{chat_id}?signing_algo=ed25519`
using the accepted worker connection. Both backend and native client verify the
Ed25519 signature against the attested model key. Signed text must be exactly
`model_id:sha256(exact_request_bytes):sha256(exact_response_bytes)`.
ECDSA records and gateway-only hash pairs are rejected under this contract.

For streaming, the signature covers the complete ordered raw SSE transcript,
including its terminal marker and usage. Deltas remain provisional until the native
client verifies the transcript and receipt; missing, changed, reordered or truncated
responses never produce successful completion.

Only bounded read-only receipt lookups are retried on transient failures. Invalid
signatures fail immediately. A proven pre-send worker change returns
`ATTESTATION_KEY_CHANGED`, allowing Desktop/TUI to establish and re-encrypt once.
An ambiguous POST failure or missing receipt never authorizes inference replay.

## Contract compatibility

Native and backend releases must implement the same registered `near-v3`
contract. Unsupported protocol offers fail closed.

NEAR source confirms [Ed25519 receipts](https://github.com/nearai/inference-proxy/blob/85ff2c493f84c835e175be74a319a8ab583a7fd3/src/routes/signature.rs)
and documents [worker-local signature records](https://docs.near.ai/cloud/verification/chat).

## Direct image inputs

For discovered image-capable models, text and original inline image data URLs
are serialized as an OpenAI content array and then encrypted together inside
NEAR's ordinary content field. The attested enclave decrypts and restores the
array. Request/response signatures still cover the exact encrypted transcript.
No file bytes are exposed to the Axiom relay. General document uploads are not
advertised for NEAR, and unsupported attachments fail before inference.
