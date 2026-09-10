import type { ClientSessionState, SecurityEvidence } from "@axiom/axiom-acp-client";

export type ProofState = "verified" | "verifying" | "failed" | "expired" | "unavailable" | "unverified" | "development";

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
  const phala = evidence.attestationProtocol === "phala-aci-tdx-v1";
  if (!phala && evidence.attestationProtocol !== "near-tdx-nvidia-v1") return [];
  return [
    {
      title: "Hardware evidence", explanation: phala
        ? "This device checked the gateway’s hardware proof and the eligible workers’ CPU and GPU evidence."
        : "This device checked the model environment’s Intel CPU and NVIDIA GPU evidence.",
      names: phala ? ["Gateway TDX", "Worker TEE", "Worker GPU"] : ["Intel TDX", "NVIDIA GPU"],
    },
    {
      title: "Encryption key", explanation: "The encryption key is bound to the verified environment before model messages are sent.",
      names: phala ? ["Encrypted channel"] : ["Model encryption key", "Model identity", "Live TLS endpoint"],
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

/** Receipt state comes exclusively from the durable reply, never the TEE report. */
export function replyProof(session: ClientSessionState): { label: string; verified: boolean; detail: string } {
  if (session.running) return { label: "Pending", verified: false, detail: "The reply stays provisional until its complete response and final receipts verify." };
  const last = [...session.timeline].reverse().find((item) => item.kind === "assistant");
  if (!last) return { label: "No reply yet", verified: false, detail: "Each reply needs its own final receipt. A TEE check alone does not verify a reply." };
  const parts = last.turnId ? session.timeline.filter((item) => item.turnId === last.turnId && ["assistant", "reasoning"].includes(item.kind)) : [last];
  const verified = parts.every((item) => item.terminalVerified === true && item.status === "completed");
  return {
    label: verified ? "Verified" : last.status === "failed" ? "Failed" : "Incomplete",
    verified,
    detail: verified ? "The native client verified the complete reply’s final receipts, including the accepted worker and response binding."
      : "No verified completion is recorded for this reply. Its text is not proof of a successful receipt check.",
  };
}
