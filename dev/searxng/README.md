# Real SearXNG development service

AxiomCLI's normal search integration uses an actual SearXNG process. It does
not substitute a fake HTTP response. `scripts/searxng-dev` clones the pinned
upstream source into `.cache/searxng`, builds an isolated Python 3.13 virtual
environment with `uv`, and binds the service only to `127.0.0.1:8888`.

```sh
scripts/searxng-dev bootstrap
scripts/searxng-dev start
scripts/searxng-dev query 'Rust programming language'
scripts/searxng-dev integration-test
scripts/searxng-dev stop
```

The chosen engines make real upstream requests, so engine throttling or an
offline network can yield an empty result set. That is surfaced as a real
availability condition. The checked-in secret is explicitly development-only;
do not expose this configuration to a network or reuse it for Axiom's hosted
service.
