# Stage 06 Review — Policy and approvals

Decision: **PASS**

- Policy tests cover all four profiles, root scope, and hard-denied privileged
  programs; deny precedence is independent of profile.
- Each tool normalizes effects before execution. A failure to normalize denies
  execution.
- TUI approval is allow-once via `y/n`; ACP uses
  `session/request_permission`. Headless mode has no approver and fails closed.
- Cancellation, disconnect, missing responders, and a 120-second timeout cannot
  become approval. Approval text contains the concrete normalized resource.
- Persistent/self-broadening grants are intentionally absent from this version.
- Untrusted project config cannot broaden profiles, budgets, or timeouts and
  cannot redirect provider/search endpoints or declare executable MCP servers.
