# Release procedure

Development lands on `dev`. A public release requires the explicit promotion
workflow in [CONTRIBUTING](../CONTRIBUTING.md), a stable `vMAJOR.MINOR.PATCH` tag
on promoted `main`, and matching Desktop/AxiomCLI versions. A version bump or
push to `dev` does not publish installers or activate the update feed.

The [update architecture](updates.md) describes installation ownership and
recovery. [Release acceptance](validation.md) covers signed OS qualification;
[repository setup](repository-setup.md) lists Actions credentials and settings.

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

Pull requests targeting `main` run Linux Rust and Desktop UI checks. Merging does
not repeat those checks automatically. Native macOS/Windows validation is an
explicit `ci.yml` dispatch with `target=native` (all four native jobs), `all`
(also Linux/UI), or a specific platform; `linux` and
`desktop` rerun only those checks. Validate the complete candidate on `dev`,
promote it, confirm the promoted tree matches, and tag that validated candidate
commit now reachable from `main`. The release gate requires successful Rust, UI,
and all four native CI jobs for the exact tagged revision, combining targeted
runs when needed. Packaging reuses those results instead of repeating the suites.
Retry only failed validation targets or packaging jobs. These checks do not
replace signed OS installation acceptance.

Routine qualification runs functional tests once. The repeated deterministic
evaluation and three host-performance benchmarks are opt-in with
`extended=true`; see [evaluations](evaluations.md#routine-checks-and-opt-in-extended-lane).
Small persistence, cancellation, process cleanup, protocol and security tests
stay required. Clippy already checks all Linux targets, and native tests compile
their targets, so CI does not precede either with a duplicate `cargo check`.
CI omits development/test debug symbols, retains assertions, downloads the pinned
`cargo-deny` executable with checksum verification, and limits a dispatched native matrix to two concurrent
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

The **Preview installers** workflow can run on `dev` without production secrets:

```sh
gh workflow run desktop-packages.yml --ref dev
```

It builds actual unsigned installers into Actions artifacts, with stable updates
disabled. It does not publish GitHub releases, CDN objects, or feeds. Local builds
use the same native builders:

```sh
cargo build --locked --release -p axiomcli -p axiom-proxy
node apps/desktop/scripts/package-desktop.mjs --linux --x64 --unsigned
node scripts/package-cli.mjs --linux --x64 --unsigned
```

Use `--mac --arm64|--x64` or `--win --arm64|--x64` on native hosts. Windows needs
NSIS 3.11 and the Visual C++ runtime files. Linux packaging uses Ubuntu 22.04,
`libarchive-tools`, `rpm`, FUSE and Bubblewrap. The builders verify native binary
architecture/version and include project and Rust dependency license notices.
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
