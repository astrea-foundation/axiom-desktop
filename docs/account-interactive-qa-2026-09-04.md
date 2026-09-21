# Account refactor: interactive QA, September 4, 2026

Status: available Linux local integration checks passed. This is not a production deployment,
hardware-passkey certification, or macOS/Windows release approval.

## Environment and boundaries

- Real Axiom Desktop renderer/main process and native `axiomcli` sidecar, using Electron 37.10.3.
- A fresh PostgreSQL database with the new sole baseline and the real account/authentication API.
- Isolated XDG directories and a unique API origin; the user's existing Desktop data was not used.
- Local HTTPS with a disposable CA trusted only by the test processes. No TLS/attestation bypass
  was added to the product. Inference against a local provider/relay remained fail-closed.
- The new minimal hosted auth surface ran in a separate sandboxed Electron browser window, not the
  old website. Approval used the real loopback callback and native token exchange.
- The passkey authenticator and EIP-1193 wallet were simulated, but WebAuthn and SIWE signatures
  were cryptographically verified by the real backend. The wallet was disposable and unfunded;
  there were no wallet transactions.
- Email capabilities came from the existing test-mode memory mailer. This did not test Resend
  delivery. Billing was deliberately unconfigured; it did not simulate a paid invoice or credit.
- No provider prompt was sent. The native client's local-address rejection was preserved.

The first GUI launch exposed a missing local development Electron install. The matching cached
Electron archive was extracted to the disposable test directory, without changing global trust or
the user's installed application. Browser-test Chromium download attempts timed out; these results
therefore do not claim a normal system-browser or browser-extension end-to-end test.

## Verified flows

| Check | Result |
| --- | --- |
| Four-method Desktop sign-in chooser | Passkey, Google, email/password, Ethereum shown |
| Passkey signup and later passkey authentication | Same stable account; real server verification |
| Email linking to an existing passkey account | Ownership proof then password enrollment; account unchanged |
| Independent email signup | Second account created after ownership verification |
| Ethereum identity linking and sign-in | Real SIWE signature verification; original account retained |
| Native approval and callback | Browser approval exchanged into a native session, not an API key |
| OS credential storage | Linux system credential store used; account restored after process restart |
| Billing outage during login/restart | Login remains successful; balance stays unknown, not zero |
| Explicit billing refresh failure | Useful failure detail shown without the Electron IPC wrapper |
| Sign-out | First account's local collection disappears from the renderer |
| Sign back into original account | Original collection restored |
| Switch to second account | First account's collection not visible |
| Password reset | Browser reset succeeds; Desktop refresh observes revoked session and clears account state |
| Replacement email capability in the same tab | New token replaces stale token; invalid token fails closed |
| Verification/reset URL hygiene | Capability fragment cleared from address bar |

The interactive isolation probe used a local collection, not an inference conversation. The
previous headless native tests remain the evidence for complete thread/history routing and
in-flight account-switch publication barriers.

## Fixes produced by this pass

1. Desktop development origin validation now accepts locally trusted HTTPS and rejects local HTTP
   API configuration at the boundary, matching the native transport requirements. Packaged builds
   remain pinned to production; provider attestation/E2EE policy is unchanged.
2. Optional billing refresh no longer fails required local-state initialization or a completed
   native sign-in. Local-store failures still fail closed, explicit billing calls still reject,
   and missing balance data is not fabricated. A focused regression test covers both cases.
3. Billing/native-login errors preserve the native client's useful error detail; the Settings
   balance error removes IPC wrapping.
4. Auth verification/reset pages handle same-document fragment navigation and reset form state
   when the capability changes. A fragment-watcher regression and real browser verification pass.

## Automated verification after these changes

- Hosted auth: 52 tests passed; TypeScript check and production build passed.
- ACP TypeScript client: 33 tests passed; TypeScript check passed.
- Desktop: 28 tests passed; TypeScript check and Electron bundle build passed.
- No Rust, backend schema, or provider-security implementation was changed in this pass, so the
  earlier full Rust/backend verification is not represented as a newly repeated run.

The production auth build and Desktop main/preload/renderer bundles were regenerated after the
fixes. Production/staging Worker dry-runs, static deployment preflight, and whitespace checks also
passed. These are local builds, not uploaded releases.

## Live prerequisites still missing

Read-only SSH inspection succeeded. The production host still runs the old backend and biller;
they were not stopped or modified. The checked local and server Axiom environment files contained
Cloudflare/PostgreSQL/inference-provider settings, but no usable Google OAuth, Resend, or Shieldz
configuration. The operator was asked for credential file paths, not secret values in chat.

Remaining gates:

- Google OAuth registration and actual sign-in; actual email delivery and recovery links.
- Shieldz test/live credentials, webhooks, merchant USDC settlement configuration, and paid-invoice
  settlement/replay/reconciliation against the deployed API.
- Staging deployment, fresh attested E2EE inference, production cutover, and staging rollback drill.
- Real system browser, physical/platform passkeys, macOS Keychain and Windows Credential Manager,
  signed/notarized installers, native platform CI, and release-set publication.
- Approved infrastructure state backend and complete private release environment files.

The coordinated rollback runbook is in the API repository at
[current platform deployment and recovery](https://github.com/astrea-foundation/axiom-platform/blob/dev/docs/operations/deployment.md). Documentation is complete; its live commands
have not been executed. Test windows were closed and the disposable native credential was cleared.

### Shieldz credential follow-up

The subsequently supplied Shieldz test key was saved outside both repositories in an owner-only
configuration file. An authenticated, read-only `GET /api/v1/invoices?limit=1` returned HTTP 200
with an empty invoice list. No invoice was created and no webhook or production setting was changed.
The test-key prerequisite is now satisfied. The earlier request for a local shielded Zcash settlement
address was incorrect: customer payment currency and merchant payout currency are separate.

### Merchant USDC settlement correction

Using the official Python SDK and the supplied test key, a short-lived $1 sandbox invoice was
created and retrieved without a settlement override. Shieldz returned merchant settlement
`BASE` / `BASE.USDC` and its configured destination. No real payment or merchant-setting change was
made. Axiom now resolves the USDC settlement from that authenticated response, records its chain and
destination fingerprint per invoice, and rejects changes before crediting. Local settlement-wallet
settings were removed; native billing validation and desktop copy now use USDC. The real webhook
signing secret is still needed for an end-to-end callback test; invoice creation does not need it.

Validation for this correction: 372 backend tests, 3 native billing tests, 33 ACP client tests,
28 desktop tests, desktop type-check/build, Rust formatting, and deployment static preflight passed.
The corrected application retrieved and validated the same sandbox invoice as `BASE` / `USDC`
after expiry. The clean-slate schema was tested against an isolated fresh database; no deployed
database, merchant setting, webhook endpoint, or production service was changed. No OS-specific
behavior was added; actual Windows/macOS installer and credential-store checks remain separate gates.
