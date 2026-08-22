# Stage 14 Review — Autonomous evaluations

Decision: **PASS (deterministic baseline)**

- A scripted-provider evaluation creates a real Rust repository, edits code via
  the authorized tool, runs real `cargo test`, checks semantic lifecycle events,
  and verifies the final workspace.
- An adversarial autonomous-profile evaluation requests an absolute out-of-root
  edit and verifies the external file remains byte-for-byte unchanged.
- Unit coverage includes state transitions, ACP parsing, policy, path/symlink
  escape, atomic edits, output floods, storage corruption/redaction, SSRF ranges,
  provider streaming, MCP namespace behavior, and terminal rendering.
- The live SearXNG evaluation is deliberately separate because it depends on
  external engines; the lifecycle script makes it reproducible and clean.

Live-model scoring/variance is deferred until a stable development endpoint and
budget are part of CI; deterministic gates do not depend on model luck.
