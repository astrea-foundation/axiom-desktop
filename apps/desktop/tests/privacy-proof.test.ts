import assert from "node:assert/strict";
import test from "node:test";
import type { ClientSessionState } from "@axiom/axiom-acp-client";
import { outdatedTeeState, proofBadge, proofGroups, proofState, replyProof } from "../src/renderer/src/privacyProof";
import { securityEvidence } from "../../../fixtures/desktop-security.mts";
const session = (): ClientSessionState => ({
  sessionId: "thread", title: null, cwd: "", settings: null, security: { state: "verified" },
  securityEvidence: securityEvidence(), timeline: [], modes: [], currentModeId: null,
  configOptions: [], interactions: [], running: false, needsResync: false, threadRevision: 0, lastTimelineSequence: 0,
});

test("outdated and accepted degraded environments stay yellow even with an authenticated reply", () => {
  const current = session();
  current.security = { state: "outdated" };
  assert.equal(proofState(current, "test-model", Date.now()), "outdated");
  assert.equal(proofBadge(current, "outdated").tone, "warning");
  current.security = { state: "degraded" };
  current.securityEvidence!.status.state = "degraded";
  current.timeline = [{ id: "answer", turnId: "turn", kind: "assistant", text: "Authenticated answer", status: "completed", terminalVerified: true }];
  assert.equal(proofState(current, "test-model", Date.now()), "degraded");
  assert.deepEqual(proofBadge(current, "degraded"), { label: "TEE updates needed", tone: "warning" });
  assert.equal(proofState(current, "test-model", current.securityEvidence!.hardExpiresAtUnixSeconds * 1000), "expired");
  current.security = { state: "verifying" };
  assert.equal(outdatedTeeState(current, "test-model"), "degraded");
  assert.equal(outdatedTeeState(current, "another-model"), undefined);
  assert.equal(proofBadge(current, "verifying").tone, "warning");
  current.security = { state: "failed" };
  assert.equal(outdatedTeeState(current, "test-model"), undefined);
});

test("report verification requires a matching, unexpired report and no pending/error state", () => {
  const current = session();
  assert.equal(proofState(current, "test-model", Date.now()), "verified");
  assert.equal(proofState(current, "other-model", Date.now()), "unavailable");
  assert.equal(proofState({ ...current, securityEvidence: null }, "test-model", Date.now()), "unavailable");
  assert.equal(proofState(current, "test-model", current.securityEvidence!.hardExpiresAtUnixSeconds * 1000), "expired");
  assert.equal(proofState({ ...current, security: { state: "failed" } }, "test-model", Date.now()), "failed");
  assert.equal(proofState({ ...current, securityVerificationPending: true }, "test-model", Date.now()), "verifying");
  assert.equal(proofState({ ...current, securityVerificationError: "Unavailable" }, "test-model", Date.now()), "unavailable");
});

test("a hardware group cannot pass with a missing GPU check", () => {
  const report = securityEvidence();
  assert.ok(proofGroups(report).every((group) => group.passed));
  report.checks = report.checks.filter((check) => check.name !== "NVIDIA GPU");
  assert.equal(proofGroups(report)[0]!.passed, false);
});

test("NEAR worker sessions display the locally attested TLS key", () => {
  const report = securityEvidence();
  report.attestationProtocol = "near-tdx-nvidia-v2";
  report.checks = report.checks.map((check) => check.name === "Live TLS endpoint"
    ? { ...check, name: "Attested worker TLS key", detail: "quote-bound" } : check);
  assert.equal(proofGroups(report).length, 3);
  assert.ok(proofGroups(report).every((group) => group.passed));
  assert.ok(!proofGroups(report).flatMap((group) => group.names).includes("Live TLS endpoint"));
});

test("Tinfoil proof names the router and cannot claim independent GPU verification", () => {
  const report = securityEvidence();
  report.attestationProtocol = "tinfoil-snp-sigstore-v1";
  report.checks = ["AMD SEV-SNP router attestation", "Measured router matches Sigstore build provenance", "Live TLS identity and attested HPKE key"]
    .map((name) => ({ name, detail: "verified", passed: true }));
  const groups = proofGroups(report);
  assert.equal(groups.length, 3);
  assert.ok(groups.every((group) => group.passed));
  assert.match(groups[0]!.explanation, /router verifies its model workers/);
  report.checks.pop();
  assert.equal(proofGroups(report)[2]!.passed, false);
});

test("TEE checks and NEAR's required-receipt policy cannot verify an incomplete reply", () => {
  const current = session();
  current.securityEvidence!.checks.push({ name: "Signed receipt", detail: "required", passed: true });
  assert.equal(replyProof(current).label, "No reply yet");
  current.timeline = [{ id: "answer", turnId: "turn", kind: "assistant", text: "Synthetic partial reply", status: "completed" }];
  assert.equal(replyProof(current).label, "Incomplete");
  current.timeline[0]!.terminalVerified = true;
  assert.equal(replyProof(current).label, "Verified");
  current.running = true;
  assert.equal(replyProof(current).label, "Pending");
  current.running = false;
  current.timeline.push({ id: "answer-2", turnId: "turn", kind: "assistant", text: "Synthetic failed segment", status: "failed" });
  assert.equal(replyProof(current).label, "Failed");
});

test("a running reply stays provisional when the cached report expires", () => {
  const current = session();
  current.running = true;
  const expiry = current.securityEvidence!.hardExpiresAtUnixSeconds * 1000;
  for (const now of [expiry - 1, expiry, expiry + 60_000]) {
    const badge = proofBadge(current, proofState(current, "test-model", now));
    assert.deepEqual(badge, { label: "Verifying reply", tone: "success" });
    assert.equal(replyProof(current).verified, false);
  }
  assert.equal(proofState(current, "test-model", expiry), "expired", "the cached report itself stays expired");
  current.security = { state: "failed" };
  assert.equal(proofBadge(current, "failed").label, "Reply check failed");
});

test("only durable completion verifies a reply, independently of report renewal", () => {
  const current = session();
  current.timeline = [{ id: "answer", turnId: "turn", kind: "assistant", text: "Reply", status: "completed" }];
  const expiry = current.securityEvidence!.hardExpiresAtUnixSeconds * 1000;
  assert.equal(proofBadge(current, "verified").label, "Reply incomplete", "a fresh report cannot verify a reply");
  current.timeline[0]!.terminalVerified = true;
  assert.equal(proofBadge(current, proofState(current, "test-model", expiry)).label, "Reply verified");
  current.securityVerificationPending = true;
  assert.equal(proofBadge(current, proofState(current, "test-model", expiry)).label, "Reply verified");
  current.securityVerificationPending = false;
  current.security = { state: "failed" };
  assert.equal(proofBadge(current, "failed").label, "Reply verified", "a later report failure does not revoke a verified reply");
  current.timeline[0]!.status = "cancelled";
  assert.equal(proofBadge(current, "verified").label, "Reply incomplete");
  current.timeline[0]!.status = "failed";
  assert.equal(proofBadge(current, "verified").label, "Reply check failed");
});

test("a newer empty or failed turn cannot inherit an older reply's verification", () => {
  const current = session();
  current.timeline = [
    { id: "old", turnId: "old-turn", kind: "assistant", text: "Verified", status: "completed", terminalVerified: true },
    { id: "question", turnId: "new-turn", kind: "user", text: "Next", status: "completed" },
  ];
  assert.equal(replyProof(current).verified, false);
  current.timeline.push({ id: "reasoning", turnId: "new-turn", kind: "reasoning", text: "Partial", status: "in_progress" });
  assert.equal(replyProof(current).label, "Incomplete");
  current.timeline.pop();
  current.timeline.push({ id: "error", turnId: "new-turn", kind: "error", text: "Verification failed", status: "failed" });
  current.security = { state: "failed" };
  assert.equal(replyProof(current).label, "Failed");
});

test("a rate-limited request without output is not a failed reply authentication", () => {
  const current = session();
  current.requestUsage = [{ requestId: "request", modelId: "test-model", providerId: "test", turnId: "new",
    purpose: "conversation", state: "failed", errorCode: "RATE_LIMITED", responseVerified: false }];
  current.timeline = [
    { id: "old", turnId: "old", kind: "assistant", text: "Earlier verified reply", status: "completed", terminalVerified: true },
    { id: "question", turnId: "new", kind: "user", text: "Next", status: "completed" },
    { id: "error", turnId: "new", kind: "error", text: "Axiom request was rate limited", status: "failed" },
  ];
  assert.equal(replyProof(current).label, "No reply received");
  assert.equal(replyProof(current).verified, false, "an earlier reply or TEE report cannot verify this request");
  assert.deepEqual(proofBadge(current, "verified"), { label: "No reply received", tone: "warning" });
  current.timeline.splice(2, 0, { id: "partial", turnId: "new", kind: "reasoning", text: "Partial", status: "failed" });
  assert.equal(proofBadge(current, "verified").label, "Reply check failed", "received output still requires authentication");
  current.timeline.splice(2, 1);
  current.security = { state: "failed" };
  assert.equal(proofBadge(current, "failed").label, "Reply check failed", "a failed TEE check must remain a security failure");
});

test("empty cryptographic, unknown and mixed failures retain failed reply verification", () => {
  for (const usage of [
    [],
    [{ errorCode: "INVALID_RESPONSE" }],
    [{ errorCode: "RATE_LIMITED", purpose: "title" as const }],
    [{ errorCode: "RATE_LIMITED", turnId: "old" }],
    [{ errorCode: "RATE_LIMITED", responseVerified: true }],
    [{ errorCode: "RATE_LIMITED" }, { errorCode: "INVALID_RESPONSE" }],
  ]) {
    const current = session();
    current.timeline = [
      { id: "question", turnId: "new", kind: "user", text: "Next", status: "completed" },
      { id: "error", turnId: "new", kind: "error", text: "Request failed", status: "failed" },
    ];
    current.requestUsage = usage.map((request, index) => ({ requestId: `request-${index}`, modelId: "test-model",
      providerId: "test", turnId: "new", purpose: "conversation" as const, state: "failed" as const, ...request }));
    assert.equal(proofBadge(current, "verified").label, "Reply check failed");
    assert.equal(replyProof(current).verified, false);
  }
});

test("all reply segments need verification while handled tool failures remain separate", () => {
  const current = session();
  current.timeline = [
    { id: "reasoning", turnId: "turn", kind: "reasoning", text: "Thought", status: "completed" },
    { id: "tool", turnId: "turn", kind: "tool", text: "No match", status: "failed" },
    { id: "answer", turnId: "turn", kind: "assistant", text: "Reply", status: "completed", terminalVerified: true },
  ];
  assert.equal(replyProof(current).label, "Incomplete");
  current.timeline[0]!.terminalVerified = true;
  assert.equal(replyProof(current).label, "Verified");
  current.needsResync = true;
  assert.equal(proofBadge(current, "unavailable").label, "Reply unavailable");
});

test("legacy replies without turn IDs are scoped to the latest user message", () => {
  const current = session();
  current.timeline = [
    { id: "old", kind: "assistant", text: "Old partial", status: "failed" },
    { id: "question", kind: "user", text: "Next", status: "completed" },
    { id: "answer", kind: "assistant", text: "Reply", status: "completed", terminalVerified: true },
  ];
  assert.equal(replyProof(current).label, "Verified");
  current.timeline.push({ id: "next", kind: "user", text: "Again", status: "completed" });
  assert.equal(replyProof(current).verified, false);
});
