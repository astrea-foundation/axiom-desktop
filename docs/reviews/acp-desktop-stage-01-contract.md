# ACP desktop stage 01 review — contract and threats

Status: **PASS** (2026-08-24)

Reviewed state: `fe6ae68` plus the current working tree.

Evidence: ADR 0011 defines standard-first ACP, negotiated `_axiom` features,
schema ownership, process boundaries, and evolution. `docs/threat-model.md`
covers the renderer, sidecar, framing, replay, forged responses, replacement,
and secret boundaries. Electron retains no authorization authority.

Commands: `cargo deny check` — PASS with duplicate-dependency warnings only;
`pnpm audit --prod` — PASS, no known vulnerabilities; `git diff --check` — PASS.

Findings: blocker/high/medium/low: none. Informational: renderer-visible paths
are intentionally limited to selected workspace/session and structured tool
locations.

Exit criteria: met.
