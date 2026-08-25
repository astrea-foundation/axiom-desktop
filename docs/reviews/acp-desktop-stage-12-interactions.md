# ACP desktop stage 12 review — interactions

Status: **PASS** (2026-08-24)

Reviewed state: `fe6ae68` plus the current working tree.

Evidence: native permission requests expose only offered scopes; form
elicitation handles required/optional, single/multiple, and text answers. The
client validates option IDs, enforces one owner, rejects stale IDs, and
fail-closes outstanding interactions on cancellation, disconnect, or shutdown.

Commands: ACP approval/question/cancellation transcript tests — PASS;
TypeScript interaction tests — PASS repeatedly (5 runs); full Rust gate — PASS.

Findings: blocker/high/medium/low: none. Informational: a denial racing Stop may
produce ACP `end_turn` or `cancelled`; both deny authority and clear ownership.

Exit criteria: met.
