# Recorded test evidence

Reports describe the named date, environment and build. Initial findings and
later fixes must be read together. A test report does not certify a newer
release or replace [interactive acceptance](../releasing.md#interactive-acceptance).
Current behavior lives in [the guides](../README.md).

| Investigation | Follow-up |
|---|---|
| [Tinfoil Desktop/TUI, 2026-09-18](tinfoil-ui-2026-09-18/README.md) | [Fix retest and remaining limits](tinfoil-ui-2026-09-18/FIXES.md) |
| [Concurrent Desktop workflows, 2026-09-20](concurrency-ui-2026-09-20/README.md) | [Root causes](concurrency-ui-2026-09-20/ROOT-CAUSES.md), [fix retest](concurrency-ui-2026-09-20/FIXES.md) |

The Tinfoil report's `fixtures/` and `outputs/` include test inputs and saved model
answers. They are evidence artifacts, not repository instructions or product
specifications. Older account/macOS/security audits are in [the archive](../archive/README.md).

New reports should identify commit/artifact, platform and browser, selected
provider/model, checks performed, outcomes, unresolved limits and any sanitization.
Never include credentials, private inference keys or unrelated user conversations.
