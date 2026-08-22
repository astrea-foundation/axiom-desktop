# Stage 09 Review — Processes and background tasks

Decision: **PASS**

- `run_command` accepts a program and argument vector without shell
  interpolation. `run_shell` is a separate surface, and policy also recognizes
  disguised `sh -c`-style calls. Every shell effect remains approval-gated even
  in `autonomous`; allowlisted structured development commands can run in
  `workspace`.
- The Linux release-candidate path requires Bubblewrap and fails closed if it is
  unavailable. It creates a namespace without network access, exposes the
  canonical workspace read/write, mounts system runtime files read-only, and
  does not expose the host home directory.
- Child environments start empty. Only a small named settings allowlist can be
  supplied. Cargo/Rust caches are mounted read-only at paths which exclude Cargo
  credentials, while build outputs remain inside the workspace.
- Cwd values are canonicalized and revalidated inside the workspace immediately
  before spawn. Arguments are passed directly to the program. Timeout and
  cancellation terminate the entire Unix process group, including descendants.
- Stdout and stderr are drained concurrently under one shared retention budget.
  Invalid UTF-8 is made lossy, ANSI/OSC/control sequences are stripped, output
  remains bounded, and truncation is explicit. A disconnected caller cannot
  stop pipe draining or leave a blocked child behind.
- Background start/list/status/wait/terminate has bounded task count, stable
  task IDs, live bounded stdout/stderr snapshots, terminal states, and wait
  timeouts. Semantic background-state events are consumed by both the TUI and
  ACP adapter.
- Linux tests prove host-home and network isolation, environment and cwd
  restrictions, flood/invalid-byte/ANSI handling, timeout recovery, descendant
  cancellation, and live background lifecycle accuracy.
- The deterministic fixture agent now performs a real failing `cargo test`,
  consumes the failure, applies a version-checked repair, reruns the same real
  command, and observes success. This exposed and corrected an agent-loop guard
  that had confused a post-edit rerun with an immediate repeated call.

Validation evidence:

```text
cargo clippy -p axiomcli --all-targets -- -D warnings    PASS
cargo test -p axiomcli                                  PASS
cargo test -p axiomcli --test autonomous_eval           PASS (3/3)
cargo deny check                                        PASS
```

Supported release-candidate platform for this stage: Linux x86-64. Non-Linux
execution keeps the same empty-environment/process-group interface for compile
portability, but is not advertised as an isolated release path.
