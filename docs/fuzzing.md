# Fuzzing secure protocol boundaries

The separate `fuzz/` workspace exercises five untrusted-input boundaries:

- authenticated ciphertext wire parsing;
- attestation evidence parsing;
- encrypted relay SSE parsing;
- OpenAI request validation;
- streamed tool-call fragment assembly.

Install `cargo-fuzz`, use a Rust nightly toolchain, and run a target from the
repository root:

```text
cargo +nightly fuzz run ciphertext -- -max_len=16384
cargo +nightly fuzz run evidence -- -max_len=16384
cargo +nightly fuzz run sse -- -max_len=16384
cargo +nightly fuzz run openai_request -- -max_len=16384
cargo +nightly fuzz run tool_fragments -- -max_len=16384
```

`fuzz/corpus/`, `fuzz/artifacts/`, and fuzz build output are intentionally
ignored. A crashing input must be minimized, converted into a deterministic
regression test when practical, and reviewed for secret content before any
corpus file is committed. The fuzz feature exposes only narrow parser entry
points; it does not expose credentials, verified-session constructors, or
provider key material.

Dependency and license policy covers this workspace separately:

```text
cargo deny --manifest-path fuzz/Cargo.toml --config deny.toml check
```
