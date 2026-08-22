# ADR 0007: Persisted plans, bounded tasks, and no subagents yet

Status: accepted for the early implementation.

Plans are explicit revisioned artifacts with draft, proposed,
revision-requested, approved, and abandoned states plus line-range comments.
The `plan` policy profile is a hard intersection: reads are allowed, the
dedicated `.axiomcli/plans` capability is allowed, and all other effects are
denied before configurable asks or session grants. Approval records review; it
does not execute a plan.

Structured questions support one to three fields. The TUI keeps question input
separate from the message draft. ACP uses form elicitation only when the client
advertises the unstable capability and otherwise fails closed.

Background work is limited to four concurrent structured process tasks owned by
a shared manager, with independent IDs, monotonically ordered display,
permission-profile provenance, bounded retained output, per-task timeouts, and
child cancellation tokens. Parent turn cancellation reaches every child
process group.

AxiomCLI does not spawn subagents in this version. Before that changes, a new
ADR must define child identity, budgets, authority inheritance (never broader
than the parent), audit attribution, concurrency limits, and cancellation-tree
cleanup. This avoids prematurely treating a model-created child as a trusted
principal.
