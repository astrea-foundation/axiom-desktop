# Axiom ACP SDK patch

This directory is the published `agent-client-protocol` 2.0.0 crate with one
upstream fix applied.

- Source release: `agent-client-protocol` 2.0.0 from crates.io
- Applied upstream commit: `832dbecb0f3a063a8a0935627c2b856e06f64939`
- Upstream PR: <https://github.com/agentclientprotocol/rust-sdk/pull/306>
- Changed production file: `src/jsonrpc/handlers.rs`
- Added upstream regression: `tests/jsonrpc_deep_chain_stack.rs`

The patch boxes each `ChainedHandler` dispatch link and constructs it behind
`#[inline(never)]`. This bounds stack use for clients such as Axiom that
register many typed handlers, including on 512 KiB macOS worker stacks.

Remove this path patch and return to the crates.io dependency after an
official ACP SDK release contains PR #306.
