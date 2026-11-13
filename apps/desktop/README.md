# Axiom desktop

Electron main supervises `axiomcli acp --frontend desktop-chat`, negotiates the
versioned Axiom extension, and exposes a narrow typed API through the sandboxed
preload. The renderer uses the real session catalog and ACP event stream for
messages, reasoning, tool calls, plans, questions, cancellation, and settings;
it does not synthesize a conversation or choose a filesystem workspace.

AxiomCLI supplies the canonical desktop chat directory and enforces the Web
permission profile. Threads, timeline items, profile preferences, sidebar
collections, and assignments all live in AxiomCLI's Desktop-profile state
database. Electron has no SQLite connection and owns only presentation state.
Folder names never become filesystem paths. Login identity is shared with
AxiomCLI, while CLI and Desktop conversation databases remain separate.

Brand treatment (palette, Geist, wordmark, sakura/ember empty state, message
chrome) is copied into this package. The closed-source website is not a
runtime or build dependency.

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

The window is frameless. Drag the top of the sidebar or the chat header to
move it. There is no File / Edit / View menu.

The renderer is sandboxed with context isolation, Node integration disabled,
a restrictive CSP, and HTTP(S)-only external-link handling. Packaged builds
embed the desktop product prompt in AxiomCLI.

Sakura petal animation is vendored from sakura-js (MIT).

Unpackaged development resolves `../../target/debug/axiomcli` from this app.
`AXIOMCLI_SIDECAR` may override that development path. Packaged builds never
use the override and require `resources/bin/axiomcli`.

```text
pnpm capture
```

writes PNG snapshots to `~/axiom-desktop-captures` (or `--capture=/path`).
