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

CI checks pushes to `dev` and `main`, and pull requests targeting either branch.
It runs tests and portability checks without packaging release archives or
Desktop applications. Feature branches use PR checks rather than duplicate push
runs; superseded runs are cancelled.

GitHub Actions generates release artifacts only from stable `vMAJOR.MINOR.PATCH`
tags whose commits have been promoted to `main` and whose version matches the
product. The signed release workflow runs when the tag is pushed. The desktop
installer workflow can be run manually against that tag, including the explicit
unsigned option for early-access releases; branch dispatches skip packaging.
See [the release procedure](docs/releasing.md) for the command. Building an
artifact does not update the public download catalog; publishing remains an
explicit release step.

Use the checks and packaging commands documented in [README.md](README.md).
If a change spans `axiom` and `axiomai`, promote compatible batches in both and
record their commit IDs together. Their branch names do not synchronize them.

## Implementation guidelines

This is an early codebase. Prefer the smallest change that establishes a clear
contract and leaves room to revise it.

1. Work only in the active stage from the implementation plan.
2. Add or update tests for externally observable behavior.
3. Keep terminal rendering, ACP translation, and persistence out of the agent
   domain model.
4. Treat model, network, tool, terminal, and ACP input as untrusted.
5. Do not copy implementation code, prompts, comments, names, or tests from a
   reference agent.
6. Run formatting, Clippy, tests, and the current stage's review checklist.
7. Do not start the next stage until the current review record is `PASS`.

Application-specific presentation and lifecycle code belongs under `apps/`.
Code belongs under `crates/` only when it has a narrow contract and more than
one application-level consumer. Shared crates must not read environment
variables or choose application policy on behalf of their callers.

Public APIs can change before `1.0`, but changes should still be intentional and
documented in an ADR when they affect architecture or persisted data.
