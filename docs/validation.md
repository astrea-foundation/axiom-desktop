# Documentation validation and outstanding acceptance

The 2026-09-21 consolidation reviewed the native implementation at `fa9c4ab`
(the then-current `dev` baseline), manifests, generated contracts, tests and workflow definitions.
The clean-baseline refactor updates the storage and billing findings below and
records its additional checks separately. These source checks do not establish
the current state of production, published installers or unrecorded manual work.

## Source checks

| Topic corrected | Source used |
|---|---|
| Components and independent hosted/native ownership | [workspace](../Cargo.toml), [package scripts](../package.json), [architecture](architecture.md) |
| Configuration precedence, bounds, storage paths and explicit account reset | [config.rs](../apps/axiomcli/src/config.rs), [paths.rs](../apps/axiomcli/src/paths.rs), [session.rs](../apps/axiomcli/src/session.rs) |
| Schema 2/application ID `0x41584133`, transactional upgrade from schema 1, and rejection of obsolete stores | [storage schema/migrations](../apps/axiomcli/src/session/schema.rs), [versioning](versioning.md) |
| Local titles, edit/regenerate, staged compaction and stable user-submission ordering | [title helper](../apps/axiomcli/src/session_title.rs), [agent.rs](../apps/axiomcli/src/agent.rs), [ACP](../apps/axiomcli/src/acp.rs), [concurrency regressions](testing/concurrency-ui-2026-09-20/FIXES.md) |
| Desktop Agent on all packaged OS targets; Linux-only Bubblewrap isolation | [workspace.rs](../apps/axiomcli/src/workspace.rs), [policy.rs](../apps/axiomcli/src/policy.rs), [Desktop](desktop.md) |
| ACP 0.2 features, API-key creation and Tinfoil generation exception | [extension DTOs](../crates/axiom-acp-extension/src/lib.rs), [contract](acp-compatibility.md) |
| Mainnet ZEC deposits, gift redemption and `billing@3` | [billing.rs](../apps/axiomcli/src/billing.rs), [TUI credit](../apps/axiomcli/src/tui/billing.rs), [Desktop gift form](../apps/desktop/src/renderer/src/components/GiftCodePanel.tsx) |
| NEAR/Tinfoil authentication and truthful evidence | [provider drivers](../crates/axiom-secure-client/src/providers), [security](threat-model.md) |
| Main-only CI, separate manual/signed packaging and publication handoff | [workflows](../.github/workflows), [packager](../apps/desktop/scripts/package-desktop.mjs), [releasing](releasing.md) |

Local Markdown links and heading anchors are checked during this consolidation,
including moved history and component entry points. Installation guidance was
also checked against its linked official Apple, Microsoft and AppImage sources.
Dated performance and security results are labeled as historical rather than
reused as current acceptance claims.

## Consolidation checks run

On 2026-09-21: all 393 local Markdown links/heading anchors resolved; the six
bundled guides contained 48 source-revision links with valid repository targets;
and the account-plan release gates were carried forward.
`cargo fmt --all -- --check`, shell syntax and `git diff --check` passed.
The existing `release_archive_is_reproducible_signed_installable_and_cleanly_uninstalled`
regression passed against the updated packaging script. Application behavior was
reviewed against source and existing regression evidence; no new live inference,
payment, production deployment or interactive OS acceptance was performed.

## Windows signing verification

On 2026-09-21, the Artifact Signing integration passed all 150 Desktop source
tests, Desktop TypeScript checks, and Actionlint 1.7.12 for the three affected
workflows. PowerShell 7.6.6 parsed the signing scripts and exercised signature
validation with mocked Authenticode results (wrong publisher, invalid signature,
and missing timestamp all fail closed). These are source checks, not a successful
Azure signing operation or generated-installer acceptance result.

The Windows installer smoke tests and extra native installer-test jobs are not
part of this pipeline. Production packaging retains publisher, timestamp,
architecture and version checks. A promoted release must establish the first
end-to-end OIDC/signing result; this change did not publish a release.

## Carry-forward work

The superseded account/invoice plans have been removed. Their remaining release
work is carried forward below and in [ADR 0014](adr/0014-clean-storage-and-deposit-billing.md).
The redesign's [open items](archive/desktop-redesign.md#6-open-items-for-the-developer)
remain separate.

| Area | Current disposition / evidence still needed |
|---|---|
| Original branch freeze and schema baseline | Historical setup, not a standing development stage. Record actual release revisions using CONTRIBUTING and current platform migrations. |
| Hosted staging, four auth methods and recovery/rollback | Requires deployment records and browser checks for the chosen release; source/tests alone cannot close it. |
| Billing edge cases | Exercise persistent address provisioning, duplicate and out-of-order signed events, namespace isolation, valuation, reorg review and partial-inference settlement with Axiom Payments and the selected inference provider. |
| macOS/Windows interactive acceptance | Record vault, system-browser, passkey/Hello, signing, installer/uninstaller, account switching, refresh/logout, billing and authenticated inference outcomes per target. |
| Artifact evidence | Record OS/browser/architecture, commit, artifact hashes and signing identity with acceptance results. A successful headless package is insufficient. |
| Initial destructive production cutover | Retired proposal, not an executable runbook. The newly authorized September reset is defined in ADR 0014 and the platform reset runbook. Platform operations own database/namespace replacement, secret changes, smoke tests and recovery. |
| Rollback and retirement | Preserve evidence of a staging rollback drill, secret/key revocation and retention decisions for any still-active retirement work; do not infer completion from the old checklist. |
| Old hosted source removal | Native ownership is now enforced by the repository layout; hosted code belongs to `axiom-platform`. This does not prove an old production route or secret was retired. |
| Branding/design hand-off | The legal-line `TODO(brand)` remains in [main](../apps/desktop/src/main/index.ts); product-name/artwork decisions and optional layered icon/tray adoption need explicit design decisions. Current packaging uses `icon.icns`. |
| Old mock Proxy and missing chat actions | Superseded by the current native proxy integration and implemented conversation controls; retained only in the historical hand-off. |
| Provider availability/provenance canaries | Run attested E2EE canaries for the promoted native/backend pair. Old near-v2 or early Tinfoil measurements do not establish current deployment health. |
| Live TUI agent retest | [2026-09-21 input/startup validation](testing/tui-input-2026-09-21.md): 398 automated tests passed; Chrome/Google native sign-in, attested NEAR agent edits/tests, approvals, steering, resume and streaming cancellation exercised successfully. Qwen 3.6 remains blocked by provider TDX OutOfDate evidence; other providers and release targets still need their own live acceptance. |

Use [release acceptance](releasing.md#interactive-acceptance) for a new release
and [platform operations](https://github.com/astrea-foundation/axiom-platform/blob/dev/docs/operations/deployment.md)
for backend rollout. Close an item with dated evidence rather than changing an
old proposal to imply that the check happened.

## Clean-baseline verification

The September reset removes retired invoice APIs/UI and old migration chains.
Local validation: 503 platform tests, all native workspace Rust tests, explicit
reset/plan CLI tests, TypeScript checks/tests, Desktop and auth builds, receiver
unit/integration tests, local reset and Compose validation. All 127 Desktop browser
regressions and the updated release-archive regression pass. The development PRs
record the paired revisions. At that baseline, ordinary Clippy completed but
strict Clippy reported existing warnings. The native maintenance work below
resolves that check. Live inference, funded deposits, OS-specific
acceptance and production cutover were not exercised by this source cleanup.


## Native maintenance verification

The [maintenance plan](native-maintenance-plan.md) removes dormant UI/counters,
unused assets and asynchronous title plumbing; historically connected the now-retired
Phala verifier to signed NVIDIA policy; tests NEAR report binding through the production
verifier; and separates ACP, TUI and Desktop controller responsibilities.

Workspace strict Clippy now passes with `--all-targets --all-features -- -D warnings`.
Accounting events box their large record payload, duplicate model reconciliation
is removed, and remaining lint fixes preserve the wire/storage contracts. The
catalog’s independent capability flags keep a documented, targeted lint expectation.

On 2026-09-21, the complete Rust workspace suite passed (510 tests, six opt-in
or manual tests ignored). All 209 JavaScript/TypeScript tests and 127 Desktop
browser regressions passed. TypeScript checks, Desktop production build, pinned
ACP SDK stack regression, generated-contract drift check and callback asset
verification passed. All 393 authored Markdown links/anchors resolved; all 93
Desktop/client source modules remain reachable from the application entry points.
Both repetitions of the deterministic evaluation script passed (62 tests each).
The maintenance plan records publication and the full command list.

Live inference, funded deposits, production deployment and interactive native
macOS/Windows acceptance remain separate release checks; browser platform fixtures
do not substitute for those checks.

## Persistence refactor verification

On 2026-09-21, the [storage module split](adr/0015-persistence-modules.md) passed
`cargo fmt --all -- --check`, strict workspace Clippy with
`--all-targets --all-features -- -D warnings`, and
`cargo test --workspace --all-features`: 511 tests passed, six opt-in/manual tests
ignored. The added timeline regression verifies duplicate-sequence rejection and
indexed pagination after removing the redundant index from the fresh baseline.
The existing storage, account-switch, recovery, authenticated response and ACP/CLI
tests pass through the unchanged public `SessionStore` API. The
[coordinated plan](https://github.com/astrea-foundation/axiom-platform/blob/dev/docs/architecture/persistence-refactor-plan.md)
records backend and receiver validation. This pass did not repeat UI/browser or
live-provider acceptance and did not reset an existing user's database.

## Provider catalog and upload verification

On 2026-09-21, the [catalog and direct upload change](provider-catalogs.md) passed
the full Rust workspace suite (510 tests, six opt-in/manual tests ignored), strict
workspace Clippy, formatting, TypeScript checks, all 261 JavaScript/TypeScript
tests, generated ACP contract consistency and the Desktop production build.
The picker was visually checked with the Tinfoil logo and image/file icons.

The final browser run passed 129 checks with four existing billing tests excluded.
All four failures reproduce on unchanged `dev` revision `aa38e4e` with its own
installed dependencies:

- `balance is a separate screen and payments stay hidden after sign-out`:
  duplicate deposit-address elements during transitions.
- `balance deposit controls fit the reference window, respect reduced motion, and match both themes`:
  the empty deposit screen overflows the reference window.
- `deposit progress keeps ZEC primary and changes estimated USD to credited USD`:
  duplicate deposit summaries during transitions.
- `gift redemption is masked, updates balance, retries safely and clears on account switch`:
  the old masked field retains its value during the account transition.

These failures remain open; the complete browser suite is not green. Reproduce
them with `pnpm exec tsx --test --test-name-pattern='balance is a separate|balance deposit controls|deposit progress keeps|gift redemption is masked' tests/browser/deletion-focus.test.ts`
from `apps/desktop`. The same expression passed to `--test-skip-pattern` with
`tests/browser/*.test.ts` selects the other 129 checks.

The platform's paired [catalog validation](https://github.com/astrea-foundation/axiom-platform/blob/dev/docs/architecture/provider-catalogs.md#validation-2026-09-21)
records discovery and backend tests. Live paid image/document inference,
deployment and interactive OS acceptance were not performed. Provider attestation
and authenticated completion must still succeed in release canaries; a public
catalog listing alone does not establish that an offering works end to end.

## Combined native PR verification (2026-09-22)

The integration of native PRs #7, #8, #9 and #12 combines provider-neutral TUI
input/security/usage, bounded local history and independently paged accounting,
Windows artifact signing, and dynamic catalogs with encrypted direct uploads.
The merged ACP contract advertises `account@2`, `timeline@2`, `attachments@2`
and `billing@3`. The product-version handshake fixture now uses the current
timeline contract; rejection coverage for `timeline@1` remains intact.

Conflict resolution preserves billing controls and proof refreshes together.
Bracketed gift-code paste stays in the masked billing input, leaves the chat
draft untouched and remains redeemable; an added regression verifies that
boundary. Provider-neutral presentation fixtures use a generic future gateway,
without restoring a retired provider registration. Explicit dependency versions
match the existing lockfile and leave the Tinfoil Git revisions unchanged.

Validation passed:

- Full Rust workspace: 546 tests passed, six opt-in/manual tests ignored;
  strict workspace Clippy, formatting and a locked all-targets/all-features check.
- All 268 JavaScript/TypeScript tests, TypeScript checks, the Desktop production
  build, generated ACP contract consistency and the pinned SDK stack regression.
- `cargo deny` advisories, bans, licenses and sources; workflow `actionlint`;
  PowerShell parsing and mocked signature-rejection checks within the JS suite.

The complete Desktop browser suite ran 137 checks: 134 passed and three failed.
The remaining failures are the previously reproduced duplicate deposit-address
elements, deposit-screen overflow and duplicate deposit summaries described
above. The gift account-transition check passed in this run; a single passing
run does not close its previously observed failure. No new browser failures
appeared, and the complete browser suite remains non-green.

This is development-branch integration evidence. Paid provider inference,
deployment, real Azure OIDC signing and interactive native OS acceptance were
not performed. Pushes and pull requests targeting `dev` do not trigger Actions.

## Desktop copy cleanup (2026-09-22)

The copy cleanup shortens routine account, gift, Web and update messages,
removes display-only dot separators, and moves usage accounting and installation
metadata into expandable details. The spending empty-state explanation remains.
Privacy disclosures, deletion warnings and protocol delimiter parsing are retained.

Desktop typechecking, the production build and 189 unit/contract tests passed;
one signing test was skipped because PowerShell was unavailable. The full browser suite passed 134
of 137 checks, including keyboard access to usage details, Web consent and
installation details. The same three billing failures listed above reproduced
on the unchanged tree shared by `a30b4a8` and merged `dev` revision `5fb9f01`.
No additional browser failures appeared. Light/dark update screens, the usage
panel and Web disclosure were visually checked with deterministic browser
fixtures; native OS interaction and live inference were not exercised.

## Verification policy cleanup (2026-09-22)

The current signed policy contains vendor requirements, certificate anchors and
validity metadata. Native and platform bundle identical schema 2, sequence 3
envelopes. The [rollout notes](threat-model.md#trust-policy-operations) cover the
new signing key and existing client journals. `securityEvidence@2` carries the
updated report contract through the native sidecar and Desktop. Tinfoil source
provenance, vendor attestation, key binding and response authentication remain
required.

Validation passed:

- Full Rust workspace: 547 tests passed, six opt-in/manual tests ignored;
  strict workspace Clippy and formatting passed.
- All 78 ACP client tests and 189 Desktop unit/contract tests passed; one
  Desktop signing test was skipped because PowerShell was unavailable.
- TypeScript checks, generated ACP contract consistency and the Desktop
  production build passed.
- All 16 privacy-proof browser checks passed, including report copying,
  refresh, expiry and keyboard behavior. Light/dark evidence dialogs were
  visually checked with deterministic fixtures.
- The paired platform change passed 87 relay and NEAR worker-session tests,
  including verification of the served policy signature and exact payload.

Policy tests cover signatures, tampering, expiry, rollback and replacement of an
older journal. Client negotiation rejects an incompatible evidence contract.
The previously recorded billing browser failures remain open; this change did
not rerun the complete browser suite, perform live inference or deploy either
repository.

## Queued-message controls (2026-09-22)

Desktop's Send now action cancels the current reply and submits the selected
queued message as a new turn after cancellation and prompt cleanup. Queued
messages can also be cancelled while that cleanup is pending. The queue retains
attachments, Web choice, Agent revision and stable message IDs; disconnects,
account changes and failed cancellation cannot silently replay the message.
Stopping during verification also prevents a waiting prompt from starting later.

All 79 ACP client tests and 198 Desktop unit/contract tests passed; one
PowerShell-dependent signing check was skipped. This includes 21 queue tests
covering priority, cancellation, storage failure, attachments, account/runtime
changes and delayed cleanup. All 12 focused queue browser tests passed across
the Linux, Windows and macOS renderer fixtures. Light/dark queue controls were
visually checked. TypeScript checks and the Desktop production build passed.
These checks used local fixtures; live inference and native OS qualification
were not repeated, and the existing billing browser failures remain open.

## Balance panel refreshes (2026-09-22)

Deposit and gift-credit panels now have distinct account-scoped React keys.
A browser regression reproduced four deposit panels before the fix and verifies
one address, QR code and copy button after repeated manual/automatic refreshes
and exchange-rate changes, while preserving unsubmitted gift-code input.

All nine focused balance, deposit and gift-redemption browser checks passed,
including the previously failing billing checks. The layout check now permits
vertical scrolling to the gift form while requiring the deposit action to remain
visible and all content to fit the panel width. Light/dark screenshots were
visually checked. All 198 Desktop unit/contract tests, Desktop TypeScript checks
and the production build passed; one PowerShell-dependent test was skipped.
These checks used local fixtures and did not submit payments or redeem live codes.

## Interrupted-reply notices (2026-09-22)

Desktop shows one notice per failed turn when the timeline includes its error.
Known stream endings and timeouts use short messages with a collapsed technical
detail. Ambiguous authentication/transport failures remain errors; presentation
never changes the stored status, terminal verification or native checks.

All 26 timeline unit tests and seven focused browser checks passed. Coverage
includes partial text/reasoning, failures in separate turns, retained verification
failures, deliberate cancellation, keyboard-operated details and regeneration
through the existing native revision flow. Desktop TypeScript checks and the
production build passed. Light/dark screenshots were visually checked; provider
traffic and native cryptographic verification were unchanged and not rerun.

Reply notices and global error banners also have keyboard-accessible dismissal
buttons. Eight focused browser checks cover dismissal, later/changed errors,
rapid connection recovery and failure, preserved native errors and failed reply
status, and removal of the notice without an empty timeline row. The 26 timeline
unit tests, Desktop TypeScript checks and production build passed. Light/dark
screenshots were checked with the dismissal controls visible.
