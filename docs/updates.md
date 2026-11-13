# Native update architecture

## Installation ownership

Desktop **Settings → Updates → Update and restart**, TUI `/update`, and
`axiomcli update` use the same native Rust updater. `axiomcli update --check`
only checks. Desktop presents progress and cancellation; it waits for running
chats and its proxy to finish, commits drafts, starts an independently verified
helper, closes its sidecar, and exits. The helper runs the owning installer,
checks the installed CLI version, and relaunches the requested frontend.

The TUI closes its terminal UI before downloading. On Unix it replaces itself
with the copied helper and then the updated TUI. Windows' installed CMD/PowerShell
launcher keeps the terminal open while the old binary exits; it runs the helper
and resumes the same session/workspace. Update failures are recorded locally and
shown on the next check. Restart resets all runtime-only attestation consent.

Installation identity comes from `axiom-install.json`. Desktop and its bundled
CLI always update the complete Desktop package. Standalone CLI and proxy update
as a pair. Installed CLI/ACP/proxy processes share an installation lock; the
helper waits up to three minutes for them to close and never kills them. A bundled
TUI can update with Desktop closed; if Desktop is open, close it when the helper
asks for other processes to stop. Simultaneous helpers recheck the installed
version under the lock and do not reinstall an already-applied target.

One native engine is used instead of adding `electron-updater`: a bundled TUI
must also update and restart when Electron is not running. The same finished
installer bytes serve initial downloads and updates. macOS uses PKG, without a
second ZIP update channel. Linux DEB/Pacman invoke their package managers through
Polkit; AppImage replaces its installed image. Standalone Linux activates a
versioned CLI/proxy directory through one symlink. Windows uses signed NSIS;
macOS uses signed, notarized PKG installers and the OS authorization dialog.

## Trust and publication

The schema-3 inventory records product, platform, architecture, format, byte
length, SHA-256, immutable CDN/GitHub URLs, native revision, version, and a
monotonic sequence. An Ed25519 signature covers the canonical inventory. Public
keys are compiled into the native client with `AXIOM_UPDATE_PUBLIC_KEYS`; the
private signer exists only in the protected release environment. Clients reject
unsigned inventories, unknown keys, metadata changes at an accepted version,
rollback, wrong targets, redirects and incorrect/truncated bytes. The helper
repeats verification immediately before installation.

The inventory is durable release metadata and has no wall-clock expiry. The
client remembers the highest verified sequence/version; this prevents rollback
after observing a newer release, but cannot detect a stale first response or
an unavailable feed. No clock-based freshness guarantee is claimed.

The native repository produces signed installers and a manifest for the platform
publisher. Repository access and signing setup are described in
[repository setup](repository-setup.md). The signed inventory identifies the
exact promoted native source revision.

Native release jobs build all 8 installers, sign their final-byte inventory,
and upload a draft release. The platform publisher validates the native tag's
`main` ancestry and signatures, stages immutable CDN objects, verifies both
mirrors, publishes the GitHub release, then deploys the download page and native
feed together. CDN `latest.json` follows. A checkpoint artifact records progress;
retries compare immutable bytes and never replace a published version.

These services have no distributed transaction. A failure before feed activation
keeps the old feed. A failure after website activation may leave the CDN alias
behind, but the native feed only references installers already available on both
mirrors. Production publication is restricted to promoted `main` revisions.

## Qualification

Record signed installer and restart acceptance for every supported package and
architecture using [release acceptance](validation.md). Local unit and integration
checks do not establish OS authorization, vault, GUI or same-terminal behavior.

## Recovery boundaries

Failed checks/downloads/authentication leave installed files untouched. The Linux
standalone installer keeps previous version directories and activates CLI/proxy
together. NSIS, PKG, DEB and Pacman use their installer recovery semantics;
AppImage replacement is atomic at the file level. There is no universal automatic
rollback or cross-version database migration across incompatible storage versions.
If application fails, retry the same verified installer; account data is stored
outside the installation. Never start an older binary against incompatible data.

See [release publication](releasing.md), [installation](installing-desktop.md)
and [CLI/TUI updates](cli.md#updates).
