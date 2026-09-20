# Root-cause investigation — 2026-09-20

All seven findings have identifiable causes in Axiom. Follow-up checks reproduced the compaction loss, connection blocking, draft deletion and layout defects locally without sending additional inference requests. Three existing tests explicitly require behaviors that produced the failures: cancelling submission on ordinary navigation, retaining a disconnected relay producer, and reordering the sidebar on live output.

The deeper problem is inconsistent ownership and lifecycle rules. The selected screen owns an in-progress submission; an attested worker session owns a single busy connection; incoming chunks own sidebar ranking; a summary replaces history after that history has already been truncated. These rules work in isolated happy paths but conflict during navigation, cancellation and long conversations.

This document adds investigation to the [original live report](README.md). Product source is unchanged. Native revision: `7756c4f`; backend runtime source: `bff88ad`. Measurements and checks are recorded in [root-cause-checks.json](root-cause-checks.json).

## UX01: compaction discards the evidence before asking for a summary

The automatic trigger is not the primary failure. The input prepared for the summarizer is destructive:

1. [AgentEngine](../../../apps/axiomcli/src/agent.rs#L1893) triggers compaction from the larger of provider-reported context and a local character estimate, including the next prompt.
2. [compaction_transcript_budget](../../../apps/axiomcli/src/agent.rs#L1238) gives GPT-OSS a 250,674-byte transcript budget, smaller than the history that triggered compaction.
3. [compaction_transcript](../../../apps/axiomcli/src/agent.rs#L2707) truncates large tool results, then removes **every non-user message before removing old user messages**. Roles are used as a substitute for relevance. Important assistant decisions and recent repetitions of facts are discarded before large, disposable user payloads.
4. Only the surviving suffix reaches the summarizer. Any nonempty summary is accepted; [compacted_history](../../../apps/axiomcli/src/agent.rs#L2748) replaces inference history with the system message and that summary. Missing details cannot be recovered from the saved transcript by ordinary inference.

**Controlled reproduction:** Extracted these functions unchanged from the checked-out Rust source and used the actual `axiom-inference` message types. Reconstructed the 30 user/assistant messages before live turn 16 from the captured synthetic transcript. Its serialized size was **398,295 bytes**. Preprocessing removed **26 messages** and retained only the four user archives from turns **12–15**. The venue, budget and original marker were all absent from the summarizer input. This matches the live summary's explanation that earlier values were not in its excerpt. The failure does not require an unreliable model.

There is a related presentation mismatch: the provider reported 79,178 input tokens, while the reconstructed history's character estimate was 98,627 before adding the next ~15,037 estimated tokens and system overhead. Therefore compaction can legitimately start while the displayed provider count is below the nominal threshold. Increasing the displayed context window or disabling the trigger would not repair the loss.

**Why tests missed it:** `AutoCompactProvider` returns the literal `successor summary` regardless of its input. Tests such as `provider_usage_triggers_automatic_compaction_at_eighty_five_percent` and `a_new_prompt_that_crosses_the_threshold_compacts_before_inference` verify timing, accounting and replacement, not preservation of source facts. The lossy role-priority policy predates the Tinfoil integration (`64fd1df`).

**Required fix boundary:** Replace delete-then-summarize with bounded, staged summarization of the full older history. Preserve prior summaries, updated requirements and a usable recent tail. If a single payload must be truncated, expose that limitation rather than silently treating the result as complete memory. Keep summaries local and run every summarization request through the selected attested E2EE protocol. Regression coverage must introduce facts early, revise them, append large irrelevant user/tool payloads, trigger automatic compaction, compact a second time, and restart.

## UX02: cancellation, worker affinity and shutdown have incompatible lifetimes

There are two separate ways work becomes orphaned:

- **During a stream:** backend `relay/routes.py:53–115` creates a tracked producer independently of the HTTP subscriber. Closing the subscriber only suppresses further delivery. The producer continues reading and holding resources. This is deliberate legacy behavior, introduced in `410c01d`, and `test_detached_relay_subscriber_does_not_cancel_or_block_the_producer` explicitly checks it.
- **Before stream headers:** the route creates an invocation row, then awaits `_prepare_session`. NEAR's `request_snapshot` awaits `connections.prepare`. There is no disconnect watcher or bounded connection-lock wait here, and this preparation work is not yet in the tracked producer set. The native client gives up after 300 seconds in [relay/client.rs](../../../crates/axiom-secure-client/src/relay/client.rs#L269), but that does not cancel the server's wait. This is the path taken by CHARLIE's follow-up. The SSE detachment helper had not yet been reached.

NEAR's `WorkerConnection` has one HTTP/1.1 connection and a lock. `WorkerConnections.use` holds that lock across the provider stream; `prepare` needs the same lock for its public attestation GET. A stopped request therefore blocks the next request that reuses its worker session. The HTTP client's 300-second read timeout bounds idle reads, not total generation time, and its 15-second pool timeout does not bound the outer `asyncio.Lock` wait.

**Controlled reproduction:** Used the actual detached-SSE helper and actual `WorkerConnections.use`, with a control-flow-only generator and no provider HTTP. After subscriber close, the producer remained alive, the connection remained locked and the follow-up remained blocked. Explicit producer cancellation released the lock and the follow-up immediately acquired it. This isolates the failure from provider availability, tokens and attestation validation.

The impact is wider than one stopped thread:

- The native client caches NEAR verified material by model within its credential-scoped client, including the worker-session ID. Reuse can make separate conversations contend for the same session. Fresh explicit verification can create a different session, explaining why some parallel requests worked. This cross-thread contention follows from the cache/lock design; it was not separately isolated in another paid run.
- Backend `relay/service.py:25` permits only **four active generic relay responses per process**. A detached stream retains its slot until completion. Four such streams can exhaust this path's capacity and cause `server_busy` for unrelated requests. This is a code-derived capacity risk, not a claim that the live test exhausted all four. Tinfoil's dedicated relay has a separate lifecycle.
- Cleanup currently cancels tracked producers in application lifespan shutdown. The installed Uvicorn waits for pending HTTP tasks before invoking lifespan shutdown. A follow-up stuck in pre-header preparation can prevent reaching that cleanup. The backend Docker command sets no graceful-shutdown deadline. This matches the live shutdown waiting until a second interrupt.
- The local test's `near_max_tokens=8192` was not a guaranteed per-request cap: model-aware execution selects the request limit or advertised model limit in `inference/executor.py:362–366`. The configuration field is not applied as a minimum there. No exact output cap for the three unresolved requests is asserted by this investigation.

**Why tests missed it:** Two relay tests require producer survival after waiter cancellation; the worker-session tests cover account binding, receipt identity and TLS reconnect pinning but do not combine Stop with a same-session follow-up. The stream, connection cache and shutdown rules were tested separately.

**Required fix boundary:** Make cancellation own preparation, upstream transport, billing cleanup and resource release. Use consistent abort-on-disconnect behavior with Tinfoil, or provide an explicit authenticated Stop operation if future reconnect support needs a different disconnect policy. Bound preparation waits and shutdown independently. Preserve exact-usage recovery where available and waive unverifiable charges according to policy. Keep uncertain dispatched requests non-replayable.

Do **not** simply remove the connection lock or raise `max_connections`: `_connected_fingerprint` is mutable state for one pinned connection. Real concurrency needs separate connection-bound pin state and bounded leases, with each new connection checked before any provider POST bytes. TEE verification and response authentication remain required.

## UX03 and UX04: screen selection owns submission and draft state

These two losses share a state-model problem but occur at different stages.

**UX03 — accepted-looking message, no durable submission yet.** [submitDraft](../../../apps/desktop/src/renderer/src/App.tsx#L744) renders the optimistic bubble, then awaits thread creation, settings and optional Agent configuration. The durable queue requires a real thread ID, so it is populated only at the end. Meanwhile [cancelThreadPreparation](../../../apps/desktop/src/renderer/src/App.tsx#L151) invalidates the one global submission reference. New thread, another thread, account changes and runtime changes all use this invalidation. The navigation handlers also clear the original draft. Late setup returns see a stale submission and exit before enqueueing, with no recoverable copy.

The guard prevents an old asynchronous result from reopening a view or crossing an account boundary, which is necessary. The defect is treating **same-account navigation as cancellation of user intent**. The browser test groups `new`, `existing`, `account` and `runtime` destinations together and explicitly asserts no prompt is sent. Both ordinary-navigation cases passed during this investigation while enforcing the reported loss.

**UX04 — only one in-memory composer draft.** [useAccountDraft](../../../apps/desktop/src/renderer/src/useAccountDraft.ts) stores `{accountId, text}` in React state, with no thread key or durable storage. [loadThread](../../../apps/desktop/src/renderer/src/App.tsx#L610) unconditionally calls `setDraft("")`, even when its argument is already the active thread. The controlled renderer test changed `Draft survival canary` to an empty string on that exact click. Account scoping prevents cross-account display; it does not provide per-thread draft preservation.

**Required fix boundary:** Separate selected view, account-scoped draft records and durable submission operations. Persist pending text before asynchronous thread setup, with a stable operation ID and captured model/Agent/Web choices. Navigation should not alter the operation or its target; late completion should not steal focus. Keep account/runtime invalidation and exactly-once queue acknowledgment checks. Use per-thread drafts plus a separate new-thread draft, backed by account-scoped local storage. Test slow setup at each boundary, navigation away/back, same-thread clicks, setup failure, restart and account switches.

## UX05 and UX07: global overflow rules conceal valid Markdown content

**UX05 — circular intrinsic sizing.** The assistant wrapper uses `self-start` and an automatic width. Its code block uses `width: 100%` together with `contain: inline-size`, which removes the content's contribution to intrinsic sizing. The parent shrinks, then the code block takes 100% of that shrunken width. The remaining 29 pixels are padding/borders, not a content limit.

In the real renderer with a fake native bridge, the code block measured **29 px**. Changing only its containment to `none` made it **287.92 px**. Restoring containment and giving its containing wrapper a definite width made it **748 px**. No model output, Markdown parser or stored message was changed. This confirms both interacting CSS rules are involved.

**UX07 — outside markers exceed their gutter.** The ordered list has a **22.5 px** left gutter. Its font needs approximately **34 px** for `320. `, and list markers are outside the content box. The ancestor's `overflow-x: hidden` clips the left side. Changing only the gutter to 3 em restored `320` and `321` in the controlled renderer. [Before](evidence/root-list-before.png) · [After the temporary style intervention](evidence/root-list-after.png).

**Why tests missed them:** Markdown tests primarily inspect static HTML. Existing real-browser overflow tests cover equations and overall page width. They do not require a code-only response to have a useful minimum width or require multi-digit list markers to remain visible. Hiding overflow makes a “no page overflow” assertion pass even while content is clipped. The containment rule predates Tinfoil (`11368d5`).

**Required fix boundary:** Give Markdown content a usable, bounded width and put horizontal scrolling on genuinely wide code/table/math elements. Keep enough marker space for digit count, nesting and text direction. Do not globally reveal overflow as a shortcut. Browser checks should cover code-only versus prose-plus-code, a very long unbroken code line, 1–4 digit lists, nested lists, small windows and RTL text.

## UX06: a streaming activity timestamp is used as a stable ordering key

[session.rs](../../../apps/axiomcli/src/session.rs#L1229) advances `last_message_at` on every text and reasoning delta, as well as accepted user messages. [App.tsx](../../../apps/desktop/src/renderer/src/App.tsx#L245) merges this timestamp into thread metadata and sorts on each update. [threadOrdering.ts](../../../apps/desktop/src/renderer/src/threadOrdering.ts) also ranks folders by their newest child, so whole folders can move. Two streams alternate which timestamp is newest; moving rows are a deterministic result of this policy.

The timestamp is serving two different needs: signaling new activity and deciding where a conversation belongs. Keeping it current is useful for unread state but unsuitable as a per-chunk position key. The existing browser test literally requires reordering “immediately on live output”; it passed during this investigation. This behavior was added with the Desktop integration in `ad413a2`.

**Required fix boundary:** Separate activity/unread timestamps from a stable turn/message ordering timestamp. Choose and document when position changes—such as an accepted user turn or the first event of a background response—then keep it stable for the rest of that turn. Apply the same policy to folders and persistence. Test alternating deltas in two threads, click targeting, folder movement, filtering and restart ordering.

## Verification and implementation order

- Six selected existing renderer tests passed, including both ordinary-navigation cancellation cases, account/runtime isolation, live reordering and account draft clearing.
- Two existing relay lifecycle tests passed when invoked directly without database fixtures or network calls.
- The offline NEAR lock/cancellation probe, Rust transcript preprocessing probe and controlled renderer experiment reproduced their expected failures; renderer style interventions confirmed the two layout causes.
- No product patch, provider request, deployment or authentication relaxation was made in this investigation. Temporary probes used synthetic content and local test processes.

Fix compaction and NEAR request ownership first, then implement submission/draft ownership together. Handle the two layout fixes together and update the ordering contract separately. Change the tests that currently assert the unwanted behaviors; adding more happy-path tests without changing those expectations would preserve the bugs.
