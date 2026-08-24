# ACP desktop stage 03 review — shared services

Status: **PASS** (2026-08-24)

Reviewed state: `fe6ae68` plus the current working tree.

Evidence: ACP reuses `Runtime`, `TurnRunner`, `SessionStore`, `AuthManager`,
policy, catalog, compaction, and security services rather than TUI widgets.
Overlay selection, animation, and composer state remain TUI-only.

Commands: `cargo test -p axiomcli --lib` — covered by the passing full workspace
test; `cargo test -p axiomcli --test tui_pty` — PASS (13).

Findings: blocker/high/medium/low: none. Informational: services remain in the
existing early-version modules instead of an artificial directory split.

Exit criteria: met; headless `exec`, ACP, and deterministic runners exercise the
same semantic kernel.
