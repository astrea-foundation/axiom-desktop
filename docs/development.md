# Development

See [architecture](architecture.md) for ownership and [CONTRIBUTING](../CONTRIBUTING.md)
for the `dev` → `main` workflow. Commands below run from this repository's root.

## Setup

Install the Rust toolchain pinned in [rust-toolchain.toml](../rust-toolchain.toml),
Node.js 22 and pnpm 10 (the CI versions). On Linux, local process tools and their
tests require `/usr/bin/bwrap`; install `bubblewrap` and `jq` for the Rust CI lane.

```sh
pnpm install --frozen-lockfile
cargo build -p axiomcli -p axiom-proxy
pnpm desktop
```

`pnpm desktop` builds the debug sidecar and starts Electron. `pnpm desktop:staging`
selects the staging API/auth pair. `pnpm desktop:install` installs the Linux
checkout launcher; it is a development convenience, not a release package.
CLI alternatives are in [the CLI guide](cli.md).

The development launcher records its Node executable and launch arguments for
factory-reset recovery. After cleanup, reset starts that launcher so both Vite
and Electron come back together. Reopening Electron by itself would retain a
renderer URL whose server exited with the old app. Missing or invalid launcher
metadata refuses reset before native processes stop; start again with
`pnpm desktop` or `pnpm desktop:staging`. Preview and packaged builds reopen their
built renderer directly. Native reset failures are forwarded to Electron without
becoming Vite flags or persisting through a later successful reset.

For Windows work, install the MSVC C++ Build Tools and Windows SDK in the test VM
and use the same pinned Rust, Node.js and pnpm versions there. Build and test from
`dev` inside the VM with the current app version. `pnpm desktop` runs the native
debug sidecar and Electron; `cargo build --locked -p axiom-setup` compiles the
setup executable locally. Test installation under an isolated standard user and
keep private previews outside the public update feed. Qualify the Windows change
locally before preparing its actual public version, promotion and signed release;
see [release previews](releasing.md#development-previews).

Electron main launches `axiomcli acp --frontend desktop-chat`. Packaged builds
resolve a fixed binary under `resources/bin`; `AXIOMCLI_SIDECAR` overrides that
path only in development. Production service origins are pinned. Development
accepts a matching production pair, matching staging pair, or a loopback pair;
the loopback API must use HTTPS with a trusted certificate. Arbitrary mixed
origins are rejected. See [service-origins.ts](../apps/desktop/src/main/service-origins.ts)
and [configuration](configuration.md).

The [confidential pool guide](inference-pool.md) describes the isolated local
profile, per-process loopback CA and installed model pins. A successful hardware
diagnostic does not permit a production verified Desktop chat. The
[AstreaAI connection guide](direct-verda.md) covers the local AstreaAI launch
workflow.

## Checks

Run the checks relevant to a change before pushing `dev`. CI runs on pushes to
`dev`, pull requests targeting `dev` or `main`, and explicit dispatches. Each dev
push validates the resulting revision, including merges.
The full local checks corresponding to the main workflow are:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
pnpm test:rust
python3 -m unittest discover -s scripts/tests -p 'test_release*.py'
node --test scripts/direct-bundle.test.mjs scripts/verify-candidate.test.mjs
cargo build --locked -p axiomcli
cargo install cargo-deny --version "=$(tr -d '\r\n' < .cargo-deny-version)" --locked
cargo deny check
node packages/brand/scripts/build-callback.mjs --check
pnpm generate:protocol
git diff --exit-code -- protocol/axiom-acp-extension packages/axiom-acp-client/src/generated
pnpm typecheck
pnpm test
pnpm --filter axiom-desktop exec playwright install --with-deps chromium
pnpm --filter axiom-desktop test:browser
```

`pnpm test:rust` runs workspace tests and the pinned ACP SDK's deep-chain stack
regression. ACP client tests need the debug AxiomCLI binary built first. Protocol
generation must leave the committed schema and TypeScript DTOs unchanged unless
an intentional contract change is part of the patch.
Browser checks run one test file at a time because fixture servers share Vite's
development dependency cache; parallel startup can leave a fixture page blank.
The generator's banner argument uses double quotes so package scripts preserve
it as one argument in both Windows `cmd.exe` and Unix shells.
Broken-pipe fixtures close the child's read end on Unix and the parent's writable
pipe on Windows, where [libuv deliberately leaves descriptors 0-2 open](https://github.com/nodejs/node/blob/v22.23.2/deps/uv/src/win/fs.c#L633). Both
paths assert request/notification rejection, safe permission-response shutdown,
and child cleanup; they do not skip the error-handling assertions on Windows.

For native reset/restart and draft-save acknowledgement regressions, build
Desktop and the debug CLI, then opt into the separate hidden Electron fixtures:

```sh
AXIOM_RESET_TEST_BINARY="$PWD/target/debug/axiomcli" AXIOM_NATIVE_LIFECYCLE_TEST=1 \
  pnpm --filter axiom-desktop exec tsx --test --test-concurrency=1 \
  tests/native-reset-relaunch.test.ts tests/native-local-state.test.ts
```

These fixtures use temporary Electron profiles. Native cleanup receives an
invalid deletion path and must reject it before accessing credentials or deleting
files. The lifecycle fixture loads only a local test page, with no native
sidecar. It checks never-mounted windows, real renderer acknowledgements, failed
saves, and a lost save handler after reload.
The failing-request fixture has a five-second RPC deadline instead of retaining
the normal two-minute timeout after a test has already failed.

The [evaluation guide](evaluations.md) owns the opt-in repeated deterministic
and performance lane (`scripts/axiomcli-eval deterministic`) and budgeted
live-provider instructions. Routine tests explicitly skip three host-performance
benchmarks and retain a small persistence regression. Live inference always uses registered, locally
verified provider E2EE; it is not a prerequisite for a documentation edit.

The [CI workflow](../.github/workflows/ci.yml) runs Rust/security, generated ACP
contract checks, workspace TypeScript checks, unit/browser tests and all four
native macOS/Windows portability jobs during normal dev/PR validation. ACP client
tests build their required debug CLI in each job that runs them. The native
matrix runs at most two jobs together.

Manual dispatch accepts `target=all`, `native`, `linux`, `desktop`, `macos-intel`,
`macos-arm64`, `windows-x64`, or `windows-arm64` so validation and failure retries
can be targeted. Set `extended=true` only when additional repetitions and
performance benchmarks are wanted. These jobs do not package applications or
establish interactive OS acceptance. After CI passes, the explicit
[unsigned candidate workflow](releasing.md#development-previews) can package and
validate a selected native target without changing versions or publishing it.

Portability fixtures compare canonical working directories and keep differently
cased Desktop resource layouts in separate directories, including on macOS's
case-insensitive filesystem. ACP workspace-edit tests decode diff and location
paths from JSON, require absolute paths, and compare canonical paths to the edited
file. This preserves path assertions across filesystem aliases and Windows JSON
escaping.

## Desktop UI and captures

Write UI labels and status messages as plain sentences, using conjunctions or
parentheses where needed. Avoid em dashes and middle-dot separators, including in
accessible labels, the native callback and native-generated tool descriptions.
Use “Not available” or “N/A” for unknown values and “Loading…” for loading states.
The ACP title parser accepts both current colon-separated identities and historic
tool titles so existing stored sessions remain compatible.

The renderer lives in `apps/desktop/src/renderer`; Electron lifecycle, origins,
updates and sidecar ownership live in `src/main`. Keep the preload API narrow:
renderer callers use typed operations rather than arbitrary ACP requests.

```sh
pnpm build:desktop
pnpm --filter axiom-desktop capture
```

Capture mode uses deterministic fixtures for visual review. It does not establish
successful sign-in, payment, live inference or native installer behavior. Record
interactive qualification with its environment, artifact revision, results and
limitations using [release acceptance](validation.md). Keep raw captures private.

Desktop theme selection is light/dark/system, stored as `axiom.theme`. Shared
semantic colors and bundled fonts support the UI; keep screenshots and controls
consistent in both light and dark themes. Product behavior is documented once in
[Desktop](desktop.md), and the wire contract in [ACP](acp-compatibility.md).

## Brand assets

[packages/brand](../packages/brand/README.md) contains native SVG artwork, bundled
fonts, tokens and the self-contained loopback callback template. Hosted branding
belongs to `axiom-platform`; neither checkout imports the other's files.

After modifying callback assets:

```sh
node packages/brand/scripts/build-callback.mjs
node packages/brand/scripts/build-callback.mjs --check
```

Desktop's checked-in installer artwork is built with:

```sh
pnpm --filter axiom-desktop brand:assets
```

The [generator](../apps/desktop/scripts/brand-assets.py) needs Python 3 and Pillow;
macOS `iconutil` and `tiffutil` are needed for the macOS formats. Set
`AXIOM_BRAND_SRC` to the external design-master directory. The default is a
`brand-desktop` directory beside the repository. Ordinary builds consume the
checked-in outputs and do not need those external masters.

[The build-directory README](../apps/desktop/build/README.md) lists the packaged
assets and handwritten installer files. Layered macOS icon artwork and tray
images are preparatory assets; their presence does not mean the app uses them.

## Documentation changes

Use [the docs index](README.md) as the entry point. Keep one current guide per
topic and link to it from concise component READMEs. Source comments, license
provenance, generated protocol files and runtime prompts retain their required
locations. Fix inbound links when moving a document. Keep public guides focused
on current behavior and keep internal plans, incident reports and raw QA records
private. Carry unresolved release requirements into [validation](validation.md).

## Updater development

The native updater lives in `apps/axiomcli/src/updates`; the shared installation
identity/lease is in `crates/axiom-installation`. Desktop only controls its lifecycle
and UI. `cargo test -p axiom-installation -p axiomcli` covers signatures, staging,
transport, locks and the Linux helper. Desktop's normal tests cover update states
and the standalone installer; `pnpm --filter axiom-desktop exec tsx --test
tests/browser/updates.test.ts` exercises the light/dark controls. See
[release previews](releasing.md#development-previews) and
[qualification](validation.md). Production keys are not
needed for local tests; committed fixture keys must never sign a release.

The AstreaAI sidecar is built from the inventoried standalone Rust source under
`vendor/axiom-direct-verifier`, with `cargo build --locked`. The existing
`node scripts/build-direct-bundle.mjs --output ...` command builds and validates
a native executable; Node, Rust and the platform linker are required. Python,
uv, PyInstaller and an external OpenSSL runtime are no longer verifier build
requirements. Native Windows ARM64 bundles are supported, while older x64
bundles remain a compatible Windows ARM64 fallback. Publisher keys, executable
digest pins, hardware verification and signed trust-policy semantics are unchanged.
