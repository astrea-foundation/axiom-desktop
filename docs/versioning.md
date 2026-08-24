# Versioning and compatibility policy

AxiomCLI follows Semantic Versioning for tagged releases. Before 1.0, a minor
version may intentionally break CLI/library/configuration behavior; patch
versions remain bug-fix compatible unless a security issue requires a narrowly
documented exception. Every breaking change must include migration and release
notes.

The persisted event envelope has its own integer schema version. Readers accept
known older versions through tested transactional migrations, reject newer
unknown versions, preserve stable session IDs, and never replay interrupted
side effects. Destructive storage migrations require a backup/export path and a
separate review. The current database supports schema versions exercised by the
v0/v1/v2 fixtures and migrates them to SQLite schema version 3, whose
`app_settings` table stores application-wide preferences such as the last-used
model and thinking level. Event envelopes also currently write schema version
3. Additive defaults keep older permission and question events readable;
legacy `auto` thinking values migrate in memory to explicit `medium`.

ACP support targets stable ACP v1 through `agent-client-protocol` 2.0.0.
Capabilities are negotiated and unsupported operations are not advertised.
Structured form elicitation is explicitly unstable and is used only when the
client advertises it. An ACP behavior change that alters advertised capability,
wire mapping, session identity, or cancellation semantics requires updating the
compatibility matrix and transcript suite.

Release support is currently Linux x86-64 GNU. macOS and Windows are source-level
portability targets, not supported binaries, until their process isolation,
terminal, installer, and security gates match Linux. Release artifacts are
locked, checksummed, signed by a configured maintainer key, and contain build
metadata plus license inventory.
