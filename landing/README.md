# AxiomAI landing redesign

The redesigned AxiomAI marketing site, including the cinematic Three.js hero,
interactive desktop demo, manifesto, and scroll-driven embossed proof cards.

Live site: https://axiom-landing-woad.vercel.app/

## Local preview

From the repository root:

```sh
cd landing
npm ci
npm run build
python3 -m http.server 4173 --bind 127.0.0.1
```

Open http://127.0.0.1:4173/ and rebuild after JavaScript changes.

## Deployment

For a Vercel Git import, set the project Root Directory to `landing`.
The included `vercel.json` runs `npm run build` and publishes `.site/`.
The existing Vercel project remains available at the live URL above.

See [DEVELOPMENT.md](DEVELOPMENT.md) for rendering details, local performance
profiling, and the card-flight regression check.
