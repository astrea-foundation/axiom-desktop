# ADR 0006: Deterministic effects and front-end approval adapters

Status: accepted.

Every tool must declare normalized effects before execution. The policy engine
evaluates those effects against a canonical workspace and one of four profiles.
Hard denials run first and cannot be overridden. An `ask` decision is carried
through an `ApprovalHandler`: the TUI presents `y/n`, ACP uses the standard
permission request, and headless mode denies because no human authority exists.

Approvals are allow-once in this version. Disconnect, cancellation, handler
loss, or a 120-second timeout denies the request. MCP annotations are untrusted
hints; only an explicit `read_only_tools` project configuration changes an MCP
effect classification.
