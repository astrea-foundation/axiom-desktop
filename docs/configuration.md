# Configuration reference

AxiomCLI merges configuration in this order: built-in defaults, trusted user
configuration, repository configuration, then the supported environment
overrides. Unknown TOML keys and invalid values are errors.

On Linux, trusted configuration is normally
`$XDG_CONFIG_HOME/axiomcli/config.toml` or
`~/.config/axiomcli/config.toml`. `axiomcli doctor` prints the exact path and
the session database location for the current platform.

## Trusted user file

```toml
model = "glm-5-2"
base_url = "http://127.0.0.1:8484/v1"
permission_profile = "confirm"
searxng_url = "http://127.0.0.1:8888"
max_agent_steps = 24
max_context_bytes = 2097152
max_context_tokens = 262144
max_tool_output_bytes = 131072
max_turn_secs = 900
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

Limits are validated as follows:

| Key | Valid range/default |
|---|---|
| `max_agent_steps` | 1–256; default 24 |
| `max_context_bytes` | 4 KiB–64 MiB; default 2 MiB |
| `max_context_tokens` | 1 Ki–4 Mi tokens; default 256 Ki |
| `max_tool_output_bytes` | 1 KiB–64 MiB; default 128 KiB |
| `max_turn_secs` | 1–86,400; default 900 |
| `request_timeout_secs` | 1–3,600; default 120 |

`base_url` and `searxng_url` must be HTTP(S) URLs. MCP names must be unique;
name and command cannot be empty. A listed `read_only_tools` entry is an
explicit user trust decision. An MCP server’s own annotation never grants that
classification.

## Repository file

`.axiomcli/config.toml` is untrusted repository input. It may select a model,
disable animation, reduce budgets, narrow the permission profile, and add
`ask`/`deny` rules. It cannot set provider/search endpoints, start MCP servers,
select plan mode, broaden a trusted profile, increase a budget, replace trusted
rules, or define an allow rule.

## Environment

| Variable | Meaning |
|---|---|
| `AXIOM_API_KEY` | optional provider bearer credential; never put it in TOML or argv |
| `AXIOM_BASE_URL` | trusted provider URL override |
| `AXIOM_MODEL` | model override |
| `AXIOM_SEARXNG_URL` | SearXNG URL override |
| `AXIOM_PERMISSION_PROFILE` | permission-profile override |
| `AXIOMCLI_COLOR` | `truecolor`, `256`, `16`, or `none` |
| `NO_COLOR` | disables color when present |
| `AXIOMCLI_ASCII=1` | replaces decorative Unicode glyphs |
| `AXIOMCLI_REDUCED_MOTION=1` | disables animation |

Child development commands do not inherit the parent environment. Only the
small key allowlist implemented by the process boundary can be set per command.
