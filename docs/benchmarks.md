# Development baselines

These are observations, not release targets. Re-measure when dependencies or
startup behavior changes.

| Date | Build/host | Probe | Result |
|---|---|---|---|
| 2026-08-21 | release, Linux x86_64 development host | `axiomcli --version` | <0.01 s elapsed; 5,424 KiB max RSS |
| 2026-08-22 | debug, Linux x86_64 development host | ten `axiomcli --version` starts | 52 ms total (3000 ms gate) |
| 2026-08-22 | debug, Linux x86_64 development host | TUI idle for one measured second | 25,852 KiB RSS; 0 scheduler ticks (65,536 KiB / 20-tick gates) |
| 2026-08-22 | test, Linux x86_64 development host | 10,000 stream chunks + 5,000 SQLite transcript entries | 2,502 ms journal time; 2,781,184-byte database (10 s / 16 MiB gates) |
| 2026-08-22 | test, Linux x86_64 development host | six batches / 24 isolated background tasks | 102 ms; all reaped and output-bounded (15 s gate) |

These numbers are development-host observations rather than universal promises.
The automated gates intentionally leave headroom for shared CI hosts. The event
runtime uses bounded broadcast capacity, tool/process/network outputs are capped,
exports are capped, and the active TUI transcript remains in memory until the
session closes.
