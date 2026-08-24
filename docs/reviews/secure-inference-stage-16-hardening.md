# Secure inference Stage 16 review — hardening and packaging

Decision: **PASS (reviewed 2026-08-23)**

Implementation commits under review:

- `1fff529` (`test: harden secure parser and proxy boundaries`)
- `44615b9` (`build: package secure proxy sidecar`)
- `841b35d` (`docs: quarantine legacy provider and document secure runtime`)
- `44c339b` (`test: verify direct secure AxiomCLI tool loops`)
- `7abdf55` (`build: audit fuzz harness dependencies`)

Claimed platform: Linux x86-64 GNU only. The Electron application and sidecar
supervision protocol remain outside this plan.

## Delivered evidence

- Five `cargo-fuzz` targets cover ciphertext, evidence, SSE, OpenAI request,
  and tool-fragment parsing. Each completed 2,000 coverage-instrumented
  executions with a 16,384-byte input limit and no crash.
- The actual proxy process fixture completed 24 streams and 12 cancellations,
  then remained within a 24-descriptor and 96-MiB RSS growth allowance.
- Existing deterministic suites cover bounded concurrency, cancellation,
  terminal-state monotonicity, long streams, repeated background work, and
  resource budgets.
- The reproducible archive is named
  `axiomai-0.1.0-x86_64-unknown-linux-gnu.tar.gz` and contains both
  `bin/axiomcli` and `bin/axiom-proxy`. Two independent builds had the same
  SHA-256 digest:
  `50e249958ab37c2c142891e19a21b189ca40c266a567ef1eaa634592639e28ab`.
- The archive passed verification, install, both binary version checks,
  loopback proxy start/readiness/SIGINT stop, credential/log scans, and
  uninstall.
- The legacy plaintext-compatible provider is absent from default production
  builds. It requires both the `development-provider` feature and the explicit
  `AXIOMCLI_UNATTESTED_DEV=1` runtime opt-in.
- Configuration, privacy, troubleshooting, threat-model, platform, migration,
  proxy, release, and fuzzing documentation describes the implemented trust
  boundary without claiming an approved workload identity.

## Direct AxiomCLI live lane

`scripts/smoke-live-axiomcli` passed twice against `glm-5-2` through
`axiom-secure-client` directly, with no localhost proxy:

- an observe-profile turn read a controlled file and returned its marker;
- a confirm-profile turn emitted an ACP `session/request_permission`, accepted
  `allow_once`, changed exactly `TOKEN=before\n` to `TOKEN=after\n`, read it
  back, and returned a final response;
- the isolated artifacts and operational logs contained neither the Axiom
  credential nor controlled plaintext sentinels.

## Commands

All passed:

```text
cargo fmt --all -- --check
cargo check --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo deny check
cargo check --manifest-path fuzz/Cargo.toml --all-targets
cargo deny --manifest-path fuzz/Cargo.toml --config deny.toml check
RUSTC_BOOTSTRAP=1 cargo fuzz run <each-target> -- -runs=2000 -max_len=16384
scripts/package-release
scripts/verify-release <archive>
scripts/install-release <archive> <prefix>
<prefix>/bin/axiomcli --version
<prefix>/bin/axiom-proxy --version
scripts/uninstall-release <prefix>
scripts/smoke-live-axiomcli
scripts/smoke-live-axiomcli
scripts/axiomcli-eval deterministic 2
bash -n scripts/package-release scripts/verify-release scripts/install-release \
  scripts/uninstall-release scripts/smoke-live-axiomcli \
  scripts/smoke-live-axiomcli-controller
```

An independent clone of `7abdf55a45aa641ac564a5b4218f8b78a99710a2`
at `/tmp/axiomai-clean-checkout.gO5neK/axiomai` repeated formatting, all-target
check, strict Clippy, all workspace tests, root and fuzz dependency policy,
fuzz-workspace compilation, and the deterministic evaluation twice. It ended
with a clean working tree.

## Privacy and failure inspection

- Proxy operational output contains readiness, request/model IDs, bounded
  verification facts and fingerprints, token totals, safe failure categories,
  and terminal state. It does not serialize credentials, plaintext requests,
  plaintext responses, tool fields, derived secrets, or raw evidence.
- The release-package credential-shape scan and exact live-credential scans of
  the repository and isolated artifacts passed. Controlled prompt, file,
  argument, result, and output sentinels were absent from operational logs and
  captured relay JSON. Synthetic credential-shaped strings remain only in
  source-level redaction tests and are not packaged.
- Production panics are limited to compile-time/static invariants; untrusted
  wire inputs return typed bounded errors. Test-only assertions and fixture
  panics are not reachable in release binaries.
- No metrics or crash-report upload exists. The future desktop control package
  contains documentation only and therefore has no message payload that could
  leak a secret.

## Findings

- Informational: workload measurements are not allowlisted yet. Evidence
  reports `approved_workload_identity=false` and does not overclaim the result.
- Informational: the current platform claim is Linux x86-64 GNU only; other
  trust stores, process models, and loopback behavior are unvalidated.
- Informational: fuzz campaigns are bounded release-gate runs, not a substitute
  for continuing scheduled fuzzing.
- Informational: the real insufficient-credit state was not deliberately
  induced; the stable 402 mapping is covered by the actual-process fixture.
- Blocker/high unresolved: none in the claimed scope.

## Exit criteria

Every Stage 16 deliverable and verification lane applicable to the claimed
platform has evidence. No unresolved blocker or high-severity finding remains.
The completion audit may close the plan.
