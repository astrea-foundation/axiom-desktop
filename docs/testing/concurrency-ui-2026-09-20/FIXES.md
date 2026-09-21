# Concurrency and long-conversation fixes — 2026-09-20

Historical validation of the dated revision. Current storage and billing use [ADR 0014](../../adr/0014-clean-storage-and-deposit-billing.md).

All seven findings in [the live report](README.md) and [root-cause investigation](ROOT-CAUSES.md) are addressed. This batch changes the native client and the companion backend on `dev`; it does not publish installers or deploy production.

Companion backend revision: [`dfef1f1`](https://github.com/astrea-foundation/axiom-platform/commit/dfef1f1), with [relay changes and migration notes](https://github.com/astrea-foundation/axiom-platform/blob/dfef1f1/docs/testing/concurrency-fixes-2026-09-20.md).

## Changes and regression coverage

| Finding | Result | Regression coverage |
| --- | --- | --- |
| UX01: facts removed before compaction | Serialize the entire older conversation and summarize consecutive bounded fragments, carrying each summary forward. Keep the newest complete turn verbatim when it fits. Commit only after every fragment succeeds; empty, oversized, interrupted or output-limited summaries retain the original history. Restoring a large journal no longer prunes messages before compaction. | Exact reconstruction of all roles, large Unicode/tool payloads, multi-part summaries, recent corrections, later-fragment failures, output limits, oversized restore, a second compaction and restart; existing automatic-trigger tests. |
| UX02: Stop leaves NEAR work running and blocks later requests | Disconnect cancels preparation, completion and streaming, closes the provider iterator, releases capacity and settles accounting. Concurrent NEAR requests lease separate connections with independent TLS pin state. Native Stop restores the committed journal before accepting a follow-up, preserving the cancelled prompt and marking its response incomplete. | Disconnect before headers, backpressure, first-frame close, deadlines, independent connection leases, pin rotation rejection, lost settlement acknowledgements, unknown-usage waivers, exact verified-usage settlement and cancelled-turn context replay. |
| UX03: navigation loses a message during setup | Persist a stable submission ID and its text before any setup IPC. Ordinary navigation lets setup finish in the background without taking focus; account/runtime changes leave a reviewable saved message. Interrupted setup is restored for review, never automatically replayed. | Delays and failures at each setup boundary, navigation to new/existing threads, two overlapping first messages, account/runtime changes, storage failure and reload recovery. |
| UX04: draft disappears on thread selection | Drafts are stored locally per account and thread, including a separate new-thread draft. Switching or reselecting a thread does not clear its draft. | Reselect, switch away/back, reload, account isolation and recovery writes. |
| UX05: code-only answer collapses | Assistant and reasoning content have a bounded full width, so code containment cannot collapse the parent. | Real-browser geometry at normal and narrow widths; live code-only answer measures 748 px. |
| UX06: streams reorder sidebar rows and folders | Persist `lastUserMessageAt` separately from activity. An accepted user message changes rank; output chunks update activity/unread state without moving rows. | Storage migration, catalog pagination, interleaved streams, folder ordering and user follow-ups. |
| UX07: numbered markers are clipped | Ordered-list padding accounts for the largest number's digit count, including a non-default start. | Real-browser lists crossing 998–1002 at normal and narrow widths; live rendering below. |

Summarization is still a model operation and cannot guarantee perfect recall. The deterministic loss before summarization is removed; every source byte reaches summarization or is preserved in the recent turn. The original journal remains available locally.

## Live retest

Used the virtual desktop with the built Electron app and native ACP sidecar, a localhost backend with a synthetic test account, and genuine NEAR/Tinfoil inference. The isolated debug harness permitted only its local HTTPS backend; provider attestation, E2EE, response authentication and TLS pinning remained enabled. That harness exception is not part of this commit. Provider credentials stayed on the server.

- Started NEAR GLM 5.3 Flash in thread A and navigated away during setup. Its request continued. Started thread B on the same model while A was streaming; B completed before A was stopped. Tinfoil GPT-OSS 120B completed concurrently with both NEAR requests. NEAR still exhibited provider latency; the separate requests no longer waited on one shared connection.
- Stopped A: its dispatch became `waived`, charge 0, usage unknown. A subsequent NEAR request completed. The initial retest also exposed loss of the cancelled prompt in the native in-memory history; the journal-restore fix and a new regression address that. A final real Tinfoil Stop/follow-up test returned the original marker `PINE-8642`, with the stopped request unverified and waived and the follow-up verified. See [accounting and answer](fixes/cancelled-followup.json) and [screenshot](fixes/cancelled-followup.png).
- Kept a draft through reselecting its thread and switching away/back. The code-only reply remained full width: [screenshot](fixes/code-and-restored-draft.png).
- Rendered list entries 998–1002 with all marker digits visible: [screenshot](fixes/list-998-to-1002.png).
- Built a 16-message conversation with six 47,550-byte archives, early project requirements and a later venue correction. Manual compaction used three authenticated summary requests. The next answer retained all six checked facts: marker `ALDER-7359`, budget 800, deadline 24 August, venue Riverside, step-free access and no peanuts. [Checks](fixes/compaction-verified.json) · [screenshot](fixes/compaction-facts.png). The date was returned as `2026-08-24`; the semantic check accepts that equivalent format.
- All 19 requests in this retest reached a final accounting state: 17 settled and two cancelled/waived, with no unresolved dispatches. Total test-account debit was 41,650 microUSD ($0.04165). This is a local ledger check, not a reconciliation against provider invoices. [Ledger states](fixes/final-accounting.json).
- Closed the test app and stopped the test backend. Shutdown completed in 0.4 seconds with one interrupt. [Shutdown check](fixes/shutdown.json).

## Validation and rollout

- Backend full suite: 618 passed.
- Desktop unit suite: 165 passed; browser suite: 127 passed.
- ACP TypeScript client: 67 passed.
- Rust workspace (`cargo test --workspace --all-features`): 514 passed, six opt-in tests ignored. This includes 330 native library tests and 31 ACP transcript tests.
- Native and Desktop builds, generated protocol, workspace TypeScript checks, Rust formatting and `git diff --check` pass.
- Strict Clippy remains blocked by existing warnings in unchanged code, including the `axiom-inference` accounting API and enum/model definitions. No lint allowances were added.

The native SQLite migration advances to schema 7 and backfills the latest user-message timestamp. The backend requires Alembic migration `0014` before running the updated cancellation code; it extends the zero-charge waiver constraint to NEAR and Phala as well as Tinfoil. No production database was migrated. Existing dispatched request IDs remain non-replayable. Unknown usage is waived, while already authenticated final usage can be billed exactly if Stop races settlement.
