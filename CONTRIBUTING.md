# Contributing to AxiomAI

Both Axiom repositories use the same branch workflow:

- `dev` collects ongoing work. Push small, tested changes here.
- Feature branches start from `dev` and merge back into `dev` through pull requests.
- `main` contains batches promoted for release. It remains the GitHub default branch.

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

Always specify the pull request base: GitHub otherwise defaults to `main`.
Do not force-push or delete either shared branch.

## Promote a batch

Promote when a batch is ready for release, not after every incremental task.
An explicit request to promote, merge into `main`, or release authorizes this step.

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

CI checks pushes to `main` and pull requests targeting `main`. Pushes to `dev`
or other development branches, and pull requests targeting `dev`, do not run
GitHub Actions. Run the relevant checks locally before pushing development
changes. Main CI runs tests and portability checks without packaging release
archives or Desktop applications; superseded runs are cancelled.

Scheduled and manually dispatched live evaluations run only on `main`.

GitHub Actions generates release artifacts only from stable `vMAJOR.MINOR.PATCH`
tags whose commits have been promoted to `main` and whose version matches the
product. The signed release workflow runs when the tag is pushed. The desktop
installer workflow can be run manually against that tag, including the explicit
unsigned option for early-access releases; branch dispatches skip packaging.
See [the release procedure](docs/releasing.md) for the command. Building an
artifact does not update the public download catalog; publishing remains an
explicit release step.

Use the [development checks](docs/development.md#checks) and
[release procedure](docs/releasing.md).
If a change spans `axiom-platform` and `axiomai`, promote compatible batches in both and
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
documented in an ADR when they affect architecture or persisted data.
