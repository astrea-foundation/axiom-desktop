# Tinfoil EHBP v1

The compiled provider `tinfoil` accepts only `tinfoil-ehbp-v1`, encryption version 1, attestation profile `tinfoil-snp-sigstore-v1`, and dynamically discovered model identities at `https://inference.tinfoil.sh/v1`. No plaintext or TLS-only inference path is registered.

## Verification and request authentication

The pinned `tinfoil` Rust SDK (commit `91e8aef8fbc34129b68de8667ece5bd9ef7b7110`) verifies the router's AMD SEV-SNP report and certificate chain, Sigstore build provenance for `tinfoilsh/confidential-model-router`, agreement between measured software and that build, and a fresh live TLS handshake. The attested TLS key, live certificate SPKI, HPKE key SAN, and attestation-document hash must agree. An unauthenticated public-key fetch cannot establish a session.

Verified public material is cached for at most 240 seconds, bounded by the trust policy's expiry. Request private contexts are fresh and never reused. Failed exchanges invalidate cached material for the next attempt, without automatically replaying a possibly billable inference. The pinned SDK supports SNP; unsupported attestation formats fail closed.

Public ACP evidence omits `attestationGeneration` for this protocol because Tinfoil does not use a relay lease generation. The native and renderer projections still require expiry and the registered Tinfoil protocol identity; protocols that use relay leases continue to require a positive generation.

The measured router verifies its workers transitively. Evidence describes this accurately: this client verifies the router, not independent quotes for every individual GPU. The source release, repository and verifier ground truth remain inspectable without manual approval of each provider release.

The native adapter constructs complete messages and tool schemas locally. A random private cache secret partitions provider prompt caching per native client and remains inside the encrypted body. Axiom account tokens are only relay credentials. Provider credentials exist only at the Axiom backend.

## Wire and completion

The pinned `tinfoil-ehbp` implementation (commit `93cc1fa1ad61e19f9a34fb23cb5b5d5635d3edb0`) establishes fresh X25519/HKDF-SHA256/AES-256-GCM HPKE. The body sent to `/api/v1/relay/tinfoil/chat/completions` consists of four-byte-length-prefixed ciphertext frames. The encapsulated request key is public metadata.

Response keys derive from the request's secret HPKE exporter, encapsulated key and response nonce. Sequence-dependent AEAD nonces bind ordering. A response encrypted to an exposed client public key is not used as an authentication shortcut.

The protocol's frame-level EOF alone does not authenticate completion. A streaming success requires all of:

1. Every received frame authenticates under that request's response context, in order.
2. The decrypted SSE has one consistent response identity, bounded deltas, valid complete tool calls, coherent token usage and a terminal finish reason.
3. An **encrypted and authenticated `[DONE]`** follows those terminal fields; no subsequent data event is accepted.
4. Backend accounting is completed, final and settled for the same request/model, with token counts matching the authenticated response.

Only then does the adapter emit `Finished` and return success. Deltas before this are provisional. Nonstreaming success requires a complete authenticated JSON response with corresponding terminal fields and accounting. No signed receipt is claimed: response AEAD and authenticated application framing establish the request/environment binding.

The outer CLI marks durable output verified only after the session returns successfully. A trailer error or transport intermediary cannot make an incomplete accounting record successful merely by supplying EOF. The backend cannot mark output cryptographically verified.

The adapter checks requested tool controls against the completed result. Undefined tools, ignored required/named choices and forbidden parallel calls fail. Tinfoil sometimes labels complete tool calls `stop`; normalization happens only after names, IDs and JSON-object arguments validate.

## Models and limits

Chat models and their tools, images, reasoning and file capabilities come from the
[refreshed catalog](../provider-catalogs.md). The
native driver still pins the protocol and router endpoint. A new model can appear
without a native release only if its advertised contract is already supported.

Original images use inline `image_url` parts; files use
`{"type":"file","file":{"filename":"…","file_data":"data:<mime>;base64,…"}}`.
Both are inside the EHBP-encrypted body, including original filenames. The router
processes documents through attested services. Axiom performs no extraction or
conversion. Audio, video, remote URLs and realtime WebSocket traffic are excluded.

The backend publishes current API context windows and token prices. The former Axiom-only 8,192-token cap is removed. An explicit upstream output
limit is preserved when advertised; otherwise the joint context window is the
conditional ceiling, not a guaranteed output length. Native chat and proxy requests
with no explicit limit omit `max_tokens`, leaving the provider's exact
remaining-context/default-generation policy in control. Explicit caller and
compaction budgets remain bounded by the catalog. See
[vLLM output-budget selection](https://docs.vllm.ai/en/v0.15.1/api/vllm/entrypoints/utils/#vllm.entrypoints.utils.get_max_tokens). Existing context compaction uses the advertised full context window. There is no message-count cap in this adapter.

The native client enforces existing request, stream, event and response bounds. Initial response wait is at most 300 seconds, stream idle timeout 120 seconds and total response deadline 900 seconds. Cancellation drops the subscriber and aborts upstream generation. Exact final metering already received can settle; otherwise the backend waives the charge, preserves unknown token counts and releases the billing hold. Both clients reconcile this metadata while idle without upgrading cancelled output to verified. Provider rate limits are account/model specific; no production numeric entitlement has been established by these canaries.

Reasoning controls are bounded maps derived from provider metadata. The client
accepts only known effort levels, thinking booleans and the protocol's specific
`chat_template_kwargs` keys. Explicit thinking-disable suppresses effort controls;
otherwise a selected effort overrides the enable-mode default. Model-specific
prompt fragments, destinations or arbitrary parameter templates are forbidden.
The [driver](../../crates/axiom-secure-client/src/providers/tinfoil/mod.rs) validates
those controls before serialization.
Conversation titles, edit/regenerate and usage presentation are shared native
behavior documented in [Desktop](../desktop.md), not separate Tinfoil flows.

## Verification commands

Public attestation only (no key or inference cost):

```sh
cargo run -p axiom-secure-client --example tinfoil_attestation
```

Paid end-to-end smoke test, using a Tinfoil-enabled Axiom relay and an **Axiom** automation key already in the environment:

```sh
export AXIOM_RELAY_URL=https://your-axiom-relay.example
export AXIOM_TINFOIL_SMOKE_ROUNDS=2
export AXIOM_TINFOIL_SMOKE_PARALLEL=1
cargo run -p axiom-secure-client --example tinfoil_relay_smoke
```

`AXIOM_TINFOIL_MODEL` optionally selects one offering ID, and `AXIOM_TINFOIL_REASONING_EFFORT` selects `low`, `medium` or `high` on supported models. The script bounds each completion to 2,048 tokens and limits rounds to three. It performs real local `lookup_code` calls and sends their results back through encrypted inference. It prints only canary status and aggregate metadata, never credentials or recovery keys.

For an isolated local relay, build with `--features test-fixture`, set `AXIOM_TINFOIL_ALLOW_LOCAL_RELAY=1`, and use `http://127.0.0.1:<port>`. This opt-in permits loopback transport to the test backend; it does **not** bypass Tinfoil attestation or EHBP. Production configuration requires HTTPS.

Regression tests cover reordered/tampered/replayed/truncated frames, wrong response contexts, missing authenticated terminal markers, incomplete tools, malformed usage, hostile catalog entries and byte-fragmented UTF-8/SSE. Desktop proof tests distinguish router verification from independent GPU verification.


`RUST_LOG=axiom_secure_client=info` enables native attestation/exchange timing records in CLI/ACP modes. They include first authenticated delta and total exchange times, with no message or key material. TUI suppresses tracing output to protect its screen. The backend separately measures headers/first ciphertext/body idle time; ciphertext arrival must not be described as time to an authenticated token.

The dependency audit permits the two pinned Tinfoil Git sources and otherwise
retains the repository's source/license policy. Current dependency versions and
advisory results must be checked using [the development checks](../development.md#checks).

References: [EHBP](https://docs.tinfoil.sh/resources/ehbp), [backend proxy contract](https://docs.tinfoil.sh/guides/proxy-server), [Rust verifier](https://docs.tinfoil.sh/sdk/rust-sdk).
