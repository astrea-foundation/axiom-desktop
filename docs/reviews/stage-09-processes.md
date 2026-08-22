# Stage 09 Review — Processes and background tasks

Decision: **PASS**

- Only structured program/argument execution is exposed; no general shell tool
  exists. Policy nevertheless models shell as higher risk for future use.
- Child cwd is the canonical workspace. The environment is cleared and only a
  small locale/path allowlist is restored, preventing inference/API-key
  inheritance.
- Stdout and stderr are concurrently drained, retained within caps, and marked
  truncated. Timeouts and cancellation terminate Unix process groups.
- Background start/status/stop uses independent IDs and cancellation state.
- The deterministic evaluation edits a real Rust fixture, runs real `cargo
  test`, observes success, and reports completion.
