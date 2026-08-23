# Axiom secure proxy

`axiom-proxy` is the standalone loopback sidecar for the future desktop app.
It exposes the initial OpenAI-compatible `/v1/models` and
`/v1/chat/completions` surface while using `axiom-secure-client` for local TEE
verification and E2EE.

Required environment:

```text
AXIOM_API_KEY=...            upstream Axiom credential
AXIOM_PROXY_TOKEN=...        separate random local-client credential (32+ bytes)
```

Optional environment:

```text
AXIOM_BASE_URL=https://api.axiom.stream
AXIOM_PROXY_BIND=127.0.0.1:8484
AXIOM_PROXY_MAX_CONCURRENCY=8
```

Start it with `cargo run --locked -p axiom-proxy`. The process emits one
structured readiness object on stdout for a supervisor. Sanitized diagnostics
go to stderr. Only numeric loopback bind addresses are accepted.

Local clients authenticate with `Authorization: Bearer $AXIOM_PROXY_TOKEN`.
They never receive the upstream Axiom credential. The proxy translates tool
semantics but never executes or authorizes a tool.
