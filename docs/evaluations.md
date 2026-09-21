# Evaluations and performance

[The development guide](development.md#checks) owns general checks. This guide
covers behavioral evaluation, real search integration and historical performance
observations. [scripts/axiomcli-eval](../scripts/axiomcli-eval) defines the lanes.

## Deterministic lane

```sh
scripts/axiomcli-eval deterministic
```

The runner repeats the autonomous agent, multi-language fixture, soak, ACP
transcript, TUI PTY and MCP integration suites twice. Fixtures are created in
fresh temporary directories. They cover edits followed by real tests, repair of
failed commands, TypeScript/Python changes, documentation investigation, policy
boundaries, cancellation/recovery, TUI/ACP outcomes and bounded background work.

Scorecards check expected workspace state, verification results, policy escapes,
tool counts, retries, latency and serialized context size. A passing mocked
provider task tests orchestration; it does not prove a live provider's behavior
or attestation availability. Linux process fixtures require Bubblewrap.

## Live provider lane

Choose an Axiom relay and a model whose registered E2EE protocol is supported:

```sh
AXIOM_LIVE_BASE_URL=https://your-axiom-relay.example \
AXIOM_LIVE_MODEL=your-model-id \
scripts/axiomcli-eval live
```

Set the required `AXIOM_API_KEY` automation credential in the environment. The test uses `SecureAxiomProvider::from_environment`,
including local attestation, encryption and response authentication. A local
OpenAI-compatible proxy is not a substitute for this relay contract. No
plaintext inference or unauthenticated-key option is available.

The [live test](../apps/axiomcli/tests/evaluation_suite.rs) runs three independent
read-only investigations. Each must return the fixture's exact marker, use one
to four tools, and leave the workspace unchanged. Limits are six agent steps,
120 seconds per run, 256 KiB/64K tokens of context and 32 KiB tool output. The
largest latency must be no greater than the larger of four times the smallest
latency or the smallest plus five seconds. Reports omit credentials.

The [live workflow](../.github/workflows/live-evals.yml) runs on its schedule or
manual dispatch only on `main`, with a 15-minute job limit and required
endpoint/model/key secrets. The pushed-tag signed release workflow has a live
gate too. The manually dispatched Desktop installer workflow is a separate
packaging lane; passing it is not evidence of a live-provider test.

## Real SearXNG integration

The default search provider is authenticated hosted Axiom search. Selecting
`web_search_provider = "searxng"` explicitly enables the alternative; see
[configuration](configuration.md). To work with a real local service:

```sh
scripts/searxng-dev bootstrap
scripts/searxng-dev start
scripts/searxng-dev query 'Rust programming language'
scripts/searxng-dev integration-test
scripts/searxng-dev stop
```

The script clones pinned upstream source into `.cache/searxng`, creates a Python
3.13 environment with `uv`, and uses `127.0.0.1:8888` for the development server.
Integration tests own ephemeral loopback instances and clean them up. They
exercise real upstream search plus empty, disabled, timeout and shutdown
configurations, citations, URL normalization and local-target denial. Upstream
throttling/network failures can still affect results. Fixture secrets are only
for local development.

`scripts/axiomcli-eval full` runs deterministic tests, real SearXNG integration,
and then the live provider test; it therefore needs the live variables too.

## Historical measurements

These observations predate the current provider and storage changes. They are
retained for comparison, not as current benchmarks or release guarantees.
Re-measure on an identified commit/build/host before drawing performance claims.

| Date | Linux x86-64 development-host observation |
|---|---|
| 2026-08-21 | Release `--version`: <0.01 s, 5,424 KiB maximum RSS |
| 2026-08-22 | Ten debug `--version` starts: 52 ms total |
| 2026-08-22 | One second of idle TUI: 25,852 KiB RSS, zero scheduler ticks |
| 2026-08-22 | 10,000 chunks and 5,000 SQLite entries: 2,502 ms journal time, 2,781,184-byte database |
| 2026-08-22 | 24 background tasks in six batches: 102 ms, bounded output, all reaped |

An older live baseline using the then-current local development service and
`glm-5-2` recorded 8,252/8,036/5,970 ms, 1/1/2 tool calls and no mutations.
This is historical test evidence, not proof of today's registered provider
contracts. Dated Desktop/provider reports and their fix retests are indexed
under [testing](testing/README.md).
