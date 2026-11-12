# Evaluations and performance

[The development guide](development.md#checks) owns general checks. This guide
covers deterministic behavior, live-provider checks and performance budgets.
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

## Harness behavior

The persistence benchmark commits each checkpoint separately and reports elapsed
time against platform-specific budgets. Database and export size bounds are
independent of host speed. Performance observations must identify the build and
host before they are compared or presented as release guarantees.

PTY fixtures wait for persisted turn completion before idle-only commands, answer
terminal cursor-position requests and wait for the first frame before resizing.
Bounded waits include captured terminal output on failure. Windows fixtures use
Git's Unix tools through the sanitized process path. Credential-store tests retain
the production operation deadline so OS setup remains part of the exercised path.
Browser fixtures control their clock explicitly when testing proof expiry.
