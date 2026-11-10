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
as a pair. Update jobs must resolve inside their installation's private cache;
both the job and cache directory are canonicalized before checking containment.
This accounts for Windows' verbatim path namespace without accepting a job in
a sibling directory or outside the cache. Installed CLI/ACP/proxy processes share
an installation lock. The helper waits up to three minutes for them to close and
never kills them. A bundled TUI can update with Desktop closed; if Desktop is
open, close it when the helper asks for other processes to stop. Simultaneous
helpers recheck the installed version under the lock and do not reinstall an
already-applied target. On Windows, an already-exited Desktop parent counts as
ready; the bounded wait runs without opening a console window and still rejects
a parent that stays open.

Desktop proceeds with restart only after the native helper acknowledges its
verified job and the handoff CLI exits successfully. It releases the bridge's
pipe handles at that point: on Windows the detached helper can retain inherited
handles while waiting for Desktop to close. Other native update operations still
wait for complete output and reject incomplete responses.

One native engine is used instead of adding `electron-updater`: a bundled TUI
must also update and restart when Electron is not running. The same finished
installer bytes serve initial downloads and updates. macOS uses PKG, without a
second ZIP update channel. Linux DEB/Pacman invoke their package managers through
Polkit; AppImage replaces its installed image. Standalone Linux activates a
versioned CLI/proxy directory through one symlink. Windows uses signed NSIS;
macOS uses signed, notarized PKG installers and the OS authorization dialog.

## Windows setup and first launch

The published Windows 0.1.12 native updater rejects its own staged job because
it compares a canonical job path with an ordinary cache path. After correcting
that check, the Desktop bridge also needs to release inherited pipes to avoid
waiting for a helper that is waiting for Desktop to close, and the Windows parent
wait must succeed when Desktop has already exited. Windows upgrades
must use the website Setup until the corrected updater is released. Setup
itself installs the current signed release successfully. Microsoft Store
submission remains held pending signed automatic-handoff acceptance.

The website's small native `AxiomSetup` executable fetches the latest stable
inventory from the same public release API as the updater. It uses the shared
`axiom-update-client` verifier: compiled Ed25519 trust keys, bounded responses,
exact target, size and SHA-256, and remembered signed sequence/version. It also
requires a valid timestamped Authenticode signature from its compiled expected
publisher, immediately before executing the full NSIS installer.

Setup installs Desktop and its matching CLI together. The setup binary is x64;
it runs through Windows 11's x64 emulation on ARM64 PCs, while the combined
NSIS installer chooses native application payloads. Setup preserves a registered
installation's destination and scope, never downgrades a newer installed version,
and waits for Desktop and CLI/proxy sessions without killing them. Concurrent
setup processes share a per-user lock. Downloads are staged privately and removed
on failure or cancellation. Installation cannot be cancelled after handoff to
NSIS; installer repair semantics still apply. Fresh setup installs per user into
`%LOCALAPPDATA%\Programs\Axiom`; unregistered nonempty destinations require repair
with the offline installer. macOS and Linux retain their existing packages.

A fresh Windows offline installation writes `resources/axiom-first-launch.json`.
Only stable packaged Windows builds claim this marker. Before networking, the
app creates a durable per-user, case-insensitive installation-path attempt in
`first-launch-updates/` inside its user data. This survives upgrades and prevents
restart loops; changing versions alone does not reset it. Existing installations
are not marked fresh. An interrupted or failed first attempt falls back to the
normal update controls on later launches.

The renderer registers its state-flush handler, then signals readiness. The
fresh launch checks for the latest signed release and automatically downloads,
verifies and applies a newer version through the existing native updater. The
startup screen shows progress, retry and **Use installed version**; no account is
needed to update. A current build proceeds directly. Offline or verification
failure leaves the complete installed app usable. Existing chat/proxy guards and
durable-state flush still run before restart; no active work is forced closed.

The website setup is a separate signed artifact with its own component version
and revision-qualified immutable URL. It is not an application update target;
the app inventory still contains eight installers. Microsoft Store submission
uses the complete offline Desktop EXE, never the downloader. The EXE/MSI Store
route leaves subsequent app updates under Axiom's existing native updater.

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
