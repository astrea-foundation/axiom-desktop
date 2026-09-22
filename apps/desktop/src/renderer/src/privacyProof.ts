import type { ClientSessionState, SecurityEvidence } from "@axiom/axiom-acp-client";

export const PROOF_IDLE_AFTER_MS = 30 * 60_000;

export type ProofState = "idle" | "verified" | "degraded" | "outdated" | "verifying" | "failed" | "expired" | "unavailable" | "unverified" | "development";

export function outdatedTeeState(session: ClientSessionState | null, modelId: string): "outdated" | "degraded" | undefined {
  const state = session?.security?.state;
  if (state === "outdated" || state === "degraded") return state;
  if (state === "verifying" && session?.securityEvidence?.modelId === modelId
    && session.securityEvidence.status.state === "degraded") return "degraded";
  return undefined;
}

export function proofState(session: ClientSessionState, modelId: string, now: number): ProofState {
  if (session.securityVerificationPending || session.security?.state === "verifying") return "verifying";
  if (session.security?.state === "failed") return "failed";
  if (session.security?.state === "outdated") return "outdated";
  if (session.security?.state === "unattested_development") return "development";
  if (session.needsResync) return "unavailable";
  if (session.securityVerificationError) return "unavailable";
  const evidence = session.securityEvidence;
  if (session.security?.state !== "verified" && session.security?.state !== "degraded") return "unverified";
  if (!evidence || evidence.modelId !== modelId || evidence.status.state !== session.security.state) return "unavailable";
  if (!Number.isFinite(evidence.hardExpiresAtUnixSeconds) || evidence.hardExpiresAtUnixSeconds * 1000 <= now) return "expired";
  return session.security.state;
}

export const proofLabels: Record<ProofState, string> = {
  idle: "Idle",
  degraded: "TEE updates needed", outdated: "TEE updates required",
  verified: "TEE verified", verifying: "Verifying TEE", failed: "TEE check failed",
  expired: "Report needs refresh", unavailable: "Proof unavailable", unverified: "Verify TEE", development: "Unattested dev",
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
  const nearSession = evidence.attestationProtocol === "near-tdx-nvidia-v2";
  if (!nearSession && evidence.attestationProtocol !== "near-tdx-nvidia-v1") return [];
  return [
    {
      title: "Hardware evidence", explanation: "This device checked the model environment’s Intel CPU and NVIDIA GPU evidence.",
      names: ["Intel TDX", "NVIDIA GPU"],
    },
    {
      title: "Encryption key", explanation: "The encryption key is bound to the verified environment before model messages are sent.",
      names: ["Model encryption key", "Model identity", nearSession ? "Attested worker TLS key" : "Live TLS endpoint"],
    },
    {
      title: "Freshness", explanation: "A fresh challenge prevents replayed evidence. Expiry limits reuse of this report for new requests; each reply is authenticated separately.",
      names: ["Fresh challenge"],
    },
  ].map((group) => ({ ...group,
    checks: evidence.checks.filter((check) => group.names.includes(check.name)),
    passed: group.names.every((name) => evidence.checks.some((check) => check.name === name && check.passed)),
  }));
}

type ReplyProofState = "none" | "pending" | "verified" | "failed" | "incomplete" | "unavailable";
type ReplyProof = { state: ReplyProofState; label: string; verified: boolean; detail: string };

/** Response authentication comes from the durable reply, never the cached report or clock. */
export function replyProof(session: ClientSessionState): ReplyProof {
  if (session.needsResync) return { state: "unavailable", label: "Unavailable", verified: false, detail: "Reply verification will be available after the thread reconnects and synchronizes." };
  if (session.running) {
    if (session.security?.state === "failed") return { state: "failed", label: "Failed", verified: false, detail: "Verification failed for the current request. Its output is not a verified reply." };
    return { state: "pending", label: "Pending", verified: false, detail: "The reply stays provisional until its complete response is authenticated. Expiry of the cached report does not decide this reply’s verification." };
  }
  const last = [...session.timeline].reverse().find((item) => ["user", "assistant", "reasoning", "error"].includes(item.kind));
  // A new user turn (including one that failed before producing text) must not
  // inherit a previous reply's verification. Legacy items may omit turn IDs.
  let start = session.timeline.length - 1;
  while (start >= 0 && session.timeline[start]?.kind !== "user") start--;
  const turn = last?.turnId ? session.timeline.filter((item) => item.turnId === last.turnId) : session.timeline.slice(start + 1);
  const parts = turn.filter((item) => ["assistant", "reasoning"].includes(item.kind));
  if (turn.some((item) => item.kind === "error") || parts.some((item) => item.status === "failed")) return { state: "failed", label: "Failed", verified: false, detail: "No verified completion is recorded for this reply. Its text alone does not prove successful authentication." };
  if (!parts.length) return { state: "none", label: "No reply yet", verified: false, detail: "Each reply must be authenticated to its request and verified environment. A TEE check alone does not verify a reply." };
  const verified = parts.every((item) => item.terminalVerified === true && item.status === "completed");
  return {
    state: verified ? "verified" : "incomplete",
    label: verified ? "Verified" : "Incomplete",
    verified,
    detail: verified ? "The native client authenticated the complete reply to its request and verified environment using the provider’s encrypted protocol."
      : "No verified completion is recorded for this reply. Its text alone does not prove successful authentication.",
  };
}

/** The composer summarizes the reply; report freshness remains independently inspectable. */
export function proofBadge(session: ClientSessionState, reportState: ProofState): { label: string; tone: "success" | "danger" | "warning" | "muted" } {
  if (outdatedTeeState(session, session.settings?.model ?? session.securityEvidence?.modelId ?? "") === "degraded") return { label: proofLabels.degraded, tone: "warning" };
  if (reportState === "outdated") return { label: proofLabels.outdated, tone: "warning" };
  const reply = replyProof(session);
  if (reportState !== "idle" && reportState !== "development") {
    switch (reply.state) {
      case "pending": return { label: "Verifying reply", tone: "success" };
      case "verified": return { label: "Reply verified", tone: "success" };
      case "failed": return { label: "Reply check failed", tone: "danger" };
      case "incomplete": return { label: "Reply incomplete", tone: "warning" };
      case "unavailable": return { label: "Reply unavailable", tone: "muted" };
      case "none": break;
    }
  }
  return { label: proofLabels[reportState], tone: reportState === "verified" ? "success" : reportState === "failed" ? "danger"
    : reportState === "expired" || reportState === "development" ? "warning" : "muted" };
}
