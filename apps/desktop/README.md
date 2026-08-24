# Axiom desktop

The current renderer is the Axiom desktop visual concept: sidebar, loose
threads, the chat surface, and a Proxy pane. Its production boundary is now
wired: Electron main supervises `axiomcli acp`, negotiates the versioned Axiom
extension, and exposes a narrow typed API through the sandboxed preload.

The visual prototype still uses its demo conversation data. A separate desktop
UI pass can consume `window.axiomDesktop.agent` without importing Rust, parsing
JSON-RPC, spawning a process, or reimplementing policy. The API covers sessions,
streaming prompts, cancellation, model/thinking/permission settings, account
login, attestation evidence, compaction, deletion, approvals, and questions.

Brand treatment (palette, Geist, wordmark, sakura/ember empty state, message
chrome) is copied into this package. The closed-source website is not a
runtime or build dependency.

## Run

From the repository root:

```text
pnpm install
cargo build -p axiomcli
pnpm desktop:install # Linux only; installs the dev launcher metadata once
pnpm desktop
```

The Linux desktop entry gives the development build its own application ID,
icon, and taskbar group instead of letting the desktop shell group it with
other programs running through the generic Electron executable. Re-run
`pnpm desktop:install` after moving the repository.

The window is frameless. Drag the top of the sidebar or the chat header to
move it. There is no File / Edit / View menu.

Sakura petal animation is vendored from sakura-js (MIT).

Unpackaged development resolves `../../target/debug/axiomcli` from this app.
`AXIOMCLI_SIDECAR` may override that development path. Packaged builds never
use the override and require `resources/bin/axiomcli`.

```text
pnpm capture
```

writes PNG snapshots to `~/axiom-desktop-captures` (or `--capture=/path`).
