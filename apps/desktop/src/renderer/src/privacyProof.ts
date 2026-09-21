import type { ClientSessionState, SecurityEvidence } from "@axiom/axiom-acp-client";

export const PROOF_IDLE_AFTER_MS = 30 * 60_000;

export type ProofState = "idle" | "verified" | "verifying" | "failed" | "expired" | "unavailable" | "unverified" | "development";

export function proofState(session: ClientSessionState, modelId: string, now: number): ProofState {
  if (session.securityVerificationPending || session.security?.state === "verifying") return "verifying";
  if (session.security?.state === "failed") return "failed";
  if (session.security?.state === "unattested_development") return "development";
  if (session.needsResync) return "unavailable";
  if (session.securityVerificationError) return "unavailable";
  const evidence = session.securityEvidence;
  if (session.security?.state !== "verified") return "unverified";
  if (!evidence || evidence.modelId !== modelId || evidence.status.state !== "verified") return "unavailable";
  if (!Number.isFinite(evidence.hardExpiresAtUnixSeconds) || evidence.hardExpiresAtUnixSeconds * 1000 <= now) return "expired";
  return "verified";
}

export const proofLabels: Record<ProofState, string> = {
  idle: "Idle",
  verified: "TEE verified", verifying: "Verifying TEE", failed: "TEE check failed",
  expired: "TEE proof expired", unavailable: "Proof unavailable", unverified: "Verify TEE", development: "Unattested dev",
};

export function verificationAge(at: number, now: number): string {
  const seconds = Math.max(0, Math.floor(now / 1000 - at));
  if (seconds < 5) return "just now";
  if (seconds < 60) return `${seconds}s ago`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`;
  if (seconds < 86400) return `${Math.floor(seconds / 3600)}h ago`;
  return `${Math.floor(seconds / 86400)}d ago`;
}

export function proofGroups(evidence: SecurityEvidence) {
  if (evidence.attestationProtocol === "tinfoil-snp-sigstore-v1") {
    return [
      { title: "Hardware evidence", explanation: "This device verified the router’s AMD SEV-SNP evidence. The measured router verifies its model workers.", names: ["AMD SEV-SNP router attestation"] },
      { title: "Source provenance", explanation: "The router’s measured software matches its verified Sigstore build provenance.", names: ["Measured router matches Sigstore build provenance"] },
      { title: "Encryption key and freshness", explanation: "A fresh connection binds the live router identity and encryption key to the verified environment. The report expires after its bounded lease.", names: ["Live TLS identity and attested HPKE key"] },
    ].map((group) => ({ ...group,
      checks: evidence.checks.filter((check) => group.names.includes(check.name)),
      passed: group.names.every((name) => evidence.checks.some((check) => check.name === name && check.passed)),
    }));
  }
  const phala = evidence.attestationProtocol === "phala-aci-tdx-v1";
  const nearSession = evidence.attestationProtocol === "near-tdx-nvidia-v2";
  if (!phala && !nearSession && evidence.attestationProtocol !== "near-tdx-nvidia-v1") return [];
  return [
    {
      title: "Hardware evidence", explanation: phala
        ? "This device checked the gateway’s hardware proof and the eligible workers’ CPU and GPU evidence."
        : "This device checked the model environment’s Intel CPU and NVIDIA GPU evidence.",
      names: phala ? ["Gateway TDX", "Worker TEE", "Worker GPU"] : ["Intel TDX", "NVIDIA GPU"],
    },
    {
      title: "Encryption key", explanation: "The encryption key is bound to the verified environment before model messages are sent.",
      names: phala ? ["Encrypted channel"] : ["Model encryption key", "Model identity", nearSession ? "Attested worker TLS key" : "Live TLS endpoint"],
    },
    {
      title: "Freshness", explanation: "A fresh challenge prevents replayed evidence. The report is usable only until its expiry.",
      names: phala ? ["Encrypted channel"] : ["Fresh challenge"],
    },
  ].map((group) => ({ ...group,
    checks: evidence.checks.filter((check) => group.names.includes(check.name)),
    passed: group.names.every((name) => evidence.checks.some((check) => check.name === name && check.passed)),
  }));
}

/** Response authentication comes from the durable reply, never the TEE report alone. */
export function replyProof(session: ClientSessionState): { label: string; verified: boolean; detail: string } {
  if (session.running) return { label: "Pending", verified: false, detail: "The reply stays provisional until its complete response is authenticated." };
  const last = [...session.timeline].reverse().find((item) => item.kind === "assistant");
  if (!last) return { label: "No reply yet", verified: false, detail: "Each reply must be authenticated to its request and verified environment. A TEE check alone does not verify a reply." };
  const parts = last.turnId ? session.timeline.filter((item) => item.turnId === last.turnId && ["assistant", "reasoning"].includes(item.kind)) : [last];
  const verified = parts.every((item) => item.terminalVerified === true && item.status === "completed");
  return {
    label: verified ? "Verified" : last.status === "failed" ? "Failed" : "Incomplete",
    verified,
    detail: verified ? "The native client authenticated the complete reply to its request and verified environment using the provider’s encrypted protocol."
      : "No verified completion is recorded for this reply. Its text alone does not prove successful authentication.",
  };
}
