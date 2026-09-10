# AxiomAI website

Production source for https://axiom.stream, integrated from the approved
`landing-page-redesign` branch (`d5f4a0b`). The original Three.js hero, desktop
demo, manifesto and proof-card animations are retained.

The website and hosted account app now live in the same product checkout:
`landing/` and `apps/auth/`. Website account and policy links lead to
`auth.axiom.stream`. The old welcome hostname redirects to the main website;
first-time visitors no longer receive a welcome redirect or tracking cookie.

## Build and preview

From the repository root, install the workspace dependencies for shared fonts
and browser checks, then build the static website:

```sh
pnpm install --frozen-lockfile
npm --prefix landing ci
npm --prefix landing run build
node apps/auth/node_modules/wrangler/bin/wrangler.js dev --config landing/wrangler.toml --local --port 4177 --env-file /dev/null
```

Open http://127.0.0.1:4177. The Cloudflare Worker provides clean page URLs,
account redirects and real 404s. Only `.site/` is published; development scripts,
source files and dependency directories are excluded.

```sh
cd landing
node scripts/check-worker.mjs
node scripts/check-proof-flight.mjs
node scripts/check-site.mjs
```

The site check requires the local preview server. It checks desktop/mobile
layouts, download buttons, auth/policy redirects, 404s and the availability and
sizes of all published installers. Set `SITE_ORIGIN=https://axiom.stream` to
run the same checks against production. Screenshots go to `dist/website-deploy/`.

## Releases

`releases.json` is the reviewed latest CDN release manifest. `releases/` retains
immutable previous manifests. The build generates `/downloads` and these public,
anonymous JSON endpoints from the same validated data:

- `GET /api/releases/latest`: latest version, signing status, and downloads.
- `GET /api/releases`: latest version and the complete release catalog.
- `GET /api/releases/0.1.0`: one version with its original download links.

Each download includes `platform` (`mac`, `win`, `linux`), `arch` (`x64`, `arm64`),
`format`, filename, byte size, SHA-256, and its immutable CDN URL. Responses use
`schemaVersion: 1`, support HEAD and public CORS, and cache for 60 seconds. Unknown
versions return JSON 404s. These routes never proxy the inference or account API.

After the CDN publisher has uploaded and verified a release, import its manifest:

```sh
node landing/scripts/import-release.mjs /path/to/installers/manifest.json
npm --prefix landing run build
node landing/scripts/check-worker.mjs
```

The import preserves the previous version and rejects downgrades and replacement
of an existing version. Deploy the built website to publish the new download page
and update feed together; promoting the CDN alone does not publish the website
feed. The page includes macOS PKG/DMG/ZIP,
Windows x64/ARM64 EXE, and Linux AppImage/DEB/Arch packages. Downloads retain
immutable, versioned CDN paths. No installer is rebuilt by the website deploy.

## Deploy

Use the Axiom Wrangler profile, rather than the unrelated default profile:

```sh
node apps/auth/node_modules/wrangler/bin/wrangler.js deploy --config landing/wrangler.toml --dry-run --profile axiom --env-file /dev/null
node apps/auth/node_modules/wrangler/bin/wrangler.js deploy --config landing/wrangler.toml --profile axiom --env-file /dev/null
pnpm --filter @axiom/auth exec wrangler deploy --env= --profile axiom --env-file /dev/null
```

The website replaces the existing `axiom-frontend` Worker and preserves its
apex and welcome routes. It does not change the API, payment receiver, CDN,
DNS records, or hosted authentication origins. The reviewed HTML is built
before deployment; the account app must likewise be built from `apps/auth`.
See `DEVELOPMENT.md` for animation and performance details.

The GitHub repository is currently private, so public navigation omits repository
links and the source-availability FAQ reflects that status. Restore public source
links only after the repository is intentionally published.
