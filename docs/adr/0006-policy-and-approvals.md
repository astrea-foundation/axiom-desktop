# ADR 0006: Deterministic effects and front-end approval adapters

Status: accepted.

Every tool must declare normalized effects before execution. The policy engine
evaluates those effects against a canonical workspace and one of five profiles.
Hard denials run first and cannot be overridden. An `ask` decision is carried
through an `ApprovalHandler`: the TUI presents a foreground permission card,
ACP uses the standard permission request, and headless mode denies because no
human authority exists.

The available response set is derived from the normalized effect: deny, allow
once, and—only when policy permits it—an exact-operation or predefined
safe-prefix grant for the current session. The model and front end cannot add a
broader choice. Session grants live only in the agent runtime and cannot
override hard denials or configured `ask` rules.

Disconnect, cancellation, or handler loss fails closed. There is no separate
human-response timeout: a user may take as long as needed unless an explicitly
configured whole-turn limit expires. Provider request timeouts remain separate
from human interaction. MCP annotations are untrusted hints; only an explicit
`read_only_tools` trusted-user configuration changes an MCP effect
classification.
