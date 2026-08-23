# Axiom secure inference and proxy implementation plan

Status: **proposed — no implementation stages started**

Primary language: Rust

Primary products:

- `axiomcli`, which will use secure inference directly in-process.
- `axiom-proxy`, which will expose a loopback OpenAI-compatible API backed by
  the same secure inference client.
- The future Electron application, which will supervise `axiom-proxy` but will
  not implement attestation or encryption itself.

## 1. Purpose

Build Axiom's production inference trust path in reviewable stages:

```text
plaintext exists locally
        |
        v
provider-specific attestation verification
        |
        v
verified provider key + live endpoint binding
        |
        v
provider-specific E2EE session
        |
        v
Axiom relay receives ciphertext and non-secret metadata only
        |
        v
attested model runtime decrypts and performs inference
```

Then expose that path through a separate local proxy:

```text
OpenAI-compatible client
        |
        | loopback HTTP + local authentication
        v
axiom-proxy
        |
        v
OpenAI compatibility adapter
        |
        v
axiom-secure-client
        |
        v
attested, end-to-end encrypted provider inference
```

The work is complete only when automated tests prove all of the following:

1. A valid provider attestation is checked on the user's machine.
2. The encryption key used for a request is the key bound into that verified
   evidence and the live TLS endpoint.
3. Messages, tool definitions, tool calls, tool results, and response content
   do not cross the Axiom relay in plaintext.
4. Streaming, cancellation, errors, usage, and tool-call reconstruction work
   through the actual `axiom-proxy` process.
5. A real external coding client completes an ordered tool loop through the
   proxy against the live Axiom service.
6. AxiomCLI can use the same secure client without a localhost proxy hop.

This is an early implementation. Interfaces should be clear and typed, but
should remain easy to revise before `1.0`. The existing Axiom proxy is a
behavioral and protocol reference, not code to copy into this repository.

## 2. Stage governance

Stages are sequential unless a stage explicitly says otherwise. A later stage
must not begin until the current stage has a review record marked `PASS`.

Every stage review must:

1. List the exact commit under review.
2. List every command run and whether it passed.
3. Map each exit criterion to concrete test or inspection evidence.
4. Record security, correctness, API, maintainability, and privacy findings.
5. Classify findings as blocker, high, medium, low, or informational.
6. Keep the decision at `FAIL` while a blocker or high-severity finding is
   unresolved.
7. Rerun the whole stage gate after corrective changes.
8. Save the result under `docs/reviews/secure-inference-stage-NN-*.md`.

The reviewer must actively try to disprove the implementation, not merely
confirm that happy-path tests exist. A failure is fixed in the stage that
introduced it; it is not deferred into a later cleanup stage.

The normal quality gate for every Rust implementation stage is:

```text
cargo fmt --all -- --check
cargo check --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo deny check
```

Stage-specific test suites are added to this baseline. Network-dependent live
tests are opt-in, but are mandatory at their designated integration and release
gates.

## 3. Scope

### 3.1 In scope

- Provider-neutral inference request, response, streaming, model, reasoning,
  usage, and tool contracts.
- A provider registry and provider-specific secure-session implementations.
- The initial NEAR `near-v2` provider implementation.
- Local verification of Intel TDX and NVIDIA evidence.
- Nonce, model, signing-key, and live TLS SPKI binding.
- Fresh client encryption identity per inference request.
- Encrypted non-streaming and streaming relay requests.
- Full encrypted OpenAI-style tool-call round trips.
- A reusable Rust secure client consumed by AxiomCLI and the proxy.
- A pure OpenAI compatibility adapter.
- A loopback-only Rust proxy executable with local client authentication.
- Deterministic, adversarial, process-level, live-service, and real-client
  integration tests.
- Typed, metadata-only security evidence suitable for the TUI, desktop app,
  proxy status, and audit records.

### 3.2 Explicitly out of scope for this plan

- Electron renderer or desktop UI implementation.
- Wallet, Astrea, or financial authorization.
- Anthropic-compatible endpoints.
- Every OpenAI endpoint; the first vertical slice is `/v1/models` and
  `/v1/chat/completions` with streaming and tools.
- Long-lived attestation caches.
- Provider-to-provider session reuse.
- Server-side prompt construction or plaintext conversation storage.
- Automatic downgrade to plaintext or TLS-only inference.
- Splitting every provider into a separate crate before a second provider
  proves that boundary useful.

## 4. Existing baseline and reference sources

### 4.1 Current repository baseline

The monorepo migration is present in the working tree:

```text
apps/axiomcli/          existing coding agent and development provider
apps/axiom-proxy/       documented placeholder
apps/desktop/           documented placeholder
crates/axiom-inference/ early transport-neutral contracts
packages/sidecar-protocol/
```

ADR 0009 already establishes the intended dependency direction. This plan
fills in the secure-client and proxy portions without coupling them to TUI,
Electron, or agent policy.

### 4.2 Axiom protocol references

The following first-party snapshots define the behavior to reconcile before
implementation:

| Reference | Snapshot | What it teaches us |
|---|---:|---|
| `/home/salt/clone/axiom/backend` | Axiom commit `114be9a4061e` | Authoritative Python `near-v2` primitives, relay/provider contracts, attestation verification, request limits, and vector generator. |
| `/home/salt/clone/axiom/backend/FRONTEND_E2EE_HANDOFF.md` | same | Privacy boundary and fail-closed frontend flow. |
| `/home/salt/clone/axiom/proxy` | `29efe06c6b65` | Working Rust proxy behavior, local attestation, encrypted relay DTOs, SSE translation, security receipts, and live OpenCode tool harness. |
| `/home/salt/clone/axiomio` | `8ef49ef98a74` | Older clean release baseline for identifying later proxy changes. |
| `/home/salt/clone/axiom/proxy/fixtures/e2ee_vectors/near-v2.json` | proxy snapshot above | Cross-language conversion, HKDF, encryption, and report-data compatibility cases. |

The new implementation will be written around AxiomAI's own boundaries and
types. It may adopt protocol constants and compatibility vectors because exact
wire compatibility is required, but it must not paste or lightly rename the
reference proxy's implementation, comments, tests, or module structure.

Before Stage 01 passes, record which backend commit is authoritative. If the
backend and proxy disagree, resolve the contract with Axiom backend maintainers
and add a versioned compatibility test rather than guessing.

### 4.3 Behaviors worth preserving from the working proxy

- Every inference request uses `provider_e2ee_v2` / `near-v2`.
- A model key is accepted only after local verification of fresh attestation.
- The requested nonce, expected model, model signing key, and live TLS SPKI are
  bound together.
- Intel collateral and NVIDIA evidence are verified on the user's computer.
- Client Ed25519 key material is fresh per request and never persisted.
- All secret-bearing message and tool fields are encrypted individually.
- The relay sees only ciphertext and necessary routing metadata.
- Decryption, stream assembly, and OpenAI translation happen locally.
- Malformed, incomplete, tampered, or contract-mismatched responses fail
  closed.
- Audit records contain evidence and fingerprints, not plaintext or keys.
- The live tool test starts a real proxy and a real OpenCode process in a
  disposable workspace, and proves an ordered read/edit/read/final-response
  workflow.

These are minimum compatibility requirements, not a mandate to preserve every
old API or type.

## 5. Non-negotiable security invariants

1. Production inference has no plaintext, TLS-only, or "skip verification"
   fallback.
2. A provider module owns the full chain from evidence retrieval through key
   binding, encryption, response decryption, and evidence reporting.
3. Application code never receives an unverified provider key that it could
   accidentally use as trusted key material.
4. Model output, relay data, provider metadata, tool calls, and tool results are
   untrusted until parsed, bounded, and validated.
5. The exact TLS connection used to fetch attestation must expose the SPKI that
   is compared with the fingerprint bound into the evidence. A second
   connection or DNS lookup must not silently stand in for it.
6. Attestation uses a fresh cryptographically random challenge. Replayed or
   mismatched challenges are rejected.
7. A model identifier from the catalog must match the model identity in the
   evidence when the provider protocol exposes one.
8. Encryption uses only key material authenticated by the accepted evidence.
9. Client private key material, shared secrets, derived keys, and plaintext are
   never logged, persisted, serialized into audit records, or exposed through
   proxy control APIs.
10. The Axiom credential is held only by the local secure client. A proxy client
    never receives it.
11. The proxy binds to loopback by default and requires a separate local client
    credential. Binding to a non-loopback address requires an explicit future
    design and is not part of this plan.
12. The proxy transports tool semantics but does not grant or execute local
    tools. The external client or AxiomCLI policy engine retains that authority.
13. Tool names, descriptions, parameter schemas, selected tool names, call
    arguments, and tool results are message secrets and must be encrypted.
14. Cancellation must reach the local HTTP request and remote relay request as
    far as the protocol permits; cancellation must never be reported as a
    verified completion.
15. An attestation failure is a visible security failure. It is never silently
    retried against an insecure route.

### 5.1 Runtime-identity policy

Valid vendor evidence proves genuine hardware and binds runtime-provided data;
it does not automatically prove that the runtime is an Axiom-approved software
build. Stage 01 must specify which identity claims are available from each
provider and which are policy-enforced.

The first `TrustPolicy` must distinguish at least:

- vendor evidence validity;
- accepted TCB/advisory status;
- nonce freshness;
- model identity;
- model signing key binding;
- live TLS endpoint binding;
- workload measurement or manifest identity, when available;
- policy version and verification time.

If an approved workload measurement allowlist is not yet available, the UI and
audit may say exactly what was verified, such as "TEE hardware and endpoint
binding verified." It must not claim "approved Axiom runtime verified." Adding
an allowlist later must not require changing the provider-neutral API.

## 6. Target architecture

```text
                            axiom-inference
                           (domain contracts)
                              ^         ^
                              |         |
                    +---------+         +----------------+
                    |                                    |
          axiom-secure-client                   axiom-openai-compat
     (trust, provider registry, relay)          (wire translation only)
                    ^                                    ^
                    |                                    |
          +---------+---------+                          |
          |                   |                          |
    apps/axiomcli       apps/axiom-proxy ----------------+
   (agent + policy)      (loopback server)
                              ^
                              |
                       future Electron app
                       (process supervisor)
```

### 6.1 `crates/axiom-inference`

Owns only provider- and transport-neutral concepts:

- messages and roles;
- assistant content, reasoning, refusal, and tool calls;
- tool definitions and tool-choice policy;
- model descriptors and capabilities;
- reasoning effort;
- request parameters and response format;
- usage;
- finish reason;
- normalized streaming events;
- stable request/cancellation identifiers where needed.

It must not depend on HTTP, OpenAI wire types, cryptographic libraries,
attestation libraries, environment variables, TUI types, ACP types, or agent
permissions.

### 6.2 `crates/axiom-secure-client`

Owns:

- authentication with Axiom services;
- model catalog retrieval and validation;
- provider registry and provider selection;
- provider-specific attestation and cryptography;
- secure request/session lifecycle;
- encrypted relay request/response DTOs;
- streaming relay parsing;
- retry and cancellation policy;
- bounded parsing and error classification;
- typed security evidence and metadata-only audit events.

It takes typed configuration in constructors. It does not read environment
variables or decide UI wording, local tool permissions, or proxy bind policy.

The initial provider can live under:

```text
crates/axiom-secure-client/src/
  client.rs
  catalog.rs
  relay/
  security.rs
  provider.rs
  providers/
    near/
      mod.rs
      protocol.rs
      crypto.rs
      attestation.rs
      evidence.rs
```

Split provider implementations into separate crates only when a second real
provider makes compile-time isolation or dependency weight valuable.

### 6.3 Provider abstraction

The abstraction must accommodate providers with different evidence formats,
key exchanges, key lifetimes, stream framing, and trust claims. It must not
assume every provider uses Ed25519, X25519, Intel, NVIDIA, a TLS fingerprint,
or one key per request.

The design should resemble this responsibility boundary, without treating the
pseudocode as a frozen public API:

```rust
trait SecureProvider: Send + Sync {
    fn id(&self) -> ProviderId;
    fn supports(&self, model: &ModelDescriptor) -> Result<(), ContractError>;

    async fn establish(
        &self,
        model: &ModelDescriptor,
        policy: &TrustPolicy,
        cancellation: CancellationToken,
    ) -> Result<Box<dyn VerifiedSession>, SecureClientError>;
}

trait VerifiedSession: Send {
    fn evidence(&self) -> &SecurityEvidence;

    async fn complete(
        &mut self,
        request: InferenceRequest,
        cancellation: CancellationToken,
    ) -> Result<InferenceResponse, SecureClientError>;

    async fn stream(
        &mut self,
        request: InferenceRequest,
        sink: EventSink,
        cancellation: CancellationToken,
    ) -> Result<InferenceResponse, SecureClientError>;
}
```

Key properties:

- `establish` returns either a fully verified, usable session or an error.
- There is no public constructor for a "verified" session from arbitrary key
  bytes.
- `SecurityEvidence` exposes normalized claims plus provider-specific display
  checks; it does not expose secret material.
- Catalog protocol/version fields must match the selected provider driver.
- Applications depend on the common contract and never branch on `near`.

### 6.4 `crates/axiom-openai-compat`

Owns pure translation between external OpenAI-compatible wire types and
`axiom-inference` types:

- model list responses;
- chat-completion validation;
- streaming and non-streaming response shapes;
- tool definitions, choices, calls, and results;
- finish reasons, usage, and errors.

It must not perform HTTP serving, read credentials, verify evidence, encrypt
payloads, execute tools, or know provider-specific protocols.

### 6.5 `apps/axiom-proxy`

Owns:

- process CLI and environment parsing;
- loopback listener lifecycle;
- local client authentication;
- Axum routes and request limits;
- one `axiom-secure-client` composition root;
- health/readiness and metadata-safe operational status;
- local OpenAI error and SSE delivery;
- shutdown and desktop-sidecar control behavior.

The proxy should initially implement:

```text
GET  /healthz
GET  /readyz
GET  /v1/models
POST /v1/chat/completions
```

`/healthz` reports that the process/event loop is alive. `/readyz` reports
whether configuration is usable, but must not claim current attestation unless
current verified evidence exists. Model-specific attestation can still occur
per request.

### 6.6 AxiomCLI integration

AxiomCLI will replace its current OpenAI-compatible development provider with
an adapter over `axiom-secure-client`. The existing agent loop, tool registry,
permission engine, TUI, ACP, and persistence remain application concerns.

The runtime path becomes:

```text
AxiomCLI agent -> axiom-inference request -> axiom-secure-client -> Axiom
```

It must not become:

```text
AxiomCLI -> localhost axiom-proxy -> axiom-secure-client
```

That keeps the proxy optional and avoids a needless local plaintext hop for the
first-party agent.

## 7. Data confidentiality contract

Stage 01 must turn this table into executable coverage tests.

| Field | Relay visibility | Notes |
|---|---|---|
| Message role | plaintext metadata | Required for provider routing. |
| Message content | encrypted | Includes system, user, assistant, and tool content. |
| Message reasoning/refusal | encrypted | If supported by the model contract. |
| Message name | encrypted | Optional but secret-bearing. |
| Tool-call ID/index/type | minimal plaintext metadata | Only what the provider wire contract requires. |
| Tool-call function name | encrypted | Never infer sensitivity from the name. |
| Tool-call arguments | encrypted | May contain code, paths, secrets, or user data. |
| Tool definition name | encrypted | Part of prompt context. |
| Tool definition description | encrypted | Part of prompt context. |
| Tool parameter JSON schema | encrypted | Part of prompt context and potentially proprietary. |
| Named tool choice | encrypted | Function name is prompt context. |
| Model/provider/protocol/version | plaintext metadata | Strictly validated against catalog and driver. |
| Sampling/reasoning/output limits | plaintext metadata | No message content. |
| Client public key | plaintext cryptographic metadata | Fresh per request for `near-v2`. |
| Attested model public key | plaintext cryptographic metadata | Public key only; fingerprints in logs. |
| Stream content/reasoning/refusal | encrypted | Decrypted only locally. |
| Streamed tool names/arguments | encrypted | Fragments reassembled locally. |
| Usage and finish reason | plaintext metadata | Validated before completion is recorded. |
| Axiom API credential | never in relay body/logs | Authorization header from secure client only. |

An integration test must place unique sentinels in every secret-bearing field,
capture the serialized relay request and operational logs, and fail if any
sentinel is present outside ciphertext.

## 8. Verification lanes

The plan uses several complementary lanes. "No mocks" does not mean every
failure test should depend on an external production service; it means the
component claimed to work must be real.

### Lane A — deterministic unit and protocol tests

- Real production cryptographic functions with deterministic vectors.
- Pure evidence parsing and policy checks.
- No network.
- Includes corruption, truncation, wrong keys, wrong nonce, wrong model, and
  boundary sizes.

### Lane B — adversarial local integration

- The real `axiom-proxy` binary runs as a child process.
- The real secure client, encryption, OpenAI adapter, HTTP routes, and SSE stack
  execute.
- A purpose-built local encrypted relay fixture supplies controlled evidence
  and stream events so every negative path can be reproduced.
- The fixture is not accepted as proof that production attestation works. Its
  role is deterministic protocol fault injection.

### Lane C — live attestation verification

- Connect to a real configured provider attestation endpoint.
- Fetch current Intel collateral and validate real NVIDIA evidence.
- Prove nonce, model, key, and exact live TLS binding.
- No bypasses or pre-verified fixtures.

### Lane D — live Axiom inference

- Start the real proxy binary against the real Axiom relay and provider.
- Exercise model discovery, non-streaming, streaming, cancellation, reasoning,
  usage, and errors.
- Capture only sanitized metadata and ciphertext for privacy assertions.

### Lane E — real external tool client

- Start the real proxy binary.
- Start a real OpenAI-compatible coding client such as OpenCode in an isolated
  home and disposable workspace.
- Give the client narrowly allowed read/edit/write tools.
- Require an ordered read/edit/read/final-response workflow.
- Prove exact file output, event ordering, no extra files, no credential leak,
  and no plaintext sentinel in proxy logs.

### Lane F — AxiomCLI direct secure-client loop

- Run AxiomCLI without `axiom-proxy`.
- Use the real Axiom service and AxiomCLI's real policy/tool runtime.
- Prove at least a read-only tool loop and a confirmed disposable edit loop.
- For the web-tool case, use the existing real local SearXNG integration rather
  than a fabricated search response.

Lanes A and B run in ordinary CI. Lanes C–F require protected credentials and
network access and run in protected CI and before a release candidate. A live
lane failure blocks release even if deterministic suites pass.

## 9. Staged implementation plan

### Stage 00 — Freeze the baseline and reconcile the monorepo migration

**Objective:** begin from a known, reviewable repository state without mixing
the existing mechanical move with security-sensitive implementation.

**Deliverables:**

- Review the current uncommitted monorepo restructuring.
- Confirm that old `axiomcli/` deletions exactly correspond to files under
  `apps/axiomcli/` and that no behavior was lost.
- Run the full existing AxiomCLI gate.
- Commit the mechanical restructure separately from encryption work.
- Record current Axiom backend and proxy reference commits.
- Add this plan and link it from the root README and ADR 0009.
- Create the Stage 00 review record.

**Verification:**

- `git diff --summary` and rename/copy inspection.
- Existing unit, integration, PTY, ACP, SearXNG, evaluation, and release tests.
- Clean checkout rebuild of AxiomCLI.
- Confirm no key, local database, cache, generated artifact, or reference repo
  enters Git.

**Exit gate:** The restructure is committed, AxiomCLI behaves identically to
the prior commit, and no secure functionality is claimed yet.

### Stage 01 — Write the versioned protocol and threat specification

**Objective:** define exactly what will be implemented before writing crypto or
network code.

**Deliverables:**

- A versioned `near-v2` protocol specification in this repository covering:
  - Ed25519-to-X25519 public and secret conversion;
  - ephemeral X25519 ECDH;
  - HKDF-SHA256 parameters;
  - XChaCha20-Poly1305 nonce and wire layout;
  - encoding and byte-length rules;
  - client-key lifetime;
  - report-data construction;
  - relay request and event schemas;
  - plaintext/ciphertext field classification;
  - maximum counts and byte sizes;
  - terminal and error semantics.
- A provider contract/version matrix derived from the live model catalog.
- A trust-policy document specifying accepted Intel statuses, NVIDIA verdicts,
  freshness, redirects, model identity, TLS SPKI, and workload identity claims.
- An update to `docs/threat-model.md` adding the inference and local-proxy trust
  boundaries.
- An ADR for the provider-owned verified-session abstraction.
- Provenance metadata for imported compatibility vectors.

**Review questions:**

- Does accepted evidence authenticate only hardware, or an approved workload?
- Which endpoint supplies the model catalog, and which fields are authoritative?
- Can a catalog-selected base URL cause local-network access? If so, how is it
  constrained without hardcoding one provider globally?
- Are redirects forbidden for attestation and inference endpoints?
- What freshness or expiry exists beyond the client nonce?
- Which relay cancellation behavior is actually supported?
- Which error fields can safely be shown without reflecting secret data?

**Verification:**

- Compare the specification against backend tests, Python primitives, the
  vector generator, and the working proxy snapshot.
- Have every field in Section 7 mapped to a wire location and test case.
- Run the vector generator twice and confirm stable deterministic portions.
- Resolve every backend/proxy discrepancy in writing.

**Exit gate:** A reviewer can implement a compatible client using only the new
specification and referenced vectors. No unresolved security-critical ambiguity
remains.

### Stage 02 — Complete provider-neutral inference contracts

**Objective:** make `axiom-inference` expressive enough for the secure client,
the proxy adapter, and AxiomCLI without importing provider assumptions.

**Deliverables:**

- Expand `InferenceRequest` with explicit reasoning effort, stream intent,
  sampling/output limits, response format, and tool choice.
- Represent optional message content, name, reasoning, refusal, tool-call IDs,
  calls, and results without lossy OpenAI-specific shortcuts.
- Add normalized `InferenceEvent` variants for:
  - connection/security state;
  - text, reasoning, and refusal deltas;
  - tool-call start/name/arguments/end;
  - usage;
  - finish reason;
  - terminal completion/cancellation/failure.
- Expand `ModelInfo` into a provider-neutral `ModelDescriptor` with provider ID,
  protocol IDs/versions, context and output limits, reasoning capabilities, and
  tool/streaming capabilities.
- Define bounded validation errors without HTTP status codes.

**Verification:**

- Serialization/round-trip tests for every domain value that crosses a crate
  boundary.
- Property tests for tool-call fragment assembly and invalid indices.
- Tests showing that a future provider with different protocol identifiers fits
  the model descriptor without changing application code.
- Dependency inspection proves the crate has no HTTP/crypto/UI dependencies.

**Exit gate:** Existing AxiomCLI behavior can compile against the expanded types,
and neither proxy nor NEAR terminology has leaked into domain contracts.

### Stage 03 — Build the secure-client lifecycle and provider registry

**Objective:** create the framework that makes insecure or mismatched provider
composition difficult by construction.

**Deliverables:**

- Create `crates/axiom-secure-client`.
- Define typed client configuration, credential wrapper, endpoint policy,
  request limits, timeout policy, and cancellation.
- Define the provider registry, `SecureProvider`, opaque `VerifiedSession`,
  `TrustPolicy`, `SecurityEvidence`, and secure-client error taxonomy.
- Define security states such as `Unverified`, `Verifying`, `Verified`,
  `Rejected`, `Cancelled`, and `Expired` without allowing UI labels to turn an
  unverified state into verified.
- Make provider selection data-driven from the validated model descriptor.
- Reject unknown providers, protocols, versions, and capability mismatches.
- Make secret types redact their debug/display output.

**Verification:**

- Fake provider drivers are used only to test lifecycle/registry mechanics.
- Compile-fail or API tests show consumers cannot construct a verified session
  from arbitrary bytes.
- Duplicate provider IDs and catalog/driver mismatches fail deterministically.
- Cancellation during establishment never yields a usable session.
- Secret-scanning tests cover error, debug, tracing, and serialized evidence.

**Exit gate:** The framework has no production provider yet, makes no security
claim, and has no API that bypasses establishment.

### Stage 04 — Reimplement and prove `near-v2` cryptography

**Objective:** produce a small, independently testable NEAR cryptographic module
with byte-for-byte compatibility.

**Deliverables:**

- New Rust implementation of the specified Ed25519/X25519 conversion, ECDH,
  HKDF-SHA256, XChaCha20-Poly1305, and wire encoding.
- Strong wrapper types for public keys, secret keys, nonce, ciphertext, and
  protocol version.
- Fresh per-request client identity generated from an OS CSPRNG.
- Zeroization for secret key material where supported and documented.
- Strict length, hex, UTF-8, authentication-tag, and payload-size validation.
- Import the versioned compatibility vectors with provenance.

**Verification:**

- Rust decrypts Python-generated ciphertext vectors.
- Python decrypts Rust-generated ciphertext produced by a test-vector utility.
- Known key-conversion and HKDF outputs match exactly.
- Empty, Unicode, multi-kilobyte, and boundary-size payloads round-trip.
- Wrong recipient, flipped bit, truncated prefix/tag, duplicate/invalid hex,
  and oversized payloads fail closed.
- Property/fuzz tests never panic on arbitrary ciphertext bytes.
- Logging and panic formatting do not expose secrets or plaintext.

**Exit gate:** Cross-language compatibility passes and an independent review
finds no protocol drift or accidental secret exposure.

### Stage 05 — Implement local NEAR attestation verification

**Objective:** accept a model key only after verifying fresh provider evidence
on the user's machine.

**Deliverables:**

- Fetch attestation with a fresh random nonce and explicit model selector.
- Require Ed25519 and the expected key size.
- Extract the SPKI fingerprint from the exact successful TLS connection.
- Verify Intel TDX quote and collateral locally.
- Verify NVIDIA NRAS evidence locally.
- Verify report data binds raw model key bytes and raw TLS fingerprint bytes,
  plus the exact requested nonce.
- Verify expected model identity when present.
- Apply the Stage 01 trust policy and expose normalized checks/evidence.
- Disable redirects for attestation endpoints unless the protocol explicitly
  authenticates the redirect target.
- Set strict response, quote, JWT, and collateral limits.

**Verification:**

- Pure tests for each evidence field and status.
- Adversarial cases: replayed nonce, wrong model, wrong key, wrong algorithm,
  wrong TLS SPKI, malformed quote, expired/invalid collateral, unacceptable TCB,
  invalid NVIDIA signature/verdict/nonce, missing evidence, and oversize data.
- Test that two TLS connections with different SPKIs cannot be combined into
  one accepted result.
- Lane C live test against a real provider.
- Evidence output contains fingerprints and claims only, never full secret or
  plaintext data.

**Exit gate:** A live current report passes locally, every binding mutation
fails, and the resulting UI claim is no stronger than the enforced policy.

### Stage 06 — Implement catalog and encrypted relay transport

**Objective:** communicate with Axiom's relay using typed, bounded,
provider-aware contracts without allowing the relay to establish trust.

**Deliverables:**

- Authenticated model-catalog client.
- Strict catalog schema validation and provider/driver resolution.
- Typed encrypted relay DTOs for requests and non-streaming/streaming responses.
- Bounded SSE parser with explicit accepted event names.
- Stable error mapping for authentication, credit exhaustion, rate limiting,
  invalid request, provider rejection, invalid response, cancellation, and
  transient transport failure.
- Retry rules limited to operations proven safe; no automatic replay after an
  ambiguous accepted inference request.
- Idempotency/correlation IDs if supported by the authoritative relay contract.

**Verification:**

- Catalog rejects duplicates, unknown providers/protocols, impossible limits,
  unsupported reasoning levels, malformed URLs, and oversized responses.
- SSE parser handles fragmented frames, comments/heartbeats, UTF-8 boundaries,
  clean completion, remote failure, abrupt EOF, and oversize frames.
- A malicious catalog cannot select loopback, link-local, or metadata endpoints
  unless explicitly permitted by a trusted provider configuration.
- Authentication values never occur in structured logs or error bodies.

**Exit gate:** Relay transport can carry only typed encrypted payloads and
metadata; it has no plaintext inference DTO.

### Stage 07 — Complete secure non-streaming inference

**Objective:** make one full verified encrypted request and decrypt its response
locally.

**Deliverables:**

- Establish a provider-owned verified session for the selected model.
- Encrypt every secret-bearing request field from Section 7.
- Build a bounded relay request containing ciphertext and allowed metadata.
- Verify the relay accepted the same client/model/protocol identity.
- Decrypt response text, reasoning, refusal, and tool calls locally.
- Validate response shape, tool indices/IDs/counts, finish reason, and usage.
- Emit an evidence receipt only after each real lifecycle step succeeds.

**Verification:**

- Lane B real-process non-stream request.
- Sentinel scan of serialized relay body and logs.
- Tampered ciphertext, wrong client key, mismatched model key, missing response,
  empty invalid response, excess tool calls, and invalid finish reason fail.
- Receipt cannot say completed unless attestation, session readiness, request
  encryption, relay acceptance, and response decryption all occurred.
- Lane D live non-stream request returns expected content without plaintext
  appearing in proxy logs or captured relay metadata.

**Exit gate:** One real live encrypted inference works, and all deterministic
tamper cases fail closed.

### Stage 08 — Add secure streaming and cancellation

**Objective:** stream decrypted local events while preserving confidentiality,
ordering, bounds, and correct terminal state.

**Deliverables:**

- Decrypt content, reasoning, refusal, and tool fragments incrementally.
- Enforce event sequence/order and per-stream aggregate limits.
- Normalize usage and finish events into `axiom-inference` values.
- Propagate backpressure from consumers.
- Propagate cancellation through secure client, relay body, and provider stream.
- Treat EOF without an authenticated terminal event as failure.
- Ensure dropping a proxy client records one cancellation, not a completion.

**Verification:**

- Lane B streaming through the real proxy process.
- Fragmentation at every byte boundary that can legally occur.
- Interleaved reasoning/content/tool deltas.
- Tampered middle delta, repeated/out-of-order sequence, invalid UTF-8 after
  decryption, oversized aggregate, remote cancellation, remote failure, abrupt
  EOF, client disconnect, and slow consumer.
- Lane D verifies visible streaming and cancellation against live Axiom.

**Exit gate:** Streaming has the same fail-closed guarantees as non-streaming,
and no cancellation path is recorded as success.

### Stage 09 — Prove encrypted tool semantics end to end

**Objective:** support the complete model tool protocol without confusing
transport correctness with tool authorization.

**Deliverables:**

- Encrypt tool name, description, JSON schema, and named tool choice.
- Decrypt non-streamed and fragmented streamed tool names/arguments.
- Support multiple and parallel tool calls while preserving index and call ID.
- Encrypt the assistant tool-call history and subsequent tool-result messages.
- Reject legacy/ambiguous forms not present in the specification.
- Validate JSON syntax and configured bounds before a client can execute a
  proposed call.
- Document that the proxy never executes tools; it only preserves the protocol.

**Verification matrix:**

| Case | Required assertion |
|---|---|
| One tool, non-streaming | Definition is encrypted; returned name/arguments decrypt exactly. |
| One tool, streaming | Arbitrary name/argument fragments assemble once in order. |
| Parallel tools | Independent indices/IDs remain independent. |
| `auto`, `none`, `required` | Choice semantics survive translation. |
| Named tool choice | Selected function name is encrypted. |
| Nested schema/Unicode | Exact JSON meaning round-trips. |
| Tool result follow-up | Result is encrypted and model produces final text. |
| Invalid JSON arguments | Client refuses execution; proxy does not repair silently. |
| Tampered tool ciphertext | Entire inference fails closed. |
| Wrong/missing index or ID | Rejected before client execution. |
| Oversized schema/arguments/result | Rejected at documented boundary. |
| Cancellation mid-call stream | No partial call is presented as executable. |

**Exit gate:** Deterministic tests prove every tool secret stays encrypted and
every reconstructed call is unambiguous. No tool is granted local authority by
this layer.

### Stage 10 — Replace AxiomCLI's development provider

**Objective:** use secure inference directly inside AxiomCLI without changing
the agent's policy authority.

**Deliverables:**

- Implement an AxiomCLI provider adapter over `axiom-secure-client`.
- Remove the default dependency on `AXIOM_BASE_URL=http://127.0.0.1:8484/v1`.
- Preserve streaming token usage, model picker, thinking level, compaction,
  title generation, cancellation, partial-context recovery, and session resume.
- Surface typed verification progress and exact failure state in the TUI/ACP
  event model.
- Keep tool proposal, permission evaluation, approval, execution, and audit in
  the existing AxiomCLI runtime.
- Preserve an explicitly named development adapter only if it remains needed
  for local tests; require an opt-in build/runtime flag and show `UNATTESTED
  DEV`. It must never be selected by production defaults.

**Verification:**

- All existing AxiomCLI tests pass against the new contract.
- Lane F real read-only and confirmed-edit agent loops.
- Escape/cancellation interrupts attestation, inference, and tool execution.
- Resume preserves model/thinking/permission state without persisting secure
  session keys.
- A failed half-streamed response remains in model context according to the
  existing recovery contract without falsely marking the run complete.
- Security status cannot be spoofed by model text or tool output.

**Exit gate:** AxiomCLI works with live secure inference and no proxy process,
while its existing deterministic policy remains the only source of tool
authority.

### Stage 11 — Build the pure OpenAI compatibility adapter

**Objective:** translate common OpenAI clients without allowing OpenAI wire
details to shape the secure-client core.

**Deliverables:**

- Request/response types for `GET /v1/models` and
  `POST /v1/chat/completions`.
- Strict validation and mapping for messages, reasoning, tools, tool choice,
  response format, sampling, limits, streaming options, finish reason, and
  usage.
- OpenAI-compatible SSE chunk generation and `[DONE]` handling.
- Stable local error envelopes without reflecting sensitive upstream bodies.
- Capability-aware rejection of unsupported parameters.

**Verification:**

- Golden request/response fixtures from multiple compatible clients.
- Stream and non-stream outputs deserialize using an independent OpenAI client
  library.
- Property tests ensure translation is lossless for supported domain values.
- Unsupported/legacy combinations fail explicitly.
- Dependency inspection shows no crypto, attestation, server, or environment
  logic in the adapter.

**Exit gate:** The adapter can be tested as pure data transformation and has no
means to bypass the secure client.

### Stage 12 — Implement the real `axiom-proxy` executable

**Objective:** expose the secure client as a robust loopback sidecar.

**Deliverables:**

- Add `apps/axiom-proxy` as a Rust binary workspace member.
- Compose Axum, `axiom-openai-compat`, and `axiom-secure-client`.
- Bind to `127.0.0.1` and `::1` only where supported and tested.
- Require a separate high-entropy local client token.
- Read the Axiom credential from a secret-safe application configuration path;
  never accept it in a URL or log it.
- Implement health, readiness, models, and chat-completions endpoints.
- Apply connection, request-body, stream, concurrency, and timeout limits.
- Graceful shutdown cancels in-flight work and emits terminal metadata.
- Reserve stdout for future desktop control messages and send sanitized logs to
  stderr when run as a sidecar.

**Verification:**

- Spawn the actual built binary in integration tests and discover readiness via
  HTTP/control output.
- Non-loopback bind attempts fail in the initial version.
- Missing/wrong local credential returns `401` without touching Axiom.
- Axiom credential is absent from child-client environment and all artifacts.
- Port collision, invalid configuration, upstream auth error, out-of-credit
  `402`, rate limit, cancellation, and shutdown are stable.
- Local request concurrency and body limits resist trivial exhaustion.

**Exit gate:** The proxy is a real independently runnable process, has no
plaintext upstream path, and can be safely supervised by the future desktop
application.

### Stage 13 — Deterministic process-level proxy verification

**Objective:** prove the complete local stack under controlled adversarial
conditions before spending live-service time.

**Deliverables:**

- A reusable test harness that starts:
  1. an adversarial encrypted relay/provider fixture;
  2. the actual `axiom-proxy` binary;
  3. an independent HTTP/OpenAI test client.
- Captures for relay request JSON, stream events, proxy metadata logs, and exit
  states without recording plaintext secrets unnecessarily.
- Tests for every Section 7 field and Stage 09 tool case.

**Verification:**

- Run all non-stream, stream, tools, cancellation, error, limit, and tamper
  scenarios through TCP and the real executable.
- Assert controlled plaintext sentinels never appear in captured relay JSON or
  proxy logs.
- Assert the client sees correct plaintext only after local decryption.
- Rebuild `ProxyCore`/server state where relevant and ensure metadata history
  cannot turn failed runs into verified completions.
- Repeated and parallel runs prove sessions and keys cannot cross-talk.
- A soak test exercises many short streams and cancellations while checking
  file descriptors, tasks, and memory return to bounded levels.

**Exit gate:** Lane B is fully green and is suitable as the ordinary CI
regression gate.

### Stage 14 — Live Axiom chat and attestation verification

**Objective:** prove the implementation interoperates with the real Axiom
service rather than only with local fixtures.

**Deliverables:**

- A secret-safe live smoke script that starts the actual proxy on a randomly
  selected free loopback port.
- Live catalog validation and model selection based on advertised capabilities.
- Live attestation evidence summary with protocol, checks, timestamps, and
  fingerprints only.
- Live non-stream, stream, usage, reasoning, cancellation, and credit-error
  checks.
- Retained failure artifacts with automatic redaction; successful runs clean up
  disposable state.

**Verification:**

- Lane C and D against `https://api.axiom.stream` or an explicitly configured
  production-equivalent endpoint.
- A unique prompt sentinel is returned correctly but absent from proxy logs.
- No test accepts a pre-recorded attestation as live evidence.
- Disconnect the network or corrupt one expected binding and confirm a visible
  fail-closed error with no fallback.
- Run with at least two available model IDs when the catalog permits, ensuring
  the attested model selection cannot bleed between them.

**Exit gate:** Real attestation and both response modes pass using a development
credential, and negative live checks fail safely.

### Stage 15 — Real proxy tool-loop verification

**Objective:** prove that an unmodified external coding client can use tools
through the encrypted proxy from proposal through final answer.

**Deliverables:**

- Rebuild the behavior of the reference
  `scripts/smoke_opencode_tools.sh` as an AxiomAI-owned test harness.
- Use a disposable workspace and isolated HOME/XDG directories.
- Give OpenCode a generated local proxy credential, never the Axiom credential.
- Deny shell, network, external directories, subagents, and unrelated tools.
- Seed one file with `TOKEN=before` and require:
  1. read the file;
  2. edit exactly one value to `TOKEN=after`;
  3. read the file again;
  4. return a non-empty final response.
- Save structured client events and sanitized proxy logs on failure.

**Verification:**

- The real proxy, real live Axiom provider, and real OpenCode process all run.
- Client JSON events prove completed read, edit/write, read, and final text in
  that order.
- The final file matches byte-for-byte and no unexpected workspace entry exists.
- Tool definitions, selected calls, arguments, and results are covered by the
  relay sentinel/privacy assertions.
- The Axiom credential is absent from OpenCode environment, config, events, and
  every captured log.
- Repeat with two independent tool calls in one turn when a suitable live model
  supports parallel calls.
- Cancel one run mid-stream and prove no partial tool call executes.

**Exit gate:** Lane E passes repeatedly. This is the required evidence for the
claim that "tools work through the proxy."

### Stage 16 — Hardening, packaging, and completion audit

**Objective:** close system-level gaps and produce a release-candidate decision.

**Deliverables:**

- Fuzz targets for ciphertext parsing, evidence parsing, SSE, OpenAI requests,
  and tool fragments.
- Concurrency, cancellation, soak, and resource-bound tests.
- Dependency/advisory/license review and reproducible sidecar packaging.
- Cross-platform loopback, TLS, process, and trust-store validation for every
  platform being claimed.
- Privacy review of logs, panics, metrics, crash artifacts, audit records, and
  desktop control messages.
- Updated configuration, troubleshooting, privacy, and threat-model docs.
- A migration note removing or quarantining the old unattested development
  provider from production builds.
- Independent clean-checkout revalidation of every applicable gate.

**Final verification:**

```text
Rust formatting/check/Clippy/tests
Cargo Deny
Lane A protocol suite
Lane B process integration suite
Lane C live attestation
Lane D live chat streaming/non-streaming
Lane E real OpenCode tool loop
Lane F direct AxiomCLI tool loop
release package build/install/start/stop/uninstall
credential and plaintext-sentinel scans
```

**Exit gate:** No blocker or high-severity finding remains inside the claimed
scope. A completion audit maps every requirement in this plan to current
evidence and explicitly lists anything deferred.

## 10. Error and status contract

Errors must be typed below the UI and mapped to clear local messages at the
application edge. At minimum distinguish:

- local configuration invalid;
- local proxy authentication failed;
- Axiom authentication failed;
- insufficient Axiom credit;
- model unavailable or capability mismatch;
- attestation unavailable;
- attestation evidence rejected;
- runtime identity not allowed by policy;
- secure-session establishment failed;
- request encryption failed;
- relay rejected encrypted request;
- response authentication/decryption failed;
- invalid provider response;
- transient network failure;
- rate limited;
- cancelled by local user;
- remote cancellation;
- client disconnected;
- internal bounded-resource rejection.

No application should pattern-match error strings. Error details must be
bounded and redacted. The proxy maps them to stable HTTP classes without
passing through arbitrary upstream bodies. AxiomCLI maps the same errors to
task state without losing a safely captured partial response.

## 11. Security evidence and observability

Each request should produce a metadata-only lifecycle receipt similar to:

```text
request id
model id
provider id
attestation protocol
E2EE protocol/version
trust-policy version
ordered verification checks and outcomes
model-key fingerprint (not full key)
TLS SPKI fingerprint
verification timestamps
session-established flag
request-encrypted flag
relay-accepted flag
response-decrypted flag
stream/non-stream
usage and terminal state
```

Receipts must never contain:

- API credentials;
- client private/public key pairs as a pair;
- shared secrets or derived keys;
- plaintext messages or tool fields;
- decrypted model output;
- full ciphertext unless an explicit, secure debugging workflow is designed;
- raw attestation artifacts in ordinary logs.

Security evidence is factual. A request is not `VerifiedCompleted` unless all
required verification, encryption, relay, decryption, and terminal checks
succeeded. Failure/cancellation updates are monotonic and cannot later become a
completion because of a delayed event.

## 12. Automated acceptance matrix

| Capability | Deterministic | Live Axiom | Real external client |
|---|---:|---:|---:|
| Model discovery | yes | yes | observed through client |
| Fresh attestation | negative fixtures | yes | transitively required |
| Cross-language crypto | yes | yes | transitively required |
| Non-stream response | yes | yes | optional |
| Streamed text/reasoning | yes | yes | yes |
| Token usage | yes | yes | captured |
| Single tool call | yes | yes | yes |
| Streamed tool fragments | yes | yes | yes |
| Tool-result follow-up | yes | yes | yes |
| Parallel tools | yes | when model supports | when model supports |
| Tamper rejection | yes | controlled negative | n/a |
| Client cancellation | yes | yes | yes |
| Out-of-credit mapping | fixture | safe account/environment | visible to client |
| Plaintext/log leakage scan | yes | yes | yes |
| Credential isolation | yes | yes | yes |
| Direct AxiomCLI secure loop | yes | yes | n/a |

## 13. Decisions intentionally deferred

- Exact API shape of a second provider. The common abstraction must permit it,
  but the second real integration should drive any further extraction.
- Attestation caching. Start with fresh establishment per request; introduce a
  cache only with a documented freshness, revocation, key-rotation, TLS-binding,
  and concurrent-use analysis.
- `/v1/responses`, embeddings, and Anthropic compatibility.
- LAN exposure, multi-user proxy hosting, and remote proxy authentication.
- Persisted security receipts beyond the minimum metadata required by the
  applications.
- Desktop keychain and sidecar-update implementation.

## 14. Definition of complete

This plan is complete when:

1. `axiom-secure-client` is the only production Axiom inference transport used
   by both AxiomCLI and `axiom-proxy`.
2. The provider abstraction contains no application-level `if near` branches
   and can describe a provider with different TEE/E2EE mechanics.
3. NEAR `near-v2` passes authoritative cross-language vectors and live local
   attestation verification.
4. The real proxy serves models and streaming/non-streaming chat completions on
   loopback with local authentication.
5. Every secret-bearing message and tool field has a passing relay-confidentiality
   assertion.
6. Tampering, contract mismatch, missing terminal events, and attestation
   failure all fail closed.
7. A real OpenCode read/edit/read workflow passes through the live proxy.
8. A real AxiomCLI tool workflow passes through the shared secure client without
   the proxy.
9. No Axiom key, client secret, plaintext sentinel, or private key appears in
   repository files, logs, test artifacts, or release packages.
10. Every stage has a `PASS` review record and an independent final audit finds
    no unresolved blocker or high-severity issue.

Until all ten conditions hold, documentation and UI must describe the feature
as under development and must not claim production-secure inference.
