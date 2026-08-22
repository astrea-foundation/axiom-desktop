# AxiomCLI

AxiomCLI is an early Rust coding agent with a terminal interface and an Agent
Client Protocol (ACP) control mode. It is intentionally under active design:
interfaces are small, configuration is explicit, and compatibility promises do
not begin until a tagged release says otherwise.

This repository contains the CLI/TUI only. It does not contain an Electron or
desktop application.

## Development status

The implementation follows the gated plan in
[`docs/axiomcli-implementation-plan.md`](docs/axiomcli-implementation-plan.md).
Each completed stage has a review record under `docs/reviews/`.

## Commands

```text
cargo run -p axiomcli -- --help
cargo run -p axiomcli -- doctor
cargo run -p axiomcli -- tui
cargo run -p axiomcli -- exec "inspect this repository"
cargo run -p axiomcli -- acp
cargo run -p axiomcli -- sessions list
```

`axiomcli acp` reserves stdout for newline-delimited ACP JSON-RPC. Logs and
diagnostics go to stderr.

## Provider configuration

Development inference uses an OpenAI-compatible chat-completions endpoint. It
does not claim TEE attestation or E2EE.

```text
AXIOM_API_KEY=...                 when required by the configured endpoint
AXIOM_BASE_URL=http://127.0.0.1:8484/v1
AXIOM_MODEL=your-model-id
```

Do not put API keys in repository configuration or command-line arguments.

## Permission profiles

- `observe`: workspace reads only; mutation, processes, network, and MCP denied.
- `confirm`: reads are automatic; every other effect asks the TUI/ACP client.
- `workspace`: in-root edits and a narrow development-command allowlist are
  automatic; network, MCP, deletion, and other programs ask.
- `autonomous`: normalized in-scope operations are automatic, while shell and
  deletion remain confirmation-gated and hard denials still win.

The model proposes operations but never authorizes them. Structured tools,
canonical paths, process groups, output caps, SSRF checks, and the policy engine
remain outside model control. Shell execution is a separate, explicitly
higher-risk tool and always requires approval.

## Real SearXNG integration

Web search uses a real SearXNG process, not a fake response server. The
development lifecycle is reproducible and loopback-only:

```text
scripts/searxng-dev bootstrap
scripts/searxng-dev start
scripts/searxng-dev query "Rust programming language"
scripts/searxng-dev integration-test
scripts/searxng-dev stop
```

See [`dev/searxng/README.md`](dev/searxng/README.md). `fetch_url` runs on the
user's machine and rejects local, private, metadata, and redirect-pivot
destinations.

## Configuration

Precedence is defaults → trusted user config → `.axiomcli/config.toml` →
environment. Layers merge by field, but repository-controlled project config
may only narrow permissions/budgets and cannot set endpoints or launch MCP
commands. `axiomcli doctor` prints the platform-specific trusted user config
path. Example user configuration:

```toml
permission_profile = "workspace"
searxng_url = "http://127.0.0.1:8888"

[[mcp_servers]]
name = "example"
command = "example-mcp-server"
args = []
read_only_tools = ["lookup"] # explicit user trust; server hints do not grant it
```

Session journals are redacted SQLite records under the platform data directory.
`sessions list/show/export/rename/archive/unarchive/prune` manages them. Use
`axiomcli tui --resume SESSION_ID` to restore a transcript and bounded model
context. ACP advertises `session/load` when its durable store opens. Interrupted
actions are reported as unknown and never replayed.

## Known early-version limits

- Development inference is plainly labeled `UNATTESTED DEV`; TEE/E2EE is later.
- Linux is the currently exercised development platform.
- ACP v1 is supported; per-session MCP configuration and `session/load` are not
  advertised yet.
- SearXNG upstream engines can throttle or fail independently.
- The TUI is intentionally compact; richer transcript search/diff navigation
  and release installers remain pre-1.0 work.

## Quality checks

```text
cargo fmt --all --check
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo deny check
```

## Reference policy

Other coding agents are behavioral references only. AxiomCLI code and tests are
written from its own contracts; reference code is not copied or mechanically
ported. See the plan's clean-room policy and repository inventory.
