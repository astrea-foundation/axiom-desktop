# ADR 0016: Byte-bounded local history pages

Status: implemented.

History is read from the account-scoped local SQLite database and transported
only between native processes on the same computer. Row-count pagination alone
can produce a response larger than Desktop's 64 MiB ACP frame ceiling. Increasing
the frame ceiling does not bound the total content of a page.

Negotiate timeline version 2. Bound serialized timeline content per page and page
request usage independently with a stable cursor. Read only the active turn for
wire snapshots; Desktop assembles all pages atomically at one thread revision and
rejects repeated/non-progressing cursors. Keep the existing message and attachment
limits, native schema version and local account-generation checks. Correct the
transport error text to name its actual 64 MiB limit.

The [coordinated plan](https://github.com/astrea-foundation/axiom-platform/blob/dev/docs/architecture/release-readiness-plan.md)
also covers account deletion and financial recovery. This decision creates no
hosted message API and never uploads history for pagination.
