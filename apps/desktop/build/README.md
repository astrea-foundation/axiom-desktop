# Desktop packaging assets

[electron-builder.yml](../electron-builder.yml) selects the active installer
assets. This directory also contains handwritten installation hooks; do not
replace the entire directory with generated output.

| Assets | Use |
|---|---|
| `icon.icns` | Current macOS bundle icon |
| `icon.ico` | Windows app and installer |
| `icons/`, `icon.svg` | Linux icon set |
| `background.tiff`, `background.png`, `background@2x.png` | macOS DMG artwork |
| `installerSidebar.bmp`, `installerHeader.bmp` | Windows NSIS artwork |
| `tahoe-icon-layers/` | Preparatory layered macOS artwork, not the configured bundle icon |
| `tray/` | Preparatory tray assets, not an implemented tray feature |

The generator also writes `resources/icon.png`, `resources/icon-mac.png` and
`resources/icon.svg`. See [brand asset development](../../../docs/development.md#brand-assets)
for prerequisites and the external master directory, and
[releasing](../../../docs/releasing.md) for signing and installer validation.
