import assert from "node:assert/strict";
import test from "node:test";
import { NativePersistence, type NativeLocalApi, type LegacyPayloadStore } from "../src/renderer/src/nativePersistence";
import { MessageQueue, type QueueApi, type QueuedMessage, type QueueStore } from "@axiom/chat-core/messageQueue";
import type { PromptPayload, PayloadStore } from "@axiom/chat-core/localPayloads";
import type { ClientState } from "@axiom/axiom-acp-client";
import { localPayloadRequest, requireLocalPayloadAccount } from "../src/main/local-payload";

const queueKey = "axiom.message-queue.v1:alice";
const queueId = "axiom.message-queue.v1";
const draftKey = "axiom.draft.v1:alice:new";
const scope = (account: string, id: string) => JSON.stringify([account, id]);
const payload = (text: string): PromptPayload => ({ text, attachments: [] });
const item = (id: string, extra: Partial<QueuedMessage> = {}): QueuedMessage => ({
  id, threadId: "thread", text: "queued text", webEnabled: false, agentRevision: 0, status: "queued", ...extra,
});
function deferred<T>() {
  let resolve!: (value: T) => void, reject!: (reason: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
const settle = () => new Promise(resolve => setImmediate(resolve));
function fixture() {
  const native = new Map<string, unknown>(), legacy = new Map<string, PromptPayload>(), storage = new Map<string, string>();
  const calls: Parameters<NativeLocalApi["localPayload"]>[0][] = [];
  const api: NativeLocalApi = { localPayload: async request => {
    calls.push(structuredClone(request));
    const key = scope(request.accountId, request.id);
    if (request.action.kind === "get") return { value: structuredClone(native.get(key)) };
    if (request.action.kind === "put") native.set(key, structuredClone(request.action.value));
    else native.delete(key);
    return {};
  } };
  const legacyApi: LegacyPayloadStore = {
    get: async (account, id) => structuredClone(legacy.get(scope(account, id))),
    put: async () => { throw new Error("plaintext writes forbidden"); },
    remove: async (account, id) => { legacy.delete(scope(account, id)); },
    removeIfUnchanged: async (account, id, expected) => {
      const current = legacy.get(scope(account, id));
      if (current !== undefined && JSON.stringify(current) !== JSON.stringify(expected)) return false;
      await legacyApi.remove(account, id);
      return true;
    },
    entries: async account => [...legacy].flatMap(([key, value]) => {
      const [owner, id] = JSON.parse(key); return owner === account ? [[id, structuredClone(value)] as [string, PromptPayload]] : [];
    }),
  };
  const browser = {
    get length() { return storage.size; },
    key: (index: number) => [...storage.keys()][index] ?? null,
    getItem: (key: string) => storage.get(key) ?? null,
    removeItem: (key: string) => { storage.delete(key); },
  };
  const persistence = new NativePersistence(() => api, legacyApi, () => browser);
  persistence.setContext("alice", "runtime", 1);
  return { native, legacy, storage, calls, api, legacyApi, browser, persistence };
}

test("migration encrypts text mirrors, attachment drafts, failed/preparing queue payloads and metadata before cleanup", async () => {
  const f = fixture();
  f.storage.set(draftKey, "newer synchronous draft");
  f.legacy.set(scope("alice", draftKey), payload("older indexed draft"));
  const image = { kind: "image" as const, name: "private.png", image: { mimeType: "image/png", data: "iVBORw0KGgo=" } };
  f.legacy.set(scope("alice", "draft-attachments:new"), { text: "", attachments: [image] });
  f.legacy.set(scope("alice", "pending"), { text: "pending text", attachments: [image] });
  f.storage.set(queueKey, JSON.stringify([
    item("pending", { text: "", payloadId: "pending", status: "sending", preparing: true, error: "setup failed", mcpRevision: 7 }),
    item("inline", { text: "legacy inline failed message", error: "delivery not confirmed" }),
  ]));
  f.storage.set("axiom.draft.v1:bob:new", "other account");
  f.legacy.set(scope("bob", "private"), payload("other account"));
  const restored = await f.persistence.load("alice");
  assert.equal(restored.length, 2);
  assert.equal(restored[0]?.error, "setup failed");
  assert.equal(restored[0]?.preparing, true);
  assert.equal(restored[0]?.mcpRevision, 7);
  assert.equal((await f.persistence.get("alice", draftKey))?.text, "newer synchronous draft");
  assert.deepEqual((await f.persistence.get("alice", "pending"))?.attachments, [image]);
  assert.equal(f.storage.has(queueKey), false);
  assert.equal(f.storage.has(draftKey), false);
  assert.equal(f.legacy.has(scope("alice", "pending")), false);
  assert.equal(f.storage.get("axiom.draft.v1:bob:new"), "other account");
  assert.equal(f.legacy.get(scope("bob", "private"))?.text, "other account");
  assert.ok(f.calls.every(call => call.accountId === "alice"));
});

test("lost native import ACK keeps every legacy record and retry never overwrites newer encrypted text", async () => {
  const f = fixture();
  f.storage.set(draftKey, "legacy text");
  f.storage.set(queueKey, JSON.stringify([item("pending")]));
  const original = f.api.localPayload;
  let fail = true;
  f.api.localPayload = async request => {
    const result = await original(request);
    if (fail && request.action.kind === "put") { fail = false; throw new Error("lost native ACK"); }
    return result;
  };
  await assert.rejects(f.persistence.get("alice", draftKey), /lost native ACK/);
  assert.equal(f.storage.get(draftKey), "legacy text");
  assert.ok(f.storage.has(queueKey));
  f.native.set(scope("alice", draftKey), { version: 1, kind: "prompt", payload: payload("new native text") });
  assert.equal((await f.persistence.get("alice", draftKey))?.text, "new native text");
  const restored = await f.persistence.load("alice");
  assert.ok(restored.some(item => item.id === "pending"));
  assert.ok(restored.some(item => item.id.startsWith("recovered-legacy:") && item.preparing));
  assert.equal(f.storage.size, 0);
});

test("migration receipts prevent replay after ACK then browser cleanup failure", async () => {
  const f = fixture();
  f.storage.set(queueKey, JSON.stringify([item("delivered")]));
  const remove = f.browser.removeItem;
  let fail = true;
  f.browser.removeItem = key => { if (fail) { fail = false; throw new Error("cleanup failed"); } remove(key); };
  await assert.rejects(f.persistence.load("alice"), /cleanup failed/);
  // Native delivery happened before restart, while the old browser key survived.
  f.native.set(scope("alice", queueId), { version: 1, kind: "queue", items: [], legacyImported: true });
  assert.deepEqual(await f.persistence.load("alice"), []);
  assert.equal(f.storage.has(queueKey), false);
});

test("retry preserves a legacy draft changed after import and imports new pending queue IDs", async () => {
  const f = fixture();
  f.storage.set(draftKey, "first browser draft");
  f.storage.set(queueKey, JSON.stringify([item("first-message")]));
  const original = f.api.localPayload;
  let changed = false;
  f.api.localPayload = async request => {
    const result = await original(request);
    if (!changed && request.id === queueId && request.action.kind === "put") {
      changed = true;
      f.storage.set(draftKey, "edited during import");
      f.storage.set(queueKey, JSON.stringify([item("first-message"), item("new-message", { error: "not sent" })]));
    }
    return result;
  };
  await assert.rejects(f.persistence.load("alice"), /draft changed/);
  const restored = await f.persistence.load("alice");
  assert.equal((await f.persistence.get("alice", draftKey))?.text, "first browser draft");
  const recovery = restored.find(item => item.id.startsWith("recovered-legacy:"));
  assert.equal((await f.persistence.get("alice", recovery!.payloadId!))?.text, "edited during import");
  assert.equal(restored.filter(item => item.id === "first-message").length, 1);
  assert.equal(restored.find(item => item.id === "new-message")?.error, "not sent");
  assert.equal(f.storage.size, 0);
});

test("partial anonymous cleanup retry does not duplicate or resurrect a recovered preparation", async () => {
  const f = fixture();
  f.native.set(scope("alice", draftKey), { version: 1, kind: "prompt", payload: payload("authored draft") });
  f.storage.set("axiom.draft.v1:anonymous:new", "anonymous text");
  f.legacy.set(scope("anonymous", "draft-attachments:new"), { text: "", attachments: [{ kind: "file", name: "a.txt", file: { name: "a.txt", mimeType: "text/plain", data: "eA==" } }] });
  const remove = f.legacyApi.remove;
  let fail = true;
  f.legacyApi.remove = async (account, id) => { if (fail) { fail = false; throw Error("cleanup failed"); } await remove(account, id); };
  await assert.rejects(f.persistence.load("alice"), /cleanup failed/);
  const saved = f.native.get(scope("alice", queueId)) as { items: QueuedMessage[] };
  assert.equal(saved.items.length, 1);
  const recoveryId = saved.items[0]!.payloadId!;
  // Simulate explicit use/removal after the commit while stale browser data remains.
  f.native.delete(scope("alice", recoveryId));
  f.native.set(scope("alice", queueId), { version: 1, kind: "queue", items: [], legacyImported: true });
  assert.deepEqual(await f.persistence.load("alice"), []);
  assert.equal(f.native.has(scope("alice", recoveryId)), false);
  assert.equal(f.legacy.size, 0);
});

test("native storage failure never falls back to plaintext; anonymous drafts remain memory-only", async () => {
  const f = fixture();
  await f.persistence.put("anonymous", "draft", payload("before unlock"));
  assert.equal((await f.persistence.get("anonymous", "draft"))?.text, "before unlock");
  assert.equal(f.calls.length, 0);
  assert.equal(f.storage.size, 0);
  assert.equal(f.legacy.size, 0);
  f.api.localPayload = async () => { throw new Error("key unavailable"); };
  await assert.rejects(f.persistence.put("alice", "draft", payload("private")), /key unavailable/);
  assert.equal(f.storage.size, 0);
  assert.equal(f.legacy.size, 0);
});

test("account transitions reject delayed reads including away-and-back to the same owner", async () => {
  const f = fixture();
  await f.persistence.put("alice", "draft", payload("alice secret"));
  const original = f.api.localPayload;
  const gate = deferred<void>();
  f.api.localPayload = async request => { const result = await original(request); await gate.promise; return result; };
  const pending = f.persistence.get("alice", "draft");
  await settle();
  f.persistence.setContext("bob", "runtime", 2);
  f.persistence.setContext("alice", "runtime", 3);
  gate.resolve();
  await assert.rejects(pending, /account or connection changed/);
  await assert.rejects(f.persistence.get("bob", "draft"), /account or connection changed/);
});

function queueFixture() {
  const nativeItems: { value: QueuedMessage[] } = { value: [] };
  const payloads = new Map<string, PromptPayload>();
  const removed: string[] = [], sends: string[] = [], errors: string[] = [];
  const persistence: QueueStore = { load: async () => structuredClone(nativeItems.value), save: async (_, items) => { nativeItems.value = structuredClone([...items]); } };
  const payloadStore: PayloadStore = { get: async (_, id) => payloads.get(id), put: async (_, id, value) => { payloads.set(id, value); }, remove: async (_, id) => { removed.push(id); payloads.delete(id); } };
  const state = { connected: true, runtimeInstanceId: "runtime", account: { state: "valid", account: { id: "alice" } }, sessions: { thread: { running: true, timeline: [] } } } as unknown as ClientState;
  const api: QueueApi = {
    getState: async () => structuredClone(state), cancel: async () => {},
    promptWithWebConsent: async (_, __, id) => { sends.push(id); return { stopReason: "end_turn" }; },
  };
  const queue = new MessageQueue({ getItem: () => { throw Error("plaintext read"); }, setItem: () => { throw Error("plaintext write"); } }, () => {}, error => errors.push(error), payloadStore, persistence);
  queue.sync(state, api);
  return { queue, persistence, payloads, nativeItems, removed, sends, errors, state, api };
}

test("durable reservation waits for encrypted metadata ACK and does not dispatch early", async () => {
  const f = queueFixture(); await f.queue.flush();
  const gate = deferred<void>();
  const save = f.persistence.save;
  f.persistence.save = async (account, items) => { await gate.promise; await save(account, items); };
  let done = false;
  const reservation = f.queue.reserveDurable("thread", "draft remains until ACK", false).then(id => { done = true; return id; });
  await settle();
  assert.equal(done, false);
  assert.equal(f.sends.length, 0);
  gate.resolve(); const id = await reservation;
  assert.equal(f.nativeItems.value[0]?.id, id);
  assert.equal(f.payloads.get(id)?.text, "draft remains until ACK");
});

test("failed native queue persistence keeps draft payload, reports error and sends nothing", async () => {
  const f = queueFixture(); await f.queue.flush();
  f.persistence.save = async () => { throw Error("encrypted disk failed"); };
  await assert.rejects(f.queue.reserveDurable("thread", "unsent", false), /encrypted disk failed/);
  assert.equal(f.sends.length, 0);
  assert.equal(f.payloads.size, 1);
  assert.equal(f.queue.preparations("alice")[0]?.status, "queued");
  assert.equal(f.removed.length, 0);
});

test("payload is deleted only after transcript ACK and encrypted queue removal ACK", async () => {
  const f = queueFixture(); await f.queue.flush();
  const id = await f.queue.reserveDurable("thread", "message", false);
  f.queue.ready(id, "thread"); await f.queue.flush();
  assert.equal(f.removed.length, 0);
  const gate = deferred<void>(); const save = f.persistence.save;
  f.persistence.save = async (account, items) => { await gate.promise; await save(account, items); };
  f.state.sessions.thread!.timeline = [{ kind: "user", clientItemId: id }] as any;
  f.queue.sync(f.state, f.api); await settle();
  assert.equal(f.removed.length, 0);
  gate.resolve(); await f.queue.flush(); await settle();
  assert.deepEqual(f.removed, [id]);
  assert.deepEqual(f.nativeItems.value, []);
});

test("failed restore cannot overwrite the saved outbox or send a new message", async () => {
  const f = queueFixture(); f.queue.dispose();
  f.persistence.load = async () => { throw Error("cannot decrypt"); };
  f.queue.sync(f.state, f.api);
  await assert.rejects(f.queue.flush(), /could not be read/);
  await assert.rejects(f.queue.reserveDurable("thread", "new", false));
  assert.equal(f.sends.length, 0);
  assert.equal(f.nativeItems.value.length, 0);
});

test("main IPC payload validation allowlists actions and enforces the exact active account", () => {
  const state = { connected: true, account: { state: "valid", account: { id: "alice" } } } as ClientState;
  requireLocalPayloadAccount("alice", state);
  assert.throws(() => requireLocalPayloadAccount("bob", state));
  assert.throws(() => localPayloadRequest({ accountId: "alice", id: "draft", action: { kind: "export-key" } }));
  assert.throws(() => localPayloadRequest({ accountId: "alice", id: "draft", action: { kind: "put", value: { version: 1, kind: "arbitrary" } } }));
  assert.deepEqual(localPayloadRequest({ accountId: "alice", id: "draft", action: { kind: "put", value: { version: 1, kind: "prompt", payload: payload("text") } } }).action.kind, "put");
});

test("failed encrypted removal keeps the queued message visible and its payload intact", async () => {
  const f = queueFixture(); await f.queue.flush();
  const id = await f.queue.reserveDurable("thread", "keep until removed", false);
  f.queue.ready(id, "thread"); await f.queue.flush();
  f.persistence.save = async () => { throw Error("encrypted deletion failed"); };
  f.queue.remove(id); await settle();
  assert.equal(f.queue.list("thread")[0]?.id, id);
  assert.equal(f.removed.length, 0);
  assert.equal(f.payloads.has(id), true);
});

test("legacy anonymous drafts migrate only on unlock, adopt an empty composer and retain conflicting text encrypted", async () => {
  const f = fixture();
  const anonymousKey = "axiom.draft.v1:anonymous:new";
  f.storage.set(anonymousKey, "legacy before sign-in");
  f.legacy.set(scope("anonymous", anonymousKey), payload("older copy"));
  assert.equal(await f.persistence.get("anonymous", anonymousKey), undefined);
  assert.equal(f.calls.length, 0);
  assert.equal((await f.persistence.get("alice", draftKey))?.text, "legacy before sign-in");
  assert.equal(f.storage.has(anonymousKey), false);
  assert.equal(f.legacy.has(scope("anonymous", anonymousKey)), false);

  const conflict = fixture();
  conflict.storage.set(anonymousKey, "recover without overwriting");
  conflict.native.set(scope("alice", draftKey), { version: 1, kind: "prompt", payload: payload("authored account draft") });
  assert.equal((await conflict.persistence.get("alice", draftKey))?.text, "authored account draft");
  assert.ok([...conflict.native].some(([id, value]) => id.includes("recovered-anonymous:") && (value as any).payload.text === "recover without overwriting"));
  assert.equal(conflict.storage.has(anonymousKey), false);
});

test("legacy queue preserves a distinct payload reference through migration and acknowledgement", async () => {
  const f = fixture();
  f.legacy.set(scope("alice", "legacy-payload-key"), payload("preserved referenced input"));
  f.storage.set(queueKey, JSON.stringify([item("message-key", { text: "", payloadId: "legacy-payload-key" })]));
  const items = await f.persistence.load("alice");
  assert.equal(items[0]?.payloadId, "legacy-payload-key");
  const state = { connected: true, runtimeInstanceId: "runtime", account: { state: "valid", account: { id: "alice" } }, sessions: { thread: { running: false, timeline: [] } } } as unknown as ClientState;
  const queue = new MessageQueue(null, () => {}, () => {}, f.persistence, f.persistence);
  const api: QueueApi = { getState: async () => state, cancel: async () => {}, promptWithWebConsent: async () => { throw Error("must not auto-send a restored message"); } };
  queue.sync(state, api); await queue.flush();
  assert.equal(queue.list("thread")[0]?.text, "preserved referenced input");
  queue.remove("message-key"); await queue.flush(); await settle();
  assert.equal(await f.persistence.get("alice", "legacy-payload-key"), undefined);
});

test("anonymous conflict is a reviewable preparation alongside a full outbox and never auto-dispatches", async () => {
  const f = fixture();
  f.native.set(scope("alice", draftKey), { version: 1, kind: "prompt", payload: payload("authored draft") });
  f.storage.set("axiom.draft.v1:anonymous:new", "unsigned draft text");
  const image = { kind: "image" as const, name: "anonymous.png", image: { mimeType: "image/png", data: "iVBORw0KGgo=" } };
  f.legacy.set(scope("anonymous", "draft-attachments:new"), { text: "", attachments: [image] });
  f.storage.set(queueKey, JSON.stringify(Array.from({ length: 32 }, (_, index) => item(`queued-${index}`))));
  const state = { connected: true, runtimeInstanceId: "runtime", account: { state: "valid", account: { id: "alice" } }, sessions: { thread: { running: false, timeline: [] } } } as unknown as ClientState;
  const sends: string[] = [];
  const api: QueueApi = { getState: async () => state, cancel: async () => {}, promptWithWebConsent: async (_, __, id) => { sends.push(id); return { stopReason: "end_turn" }; } };
  const queue = new MessageQueue(null, () => {}, error => { throw Error(error); }, f.persistence, f.persistence);
  queue.sync(state, api); await queue.flush();
  const recovered = queue.preparations("alice");
  assert.equal(recovered.length, 1);
  assert.equal(recovered[0]?.text, "unsigned draft text");
  assert.deepEqual(recovered[0]?.attachments, [image]);
  assert.equal(recovered[0]?.status, "queued");
  assert.equal(queue.list("thread").length, 32);
  queue.sendNow(recovered[0]!.id); await settle();
  assert.deepEqual(sends, []);
  const beforeFullReservation = f.native.size;
  await assert.rejects(queue.reserveDurable("thread", "new input", false), /queue is full/);
  assert.equal(f.native.size, beforeFullReservation);
  const id = recovered[0]!.id;
  assert.equal((await f.persistence.get("alice", id))?.text, "unsigned draft text");
  queue.remove(id); await queue.flush(); await settle();
  assert.equal(await f.persistence.get("alice", id), undefined);
  assert.equal((await f.persistence.get("alice", draftKey))?.text, "authored draft");
});
