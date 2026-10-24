# 0019: Combined macOS and Windows installers

Status: Accepted, 2026-09-22.

## Decision

Publish one Desktop installer and one standalone CLI/proxy installer per OS on
macOS and Windows. macOS PKGs contain universal Intel/Apple Silicon executables.
Windows NSIS installers contain separate x64/ARM64 payloads and choose the native
payload at install time. Linux remains x64 AppImage, DEB, Pacman and standalone SH.

Schema 3 identifies these installers with `arch: "universal"` and requires eight
artifacts for publication. Both products select the same signed artifact on each
supported Mac/Windows architecture. The existing latest-feed URL is retained.
There are no users to migrate: no compatibility feed, per-architecture aliases,
or bridge release is introduced. Historical signed schema-2 catalogs remain
readable so published release metadata is immutable.

## Consequences

Downloads become larger because they contain both architectures, while users no
longer need to identify their processor. Native Rust compilation still runs on
each architecture; JavaScript preparation and signing/packaging run once per OS.
Native inputs carry checked version, revision, update trust and content hashes.
Windows captures the matching native redistributable runtime before packaging.
Code signing, notarization and final-byte manifest signing remain required.

The downloads page links only to the canonical CDN. Signed metadata retains
GitHub mirror URLs for verification and distribution. Publication and installer
qualification follow [the release procedure](../releasing.md).
