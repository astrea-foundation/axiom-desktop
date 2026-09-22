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

Electron main launches `axiomcli acp --frontend desktop-chat`. Packaged builds
resolve a fixed binary under `resources/bin`; `AXIOMCLI_SIDECAR` overrides that
path only in development. Production service origins are pinned. Development
accepts a matching production pair, matching staging pair, or a loopback pair;
the loopback API must use HTTPS with a trusted certificate. Arbitrary mixed
origins are rejected. See [service-origins.ts](../apps/desktop/src/main/service-origins.ts)
and [configuration](configuration.md).

## Checks

Run the checks relevant to a change before pushing `dev`; CI currently runs on
pushes to `main` and pull requests targeting `main`, not on development pushes.
The full local checks corresponding to the main workflow are:

```sh
cargo fmt --all -- --check
cargo check --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
pnpm test:rust
scripts/axiomcli-eval deterministic
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

The [evaluation guide](evaluations.md) owns deterministic, real SearXNG and
budgeted live-provider instructions. Live inference always uses registered,
locally verified provider E2EE; it is not a prerequisite for a documentation edit.

The [main CI workflow](../.github/workflows/ci.yml) has Rust/security, browser UI,
macOS/Windows portability, and real-SearXNG jobs. Portability runners cover both
macOS architectures and Windows x64/ARM64. These jobs do not package applications
or establish interactive OS acceptance. [Release workflows](releasing.md) do the
packaging and require separate release evidence.

Portability fixtures compare canonical working directories and keep differently
cased Desktop resource layouts in separate directories, including on macOS's
case-insensitive filesystem. The real SearXNG CI job uses a local HTTP upstream;
public-engine availability has a separate opt-in check in the evaluation guide.

## Desktop UI and captures

The renderer lives in `apps/desktop/src/renderer`; Electron lifecycle, origins,
updates and sidecar ownership live in `src/main`. Keep the preload API narrow:
renderer callers use typed operations rather than arbitrary ACP requests.

```sh
pnpm build:desktop
pnpm --filter axiom-desktop capture
```

Capture mode uses deterministic fixtures for visual review. It does not establish
successful sign-in, payment, live inference or native installer behavior. Dated
interactive reports belong in [testing](testing/README.md) with their environment,
artifact/commit, results and limitations.

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
The original visual proposal is [archived](archive/desktop-redesign.md).

## Documentation changes

Use [the docs index](README.md) as the entry point. Keep one current guide per
topic and link to it from concise component READMEs. Source comments, license
provenance, generated protocol files and runtime prompts retain their required
locations. When moving a document, fix inbound links and distinguish historical
plans/test evidence from current instructions. Carry unresolved work into
[validation](validation.md) before retiring a plan.
