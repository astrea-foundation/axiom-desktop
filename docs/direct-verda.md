# Direct Verda development connection

The explicit operator connection registers `direct:qwen3.8-27b` as **Qwen3.8
Uncensored**, under the **Axiom Verda** provider. Account sign-in and local storage use the isolated HTTPS loopback
profile. Inference connects directly to the independently verified confidential
worker; the account service never receives conversation plaintext or the
operator issuer key.

The approved guest loads `munekazu/Huihui-Qwen3.8-27B-abliterated-FP8`, pinned by
the signed model inventory. Its exact native model ID selects Axiom's bundled
Qwen3.8 Uncensored system prompt, including after a model switch. The compact
model label retains “Uncensored” so the checkpoint is identifiable in the composer.

This development connection supports streaming text conversations with the
measured worker's medium reasoning template, a 32,768-token context and at most
4,096 output tokens. Native requests omit tools when the offering does not
support them. Image understanding is an explicit installed capability: set
`vision: true` only for a release whose signed capabilities include `vision` and
install its digest-pinned verifier. Both native configuration and fresh verified
release evidence must authorize it before image dispatch. The legacy text-only
installation defaults to `vision: false`.

Images are normalized locally to static PNG/JPEG within 1024×1024. At most eight
images may be present in the current request context, within a 16 MiB encrypted
payload bound. The worker rechecks full image decoding, dimensions and a separate
signed input-pixel budget before work is claimed. It forwards identical multimodal
messages to tokenization and generation. Signed terminal usage must match the
originating image pixel count.

Tools, document uploads, image edits and Desktop image generation remain
unavailable in this connection. The separately qualified operator client in
the inference repository supports base image generation. Desktop image calls
fail with an explicit capability error rather than selecting another provider.
The VM is rented by the hour; the model has no per-token price in the account
catalog. Local account billing is independent of Verda's compute charges.
Conversation titles use the local first-message label, keeping cosmetic
background inference off this worker.

## Install and launch

Build this branch normally, retaining the current development version:

```sh
cargo build -p axiomcli
pnpm --filter axiom-desktop build
```

Set these trusted startup variables for an unpackaged development launch:

```text
AXIOM_BASE_URL=https://127.0.0.1:9443
AXIOM_AUTH_URL=https://127.0.0.1:9443
AXIOM_DESKTOP_LOCAL_PROFILE=1
AXIOM_DESKTOP_LOCAL_CREDENTIALS=1
AXIOM_LOCAL_RELAY_CA_FILE=/absolute/private/account-state/relay-ca.pem
AXIOM_DIRECT_CLIENT_CONFIG_FILE=/absolute/private/direct-state/desktop-direct.json
AXIOMCLI_SIDECAR=/absolute/desktop/target/debug/axiomcli
```

The direct configuration is an ordinary absolute file with private Unix
permissions. Its strict schema contains `origin`, optional `connect_ip`,
`model_manifest`, `issuer_key_file`, `verifier`, and optional `vision` (default
false). The verifier object pins an
absolute executable, its `binary_sha256`, the publisher public key and the
minimum policy sequence. The issuer key is a local private PKCS#8 file with
private Unix permissions. Keep both files outside Git and application resources.
The renderer receives neither this configuration nor the private key.

The canonical inference repository's `scripts/direct_desktop.py` constructs a
small launch environment from explicit `--state`, `--desktop` and
`--account-state` paths. It does not inherit provider API credentials or Chutes
preview settings. The existing local account backend must be running. Select
**Qwen3.8 Uncensored** under Axiom Verda after launch if the profile previously selected another model.

## Verification and completion

`axiom-snp-nvidia-v1` verifies the approved AMD SEV-SNP launch, production NVIDIA
confidential-computing evidence, signed release and model inventory, worker boot
identity and keys, and the live CPU-bound TLS certificate. `connect_ip` changes
network routing while retaining the installed logical HTTPS hostname. Initial
untrusted TLS carries only a public challenge; prompt encryption and admission
use the attested TLS pin after independent local verification.

The native provider reports the verified release generation, separate policy
sequence, key fingerprint and public evidence in the privacy report. A verified
environment is distinct from a verified response. Streaming deltas remain
provisional until AEAD framing, ordering, request binding, the signed terminal
receipt and EOF all pass. Any failure blocks success without replaying inference.

AMD collateral downloads retry bounded vendor throttling or service failures,
respecting numeric retry intervals up to ten seconds. Every returned certificate,
revocation list, firmware floor, signature and challenge binding is still
verified. Operator admission may retry a rejected lease before a request grant
or prompt ciphertext exists. Inference itself is sent once.

Permanent release approval does not extend hardware proof freshness. New proof
and admission lifetimes are short and obtained automatically. A deployed worker
that quarantines an incomplete engine execution requires operator recovery;
the client cannot clear that condition or disable verification.

Deployment, issuer/publisher key management and measured guest recovery belong
to the canonical inference repository's [Verda guide](../../axiom-inference/docs/direct-verda.md).
