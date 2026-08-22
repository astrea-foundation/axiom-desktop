# ADR 0008: Linux release, signing, and compatibility

Status: accepted for 0.1.x.

AxiomCLI distributes one supported artifact: a reproducible
`x86_64-unknown-linux-gnu` tarball. The artifact is generated from the locked
Cargo graph with a fixed source epoch and deterministic tar/gzip metadata. It
contains the optimized binary, build metadata, project license, third-party
notice and generated license inventory, configuration/privacy/troubleshooting
documentation, and compatibility policy.

Every tagged artifact requires SHA-256 plus an armored detached GPG signature
from an environment-protected maintainer key. The private key is never stored in
the repository. Local tests generate an ephemeral key only to validate the
sign/verify mechanics; such signatures are not release identities.

Installation is a copy into an explicit prefix. Uninstall removes only the
known program and documentation files; user config, journals, workspace plans,
and service data survive. The installation and binary startup are tested in a
fresh Bubblewrap mount/network namespace.

Semantic Versioning governs the program version. Persisted event schemas and
ACP have separate compatibility rules in `docs/versioning.md`. Linux x86-64 GNU
is supported because its Bubblewrap process boundary and PTY behavior have been
reviewed. macOS and Windows are not advertised until equivalent security and
installer gates exist.
