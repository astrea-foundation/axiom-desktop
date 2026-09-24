// Synthetic presentation data only; never used by the provider verifier.
import type { SecurityEvidence } from "../packages/axiom-acp-client/src/generated/protocol.js";
export function securityEvidence(now = Date.now()): SecurityEvidence {
  return {
    status: { state: "verified" }, providerId: "near", modelId: "test-model",
    attestationProtocol: "near-tdx-nvidia-v1", e2eeProtocol: "near-e2ee-v2", e2eeEncryptionVersion: 2,
    trustPolicyVersion: "axiom-native-v1", verifiedAtUnixSeconds: Math.floor(now / 1000) - 24,
    hardExpiresAtUnixSeconds: Math.floor(now / 1000) + 300, attestationGeneration: 7,
    modelKeyFingerprint: "a1b2c3d4".repeat(8), tlsSpkiFingerprint: "b2c3d4e5".repeat(8),
    checks: [
      ["Fresh challenge", "matched"], ["Model identity", "matched"], ["Model encryption key", "bound"],
      ["Live TLS endpoint", "bound"], ["Intel TDX", "UpToDate"], ["NVIDIA GPU", "verified"], ["Workload manifest", "quote-bound"],
    ].map(([name, detail]) => ({ name: name!, detail: detail!, passed: true })),
    providerClaims: [{ name: "workload_manifest_sha256", value: "c3d4e5f6".repeat(8) }],
    workloadManifest: JSON.stringify({ name: "Synthetic UI fixture", services: { model: { image: "example.invalid/model@sha256:" + "c3d4e5f6".repeat(8) } } }, null, 2),
  };
}
