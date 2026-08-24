# `@axiom/axiom-acp-client`

Node/Electron-main client for `axiomcli acp`. It owns subprocess framing,
capability negotiation, request cancellation, interaction routing, ordered
state reduction, crash handling, and extension resynchronization.

This package is not renderer-safe by itself because it can spawn a process.
Electron main owns it and exposes the narrower API in
`apps/desktop/src/preload/agent-api.ts`. Do not add a raw `request(method)` API
to preload.

Generate DTOs after changing the Rust extension contract:

```text
pnpm --filter @axiom/axiom-acp-client generate
pnpm --filter @axiom/axiom-acp-client typecheck
pnpm --filter @axiom/axiom-acp-client test
```

Tests launch the real debug AxiomCLI binary with deterministic runners. Build
it first with `cargo build -p axiomcli` on a clean checkout.
