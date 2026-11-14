# NEAR worker sessions

Contract: `near-v3`, encryption wire version `2`, attestation
`near-tdx-nvidia-v2`, inference envelope `provider_e2ee_v2`.

This contract uses one freshly attested Ed25519 identity for recipient-key
conversion and response signatures. It replaces NEAR v2's independently fetched
Ed25519/ECDSA reports. The `near-tdx-nvidia-v2` attestation contract covers
direct model workers; `near-tdx-nvidia-gateway-v1` covers the same workers reached
through NEAR's attested cloud gateway (see [Gateway route](#gateway-route)).
`near-tdx-nvidia-gateway-v2` verifies multiple supplied environments sharing the
selected recipient key, while retaining the same E2EE and response authentication.

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

NVIDIA verdict verification independently downloads the vendor's rotating JWKS,
bounded to 1 MiB. The verdict's key ID selects a certificate chain from that set;
the signed-policy anchor, certificate validity, ES384 signature, passing verdict
and challenge nonce must all verify. A larger set of published rotation keys does
not change those checks. Documents above the bound are rejected before parsing.

The native client does **not** claim to have observed the provider's TLS connection:
its security comes from the locally attested encryption key and authenticated
response. The backend's TLS pin keeps routing on the accepted worker. This is an
explicit protocol change, not removal of a check from the old registered profile.

## Routing and lifetime

`provider_worker_sessions` stores account ownership, an immutable verification
snapshot, and expiry (at most five minutes). The client bounds its verified cache
to its own policy and cache lifetime too. Different sessions for one model coexist;
a new worker never overwrites another client's accepted recipient.

The relay context binds `request_body_hash`, `response_signing_address`,
and `worker_session_id`. New clients also send `request_body_base64` and
`response_wire_version: 1`. The backend decodes and structurally compares the
exact JSON to the admitted encrypted fields, model, stream mode and token cap,
rejects duplicate keys and type substitutions, then forwards those bytes without
reserializing. Number spelling, whitespace and Unicode escaping do not affect
admission or change the client-authenticated hash. Legacy clients retain the
previous reconstruction contract. The backend looks up the account-owned session rather than
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
A separate generation-recovery authorization can issue a new ID only after a
confirmed terminal transport failure and charge reconciliation. It never reuses
the dispatch ID, bypasses verification or treats a partial response as complete.
See [generation recovery](../cli.md#generation-recovery).

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

For streaming, raw wire version 1 relays bounded `message.provider_chunk` events
with a run ID, contiguous zero-based index and base64 of the original provider
bytes. The native client parses and decrypts these bytes for provisional output;
the backend parses the same encrypted stream for independent billing and usage
checks. The terminal proof carries `response_wire_version: 1` and the exact
`response_chunk_count` instead of repeating the transcript. Missing, duplicate,
mixed or reordered chunks fail closed. Legacy clients retain terminal transcript
chunks. Non-streaming retains its signed raw response because the receipt must
authenticate provider JSON metadata as well as encrypted fields.

For streaming, the signature covers the complete ordered raw SSE transcript,
including its terminal marker and usage. Deltas remain provisional until the native
client verifies the transcript and receipt; missing, changed, reordered or truncated
responses never produce successful completion.

After receipt/transcript verification, terminal run completion and transport EOF,
the shared provider emits `ResponseVerified`. Downstream encrypted relays require
both that event and the successful stream result before emitting their own
authenticated terminal. Provisional output, a receipt without run completion,
cancellation and a truncated connection never emit that signal.

A non-streaming result may contain authenticated encrypted reasoning without
answer text when its generation budget ends. The relay and native client retain
that reasoning with the provider's `length` finish reason, after the same receipt,
request/response binding and field-authentication checks. An empty result without
reasoning, answer text, refusal or tool calls is still rejected.

Only bounded read-only receipt lookups are retried on transient failures. Invalid
signatures fail immediately. A proven pre-send worker change returns
`ATTESTATION_KEY_CHANGED`, allowing Desktop/TUI to establish and re-encrypt once.
An ambiguous POST failure or missing receipt never authorizes inference replay.

## Gateway route

Attestation contract `near-tdx-nvidia-gateway-v1` keeps `near-v3`, encryption
version `2` and `provider_e2ee_v2`, and routes the same worker through
`https://cloud-api.near.ai/v1`. NEAR serves some models (for example preview
models) only on this route. The gateway's attestation report carries its own
`gateway_attestation` and the selected worker's `model_attestations` entry, both
answering the client's challenge.

The backend pins its live TLS connection to the **gateway's** attested TLS key
(`SHA256(gateway_ed25519_key || gateway_tls_spki_sha256) || nonce` in its TDX
report data), verifies the gateway quote and workload manifest, and verifies the
worker exactly as on the direct route except for the live TLS check. Requests
carry the worker key in `X-Model-Pub-Key` so the gateway forwards to that worker.
The evidence relayed to the client adds a `gateway` object with the gateway's
key, nonce, TLS fingerprint, Intel quote and workload manifest.

The native client requires that object exactly for this contract and rejects it
for the direct contract. It verifies the gateway quote with the same Intel path,
report-data binding (gateway key, gateway TLS key, nonce) and manifest binding as
the worker, under the same Intel status policy, alongside the worker's Intel and
NVIDIA evidence. Message ciphertext is still encrypted to the attested worker key,
so the gateway forwards only ciphertext. Receipts are unchanged: NEAR's gateway
returns the worker's Ed25519 signature over
`model_id:sha256(exact_request_bytes):sha256(exact_response_bytes)` for both
streaming and non-streaming requests, which the client and backend verify against
the attested worker key. A misrouted request cannot decrypt or produce a valid
receipt and fails closed. The gateway, like the backend, observes request sizes
and timing but no message content.

## Contract compatibility

Native and backend releases must implement the same registered `near-v3`
contract. Unsupported protocol offers fail closed.

Updated clients prefer advertised `near-tdx-nvidia-gateway-v2` over v1. The server
retains the v1 default for older clients; the native engine uses v1 if v2 is not
advertised. V1 continues to require a single selected worker.

V2 requires a `worker_attestations` array of one to sixteen normalized worker
reports. Its first member must match the top-level selected worker. Every member
must name the same recipient key, model and fresh challenge. The client verifies
each Intel quote, exact measured workload manifest, TLS/key binding and NVIDIA
evidence independently. A bad second entry rejects the whole group before any
session is cached or inference encrypted. The gateway quote is also required and
verified. The explicit, runtime-only `OutOfDate` exception applies separately to
each TDX environment; other failures remain rejected.

Evidence exposes additional worker checks, measurements, TLS fingerprints and
manifest digests. For v2, the workload-manifest field is a JSON bundle containing
the exact manifest strings for all verified workers and the gateway. Any accepted
outdated environment produces degraded evidence. The gateway preflight accepts
only the group's previously verified model/TLS/manifest bindings; a new same-key
environment forces fresh establishment. Requests and signed response receipts
remain bound to the selected key, not the array position.

NEAR source confirms [Ed25519 receipts](https://github.com/nearai/inference-proxy/blob/85ff2c493f84c835e175be74a319a8ab583a7fd3/src/routes/signature.rs)
and documents [worker-local signature records](https://docs.near.ai/cloud/verification/chat).

## Direct image inputs

For discovered image-capable models, text and original inline image data URLs
are serialized as an OpenAI content array and then encrypted together inside
NEAR's ordinary content field. The attested enclave decrypts and restores the
array. Request/response signatures still cover the exact encrypted transcript.
No file bytes are exposed to the Axiom relay. General document uploads are not
advertised for NEAR, and unsupported attachments fail before inference.
