# Phala gateway source policy

Compatible Phala gateway updates do not require manual release approval or a
repository/commit allowlist. Backend and native clients verify the reported
source against the exact TEE-measured workflow, along with fresh nonce-bound
keys, worker sessions, model identity, and the final signed response receipt.
The historical suffixes in ACI profile IDs identify supported wire formats,
not approved source revisions. Unknown or mixed formats still fail closed.

Version 0.1.1 restores this behavior after the protocol refactor reintroduced
source pins. Regression tests accept source updates and reject mismatched
provenance, modified manifests, stale/invalid quotes, and invalid stream receipts.
