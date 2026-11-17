import "./verify-state-views.js";
import assert from "node:assert/strict";
import test, { type TestContext } from "node:test";
import { AxiomAcpClient } from "../src/client.js";

function fixture(t: TestContext) {
  const client = new AxiomAcpClient({ command: process.execPath, args: ["-e", "process.stdin.resume()"] });
  t.after(() => client.close());
  client.state.setAccount({ revision: 1, state: "valid", account: { id: "account-a", linkedMethods: [] } });
  client.state.addSession("thread", "/tmp");
  Object.assign(client, { supportsSessionContinuation: true, waitForSecurityPreflight: async () => {}, refreshThread: async () => {} });
  return client;
}

test("Retry is one native continuation control with no user message or prompt text", async t => {
  const client = fixture(t);
  const requests: { method: string; params: unknown }[] = [];
  let finish!: (value: { stopReason: string }) => void;
  t.mock.method(client.process, "request", async (method: string, params: unknown) => {
    requests.push({ method, params });
    return new Promise(resolve => { finish = resolve; });
  });
  const pending = client.continueSession("thread", true, 2);
  await assert.rejects(client.continueSession("thread"), /current response/);
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(client.getState().sessions.thread?.running, true);
  assert.deepEqual(requests, [{ method: "_axiom/session/continue", params: { sessionId: "thread", webEnabled: true, agentRevision: 2 } }]);
  assert.deepEqual(client.getState().sessions.thread?.timeline, []);
  finish({ stopReason: "end_turn" });
  await pending;
  assert.equal(client.getState().sessions.thread?.running, false);
});

test("Stop during continuation preparation prevents a later native request", async t => {
  const client = fixture(t);
  let prepared!: () => void;
  Object.assign(client, { waitForSecurityPreflight: () => new Promise<void>(resolve => { prepared = resolve; }) });
  const request = t.mock.method(client.process, "request", async () => { throw new Error("unexpected request"); });
  t.mock.method(client.process, "notify", async () => {});
  const pending = client.continueSession("thread");
  assert.equal(client.getState().sessions.thread?.running, true, "Stop must be available during preparation");
  await client.cancel("thread");
  assert.deepEqual(await pending, { stopReason: "cancelled" });
  assert.equal(client.getState().sessions.thread?.running, false);
  assert.equal(request.mock.callCount(), 0);
  prepared();
});

test("older sidecars cannot turn Retry into a visible prompt", async t => {
  const client = fixture(t);
  Object.assign(client, { supportsSessionContinuation: false });
  const request = t.mock.method(client.process, "request", async () => {});
  await assert.rejects(client.continueSession("thread"), /Restart Axiom/);
  assert.equal(request.mock.callCount(), 0);
});

test("an old account's continuation cannot refresh the next account", async t => {
  const client = fixture(t);
  let finish!: (value: { stopReason: string }) => void;
  t.mock.method(client.process, "request", async () => new Promise(resolve => { finish = resolve; }));
  const refresh = t.mock.method(client, "refreshThread", async () => {});
  const pending = client.continueSession("thread");
  await new Promise(resolve => setImmediate(resolve));
  const checked = assert.rejects(pending, /account|context/i);
  client.state.setAccount({ revision: 2, state: "valid", account: { id: "account-b", linkedMethods: [] } });
  finish({ stopReason: "end_turn" });
  await checked;
  assert.equal(refresh.mock.callCount(), 0);
});
