# Astrea banner assets

Provided by the owner from the private `dmitrykurash/astrea-motion` production
kit. These assets retain that kit's private asset notices; they are not covered
by this repository's Apache-2.0 code license.

- `astrea-lockup.svg`: `projects/shape scene/assets/astrea-logo-light.svg`,
  with the branded periwinkle symbol and white wordmark.
- `field-banner.webm`: a silent 720×160, 24 fps, 24-second re-render of the
  original Astrea held field (`projects/shape scene/src/field.ts` and
  `src/fieldkit.ts`). The camera orbits the stationary scene with fixed world-up;
  no image rotation or camera roll is applied. Counter and seed motion close
  on the loop boundary. This variant retains the original field, materials and
  post chain; it is a new render rather than the accepted bumper's exact pixels.
  Editable variant: `projects/shape scene/src/fieldbanner.ts`, entry
  `field-banner.html`, exporter `scripts/render-field-banner.mjs` in Astrea Motion
  commit `6edefaaa0540d92f6a8be789b24ad3ee86ee30ac`
  (`codex/astrea-side-marketing-banner`). See that kit's `docs/bumper.md`.
- `field-banner-poster.png`: frame zero of the same camera-orbit render.
- `jetbrains-mono-latin.woff2`: JetBrains Mono Variable, Latin subset, from
  `projects/astrea-privacy-cinetic/films/astrea/src/brand/fonts/jetbrains-mono/files/jetbrains-mono-latin-wght-normal.woff2`.
  The banner uses Medium (weight 500). This font is licensed under the SIL
  Open Font License 1.1; its full license is in `JetBrainsMono-LICENSE.txt`.

Source: https://github.com/dmitrykurash/astrea-motion
Revision: `0965fbf15b8d3de6b1433dc5c17016984702766d`.
