# MCP connections

Axiom Desktop connects to remote Model Context Protocol services over Streamable
HTTP and local servers over stdio. Connections belong to the signed-in account.
Each conversation chooses its own tools; adding a connection does not select
its tools in every conversation.

## Connect a remote service

Open **Settings → MCP connections → Add connection → Remote service**. Enter a
name such as “Project tracker” and the service’s MCP endpoint URL. Axiom keeps a
stable internal identifier when the displayed name changes. The form shows the
destination before connecting, with this notice:

> **Remote tools share data outside Axiom’s attested environment**
>
> When a remote tool runs, its inputs are sent to the service domain. These
> inputs may include content from your conversation or files. Axiom does not
> attest how this service processes or stores that data.

Choose **Sign in with browser**, **API token**, or **No authentication**.
Browser sign-in opens the service's OAuth authorization page and completes in
the native client through a loopback callback, with PKCE, state and issuer
validation. It expires after three minutes and can be cancelled in Settings.
Advanced sign-in settings accept an existing public client ID or an HTTPS
Client ID Metadata Document URL. Otherwise Axiom uses dynamic registration
when the service supports it. A service without compatible client registration
must supply a registered public native client ID.

API tokens and OAuth credentials are stored in the OS credential store. Saved
values are never returned in settings, snapshots or exports. Leave an existing
API token unchanged to preserve it; **Clear saved token** removes it. A missing
or locked credential store fails closed, without a plaintext fallback.
**Disconnect** removes the local saved grant, disables the connection and clears
its selected tools. It does not revoke tokens at the external service; manage
service-side grants through that service's account settings when needed.

URLs must use HTTPS, except that loopback HTTP is allowed for development.
Embedded credentials, query strings and fragments are rejected. The native HTTP
client does not inherit Axiom account headers or follow redirects. Current
`2026-07-28` and older `2025-11-25` Streamable HTTP services are supported. The
separate legacy HTTP+SSE transport is not supported.

## Connect a local process

Choose **Add connection → Local process** and enter the executable and its
arguments as a JSON array. Axiom launches the executable directly, without a
shell command. The server and its runtime must already be installed; Axiom does
not install Node, Python or other runtimes.

Prefer absolute executable paths when a program is not on the sanitized PATH.
Use absolute paths for file arguments too. MCP processes use the native
sidecar's launch directory; Agent's working-directory setting controls Axiom's
builtin workspace tools. Windows Node `.cmd` launchers use the native launcher.

Add credentials under **Environment variables**. Values are write-only OS
credentials. Leaving the section unchanged preserves saved variables; replacing
variables replaces the whole environment. **Clear stored variables** removes it.
Commands and arguments are ordinary configuration, so put secrets in environment
variables rather than arguments.

Local processes run with the computer's permissions, outside the attested model
environment. They can access files and the network. The selected tools and
Agent working directory do not create an OS sandbox for a local MCP server.

## Import and test connections

**Import configuration** accepts the common `{"mcpServers": {...}}` JSON shape.
Local entries accept `command`, `args` and `env`. Remote entries accept `url`
and an optional Streamable HTTP transport type. A bearer `Authorization` header
is imported into the credential store, not retained in public configuration.
Remote entries can explicitly select `auth: "oauth"`; without an authentication
setting or header they start with no authentication. Arbitrary custom headers,
legacy SSE and mixed local/remote fields are rejected. Imports start disabled.
Review the destinations and commands, enable trusted connections and test them.

**Connect** saves a remote connection, signs in when required and tests it.
**Test connection** establishes MCP and discovers tools without invoking a tool.
A successful card shows **Ready** and the tool count; this describes readiness,
not attestation or an always-open network connection. Failures appear on the
connection card. Testing and turn initialization have a 30-second overall
deadline; credential-store waits are bounded to five seconds.

An account supports up to 32 connections, 512 discovered tools and 4 MiB of
configuration; a conversation can select up to 256 tools. New credentials use
immutable references so a failed save cannot replace an existing connection's
credentials. Retired credentials are removed after a successful configuration
change when the credential store is available. Routine OAuth refresh for the
same issuer, client and scopes preserves tool selections; changed grants require
reconnection and review. Concurrent turns share token-refresh coordination.

## Choose tools per conversation

Open **Tools** beside the composer. Connections are grouped as **Remote
services** and **On this device**. Select a connection's tools together or
individual tools from enabled, tested connections, then **Save tools**.
Selections persist with that conversation. Other and new conversations start
without those tools; tools can also be selected before a new conversation's
first message.

Tools work with Agent mode off. Agent's shell/filesystem permissions, MCP tool
selection and Web consent are independent. **Full access** does not automatically
authorize MCP. Calls ask for approval unless narrow access has been granted
for the conversation. Grants expire when the native runtime restarts or
authority changes; configured deny/ask rules remain binding.

Remote selections show a globe indicator on **Tools** and a sharing notice in
the picker. Approvals show the destination and inputs before dispatch. Tool
activity records the connection and destination used at the time of the call,
so editing or removing a connection does not rewrite historical disclosure.
Details distinguish completed calls, attempts with potentially unknown outcomes,
and proposed calls that never started. Request start is not proof that the
service received or completed an action.

Connections start for discovery, sign-in or a turn using selected tools. Local
server processes belong to the turn and are closed with their descendants on
completion, errors, Stop or sidecar shutdown. Remote requests are cancelled
through the SDK's protocol-specific lifecycle. Deadlines default to five minutes.
A timed-out or ambiguously interrupted side-effecting call is never replayed;
HTTP session-expiration recovery and HTTP POST retries are disabled. Server
annotations do not establish read-only authority.

Changes require idle native work; conversation settings also require an empty
message queue. Queued messages capture an account-wide MCP revision. Changed
settings require review and resend. Executable, endpoint, authentication,
credential, enablement or discovered-tool changes clear affected selections.
Unrelated connections retain theirs. Each turn rechecks discovered schemas and
fails closed on drift. Test and select again after a service changes its tools.
Older Desktop clients cannot select or execute remote connections without the
remote-aware capability negotiation.

## Privacy and verification scope

The shield reports the model inference environment's actual verification state.
Remote connections have a separate **Remote** label and do not change that
verification result. **Privacy proof → External services** lists enabled
services separately from destinations with requests attempted in the loaded
conversation history. Historical entries retain their recorded destinations.
The report also explains that local processes are outside model attestation.

Connected services can receive tool inputs and generate results outside the
model's TEE protection. This is not governed by the built-in Web toggle. MCP
schemas and results remain untrusted. Remote JSON bodies and SSE events are
bounded before parsing, displayed results are bounded, and known credentials
are redacted from tool output. The native client incorporates results locally
and encrypts subsequent inference with the selected provider's attested E2EE
protocol. The hosted backend never receives plaintext MCP arguments, results or
reconstructed model messages.

See [privacy](privacy.md), [process boundaries](threat-model.md#local-tools-and-processes)
and [ACP authority](acp-compatibility.md#desktop-authority-and-revisions).

After a local MCP connection reconnects, its complete discovered tool list is
bound to the new connection generation. Every call checks its original schema
against that generation, including sibling tools that did not initiate the
reconnect. Removed tools or changed schemas are rejected before dispatch;
restart the connection to review the new tool definitions.
