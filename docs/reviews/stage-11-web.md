# Stage 11 Review — Real SearXNG and local fetch

Decision: **PASS**

- `scripts/searxng-dev` clones pinned official SearXNG commit
  `5f4005996e38`, provisions Python 3.13 and dependencies with `uv`, binds the
  service to loopback, health-checks it, and supports a clean owned lifecycle.
- The ignored real-service suite starts the pinned process on ephemeral ports,
  queries DuckDuckGo/Wikipedia through the normal configuration, and kills every
  child it owns. Checked-in real SearXNG configurations deterministically cover
  empty results, all-disabled engines, engine timeout/unavailability, and
  post-shutdown client behavior. No HTTP response mock substitutes for these
  paths.
- Search concurrency is capped. Results carry exact configured-service and
  engine provenance; URLs reject credentials/local targets, strip fragments and
  default ports, deduplicate, and retain citation URLs. Titles/snippets/warnings
  are control-clean and bounded. Empty results and per-engine failures remain
  distinct, understandable output.
- The real-service research-agent scenario makes exactly one `web_search` call,
  cites a URL returned by that call, and never turns retrieved text into another
  tool action. Tool context labels all search data untrusted.
- Direct fetch pins every validated DNS answer into a fresh no-proxy client for
  each redirect hop, disables automatic redirects, revalidates redirects, and
  accepts only text-readable content. Stream, time, and redirect counts are
  bounded.
- The SSRF suite blocks mixed public/private DNS answers (rebinding simulation),
  alternate loopback encodings, redirect pivots, local services, cloud metadata
  addresses/names, IPv4-mapped IPv6, 6to4, credentials, and reserved ranges.
- Network policy effects name the exact search endpoint or fetched URL. Observe
  mode denies both before execution; approval text and persisted bounded tool
  output preserve destination/service provenance for audit.
- Wording distinguishes a “configured SearXNG service” from a “direct local
  fetch” and does not claim that an arbitrary configured endpoint is hosted by
  Axiom.

Validation evidence:

```text
scripts/searxng-dev integration-test                       PASS (real service)
cargo test -p axiomcli web::tests::                        PASS (6/6)
cargo clippy -p axiomcli --all-targets -- -D warnings      PASS
cargo test -p axiomcli                                     PASS
cargo deny check                                           PASS
```

## Final revalidation

Reopened and passed against implementation `f9fcedf` on 2026-08-22 by Codex.
The criterion mapping, exact commands/test totals, architecture/security/front-
end/dependency/performance findings, deferrals, and final rationale are
incorporated from the [final Stage 00–15 revalidation](final-revalidation-2026-08-22.md).
Final decision: **PASS**.
