# Release procedure

Development lands on `dev`. A public release requires the explicit promotion
workflow in [CONTRIBUTING](../CONTRIBUTING.md), a stable `vMAJOR.MINOR.PATCH` tag
on promoted `main`, and matching Desktop/AxiomCLI versions. A version bump or
push to `dev` does not publish installers or activate the update feed.

Compile and test Windows development changes in the Windows VM from `dev`, using
the current version for private previews. Qualify installer changes there before
preparing a version bump or promoting a release. Do not use `main`, stable tags
or cross-platform release builds as a development test loop: those actions are
reserved for an explicitly authorized public release. A fix found during testing
returns to local VM qualification before another release is considered.

The [update architecture](updates.md) describes installation ownership and
recovery. [Release acceptance](validation.md) covers signed OS qualification;
[repository setup](repository-setup.md) lists Actions credentials and settings.

The optional managed Chutes provider uses a prebuilt standalone client payload.
Set `AXIOM_CHUTES_BUNDLE` to its target-specific directory and explicitly set
`AXIOM_CHUTES_RELEASE_CHANNEL=staging` for staging; production is the default.
The packaging hook validates its exact inventory, native architecture, public
authority and executable pin before copying it to `resources/chutes`. Signed
macOS packages require the payload's native code to be signed by `APPLE_TEAM_ID`
before its inventory is generated. Electron excludes that runtime from its own
signing walk to preserve the executable hash. Use the
[helper build guide](../../axiom-inference/docs/native-helper.md), including
dependency notices, and retain signed installer acceptance as a separate check.
An ad-hoc payload fails signed packaging. Windows and universal helper bundles
are not yet qualified; omit the optional provider for those targets.

## Supported platforms

| Target | Desktop installer | Standalone CLI + proxy installer |
|---|---|---|
| Windows x64 / ARM64 | Combined NSIS EXE | Combined NSIS EXE, per-user |
| macOS Intel / Apple Silicon | Universal PKG | Universal PKG |
| Linux x64, glibc 2.35+ | DEB, Pacman, AppImage | Self-extracting SH, per-user |

Each Mac PKG contains universal Mach-O binaries for Intel and Apple Silicon.
Each Windows EXE contains x64 and ARM64 payloads and selects the native one at
installation. There is one download per product for each of these operating
systems; Linux keeps its three Desktop formats and standalone SH installer.

Native jobs record version, source revision, update key and hashes before
uploading inputs. Windows inputs also carry their native Visual C++ runtime.
Packaging verifies both receipts before combining or signing anything. Failed
packaging can reuse the retained native inputs for seven days. It does not
rebuild Rust or repeat successful validation jobs.

All Desktop packages contain their matching CLI. No bundled CLI is updated
independently. Linux ARM and musl are not public targets. AppImage requires FUSE;
Linux local tool execution requires Bubblewrap. macOS/Windows approved host tools
retain the boundaries documented in [the threat model](threat-model.md).

## Update trust and release setup

Use a dedicated Ed25519 update-signing key, separate from account, inference,
Apple and Windows signing keys. Store its PKCS#8 PEM as
`AXIOM_UPDATE_SIGNING_KEY` in the native protected `release` environment.
Set repository variable `AXIOM_UPDATE_PUBLIC_KEYS` in both repositories to the
comma-separated, lowercase 32-byte public keys in hex. Native release builds
compile this list; packaging checks it before shipping. Never use the committed
signature-test fixture key for a release.

Generate and back up the key through the normal maintainer secret-management
process. No private key is checked in or provisioned by the development task.
For rotation, first publish clients that trust both keys, then change the signer.
Retain old public keys while installed clients or remembered signed inventories
still require them. Removing a trusted key without that overlap fails closed.

The source repository publishes signed installers in its Releases tab. The
platform publisher also uses the public `astrea-foundation/axiom-releases`
distribution repository for installer assets and signed metadata. Its tag is a
distribution label; the signed `revision` identifies the promoted native source
commit. Repository names and access must agree across both publishers.

Configure protected release credentials:

| Repository/environment | Credential | Required access |
|---|---|---|
| native `release` | `AXIOM_UPDATE_SIGNING_KEY` | Ed25519 private PEM |
| native `release` | `AXIOM_RELEASES_TOKEN` | Contents write on the public distribution repository only |
| native `release` | `AXIOM_PLATFORM_RELEASE_TOKEN` | Actions write on `axiom-platform`, to dispatch `release.yml` on `main` |
| platform `release` | `AXIOM_RELEASES_TOKEN` | Contents write on the distribution repository, to read drafts and publish them |
| platform `release` | `AXIOM_NATIVE_READ_TOKEN` | Contents read on `axiom-desktop`, for source-tag ancestry verification |
| platform `release` | `CLOUDFLARE_API_TOKEN` | Existing account/zone validation, CDN object write and website deployment |

The platform workflow also records the published catalog on `main` using its
repository token. Configure release-bot access consistent with branch protection;
ordinary development remains on `dev`. Bring the generated release catalog back
to `dev` with the normal post-release `main` merge. Tokens never enter installers.

macOS uses the protected `release` secrets `MACOS_CERTIFICATE_BASE64`,
`MACOS_CERTIFICATE_PASSWORD`, `APPLE_ID`, `APPLE_APP_SPECIFIC_PASSWORD`, and
`APPLE_TEAM_ID`. The certificate bundle must contain Developer ID **Application**
and **Installer** identities. The workflow uses a temporary keychain, signs the
code and PKGs, submits notarization and staples/verifies tickets. Desktop's
packaging script disables electron-builder's separate app notarization and
submits the final PKG once, including its signed app payload. This follows
[Apple's outermost-container guidance](https://developer.apple.com/documentation/xcode/packaging-mac-software-for-distribution)
and avoids an extra notarization queue wait for each product. The standalone
CLI's separate PKG still requires its own notarization and stapled ticket.

Windows uses the existing protected `windows-signing` environment and Azure OIDC:
`AZURE_CLIENT_ID`, `AZURE_TENANT_ID`, `AZURE_SUBSCRIPTION_ID`, and
`AXIOM_SIGNING_ENDPOINT`, `AXIOM_SIGNING_ACCOUNT`, `AXIOM_SIGNING_PROFILE`,
`AXIOM_SIGNING_PUBLISHER` variables. Signing runs on x64 with .NET 8 and pinned
`ArtifactSigning` 0.1.20; ARM64 binaries are built on a native ARM64 runner first.
All payload code, uninstallers and final NSIS installers require valid timestamped
Authenticode signatures from the expected publisher. No unsigned fallback exists.
The signing host installs and uninstalls both combined packages on x64. A short
ARM64 job repeats installation, checks native PE architecture and executes the
installed CLI to verify version and update trust. These jobs reuse signed bytes;
they do not compile Rust or repeat the functional suites.

## Build and publish

The tag-triggered [release workflow](../.github/workflows/release.yml):

1. Checks `main` ancestry, stable version, completed CI for the tagged revision,
   and the existing budgeted live E2EE gate.
2. Builds Linux Desktop/CLI and native macOS/Windows x64/ARM64 inputs. One
   signing job per OS combines the two architectures into Desktop and CLI installers.
3. Collects the complete 8-installer matrix. `download-manifest.mjs` hashes
   final signed/notarized bytes, writes `SHA256SUMS`, and signs `manifest.json`.
4. Publishes signed installers and checksums in `axiom-desktop`’s Releases tab using
   the workflow token, then creates or verifies the public distribution draft.
5. Dispatches the platform publisher on its `main` branch with native version
   and revision. Its separate run owns CDN verification and channel activation.

Windows native x64 builds also produce the static setup input once. The existing
Windows signing job verifies its input receipt, signs it, and checks native GUI
initialization and executes the bundled PowerShell helper without networking.
ARM64 qualification runs that same x64 setup
through emulation. No new scheduled workflow or duplicate platform build is added.

Setup is retained separately from the app inventory. Its immutable name is
`AxiomSetup-SETUP_VERSION-REVISION_PREFIX.exe`; signed `windows-setup.json` binds
its full source revision, associated app release, publisher, length, hash and
exact CDN/GitHub URLs. A new app release gets its own immutable setup path even
when the setup component version stays unchanged. Keep the schema-3 eight-target
app inventory unchanged. The final-byte signing job generates setup metadata with
the same protected Ed25519 identity as app updates, after Authenticode signing.

The retained `axiom-signed-release` Actions artifact has `release/` and `setup/`
subdirectories. For operator retry, pass `AXIOM_SETUP_DIRECTORY=setup` when
running `publish-github-release.mjs release [--source]`. Both public release tabs
carry the setup EXE and `windows-setup.json` beside the app installers. The
platform publisher verifies both metadata records and every byte before activating
`/download/windows` and `/api/installers/windows`; it keeps the offline download.
The optional CDN aliases follow mirror verification and website activation.

For signed-package qualification before public activation, set repository
variable `AXIOM_RELEASE_BUILD_ONLY=true` before tagging. The complete build,
signing, native installation checks, signed inventory and Store handoff still
run, while source publication, distribution uploads and platform dispatch are
held. Download `axiom-signed-release`, qualify those exact bytes, then publish
the retained `release/` and `setup/` through the operator commands above and
dispatch the platform publisher on `main`. Clear the variable after publication.
Do not rebuild or resign qualified files for this handoff.

Exercise a model-generated image tool call from the retained signed installation
without configuration-file environment overrides. The installed secure provider
must discover its bundled AstreaAI configuration and register `generate` and
`view_image` from that loaded configuration. Verify the authenticated image result and settled usage
before publishing; version and ordinary chat checks cannot substitute for this
test.

The macOS signing job installs each signed PKG on its disposable runner and runs
`scripts/verify-installed-image.mjs` against the installed CLI. An isolated profile
and a fresh native-session QA credential keep this check separate from
maintainer accounts. Before dispatch, provision `AXIOM_INSTALLED_NATIVE_QA` in the
protected `release` environment using a dedicated funded QA account and the normal
native authorization/PKCE exchange. Its JSON contains `version`, `accountId`,
`clientId`, `refreshToken`, and `sessionId`. An automation API key cannot access
native account sessions and cannot substitute for this credential.

The harness accepts only a disposable GitHub-hosted macOS runner, creates a
separate temporary Keychain, makes it the runner's default User-domain Keychain,
and installs that session's client identity in each
isolated profile. Desktop and standalone CLI share the rotating Keychain entry;
the original refresh token is seeded once, never restored after rotation. If a
run consumes the token before failing, authorize a fresh QA native session before
another dispatch. Revoke the QA session and remove the temporary secret after
qualification. Never seed this fixture into a maintainer's login Keychain.
The child process inherits no provider configuration or test
overrides. It must register image tools, generate and view one image, finish ready,
and report authenticated, final, settled image usage. The private
`signed-installation-image-acceptance` artifact binds the installer and installed
binary hashes to the generated image and usage receipt. Publication depends on
this check; downloading the signed inventory does not replace it.

Before selecting the AstreaAI image-test model, qualification must use the normal
installed catalog and a fresh profile without `AXIOM_MODEL`. Require NEAR's exact
`deepseek-ai/DeepSeek-V4.1-Flash-1017-abliterated` offering, its `uncensored` tag,
and that offering as the new session's selected default. Use the Desktop chat
frontend and bootstrap workspace for Desktop, and the CLI frontend for standalone
CLI. Only after recording the actual default may the test select AstreaAI/Qwen
through the normal model-selection RPC for its image request. A working image
service cannot substitute for the required chat catalog and default selection;
provider discovery outages must block release qualification.

`scripts/prepare-store-submission.mjs release setup store` prepares the private
`windows-store-submission` workflow artifact: the exact full offline EXE URL,
hash, silent arguments, certification notes and remaining acceptance fields.
This packet is not a Store submission or certification result. Partner Center
identity/listing/reviewer fields and signed first-launch/offline OS acceptance
remain maintainer requirements in [release acceptance](validation.md). The
website downloader is not eligible for the Store's offline EXE package slot.

To build a local unsigned setup preview on x64 Windows:

```sh
node scripts/package-setup.mjs --unsigned
```

Production setup builds require `AXIOM_UPDATE_PUBLIC_KEYS` and the existing
Windows signing publisher. `--build-only` creates a receipt-bound input for the
protected signing job; `--input DIRECTORY` signs that exact captured input.
Unsigned previews compile no stable-feed trust and cannot install a stable app.
Packaging embeds the project license and locked Rust dependency notices in the
standalone EXE; its Licenses button opens that bundle without networking. Stable
setup builds fail if the bundle is missing, and native signing checks its presence.
The website setup requires Windows 10+ on x64 or Windows 11 on ARM64.

The manifest's `sequence` defaults to the native commit timestamp. It must
increase between versions; a deliberate `AXIOM_RELEASE_SEQUENCE` override is
available to the generator. `revision` defaults to `git rev-parse HEAD`.
Equal version or sequence with changed metadata is rejected by clients. Use a
new version for any artifact correction; never rebuild over an existing release.

Both products consume `https://axiom.stream/api/releases/latest`. The schema-3
manifest uses `downloads` for Desktop and `cliDownloads` for standalone installers.
Mac and Windows records use `arch: "universal"`; Linux uses `arch: "x64"`.
The current updater selects a universal installer on either supported native
architecture. There is one feed, with no bridge release or compatibility aliases.
Schema-2 parsing remains solely for already-published signed release records.
See [update architecture](updates.md).
Each record contains product, platform, architecture, format, name, length,
SHA-256, and exact CDN and public GitHub asset URLs. Canonical signing recursively
sorts object keys and uses compact UTF-8 JSON without the `signature` field.
The native and platform repositories each check in their own contract and vectors.

Coordinate publication retries with the platform maintainer. The publisher
verifies all bytes before changing the public feed and records
`release-publication.json` as a workflow artifact. An Actions build-input ZIP is
never linked as an installer. Blockmaps and Electron support files are not part
of this updater's public contract; the native helper uses the complete installer.

## CI baseline

### Completing unpublished packaging from retained inputs

The one-time `release-0.1.21-recovery.yml` workflow uses the reviewed
`v0.1.21-packaging.2` acceptance tag to correct the installed-image acceptance
authentication. It retains native source/tag
`0cea3d47db3a75c1cab7495af930bc864604a6fc` and the accepted Windows/Linux bytes from
run `42696233901`. The guard requires that exact original attempt's successful
native builds, live evaluation, Windows signing and ARM64 installation, and Mac
signing/notarization; only its failed image acceptance and skipped publisher are
recoverable. Signed Mac packages from assembly run `42709923710` are retained
unchanged after correcting the fixture's default User-domain Keychain selection.
That exact run's source qualification, signing and notarization must have passed
and publication must have been skipped. All platforms reuse their retained signed
installers. Both installed Mac products must then generate and view
an authenticated image using their bundled configuration and native session.
The receipt records the native source revision and separate acceptance revision.
Publication still requires this gate and the complete signed inventory. No
published assets or stable tags are replaced.

The one-time 0.1.17 recovery completed with assembly tag
`v0.1.17-packaging.3`. Its signed public inventory still identifies the qualified
native source `7c41a12b31d42aefe4f21896864a166f0d087b3a`. Preserve the published
bytes; publication retries reuse the retained signed release or public assets.
Future installer corrections require the normal new-version release procedure.
The recovery details below document the assembly provenance.

The manual `release-packaging-recovery.yml` workflow completes the unpublished
0.1.17 installer set from its exact qualified native source and retained build
receipts. Dispatch the reviewed `v0.1.17-packaging.3` tag after its workflow is
promoted to `main`. This tag records assembly recovery and is excluded from the
normal stable-tag build trigger; the native `v0.1.17` tag remains unchanged.

The recovery gate requires the original live evaluation, Linux packages, both
Mac native inputs, both Windows native inputs, signed Windows installers and
Windows ARM64 installation acceptance to have passed. It refuses an active or
completed original publisher. Native receipt revisions, hashes, architecture,
compiled update keys and exact-source CI stay mandatory.

Mac assembly runs on Intel capacity. A separate checkout of the reviewed
assembly tag supplies the packaging hooks while the app and native source
checkout stays unchanged. The temporary x64/ARM64 app inputs omit frozen
runtimes: Electron rewrites nested `Info.plist` files and would invalidate their
recorded hashes and signatures. The final universal hook copies both complete
signed runtime trees after the merge and before app signing, with the existing
inventory, architecture, signature and no-replacement checks. Ordinary thin
builds and standalone CLI packaging retain their normal strict copies. App code
and Rust sidecars retain normal universal merging. Recovery repeats code signing,
notarization, stapling and installer assessment, reuses the already accepted
Windows/Linux bytes, and signs the complete final inventory before publication.
The inventory continues to identify the exact qualified native source; the
recovery workflow tag records the assembly configuration. This procedure does
not replace bytes or metadata in a published release.

### Qualifying a candidate

Pushes to `dev` and pull requests targeting `dev` or `main` run Linux Rust,
TypeScript/unit/browser checks and all four native macOS/Windows portability
jobs. Manual `ci.yml` dispatches can retry `target=native`, `all`, a specific
native platform, `linux`, or `desktop`. After those checks succeed for the exact
development revision, use the non-publishing candidate workflow below to catch
bundled-verifier and installer packaging failures before production tags exist.
Retry only failed validation targets or candidate packaging jobs.

A candidate run does not authorize promotion or publication. For a separately
authorized release, validate the complete candidate on `dev`, promote it, confirm
the promoted tree matches, and tag the validated commit now reachable from
`main`. The production gate still requires successful Rust, UI and all four
native CI jobs for the exact tagged revision, combining targeted runs when
needed. Candidate-target selection does not relax that requirement. Checks and
unsigned packaging do not replace signed OS installation acceptance.

Routine qualification runs functional tests once. The repeated deterministic
evaluation and three host-performance benchmarks are opt-in with
`extended=true`; see [evaluations](evaluations.md#routine-checks-and-opt-in-extended-lane).
Small persistence, cancellation, process cleanup, protocol and security tests
stay required. Clippy already checks all Linux targets, and native tests compile
their targets, so CI does not precede either with a duplicate `cargo check`.
CI omits development/test debug symbols, retains assertions, downloads the pinned
`cargo-deny` executable with checksum verification, and limits the native matrix to two concurrent
jobs. Release build profiles and signing checks are unchanged.
The audit installer action is pinned to a commit and disables source-build
fallbacks, so a fresh PR does not spend minutes compiling the audit tool itself.

The release build retains the complete signed `axiom-signed-release` artifact for seven days
before cross-repository publication. If the narrow upload or dispatch credentials
are not configured, the job summary records that operator publication is still
required; a successful build alone does not mean the release is public. An
authenticated operator can download that artifact, set `AXIOM_UPDATE_PUBLIC_KEYS`
to the release public key, and run `node scripts/publish-github-release.mjs DIRECTORY`
from the promoted native checkout. Follow the platform publication guide to
verify both mirrors and promote the catalog. Do not rebuild installers merely to
retry publication, and do not copy a broad personal token into Actions.
New drafts are resolved through the authenticated release listing, since GitHub's
release-by-tag endpoint resolves published releases. Retries resume the same draft
and preserve the immutable artifact checks.

Branch checks use Ubuntu 24.04 with Python 3.12 and permit Bubblewrap user
namespaces on the disposable runner. Release binaries retain Ubuntu 22.04 for
glibc 2.35; the release/preview jobs install Python 3.12 explicitly. Rust test
concurrency stays at two, and failure caches are retained. Test credential
deadlines retain their scheduling headroom; production deadlines and runtime
sandboxing are unchanged. Git attributes preserve LF across Windows checkouts.

## Development previews

The **Candidate validation (unpublished)** workflow runs explicitly on a
non-`main` development branch. It rejects tag dispatches, retains the existing
product version and requires successful `ci.yml` jobs for the exact revision.
Select one native target for a focused packaging check; `all` requests all five
native installer targets, with at most two jobs running together:

```sh
gh workflow run desktop-packages.yml --ref dev -f target=linux
# Other targets: macos-intel, macos-arm64, windows-x64, windows-arm64, all
```

Every selected target requires the Rust and Desktop UI CI jobs; macOS/Windows
also require their matching native portability job. A missing, active, cancelled
or failed required result blocks packaging. Run or retry CI first; this workflow
does not dispatch CI on the operator's behalf.

The workflow builds and self-tests the native AstreaAI verifier through
`scripts/build-direct-bundle.mjs`, bundles it into actual unsigned Desktop and CLI
installers, and compiles with stable update trust disabled. Each packaging job
installs Node for its selected architecture and checks `process.arch` before
building, including native ARM64 on macOS and Windows. It validates that the
unsigned inventory matches the selected source SHA, existing product version,
platform and architecture, includes every required Desktop/CLI format, and
matches each artifact's length/hash and `SHA256SUMS`. A source-tree check rejects
uncommitted packaging changes. The resulting `candidate-SHA-PLATFORM-ARCH` Actions
artifact contains the installers, inventory, checksums and
`candidate-validation.json` receipt, retained for seven days. The token has only
read access to repository contents and Actions. No signing secrets, release
upload, CDN upload, feed change, production inference or deployment is involved.

Local unsigned packaging uses the same native builders from a clean, committed
checkout. Confirm the existing Desktop and CLI versions match first:

```sh
export AXIOM_UPDATE_PUBLIC_KEYS=''
node scripts/build-direct-bundle.mjs --output .local/direct/bundle
export AXIOM_DIRECT_BUNDLE=.local/direct/bundle
cargo build --locked --release -p axiomcli -p axiom-proxy
node apps/desktop/scripts/package-desktop.mjs --linux --x64 --unsigned
node scripts/package-cli.mjs --linux --x64 --unsigned
version=$(node -p "require('./apps/desktop/package.json').version")
node scripts/download-manifest.mjs apps/desktop/dist "$version" unsigned
node scripts/verify-candidate.mjs apps/desktop/dist linux x64 "$(git rev-parse HEAD)" "$version"
```

Use `--mac --arm64|--x64` or `--win --arm64|--x64` on native hosts. Windows needs
NSIS 3.11 and the Visual C++ runtime files. Linux packaging uses Ubuntu 22.04,
`libarchive-tools`, `rpm`, FUSE and Bubblewrap. The builders verify native binary
architecture/version and include project and Rust dependency license notices.
The Desktop NSIS include is compiled separately for installer and uninstaller
by electron-builder's bundled compiler. Guard installer-only global variables
with `!ifndef BUILD_UNINSTALLER`; unused variables fail packaging because warnings
are errors. Check both compilation modes when changing this include.
Standalone macOS and Desktop both register `axiomcli`; an unrelated existing
command is preserved and must be resolved before switching owners.

## Acceptance and recovery

Run fresh install, A-to-B update, same-terminal TUI resume, GUI reopen, cancel,
locked-process waits, denied elevation, interrupted installation and uninstall
on every advertised OS/architecture/package. Preserve account data, drafts,
workspace and session; reset runtime-only outdated-TEE consent. Verify OS
publisher signatures and test with real signed installers, not just mocks.
Record results in [validation](validation.md). Current local tests do not establish
Windows/macOS signed installation acceptance.

The native helper checks the installed CLI's exact version before relaunch. This
is a headless startup check, not proof that every GUI or account feature works.
An installer failure is recorded for the next app check. Re-run the matching
verified installer to repair files. Linux standalone retains old version slots;
never manually reactivate an older version against incompatible account data.
Developer archive scripts support reproducibility work; production downloads
and updates use the installers listed in the signed inventory.

The source repository's Releases tab is populated after successful signing and
inventory verification, even when cross-repository publication needs an operator.
`node scripts/publish-github-release.mjs DIRECTORY --source` resumes this step,
checks every existing asset and publishes only a complete draft. Failed or
superseded build tags are not advertised as releases. Public website/feed
activation remains the platform publisher's responsibility.
