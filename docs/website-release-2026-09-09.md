# Website and account release — September 9, 2026

Production deployments use Cloudflare account `e6b5ee9a1dc91f1eec4659442cd2dce9`
and the `axiom` Wrangler OAuth profile.

| Surface | Source | Worker | Version |
| --- | --- | --- | --- |
| https://axiom.stream | `landing/` | `axiom-frontend` | `2a3efea8-66b9-4734-b29a-2612444fb5e3` |
| https://auth.axiom.stream | `apps/auth/` | `axiom-auth` | `c71fdde5-e665-46e1-9fde-24140f3c9594` |
| https://auth-staging.axiom.stream | `apps/auth/` | `axiom-auth-staging` | `5c0809ad-2013-457a-b3a4-17294437245e` |

Staging account pages were subsequently synchronized to the same production
assets (`index-D3o7thQh.js`, `index-D_zVXlS3.css`). They continue to use
`api-staging.axiom.stream` and staging CSP/settings; production is unchanged.
For future account UI releases, build `apps/auth/` once, deploy and verify
staging first, then promote the same build to production using the matching
Wrangler environment and the `axiom` profile. Staging previously used version
`66938817-0607-4980-83f8-c4b8f14af966` from the legacy auth frontend.

The website source was integrated from landing redesign commit `d5f4a0b`,
with production download, sign-in and policy links. `/downloads` serves all
11 published 0.1.0 files, including macOS installers and disk images, Windows
x64/ARM64 installers, and Linux AppImage, DEB and Arch packages. First-time
visitors receive the website directly. `welcome.axiom.stream` redirects to it.
The private GitHub repository is not linked publicly; the FAQ states its current
source availability. Repository visibility was not changed.

Hosted account pages preserve the production password plus email-code flow,
including challenge validation, resend cooldowns and failure handling. The
branded UI retains Google, passkey and Ethereum options, native approval,
account controls, recovery, payments and policy routes. Ethereum is a full-width
provider button above email. Legal links on the website point to the branded
auth-origin policies, with no circular promise of a separate full policy.

Validation: 58 auth tests, production-build browser regressions including wrong
email-code rejection and native return paths, five website Worker route tests,
and the landing proof-card geometry regression passed. Desktop/mobile browser
checks cover website, manifesto and downloads; all 11 CDN downloads return the
expected byte sizes. Public auth pages load their assets and unauthenticated
session state successfully. API health returns 200.

No account credentials were entered and no live email, payment or inference
was triggered by these checks. Google/passkey/wallet provider prompts were not
rerun. The API, payment receiver, CDN and installer bytes were not redeployed.
The native callback template requires a future desktop/CLI build to reach
installed clients; it is not part of either hosted Worker.

## Rollback reference

Prior website version: `a4fb6bd7-2c7c-4264-bdbc-1bca1c53d2f5`.
Prior auth version (including production email codes):
`c86ea7aa-8ba7-47d2-b76d-679aa434c7b1`.
Routing, DNS and website version metadata were saved locally under
`/home/salt/.config/axiom-deploy/website-20260909/` before cutover.
Use Wrangler rollback from the matching source/config and the Axiom profile;
never deploy the old API-repository auth source over this branded app.
