# Release acceptance

Qualify the exact promoted source revision and signed installer bytes. Record
OS, architecture, browser, source revision, artifact SHA-256, signing identity,
result and limitations in the release's maintainer acceptance record. Unit tests
and browser fixtures do not establish signed native OS acceptance.

## Automated checks

Run the [development checks](development.md#checks), including Rust formatting,
strict Clippy, functional tests, dependency policy, generated ACP contract checks,
TypeScript checks, Desktop unit/browser tests and the production build. Use the
[release procedure](releasing.md#ci-baseline) to reuse successful CI for the exact
candidate instead of repeating unrelated jobs. The live release gate uses the
registered provider's attested E2EE protocol; it does not replace interactive QA.

## Interactive qualification

| Area | Required evidence |
|---|---|
| Installation | Fresh install and uninstall on Windows x64/ARM64, macOS Intel/Apple Silicon, and each supported Linux package; expected publisher, notarization and architecture |
| Native authentication | System-browser sign-in using each supported method, callback/poll completion, cancellation and expiry, vault storage, restart, refresh/logout and account switching |
| Provider security | Current catalog, fresh local attestation, authenticated encrypted completion, stream cancellation/truncation, proof expiry, and explicit degraded-TEE consent/reset |
| Conversation state | Draft/queue/history retention, attachments, edit/regenerate, compaction, interrupted-turn recovery, restart and account isolation |
| Tools | Approval, denial, cancellation and workspace boundaries on each supported OS; Web consent and search's distinct privacy boundary |
| Account credit | Address provisioning, deposit progress and USD valuation, posted usage, gift-code masking/redemption and account-switch cleanup; coordinate real settlement/reorg checks with platform maintainers |
| Automatic updates | Signed A-to-B upgrades from idle and busy Desktop/TUI; same-terminal workspace/session restoration, GUI reopen, matching CLI version and preserved drafts/history |
| Update failure paths | Denied OS authorization, cancellation, locked-process waits, interruption/repair, corrupt or wrong-target artifacts, rollback rejection and duplicate updater attempts; staging cleanup, including Windows helpers still in use and abandoned downloads |
| Publication | Matching source tag, signed inventory, complete installer matrix, verified mirrors, feed activation and safe publication retries |
| Branding and notices | Confirm the copyright/publisher line, artwork/font redistribution permissions, third-party notices and packaged icons |

An update must reset runtime-only outdated-TEE consent and preserve account data.
Do not downgrade a database into an incompatible client. Test real OS dialogs,
vaults and signed installers rather than treating mocks as acceptance evidence.

Backend rollback, payment-receiver recovery and secret management are separate
platform operations. Coordinate their acceptance with compatible native releases;
this document does not authorize a production reset or deployment.

## Recording results

Keep unresolved requirements visible in the candidate's acceptance record. Record
fresh outcomes rather than carrying forward old test counts or failure claims.
Use synthetic accounts for screenshots and never attach credentials, gift codes,
private keys or real conversation data. Publish a concise qualification summary
with the release; retain raw operational captures privately.

## Windows setup and Store acceptance

For the 0.1.11 Windows distribution candidate, record the promoted revision and final signed
bytes before release. Cross-compilation, controller/browser tests and hidden
native-control initialization checks are development evidence, not Windows Store
certification. Outstanding signed-package acceptance includes:

The 0.1.10 build failed before publication because an installer-only NSIS
variable was declared in the separately compiled uninstaller. The corrected
include guards that declaration. Both modes were checked with electron-builder's
NSIS 3.0.4.1 and warnings treated as errors; 0.1.10 remains an unpublished build.

- Fresh standard-user silent offline install at the default destination on x64
  and ARM64; shortcuts, Add/Remove Programs, no installer-triggered app launch,
  native CLI/proxy pairing, timestamped publisher and clean uninstall.
- First-launch latest/no-update/offline/error/retry/continue paths, saved-state
  handoff and one restart, including interrupted launches and no restart loop.
- Website setup on both CPUs: signed feed, digest/size/publisher rejection,
  cancelled downloads, equal/newer installed versions, lock waits, existing
  destination/scope preservation, denied UAC and installer repair.
- Defender scan, full EXE qualification and private Partner Center listing,
  funded reviewer credentials, privacy review and certification. The Store
  receives the full offline EXE; the website setup EXE is never submitted there.

The release workflow emits `windows-store-submission` with immutable full-package
references and remaining fields. Private maintainer records live in the platform
repository. Store approval covers its initial installation channel; direct website
downloads and subsequent updates can still receive Windows reputation prompts.
