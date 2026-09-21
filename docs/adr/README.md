# Architecture decisions

ADRs preserve rationale. Current commands and behavior belong to the
[guides](../README.md); implementation details below can be superseded without
erasing the original decision.

| Decision | Status / current reference |
|---|---|
| [0001: Command/event kernel](0001-command-event-kernel.md) | Accepted; [architecture](../architecture.md) |
| [0002: Async and errors](0002-async-and-errors.md) | Accepted implementation conventions |
| [0003: Early configuration](0003-configuration.md) | Superseded details; [configuration](../configuration.md) |
| [0005: SQLite and recovery](0005-storage-and-recovery.md) | Accepted storage design; schema details superseded by [versioning](../versioning.md) |
| [0006: Policy and approvals](0006-policy-and-approvals.md) | Accepted; [security](../threat-model.md) defines actual OS boundaries |
| [0007: Plans/concurrency/subagents](0007-plans-concurrency-and-subagents.md) | Accepted scope; background tasks exist, subagents do not |
| [0009: Monorepo boundaries](0009-monorepo-and-secure-client-boundaries.md) | Accepted; [architecture](../architecture.md) |
| [0010: Provider-owned sessions](0010-provider-owned-secure-sessions.md) | Accepted abstraction; current providers in [security](../threat-model.md) |
| [0011: Standard ACP and extension](0011-standard-acp-and-negotiated-axiom-extension.md) | Accepted architecture; current Agent/account/billing contracts in [ACP](../acp-compatibility.md) |
| [0012: Leases and trust policy](0012-expiring-attestation-leases-and-signed-trust-policy.md) | Partially superseded by 0013 and current Phala provenance semantics |
| [0013: NEAR worker sessions](0013-near-worker-sessions.md) | Implemented contract; [NEAR v3](../protocols/near-v3.md); deployment evidence is dated |

Numbering gaps reflect repository history; no missing numbered ADR is required
to follow the current implementation.
