# AxiomAI

This repository is the AxiomAI product monorepo. Rust applications live under
`apps/`, reusable Rust contracts and infrastructure live under `crates/`, and
the ACP client packages live under `packages/`.

The implemented applications today are AxiomCLI—an early Rust coding agent with
a terminal interface and Agent Client Protocol (ACP) control mode—and
`axiom-proxy`, an authenticated loopback OpenAI-compatible sidecar. The
frameless Electron app supervises and controls AxiomCLI over ACP. AxiomCLI is
the Desktop chat runtime and durable state owner; Electron owns presentation.

## Workspace layout

```text
apps/
  axiomcli/       Coding-agent TUI, headless CLI, and ACP server
  axiom-proxy/    Local OpenAI-compatible Rust sidecar
  desktop/        Electron shell and typed AxiomCLI sidecar boundary
crates/
  axiom-acp-extension/ Versioned first-party ACP extension DTOs/schema
  axiom-inference/      Transport-independent inference contracts
  axiom-openai-compat/  Pure OpenAI wire translation
  axiom-secure-client/  Provider-modular attestation and E2EE
packages/
  axiom-acp-client/ Typed Node/Electron-main ACP client and state reducer
  sidecar-protocol/ Reserved for generated/shared desktop control types
```

The proxy and AxiomCLI share the secure Axiom client; the TUI performs secure
inference directly rather than routing through localhost. See
[`docs/adr/0009-monorepo-and-secure-client-boundaries.md`](docs/adr/0009-monorepo-and-secure-client-boundaries.md).
The current account, local-storage, native-auth, and invoice design is defined
in the
[`account/auth/billing clean-slate plan`](docs/account-auth-billing-clean-slate-plan.md).
The older [`v1 plan`](docs/v1-clean-slate-refactor-plan.md) is retained as design
history rather than the current account contract.

## Development status

The current secure path verifies provider hardware, model/key identity, and the
live TLS endpoint and applies provider E2EE. The desktop-to-AxiomCLI control
plane and product parity are documented in
[`docs/acp-desktop-parity-matrix.md`](docs/acp-desktop-parity-matrix.md).

## Commands

```text
cargo run -p axiomcli -- --help
cargo run -p axiomcli -- doctor
cargo run -p axiomcli -- tui
cargo run -p axiomcli -- exec "inspect this repository"
cargo run -p axiomcli -- --system-prompt-file ./AGENT.md tui
cargo run -p axiomcli -- acp
cargo run -p axiomcli -- sessions list
cargo run -p axiomcli -- plans list
cargo run -p axiomcli -- permissions add-project-rule --action ask --effect network
cargo run -p axiom-proxy -- --help
pnpm desktop
```

`pnpm desktop` builds the current AxiomCLI binary first, then launches Electron
with that binary as its ACP sidecar. It is the normal development command on
Linux, macOS, and Windows.

`axiomcli acp` reserves stdout for newline-delimited ACP JSON-RPC. Logs and
diagnostics go to stderr.

`--system-prompt-file PATH` is a global launch option, so it works with `tui`,
`exec`, and `acp` whether it appears before or after the subcommand. The UTF-8
file replaces AxiomCLI's default behavioral instructions for that process.
AxiomCLI still appends trusted runtime context (the selected model, thinking
level, workspace) and its untrusted-input rule. Prompt files are capped at
256 KiB and their contents are neither written to session journals nor exposed
in debug output. A resumed session uses the prompt file supplied to that launch;
without the option, it uses the default instructions.

The TUI uses the current terminal viewport (it does not resize the terminal).
It needs at least 42×10 cells. Press `?` for its keyboard reference;
search, task-detail folding, entry/diff viewing, reduced motion, ASCII, and
no-color modes are built in. Each prompt is presented as one task that updates
in place: checklist progress and compact tool summaries remain visible while
raw reasoning, arguments, output, and diffs stay in expandable details. The
assistant's final Markdown response remains attached to that task. AxiomCLI
does not capture pointer input, so normal terminal text selection and copying
remain available.

The composer also owns a typed slash-command layer. Slash input is handled by
the application and is never sent to the model as a prompt:

```text
/permissions
/model
/resume
/delete
/thinking <minimal|low|medium|high|xhigh>
/compact [what the successor summary should preserve]
/login
/logout
/account
/security
/refresh
/help
```

Pressing `/` from transcript focus returns to the composer with a slash ready.
While typing a command, `Tab` completes command names and valid
thinking values; repeated presses cycle matches. Enter on an invalid
prefix with a valid completion performs that same completion instead of showing
an error. `/permissions` opens a keyboard-controlled window rather than taking
an argument. Its Tools tab describes and selects the active tool profile; the
separate Crypto tab is reserved for future wallet and transaction permissions.
`/model` loads the configured provider's real model catalog on demand and opens
a searchable, scrollable picker. Each priced entry shows its live uncached
input/output rate per one million tokens. Model IDs cannot be entered manually.

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
cancels compaction just like foreground agent work. AxiomCLI also compacts
automatically when provider-reported active usage reaches 85% of the selected
model's live context window. Automatic and explicit compaction use the same
durable successor-summary path.

The terminal shares the desktop's eagle-and-globe mark, neutral surfaces, and
orange activity indicators. Model, reasoning, agent permissions, and Web status
sit beneath the composer. Use `/model`, `/thinking`, and `/permissions` to change
the corresponding controls; Tab enters the transcript, Enter expands task
details, and `v` opens the full view.

`/theme dark`, `/theme light`, and `/theme terminal` change appearance for the
current session. `AXIOMCLI_THEME=dark|light|terminal` sets the startup appearance
(default: dark). Terminal mode inherits the terminal's foreground/background.
Color depth remains independent (`AXIOMCLI_COLOR` / `NO_COLOR`).

Web starts off in each terminal session. `/web on` opts into external search and
URL fetching for subsequent turns; `/web off` disables both. Search queries and
fetched URLs are shared with external services and are not end-to-end encrypted.
Model traffic continues to use locally verified provider E2EE.

Active tasks use a small orange pulse and a traveling signal on the task rail.
Waiting is static; completion settles directly into a check. The composer status
dot keeps its explicit outcome color. `AXIOMCLI_REDUCED_MOTION=1` disables motion;
`AXIOMCLI_ASCII=1` uses ASCII indicators and a compact welcome wordmark.

## Installation

Development installs can use `cargo install --locked --path apps/axiomcli`. The
AxiomCLI coding-agent release artifact is Linux x86-64 GNU:

```text
scripts/verify-release dist/axiomai-VERSION-x86_64-unknown-linux-gnu.tar.gz \
  dist/axiomai-VERSION-x86_64-unknown-linux-gnu.tar.gz.asc
scripts/install-release dist/axiomai-VERSION-x86_64-unknown-linux-gnu.tar.gz PREFIX
scripts/uninstall-release PREFIX
```

The tagged release workflow also builds native Axiom Desktop artifacts for
macOS Intel, macOS Apple Silicon, Windows x64, and Windows ARM64. Those packages
embed an architecture-matched AxiomCLI sidecar and are signed on their native
runners. See [`docs/platform-support.md`](docs/platform-support.md) for the
separate automated-package and interactive-acceptance support boundaries.

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
AXIOM_BASE_URL=https://api.axiom.stream
AXIOM_AUTH_URL=https://auth.axiom.stream
AXIOM_MODEL=your-model-id
```

On first use, AxiomCLI opens `/login`. Native authorization creates a
short-lived, PKCE-bound request in the system browser, where the user chooses
passkey, Google, email/password, or Ethereum-wallet sign-in. The loopback
callback is only a best-effort completion notification; AxiomCLI continues
polling when the callback is unreachable. The resulting access token stays in
memory and the rotating refresh token is stored only in the operating-system
credential store. There is no plaintext-file fallback. `/account` validates the
native session and `/logout` revokes and clears it. `AXIOM_API_KEY` remains a
separate environment override for standalone automation; Axiom Desktop removes
it from the sidecar environment and has no manual API-key sign-in path.
Credits belong to the authenticated account. Top-ups use Shieldz Zcash invoices
through the hosted account surface, not an Ethereum funding contract; Desktop
and the model receive no wallet custody or transaction authority.

## Permission profiles

- `none`: no tools are exposed. The model can only respond in conversation.
- `web`: only local `web_search` and `fetch_url` are exposed.
- `observe`: only read-only tools are exposed, including workspace inspection
  and explicitly trusted read-only MCP tools; web and mutation tools are hidden.
- `confirm` (default): every available tool is exposed and every invocation
  requires a fresh confirmation. Approval is one-shot, not remembered.
- `full_access`: every available tool is exposed and tool invocations run
  automatically.

Full Access removes tool-profile restrictions and prompts; it does not disable
the runtime's structural safety boundaries. Canonical workspace containment,
credential isolation, SSRF defenses, process/output bounds, and explicit
user/project deny rules still apply. Tool permissions never grant wallet or
transaction authority. Crypto permissions will be modeled separately in the
Crypto tab.

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
higher-risk tool; Confirm asks before it and Full Access permits it within the
same structural runtime boundaries.

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
permission_profile = "confirm"
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

Session journals are redacted, account-scoped SQLite records under the platform
data directory. Signing out closes the active account store; signing into
another account never opens the previous account's threads.
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
- Axiom's relay catalog supplies live NEAR/Phala context and output limits to
  the secure client and ACP model catalog, along with the same live per-token
  prices used for billing. The TUI uses the selected model's 85% threshold for
  automatic compaction and shows normalized input/output prices per million
  tokens in the model picker; local context settings remain only as
  configurable memory-safety ceilings.
- Linux is the currently exercised AxiomCLI coding-agent platform. Desktop CI
  also builds and tests native macOS Intel/Apple-Silicon and Windows x64/ARM64
  packages; target-machine interactive acceptance remains a separate release
  gate.
- ACP v1 and durable `session/load` are supported. Per-session MCP configuration
  is not advertised yet; MCP servers come only from trusted user configuration.
- SearXNG upstream engines can throttle or fail independently.
- Background process work is limited to four concurrent children. It has
  independent IDs, time/output budgets, ordered provenance, and inherits parent
  cancellation. Subagents are deliberately not implemented yet.
- AxiomCLI's local coding tools remain Linux x86-64 GNU only because they require
  Bubblewrap. The macOS and Windows Desktop profile is Web-only and does not
  claim local coding-tool isolation.
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
