# Secure inference Stage 09 review — encrypted tool semantics

Decision: **PASS (reviewed 2026-08-23)**

Implementation commit under review: `ac1a83d` (`feat: implement encrypted relay
inference`)

## Evidence

- Tool definition name/description/schema and named choice are encrypted.
- Assistant call history and tool-result content are encrypted on follow-up.
- Non-stream calls reject duplicate/invalid IDs, non-function types, excessive
  counts, invalid ciphertext, invalid UTF-8, and non-JSON arguments.
- Stream calls reconstruct independent indices with stable IDs/types and
  append-only encrypted name/argument fragments; missing indices/IDs and
  partial terminal states fail before a call becomes executable.
- `auto`, `none`, `required`, and named choice wire forms are explicit. Named
  choices must identify a supplied tool and parallel mode requires tools and a
  matching model capability.
- Tests include nested Unicode schema sentinels, assistant history, tool
  results, fragmented names/arguments, tamper failure, and cancellation/EOF
  terminal rejection.

## Commands and findings

The full Rust/deny gate listed in Stage 06 passed after the final changes.
High, corrected: JSON argument validation was previously left to the agent;
the secure transport now refuses malformed decrypted arguments before they can
be presented as executable. No blocker/high finding remains.

The proxy does not execute or authorize tools. It preserves encrypted protocol
semantics; AxiomCLI's deterministic policy engine or the external client owns
execution authority.

## Exit criteria

Every tool secret in the authoritative contract is encrypted and reconstructed
calls are bounded and unambiguous. Stage 10 may proceed.

