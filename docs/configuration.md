# Configuration reference

Implementation: [config.rs](../apps/axiomcli/src/config.rs),
[paths.rs](../apps/axiomcli/src/paths.rs), and
[auth.rs](../apps/axiomcli/src/auth.rs). Product commands are in the
[CLI guide](cli.md); proxy settings are in the [proxy guide](proxy.md).

## Precedence and trust

Startup loads built-in defaults, shared trusted user configuration,
frontend-specific trusted configuration and supported environment overrides.
CLI workspaces then apply their restrictive project configuration, after which
supported environment values are reapplied as the final trusted override.
Launch arguments select workspace/frontend rather than arbitrary TOML keys.

Unknown additive top-level and MCP keys warn and are ignored. Invalid known
values, permission names, policy-rule fields and authority-bearing structures
are errors. CLI/TUI/standard ACP canonicalize the selected workspace before
applying its project restrictions.

`.axiomcli/config.toml` is untrusted repository input. It may select a model,
disable animation, reduce limits, narrow the tool profile, and add `ask`/`deny`
rules. It cannot choose provider/search endpoints, launch MCP servers, broaden
permissions or budgets, replace trusted rules, or add `allow` rules.

```sh
axiomcli permissions add-project-rule --action ask --effect network \
  --resource-prefix https://example.com/ --reason "review this destination"
```

The explicit writer appends restrictions atomically.

## Paths and local state

On Linux, config normally lives under `$XDG_CONFIG_HOME/axiom` (or the platform
fallback), with `config.toml`, `cli/config.toml` and `desktop/config.toml`.
macOS and Windows use their native config/data roots. Absolute `XDG_CONFIG_HOME`
and `XDG_DATA_HOME` overrides apply on every platform; relative values are rejected.
`axiomcli doctor` prints the resolved paths.

```text
<local data>/axiom/
  shared/identity/client-id
  shared/trust-policy.json
  accounts/<opaque-account-id>/
    cli/state.sqlite3
    desktop/state.sqlite3
    desktop/chat/<thread-id>/
```

Shared installation identity and the trust-policy rollback journal are not
conversation databases. Refresh tokens live only in the OS credential store.
No account database opens while signed out. Schema migrations are described in
[versioning](versioning.md).

`axiomcli reset-local-state --account ACCOUNT_ID --frontend cli --confirm`
(or `--frontend desktop-chat`) removes the selected account/frontend database and
SQLite sidecars. Stop Desktop and CLI first. The command runs before authentication
and schema loading, so it can remove obsolete state. Other accounts/frontends,
configuration, identity, vault credentials, trust policy and workspace files are
separate. Reset is destructive for the selected local history; it does not
reset a hosted account or payment ledger.

## Trusted user configuration

```toml
base_url = "https://api.axiom.stream"
permission_profile = "confirm"
web_search_provider = "axiom" # the only supported search provider
max_context_bytes = 16777216
max_context_tokens = 4194304
max_tool_output_bytes = 131072
request_timeout_secs = 120
animation = true

[[policy_rules]]
action = "deny"                 # ask | deny
effect = "network"
resource_prefix = "https://"     # requires effect
reason = "offline workspace"

[[mcp_servers]]
name = "documentation"
command = "documentation-mcp"
args = []
read_only_tools = ["lookup"]
tool_timeout_secs = 300 # per-call absolute deadline; 1–86400 seconds
```

Set `model` to an ID discovered by `axiomcli models` or use the model picker.
Model availability, prices, context capacity and reasoning capabilities come
from the authenticated catalog and are validated by the registered provider.
MCP names are unique and names/commands cannot be empty. `read_only_tools` is
an explicit trust decision about a user-configured executable; a server's own
annotation cannot grant that classification. MCP initialization/discovery retain a
30-second deadline. Tool calls default to five minutes and use the server's
`tool_timeout_secs`, including any progress notifications; progress does not reset
the absolute deadline. Stop and timeout send a bounded MCP cancellation notification.
A timed-out tool is never automatically replayed, even when marked read-only.
Desktop still does not launch MCP servers.

| Tool profile | Behavior |
|---|---|
| `none` | No model tools |
| `web` | Search and URL fetching, subject to explicit Web consent |
| `observe` | Read-only local tools and explicitly trusted read-only MCP tools |
| `confirm` | Default CLI profile; ask before each exposed tool invocation |
| `full_access` | Exposed tools run without routine approval prompts |

All profiles retain deny rules, input validation and the platform's actual
process boundaries. See [security](threat-model.md#local-tools-and-processes).

## Desktop profile

`axiomcli acp --frontend desktop-chat` starts with Agent off and profile `web`.
It ignores shared CLI `permission_profile`/`mcp_servers`, clears MCP servers,
rejects a non-Web startup profile from frontend config/environment, and uses
the embedded product prompt. `--system-prompt-file` cannot replace it.

After authentication, `desktopAgent@1` can atomically configure a thread's
Agent enablement, approval level and working directory while idle. This is the
supported path for Desktop local tools; generic ACP mode/config setters cannot
bypass it. A thread starts with its own managed directory. An existing absolute
custom directory can be selected through the UI. Every Desktop prompt carries
explicit Web consent independently of Agent permissions. See [Desktop](desktop.md).

## Limits

| Key | Accepted values | Default |
|---|---|---|
| `max_agent_steps` | Optional 1–256 | Disabled |
| `max_context_bytes` | 4 KiB–64 MiB | 16 MiB |
| `max_context_tokens` | 1 Ki–4 Mi tokens | 4 Mi tokens |
| `max_tool_output_bytes` | 1 KiB–64 MiB | 128 KiB |
| `max_turn_secs` | Optional 1–86,400 | Disabled |
| `request_timeout_secs` | 1–3,600 | 120 seconds |

Provider-specific exchanges also enforce their own bounded deadlines; this
setting is not a promise that every protocol uses one identical timeout.
Provider request timeouts do not expire human approval/question cards. Those
waits end on an answer, cancellation/disconnect, or an enabled whole-turn limit.

Automatic compaction is based on 85% of the selected model's advertised context
capacity, capped by local limits. Unsupported reasoning levels fail locally.
`minimal`, `low`, `medium`, `high`, and `xhigh` are the application enum; a model
may expose only a subset. See [CLI](cli.md) for remembered model/thinking settings.

Web search uses the authenticated Axiom account endpoint. An unsupported
`web_search_provider` value fails startup instead of silently choosing a
different search destination. Search queries are not inference E2EE; see
[privacy](privacy.md).

## Environment

| Variable | Meaning |
|---|---|
| `AXIOM_API_KEY` | Standalone automation bearer override; keep out of TOML and argv |
| `AXIOM_BASE_URL` | Relay origin, default `https://api.axiom.stream` |
| `AXIOM_AUTH_URL` | Hosted account origin, default `https://auth.axiom.stream` |
| `AXIOM_MODEL` | Discovered model ID override |
| `AXIOM_WEB_SEARCH_PROVIDER` | `axiom` (default and only supported value) |
| `AXIOM_PERMISSION_PROFILE` | Trusted startup tool profile |
| `AXIOMCLI_CREDENTIAL_STORE` | Optional `keyring`; plaintext-file stores are rejected |
| `AXIOMCLI_THEME` | `dark` (default), `light`, or `terminal` |
| `AXIOMCLI_COLOR` | `truecolor`, `256`, `16`, or `none` |
| `NO_COLOR` | Disable color when present |
| `AXIOMCLI_ASCII=1` | Use ASCII decorations |
| `AXIOMCLI_REDUCED_MOTION=1` | Disable terminal animation |

Desktop service-origin pairing and development overrides are described in
[development](development.md#setup). Child tool processes receive
a sanitized environment, not the application's credentials; per-command additions
are limited by native policy.

The default model setting is `auto`: prefer Tinfoil's DeepSeek V4.1 Flash
(`deepseek-v4-1-flash`) when it appears in the validated provider catalog. Other
Tinfoil offerings come next, followed by NEAR and then other registered providers.
Configure an explicit model ID to select a particular offering. Saved selections
take precedence; this preference cannot add an unavailable model to the catalog.
