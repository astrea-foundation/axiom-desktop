# Automatic updater implementation checks — 2026-09-22

Scope: development implementation in `axiomai` and `axiom-platform`, targeting
`dev`. No main promotion, production signing, release tag, public repository
creation, CDN upload, GitHub Release publication, or live feed change was run.
There are no user migrations or legacy update adapters.

## Local evidence

- Rust CLI/proxy/installation suites: **469 passed, 3 existing tests ignored**.
  Commands: `cargo test -p axiom-installation -p axiomcli -p axiom-proxy --all-features`.
  New tests verify the shared Node/Rust Ed25519 vector, tampering, anonymous
  bounded HTTP, redirects, partial/corrupt downloads, remembered-release rollback,
  installation ownership and shared/exclusive locks. ACP `/update` stays local.
- Native helper integration: uses a real shell installer and deterministic
  executable fixtures in an isolated prefix. A-to-B activation changes CLI/proxy
  together, refuses changed staging, checks the resulting version and handles
  a second updater observing the already-installed target. This does not use
  production binaries or a live update feed.
- Desktop: **199 passed, 1 skipped** (PowerShell is unavailable on this Linux host).
  Tests cover verified handoff, active-work waiting, cancellation, duplicate
  requests, failed saves, missing acknowledgement, native event bounds and
  restart flush. The standalone Linux installer also exercises install, upgrade,
  damaged candidate rejection, retained previous versions and uninstall.
- Desktop browser: three light/dark update-control cases passed for progress,
  waiting, cancellation, retry and unavailable target. Other application tests
  continue to cover ordinary account-scoped draft behavior.
- Workspace TypeScript checks and JS tests passed. Rust formatting and Clippy
  with warnings denied passed for CLI/proxy/installation targets.
- Hosted website build and landing tests passed: independent signature/contract
  vectors, local artifact preflight, complete-matrix mirror checks, missing or
  changed bytes, signed catalog import, immutable history, API delivery and
  launch-hold versus published-download behavior.
- Both repositories' changed workflows passed current `actionlint`. This is
  syntax/expression validation, not execution on native runners. An older pinned
  actionlint lacked the existing macOS Intel and Windows ARM runner labels;
  current actionlint accepts them. Runner availability is documented by
  [GitHub](https://docs.github.com/en/actions/reference/runners/github-hosted-runners).

## Release qualification still required

Real signed Windows x64/ARM64 and macOS x64/ARM64 installers were **not built or
executed on this Linux host**. Nor were real DEB/Pacman/AppImage GUI A-to-B upgrades
or a Windows same-console restart run. The native version check verifies CLI
startup; it is not evidence of GUI, vault or OS authorization behavior.

Before advertising the first public release, record for each package/architecture:
initial install; idle and busy Desktop/TUI A-to-B update; same-terminal TUI
workspace/session restoration; draft/history retention; runtime-consent reset;
OS authorization acceptance/denial; interruption and repair; and uninstall.
Record the source revision, artifact hash, OS and signature identity.

Provision the dedicated public `astrea-foundation/axiom-releases` repository,
update-signing key and narrowly scoped protected credentials using
[release setup](../releasing.md#update-trust-and-release-setup). Exercise publication
and retry in an isolated environment before production activation. The configured
repository name is the implementation default; no private source was published.

[The implementation plan](../automatic-updates-plan.md) tracks these outstanding
release checks. Passing local tests establishes the dev implementation; it does
not mean an updater or installer has been shipped.
