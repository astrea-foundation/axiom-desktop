import assert from "node:assert/strict";
import test from "node:test";
import type { ClientState, PromptResult, SteerTurnResponse } from "@axiom/axiom-acp-client";
import { MessageQueue, type QueueApi } from "../src/renderer/src/messageQueue";

const settle = () => new Promise((resolve) => setImmediate(resolve));
function fixture(running = true) {
  const values = new Map<string, string>();
  const storage = { getItem: (key: string) => values.get(key) ?? null, setItem: (key: string, value: string) => { values.set(key, value); } };
  const state = { connected: true, runtimeInstanceId: "runtime", account: { state: "valid", account: { id: "account" } }, sessions: {
    thread: { running, activeTurnId: running ? "turn" : null, timeline: [] },
  } } as unknown as ClientState;
  const calls: { type: string; id: string; text: string; turn?: string; web: boolean }[] = [];
  const pending = new Map<string, { resolve: (value: any) => void; reject: (error: Error) => void }>();
  const api: QueueApi = {
    getState: async () => structuredClone(state),
    promptWithWebConsent: async (_thread, text, id, web) => {
      calls.push({ type: "prompt", id, text, web });
      state.sessions.thread!.running = true;
      return new Promise<PromptResult>((resolve, reject) => pending.set(id, { resolve, reject }));
    },
    steer: async (request) => {
      calls.push({ type: "steer", id: request.clientItemId, text: request.text, turn: request.expectedTurnId, web: request.webEnabled });
      return new Promise<SteerTurnResponse>((resolve, reject) => pending.set(request.clientItemId, { resolve, reject }));
    },
  };
  const errors: string[] = [];
  const queue = new MessageQueue(storage, () => {}, (error) => errors.push(error));
  const sync = () => queue.sync(structuredClone(state), api);
  const echo = (id: string) => { state.sessions.thread!.timeline.push({ id, clientItemId: id, kind: "user", text: "saved" }); sync(); };
  const finish = async (id: string) => {
    echo(id); state.sessions.thread!.running = false; state.sessions.thread!.activeTurnId = null;
    pending.get(id)!.resolve({ stopReason: "end_turn" }); await settle(); sync();
  };
  sync();
  return { queue, api, state, calls, pending, storage, values, errors, sync, echo, finish };
}

test("busy messages queue; Send now steers once with the exact turn and Web choice", async () => {
  const f = fixture();
  const first = f.queue.enqueue("thread", "follow up", false);
  const second = f.queue.enqueue("thread", "new direction", true);
  assert.equal(f.calls.length, 0);
  f.queue.sendNow(second); f.queue.sendNow(second);
  assert.deepEqual(f.calls, [{ type: "steer", id: second, text: "new direction", turn: "turn", web: true }]);
  assert.equal(f.queue.list("thread")[1]?.status, "steering");
  f.echo(second); f.pending.get(second)!.resolve({ turnId: "turn", clientItemId: second }); await settle();
  assert.deepEqual(f.queue.list("thread").map((item) => item.id), [first]);
  assert.equal(f.state.sessions.thread!.running, true, "steering never cancels current work");
  f.state.sessions.thread!.running = false; f.sync();
  assert.equal(f.calls.at(-1)?.id, first);
  await f.finish(first);
  assert.deepEqual(f.queue.list("thread"), []);
});

test("normal messages dispatch FIFO with one active prompt even before state catches up", async () => {
  const f = fixture(false);
  const first = f.queue.enqueue("thread", "one", false);
  const second = f.queue.enqueue("thread", "two", false);
  assert.equal(f.calls.length, 1);
  f.echo(first);
  assert.equal(f.queue.list("thread").length, 1);
  await f.finish(first);
  assert.equal(f.calls.length, 2); assert.equal(f.calls[1]?.id, second);
  await f.finish(second); assert.equal(f.queue.list("thread").length, 0);
});

test("stop, failed delivery, and reconnect preserve pending messages without automatic retries", async () => {
  const f = fixture();
  const id = f.queue.enqueue("thread", "keep me", false);
  f.queue.pause("thread"); f.state.sessions.thread!.running = false; f.sync();
  assert.equal(f.calls.length, 0);
  f.queue.sendNow(id); f.pending.get(id)!.reject(new Error("transport lost")); await settle();
  f.sync(); f.sync(); assert.equal(f.calls.length, 1);
  assert.equal(f.queue.list("thread")[0]?.status, "queued");
  assert.equal(f.queue.isPaused("thread"), true);
  const restored = new MessageQueue(f.storage, () => {}, () => {});
  restored.sync(f.state, f.api); // Paused recovery performs no transport call.
  assert.equal(restored.list("thread")[0]?.id, id);
  assert.equal(restored.isPaused("thread"), true);
});

test("late completions never mutate a different account or restart; transcript IDs prevent replay", async () => {
  const f = fixture();
  const id = f.queue.enqueue("thread", "account A only", false);
  f.queue.sendNow(id);
  f.state.account!.account!.id = "account-b"; f.sync();
  assert.equal(f.queue.list("thread").length, 0);
  f.pending.get(id)!.resolve({ turnId: "turn", clientItemId: id }); await settle();
  assert.equal(f.queue.list("thread").length, 0);
  f.state.account!.account!.id = "account"; f.state.runtimeInstanceId = "new-runtime";
  f.state.sessions.thread!.timeline.push({ id, clientItemId: id, kind: "user", text: "native saved it before disconnect" });
  f.sync();
  assert.equal(f.queue.list("thread").length, 0); assert.equal(f.calls.length, 1);
});

test("queue deletion is scoped and write failures prevent dispatch", () => {
  const f = fixture();
  const first = f.queue.enqueue("thread", "delete me", false);
  f.state.sessions.other = structuredClone(f.state.sessions.thread!); f.sync();
  f.queue.enqueue("other", "keep me", false);
  f.queue.remove(first); assert.equal(f.queue.list("thread").length, 0);
  assert.equal(f.queue.list("other").length, 1);
  f.storage.setItem = () => { throw new Error("full"); };
  assert.throws(() => f.queue.enqueue("thread", "keep draft", false), /draft was kept/);
  assert.equal(f.calls.length, 0);
});

test("queued prompts and steering cannot silently inherit newly granted Agent permissions", () => {
  for (const running of [true, false]) {
    const f = fixture(true);
    const id = f.queue.enqueue("thread", "captured with Agent off", false);
    f.state.sessions.thread!.desktopAgent = { enabled: true, permission: "full_access", revision: 1,
      workingDirectory: "/workspace", defaultWorkingDirectory: "/workspace", usesDefaultDirectory: true };
    f.state.sessions.thread!.running = running;
    f.sync(); f.queue.sendNow(id);
    assert.equal(f.calls.length, 0);
    assert.match(f.queue.list("thread")[0]!.error ?? "", /Agent settings changed/);
    assert.equal(f.queue.isPaused("thread"), true);
  }
});


test("a fresh send after Stop runs without releasing older paused messages", async () => {
  const f = fixture();
  const held = f.queue.enqueue("thread", "older queued work", false);
  f.queue.pause("thread");
  f.state.sessions.thread!.running = false; f.sync();
  const fresh = f.queue.enqueue("thread", "fresh user instruction", false);
  assert.deepEqual(f.calls.map((call) => call.id), [fresh]);
  await f.finish(fresh);
  assert.deepEqual(f.calls.map((call) => call.id), [fresh]);
  assert.equal(f.queue.isPaused("thread"), true);
  assert.deepEqual(f.queue.list("thread").map((item) => item.id), [held]);
  f.queue.sendNow(held);
  await f.finish(held);
  assert.deepEqual(f.calls.map((call) => call.id), [fresh, held]);
});


test("a message sent while Stop is still draining runs after cleanup and keeps old work paused", async () => {
  const f = fixture(false);
  const running = f.queue.enqueue("thread", "long request", false);
  f.echo(running);
  const old = f.queue.enqueue("thread", "old queued work", false);
  f.queue.pause("thread");
  const fresh = f.queue.enqueue("thread", "new intent after Stop", false);
  assert.equal(f.calls.length, 1);
  f.state.sessions.thread!.running = false;
  f.pending.get(running)!.resolve({ stopReason: "cancelled" });
  await settle();
  assert.deepEqual(f.calls.map((call) => call.id), [running, fresh]);
  await f.finish(fresh);
  assert.deepEqual(f.queue.list("thread").map((item) => item.id), [old]);
});


test("first messages are durable before thread creation and never replay interrupted setup", async () => {
  const f = fixture(false);
  const id = f.queue.reserve(null, "first message", true);
  assert.equal(f.calls.length, 0);
  assert.equal(f.queue.preparations("account")[0]?.text, "first message");
  assert.equal(f.queue.preparations("other-account").length, 0);
  const restored = new MessageQueue(f.storage, () => {}, () => {});
  restored.sync(f.state, f.api);
  const saved = restored.preparations("account")[0]!;
  assert.equal(saved.id, id); assert.equal(saved.status, "queued");
  restored.sendNow(id);
  assert.equal(f.calls.length, 0, "review is required before interrupted setup can send");
  f.queue.bindPreparation(id, "thread");
  f.queue.ready(id, "thread");
  assert.equal(f.calls[0]?.id, id);
  assert.equal(f.queue.preparations("account").length, 0);
  await f.finish(id);
});

test("setup failure remains reviewable; storage failure prevents the ready transition", () => {
  const f = fixture(false);
  const id = f.queue.reserve(null, "keep all text", false);
  f.queue.bindPreparation(id, "thread");
  f.storage.setItem = () => { throw new Error("full"); };
  assert.throws(() => f.queue.ready(id, "thread"), /full/);
  assert.equal(f.calls.length, 0);
  f.queue.failPreparation(id, "Setup failed");
  assert.equal(f.queue.preparations("account")[0]?.status, "queued");
});

test("large attachment payloads commit before queuing, hydrate paused, and stay account-scoped", async () => {
  const f = fixture();
  const data = new Map<string, { text: string; attachments: any[] }>();
  const payloads = {
    get: async (account: string, id: string) => data.get(`${account}:${id}`),
    put: async (account: string, id: string, payload: any) => { data.set(`${account}:${id}`, structuredClone(payload)); },
    remove: async (account: string, id: string) => { data.delete(`${account}:${id}`); },
  };
  const queue = new MessageQueue(f.storage, () => {}, assert.fail, payloads);
  queue.sync(f.state, f.api);
  const text = "large private prompt ".repeat(20_000);
  const files = [{ kind: "image" as const, name: "private.png", image: { mimeType: "image/png", data: "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGP4z8AAAAMBAQDJ/pLvAAAAAElFTkSuQmCC" } }];
  const id = await queue.reserveDurable("thread", text, false, files);
  queue.ready(id, "thread");
  const manifest = [...f.values.values()].join("");
  assert.ok(manifest.length < 1000);
  assert.ok(!manifest.includes(files[0]!.image.data));
  const restored = new MessageQueue(f.storage, () => {}, assert.fail, payloads);
  restored.sync(f.state, f.api); await settle();
  assert.equal(restored.list("thread")[0]?.text, text);
  assert.deepEqual(restored.list("thread")[0]?.attachments, files);
  assert.ok(restored.isPaused("thread"));
  restored.sendNow(id); assert.equal(f.calls.length, 0, "attachments are queued for the next turn, never dropped into text-only steering");
  const other = structuredClone(f.state); other.account!.account!.id = "other";
  restored.sync(other, f.api); await settle();
  assert.deepEqual(restored.list("thread"), []);
  restored.sync(f.state, f.api); await settle(); restored.remove(id); await settle();
  assert.equal(data.size, 0);
});

test("disk failure or account switch during payload commit leaves the draft unconsumed", async () => {
  const f = fixture();
  let release!: () => void;
  const payloads = { get: async () => undefined, put: async () => new Promise<void>((resolve) => { release = resolve; }), remove: async () => {} };
  const queue = new MessageQueue(f.storage, () => {}, assert.fail, payloads);
  queue.sync(f.state, f.api);
  const reserve = queue.reserveDurable("thread", "private", false);
  await settle();
  const other = structuredClone(f.state); other.account!.account!.id = "other";
  queue.sync(other, f.api); release();
  await assert.rejects(reserve, /account or connection changed/);
  assert.equal(queue.list("thread").length, 0);
  payloads.put = async () => { throw new Error("disk full"); };
  await assert.rejects(queue.reserveDurable("thread", "keep this", false), /disk full/);
  assert.equal(f.calls.length, 0);
});
