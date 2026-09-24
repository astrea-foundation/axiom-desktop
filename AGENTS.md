# Agent Instructions

## Explicit outdated-TEE consent

A local user may accept Intel's exact `OutOfDate` TCB status for NEAR until the
native runtime restarts. Present accepted evidence as degraded with a yellow
warning. Default behavior remains strict. This exception never permits expired
evidence, revocation, configuration faults, invalid signatures, nonce/key/model
or TLS binding mismatches, GPU verification failures, or unauthenticated replies.
Keep consent out of persisted preferences and signed trust-policy documents.

## Branch workflow

Use `dev` for ongoing development and push incremental changes there. Start
larger features from `dev` and target `dev` in their pull requests. Do not push
routine changes directly to `main` or promote them automatically after a task.

`main` is the release branch. **Promotion to `main` authorizes production release
work; do not promote unless the user explicitly says to promote to `main`.** A
request to merge changes, build, or finish work does not authorize promotion.
Signed installers are published by stable version tags on promoted `main` commits;
never create or push release tags without explicit release authorization.

When explicitly authorized, open a `dev` → `main` pull request, validate the batch,
and use a merge commit to preserve shared history. Merge `main` back into `dev`
afterward. Production
deployments and published installers must come from the promoted revision on
`main`; previews and staging can use `dev`. See [CONTRIBUTING.md](CONTRIBUTING.md).

## Repository ownership

This repository owns Axiom Desktop, AxiomCLI, the local proxy, and installer
builds. All hosted website and account UI, release catalogs, web assets and
Worker deployment tools belong exclusively in the companion backend repository,
`axiom-platform`, under `landing/` and `auth/`. Do not add hosted web workspaces here.

Native branding and the compiled local loopback callback page remain in
`packages/brand/`. Keep this repository independently buildable. Desktop consumes
the public release API; update tests use native fixtures, never a neighboring
website checkout. Installer files and their generated manifest are the handoff
to the backend repository's CDN and website publisher.

## Documentation

Put authored guides, plans, specifications, runbooks, QA instructions and release
notes in `docs/`, linked from [the documentation index](docs/README.md). Root
entry points, concise component READMEs, license/provenance files, generated
contracts and runtime Markdown assets keep their required locations. Detailed
guides belong in `docs/`; link to one authoritative document instead of copying it.

When changing documented behavior, APIs, configuration, security, commands,
deployment or tests, update the affected documents in the same change. Repair
inbound links when moving or deleting files, including companion-repository
links. Preserve unfinished release requirements in `docs/validation.md`. Keep
public guides focused on current behavior; internal plans, incident records and
raw QA captures belong in private maintainer records.

## Inference authentication policy

Inference requires provider E2EE to a key bound to fresh, locally verified TEE
evidence. Authenticate each response to the accepted worker and request. A
separate signed inference receipt is not universally required when the E2EE
exchange establishes those bindings. Preserve each registered protocol's actual
authentication checks; current NEAR implementations still use their
signatures/receipts. Streaming must authenticate ordering and completion before
success. There is no plaintext, ordinary-TLS-only, or unverified-key fallback.
