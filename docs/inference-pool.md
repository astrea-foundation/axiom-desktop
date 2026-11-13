# Confidential inference pool

The native `axiom-pool` provider uses the shared request-bound encrypted pool
protocol. The backend owns authorization, reservations and opaque routing; the
native client owns messages, recipient verification, encryption, decryption and
authenticated stream completion. Checked source snapshots under `vendor/` keep
this checkout independently buildable. `AXIOM_SOURCE.json` records each file's
SHA-256 and canonical source repository.

## Current Chutes boundary

The registered managed `chutes` provider now uses normal Axiom account
authentication and the backend's opaque Chutes relay. It verifies hardware and
the quoted SDK key locally, accepts eligible publisher-signed updates, and
authenticates ordered completion plus independently signed usage. The provider
is available to ordinary native builds with installed configuration; it does
not enable this document's independent Rust pool profile. See the canonical
[managed integration guide](../../axiom-inference/docs/chutes-managed.md) for
runtime trust, pricing, installation and deployment requirements. Packaged
Desktop reads its own `resources/chutes/native.json`; platform-specific helper
bundling remains installer release work.

Image tools retain a separate Chutes helper in the account's secure client,
regardless of the chat provider. Every image request checks the image recipient's
fresh, image-specific evidence before dispatch; the helper reuses still-valid
hardware evidence and obtains a new invocation nonce for each send. It checks
that the discovered worker and key still match that evidence. Failed or
cancelled operations discard the helper without replay. Account changes and
shutdown clear cached clients, and idle image helpers exit after five minutes.
See the [helper lifecycle](../../axiom-inference/docs/native-helper.md) for cache
freshness and isolation details.

The separate debug-only direct Chutes adapter now supports a stable staging
publisher authority. Its installed native configuration pins the publisher-key
fingerprint, and the native helper verifies signed release manifests, expiry,
revocation and rollback before fresh Intel/NVIDIA and ML-KEM key verification.
Eligible signed updates are accepted automatically. This path reports
`chutes-tdx-nvidia-release-v1` with **TEE verified** and a separate scope notice:
publisher authorization does not independently prove the executing image/model
inventory. It uses Chutes' actual attested recipient and has no Axiom relay
lease generation. An unsigned local adapter remains development-only; native
release builds exclude both local modes. This does not qualify the custom pool
profile discussed below.

The installed Qwen3.8 Uncensored offering can be discovered locally with
`chutes-gpu-v1` metadata. This profile **cannot establish a verified session**.
Native establishment returns `ATTESTATION_REJECTED` before sending a prompt,
loading an external verifier, or accepting discovered worker keys. An untrusted
remote catalog cannot enable this protocol. The current Chutes CPU/GPU hardware
checks do not establish the exact Axiom application/model release or endorse the
Rust worker's encryption and identity keys.

Do not change that profile to `tinfoil-gpu-v3`, insert observed measurements into
trusted policy, or use the deterministic fixture in Desktop. A qualified Chutes
implementation still needs the provider's verifiable workload/key endorsement,
the corresponding native/controller verifier, instance routing and live tests
against the actual admitted worker. A hardware diagnostic boolean is not an
application verification result.

## Local configuration

Set `AXIOM_POOL_CLIENT_CONFIG_FILE` to an independently installed public file.
For the current discovery-only Chutes offering its shape is:

```json
{
  "issuer_key": "LOCAL_CONTROLLER_ED25519_PUBLIC_KEY_HEX",
  "verifier": null,
  "models": [{
    "model_manifest": "PINNED_MODEL_INVENTORY_SHA256",
    "label": "Qwen3.8 Uncensored",
    "context_tokens": 32768,
    "output_tokens": 4096,
    "attestation_protocol": "chutes-gpu-v1"
  }]
}
```

Discovery exposes `pool:<manifest-digest>` and adds installed offerings to the
ordinary catalog. Local metadata does not grant inference authority. Qualified
`tinfoil-gpu-v3` offerings require an installed `verifier` object with `binary`,
`binary_sha256`, `publisher_key` and `minimum_policy_sequence`; its actual release
and hardware still need qualification. No verifier artifact is fabricated for
the Chutes preview.

The lower-level client binds grants, account, session, proof key, worker boot,
request and attempt IDs, limits and response keys. It accepts provisional deltas
only through its AEAD and frame verifier, then requires the signed terminal and
actual EOF before `ResponseVerified`. Cancellation is owner scoped and ambiguous
inference is never replayed. Only zero-price preview grants are currently
supported. Native chat supports text and low/medium/high reasoning; tools,
attachments, arbitrary sampling and structured output are not advertised.

## Isolated Desktop development profile

`AXIOM_DESKTOP_LOCAL_PROFILE=1` is limited to an unpackaged Desktop and a loopback
API/auth pair. It uses separate `Axiom Local` Electron data and `.axiom-local`
native data. Public pool configuration and a local CA path are passed explicitly
to the native sidecar and proxy; provider credentials are not inherited.

For repeated local debug rebuilds, also set
`AXIOM_DESKTOP_LOCAL_CREDENTIALS=1`. This passes
`AXIOMCLI_LOCAL_DEV_CREDENTIALS=1` to the debug Unix native runtime only. Account
tokens are then stored under `shared/development-credentials/` in the isolated
native data root, with directory mode 0700 and file mode 0600. The account name
is scoped to the exact API/auth origins. Symlinks and shared files are rejected.
This explicit development mode avoids Keychain approval dialogs when an unsigned
debug executable changes. It requires two loopback origins; packaged Desktop,
release native builds and normal profiles keep using the system Keychain. It
does not copy production credentials or relax provider attestation or E2EE.

`AXIOM_LOCAL_RELAY_CA_FILE` accepts one bounded PEM certificate for an exact HTTPS
loopback relay. It does not modify OS trust, provider recipient validation,
Intel policy or NVIDIA roots. Provider endpoints retain their strict address
checks. Remote origins, HTTP, invalid certificates and redirects fail closed.

The owning repository's `scripts/chutes_local.py` prepares this setup, starts the
local HTTPS backend and invokes the native diagnostic:

```sh
cargo run --locked -p axiom-secure-client --example local-pool-preflight
```

Set `AXIOM_BASE_URL`, `AXIOM_POOL_CLIENT_CONFIG_FILE` and
`AXIOM_LOCAL_RELAY_CA_FILE` first. This command reads only public local metadata,
verifies HTTPS and installed issuer/model pins, and reports the native rejection.
Exit code 2 means unqualified attestation. It sends no prompt, provides no account
credential, starts no Desktop and never permits a production verified-chat launch. Normal
account authorization is separate from inference qualification.

## Explicit interactive Chutes development adapter

The local debug workflow can launch an interactive text-only model separately
from the qualified pool: `chutes-preview:qwen3.8-27b`. Its trusted startup input is
`AXIOM_CHUTES_DESKTOP_PREVIEW_CONFIG`, an absolute native configuration file
containing `python`, `script` and `config` paths. The Rust adapter is compiled
only with debug assertions, requires the exact local API
`https://127.0.0.1:9443`, and never accepts a remote catalog contract. Packaged
Desktop does not pass this configuration to its sidecar.

The native stdio helper performs Intel DCAP, challenge/ML-KEM/proxy binding,
proxy-signature and NVIDIA checks, then uses instance-pinned Chutes E2EE. A
random secret inside each encrypted request authenticates every streaming frame,
its request hash/ID, sequence and previous frame MAC. The native bridge forwards
reasoning/text deltas as each one authenticates. Its terminal frame binds the
text/reasoning transcript hashes, usage and finish reason. Successful completion
requires that terminal and actual HTTP EOF; truncation, reordering, duplicate or
extra frames and cancellation never produce `ResponseVerified`. The backend sees
account and operational metadata only; messages and Chutes credentials stay
outside it. Both streaming and complete native calls use the streaming transport;
there is no inference replay on an ambiguous failure.

This preview uses observed development runtime pins and does not qualify
application provenance or model-file binding. Its evidence remains degraded,
its application-provenance check is explicitly false, and the UI displays a
yellow **Chutes dev preview** label even for an authenticated reply. This is
distinct from NEAR's outdated-TCB consent. Public report validation requires all
hardware/key checks to pass and rejects promotion to verified. The direct
adapter has no relay lease generation.

Use the owning inference repository's local runbook and
`scripts/chutes_local.py desktop` launcher. Its local test-account helper uses
in-memory email delivery and real native PKCE authorization; Desktop stores its
own refresh token in Keychain. Rebuilding an unsigned debug sidecar can require
fresh native sign-in because macOS binds credential access to the executable.
The production pool admission requirements above remain outstanding.

## Automatic reports and repository release authorization

The native provider now publishes the public report for the actual request's
verified session through ACP, before display deltas. The SDK validates its model,
provider contract, fingerprint and expiry, and invalidates pending older-key
preflights before retaining it. A report never establishes terminal reply
authentication. Development threads with missing/expired reports refresh through
the same native verifier between requests while active, including after wake.

The dialog shows **Hardware verified · code unverified** for a fresh Chutes
hardware-only report. Application provenance remains unestablished. Optional
GitHub/Sigstore release authorization is implemented in `axiom-inference`, with
exact repository/workflow identity, immutable repository/owner IDs, signed
time-bounded policies, revocation and rollback protection. It remains separate
from execution proof. Neither a signed release nor a provider `verified` flag
opens the existing production Chutes admission gate.

The current live worker has no configured GitHub release authority and no
provider-authenticated image/source/model/key-custody endorsement. See the
companion repository's `docs/release-verification.md` for the prepared workflow,
native metadata-feed configuration and production qualification requirements.

