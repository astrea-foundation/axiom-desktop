# Local MCP connections

Desktop supports local Model Context Protocol servers over stdio. Remote URLs,
HTTP/SSE transports and OAuth connections are not supported. The server must
already be runnable on the computer; Axiom does not install Node, Python or
other runtimes.

## Connect a server

Open **Settings → MCP connections → Add local server**. Choose a unique name
using lowercase letters, numbers and underscores, the executable, and its
arguments as a JSON array. Axiom launches the executable directly rather than
interpreting a shell command. Prefer absolute executable paths when a program
is not on the sanitized PATH. Use absolute paths for file arguments as well;
MCP processes use the native sidecar’s launch directory, while the Agent working
directory setting controls Axiom’s builtin workspace tools. Windows Node `.cmd` launchers are supported by
the existing native MCP launcher.

Add credentials under **Environment variables**. Values are written to the OS
credential store and are never returned in settings, snapshots or exports.
Leave this section unchanged to preserve saved variables; replacing variables
replaces the whole saved environment. **Clear stored variables** removes it.
A missing or locked credential store fails closed, with no plaintext fallback.
Commands and arguments are ordinary local configuration, so put secrets in
environment variables rather than arguments.

**Import configuration** accepts the common `{"mcpServers": {...}}` JSON shape
with local `command`, `args` and `env` entries. Imports start disabled. Review
and enable only commands you trust. Unsupported fields and remote connections
are rejected rather than silently ignored.

**Test connection** starts the process, initializes MCP and discovers its tools,
then closes the process. It never invokes a discovered tool. Inspection shows
names and descriptions; failures appear on the connection card. Testing and
turn initialization have a 30-second overall deadline. Credential-store waits
are bounded to five seconds. An account supports up to 32 connections, 512
discovered tools and 4 MiB of configuration; a thread can select up to 256 tools. Saved credentials use
immutable references so a failed configuration save cannot replace credentials
used by an existing thread. Retired credential entries are removed after a
successful configuration change when the credential store is available.

## Choose tools per thread

Open **Agent** beside the composer. Under **MCP tools for this thread**, select
a server’s tools together, or individual tools from enabled, tested connections,
and save. Choices persist
with the local thread; other threads and new threads start without those tools.
The same menu supports choosing tools for a new thread before its first message.

MCP selection is independent of Agent's shell/filesystem permissions and Web.
You can leave Agent off while using selected MCP tools. Agent **Full access**
does not automatically authorize MCP. Calls show the server, tool and arguments
in the existing approval flow. You can allow once, deny, or explicitly grant
narrow access for the current thread. Grants are in memory and expire when the
native runtime restarts or authority changes. Configured deny/ask rules remain
binding. Tool calls and results use the usual transcript cards.

Connections start only for testing or a turn using selected tools. Each turn
owns its server processes; completion, errors, Stop and sidecar shutdown close
those processes and their descendants through the existing native lifecycle
controls. A timed-out or ambiguously interrupted side-effecting call is never
replayed. Desktop does not trust server annotations as read-only declarations.
Tool calls default to a five-minute absolute deadline.

Changes require idle native work; thread settings also require an empty queue
in the renderer. An account-wide MCP revision is captured when a message is
queued. Changing a connection or any thread's selection requires reviewing and
resending messages queued with the old revision. Changed executable, credential,
enablement or discovered tool authority clears selections for that server;
unrelated servers retain their selections. Each turn rechecks the discovered
schemas against the tested snapshot and fails closed on drift. Test and select
again after a server update changes its tools.

## Privacy and local authority

A local MCP server runs as trusted user-selected host code. It can access files
and the network with your computer's permissions; the selected tools and Agent
working directory do not create an OS sandbox for the server. Axiom supplies a
sanitized process environment plus explicitly saved server variables, not
inherited application credentials. Connections and thread selections are scoped
to the signed-in account's Desktop database.

Connected services may receive tool inputs and results. This traffic is outside
the model's TEE protection and is not governed by the built-in Web toggle. MCP
results and schemas remain untrusted; results are bounded and known saved
environment values are redacted from tool output. The native client incorporates
results into model context locally and encrypts subsequent inference with the
selected provider's attested E2EE protocol. The hosted backend never receives
plaintext MCP arguments, results or reconstructed model messages.

See [privacy](privacy.md), [local process boundaries](threat-model.md#local-tools-and-processes)
and [ACP authority](acp-compatibility.md#desktop-authority-and-revisions).
