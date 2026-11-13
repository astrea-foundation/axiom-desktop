# ADR 0009: Product monorepo and secure-client boundaries

Status: accepted

## Context

The early coding agent stayed in one package until a boundary had multiple
real consumers. AxiomCLI, a standalone local proxy, and
the Electron desktop application now establish those consumers. They need the
same Axiom inference contracts and the same attested, end-to-end encrypted
client without sharing presentation or server concerns.

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
```

`axiom-inference` owns transport-independent request, response, model, tool,
usage, and streaming-event contracts. It does not own HTTP, OpenAI wire types,
attestation, encryption, agent policy, persistence, or presentation.

The `axiom-secure-client` crate owns Axiom server communication,
attestation verification, secure-session establishment, E2EE, authentication,
retry/cancellation behavior, model discovery, and typed security state. Its
normal production API must fail closed: inference cannot silently fall back to
an unattested or unencrypted connection.

The `axiom-openai-compat` crate translates OpenAI-compatible wire requests and
streaming responses to and from `axiom-inference` contracts. It does not
establish trust or perform encryption.

`apps/axiom-proxy` is a separate Rust executable. It binds to loopback
only, authenticates local clients, exposes the selected compatibility
endpoints, and uses `axiom-secure-client` for upstream inference. AxiomCLI calls
the same secure client in-process and does not require the local proxy.

The Electron main process supervises `axiomcli acp` for Desktop chat. The Proxy
screen separately supervises `axiomcli desktop-proxy`, which hosts the same
proxy library using an account-bound native session. Desktop chat does not use
the loopback proxy. The standalone executable remains available to users of the
OpenAI-compatible integration. ACP reserves stdout for
machine-readable control messages and sends logs to stderr.

Application configuration remains in each composition root. Shared crates take
typed constructor arguments and do not read environment variables, choose
permission profiles, or silently enable development modes.

## Consequences

- TUI inference has no unnecessary localhost HTTP hop.
- The proxy and TUI can share all attestation, E2EE, model, and transport logic.
- OpenAI compatibility remains an edge adapter rather than shaping Axiom's
  internal protocol.
- Electron supervises a Rust sidecar; current release packages require a
  matching product version and architecture.
- Moving the current package changes source paths but preserves the `axiomcli`
  Cargo package and executable names.
