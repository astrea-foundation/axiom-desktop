import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { deserialize, serialize } from "node:v8";
import { runInNewContext } from "node:vm";
import ts from "typescript";
import { AxiomStateStore, type ClientState, type GetThreadTimelineResponse, type StateChange } from "@axiom/axiom-acp-client";
import { AgentStateTransport } from "../src/main/agent-state-transport";
import type { AgentApi } from "../src/preload/agent-api";
import { AgentStateImages } from "../src/preload/agent-state-images";
import { subscribeAgentState } from "../src/renderer/src/useDesktopState";
import type { AgentStatePacket } from "../src/shared/agent-state";

// Shared views are verified against a complete copy and frozen for every step.
AxiomStateStore.verifyViews = true;

const resources = () => ({ create: () => `blob:${Math.random()}`, revoke: () => {} });
const sha = (index: number) => index.toString(16).padStart(64, "0");

function page(threadId: string, items: number, revision = items): GetThreadTimelineResponse {
  return {
    thread: { threadId, title: `Thread ${threadId}`, cwd: "/tmp/chat", origin: "desktop", profile: "web", selectedModel: "model",
      thinkingLevel: "medium", lifecycle: "ready", archived: false, revision, lastTimelineSequence: items,
      createdAt: "2026-11-15T00:00:00Z", updatedAt: "2026-11-15T00:00:00Z" },
    items: Array.from({ length: items }, (_, index) => ({ id: `${threadId}-${index}`, threadId, turnId: `turn-${index >> 1}`, sequence: index + 1,
      kind: index % 2 ? "assistant_message" : "user_message", status: "completed", content: `message ${index} `.repeat(40), metadata: {},
      createdAt: "2026-11-15T00:00:00Z", updatedAt: "2026-11-15T00:00:00Z" })),
  } as GetThreadTimelineResponse;
}

/** Store → main transport → IPC copy → real preload API → bridge copy → page subscription. */
function pipeline() {
  const store = new AxiomStateStore();
  const transport = new AgentStateTransport();
  const sent: AgentStatePacket[] = [];
  const wire = <T>(value: T): T => deserialize(serialize(value)) as T;
  let dropping = false;
  const documents: { channel: (packet: AgentStatePacket) => void }[] = [];
  store.on("change", (state: ClientState, change: StateChange) => {
    const packet = transport.encode(state, change);
    if (!packet.state && !packet.patch) return;
    sent.push(packet);
    if (!dropping) for (const document of documents) document.channel(wire(packet));
  });
  const source = readFileSync(new URL("../src/preload/agent-api.ts", import.meta.url), "utf8");
  const { outputText } = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 } });
  /** A fresh document: its own preload state, as after a reload. */
  const open = () => {
    const exports: { agentApi?: AgentApi } = {};
    const calls: string[] = [];
    let channel!: (packet: AgentStatePacket) => void;
    runInNewContext(outputText, {
      exports, window: { addEventListener: () => {} },
      require: (name: string) => {
        if (name === "./agent-state-images") return { AgentStateImages: class extends AgentStateImages { constructor() { super(resources()); } } };
        assert.equal(name, "electron");
        return { ipcRenderer: {
          invoke: async (name: string) => { calls.push(name); assert.equal(name, "agent:state"); return wire(transport.snapshot(store.snapshot())); },
          on: (name: string, callback: (event: unknown, packet: AgentStatePacket) => void) => {
            if (name === "agent:state-changed") channel = packet => callback(null, packet);
          },
          removeListener: () => {},
        } };
      },
    });
    const api = exports.agentApi!;
    const states: ClientState[] = [];
    // The context bridge copies every argument into the page world.
    const unsubscribe = subscribeAgentState({
      onState: api.onState,
      onStateUpdate: callback => api.onStateUpdate!(update => callback(structuredClone(update))),
    }, state => { states.push(state); });
    const document = { api, calls, states, channel, unsubscribe, get state() { return states.at(-1) ?? null; } };
    documents.push(document);
    return document;
  };
  // What a brand-new transport and document would show for the same store.
  const reference = () => new AgentStateImages(resources()).accept(new AgentStateTransport().snapshot(store.snapshot()));
  const normalize = (state: ClientState | null) => JSON.parse(JSON.stringify(state, (key, value) => key === "assetId" ? "<asset>" : value));
  const check = (document: { state: ClientState | null }, message?: string) => assert.deepEqual(normalize(document.state), normalize(reference()), message);
  return { store, transport, sent, open, check, drop: (value: boolean) => { dropping = value; } };
}

function signedIn(store: AxiomStateStore, account = "account-one") {
  store.initializeRuntime("runtime-one");
  store.setConnected(true);
  store.setAccount({ revision: store.snapshot().account ? store.snapshot().account!.revision + 1 : 1, state: "valid", account: { id: account, linkedMethods: [] } });
}
const chunk = (store: AxiomStateStore, sessionId: string, text: string, turn = "live") => {
  store.standardUpdate({ sessionId, update: { sessionUpdate: "agent_message_chunk", messageId: `assistant:${turn}`, content: { type: "text", text } } });
  store.flush();
};

test("the page follows every store change through patches and keeps untouched sessions identical", () => {
  const f = pipeline(), document = f.open();
  signedIn(f.store);
  for (const id of ["a", "b", "c"]) { f.store.addSession(id, "/tmp/chat"); f.store.replaceTimeline([page(id, 60)]); }
  f.store.setCatalog(["a", "b", "c", "d"].map(id => page(id, 60).thread));
  f.store.flush();
  f.check(document, "initial state");
  assert.ok(f.sent[0]!.state, "a new window first receives the whole projection");

  const before = document.state!;
  const packets = f.sent.length;
  chunk(f.store, "b", "streamed ");
  f.check(document, "one streamed chunk");
  const after = document.state!;
  assert.equal(after.sessions.a, before.sessions.a, "an untouched session is the same object in the page");
  assert.equal(after.sessions.c, before.sessions.c);
  assert.equal(after.catalog, before.catalog);
  assert.equal(after.sessions.b!.timeline[0], before.sessions.b!.timeline[0], "rows that did not change keep their identity too");
  assert.notEqual(after.sessions.b, before.sessions.b);
  const delta = f.sent.at(-1)!;
  assert.equal(f.sent.length, packets + 1);
  assert.equal(delta.state, undefined);
  assert.ok(serialize(delta).byteLength < 1024, `a chunk costs ${serialize(delta).byteLength} bytes on the wire, not the whole state`);

  // A burst within one turn of the event loop is published once.
  f.store.standardUpdate({ sessionId: "b", update: { sessionUpdate: "agent_message_chunk", messageId: "assistant:live", content: { type: "text", text: "one " } } });
  f.store.standardUpdate({ sessionId: "b", update: { sessionUpdate: "agent_message_chunk", messageId: "assistant:live", content: { type: "text", text: "two " } } });
  f.store.setModes("c", "confirm", [{ id: "web", name: "Web" }, { id: "confirm", name: "Ask" }]);
  assert.equal(f.sent.length, packets + 1, "nothing is sent until the burst ends");
  f.store.flush();
  assert.equal(f.sent.length, packets + 2);
  f.check(document, "coalesced burst");
  assert.equal(document.state!.sessions.a, before.sessions.a);

  // Authoritative refresh with identical rows changes almost nothing.
  const rows = document.state!.sessions.a!.timeline;
  f.store.replaceTimeline([page("a", 60)]); f.store.flush();
  f.check(document, "metadata-only refresh");
  assert.equal(document.state!.sessions.a!.timeline[10], rows[10]);

  // Every remaining kind of change, including ones that touch all sessions.
  const steps: (() => void)[] = [
    () => f.store.replaceTimeline([page("a", 62, 70)]),
    () => f.store.standardUpdate({ sessionId: "c", update: { sessionUpdate: "tool_call", toolCallId: "tool-1", title: "generate_image", status: "in_progress" } }),
    () => f.store.standardUpdate({ sessionId: "c", update: { sessionUpdate: "tool_call_update", toolCallId: "tool-1", status: "completed",
      rawOutput: { imageResource: { mimeType: "image/png", width: 1024, height: 1024, callId: "tool-1", sha256: sha(1) } } } }),
    () => f.store.standardUpdate({ sessionId: "c", update: { sessionUpdate: "tool_call_update", toolCallId: "tool-1", status: "completed",
      rawOutput: { imageResource: { mimeType: "image/png", width: 1024, height: 1024, callId: "tool-1", sha256: sha(2) } } } }),
    () => f.store.setRunning("b", true),
    () => f.store.extensionEvent({ runtimeInstanceId: "runtime-one", sequence: 1, sessionId: "b", event: { kind: "active_turn_changed", turnId: "live" } } as never),
    () => f.store.extensionEvent({ runtimeInstanceId: "runtime-one", sequence: 2, sessionId: null, event: { kind: "collections_changed", state: { revision: 3, collections: [] } } } as never),
    () => f.store.addInteraction({ id: "ask", requestId: 1, sessionId: "a", kind: "permission", payload: { sessionId: "a" } }),
    () => f.store.removeInteraction("ask"),
    () => f.store.setDesktopMcp({ revision: 2, servers: [], selectedTools: [] } as never, "a"),
    () => f.store.applyThreadRename({ ...page("b", 60).thread, title: "Renamed", revision: 900 }),
    () => f.store.standardUpdate({ sessionId: "d", update: { sessionUpdate: "session_info_update", title: "Background title" } }),
    () => f.store.setDiagnostic("stderr line"),
    () => f.store.setSecurityVerification("a", { pending: true }),
    () => f.store.extensionEvent({ runtimeInstanceId: "runtime-one", sequence: 9, sessionId: null, event: { kind: "profile_preferences_changed", preferences: { model: "model", thinkingLevel: "high" } } } as never),
    () => f.store.removeSession("c"),
    () => f.store.setCatalog(["a", "b"].map(id => ({ ...page(id, 60).thread, revision: 950, title: `Listed ${id}` }))),
    () => f.store.setConnected(false, "sidecar exited"),
  ];
  for (const [index, step] of steps.entries()) { step(); f.store.flush(); f.check(document, `step ${index}`); }
  assert.equal(document.calls.length, 0, "an in-order document never needs a snapshot");
});

test("account and runtime changes replace the document state; they are never patched onto the previous one", () => {
  const f = pipeline(), document = f.open();
  signedIn(f.store);
  f.store.addSession("a", "/tmp/chat"); f.store.replaceTimeline([page("a", 4)]); f.store.flush();
  const generation = f.sent.at(-1)!.generation;
  f.store.setAccount({ revision: 5, state: "valid", account: { id: "account-two", linkedMethods: [] } });
  const switched = f.sent.at(-1)!;
  assert.ok(switched.state, "a new account context is a complete packet");
  assert.equal(switched.patch, undefined);
  assert.ok(switched.generation > generation);
  assert.deepEqual(Object.keys(document.state!.sessions), []);
  f.check(document);
  f.store.extensionEvent({ runtimeInstanceId: "runtime-two", sequence: 1, sessionId: null, event: { kind: "collections_changed", state: { revision: 1, collections: [] } } } as never);
  f.check(document);
});

test("a missed packet or a reloaded document recovers from one snapshot and replays what arrived meanwhile", async () => {
  const f = pipeline(), document = f.open();
  signedIn(f.store);
  for (const id of ["a", "b"]) { f.store.addSession(id, "/tmp/chat"); f.store.replaceTimeline([page(id, 20)]); }
  f.store.flush();
  f.check(document);

  f.drop(true); chunk(f.store, "a", "lost "); f.drop(false);
  const stale = document.state;
  chunk(f.store, "a", "after the gap ");
  chunk(f.store, "b", "also waiting ");
  assert.equal(document.state, stale, "patches made from a revision the document does not hold are not applied");
  assert.deepEqual(document.calls, ["agent:state"], "exactly one snapshot is requested for the gap");
  // More changes arrive before the snapshot reply is processed.
  chunk(f.store, "b", "sent after the snapshot was taken ");
  await new Promise(resolve => setImmediate(resolve));
  f.check(document, "recovered");
  assert.deepEqual(document.calls, ["agent:state"]);
  chunk(f.store, "a", "and patches resume ");
  f.check(document);
  assert.deepEqual(document.calls, ["agent:state"]);

  // A reload keeps the window's transport but starts an empty document.
  const reloaded = f.open();
  chunk(f.store, "a", "first event after reload ");
  assert.equal(reloaded.state, null);
  await new Promise(resolve => setImmediate(resolve));
  f.check(reloaded, "reloaded document");
  f.check(document, "the other document is unaffected");
  chunk(f.store, "b", "both follow ");
  f.check(reloaded); f.check(document);
  assert.deepEqual(reloaded.calls, ["agent:state"]);
});

test("getState keeps subscribers in step and a hint from another emission stream is not trusted", async () => {
  const f = pipeline(), document = f.open();
  signedIn(f.store);
  f.store.addSession("a", "/tmp/chat"); f.store.replaceTimeline([page("a", 8)]); f.store.flush();
  f.drop(true); chunk(f.store, "a", "unseen "); f.drop(false);
  // The page asks for state itself (as the renderer does while submitting).
  const direct = await document.api.getState();
  assert.equal(direct.sessions.a!.timeline.at(-1)!.text, "unseen ");
  f.check(document, "subscribers received the same advance");
  chunk(f.store, "a", "next ");
  f.check(document);

  // A transport that did not encode the previous emission compares everything.
  const late = new AgentStateTransport(), preload = new AgentStateImages(resources());
  const changes: [ClientState, StateChange][] = [];
  f.store.on("change", (state: ClientState, change: StateChange) => changes.push([state, change]));
  chunk(f.store, "a", "one ");
  preload.accept(late.snapshot(changes[0]![0]));
  f.store.addSession("b", "/tmp/chat"); f.store.flush();   // emission the late transport never sees
  chunk(f.store, "a", "two ");
  const [state, change] = changes.at(-1)!;
  assert.deepEqual(change.sessions, ["a"]);
  const visible = preload.accept(late.encode(state, change));
  assert.ok(visible.sessions.b, "the session added by the unseen emission is still delivered");
  assert.deepEqual(visible, new AgentStateImages(resources()).accept(new AgentStateTransport().snapshot(state)));
});

test("inline image bytes travel with the patch that introduces them and survive a waiting patch", () => {
  const png = (seed: number) => {
    const raw = Buffer.alloc(2048, seed);
    Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]).copy(raw);
    raw.writeUInt32BE(1024, 16); raw.writeUInt32BE(1024, 20);
    return raw.toString("base64");
  };
  const state = (images: number[], sequence: number): ClientState => ({
    connected: true, runtimeInstanceId: "native", lastSequence: sequence,
    account: { revision: 1, state: "valid", account: { id: "one", linkedMethods: [] } },
    sessions: { thread: { sessionId: "thread", title: null, cwd: "", settings: null, security: null, modes: [], currentModeId: null,
      configOptions: [], interactions: [], running: false, needsResync: false, threadRevision: 1, lastTimelineSequence: 1,
      timeline: images.map(seed => ({ id: `image-${seed}`, kind: "tool" as const, text: "",
        tool: { callId: `image-${seed}`, title: "Image", kind: "other", status: "completed", input: {}, locations: [],
          content: [{ type: "image", mimeType: "image/png", data: png(seed) }] } })) } },
    catalog: [], collections: { revision: 0, collections: [] }, preferences: null, billing: null, diagnostic: "", error: null,
  });
  const url = (visible: ClientState, index: number) => (visible.sessions.thread!.timeline[index]!.tool!.content[0] as { imageUrl?: string }).imageUrl;
  const transport = new AgentStateTransport();
  const created: string[] = [], revoked: string[] = [];
  const preload = new AgentStateImages({ create: () => { created.push(`blob:${created.length}`); return created.at(-1)!; }, revoke: value => { revoked.push(value); } });
  const first = preload.accept(transport.encode(state([1], 1)));
  const second = transport.encode(state([1, 2], 2));
  assert.equal(second.state, undefined);
  assert.equal(second.images.length, 1, "only the new image is carried");
  const third = transport.encode(state([1, 2], 3));
  assert.equal(third.images.length, 0);
  // The later patch arrives first and waits; the earlier one brings the bytes.
  assert.equal(preload.accept(third), first);
  assert.equal(preload.needsSnapshot, true);
  const visible = preload.accept(second);
  assert.equal(preload.needsSnapshot, false);
  assert.equal(visible.lastSequence, 3);
  assert.equal(url(visible, 0), url(first, 0), "an existing image keeps its URL");
  assert.match(String(url(visible, 1)), /^blob:/);
  assert.deepEqual(revoked, []);
  // Removing a row releases exactly its resource.
  const removed = preload.accept(transport.encode(state([2], 4)));
  assert.deepEqual(revoked, [url(first, 0)]);
  assert.equal(url(removed, 0), url(visible, 1));
});
