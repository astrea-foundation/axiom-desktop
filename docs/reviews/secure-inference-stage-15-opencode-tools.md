# Secure inference Stage 15 review — real OpenCode tool loops

Decision: **PASS (reviewed 2026-08-23)**

Implementation commit under review: `5feb383` (`test: verify live OpenCode tool
loops`)

External client: OpenCode `1.17.18`, installed into a disposable temporary
prefix and invoked without source modifications.

Live model: `glm-5-2`, `medium` reasoning, through the actual `axiom-proxy`
binary and live Axiom service.

## Evidence

`scripts/smoke-opencode-tools` passed twice consecutively with a development
credential. Each run:

- created isolated HOME/XDG directories and a disposable workspace;
- supplied OpenCode only a generated localhost proxy credential;
- denied shell, network fetch, external directories, subagents, and all
  unlisted tools;
- completed read `target.txt` → edit exactly one value → read again → non-empty
  final text in structured OpenCode events;
- produced the exact bytes `TOKEN=after\n` and no unexpected workspace entry;
- completed two independent reads (`alpha.txt` and `beta.txt`) with one shared
  assistant message ID, proving a parallel tool proposal;
- opened a verified streaming request, terminated the real OpenCode process,
  observed the proxy record cancellation, and proved `cancel-target.txt`
  remained byte-for-byte unchanged;
- scanned the full isolated artifact tree for both credentials and scanned
  proxy operational logs for controlled file values and paths.

The actual proxy log contained completed and cancelled terminal states, and no
cancelled request was recorded as completed. Confidentiality of the relay JSON
for definitions, selected calls, arguments, and results is independently
captured by the Stage 13 actual-process fixture using the same production
request builder.

## Commands

All passed:

```text
npm install --prefix <disposable-prefix> opencode-ai@1.17.18
node node_modules/opencode-ai/postinstall.mjs
opencode --version
scripts/smoke-opencode-tools
scripts/smoke-opencode-tools
bash -n scripts/smoke-opencode-tools
```

## Findings

- High, corrected: OpenCode assumed a larger default output limit than the
  selected model, so the secure client correctly rejected the request before
  relay execution. The harness now maps catalog context/output limits into the
  OpenCode model definition.
- High, corrected: the first cancellation check timed out during attestation,
  which did not prove a remote stream was cancelled. The proxy now emits a
  metadata-only `request_stream_open` event; the harness waits for it before
  terminating OpenCode and requires a later `cancelled` terminal record.
- Medium, corrected: a successful serial tool loop alone did not cover parallel
  calls. A separate real turn now requires and verifies two read calls under
  one assistant message ID.
- Medium, corrected: credential scanning initially covered selected logs only.
  It now scans the entire disposable HOME/XDG/workspace artifact tree.
- Blocker/high unresolved: none in the Stage 15 exit scope.

## Exit criteria

Lane E passes repeatedly with the real proxy, real Axiom provider, and an
unmodified external coding client. Ordered and parallel tools survive encrypted
transport, while cancelled partial work receives no local authority. Stage 16
may proceed.
