# Tinfoil UX fixes and retest

The actionable findings from the [initial test](README.md) are implemented on `feat/tinfoil-provider` in both repositories. Stop aborts the provider connection and waives charges without final metering, as requested. No merge or deployment was performed. Chutes remains on its separate branches.

The retest used the same isolated HTTPS relay, real Tinfoil inference, synthetic account/credit, and locally verified TEE/EHBP protocol as the original run. Desktop and TUI shared the account and credential store. Desktop-control input drove the live UI tests; read-only native SQLite, CDP and backend accounting confirmed persistence and charges. The temporary debug-only loopback transport patch was restored before the product build. Provider attestation, encryption and response authentication stayed enabled.

## Resolutions

| Finding | Implemented behavior | Evidence |
| --- | --- | --- |
| F01: valid proof rejected | The previously committed exact-protocol evidence fix remains intact. | Original proof screenshots; proof regression tests. |
| F02: shared-account refresh rejected | Authenticate a peer-rotated credential before changing account identity. Same-account refresh retains live operation leases; an actual account switch still invalidates them. Authentication errors request account refresh rather than reporting bad TEE evidence. | Two-manager concurrency/isolation tests; simultaneous Desktop/TUI requests succeeded. |
| F03: stopped accounting stays running | Persist cancellation before finishing a turn, then reconcile pending request metadata in the background. Keep partial output unverified. | Live zero-charge settlement in both clients without restart; store regression. |
| F04: Stop continues generating and billing | Cancel upstream before or after response headers. Preserve exact final metering if already received; otherwise finalize a durable waiver and release the hold. Unknown tokens stay unknown. | Three real cancelled requests; pre-header/midstream disconnect, idempotency and no-double-settlement tests. |
| F05: fresh message joins a held queue | A fresh explicit send after Stop can run once cleanup finishes; older paused entries stay held. Reconnect does not grant automatic replay. | Live immediate follow-up; queue race/reconnect tests. |
| F06: approvals leave the next prompt offscreen | Approval-card clicks preserve following the conversation. Manual scrolling still pauses following. Routine permission notices no longer duplicate tool cards. | Three consecutive visible GLM approvals, followed by successful read/write/read-back. |
| F07: paid empty titles | Derive a short local title from the initial prompt. No title inference, reasoning budget or charge. Manual names remain preserved. | Instant Desktop title; unit test asserts zero provider requests. |
| F08: edit/regenerate unavailable | Confirm replacement of the selected turn and all later messages; atomically persist the replacement, rebuild local history and send normal encrypted inference. Preserve all request charges. Reject stale, active or foreign-account edits. Steering is not an editable turn boundary. | Live regenerate and two edits; native rollback test; ACP stale/reload test; browser control test. |
| F09: question permission and retries after Deny | Built-in structured questions do not require a redundant tool approval. A human denial ends the turn, records the reason and does not request another model/tool attempt. | Direct question dialog; denied files absent; TUI shows the tool as stopped. |
| F10: ambiguous patch `mode` | Expose numeric `file_permissions`, document Unix permission bits and preserve the legacy numeric alias. String operations in that field remain invalid. | GLM created/read the fixture on its first patch attempt; schema/engine tests. |
| F11: broken table words | Preserve normal word boundaries inside a horizontally scrollable table. | Live Electrical/Sewing/Woodwork table at 1280 × 800. |
| F12: cancelled preflight keeps VERIFYING | Clear transient verification state when verification/turn cancellation ends. | TUI regression test. A fresh live cancellation during uncached preflight was not reproduced deterministically. |

Kimi also exposes Low/Medium/High thinking. Axiom maps these to the provider's low/high/max values, using its supported top-level parameter. All three levels passed tool-call and encrypted tool-result continuations; the canary exercised parallel calls. GPT-OSS and GLM retain their provider-specific controls; Llama has none.

## Live evidence

- [Local title and readable table](evidence/fixes/desktop-table-local-title.png).
- [Direct structured question](evidence/fixes/tui-direct-question.png), [Deny stops the tool](evidence/fixes/tui-denial-final.png), [cancelled usage becomes waived while idle](evidence/fixes/tui-cancelled-waived-final.png).
- [Regenerated response](evidence/fixes/desktop-regenerated.png), [final atomic edit to Birch](evidence/fixes/desktop-atomic-edit-final.png), [fresh follow-up after Stop](evidence/fixes/desktop-stop-followup-visible.png).
- [Next approval stays visible](evidence/fixes/desktop-next-approval-follows.png), [clean final tool history](evidence/fixes/desktop-clean-agent-history.png), [Kimi retains context after cancellation/model changes](evidence/fixes/tui-kimi-followup-after-cancel.png).

The GLM task read `README.md`, created `approval-followup.txt`, read it back, and returned a verified answer. `denied-after-fix.txt` and `denied-final.txt` were absent after their respective denials. Earlier requests already metered before a tool denial remained charged; denying a local tool does not erase a completed model request.

The [operational usage export](fixes-usage.json) contains **22 requests**: 16 UI requests and six Kimi reasoning-canary requests. Nineteen had exact final metering totaling **$0.073566**. Three were cancelled and waived, with **zero customer charge and unknown provider usage/cost**. This is the isolated Axiom ledger, not a reconciled provider invoice. Every dispatch in the retest window was either settled or waived at export. Cancelled output remained unverified; replacing a conversation did not remove its older accounting records.

## Validation and limits

- Native CLI library: 325 passed, two existing tests ignored.
- Tinfoil secure-client tests: eight passed; ACP extension: five passed.
- ACP client: 67 passed, including revision persistence across native-process restart.
- Desktop unit tests: 161 passed; browser suite: 122 passed. Browser tests simulate Linux/Windows/macOS renderer behavior; actual UI control was on Linux.
- Backend provider/relay/billing regression suite: 120 passed. The final Tinfoil subset also passed all 24 tests after the logging refinement.
- Type checks for Desktop and ACP, native/Desktop builds, Rust formatting, changed-backend Ruff checks and whitespace checks passed. Strict Clippy was not claimed clean; the baseline has unrelated warnings recorded in the original report.

The prior 98-second GLM outlier is now diagnosable through operational timing. Backend logs distinguish headers, first ciphertext, body idle intervals and total duration. Native CLI/ACP tracing separately records attestation and first authenticated delta timing. Neither logs message or key material. Instrumentation does not guarantee faster provider responses, sustained capacity or a numeric production rate limit.

The retest did not fill a maximum-size context, load-test concurrent production traffic, or repeat every tool/model/UI combination. The initial report contains all-four-model UI coverage and manual compaction. These fixes preserve the existing fail-closed verification and authenticated-stream-completion requirements.
