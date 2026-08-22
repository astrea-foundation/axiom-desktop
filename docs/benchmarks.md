# Development baselines

These are observations, not release targets. Re-measure when dependencies or
startup behavior changes.

| Date | Build/host | Probe | Result |
|---|---|---|---|
| 2026-08-21 | release, Linux x86_64 development host | `axiomcli --version` | <0.01 s elapsed; 5,424 KiB max RSS |

Interactive idle CPU and long-session memory need a repeatable terminal
benchmark before a public release threshold is set. The event runtime uses
bounded broadcast capacity, tool/process/network outputs are capped, and the
current transcript remains in memory for the active TUI session.
