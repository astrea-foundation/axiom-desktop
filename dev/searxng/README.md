# Real SearXNG development service

When trusted configuration selects `web_search_provider = "searxng"`, AxiomCLI
uses an actual SearXNG process. The default provider is authenticated hosted Axiom
search. `scripts/searxng-dev` clones the pinned
upstream source into `.cache/searxng`, builds an isolated Python 3.13 virtual
environment with `uv`, and binds the service only to `127.0.0.1:8888`.

```sh
scripts/searxng-dev bootstrap
scripts/searxng-dev start
scripts/searxng-dev query 'Rust programming language'
scripts/searxng-dev integration-test
scripts/searxng-dev stop
```

The integration command starts the pinned real service itself on ephemeral
loopback ports and always terminates processes it owns. It exercises the normal
real-upstream configuration plus checked-in empty, disabled, timeout, and
shutdown configurations. Those fixtures change SearXNG configuration and
lifecycle; they do not replace SearXNG with a response mock.

The chosen normal engines make real upstream requests, so engine throttling or
an offline network can still yield an empty result set. AxiomCLI reports empty
results and per-engine failures separately. The endpoint shown in tool output is
the *configured SearXNG service*; it may be local or operated by Axiom later and
the client does not claim which merely from its URL.

The checked-in secrets are explicitly development-only. Do not expose these
configurations to a network or reuse their keys for Axiom's hosted service.
