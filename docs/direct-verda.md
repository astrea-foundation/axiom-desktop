# AstreaAI connection

Production installers include the frozen AstreaAI verifier and public account
configuration. Desktop and CLI discover it from their installation resources
when signed in to `https://api.axiom.stream`; customers do not install Python or
provide an environment file. Each platform bundles its native verifier. macOS
ships separate signed ARM64 and x64 runtimes; Windows ARM64 uses the signed x64
verifier. Packaging validates the complete runtime inventory, executable digest
and platform signatures before including it. The native client still verifies
fresh CPU, GPU, workload, key and response evidence before accepting inference.

AstreaAI is additive to the normal model catalog. Installing its verifier must
preserve the NEAR AI and Tinfoil offerings discovered through the account API.
Each offering retains its own locally verified attestation and E2EE protocol;
selecting another provider does not route that request through AstreaAI. If the
remote catalog is unavailable, the locally pinned AstreaAI offering remains
discoverable and still requires fresh worker admission before inference.

Build the frozen verifier from a clean committed checkout with
`node scripts/build-direct-bundle.mjs --output .local/direct/bundle`. The builder
checks the vendored source inventory and installs hash-locked dependencies.
Intel macOS compiles the locked cryptography source using locked build tools,
Rust and static Homebrew OpenSSL; the other platforms use upstream wheels.
Set `AXIOM_DIRECT_BUNDLE` to that output root when packaging. Signed production
installers require the bundle; private issuer keys are rejected from it.
Relative bundle paths resolve from the workspace root even when Electron's
packager changes into the Desktop directory. Absolute paths are also supported.

The explicit operator connection registers `direct:qwen3.8-27b` as **Qwen3.8
Uncensored**, under the **AstreaAI** provider. Account sign-in and local storage use the isolated HTTPS loopback
profile. Inference connects directly to the independently verified confidential
worker; the account service never receives conversation plaintext or the
operator issuer key.

The account-authorized production mode uses `https://api.axiom.stream` and
service-issued grants. Its public installed `account` configuration binds the exact
account API origin, issuer public key, immutable chat/image prices and a native
spending ceiling. `issuer_key_file` must be empty in this mode. The client obtains
an account-owned reservation and one-use grant, then sends encrypted inference
directly to the same independently verified worker. There is no operator-signing
fallback when account authorization, credit or price acceptance fails.

The approved guest loads `munekazu/Huihui-Qwen3.8-27B-abliterated-FP8`, pinned by
the signed model inventory. Its exact native model ID adds the Qwen3.8
Uncensored identity line to Axiom's uncensored system prompt, including after a
model switch. The compact
model label retains “Uncensored” so the checkpoint is identifiable in the composer.

This connection supports streaming text conversations with the
measured worker's medium reasoning template, a 32,768-token context and at most
4,096 output tokens. Native requests omit tools when the offering does not
support them. Image understanding is an explicit installed capability: set
`vision: true` only for a release whose signed capabilities include `vision` and
install its digest-pinned verifier. Both native configuration and fresh verified
release evidence must authorize it before image dispatch. The legacy text-only
installation defaults to `vision: false`.

Images are normalized locally to static PNG/JPEG within 1024×1024. At most eight
images may be present in the current request context. Composer attachments retain
their 16 MiB aggregate upload bound; the sealed protocol permits up to 36 MiB
for normalized image references and envelope overhead. The worker rechecks full image decoding, dimensions and a separate
signed input-pixel budget before work is claimed. It forwards identical multimodal
messages to tokenization and generation. Signed terminal usage must match the
originating image pixel count.

The earlier generation-7 / policy-12 connection passed fresh CPU/GPU/workload
verification and two authenticated app turns with an attached image. The second
turn retained the image and correctly counted three rectangles with their colors
in order. Both six-digit OCR canaries misread a digit, so exact OCR accuracy is
not qualified. The canonical inference repository's
[vision validation record](../../axiom-inference/docs/qa/direct-verda-vision-2026-11-03.json)
preserves those results together with the signed release, permanent policy and
base image-generation receipt.

The earlier generation-8 / permanent policy-13 deployment passed fresh hardware
and workload verification, authenticated tool calls and image generation from
the normal composer. With Thinking On, the model generated a 1024×1024 image
in 9,539 ms, viewed the image and completed an authenticated final reply. A
normal Thinking On chat reached visible text in 1,788 ms and authenticated
completion in 1,893 ms after 335 seconds since the initial proof. Its renewed
proof predated the request. The canonical inference repository's
[tools and latency validation record](../../axiom-inference/docs/qa/direct-verda-tools-latency-2026-11-03.json)
preserves the signed deployment and these measurements.

Generation 9 / permanent policy 14 qualified the full image path in a fresh
account and named QA profile. A composer request without size arguments used
the 1536×1536 square default (12,014 ms for the image operation). Five uploaded
references produced a verified image in 30,041 ms, an edit using the generated
image took 8,758 ms, and Regenerate preserved all five original reference IDs
in order and the same size (21,657 ms). Thinking stayed On. The
[reference-image qualification record](../../axiom-inference/docs/qa/direct-verda-reference-images-2026-11-03.json)
contains the signed deployment, image hashes and checks.

The final client check supplied five attachments with no size/aspect arguments
and produced another authenticated 1536×1536 image (21,768 ms). A smaller
reference therefore does not silently reduce the default generation size.

Tool-capable releases opt in with installed `tools: true` and
`image_generation: true`, in addition to `vision: true`. The signed measured
release must authorize `tools`, `image` and `vision`. The model can invoke the
existing `generate` tool; the image result, preview, save and regeneration paths
use the attested Direct client. Generation defaults to 1536×1536 square for
both new images and reference-based requests without size/aspect arguments, with
the measured eight-step image runtime. Other supported aspect ratios use the
32-pixel grid within 2,359,296 total pixels. Up to five ordered references can
come from attachments or generated images in the same conversation. Reference
bytes stay in the encrypted request; signed usage binds their normalized pixel
count. The adapter preserves source IDs for preview, saving and regeneration.
Regenerate retains the original request's chosen dimensions and reference order.
The requesting chat may use any supported vision-capable provider, including
NEAR or Tinfoil. Its model ID is image provenance, not a selector for the image
worker. AstreaAI still verifies its own worker, keys, signed image result and
account-owned usage; image evidence and billing remain attributed to AstreaAI.
The uncensored model selects its pinned image LoRA. Document uploads remain
unavailable.
The legacy operator connection uses free operator grants. The production account
connection uses its immutable published prices and account-owned reservations;
customer charges come only from authenticated worker usage.
Conversation titles use the local first-message label, keeping cosmetic
background inference off this worker.

## Billing readiness

The worker signs usage and binds it to the request, attempt, reservation, client
proof key and exact worker. Receipts include input/output tokens, cached-input
and reasoning subsets, output image pixels and steps, and input-image pixels.
Cached tokens are included in input tokens; reasoning tokens are included in
output tokens. Do not bill either subset twice.

The legacy operator connection does not charge customers. In that mode the Direct client
creates random account/session identifiers and a local issuer-signed
`pool-free-preview-v1` grant with no price snapshot and a zero maximum charge.
The worker accepts only these free grants. An image result's `settled: true` and
zero cost describe completion of that free request, not a debit from the signed-in
customer's balance.

The account path forwards complete chat accounting, including cached-input
and reasoning subsets, and image pixel/step/reference counters. Costs come from
the verified worker receipt and pinned price, and `settled` becomes true only
after the account controller confirms receipt reconciliation. The worker also
delivers the same receipt through its bounded private outbox. Both paths converge
on one durable ledger debit; accounting recovery remains scoped to the signed-in
account and does not assert that a missing response was authenticated locally.

The production backend accepts the current `input_image_pixels` field, 36 MiB
request bound and 2,359,296-pixel image output. The
[production qualification](../../axiom-inference/docs/qa/direct-astreaai-production-2026-11-05.json)
records concurrent account-owned completions, wrong-account refusal, idempotent
settlement, a zero-credit refusal before dispatch, cancelled metadata-only
reservations and the exact five-reference image charge. Immutable production
prices are `astreaai-chat-20261105-v1` ($0.25/M input, $0.05/M cached input and
$1.50/M output, including reasoning) and `astreaai-image-20261105-v1` ($0.008 per
generated image). These checks do not establish sustained throughput or all
signed-OS interactive flows; retain the
[remaining acceptance requirements](validation.md#astreaai-rollout-gates).

## Install and launch

Production customers install [Desktop or AxiomCLI 0.1.18](https://axiom.stream/downloads)
and sign in to their normal production account. The signed package supplies the
verifier and public trust configuration automatically. Select **Qwen3.8
Uncensored** under **AstreaAI**. The source/operator commands below are for an
isolated development connection.

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
`model_manifest`, `issuer_key_file`, `verifier`, and optional `vision`, `tools`
and `image_generation` flags (default false). The qualified generation-9
installation enables all three. The verifier object pins an
absolute executable, its `binary_sha256`, the publisher public key and the
minimum policy sequence. The issuer key is a local private PKCS#8 file with
private Unix permissions. Keep both files outside Git and application resources.
The renderer receives neither this configuration nor the private key.

The canonical inference repository's `scripts/direct_desktop.py` constructs a
small launch environment from explicit `--state`, `--desktop` and
`--account-state` paths. It does not inherit provider API credentials or Chutes
preview settings. The existing local account backend must be running. Select
**Qwen3.8 Uncensored** under AstreaAI after launch if the profile previously selected another model.

Development QA can set `AXIOM_DESKTOP_LOCAL_PROFILE_NAME` to a distinct lowercase
slug such as `verda-images-qa`. That gives Electron a separate profile and puts
native databases and credentials under `.axiom-local/profiles/<name>/`, keeping
existing conversations out of the QA runtime. The default local profile keeps
its existing paths. Named profiles require the same explicit loopback origins
and trusted configuration; packaged applications ignore the override.

Hosted staging QA instead sets `AXIOM_DESKTOP_STAGING_PROFILE=1` with the exact
staging API/authentication origins. An optional
`AXIOM_DESKTOP_STAGING_PROFILE_NAME=astreaai-qa-20261104` isolates Electron and
native storage beneath `.axiom-staging/profiles/<name>/`. Debug builds also
scope a named staging profile's system Keychain credential to that profile,
so qualification never reads or replaces the ordinary staging account's token.
The default staging
profile retains its previous paths. Only public Direct/pool configuration paths
are forwarded; local CA and local credential overrides are not forwarded to a
hosted staging process. Packaged applications ignore these development profiles.

`vendor/axiom-direct-verifier/AXIOM_SOURCE.json` records the canonical verifier
source and locked Python requirements. This owned bundle lets companion measured
gateways package the same independent verifier from the pinned native repository
without relying on a neighboring checkout or a client-held signing key.

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

Session establishment and inference share a bounded cache of verified public
evidence and the CPU-pinned TLS transport. Prewarming prepares admission before
inference. Both cached proof and admission must cover the entire new request
deadline; request keys, nonces and grants are never reused. Explicit Refresh
invalidates the cache and any failed exchange drops it without inference replay.
The proof's original local verification time is retained in the privacy report.

The account-owned provider checks every 20 seconds and prepares 210 seconds of
proof and admission coverage, providing 30 seconds of headroom above a normal
request deadline. Valid foreground cache hits do not wait for background
renewal network calls. Dropping the provider cancels renewal. Initial proof and
admission preparation, and the first engine request after a reboot, can still
have startup costs. The app's normal Thinking preference remains On.

Tool-capable releases support Thinking On and Off with identical template inputs
for token counting and generation. Off disables the private reasoning template;
the existing user preference is preserved. Native timings separate verification,
admission, first reasoning, first text and total request time.

Deployment, issuer/publisher key management and measured guest recovery belong
to the canonical inference repository's [AstreaAI guide](../../axiom-inference/docs/direct-verda.md).


Validated requests may wait in the worker's private RAM for up to 30 seconds.
If capacity is full or the wait expires, the worker returns an encrypted refusal
bound to the original request. Native verifies the stream through EOF and shows
“AstreaAI is busy. Your request was not started; try again shortly.” It preserves
verified connection preparation and does not automatically replay inference.
An ordinary HTTP error or a refusal after output remains a verification failure.
The worker reserves measured input plus maximum output tokens, within four
sequences and 65,536 tokens; long contexts can reduce simultaneous generation.
See the canonical guide for queue limits and external recovery operations.


Secure connection preparation retries explicit public attestation/admission
backpressure within fixed limits before transmitting inference. Each client
independently verifies the manufacturer's signed AMD collateral supplied with
the fresh report, including revocation and expiration. The worker shares these
public endorsements in RAM and coalesces only admission appraisals that cover
the entire signed lease. This prevents independent customers from repeatedly
fetching identical AMD collateral. Transport backpressure after dispatch shows
an overload error without asserting that work did not start; inference is not
automatically replayed.
