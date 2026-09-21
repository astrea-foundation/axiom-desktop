# Historical plans and audits

These files retain dated proposals and evidence. Their statements about “current”
behavior refer to the original work, and unchecked boxes are not proof that the
work remains undone today. They are not current deployment instructions.
Use [the current guides](../README.md) and [validation follow-up](../validation.md).

| Record | Why retained | Current authority |
|---|---|---|
| [Account/auth/billing plan](account-auth-billing-clean-slate-plan.md) | Original cross-repository design, implementation record and unfinished rollout/acceptance checklist | [Architecture](../architecture.md), [Desktop](../desktop.md), platform operations |
| [V1 refactor plan](v1-clean-slate-refactor-plan.md) | Storage/ACP redesign rationale and implementation evidence | [ACP](../acp-compatibility.md), [versioning](../versioning.md) |
| [Account QA, 2026-09-04](account-interactive-qa-2026-09-04.md) | Scoped sign-in, vault and billing observations | [Validation](../validation.md) and per-release acceptance |
| [macOS audit, 2026-09-09](macos-compatibility-audit-2026-09-09.md) | Packaging observations and then-open target-machine tests | [Releasing](../releasing.md) |
| [Desktop redesign hand-off](desktop-redesign.md) | Original visual decisions, captures and unfinished design items | [Development](../development.md), [Desktop](../desktop.md) |
| [NEAR v2 contract](near-v2.md) | Retired protocol reference | [NEAR v3](../protocols/near-v3.md) |
| [Security audit, 2026-09-04](security-audit-2026-09-04.md) | Original scoped controls and test conclusions | [Security](../threat-model.md) |

Archived on 2026-09-21 during the source-grounded consolidation. Original plan
checklists remain intact. Paths in historical prose/code blocks retain their
original checkout context; navigable links have been repaired where a surviving
authority is available. In particular, initial schema-reset/cutover proposals do
not authorize deleting released account data. Current platform migration and
payment operations belong to
[axiom-platform](https://github.com/astrea-foundation/axiom-platform/blob/dev/docs/README.md).
