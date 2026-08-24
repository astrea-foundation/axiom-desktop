# Axiom desktop application

This directory is reserved for the Electron desktop application. The Electron
main process will supervise Rust sidecars; renderer processes will not receive
raw secrets, wallet keys, or direct process-spawning authority.

The Agent surface will control `axiomcli acp`. The Proxy surface will control
the standalone `axiom-proxy` process through a separate versioned lifecycle
protocol. The desktop implementation is intentionally left to its dedicated
design and implementation effort.

See [`../../docs/adr/0009-monorepo-and-secure-client-boundaries.md`](../../docs/adr/0009-monorepo-and-secure-client-boundaries.md).
