# ADR 0010: Provider-owned verified secure sessions

Status: accepted

## Context

AxiomAI will initially support NEAR's `near-v2` protocol, but future inference
providers may use different TEE vendors, evidence formats, key exchanges,
session lifetimes, TLS bindings, and encrypted stream framing. A generic pair of
`Attestor` and `Cipher` traits makes it possible for application code to combine
verified key material from one operation with the wrong cipher or endpoint.

AxiomCLI and the standalone proxy need the same production trust path without
sharing presentation, agent policy, or OpenAI compatibility concerns.

## Decision

`axiom-secure-client` owns provider selection and secure-session establishment.
Each `SecureProvider` implementation owns its complete sequence of evidence
retrieval, local verification, key/endpoint binding, cryptographic session
creation, encrypted request construction, and response decryption.

Successful establishment returns an opaque `VerifiedSession`. Consumers cannot
construct one from arbitrary public-key bytes. The session exposes normalized
`SecurityEvidence`, but never exposes its secret material or a bare trusted key.

The model catalog selects a provider driver by stable provider/protocol/version
identifiers. The selected driver validates all catalog claims against its
compiled capabilities. Applications do not branch on provider IDs.

`axiom-inference` remains transport-neutral. `axiom-openai-compat` translates
edge wire formats only. AxiomCLI calls `axiom-secure-client` in-process;
`axiom-proxy` composes the same client behind loopback HTTP.

The first provider remains an internal module of `axiom-secure-client`. It moves
to a separate crate only when a second provider demonstrates a real dependency
or compilation boundary.

## Consequences

- An attestation result cannot be casually separated from the endpoint and
  cryptographic mechanism it authenticated.
- Provider-specific claims can be normalized for UI/audit while retaining typed
  provider detail.
- A new provider can bring a completely different TEE/E2EE handshake without
  changing AxiomCLI or the proxy.
- Provider modules are larger responsibility units and require focused tests.
- Test-only provider drivers may exercise registry mechanics, but production
  builds contain no provider that reports verification without performing it.
