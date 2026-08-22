# ADR 0007: Persisted plans, bounded tasks, and no subagents yet

Status: accepted for the early implementation.

Plans are explicit revisioned artifacts with draft, proposed, approved, and
abandoned states plus line-range comments. Background work is limited to
structured process tasks owned by a shared manager, with independent IDs,
bounded retained output, timeouts, and cancellation tokens.

AxiomCLI does not spawn subagents in this version. Before that changes, a new
ADR must define child identity, budgets, authority inheritance (never broader
than the parent), audit attribution, concurrency limits, and cancellation-tree
cleanup. This avoids prematurely treating a model-created child as a trusted
principal.
