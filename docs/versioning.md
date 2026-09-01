# Versioning and compatibility policy

AxiomCLI follows Semantic Versioning for tagged releases. Before 1.0, a minor
version may intentionally break CLI/library/configuration behavior; patch
versions remain bug-fix compatible unless a security issue requires a narrowly
documented exception.

The first supported local database contract is SQLite schema version 1. It
stores threads, turns, timeline items, profile preferences, collections, and
thread-to-collection assignments. Runtime event envelopes are not the storage
format. A fresh database is created at version 1; an unknown, malformed, or
newer database is rejected without modification. There are no compatibility
paths for pre-release databases. The first post-release schema change will be a
transactional version 2 migration that preserves released data.

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
