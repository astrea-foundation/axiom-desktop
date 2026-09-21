# Documentation validation and outstanding acceptance

The 2026-09-21 consolidation reviewed the native implementation at `fa9c4ab`
(the then-current `dev` baseline), manifests, generated contracts, tests and workflow definitions.
It corrects source claims; it does not establish the current state of production,
published installers or unrecorded manual work. Concurrent changes on other
branches are outside this baseline.

## Source checks

| Topic corrected | Source used |
|---|---|
| Components and independent hosted/native ownership | [workspace](../Cargo.toml), [package scripts](../package.json), [architecture](architecture.md) |
| Configuration precedence, bounds, storage paths and explicit account reset | [config.rs](../apps/axiomcli/src/config.rs), [paths.rs](../apps/axiomcli/src/paths.rs), [session.rs](../apps/axiomcli/src/session.rs) |
| Fresh schema 1/application ID `0x41584133` and rejection of obsolete stores | [schema/migrations](../apps/axiomcli/src/session.rs), [versioning](versioning.md) |
| Local titles, edit/regenerate, staged compaction and stable user-submission ordering | [agent.rs](../apps/axiomcli/src/agent.rs), [ACP](../apps/axiomcli/src/acp.rs), [concurrency regressions](testing/concurrency-ui-2026-09-20/FIXES.md) |
| Desktop Agent on all packaged OS targets; Linux-only Bubblewrap isolation | [workspace.rs](../apps/axiomcli/src/workspace.rs), [policy.rs](../apps/axiomcli/src/policy.rs), [Desktop](desktop.md) |
| ACP 0.2 features, API-key creation and Tinfoil generation exception | [extension DTOs](../crates/axiom-acp-extension/src/lib.rs), [contract](acp-compatibility.md) |
| Mainnet ZEC deposits, valuation and `billing@2` | [billing.rs](../apps/axiomcli/src/billing.rs), [account UI](../apps/desktop/src/renderer/src/App.tsx) |
| NEAR/Phala/Tinfoil authentication and truthful evidence | [provider drivers](../crates/axiom-secure-client/src/providers), [security](threat-model.md) |
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

## Carry-forward work

The superseded account/invoice plans have been removed. Their remaining release
work is carried forward below and in [ADR 0014](adr/0014-clean-storage-and-deposit-billing.md).
The redesign's [open items](archive/desktop-redesign.md#6-open-items-for-the-developer)
remain separate.

| Area | Current disposition / evidence still needed |
|---|---|
| Original branch freeze and schema baseline | Historical setup, not a standing development stage. Record actual release revisions using CONTRIBUTING and current platform migrations. |
| Hosted staging, four auth methods and recovery/rollback | Requires deployment records and browser checks for the chosen release; source/tests alone cannot close it. |
| Billing edge cases | Revalidate paid/duplicate/under/over/mismatched/late/reconciled payments and partial-inference settlement for the backend's selected provider. Exercise persistent address provisioning, signed event replay, valuation and reorg review for Axiom Payments. |
| macOS/Windows interactive acceptance | Record vault, system-browser, passkey/Hello, signing, installer/uninstaller, account switching, refresh/logout, billing and authenticated inference outcomes per target. |
| Artifact evidence | Record OS/browser/architecture, commit, artifact hashes and signing identity with acceptance results. A successful headless package is insufficient. |
| Initial destructive production cutover | Retired proposal, not an executable runbook. The newly authorized September reset is defined in ADR 0014 and the platform reset runbook. Platform operations own database/namespace replacement, secret changes, smoke tests and recovery. |
| Rollback and retirement | Preserve evidence of a staging rollback drill, secret/key revocation and retention decisions for any still-active retirement work; do not infer completion from the old checklist. |
| Old hosted source removal | Native ownership is now enforced by the repository layout; hosted code belongs to `axiom-platform`. This does not prove an old production route or secret was retired. |
| Branding/design hand-off | The legal-line `TODO(brand)` remains in [main](../apps/desktop/src/main/index.ts); product-name/artwork decisions and optional layered icon/tray adoption need explicit design decisions. Current packaging uses `icon.icns`. |
| Old mock Proxy and missing chat actions | Superseded by the current native proxy integration and implemented conversation controls; retained only in the historical hand-off. |
| Provider availability/provenance canaries | Run attested E2EE canaries for the promoted native/backend pair. Old near-v2 or early Tinfoil measurements do not establish current deployment health. |

Use [release acceptance](releasing.md#interactive-acceptance) for a new release
and [platform operations](https://github.com/astrea-foundation/axiom-platform/blob/dev/docs/operations/deployment.md)
for backend rollout. Close an item with dated evidence rather than changing an
old proposal to imply that the check happened.

## Clean-baseline verification

The September reset removes retired invoice APIs/UI and old migration chains.
Local validation: 503 platform tests, all native workspace Rust tests, explicit
reset/plan CLI tests, TypeScript checks/tests, Desktop and auth builds, receiver
unit/integration tests and Compose validation. Browser regression checks are
recorded with the development PRs. Strict native Clippy remains blocked by
pre-existing warnings (including `axiom-inference` enum size and boolean-count
lints); ordinary Clippy completes. Live inference, funded deposits, OS-specific
acceptance and production cutover were not exercised by this source cleanup.
