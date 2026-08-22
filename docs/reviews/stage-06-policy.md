# Stage 06 Review — Policy and approvals

Decision: **PASS**

- Every tool request is converted to a typed effect and normalized before
  policy evaluation. Paths become canonical workspace paths, URLs are parsed
  and stripped of fragments/embedded credentials, and invalid process/MCP
  identities fail closed.
- Deterministic precedence is: hard deny, configured deny, read-only-profile
  deny, configured ask, scoped session grant, profile allow, then profile ask.
  Both table tests and a property test show that deny wins independently of
  rule order.
- Trusted-user and project rules are stored separately. Project configuration
  can add only `ask`/`deny` restrictions and cannot replace trusted rules,
  broaden a profile, redirect trusted endpoints, or launch MCP commands.
- Approvals support deny once, allow once, exact-operation session grants, and
  predefined safe-prefix session grants. File grants retain their effect class;
  network grants retain their origin; MCP grants retain side-effect level.
  Process operations deliberately have no prefix grant.
- Configured ask/deny rules and hard denials cannot be converted into session
  grants. Grants are held in the agent runtime for one session and expose no
  self-modification tool.
- TUI and ACP present the same narrow choices. Dropping the TUI responder,
  cancellation, an ACP-style responder failure/disconnect, missing responders,
  and turn/approval timeout paths all deny execution. Resolution events record
  the selected or fail-closed outcome.
- Approval text includes the actual normalized effect and concrete resource.

Review commands on 2026-08-22:

```text
cargo clippy --locked --manifest-path axiomcli/Cargo.toml --all-targets --all-features -- -D warnings
cargo test --locked --manifest-path axiomcli/Cargo.toml --all-features --no-fail-fast
```

The focused policy, approval lifecycle, configuration-precedence, TUI-drop,
and agent disconnect/timeout tests pass. No operation executes on a fail-closed
path.
