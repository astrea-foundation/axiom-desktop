# Confidential pool provider

`axiom-secure-client` optionally discovers locally configured `axiom-pool`
models. Set `AXIOM_POOL_CLIENT_CONFIG_FILE` to trusted operator configuration:

```json
{
  "issuer_key": "INSTALLED_CONTROLLER_ED25519_PUBLIC_KEY_HEX",
  "verifier": {
    "binary": "/absolute/path/to/axiom-pool-verify",
    "binary_sha256": "INSTALLED_VERIFIER_SHA256",
    "publisher_key": "INSTALLED_RELEASE_PUBLISHER_KEY_HEX",
    "minimum_policy_sequence": 1
  },
  "models": [{
    "model_manifest": "PINNED_MODEL_BUNDLE_SHA256",
    "label": "Axiom pool preview",
    "context_tokens": 32768,
    "output_tokens": 4096
  }]
}
```

Build the verifier from the gateway repository's `verifier/cmd/worker-native`
using its pinned Go source. The new shared Rust packages are checked source
snapshots under `vendor/`, recorded by `AXIOM_SOURCE.json`; ordinary builds do
not require a neighboring checkout. Existing providers retain their own code.

Discovery exposes `pool:<manifest-digest>` with the `axiom-pool-e2ee-v1` /
`tinfoil-gpu-v3` contract. Remote catalogs cannot change installed pins, origin
or protocol. Establishment independently verifies a fresh worker; requests
reserve that worker, refresh evidence and obtain scoped signed grants before
encrypting directly to its attested key. Worker changes or expiry fail closed.

Strict CPU status, NVIDIA appraisal, code/build provenance, model binding and
endorsed keys are mandatory. NEAR's temporary local OutOfDate consent does not
apply. The pool authenticates its HPKE key through opaque transport and does
not claim a direct worker TLS connection.

Streaming uses bounded asynchronous backpressure. Text/reasoning remains
provisional until transcript, request binding, receipt and EOF pass, then
`ResponseVerified` is emitted. UUID IDs preserve all bits during normalization.
Cancellation is owner scoped. Failed or ambiguous work is never replayed.

The native preview accepts text conversation with low/medium/high reasoning.
Tools, tool history, images/files, arbitrary sampling and structured output are
unsupported. Only zero-price grants are accepted. Image generation is available
in the lower-level shared client, not native chat input capability. See the
[owning runbook](https://github.com/astrea-foundation/axiom-inference/blob/dev/docs/implementation.md)
for deployment and remaining real Tinfoil hardware/browser qualification. A
synthetic Vast fixture cannot establish production trust.
