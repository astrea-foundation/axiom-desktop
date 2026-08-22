# Stage 11 Review — Real SearXNG and local fetch

Decision: **PASS**

- `scripts/searxng-dev` clones pinned official SearXNG commit
  `5f4005996e38`, provisions Python 3.13 and dependencies with `uv`, binds the
  actual service to `127.0.0.1:8888`, health-checks it, and owns clean shutdown.
- The real-service integration test started SearXNG, queried real upstream
  engines, normalized non-empty results, and stopped the process successfully.
- Ordinary search integration never uses a mocked SearXNG payload.
- Search results carry service/engine provenance, URLs are normalized and
  deduplicated, and external text is labeled untrusted.
- Direct fetch pins validated DNS results per hop, disables automatic redirects,
  revalidates redirects, blocks private/local/metadata/reserved addresses and
  credential URLs, and enforces time/body/redirect limits.
