# Automatic updates and installer publication

Updated 2026-09-22. Implementation targets `dev` in `axiomai` and
`axiom-platform`. No promotion, production deployment, release tag, signing-key
provisioning, or live-feed activation is part of this development change.

There are **zero users to migrate**. The first release uses one signed schema-3
inventory and installation markers. There is no legacy-client adapter, bootstrap
release, format guessing, or account/storage migration in this feature.

## Implemented design

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

Both source repositories are private. The publisher targets a separate public
`astrea-foundation/axiom-releases` distribution repository; it contains installer
assets and release metadata, not a copy of either source checkout. Provisioning
that repository and release credentials belongs to release setup, not this dev
change. The signed inventory identifies the private native source revision.

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

## Implementation and remaining qualification

- [x] Common signed contract and independent native/platform fixtures.
- [x] Native ownership, installation locks, staging, helper and version check.
- [x] Desktop/TUI update controls, restart handoff and draft preservation.
- [x] Desktop and standalone installer builders across the supported matrix.
- [x] GitHub/CDN publication, signature verification and readiness checks.
- [x] Local contract, helper, Linux installer, UI and publication regressions.
- [ ] Record signed A-to-B installer/restart acceptance on real Windows x64/ARM64,
  macOS x64/ARM64 and Linux DEB/Pacman/AppImage installations.
- [ ] Provision the public distribution repository and protected release secrets.
- [ ] Promote only after an explicit release request and qualification review.

Unit and local integration tests are implementation evidence, not signed native
OS acceptance. Current evidence and limitations belong in
[the update test report](testing/automatic-updates-2026-09-22.md).

## Recovery boundaries

Failed checks/downloads/authentication leave installed files untouched. The Linux
standalone installer keeps previous version directories and activates CLI/proxy
together. NSIS, PKG, DEB and Pacman use their installer recovery semantics;
AppImage replacement is atomic at the file level. There is no universal automatic
rollback or cross-version database migration in this initial implementation.
If application fails, retry the same verified installer; account data is stored
outside the installation. Never start an older binary against incompatible data.

See [release setup and publication](releasing.md), [installation](installing-desktop.md),
[CLI/TUI](cli.md#updates), and the
[platform publisher guide](https://github.com/astrea-foundation/axiom-platform/blob/dev/docs/operations/releases.md).
