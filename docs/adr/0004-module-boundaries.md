# ADR 0004: Modules before crates

Status: superseded by ADR 0009 for the product monorepo

## Decision

Begin with cohesive modules in the single `axiomcli` package. Extract a crate
only after a boundary has tests and multiple real consumers. The intended
dependency flow is adapters → application/agent → provider/tool/policy traits;
infrastructure implements those traits and does not leak into the domain.

This favors easy revision during the early version over a prematurely stable
public crate graph.
