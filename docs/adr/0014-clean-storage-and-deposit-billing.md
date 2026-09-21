# ADR 0014: Fresh account storage and deposit-only billing

Status: accepted for the September 2026 development reset. Supersedes the old
clean-slate plans and the invoice billing portion of ADR 0011.

Billing scope later adds gift credit through [ADR 0017](0017-gift-credit-controls.md)
and `billing@3`; this ADR retains the original reset/deposit decision.

## Decision

Axiom keeps its current account-scoped architecture and removes the retired
invoice experiment. Billing uses persistent mainnet ZEC deposits through Axiom
Payments. NEAR v3, Phala ACI and Tinfoil attestation, authenticated E2EE completion,
request accounting, native-session credentials and account-switch barriers remain.

`billing@2` exposes credit status, required trial/paid buckets and nullable deposit
and current-quote metadata. The held-credit placeholder, invoice DTOs, creation and
status methods, checkout IPC/URL capabilities, retry storage and invoice UI are
removed. Older peers cannot negotiate the new billing feature. Native and hosted
backend contracts ship as one compatible batch. Market quotes are indicative;
only authenticated backend settlement grants credit. No wallet custody crosses ACP.

Local SQLite starts with application ID `0x41584133` and schema version `1`. The
baseline contains threads, turns, timeline items, collections, preferences, Agent
settings, both activity clocks and request usage. Old receipt-binding repairs and
activity backfills are removed. Current receipt verification/commit rules and
account ownership checks remain. Old application IDs and unsupported versions
fail closed. No old conversation or credential is imported by opening the store.

## Reset

Stop all Desktop and CLI processes that use the target account. Reset only its
selected frontend database:

```sh
axiomcli reset-local-state --account ACCOUNT_ID --frontend desktop-chat --confirm
axiomcli reset-local-state --account ACCOUNT_ID --frontend cli --confirm
```

The command runs before authentication or database loading, so an obsolete schema
cannot prevent reset. It validates the account path and deletes SQLite plus its
WAL/SHM/journal files. Configuration, other accounts/frontends, credential-vault
entries, trust policy and workspace files remain separate. Reset does not perform
a backend account deletion. New backend accounts after the service reset have new
IDs; old native sessions are invalid and require sign-in again.

The platform and receiver also use fresh PostgreSQL baselines. Follow the
[coordinated service reset](https://github.com/astrea-foundation/axiom-platform/blob/dev/docs/operations/clean-baseline.md)
for namespace, address, ledger and deployment sequencing. No production promotion
or live database erasure is implied by this source change.

## Validation and release work

Fresh/reopened stores, rejection of retired layouts, reset scoping, authenticated
message persistence, request settlement, per-account state, generated ACP schemas
and desktop deposit rendering are covered by local checks. Run Rust and TypeScript
checks and regenerate contracts before publishing.

The retired plans' remaining operational gates still apply to each release:
real system-browser sign-in and failure/cancellation/expiry flows for all four
methods, macOS Keychain and Windows Credential Manager, signed/notarized installers,
staging mainnet deposits and event replay, live attested E2EE, and a coordinated
cutover/recovery drill. Historical local QA is not proof of those release gates.
Future schema changes must add forward migrations from this baseline and preserve
released data unless a new explicit reset is authorized.
