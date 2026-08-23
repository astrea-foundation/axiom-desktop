# AxiomAI

This repository is the AxiomAI product monorepo. Rust applications live under
`apps/`, reusable Rust contracts and infrastructure live under `crates/`, and
future JavaScript packages live under `packages/`.

The implemented applications today are AxiomCLI—an early Rust coding agent with
a terminal interface and Agent Client Protocol (ACP) control mode—and
`axiom-proxy`, an authenticated loopback OpenAI-compatible sidecar. The Electron
desktop directory still documents its future boundary and contains no product
implementation yet.

## Workspace layout

```text
apps/
  axiomcli/       Coding-agent TUI, headless CLI, and ACP server
  axiom-proxy/    Local OpenAI-compatible Rust sidecar
  desktop/        Reserved for the Electron application
crates/
  axiom-inference/      Transport-independent inference contracts
  axiom-openai-compat/  Pure OpenAI wire translation
  axiom-secure-client/  Provider-modular attestation and E2EE
packages/
  sidecar-protocol/ Reserved for generated/shared desktop control types
```

The proxy and AxiomCLI share the secure Axiom client; the TUI performs secure
inference directly rather than routing through localhost. See
[`docs/adr/0009-monorepo-and-secure-client-boundaries.md`](docs/adr/0009-monorepo-and-secure-client-boundaries.md).
Implementation is governed by the gated
[`Axiom secure inference and proxy plan`](docs/axiom-secure-inference-and-proxy-plan.md).

## Development status

The implementation follows the gated plan in
[`docs/axiomcli-implementation-plan.md`](docs/axiomcli-implementation-plan.md).
Stages 00–16 pass, including the independent completion audit. Each stage has
an evidence-backed review under `docs/reviews/`.
The current secure path verifies provider hardware, model/key identity, and the
live TLS endpoint and applies E2EE; an approved workload allowlist and the
desktop application remain future work.

## Commands

```text
cargo run -p axiomcli -- --help
cargo run -p axiomcli -- doctor
cargo run -p axiomcli -- tui
cargo run -p axiomcli -- exec "inspect this repository"
cargo run -p axiomcli -- acp
cargo run -p axiomcli -- sessions list
cargo run -p axiomcli -- plans list
cargo run -p axiomcli -- permissions add-project-rule --action ask --effect network
cargo run -p axiom-proxy -- --help
```

`axiomcli acp` reserves stdout for newline-delimited ACP JSON-RPC. Logs and
diagnostics go to stderr.

The TUI uses the current terminal viewport (it does not resize the terminal).
It needs at least 42×10 cells. Press `?` for its keyboard/mouse reference;
search, task-detail folding, entry/diff viewing, reduced motion, ASCII, and
no-color modes are built in. Each prompt is presented as one task that updates
in place: checklist progress and compact tool summaries remain visible while
raw reasoning, arguments, output, and diffs stay in expandable details. The
assistant's final Markdown response remains attached to that task.

The composer also owns a typed slash-command layer. Slash input is handled by
the application and is never sent to the model as a prompt:

```text
/permissions <plan|observe|confirm|workspace|autonomous>
/model
/resume
/delete
/thinking <minimal|low|medium|high|xhigh>
/compact [what the successor summary should preserve]
/help
```

Pressing `/` from transcript focus returns to the composer with a slash ready.
While typing a command, `Tab` completes command names and valid
permission/thinking values; repeated presses cycle matches. Enter on an invalid
prefix with a valid completion performs that same completion instead of showing
an error. `/model` loads the configured provider's real model catalog on demand
and opens a searchable, scrollable picker. Model IDs cannot be entered manually.

`/resume` opens a searchable, scrollable list of saved transcripts from the
current workspace. Selecting one restores its transcript, model context, and
session-scoped settings without replaying interrupted actions. A new chat starts
with the model used for the most recently submitted prompt across AxiomCLI. A
resumed chat instead restores the model that was active for that transcript's
latest prompt; selecting a model without subsequently using it does not alter
either remembered value.

`/delete` opens all saved transcripts except the active one. Space toggles a
row, Ctrl+A toggles every visible search match, and Enter opens a permanent
deletion confirmation. The selected rows are deleted together only after `y`.

`/compact` is a real, cancellable inference operation: it exposes no tools,
creates a successor summary, replaces the model's live conversation context,
and journals that summary so resume restores the compacted context. Escape
cancels compaction just like foreground agent work. Repeated compaction is
lossy, so it is explicit in this early version rather than automatic.

Active task cards use a small cherry-blossom animation and a moving accent rail.
Its weighted cycle rests, blooms, holds, and quickly wilts instead of advancing
at a mechanically uniform speed. The composer status row uses a static dot whose
color reflects active, waiting, successful, cancelled, or failed state.
`AXIOMCLI_REDUCED_MOTION=1` disables task animation; `AXIOMCLI_ASCII=1` uses an
equivalent `. * o O` sequence.

## Installation

Development installs can use `cargo install --locked --path apps/axiomcli`. The
supported release artifact is currently Linux x86-64 GNU:

```text
scripts/verify-release dist/axiomai-VERSION-x86_64-unknown-linux-gnu.tar.gz \
  dist/axiomai-VERSION-x86_64-unknown-linux-gnu.tar.gz.asc
scripts/install-release dist/axiomai-VERSION-x86_64-unknown-linux-gnu.tar.gz PREFIX
scripts/uninstall-release PREFIX
```

See [`docs/releasing.md`](docs/releasing.md) for reproducible packaging and
signing, and [`docs/configuration.md`](docs/configuration.md),
[`docs/privacy.md`](docs/privacy.md), and
[`docs/troubleshooting.md`](docs/troubleshooting.md) for operation details.
Parser fuzzing and corpus-handling instructions are in
[`docs/fuzzing.md`](docs/fuzzing.md).

## Provider configuration

Production AxiomCLI talks directly to Axiom's relay. It discovers the selected
model, verifies current TEE evidence locally, binds the attested model key to
the live provider TLS endpoint, and encrypts inference end to end in-process.

```text
AXIOM_API_KEY=...                 required; never put it in repository config
AXIOM_BASE_URL=https://api.axiom.stream
AXIOM_MODEL=your-model-id
```

Do not put API keys in repository configuration or command-line arguments.
The legacy OpenAI-compatible development adapter is available only in builds
compiled with `--features development-provider` and only when
`AXIOMCLI_UNATTESTED_DEV=1`; the TUI labels that path `UNATTESTED DEV`.

## Permission profiles

- `plan`: workspace reads plus writes to revisioned `.axiomcli/plans` artifacts;
  all code edits, processes, network, and MCP calls are deterministically denied.
- `observe`: workspace reads only; mutation, processes, network, and MCP denied.
- `confirm`: reads are automatic; every other effect asks the TUI/ACP client.
- `workspace`: in-root edits and a narrow development-command allowlist are
  automatic; network, MCP, deletion, and other programs ask.
- `autonomous`: normalized in-scope operations are automatic, while shell and
  deletion remain confirmation-gated and hard denials still win.

Repository mutations such as `git commit`, `reset`, `checkout`, and unknown Git
subcommands are separately classified and require one-shot confirmation in
every mutating profile. Credential/keychain access is represented explicitly
and hard-denied in this early release.

Use `axiomcli tui --plan` or `axiomcli exec --plan "..."` for plan mode.
Review durable plans with `axiomcli plans list/show/comment/request-revision/
revise/propose/approve/abandon`. Plan approval changes artifact state; it does
not itself execute the plan or grant broader authority.

Agents can ask up to three structured questions with required or optional
answers. The TUI opens a focused interaction card, supports numbered choices,
paste, and backward navigation, and preserves any in-progress message draft.
ACP maps the same schema to form elicitation when the client advertises that
currently unstable capability; unsupported or malformed interactions close
with an explicit terminal failure instead of leaving the session stuck.

The agent tool surface also exposes a bounded session-scoped text-file diff and
structured task/checklist progress. Plan proposals pause for approve, revision,
or abandon input through the same TUI/ACP question path; revision comments are
stored with line ranges.

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

Configured MCP servers are discovered over stdio. A crashed server reconnects
on demand; only explicitly trusted read-only tools can be retried, and only when
their schema is unchanged. Calls with possible side effects are never replayed
after an ambiguous disconnect. MCP output is labeled untrusted, redacted, and
bounded before it reaches either front end.
The configured executable itself is trusted user-selected local code and starts
with user authority; its protocol data and claims remain untrusted. Repository
configuration cannot add MCP executables.

Session journals are redacted SQLite records under the platform data directory.
`sessions list/show/export/rename/archive/unarchive/prune` and the TUI's
multi-select `/delete` picker manage them. Use
`axiomcli tui --resume SESSION_ID` to restore a transcript and bounded model
context, or use `/resume` in the TUI. After the first prompt is accepted, a
parallel tool-free inference request creates a short transcript title; it never
overwrites a title chosen with `sessions rename`. ACP advertises `session/load`
when its durable store opens. Interrupted actions are reported as unknown and
never replayed.
Headless `exec` uses the same runtime and creates a durable terminal turn rather
than bypassing the event journal.

## Known early-version limits

- Production inference verifies the current provider attestation per request;
  attestation caching and approved-workload measurement allowlists are later.
- `/model` validates selections against the provider's discovered catalog.
  `/thinking` is always explicit, defaults to `medium`, becomes the default for
  future chats after it is used for a prompt, and is restored per transcript.
  Unsupported levels are rejected against the selected model's catalog entry.
- Axiom's relay catalog now supplies authoritative context limits to the secure
  client. The TUI still uses its configured local compaction safety budget and
  does not yet render a model-relative context percentage.
- Linux is the currently exercised development platform.
- ACP v1 and durable `session/load` are supported. Per-session MCP configuration
  is not advertised yet; MCP servers come only from trusted user configuration.
- SearXNG upstream engines can throttle or fail independently.
- Background process work is limited to four concurrent children. It has
  independent IDs, time/output budgets, ordered provenance, and inherits parent
  cancellation. Subagents are deliberately not implemented yet.
- The release package supports Linux x86-64 GNU only. macOS and Windows remain
  unsupported until their sandbox, PTY, and installer gates match Linux.
- Active TUI transcript content is retained in memory until the session closes;
  durable exports and loads are bounded.

## Quality checks

```text
cargo fmt --all --check
cargo check --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo deny check
cargo check --manifest-path fuzz/Cargo.toml --bins
```

The required deterministic evaluation lane is
`scripts/axiomcli-eval deterministic`. Budgeted live-model and full real-service
runs are opt-in via `scripts/axiomcli-eval live|full`; see
[`docs/evaluations.md`](docs/evaluations.md) for scoring and the recorded
baseline.

## Reference policy

Other coding agents are behavioral references only. AxiomCLI code and tests are
written from its own contracts; reference code is not copied or mechanically
ported. See the plan's clean-room policy and repository inventory.
