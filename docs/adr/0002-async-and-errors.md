# ADR 0002: Async runtime and error boundaries

Status: accepted for the early implementation

## Decision

Use Tokio for asynchronous orchestration. Cancellation is explicit through
`CancellationToken`; dropping a future is not the only cancellation mechanism.

Domain errors are typed and safe to show. Adapter and dependency errors are
wrapped at boundaries. Detailed diagnostics go to redacted tracing; user events
contain stable categories and actionable summaries.

ACP mode reserves stdout for protocol frames. TUI and regular CLI diagnostics
use stderr before terminal initialization and the transcript after it.
