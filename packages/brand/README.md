# Axiom native brand

Shared masters from Axiom's `landing-page-redesign` branch and desktop redesign:
original SVG wordmarks/symbols, PP Cirka, DM Sans, DM Mono, paper `#F2F4FA`, ink
`#1F1F1F`, and signal orange `#FF7653`. Keep the original artwork intact.

- Import `@axiom/brand/fonts.css` for locally bundled typefaces.
- `tokens.css` supplies the light/dark palette used by the native callback generator.
- Import `@axiom/brand/assets/<file>.svg` for versioned artwork.
- Desktop consumes these native files directly. Hosted web branding lives in
  the companion `axiom-platform` repository. Coordinate intentional changes that
  affect both copies without importing files between checkouts.

The native completion/rejection page is a self-contained, script-free HTML
artifact, with embedded fonts and artwork and no network dependencies:

```sh
node packages/brand/scripts/build-callback.mjs
node packages/brand/scripts/build-callback.mjs --check
```

Regenerate after changes to the masters. AxiomCLI embeds the resulting
`templates/desktop-callback.html`; its three placeholders receive fixed strings
only. No user input or authorization parameters are interpolated into the page.
