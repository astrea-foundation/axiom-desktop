# Automatic updates and installer publication plan

Date: 2026-09-22. Status: planned; implementation has not started.
Reviewed bases: native `76bd009`, platform `59d1ba4`.

This is the authoritative implementation plan for Desktop/TUI automatic updates
and coordinated CDN/GitHub Releases downloads. Implementation branches start from
`dev` and target `dev`. This plan does not authorize promotion to `main`, release
tags, production publication, or changes to the live update feed. Those remain
separate release work under [CONTRIBUTING](../CONTRIBUTING.md).

## Intended experience

- Desktop exposes **Update and restart**. After the user selects it, Axiom
  downloads and verifies the update, installs it, and reopens the updated app.
  There is no Save As dialog or requirement to find and run an installer.
- TUI exposes an update action and `/update`; `axiomcli update` uses the same
  operation. A check-only CLI option preserves a non-mutating discovery path.
  An updated TUI resumes in the same terminal, workspace, account, and session.
- Progress, cancellation before installation, waiting for active work, and
  failures are visible. The previous working installation remains usable if
  downloading or verification fails. Installation success requires the expected
  version to launch, not merely an installer process being started.
- Desktop and the CLI bundled with it update as one installation, whichever
  interface initiated the update. Standalone CLI installations update their CLI
  and local proxy together.
- Users can download the same finished installers from the Axiom CDN and
  GitHub Releases. Public download links do not point at Actions artifact ZIPs
  or the internal `windows-input-*` build handoff.
- System-owned installations may need the operating system's authorization
  prompt. User-installed packages should not request elevation unnecessarily.

## Current implementation and reusable pieces

| Area | Current behavior | Required change |
|---|---|---|
| [Desktop updater](../apps/desktop/src/main/update-service.ts) | Discovers newer releases; downloads and checks an installer; reveals the saved file | Add authenticated installation, lifecycle coordination, and relaunch |
| [CLI updater](../apps/axiomcli/src/updates.rs) | Checks the feed and prints manual instructions; TUI shows a startup notice | Add update UI, installation handoff, and terminal/session restoration |
| [Desktop packaging](../apps/desktop/electron-builder.yml) | Builds Windows NSIS EXE, macOS PKG/DMG/ZIP, and Linux DEB/Pacman/AppImage | Preserve installers; generate and retain updater payloads and metadata |
| [Standalone packaging](../scripts/package-release) | Linux x64 CLI/proxy archive with manual installation scripts | Add supported standalone installers and a transactional update layout |
| [Windows signing](../.github/workflows/windows-signed.yml) | Native build, separate signing/NSIS packaging, then Actions artifacts | Preserve final-byte signing; publish updater metadata and release assets |
| [Release contract](../packages/desktop-releases/manifest.mjs) | Schema-1 CDN URLs, sizes, hashes, and a Linux-x64-only standalone archive entry | Preserve legacy clients and add a versioned update contract |
| Platform publication | CDN upload and website catalog publication are separate operations | Coordinate those operations with GitHub Release publication |

Existing discovery, progress UI, local storage, package builders, signing,
version matching, CDN verification, and release catalog tooling are reusable.
The current downloader is not a complete installation engine.

## Architecture decisions

### One installation owner

Record installation identity, product kind, platform, binary architecture,
package format, scope, and canonical installation root. Resolve CLI symlinks
before selecting an owner. Do not guess ownership from an executable filename.

Use one local update coordinator and installation lock per installation. Desktop
and bundled TUI requests join the same operation rather than racing separate
installers. A TUI-only invocation must also work when Desktop is closed: start
the owning package's update helper, complete the update, and relaunch the TUI.
It must not require the user to open Desktop or start an unwanted Desktop window.

The bundled CLI is never replaced independently inside a signed app bundle.
Standalone CLI/proxy packages have their own installation identity and helper.
Any local IPC used for coordination is restricted to the owning OS user and
installation; it is separate from the public inference API and local proxy API.

### Desktop update engine

Integrate `electron-updater` behind the existing Desktop update service, using a
version compatible with the checked-in electron-builder major. Pin and test that
combination; upgrading the packaging major is a separate, explicit change.
Keep UI and IPC in Axiom, with platform-specific apply/relaunch behind an adapter.
The generic HTTPS provider can use the CDN; GitHub is also a public download mirror.

Windows continues to use signed NSIS installers. macOS retains PKG/DMG downloads
and supplies the signed ZIP payload needed by the updater. Test upgrades of
PKG-origin installations and CLI symlinks: replacing the app bundle does not
automatically rerun PKG scripts. Linux DEB/Pacman updates must use the owning
package manager; AppImage updates must preserve the existing installed launcher,
CLI wrapper, and durable copy. No updater may overwrite files behind a package
manager's back or silently switch installation formats.

### Standalone TUI update engine

Add a native update helper that can stage files, wait for the old processes to
exit, apply the verified package, and relaunch the intended frontend. Use
maintained verification/extraction components. Restore terminal modes before
handoff, and keep the helper alive independently of Axiom's normal descendant
cleanup. On Windows, replacement happens only after executable locks are released.

For user-owned standalone installs, use versioned installation directories and a
recoverable activation step so CLI and proxy cannot end up on different versions.
For system packages, delegate to the platform installer/package manager. Persist
only bounded operation state and non-secret restart identifiers. Do not serialize
credentials, conversation text, or an arbitrary shell command into an update job.

Initial qualification covers the existing Desktop target matrix: Windows x64 and
ARM64, macOS x64 and ARM64, and Linux x64. Add standalone CLI/proxy installers for
that matrix as part of this work; the current Linux archive alone does not satisfy
standalone Windows/macOS installation. Publish an artifact only once its install,
upgrade, uninstall, and restart paths have been tested on that target.

### Common lifecycle and recovery

Use explicit states: checking, available, downloading, verified, waiting for
active work, applying, restarting, complete, and failed. Downloading can occur
while work continues; applying requires a quiet installation. Do not kill other
TUI sessions, interrupt inference, or terminate tool/proxy work without the user
choosing that action. Offer waiting or cancellation, and identify sessions that
must close. Do not mark partially received model output as a completed response.

Flush local state before shutdown. Restart restores the selected chat/session,
drafts, workspace, and ordinary preferences; it never replays model requests or
tool calls. Persist a minimal pending-update marker and verify version/health on
launch. Attestation state and runtime-only outdated-TEE consent do not survive
restart; subsequent inference still requires the normal fresh verification path.

An interrupted download must leave the old installation untouched. Interrupted
application must have a documented repair path. Retain an old executable slot
where the installation mechanism supports it, but do not promise universal atomic
rollback for NSIS or system package managers. Never run an older binary against
an incompatible migrated database. Follow [versioning](versioning.md), separate
pre-migration installation recovery from post-migration data recovery, and test both.

## Release contract and trust

Keep `/api/releases/latest` and its schema-1 projection usable by current clients.
The existing Rust parser validates every standalone entry and only accepts Linux
x64 archives, so adding Windows/macOS CLI entries to that feed would break it.
Introduce a separate versioned update contract for expanded products and targets;
do not silently redefine schema 1.

Generate one release inventory after signing/notarization completes. It records
the release version, native source revision, target/product/package identities,
artifact names, sizes, hashes, signature references, and minimum updater/storage
compatibility. Include installers, update payloads, helper files, metadata and
blockmaps in an explicit inventory; the current CDN uploader does not accept all
of these file types. Keep human download choices separate from updater support files.

Authenticate native update metadata with a release-signing key embedded in the
client trust configuration, separate from account credentials and provider TEE
keys. Define key rotation and legacy bootstrap before shipping. Verify platform
publisher signatures where available and bind any selected/downloaded bytes to
the accepted release. Hashes fetched alongside a payload are not independently
proof of its publisher. For the Electron adapter, ensure its own metadata fetch
cannot substitute a different artifact after Axiom accepts the release inventory.

Reject wrong products, architectures, versions, signatures, truncated downloads,
unexpected archive paths, unsupported install roots, and downgrades. Test the
highest accepted release/version policy and bounded metadata lifetime, including
clock errors and refresh of expiring signed channel metadata. Network failure
must not replace a working installation. Update checks/downloads are anonymous;
they carry no account token, prompt, history, or provider secrets.

Use the CDN as the initial update source. Publish identical installer bytes on
GitHub for direct downloads. Automatic CDN-to-GitHub failover is not required for
the first implementation and must not be implemented as an unrestricted redirect.

## Build once, publish to both destinations

Ownership remains explicit:

- `axiomai` owns native update code, helper/installer builds, OS signing,
  checksums/update metadata generation, and its GitHub Release assets.
- `axiom-platform` owns CDN publication, hosted update feeds, the public release
  API, download pages, and links to the GitHub mirror.
- Both repositories consume a documented, versioned release artifact contract.
  Each keeps its own contract fixtures and builds independently; neither imports
  code or tests from a neighboring checkout.

The eventual production release sequence is:

1. Validate a promoted stable tag, matching product versions, required CI results
   for the exact revision, and the complete release target matrix.
2. Build each target once, sign it, and generate metadata from final bytes. Keep
   the existing separate Windows build/sign stages. Include Linux Desktop in
   the coordinated release instead of relying on an unrelated manual follow-up.
3. Assemble and validate the complete inventory. Upload the installers and
   supporting assets to a draft GitHub Release and immutable CDN version paths.
   Transfer the inventory to the platform publisher through release/workflow
   artifacts, with explicit native and platform revision identities.
4. Verify the uploaded bytes at both destinations. Publish the GitHub Release
   and the reviewed website catalog with direct CDN and GitHub download links.
5. Activate the stable updater channels only after the required assets and public
   release are ready. Preserve the legacy JSON feed and generate Electron's
   platform/architecture-specific metadata from the same inventory.

These services do not share a transaction. Record publication progress and make
retries idempotent. Never mark a release ready when assets are missing or replace
an existing version's bytes. If publication stops partway through, leave the old
update channel active until readiness is re-established. During channel activation,
clients may temporarily see either fully available version; they must never be
directed to a partially uploaded release.

Prefer narrow credentials for the cross-repository handoff. Publishing credentials
stay in the owning repository's protected release environment and never reach
client code or ordinary tests. GitHub Release assets must be accessible to the
intended download audience without shipping a GitHub token in Axiom.

## Implementation sequence on dev

Each phase is a reviewable change or coordinated pair of changes targeting `dev`.
Keep this checklist current and update the implementation guides as behavior lands.

- [ ] **1. Freeze contracts and prove installation handoff.** Define the new
  inventory/update schema, install ownership records, helper protocol, trust/key
  rotation, package matrix, and compatibility projections. Prototype Desktop-closed
  bundled-TUI updates and same-terminal relaunch on Windows/macOS/Linux before
  committing to an engine that cannot support them. Record the adapter choice and
  packaging constraints; add independent native/platform contract fixtures.
- [ ] **2. Produce update-capable installers.** Extend packaging for standalone
  CLI/proxy targets, installation markers, the helper, and required Electron
  payloads/metadata. Preserve Windows PATH integration, macOS CLI links, and Linux
  launchers/package ownership. Validate metadata after the final signing step.
- [ ] **3. Implement apply/restart and recovery.** Add staging, authentication,
  installation locking, busy-process coordination, helper lifetime management,
  pending-operation recovery, and post-launch version checks. Prove failure paths
  before wiring a success message into either frontend.
- [ ] **4. Connect Desktop and TUI.** Replace Save As/Show in folder with the
  requested in-app update flow. Add TUI `/update`, CLI apply/check behavior,
  progress/errors, cancellation, and restoration. Bundled TUI delegates to its
  installation owner; standalone TUI updates CLI/proxy as a pair.
- [ ] **5. Coordinate publication and feeds.** Add GitHub Release publishing and
  extend the platform inventory validator/CDN publisher for updater support files.
  Publish both download links, preserve schema 1, add the new feed, and implement
  readiness checks/retry records. Exercise everything against isolated preview
  destinations before any production activation.
- [ ] **6. Qualify upgrades and remove redundant CI work.** Run installed version
  A-to-B upgrades on every supported target/installation type. Factor shared
  protocol/type checks into one job; retain native tests and package/upgrade
  checks where the OS matters. Skip repeated suites only when successful evidence
  belongs to the exact release revision. Keep the live E2EE release gate and
  signing verification. Ensure release packaging still compiles optimized binaries.
- [ ] **7. Merge completed implementation to dev and record readiness.** Record
  tested commits, artifacts, platforms, failure-recovery results, and remaining
  gaps. Update the guides below. Stop at `dev`; a later explicit release request
  controls promotion, stable tags, production publication, and feed activation.

Primary native touchpoints: `apps/desktop/src/main/update-*`, Desktop update IPC/UI,
`apps/axiomcli/src/updates.rs`, TUI command/runtime handling, package/install scripts,
Electron configuration, `.github/workflows/`, and `packages/desktop-releases/`.
Keep frontend-specific code under `apps/`; extract a shared Rust crate only when
there are multiple concrete native consumers of its narrow contract.

Primary platform touchpoints: `deploy/cdn/`, `packages/desktop-releases/`,
`landing/scripts/`, the release catalog/API and download page, and publication
automation. An inference/backend message path is not part of this feature.

Update [installation](installing-desktop.md), [CLI/TUI](cli.md),
[Desktop](desktop.md), [releasing](releasing.md), [versioning](versioning.md),
[privacy](privacy.md), [troubleshooting](troubleshooting.md),
[development](development.md), [validation](validation.md), and affected contribution
rules when their described behavior changes. Update the platform's release,
website/API, and release-acceptance guides in the same implementation phases.

## Acceptance and evidence

| Scenario | Required evidence |
|---|---|
| Desktop update on Windows x64/ARM64, macOS x64/ARM64, Linux x64 | Install A, select Update, download/verify B, install and relaunch B; no manual installer launch |
| Bundled TUI, Desktop open and closed | Update the entire package once; return to the same terminal/session; preserve the app/CLI version match |
| Standalone TUI/proxy on the qualified matrix | Upgrade through the owning installer/helper; restart TUI; keep proxy and CLI versions paired |
| Package origin/scope | Exercise Windows per-user/system scope where supported, macOS PKG/DMG origins, Linux DEB/Pacman and installed AppImage; preserve PATH/links and uninstall ownership |
| Active work and concurrent update requests | Wait/cancel clearly; no duplicate installer, forced loss of another session, tool replay, or false inference completion |
| Network/download/verification failures | Disconnect, cancel, wrong digest/signature/product/arch, expired metadata, replay/downgrade, and full disk leave the working installation intact |
| Installation/restart failures | Exercise denied elevation, locked binaries, installer failure, shutdown interruption, stale operation markers, failed launch, and supported repair paths |
| Data compatibility | Preserve credentials, drafts and history; test migrations; never automatically roll back into an incompatible schema |
| Publication | CDN and GitHub serve identical verified installer bytes; missing target, upload failure, retry, immutable-version conflict, and premature activation are rejected |
| Bootstrap and channels | Existing clients can install the first updater-enabled version; preview clients cannot consume/modify stable channels; subsequent updates use the new in-app flow |

Unit/contract tests cover selection, parsing, state transitions, signatures and
publication invariants. Integration tests cover real staging/apply helpers,
package managers, and failure recovery. PTY tests cover terminal restoration and
session resume. OS installer tests use two real packaged versions, with signing
appropriate to the target; browser mocks alone cannot establish update success.
Use isolated staging feeds and test identities, with no live paid inference needed
for updater tests. Record actual OS/architecture/signature evidence in `docs/testing/`
and link it from [validation](validation.md).

## Bootstrap and eventual release

The first release containing this implementation still needs installation through
today's download/manual-install flow. Maintain that compatibility path on both
download hosts. Test an existing schema-1 client discovering and installing the
bootstrap release, then the bootstrap release automatically upgrading to a second
version. Do not choose a release version in this plan; use the coordinated product
version selected when implementation is ready.

Development uses isolated preview destinations and distinct trust/channel settings.
Add a preview packaging/test lane as needed, document its trigger rules, and keep
it separate from the production stable-tag checks. No change in this plan loosens
the requirement that published production installers come from promoted `main`.

Production readiness requires all phases and applicable acceptance rows to be
complete, or a documented reduction of the advertised platform matrix. Merging
the plan or implementation into `dev` is not evidence of a shipped updater.

## References

- [electron-updater for the current electron-builder v26 line](https://www.electron.build/v26/docs/features/auto-update/).
- [Publishing configuration and update metadata](https://www.electron.build/v26/docs/publish/).
- [Current platform CDN/catalog publication](https://github.com/astrea-foundation/axiom-platform/blob/dev/docs/operations/releases.md).
