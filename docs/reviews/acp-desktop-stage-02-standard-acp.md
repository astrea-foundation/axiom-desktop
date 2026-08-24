# ACP desktop stage 02 review — standard ACP baseline

Status: **PASS** (2026-08-24)

Reviewed state: `fe6ae68` plus the current working tree.

Evidence: ACP runtime crate 2.0.0 / wire v1 behavior is documented in
`docs/acp-compatibility.md`. Generic clients can initialize, create, prompt,
stream, cancel, approve, elicit, configure, and load without `_axiom`.

Commands: `cargo test -p axiomcli --test acp_transcript` — PASS (17);
`cargo check --workspace --all-targets --all-features` — PASS.

Findings: blocker/high/medium/low: none. Informational: client-owned terminals,
filesystems, and MCP injection remain disabled to preserve AxiomCLI policy.

Exit criteria: met; standard-only fallback has explicit automated coverage.
