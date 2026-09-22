# Release procedure

Development lands on `dev`. A public release requires the explicit promotion
workflow in [CONTRIBUTING](../CONTRIBUTING.md), a stable `vMAJOR.MINOR.PATCH` tag
on promoted `main`, and matching Desktop/AxiomCLI versions. A version bump or
push to `dev` does not publish installers or activate the update feed.

There are no existing users to migrate. Fresh installers include installation
identity and the native updater. The [implementation plan](automatic-updates-plan.md)
and [qualification report](testing/automatic-updates-2026-09-22.md) distinguish
implemented code from outstanding signed OS acceptance.

## Supported platforms

| Target | Desktop installer | Standalone CLI + proxy installer |
|---|---|---|
| Windows x64 / ARM64 | NSIS EXE | NSIS EXE, per-user |
| macOS Intel / Apple Silicon | PKG | PKG |
| Linux x64, glibc 2.35+ | DEB, Pacman, AppImage | Self-extracting SH, per-user |

All Desktop packages contain their matching CLI. No bundled CLI is updated
independently. Linux ARM and musl are not public targets. AppImage requires FUSE;
Linux local tool execution requires Bubblewrap. macOS/Windows approved host tools
retain the boundaries documented in [the threat model](threat-model.md).

## Update trust and release setup

Use a dedicated Ed25519 update-signing key, separate from account, inference,
Apple, Windows and old archive/GPG keys. Store its PKCS#8 PEM as
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

Both source repositories are private. Create **public**
`astrea-foundation/axiom-releases` with an initial README commit before the first
publication. Only installers, checksums and signed manifests are uploaded there.
Its release tag is a distribution label; the signed `revision` identifies the
native source tag checked in the private source repository.

Configure protected release credentials:

| Repository/environment | Credential | Required access |
|---|---|---|
| native `release` | `AXIOM_UPDATE_SIGNING_KEY` | Ed25519 private PEM |
| native `release` | `AXIOM_RELEASES_TOKEN` | Contents write on the public distribution repository only |
| native `release` | `AXIOM_PLATFORM_RELEASE_TOKEN` | Actions write on `axiom-platform`, to dispatch `release.yml` on `main` |
| platform `release` | `AXIOM_RELEASES_TOKEN` | Contents write on the distribution repository, to read drafts and publish them |
| platform `release` | `AXIOM_NATIVE_READ_TOKEN` | Contents read on `axiomai`, for source-tag ancestry verification |
| platform `release` | `CLOUDFLARE_API_TOKEN` | Existing account/zone validation, CDN object write and website deployment |

The platform workflow also records the published catalog on `main` using its
repository token. Configure release-bot access consistent with branch protection;
ordinary development remains on `dev`. Bring the generated release catalog back
to `dev` with the normal post-release `main` merge. Tokens never enter installers.

macOS uses the protected `release` secrets `MACOS_CERTIFICATE_BASE64`,
`MACOS_CERTIFICATE_PASSWORD`, `APPLE_ID`, `APPLE_APP_SPECIFIC_PASSWORD`, and
`APPLE_TEAM_ID`. The certificate bundle must contain Developer ID **Application**
and **Installer** identities. The workflow uses a temporary keychain, signs the
code and PKGs, submits notarization and staples/verifies tickets.

Windows uses the existing protected `windows-signing` environment and Azure OIDC:
`AZURE_CLIENT_ID`, `AZURE_TENANT_ID`, `AZURE_SUBSCRIPTION_ID`, and
`AXIOM_SIGNING_ENDPOINT`, `AXIOM_SIGNING_ACCOUNT`, `AXIOM_SIGNING_PROFILE`,
`AXIOM_SIGNING_PUBLISHER` variables. Signing runs on x64 with .NET 8 and pinned
`ArtifactSigning` 0.1.20; ARM64 binaries are built on a native ARM64 runner first.
All payload code, uninstallers and final NSIS installers require valid timestamped
Authenticode signatures from the expected publisher. No unsigned fallback exists.

## Build and publish

The tag-triggered [release workflow](../.github/workflows/release.yml):

1. Checks `main` ancestry, stable version, completed CI for the tagged revision,
   and the existing budgeted live E2EE gate.
2. Builds Linux Desktop/CLI, native macOS x64/ARM64 PKGs, and native Windows
   x64/ARM64 inputs followed by the protected Windows signing job.
3. Collects the complete 12-installer matrix. `download-manifest.mjs` hashes
   final signed/notarized bytes, writes `SHA256SUMS`, and signs `manifest.json`.
4. Creates or verifies immutable assets on a draft distribution release.
5. Dispatches the platform publisher on its `main` branch with native version
   and revision. Its separate run owns CDN verification and channel activation.

The manifest's `sequence` defaults to the native commit timestamp. It must
increase between versions; a deliberate `AXIOM_RELEASE_SEQUENCE` override is
available to the generator. `revision` defaults to `git rev-parse HEAD`.
Equal version or sequence with changed metadata is rejected by clients. Use a
new version for any artifact correction; never rebuild over an existing release.

Both products consume `https://axiom.stream/api/releases/latest`. The schema-2
manifest uses `downloads` for Desktop and `cliDownloads` for standalone installers.
Each record contains product, platform, architecture, format, name, length,
SHA-256, and exact CDN and public GitHub asset URLs. Canonical signing recursively
sorts object keys and uses compact UTF-8 JSON without the `signature` field.
The native and platform repositories each check in their own contract and vectors.

Follow the [platform publication guide](https://github.com/astrea-foundation/axiom-platform/blob/dev/docs/operations/releases.md)
for retries. It verifies all bytes before changing the public feed and records
`release-publication.json` as a workflow artifact. An Actions build-input ZIP is
never linked as an installer. Blockmaps and Electron support files are not part
of this updater's public contract; the native helper uses the complete installer.

## CI baseline

Pull requests targeting `main` run Linux Rust and Desktop UI checks. Merging does
not repeat those checks automatically. Native macOS/Windows validation is an
explicit `ci.yml` dispatch with `target=all` or a specific platform; `linux` and
`desktop` rerun only those checks. Validate the complete candidate on `dev`,
promote it, confirm the promoted tree matches, and tag that validated candidate
commit now reachable from `main`. The release gate requires successful Rust, UI,
and all four native CI jobs for the exact tagged revision, combining targeted
runs when needed. Packaging reuses those results instead of repeating the suites.
Retry only failed validation targets or packaging jobs. These checks do not
replace signed OS installation acceptance.

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
The old developer archive scripts remain available for reproducibility work but
are not release downloads or automatic-update installation owners.
