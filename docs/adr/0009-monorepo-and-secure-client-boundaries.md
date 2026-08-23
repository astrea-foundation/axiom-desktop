# ADR 0009: Product monorepo and secure-client boundaries

Status: accepted

## Context

ADR 0004 intentionally kept the early coding agent in one package until a
boundary had multiple real consumers. AxiomCLI, a standalone local proxy, and
the Electron desktop application now establish those consumers. They need the
same Axiom inference contracts and, once implemented, the same attested and
end-to-end encrypted client without sharing presentation or server concerns.

## Decision

Organize deployable products under `apps/`, reusable Rust libraries under
`crates/`, and reusable JavaScript or generated protocol packages under
`packages/`.

The dependency direction is:

```text
apps/axiomcli -----------------------------+
                                             |
apps/axiom-proxy -> axiom-openai-compat -----+--> axiom-secure-client
                                             |             |
                                             +------> axiom-inference

apps/desktop --spawns--> axiomcli acp
             --spawns--> axiom-proxy
```

`axiom-inference` owns transport-independent request, response, model, tool,
usage, and streaming-event contracts. It does not own HTTP, OpenAI wire types,
attestation, encryption, agent policy, persistence, or presentation.

The future `axiom-secure-client` crate will own Axiom server communication,
attestation verification, secure-session establishment, E2EE, authentication,
retry/cancellation behavior, model discovery, and typed security state. Its
normal production API must fail closed: inference cannot silently fall back to
an unattested or unencrypted connection.

The future `axiom-openai-compat` crate will translate OpenAI-compatible wire
requests and streaming responses to and from `axiom-inference` contracts. It
will not establish trust or perform encryption.

`apps/axiom-proxy` will be a separate Rust executable. It will bind to loopback
by default, authenticate local clients, expose the selected compatibility
endpoints, and use `axiom-secure-client` for upstream inference. AxiomCLI will
call the same secure client in-process and will not require the local proxy.

The Electron main process will supervise `axiomcli acp` for agent workflows and
`axiom-proxy` for proxy workflows. ACP remains an agent protocol; proxy
lifecycle control uses a separate small, versioned protocol. Sidecars reserve
stdout for machine-readable control messages and send logs to stderr.

Application configuration remains in each composition root. Shared crates take
typed constructor arguments and do not read environment variables, choose
permission profiles, or silently enable development modes.

## Initial migration

The first migration is deliberately mechanical and narrow:

1. Move the existing package to `apps/axiomcli` without behavioral changes.
2. Extract the already-stable inference data contracts to
   `crates/axiom-inference` while retaining the development provider adapter in
   AxiomCLI.
3. Reserve documented locations for the proxy, desktop, and their control
   protocol without creating fake implementations.

The provider trait and error contract remain in AxiomCLI during this migration.
They move only when the direct secure client is introduced and its failure
model can be defined from the real Axiom protocol.

## Consequences

- TUI inference has no unnecessary localhost HTTP hop.
- The proxy and TUI can share all attestation, E2EE, model, and transport logic.
- OpenAI compatibility remains an edge adapter rather than shaping Axiom's
  internal protocol.
- Electron can ship and supervise independently versioned Rust sidecars.
- Moving the current package changes source paths but preserves the `axiomcli`
  Cargo package and executable names.
- Empty architectural placeholders do not claim functionality that has not yet
  been implemented or tested.
