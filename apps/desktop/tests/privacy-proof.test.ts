import assert from "node:assert/strict";
import test from "node:test";
import type { ClientSessionState } from "@axiom/axiom-acp-client";
import { proofGroups, proofState, replyProof } from "../src/renderer/src/privacyProof";
import { securityEvidence } from "../../../fixtures/desktop-security.mts";
const session = (): ClientSessionState => ({
  sessionId: "thread", title: null, cwd: "", settings: null, security: { state: "verified" },
  securityEvidence: securityEvidence(), timeline: [], modes: [], currentModeId: null,
  configOptions: [], interactions: [], running: false, needsResync: false, threadRevision: 0, lastTimelineSequence: 0,
});

test("badge verification requires a matching, unexpired report and no pending/error state", () => {
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

test("TEE checks and Phala's required-receipt policy cannot verify an incomplete reply", () => {
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
