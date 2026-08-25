# Secure inference Stage 12 review — loopback proxy executable

Decision: **PASS (reviewed 2026-08-23)**

Implementation commit under review: `ae006f8` (`feat: implement secure loopback
proxy`)

## Evidence

- `axiom-proxy` is an independently runnable Axum process composed from the
  pure OpenAI adapter and `axiom-secure-client`; there is no alternate upstream
  chat transport.
- Numeric non-loopback binds are rejected before configuration or network use.
- A separate 32–512 byte local bearer token is accepted only through the
  environment, stored as a SHA-256 digest, and compared in constant time.
- The Axiom credential is accepted only through `AXIOM_API_KEY`, wrapped by the
  redacting credential type, and never returned to local clients.
- Health, readiness, models, non-stream chat, and bounded/backpressured SSE
  routes are present. Readiness says attestation is per request rather than
  falsely claiming current verification.
- JSON body size, local concurrency, upstream response/event/aggregate sizes,
  request timeouts, shutdown, and client-disconnect cancellation are bounded.
- Upstream errors are mapped to stable local envelopes without reflecting
  arbitrary upstream bodies.

## Commands and findings

All passed:

```text
cargo fmt --all -- --check
cargo check --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo deny check
AXIOM_API_KEY=<dummy> AXIOM_PROXY_TOKEN=<generated> cargo run -p axiom-proxy -- --bind 127.0.0.1:0
curl http://<announced-loopback-address>/healthz
curl -H 'Authorization: Bearer <wrong-token>' http://<announced-loopback-address>/v1/models
kill -INT <proxy-pid>
cargo run -p axiom-proxy -- --bind 0.0.0.0:8484
```

The real process emitted one structured readiness record, answered health,
rejected the wrong local credential with `401` before upstream access, exited
cleanly on SIGINT, and rejected the non-loopback bind.

- High, corrected in the reviewed commit: a proxy client could otherwise have
  reused the upstream Axiom credential; the proxy now requires a separate local
  credential.
- Medium, corrected in the reviewed commit: unbounded local work could exhaust
  the sidecar; body and concurrency limits are applied before inference.
- Informational: production chat interoperability is intentionally evaluated
  in the protected Stage 14 lane, not inferred from process startup.
- Blocker/high unresolved: none in Stage 12 scope.

## Exit criteria

The proxy is a real loopback-only supervised process, uses the secure client as
its sole upstream inference path, and exposes no plaintext-upstream mode. Stage
13 may proceed.
