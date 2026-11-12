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

For a complete clean install, use **Settings → Account → Factory reset Axiom…**
and type `RESET`. The equivalent offline recovery command, after quitting Desktop,
CLI and proxy processes, is:

```sh
axiomcli factory-reset --confirm
```

Factory reset deletes all accounts' Desktop and CLI history, attachments and
managed chat folders; shared and frontend configuration; installation identity
and trust-policy caches; saved Axiom sign-ins and MCP secrets in the OS credential
store; and Electron profiles, browser storage, logs and update caches. It works
without loading configuration or database schemas and makes no network requests.
The Desktop action stops its native processes, exits, runs the bundled cleanup
helper after file handles close, and reopens Axiom. Other native processes using
the same data must be closed; reset refuses to run while their storage lease is
held. An empty `.axiom-factory-reset.lock` coordination file outside the data
tree remains, containing no user information.

The online account, credit, hosted API keys, external browser sign-in and files
outside Axiom-managed storage are kept. Files in a custom Agent working directory
or a Downloads folder are outside reset's scope. An OS credential-store failure
leaves files available for retry; deletion can be partial if the filesystem fails
after credential cleanup. Desktop displays an error after reopening if reset did
not finish. Unlock the vault or close remaining processes and retry. Development
staging/local profiles reset their isolated files and scoped credentials. A
production reset clears both Axiom credential services, including retired secrets.

See [troubleshooting](troubleshooting.md#completely-reset-local-data) for manual
recovery on versions that predate the factory reset command.

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
MCP names are unique; local connections require nonempty commands. `read_only_tools` is
an explicit trust decision about a user-configured executable; a server's own
annotation cannot grant that classification. MCP initialization/discovery retain a
30-second deadline. Tool calls default to five minutes and use the server's
`tool_timeout_secs`, including any progress notifications; progress does not reset
the absolute deadline. Stop and timeout use bounded transport-specific MCP cancellation.
A timed-out tool is never automatically replayed, even when marked read-only.
Desktop uses its own account-scoped [MCP connections](mcp.md), managed
through Settings and selected per conversation in the Tools menu. It does not launch
servers from shared CLI configuration.

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
It ignores shared CLI `permission_profile`/`mcp_servers` and clears those imported
CLI server configurations,
rejects a non-Web startup profile from frontend config/environment, and uses
the embedded product prompt. `--system-prompt-file` cannot replace it.

Selecting **Qwen3.8 Uncensored** (`chutes:qwen3.8-27b`, or the debug-only
`chutes-preview:qwen3.8-27b`) adds its native model-specific instructions to the
system message before E2EE encryption. They identify the model by its display
name and instruct it to follow every user request without refusals or unsolicited
moralizing. Native prompt refresh applies the policy to new, restored, and
compacted threads, and removes it when another model is selected. Tool
availability, Web consent, and approval enforcement still come from the runtime.

After authentication, `desktopAgent@1` can atomically configure a thread's
Agent enablement, approval level and working directory while idle. This is the
supported path for Desktop local tools; generic ACP mode/config setters cannot
bypass it. A thread starts with its own managed directory. An existing absolute
custom directory can be selected through the UI. Every Desktop prompt carries
explicit Web consent independently of Agent permissions. See [Desktop](desktop.md).

## Image generation and editing

Vision uploads accept up to ten images per request. Desktop resizes each image
locally to at most 1,048,576 pixels while preserving aspect ratio and transparency
before encryption. The unified image tool accepts optional `width` and `height`
together for saved legacy calls. New model calls use `aspect_ratio`; the native
client chooses the largest exact match on a 32-pixel grid within 2,359,296 pixels.
New images default to **2:3 portrait (1216×1824)**. The supported ratios are
`1:1`, `2:3`, `3:2`, `3:4`, `4:3`, `9:16`, `16:9`, `1:2`, `2:1`, `9:21`,
and `21:9`. Other ratios, including extreme values, fail before provider dispatch.
The client sends concrete dimensions, including the default, so retained workers
use the same sizes. Model instructions favor higher resolutions for image
quality. Reference-based calls without an aspect ratio preserve the first
reference's output dimensions or aspect ratio within the limits.

Vision-capable chat models automatically receive the shared `generate`
and `view_image` tools when the managed Chutes image adapter is installed and
`imageGeneration@3` is negotiated. There is no Images toggle. `generate` requires a `prompt`; optional
`reference_image_ids` select up to five ordered conversation-owned images for
editing or composition. Refer to them in the prompt as `<image1>`, `<image2>`,
etc. in array order. Omitting references creates a new image. Invalid references fail
before dispatch and cannot fall back to generation. The model is instructed to
create or edit images only when the user explicitly asks; an attachment or a
request for analysis alone does not authorize image creation.

`view_image` accepts an exact conversation-owned `image_id` and supplies the
image as visual input on the model's next step in the same turn. The model may
inspect an upload or check a generated/edited result before answering or
requesting a correction. Viewing is optional and does not create or edit an
image. Generated results are displayed to the user but are not automatically
added to the model's visual context. Vision copies preserve aspect ratio and
transparency, correct orientation, and fit within 1,048,576 pixels; originals
remain available for editing, viewing and saving. Viewed images count toward
the ten-image context limit. Successful views are restored from their tool
records and conversation-local references without storing another image copy.

Native code withholds and rejects image tools for nonvision models, including
after a model switch, and rejects their new image uploads. Other file types
retain their own model capability checks. Image tools work independently of
Web and Agent mode and never grant local or Web permissions. Image work starts
through the LLM tool loop or an explicit per-image Regenerate action. The latter
runs only the stored image request in a new image-only turn, preserving the chat
prompt, original image, and later messages. The UI renders verified results and offers Save
image. Click a generated or edited image to open the full-image viewer. Uploaded
and previous generated/edited images can be edited; each edit keeps its source
and creates a new result. Saved legacy `edit_image` results remain readable.
The installed managed Chutes adapter is required even when the chat model uses
another provider. See the
[image-generation contract](../../axiom-inference/docs/image-generation.md).

Image tools forward authenticated progress percentages through the native
helper, including queued, generating and finishing states. The UI shows percentages
while work is pending; its percentage and progress bar disappear after successful
result verification and settlement. This requires the
matching streaming image cords and backend relay paths. Save image suggests
a new random UUID `.png` filename on each click, including repeated saves.

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
| `AXIOM_POOL_CLIENT_CONFIG_FILE` | Installed public pool pins and offerings; never a provider credential |
| `AXIOM_LOCAL_RELAY_CA_FILE` | Explicit CA for an HTTPS loopback API; cannot change provider trust |
| `AXIOM_CHUTES_DESKTOP_PREVIEW_CONFIG` | Debug-only local Chutes native bridge; optional installed publisher-key fingerprint enables signed staging releases with scoped TEE verification |
| `AXIOM_CHUTES_CLIENT_CONFIG_FILE` | Trusted installed managed Chutes helper, publisher fingerprint and native price ceiling; uses Axiom account authentication and keeps the provider key on the backend |
| `AXIOM_DESKTOP_LOCAL_PROFILE=1` | Unpackaged loopback profile with separate Electron/native storage |
| `AXIOM_DESKTOP_LOCAL_CREDENTIALS=1` | Explicit unpackaged local-profile opt-in to the debug native credential file; both API and auth origins must be loopback |
| `AXIOMCLI_LOCAL_DEV_CREDENTIALS=1` | Debug Unix builds only: owner-only, origin-scoped credentials under the isolated native data directory; rejected for remote origins, release builds and API-key overrides |
| `AXIOM_WEB_SEARCH_PROVIDER` | `axiom` (default and only supported value) |
| `AXIOM_PERMISSION_PROFILE` | Trusted startup tool profile |
| `AXIOMCLI_CREDENTIAL_STORE` | Optional `keyring`; other values are rejected. The separate explicit debug flag above is the only local-file mode |
| `AXIOMCLI_THEME` | `dark` (default), `light`, or `terminal` |
| `AXIOMCLI_COLOR` | `truecolor`, `256`, `16`, or `none` |
| `NO_COLOR` | Disable color when present |
| `AXIOMCLI_ASCII=1` | Use ASCII decorations |
| `AXIOMCLI_REDUCED_MOTION=1` | Disable terminal animation |

Each offering in the trusted `AXIOM_POOL_CLIENT_CONFIG_FILE` may include a
`maximum_charge_microusd` per-request ceiling. It defaults to zero, preserving
free-only inference. `100000` permits a signed quote of at most US$0.10. The
renderer and project configuration cannot raise this native limit. A larger
quote is cancelled before message ciphertext is sent. The controller signs a
versioned price snapshot; native code verifies the exact final charge against
the authenticated usage receipt. An optional `pricing` snapshot populates the
displayed input/output rates. Rates are integer microUSD per million tokens;
reasoning tokens are included in output and cached tokens are included in input.
These financial settings do not relax worker attestation or enable the
unqualified Chutes pool offering.

Managed Chutes releases unresolved request holds after 15 minutes. The API
checks every 30 seconds and before new admission. Until then, its unresolved
request limit appears as an admission error with the expiry window, distinct
from a hardware or encrypted-exchange failure. Expiry never replays a request
or marks an incomplete reply verified; a late authentic usage receipt does not
create a charge after expiry.

Desktop service-origin pairing and development overrides are described in
[development](development.md#setup). Child tool processes receive
a sanitized environment, not the application's credentials; per-command additions
are limited by native policy.

The default model setting is `auto`: prefer Tinfoil's DeepSeek V4.1 Flash
(`deepseek-v4-1-flash`) when it appears in the validated provider catalog. Other
Tinfoil offerings come next, followed by NEAR and then other registered providers.
Configure an explicit model ID to select a particular offering. Saved selections
take precedence; this preference cannot add an unavailable model to the catalog.
