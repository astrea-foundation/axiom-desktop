# Account, authentication, and invoice billing clean-slate plan

> Historical record, archived 2026-09-21. This preserves the original proposal
> or dated evidence, including superseded behavior and unfinished checklists.
> It is not a current implementation or deployment runbook. See the
> [current guides](../README.md) and [validation follow-up](../validation.md).

Status: headless implementation and available local Linux interactive checks complete; live-service,
native macOS/Windows release, and cutover gates remain. The original v1/pre-implementation plan is superseded by the account contract implemented
in ACP extension v0.2. No staging or production deployment has been performed.

Date: 2026-09-04

Scope:

- Desktop/TUI repository: `/home/salt/sync2/work/axiomai`
- API, legacy browser, deployment, and contract repository: `/home/salt/clone/axiom`
- Production API host: `api.axiom.stream`
- Hosted authentication surface: new, minimal `auth.axiom.stream`

Data policy: the account, authentication, chat-storage, and billing schemas are unreleased and may
be destroyed and recreated. There is intentionally no compatibility layer, data migration, dual
read, dual write, or preservation of old balances. A point-in-time operational backup is still
required immediately before the destructive production cutover so deployment rollback remains
possible.

Implementation snapshot (2026-09-04):

- Complete in the native repository: short access tokens plus rotating refresh sessions,
  origin-scoped OS-keyring storage and cross-process rotation locking, explicit automation API-key
  override, account-scoped CLI/Desktop SQLite routing, account-switch task/publication barriers,
  ACP v0.2 account and billing contracts, generated TypeScript client state, bounded Electron IPC,
  Desktop sign-in/account/balance/invoice presentation, and headless Rust/TypeScript adversarial
  tests.
- Complete for Desktop invoice intent handling: account-and-amount-scoped idempotency keys survive
  ambiguous timeout/cancellation/error and renderer restart; a definitive response or explicit
  account/amount change retires the pending intent. Failure to persist the durable retry key blocks
  dispatch instead of degrading to memory-only idempotency.
- Complete in the backend/auth repository: the sole PostgreSQL `0001` baseline, all four login
  methods and safe linking, browser/native/automation credential separation, the independent hosted
  auth surface, Shieldz invoice reconciliation, append-only ledger billing, fail-closed inference
  admission/settlement, generated OpenAPI, removal of the contract/biller/browser-chat/storage-E2EE
  model, and active-path residue audit.
- Complete for headless deployment preparation: isolated production/staging Worker, Tunnel,
  Compose, Terraform, managed-database, immutable-image, nonroot token-file, and secret-surface
  contracts validate without performing an external mutation.
- Still gated: real system-browser authentication, macOS Keychain and Windows Credential Manager,
  signed/notarized platform artifacts, staging auth/Shieldz/verified-inference exercises, and every
  production/Cloudflare/PostgreSQL cutover action. The user authorized interactive tests on
  September 4. Missing live-service credentials and platform/signing infrastructure—not the
  earlier restriction on opening windows—are now the outstanding dependencies.

Latest native headless verification (2026-09-04): generated ACP v0.2 bindings reproduced; Rust
format, all-target/all-feature check, Clippy with warnings denied, and dependency policy passed; 437
workspace tests passed with six deliberately ignored live/visual tests; the patched upstream ACP
small-stack regression passed; all 60 TypeScript tests passed; TypeScript type checking, the
production Electron bundle, optimized AxiomCLI/proxy binaries, and an unpacked Linux Desktop bundle
with its staged sidecar all built successfully. No app window, system browser, platform credential
prompt, live provider, signing service, or deployment was invoked.

Latest backend/auth/infra headless verification (2026-09-04): a fresh local PostgreSQL database
upgraded to the sole `0001`; all 304 backend tests and a repeated 131-test high-risk subset passed;
Ruff, compilation, dependency consistency, schema/residue checks, and deterministic OpenAPI
generation passed. OpenAPI SHA-256 is
`835c13156e41d6055ad08a3cea74fd83937f1440d408be2951d877b0eed779c3`. All 51 hosted-auth tests,
TypeScript checking, the production bundle, dependency audit, and production/staging Wrangler dry
runs passed. Both merged release Compose models, Terraform offline validation, the pinned
multi-architecture/nonroot cloudflared image, and the static release preflight passed. No SSH,
managed database, Shieldz account, provider, Cloudflare state, or deployment target was touched.

Subsequent interactive continuation (2026-09-04): read-only SSH inventory confirmed the old server
is still running. Local Linux GUI tests verified real native approval, OS-vault restoration,
WebAuthn with a virtual authenticator, email proof/link/login, SIWE with a disposable wallet,
two-account local collection isolation, and reset-triggered session revocation. This pass fixed
the development HTTPS mismatch, billing-outage sign-in failure, opaque billing errors, and stale
same-tab email tokens. The changed TypeScript areas pass 52 auth, 33 ACP-client, and 28 Desktop
tests plus type checks and builds. See [the full QA record](account-interactive-qa-2026-09-04.md)
for the simulated/real boundaries and remaining live/platform gates. No live deployment occurred.

## 1. Objective

Replace Axiom's wallet-derived pseudonymous identity and on-chain funding contract with a
conventional account model that supports independently linkable passkeys, Google, email/password,
and Ethereum wallet sign-in. Make the desktop application the product client: AxiomCLI owns account
sessions, local threads, provider attestation, and provider E2EE; Electron remains a sandboxed
presentation and process-supervision layer.

Replace Base/USDC contract deposits with prepaid Shieldz Zcash invoices. A verified paid invoice
credits an append-only account ledger; verified inference usage debits it. The backend continues to
relay provider ciphertext and operational metadata only. It must never receive plaintext prompts,
history, tool data, reasoning, or model output.

This plan assumes invoices are prepaid top-ups. Postpaid monthly invoicing, subscriptions, cloud
conversation sync, family/team accounts, organization billing, and automatic Zcash refunds are not
part of this refactor.

## 2. Current-state findings

### 2.1 Legacy account and storage model

The browser currently asks an Ethereum wallet to sign a fixed derivation message. It canonicalizes
the signature and derives an `ax_` account handle, an Ed25519 authentication key, and account storage
key material with HKDF. The backend hashes the account handle, accepts Ed25519 challenges, stores
account/device keyrings, and persists encrypted browser threads and messages.

Primary legacy seams:

- `frontend/src/lib/accountIdentity.ts`
- `frontend/src/lib/wallet.ts`
- `backend/src/axiom_api/auth/identity.py`
- `backend/src/axiom_api/auth/service.py`
- `backend/src/axiom_api/auth/routes.py`
- `backend/src/axiom_api/db/repositories/auth.py`
- `backend/src/axiom_api/encryption/`
- `backend/src/axiom_api/linking/`
- `backend/src/axiom_api/chats/`
- `backend/alembic/versions/0001_baseline.py`

### 2.2 Native authentication model

AxiomCLI already has the correct outer shape for native authorization:

1. Bind a random `127.0.0.1` callback port.
2. Create a PKCE verifier/challenge and one-time device secret.
3. Register a short-lived authorization with the API.
4. Open the system browser.
5. Poll until browser approval completes.
6. Exchange the authorization and persist a credential in the OS credential store.

The problem is the credential being issued: browser approval currently creates a long-lived `axm_`
API key. Account status is therefore API-key metadata (`keyId`, `keyName`) rather than a user
account. The relevant native seams are:

- `apps/axiomcli/src/auth.rs`
- `apps/axiomcli/src/provider.rs`
- `crates/axiom-secure-client/src/client.rs`
- `crates/axiom-secure-client/src/relay/client.rs`
- `crates/axiom-acp-extension/src/lib.rs`
- `protocol/axiom-acp-extension/v0.1/`
- `packages/axiom-acp-client/src/`
- `apps/desktop/src/main/agent-sidecar.ts`
- `apps/desktop/src/preload/agent-api.ts`
- `apps/desktop/src/renderer/src/signInFlow.ts`
- `apps/desktop/src/renderer/src/components/SignInDialog.tsx`

### 2.3 Desktop storage model

AxiomCLI already owns Desktop's SQLite threads, turns, timeline items, collections, and preferences.
Electron has no database connection. Local messages are plaintext at rest and rely on the OS user
and disk-encryption boundary. This is the intended replacement for browser storage E2EE.

The current database is frontend/installation-scoped (`desktop/state.sqlite3`), not account-scoped.
Without a change, signing out and signing in as a different account on the same OS profile would
expose the first account's local conversations to the second account.

### 2.4 Secure inference model

`axiom-secure-client` already performs model discovery, local TEE evidence verification,
provider-E2EE encryption/decryption, stream transcript validation, and terminal receipt validation.
The desktop calls the stateless relay endpoint `/api/v1/relay/chat/completions`; the relay explicitly
does not create server-side threads, messages, or runs.

This refactor removes account/storage E2EE. It must not remove, bypass, weaken, or relabel provider
E2EE. Fresh inference key material is not an account credential and remains mandatory.

### 2.5 Billing and deployment model

The legacy billing system derives a blinded `bytes32` account from the internal user ID and a server
secret, reads an `AxiomFunding` Base/USDC balance, reserves in-flight spend in PostgreSQL, aggregates
usage, and has a single biller process post charges on-chain.

Production currently has:

- `axiom-backend`, healthy, running an immutable image;
- `axiom-biller`, running the contract billing worker;
- `axiom-cloudflared`, routing the API without publishing port 8080 on the host;
- an external managed PostgreSQL database at Alembic revision `0001`;
- two pre-release account/session/keyring/API-key/billing-account records;
- no thread, message, run, usage, charge, hold, or billing-event records.

Cloudflare infrastructure is managed by Terraform under `infra/cloudflare/`. The static Worker is
deployed with repository-pinned Wrangler 4.110.0. Wrangler authentication and deployment history are
available. The production server is Ubuntu 24.04 and deploys Axiom with Docker Compose under
`/home/ubuntu/axiom`, reached through the existing Cloudflare Tunnel.

## 3. Non-negotiable invariants

1. Every remote inference request uses an explicitly registered provider-E2EE protocol after fresh,
   local verification of the complete provider TEE attestation chain.
2. No Axiom backend or Cloudflare Worker receives plaintext prompt text, history, reasoning, tool
   names/schemas/arguments/results, assistant deltas, or completion text.
3. A response becomes successful and durable only after its exact terminal receipt and complete
   encrypted transcript verify against the exact preverified worker/session.
4. `accounts.id` is an opaque server-generated identifier. It is never derived from a wallet,
   email address, Google profile, passkey, device, or installation.
5. A login identity is replaceable and linkable. No authentication method is the account primary
   key.
6. Identity linking requires an authenticated account plus fresh proof of the new method. Identity
   unlinking requires recent reauthentication and cannot remove the final usable method.
7. Accounts are never silently merged because two providers report the same email address.
8. Ethereum wallet authentication follows SIWE challenge semantics. Wallet signatures derive no
   account, encryption, billing, or transaction keys.
9. The normal Desktop/TUI session uses a short-lived access token and a rotating refresh token.
   Manually created `axm_` API keys remain a separate automation credential class.
10. Refresh tokens, API keys, OAuth codes, passwords, passkey assertions, SIWE signatures, provider
    secrets, Shieldz keys, webhook secrets, and invoice payment URLs never enter logs, SQLite
    timelines, ACP notifications, crash reports, analytics, or child-process environments.
11. Local conversations are isolated by authenticated account. Account switching closes the old
    database and clears all old account data from process and renderer state before publishing the
    new account.
12. An invoice credits an account exactly once, only after authenticated server-side confirmation.
    Client redirects, polling claims, screenshots, transaction strings, and renderer messages are
    never payment authority.
13. Balance changes are append-only ledger entries. Cached/projected balances are not the source of
    truth and cannot be edited independently.
14. Inference admission, holds, terminal usage debit, partial-usage debit, and hold release remain
    transactionally fail-closed under concurrency and process failure.
15. macOS and Windows absence or platform failure never enables plaintext credential storage,
    insecure browser embedding, unverified inference, or a weaker authentication fallback.
16. Electron's renderer remains sandboxed, context-isolated, without Node integration, and limited
    to a narrow typed preload surface.
17. No destructive local, Cloudflare, server, contract, or PostgreSQL action occurs merely by running
    tests, starting a development app, or installing an update.

## 4. Chosen target architecture

```text
System browser: auth.axiom.stream
  passkey | Google | email/password | SIWE
                   |
                   | approves one PKCE-bound native authorization
                   v
Electron renderer -> preload -> Electron main -> ACP -> AxiomCLI
                                                    |-- OS keyring: refresh token
                                                    |-- memory: access token
                                                    |-- per-account SQLite
                                                    `-- axiom-secure-client
                                                              |
                                                              | provider ciphertext only
                                                              v
                                                        api.axiom.stream
                                                          |-- accounts/identities
                                                          |-- native sessions
                                                          |-- invoices/ledger/holds
                                                          `-- attestation/relay/usage
                                                                    |
                                                                    v
                                                        attested model provider
```

The hosted authentication surface is a small security component, not a continuation of the browser
chat product. It contains only authentication, account linking/recovery, native-client approval,
legal links, and payment return/status pages. It owns no conversation UI or message state.

## 5. Target PostgreSQL contract

Use lowercase UUID text or native PostgreSQL UUID consistently. Prefer UUIDv7 when the selected
runtime/library provides a reviewed implementation; otherwise use cryptographically random UUIDv4.
Never expose email addresses, wallet addresses, or provider subjects as foreign keys.

### 5.1 Accounts and identities

`accounts`

- `id` primary key;
- `display_name` nullable, bounded Unicode text;
- `avatar_url` nullable and restricted to explicitly supported origins, or store a managed object ID;
- `status`: `active`, `disabled`, or `deletion_pending`;
- `created_at`, `updated_at`, `deleted_at`;
- no `account_id_hash`, root auth public key, encrypted profile fields, key namespace, or trial state.

`login_identities`

- `id` primary key;
- `account_id` foreign key;
- `kind`: `passkey`, `google`, `password`, or `ethereum`;
- `subject` normalized, provider-specific stable identifier;
- `label` nullable presentation label;
- `created_at`, `last_used_at`, `disabled_at`;
- unique active `(kind, subject)`;
- subjects are never returned wholesale in ordinary account status.

Provider-specific subject rules:

- Google: validated OIDC `sub`; email is profile/recovery metadata, not the identity key.
- Password: canonicalized verified email stored separately with a uniqueness constraint.
- Ethereum: normalized 20-byte address; chain ID is validated and recorded with the SIWE event but
  does not create a second identity for the same EVM address unless a future product requirement
  explicitly chooses chain-specific identities.
- Passkey: the account is resolved through the credential's random credential ID and WebAuthn user
  handle; no human-readable username is required for discoverable sign-in.

`passkey_credentials`

- `id`, `identity_id`, `account_id`;
- `credential_id` unique binary value;
- COSE public key;
- random non-PII `user_handle` bound to the account;
- signature counter and counter-risk state;
- transports, authenticator attachment, backup eligibility/state;
- creation and last-use timestamps;
- optional user-supplied device label;
- revocation timestamp.

Synced passkeys may legitimately leave counters at zero. A non-incrementing counter is a risk signal,
not an unconditional account lockout; all other WebAuthn verification remains strict.

`password_credentials`

- one active row per password identity;
- Argon2id PHC string with parameters embedded;
- password/version timestamps;
- verified-email timestamp;
- locked-until and failed-attempt metadata where useful;
- no recoverable password, security question, reversible encryption, or application log value.

`account_emails`

- normalized email and display form;
- verified timestamp;
- purpose flags for login, recovery, and notifications;
- unique normalized email when it is an active login identity;
- changing an email requires verification of the new address and recent authentication.

### 5.2 Challenges and browser state

Use separate tables/types instead of one loosely typed challenge table:

- `webauthn_challenges`: random challenge hash, ceremony, expected origin/RP ID, account or anonymous
  discovery context, expiry, consumed timestamp;
- `oauth_transactions`: provider, state hash, nonce hash, PKCE material where applicable, return path,
  expiry, consumed timestamp;
- `password_email_tokens`: purpose (`verify` or `reset`), token hash, account/identity, expiry,
  consumed timestamp;
- `siwe_challenges`: nonce hash, exact domain, URI, allowed chain, issued/expiry/not-before times,
  expected address when linking, consumed timestamp;
- `web_sessions`: browser session token hash, account, creation/expiry, last authentication time,
  revocation state, CSRF secret hash, and bounded client metadata.

Every challenge is one-time, short-lived, purpose-bound, rate-limited, and consumed transactionally.
Failures reveal the same public result for existing and nonexistent email identities.

### 5.3 Native authorizations and sessions

Rename `cli_authorizations` to `native_authorizations`, because both Axiom Desktop and AxiomCLI use
it. Preserve the current device-secret, PKCE, user-code, callback, polling, and cancellation
properties while changing the result.

`native_authorizations`

- ID and hashed one-time device secret;
- PKCE S256 challenge;
- random state binding;
- exact numeric loopback callback URI;
- installation ID hash and bounded client/platform label;
- optional login-method hint;
- `pending`, `approved`, `consumed`, `expired`, `cancelled` state;
- account ID only after approval;
- timestamps and single-use exchange result.

`native_sessions`

- session and account IDs;
- installation ID hash;
- refresh-token family ID;
- current refresh-token hash;
- creation, last-use, absolute expiry, idle expiry, and revocation timestamps;
- client platform/version metadata with strict length bounds;
- optional reason for administrative revocation.

`access_tokens`

- opaque random token hash, session/account IDs, audience, issued/expiry timestamps, revocation state;
- 10-15 minute lifetime;
- no refresh token in an access-token response after initial exchange/rotation.

Refresh behavior:

1. AxiomCLI stores only the current refresh token in the OS credential store.
2. `/auth/token/refresh` verifies and consumes the current token in one transaction.
3. It rotates both the token and stored hash and returns a fresh access token.
4. Reuse of a consumed refresh token revokes the entire token family.
5. Concurrent refreshes have one winner; the loser receives a terminal reauthentication result, not
   a second valid family branch.
6. Logout revokes the native session and deletes the local refresh token. Logout-all revokes every
   browser/native session and API key only after explicit confirmation.

Keep `api_keys` as a distinct table for automation. Manual keys remain hash-only, individually named,
scoped, revocable, and returned exactly once. Native browser approval must never create one.

### 5.4 Invoice billing and ledger

`payment_invoices`

- internal ID and account ID;
- provider fixed to `shieldz` in the first release;
- Shieldz invoice ID unique;
- server-generated request idempotency key unique;
- requested/confirmed USD cents;
- merchant USDC settlement chain, asset, and destination fingerprint resolved by Shieldz at creation,
  then pinned per invoice; never accept a settlement override from a Desktop request;
- `pending`, `paid`, `expired`, `cancelled`, `failed`, or `review` state;
- payment URL stored encrypted or omitted after terminal settlement;
- provider creation/expiry/paid timestamps;
- response/payload digest and bounded non-secret error classification;
- no email, wallet address, or account ID placed into public provider metadata unless required.

`payment_webhook_events`

- provider event identifier when supplied, otherwise a canonical event digest;
- timestamp and payload digest;
- received/verified/processed timestamps;
- terminal processing result;
- unique provider event/digest for replay defense;
- raw bodies retained only when required for dispute/debug policy, encrypted and time-limited.

`account_ledger_entries`

- ID, account ID, signed `amount_microusd`, and resulting sequence;
- kind: `invoice_credit`, `inference_debit`, `promotional_credit`, `manual_adjustment`, or
  `refund_adjustment`;
- source type and source ID;
- immutable creation timestamp and bounded operator reason for manual entries;
- unique `(kind, source_type, source_id)` to make posting idempotent;
- nonzero amount and sign constraints by kind.

`account_balances`

- account ID, posted balance, monotonically increasing ledger sequence, update timestamp;
- transactionally updated projection, never independently editable;
- periodic invariant job compares it to the full ledger sum and pages on any mismatch.

`billing_inflight_holds`

- hold/request ID, account ID, maximum reserved micro-USD, creation/expiry/renewal timestamps;
- no blinded billing account or chain address;
- unique request ID and positive amount;
- inserted only while holding an account-balance row lock.

`usage_events` and `usage_aggregates`

- retain provider/model/token/cost accounting without conversation content;
- one event per terminal or partial billed relay request;
- unique relay request/source ID;
- debit and usage event commit in the same transaction;
- aggregate tables remain derived reporting state and can be rebuilt.

Available balance is:

```text
posted ledger balance - sum(active, unexpired holds)
```

Invoice settlement is one transaction: lock invoice, validate exact expected amount/currency and a
legal state transition, insert the unique positive ledger entry, update the balance projection, and
mark the invoice paid. Duplicate webhooks and reconciliation must return the existing result without
posting again.

Inference settlement is one transaction: lock hold and balance, insert the usage event and unique
negative ledger entry, update the balance projection, and delete the hold. If exact terminal usage
cannot be proven after provider dispatch, preserve a conservative hold until bounded expiry/recovery
instead of silently granting free usage.

## 6. Authentication method behavior

### 6.1 Passkey

- Serve ceremonies only from `https://auth.axiom.stream` with RP ID `auth.axiom.stream`.
- Validate exact origin, RP ID hash, challenge, ceremony type, user-presence and user-verification
  policy, flags, signature, and credential/account binding.
- Registration requests a discoverable credential and user verification.
- A passkey can create an account directly or be linked from an authenticated account.
- Support multiple passkeys per account and user-visible names/revocation.
- Do not require attestation conveyance unless a later enterprise policy needs it.
- Never confuse authenticator attestation with model-worker TEE attestation.

### 6.2 Google

- Use OAuth 2.0 Authorization Code and OIDC with state and nonce validation.
- Validate issuer, audience, nonce, expiry, signature, and hosted-domain policy only if explicitly
  configured.
- Key identity by Google `sub`, not mutable email.
- Store no Google access/refresh token unless a product feature needs Google APIs; authentication
  alone requires only validated identity claims.
- An existing email match offers an explicit sign-in-and-link flow; it never silently merges.

### 6.3 Email/password

- Require email verification before paid inference or account recovery.
- Accept long passphrases, Unicode, password-manager paste, and at least 64 characters; impose a
  documented upper bound that prevents denial-of-service.
- Use Argon2id through a reviewed library with production-calibrated memory/time/parallelism and PHC
  parameter storage.
- Use generic login/reset responses, per-IP and per-identity rate limits, escalating delays, and
  optional Cloudflare Turnstile on abuse paths.
- Hash verification/reset tokens and expire/consume them atomically.
- Do not force periodic password changes. Revoke native/browser sessions after a password reset,
  except optionally the session completing the reset after explicit confirmation.
- Select and configure a transactional email provider before this method exits staging.

### 6.4 Ethereum wallet

- Implement EIP-4361 SIWE, not the legacy fixed derivation message.
- Bind exact domain, URI, version, chain ID, nonce, issued-at, expiration, request/session ID, and
  statement.
- Recover and normalize the signer address server-side and compare every signed field.
- Consume each nonce once and reject stale, future, wrong-domain, wrong-chain, or malformed messages.
- Use injected-wallet or WalletConnect support only in the hosted auth surface.
- Treat wallet login and future crypto transaction permissions as separate authority classes.
- Zcash invoices are independent of the Ethereum identity used to authenticate.

## 7. Native protocol and AxiomCLI refactor

### 7.1 Replace `AuthManager` with session semantics

Keep the serialized mutation lane, monotonic publication revision, cancellation behavior, loopback
validation, secret redaction, and unavailable-keyring behavior in `apps/axiomcli/src/auth.rs`.
Replace:

- `credential: Option<SecretString>` with refresh-token state plus a short-lived access-token cache;
- `key_id`/`key_name` status with account profile and session metadata;
- `/api/v1/auth/api-key` validation with `/api/v1/auth/session` or `/api/v1/auth/me`;
- `/api/v1/auth/cli/*` with `/api/v1/auth/native/*`;
- browser exchange returning `axm_` with native session exchange returning access/refresh tokens;
- keyring item `inference-api-key` with a versioned native refresh-token item.

Define a narrow `AccessTokenSource` abstraction used by Axiom's secure HTTP client. The refresh
protocol stays in AxiomCLI, not in provider adapters. A secure operation obtains an access token with
a minimum remaining lifetime before model discovery/attestation begins. A 401 may trigger one refresh
and retry only before provider ciphertext could have been dispatched. Never replay a possibly sent
inference merely because a token expired or the HTTP response was lost.

`AXIOM_API_KEY` remains an explicit automation override and bypasses native refresh behavior. It
must not be persisted, printed, or exposed to the renderer.

### 7.2 Account-aware ACP extension

Bump the Axiom extension/account feature contract without compatibility aliases. Regenerate Rust
schema and TypeScript from the authoritative Rust DTOs.

Target account status shape:

```text
AccountStatus {
  revision
  state: starting | signed_out | valid | expired | unavailable
  account?: {
    id
    displayName?
    avatarUrl?
    verifiedEmail?
    linkedMethods[]
  }
  session?: {
    expiresAt
    credentialStore
  }
  detail?
}
```

Target methods:

- `_axiom/account/status`
- `_axiom/account/native_login_start { methodHint? }`
- `_axiom/account/native_login_complete { loginId }`
- `_axiom/account/native_login_cancel { loginId }`
- `_axiom/account/logout`
- `_axiom/account/logout_all` only after a distinct confirmation contract exists
- `_axiom/billing/status`
- `_axiom/billing/invoice_create { amountUsdCents, clientRequestId }`
- `_axiom/billing/invoice_status { invoiceId }`

Remove `_axiom/account/use_api_key` from the Desktop preload. Manual API-key entry may remain in the
TUI/CLI command surface but must use an explicitly separate method/capability unavailable to the
sandboxed Desktop renderer.

Account and billing changes use monotonic revisions and typed activity events. A runtime-instance
change or sequence gap triggers authoritative replacement. No ACP payload contains refresh/access
tokens or invoice provider secrets.

### 7.3 Account context and local SQLite

Target layout, using platform-native roots from the `directories` crate:

```text
<data>/axiom/
  shared/identity/client-id
  shared/trust-policy.json
  accounts/<opaque-account-id>/cli/state.sqlite3
  accounts/<opaque-account-id>/desktop/state.sqlite3
  accounts/<opaque-account-id>/desktop/chat/
```

The keyring remains outside these directories. No email, display name, wallet address, or Google
subject appears in a path.

Introduce an `AccountContext`/`SessionStoreRouter` owned by AxiomCLI:

1. Startup initializes authentication and publishes signed-out state without opening an account DB.
2. Valid account status activates exactly that account's canonical data directory.
3. Activation opens/validates SQLite before publishing threads.
4. Login/account switch first cancels active work, drains persistence, closes the old connection,
   clears runtime/ACP state, then opens the new store.
5. Logout clears renderer/client state before or atomically with publication of signed-out state.
6. New chat, load, prompt, collection, and transcript calls fail with typed authentication-required
   errors when no account store is active.
7. A store-open failure never falls back to another account or installation-wide database.

Because no local user data must be preserved, select a new SQLite baseline/application ID and require
an explicit development reset for old `desktop/state.sqlite3` and `cli/state.sqlite3`. Do not scan,
import, rename, or guess ownership of old files. Preserve the current transactional turn model,
interruption recovery, sequence/revision checks, redaction, owner-only Unix permissions, and WAL
behavior.

## 8. Electron desktop changes

Do not begin renderer work until the existing `integrate/redesign` dirty worktree is committed or
moved to a clean branch. Account work must not rewrite or discard those changes.

Electron remains presentation-only:

- main supervises the AxiomCLI sidecar and opens external HTTPS URLs;
- preload exposes bounded typed intents;
- renderer displays methods/status and never receives tokens;
- passwords, Google codes, passkey assertions, and SIWE signatures remain in the system browser;
- billing UI receives only account balance, invoice amount/status/expiry, and an HTTPS pay URL;
- the renderer cannot mark an invoice paid or adjust a balance.

Desktop sign-in may show four method buttons, but each supplies only a `methodHint` when creating the
same native authorization. The system-browser page permits switching methods and remains the source
of method-specific authentication. Keep polling authoritative so loopback notification failure does
not strand login.

When an account context changes, the TypeScript state reducer performs one atomic reset of sessions,
catalog, active thread, optimistic items, collections, model errors, billing state, and cached account
presentation. No old-account UI frame may be visible after new-account status is published.

## 9. macOS and Windows requirements

### 9.1 Shared requirements

- Build and test AxiomCLI and Electron for x64 and arm64 where packaging claims those architectures.
- Use the system browser; never an embedded webview for Google, passkey, password, wallet, or invoice
  approval.
- Bind only `127.0.0.1` on a random unprivileged port. Validate the exact callback path, state,
  authorization ID, HTTP method, host/port, origin behavior, request size, and timeout.
- Polling remains sufficient when a browser blocks the callback, a firewall interferes, the default
  browser is remote, or the callback arrives before the app is foregrounded.
- Refresh tokens remain small and bounded for both credential stores. Reject unexpected token sizes
  before persistence.
- Account switching must tolerate SQLite WAL/antivirus/indexer delays with bounded retries and a
  visible error; it must not open both account stores concurrently.
- Installation/update/uninstall never deletes account SQLite automatically.
- Uninstall documentation explains how to remove local account data and credential-store entries
  intentionally.

### 9.2 macOS

- Persist refresh tokens as generic passwords in macOS Keychain through the Rust `keyring` backend.
- Do not silently fall back to a plaintext/protected file when Keychain is locked, denied, or
  unavailable. Run signed out and present recovery guidance.
- Verify behavior on Apple Silicon and Intel, Keychain locked/unlocked, first access, overwrite,
  logout deletion, application update, and a changed code signature/team identity.
- Verify Safari and Chrome system-browser flows plus iCloud Keychain passkeys.
- Preserve hardened runtime, app sandboxing boundaries actually in use, DMG/ZIP packaging, code
  signing, notarization, and Gatekeeper verification.
- Review entitlements and Keychain access groups before signing; do not depend on an entitlement
  available only to development builds.
- Verify app activation after browser approval, native menu shortcuts, single-instance behavior, and
  sidecar shutdown during logout/account switching.

### 9.3 Windows

- Persist refresh tokens in Windows Credential Manager through the Rust `keyring` backend.
- Never use registry, config TOML, command-line arguments, environment inheritance, or SQLite as a
  fallback credential store.
- Verify Windows Hello passkeys through Edge and Chrome.
- Verify callback behavior under Defender Firewall, proxy/VPN use, non-administrator installs, paths
  containing spaces/non-ASCII text, and antivirus scanning of SQLite/sidecar files.
- Preserve `.exe` sidecar resolution, kill-on-close Job Object behavior, per-user NSIS install, Start
  menu/shortcut identity, and single-instance behavior.
- Add Authenticode signing and installer verification before Windows becomes a supported release;
  unsigned portability builds are not production artifacts.
- Test x64 on Windows 10/11 and arm64 on actual or supported emulated hardware before claiming arm64.

### 9.4 Platform CI and release gates

Expand the current macOS/Windows portability matrix beyond `cargo check`:

- Rust formatting/check and targeted auth/path/session tests;
- ACP extension generation and schema no-diff check;
- Node install from lockfile, Desktop typecheck, and renderer/main tests;
- unpacked Electron build with the correct platform sidecar name;
- callback parser/property tests using Windows and Unix line endings;
- mock credential-store tests for success, missing, locked, denied, timeout, rotation, and deletion;
- account-store switching and cross-account non-disclosure tests.

Real Keychain/Credential Manager and signed installer tests require dedicated release machines or
manual release evidence; hosted CI mocks do not establish platform support.

## 10. Hosted auth and Cloudflare plan

Create a small independent auth asset/Worker project instead of extending the legacy browser chat
bundle. Its route is `auth.axiom.stream/*`; `axiom.stream` may later become marketing/download-only.

Cloudflare/Terraform changes:

1. Add the `auth.axiom.stream` DNS record and explicit Worker route.
2. Add a separately named Worker/config (`axiom-auth`) so auth can deploy and roll back without the
   marketing site.
3. Pin its compatibility date and Wrangler version in the repository.
4. Use immutable hashed assets, no-cache HTML, strict CSP, HSTS, frame denial, nosniff, restrictive
   referrer and permissions policies.
5. Allow outbound requests only to `api.axiom.stream` and required first-party asset origins.
6. Put no Google client secret, password pepper, email-provider key, Shieldz key, webhook secret,
   database URL, or provider credential in Worker assets or public environment bindings.
7. Add `https://auth.axiom.stream` to API CORS with exact-origin credential support; remove the old
   application origin when the legacy site is retired.
8. Register exact Google redirect URIs and WebAuthn origin/RP-ID configuration for production and a
   separate staging hostname.
9. Apply Cloudflare rate limiting/WAF rules to authentication and webhook paths as defense in depth;
   retain authoritative application/database rate limits.
10. Keep the existing Cloudflare Tunnel for `api.axiom.stream`; do not expose backend port 8080.

Deployment scripts must perform Wrangler dry-run, tests, an authenticated staging deployment, and an
explicit production deploy. Record the prior Worker version for one-command rollback. Never source or
print broad `.env` files; pass the minimum needed variables to each process.

## 11. Shieldz integration plan

Add a narrow `ShieldzClient` behind a provider-neutral invoice interface:

- bounded connect/request timeouts and response sizes;
- server-only API key;
- idempotent invoice creation;
- strict HTTPS base URL validation;
- typed response parsing and allowlisted statuses;
- no redirects to untrusted payment origins without validation;
- safe error classification that excludes remote bodies and secrets;
- injectable clock/client for deterministic tests.

The first release uses Shieldz API-key mode with USDC settlement configured in the merchant dashboard.
The payer may use Zcash or another supported checkout asset; Shieldz handles conversion. Do not send
a settlement override from Axiom or let the renderer, account profile, query string, or invoice body
choose the merchant destination. The authenticated create response supplies the settlement tuple,
which is pinned per invoice and must match every later confirmation. A real SDK test invoice confirmed
`BASE` / `BASE.USDC`; normalize only `USDC` and the matching chain-qualified USDC identifier, and reject
other payout assets. Axiom does not need a local Zcash receiving address or customer payment keys.

Backend endpoints:

- `GET /api/v1/billing/status`
- `POST /api/v1/billing/invoices`
- `GET /api/v1/billing/invoices/{id}`
- `POST /api/v1/webhooks/shieldz`

Invoice creation accepts an allowlisted USD-cent amount or a bounded product-defined range and a
client request ID. The backend owns the Shieldz idempotency key. Return only internal invoice ID,
amount, state, expiry, and validated HTTPS `pay_url`.

Webhook processing:

1. Read the bounded raw body exactly once.
2. Parse `X-Shieldz-Signature` as `t=<unix>,v1=<hex>` and verify
   `HMAC-SHA256(secret, "<t>.<raw-body>")` with constant-time comparison; accept any matching `v1`
   during documented key rotation.
3. Reject timestamps outside the configured replay window and deduplicate the
   `X-Shieldz-Delivery` value transactionally.
4. Parse only after authentication.
5. Find the internal invoice by exact provider invoice ID.
6. Compare USD cents, the invoice's pinned merchant USDC settlement tuple, environment mode, and legal transition.
7. Post credit transactionally and idempotently.
8. Return success for a verified duplicate already processed.
9. Put unknown invoice IDs or amount/status mismatches into `review`, emit an alert, and post no
   credit.

Implement periodic server-to-server reconciliation for pending/review invoices so missed webhooks
recover. Replace the chain `axiom-biller` with an optional horizontally safe payment reconciler using
row claims/advisory locking; it owns no wallet private key or nonce. Webhooks remain the low-latency
path.

Required server configuration:

- `SHIELDZ_API_BASE_URL`
- `SHIELDZ_API_KEY`
- `SHIELDZ_WEBHOOK_SECRET`
- `SHIELDZ_MODE` (`test` for staging, `live` for production)
- USDC merchant settlement configured in Shieldz; no local `SHIELDZ_SETTLEMENT_*` overrides
- invoice min/max and expiry policy
- webhook replay window
- reconciliation cadence and batch limit

Do not reuse or rename old chain secrets into these values. Provision new secrets, test them in
staging, then remove old biller private key, RPC URL, contract address, account blinding secret,
chain ID, confirmation, batch, and sweep variables from local examples, Compose, server `.env`, and
secret stores.

## 12. Backend API and repository restructuring

### Keep and adapt

- provider registry and explicit provider/model/protocol contracts;
- verification snapshots and expiring attestation leases;
- provider evidence endpoints;
- signed trust policy;
- stateless ciphertext relay;
- pricing, stream metering, usage normalization, and partial-usage handling;
- database-backed in-flight reservations, rewritten against the ledger;
- API/body/provider response bounds and rate limiting.

### Replace

- account bootstrap/challenge/verify with method-specific auth services;
- cookie-only `AuthAccount` with one account principal resolved from browser session, native access
  token, or manual API key;
- CLI API-key issuance with native session issuance;
- encrypted profile fields with ordinary bounded account profile fields;
- on-chain balance reads and billing service with ledger balance/holds;
- contract biller worker with Shieldz webhook/reconciliation;
- old Alembic baseline with one final clean-slate baseline.

### Delete

- wallet-derived `ax_` account derivation and Ed25519 account authentication;
- account/device keyrings, device cryptographic linking, and related schemas/routes/tests;
- backend browser threads, messages, persistent chat runs, dispatches, regenerate/edit paths, and
  server chat reconstruction;
- encryption-key namespace/config and account-storage encryption endpoints;
- blinded billing account derivation and all chain clients;
- `AxiomFunding` Solidity code, ABIs, Foundry scripts/tests, deployment manifests, refund/sweep/gas
  scripts, and contract documentation;
- biller Docker service and hot-wallet secret surface;
- old frontend chat/account/funding code after the independent auth surface is live;
- compatibility DTOs, aliases, tests, and documentation for the retired model.

Keep negative tests that reject plaintext inference and unauthenticated/unattested provider paths.

## 13. Implementation phases and dependency gates

### Phase 0: freeze contracts and prepare isolated branches

These preparation entries remain historical and intentionally unchecked: implementation began on
the user's existing dirty worktrees, so creating or claiming clean base branches after the fact
would be misleading. They are not remaining product-code work.

- [ ] Commit or isolate the existing Desktop redesign work; record its base commit.
- [ ] Create dedicated backend/auth-billing and native-account branches/worktrees.
- [ ] Record OpenAPI, ACP schema, current DB schema, Cloudflare Worker version, API image digest, and
      production environment-variable names without recording values.
- [ ] Confirm prepaid invoice semantics, supported top-up amounts, ledger overdraft policy, account
      deletion/financial retention policy, transactional email provider, Google project, and Shieldz
      production/staging credentials.
- [ ] Disable creation of real external users during the destructive pre-release window if the
      service is publicly reachable.

Exit gate: written contract decisions are approved and both worktrees are clean enough to prevent
unrelated changes from entering account commits.

### Phase 1: executable contracts and new PostgreSQL baseline

- [x] Write new account/auth/billing DDL as the sole Alembic `0001` baseline.
- [x] Add all uniqueness, foreign-key, state, sign, size, expiry, and idempotency constraints.
- [x] Add repository methods with explicit transactions and `SELECT ... FOR UPDATE` where required.
- [x] Generate OpenAPI and schema-level tests from a fresh local PostgreSQL database.
- [x] Add concurrency tests for identity uniqueness, refresh rotation, invoice settlement, ledger
      posting, inference admission, and partial usage.
- [x] Do not modify production PostgreSQL; implementation and verification were local/headless only.

Exit gate: empty PostgreSQL upgrades successfully; schema audit finds none of the retired tables or
columns; repository concurrency tests pass.

### Phase 2: browser authentication services

- [x] Implement browser sessions, CSRF, challenge storage, rate limits, and security-event auditing.
- [x] Implement passkey registration/discovery/verification.
- [x] Implement Google OIDC start/callback/verification.
- [x] Implement password signup/login/email verification/reset.
- [x] Implement SIWE challenge/verification.
- [x] Implement authenticated link/unlink/list flows with recent-auth requirements.
- [x] Implement disabled/deleted account and logout/logout-all behavior.

Exit gate: every method can create an independent account, link to an existing account only with
proof, and cannot collide, auto-merge, replay, or remove the final login method.

### Phase 3: independent Cloudflare auth surface

- [x] Build the minimal auth Worker/assets with no browser chat dependencies.
- [x] Add CSP/security headers, exact API CORS, and bounded production configuration.
- [ ] Add and deploy the staging DNS/route.
- [ ] Exercise all four methods through system browsers.
- [ ] Test failed/cancelled/expired/native-approval recovery.
- [x] Perform the local Wrangler dry-run without deploying.
- [ ] Perform an authenticated staging deployment and deployment rollback drill.

Exit gate: staging authentication works independently of the old website and exposes no secrets in
assets, bindings, responses, browser storage, or logs.

### Phase 4: native session backend

- [x] Implement native authorization create/read/approve/exchange/cancel.
- [x] Issue short-lived access and rotating refresh tokens, never API keys.
- [x] Implement refresh reuse detection, revocation, account status, and logout.
- [x] Accept native access tokens and manual API keys through a unified bounded account principal on
      relay/evidence/model/billing endpoints.
- [x] Preserve explicit authorization header precedence; an invalid bearer must not fall back to a
      browser cookie.

Exit gate: protocol tests cover polling without callback, callback without polling race, cancellation,
expiry, refresh concurrency/reuse, revocation, and redaction.

### Phase 5: AxiomCLI auth and secure-client integration

- [x] Refactor `auth.rs` from permanent API-key storage to native session management.
- [x] Add asynchronous access-token acquisition/refresh with serialized mutation.
- [x] Update secure-client credential injection without changing provider verification/E2EE.
- [x] Preserve API-key automation override as a distinct path.
- [x] Update TUI `/login`, `/logout`, `/account`, startup validation, doctor, configuration, and
      troubleshooting output.
- [x] Prove in headless tests that no token enters child environments, command args, ACP, journals,
      exports, or Debug.

Exit gate: live staging model discovery, fresh attestation, encrypted completion, receipt verification,
token refresh, logout, and revoked-session failure all pass.

Headless implementation is complete. This live-staging exit gate remains open.

### Phase 6: account-scoped local state

- [x] Implement `AccountContext` and per-account paths for CLI and Desktop.
- [x] Start signed out without opening the old install-scoped databases.
- [x] Implement atomic in-memory/ACP reset and store close/open on account changes.
- [x] Reject prompts and thread operations while signed out or switching.
- [x] Add explicit pre-release reset command for old local databases; do not migrate them.
- [x] Add cross-account isolation, crash recovery, WAL close, and failed-switch tests.

Exit gate: account B cannot observe any account A thread, collection, title, model preference,
optimistic item, error, or active task before, during, or after switching/restart.

### Phase 7: ACP and Electron integration

- [x] Replace account feature DTOs and regenerate JSON Schema/TypeScript.
- [x] Add billing DTOs and activity events.
- [x] Update the state reducer's account-context reset semantics.
- [x] Update Electron main/preload wrappers with bounded non-secret methods.
- [x] Update Desktop sign-in/account/billing UI after rebasing onto the completed redesign.
- [x] Keep all auth and payment URLs external and validate HTTPS/origin before `shell.openExternal`.

Exit gate: renderer process inspection and transcript fixtures prove no credential/assertion/password,
cross-account state, or payment authority crosses the preload.

### Phase 8: Shieldz invoices and ledger billing

- [x] Implement provider-neutral invoice service and Shieldz client.
- [x] Implement create/status/webhook/reconciliation paths.
- [x] Implement idempotent invoice credit, balance projection, and invariant checks.
- [x] Rewrite inference admission/hold/settlement against the ledger.
- [x] Add Desktop balance and top-up flows through ACP, including durable lost-response idempotency
      reuse and explicit-user-action checkout opening. The single **Top up** gesture now creates
      or resumes checkout and opens it automatically, with browser-launch retry reuse,
      **Reopen checkout** recovery, and background confirmation/balance refresh (2026-09-05).
- [ ] Exercise paid, duplicate, expired, under/over/mismatched, delayed webhook, reconciliation, and
      partial inference cases in staging.

Exit gate: no client-controlled input can create balance; every paid invoice credits exactly once;
concurrent inference cannot overspend; terminal/partial usage debits exactly once.

The backend and native headless contracts/tests are complete. The staging exercise above, including
real Shieldz settlement credentials and provider callbacks, remains open.

### Phase 9: legacy removal and residue audit

- [x] Delete active backend account derivation, storage E2EE, server chat, keyring/linking, and
      contract code.
- [x] Remove old dependencies (`WalletConnect` from active product code unless used by auth,
      chain-only
      Python/Solidity dependencies, and unused crypto packages).
- [x] Remove obsolete routes from FastAPI and deployment health/smoke tests.
- [x] Remove biller Compose service and chain environment variables.
- [x] Remove Solidity/ABI/script contract artifacts from active build, test, and deployment paths.
- [ ] Delete the old website source tree. It is intentionally frozen and excluded from the new
      product/deployment path for now, rather than rewritten or deleted during this implementation.
- [x] Rewrite privacy, threat model, configuration, deployment, troubleshooting, and account docs.
      Public landing-page legal copy remains frozen with the old website until its separately
      approved replacement/deletion.
- [x] Search code, tests, schemas, generated artifacts, docs, Compose, and examples for every retired
      identifier listed in Section 17.

Exit gate: only SIWE auth references an Ethereum wallet; only provider E2EE references inference
encryption keys; only manual automation paths reference `axm_` API keys.

The headless residue search is clean for active native/backend/deployment paths. Remaining matches
are confined to the frozen website, this plan and its explicitly superseded historical predecessor,
negative schema/preflight and secret-redaction tests, SIWE authentication, and the intentional
manual-automation API-key path.

### Phase 10: cross-platform validation

- [x] Expand CI for macOS Intel/Apple Silicon and Windows x64/ARM64 Rust, ACP, TypeScript, release
      sidecar, and unpacked Desktop build/architecture checks.
- [ ] Complete real macOS Keychain/system-browser/passkey/signing/notarization matrix.
- [ ] Complete real Windows Credential Manager/Hello/Defender/NSIS/Authenticode matrix.
- [ ] Run account switching, refresh, logout, invoice, and verified inference on both platforms.
- [ ] Record artifact hashes, OS versions, architectures, browsers, and test outcomes.

Exit gate: macOS and Windows support claims are made only for configurations with equivalent
credential, callback, packaging, process, storage, and secure-inference evidence.

### Phase 11: staging and destructive production cutover

- [ ] Deploy new API/auth Worker against an isolated staging database.
- [ ] Run clean-schema, auth, Shieldz, ledger, secure inference, Cloudflare, and Desktop release
      candidate tests.
- [ ] Announce/enable a maintenance window; block old auth and inference admission.
- [ ] Capture a managed PostgreSQL point-in-time backup plus schema-only dump and record the old API
      image and Worker version. This backup is for deployment rollback, not application migration.
- [ ] Confirm the two existing production account records remain disposable and that no new usage or
      payment rows appeared since inventory.
- [ ] Stop `axiom-biller` before schema replacement.
- [ ] Recreate the production application schema with the new sole `0001` using the migration-owner
      credential, never the runtime credential.
- [ ] Provision new server secrets and deploy the backend without chain secrets.
- [ ] Deploy `auth.axiom.stream`, Terraform DNS/route changes, and exact CORS/redirect configuration.
- [ ] Run server-local health, public Tunnel health, auth-method, native exchange/refresh, invoice,
      ledger, and verified E2EE inference smoke tests.
- [ ] Deploy signed/notarized Desktop builds only after the backend/auth smoke gate passes.
- [ ] Monitor errors, refresh failures, invalid webhooks, ledger invariants, and paid inference.
- [ ] Remove old Cloudflare app route and contract secrets only after the observation window.

Exit gate: production has only the new schema and account model; the biller is absent; Shieldz and
verified inference work; old clients/routes fail clearly rather than partially operating.

### Phase 12: rollback and retirement

Before destructive schema replacement, rollback is ordinary API/Worker rollback. After replacement,
the only valid rollback is a coordinated rollback of database backup, API image, server environment,
Cloudflare Worker/routes, and client availability. Never run the old API against the new schema or
the new API against the old schema.

- [x] Document exact rollback commands without embedding secret values:
      [current platform deployment and recovery](https://github.com/astrea-foundation/axiom-platform/blob/dev/docs/operations/deployment.md).
- [ ] Drill rollback in staging.
- [x] Define the financial no-return point and forward-repair requirement after real new-model
      accounts, usage, or payments are accepted; see the cutover runbook.
- [ ] After the retention window, securely remove obsolete database backups and retired chain/API
      secrets from the server and operator stores.
- [ ] Revoke the biller wallet key and old API keys where applicable; do not merely delete local
      copies.
- [ ] Archive or remove contract deployment artifacts according to legal/accounting needs, while
      keeping them outside active application/deployment paths.

## 14. Test matrix

### Backend authentication

- fresh signup and login for each method;
- discoverable passkey login and multiple passkeys;
- WebAuthn wrong RP/origin/challenge/type/signature/user handle and replay;
- Google wrong state/nonce/issuer/audience/signature/expiry and mutable email;
- password hash calibration, Unicode/length limits, enumeration resistance, rate limit, verify/reset
  expiry and replay;
- SIWE wrong domain/URI/chain/address/nonce/time/version/signature and replay;
- link collision, explicit link, unlink final method, recent-auth expiry, disabled/deleted account;
- browser session fixation/CSRF/logout/logout-all;
- native authorization callback/poll races, PKCE mismatch, device-secret mismatch, expiry/cancel;
- access expiry, refresh rotation, concurrent refresh, reuse-family revocation, credential redaction;
- manual API key scopes/revocation remain independent of native sessions.

### Billing

- invoice amount/range/idempotency, per-invoice merchant USDC settlement pinning, test/live mode separation, and
  provider response bounds;
- invalid/stale/malformed webhook and constant-time signature comparison;
- duplicate webhook, webhook/reconciliation race, paid-after-expiry provider state;
- unknown invoice, wrong account, amount/currency mismatch, state regression;
- atomic ledger/balance posting and invariant rebuild;
- concurrent invoice credits and inference admissions;
- hold renewal/expiry, exact terminal debit, partial debit, missing usage, cancellation, process death;
- no negative available balance unless an explicit bounded overdraft product decision is later added;
- logs and public errors contain no Shieldz secret, pay URL, raw body, or sensitive transaction data.

### Desktop/native

- keyring load/save/rotate/delete/unavailable/locked/timeout;
- signed-out startup and no local DB opened;
- login opens correct account DB; logout closes and clears it;
- account switch during idle, model loading, active stream, compaction, and pending interaction;
- sidecar restart during login/refresh/store switch;
- callback unavailable while polling succeeds;
- access refresh before catalog/evidence/inference and no ambiguous inference replay;
- no token/password/assertion/signature in ACP, IPC, state snapshots, stderr, panic, export, or SQLite;
- account A/B cross-disclosure property tests;
- macOS/Windows path, Unicode, permissions, file locking, installer/update, and browser matrices.

### Provider security regression

- every model catalog entry requires a registered provider/protocol tuple;
- fresh nonce-bound attestation and exact key/session binding;
- stale/invalid/mismatched evidence fails before encryption/send;
- missing, wrong-signer, truncated, reordered, or mismatched terminal receipt fails closed;
- provisional stream content is never persisted as a verified successful completion;
- auth refresh and billing changes cannot introduce a TLS-only/plaintext provider path;
- backend request/log/DB inspection proves message plaintext never arrives.

### Deployment

- fresh local PostgreSQL and isolated managed staging database;
- schema version and constraint audit;
- Docker Compose config with no biller/chain secret surface;
- Cloudflare Worker dry-run, staging deploy, CSP, CORS, DNS, Tunnel, rollback;
- server health without host port 8080 exposure;
- new API image fails fast when required auth/Shieldz configuration is missing;
- rollback restores matched schema/API/Worker versions.

## 15. Observability and operations

Emit bounded, low-cardinality metrics for:

- auth starts/success/failure by method and safe failure class;
- challenge replay/expiry, refresh/reuse revocation, and account lock/disable;
- native authorization age and completion mode (poll/callback);
- invoice create/status transitions, verified/invalid/duplicate webhooks, reconciliation lag;
- ledger post failures, balance invariant mismatch, active/expired holds;
- relay admission, insufficient balance, terminal/partial usage, provider security failures;
- Desktop auth/store-switch failures by OS and app version.

Security/audit events use internal event IDs and a one-way account correlation fingerprint. Do not log
raw emails, wallet addresses, Google subjects, WebAuthn credential IDs/assertions, SIWE messages or
signatures, passwords, tokens, payment URLs, webhook bodies, provider ciphertext, or attestation raw
artifacts. Operator tools must require explicit reason and authorization for manual ledger entries or
account disablement.

Create runbooks for:

- Google or email-provider outage;
- WebAuthn origin/RP misconfiguration;
- refresh-token theft/reuse alert;
- Shieldz outage, invalid webhook spike, missed webhook, or reconciliation backlog;
- ledger projection mismatch;
- insufficient-balance false positive;
- Keychain/Credential Manager outage;
- Cloudflare Worker/Tunnel rollback;
- managed PostgreSQL restore;
- provider attestation or receipt failure.

## 16. Documentation and policy updates

Update at minimum:

- root and backend READMEs;
- `docs/configuration.md` and environment examples;
- `docs/privacy.md` and threat model;
- `docs/platform-support.md`, `docs/releasing.md`, and versioning policy;
- ACP extension and Desktop parity documentation;
- Cloudflare and API deployment documentation;
- troubleshooting and account-recovery documentation;
- public privacy/terms disclosures for account identifiers, authentication providers, local thread
  storage, Shieldz invoice processing, and financial-record retention.

Documentation must clearly distinguish:

- account authentication credentials;
- local conversation storage;
- provider TEE attestation/E2EE keys;
- manual API keys;
- Ethereum sign-in identities;
- Zcash invoice payments;
- future wallet transaction permissions.

## 17. Legacy residue audit

Before completion, repository-wide searches must find no production references to the following
outside explicit historical/negative-test documentation:

- `axiom-chat-v1`
- `accountDerivationMessage`
- `deriveAccountIdentity`
- `canonicalSignature`
- wallet-derived `ax_` identity
- account/device `auth_public_key` authentication
- `account_keyrings`, `device_keyrings`, `link_sessions`
- browser `threads`, `messages`, `runs`, `run_dispatches`, or regenerate endpoints
- `ENCRYPTION_KEY_NAMESPACE`
- `derive_billing_account`
- `billing_accounts`, `billing_batches`, `billing_charges`
- `AxiomFunding`, funding ABI, biller role, biller private key
- chain RPC/ID/contract/sweep/confirmation configuration
- Desktop browser login exchange issuing an `axm_` key
- `keyId`/`keyName` as user account presentation
- Desktop `useApiKey` preload method
- plaintext or TLS-only provider inference fallback

Manual `axm_` automation keys, SIWE Ethereum authentication, historical migration notes, and tests
that prove rejection may remain only where their purpose is explicit.

## 18. Completion gate

The refactor is complete only when:

1. A fresh PostgreSQL database contains only the new account/auth/native-session/invoice/ledger,
   usage, relay, pricing, and attestation contract.
2. Passkey, Google, email/password, and Ethereum login each create or authenticate the same kind of
   stable account and can be explicitly linked/unlinked safely.
3. Native browser authorization issues rotating account sessions rather than API keys.
4. Desktop and CLI local histories are isolated per authenticated account and never stored by the
   backend.
5. Shieldz paid invoices credit exactly once and inference debits exactly once under concurrency,
   retry, webhook replay, cancellation, and process failure.
6. The smart contract, chain billing worker, blinded accounts, storage keyrings, and server browser
   chat paths are absent from production code and deployment.
7. Provider TEE attestation and provider E2EE pass the complete regression matrix with no plaintext
   fallback.
8. macOS and Windows meet the credential-store, browser callback, local storage, sidecar, packaging,
   signing, installer, and verified-inference gates appropriate to their claimed support.
9. Cloudflare auth/API routes, managed PostgreSQL schema, API image, and signed Desktop release are
   deployed as one recorded compatible release set with a tested rollback set.
10. All generated contracts reproduce without diff; Rust, Python, TypeScript, Electron, backend,
    integration, cross-platform, live provider, and deployment tests pass; the Section 17 residue
    audit is clean.

## 19. Reference standards and provider contracts

- WebAuthn Level 3: <https://www.w3.org/TR/webauthn-3/>
- Google OAuth 2.0 for installed applications: <https://developers.google.com/identity/protocols/oauth2/native-app>
- Sign-In with Ethereum (EIP-4361): <https://eips.ethereum.org/EIPS/eip-4361>
- OWASP Password Storage Cheat Sheet: <https://cheatsheetseries.owasp.org/cheatsheets/Password_Storage_Cheat_Sheet.html>
- Shieldz invoice/API documentation: <https://shieldz.cash/docs>
- Shieldz OpenAPI contract: <https://shieldz.cash/openapi.json>
