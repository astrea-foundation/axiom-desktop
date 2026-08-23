# NEAR provider E2EE v2 protocol

Status: implementation contract for AxiomAI secure inference

Protocol identifier: `near-v2`

Encryption version: `2`

Attestation protocol: `near-tdx-nvidia-v1`

Authoritative source snapshot: Axiom backend commit
`114be9a4061e8b1a9049f1e31b7b05c52a694c50`

Compatibility reference: Axiom proxy commit
`29efe06c6b6598ee94b81d47c5b0409d99974c15`

## 1. Security goal

The local client constructs model input, verifies the selected provider's TEE
evidence, encrypts every secret-bearing field to the model key authenticated by
that evidence, and decrypts provider output locally. The Axiom relay handles
ciphertext and non-secret routing/accounting metadata only.

TLS authenticates and protects the network connection, but TLS alone does not
satisfy this protocol. Failure to verify attestation, bind its key, or establish
E2EE terminates the request.

## 2. Key material

The provider attests a 32-byte Ed25519 public key encoded as 64 lowercase hex
characters. A leading `0x` may be accepted on input but is removed during
canonicalization.

For every inference request, the client creates a fresh Ed25519 signing key from
a 32-byte OS-CSPRNG seed. It publishes only the 32-byte Ed25519 public key. The
client converts `seed || public_key` to an X25519 secret as specified below and
keeps that secret only for the lifetime of the request.

Client seeds, Ed25519 secret material, X25519 secret material, shared secrets,
and derived AEAD keys are never persisted or logged.

## 3. Ed25519 to X25519 conversion

Public conversion is byte-compatible with libsodium
`crypto_sign_ed25519_pk_to_curve25519`: decompress the Ed25519 Edwards-Y point,
reject an invalid point, convert it to Montgomery form, and serialize 32 bytes.

Secret conversion is byte-compatible with libsodium
`crypto_sign_ed25519_sk_to_curve25519`:

1. Require exactly 64 bytes formatted as `seed(32) || public_key(32)`.
2. Compute SHA-512 over `seed` only.
3. Take bytes `0..32`.
4. Clamp: `out[0] &= 248`, `out[31] &= 127`, `out[31] |= 64`.

## 4. Encryption

For every encrypted field:

1. Convert the recipient's Ed25519 public key to X25519.
2. Generate a fresh ephemeral X25519 secret using the OS CSPRNG.
3. Compute X25519 ECDH with the recipient public key.
4. Derive 32 bytes with HKDF-SHA256:
   - salt: absent (`None`, equivalent to the RFC 5869 zero salt);
   - input key material: the 32-byte ECDH shared secret;
   - info: ASCII bytes `ed25519_encryption`;
   - output length: 32 bytes.
5. Generate a fresh 24-byte nonce using the OS CSPRNG.
6. Encrypt the exact UTF-8 bytes using XChaCha20-Poly1305-IETF with no AAD.

The wire payload is:

```text
ephemeral_x25519_public_key (32 bytes)
|| xchacha_nonce             (24 bytes)
|| ciphertext_and_tag        (plaintext length + 16 bytes)
```

The wire payload is encoded as lowercase hex. A valid decoded payload must be
longer than 56 bytes. Authentication failure, invalid hex, invalid key material,
or invalid UTF-8 after response decryption is terminal.

Every field is independently sealed with a fresh ephemeral key and nonce. No
nonce, ephemeral key, or ciphertext is reused as protocol state.

## 5. TDX report-data binding

All inputs below are decoded raw bytes, not their hex-string bytes:

```text
binding_hash = SHA256(model_ed25519_public_key || tls_spki_sha256)

report_data[0..32]  == binding_hash
report_data[32..64] == request_nonce
```

The signing key, TLS SPKI fingerprint, and nonce must each decode to exactly 32
bytes. `report_data` must contain at least 64 bytes. Extra report-data bytes are
not interpreted by protocol v2.

## 6. Attestation request and verification

The client generates a fresh 32-byte nonce and requests:

```text
GET {model_base_url}/attestation/report
    ?signing_algo=ed25519
    &nonce={64 lowercase hex characters}
    &include_tls_fingerprint=true
```

The NEAR multi-model gateway also receives `model={upstream_model}`. Per-model
hosts do not require that selector. Redirects are disabled for this request.

From the exact TLS connection carrying the successful report response, the
client hashes the leaf certificate's DER-encoded SubjectPublicKeyInfo with
SHA-256. A separate probe connection is not an acceptable substitute.

The client accepts a report only when:

- `signing_algo` is exactly `ed25519`;
- `signing_public_key`, or the documented legacy `signing_address` alias, is a
  valid 32-byte key;
- the echoed nonce equals the requested nonce;
- `model_name`, when present, equals the selected upstream model;
- the report's TLS fingerprint equals the exact live SPKI hash;
- Intel DCAP QVL validates the TDX quote and returns `UpToDate` or
  `SWHardeningNeeded` under trust-policy version 1;
- the TDX report data has the binding in Section 5;
- NVIDIA NRAS verifies the GPU evidence and binds the same request nonce;
- the NVIDIA result status is exactly `verified`;
- the provider catalog's provider, E2EE protocol/version, and attestation
  protocol match the compiled NEAR driver.

Trust-policy version 1 proves verified vendor evidence and key/model/live-TLS
binding. It does not claim an Axiom-approved workload measurement until a
separately maintained measurement or workload-manifest allowlist is available.

## 7. Model catalog

The authenticated Axiom relay endpoint is:

```text
GET /api/v1/relay/models
Authorization: Bearer {axiom_api_key}
```

The response is an array whose entries contain:

```text
id
label
short_label
model
base_url
provider
provider_label
e2ee_protocol
e2ee_encryption_version
attestation_protocol
context_window_tokens
max_output_tokens
supported_reasoning_efforts[]
```

The catalog selects a compiled provider driver; it does not make evidence
trusted. Unknown providers/protocols, duplicate model IDs, non-HTTPS provider
URLs, local/private/metadata destinations, inconsistent limits, and unsupported
reasoning levels are rejected before attestation.

## 8. Encrypted relay request

The authenticated endpoint is:

```text
POST /api/v1/relay/chat/completions
Authorization: Bearer {axiom_api_key}
Content-Type: application/json
```

Top-level fields are:

```text
model_id
encryption_version
e2ee_protocol
client_public_key_hex
model_public_key_hex
encrypted_messages[]
encrypt_all_fields = true
encrypted_tools[]?
encrypted_tool_choice?
parallel_tool_calls?
stream
max_tokens?
sampling?
response_format?
reasoning_effort?
```

Each message preserves only the required role and tool-call routing metadata in
plaintext. Content, reasoning, name, refusal, assistant tool-call names and
arguments, tool definitions, tool descriptions, tool schemas, named tool
choices, and tool-result contents are encrypted independently.

The initial client bounds are no broader than the current backend:

| Item | Limit |
|---|---:|
| messages | 101 |
| tools | 128 |
| tool calls per message/response | 128 |
| tool-call ID | 256 characters |
| function name before encryption | 64 characters |
| message text before encryption | 262,000 bytes |
| tool description before encryption | 16,384 bytes |
| tool schema JSON before encryption | 131,072 bytes |
| tool arguments before encryption | 262,000 bytes |
| individual ciphertext hex | 524,288 characters |
| aggregate encrypted relay fields | 1,500,000 characters |
| local serialized relay request | 2 MiB |

Limits are validated before expensive cryptography where possible and again on
the serialized encrypted request.

## 9. Relay response and streaming

A non-stream response contains encrypted content, reasoning, refusal, and tool
calls plus plaintext operational metadata such as ID, model, finish reason, and
usage. Every encrypted field is decrypted locally with the request's fresh
client secret.

For streaming, the relay uses SSE event names:

- `message.encrypted_delta`: optional encrypted content/reasoning/refusal and
  encrypted tool-call fragments, plus a monotonically increasing sequence;
- `message.encrypted_completed`: usage and optional finish reason;
- `run.cancelled`: optional non-secret reason;
- `run.failed`: bounded error;
- `run.created` and `run.completed`: lifecycle notifications without message
  output.

Unknown events, malformed fields, invalid sequence, invalid tool indices, failed
decryption, excessive aggregate output, and EOF without an encrypted-completed
terminal event fail closed. A cancelled or failed stream cannot later become a
verified completion.

## 10. Compatibility vectors

The source vector set is generated by:

```text
/home/salt/clone/axiom/backend/scripts/gen_e2ee_vectors.py
```

and currently lives at:

```text
/home/salt/clone/axiom/proxy/fixtures/e2ee_vectors/near-v2.json
```

The committed AxiomAI fixture must include its source commit and SHA-256 in a
sidecar provenance file. It covers public/secret conversion, HKDF output,
Python-encrypt/Rust-decrypt round trips, Unicode and boundary payloads, and
report-data binding. Rust-generated ciphertext is also decrypted by the
authoritative Python implementation during the cross-language gate.

## 11. Terminal failure rule

There is no plaintext fallback. Any failure in catalog validation, evidence
retrieval, evidence verification, key binding, session creation, encryption,
relay contract validation, response authentication, decryption, stream
completion, or tool-call reconstruction terminates the request with a typed
local error.
