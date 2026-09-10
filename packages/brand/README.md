# Axiom brand

Shared masters from Axiom's `landing-page-redesign` branch and desktop redesign:
original SVG wordmarks/symbols, PP Cirka, DM Sans, DM Mono, paper `#F2F4FA`, ink
`#1F1F1F`, and signal orange `#FF7653`. Keep the original artwork intact.

- Import `@axiom/brand/fonts.css` for locally bundled typefaces.
- Import `@axiom/brand/tokens.css` for the hosted light/dark surface palette.
- Import `@axiom/brand/assets/<file>.svg` for versioned artwork.
- Desktop and hosted auth consume these files directly. The marketing site is now integrated under `landing/`, preserving the
  original artwork and using the shared DM Sans font for its download pages.

The native completion/rejection page is a self-contained, script-free HTML
artifact, with embedded fonts and artwork and no network dependencies:

```sh
node packages/brand/scripts/build-callback.mjs
node packages/brand/scripts/build-callback.mjs --check
```

Regenerate after changes to the masters. AxiomCLI embeds the resulting
`templates/desktop-callback.html`; its three placeholders receive fixed strings
only. No user input or authorization parameters are interpolated into the page.
