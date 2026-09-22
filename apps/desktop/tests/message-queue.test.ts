import assert from "node:assert/strict";
import test from "node:test";
import type { ClientState, PromptResult } from "@axiom/axiom-acp-client";
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
    cancel: async (threadId) => { calls.push({ type: "cancel", id: threadId, text: "", web: false }); },
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

test("Send now cancels once, waits for idle, and prioritizes the selected message", async () => {
  const f = fixture();
  const first = f.queue.enqueue("thread", "follow up", false);
  const second = f.queue.enqueue("thread", "new direction", true);
  f.queue.sendNow(second); f.queue.sendNow(second); f.queue.sendNow(first);
  assert.deepEqual(f.calls.map((call) => call.type), ["cancel"]);
  assert.equal(f.queue.list("thread")[1]?.status, "stopping");
  await settle();
  assert.equal(f.calls.length, 1, "a cancellation notification does not confirm cleanup");
  f.state.sessions.thread!.running = false; f.sync();
  assert.deepEqual(f.calls[1], { type: "prompt", id: second, text: "new direction", web: true });
  f.sync(); assert.equal(f.calls.length, 2);
  await f.finish(second);
  assert.equal(f.calls.at(-1)?.id, first);
  await f.finish(first);
  assert.deepEqual(f.queue.list("thread"), []);
});

test("Send now waits for the old prompt promise after idle is published", async () => {
  const f = fixture(false);
  const running = f.queue.enqueue("thread", "current request", false);
  f.echo(running);
  const next = f.queue.enqueue("thread", "interrupt", true);
  f.queue.sendNow(next); await settle();
  f.state.sessions.thread!.running = false; f.sync();
  assert.deepEqual(f.calls.map((call) => call.type), ["prompt", "cancel"]);
  f.pending.get(running)!.resolve({ stopReason: "cancelled" }); await settle();
  assert.deepEqual(f.calls.map((call) => call.id), [running, "thread", next]);
  await f.finish(next);
});

test("queued cancellation also works during Send now and keeps remaining work paused", async () => {
  const f = fixture();
  const older = f.queue.enqueue("thread", "keep queued", false);
  const selected = f.queue.enqueue("thread", "do not send", false);
  f.queue.sendNow(selected); f.queue.remove(selected);
  await settle(); f.state.sessions.thread!.running = false; f.sync();
  assert.deepEqual(f.calls.map((call) => call.type), ["cancel"]);
  assert.deepEqual(f.queue.list("thread").map((item) => item.id), [older]);
  assert.equal(f.queue.isPaused("thread"), true);
  assert.equal(f.queue.isStopping("thread"), false);
  const restored = new MessageQueue(f.storage, () => {}, assert.fail);
  restored.sync(f.state, f.api);
  assert.deepEqual(restored.list("thread").map((item) => item.id), [older]);
});

test("Stop withdraws Send now without deleting the saved message", async () => {
  const f = fixture();
  const id = f.queue.enqueue("thread", "review later", false);
  f.queue.sendNow(id); f.queue.pause("thread");
  await settle(); f.state.sessions.thread!.running = false; f.sync();
  assert.deepEqual(f.calls.map((call) => call.type), ["cancel"]);
  assert.equal(f.queue.list("thread")[0]?.status, "queued");
  assert.equal(f.queue.isPaused("thread"), true);
});

test("cancelling still prevents delivery when the queue cannot be saved", async () => {
  for (const stopping of [false, true]) {
    const f = fixture();
    const id = f.queue.enqueue("thread", "keep for review", false);
    if (stopping) f.queue.sendNow(id);
    f.storage.setItem = () => { throw new Error("disk full"); };
    f.queue.remove(id);
    await settle(); f.state.sessions.thread!.running = false; f.sync();
    assert.equal(f.calls.some((call) => call.type === "prompt"), false);
    assert.equal(f.queue.list("thread")[0]?.status, "queued");
    assert.equal(f.queue.isPaused("thread"), true);
    assert.ok(f.errors.length > 0);
  }
});

test("failed cancellation retains the queued message without automatic retry", async () => {
  const f = fixture();
  f.api.cancel = async () => { throw new Error("offline"); };
  const id = f.queue.enqueue("thread", "keep me", false);
  f.queue.sendNow(id); await settle();
  f.state.sessions.thread!.running = false; f.sync(); f.sync();
  assert.equal(f.calls.length, 0);
  assert.match(f.queue.list("thread")[0]?.error ?? "", /Couldn’t stop/);
  assert.equal(f.queue.isPaused("thread"), true);
  assert.equal(f.queue.isStopping("thread"), false);
});

for (const change of ["account", "runtime", "disconnect"] as const) test(`Send now cannot cross a ${change} change`, async () => {
  const f = fixture();
  let release!: () => void;
  f.api.cancel = () => new Promise<void>((resolve) => { release = resolve; });
  const id = f.queue.enqueue("thread", "old context", false);
  f.queue.sendNow(id);
  if (change === "account") f.state.account!.account!.id = "another-account";
  if (change === "runtime") f.state.runtimeInstanceId = "another-runtime";
  if (change === "disconnect") f.state.connected = false;
  f.sync(); release(); await settle();
  f.state.sessions.thread!.running = false; f.sync();
  assert.equal(f.calls.length, 0);
  assert.equal(f.queue.list("thread").some((item) => item.status === "stopping"), false);
});

test("a cancelled queue entry waits for the cancel notification before releasing fresh work", async () => {
  const f = fixture();
  let release!: () => void;
  f.api.cancel = () => new Promise<void>((resolve) => { release = resolve; });
  const id = f.queue.enqueue("thread", "cancel me", false);
  f.queue.sendNow(id); f.queue.remove(id);
  f.state.sessions.thread!.running = false; f.sync();
  const fresh = f.queue.enqueue("thread", "fresh instruction", false);
  assert.equal(f.calls.length, 0);
  release(); await settle();
  assert.equal(f.calls[0]?.id, fresh);
  await f.finish(fresh);
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
  const f = fixture(false);
  const id = f.queue.enqueue("thread", "account A only", false);
  f.state.account!.account!.id = "account-b"; f.sync();
  assert.equal(f.queue.list("thread").length, 0);
  f.pending.get(id)!.resolve({ stopReason: "end_turn" }); await settle();
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

test("queued prompts cannot silently inherit newly granted Agent permissions", () => {
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
  const files = [{ kind: "file" as const, name: "private.txt", file: { name: "private.txt", mimeType: "text/plain", data: Buffer.from("private attachment").toString("base64") } }];
  const id = await queue.reserveDurable("thread", text, false, files);
  queue.ready(id, "thread");
  const manifest = [...f.values.values()].join("");
  assert.ok(manifest.length < 1000);
  assert.ok(!manifest.includes("private attachment"));
  const restored = new MessageQueue(f.storage, () => {}, assert.fail, payloads);
  restored.sync(f.state, f.api); await settle();
  assert.equal(restored.list("thread")[0]?.text, text);
  assert.deepEqual(restored.list("thread")[0]?.attachments, files);
  assert.ok(restored.isPaused("thread"));
  let delivered: unknown;
  f.api.promptWithAttachments = async (_thread, text, deliveredId, web, revision, attachments) => {
    delivered = { text, deliveredId, web, revision, attachments };
    return f.api.promptWithWebConsent(_thread, text, deliveredId, web, revision);
  };
  restored.sendNow(id); await settle();
  assert.deepEqual(f.calls.map((call) => call.type), ["cancel"]);
  f.state.sessions.thread!.running = false; restored.sync(f.state, f.api);
  assert.deepEqual(delivered, { text, deliveredId: id, web: false, revision: 0, attachments: files });
  f.state.sessions.thread!.timeline.push({ id, clientItemId: id, kind: "user", text: "saved" });
  f.state.sessions.thread!.running = false;
  f.pending.get(id)!.resolve({ stopReason: "end_turn" }); await settle();
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
