import assert from "node:assert/strict";
import test from "node:test";
import { AxiomStateStore, type GetThreadTimelineResponse } from "../src/index.js";

function page(revision: number, text = "", kind: "assistant_message" | "reasoning" = "assistant_message"): GetThreadTimelineResponse {
  return {
    thread: {
      threadId: "thread", title: "Test", cwd: "/tmp", origin: "acp", profile: "web",
      selectedModel: "model", thinkingLevel: "medium", lifecycle: "running", archived: false,
      revision, lastTimelineSequence: 1,
      createdAt: "2026-09-07T00:00:00Z", updatedAt: "2026-09-07T00:00:00Z",
    },
    items: [{
      id: "durable-id", threadId: "thread", turnId: "turn", sequence: 1,
      kind, status: "in_progress", content: text, metadata: {},
      createdAt: "2026-09-07T00:00:00Z", updatedAt: "2026-09-07T00:00:00Z",
    }],
  };
}

function delta(revision: number, text: string, kind = "assistant", turn = "turn") {
  return {
    sessionId: "thread",
    _meta: { axiom: { threadRevision: revision, lastTimelineSequence: 1 } },
    update: {
      sessionUpdate: kind === "assistant" ? "agent_message_chunk" : "agent_thought_chunk",
      messageId: `${kind}:${turn}`, content: { type: "text", text },
    },
  };
}

function toolUpdate(revision: number, callId: string, status = "pending") {
  return {
    sessionId: "thread", _meta: { axiom: { threadRevision: revision, lastTimelineSequence: 1 } },
    update: {
      sessionUpdate: status === "pending" ? "tool_call" : "tool_call_update",
      toolCallId: callId, status,
      ...(status === "pending" ? { title: "Searching the web for news · web_search", kind: "search", rawInput: { query: "news" } } : {}),
    },
  };
}

const report = (inputTokens: number) => ({ inputTokens, outputTokens: 10, modelId: "model", reportedAt: "2026-09-07T00:00:00Z", contextWindowTokens: 100_000, autoCompactThresholdTokens: 85_000 });

test("late generated titles update the sidebar without replacing chat or recency", () => {
  const store = new AxiomStateStore();
  const snapshot = page(1, "The answer");
  store.setCatalog([snapshot.thread]);
  store.replaceTimeline([snapshot]);
  store.standardUpdate({
    sessionId: "thread",
    _meta: { axiom: { threadRevision: 2, lastTimelineSequence: 1 } },
    update: { sessionUpdate: "session_info_update", title: "Generated title" },
  });
  assert.equal(store.snapshot().catalog[0]!.title, "Generated title");
  assert.equal(store.snapshot().sessions.thread!.title, "Generated title");
  assert.equal(store.snapshot().sessions.thread!.timeline[0]!.text, "The answer");
  assert.equal(store.snapshot().catalog[0]!.updatedAt, snapshot.thread.updatedAt);
  store.applyThreadRename({ ...snapshot.thread, revision: 4, title: "My manual title" });
  store.standardUpdate({
    sessionId: "thread",
    _meta: { axiom: { threadRevision: 3, lastTimelineSequence: 1 } },
    update: { sessionUpdate: "session_info_update", title: "Stale generated title" },
  });
  assert.equal(store.snapshot().catalog[0]!.title, "My manual title");
  assert.equal(store.snapshot().sessions.thread!.title, "My manual title");
});

test("steering identity survives snapshots and separates same-turn assistant segments", () => {
  const store = new AxiomStateStore();
  store.replaceTimeline([{ ...page(1, "Before the update."), activeTurnId: "turn" }]);
  assert.equal(store.snapshot().sessions.thread!.activeTurnId, "turn");
  store.standardUpdate({
    sessionId: "thread",
    _meta: { axiom: { threadRevision: 2, lastTimelineSequence: 2, timelineItemId: "steered-user", clientItemId: "input-id" } },
    update: { sessionUpdate: "user_message_chunk", messageId: "input-id", content: { type: "text", text: "New direction" } },
  });
  const continuation = delta(3, "After the update.");
  Object.assign(continuation._meta.axiom, { timelineItemId: "continued-answer", lastTimelineSequence: 3 });
  store.standardUpdate(continuation);
  const rows = store.snapshot().sessions.thread!.timeline;
  assert.deepEqual(rows.map((row) => row.text), ["Before the update.", "New direction", "After the update."]);
  assert.equal(rows[1]!.clientItemId, "input-id");
  assert.notEqual(rows[0]!.id, rows[2]!.id);
  store.extensionEvent({ runtimeInstanceId: "runtime", sessionId: "thread", sequence: 1,
    threadRevision: 4, occurredAt: "2026-09-07T00:00:00Z", event: { kind: "active_turn_changed", turnId: null } });
  assert.equal(store.snapshot().sessions.thread!.activeTurnId, null);
  store.setRunning("thread", false);
  assert.equal(store.snapshot().sessions.thread!.running, false);
});

test("last reported usage replaces rather than accumulates and survives compaction and resync", () => {
  const store = new AxiomStateStore();
  store.replaceTimeline([{ ...page(1), contextUsage: report(200) }]);
  const event = (sequence: number, inputTokens: number) => ({
    runtimeInstanceId: "runtime", sessionId: "thread", sequence, threadRevision: sequence,
    occurredAt: "2026-09-07T00:00:00Z",
    event: { kind: "context_usage_changed" as const, usage: report(inputTokens) },
  });
  store.extensionEvent(event(2, 10_000));
  store.extensionEvent(event(3, 12_000));
  assert.equal(store.snapshot().sessions.thread!.contextUsage?.inputTokens, 12_000);
  store.extensionEvent({ ...event(4, 0), event: { kind: "compaction", phase: "completed", detail: "Compacted" } });
  store.setRunning("thread", true);
  store.extensionEvent(event(3, 12_000)); // delayed delivery cannot restore old usage
  assert.equal(store.snapshot().sessions.thread!.contextUsage?.inputTokens, 12_000);
  assert.equal(store.replaceTimeline([{ ...page(2), contextUsage: report(10_000) }]), false);
  assert.equal(store.snapshot().sessions.thread!.contextUsage?.inputTokens, 12_000);
  store.replaceTimeline([{ ...page(5), contextUsage: report(12_000) }]);
  assert.deepEqual(store.snapshot().sessions.thread!.contextUsage, report(12_000));
  assert.equal(store.snapshot().sessions.thread!.timeline.length, 1); // no telemetry transcript rows
  store.extensionEvent({ ...event(5, 30), sessionId: "another-thread" });
  assert.equal(store.snapshot().sessions.thread!.contextUsage?.inputTokens, 12_000);
  assert.equal(store.snapshot().sessions["another-thread"]!.contextUsage?.inputTokens, 30);
  store.extensionEvent(event(6, 500)); // first model request after compaction
  assert.equal(store.snapshot().sessions.thread!.contextUsage?.inputTokens, 500);
  const restored = new AxiomStateStore();
  restored.replaceTimeline([{ ...page(6), contextUsage: report(500) }]);
  assert.deepEqual(restored.snapshot().sessions.thread!.contextUsage, report(500));
  store.setAccount({ revision: 1, state: "valid", account: { id: "one", linkedMethods: [] } });
  store.setAccount({ revision: 2, state: "valid", account: { id: "two", linkedMethods: [] } });
  assert.deepEqual(store.snapshot().sessions, {});
});

test("invalid, legacy estimated, or unavailable telemetry never becomes a provider report", () => {
  const store = new AxiomStateStore();
  for (const inputTokens of [-1, NaN, Infinity, Number.MAX_SAFE_INTEGER]) {
    store.replaceTimeline([{ ...page(1), contextUsage: report(inputTokens) }]);
    assert.equal(store.snapshot().sessions.thread!.contextUsage, null);
  }
  store.replaceTimeline([page(1)]);
  assert.equal(store.snapshot().sessions.thread!.contextUsage, null);
  for (const invalid of [
    { usedTokens: 10_000, estimated: false },
    { ...report(100), modelId: "" },
    { ...report(100), reportedAt: "not a time" },
    { ...report(100), contextWindowTokens: 0 },
    { ...report(100), autoCompactThresholdTokens: 100_001 },
  ]) {
    store.replaceTimeline([{ ...page(1), contextUsage: invalid as ReturnType<typeof report> }]);
    assert.equal(store.snapshot().sessions.thread!.contextUsage, null);
  }
});

test("standard ACP interleaves multiple text/tool rounds within one turn", () => {
  const store = new AxiomStateStore();
  store.standardUpdate(delta(1, "Let me check: "));
  store.standardUpdate(toolUpdate(2, "one"));
  store.standardUpdate(toolUpdate(3, "one", "completed"));
  store.standardUpdate(delta(4, "A source. "));
  store.standardUpdate(delta(5, "Checking another."));
  store.standardUpdate(toolUpdate(6, "two"));
  store.standardUpdate(toolUpdate(7, "two", "completed"));
  store.standardUpdate(delta(8, "The answer."));
  const rows = store.snapshot().sessions.thread!.timeline;
  assert.deepEqual(rows.map((item) => item.kind), ["assistant", "tool", "assistant", "tool", "assistant"]);
  assert.deepEqual(rows.filter((item) => item.kind === "assistant").map((item) => item.text), ["Let me check: ", "A source. Checking another.", "The answer."]);
  assert.equal(new Set(rows.map((item) => item.id)).size, rows.length);
  assert.ok(rows.every((item) => item.turnId === "turn"));
});

test("native segment IDs survive in-flight snapshots and late tool updates do not duplicate rows", () => {
  const store = new AxiomStateStore();
  const initial = page(1, "Let me check.");
  const template = initial.items[0]!;
  initial.items.push({ ...template, id: "durable-tool", externalId: "call-1", sequence: 2,
    kind: "tool_call", status: "pending", content: "", metadata: { name: "web_search", arguments: { query: "news" } } });
  initial.thread.lastTimelineSequence = 2;
  store.replaceTimeline([initial]);
  store.standardUpdate(toolUpdate(2, "call-1", "in_progress"));
  assert.equal(store.snapshot().sessions.thread!.timeline.length, 2);
  store.standardUpdate(toolUpdate(3, "call-1", "completed"));
  const first = delta(4, "Current ");
  Object.assign(first._meta.axiom, { timelineItemId: "after-tool", lastTimelineSequence: 3 });
  store.standardUpdate(first);
  const snapshot = structuredClone(initial);
  snapshot.thread.revision = 4;
  snapshot.thread.lastTimelineSequence = 3;
  snapshot.items[1]!.status = "completed";
  snapshot.items.push({ ...template, id: "after-tool", sequence: 3, content: "Current " });
  store.replaceTimeline([snapshot]);
  store.standardUpdate(first); // already included in the snapshot
  const second = delta(5, "news.");
  Object.assign(second._meta.axiom, { timelineItemId: "after-tool", lastTimelineSequence: 3 });
  store.standardUpdate(second);
  store.standardUpdate(second); // duplicate delivery
  const rows = store.snapshot().sessions.thread!.timeline;
  assert.deepEqual(rows.map((item) => item.id), ["durable-id", "durable-tool", "after-tool"]);
  assert.deepEqual(rows.map((item) => item.text), ["Let me check.", "web_search", "Current news."]);
  assert.equal(rows[1]?.tool?.callId, "call-1");
  assert.equal(rows[2]?.terminalVerified, false);
});

test("reasoning after a tool stays with the new response segment", () => {
  const store = new AxiomStateStore();
  store.standardUpdate(delta(1, "First thought", "reasoning"));
  store.standardUpdate(delta(2, "Checking"));
  store.standardUpdate(toolUpdate(3, "search"));
  store.standardUpdate(toolUpdate(4, "search", "completed"));
  store.standardUpdate(delta(5, "Second thought", "reasoning"));
  store.standardUpdate(delta(6, "Answer"));
  assert.deepEqual(store.snapshot().sessions.thread!.timeline.map((item) => item.kind), [
    "reasoning", "assistant", "tool", "reasoning", "assistant",
  ]);
});

test("live assistant and reasoning deltas reuse their durable row identity", () => {
  for (const kind of ["assistant", "reasoning"]) {
    const store = new AxiomStateStore();
    store.replaceTimeline([page(1, "", kind === "assistant" ? "assistant_message" : "reasoning")]);
    store.standardUpdate(delta(2, "first ", kind));
    store.standardUpdate(delta(3, "second", kind));
    const rows = store.snapshot().sessions.thread!.timeline;
    assert.equal(rows.length, 1);
    assert.equal(rows[0]?.id, "durable-id");
    assert.equal(rows[0]?.text, "first second");
    assert.equal(rows[0]?.status, "streaming");
    assert.equal(rows[0]?.terminalVerified, false);
  }
});

test("message activity comes from native timestamps, not load or notification arrival times", () => {
  const store = new AxiomStateStore();
  const snapshot = page(1, "old answer");
  snapshot.thread.lastMessageAt = "2026-01-01T00:00:00.000Z";
  store.replaceTimeline([snapshot]);
  assert.equal(store.snapshot().sessions.thread?.lastMessageAt, snapshot.thread.lastMessageAt);
  const message = delta(2, "more");
  Object.assign(message._meta.axiom, { lastMessageAt: "2026-01-02T00:00:00.000Z" });
  store.standardUpdate(message);
  // An older catalog response must not overwrite the newer live session.
  store.setCatalog([snapshot.thread]);
  assert.equal(store.snapshot().sessions.thread?.lastMessageAt, "2026-01-02T00:00:00.000Z");
  store.standardUpdate({
    sessionId: "thread", _meta: { axiom: { threadRevision: 3, lastTimelineSequence: 1, lastMessageAt: "2026-01-02T00:00:00.000Z" } },
    update: { sessionUpdate: "agent_thought_chunk", messageId: "provider:1", content: { type: "text", text: "[provider] verified" } },
  });
  assert.equal(store.snapshot().sessions.thread?.lastMessageAt, "2026-01-02T00:00:00.000Z");
  store.standardUpdate(message); // delayed duplicate
  assert.equal(store.snapshot().sessions.thread?.lastMessageAt, "2026-01-02T00:00:00.000Z");
});

test("snapshot replacement during a stream never duplicates text or placeholders", () => {
  const store = new AxiomStateStore();
  store.standardUpdate(delta(1, "first "));
  store.replaceTimeline([page(1, "first ")]);
  store.standardUpdate(delta(2, "second "));
  store.replaceTimeline([page(3, "first second third ")]);
  // An already-persisted chunk can arrive after its snapshot.
  store.standardUpdate(delta(3, "third "));
  store.standardUpdate(delta(4, "last"));
  store.standardUpdate(delta(4, "last"));
  const rows = store.snapshot().sessions.thread!.timeline;
  assert.equal(rows.length, 1);
  assert.equal(rows[0]?.text, "first second third last");
});

test("different turns never share the same assistant row", () => {
  const store = new AxiomStateStore();
  store.replaceTimeline([page(1, "earlier")]);
  store.standardUpdate(delta(2, "new", "assistant", "next-turn"));
  assert.deepEqual(store.snapshot().sessions.thread!.timeline.map((item) => item.text), ["earlier", "new"]);
});

test("additional output cannot inherit an earlier receipt marker", () => {
  const store = new AxiomStateStore();
  const verified = page(1, "first ");
  verified.items[0]!.metadata = { terminal_verified: true };
  verified.items[0]!.status = "completed";
  store.replaceTimeline([verified]);
  store.standardUpdate(delta(2, "more"));
  const reply = store.snapshot().sessions.thread!.timeline[0];
  assert.equal(reply?.text, "first more");
  assert.equal(reply?.terminalVerified, false);
});

test("a runtime restart clears stream revision tracking", () => {
  const store = new AxiomStateStore();
  const notification = {
    runtimeInstanceId: "old", sequence: 1, occurredAt: "2026-09-07T00:00:00Z",
    event: { kind: "background_task" as const, task_id: "test", state: "done" },
  };
  store.extensionEvent(notification);
  store.replaceTimeline([page(10, "old")]);
  store.extensionEvent({ ...notification, runtimeInstanceId: "new" });
  store.standardUpdate(delta(1, "new"));
  assert.equal(store.snapshot().sessions.thread!.timeline[0]?.text, "new");
});

test("session removal clears stream revision tracking", () => {
  const store = new AxiomStateStore();
  store.replaceTimeline([page(10, "old")]);
  store.removeSession("thread");
  store.standardUpdate(delta(1, "new"));
  assert.equal(store.snapshot().sessions.thread!.timeline[0]?.text, "new");
});

test("account switches clear stream revision tracking with the account-local timeline", () => {
  const store = new AxiomStateStore();
  store.setAccount({ revision: 1, state: "valid", account: { id: "one", linkedMethods: [] } });
  store.replaceTimeline([page(10, "old")]);
  store.setAccount({ revision: 2, state: "valid", account: { id: "two", linkedMethods: [] } });
  store.standardUpdate(delta(1, "new"));
  assert.equal(store.snapshot().sessions.thread!.timeline[0]?.text, "new");
});
