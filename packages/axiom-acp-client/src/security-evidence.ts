import type { SecurityEvidence, VerifySecurityResponse } from "./generated/protocol.js";
import { ProtocolError } from "./errors.js";

const MAX_WORKLOAD_MANIFEST_BYTES = 1024 * 1024;

/** Validate and project only public evidence fields before renderer/clipboard use.
 * This checks the report contract, never replaces the native cryptographic verifier. */
export function publicSecurityEvidence(result: VerifySecurityResponse, modelId: string): SecurityEvidence | null {
  const evidence = result.evidence;
  if (result.status.state !== "verified" && result.status.state !== "degraded") return null;
  const degraded = result.status.state === "degraded";
  const text = (value: unknown, max = 512): value is string => typeof value === "string" && value.length > 0 && value.length <= max;
  const positive = (value: unknown): value is number => Number.isSafeInteger(value) && Number(value) > 0;
  const fingerprint = (value: unknown) => typeof value === "string" && /^[a-f0-9]{64}$/i.test(value);
  const directTinfoil = evidence?.providerId === "tinfoil"
    && evidence.attestationProtocol === "tinfoil-snp-sigstore-v1"
    && evidence.e2eeProtocol === "tinfoil-ehbp-v1";
  // Display-only contract for the explicit local debug adapter. This report
  // must retain its missing provenance check and can never claim verified.
  const directPreview = degraded && evidence?.providerId === "chutes-preview"
    && evidence.modelId === "chutes-preview:qwen3.8-27b"
    && evidence.attestationProtocol === "chutes-hardware-development-v1"
    && evidence.e2eeProtocol === "chutes-mlkem768-desktop-v1"
    && evidence.e2eeEncryptionVersion === 1 && Array.isArray(evidence.checks)
    && ["Intel TDX", "NVIDIA GPU", "Chutes encryption key and fresh challenge"]
      .every(name => evidence.checks.some(check => check.name === name && check.passed === true))
    && evidence.checks.some(check => check.name === "Application provenance"
      && check.passed === false && check.detail === "unqualified development preview");
  // The native adapter has already checked the installed publisher pin. This
  // projection describes hardware + key + release authorization, never claims
  // independent measurement of the application image or model weights.
  const directRelease = !degraded && ((evidence?.providerId === "chutes-preview"
    && evidence.modelId === "chutes-preview:qwen3.8-27b") || (evidence?.providerId === "chutes"
    && evidence.modelId === "chutes:qwen3.8-27b"))
    && evidence.attestationProtocol === "chutes-tdx-nvidia-release-v1"
    && evidence.e2eeProtocol === "chutes-mlkem768-desktop-v1"
    && evidence.e2eeEncryptionVersion === 1 && Array.isArray(evidence.checks)
    && ["Intel TDX", "NVIDIA GPU", "Chutes encryption key and fresh challenge", "Repository release authorization"]
      .every(name => evidence.checks.some(check => check.name === name && check.passed === true))
    && evidence.checks.some(check => check.name === "Intel TDX" && check.detail === "UpToDate")
    && Array.isArray(evidence.providerClaims)
    && evidence.providerClaims.some(claim => claim.name === "release_authority_fingerprint" && fingerprint(claim.value))
    && evidence.providerClaims.some(claim => claim.name === "release_channel"
      && (claim.value === "staging" || (evidence.providerId === "chutes" && claim.value === "production")))
    && evidence.providerClaims.some(claim => claim.name === "execution_provenance"
      && claim.value === "Not independently established by the Chutes evidence; release authorization is a separate claim.");
  const validGeneration = evidence?.attestationGeneration == null
    ? directTinfoil || directPreview || directRelease : positive(evidence.attestationGeneration);
  if (!evidence || evidence.status.state !== result.status.state || evidence.modelId !== modelId
    || (["chutes-preview", "chutes"].includes(evidence.providerId) && !directPreview && !directRelease)
    || (degraded && !directPreview && (evidence.providerId !== "near" || evidence.e2eeProtocol !== "near-v3"
      || evidence.attestationProtocol !== "near-tdx-nvidia-v2"
      || !Array.isArray(evidence.checks)
      || !evidence.checks.some((check) => check.name === "Intel TDX" && check.detail === "OutOfDate" && check.passed === false)))
    || !text(evidence.providerId) || !text(evidence.modelId)
    || !text(evidence.attestationProtocol) || !text(evidence.e2eeProtocol) || !text(evidence.trustPolicyVersion)
    || !validGeneration || !positive(evidence.e2eeEncryptionVersion)
    || !positive(evidence.verifiedAtUnixSeconds) || !positive(evidence.hardExpiresAtUnixSeconds)
    || evidence.hardExpiresAtUnixSeconds <= evidence.verifiedAtUnixSeconds
    || evidence.verifiedAtUnixSeconds > Math.floor(Date.now() / 1000) + 60
    || !fingerprint(evidence.modelKeyFingerprint)
    || (evidence.tlsSpkiFingerprint != null && !fingerprint(evidence.tlsSpkiFingerprint))
    || !Array.isArray(evidence.checks) || evidence.checks.length === 0 || evidence.checks.length > 128
    || evidence.checks.some((check) => !text(check.name) || !text(check.detail, 8192)
      || (check.passed !== true && !(!directPreview && degraded && check.name === "Intel TDX" && check.detail === "OutOfDate" && check.passed === false)
        && !(directPreview && check.name === "Application provenance" && check.detail === "unqualified development preview" && check.passed === false)))
    || !Array.isArray(evidence.providerClaims) || evidence.providerClaims.length > 128
    || evidence.providerClaims.some((claim) => !text(claim.name) || typeof claim.value !== "string" || claim.value.length > 8192)
    || (evidence.workloadManifest != null && (typeof evidence.workloadManifest !== "string"
      || new TextEncoder().encode(evidence.workloadManifest).byteLength > MAX_WORKLOAD_MANIFEST_BYTES))) {
    throw new ProtocolError("AxiomCLI returned an incomplete or mismatched verification report");
  }
  return {
    status: { state: evidence.status.state },
    providerId: evidence.providerId,
    modelId: evidence.modelId,
    attestationProtocol: evidence.attestationProtocol,
    e2eeProtocol: evidence.e2eeProtocol,
    e2eeEncryptionVersion: evidence.e2eeEncryptionVersion,
    trustPolicyVersion: evidence.trustPolicyVersion,
    verifiedAtUnixSeconds: evidence.verifiedAtUnixSeconds,
    hardExpiresAtUnixSeconds: evidence.hardExpiresAtUnixSeconds,
    attestationGeneration: evidence.attestationGeneration,
    modelKeyFingerprint: evidence.modelKeyFingerprint,
    tlsSpkiFingerprint: evidence.tlsSpkiFingerprint ?? null,
    checks: evidence.checks.map(({ name, passed, detail }) => ({ name, passed, detail })),
    providerClaims: evidence.providerClaims.map(({ name, value }) => ({ name, value })),
    workloadManifest: evidence.workloadManifest ?? null,
  };
}
