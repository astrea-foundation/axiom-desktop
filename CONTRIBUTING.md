# Contributing to Axiom

Both Axiom repositories use the same branch workflow:

- `dev` collects ongoing work. Push small, tested changes here.
- Feature branches start from `dev` and merge back into `dev` through pull requests.
- `main` contains batches explicitly promoted for release.
- `dev` is this repository's GitHub default branch.

## Everyday work

```bash
git fetch origin
git switch dev
git pull --ff-only origin dev
# Make changes, run the relevant checks, and commit.
git push origin dev
```

For a larger change, create a feature branch from the updated `dev` checkout:

```bash
git switch -c feat/short-description
# Make changes, run the relevant checks, and commit.
git push -u origin HEAD
gh pr create --base dev
```

Always specify the pull request base: use `dev` for development and `main` only
for an explicitly authorized promotion.
Do not force-push or delete either shared branch.

## Promote a batch

Promote when a batch is ready for release, not after every incremental task.
An explicit request to promote or merge into `main` authorizes this step. A general
request to merge, build, or finish work does not authorize promotion to `main`.

1. Open a pull request with `gh pr create --base main --head dev`. Describe the
   complete batch and its validation.
2. Run the relevant checks and review the result. Resolve conflicts on `dev`
   by merging `origin/main` into it and pushing the resolution.
3. Merge the pull request using **Create a merge commit** (`gh pr merge <number> --merge`).
   Do not squash or rebase this long-lived branch, and keep `dev` after merging.
4. Sync the resulting merge commit back into `dev`:

   ```bash
   git fetch origin
   git switch dev
   git merge origin/main
   git push origin dev
   ```

New work may already be on `dev`; this merge preserves it. Future development
continues on `dev`. Avoid direct commits or pushes to `main`.

## Releases

CI checks pull requests targeting `main`; merges do not repeat those checks.
Pushes to development branches and pull requests targeting `dev` do not start
Actions automatically. Run relevant local checks before pushing. Manual CI
dispatch supports targeted native validation and retries; releases require the
complete set of successful checks for their exact tagged revision. CI does not
package applications; superseded runs for the same target are cancelled.

GitHub Actions is limited to CI and release artifact production. There is no
standalone scheduled or manually dispatched live-evaluation workflow. Run ad hoc
live evaluations locally; the signed release workflow retains its live-provider
regression gate as part of producing release artifacts.

GitHub Actions generates release artifacts only from stable `vMAJOR.MINOR.PATCH`
tags whose commits have been promoted to `main` and whose version matches the
product. The signed release workflow runs when the tag is pushed, builds both
Desktop and standalone installers, and hands their signed inventory to the
platform publisher. The separate Preview installers workflow can run on `dev`
and produces unsigned Actions artifacts with stable updates disabled. It does
not publish downloads or change a feed. See [the release procedure](docs/releasing.md)
for setup, native acceptance and publication commands.

Use the [development checks](docs/development.md#checks) and
[release procedure](docs/releasing.md).
If a change spans `axiom-platform` and `axiom-desktop`, promote compatible batches in both and
record their commit IDs together. Their branch names do not synchronize them.

## Implementation guidelines

This is an early codebase. Prefer the smallest change that establishes a clear
contract and leaves room to revise it.

1. Keep changes within the requested scope; use current guides rather than archived plans.
2. Add or update tests for externally observable behavior.
3. Keep terminal rendering, ACP translation, and persistence out of the agent
   domain model.
4. Treat model, network, tool, terminal, and ACP input as untrusted.
5. Do not copy implementation code, prompts, comments, names, or tests from a
   reference agent.
6. Run formatting, Clippy and tests appropriate to the change; record any failures.
7. Update affected documentation with the implementation and preserve unresolved
   acceptance work in [validation](docs/validation.md).

Application-specific presentation and lifecycle code belongs under `apps/`.
Code belongs under `crates/` only when it has a narrow contract and more than
one application-level consumer. Shared crates must not read environment
variables or choose application policy on behalf of their callers.

Public APIs can change before `1.0`, but changes should still be intentional and
documented in the architecture and versioning guides when they affect shared
contracts or persisted data.
