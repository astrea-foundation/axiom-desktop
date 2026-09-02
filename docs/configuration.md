# Configuration reference

Axiom loads built-in defaults, shared trusted configuration, frontend trusted
configuration, supported environment overrides, and—only for CLI workspaces—the
selected repository's restrictive configuration. Unknown additive top-level
and MCP keys are ignored with a warning so a newer config does not prevent an
older binary from starting. Invalid known values, policy-rule fields,
permission names, and authority-bearing structures remain errors. TUI, `exec`,
and generic ACP resolve the project layer after canonicalizing their workspace.

On Linux, shared configuration is normally
`$XDG_CONFIG_HOME/axiom/config.toml`; CLI and desktop overrides live in
`axiom/cli/config.toml` and `axiom/desktop/config.toml`. The equivalent
platform-native roots are used on macOS and Windows. `axiomcli doctor` prints
the exact roots, databases, and desktop chat directory.

Local data uses one Axiom namespace with explicit ownership:

```text
<local data>/axiom/
  shared/identity/client-id
  shared/credentials/
  shared/trust-policy.json
  cli/state.sqlite3
  desktop/state.sqlite3
  desktop/chat/
```

The protected credential fallback and installation identity are shared so the
desktop and CLI use the same account. Conversation databases are separate.
Versioned databases are upgraded through ordered, transactional migrations.
Unversioned pre-release layouts are not guessed at and must be reset explicitly.

During pre-release development, reset exactly one frontend database with
`axiomcli reset-local-state --frontend cli --confirm` or
`axiomcli reset-local-state --frontend desktop-chat --confirm`. The command
removes only that profile's `state.sqlite3` and SQLite sidecars. It preserves
configuration, shared identity, credentials, trust policy, the other frontend's
database, and `desktop/chat/`; no reset happens implicitly.

`axiomcli acp --frontend desktop-chat` is a product profile: its cwd is the
canonical `desktop/chat` directory, MCP servers are disabled, its embedded
desktop system prompt cannot be replaced with `--system-prompt-file`, and its
permission profile is exactly `web`. Desktop parses known shared configuration
and applies its common fields, but ignores additive unknown keys and shared
`permission_profile` and `mcp_servers` because those fields configure the CLI.
A non-Web permission setting in `desktop/config.toml`, the environment, ACP
mode, or extension settings is rejected.

## Trusted user file

```toml
model = "deepseek-v4-flash"
base_url = "https://api.axiom.stream"
permission_profile = "confirm"
searxng_url = "http://127.0.0.1:8888"
max_context_bytes = 16777216
max_context_tokens = 4194304
max_tool_output_bytes = 131072
request_timeout_secs = 120
animation = true

[[policy_rules]]
action = "deny"                 # ask | deny; allow rules are rejected
effect = "network"              # optional without resource_prefix
resource_prefix = "https://"    # optional, requires effect
reason = "offline workspace"    # optional user-facing explanation

[[mcp_servers]]
name = "documentation"
command = "documentation-mcp"
args = []
read_only_tools = ["lookup"]
```

Tool profiles are `none`, `web`, `observe`, `confirm`, and `full_access`.
`confirm` is the default. None exposes no tools; Web exposes only web search and
page fetching; Observe exposes only read-only tools; Confirm exposes all tools
but asks once for every invocation; Full Access exposes all tools without tool
approval prompts. Structural runtime boundaries and explicit deny rules still
apply to every profile. Permission names are strict; retired aliases are
rejected. Resume restores the thread's durable profile.

## Session slash commands

While a TUI session is idle, `/permissions`, `/model`, and `/thinking` change
session-scoped settings used by subsequent turns. `/permissions` takes no
argument and opens a two-tab window. Tools selects and explains the five tool
profiles; Crypto is marked Coming soon and will later hold wallet, network,
spending, and transaction authority separately. `/model` opens a searchable
picker populated from the provider's discovered catalog; arbitrary model IDs
are not accepted. These changes are stored on the thread.
When an ordinary prompt is submitted, its active model becomes the durable
application-wide default for future chats. Resume derives the model from the
latest prompt in the selected transcript, so a later `/model` selection that
never ran does not change the resumed model. The configured `model` remains the
fallback when no prompt has established a remembered default.
Thinking is always one of `minimal`, `low`, `medium`, `high`, or `xhigh`; it
defaults to `medium`. The thinking level used for an ordinary prompt becomes
the application-wide default for future chats. Resume explicitly restores the
selected transcript's final model, thinking level, and permission profile.
`/resume` opens a searchable picker containing non-archived transcripts whose
canonical workspace matches the current TUI. ACP clients use native
`session/load` for the equivalent operation.
`/delete` lists saved transcripts from every workspace except the active
transcript. Space toggles one row, Ctrl+A toggles all visible search matches,
Enter advances to a permanent-deletion warning, and `y` atomically deletes the
selection.
`/compact [focus]` uses the current session
model to summarize live history with an empty tool set, then installs that
summary as successor context. It can be cancelled with Escape.
`/login` offers browser sign-in or an existing Axiom API key. Browser sign-in
completes through PKCE polling even when the loopback callback is blocked; the
callback only accelerates completion. `/account` validates the current credential without revealing it, and
`/logout` clears a persisted credential. Login state is application-wide and is
never written to a conversation timeline.
`/security` opens the latest in-memory attestation inspector. It shows hardware
checks, endpoint and encryption-key bindings, workload measurements, parsed
Compose services, and the exact quote-bound `app_compose` manifest. `R` performs
a fresh verification; evidence is never written into the chat transcript.
`/refresh` runs that fresh attestation verification directly without opening the
inspector. The header animates while it is running and reports failures
prominently.
`/help` prints the full command list, and typing `/` opens filtered usage hints.

The first ordinary prompt also starts a parallel, tool-free title request. Its
short result fills the session title only when one has not already been set;
failure is non-fatal and does not alter the conversation or tracked token usage.

The direct secure client validates the changed model ID against Axiom's signed-in
catalog and establishes a verified provider session. Only verified public
material is reused, for at most four minutes and never beyond provider evidence,
backend lease, or signed-policy expiry. Every inference still creates fresh
client key material, nonces, request ciphertext, and receipt bindings. The
explicit thinking level is sent only when the selected model advertises it.
Unsupported model/thinking combinations fail locally rather than being guessed
from model names.

The authenticated relay publishes a signed trust manifest. AxiomCLI verifies it
locally and keeps the highest accepted sequence in
`<local data>/axiom/shared/trust-policy.json`. Network failure uses the still-valid
last-known-good manifest; an expired policy, invalid signature, rollback,
same-sequence equivocation, or unsafe NVIDIA/workload rule fails closed. The
cache is atomic, bounded, non-symlinked, and shared by CLI and desktop so the two
frontends cannot disagree after a policy upgrade.

The catalog may contain offerings for more than one provider. Production builds
currently register NEAR (`near-v2` / `near-tdx-nvidia-v1`) and Phala
(`phala-aci-v2` / `phala-aci-tdx-v1`). A catalog entry for an unregistered future
provider is ignored without hiding otherwise supported models; an entry that
claims a registered provider with the wrong protocol tuple still fails closed.
Phala offerings are chat-only for now: streaming is supported, but tool and
parallel-tool capabilities are disabled until every tool field has a receipt-bound
AAD contract. The outer `phala-aci-v2` contract accepts exactly two measured
artifact subprofiles: `dstack-aci-draft-d567f6` for the currently deployed,
pinned draft and `aci-v1-08d550` for the pinned standard ACI artifact. Selection
uses the measured repository commit plus a structurally disjoint report shape;
unknown commits and hybrid draft/standard documents fail closed.

Limits are validated as follows:

| Key | Valid range/default |
|---|---|
| `max_agent_steps` | Optional 1–256; disabled when omitted (default) |
| `max_context_bytes` | 4 KiB–64 MiB; default 16 MiB local safety ceiling |
| `max_context_tokens` | 1 Ki–4 Mi tokens; default 4 Mi model-independent safety ceiling |
| `max_tool_output_bytes` | 1 KiB–64 MiB; default 128 KiB |
| `max_turn_secs` | Optional 1–86,400; disabled when omitted (default) |
| `request_timeout_secs` | 1–3,600; default 120 |

`request_timeout_secs` bounds provider HTTP requests. It does not time out a
permission card or structured question. Human waits end on a response,
cancellation/disconnect, or an enabled whole-turn limit.

Normal context rollover is model-relative: the relay reads each curated
offering's current `context_length` and `max_output_length` from the live NEAR
or Phala catalog, and AxiomCLI automatically compacts at 85% of that context
window. `max_context_bytes` and `max_context_tokens` can deliberately lower
the local hard ceiling, but their defaults do not replace provider metadata.

`base_url` and `searxng_url` must be HTTP(S) URLs. MCP names must be unique;
name and command cannot be empty. A listed `read_only_tools` entry is an
explicit user trust decision. An MCP server’s own annotation never grants that
classification.

Agent-step and whole-turn time limits are off by default. To opt into them,
add numeric values to the trusted user configuration, for example:

```toml
max_agent_steps = 64
max_turn_secs = 3600
```

These are execution safeguards only. AxiomCLI does not display step counts or
time-limit counters in the conversation UI.

## Repository file

`.axiomcli/config.toml` is untrusted repository input. It may select a model,
disable animation, reduce budgets, narrow the permission profile, and add
`ask`/`deny` rules. It cannot set provider/search endpoints, start MCP servers,
select plan mode, broaden a trusted profile, increase a budget, replace trusted
rules, or define an allow rule.

Use the explicit writer instead of hand-editing when adding a restriction:

```text
axiomcli permissions add-project-rule --action ask --effect network \
  --resource-prefix https://example.com/ --reason "review this destination"
```

The command appends atomically and accepts only `ask` or `deny`.

## Environment

| Variable | Meaning |
|---|---|
| `AXIOM_API_KEY` | optional Axiom bearer override for automation; takes precedence over persisted login and must never be put in TOML or argv |
| `AXIOM_BASE_URL` | trusted Axiom relay override; defaults to `https://api.axiom.stream` |
| `AXIOM_WEB_URL` | trusted browser-login website; defaults to `https://axiom.stream` |
| `AXIOM_MODEL` | model override |
| `AXIOM_SEARXNG_URL` | SearXNG URL override |
| `AXIOM_PERMISSION_PROFILE` | permission-profile override |
| `AXIOMCLI_CREDENTIAL_STORE` | `keyring` or `file`; normally keyring with an owner-only file fallback |
| `AXIOMCLI_COLOR` | `truecolor`, `256`, `16`, or `none` |
| `NO_COLOR` | disables color when present |
| `AXIOMCLI_ASCII=1` | replaces decorative Unicode glyphs |
| `AXIOMCLI_REDUCED_MOTION=1` | disables animation |

`axiom-proxy` is configured separately because the future desktop app will
supervise it as a sidecar:

| Variable | Meaning |
|---|---|
| `AXIOM_API_KEY` | upstream Axiom credential, held only by the sidecar |
| `AXIOM_PROXY_TOKEN` | independent 32+ byte local-client bearer token |
| `AXIOM_BASE_URL` | Axiom relay URL; defaults to `https://api.axiom.stream` |
| `AXIOM_PROXY_BIND` | numeric loopback address; defaults to `127.0.0.1:8484` |
| `AXIOM_PROXY_MAX_CONCURRENCY` | bounded simultaneous requests; defaults to 8 |
| `AXIOM_PROXY_COMPAT` | `lenient` (default) or `strict`; see below |

### Unsupported request parameters

The Axiom relay accepts a fixed field set, so an OpenAI parameter with no relay
equivalent can only be dropped or refused. `AXIOM_PROXY_COMPAT` chooses which.

`lenient`, the default, drops the parameter and answers the request. This exists
because the person running a third-party application usually cannot edit that
application's request builder, and an answer is more useful to them than a
rejection. Drops are never silent: parameters that could have changed the answer
are named on the `x-axiom-ignored-parameters` response header and in a
`request_parameters_ignored` line on stderr for the supervisor. Parameters that
cannot affect the answer at all — `user`, `metadata`, `store`, `service_tier` —
are dropped without a report.

`strict` refuses any request carrying a parameter that cannot be applied,
naming them on the same header. Use it when a caller must be certain that every
parameter it sent was honored.

Two cases are refused in both modes, because dropping them would return a
confidently wrong answer rather than a degraded one:

- `n` greater than 1, which the caller would index into.
- A message content part that is not text. No catalog model advertises image,
  audio, or file input, so answering would mean ignoring what was asked about.

Text delivered as a content part array is accepted in both modes and joined with
newlines, since the OpenAI SDKs emit that shape even for plain text.

At startup, the AxiomCLI TUI validates a discovered credential with Axiom. A
rejected or revoked persisted credential is removed and the TUI opens the login
surface; an `AXIOM_API_KEY` override must instead be unset by the caller. A
temporary network failure is reported but does not erase the credential.
After credential validation, the TUI establishes and locally verifies a real
model-specific attested session without sending a prompt. The header animates
while this preflight runs and fails visibly if verification does not succeed.
Inference requests use only the bounded verified-material cache described above;
startup preflight is never an authorization to outlive those bounds.
Browser authorization listens on a random `127.0.0.1` port and
expires after the server-defined short lifetime. The protected-file fallback is
stored beneath the platform data directory with `0700` directory and `0600`
file permissions on Unix. Prefer the system keyring on shared machines.

With animation enabled, active task cards redraw while foreground/background
work or a short completion transition is active. The blossom uses a weighted
rest/bloom/hold/wilt cadence rather than a constant-rate spinner. The composer
status marker is always static and changes color with current/last terminal
state. Reduced-motion mode keeps task decoration static as well.

Child development commands do not inherit the parent environment. Only the
small key allowlist implemented by the process boundary can be set per command.
