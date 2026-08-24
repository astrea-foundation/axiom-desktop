# ACP desktop stage 06 review — state and recovery

Status: **PASS** (2026-08-24)

Reviewed state: `fe6ae68` plus the current working tree.

Evidence: snapshots carry runtime ID, sequence, lifecycle, active turn,
settings, security, title, cwd, and prompt state. The client detects runtime
changes/sequence gaps and repairs with `_axiom/session/get_state`; process exit
rejects requests and Electron supervision rebuilds durable state.

Commands: `pnpm --filter @axiom/axiom-acp-client test` — PASS (4); `cargo test
-p axiomcli --test acp_transcript` — PASS (17).

Findings: blocker/high/medium/low: none. Informational: transient approvals are
fail-closed rather than replayed after process restart.

Exit criteria: met.
