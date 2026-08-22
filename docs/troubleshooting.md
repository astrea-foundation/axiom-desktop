# Troubleshooting

Start with `axiomcli doctor`. It reports the selected model, provider URL,
explicit `UNATTESTED DEV` security state, permission profile, SearXNG health,
and local configuration/session paths without printing credentials.

## Provider or model errors

- Confirm the local AxiomIO service or configured HTTP(S) endpoint is running.
- Run `axiomcli models` to test bounded model discovery.
- Set `AXIOM_API_KEY` in the process environment only if the endpoint requires
  it. Authentication errors are not retried; rate limits and transient errors
  use bounded retries with a stable idempotency key.
- A truncated or malformed stream fails the turn. It is never treated as a
  complete answer.

## A command will not run

On Linux, `/usr/bin/bwrap` is required. AxiomCLI fails closed if it is absent.
The `workspace` profile automatically permits only its narrow development
command list; other structured programs require confirmation. Shell commands
always require confirmation. The process receives no network and only an
allowlisted environment.

If Cargo cannot find an already installed toolchain inside the sandbox, verify
that the invoking account’s Cargo and Rustup directories exist. AxiomCLI mounts
those caches read-only into the sandbox.

## Search fails

Run `scripts/searxng-dev start` and then `scripts/searxng-dev query "test"` in a
source checkout, or point `searxng_url` at a SearXNG service you operate or
otherwise trust. A hosted Axiom service is not part of this repository.
Individual upstream engines can throttle or time out. Empty results and engine
warnings are returned honestly. `fetch_url` rejecting a loopback/private target
is expected security behavior.

## MCP fails or disconnects

MCP commands must be in trusted user configuration, not repository config. A
read-only tool is retried once after reconnect only when its name and schema are
unchanged. A side-effecting disconnect reports an unknown outcome and is never
replayed. Inspect the bounded error and the server’s own stderr without routing
MCP logs into ACP stdout.

## Session recovery

Use `axiomcli sessions list` and `axiomcli tui --resume SESSION_ID`. The saved
workspace must still exist and must match any supplied `--cwd`. Interrupted
tools are shown as unknown and are not replayed. Use `sessions export` before
pruning or manually repairing data. Corrupt tails are reported and recovery
stops before the damaged event rather than deleting evidence.

## Terminal display

The minimum useful viewport is 42×10 cells. Press `?` for keyboard help. Set
`AXIOMCLI_COLOR=none`, `NO_COLOR=1`, `AXIOMCLI_ASCII=1`, or
`AXIOMCLI_REDUCED_MOTION=1` for accessibility/compatibility. `Esc` closes an
overlay or exits an idle TUI; `Ctrl+C` cancels active work. Terminal restoration
is guarded across normal exit, cancellation, and panic.

## Permission was denied

Read the normalized effect shown by the prompt. `n` denies, `y` permits one
operation, and—when offered—`e`/`p` grant an exact or prefix scope for the
current session. Configured asks/denials and hard workspace boundaries cannot
be overridden by a session grant. Headless mode cannot answer approvals and
therefore fails them closed.

To persist a restrictive repository rule through the explicit configuration
flow, use `axiomcli permissions add-project-rule --action ask|deny --effect ...`.
This command can only add `ask` or `deny` rules; project configuration cannot
grant authority.
