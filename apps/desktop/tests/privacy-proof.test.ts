import assert from "node:assert/strict";
import test from "node:test";
import type { ClientSessionState } from "@axiom/axiom-acp-client";
import { outdatedTeeState, proofBadge, proofGroups, proofRefreshDue, proofState } from "../src/renderer/src/privacyProof";
import { securityEvidence } from "../../../fixtures/desktop-security.mts";
const session = (): ClientSessionState => ({
  sessionId: "thread", title: null, cwd: "", settings: null, security: { state: "verified" },
  securityEvidence: securityEvidence(), timeline: [], modes: [], currentModeId: null,
  configOptions: [], interactions: [], running: false, needsResync: false, threadRevision: 0, lastTimelineSequence: 0,
});

test("signed Chutes releases show TEE verification with separate application provenance scope", () => {
  const current = session();
  const evidence = current.securityEvidence!;
  evidence.attestationProtocol = "chutes-tdx-nvidia-release-v1";
  evidence.checks = ["Intel TDX", "NVIDIA GPU", "Chutes encryption key and fresh challenge", "Repository release authorization"]
    .map(name => ({ name, passed: true, detail: "verified" }));
  current.settings = { model: "chutes-preview:qwen3.8-27b", thinkingLevel: "medium", permissionProfile: "chat" };
  assert.equal(proofState(current, "test-model", Date.now()), "verified");
  assert.deepEqual(proofBadge(current, "verified"), { label: "TEE verified", tone: "success" });
  assert.deepEqual(proofGroups(evidence).map(group => group.passed), [true, true, true]);
  assert.ok(proofGroups(evidence).every(group => !/running application matches/i.test(group.explanation)));
  assert.equal(proofState(current, "test-model", evidence.hardExpiresAtUnixSeconds * 1000), "expired");
});

test("Chutes hardware preview stays visibly development-only after an authenticated reply", () => {
  const current = session();
  current.security = { state: "degraded" };
  current.securityEvidence!.status.state = "degraded";
  current.securityEvidence!.attestationProtocol = "chutes-hardware-development-v1";
  current.securityEvidence!.checks = [
    { name: "Intel TDX", detail: "UpToDate", passed: true },
    { name: "NVIDIA GPU", detail: "verified", passed: true },
    { name: "Chutes encryption key and fresh challenge", detail: "quote-bound", passed: true },
    { name: "Application provenance", detail: "unqualified development preview", passed: false },
  ];
  current.timeline = [{ id: "answer", turnId: "turn", kind: "assistant", text: "Authenticated preview", status: "completed", terminalVerified: true }];
  assert.equal(proofState(current, "test-model", Date.now()), "development");
  assert.equal(outdatedTeeState(current, "test-model"), undefined);
  assert.deepEqual(proofBadge(current, "development"), { label: "Chutes dev preview", tone: "warning" });
  assert.deepEqual(proofGroups(current.securityEvidence!).map(group => group.passed), [true, true, false]);
  assert.equal(proofState(current, "other-model", Date.now()), "unavailable");
  assert.equal(proofState(current, "test-model", current.securityEvidence!.hardExpiresAtUnixSeconds * 1000), "expired");
  current.securityVerificationError = "Hardware unavailable";
  assert.equal(proofState(current, "test-model", Date.now()), "unavailable");
});

test("preview admission without an inspected report never shows the NEAR consent warning", () => {
  const current = session();
  current.security = { state: "degraded" };
  current.securityEvidence = null;
  current.settings = { model: "chutes-preview:qwen3.8-27b", thinkingLevel: "medium", permissionProfile: "chat" };
  assert.equal(outdatedTeeState(current, current.settings.model), undefined);
  assert.equal(proofState(current, current.settings.model, Date.now()), "development");
  assert.deepEqual(proofBadge(current, "development"), { label: "Chutes dev preview", tone: "warning" });
  current.security = { state: "unverified" };
  current.timeline = [{ id: "answer", turnId: "turn", kind: "assistant", text: "Cached preview reply", status: "completed", terminalVerified: true }];
  assert.deepEqual(proofBadge(current, "unverified"), { label: "Chutes dev preview", tone: "warning" });
  current.timeline.push({ id: "error", turnId: "turn", kind: "error", text: "Request failed", status: "failed" });
  assert.equal(proofBadge(current, "failed").tone, "danger");
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

test("successful image and view steps cannot turn an environment badge into an incomplete reply", () => {
  const current = session();
  current.timeline = [
    { id: "question", turnId: "turn", kind: "user", text: "Make a landscape", status: "completed" },
    { id: "thought", turnId: "turn", kind: "reasoning", text: "Prepare the image", status: "completed" },
    { id: "generate", turnId: "turn", kind: "tool", text: "Generated image", status: "completed" },
    { id: "view", turnId: "turn", kind: "tool", text: "Viewed image", status: "completed" },
    { id: "thought-2", turnId: "turn", kind: "reasoning", text: "Inspect the result", status: "completed" },
    { id: "answer", turnId: "turn", kind: "assistant", text: "Done.", status: "completed", terminalVerified: true },
  ];
  assert.deepEqual(proofBadge(current, proofState(current, "test-model", Date.now())),
    { label: "TEE verified", tone: "success" });
});

test("reply and tool history never determine environment attestation", () => {
  const current = session();
  for (const status of ["in_progress", "completed", "cancelled", "interrupted", "failed"]) {
    current.timeline = [
      { id: "answer", turnId: "turn", kind: "assistant", text: "Output", status },
      { id: "tool", turnId: "turn", kind: "tool", text: "Tool result", status },
      { id: "error", turnId: "turn", kind: "error", text: "Request error", status: "failed" },
    ];
    for (const running of [true, false]) {
      current.running = running;
      assert.deepEqual(proofBadge(current, "verified"), { label: "TEE verified", tone: "success" });
    }
  }
  current.security = { state: "failed" };
  assert.deepEqual(proofBadge(current, "failed"), { label: "TEE check failed", tone: "danger" });
});

test("completed replies cannot hide an expired or failed environment report", () => {
  const current = session();
  current.timeline = [{ id: "answer", kind: "assistant", text: "Done", status: "completed", terminalVerified: true }];
  const expiry = current.securityEvidence!.hardExpiresAtUnixSeconds * 1000;
  assert.deepEqual(proofBadge(current, proofState(current, "test-model", expiry)),
    { label: "Report needs refresh", tone: "warning" });
  current.securityVerificationPending = true;
  assert.deepEqual(proofBadge(current, proofState(current, "test-model", expiry)),
    { label: "Verifying TEE", tone: "muted" });
  current.securityVerificationPending = false;
  current.security = { state: "failed" };
  assert.deepEqual(proofBadge(current, "failed"), { label: "TEE check failed", tone: "danger" });
});

test("rate limits and reply bookkeeping do not change the environment badge", () => {
  const current = session();
  current.requestUsage = [{ requestId: "request", modelId: "test-model", providerId: "test", turnId: "turn",
    purpose: "conversation", state: "failed", errorCode: "RATE_LIMITED", responseVerified: false }];
  current.timeline = [{ id: "error", turnId: "turn", kind: "error", text: "Rate limited", status: "failed" }];
  assert.equal(proofBadge(current, "verified").label, "TEE verified");
  current.needsResync = true;
  assert.equal(proofBadge(current, proofState(current, "test-model", Date.now())).label, "Proof unavailable");
});

test("report explanations cover the recipient environment without reply verdicts", () => {
  const report = securityEvidence();
  for (const protocol of ["near-tdx-nvidia-v1", "near-tdx-nvidia-v2", "tinfoil-snp-sigstore-v1",
    "chutes-tdx-nvidia-release-v1", "chutes-hardware-development-v1", "axiom-gateway-v2"]) {
    report.attestationProtocol = protocol;
    for (const group of proofGroups(report)) {
      assert.doesNotMatch(group.explanation, /repl(?:y|ies)|stream order|completion/i);
    }
  }
});

test("selected reports renew before expiry without extending their lease", () => {
  const current = session();
  const evidence = current.securityEvidence!;
  const expiry = evidence.hardExpiresAtUnixSeconds * 1000;
  assert.equal(proofRefreshDue(current, "test-model", expiry - 30_001), false);
  assert.equal(proofRefreshDue(current, "test-model", expiry - 30_000), true);
  assert.equal(proofRefreshDue(current, "test-model", expiry), true);
  assert.equal(proofState(current, "test-model", expiry), "expired");
  assert.equal(proofRefreshDue(current, "other-model", Date.now()), true);
  current.securityEvidence = null;
  assert.equal(proofRefreshDue(current, "test-model", Date.now()), true);
});

test("short leases renew proportionally and malformed leases require a fresh report", () => {
  const current = session();
  const evidence = current.securityEvidence!;
  evidence.verifiedAtUnixSeconds = 100;
  evidence.hardExpiresAtUnixSeconds = 110;
  assert.equal(proofRefreshDue(current, "test-model", 104_999), false);
  assert.equal(proofRefreshDue(current, "test-model", 105_000), true);
  evidence.verifiedAtUnixSeconds = Number.NaN;
  assert.equal(proofRefreshDue(current, "test-model", 100_000), true);
  evidence.hardExpiresAtUnixSeconds = Number.NaN;
  assert.equal(proofRefreshDue(current, "test-model", 100_000), true);
});
