# ADR 0007: Persisted plans, bounded tasks, and no subagents yet

Status: accepted for the early implementation.

Plans are explicit revisioned artifacts with draft, proposed,
revision-requested, approved, and abandoned states plus line-range comments.
The original `plan` policy profile was a hard intersection: reads were allowed,
the dedicated `.axiomcli/plans` capability was allowed, and all other effects
were denied before configurable asks or session grants. That profile has since
been retired: legacy sessions migrate to Observe, while modifying plan
artifacts now follows the selected tool profile. Approval records review; it
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
