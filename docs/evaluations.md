# Evaluation policy

AxiomCLI has two deliberately separate evaluation lanes. Deterministic
evaluations are a required change gate; live-model evaluations measure model
behavior without making ordinary CI depend on model availability or luck.

## Deterministic lane

Run:

```text
scripts/axiomcli-eval deterministic
```

The original fixture set is generated in fresh temporary directories. It
covers:

| Outcome | Fixture/evidence |
|---|---|
| Rust targeted edit and real test | `autonomous_eval::deterministic_agent_edits_runs_tests_and_reports_completion` |
| Rust failed command, repair, rerun | `autonomous_eval::deterministic_agent_observes_failure_repairs_and_reruns` |
| TypeScript multi-file refactor | `evaluation_suite::original_multilanguage_evaluation_set_is_repeatable_from_clean_fixtures` |
| Python test repair | same multi-language evaluation |
| Configuration and documentation consistency | same multi-language evaluation |
| Read-only investigation with file citation | same multi-language evaluation |
| Denied/out-of-root operations | `autonomous_eval`, workspace, policy, and web adversarial tests |
| Web research with a real SearXNG service | ignored `searxng_integration` test, run by the dedicated CI job |
| Real stdio MCP discovery/call/reconnect | `mcp_integration` |
| Cancellation and durable resume | agent, provider, process, session, ACP, and TUI suites |
| Equivalent TUI/ACP mutation outcome | `workspace_edit_reaches_*` PTY/transcript tests |
| Long stream, journal, and background work | `soak` |

Each task scorecard records completion, expected workspace state, verification
command result, policy escapes, tool-call count, expected command failures,
provider retries, latency, and maximum serialized context size. Deterministic
tasks fail on any policy escape, wrong workspace outcome, unexpected retry,
failed verification, context overflow, latency overflow, or excess tool use.

## Live-model lane

Run only against an explicitly selected development endpoint:

```text
AXIOM_LIVE_BASE_URL=... AXIOM_LIVE_MODEL=... scripts/axiomcli-eval live
```

`AXIOM_API_KEY` is optional for endpoints that do not require it. Secrets are
read from the process environment and never included in the report. The live
test runs three fresh, read-only repository investigations with a six-step,
120-second-per-run budget. It requires the exact private fixture marker, at
least one and no more than four tools, no workspace changes, and maximum latency
no greater than four times minimum latency (with a five-second allowance).

Baseline recorded on the Linux x86-64 development host on 2026-08-22 using the
local AxiomIO development proxy and `glm-5-2`:

| Run | Latency | Tool calls | Correct | Workspace mutations |
|---:|---:|---:|---:|---:|
| 1 | 8,252 ms | 1 | yes | 0 |
| 2 | 8,036 ms | 1 | yes | 0 |
| 3 | 5,970 ms | 2 | yes | 0 |

Observed max/min latency ratio was 1.38. A release candidate is blocked by a
wrong answer, mutation, policy escape, tool/step/latency budget failure, or a
variance failure. The scheduled workflow is bounded to 15 minutes and fails
when its required endpoint/model/key secrets are absent. Tagged release
packaging has the same mandatory live gate.

## Real integration lane

`scripts/axiomcli-eval full` additionally starts pinned real SearXNG instances
and runs live upstream search behavior. SearXNG results are inherently affected
by upstream availability, but service lifecycle, empty/disabled/timeout modes,
agent citations, shutdown behavior, URL normalization, and local-target denial
are asserted.
