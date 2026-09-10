# Interactive authentication QA

The account redesign was reviewed in the isolated preview with sample data. Production-build
browser regressions cover the account API boundaries with mocked HTTP responses; light/dark
desktop/mobile captures cover the page layouts. See the app README for reproducible commands.

The provider and OS integrations below remain a separate release checklist. Those live flows
were not rerun as part of the design review. Preview completion does not establish that a real
passkey, external provider, payment, or native session was created.

## Test safety and setup

- Use staging API/auth origins, a staging Google client, non-production email recipients, disposable
  Ethereum accounts, and Shieldz test mode. Never paste secrets into screenshots or issue logs.
- Capture browser/OS/version, pass/fail, safe failure class, and a redacted screenshot where useful.
- Keep browser developer tools closed while entering passwords, tokens, wallet signatures, or
  payment data. Inspect the sanitized network/storage record only after the secret-bearing step.
- Verify no test starts a model request; authentication QA must not exercise plaintext or non-E2EE
  inference.

## macOS matrix

- Safari and Chrome: create and discoverably sign in with a Touch ID/iCloud Keychain passkey.
- Cancel, time out, and deny both passkey creation and assertion; verify generic recovery UI.
- Open the native authorization URL from the signed/notarized Desktop build, approve it, and verify
  the exact `127.0.0.1` or `[::1]` callback returns focus without a warning or second authorization.
- Block the callback listener and confirm polling still completes, without repeated browser tabs.
- Verify sign-out/session revocation against macOS Keychain behavior in the Desktop checklist.

## Windows matrix

- Edge and Chrome: create and discoverably sign in with Windows Hello; repeat with a security key if
  supported by the release policy.
- Cancel, time out, and deny both passkey creation and assertion; verify generic recovery UI.
- Open the native authorization URL from the signed Desktop build, approve it, and verify numeric
  IPv4 and bracketed IPv6 loopback behavior under Windows Defender Firewall.
- Block the callback listener and confirm polling still completes, without repeated browser tabs.
- Verify sign-out/session revocation against Windows Credential Manager behavior in the Desktop
  checklist.

## Authentication methods

- Google: first account creation, returning sign-in, explicit linking, cancellation, provider error,
  wrong account selection, and collision with a Google identity already linked elsewhere.
- Email/password: signup, generic resend/recovery response, email verification, login, reset,
  expired/replayed links, Unicode/long password-manager paste, and authenticated email-method link.
- SIWE: first sign-in/account creation and explicit link with supported injected wallets; reject the
  signature and inspect the statement before accepting it. Exercise wrong chain, account change,
  rejected signature, stale nonce, and a wallet already linked elsewhere.
- Passkey: multiple credentials, user-visible labels, final-method removal refusal, and explicit
  removal after a recent reauthentication.
- Confirm no matching Google/email profile causes a silent account merge.

## Native approval

- Compare the user code, client name, platform, and initiating device against Desktop before approve.
- Approve, cancel, expire, replay, and open an altered `authorization_id` or `state` link.
- Sign in during a pending approval with each supported method and confirm the exact original request
  remains bound after the round trip.
- Reject named-localhost, non-loopback, alternate-port substitution, alternate callback path, query,
  fragment, and HTTPS callback attempts.
- Confirm approval creates a native session and never displays or downloads an `axm_` API key.

## Account and invoice UI

- Link/unlink every method, try to remove the final method, and exercise recent-auth expiry.
- Sign out here and sign out everywhere; verify browser, Desktop, CLI, and automation API-key
  revocation behavior.
- Create each enabled top-up amount in Shieldz test mode, cancel checkout, return pending, settle,
  refresh to paid, expire, and review. Verify only the exact `https://shieldz.cash/pay/…` target opens.
- Confirm a return URL/query cannot change amount, account, asset, destination, or invoice state.
- Retry after a simulated dropped invoice-create response and verify the idempotency key returns the
  same invoice rather than creating a duplicate.

## Browser security and presentation

- Inspect response headers for CSP, HSTS, frame denial, no-referrer, nosniff, cross-origin policies,
  Permissions Policy, no-store HTML, and immutable hashed assets.
- Attempt framing, inline script/style injection, mixed content, an off-origin API request, and a
  non-allowlisted Google/Shieldz redirect; all must fail closed.
- Inspect cookies, local/session storage, IndexedDB, Cache Storage, console, URLs, history, network
  entries, and crash/error reports for tokens, assertions, signatures, passwords, and payment URLs.
- Keyboard-only navigation, screen-reader labels/status changes, 200% zoom, reduced motion, high
  contrast, narrow window, and both OS display scaling defaults.
- Visual review on macOS and Windows for clipping, focus rings, native font fallback, error states,
  long translated/profile strings, and Desktop-to-browser handoff clarity.

## Release and rollback

- Run the complete matrix against `auth-staging.axiom.stream` before production approval.
- Perform an authenticated staging Worker deployment and rollback to the recorded prior version.
- Confirm exact production/staging DNS, CORS, Google redirect URI, WebAuthn origin/RP ID, WAF/rate
  limit, and Tunnel configuration without exposing backend port 8080.
- Production deployment and any real-payment smoke test require a separate explicit approval.
