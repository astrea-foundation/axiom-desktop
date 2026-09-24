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
