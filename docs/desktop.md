# Desktop guide

Axiom Desktop is an Electron interface to `axiomcli acp --frontend desktop-chat`.
The native sidecar owns accounts, messages, tools, storage and provider E2EE.
For installation see [Install and update](installing-desktop.md); source builds,
capture and visual conventions are in [Development](development.md).

## Accounts and conversations

Sign in opens the hosted account page in the system browser. Desktop displays
the verification code and provides reopen/cancel/retry actions; the website
handles passkey, Google, password and wallet authentication. Native login credentials stay
outside the renderer. Signing out closes the active account store and invalidates
pending account operations.

Conversations and folders are account-local. Folder names are labels, not file
paths. Rename and Delete use in-app dialogs; batch deletion is confirmed against
the exact native selection. Deleting a conversation does not remove workspace files.
Sidebar recency follows the latest **user submission** (`lastUserMessageAt`), so
concurrent assistant streams do not continually reorder the list. `lastMessageAt`
remains message activity metadata. Folder ordering derives from its member threads.

Titles are generated locally from the first prompt without a separate model
request or charge. Drafts are scoped to account/thread, with a separate new-chat
draft; navigation, reselecting a thread and reload preserve them. An anonymous
draft can follow the first sign-in. Pending first-message setup has its own
identity, so navigating away does not silently discard or redirect it.

## Balance and gift credit

Open the account menu and choose **Balance** to view credit and top up with mainnet
Zcash. The reusable address and QR code appear once; automatic and manual balance
refreshes update that panel in place without clearing an entered gift code.
**Redeem a gift code** adds its USD value to the signed-in account. Code
entry is masked and cleared on success, closing the screen, disconnect or account
change. It is never a chat message or saved draft. A failed/uncertain request can
be retried with the same code without duplicate credit; a code already redeemed
by this account reports that result and refreshes the balance.

Gift credit is part of **Other credit**; trial credit is spent first. Gift
redemption does not clear payment-review holds. It requires a native interactive
account and a backend with gift redemption deployed. Desktop and its bundled
sidecar negotiate `billing@3`; older sidecars must be updated together.

## Agent and Web controls

New threads start with Agent **off**, a separate managed working directory and
Web **off**. Agent settings let the user enable local tools, choose **Approve
commands** or **Full access**, and select an existing absolute directory using
the native picker or reset to the managed thread directory.

Agent settings persist atomically. Save requires an idle thread; the renderer
also requires an empty queue. Prompts and steering carry the saved settings
revision, so queued text cannot silently inherit a different permission/directory
configuration. A change clears cached policy grants and file-change tracking.
Desktop does not launch configured MCP servers.

Full access removes routine approval prompts while native validation and deny
rules still apply. Linux command tools require Bubblewrap and run without host
network access. macOS/Windows commands run with the user's OS authority and a
sanitized environment; selecting a directory does not create an OS sandbox there.
See [process boundaries](threat-model.md#local-tools-and-processes).

Web independently controls Axiom's search and page-fetch tools. Enabling it
shows a privacy disclosure: queries and fetched URLs are shared with external
services and are not provider E2EE. The warning explains that Web searches run
outside Axiom’s verified private environment while model conversations remain
end-to-end encrypted; **Yes, I understand** confirms enabling Web. Each existing
thread remembers its explicit choice locally; warning acknowledgement is a
separate per-account preference.
Every prompt passes an explicit boolean, and the native runtime rejects Web
execution without consent. Changing Web access requires the active response to stop.
On platforms without process network isolation, turning Web off does not disable
networking by approved local programs.

## Sending and queues

The first message appears immediately while its thread is created and configured.
The composer stays visible at the bottom during setup, with input and controls
temporarily disabled until the thread is ready. It becomes usable as soon as the
message is dispatched, without waiting for the provider's reply.

Enter sends when idle. During a response, it adds input to a FIFO queue.
**Send now** stops the current reply, waits for its cleanup, then sends the
selected message as a new turn. Its text, attachments, Web choice and Agent
settings are preserved. Other queued messages retain their order. The cancel
button removes a queued message, including while Send now is stopping a reply.
Cancelling that pending send leaves the remaining queue paused. Once a message
has been sent, use Stop on its reply.

Prompt text may use up to **4 MiB of UTF-8**; direct file bounds are listed below.
The model's context capacity and the configured native context budget still apply.
Queues hold at most 32 messages and 64 MiB of payloads, scoped to the account.
Large payloads commit to local IndexedDB before the draft is cleared; localStorage
holds only the outbox manifest. Stable
client IDs reconcile delivery with the native transcript. Stop pauses automatic
queue delivery. Restored or uncertain submissions require review; disconnect,
restart or account changes never justify silently resending them. A fresh prompt
after Stop can start normally when no older queued message needs review.

## Images and files

Use Attach, paste an image, or drop files anywhere in the main chat panel,
including its empty space and transcript. A drop indicator marks the panel;
the sidebar and Proxy view do not attach files. Image and file icons
in the model picker show the selected model's current capabilities; hover or use
the accessible label for their meaning. See [catalog discovery](provider-catalogs.md)
for refreshes and unavailable models.

Images are original PNG/JPEG/WebP/GIF bytes. File-capable Tinfoil models accept
text/code, Markdown, CSV, HTML, JSON, XML, PDF and Office Open XML files
(DOCX/PPTX/XLSX). NEAR supports images where advertised, but currently advertises
no general document uploads. Axiom reads original bytes without extraction,
OCR, conversion or rasterization. Document processing belongs to the attested
provider. Unsupported files remain visible in the draft and block sending until
you remove them or select a model that supports them.

A message can contain eight attachments, up to 5 MiB per image, 10 MiB per file,
and 16 MiB of text plus base64-encoded attachment payloads. Text remains bounded
to 4 MiB. Model context and native budgets still apply; inputs are never silently
truncated. Audio/video and remote URLs are unsupported.

Original bytes and filenames reach the provider inside its authenticated E2EE
request. Attachments remain in account-local history, survive edit/regeneration,
and appear as wrapping cards in the draft, queue, and transcript. Image cards
show local previews. Sending retains these cards through thread setup and native
acknowledgement, without waiting for the provider or reloading the original
files. Saved image previews load from local history as they come into view.
Compaction sends original image/file
parts to a capable model and retains a textual successor summary; it never
inserts encoded file bytes as message text. Older extracted-text attachments are
readable in saved history but cannot be submitted as new uploads.

## Edit, regenerate and compaction

Edit and Regenerate use the negotiated `messageRevision@1` flow. The client
submits the original user item ID and expected thread revision. AxiomCLI
atomically truncates that turn and later local history, preserves request
accounting, constructs the replacement messages, and starts ordinary attested
E2EE inference. Stale revisions, cross-account targets and active turns are
rejected. Regeneration after steering targets the turn's original prompt.
Already-performed file edits and other tool side effects are not rolled back.

Compaction uses encrypted, tool-free model requests to build successor context.
Automatic compaction follows the selected model's advertised capacity and local
limits. The runtime summarizes older history in bounded stages and keeps a usable
recent tail; summaries persist for resume. Cancellation never marks unfinished
compaction successful. Compaction is model-generated and is not a guarantee of
perfect recall. See the [recorded regression evidence](testing/concurrency-ui-2026-09-20/FIXES.md).

## Usage and verification

The composer meter displays provider-reported input/output usage for the **last
completed conversation request**, with that request's model capacity and effective
compaction threshold. The 85% threshold controls when history is compacted; it
no longer caps every response to 15% of context. Tinfoil uses upstream's remaining-context generation default. Other protocols
use the advertised output ceiling and estimated remaining request space (including
tools/images and a margin). Exact tokenization and final context enforcement belong
to the provider.
A verified `length` finish reason is shown as an output-limit notice, with the
option to ask for continuation. The usage meter is not a draft estimate or cumulative charge. Reports
survive reload, and model switches do not rescale old counts against a new model.
No report is shown as unknown, rather than fabricated zero usage.

Hover, focus or click the meter to see context use and the compaction threshold.
Expand **Usage details** for input/output counts, turn totals, cache and reasoning
subsets, settlement state, elapsed time and other thread charges. Tab reaches the
disclosure; Escape closes the panel and returns focus to the meter. Partial or
unavailable usage and reports from a different model remain visible above it.

Click the TEE badge for the last local verification report; Refresh requests
fresh evidence when allowed. While active and connected, the badge automatically
renews within 60 seconds of expiry, between replies. Checks are deduplicated and retries back off
from 30 seconds to at most five minutes; even a successful short lease is paced.
After 30 minutes without user interaction or a running reply, it shows neutral
`Idle` and stops automatic checks. Keyboard/pointer activity or returning to the
window resumes verification. Background checks and network events do not reset
inactivity. A failed or blocked refresh cannot extend a report or mark it verified;
native inference always enforces fresh proof independently of this display.
Reports show hardware, encryption-key, freshness and source-provenance checks
for the selected provider. Tinfoil reports router
verification, and NEAR v3 identifies an attested
worker TLS key rather than claiming the client observed the backend's socket.
Provisional streamed text becomes verified only after authenticated completion;
cancelled/incomplete output retains that status even if billing later settles.
Unexpected stream endings show one short notice, with the technical reason under
**Details**. A turn's error notice replaces the duplicate warning below its reply.
Stopping a reply yourself does not show an error banner. These presentation
choices do not change completion verification or mark partial replies successful.
Reply notices and the top error banners have an **×** button to dismiss them.
Dismissal hides the current notice in the open view; a new or changed error is
shown again. It does not delete conversation records or change a failed reply's
status.
See [security](threat-model.md).

## Account credit and usage

The Axiom Payments UI shows a persistent mainnet ZEC address and bounded
deposit receipts from the authenticated backend. Integer zatoshi amounts and
versions remain exact strings/BigInt. Optional USD credit uses the backend's
reported confirmation-time valuation; a live quote is indicative, not a promise
of final credit. No ZEC-to-USDC conversion or native wallet custody is claimed.
Testnet or inconsistent valuation responses are rejected. Only mainnet deposits
are supported; invoice checkout is removed.

Settings → Usage shows posted account-ledger inference charges, including other
clients/devices. Top-ups, pending holds and unbilled estimates are excluded.
Unknown historical model attribution stays in Other models. Account changes
invalidate stale requests and projections.

Settings → API keys lists, creates and revokes named automation keys and shows
usage attribution. Creation can require recent sign-in; the newly created key
is deliberately displayed once for copying. This separate account-management
flow does not expose the native access/refresh token or accept an API key as a
Desktop login credential.

## Presentation and local integrations

Tool activity appears at its execution point between assistant segments, with
stable IDs for live/snapshot reconciliation. Markdown, code and KaTeX render
locally with bundled fonts. Scrolling up or selecting transcript content pauses
automatic following; Scroll to latest resumes it. Theme is light, dark or system.

The Proxy screen starts the optional authenticated local integration explicitly.
See [Local proxy](proxy.md) for its separate credential and lifecycle. Updates
use one release feed with AxiomCLI and are downloaded and checked before user
installation. The Updates panel shows the installed version, with platform,
architecture and package format under **Installation details**. Quit Axiom and
AxiomCLI before installing. Desktop requires its CLI to report the exact same
product version at ACP initialization; a mismatch blocks bootstrap with
reinstall/rebuild guidance.
See
[Install and update](installing-desktop.md#install-a-newer-version).

Source and regression evidence:
[renderer](../apps/desktop/src/renderer/src/App.tsx),
[ACP runtime](../apps/axiomcli/src/acp.rs),
[session storage](../apps/axiomcli/src/session.rs),
[headless UI tests](../apps/desktop/tests/browser/deletion-focus.test.ts),
[ordering](../apps/desktop/tests/thread-ordering.test.ts),
[context reports](../apps/desktop/tests/context-usage.test.ts), and
[deposit validation](../apps/axiomcli/src/billing.rs).
