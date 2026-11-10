import assert from "node:assert/strict";
import test, { type TestContext } from "node:test";
import { AxiomAcpClient } from "../src/client.js";
import type { ClientState } from "../src/state.js";
import type { InitializeResult } from "../src/types.js";

const runtimeId = "startup-runtime";

function fixture(t: TestContext, options: { identity?: unknown } = { identity: runtimeId }) {
  // Exercise the real client/state boundary without credentials, a Rust build,
  // provider traffic, or access to any user's conversation storage.
  const client = new AxiomAcpClient({ command: process.execPath, args: ["-e", "process.stdin.resume()"] });
  t.after(() => client.close());
  const response: InitializeResult = {
    protocolVersion: 1,
    agentCapabilities: { _meta: { axiom: {
      protocolVersion: "0.2", runtimeInstanceId: options.identity,
      features: {
        desktopChat: 1, threadCatalog: 1, timeline: 2, modelCatalog: 1,
        profilePreferences: 1, collections: 1, account: 2, billing: 4,
        securityEvidence: 4, webConsent: 1,
      },
    } } },
  };
  t.mock.method(client.process, "request", async () => response);
  return client;
}

function event(identity = runtimeId, sequence = 1) {
  return {
    runtimeInstanceId: identity, sequence, occurredAt: "2026-10-20T00:00:00Z",
    event: { kind: "profile_preferences_changed" as const, preferences: {
      model: "test-model", thinkingLevel: "medium", updatedAt: "2026-10-20T00:00:00Z",
    } },
  };
}

test("initialization publishes its runtime identity before any connected snapshot", async (t) => {
  const client = fixture(t);
  const connected: ClientState[] = [];
  client.on("state", state => { if (state.connected) connected.push(state); });
  await client.initialize();
  assert.equal(client.getState().runtimeInstanceId, runtimeId);
  assert.ok(connected.length > 0);
  assert.ok(connected.every(state => state.runtimeInstanceId === runtimeId));
  assert.equal(client.getState().lastSequence, 0, "a handshake must not invent an event");
});

test("the first event preserves the runtime captured by thread setup", async (t) => {
  const client = fixture(t);
  await client.initialize();
  client.state.setAccount({ revision: 1, state: "valid", account: { id: "test-account", linkedMethods: [] } });
  const capturedRuntime = client.getState().runtimeInstanceId;
  client.state.extensionEvent(event());
  assert.equal(client.getState().runtimeInstanceId, capturedRuntime, "thread setup must retain its captured runtime");
  assert.equal(client.getState().account?.account?.id, "test-account");
});

test("an early same-runtime event survives the initialization response with its sequence and account context", async (t) => {
  const client = fixture(t);
  client.state.extensionEvent(event(runtimeId, 7));
  client.state.setAccount({ revision: 1, state: "valid", account: { id: "test-account", linkedMethods: [] } });
  client.state.addSession("thread", "");
  const context = client.state.accountContextToken();
  const security = client.state.securityContextToken("thread");
  await client.initialize();
  assert.equal(client.getState().lastSequence, 7);
  client.state.extensionEvent(event(runtimeId, 8));
  assert.equal(client.state.accountContextToken(), context);
  assert.equal(client.state.securityContextToken("thread"), security);
  assert.equal(client.getState().account?.account?.id, "test-account");
});

test("an actual runtime replacement still invalidates account, sessions and pending authority", async (t) => {
  const client = fixture(t);
  await client.initialize();
  client.state.setAccount({ revision: 1, state: "valid", account: { id: "test-account", linkedMethods: [] } });
  client.state.addSession("thread", "");
  const context = client.state.accountContextToken();
  client.state.extensionEvent(event("replacement-runtime"));
  assert.equal(client.getState().runtimeInstanceId, "replacement-runtime");
  assert.equal(client.getState().account, null);
  assert.deepEqual(client.getState().sessions, {});
  assert.notEqual(client.state.accountContextToken(), context);
});

for (const identity of [undefined, null, "", " ", 5, {}, "x".repeat(129), "runtime\nforged"]) {
  test(`initialization rejects an invalid runtime identity ${JSON.stringify(identity)}`, async (t) => {
    const client = fixture(t, { identity });
    const connected: boolean[] = [];
    client.on("state", state => connected.push(state.connected));
    await assert.rejects(client.initialize(), /runtime identity/i);
    assert.equal(client.getState().connected, false);
    assert.ok(connected.every(value => !value));
    await assert.rejects(client.bootstrapDesktop(), /not initialized/);
  });
}

test("a delayed initialization response cannot replace an identity already observed from events", async (t) => {
  const client = fixture(t);
  client.state.extensionEvent(event("different-runtime", 4));
  await assert.rejects(client.initialize(), /runtime identity/i);
  assert.equal(client.getState().connected, false);
  assert.equal(client.getState().runtimeInstanceId, "different-runtime");
  assert.equal(client.getState().lastSequence, 4);
});
