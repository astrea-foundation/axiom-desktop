# Release procedure

Tagged releases build the reproducible AxiomCLI Linux archive and native Axiom
Desktop packages for macOS Intel, macOS Apple Silicon, Windows x64, and Windows
ARM64. A pushed release tag must be a stable `vMAJOR.MINOR.PATCH` with no
prerelease or build suffix, and the version after `v` must match
`apps/axiomcli/Cargo.toml`. This keeps the product version valid for macOS bundle
and Windows executable metadata. The tag must point to a commit already promoted
to `main`. The workflow validates this ancestry and version contract before the
budgeted live evaluation and before any packaging job. Desktop's source manifest
is synchronized with AxiomCLI by the packaging script before invoking
electron-builder.

## Unified release catalog

The next product release is **0.1.2**. Existing public 0.1.0/0.1.1 packages predate
the updater and need one manual upgrade. A version bump in source does not publish
installers or change the public feed.

Gather the Desktop installers and the standalone Linux archive from the same
promoted tag into a clean directory, including the archive's `.asc` signature
when signed. Run `node scripts/download-manifest.mjs DIRECTORY VERSION unsigned`
(or `signed` only when the complete release is signed). The generator emits one
schema-1 manifest, combined `SHA256SUMS`, and the CLI archive's adjacent `.sha256`.
It rejects mixed versions and unsupported CLI archive variants before writing metadata.

The additive `cliDownloads` list contains the Linux x64 `tar.gz` bundle;
`downloads` remains exclusively Desktop installers so older updaters continue to
work. Both lists use the same version and immutable CDN paths. Empty/absent CLI
lists preserve historical desktop-only releases. Publish and import that combined
manifest through `axiom-platform`'s CDN and website pipeline; both repositories
keep identical contract code and JSON test vectors without cross-repo imports.
The publisher includes the CLI checksum and available signature sidecars. Promoting
the CDN alone does not update the public release API.

## Supported platforms

These are checked-in build targets, not a claim about the current public catalog
or a completed real-machine acceptance run.

| Target | Desktop package formats | Minimum/build constraint |
|---|---|---|
| Linux x86-64 | AppImage, DEB, Arch `pkg.tar.xz` | Release build on Ubuntu 22.04; glibc 2.35+ |
| macOS Intel and Apple Silicon | PKG, DMG, ZIP | Separate x64/arm64 binaries; configured minimum macOS 11 |
| Windows x64 and ARM64 | NSIS EXE | Matching native sidecar and Visual C++ runtime |

Every Desktop target bundles AxiomCLI. The standalone signed archive targets
Linux x86-64 GNU and includes both `axiomcli` and `axiom-proxy`. Linux ARM and
musl are not in the release matrix. Source build support is not a publication
promise.

Desktop starts in a restricted Web profile and can negotiate per-thread Agent
mode. CLI and Desktop local tools run on the supported operating systems, with
different process boundaries: Linux requires Bubblewrap; macOS/Windows currently
run approved processes on the host with a sanitized environment. They do not
provide Linux's filesystem/network isolation. See
[local tool security](threat-model.md#local-tools-and-processes).

Accounts use the OS credential vault and account/frontend-scoped native data
paths on each platform. No plaintext credential-file fallback is supported.

## Desktop and terminal CLI installers

Pushes to `main` and pull requests targeting `main` run checks and tests without
producing release archives or packaged Desktop applications. Development branches
need local checks; see [development](development.md#checks). Release packaging
and upload happen only in the release workflows.

The `desktop installers` workflow (`desktop-packages.yml`) builds Linux x64,
macOS Intel/Apple Silicon, and Windows x64/ARM64. Run it manually against a stable
release tag already promoted to `main`; a branch dispatch skips packaging. For
example, after promoting and tagging version `0.1.0`:

```sh
gh workflow run desktop-packages.yml --ref v0.1.0 -f unsigned=true -f platform=all
```

The manual workflow currently defaults `unsigned` to `true`; set it explicitly
for a reproducible invocation. Signed builds need `unsigned=false` and the
relevant credentials. This does not change the signed defaults in
`electron-builder.yml` or the mandatory signing in the pushed-tag workflow.
The manual lane does not run the signed workflow's live-provider gate.
After pushing a signed release tag, dispatch the manual workflow with
`unsigned=false` and `platform=linux` to add Linux Desktop installers. The tag
workflow already builds the standalone Linux archive and signed macOS/Windows
installers; the Linux-only dispatch avoids repeating those native builds.

The manual packaging job uses the protected `release` environment, including for
unsigned builds. Allow the intended `v*` release tags in that environment's
deployment rules. Signed macOS builds read the five Apple secrets listed below
from `release`; Apple credentials are passed only to signed macOS packaging.
Signed Windows builds use the separate `windows-signing` environment.

Every desktop package includes the complete matching AxiomCLI binary. The
packaging script checks its architecture and version before packaging.
User-facing steps are in [Install Axiom Desktop and AxiomCLI](installing-desktop.md).
Windows packages include the matching Visual C++ runtime DLLs beside the CLI;
the build machine needs Visual Studio's C++ tools and redistributable files.

| Platform | Downloads | Terminal CLI installation |
|---|---|---|
| Windows | NSIS EXE | Installer adds its resources/bin directory to PATH for the selected user/machine scope; open a new terminal. Uninstall removes only that entry. |
| macOS | PKG, DMG, ZIP | Recommended PKG installs Axiom.app and /usr/local/bin/axiomcli. DMG/ZIP users copy Axiom.app to /Applications, then choose Axiom → Install Command-Line Tool. |
| Linux | AppImage, DEB, Arch pkg.tar.xz | DEB/Arch own /usr/bin/axiomcli. Run the AppImage with --install to install the app and CLI for the current user. |

Build on the target OS after installing Rust, Node and pnpm:

```sh
cargo build --locked --release -p axiomcli
node apps/desktop/scripts/package-desktop.mjs --linux --x64 --unsigned
# Native macOS: --mac --arm64 (or --x64)
# Native Windows: --win --x64 (or --arm64)
```

Linux Desktop and standalone CLI release builds use Ubuntu 22.04 so the CLI does not acquire newer glibc
requirements from a rolling-release workstation. Linux ARM is not in this
release matrix. Install `libarchive-tools` on the Ubuntu build machine: the
Arch packaging step requires `bsdtar`. The Arch package installs with `sudo pacman -U FILE.pkg.tar.xz`;
the DEB installs with `sudo apt install ./FILE.deb`.

AppImage users run `chmod +x FILE.AppImage` followed by `./FILE.AppImage --install`.
This installs a durable copy under `~/.local/share/axiom-desktop`, desktop menu
metadata, and `~/.local/bin/axiomcli`; it registers that directory in common
shell profiles. Open a new terminal afterward. Repeat --install on a newer
AppImage to replace the program while retaining account data. For portable
use, `./FILE.AppImage --axiom-cli --help` runs the bundled CLI directly. The
custom AppRun never silently disables Electron's sandbox.

The macOS PKG requires the standard administrator installation authorization.
It refuses to overwrite an unrelated /usr/local/bin/axiomcli. Unsigned preview
apps are ad-hoc signed for executable integrity, with the Electron JIT/library
entitlements; they are not Developer-ID signed or notarized. Users must approve
the app/installer in Privacy & Security where the OS permits it. Windows may
show SmartScreen warnings or block unsigned code under stricter policies.
For signed PKG releases the certificate bundle must also contain a Developer ID
Installer certificate, in addition to the Developer ID Application certificate.

Keep appId `stream.axiom.desktop`, product name `Axiom`, Linux package name
`axiom-desktop`, installation scope, and native account-data paths stable.
Increment the embedded product version for each public release. Exit the app
and CLI before an upgrade, and use the same installation format. Switching
between AppImage and a system package requires removing the old program
installation to avoid duplicate launchers. Installers never remove account data; schema changes
still require compatible application migrations.

The manual workflow exercises installed CLI startup and reinstall behavior on
native macOS runners and a DEB installation on Ubuntu. Windows packaging does
not install or smoke-test generated installers on additional runners. Its signed
lane verifies executable architecture/version and Authenticode publisher and
timestamp checks as part of signing. Minimal Ubuntu and Arch
containers verify the declared CLI and Electron runtime dependencies. The
AppImage installer tests cover argument forwarding, replacement, command
collisions, and preserved account data. GUI/credential-vault/inference acceptance
remains a separate check.

## Public CDN publication

Collect the workflow's installer files into one clean directory, then generate
the combined manifest and checksums:

```sh
node scripts/download-manifest.mjs /path/to/installers 0.1.0 unsigned
```

The companion API repository has `deploy/cdn/cdn.mjs` to provision the R2 bucket
`axiom-cdn` at `cdn.axiom.stream` and publish this directory. Versioned installers
live at `/downloads/<version>/<filename>`. `/downloads/latest.json` is promoted
only after all listed files have been uploaded and fetched with matching hashes.

Then publish the website's release catalog from the `axiom-platform` repository,
which owns the website, account app, CDN uploader and release API. Follow its
[CDN and catalog publication guide](https://github.com/astrea-foundation/axiom-platform/blob/dev/docs/operations/releases.md#publish-installers).
Pass the installer directory and generated manifest to that publisher; this
native repository contains no website build or deployment tooling. CDN promotion
alone does not change the website's reviewed feed.

The website deploy updates `/downloads`, `/api/releases/latest`,
`/api/releases`, and `/api/releases/<version>` atomically. Archived versions keep
their original links and checksums. Never republish different bytes at an existing
version. Desktop checks the feed on startup and every six hours.

Use `package:release` for installers: it builds each target separately and stamps
`axiomUpdateFormat` into the packaged app's metadata. This keeps updates on the
same OS, binary architecture, and installer format, including DMG versus ZIP.
Both the manual installer and automatic macOS tag workflows use this packager.
The app downloads and verifies the new installer; installation is user initiated.
Existing releases without update support need one manual upgrade to a release
containing this feature before they can display future update notifications.
Public image/font assets use `/assets/` in the same scalable R2 bucket.

## Automated tag workflow

Before pushing a tag, run the ordinary workspace quality checks and the
deterministic and real-service evaluation lanes documented in
[`evaluations.md`](evaluations.md). Push the tag only from the intended reviewed
commit. The `signed release artifact` workflow then:

1. requires a budgeted live provider-E2EE regression gate;
2. builds, signs, and verifies the reproducible Linux x86-64 GNU archive;
3. builds AxiomCLI and Desktop natively on Intel and Apple-Silicon macOS
   runners, signs the complete app, notarizes it, and emits PKG, DMG, and ZIP files;
4. builds AxiomCLI and Desktop natively on x64 and ARM64 Windows runners,
   signs both payloads and packages NSIS installers on x64 signing runners; and
5. uploads each architecture as a separate GitHub Actions artifact.

The workflow needs a protected `release` environment with these secrets:

| Secret | Purpose |
|---|---|
| `AXIOM_EVAL_API_KEY` | live provider-E2EE regression credential |
| `AXIOM_EVAL_BASE_URL` | live evaluation API origin |
| `AXIOM_EVAL_MODEL` | live evaluation model ID |
| `AXIOMCLI_RELEASE_GPG_KEY_BASE64` | base64-encoded Linux archive signing key |
| `AXIOMCLI_RELEASE_GPG_FINGERPRINT` | full authorized Linux signing-key fingerprint |
| `MACOS_CERTIFICATE_BASE64` | base64-encoded PKCS#12 (`.p12`) bundle containing Developer ID Application and Installer certificates with their matching private keys |
| `MACOS_CERTIFICATE_PASSWORD` | password protecting that PKCS#12 bundle |
| `APPLE_ID` | notarization account |
| `APPLE_APP_SPECIFIC_PASSWORD` | notarization app password |
| `APPLE_TEAM_ID` | Apple developer team |

The downloaded `.cer` files contain public certificates; the PKCS#12 bundle must
also include the private keys that generated their signing requests. `APPLE_ID`
and `APPLE_APP_SPECIFIC_PASSWORD` must belong to the same Apple Account, with
Account Holder, Admin, App Manager, or Developer access to the selected team.
Generate an app-specific password for notarization through
[Apple Account settings](https://support.apple.com/102654).

Windows signing uses the separate `windows-signing` environment described below,
not uploaded PFX certificates or a long-lived Azure client secret.

The current workflow uploads run artifacts; it does not create or publish a
GitHub Release. Publishing, channel promotion, and update-feed changes are
separate, explicitly reviewed operations.

## Windows production signing

Both `release.yml` and signed Windows runs of `desktop-packages.yml` call
`windows-signed.yml`. The production resources are:

| Setting | Value |
|---|---|
| Azure tenant | `49d43110-cd4f-48d9-88c3-d21ec62243c6` |
| Subscription | `dd9fa81a-94bc-4497-9fc0-ea70320fa8f5` |
| Resource group | `astrea-signing` |
| Signing account | `astrealabssigning` |
| Endpoint | `https://eus.codesigning.azure.net/` |
| Certificate profile | `axiom-production` — **Public Trust**, not Public Trust Test |
| Publisher common name | `Astrea Labs, Inc.` |
| GitHub identity | User-assigned managed identity `axiomai-release-signing` |

For guest-account portal access, use the
[tenant-specific Azure portal](https://portal.azure.com/49d43110-cd4f-48d9-88c3-d21ec62243c6).
The generic Microsoft sign-in endpoint may not discover an invited guest account.

Configure these GitHub **environment variables** in `windows-signing`:
`AZURE_CLIENT_ID` (the managed identity's client ID), `AZURE_TENANT_ID`,
`AZURE_SUBSCRIPTION_ID`, `AXIOM_SIGNING_ENDPOINT`, `AXIOM_SIGNING_ACCOUNT`,
`AXIOM_SIGNING_PROFILE`, and `AXIOM_SIGNING_PUBLISHER`. These are identifiers, not
private keys. The environment must allow only `v*` **tags**, not matching branch
names. Each build/sign job also checks the stable product version and promoted
`main` ancestry before using the signing identity.

The managed identity's federated credential trusts only issuer
`https://token.actions.githubusercontent.com`, audience `api://AzureADTokenExchange`,
and subject
`repo:astrea-foundation@281712270/axiomai@1343021664:environment:windows-signing`.
This repository uses GitHub's immutable-ID subject format; confirm the prefix with
`gh api repos/astrea-foundation/axiomai/actions/oidc/customization/sub` before
recreating the credential. Grant it
**Artifact Signing Certificate Profile Signer** only on:

```text
/subscriptions/dd9fa81a-94bc-4497-9fc0-ea70320fa8f5/resourceGroups/astrea-signing/providers/Microsoft.CodeSigning/codeSigningAccounts/astrealabssigning/certificateProfiles/axiom-production
```

Do not give CI subscription Owner/Contributor or Identity Verifier. Only the
signing job requests `id-token: write`. Azure Login exchanges that short-lived
GitHub token; the signer uses only its Azure CLI credential, with other credential
fallbacks disabled. Protect tag creation and review changes to signing workflows.
Environment deployment reviewers can provide an additional approval gate where
supported by the repository's GitHub plan.

The native build stage uses `package-desktop.mjs --win --x64|--arm64 --unsigned --dir`.
It validates and executes the matching CLI before producing an **internal build
input**, not a distributable installer. Signing runs on Windows x64 because
[Microsoft's signing toolchain](https://github.com/Azure/artifact-signing-action#runner-requirements)
does not support ARM runners. ARM64 payloads retain their native architecture.
On the signing host, install .NET 8 and the pinned `ArtifactSigning` PowerShell
module `0.1.20`. `package-signed-windows.ps1` signs packaged EXE/DLL/native-module
and PowerShell files before NSIS compression. The electron-builder hook signs
the generated uninstaller and final installer before blockmap generation.
The prepackaged path explicitly retains the NSIS-compatible BCJ compression filter.

Every signing invocation requires a valid Authenticode signature from the expected
publisher and a timestamp. Artifact Signing rotates short-lived certificates;
do not pin an individual certificate thumbprint. SHA256 and Microsoft's RFC3161
timestamp service preserve signatures beyond the certificate's short validity.
See [Microsoft's signing integration guidance](https://learn.microsoft.com/en-us/azure/artifact-signing/how-to-signing-integrations).
Checksums/download metadata are generated from the final signed bytes. Signing
failure never produces a successful release artifact or an unsigned fallback.
No extra native installer test job is run; interactive installation acceptance is
still a separate release decision.

For a signed local x64 Windows build, set the same `AXIOM_SIGNING_*` configuration,
authenticate Azure CLI as an authorized signer, install the pinned signing module
and .NET 8, then run `package-desktop.mjs --win --x64`. ARM64 signing uses the
split native-build/x64-sign workflow. `--unsigned` explicitly disables executable
signing and strips Apple, PFX, Azure and Artifact Signing environment variables.

## Artifact verification

The Axiom CLI release-signing public key is checked in at
[`release-keys/axiomcli-release-2026.asc`](release-keys/axiomcli-release-2026.asc).
Its full fingerprint is `625118F3A2C54E247BF4EC2E09F5CD2FE0F6423D`.
Import this public key into the verification keyring before checking the Linux
archive's detached signature. The corresponding private key is restricted to the
protected release environment and the operator's private backup.

The Linux archive contains `bin/axiomcli`, `bin/axiom-proxy`, the Apache-2.0
project license, third-party notice/inventory files, user documentation,
compatibility policy, and build metadata. Bundled Markdown navigation and source
links point to the exact packaged source revision, so the installed subset does
not depend on a surrounding repository checkout. The exact cargo-deny release used to
generate the embedded license inventory is pinned in `.cargo-deny-version` and
recorded in `BUILD-METADATA`. Reproduce and verify it locally with:

```text
cargo install cargo-deny --version "=$(tr -d '\r\n' < .cargo-deny-version)" --locked
export SOURCE_DATE_EPOCH=$(git show -s --format=%ct HEAD)
export AXIOMCLI_SIGNING_KEY=<full maintainer-key fingerprint>
scripts/package-release --sign
scripts/verify-release dist/*.tar.gz dist/*.tar.gz.asc
```

Run the package command twice in separate output directories and compare
SHA-256 digests. Install into a clean prefix with
`scripts/install-release ARCHIVE PREFIX`, run the release smoke workflow, and
then use `scripts/uninstall-release PREFIX`. Verification rejects checksum or
signature failure, path traversal, multiple top-level roots, missing notices,
missing/non-starting binaries, and strings shaped like Axiom credentials.

For every Desktop artifact, verify before promotion that:

- the filename and application version equal the tag and expected architecture;
- `Axiom.app` or `Axiom.exe` and the embedded
  `resources/bin/axiomcli[.exe]` have the expected native architecture;
- the embedded sidecar starts with `--version` and reports the tagged product
  version without opening a window;
- all executable code and the installer carry the intended publisher identity;
- macOS Gatekeeper accepts the signed app/DMG and validates the app's stapled
  notarization ticket, or Windows reports valid Authenticode signatures; and
- extracted artifacts contain no credentials or unplanned files.

CI requires exact primary artifact counts and tag-derived names, verifies the
embedded application version, checks native application and sidecar
architectures, and validates the app/sidecar/installer signatures as applicable.
Windows executes the native CLI's headless `--version` before packaging; it does
not install or execute the generated signed installer. macOS and Linux retain
their packaged-sidecar smoke checks. macOS additionally verifies
the signed DMG and the app's stapled notarization ticket. Credential scanning
and independent target-machine verification remain explicit release-review
checks.

## Interactive acceptance

Before public promotion, record the commit, artifact hash, OS version,
architecture, browser and outcome for each target. Exercise installation,
upgrade/uninstall and terminal CLI discovery; system-browser return and
passkey/Google/password/wallet sign-in; vault refusal/locking, refresh,
account switching and logout; payment/usage UI; verified E2EE chat, cancellation
and recovery; Agent approvals and OS-specific process behavior; window controls,
accessibility, sleep/resume and OS security prompts.

Headless packaging cannot establish those outcomes. Preserve remaining gaps in
[validation](validation.md); [dated test reports](testing/README.md) describe only
their recorded scope. Interactive tests should run in an owned test session.

Uninstall removes only installed program and documentation files. It preserves
platform configuration, account-scoped local threads, installation identity,
and credential-vault entries unless the user separately signs out or explicitly
deletes that data.
