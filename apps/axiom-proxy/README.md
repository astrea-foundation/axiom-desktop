# Axiom secure proxy

`axiom-proxy` is a standalone loopback integration for third-party
OpenAI-compatible clients. Its server implementation is also a library hosted
by `axiomcli desktop-proxy` for the Desktop Proxy screen. Desktop chat continues
to use `axiomcli acp`. The proxy exposes the
initial OpenAI-compatible `/v1/models` and
`/v1/chat/completions` surface while using `axiom-secure-client` for local TEE
verification and E2EE.

Required environment:

```text
AXIOM_API_KEY=...            upstream Axiom credential
AXIOM_PROXY_TOKEN=...        separate random local-client credential (32+ bytes)
```

These variables describe the standalone executable. Desktop's native host
instead uses the signed-in account from the OS credential store, refreshes
access tokens before requests, and rejects an account change. Electron starts
it only after a user action, pins the expected account and service origin, and
generates a new random local token for each start. Copy token writes that token
directly to the clipboard; upstream credentials never reach the renderer.
Sign-out, account/runtime changes, app shutdown, and supervisor stdin EOF stop
the hosted proxy. Port changes take effect on Restart. UI usage and evidence
come from bounded native metadata and apply only to the current run.

Optional environment:

```text
AXIOM_BASE_URL=https://api.axiom.stream
AXIOM_PROXY_BIND=127.0.0.1:8484
AXIOM_PROXY_MAX_CONCURRENCY=8
AXIOM_PROXY_COMPAT=lenient      lenient (default) drops unsupported request
                                parameters; strict refuses them
```

Unsupported parameters are those the Axiom relay has no field for, such as
`stop`, `seed`, and the penalties. Lenient mode drops them so an application the
operator cannot modify still gets an answer, and names the ones that could have
changed the answer on the `x-axiom-ignored-parameters` response header and in a
`request_parameters_ignored` stderr line. `n` above 1 and non-text content parts
are refused in both modes. See
[`../../docs/configuration.md`](../../docs/configuration.md).

Start it with `cargo run --locked -p axiom-proxy`. The process emits one
structured readiness object on stdout for a supervisor. Sanitized diagnostics
go to stderr. Only numeric loopback bind addresses are accepted.

Local clients authenticate with `Authorization: Bearer $AXIOM_PROXY_TOKEN`.
They never receive the upstream Axiom credential. The proxy translates tool
semantics but never executes or authorizes a tool.

The local HTTP hop is plaintext by design and is protected by loopback binding,
the independent bearer token, and the user's operating-system process boundary.
The remote hop is never downgraded: every request establishes provider E2EE
using fresh secrets and a provider key accepted only from current, locally
verified attestation evidence. Bounded verified public evidence may be reused
only within its validity limits. Evidence, TLS binding, encryption, stream
ordering, receipt, or decryption failure closes the request.

The packaged Linux proxy remains dormant until an OpenAI-compatible integration
explicitly starts it. Its supervisor must generate the local token, pass both
secrets through the child environment, parse exactly one readiness JSON object
from stdout, consume metadata-only JSON logs from stderr, and terminate the
process when the integration shuts down.
