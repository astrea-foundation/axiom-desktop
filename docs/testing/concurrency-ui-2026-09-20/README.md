# Desktop concurrency and long-conversation test — 2026-09-20

**Seven issues reproduced.** Fix compaction data loss, NEAR cancellation/connection blocking, and first-send loss before treating the multi-conversation experience as reliable. Three simultaneous Tinfoil generations completed and verified; switching providers retained conversation facts; saved history survived restart. Automatic compaction then discarded those facts.

This is an exploratory test report, not an implementation of the fixes. No deployment or release was performed.

## Environment and coverage

- Native `dev`: `244d51f734a66e5a34090bc4b8534fc2e965c628`; backend inference code from `b85d06c0932be2944690912b9463a0b522724dc4`. The backend checkout advanced to `bff88add9782d26a2bce467015537130b0ddf951` during the setup period; that commit retired the browser chat app and made no changes under `backend/src/`.
- Linux virtual desktop, 1280 × 800, with an additional 860 × 620 window check. Real Electron Desktop and native sidecar, built from the merged revisions. All prompts, model choices, cancellation, navigation and restart used desktop control. Read-only CDP/native state and backend operational metadata supplemented screenshots.
- Isolated local profile, synthetic account credit and local PostgreSQL; real paid Tinfoil and NEAR inference. Rate limiting was enabled. This does not measure production deployment behavior or the providers' general reliability.
- The local relay used HTTPS and a dedicated test CA. A detached test worktree allowed precisely the loopback relay through a debug-only, environment-gated address check. Provider attestation, E2EE and response authentication remained enabled. No plaintext inference or verification bypass was used. The normal product debug binary was rebuilt afterward and checked to exclude the test-loopback marker.
- Models exercised: Tinfoil GPT-OSS 120B, Tinfoil GLM-5.3 and NEAR GLM 5.3 Flash. This pass does not retest every model, filesystem tool, OS, TUI or production auth flow; see the [earlier Desktop/TUI report](../tinfoil-ui-2026-09-18/README.md) for that separate coverage.

| Scenario | Observed result |
| --- | --- |
| Tinfoil + two NEAR conversations | All three dispatched and streamed concurrently. Tinfoil finished; NEAR's long reasoning was stopped and exposed the cancellation defect below. |
| Two Tinfoil GPT-OSS conversations + Tinfoil GLM | All three actual requests overlapped and completed with native response verification. No mixed response text was observed. |
| Five client generations at once | Three Tinfoil runs, one NEAR stream and one NEAR follow-up waiting before dispatch. This is **not** five successful upstream requests. |
| Stop one Tinfoil generation | Target stopped; other conversations continued. Unknown usage was waived at $0 and not marked verified. |
| Queue a follow-up, navigate, then Stop | Queued text survived navigation; Stop paused it; explicit Send now resumed it. NEAR's resulting long run was subsequently stopped. |
| Tinfoil → NEAR → Tinfoil within one chat | Venue, budget, attendance, volunteers and original marker survived the switch. Model context metadata changed appropriately. |
| 17-turn conversation, including seven ~60 KB messages | Grew to 79,178 reported input tokens before automatic compaction on turn 16. Compaction completed cryptographically but lost all tested established facts. |
| Restart after compaction | All 39 ECHO timeline entries and 18 usage records restored in the same order with identical content. ALPHA also recalled its original marker after restart. |
| Smaller window | Composer controls remained available and wrapped into two rows. Existing list-marker clipping remained visible. |

[Concurrent-run snapshots](concurrency.json), [per-request metadata](requests.json), [conversation checkpoints and restart comparison](long-conversation.json), [small-window/restart screenshot](evidence/25-small-window-restart-recall.png).

## Confirmed issues

### UX01 · High · Automatic compaction loses established facts

**Reproduce:** Build a conversation with revised facts, then send several large, valid messages containing disposable data. In ECHO, turns 3–9 established a science event, changed its venue to Riverside Hall, budget to $2,800, attendance to 210 and volunteers to 16, while retaining its date, constraints and original marker. Turns 10–16 each supplied about 60 KB of synthetic inspection records and explicitly said they changed no event facts. Automatic compaction ran before answering turn 16.

**Actual:** Through turn 15 the model correctly repeated the venue and budget. The compaction summary said the earlier values were not present in its excerpt and substituted placeholders. Turn 16 answered “Venue: (as previously set)” and “Budget: (as previously set).” Turn 17 could recall none of nine requested fields, including the private marker. This is a loss of inference context; the visible saved transcript remained intact.

**Cause:** [compaction_transcript](../../../apps/axiomcli/src/agent.rs) around line 2707 removes assistant/tool messages first, then oldest user messages until the serialized transcript fits the budget. For GPT-OSS, `compaction_transcript_budget` permits only 250,674 bytes (`111411 × 3 × 3 / 4`). Large recent user messages displaced both the original requirements and the assistant replies repeating their values. The summarizer only saw the later archives, so it could not preserve facts absent from its input. Request `b8cada3e69c44b7983fbcdd0d8b715e5` completed and verified with 48,390 input / 494 output tokens; cryptographic success does not establish summary quality.

**Fix direction:** Summarize the older prefix before discarding it, preserve an existing summary and explicit decisions, and bound large recent payloads without silently evicting every earlier requirement. Validate this exact automatic-compaction case with facts introduced early and revised later; the previous small manual-compaction test did not cover it.

[Facts before compaction](evidence/13-memory-after-provider-switch.png) · [Compaction summary](evidence/18-compaction-loses-established-facts.png) · [Failed recall](evidence/19-post-compaction-recall-failed.png) · [Full transcript retained after restart](evidence/24-long-thread-restored-after-restart.png).

### UX02 · High · NEAR Stop leaves upstream running and blocks a follow-up

**Reproduce:** Start a long NEAR GLM response, press Stop, then immediately send a short follow-up in that conversation. CHARLIE's first request had been streaming reasoning for about 3 minutes 48 seconds when stopped.

**Actual:** The UI stopped, but the backend retained the old upstream stream. The follow-up waited **300.024 seconds** and failed with `STREAM_TIMEOUT`, “Timed out starting the encrypted request.” It never received a provider dispatch row. The stopped requests remained running at the relay even after Desktop closed. Test cleanup had to stop the isolated relay; three NEAR dispatches still lacked settled usage in the final local ledger.

**Cause:** Backend `backend/src/axiom_api/relay/routes.py:69`, `_detached_sse_response`, starts a separate execution task. Subscriber disconnect only sets `detached`; it does not cancel upstream. `backend/src/axiom_api/inference/providers/near_sessions.py:171`, `WorkerConnections.use`, holds the worker-session connection lock across the response. A follow-up using that session waits in `prepare` behind the old response. The frontend timeout also detaches, so it does not resolve the backend task. Tinfoil uses a different cancellation path and passed the abort/waiver check.

Original request: `55eb33127a7e47acb9876f949f5433f8`. Blocked follow-up: `3c150e7c9d364798b47ca20b9313f4c7`. Native and relay states are recorded separately in [requests.json](requests.json).

**Fix direction:** Give explicit Stop an upstream cancellation path, promptly release its connection, and bound connection waits. Recover exact usage where supported; otherwise apply the chosen unverified-charge policy. Do not automatically replay an uncertain dispatched request. Add cancellation/timeout/shutdown coverage with a second request using the same worker session. Separately review whether independent simultaneous requests need independent pinned connections.

[Immediately after Stop/follow-up](evidence/06-near-stop-followup.png) · [Five-minute timeout](evidence/16-near-followup-times-out.png) · [Tinfoil Stop passed](evidence/05-stop-one-thread.png).

### UX03 · High · Leaving a new thread during “Sending…” silently loses the message

**Reproduce:** On a new thread, select NEAR GLM, send a message, then click New thread while the optimistic message says “Sending…”. Reproduced during initial BRAVO setup and again with `FOXTROT-FIRST-SEND`.

**Actual:** The displayed message disappears; an empty untitled thread remains. There is no saved draft, queued message or inference request. In the second reproduction the new thread had only a security-status timeline item and zero request records.

**Cause:** [App.tsx](../../../apps/desktop/src/renderer/src/App.tsx), `submitDraft` around line 751, displays the message before `newChat` and settings calls finish. Navigation calls `cancelThreadPreparation`, invalidates `submittingDraft`, and clears the draft. `currentSubmissionState` then returns null and submission exits before durable enqueueing. Preventing a late selection change is correct, but dropping the user's text is not.

**Fix direction:** Persist the pending submission before asynchronous setup; let navigation change selection without losing that operation. If setup is intentionally cancelled, retain a recoverable draft and show its status. Test navigation at each asynchronous boundary.

[Draft](evidence/20-new-thread-draft.png) · [Message shown as Sending](evidence/21-first-send-in-progress.png) · [Empty thread afterward](evidence/22-first-send-abandoned-empty-thread.png).

### UX04 · Medium · Clicking the selected thread erases its unsent draft

**Reproduce:** Type an unsent message, then click the same currently selected sidebar row.

**Actual:** The composer becomes empty immediately. Switching away also loses the draft. This reconfirms an existing draft-loss problem rather than establishing it was introduced by Tinfoil.

**Cause/fix:** `loadThread` in [App.tsx](../../../apps/desktop/src/renderer/src/App.tsx), around line 609, calls `setDraft("")` unconditionally. Make reselecting the active thread a no-op for the draft and persist drafts per account/thread.

[Before clicking](evidence/14-draft-before-current-thread-click.png) · [After clicking the same thread](evidence/15-draft-lost-current-thread-click.png).

### UX05 · Medium · A code-only answer collapses to 29 pixels wide

**Reproduce:** Ask for a single fenced JSON object with no surrounding prose. ECHO turn 8 produced a correct 399-character JSON object.

**Actual:** The entire block renders as a narrow vertical sliver, hiding almost all its content behind horizontal scrolling. DOM measurements: bounding width 29 px, client width 27 px, scroll width 329 px. This is a rendering defect, not a missing provider response.

**Cause/fix:** [MessageBubble.tsx](../../../apps/desktop/src/renderer/src/components/MessageBubble.tsx), around line 97, places assistant content in a `self-start` container without a definite width. [index.css](../../../apps/desktop/src/renderer/src/index.css), around line 1007, gives `pre` `contain: inline-size` and `width: 100%`. Inline-size containment removes its intrinsic width contribution inside the shrinking parent. Give the assistant Markdown container a suitable definite width or remove that containment in this layout; retain horizontal scrolling for genuinely wide code.

[Collapsed JSON](evidence/12-json-code-block-collapse.png).

### UX06 · Medium · Concurrent streams continually reorder the sidebar

**Reproduce:** Run multiple responses while switching between their sidebar entries. Incoming text and reasoning repeatedly move active rows; clicking a remembered position can select a different conversation. Search filtering was needed to select the intended thread consistently during the test.

**Cause/fix:** [session.rs](../../../apps/axiomcli/src/session.rs), around line 1229, changes `last_message_at` on every `TextDelta` and `ReasoningDelta`. [App.tsx](../../../apps/desktop/src/renderer/src/App.tsx), line 284, sorts by that value. Use a stable turn/message ordering timestamp during streaming, or defer movement until completion. Preserve unread indicators without moving click targets for each chunk.

[Concurrent-run state and timestamps](concurrency.json) · [Sidebar during concurrent activity](evidence/07-concurrent-sidebar-a.png). A still image alone does not demonstrate the movement; this finding also relies on direct interaction and the timestamp update/sort path.

### UX07 · Low · Three-digit ordered-list numbers are clipped

**Reproduce:** Generate a numbered list exceeding 99 entries. Tested with 160, 240 and 320 entries.

**Actual:** The leading digit of three-digit list markers is clipped at the left edge. At the smaller window, item 160 displayed as 60. The underlying response contains the correct list.

**Cause/fix:** [index.css](../../../apps/desktop/src/renderer/src/index.css), around lines 925 and 1028, combines `overflow-x: hidden` with a 1.5 em ordered-list gutter. Reserve enough marker width for longer lists or use a marker layout that stays inside the content box.

[Long-list clipping](evidence/02-long-list-clipping.png) · [Small-window example](evidence/25-small-window-restart-recall.png).

## Other observations and limits

- A ~300 KB paste was retained in the composer but rejected on Send with “Queued messages must contain text and be no larger than 64 KB.” The cap also applies to an idle conversation, not only queued follow-ups. A visible size indicator and earlier validation would explain this better. This was a handled rejection, not counted among the seven confirmed defects. [Screenshot](evidence/17-large-paste-64kb-rejection.png).
- The long conversation showed delayed typing/painting; one read-only CDP connection timed out after 30 seconds. Large transcript rendering is a profiling candidate, but this run does not isolate renderer work from tooling or shared-host load, so it is not claimed as a separately proven root cause.
- Long NEAR reasoning eventually displayed “TEE proof expired” while the original response was still running. Clarify whether the badge describes freshness for a new request or the accepted in-flight session. This observation does **not** establish an attestation bypass. [Screenshot](evidence/09-near-long-reasoning-expired-proof.png).
- Fixed word-count prompts made NEAR reason at length about sentence lengths. That model behavior is distinct from UX02's local connection wait, which had no upstream dispatch. The run is insufficient to judge overall NEAR reliability.

## Usage and cleanup

There were **30 relay invocations and 29 actual provider dispatches**: 25 completed with native response verification, one Tinfoil cancellation waived at $0, and three NEAR dispatches without final usage. The remaining invocation was CHARLIE's blocked follow-up and never dispatched. Settled Axiom test-ledger charges totaled **$0.116476**: Tinfoil $0.115164 and NEAR $0.001312. These are not a complete provider-spend reconciliation; the three unresolved NEAR requests must not be treated as free or verified.

After recording final state, Desktop was closed and the isolated relay stopped to terminate its remaining local upstream connections. Graceful relay shutdown waited on background work and required a second interrupt. Final database rows still showed the unresolved NEAR operations as running/dispatched; that state is preserved in the evidence rather than manually rewritten. The normal native debug binary was rebuilt without the loopback test allowance. Credentials, complete native state dumps and large pasted archives remain outside this report.

Recommended fix order: **UX01, UX02, UX03**, then draft preservation and rendering, followed by stable sidebar ordering. Retest the same concurrency and automatic-compaction scenarios after the fixes.
