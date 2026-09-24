# CLI and TUI guide

AxiomCLI provides an interactive terminal, a one-shot command, and a local ACP
server over the same native runtime. It performs provider verification and E2EE
directly. The local proxy is an independent integration.

## Launch and inspect

```sh
axiomcli --help
axiomcli tui
axiomcli tui --cwd /absolute/path/to/project
axiomcli tui --resume SESSION_ID
axiomcli exec "inspect this repository" --cwd /absolute/path/to/project
axiomcli acp
axiomcli doctor
axiomcli models
axiomcli update
```

Without a subcommand, AxiomCLI starts the TUI. `doctor` reports configuration,
paths and dependency/account diagnostics; `models` lists the discovered catalog.
These commands can perform account/catalog/service checks, so they are not all
offline probes. `--help` and `--version` do not require sign-in.

Native sign-in uses the browser and OS credential store. `/login`, `/account`
and `/logout` manage it; `AXIOM_API_KEY` is a separate standalone automation
override. See [configuration](configuration.md) and [privacy](privacy.md).

`exec` cannot answer human approval or question prompts. Choose an appropriate
trusted tool profile; requests requiring unavailable human authority fail closed.
`acp` reserves stdout for JSON-RPC and writes diagnostics to stderr. The
[ACP guide](acp-compatibility.md) owns its capabilities and extension contract.

## Updates

Use `/update` while the TUI is idle, or run `axiomcli update`. The native client
checks the signed public release inventory, downloads its matching installer,
verifies the bytes, installs and restarts the TUI in the same terminal. `/update`
restores the current workspace and session. `axiomcli update --check` only checks.
The TUI leaves fullscreen mode before downloading; Ctrl-C can cancel a download.
Close other CLI/proxy sessions and, for a bundled CLI, Desktop when the helper
waits for the installation lock. It times out after three minutes without killing
them. Windows users should launch through the installed `axiomcli.cmd` command so
the terminal launcher can hold the console open during replacement.

A bundled CLI updates the entire Desktop package; standalone installers update
CLI and proxy together. OS authorization may be required for system packages.
Source builds have no installation owner and must be rebuilt. Unsigned preview
builds have no trusted stable release key and cannot install stable updates.

Installed TUIs check quietly at startup and every six hours. Notices and update
failures remain local and are never sent to a model. `exec` and ACP do not check
in the background. Update HTTP requests carry no account credentials or messages.
Signatures, strict target selection, bounded transport, byte length, SHA-256 and
remembered release sequence/version are checked again before application.
See [installation](installing-desktop.md) and [release trust](releasing.md).

## Terminal controls

The useful minimum viewport is 42×10 cells. Press `?` for help, `Tab` to enter
the transcript, `Enter` to expand task details, `v` for the full viewer, and
`Ctrl+F` to search. Terminal mouse selection remains available. `Esc` cancels
active work or closes the current overlay; two consecutive `Ctrl+C` presses exit.
Terminal restoration is guarded on exit and panic.

The status line shows task progress and interaction prompts. Open `/balance` to
check account credit. Composer controls show the selected model, thinking level,
Permissions profile and Web access.

Typing `/` filters slash-command hints; `Tab` completes command names/arguments.
Slash commands are handled locally rather than sent to the model as prompts.
Signed-out startup opens the login view without requesting the authenticated
model catalog. After sign-in, the selected model and reasoning settings are
reconciled against the live catalog before the new session is persisted.

| Command | Purpose |
|---|---|
| `/model` | Search the discovered catalog; unsupported/manual IDs are rejected |
| `/thinking <level>` | Select a supported reasoning level |
| `/permissions` | Open tool permission settings |
| `/web on` / `/web off` | Opt into or disable external search/fetch tools for subsequent turns |
| `/resume` | Search saved transcripts matching the canonical workspace |
| `/delete` | Select and confirm deletion of saved sessions except the active one |
| `/compact [focus]` | Cancellable encrypted successor-summary operation |
| `/login`, `/logout`, `/account` | Native account operations |
| `/balance` (or `/credits`) | View available, trial and other credit; refresh deposit status |
| `/topup` | Show the mainnet Zcash deposit address |
| `/redeem` | Open masked gift-code entry; takes no code argument |
| `/security`, `/refresh` | Inspect cached public evidence or request fresh local verification |
| `/security accept-outdated` | Explicitly accept NEAR’s Intel `OutOfDate` posture until restart, then verify again |
| `/usage` | Inspect the last reported request's usage, context capacity and compaction threshold |
| `/theme dark\|light\|terminal` | Set the session appearance |
| `/help` | Show the complete command reference |

Web starts off. Search queries and fetched URLs leave the encrypted inference
boundary. Tool profiles and process isolation are described in
[configuration](configuration.md) and [security](threat-model.md).

Submitting an ordinary prompt remembers its model and thinking level as defaults
for future chats. Resume restores the selected transcript's durable settings;
a model selection that never ran does not become the resumed prompt's model.
In the TUI, a short local label appears immediately, followed by one background
title request to the selected model through attested provider E2EE. Its normal
model charge is recorded as title usage. Failures retain the local label; manual
renames and existing named threads are preserved. Title requests omit tools and
attachments, use bounded first-prompt text and stop after 60 seconds. Headless
commands retain local labels. A model's reasoning choices and
context capacity come from validated catalog metadata, not its name.
The composer and `/thinking` suggestions use the selected offering's exact
controls, including enabled/disabled modes or supported effort levels. Models
without reasoning controls do not show a thinking selector.

During active work, Enter can submit steering for the next safe boundary.
Typing (including uppercase letters) and pasting both edit the steering draft.
Slash commands remain local: `/help`, `/usage`, `/security` and `/theme` are
available while running; other commands preserve the draft and ask you to stop
the active task first. `/security` shows retained evidence during a task; fresh
verification waits until the task is stopped or complete. Escape closes an open
viewer or search before it cancels active work.
Incoming approval and question cards appear above open inspectors and take
keyboard focus; Escape on a foreground interaction still cancels the task.
Approval decisions require explicit chords: `Ctrl+Y` allows once, `Ctrl+N`
denies, and, when offered, `Ctrl+G` grants the exact operation for the session
or `Ctrl+P` grants the displayed safe scope. Ordinary typing, Enter and paste
cannot approve or deny a tool when its card arrives during steering input.
Paste targets the visible input: picker filters, transcript search, question
answers, the masked gift-code field or the composer. Non-editable overlays never change a hidden draft.
Automatic and explicit compaction use tool-free attested E2EE inference and
persist successor context for resume. Failed or cancelled work remains incomplete.

The footer's context percentage uses the last completed conversation request's
reported input/output tokens and that report's model capacity. `/usage` shows the
counts, model, report time and effective automatic-compaction threshold. Missing
usage or capacity is shown as unknown. Reports survive resume; switching models
does not rescale old counts against a new model. These values are not a draft
estimate, cumulative turn usage or a response-length limit. Per-request usage and charges remain available in the account usage views.

When the last conversation request in a completed task has an authenticated
`length` finish reason, the transcript explains that the model reached its output
limit and can be asked to continue. This uses the native request record across
providers and survives resume. A settled charge alone never verifies a response;
cancelled, failed or unauthenticated output does not gain this completion notice.

## Verification reports

The header describes the current model's retained TEE report. Expired, missing,
mismatched or failed reports are never displayed as current verified proof.
While signed in, editing a nonempty message starts verification if the selected
model has no valid proof and no check is already running. Pasting also counts;
startup, sign-in, switching threads, navigation and timers do not trigger checks.
Valid proof is reused until expiry. Failed checks back off from 30 seconds to
five minutes, and another edit is required to retry. Enter reuses native cached
keys or joins verification in progress without requiring a second submission.

After 30 minutes without user input or running work, the header becomes neutral
`IDLE`. Input wakes the display; only composition or an explicit refresh starts
verification. Background accounting does not reset inactivity. Opening `/security`
requests fresh evidence between tasks when the current report is unavailable or expired;
`/refresh` or `R` in the inspector explicitly verifies again. Inference continues
to enforce fresh attestation and authenticated completion in the native provider
implementation, independently of the displayed report.
When verification fails without producing a report, the inspector retains the
sanitized failure reason in its scrollable body. A new check or model/account
reset clears that process-local diagnostic; it is not saved as verified evidence.

The inspector's summary includes the checked/expiry times, protocol identifiers,
key bindings, local checks and every provider evidence field. `V` cycles through
the summary, provider evidence and complete raw JSON report. Optional workload
documents are displayed without assuming Docker Compose, a particular hardware
vendor, a signed receipt or a direct worker-attestation topology. Source
provenance and verification scope remain visible as supplied in each provider's
claims. A provider with no separate workload document can still have valid proof.

Terminal output is sanitized and large raw/document displays are bounded; `S`
from the raw view saves the complete original report without overwriting an
existing file. The retained report's status describes its last check; the header
and inspector banner separately explain whether that report is current. Report
verification is distinct from the terminal authentication of each reply.

## Credit and funding

Use `/login` for native interactive sign-in before managing credit. An
`AXIOM_API_KEY` automation credential does not authorize billing operations.
`/balance` and `/topup` open the credit screen: **R** refreshes, **G** opens gift
redemption, **C** copies a ready deposit address using the terminal's OSC 52
clipboard support, and **Esc** closes it. Terminal selection is also available.
Use Up/Down or Page Up/Page Down when the content exceeds the viewport. Status
refreshes every five seconds while viewing the balance.

Send only mainnet ZEC to the displayed address from your wallet. Credit is added
after the required confirmations and USD valuation. The screen does not offer
funding while conversion is unavailable. A payment-review hold can keep available
credit at zero even after a gift is redeemed; topping up does not clear that hold.

Run `/redeem` **without arguments**, then paste/type the gift code in its masked
input and press Enter. Ctrl+U clears it. Never enter a gift code as a chat prompt.
The screen handles code input separately and does not save it in conversations.
After an interrupted attempt, paste the same code to retry; the server credits a
card once, and replay by the same account is successful without another credit.
Codes add non-trial USD inference credit and do not expire. The backend must have
gift redemption deployed; older servers return an availability error.

## System instructions

```sh
axiomcli --system-prompt-file ./AGENT.md tui
axiomcli --system-prompt-file ./AGENT.md exec "inspect this repository"
axiomcli --system-prompt-file ./AGENT.md acp
```

The global option accepts a UTF-8 file up to 256 KiB and replaces the process's
behavioral prompt. Trusted runtime context and the untrusted-input rule are
still appended. The file is not persisted in session journals. Resuming uses
the prompt supplied to that launch; Desktop's product prompt cannot be replaced
through this option.

## Saved sessions and plans

```sh
axiomcli sessions list
axiomcli sessions list --all
axiomcli sessions show SESSION_ID
axiomcli sessions export SESSION_ID --omit-tool-output
axiomcli sessions rename SESSION_ID "New title"
axiomcli sessions archive SESSION_ID
axiomcli sessions unarchive SESSION_ID
axiomcli plans --help
axiomcli plans --cwd /absolute/path/to/project list
```

The implemented session subcommands are list, show, export, rename, archive and
unarchive. Interactive deletion uses `/delete`; ACP uses preview/confirm.
There is no `sessions prune` command. Export supports omitting prompts/tool
output and a bounded `--max-items` count. Local recovery never replays interrupted
tool side effects. The TUI warns about interrupted operations and recovery
problems, not ordinary file edits. Recorded edits remain in the task history;
resume neither adds nor displays the legacy workspace-change summary warning.

Plans are revisioned local artifacts with comments, revision requests, proposal,
approval and abandonment. Plan approval records review; it does not execute the
plan. Background process tasks are bounded to four concurrent children. Subagents
are not implemented.

Command authority: [main.rs](../apps/axiomcli/src/main.rs),
[slash.rs](../apps/axiomcli/src/slash.rs),
[TUI](../apps/axiomcli/src/tui.rs), and
[agent runtime](../apps/axiomcli/src/agent.rs).

Outdated-provider consent is held in memory for the current native runtime and
provider. The TUI keeps a yellow TEE warning after acceptance. Submit the prompt
again after the fresh check succeeds. No other failed check becomes acceptable;
headless and local-proxy processes remain strict unless explicitly controlled
through a supported interactive consent operation.
