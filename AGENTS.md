# Agent Instructions

## Branch workflow

Use `dev` for ongoing development and push incremental changes there. Start
larger features from `dev` and target `dev` in their pull requests. Do not push
routine changes directly to `main` or promote them automatically after a task.

`main` is the release branch. When the user requests a promotion or release,
open a `dev` → `main` pull request, validate the batch, and use a merge commit
to preserve shared history. Merge `main` back into `dev` afterward. Production
deployments and published installers must come from the promoted revision on
`main`; previews and staging can use `dev`. See [CONTRIBUTING.md](CONTRIBUTING.md).

## Repository ownership

This repository owns Axiom Desktop, AxiomCLI, the local proxy, and installer
builds. All hosted website and account UI, release catalogs, web assets and
Worker deployment tools belong exclusively in the companion backend repository,
`axiom`, under `landing/` and `auth/`. Do not add hosted web workspaces here.

Native branding and the compiled local loopback callback page remain in
`packages/brand/`. Keep this repository independently buildable. Desktop consumes
the public release API; update tests use native fixtures, never a neighboring
website checkout. Installer files and their generated manifest are the handoff
to the backend repository's CDN and website publisher.
