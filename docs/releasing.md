# Release procedure

The supported artifact is a reproducible Linux x86-64 GNU tarball.

1. Use a clean checkout of the intended commit with Rust 1.97.1, Cargo Deny,
   GNU tar, gzip, SHA-256 tools, GPG, and Bubblewrap installed.
2. Run formatting, strict Clippy, all-target tests, Cargo Deny, the deterministic
   evaluation lane, the real SearXNG integration, and the budgeted live-model
   lane.
3. Set `SOURCE_DATE_EPOCH` to the commit timestamp. Set
   `AXIOMCLI_SIGNING_KEY` to the full fingerprint of the authorized maintainer
   signing key.
4. Run `scripts/package-release --sign` twice in separate output directories
   and compare SHA-256 digests.
5. Run `scripts/verify-release ARCHIVE ARCHIVE.asc` in a keyring containing the
   trusted public key.
6. Install into a clean prefix with `scripts/install-release ARCHIVE PREFIX`,
   run the release smoke workflow, then use `scripts/uninstall-release PREFIX`.

The package contains `bin/axiomcli`, the Apache-2.0 project license, third-party
notice/inventory files, user documentation, compatibility policy, and build
metadata. Verification rejects checksum failure, signature failure, path
traversal, multiple top-level roots, missing notices, missing/non-starting
binaries, and strings shaped like Axiom credentials.

Uninstall removes only installed program/documentation files. User config,
session journals, plans in workspaces, and SearXNG data are intentionally
preserved. This behavior prevents an ordinary program uninstall from becoming
a destructive data purge.
