# Axiom secure proxy

Standalone authenticated loopback server and the server library hosted by
`axiomcli desktop-proxy`. Both use the shared attested provider-E2EE client.

Set `AXIOM_API_KEY` and a separate random `AXIOM_PROXY_TOKEN` (at least 32 bytes),
then run from the repository root:

```sh
cargo run --locked -p axiom-proxy
```

See the [proxy guide](../../docs/proxy.md) for configuration, OpenAI compatibility,
Desktop supervision and the local plaintext boundary.
