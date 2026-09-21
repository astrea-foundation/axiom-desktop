# Axiom native documentation

These guides describe the checked-in native implementation. Source and workflow
review: **2026-09-21**. A documented build target or old test result does not establish
that a particular installer is published or that a live deployment has passed acceptance.

## Use Axiom

| Guide | Owns |
|---|---|
| [Install and update](installing-desktop.md) | Package formats, bundled CLI and update installation |
| [Desktop](desktop.md) | Chat, Agent/Web controls, editing, queues, evidence, credit and usage UI |
| [CLI and TUI](cli.md) | Commands, terminal interaction, plans and session management |
| [Local proxy](proxy.md) | Loopback setup, credentials and OpenAI compatibility |
| [Configuration](configuration.md) | Configuration precedence, paths, limits and environment settings |
| [Privacy](privacy.md) | What leaves the device, local storage and credential handling |
| [Troubleshooting](troubleshooting.md) | Diagnosis and recovery for supported operations |

## Build and integrate

| Guide | Owns |
|---|---|
| [Architecture](architecture.md) | Components, account/storage ownership and backend boundary |
| [ACP contract](acp-compatibility.md) | Standard ACP, negotiated features, recovery and DTO generation |
| [Security and trust operations](threat-model.md) | Trust boundaries, provider verification and signed-policy handling |
| [NEAR v3](protocols/near-v3.md) | Worker sessions, Ed25519 receipts and relay routing |
| [Tinfoil EHBP](protocols/tinfoil-ehbp-v1.md) | Router verification and authenticated encrypted completion |
| [Development](development.md) | Build/check commands, Desktop development and visual assets |
| [Evaluations](evaluations.md) | Deterministic, live-provider and real-search lanes |
| [Release procedure](releasing.md) | Target matrix, signing, packaging and publication handoff |
| [Versioning](versioning.md) | Product, storage and protocol compatibility |
| [Validation and outstanding work](validation.md) | Source evidence and release checks still requiring recorded outcomes |

The repository's [contribution workflow](../CONTRIBUTING.md) owns branch and
promotion rules. Hosted API, auth, payments and deployment guidance belongs to
[axiom-platform documentation](https://github.com/astrea-foundation/axiom-platform/blob/dev/docs/README.md).

## Decisions and historical evidence

- [Architecture decisions](adr/README.md) explain the rationale and identify superseded details.
- [Historical plans and audits](archive/README.md) retain dated evidence and original unfinished tasks.
- [Test reports](testing/README.md) retain scoped findings, fixes and measurement limits.

Current behavior belongs in one guide per topic. Component READMEs link here.
When behavior changes, update its guide with the code and link the relevant source
or test. Move useful superseded records to the archive, carry unresolved work into
[validation](validation.md), and repair incoming links. Runtime prompts, generated
contracts and license/provenance files stay beside their owning code.
