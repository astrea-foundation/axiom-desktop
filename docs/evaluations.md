# Evaluations and performance

[The development guide](development.md#checks) owns general checks. This guide
covers behavioral evaluation and historical performance observations.
[scripts/axiomcli-eval](../scripts/axiomcli-eval) defines the lanes.

## Routine checks and opt-in extended lane

Normal `cargo test` and CI run the functional suites once. They include a small
64-chunk/32-checkpoint persistence case with the same transcript merging,
durability and storage-bound assertions as the large soak. Background process
ordering/reaping and functional PTY tests remain required. Only the ten-start
timing benchmark, 10,000-chunk/5,000-durable-write soak, and Linux idle CPU/RSS
benchmark are marked `#[ignore]`; Rust reports those skips explicitly. These
benchmarks are opt-in both locally and in CI, without checking a hidden `CI`
environment variable or disabling security/protocol assertions.

Run the extended lane locally:

```sh
scripts/axiomcli-eval deterministic
```

Or dispatch `ci.yml` with `extended=true` and a chosen target, for example:

```sh
gh workflow run ci.yml --ref dev -f target=linux -f extended=true
```

There is no automatic schedule for these benchmarks. Routine PR/release
qualification leaves `extended=false`. The extended deterministic suites use
the workspace/all-features selection of Linux CI. This reuses its artifacts instead of
building a second dependency graph with a different feature selection, which
can exhaust a hosted runner's disk before evaluation starts.

The runner repeats the autonomous agent, multi-language fixture, soak, ACP
transcript, TUI PTY and MCP integration suites twice. Fixtures are created in
fresh temporary directories. It explicitly includes the three ignored
benchmarks, without enabling the separate ignored live-provider tests. The
suites cover edits followed by real tests, repair of
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

Live evaluations run locally on demand with the command above. GitHub Actions
has no standalone live-evaluation workflow or schedule; automation is limited
to CI and release artifact production. The pushed-tag [signed release
workflow](../.github/workflows/release.yml) retains its live-provider gate, with
a 15-minute job limit and required endpoint/model/key secrets. The manually
dispatched Desktop installer workflow is a separate packaging lane; passing it
is not evidence of a live-provider test.

`scripts/axiomcli-eval full` runs the deterministic lane and then the live
provider test; it requires the live variables described above. Hosted-search
client tests use a local authenticated fixture, without paid search requests.

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

The 5,000-checkpoint persistence soak reports timing before checking its
30-second Unix or 180-second Windows budget. Shared Linux runners also have variable
durable-write latency: [the September 22 CI run](https://github.com/astrea-foundation/axiomai/actions/runs/35774248238)
took 14.17 seconds, exceeding the former Linux-only 10-second limit. Each
checkpoint still commits separately. The [Windows x64 qualification run](https://github.com/astrea-foundation/axiomai/actions/runs/35780630279)
measured 103.13 seconds for those individually durable writes, so Windows has a
separate disk-latency allowance. The 16 MiB database and 512 KiB export
bounds remain the same on every platform. Path-display fixtures use native
absolute paths.

PTY interactions wait for persisted turn completion before sending idle-only
commands, including after permission decisions, question answers, and plan
review. Answer submission alone does not establish completion on a loaded
runner. The harness retains bounded waits and terminal behavior checks, and
includes captured PTY output when completion times out.

The harness answers the terminal's cursor-position request and waits for its
initial frame before resizing. Unix CLI input uses Crossterm's `use-dev-tty`
polling backend so concurrent resize and keyboard readiness cannot lose an
edge and leave the prompt waiting for another keypress. The PTY suite exercises
startup, resize, prompt submission, and terminal restoration through that path.
Credential tests use the production three-second operation deadline; deliberately
stalled stores derive their delay from that deadline. A shorter test-only limit
can reject ordinary Windows identity-file and ACL setup on a loaded machine.
Windows test environments include Git's `usr/bin` directory on `PATH` for the
`sh`, `printf`, and `env` process fixtures. MCP fixtures resolve `env` through the
same sanitized path as other processes, without a Unix-only absolute path.

Browser proof-renewal tests pause their mock clock before loading the fixture,
then advance it explicitly; page startup time must not shift expiry boundaries.
