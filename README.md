# Axiom

Axiom's native clients: an Electron desktop app, AxiomCLI with a terminal UI and
headless/ACP modes, and a local OpenAI-compatible proxy. AxiomCLI owns Desktop's
agent runtime and account-local conversations. Every remote inference request
uses a registered provider-E2EE protocol with locally verified TEE evidence.

**Start with the [documentation index](docs/README.md).** It covers product behavior,
development, security and release acceptance.
For packaged applications, use the [installation guide](docs/installing-desktop.md).

## Develop from source

Use the Rust toolchain pinned in [rust-toolchain.toml](rust-toolchain.toml), Node
22 and pnpm 10 as used by CI. On Linux, local process tools require Bubblewrap.

```sh
pnpm install --frozen-lockfile
pnpm desktop
```

`pnpm desktop` builds AxiomCLI and starts Electron. Terminal-only development:

```sh
cargo run --locked -p axiomcli -- tui
cargo run --locked -p axiomcli -- exec "inspect this repository"
cargo run --locked -p axiomcli -- acp
```

See [development and checks](docs/development.md),
[configuration](docs/configuration.md), and [troubleshooting](docs/troubleshooting.md).
Native sign-in uses the system browser and OS credential store. Automation keys
are a separate CLI/proxy option; Desktop uses the native account session.

## Repository ownership

| Path | Responsibility |
|---|---|
| `apps/desktop/` | Electron presentation, native integration and sidecar supervision |
| `apps/axiomcli/` | TUI, headless CLI, ACP, tools, account sessions and local storage |
| `apps/axiom-proxy/` | Authenticated loopback OpenAI-compatible server |
| `crates/` | Shared inference, attestation/E2EE, wire translation and ACP contracts |
| `packages/` | Typed ACP client, native branding and release-manifest validation |
| `docs/` | Product, architecture, security, development and release guides |

The companion [axiom-platform repository](https://github.com/astrea-foundation/axiom-platform)
owns the hosted API, account website, marketing/download website, public release
API and CDN publishing. Both repositories build independently. See
[architecture](docs/architecture.md) for the boundaries.

Ongoing work goes to `dev`. Releases are promoted to `main` through the
[contribution workflow](CONTRIBUTING.md); installer production and publication
follow the [release procedure](docs/releasing.md).

Licensed under [Apache-2.0](LICENSE); see [third-party notices](THIRD_PARTY.md).
