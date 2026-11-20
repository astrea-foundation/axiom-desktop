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

Startup replaces the composer with an integrated preparation screen. A large
heading names the native operation: checking the saved sign-in, updating the
local account, unlocking the saved key, or **Decrypting your chats**. Decryption
is reported only after obtaining the key and immediately before opening an
existing encrypted database. Fresh stores and legacy migrations do not report
that decryption stage. The screen lists only stages actually observed. Its
progress bar reports the observed position in the fixed four-step pipeline
(start, sign-in or account update, key, decryption) as “Step N of 4”, with motion
only inside the running step. SQLCipher opens the database in one indivisible
step, so the bar never advances on a timer and invents no per-chat counts or
completion times. Reduced motion stops the sweep.

On macOS, saved sign-in and chat keys use a shared Keychain unlock, keeping
their secret values independent. The first upgrade may still request approval
for each older Keychain item while migrating it. Later starts read the combined
item once and reuse the native memory snapshot to open chats. Signing out keeps
the chat keys protected in Keychain and clears the unlocked snapshot. See
[encrypted local storage](local-storage.md#keys-and-account-boundaries).

Opening a saved thread shows “Loading thread…” with an indeterminate progress
bar until its native session and transcript are ready; it reports no percentage.
For listed chats, the catalog already supplies the saved workspace, so opening
skips the preliminary timeline peek. After `session/load`, the authoritative
timeline and MCP selection are read concurrently. A rejected stale catalog
workspace is re-read once before loading the actual saved location.
Saved threads restore their journaled model and thinking settings without
waiting for online model discovery. The initial session config reports only
those saved values; the independent model catalog supplies selectable
capabilities. Provider availability and supported settings are checked when
selecting a model or starting inference. An unavailable provider does not
prevent reading local history.

State changes share immutable views of untouched sessions and coalesce bursts
within one event-loop turn. Each window gets a complete initial state, followed
by structural patches through IPC, preload and the context bridge. Untouched
sessions and timeline rows keep their identity in the renderer. Account/runtime
changes replace the complete state, and a missing patch triggers snapshot
recovery before later patches are applied. Generated image identities retain
the same document-scoped validation and resource lifecycle.

The saved sign-in is checked with the server once per startup, before the local
account store is opened. When the store is ready, the native runtime publishes
that same result under a newer account revision instead of asking the server
again. It repeats the check only if authorization changed while the store was
opening (a different credential, account or session), the store was closed, or
more than 30 seconds passed, as during a long storage update. A result for any
account other than the one whose store was opened is reported as unavailable.

The textbox, model picker, Web, Agent and Send controls are absent during
preparation. Drafts remain in local state and return with the composer when the
account is ready. Readiness does not steal focus from Settings or the sidebar.
The account control mirrors progress and keeps Settings accessible. A genuine
preparation failure stays in the same screen with **Try again**, **Open Settings**
and expandable diagnostics. Older storage migrations retain their dedicated
progress screen and only show counts actually reported by the native runtime.

The **Runs on Astrea** sidebar introduction appears once per local app profile,
after the account is ready. Dismissing it, reloading a window, changing accounts,
updating Axiom or relaunching cannot show it again. Leaving it open also limits
it to its first session. A fresh profile or factory reset resets this preference.

Open **Settings → Chats → Factory reset Axiom…** to start fresh on this
computer. The control is also available while signed out or disconnected. Type
`RESET` and confirm to stop active work, delete local chats and managed files,
settings, cached app data, and saved Axiom/MCP credentials, then restart for a
fresh sign-in. This also removes local data shared with AxiomCLI. Online accounts,
balances, hosted API keys and files outside Axiom are kept. Close other CLI
windows first. See [recovery and deletion scope](troubleshooting.md#completely-reset-local-data).

The Account pane keeps the avatar, name, email and current status together in a
compact column. Linked sign-in methods have their own row. **Manage account**
opens the hosted account app; **Sign out** stays available as a separate action.
Settings tabs keep keyboard focus inside their selected shape so the scrolling
navigation does not clip it.

**Settings → Chats → Clear all chats…** permanently deletes every chat listed
for the signed-in account on this computer after an in-app confirmation that
shows the count. Active responses stop first. Folders, settings, sign-ins and
workspace files are kept. It requires a connected, signed-in account; deletion
runs in native-limited batches, and a failure reports how many chats were
deleted. Factory reset sits below it on the same page.

Conversations and folders are account-local. Folder names are labels, not file
paths. Hover over a thread or tab to it to reveal the three-dot button for
**Rename** and **Delete**. On touch-only devices the button is always visible.
Right-clicking a thread or folder also opens its menu. Rename and Delete use
in-app dialogs. Batch deletion is confirmed against the exact native selection.
Deleting a conversation does not remove workspace files.
Sidebar recency follows the latest **user submission** (`lastUserMessageAt`), so
concurrent assistant streams do not continually reorder the list. `lastMessageAt`
remains message activity metadata. Folder ordering derives from its member threads.

New threads immediately show a short label from the first prompt. One background
request to the selected model replaces that label with a generated title.
AstreaAI’s Qwen3.8 Uncensored starts this request after the first reply finishes
to give the conversation priority on the worker. Title generation uses the same
attested provider-E2EE path as chat and records its normal model charge as
title usage, separately from conversation context usage. Reasoning is
disabled when supported; otherwise the request uses the lowest supported effort
and allows room for reasoning. Title requests use only bounded first-prompt text,
without tools or attachments, and time out after 60 seconds. Failed or empty
results retain the local label. Manual renames always win, and existing named
threads are not regenerated. Titles update in the sidebar even after the answer
finishes. Drafts are scoped to account/thread, with a separate new-chat
draft; navigation, reselecting a thread and reload preserve them. An anonymous
draft can follow the first sign-in. Pending first-message setup has its own
identity, so navigating away does not silently discard or redirect it.

Saved generated images load as they approach the viewport. Loading failures show
**Retry loading image** without blocking messages or the rest of the thread.
Image reads and cache memory are bounded; reopening a conversation no longer
replays all saved image bytes before loading its timeline.

## Balance and payments

Open the account menu and choose **Billing** to view weekly and purchased credits,
manage a subscription, or add credits with Card or Crypto. Capabilities depend on
the selected account/server. See [credits and subscriptions](billing.md) for plan,
auto top-up and reset behavior. Direct Zcash remains available within Crypto;
its reusable address and confirmation-time valuation are retained.

When there is too little available credit to start a request, chat and image
errors say **Insufficient balance. Top up your Axiom balance to continue.**
Open **Billing** to add credit or redeem a gift code. Existing request holds can
reduce available credit even when the displayed account balance is positive.
The CLI gives `/topup`, `/redeem` and `/balance` guidance for the same failure;
the local proxy returns HTTP 402 with `insufficient_credit` and the top-up message.

When there is too little available credit to start a request, chat and image
errors say **Insufficient balance. Top up your Axiom balance to continue.**
Open **Balance** to add credit or redeem a gift code. Existing request holds can
reduce available credit even when the displayed account balance is positive.
The CLI gives `/topup`, `/redeem` and `/balance` guidance for the same failure;
the local proxy returns HTTP 402 with `insufficient_credit` and the top-up message.

Balance refreshes reuse a ready address instead of provisioning it again. A
temporary deposit-service failure leaves validated credit visible while an
unfinished address remains pending. If the balance read itself fails, the screen
shows **Balance unavailable** and the error instead of loading indefinitely or
assuming the credit is zero. **Refresh balance** retries immediately. Automatic
monitoring backs off after failures, up to one refresh per minute, and stops when
you leave the screen or disconnect.

Choose **Other crypto** to open the deposit window. Pick a coin from the dropdown
with its icon, enter the USD credit amount and create a payment. Send the exact
crypto amount on the displayed network, including the memo if shown, before the
countdown ends. The countdown uses the
fixed-rate quote's expiry and updates every second. **Send within … for this rate**
is the time to send, not a confirmation deadline. An unsent quote renews automatically
while the deposit window is open, with a fresh amount and address. Old quotes
remain tracked separately under the same invoice. Failed renewal offers **Retry quote** without
duplicating the order; it never loops on a failed or stale quote.

Choose **I’ve sent it** after sending to stop renewal while the transfer is detected
and confirmed. This hint is saved on this device for the signed-in account; it
does not authorize credit. Confirming payments continue beyond the rate deadline.
The invoice has its own **Invoice expires in** countdown and lasts six hours;
refreshing a rate never extends that deadline. You can have five active invoices.
**Cancel invoice** releases a slot on the server and hides its payment instructions.
It does not reverse funds already sent; the invoice remains in recent payments
and late transfers continue to be processed. **New payment** returns to the coin
selector. **Check payment status** requests reconciliation even after expiry or
cancellation.
Each payment has its own address. ETH, USDC, USDT and assets offered on multiple
networks show the exact network and a permanent-loss warning before payment
creation and beside the deposit details. Status updates automatically; recent
payments remain available when you reopen Balance. Partial payments need review.
Provider fees are covered by Axiom.
The discount applies to Zcash only. Crypto availability requires backend setup.

Automatic and manual balance refreshes preserve an entered gift code.
**Redeem a gift code** adds its USD value to the signed-in account. Code
entry is visible and cleared on success, closing the screen, disconnect or account
change. It is never a chat message or saved draft. A failed/uncertain request can
be retried with the same code without duplicate credit; a code already redeemed
by this account reports that result and refreshes the balance.

Gift credit is part of **Other credit**. Trial credit is normally spent first.
A shared $15 rolling-24-hour trial admission threshold can pause new trial-funded
requests with a retry time. Already running responses finish normally. Paid credit
remains usable and is spent first while trial capacity is full. Gift
redemption does not clear payment-review holds. It requires a native interactive
account and a backend with gift redemption deployed. Desktop and its bundled
sidecar negotiate `billing@4`; older sidecars must be updated together.

## Agent and Web controls

New threads start with Agent **Off**, a separate managed working directory and
Web **off**. Agent settings offer three access levels: **Off** disables local
commands and file changes, **Ask for permission** asks before each local tool
action, and **Full access** runs local tools without routine approval prompts.
Select an existing absolute working directory using the native picker or reset
to the managed thread directory.

Changes to the access level, working directory and MCP selection save when the
dialog closes using its close button, Escape, or a click on the backdrop. There
are no Save or Cancel buttons. The dialog waits for save acknowledgement; a
failed save keeps the choices and offers **Try again** or **Discard changes**.
Closing unchanged settings makes no write. Account or runtime changes dismiss
the old dialog without saving its draft into the replacement context.

Agent settings persist atomically. Saving requires an idle thread; the renderer
also requires an empty queue. Prompts and steering carry the saved settings
revision, so queued text cannot silently inherit a different permission/directory
configuration. A change clears cached policy grants and file-change tracking.
Desktop supports explicitly configured [MCP connections](mcp.md). The
Agent dialog selects MCP tools per conversation independently of shell/filesystem
permissions; MCP can be used while Agent is off. Settings provides connection
management, testing, tool inspection and configuration import.

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

Enter sends when idle. A dispatched message appears in the conversation with
**Sending…** until the native transcript acknowledges it, including while
attestation checks finish. It is not displayed as a queued follow-up. Stop is
available during this wait, and another message waits in the FIFO queue until
the active request finishes. This applies to existing chats, first messages,
retries and queued messages when their turn arrives.
During a response, Enter adds input to the same FIFO queue.
The model picker is disabled while a message is being sent or a response is
running, including tool calls. An open picker closes when generation starts.
Wait for the response to finish or stop it before changing models. Other idle
threads and the new-chat screen keep their own model selection available.
**Send now** stops the current reply, waits for its cleanup, then sends the
selected message as a new turn. Its text, attachments, Web choice and Agent
settings are preserved. Other queued messages retain their order. The cancel
button removes a queued message, including while Send now is stopping a reply.
Cancelling that pending send leaves the remaining queue paused. Once a message
has been sent, use Stop on its reply.

**Stop** immediately changes to **Stopping…**, including while the model is
writing a tool call. The transcript shows the same status while cancellation
and cleanup finish in the background. You can keep editing your draft or switch
chats; returning to the stopping chat retains its status. Older queued messages
stay paused, and model selection remains locked until cleanup finishes. Repeated
Stop requests do not send duplicate cancellations. If cancellation fails, Stop
becomes available for retry and an error explains that sending remains paused.

**Retry** and **Continue** resume the preserved model history through a native
continuation control. Its instruction is supplied to the model behind the scenes;
it creates no user-message bubble, queued message, or new attachment submission.
Reloading the conversation keeps that instruction out of the transcript. Earlier
messages, completed tools, partial replies and the current draft remain intact.
The controls retain the current Web choice and Agent/MCP settings and use the
normal security, cancellation and account boundaries.

Prompt text may use up to **4 MiB of UTF-8**; direct file bounds are listed below.
The model's context capacity and the configured native context budget still apply.
New queue reservations allow at most 32 messages and 64 MiB of payloads, scoped to
the account. Importing legacy saved preparations can restore more entries so
existing drafts survive migration; new reservations retain the normal bounds.
Payloads and outbox metadata commit to encrypted native storage before the draft
is cleared. Images/documents are encrypted external files, referenced by the
database. Existing localStorage/IndexedDB payloads import on account unlock,
with acknowledgement before removal. Recovered conflicts appear as saved
preparations requiring explicit review. Stable
client IDs reconcile delivery with the native transcript. Stop pauses automatic
queue delivery. Restored or uncertain submissions require review; disconnect,
restart or account changes never justify silently resending them. A fresh prompt
after Stop can start normally when no older queued message needs review.
Normal window close, quit and explicit account changes wait for pending native
draft/outbox writes. A failed save keeps Axiom open for retry. Text edits are
debounced; an abrupt process kill or raw page reload before acknowledgement can
lose an uncommitted edit.

A new window whose application never mounted has no in-memory draft writer and
can close immediately. Once the app registers its writer, it must acknowledge
saves even after a renderer failure or reload; losing the renderer cannot bypass
that protection.

## Images and files

Image creation and editing are automatically available to tool-capable chat
models when the native image service is installed and passes its verified
capability preflight. Generation does not require the chat model to support
vision. There is no Images toggle, and no MCP connection, Agent mode or Web
setting is needed.
Ask the model to create an image or edit an uploaded or earlier generated image;
an upload or request to describe it does not by itself request generation.
The model uses `generate` for both operations, with up to five optional reference
images. It refers to them as `<image1>`, `<image2>`, etc. in the order supplied
to the tool. The image service uses ComfyUI inside the encrypted worker.
With AstreaAI installed, NEAR and Tinfoil chats use that same
attested image service while keeping their selected chat model. Image receipts
and billing identify the AstreaAI worker; saved image provenance retains the
originating chat model for regeneration.
Images from chat models with the catalog's **Uncensored** tag (such as the
abliterated DeepSeek V4.1 Flash and Qwen3.8 Uncensored) use the pinned uncensored
LoRA at strength 0.9; other chat models use the base image model. Vision-capable
chat models automatically receive a bounded preview of each generated or edited
result, so they can inspect it before finishing without a separate tool call.
They can use `view_image` for an image absent from their current visual context,
including after compaction. Its row names the inspected image ID; expand it to
see that image. Expand an image request to see
its prompt, aspect ratio, output size and any reference images as cards labelled
`<image1>`, `<image2>`, etc., above the result. While the model is writing a
tool call, a "Writing tool call…" row appears until the call itself is shown. Click a generated or edited
image to view it in the larger viewer; Escape, the close button or a backdrop
click closes it. Click the image to zoom in, scroll to move around the larger
image, and click again to fit it in the viewer. Enter or Space on the image
performs the same zoom toggle. Right-click an image in the chat or viewer to choose
Copy image, which puts the image itself on the system clipboard, or Save image as…
to select a destination. Generated images are attributed to AstreaAI beneath the result.
Save image also remains available beneath the result;
each save suggests a fresh random PNG filename. Image generation and editing show a
sampling percentage and progress bar. Queued work displays 0%. Once the worker
announces the PNG, the row changes to “Receiving image…” with an indeterminate
bar while transfer, authenticated completion and local storage finish. This
phase has no percentage or remaining-time estimate; the bar disappears when the
verified result is ready.
Chat models without vision can create images and edit earlier generated images
by their conversation-local IDs, but cannot inspect results or accept new image
uploads. A failed image-service preflight withholds image tools and supplies
the native diagnostic to the chat model. The model is instructed to briefly say
"Image generation is currently unavailable. Please try again later." It must
not claim that the selected chat model cannot generate images or recommend
switching models to resolve an image-service failure.

Opening an image-heavy conversation loads its messages and image dimensions
first. Original generated images load as they approach the visible part of the
conversation. Reserved image space keeps scrolling stable, and recently viewed
images are cached within a bounded memory budget. A failed read offers
**Retry loading image**. Viewing and saving use the original image.

Each generated image has its own Regenerate button. It reruns only that image's
stored prompt, reference images in their original order, output dimensions,
and original model's image settings when saved provenance is available,
with a fresh seed. It adds a new image while keeping the original and the chat
response. It performs no chat-model or title-generation request. Progress and
Stop apply to this image job; regeneration never automatically retries after a
failure or account/runtime change.

The default policy lets the model choose `aspect_ratio` from `1:1`, `2:3`, `3:2`, `3:4`, `4:3`, `9:16`,
`16:9`, `1:2`, `2:1`, `9:21`, and `21:9`. The default for new images is **1:1**.
Native code selects the largest exact ratio on the supported 32-pixel grid,
within 2,359,296 pixels. This gives 1536×1536 for 1:1, 1216×1824 for 2:3, and
2048×1152 for 16:9. Without a ratio, reference-based requests use the same
default size.

The installed AstreaAI client supplies the current default, ratio list and
reference limit. The native tool definition is updated for each turn; generic
model instructions defer to it.

Generation references use their own 2048² pixel budget. The tool passes stored
images directly to the provider instead of using `view_image` thumbnails. The
provider normalizes them once onto the 32-pixel grid using Lanczos3 in linear
light with premultiplied alpha, writes lossless PNG, and passes their dimensions
through Comfy without another forced 1024 resize. Browser attachment preparation
uses high quality canvas resampling and lossless PNG on that grid. `view_image`
returns a separate cached lossy WebP bounded to one megapixel and 512 KiB, with
a quality and resolution floor. NEAR and Tinfoil also prepare these smaller
copies for uploaded images before encryption. NEAR reserves actual text and tool
overhead, sizes current uploads within its encrypted request budget, and compacts
older image history while keeping originals by ID. The model is instructed to
inspect pixels already inline; redundant `view_image` calls acknowledge the
existing image without adding another copy. See [configuration](configuration.md#image-generation-and-editing).
Display, Save and generation references retain their detailed local assets.
2K RGBA outputs may be up to 20 MiB. The Turbo sampler and LoRA policy are unchanged.

Copy and Regenerate appear after the full response turn finishes, including any
tool calls; an introductory message or completed tool-call segment does not
end the turn. The **Thinking** panel opens when reasoning starts streaming and
collapses when the model starts its answer, starts a tool call, or finishes the
response. **Thought for N seconds** shows the recorded span of the reasoning
stream. Older messages without timing show **Thought**. Click the label to
reopen it. Expanded tool details and thinking text support selection and copying
with the usual keyboard shortcuts or the right-click Copy menu.

Use Attach, paste an image, or drop files anywhere in the main chat panel,
including its empty space and transcript. A drop indicator marks the panel;
the sidebar and Proxy view do not attach files. Image and file icons
in the model picker show the selected model's current capabilities; hover or use
the accessible label for their meaning. The picker lists each model once and
can filter by catalog tag and upload support. A new thread starts on the best
provider for the chosen model; see [catalog discovery](provider-catalogs.md#model-tags-and-picker-filters)
for that rule, tags, refreshes and unavailable models.

Image uploads accept PNG/JPEG/WebP/GIF. File-capable Tinfoil models accept
text/code, Markdown, CSV, HTML, JSON, XML, PDF and Office Open XML files
(DOCX/PPTX/XLSX). NEAR supports images where advertised, but currently advertises
no general document uploads. Axiom reads original document bytes without extraction,
OCR, conversion or rasterization. Document processing belongs to the attested
provider. Unsupported files remain visible in the draft and block sending until
you remove them or select a model that supports them.

A message can contain ten attachments, up to 20 MiB per image, 10 MiB per file,
and 144 MiB of text plus base64-encoded attachment payloads. Text remains bounded
to 4 MiB. Model context and native budgets still apply; inputs are never silently
truncated. Audio/video and remote URLs are unsupported.

Original document bytes, image vision copies and filenames reach the chat provider
inside its authenticated E2EE request. Attachments remain in account-local history, survive edit/regeneration,
and appear as wrapping cards in the draft, queue, and transcript. Image cards
show local previews. Click an image card to view the full image in a larger
overlay; close it with Escape, the close button, or a click outside the viewer.
Sending retains these cards through thread setup and native
acknowledgement, without waiting for the provider or reloading the original
files. Saved image previews load from local history as they come into view.
Compaction sends image/file parts through the same provider preparation to a
capable model and retains a textual successor summary; it never
inserts encoded file bytes as message text. Older extracted-text attachments are
readable in saved history but cannot be submitted as new uploads.

## Edit, regenerate and compaction

Sent user messages show **Edit**, **Regenerate** and **Copy** beneath the bubble.
Copy puts the original message text on the clipboard, including Markdown and line
breaks. It is disabled for attachment-only messages. Regenerate resends
that message with its original text and attachments, including attachment-only
messages, through the same replacement confirmation as editing and sending.

Edit and Regenerate use the negotiated `messageRevision@1` flow. The client
submits the original user item ID and expected thread revision. AxiomCLI
atomically truncates that turn and later local history, preserves request
accounting, constructs the replacement messages, and starts ordinary attested
E2EE inference. Stale revisions, cross-account targets and active turns are
rejected. Regeneration after steering targets the turn's original prompt.
Already-performed file edits and other tool side effects are not rolled back.

Compaction uses encrypted, tool-free model requests to build successor context.
Automatic compaction follows context tokens at the selected model's advertised
threshold capped by local token limits. NEAR also compacts prepared history
before its encrypted request would exceed the 10 MiB operating budget. Uploaded images, automatic
generated previews and successful `view_image` observations contribute visual
tokens for vision-capable models. Provider-reported usage takes precedence over
estimates for existing context. Only new material is estimated until the next
provider report.
Image counts alone do not trigger compaction. The runtime
uses one summary request whenever the conversation fits with room for summary
output, reasoning and a safety margin. It passes historical messages and native
image/file inputs together, preserving their order and association. Historical
system notes cannot override the summarizer's instructions; unsupported reasoning
fields and refusal details remain available as historical text.
When token or provider request limits require multiple requests, consecutive
fragments are grouped into as few passes as the budgets allow. Each pass reserves
space for the previous summary and new output. A usable recent tail is retained
verbatim, and summaries persist for resume. Cancellation never marks unfinished
compaction successful. Compaction is model-generated and is not a guarantee of
perfect recall. The original local journal remains available independently of
the compacted model context.

If automatic pre-turn summarization fails, the turn reports an error and retains
the original conversation and media for retrying compaction or choosing a model
with a larger context. Cancellation stops the turn. Original saved messages remain
separate from the compacted model context.
On restore, missing or invalid old attachments and image details are omitted
instead of blocking the entire conversation. Interrupted tool calls receive an
unknown-outcome marker; recovery does not execute them. Older file formats that
the newly selected model cannot read are omitted from context while their text
remains available. New uploads still require a compatible model.

The image tools keep conversation-owned references independently of the bounded
vision-copy cache. Compaction and cache eviction preserve original assets and
their IDs for later viewing or editing. A completed or cancelled response refreshes
thread state after any older timeline fetch, so a stale running snapshot cannot
leave the composer waiting indefinitely.
Late generated titles update the sidebar without reopening an unloaded thread.

## Usage and verification

The composer meter displays provider-reported input/output usage for the **last
completed conversation request**, with that request's model capacity and effective
compaction threshold. The 85% threshold controls when history is compacted.
New uploads, automatic generated previews and successful `view_image` observations
count toward the next request's context; the meter updates when the provider
reports that request.
Generated previews enter vision-capable models' context automatically. Encoded
image/file data has per-message native size limits. These transport limits are
enforced alongside compaction. Ten attachments is a per-message upload limit: later
messages can add ten more, and image views and generation have no conversation
count limit. Compaction preserves saved assets and their reference IDs for later
use.
Tinfoil uses upstream's remaining-context generation default. Other protocols
use the advertised output ceiling and estimated remaining request space (including
tools/images and a margin). Exact tokenization and final context enforcement belong
to the provider.
A verified `length` finish reason is shown as an output-limit notice, with the
option to ask for continuation. The usage meter is not a draft estimate or cumulative charge. Reports
survive reload, and model switches do not rescale old counts against a new model.
The ring stays empty until a usage percentage is available; the usage panel
describes missing reports as unavailable.

Hover, focus or click the meter to see context use and the compaction threshold.
Request records are selected by their start timestamps, independent of the random
request IDs used for accounting pagination. During compaction, a spinner shows
cumulative generated tokens and the current pass when multiple passes are needed.
A `~` marks counts estimated from
streamed summary text and available reasoning; provider output-token reports
replace those estimates when available. Counts describe work completed, not a
percentage or a promise of the final summary length.
Expand **Usage details** for input/output counts, turn totals, cache and reasoning
subsets, settlement state, elapsed time and other thread charges. Tab reaches the
disclosure; Escape closes the panel and returns focus to the meter. Partial or
unavailable usage and reports from a different model remain visible above it.

The privacy badge describes the selected server environment and encrypted
connection. It shows **TEE verified**, **Verifying TEE**, or the actual report
state. Reply history, thought segments, image tools, rate limits and cancellations
do not determine this badge. Messages, image captions, the local proxy and the
privacy inspector do not display per-reply verification verdicts. Request errors
remain visible as ordinary errors.

Click the badge to inspect **Environment attestation**. The question-mark button
beside **TEE verified** explains TEE protection and the server hardware check in
plain language. Hover, focus or click it to show the help; Escape dismisses the
help before closing the report. The inspector contains hardware, encryption-key,
freshness and source-provenance checks for the selected provider. Tinfoil reports
router verification, and NEAR v3 identifies an attested worker TLS key and, on
the gateway route, the attested NEAR gateway's key, TLS key and workload.

Reports renew automatically for the selected model while the window is active,
visible and online. Renewal starts up to thirty seconds before expiry, capped
at half the report lifetime for short leases, with a minimum one-second margin.
The selected model warms when the composer opens or the selection changes,
including on the welcome screen before a thread exists. The native cache
shares checks for the same model and renews NEAR and Tinfoil early. Failures
retry with backoff from thirty seconds to five minutes. An outdated TEE still
requires explicit user consent. Automatic
refresh never accepts that warning. Manual **Refresh** remains available.
Only the model ID crosses the verification IPC boundary. Send proceeds through
native inference using still-valid evidence or shares an in-progress check on a
cold path. A valid badge remains visible during renewal until its hard expiry.
Explicit **Refresh** requests new attestation. Cached warmup reports can populate
newly loaded idle threads, with account, runtime and model fences on late results.
The installed AstreaAI image service warms independently. Native image dispatch
requires fresh evidence and authorization for the actual worker.

After thirty minutes without user interaction or a running reply, the badge
shows neutral **Idle** and automatic renewal pauses. Activity or returning to the
window resumes renewal. Hidden and offline windows also pause automatic work.
A failed or blocked refresh never extends a report or marks it verified.

Unexpected stream endings show one short notice, with the technical reason under
**Details**. A turn's error notice replaces the duplicate warning below its reply.
Stopping a reply yourself does not show an error banner.
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
are supported.

Settings → Usage shows posted account-ledger inference charges, including other
clients/devices. **This week**, **This month** and **All time** tabs update the total,
pie chart and model breakdown together; All time is the initial selection. Weeks
start Monday and months start on the first, using the device's IANA timezone and
calendar boundaries (including DST). Charges count when posted to the ledger.
Top-ups, pending holds and unbilled estimates are excluded. Empty periods show
zero spending; failed loads show an error with retry. Arrow keys, Home and End
switch the period tabs. Account/period changes discard stale responses, and a
backend or sidecar that returns a different period fails without showing mislabeled
totals. Inconsistent model attribution remains an explicit error.

Settings → API keys lists, creates and revokes named automation keys and shows
usage attribution. Creation can require recent sign-in; the newly created key
is deliberately displayed once for copying. This separate account-management
flow does not expose the native access/refresh token or accept an API key as a
Desktop login credential.

## Presentation and local integrations

A compact horizontal black **RUNS ON Astrea** banner sits above the account area
in the left sidebar, with small, muted JetBrains Mono Medium text and a branded
Astrea lockup on one line. It eases into view and stays visible until closed.
The field bumper loops continuously with the camera orbiting the 3D field;
the text and logo stay steady. The Astrea logo and wordmark open `astrea.systems`
in the system browser. The × hides it until Axiom quits and launches again,
including across renderer reloads and closing/reopening the window. Reduced
motion disables the video and animated transitions.

Tool activity appears at its execution point between assistant segments, with
stable IDs for live/snapshot reconciliation. Markdown, code and KaTeX render
locally with bundled fonts. Code blocks use bundled syntax grammars and light/dark
token colours; language-labelled fences and common unlabelled code are highlighted.
Single-dollar inline math requires non-whitespace immediately inside both
delimiters, and the closing dollar cannot be followed by a digit. This keeps
ordinary prices such as `$5 and $10` and `$8–10 billion` literal while preserving
`$x^2$`, `$$` math and math fences. Escaped dollars and code stay literal.
Inline code, explicit text/log/output fences and unknown languages remain literal.
Highlighting preserves all whitespace and never executes code or loads remote assets.
To keep streamed replies responsive, blocks over 50,000 characters (10,000 for
automatic language detection), or beyond a 100,000-character per-message highlighting
budget, remain plain text without truncation. Scrolling up or selecting transcript content pauses
automatic following; Scroll to latest resumes it. Theme is light, dark or system.
Enabled Web and Agent controls, Agent selection indicators, and the composer's
Send/Stop button use Axiom signal orange (`#FF7653`) in light mode. Dark mode uses
neutral white/gray selections and buttons. Filled controls use dark labels and
icons, and disabled controls retain their muted state.

The Proxy screen starts the optional authenticated local integration explicitly.
See [Local proxy](proxy.md) for its separate credential and lifecycle. Updates
use the signed release feed shared with AxiomCLI. **Update and restart** downloads,
verifies, waits for active chats/proxy work, commits drafts and applies the owning
installer before reopening. Cancel is available before installation starts. Other
installed CLI/proxy processes must close before replacement; they are never killed.
Desktop requires its CLI to report the exact same
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

## Outdated provider security updates

A provider with Intel TDX `OutOfDate` evidence shows a yellow warning and
**Continue generation**. Accepting remembers the choice for that provider until
Axiom restarts, rechecks its evidence, and retries the blocked message through
the normal local edit/regenerate flow, preserving attachments. Switching model,
thread, account or runtime while the check runs prevents an unintended retry.
A yellow **TEE updates needed** badge remains after acceptance; messages are
still encrypted and replies authenticated, but the missing security updates
may leave the environment vulnerable. Other attestation failures cannot be
accepted. The backend must also support the explicit consent parameter; an
older backend continues blocking the request.
