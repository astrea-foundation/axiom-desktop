# Packaged brand assets

Everything in this folder is **generated** from the design masters in
`Axiom new UI/brand-desktop/` (outside the repo) by `pnpm brand:assets`
(`scripts/brand-assets.py`, needs python3 + Pillow on macOS). Edit the masters,
re-run, commit the outputs. `electron-builder.yml` points at these paths.

| File | Used by | Notes |
|---|---|---|
| `icon.icns` | macOS bundle (`mac.icon`) | Legacy shape baked in: 824 px smooth-cornered square on a 1024 canvas with Apple's margins and shadow, for macOS 15 and earlier. macOS 26 masks it again to the same shape, which is harmless. |
| `tahoe-icon-layers/` | macOS 26 Liquid Glass icon | Not compiled here (needs Xcode 26). Open **Icon Composer**, new macOS icon, drop `background.svg` as the background fill and `foreground.svg` as a layer, export `Axiom.icon` next to this README, then add `"icon": "build/Axiom.icon"` under `mac` in `electron-builder.yml` once the builder in use supports `.icon` bundles (otherwise keep `icon.icns`). |
| `icon.ico` | Windows exe / installer | Full bleed, 16–256 px. |
| `icons/NxN.png`, `icon.svg` | Linux (`linux.icon`) | hicolor set 16–1024 plus the scalable SVG. |
| `background.tiff` (+ `background.png`, `background@2x.png`) | DMG window (`dmg.background`) | 660×400 pt, 1× and 2× packed into one TIFF (`tiffutil -cathidpicheck`). Icon slots are placed around the arrow in the art: see `dmg.contents`. |
| `installerSidebar.bmp` 164×314, `installerHeader.bmp` 150×57 | NSIS assisted installer | 24-bit BMP, no alpha, as NSIS requires. |
| `tray/axiomTemplate.png`, `@2x` | Future menu-bar / tray status item | Black-on-alpha template images (24×18 pt). Not wired up yet. The one output not made by the Python script: rasterised from `symbol-mono.svg` with `scripts/rasterize-svg.mjs` (see its header for the job format). |

Also refreshed by the generator: `resources/icon.png` (512, full bleed — window
icon on Windows/Linux), `resources/icon-mac.png` (legacy shape — dev Dock icon
and the About panel), `resources/icon.svg`.

Signing and notarisation are configured by environment, not here
(`CSC_LINK`/`CSC_KEY_PASSWORD`, `APPLE_ID`/`APPLE_APP_SPECIFIC_PASSWORD`/`APPLE_TEAM_ID`).
