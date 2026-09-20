# Tinfoil Desktop and TUI test — 2026-09-18

All four configured Tinfoil models completed real, encrypted requests in both applications. Desktop and TUI could complete filesystem tasks, retain conversation context across model switches, and display settled token counts and costs. TUI manual compaction preserved the important facts. Both applications rendered streamed text and stopped without marking the partial answer verified.

One integration defect was fixed during this initial test: Desktop rejected valid Tinfoil proof reports because the ACP contract required a relay lease generation that Tinfoil does not use. The findings below preserve that initial baseline. **The remaining issues have since been addressed; see the [fixes and live retest](FIXES.md) for current behavior, evidence and verification limits.**

The UI run dispatched **31 provider requests**, including tool continuations, titles, compaction and two responses stopped by the client. All 31 settled at the backend, totaling **$0.100773**. This is the Axiom test ledger's metered cost, not an independently reconciled Tinfoil invoice. Earlier API canaries are excluded. [Per-request operational metadata](usage.json) records native verification separately from backend completion.

## Environment and limits

- Native feature branch: `feat/tinfoil-provider`, starting at `2d4e8d0`; backend feature branch at `01cc95d`. The proof fix accompanies this report. No merge or deployment was performed; Chutes stayed separate.
- Linux virtual desktop, 1280 × 800; Electron Desktop and Alacritty TUI. Prompts, model changes, file permissions, cancellation and restart were exercised through desktop control. Read-only CDP state, native SQLite records and the local backend ledger supplemented the screenshots.
- Isolated native data/profile directories and a synthetic local account. Desktop and TUI deliberately shared that account and its native credential store. The provider credential remained server-side.
- Real backend with a local PostgreSQL database and synthetic account credit; real paid Tinfoil inference. Local rate limits were disabled. This was neither a production concurrency benchmark nor a production authentication/deployment test.
- The local relay used HTTPS with a dedicated test CA. A temporary debug-only, environment-gated transport patch permitted precisely the loopback test relay in the UI binaries. That patch was restored before committing; it is not in the product change. Provider URL restrictions, native Tinfoil attestation, HPKE and response authentication stayed enabled. No plaintext inference or provider verification bypass was used.
- Native verification covers the Tinfoil router's SNP evidence, source provenance and live key binding. The measured router verifies workers transitively; these screenshots do not establish independent client verification of every GPU.
- Manual `/compact` was tested; filling a million-token context or triggering the automatic compaction threshold was not. Tool execution in this UI run covered GPT-OSS in TUI and GLM in Desktop; all four models' tool protocols were exercised by the earlier integration canaries, not every tool/model/UI combination here.

## Coverage

| Model | Desktop result | TUI result |
| --- | --- | --- |
| GPT-OSS 120B | Correct three-row repair-cafe table, remembered “Lantern”, token/cost tooltip, verified proof after fix | Read CSV and instructions, wrote and read back `summary.md`; later compaction, streaming and cancellation |
| GLM-5.3 | Read files, retained project name from GPT history, created correct `desktop-plan.md` after one tool argument correction | Correctly identified the two excluded registration rows from the previous model's history |
| Kimi K3 | Completed the requested three-paragraph reply; background title failed separately | Correctly updated the hypothetical count to five after one electrical volunteer withdrew |
| Llama 3.3 70B | Correct Unicode bullet response; visible streaming, Stop, incomplete proof and successful follow-up | Correctly updated the hypothetical count to eight after the requested replacements/additions |

The model menu exposed the catalog prices, context windows and model-specific thinking controls. [Catalog](evidence/desktop-model-catalog.png), [Desktop token/cost tooltip](evidence/desktop-gpt-usage.png), [Desktop GLM result](evidence/desktop-glm-final.png), [Desktop Kimi result](evidence/desktop-kimi-response.png), [Desktop Llama result](evidence/desktop-llama-response.png).

TUI results: [GPT agent task](evidence/tui-gpt-agent-completed.png), [GLM](evidence/tui-glm-followup.png), [Kimi](evidence/tui-kimi-followup.png), [Llama](evidence/tui-llama-followup.png), [security inspector](evidence/tui-security-inspector.png), [manual compaction](evidence/tui-compaction.png), [facts retained afterward](evidence/tui-after-compaction.png).

The synthetic [input fixtures](fixtures/README.md) contain eight registration records: six valid unique entries, one duplicate email and one missing item. Both [TUI output](outputs/summary.md) and [Desktop output](outputs/desktop-plan.md) correctly reported six valid registrations, two per workshop. The denied-write test left `denied.txt` absent.

## Findings

### F01 · Fixed · Desktop rejects valid Tinfoil proof evidence

**Reproduction:** Complete a Tinfoil response, then open the privacy proof. The native response had authenticated successfully, but the composer showed “Proof unavailable” and later “TEE check failed.” The ACP error was `verified provider evidence is missing its attestation generation`.

**Cause and fix:** The ACP extension and TypeScript public evidence projection assumed every provider had an attestation lease generation. Tinfoil verifies its router directly and does not have that lease field. `attestationGeneration` is now optional in the wire schema, accepted without a value only for the exact Tinfoil provider/attestation/EHBP protocol combination. Zero generations, other protocols missing the field, missing expiry and invalid fingerprints still fail. Desktop omits the inapplicable generation row. Native cryptographic verification was unchanged.

**Retest:** Rebuilt Desktop/sidecar, reopened the existing conversation and verified all four proof sections. [Before](evidence/desktop-first-response-proof-error.png), [after](evidence/desktop-proof-fixed.png).

Relevant files: [ACP projection](../../../apps/axiomcli/src/acp.rs), [extension contract](../../../crates/axiom-acp-extension/src/lib.rs), [public evidence validation](../../../packages/axiom-acp-client/src/security-evidence.ts), [proof dialog](../../../apps/desktop/src/renderer/src/components/PrivacyProofDialog.tsx).

### F02 · High · Concurrent Desktop/TUI sessions reject the same account's work

**Reproduction:** Keep both applications open on the same native account, use one and then resume work in the other. TUI rejected a new turn with `invalid state transition: the native account changed in another Axiom process; refresh account status`. `/account` confirmed the same connected account and allowed the next request. Later Desktop rejected proof refresh for the same reason after returning to its threads.

**Impact:** Normal simultaneous use can block inference and make a healthy TEE provider appear broken. Desktop prominently labels this account-state error “TEE check failed”; the real explanation is hidden under Failure details. Restarting Desktop restored verification without changing provider configuration.

**Investigation:** [AuthManager](../../../apps/axiomcli/src/auth.rs), especially `adopt_external_credential` and `access_token_internal`, marks externally changed credentials as pending and gates work. Credential rotation by a peer is a likely trigger; the precise initiating refresh was not traced. Preserve real account-switch isolation while distinguishing a refresh for the same account, and offer an actionable account refresh instead of classifying it as failed TEE evidence.

[TUI error](evidence/tui-concurrent-account-change-error.png), [Desktop error and underlying account detail](evidence/desktop-account-error-mislabeled-tee.png).

### F03 · High · Stopped Desktop requests remain “running” in local accounting

**Reproduction:** Stop a Llama stream, wait for backend settlement, send another message and switch away from/back to the thread. The cancelled answer remained incomplete, correctly, but its local `request_usage` stayed `state: running`, `settled: false`, with null tokens/cost for more than seven minutes after backend settlement.

**Measured example:** Request `18991035c8304c3db29f5b974efead1c` settled with 1,254 input / 3,895 output tokens and cost $0.012906. The next short reply settled normally while the stopped request remained locally pending. Restarting Desktop converted the orphan record to `CLIENT_INTERRUPTED`; reopening the thread then recovered its exact counts and cost, with `responseVerified: false` preserved.

**Investigation:** [ACP timeline recovery](../../../apps/axiomcli/src/acp.rs), `GetThreadTimelineRequest`, excludes unsettled records whose state is still `Running`. The ACP cancellation path can finish the turn without receiving a terminal request-accounting update, leaving the record outside that recovery filter. Persist cancellation before ending the turn and reconcile accounting independently of whether the response completed successfully.

TUI's stopped response recorded `cancelled` immediately, but its displayed cost remained pending while idle. Its existing recovery path runs when resuming a session; that restart/resume path was not exercised for TUI in this run. [Stopped TUI output with pending usage](evidence/tui-cancelled-mid-text.png).

### F04 · Medium · Stop does not stop provider generation or billing

**Reproduction:** Request a long list and press Stop during visible text. The client stops rendering, but the backend continues consuming the encrypted response to obtain terminal metering. Desktop was stopped around item 14 of 100; the provider request ran for 88.5 seconds in total and billed 3,895 output tokens. A stopped TUI GPT request also settled after cancellation: 3,576 input / 3,178 output, $0.002443.

**Cause:** This is the current deliberate settlement design in backend `relay/tinfoil.py`: detaching a subscriber does not cancel the tracked provider task. It avoids losing billing data, but the button does not communicate that generation and spend may continue. Resolve the cancellation/settlement contract before promising that Stop ends charges. Do not discard provider usage or mark a partial answer verified to conceal the difference.

[Desktop streaming before Stop](evidence/desktop-long-stream-before-cancel.png), [incomplete reply proof](evidence/desktop-cancelled-reply-proof.png), [TUI streaming text](evidence/tui-streaming-text.png), [TUI stopped mid-text](evidence/tui-cancelled-mid-text.png).

### F05 · Medium · The first message after Stop silently joins a paused queue

**Reproduction:** Stop the Desktop list, type a fresh short follow-up and press Send. It appears under “Queue paused · 1” with “Sends after the current reply,” even though generation has stopped. Clicking “Send now” sends it successfully.

**Cause:** [MessageQueue](../../../apps/desktop/src/renderer/src/messageQueue.ts) retains the paused thread state; `enqueue` does not distinguish a new explicit send from work queued before cancellation. [Queue copy](../../../apps/desktop/src/renderer/src/components/MessageQueue.tsx) also promises a future send while paused. Resume on an explicit new send or clearly require review, while preserving cancellation of previously queued work.

[Paused queue](evidence/desktop-after-stop-queue-paused.png).

### F06 · Medium · Approving one tool hides the next approval buttons below the composer

**Reproduction:** In Desktop Agent mode, ask GLM to read the supplied files and write the plan. After approving a tool, the next permission card appears partly below the viewport, with Allow/Deny behind the fixed composer. This repeated for subsequent reads without manually scrolling upward. The down-arrow jump restores access.

**Cause:** [useChatAutoScroll](../../../apps/desktop/src/renderer/src/useChatAutoScroll.ts) pauses following for clicks inside any `[data-timeline-kind]`, including approving a tool. The next permission therefore does not reliably enter view. Treat approval activation differently from intentionally browsing history, or keep the active approval controls visible. The repeated raw “confirm profile requires approval for this tool use…” rows also make the transcript hard to scan.

[Clipped permission card](evidence/desktop-approval-below-fold.png).

### F07 · Medium · Kimi's background title generation spends tokens and leaves “New thread”

**Reproduction:** Start a Kimi thread and ask for a three-paragraph reply. The main answer succeeds; the thread remains “New thread.” Native diagnostics report `provider returned an empty session title`.

**Evidence:** Title request `f8644e404aef4d75a7326ba37fb019d3` used 201 input / 128 output tokens, ended with `length`, and cost $0.003364. The main answer cost $0.017412. There was no visible title-failure notice.

**Investigation:** [generate_title](../../../apps/axiomcli/src/agent.rs) sets a 128-token output budget and `ThinkingMode::ProviderDefault`, despite its comment about avoiding thinking tokens for labels. Exhausting the tiny budget on reasoning is a likely explanation; this provider accounting did not supply a separate reasoning-token counter. Use a supported title strategy with enough visible-output budget and a deterministic local fallback when normalization gets empty text.

[Completed Kimi reply with unchanged title](evidence/desktop-kimi-response.png).

### F08 · Medium · Regenerate is visible but permanently disabled

The completed Desktop reply exposes a disabled Regenerate icon with tooltip “Regeneration is temporarily unavailable.” [MessageBubble](../../../apps/desktop/src/renderer/src/components/MessageBubble.tsx) hardcodes `disabled`. Clicking it produced no request. This is a pre-existing product limitation, not a Tinfoil failure, and regeneration was **not** validated. Implement the native encrypted message-edit flow or make availability clear before presenting this control.

### F09 · Medium · Question permission and repeated confirmation obstruct the TUI agent flow

During the explicit request to create a harmless test file, GPT first invoked `ask_user_questions`. The confirm profile asked permission to run the question tool, then the question asked again whether to create the already requested file. After the eventual write was denied, the file stayed absent, but the model continued with a read of the missing file and another question instead of ending cleanly. Esc stopped the turn.

The permission boundary worked. The issue is redundant interaction and recovery after denial. Review the question-tool permission presentation and teach the agent to respect a denial without immediately circling back through equivalent requests.

### F10 · Medium · Models confuse the patch tool's `mode` field with an operation name

GLM in Desktop and GPT in the TUI permission scenario emitted an `apply_patch` call with a string such as `mode: "create"`; the local tool expected an integer file mode and rejected it. GLM corrected the call on its next turn and successfully wrote the file. This was a schema/model usability failure, not a dropped provider tool call.

Clarify the file-permissions field in the schema, or choose a less ambiguous field name. Keep strict validation. The extra repair request adds latency and billed tokens.

### F11 · Low · Markdown tables split short labels mid-word

At 1280-pixel desktop width, the otherwise correct GPT table rendered “Electrical,” “Sewing” and “Woodwork” over awkward character breaks in a very narrow first column. [Screenshot](evidence/desktop-first-response-proof-error.png). Review the table's intrinsic column sizing and inherited `word-break` rules in [index.css](../../../apps/desktop/src/renderer/src/index.css).

### F12 · Low · TUI can keep “VERIFYING” after a pre-inference cancellation

Esc during a slow attestation check ended the turn as cancelled, but the header continued saying “VERIFYING.” A later successful request restored “SECURE.” No inference was dispatched for that cancelled attempt. Ensure cancellation clears the in-progress security display without promoting stale evidence to a fresh verification. [Screenshot](evidence/tui-stream-cancelled.png).

## Latency and usage observations

One GLM tool continuation took **98.4 seconds** between relay start and completion for 203 output tokens. Other GLM requests in this run took 0.8–5.3 seconds. The UI mainly showed a generic generation state during the outlier. This single observation does not establish a provider reliability rate or distinguish provider queuing from an upstream transport delay; instrument time to headers/first authenticated delta and subsequent idle intervals.

| Model | Provider requests | Input tokens | Output tokens | Test ledger cost |
| --- | ---: | ---: | ---: | ---: |
| GPT-OSS 120B | 19 | 38,785 | 7,601 | $0.010378 |
| GLM-5.3 | 5 | 20,664 | 546 | $0.024437 |
| Kimi K3 | 3 | 5,297 | 885 | $0.038888 |
| Llama 3.3 70B | 4 | 9,233 | 3,968 | $0.027070 |
| Total | 31 | 73,979 | 13,000 | $0.100773 |

Desktop accounted for 11 requests / $0.055255; TUI for 20 / $0.045518. The backend had zero unsettled dispatches for this UI window at the end. No provider HTTP 429 or failed dispatched inference was observed, but this small serial run does not validate sustained rate limits. Failed local account-state operations and cancellation before dispatch are not counted as provider calls.

## Validation of the proof fix

- `cargo test -p axiomcli --lib security_`: 6 passed.
- `cargo test -p axiom-acp-extension --lib`: 5 passed.
- ACP client `security-evidence.test.ts`: 10 passed, including the new Tinfoil contract regression.
- Desktop `privacy-proof.test.ts`: 4 passed.
- Desktop and ACP client type checks, native build, Desktop build, formatting and `git diff --check` passed.
- Clippy completed with warnings. Strict Clippy (`-D warnings`) stops on unchanged `axiom-inference` warnings: `must_use_candidate`, `large_enum_variant`, and `struct_excessive_bools`. These are not introduced by this proof fix; the test does not claim a clean strict lint run.
- Rebuilt Desktop showed valid proof for the persisted conversation; closing/reopening Desktop later preserved history, recovered stopped-request accounting, and restored live TEE verification.

The fix does not relax the registered provider's cryptographic, freshness, stream ordering or authenticated-completion requirements. It changes public proof transport/presentation to match the protocol actually in use.
