# ACP desktop stage 00 review — baseline

Status: **PASS** (2026-08-24)

Reviewed state: commit `fe6ae68` plus the current ACP desktop-parity working tree.

Evidence: `docs/acp-desktop-parity-matrix.md` inventories commands, controls,
events, overlays, durability, cancellation, sensitivity, and test ownership.
Baseline TUI/ACP behavior is covered by snapshot, PTY, and transcript tests.

Commands: `cargo test --workspace --all-features` — PASS; `cargo test -p
axiomcli --test acp_transcript` — PASS (17); `cargo test -p axiomcli --test
tui_pty` — PASS (13).

Findings: blocker/high/medium/low: none. Informational: four explicitly opt-in
live/visual tests remain ignored by the deterministic workspace gate.

Exit criteria: met. No unexplained semantic command or presentation overlay is
treated as an ACP capability.
