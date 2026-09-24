# Axiom native documentation

These guides describe Desktop, AxiomCLI and the local proxy. Release acceptance
must be recorded against the actual source revision and signed installers.

## Use Axiom

| Guide | Covers |
|---|---|
| [Install and update](installing-desktop.md) | Packages, bundled CLI and installation |
| [Desktop](desktop.md) | Conversations, Agent/Web controls, evidence, credit and usage |
| [CLI and TUI](cli.md) | Commands, terminal interaction, funding, plans and sessions |
| [Local proxy](proxy.md) | Loopback integration and OpenAI compatibility |
| [Configuration](configuration.md) | Configuration precedence, paths and limits |
| [Privacy](privacy.md) | Local storage, credentials and what leaves the device |
| [Troubleshooting](troubleshooting.md) | Diagnosis and recovery |
| [0.1.9 release notes](releases/0.1.9.md) | Changes in this release |
| [0.1.8 release notes](releases/0.1.8.md) | Previous release |

## Build and integrate

| Guide | Covers |
|---|---|
| [Architecture](architecture.md) | Runtime, storage and service boundaries |
| [ACP contract](acp-compatibility.md) | Negotiation, methods, authority and recovery |
| [Security](threat-model.md) | Provider verification, tool boundaries and trust policy |
| [Provider discovery](provider-catalogs.md) | Catalogs and encrypted attachments |
| [NEAR v3](protocols/near-v3.md) | Worker sessions, encryption and receipts |
| [Tinfoil EHBP](protocols/tinfoil-ehbp-v1.md) | Router verification and authenticated completion |
| [Development](development.md) | Build commands, checks and visual assets |
| [Evaluations](evaluations.md) | Deterministic and live-provider checks |
| [Update architecture](updates.md) | Signed metadata, installation ownership and recovery |
| [Repository setup](repository-setup.md) | Actions environments, credentials and repository handoff |
| [Release procedure](releasing.md) | Signing, packaging and publication |
| [Versioning](versioning.md) | Product, storage and protocol compatibility |
| [Release acceptance](validation.md) | Checks and evidence required for each release |

[CONTRIBUTING](../CONTRIBUTING.md) defines the branch and promotion rules.
Hosted API, auth, payments and deployment operations are maintained separately
in the private platform repository. Native builds and tests are self-contained.

Keep one current guide per topic. Internal plans, incident records and dated QA
captures belong in private maintainer records. Preserve outstanding qualification
requirements in [release acceptance](validation.md) when consolidating documents.
