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

Brand treatment (light and dark token themes, DM Sans / DM Mono, the PP Cirka
display face, the falcon symbol) lives in this package; the closed-source
website is not a runtime or build dependency. `REDESIGN.md` explains the
design system, its conventions, and the packaging pipeline.

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

On macOS the window keeps its native traffic lights over a hidden title bar
and has a minimal application menu (About, Edit, Window). Windows and Linux
are frameless with the app's own window controls and no menu. Drag the top
strip or the header to move the window.

The renderer is sandboxed with context isolation, Node integration disabled,
a restrictive CSP, and HTTP(S)-only external-link handling. Packaged builds
embed the desktop product prompt in AxiomCLI.

Unpackaged development resolves `../../target/debug/axiomcli` from this app.
`AXIOMCLI_SIDECAR` may override that development path. Packaged builds never
use the override and require `resources/bin/axiomcli`.

```text
pnpm capture
```

writes PNG snapshots of every UI state in both themes
(`light-*.png`, `dark-*.png`) to `~/axiom-desktop-captures` (or
`--capture=/path`). `AXIOMCLI_SIDECAR` is honoured here too.

## Package

```text
pnpm brand:assets   # regenerate build/ from the design masters (macOS, python3 + Pillow)
pnpm dist           # electron-vite build && electron-builder (needs electron-builder installed)
```

`electron-builder.yml` and `build/README.md` describe the targets and assets.
