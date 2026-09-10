# Axiom account website

The product-owned source for `auth.axiom.stream`, including sign-in, account
creation, recovery, native desktop approval, account settings and legal pages.
This app supersedes the hosted frontend previously kept in the Axiom API
repository's `auth/` directory. The API stays in that repository. The previous frontend remains a rollback reference. This app was deployed
to production on September 9, 2026; see `docs/website-release-2026-09-09.md`
in the repository root for versions and validation.

The website uses `@axiom/brand` (the actual redesign's wordmarks, symbol, local
fonts and palette) and `@axiom/ui` (surfaces, notices, loading and confirmation
dialogs). Axiom Desktop also consumes the shared fonts and symbol. Auth uses
only UI and account APIs; it has no conversation storage or inference code.

## Develop and review

From the `axiomai` repository root:

```sh
pnpm install
pnpm auth                 # real application; uses configured API origin
pnpm auth:review          # isolated design preview, http://127.0.0.1:4174/signin
```

Run one server at a time on 4174. The design preview has a page selector with
all routes and representative states. It uses fictional account data, blocks
live fetches and wallet access, and does not create accounts, send emails,
charge payments or connect a native device. Sample email forms demonstrate
success; email sign-in requires the sample code `123456`, and passkey controls demonstrate cancellation without opening an OS prompt.
The theme control works on every app page. Theme choice stays in memory; a
fresh page follows the OS preference.

Preview code lives in `review/`, with a separate HTML entry and Vite config.
It is never imported by the production entry. `build` emits `dist/`, while
`build:review` emits `dist-review/`. Never deploy the latter to an auth origin.
The static review build covers hosted pages. Local callback previews are
served by the review dev server from the exact shared native HTML template.

```sh
pnpm --filter @axiom/auth test
pnpm --filter @axiom/auth build
pnpm --filter @axiom/auth test:browser
pnpm --filter @axiom/auth build:review
# With auth:review running:
pnpm --filter @axiom/auth capture
```

Captures and an HTML gallery are written to `dist/auth-review/` at the repo
root. They cover light/dark, desktop/mobile, and additional confirmation and
email completion states. The browser regression script starts its own server
on 4176 and exercises the **production build**, mocking HTTP at the browser
boundary; it checks native return paths and CSRF, account confirmations,
recovery/token handling, server-authoritative payment state and preview exclusion.

## Routes

`/signin` (also `/`), `/signup`, `/recover`, `/reset-password`, `/verify-email`,
`/native/authorize`, `/account`, `/signup?intent=link`, `/payments/return`,
`/privacy`, `/terms`. Missing document routes render a branded 404 with status
404; API and missing asset requests continue to fail with a plain 404.

The payment-return route is an invoice status flow. Current mainnet ZEC
funding remains in Desktop and is presented according to the server's
valuation flag. The redesign does not enable legacy invoice checkout.
Legal wording was retained; presentation now matches the product website.

## Deployment

The Worker retains the production/staging hostnames and existing account API
origins. Build and inspect the normal production output before deployment:

```sh
pnpm --filter @axiom/auth build
pnpm --filter @axiom/auth exec wrangler deploy --env= --dry-run --profile axiom --env-file /dev/null
# When deploying the reviewed change:
pnpm --filter @axiom/auth exec wrangler deploy --env staging --profile axiom --env-file /dev/null
pnpm --filter @axiom/auth exec wrangler deploy --env= --profile axiom --env-file /dev/null
```

Use the authenticated Wrangler profile for the intended Cloudflare account.
No backend secrets belong in this workspace or the Worker. Existing callback
validation, CSRF handling, token-fragment clearing, no-store HTML, restrictive
CSP and bounded account API responses are retained. The production API client
contains no preview fixture path or bypass.

The native loopback page is generated from `packages/brand` and compiled into
AxiomCLI. It requires a future desktop/CLI build to reach installed clients;
the hosted redesign itself can be deployed without replacing installers.
See `API_CONTRACT.md` and `MANUAL_QA.md` for provider and OS integration details.

Password sign-in retains the production email-code challenge. Passwords alone
do not create a browser session; resend cooldowns, expiry, and verification
errors are presented before account or native approval can proceed.
