# Axiom proxy application

This directory is reserved for the standalone Rust sidecar that will expose a
loopback OpenAI-compatible API and send requests upstream through the shared
attested, end-to-end encrypted Axiom client.

It is intentionally not a Cargo package yet. Its server, lifecycle, local
authentication, compatibility endpoints, and real secure-client integration
will be introduced together so the repository does not advertise a fake proxy.

See [`../../docs/adr/0009-monorepo-and-secure-client-boundaries.md`](../../docs/adr/0009-monorepo-and-secure-client-boundaries.md).
