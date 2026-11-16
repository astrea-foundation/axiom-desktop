import "./verify-state-views.js";
import assert from "node:assert/strict";
import test from "node:test";
import { AxiomAcpClient } from "../src/client.js";
import { AxiomStateStore, type ClientState, type StateChange } from "../src/state.js";

const chunk = (sessionId: string, text: string) => ({ sessionId, update: { sessionUpdate: "agent_message_chunk", messageId: "assistant:turn", content: { type: "text", text } } });

function observed(store: AxiomStateStore) {
  const emissions: { state: ClientState; change: StateChange }[] = [];
  store.on("change", (state: ClientState, change: StateChange) => emissions.push({ state, change }));
  return emissions;
}

test("a burst of changes is published once, before anything queued after it", async () => {
  const store = new AxiomStateStore(), emissions = observed(store);
  store.addSession("a", "/tmp"); store.addSession("b", "/tmp");
  store.setModes("a", "web", [{ id: "web", name: "Web" }]);
  store.standardUpdate(chunk("a", "one ")); store.standardUpdate(chunk("a", "two "));
  assert.equal(emissions.length, 0, "nothing is published while the burst is still running");
  // A continuation queued after the burst (an RPC reply to the renderer, for
  // example) must observe the publication first.
  const order: string[] = [];
  store.on("change", () => order.push("state"));
  await Promise.resolve().then(() => order.push("continuation"));
  assert.deepEqual(order, ["state", "continuation"]);
  assert.equal(emissions.length, 1);
  assert.equal(emissions[0]!.change.sessions, null, "the first emission makes no claim about untouched sessions");
  assert.equal(emissions[0]!.state.sessions.a!.timeline[0]!.text, "one two ");

  store.standardUpdate(chunk("b", "three "));
  store.setDiagnostic("stderr");
  await Promise.resolve();
  assert.equal(emissions.length, 2);
  assert.deepEqual(emissions[1]!.change, { source: store, revision: 2, sessions: ["b"] });
  assert.equal(emissions[1]!.state.sessions.a, emissions[0]!.state.sessions.a, "an untouched session is shared between views");
  assert.notEqual(emissions[1]!.state.sessions.b, emissions[0]!.state.sessions.b);
  assert.equal(emissions[1]!.state.diagnostic, "stderr");
  assert.equal(store.snapshot(), emissions[1]!.state, "reading state does not copy it again");
  assert.throws(() => { (store.snapshot().sessions.a as { title: string | null }).title = "mutated"; }, TypeError, "views are immutable");
});

test("flush publishes immediately and account or connection changes never wait", () => {
  const store = new AxiomStateStore(), emissions = observed(store);
  store.addSession("a", "/tmp");
  store.flush();
  assert.equal(emissions.length, 1);
  store.flush();
  assert.equal(emissions.length, 1, "nothing pending, nothing published");
  store.setAccount({ revision: 1, state: "valid", account: { id: "one", linkedMethods: [] } });
  assert.equal(emissions.length, 2, "account context is synchronous");
  assert.equal(emissions[1]!.change.sessions, null);
  store.setConnected(false, "exit");
  assert.equal(emissions.length, 3, "connection state is synchronous");
  store.initializeRuntime("runtime-one");
  store.extensionEvent({ runtimeInstanceId: "runtime-two", sequence: 1, event: { kind: "collections_changed", state: { revision: 1, collections: [] } } } as never);
  assert.equal(emissions.length, 4, "a runtime replacement is synchronous");
  assert.equal(emissions[3]!.state.runtimeInstanceId, "runtime-two");
});

test("changes that return without publishing still reach the next view", async () => {
  const store = new AxiomStateStore(), emissions = observed(store);
  store.addSession("a", "/tmp"); store.addSession("b", "/tmp");
  const delivery = (threadRevision: number) => ({ _meta: { axiom: { threadRevision, lastTimelineSequence: threadRevision, timelineItemId: "row" } } });
  store.standardUpdate({ ...chunk("a", "text "), ...delivery(4) });
  await Promise.resolve();
  // The same revision again is ignored for text, but advances thread metadata silently.
  store.standardUpdate({ ...chunk("a", "duplicate "), _meta: { axiom: { threadRevision: 4, lastTimelineSequence: 9, timelineItemId: "row" } } });
  await Promise.resolve();
  assert.equal(emissions.length, 1, "an ignored duplicate publishes nothing");
  assert.equal(store.snapshot().sessions.a!.lastTimelineSequence, 9);
  store.standardUpdate(chunk("b", "other "));
  await Promise.resolve();
  assert.deepEqual([...emissions[1]!.change.sessions!].sort(), ["a", "b"], "the silent change is announced with the next emission");
  assert.equal(emissions[1]!.state.sessions.a!.lastTimelineSequence, 9);
});

test("an interaction is announced after the state that lists it", (t) => {
  const client = new AxiomAcpClient({ command: process.execPath, args: ["-e", "process.stdin.resume()"] });
  t.after(() => client.close());
  const order: string[] = [];
  client.on("state", (state: ClientState) => order.push(`state:${state.sessions.a?.interactions.length ?? 0}`));
  client.on("interaction", () => order.push("interaction"));
  client.state.addSession("a", "/tmp");
  client.process.emit("request", { request: { id: 7, method: "session/request_permission", params: { sessionId: "a" } }, respond: () => {}, reject: () => {} });
  assert.deepEqual(order, ["state:1", "interaction"]);
});
