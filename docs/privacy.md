# Privacy behavior

AxiomCLI is currently an unattested development client. Prompts, selected
workspace content, tool schemas, and tool results needed for a turn are sent to
the configured inference endpoint. The current build does not provide TEE
attestation or end-to-end encryption and never labels this connection secure.

Local reads, edits, Git inspection, and process execution happen on the user’s
machine. On supported Linux builds, child commands run through Bubblewrap with
the workspace writable, system directories read-only, a new network namespace,
temporary home directories, an explicit environment, and process-group
cancellation. There is no silent unsandboxed fallback when Bubblewrap is
missing.

`web_search` contacts the configured SearXNG service. Search terms and ordinary
HTTP metadata are therefore visible to that service and to whatever upstream
engines it uses. `fetch_url` is performed locally, with local/private/metadata
destinations and redirect pivots rejected. A configured MCP executable is
trusted user-selected local code and starts with that user's host authority;
therefore MCP commands are accepted only from trusted user configuration, not
repository configuration. Once connected, server descriptions, schemas,
results, and side-effect claims remain untrusted. Tool calls receive only their
explicit protocol arguments from AxiomCLI.

Session events are stored in a local SQLite database under the platform data
directory. AxiomCLI redacts known secret shapes before persistence, bounds
events/exports, and supports explicit export and retention commands. Redaction
is defense in depth, not permission to paste secrets into prompts. Session data
is not encrypted at rest by AxiomCLI; rely on operating-system account and disk
encryption protections.

`axiomcli sessions prune` can delete archived sessions under an explicit
retention policy. Package uninstall intentionally preserves user configuration
and session data. Remove the config/data paths printed by `axiomcli doctor`
manually only when permanent local-data deletion is intended.

Provider credentials are accepted only through `AXIOM_API_KEY`, wrapped as a
secret in memory, redacted from debug output, excluded from child environments,
and absent from deterministic tests and release artifacts.
