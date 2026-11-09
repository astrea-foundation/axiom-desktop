# Confidential inference pool

The native `axiom-pool` provider uses the shared request-bound encrypted pool
protocol. The backend owns authorization, reservations and opaque routing; the
native client owns messages, recipient verification, encryption, decryption and
authenticated stream completion. Checked source snapshots under `vendor/` keep
this checkout independently buildable. `AXIOM_SOURCE.json` records each file's
SHA-256 and canonical source repository.

## Admission boundary

The only qualified pool attestation profile is `tinfoil-gpu-v3`. Installed
configuration rejects any other profile, and every installed offering requires
an installed `verifier`. The native provider refuses a model whose catalog entry
does not match an installed offering's manifest, origin, limits and profile, so
an untrusted remote catalog cannot enable an unsupported protocol. Do not insert
observed measurements into trusted policy or use the deterministic fixture in
Desktop. A hardware diagnostic boolean is not an application verification result.

Image tools do not use the pool. They use only the installed AstreaAI provider;
see the [AstreaAI connection guide](direct-verda.md).

## Local configuration

Set `AXIOM_POOL_CLIENT_CONFIG_FILE` to an independently installed public file:

```json
{
  "issuer_key": "LOCAL_CONTROLLER_ED25519_PUBLIC_KEY_HEX",
  "verifier": {
    "binary": "/absolute/path/to/axiom-pool-verify",
    "binary_sha256": "PINNED_VERIFIER_SHA256",
    "publisher_key": "RELEASE_PUBLISHER_ED25519_PUBLIC_KEY_HEX",
    "minimum_policy_sequence": 1
  },
  "models": [{
    "model_manifest": "PINNED_MODEL_INVENTORY_SHA256",
    "label": "Pool model",
    "context_tokens": 32768,
    "output_tokens": 4096,
    "attestation_protocol": "tinfoil-gpu-v3"
  }]
}
```

Discovery exposes `pool:<manifest-digest>` and adds installed offerings to the
ordinary catalog. Local metadata does not grant inference authority. The
`attestation_protocol` field defaults to `tinfoil-gpu-v3`; the `verifier` object
pins `binary`, `binary_sha256`, `publisher_key` and `minimum_policy_sequence`.
The actual release and hardware still need qualification.

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

## Automatic reports

The native provider publishes the public report for the actual request's
verified session through ACP, before display deltas. The SDK validates its model,
provider contract, fingerprint and expiry, and invalidates pending older-key
preflights before retaining it. A report never establishes terminal reply
authentication. Development threads with missing/expired reports refresh through
the same native verifier between requests while active, including after wake.
