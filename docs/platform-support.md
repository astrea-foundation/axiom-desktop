# Platform support

The early release artifact claims support only for
`x86_64-unknown-linux-gnu`. On that platform the repository verifies numeric
loopback-only binding, authenticated local HTTP, Rustls/native trust-root
loading, live HTTPS endpoint binding, child-process startup/readiness/shutdown,
Bubblewrap tool isolation, terminal behavior, deterministic install/uninstall,
and proxy cancellation/resource bounds.

macOS, Windows, Linux musl, Arm Linux, LAN binding, and multi-user proxy hosting
are not release claims. Source code may compile on some of them, but support
will be added only after that platform has equivalent tests for trust-store and
TLS behavior, loopback/process supervision, credential transport, tool
sandboxing, terminal integration, packaging, installation, cancellation, and
cleanup. The secure protocol never uses platform absence as a reason to fall
back to plaintext.
