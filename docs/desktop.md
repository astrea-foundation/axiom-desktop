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
services and are not provider E2EE. Each existing thread remembers its explicit
choice locally; warning acknowledgement is a separate per-account preference.
Every prompt passes an explicit boolean, and the native runtime rejects Web
execution without consent. Changing Web access requires the active response to stop.
On platforms without process network isolation, turning Web off does not disable
networking by approved local programs.

## Sending, queues and steering

Enter sends when idle. During a response, it adds input to a FIFO queue.
**Send now** steers the active turn at the next safe model/tool/compaction
boundary. It does not cancel an in-flight provider stream or repeat completed
tools; unstarted tool calls are explicitly skipped. Model and Web authority
remain those of the active turn. Steering remains bounded to 64 KiB; larger
messages and attachments wait for a new turn without dropping any content.

Prompt text may use up to **4 MiB of UTF-8**.
The model's context capacity and the configured native context budget still apply.
Queues hold at most 32 messages and 64 MiB of payloads, scoped to the account.
Large payloads commit to local IndexedDB before the draft is cleared; localStorage
holds only the outbox manifest. Stable
client IDs reconcile delivery with the native transcript. Stop pauses automatic
queue delivery. Restored or uncertain submissions require review; disconnect,
restart or account changes never justify silently resending them. A fresh prompt
after Stop can start normally when no older queued message needs review.

On the welcome screen, Send or Enter moves the same composer to the bottom with
a smooth animation immediately after the local outbox save. It stays visible
through thread creation and TEE verification; typing a follow-up during setup is
preserved when the thread opens. Reduced motion docks it immediately.

In an open thread, Enter immediately shows the message as a **Sending…** bubble
at the end of the transcript, before thread setup and provider verification
finish. The bubble is local feedback only: delivery is confirmed solely by the
native transcript echo, which replaces it in place. Messages that must wait for
the current reply, are paused, or need review stay in the queue panel above the
composer instead.

Opening the model picker refreshes its catalog. Provider tabs contain only
offerings currently returned by the service, so disabled providers disappear
without restarting Desktop.

## Images and files

Attachments are enabled only when the selected upstream model and its registered
E2EE adapter support sending the actual file bytes. Currently this means
PNG/JPEG/WebP/GIF images with **Tinfoil Kimi K3**, inside its attested, authenticated
encrypted request body. Current NEAR/Phala adapters do not support image uploads.
PDFs, text/code files, Office documents, and other files are unsupported: Desktop
does not extract file text or substitute it into a prompt.

Use Attach, paste an image, or drop images onto the composer. The picker accepts
only the supported image formats. Attach is disabled and greyed out when the model
does not support attachments, the catalog is unavailable, Axiom is disconnected,
the account is signed out, or the build lacks attachment support. Hover explains
why, including the provider and model name (for example, “NearAI’s GLM 5.3 Flash
doesn’t support attachments”); file drop/paste and picker events cannot bypass
this restriction.

A message can include 8 images, up to 5 MiB per image and 16 MiB of prompt text plus
encoded images in total. Desktop bounds image dimensions to 40 million pixels.
Unsupported or oversized inputs are rejected without clearing the text draft.
Switching to a text-only model blocks sending a draft that still contains images.
Bridge and native prompt validation also reject legacy extracted-text attachments,
including queued submissions and message revisions. Previously saved inputs remain
available for local inspection; no new extraction is performed.

Saved messages show attachment names; click to inspect the original local inputs.
Images stay in account-local history and survive edits and regeneration. Compaction
sends image inputs to the vision model and retains a textual successor summary.

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

Click the TEE badge for the last local verification report; Refresh requests
fresh evidence when allowed. Expanding **Technical details** widens the report
dialog so fingerprints, evidence fields and the workload manifest have room. Reports describe the provider-specific verified
claims. Tinfoil reports router verification, and NEAR v3 identifies an attested
worker TLS key rather than claiming the client observed the backend's socket.
Provisional streamed text becomes verified only after authenticated completion;
cancelled/incomplete output retains that status even if billing later settles.
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
Dialogs and their backdrops, model/thinking/account/context menus, and tooltips
animate on both opening and dismissal. Closing overlays remain mounted but inert
until the exit completes; focus is restored afterward. Rapidly reopening a menu
cancels its pending removal. Disclosures, queue panels, attachments and new
transcript rows also animate. The OS reduced-motion preference skips motion and
dismisses overlays immediately.

The Proxy screen starts the optional authenticated local integration explicitly.
See [Local proxy](proxy.md) for its separate credential and lifecycle. Updates
are downloaded and checked before user installation; see
[Install and update](installing-desktop.md#install-a-newer-version).

Source and regression evidence:
[renderer](../apps/desktop/src/renderer/src/App.tsx),
[ACP runtime](../apps/axiomcli/src/acp.rs),
[session storage](../apps/axiomcli/src/session.rs),
[headless UI tests](../apps/desktop/tests/browser/deletion-focus.test.ts),
[ordering](../apps/desktop/tests/thread-ordering.test.ts),
[context reports](../apps/desktop/tests/context-usage.test.ts), and
[deposit validation](../apps/axiomcli/src/billing.rs).
