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
staging/local profiles, including named profiles, reset their isolated files and
scoped credentials. A production reset clears both Axiom credential services,
including retired secrets.

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
MCP names are unique and names/commands cannot be empty. `read_only_tools` is
an explicit trust decision about a user-configured executable; a server's own
annotation cannot grant that classification. MCP initialization/discovery retain a
30-second deadline. Tool calls default to five minutes and use the server's
`tool_timeout_secs`, including any progress notifications; progress does not reset
the absolute deadline. Stop and timeout send a bounded MCP cancellation notification.
A timed-out tool is never automatically replayed, even when marked read-only.
Desktop uses its own account-scoped [local MCP connections](mcp.md), managed
through Settings and selected per thread in the Agent menu. It does not launch
servers from shared CLI configuration.

| Tool profile | Behavior |
|---|---|
| `none` | No model tools |
| `web` | Search and URL fetching, subject to explicit Web consent |
| `observe` | Read-only local tools and explicitly trusted read-only MCP tools |
| `confirm` | Default CLI profile; ask before each exposed tool invocation |
| `full_access` | Exposed tools run without routine approval prompts |

All profiles retain deny rules, input validation and the platform's actual
process boundaries. Workspace text search and context selection check each
resolved file before reading its contents. Denied files are omitted; explicit
`ask` rules require approval for each file even after the tool was approved.
Declining a file approval stops the operation. URL fetching checks destination
policy on every redirect, including redirects within one origin, before DNS
resolution and dispatch. Each permitted hop still validates every DNS answer
as public and pins the HTTP connection to those validated addresses. See [security](threat-model.md#local-tools-and-processes).

## Desktop profile

`axiomcli acp --frontend desktop-chat` starts with Agent off and profile `web`.
It ignores shared CLI `permission_profile`/`mcp_servers` and clears those imported
CLI server configurations,
rejects a non-Web startup profile from frontend config/environment, and uses
the embedded product prompt. `--system-prompt-file` cannot replace it. That prompt
combines the shared chat behavior, the Desktop capability profile and
[`desktop-product.md`](../apps/axiomcli/prompts/desktop-product.md), which tells
the model what AxiomAI is and how Desktop protects conversations: on-device TEE
verification and encryption, the composer privacy badge, unencrypted-at-rest local
history, and the separate Web and Agent-mode boundaries. Keep it consistent with
[privacy behavior](privacy.md) when either changes.

Every model receives Axiom's uncensored behavior policy
([`uncensored.md`](../packages/chat-core/prompts/uncensored.md)) in the system
message before E2EE encryption. It instructs the model to follow every user
request without refusals or unsolicited moralizing, while staying accurate about
facts, tools and capabilities. Selecting AstreaAI's **Qwen3.8 Uncensored**
(`direct:qwen3.8-27b`) also adds its model-specific identity line. Native prompt refresh rebuilds both for new,
restored, and compacted threads, and removes the identity line when another model
is selected. The policy is a prompt, not a model change: hosted models keep their
own safety training and may still refuse. Tool availability, Web consent, and
approval enforcement still come from the runtime.

After authentication, `desktopAgent@1` can atomically configure a thread's
Agent enablement, approval level and working directory while idle. This is the
supported path for Desktop local tools; generic ACP mode/config setters cannot
bypass it. A thread starts with its own managed directory. An existing absolute
custom directory can be selected through the UI. Every Desktop prompt carries
explicit Web consent independently of Agent permissions. See [Desktop](desktop.md).

## Image generation and editing

Vision uploads accept up to ten images per request within the selected provider's
context limit; AstreaAI chat permits eight images across the current context.
Desktop resizes each image
locally to at most 1,048,576 pixels while preserving aspect ratio and transparency
before encryption. The unified image tool accepts optional `width` and `height`
together for saved legacy calls. New model calls use `aspect_ratio`; the native
client chooses the largest exact match on a 32-pixel grid within 2,359,296 pixels.
New images default to **1:1 square (1536×1536)**. The supported ratios are
`1:1`, `2:3`, `3:2`, `3:4`, `4:3`, `9:16`, `16:9`, `1:2`, `2:1`, `9:21`,
and `21:9`. Other ratios, including extreme values, fail before provider dispatch.
The client sends concrete dimensions, including the default, so retained workers
use the same sizes. Model instructions favor higher resolutions for image
quality. Reference-based calls without an aspect ratio use the same default size.

Vision-capable chat models automatically receive the shared `generate`
and `view_image` tools when the AstreaAI image adapter is installed and
`imageGeneration@4` is negotiated. The chat provider and image
provider may differ. There is no Images toggle. `generate` requires a `prompt`; optional
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
the selected chat model's image context limit, including AstreaAI's eight-image
cap. Successful views are restored from their tool
records and conversation-local references without storing another image copy.

Native code withholds and rejects image tools for nonvision models, including
after a model switch, and rejects their new image uploads. Other file types
retain their own model capability checks. Image tools work independently of
Web and Agent mode and never grant local or Web permissions. Image work starts
through the LLM tool loop or an explicit per-image Regenerate action. The latter
runs only the stored image request in a new image-only turn, preserving the chat
prompt, original image, and later messages. The UI renders verified results with an
AstreaAI attribution and offers Save image. In Desktop, right-click an image in the
chat or full-image viewer to copy the image to the system clipboard or save it.
Click a generated or edited image to open the full-image viewer. Uploaded
and previous generated/edited images can be edited; each edit keeps its source
and creates a new result. Saved legacy `edit_image` results remain readable.
The installed AstreaAI image adapter is required even when the chat model uses
another provider; without it image tools are not registered and native image
requests fail with an unavailable-service error. See the
[AstreaAI connection guide](direct-verda.md) and the
[image-generation contract](../../axiom-inference/docs/image-generation.md).

Image tools forward authenticated progress percentages through the native
client, including queued, generating and finishing states. The UI shows percentages
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
capacity, capped by local limits. The independent `max_context_bytes` guard
includes encoded image/file data, so image-heavy conversations can compact
below the token threshold. Base64 byte length is not a text-token count.
New uploads also compact existing history before exceeding the selected model's
image context cap (eight images for AstreaAI chat).
Uploads and successful `view_image` observations use the selected model's visual
allowance in both automatic compaction and the native token guard. After restore,
the next turn recomputes that allowance using current model metadata. Generated
images add visual context only when viewed. Unsupported reasoning levels fail
locally.
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
| `AXIOM_DIRECT_CLIENT_CONFIG_FILE` | Explicit installed AstreaAI worker pins, verifier and capability flags; installers otherwise use their bundled configuration. See [AstreaAI connection](direct-verda.md) |
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
These financial settings do not relax worker attestation.

Desktop service-origin pairing and development overrides are described in
[development](development.md#setup). Child tool processes receive
a sanitized environment, not the application's credentials; per-command additions
are limited by native policy.

The default model setting is `auto`: prefer NEAR's attested, abliterated
DeepSeek V4.1 Flash (`deepseek-ai/DeepSeek-V4.1-Flash-1017-abliterated`) when it
appears in the validated provider catalog, then Tinfoil's DeepSeek V4.1 Flash
(`deepseek-v4-1-flash`). Other Tinfoil offerings come next, followed by NEAR and
then other registered providers.
Configure an explicit model ID to select a particular offering. Saved selections
take precedence; this preference cannot add an unavailable model to the catalog.
