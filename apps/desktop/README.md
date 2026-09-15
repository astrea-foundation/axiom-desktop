# Axiom desktop

Installed releases check `https://axiom.stream/api/releases/latest` at startup
and every six hours, without account credentials. A compact title-bar notice
opens **Settings → Updates**, which also supports manual checks. Downloading an
update selects the running binary's OS and architecture and its recorded package
format. An older installation without a format marker asks the user to choose
among matching formats; it never guesses a different architecture or switches an
identified package type. Development builds do not check the public feed.

Downloads use a native save dialog, show progress, and verify byte size and
SHA-256 before publishing the file. Interrupted or invalid transfers remove their
temporary files and preserve existing downloads. **Show in folder** reveals the
verified installer. Installation remains manual, so a download does not interrupt
a chat. The check and download state lives in the main process and survives
closing Settings. Package releases through `scripts/package-desktop.mjs` so each
installer embeds its own `axiomUpdateFormat` value.

Electron main supervises `axiomcli acp --frontend desktop-chat`, negotiates the
versioned Axiom extension, and exposes a narrow typed API through the sandboxed
preload. The renderer uses the real session catalog and ACP event stream for
messages, reasoning, tool calls, plans, questions, cancellation, and settings;
it does not synthesize a conversation or choose a filesystem workspace.

AxiomCLI supplies the active account's canonical desktop chat directory and
enforces the Web permission profile. Threads, timeline items, profile
preferences, sidebar collections, and assignments all live in that account's
Desktop-profile state database. Electron has no SQLite connection and owns
presentation state plus an account-scoped, on-device pending-message outbox.
Folder names never become filesystem paths. Signing out
closes the store; signing into another account selects a different local tree.
Threads are ordered newest-message-first, and folders by the newest message in
any member thread. Streamed assistant text/reasoning counts as message activity;
opening, renaming, settings, attestation, and other operational updates do not.
Message-free threads/folders come last, with stable ties. Folders remain above
unfiled threads, and filtering the sidebar does not change folder ranking.
Local schema v2 backfills message activity for existing conversations without
resetting them; message timestamps also travel with live ACP revision metadata.
The rotating refresh token is shared through the operating-system credential
store, while CLI and Desktop conversation databases remain separate.
Account credit and Axiom Payments deposits use the negotiated `billing@1`
extension. When the backend enables persistent deposits, sign-in/billing refresh
provisions an account-bound mainnet ZEC address. Account menu → Balance shows the
reusable address beside a QR generated entirely on-device, one copy action, and
recent deposits. Credit and deposit details expand on demand. The default view
fits a 751 × 791 window without scrolling; history and smaller windows can scroll.
Users can send arbitrary repeated amounts without invoices or memos.
No browser checkout is opened. Balance polls every five seconds, including
while the address is provisioning; the backend owns durable idempotency keys.

The native billing request opts in to `include_zec_usd_quote=true`. The backend
returns a cached Coinbase-first ZEC/USD quote with a 60-second market-timestamp
expiry, falling back to Kraken; the renderer hides expired or offline quotes.
Older backends can omit the quote. The displayed rate is indicative, and the
confirmation-time rate determines credit.

**Mainnet ZEC only:** funds remain ZEC. When valuation is enabled, confirmed
deposits add USD-denominated inference credit without currency conversion. Testnet service
responses are rejected instead of displaying an address under mainnet labels. Reorgs
can revoke and reinstate confirmation without duplicate credit. Zatoshis and
object versions remain exact decimal strings/BigInt, never JavaScript numbers.

The Rust account-operation lease and ACP account context reject responses from a
previous login. Signing out clears addresses, receipts and balance together.
Payment-service credentials and webhook keys stay exclusively in the backend;
the native client holds only its normal account session. No OS-specific wallet
or payment daemon is introduced into the Windows/macOS/Linux desktop.

The receiver on `127.0.0.1:8080` is **not** the application's API. Point the
desktop at the application API's trusted HTTPS origin (and matching auth
origin), with the backend configured to reach that receiver. See the backend
repository's `docs/axiom-payments-integration.md` for migration and webhook setup.
The legacy Shieldz invoice UI remains compatibility-only for a backend explicitly
configured with `AXIOM_BILLING_PROVIDER=shieldz`; it is hidden for Axiom Payments.

Brand treatment (light and dark token themes, DM Sans / DM Mono, the PP Cirka
display face, the falcon symbol) lives in this package; the closed-source
website is not a runtime or build dependency. `REDESIGN.md` explains the
design system, its conventions, and the packaging pipeline.

## Privacy proof

Click the composer's TEE badge to inspect the last local verification report;
opening it does not perform a network check. The separate refresh button obtains
fresh attestation and is disabled while that thread has active work. The dialog
uses a short summary with expandable hardware, key-binding, freshness and
provider-specific results. Technical details include policy, public fingerprints,
and the bounded workload report; Copy verification report exports only those
public fields, not messages or credentials. This JSON is a verification report,
not a standalone raw attestation bundle.

Reports remain in memory, scoped to the account, native runtime, thread and model.
Expired or failed reports cannot produce a green badge. The latest reply's receipt
status comes from its durable terminal verification markers, independently of TEE
preflight. Web-enabled threads explicitly disclose that search queries are not
end-to-end encrypted.

## Local proxy and message actions

The Proxy screen controls an optional `axiomcli desktop-proxy` process using
the existing `axiom-proxy` server library. Start launches the listener on
numeric IPv4 loopback and displays its URL only after validated readiness.
Stop closes it; Restart applies a new port and rotates the local client token.
Copy token writes the local token directly from Electron main to the clipboard.
The native process uses the OS account session and refreshes upstream access
tokens without exposing them to Electron. Sign-out, account/sidecar changes and
application shutdown stop the proxy. Usage, errors and attestation in this view
are native operational metadata; a successful response receipt is shown only
after a completed request. Desktop chat continues to use ACP directly.

Edit and Regenerate are temporarily disabled because the current ACP extension
does not expose a durable message edit/truncation operation. They must be
implemented through a native provider-E2EE edit flow before being enabled.
Unsent account drafts are cleared on sign-out and account changes; a draft
written while signed out can follow the user's first sign-in.

## Queuing and steering

Enter sends immediately when idle. During a reply, Enter adds the message to a
visible FIFO queue above the composer; queued messages send one at a time after
the active turn ends. **Send now** promotes that message to steering within the
current turn. It waits for the active model request (including its verified E2EE
receipt), tool call, or compaction to finish, skips tools that have not started,
then includes the instruction in the next encrypted model request. It does not
interrupt a provider stream or restart completed tools. Model/settings and Web
permission remain those of the active turn; steering cannot grant Web access.
The TUI also supports Enter-to-steer while a turn is running.

The queue captures each message's Web choice and stores pending text only in
account-scoped desktop local storage. No queue text goes to an application
backend; the native client still constructs and encrypts all inference messages.
Messages leave the queue only after acknowledgement in the native local
transcript. Delivery uses stable client IDs, and the native turn-scoped steering
inbox deduplicates retries. Applied steering appears chronologically in the
transcript and survives reopening, with its continuation in a new output segment.

**Stop** pauses automatic queue delivery. Errors, disconnects, account switches,
and restarts preserve unsent messages but restore them **paused**: review the
thread, then choose **Send now** or remove the pending message. Never silently
resubmit input whose delivery is uncertain. The queue is bounded to 32 messages,
64 KiB each. Shared native/IPC logic and the renderer work across Linux, Windows,
and macOS; this feature requires no OS keyboard hooks or provider cancellation API.

## Agent mode

The composer’s **Agent** button opens a centered, theme-aware modal for per-thread
settings: enable Agent, choose
**Approve commands** (ask before each tool action) or **Full access**, and select
an existing working directory with a native folder picker or an absolute path.
New threads start with Agent **off** and a separate workspace at
`<Axiom data>/accounts/<account>/desktop/chat/<thread-id>`. **Use default thread
folder** resets a custom path. Deleting a thread does not delete its workspace files.

The renderer-owned dialog uses an enable switch, permission cards, and a Browse
button. Changes apply only with **Save changes**; Cancel, Escape, or clicking the
backdrop discard edits and return focus to Agent. The background is inert while
the modal is open, and failed saves keep the settings available for correction.

Settings persist in account-local SQLite (schema v4, preserving existing threads).
They can change only while the thread is idle and its message queue is empty.
Queued prompts and steering capture a native settings revision; stale messages
are rejected instead of inheriting broader access. Switching permissions or
directories clears cached policy grants and file-change tracking for that thread.

Full access skips routine approvals but retains the native engine’s safety rules.
The working directory is **not an OS sandbox**: approved programs use the user’s
OS permissions and may access files/network beyond that folder. Web search/fetch
tools still require their separate Web toggle. Tool execution, message building,
and attested provider E2EE stay in the native client. Directory selection uses
Electron’s native dialog on Linux, Windows, and macOS; no platform-specific shell
or input automation is needed.

## Context usage

The composer's circular meter replaces the shortcut hint. Hover or keyboard-focus
it to see **Usage**: the provider's input/output counts for its last
completed conversation request, their total, and the percentage of that request's
model capacity. The model and effective compaction threshold are saved with the
report; switching models never rescales an old count against a new capacity.
Escape dismisses the tooltip. No report yet is a dash, not an invented zero.

Reports replace one another after each conversation-model request (including
tool rounds), and persist in the account-local SQLite database across app/thread
reloads. They are neither cumulative billing totals nor measurements of unsent
drafts or the next request. Edits and compaction may change the current context;
they never overwrite this display with local estimates. Auxiliary title and
compaction requests also never overwrite a conversation report.

Schema v3 preserves existing conversations; older threads acquire a report on
their next completed model request. Their legacy turn totals lack model/request
provenance and are not backfilled as trustworthy context reports. Internal
compaction estimates remain separate and unchanged. No prompt text leaves the
desktop for counting, and this display does not change billing or Enter-to-send.

## Inline tool activity

The native system prompt includes the thread's start date and time in UTC. This
comes from its saved creation event and stays unchanged across messages,
compaction, and app restarts, so clock ticks do not invalidate prompt caching.
It is explicitly a conversation-start reference, not a continuously updated clock.

Streaming follows the latest text through hover and content/viewport resizing.
Scrolling upward or clicking/selecting transcript content pauses following;
scrolling back to the bottom or choosing **Scroll to latest** resumes it. Layout
and programmatic scroll events never count as an intentional opt-out.

The composer's **Web** toggle starts off in each new chat. Existing chats remember
their enabled or disabled choice across app and sidecar restarts, stored locally
per account and thread. Toggling saves immediately, even without sending a message;
the welcome draft's choice is saved only when it becomes a thread. Missing or
invalid saved choices default off. No previous search history is used to infer consent.
Enabling Web opens a vendor-neutral privacy warning with **No thanks**, **Yes, I
understand**, and **Don’t show this warning again**. The warning acknowledgement is
a separate account-local preference; it never enables Web for another chat.
Restoring a saved enabled chat does not ask again. Turning Web off needs no confirmation.
The choice is fixed
while a response runs; stop that response before changing its access.

Each prompt carries an explicit `_meta.axiom.webEnabled` boolean. Missing consent
is false in desktop mode: the native engine both withholds Web tool definitions
and rejects attempted search/page-fetch calls. This is a separate restriction
inside every desktop permission profile, not a grant of file/process
access; local tools require explicit Agent enablement. Desktop requires `webConsent@1` and refuses older sidecars that could
silently ignore the opt-out. CLI/TUI policy behavior remains unchanged.

The renderer uses the distinct `promptWithWebConsent` bridge and
`agent:prompt-with-agent-context` IPC channel, including the captured Agent settings
revision. There is no legacy prompt fallback:
if a renderer hot update encounters an older preload or main process, sending
fails closed until Axiom restarts, rather than silently dropping the Web choice.

Tool calls appear at their execution point: assistant introduction → tool steps
→ assistant continuation. The native store starts a new assistant/reasoning
segment after each tool boundary; ACP's existing `timelineItemId` metadata ties
live deltas to those exact durable segments. Tool updates reconcile by call ID,
including across snapshot replacements, so refreshing cannot duplicate a step.
This is presentation/storage ordering only: response receipt verification and
client-side tool execution/security gates are unchanged.

Rows use phase-aware TUI-style labels (for example, “Searched the web for …”),
compact 2px gaps between adjacent tools, and one action bar at the end of a
response. Fetches are plain, non-expandable rows. Failed tools show a short
reason inline (such as “Blocked by website”, “Page not found”, or “Request timed
out”), without raw JSON or internal errors. Search details still show bounded,
plain-text source summaries and safe HTTP(S) links; successful non-fetch tool
details remain expandable and credential-redacted.
Keyboard expansion works without native dialogs. Earlier saved turns that
already merged all assistant text cannot recover their original interleaving;
their content is preserved, while new turns retain the execution boundaries.

## Run

From the repository root:

```text
pnpm install
pnpm desktop
```

`pnpm desktop` compiles AxiomCLI before starting Electron, so it always drives
the current Rust code. On Linux, `pnpm desktop:install` is an optional one-time
step that installs development launcher metadata.

The Linux desktop entry gives the development build its own application ID,
icon, and taskbar group instead of letting the desktop shell group it with
other programs running through the generic Electron executable. Re-run
`pnpm desktop:install` after moving the repository.
The normal launcher uses production services. For a separate **Axiom Staging**
launcher targeting the testing profile, run
`node apps/desktop/scripts/install-linux-desktop-entry.mjs --staging`.
Installing staging does not replace the normal Axiom launcher.
The launch script and main process replace inherited IDE desktop-identity hints;
X11 uses the stable `Axiom` window class and Wayland uses
`stream.axiom.desktop`. Both resolve to Axiom's installed launcher and icon.

On macOS the window keeps its native traffic lights over a hidden title bar
and has a minimal application menu (About, Edit, Window). Windows and Linux
use app-drawn controls in a matching cutout on the outer canvas and have no
menu. Drag the top strip or the header to move the window.

The renderer is sandboxed with context isolation, Node integration disabled,
a restrictive CSP, and HTTP(S)-only external-link handling. Packaged builds
embed the desktop product prompt in AxiomCLI.

Unpackaged development resolves `../../target/debug/axiomcli` from this app.
`AXIOMCLI_SIDECAR` may override that development path. Packaged builds never
use the override and require `resources/bin/axiomcli`.

Packaged builds pin `https://auth.axiom.stream` and
`https://api.axiom.stream`. Unpackaged development may select the matching
production pair, staging pair, or loopback pair with `AXIOM_AUTH_URL` and
`AXIOM_BASE_URL`; the local API must use HTTPS with a trusted certificate, matching
the native secure client's transport policy. The local auth origin may use HTTP
or HTTPS (use HTTPS for both when testing browser cookies). Mixed deployments
and arbitrary origins are rejected. Desktop
also removes `AXIOM_API_KEY` from the sidecar environment so renderer-visible
sign-in is always a native account session.

The desktop Sign in action opens browser authorization directly; the website
handles method selection. An inline status shows the matching verification code,
with reopen, cancel, and retry actions instead of a desktop modal.
Desktop sign-in URLs are validated and opened by Electron using the operating
system's default browser. The native sidecar does not launch a browser in desktop
mode, so isolated staging account storage cannot override browser preferences.
Standalone CLI sign-in still opens the browser itself.

```text
pnpm capture
```

writes PNG snapshots of every UI state in both themes
(`light-*.png`, `dark-*.png`) to `~/axiom-desktop-captures` (or
`--capture=/path`). `AXIOMCLI_SIDECAR` is honoured here too.

## Headless UI regression tests

```text
pnpm --filter axiom-desktop exec playwright install chromium
pnpm --filter axiom-desktop test:browser
```

These run the real renderer in isolated headless Chromium with an in-memory
desktop bridge. They never open Electron, control the system mouse/keyboard,
access real conversations, or make provider requests. Coverage includes thread
deletion followed by typing/submitting, cancellation and focus restoration,
deletion failures, and folder deletion. The Linux, Windows, and macOS renderer
layouts are exercised; this does not replace native OS smoke tests.

Thread and folder confirmations use renderer-owned HTML dialogs, not blocking
JavaScript/OS dialogs. Modal focus is released before navigation and restored to
the composer, including cancellation and failed deletion.

## Math in chat

Replies support inline `$...$`, display `$$` blocks, and fenced `math` blocks,
including fractions, integrals, matrices, and aligned equations. KaTeX and its
fonts ship inside the app, with no CDN or server-side message processing. Wide
display equations scroll horizontally. Literal code and escaped dollar amounts
remain unchanged, and unfinished/invalid streamed formulas remain readable.
The renderer keeps HTML/image/link-producing TeX commands disabled and bounds
macro expansion and dimensions (see [KaTeX options](https://katex.org/docs/options)).

## Package

```text
pnpm brand:assets   # regenerate build/ from the design masters (macOS, python3 + Pillow)
pnpm dist           # electron-vite build && electron-builder (needs electron-builder installed)
```

`electron-builder.yml` and `build/README.md` describe the targets and assets.
Tagged CI builds the matching release-mode sidecar and Desktop package natively
for macOS Intel, macOS Apple Silicon, Windows x64, and Windows ARM64. Local
`pnpm dist` requires the appropriate release sidecar for the host platform;
cross-building an installer does not synthesize a sidecar for another OS or
architecture.

### Usage

Settings → Usage shows all-time posted inference charges and each model's share
in a pie chart. The native client reads `/api/v1/usage/spend` with an account
access token through the optional `usage@1` ACP extension. Totals come from the
account ledger, survive usage-period resets, and include charges across devices,
the CLI, proxy, and desktop. Top-ups, pending holds, and unbilled estimates are
excluded. Costs remain integer USD millionths through aggregation and display.
Unattributed historical charges remain in “Other models” so the total stays
complete. Signed-out, loading, empty, and error states are distinct; changing
accounts or reconnecting discards pending/stale results.
