# Hosted authentication API contract

This file is the integration boundary between the independent `auth.axiom.stream` surface and the
Axiom API. Paths and JSON fields are versioned application contracts; browser pages do not call
legacy wallet-derivation, chat-storage, account-keyring, or API-key issuance endpoints.

## Transport and browser session

- API origin: `https://api.axiom.stream` (separate staging origin in staging builds).
- Auth origin: `https://auth.axiom.stream`; WebAuthn RP ID: `auth.axiom.stream`.
- Browser requests use `credentials: include`. The API allows only the exact auth origin with
  credentialed CORS and must include `Vary: Origin`.
- The browser session is an `HttpOnly; Secure` cookie. JavaScript receives no session token.
- `GET /api/v1/auth/session` returns `{authenticated, account?, session?, csrf_token?}`. The CSRF
  token is kept only in memory and sent as `X-CSRF-Token` on cookie-authenticated mutations.
- An invalid `Authorization` header never falls back to the browser cookie.
- Errors use a bounded allowlisted `code`; UI code never renders a remote `detail` or response body.

Account summary:

```json
{
  "id": "opaque-account-id",
  "display_name": "Ada",
  "avatar_url": null,
  "verified_email": "ada@example.test",
  "linked_methods": ["passkey", "google", "password", "ethereum"]
}
```

## Password and email

All bodies are JSON.

| Method | Path | Request |
| --- | --- | --- |
| POST | `/api/v1/auth/password/signup` | `{email,display_name?}` starts an ownership proof; creates no account or password |
| POST | `/api/v1/auth/password/link` | Authenticated link with `{email}`; binds proof to the exact recent browser session |
| POST | `/api/v1/auth/password/login` | `{email,password}` starts a required email-code challenge; HTTP 202, no session |
| POST | `/api/v1/auth/password/login/verify` | `{challenge_id,code}` verifies six ASCII digits and creates the session |
| POST | `/api/v1/auth/password/login/resend` | `{challenge_id}` replaces the pending code after the cooldown |
| POST | `/api/v1/auth/password/verify/request` | `{email}` |
| POST | `/api/v1/auth/password/verify` | `{token,new_password}` atomically creates or links the verified credential |
| POST | `/api/v1/auth/password/reset/request` | `{email}` |
| POST | `/api/v1/auth/password/reset` | `{token,new_password}` |

Signup, resend, and reset run the same delivery path for existing and nonexistent addresses. The
server stores only a short-lived pending email proof until verification; it does not let an
unverified caller reserve an account, identity, or password. Link completion requires the same
still-active, recently authenticated browser session and CSRF token that started it. Email links carry the
one-time token in the URL fragment (`/verify-email#token=...`, `/reset-password#token=...`), not the
query string, so it is absent from HTTP request logs and referrers. The page removes the fragment
from visible history after reading it.

Password login always requires both the password and the email code. The first response is
`{authenticated:false,challenge_id,expires_at,resend_after_seconds}` and sets only a short-lived
HttpOnly, Secure, SameSite=Strict browser-binding cookie. Keep the challenge ID in memory;
submit the code using the same browser and `credentials: include`. Only `/login/verify` returns
the authenticated session/CSRF response. A password-only session response is rejected by the UI.

Codes expire after 10 minutes, allow five attempts per challenge, and are stored as a
purpose-separated keyed HMAC. Resends require a 60-second cooldown; five sends and ten failed
code guesses per identity per rolling 15 minutes are enforced in PostgreSQL across replicas.
Resending consumes the old challenge and rotates the browser binding. Password changes, identity
disable/unlink, account disable/deletion, and logout-everywhere invalidate pending proofs.
Code consumption and session issuance share an account-locked transaction.

Signup verification links expire after 24 hours and reset links after one hour by default;
email copy uses the actual configured lifetimes. A successful password reset revokes existing
browser/native sessions and API keys and atomically queues a password-changed confirmation.
Delivery retries independently; a provider outage does not undo a completed reset.

## Passkeys

| Method | Path | Request |
| --- | --- | --- |
| POST | `/api/v1/auth/passkeys/registration/options` | `{display_name?,device_label?}`; ambient authenticated session means link |
| POST | `/api/v1/auth/passkeys/registration/verification` | `{challenge_id,credential,device_label?}` |
| POST | `/api/v1/auth/passkeys/authentication/options` | `{intent:"signin"}` |
| POST | `/api/v1/auth/passkeys/authentication/verification` | `{challenge_id,credential}` |

Option responses contain `{challenge_id,public_key}`. Binary WebAuthn fields are unpadded
base64url. Registration requests a discoverable credential with user verification. The API validates
the exact origin/RP ID, transaction, challenge, flags, signature, credential binding, and one-time
consumption. An ambient authenticated session turns registration into explicit linking and requires
the browser CSRF header and recent authentication.

## Google OIDC

- `GET /api/v1/auth/google/start?return_to=<relative path>` starts sign-in/account creation.
  `return_to` must be an allowlisted auth-origin relative path.
- `POST /api/v1/auth/google/link/start?return_to=<relative path>` requires CSRF and returns an
  `authorization_url` for explicit linking. The UI validates the exact Google origin before leaving.
- `GET /api/v1/auth/google/callback` is the registered Google callback. It validates state, nonce,
  issuer, audience, signature, and expiry before redirecting back to the auth origin with only a
  bounded public status (`auth=cancelled` or `auth=failed` on errors).
- Linking requires an authenticated browser transaction. Matching email never merges accounts.

## SIWE

| Method | Path | Request |
| --- | --- | --- |
| POST | `/api/v1/auth/siwe/challenge` | `{address,chain_id,intent:"signin"\|"link"}` |
| POST | `/api/v1/auth/siwe/verify` | `{challenge_id,message,signature}` |

The API constructs the exact EIP-4361 message. The UI asks the injected wallet to sign that exact
string with `personal_sign`; it does not derive any account, payment, or encryption key. Linking
requires CSRF and recent authentication.

## Identity and session management

| Method | Path | Behavior |
| --- | --- | --- |
| GET | `/api/v1/auth/identities` | Return `{identities:[...]}` with bounded, redacted summaries |
| DELETE | `/api/v1/auth/identities/{identity_id}` | Unlink after CSRF/recent-auth/final-method checks |
| POST | `/api/v1/auth/logout` | Revoke this browser session |
| POST | `/api/v1/auth/logout-all` | `{confirmation:"LOG_OUT_ALL"}` revokes all browser/native sessions and automation API keys |
| DELETE | `/api/v1/auth/session` | Revoke the current browser or native session; browser calls require CSRF |
| DELETE | `/api/v1/auth/account` | `{confirmation:"DELETE_ACCOUNT"}` disables the account after CSRF and recent authentication |

Method-specific registration/start endpoints perform linking only with an authenticated session and
CSRF proof; there is no generic identity merge endpoint.

## Native desktop approval

- The Desktop opens `/native/authorize?authorization_id=<authorization-id>&state=<opaque-state>` in the system
  browser.
- `GET /api/v1/auth/native/authorizations/{id}?state=...` returns a bounded, non-secret request
  summary for the authenticated browser account.
- `POST /api/v1/auth/native/authorizations/{id}/approve` with `{state}` and the CSRF header approves
  the request once and returns `{status:"approved",callback_uri?}`.
- A callback URI, when returned, must be exactly `http://127.0.0.1:<port>/callback` or bracketed
  `http://[::1]:<port>/callback`. The UI rejects named hosts, HTTPS, credentials, missing ports,
  alternate paths, queries, and fragments before navigation.
- Polling remains authoritative if the loopback callback is unavailable. Approval issues a native
  session during the separate PKCE/device-secret exchange; it never creates an `axm_` API key.

## Payment status shell

| Method | Path | Behavior |
| --- | --- | --- |
| GET | `/api/v1/billing/status` | Posted/held/available micro-USD projection and allowed invoice range |
| POST | `/api/v1/billing/invoices` | `{amount_usd_cents,client_request_id}` creates an idempotent, server-fixed Shieldz/Zcash invoice |
| GET | `/api/v1/billing/invoices/{id}` | Read an account-owned invoice status |

The creation response may include `pay_url` only while the invoice is pending. The UI navigates only
to the exact origin `https://shieldz.cash` with a `/pay/` path; subdomains, alternate ports,
credentials, fragments, and other paths fail closed.

The return page is informational. Only authenticated webhook/reconciliation settlement may post the
unique positive ledger entry. The UI never accepts an invoice state, amount, chain, asset, address,
or checkout URL from its own query string.
