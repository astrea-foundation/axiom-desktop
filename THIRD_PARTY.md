# Third-party components

AxiomCLI is original code built on third-party Rust crates recorded exactly in
`Cargo.lock`. Run `cargo deny list` and `cargo deny check` for the resolved
inventory and license policy.

Release archives include `THIRD_PARTY-LICENSES.tsv`, mechanically generated
from the locked dependency graph by Cargo Deny. They also include the license,
copying, and notice files shipped at the root of every resolved dependency
package under `third-party/`, with a deterministic `MANIFEST.tsv`. Dependencies
that publish license metadata but no corresponding file are explicitly marked
`(metadata only)` in that manifest. These materials are packaged alongside this
explanatory notice and the project’s Apache-2.0 license.

Notable protocol/runtime libraries include Ratatui/Crossterm, Tokio, Reqwest,
the official Agent Client Protocol Rust SDK, the official Rust MCP SDK,
Rusqlite/SQLite, Serde, and Clap. Their license texts and notices remain those
published by their respective packages. Git inspection uses the `git2` Rust
bindings to libgit2 so read-only repository status/diff/log operations do not
silently spawn a shell or inherit process credentials. Glob matching uses
`globset` over `ignore`'s bounded walker.

The test suite uses the MIT-licensed `portable-pty` crate to exercise terminal
resize, cancellation, panic unwinding, and restoration against an actual
pseudo-terminal rather than a mocked input stream.

The development search service downloads and runs official SearXNG separately.
SearXNG is AGPL-3.0-or-later and is not linked, vendored, or redistributed as
part of the AxiomCLI Rust binary. See `dev/searxng/README.md` and the pinned
source revision in `scripts/searxng-dev`.
