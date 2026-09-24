import assert from "node:assert/strict";
import test, { type TestContext } from "node:test";
import { AxiomAcpClient, type VerifySecurityResponse } from "../src/index.js";
import { publicSecurityEvidence } from "../src/security-evidence.js";
import { securityEvidence } from "../../../fixtures/desktop-security.mjs";

function fixture(t: TestContext) {
  // Idle local process, no account/network/provider access. All RPCs are mocked.
  const client = new AxiomAcpClient({ command: process.execPath, args: ["-e", "process.stdin.resume()"] });
  t.after(() => client.close());
  client.state.setConnected(true);
  client.state.setAccount({ revision: 1, state: "valid", account: { id: "account-one", linkedMethods: [] } });
  client.state.addSession("thread", "");
  model(client, "test-model");
  return client;
}

test("degraded reports retain OutOfDate and reject every other failed check", () => {
  const evidence = securityEvidence();
  evidence.status.state = "degraded";
  evidence.providerId = "near";
  evidence.attestationProtocol = "near-tdx-nvidia-v2";
  evidence.e2eeProtocol = "near-v3";
  const intel = evidence.checks.find((check) => check.name === "Intel TDX")!;
  intel.detail = "OutOfDate"; intel.passed = false;
  const result: VerifySecurityResponse = { status: { state: "degraded" }, evidence };
  assert.equal(publicSecurityEvidence(result, "test-model")!.status.state, "degraded");
  for (const status of ["Revoked", "OutOfDateConfigurationNeeded", "unknown"]) {
    intel.detail = status;
    assert.throws(() => publicSecurityEvidence(result, "test-model"), /incomplete|mismatched/);
  }
  intel.detail = "OutOfDate";
  evidence.checks.find((check) => check.name === "NVIDIA GPU")!.passed = false;
  assert.throws(() => publicSecurityEvidence(result, "test-model"), /incomplete|mismatched/);
});

test("consent requires the selected model and is sent only by an explicit call", async (t) => {
  const client = fixture(t);
  const requests: unknown[] = [];
  t.mock.method(client.process, "request", async (_method: string, params: unknown) => { requests.push(params); return result(); });
  await assert.rejects(client.verifySecurity("thread", true, "another-model"), /selected model changed/);
  assert.equal(requests.length, 0);
  await client.verifySecurity("thread");
  await client.verifySecurity("thread", true, "test-model");
  assert.deepEqual(requests, [{ threadId: "thread" }, { threadId: "thread", acceptOutdatedTee: true, modelId: "test-model" }]);
});
function model(client: AxiomAcpClient, model: string) {
  client.state.setConfig("thread", [{ id: "model", name: "Model", currentValue: model, options: [] }]);
}
const result = (): VerifySecurityResponse => ({ status: { state: "verified" }, evidence: securityEvidence() });

function hold(t: Parameters<typeof fixture>[0], client: AxiomAcpClient) {
  let resolve!: (value: VerifySecurityResponse) => void;
  let reject!: (error: Error) => void;
  const gate = new Promise<VerifySecurityResponse>((yes, no) => { resolve = yes; reject = no; });
  const request = t.mock.method(client.process, "request", () => gate);
  return { resolve, reject, request };
}

test("automatic and manual verifications retain the public report and coalesce refreshes", async (t) => {
  const client = fixture(t);
  const pending = hold(t, client);
  const first = client.verifySecurity("thread");
  assert.equal(client.verifySecurity("thread"), first);
  assert.equal(client.getState().sessions.thread?.securityVerificationPending, true);
  await Promise.resolve();
  assert.equal(pending.request.mock.callCount(), 1);
  const response = result();
  response.evidence!.unexpectedSecret = "never-export-this";
  response.evidence!.checks[0]!.unexpectedField = "never-export-this";
  pending.resolve(response);
  const accepted = await first;
  assert.deepEqual(client.getState().sessions.thread?.securityEvidence, accepted.evidence);
  assert.equal(client.getState().sessions.thread?.securityVerificationPending, false);
  assert.doesNotMatch(JSON.stringify(accepted), /never-export-this|unexpected/);
});

for (const change of ["model", "account", "runtime", "disconnect", "remove"] as const) {
  test(`a delayed report cannot cross a ${change} change`, async (t) => {
    const client = fixture(t);
    const pending = hold(t, client);
    const verification = client.verifySecurity("thread");
    const consent = client.verifySecurity("thread", true, "test-model");
    await Promise.resolve();
    if (change === "model") { model(client, "other-model"); model(client, "test-model"); }
    if (change === "account") client.state.setAccount({ revision: 2, state: "valid", account: { id: "account-two", linkedMethods: [] } });
    if (change === "disconnect") client.state.setConnected(false);
    if (change === "remove") { client.state.removeSession("thread"); client.state.addSession("thread", ""); model(client, "test-model"); }
    if (change === "runtime") {
      for (const runtimeInstanceId of ["one", "two"]) client.state.extensionEvent({
        runtimeInstanceId, sequence: 1, occurredAt: new Date().toISOString(), sessionId: "thread",
        event: { kind: "security_changed", status: { state: "unverified" } },
      });
    }
    pending.resolve(result());
    await assert.rejects(verification, /context changed/);
    await assert.rejects(consent, /context changed/);
    assert.equal(pending.request.mock.callCount(), 1, "a delayed consent never dispatches after its context changes");
    assert.equal(client.getState().sessions.thread?.securityEvidence ?? null, null);
    assert.equal(client.getState().sessions.thread?.securityVerificationPending ?? false, false);
  });
}

test("a failed refresh leaves an inspectable previous report and a visible error", async (t) => {
  const client = fixture(t);
  client.state.setSecurityVerification("thread", { pending: false, evidence: securityEvidence() });
  const pending = hold(t, client);
  const verification = client.verifySecurity("thread");
  await Promise.resolve(); pending.reject(new Error("Provider unavailable"));
  await assert.rejects(verification, /unavailable/);
  const session = client.getState().sessions.thread!;
  assert.ok(session.securityEvidence);
  assert.equal(session.securityVerificationPending, false);
  assert.match(session.securityVerificationError!, /Couldn’t refresh/);
  client.state.setRunning("thread", true);
  await assert.rejects(client.verifySecurity("thread"), /current reply/);
});

test("invalid reports never become renderer evidence", () => {
  const valid = result();
  for (const change of [
    { evidence: null },
    { evidence: { ...valid.evidence, modelId: "other-model" } },
    { evidence: { ...valid.evidence, attestationGeneration: 0 } },
    { evidence: { ...valid.evidence, hardExpiresAtUnixSeconds: NaN } },
    { evidence: { ...valid.evidence, modelKeyFingerprint: "not-a-fingerprint" } },
    { evidence: { ...valid.evidence, verifiedAtUnixSeconds: Math.floor(Date.now() / 1000) + 120 } },
    { evidence: { ...valid.evidence, checks: [] } },
    { evidence: { ...valid.evidence, checks: [{ name: "Intel TDX", passed: false, detail: "rejected" }] } },
    { evidence: { ...valid.evidence, workloadManifest: "x".repeat(1024 * 1024 + 1) } },
  ]) assert.throws(() => publicSecurityEvidence({ ...valid, ...change } as VerifySecurityResponse, "test-model"));
  assert.equal(publicSecurityEvidence({ status: { state: "failed" }, evidence: valid.evidence }, "test-model"), null);
});

test("Tinfoil reports omit relay lease generations without weakening evidence checks", () => {
  const valid = result();
  Object.assign(valid.evidence!, {
    providerId: "tinfoil", attestationProtocol: "tinfoil-snp-sigstore-v1",
    e2eeProtocol: "tinfoil-ehbp-v1",
  });
  delete valid.evidence!.attestationGeneration;
  assert.equal(publicSecurityEvidence(valid, "test-model")!.attestationGeneration, undefined);
  for (const change of [
    { providerId: "near" }, { attestationProtocol: "other" }, { e2eeProtocol: "other" },
    { attestationGeneration: 0 }, { hardExpiresAtUnixSeconds: undefined },
    { modelKeyFingerprint: "invalid" },
  ]) assert.throws(() => publicSecurityEvidence({ ...valid, evidence: { ...valid.evidence!, ...change } } as VerifySecurityResponse, "test-model"));
});

test("workload manifests accept up to 1 MiB of UTF-8 including the truncation marker", () => {
  const valid = result();
  const limit = 1024 * 1024;
  for (const manifest of ["x".repeat(limit), "é".repeat(limit / 2), "x".repeat(limit - 3) + "…"]) {
    valid.evidence!.workloadManifest = manifest;
    assert.equal(publicSecurityEvidence(valid, "test-model")!.workloadManifest, manifest);
  }
  valid.evidence!.workloadManifest = "é".repeat(limit / 2) + "x";
  assert.throws(() => publicSecurityEvidence(valid, "test-model"), /incomplete or mismatched/);
});

test("composition warms once while pending and reuses the result until expiry", async (t) => {
  const client = fixture(t);
  const pending = hold(t, client);
  let now = Date.now();
  t.mock.method(Date, "now", () => now);
  const first = client.prewarmSecurity("test-model");
  for (let key = 0; key < 20; key++) assert.equal(client.prewarmSecurity("test-model"), first);
  await Promise.resolve();
  assert.equal(pending.request.mock.callCount(), 1);
  assert.deepEqual(pending.request.mock.calls[0]!.arguments.slice(0, 2), ["_axiom/security/prewarm", { modelId: "test-model" }]);
  const report = result();
  pending.resolve(report);
  await first;
  now = report.evidence!.hardExpiresAtUnixSeconds * 1000 - 1;
  assert.equal(client.prewarmSecurity("test-model"), first, "even the final valid second is reusable");
  now++;
  const next = result();
  t.mock.method(client.process, "request", async () => next);
  assert.notEqual(client.prewarmSecurity("test-model"), first);
  await client.prewarmSecurity("test-model");
  assert.equal(client.getState().sessions.thread?.security?.state, "verified");
});

test("typing failures back off without timers, and manual refresh remains available", async (t) => {
  const client = fixture(t);
  let now = Date.now();
  t.mock.method(Date, "now", () => now);
  const request = t.mock.method(client.process, "request", async () => { throw new Error("offline"); });
  for (const delay of [30_000, 60_000, 120_000, 240_000, 300_000]) {
    const failed = client.prewarmSecurity("test-model");
    await assert.rejects(failed, /offline/);
    const calls = request.mock.callCount();
    now += delay - 1;
    for (let key = 0; key < 10; key++) assert.equal(client.prewarmSecurity("test-model"), failed);
    assert.equal(request.mock.callCount(), calls);
    now++;
  }
  const reply = result();
  t.mock.method(client.process, "request", async () => reply);
  await client.verifySecurity("thread");
});

for (const change of ["account", "runtime", "disconnect"] as const) {
  test(`warmup cannot reuse or apply a result across a ${change} change`, async (t) => {
    const client = fixture(t);
    const pending = hold(t, client);
    const warmup = client.prewarmSecurity("test-model");
    await Promise.resolve();
    if (change === "account") client.state.setAccount({ revision: 2, state: "valid", account: { id: "account-two", linkedMethods: [] } });
    if (change === "disconnect") client.state.setConnected(false);
    if (change === "runtime") {
      for (const runtimeInstanceId of ["one", "two"]) client.state.extensionEvent({
        runtimeInstanceId, sequence: 1, occurredAt: new Date().toISOString(), sessionId: "thread",
        event: { kind: "security_changed", status: { state: "unverified" } },
      });
    }
    pending.resolve(result());
    await assert.rejects(warmup, /changed/);
    assert.equal(client.getState().sessions.thread?.securityEvidence ?? null, null);
    if (change === "account") {
      t.mock.method(client.process, "request", async () => result());
      assert.notEqual(client.prewarmSecurity("test-model"), warmup);
      await client.prewarmSecurity("test-model");
    }
  });
}

test("model warmup before thread creation sends no draft and stale model reports never overwrite a thread", async (t) => {
  const client = fixture(t);
  client.state.removeSession("thread");
  const pending = hold(t, client);
  const warmup = client.prewarmSecurity("test-model");
  await Promise.resolve();
  client.state.addSession("thread", "");
  model(client, "other-model");
  pending.resolve(result());
  await warmup;
  assert.deepEqual(pending.request.mock.calls[0]!.arguments[1], { modelId: "test-model" });
  assert.equal(client.getState().sessions.thread?.securityEvidence, null);
});

test("composing during explicit verification joins it without starting a warmup", async (t) => {
  const client = fixture(t);
  const pending = hold(t, client);
  const manual = client.verifySecurity("thread");
  assert.equal(client.prewarmSecurity("test-model"), manual);
  await Promise.resolve();
  assert.equal(pending.request.mock.callCount(), 1);
  pending.resolve(result());
  await manual;
});

for (const cancelled of [false, true]) {
  test(`Enter during warmup ${cancelled ? "can be cancelled without sending" : "waits and sends exactly once"}`, async (t) => {
    const client = fixture(t);
    let resolve!: (value: VerifySecurityResponse) => void;
    const gate = new Promise<VerifySecurityResponse>((yes) => { resolve = yes; });
    let sent = 0;
    t.mock.method(client.process, "request", async (method: string) => {
      if (method === "_axiom/security/prewarm") return gate;
      if (method === "session/prompt") { sent++; return { stopReason: "end_turn" }; }
      throw new Error("No durable fixture timeline");
    });
    const warmup = client.prewarmSecurity("test-model");
    const prompt = client.prompt("thread", "local draft", "client-message");
    await Promise.resolve();
    assert.equal(sent, 0);
    if (cancelled) await client.cancel("thread");
    resolve(result());
    await warmup;
    assert.equal((await prompt).stopReason, cancelled ? "cancelled" : "end_turn");
    assert.equal(sent, cancelled ? 0 : 1);
  });
}
