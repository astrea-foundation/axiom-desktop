import type { ClientSessionState, SecurityEvidence } from "@axiom/chat-core/viewTypes";

export const PROOF_IDLE_AFTER_MS = 30 * 60_000;
export const PROOF_REFRESH_RETRY_MS = 30_000;
export const PROOF_REFRESH_MAX_RETRY_MS = 5 * 60_000;

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

/** Renew the selected environment report shortly before its lease expires. */
export function proofRefreshDue(session: ClientSessionState, modelId: string, now: number): boolean {
  const evidence = session.securityEvidence;
  if (!evidence || evidence.modelId !== modelId) return true;
  const expires = evidence.hardExpiresAtUnixSeconds * 1000;
  if (!Number.isFinite(expires)) return true;
  const lifetime = expires - evidence.verifiedAtUnixSeconds * 1000;
  if (!Number.isFinite(lifetime) || lifetime <= 0) return true;
  const margin = Math.min(30_000, Math.max(1000, lifetime / 2));
  return now >= expires - margin;
}

export function verificationAge(at: number, now: number): string {
  const seconds = Math.max(0, Math.floor(now / 1000 - at));
  if (seconds < 5) return "just now";
  if (seconds < 60) return `${seconds}s ago`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`;
  if (seconds < 86400) return `${Math.floor(seconds / 3600)}h ago`;
  return `${Math.floor(seconds / 86400)}d ago`;
}

export function proofGroups(evidence: SecurityEvidence) {
  if (evidence.attestationProtocol === "axiom-gateway-v2") {
    return [
      { title: "Gateway hardware", explanation: "Your browser locally verified the confidential gateway’s hardware evidence.", names: ["Gateway hardware attestation"] },
      { title: "Gateway code", explanation: "The measured workload matches the inspected source and verified build provenance.", names: ["Gateway workload provenance"] },
      { title: "Gateway key and freshness", explanation: "A fresh challenge binds this gateway’s encryption key and origin to its verified workload.", names: ["Gateway challenge and encryption key"] },
    ].map(group => ({ ...group, checks: evidence.checks.filter(check => group.names.includes(check.name)),
      passed: group.names.every(name => evidence.checks.some(check => check.name === name && check.passed)) }));
  }
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
      title: "Freshness", explanation: "A fresh challenge prevents replayed evidence. This report must stay current before new model messages are sent.",
      names: ["Fresh challenge"],
    },
  ].map((group) => ({ ...group,
    checks: evidence.checks.filter((check) => group.names.includes(check.name)),
    passed: group.names.every((name) => evidence.checks.some((check) => check.name === name && check.passed)),
  }));
}

/** The composer describes environment attestation, independently of reply/tool history. */
export function proofBadge(session: ClientSessionState, reportState: ProofState): { label: string; tone: "success" | "danger" | "warning" | "muted" } {
  if (reportState === "failed") return { label: proofLabels.failed, tone: "danger" };
  if (outdatedTeeState(session, session.settings?.model ?? session.securityEvidence?.modelId ?? "") === "degraded") return { label: proofLabels.degraded, tone: "warning" };
  if (reportState === "outdated") return { label: proofLabels.outdated, tone: "warning" };
  return { label: proofLabels[reportState], tone: reportState === "verified" ? "success"
    : reportState === "expired" || reportState === "development" ? "warning" : "muted" };
}
