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
the exact roots, active account database, and desktop chat directory. An
explicit absolute `XDG_CONFIG_HOME` or `XDG_DATA_HOME` overrides the native root
on every platform; relative overrides are rejected.

Local data uses one Axiom namespace with explicit ownership:

```text
<local data>/axiom/
  shared/identity/client-id
  shared/trust-policy.json
  accounts/<opaque-account-id>/
    cli/state.sqlite3
    desktop/state.sqlite3
    desktop/chat/
```

The installation identity is shared. The rotating native refresh token lives in
the operating-system credential store rather than this tree, so Desktop and CLI
can use the same signed-in account without placing a bearer credential on disk.
Each account and frontend has its own conversation database. No account database
is opened while signed out. Versioned databases are upgraded through ordered,
transactional migrations; unversioned pre-release layouts are not guessed at.

During pre-release development, remove an old installation-wide frontend
database with
`axiomcli reset-local-state --frontend cli --confirm` or
`axiomcli reset-local-state --frontend desktop-chat --confirm`. This command is
only a clean-slate cleanup for the retired `cli/state.sqlite3` or
`desktop/state.sqlite3` layout. It never scans, migrates, or deletes any
`accounts/<opaque-account-id>/` data, configuration, installation identity,
credential-store entry, trust policy, or the other legacy frontend database.

`axiomcli acp --frontend desktop-chat` is a product profile. After sign-in, its
cwd is the active account's canonical `desktop/chat` directory. While signed
out, thread and chat operations fail with an authentication-required error and
no fallback database is opened. MCP servers are disabled, the embedded desktop
system prompt cannot be replaced with `--system-prompt-file`, and the permission
profile is exactly `web`. Desktop parses known shared configuration and applies
its common fields, but ignores additive unknown keys and shared
`permission_profile` and `mcp_servers` because those fields configure the CLI. A
non-Web permission setting in `desktop/config.toml`, the environment, ACP mode,
or extension settings is rejected.

## Trusted user file

```toml
model = "deepseek-v4-flash"
base_url = "https://api.axiom.stream"
permission_profile = "confirm"
web_search_provider = "axiom" # authenticated hosted Decodo search; or "searxng"
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
`/login` starts a native authorization in the system browser, where the user can
choose passkey, Google, email/password, or Ethereum-wallet sign-in. It completes
through PKCE polling even when the loopback callback is blocked; the callback
only accelerates completion. `/account` validates the native session and shows
bounded account/session metadata without revealing tokens. `/logout` attempts
server-side revocation, clears the persisted refresh token, and closes the
active account store. Login state is application-wide and is never written to a
conversation timeline. Manual `axm_` API-key entry is not a native sign-in path.
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
currently register NEAR (`near-v3` / `near-tdx-nvidia-v2`), Phala
(`phala-aci-v2` / `phala-aci-tdx-v1`) and
[Tinfoil](protocols/tinfoil-ehbp-v1.md)
(`tinfoil-ehbp-v1` / `tinfoil-snp-sigstore-v1`). A catalog entry for an unregistered
future provider is ignored without hiding otherwise supported models; an entry that
claims a registered provider with the wrong protocol tuple still fails closed.
Phala offerings are chat-only for now: streaming is supported, but tool and
parallel-tool capabilities are disabled until every tool field has a receipt-bound
AAD contract. The outer `phala-aci-v2` contract accepts two structurally disjoint
artifact formats: `dstack-aci-draft-d567f6` and `aci-v1-08d550`. Their historical
suffixes are stable format identifiers, not gateway source pins. The reported
repository and commit must match the exact TEE-measured workflow; compatible
gateway updates do not require manual approval. Unknown and hybrid formats,
unbound provenance, and invalid cryptographic evidence still fail closed.

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
offering's current `context_length` and `max_output_length` from its provider
catalog, and AxiomCLI automatically compacts at 85% of that context
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
| `AXIOM_AUTH_URL` | trusted hosted account origin; defaults to `https://auth.axiom.stream` |
| `AXIOM_MODEL` | model override |
| `AXIOM_SEARXNG_URL` | SearXNG URL override |
| `AXIOM_WEB_SEARCH_PROVIDER` | `axiom` (default hosted account search) or `searxng` |
| `AXIOM_PERMISSION_PROFILE` | permission-profile override |
| `AXIOMCLI_CREDENTIAL_STORE` | optional explicit `keyring`; `file` and other values are rejected because native refresh tokens require an OS credential store |
| `AXIOMCLI_THEME` | `dark` (default), `light`, or `terminal`; `/theme` changes the current session |
| `AXIOMCLI_COLOR` | `truecolor`, `256`, `16`, or `none` |
| `NO_COLOR` | disables color when present |
| `AXIOMCLI_ASCII=1` | replaces decorative Unicode glyphs |
| `AXIOMCLI_REDUCED_MOTION=1` | disables animation |

`axiom-proxy` is a separate local integration for OpenAI-compatible clients.
Axiom Desktop supervises `axiomcli acp` and does not use the proxy for chat:

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

At startup, AxiomCLI validates a discovered native session with Axiom, rotating
the refresh token when necessary. A rejected, expired, or rotation-ambiguous
session is removed and the TUI opens the login surface; an `AXIOM_API_KEY`
override must instead be unset by the caller. A temporary network failure is
reported but does not erase an otherwise usable refresh token.
After credential validation, the TUI establishes and locally verifies a real
model-specific attested session without sending a prompt. The header animates
while this preflight runs and fails visibly if verification does not succeed.
Inference requests use only the bounded verified-material cache described above;
startup preflight is never an authorization to outlive those bounds.
Native authorization listens on a random `127.0.0.1` port and expires after the
server-defined short lifetime. The system-browser page handles every
method-specific credential. The OS credential store is mandatory for a native
refresh token; failure to read or write it leaves Axiom signed out.

With animation enabled, active tasks show a small orange pulse and a traveling
signal on the task rail. Waiting indicators are static, and a verified successful
completion resolves directly to a check. The composer status dot reflects the
current/last terminal state. Reduced-motion mode keeps decoration static.

Appearance is independent of color depth. `/theme dark|light|terminal` changes the
current terminal session; set `AXIOMCLI_THEME` to choose the startup appearance.
Dark and light use the desktop's neutral surfaces and orange brand mark. Terminal
appearance inherits foreground/background colors, including for code blocks;
no-color mode retains explicit labels and selection markers.

The settings row beneath the input shows the model's display name, supported thinking
level, agent permissions, and Web status. `Agent Ask` means tools require approval;
`Agent Read only` and `Agent Full access` reflect the other workspace profiles.
`/model`, `/thinking`, and `/permissions` change these settings. Web starts
off, and `/web on` enables search and URL fetching for subsequent turns. Search
queries and fetched URLs are shared with external services, not protected by
provider E2EE. `/web off` withholds and rejects Web tools again. These are local
terminal controls; the desktop retains its own appearance settings and Web toggle.

Child development commands do not inherit the parent environment. Only the
small key allowlist implemented by the process boundary can be set per command.
