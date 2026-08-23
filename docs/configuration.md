# Configuration reference

AxiomCLI merges configuration in this order: built-in defaults, trusted user
configuration, the selected workspace's repository configuration, then the
supported environment overrides. Unknown TOML keys and invalid values are
errors. TUI and `exec` resolve the full project layer after canonicalizing their
workspace. ACP resolves the permission profile and restrictive policy rules
independently for every session rather than using the server process's launch
directory; provider/model/tool construction remains process-wide for now.

On Linux, trusted configuration is normally
`$XDG_CONFIG_HOME/axiomcli/config.toml` or
`~/.config/axiomcli/config.toml`. `axiomcli doctor` prints the exact path and
the session database location for the current platform.

## Trusted user file

```toml
model = "glm-5-2"
base_url = "https://api.axiom.stream"
permission_profile = "confirm"
searxng_url = "http://127.0.0.1:8888"
max_context_bytes = 2097152
max_context_tokens = 262144
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

Profiles are `plan`, `observe`, `confirm`, `workspace`, and `autonomous`.
`plan` can also be selected for a new TUI/headless session with `--plan`.
Resume always restores the journaled profile; command-line flags cannot broaden
or silently change it.

## Session slash commands

While a TUI session is idle, `/permissions`, `/model`, and `/thinking` change
session-scoped settings used by subsequent turns. `/model` opens a searchable
picker populated from the provider's discovered catalog; arbitrary model IDs
are not accepted. These changes are journaled and restored with the session.
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
model to summarize live history with an empty tool set, then journals and
installs that summary as successor context. It can be cancelled with Escape.
`/help` prints the full command list, and typing `/` opens filtered usage hints.

The first ordinary prompt also starts a parallel, tool-free title request. Its
short result fills the session title only when one has not already been set;
failure is non-fatal and does not alter the conversation or its token counter.

The direct secure client validates the changed model ID against Axiom's signed-in
catalog, establishes a fresh verified provider session for each inference, and
sends the explicit thinking level only when the selected model advertises it.
Unsupported model/thinking combinations fail locally rather than being guessed
from model names.

Limits are validated as follows:

| Key | Valid range/default |
|---|---|
| `max_agent_steps` | Optional 1–256; disabled when omitted (default) |
| `max_context_bytes` | 4 KiB–64 MiB; default 2 MiB |
| `max_context_tokens` | 1 Ki–4 Mi tokens; default 256 Ki |
| `max_tool_output_bytes` | 1 KiB–64 MiB; default 128 KiB |
| `max_turn_secs` | Optional 1–86,400; disabled when omitted (default) |
| `request_timeout_secs` | 1–3,600; default 120 |

`request_timeout_secs` bounds provider HTTP requests. It does not time out a
permission card or structured question. Human waits end on a response,
cancellation/disconnect, or an enabled whole-turn limit.

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
| `AXIOM_API_KEY` | required Axiom bearer credential; never put it in TOML or argv |
| `AXIOM_BASE_URL` | trusted Axiom relay override; defaults to `https://api.axiom.stream` |
| `AXIOM_MODEL` | model override |
| `AXIOM_SEARXNG_URL` | SearXNG URL override |
| `AXIOM_PERMISSION_PROFILE` | permission-profile override |
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

The old unattested adapter is not part of the default build. For deliberate
local protocol development only, compile with `development-provider` and set
`AXIOMCLI_UNATTESTED_DEV=1`. Never distribute that feature as a production
binary. See the provider migration note for the exact quarantine boundary.

With animation enabled, active task cards redraw while foreground/background
work or a short completion transition is active. The blossom uses a weighted
rest/bloom/hold/wilt cadence rather than a constant-rate spinner. The composer
status marker is always static and changes color with current/last terminal
state. Reduced-motion mode keeps task decoration static as well.

Child development commands do not inherit the parent environment. Only the
small key allowlist implemented by the process boundary can be set per command.
