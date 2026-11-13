# Release procedure

Tagged releases build the reproducible AxiomCLI Linux archive and native Axiom
Desktop packages for macOS Intel, macOS Apple Silicon, Windows x64, and Windows
ARM64. A pushed release tag must be a stable `vMAJOR.MINOR.PATCH` with no
prerelease or build suffix, and the version after `v` must match
`apps/axiomcli/Cargo.toml`. This keeps the product version valid for macOS bundle
and Windows executable metadata. The workflow validates the contract before the
budgeted live evaluation and before any packaging job. Desktop's source manifest
is synchronized with AxiomCLI by the packaging script before invoking
electron-builder.

## Desktop and terminal CLI installers

The `desktop installers` workflow (`desktop-packages.yml`) builds Linux x64,
macOS Intel/Apple Silicon, and Windows x64/ARM64. Run it manually with the
explicit `unsigned` option for early-access releases without Apple or Windows
credentials. The `codex/desktop-packaging-*` build branches always build unsigned
artifacts. This does not change the signed defaults in electron-builder.yml.

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

Linux release builds use Ubuntu 22.04 so the CLI does not acquire newer glibc
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

The workflow exercises installed CLI startup and reinstall behavior on native
macOS/Windows runners and a DEB installation on Ubuntu. The AppImage installer
tests cover argument forwarding, replacement, command collisions, and preserved
account data. GUI/credential-vault/inference acceptance remains a separate check.

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
Public image/font assets use `/assets/` in the same scalable R2 bucket.

## Automated tag workflow

Before pushing a tag, run the ordinary workspace quality checks and the
deterministic and real-service evaluation lanes documented in
[`evaluations.md`](evaluations.md). Push the tag only from the intended reviewed
commit. The `signed release artifact` workflow then:

1. requires a budgeted live provider-E2EE regression gate;
2. builds, signs, and verifies the reproducible Linux x86-64 GNU archive;
3. builds AxiomCLI and Desktop natively on Intel and Apple-Silicon macOS
   runners, signs the complete app, notarizes it, and emits DMG and ZIP files;
4. builds AxiomCLI and Desktop natively on x64 and ARM64 Windows runners,
   Authenticode-signs the package, and emits NSIS installers; and
5. uploads each architecture as a separate GitHub Actions artifact.

The workflow needs a protected `release` environment with these secrets:

| Secret | Purpose |
|---|---|
| `AXIOM_EVAL_API_KEY` | live provider-E2EE regression credential |
| `AXIOM_EVAL_BASE_URL` | live evaluation API origin |
| `AXIOM_EVAL_MODEL` | live evaluation model ID |
| `AXIOMCLI_RELEASE_GPG_KEY_BASE64` | base64-encoded Linux archive signing key |
| `AXIOMCLI_RELEASE_GPG_FINGERPRINT` | full authorized Linux signing-key fingerprint |
| `MACOS_CERTIFICATE_BASE64` | base64-encoded Apple signing certificate |
| `MACOS_CERTIFICATE_PASSWORD` | Apple signing-certificate password |
| `APPLE_ID` | notarization account |
| `APPLE_APP_SPECIFIC_PASSWORD` | notarization app password |
| `APPLE_TEAM_ID` | Apple developer team |
| `WINDOWS_CERTIFICATE_BASE64` | base64-encoded Authenticode certificate |
| `WINDOWS_CERTIFICATE_PASSWORD` | Authenticode certificate password |

The current workflow uploads run artifacts; it does not create or publish a
GitHub Release. Publishing, channel promotion, and update-feed changes are
separate, explicitly reviewed operations.

## Artifact verification

The Linux archive contains `bin/axiomcli`, `bin/axiom-proxy`, the Apache-2.0
project license, third-party notice/inventory files, user documentation,
compatibility policy, and build metadata. The exact cargo-deny release used to
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
architectures, runs the packaged sidecar's headless `--version`, and validates
the app/sidecar/installer signatures as applicable. macOS additionally verifies
the signed DMG and the app's stapled notarization ticket. Credential scanning
and independent target-machine verification remain explicit release-review
checks.

Interactive target-machine acceptance is also required before public promotion;
see [`platform-support.md`](platform-support.md). Those tests are intentionally
not run as part of a headless build or while another user controls the desktop.

Uninstall removes only installed program and documentation files. It preserves
platform configuration, account-scoped local threads, installation identity,
and credential-vault entries unless the user separately signs out or explicitly
deletes that data.
