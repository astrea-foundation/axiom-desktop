# `@axiom/axiom-acp-client`

Node/Electron-main client for `axiomcli acp`: subprocess framing, negotiation,
cancellation, interaction routing, ordered state reduction, crash handling and
snapshot recovery. It can spawn processes and is not renderer-safe. Electron
main exposes a narrower [preload API](../../apps/desktop/src/preload/agent-api.ts).

The [ACP guide](../../docs/acp-compatibility.md) owns the wire contract, feature
versions, recovery semantics and generation commands. Build the debug sidecar
with `cargo build -p axiomcli` before running this package's tests:

```sh
pnpm --filter @axiom/axiom-acp-client generate
pnpm --filter @axiom/axiom-acp-client typecheck
pnpm --filter @axiom/axiom-acp-client test
```
