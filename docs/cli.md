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

`axiomcli update` checks the same anonymous release feed as Desktop, without
loading credentials, account state, configuration, or model messages. It reports
the latest version and the update path for the running executable. Checks reject
redirects, malformed metadata, unsupported archives, and oversized responses.

Installed TUIs check quietly at startup and every six hours. A newer version
appears as a local transcript notice; failed checks do not interrupt work, and
notices are neither saved as conversation history nor sent to models. Source
builds skip background checks. `exec` and ACP never start update checks.

A CLI inside Desktop's resources directory, including a PATH symlink or AppImage
wrapper, directs you to **Axiom → Settings → Updates**. One desktop installer
replaces Desktop and its bundled CLI together; there is no independent bundled
CLI replacement. Close Desktop and all terminal sessions before installing.

Standalone Linux x86-64 installs made by `scripts/install-release` receive the
matching CLI + local proxy archive URL, byte size, SHA-256 and checksum-file URL.
Download and verify the archive, stop CLI and proxy sessions, then reinstall the
complete bundle with the same prefix. If the release has no compatible standalone
archive, the command says so and never substitutes a desktop installer or another
architecture. The command reports updates; it does not install or execute them.
Unrecognized binaries receive source-build guidance.

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
| `/theme dark\|light\|terminal` | Set the session appearance |
| `/help` | Show the complete command reference |

Web starts off. Search queries and fetched URLs leave the encrypted inference
boundary. Tool profiles and process isolation are described in
[configuration](configuration.md) and [security](threat-model.md).

Submitting an ordinary prompt remembers its model and thinking level as defaults
for future chats. Resume restores the selected transcript's durable settings;
a model selection that never ran does not become the resumed prompt's model.
Titles derive locally from the first prompt. A model's reasoning choices and
context capacity come from validated catalog metadata, not its name.

During active work, Enter can submit steering for the next safe boundary.
Slash commands wait until the active task finishes and are never submitted as steering.
Automatic and explicit compaction use tool-free attested E2EE inference and
persist successor context for resume. Failed or cancelled work remains incomplete.

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
tool side effects.

Plans are revisioned local artifacts with comments, revision requests, proposal,
approval and abandonment. Plan approval records review; it does not execute the
plan. Background process tasks are bounded to four concurrent children. Subagents
are not implemented.

Command authority: [main.rs](../apps/axiomcli/src/main.rs),
[slash.rs](../apps/axiomcli/src/slash.rs),
[TUI](../apps/axiomcli/src/tui.rs), and
[agent runtime](../apps/axiomcli/src/agent.rs).
