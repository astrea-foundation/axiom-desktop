import assert from "node:assert/strict";
import { once } from "node:events";
import { mkdtemp, mkdir, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { AxiomAcpClient, AxiomStateStore, type GetThreadTimelineResponse } from "../src/index.js";

const here = dirname(fileURLToPath(import.meta.url));
const binary = resolve(
  here,
  "../../../target/debug",
  process.platform === "win32" ? "axiomcli.exe" : "axiomcli",
);

async function fixture(runner = "echo"): Promise<{ client: AxiomAcpClient; workspace: string }> {
  const root = await mkdtemp(resolve(tmpdir(), "axiom-acp-client-"));
  const workspace = resolve(root, "workspace");
  await mkdir(workspace);
  await writeFile(resolve(workspace, "README.md"), "fixture\n");
  const client = new AxiomAcpClient({
    command: binary,
    args: ["acp"],
    env: {
      AXIOMCLI_TEST_RUNNER: runner,
      XDG_CONFIG_HOME: resolve(root, "config"),
      XDG_DATA_HOME: resolve(root, "data"),
    },
  });
  await client.initialize();
  return { client, workspace };
}

function timelinePage(revision = 1): GetThreadTimelineResponse {
  return {
    thread: {
      threadId: "session",
      title: "Thread",
      cwd: "/tmp",
      origin: "acp",
      profile: "web",
      selectedModel: "model",
      thinkingLevel: "medium",
      lifecycle: "ready",
      archived: false,
      revision,
      lastTimelineSequence: 1,
      createdAt: "2026-08-24T00:00:00Z",
      updatedAt: "2026-08-24T00:00:01Z",
    },
    items: [{
      id: "item-1",
      threadId: "session",
      turnId: "turn-1",
      sequence: 1,
      kind: "user_message",
      status: "completed",
      clientItemId: "client-1",
      content: "hello",
      metadata: {},
      createdAt: "2026-08-24T00:00:00Z",
      updatedAt: "2026-08-24T00:00:00Z",
    }],
  };
}

test("production client negotiates, streams, and controls standard ACP settings", async () => {
  const { client, workspace } = await fixture();
  try {
    const created = await client.newSession(workspace);
    const models = await client.listModels();
    assert.ok(models.models.some((model) => model.id === "alternate-model"));
    assert.ok(models.models.every((model) =>
      model.providerId && model.providerLabel && model.shortLabel && model.upstreamModel,
    ));
    await client.setSettings({
      threadId: created.sessionId,
      model: "alternate-model",
      thinkingLevel: "high",
    });
    await client.setMode(created.sessionId, "observe");
    const result = await client.prompt(created.sessionId, "hello from desktop", "client-message-1");
    assert.equal(result.stopReason, "end_turn");
    const state = client.getState().sessions[created.sessionId];
    assert.equal(state?.settings?.permissionProfile, "observe");
    assert.ok(state?.timeline.some((item) =>
      item.kind === "user"
      && item.text === "hello from desktop"
      && item.clientItemId === "client-message-1",
    ));
  } finally {
    await client.close();
  }
});

test("model changes keep the authoritative reasoning setting across disjoint capabilities", async () => {
  const { client, workspace } = await fixture();
  try {
    const created = await client.newSession(workspace);
    await client.setSettings({
      threadId: created.sessionId,
      model: "alternate-model",
      thinkingLevel: "xhigh",
    });

    const nonreasoning = await client.setSettings({
      threadId: created.sessionId,
      model: "grok-code",
      thinkingLevel: "xhigh",
    });
    assert.equal(nonreasoning.preferences.thinkingLevel, "xhigh");
    assert.equal(nonreasoning.settings.model, "grok-code");
    assert.equal(nonreasoning.settings.thinkingLevel, "xhigh");
    assert.ok(nonreasoning.configOptions.every((option) => option.id !== "thinking"));
    assert.equal(client.getState().sessions[created.sessionId]?.settings?.thinkingLevel, "xhigh");

    const reconciled = await client.setSettings({
      threadId: created.sessionId,
      model: "medium-only",
      thinkingLevel: "xhigh",
    });
    assert.equal(reconciled.preferences.model, "medium-only");
    assert.equal(reconciled.preferences.thinkingLevel, "medium");
    assert.equal(reconciled.settings.model, "medium-only");
    assert.equal(reconciled.settings.thinkingLevel, "medium");
    assert.equal(client.getState().sessions[created.sessionId]?.settings?.model, "medium-only");
    assert.equal(client.getState().sessions[created.sessionId]?.settings?.thinkingLevel, "medium");
  } finally {
    await client.close();
  }
});

test("a model change immediately invalidates a verified client security badge", () => {
  const store = new AxiomStateStore();
  store.addSession("session", "/tmp");
  store.extensionEvent({
    runtimeInstanceId: "runtime",
    sequence: 1,
    occurredAt: "2026-08-28T00:00:00Z",
    sessionId: "session",
    event: { kind: "security_changed", status: { state: "verified" } },
  });
  store.standardUpdate({
    sessionId: "session",
    update: {
      sessionUpdate: "config_option_update",
      configOptions: [{
        id: "model",
        name: "Model",
        currentValue: "different-model",
        options: [{ value: "different-model", name: "different-model" }],
      }],
    },
  });
  store.extensionEvent({
    runtimeInstanceId: "runtime",
    sequence: 2,
    occurredAt: "2026-08-28T00:00:01Z",
    sessionId: "session",
    event: { kind: "security_changed", status: { state: "unverified" } },
  });
  assert.equal(store.snapshot().sessions.session?.settings?.model, "different-model");
  assert.equal(store.snapshot().sessions.session?.security?.state, "unverified");
});

test("a terminal receipt failure revokes a previously verified client security badge", () => {
  const store = new AxiomStateStore();
  store.addSession("session", "/tmp");
  store.extensionEvent({
    runtimeInstanceId: "runtime",
    sequence: 1,
    occurredAt: "2026-08-28T00:00:00Z",
    sessionId: "session",
    event: { kind: "security_changed", status: { state: "verified" } },
  });
  store.extensionEvent({
    runtimeInstanceId: "runtime",
    sequence: 2,
    occurredAt: "2026-08-28T00:00:01Z",
    sessionId: "session",
    event: {
      kind: "security_changed",
      status: { state: "failed", detail: "missing signed terminal receipt" },
    },
  });
  assert.equal(store.snapshot().sessions.session?.security?.state, "failed");
});

test("session preflight never upgrades a response to terminal-verified evidence", () => {
  const store = new AxiomStateStore();
  const page = timelinePage(2);
  page.thread.lastTimelineSequence = 2;
  page.items.push({
    id: "assistant-1",
    threadId: "session",
    turnId: "turn-1",
    sequence: 2,
    kind: "assistant_message",
    status: "failed",
    content: "provisional output",
    metadata: {},
    createdAt: "2026-08-24T00:00:00Z",
    updatedAt: "2026-08-24T00:00:01Z",
  });
  store.replaceTimeline([page]);
  store.extensionEvent({
    runtimeInstanceId: "runtime",
    sequence: 1,
    occurredAt: "2026-08-28T00:00:00Z",
    sessionId: "session",
    event: { kind: "security_changed", status: { state: "verified" } },
  });

  const assistant = store.snapshot().sessions.session?.timeline[1];
  assert.equal(assistant?.status, "failed");
  assert.equal(assistant?.terminalVerified, false);
  assert.equal(store.snapshot().sessions.session?.security?.state, "verified");
});

test("only row-bound terminal evidence marks an individual response verified", () => {
  const store = new AxiomStateStore();
  const page = timelinePage(2);
  page.thread.lastTimelineSequence = 2;
  page.items.push({
    id: "assistant-1",
    threadId: "session",
    turnId: "turn-1",
    sequence: 2,
    kind: "assistant_message",
    status: "completed",
    content: "verified output",
    metadata: { terminal_verified: true },
    createdAt: "2026-08-24T00:00:00Z",
    updatedAt: "2026-08-24T00:00:01Z",
  });
  store.replaceTimeline([page]);
  assert.equal(store.snapshot().sessions.session?.timeline[1]?.terminalVerified, true);
});

test("ending a request does not relabel an unrefreshed streaming item successful", () => {
  const store = new AxiomStateStore();
  store.addSession("session", "/tmp");
  store.standardUpdate({
    sessionId: "session",
    update: {
      sessionUpdate: "agent_message_chunk",
      messageId: "assistant:turn-1",
      content: { type: "text", text: "partial" },
    },
  });
  store.setRunning("session", true);
  store.setRunning("session", false);
  assert.equal(store.snapshot().sessions.session?.timeline[0]?.status, "streaming");
});

test("permission requests remain owned by AxiomCLI and resolve through the client", async () => {
  const { client, workspace } = await fixture("approval");
  try {
    const created = await client.newSession(workspace);
    client.once("interaction", (interaction) => {
      assert.equal(interaction.kind, "permission");
      client.resolvePermission(interaction.id, { outcome: "selected", optionId: "allow_once" });
    });
    const result = await client.prompt(created.sessionId, "approve fixture", "client-approval");
    assert.equal(result.stopReason, "end_turn");
    assert.equal(client.getState().sessions[created.sessionId]?.interactions.length, 0);
  } finally {
    await client.close();
  }
});

test("cancelling fail-closes and clears a pending permission", async () => {
  const { client, workspace } = await fixture("approval");
  try {
    const created = await client.newSession(workspace);
    const interaction = once(client, "interaction");
    const prompt = client.prompt(created.sessionId, "cancel approval fixture", "client-cancel");
    const [pending] = await interaction;
    assert.equal(pending.kind, "permission");
    await client.cancel(created.sessionId);
    const result = await prompt;
    assert.ok(["cancelled", "end_turn"].includes(result.stopReason));
    assert.equal(client.getState().sessions[created.sessionId]?.interactions.length, 0);
  } finally {
    await client.close();
  }
});

test("revision gaps are repaired by authoritative timeline replacement", () => {
  const store = new AxiomStateStore();
  store.addSession("session", "/tmp");
  store.replaceTimeline([timelinePage(1)]);
  const resync = store.standardUpdate({
    sessionId: "session",
    _meta: { axiom: { threadRevision: 3, lastTimelineSequence: 2 } },
    update: {
      sessionUpdate: "agent_message_chunk",
      messageId: "assistant:turn-2",
      content: { type: "text", text: "missed something" },
    },
  });
  assert.equal(resync, "session");
  assert.equal(store.snapshot().sessions.session?.needsResync, true);
  store.replaceTimeline([timelinePage(3)]);
  assert.equal(store.snapshot().sessions.session?.needsResync, false);
  assert.equal(store.snapshot().sessions.session?.timeline.length, 1);
});

test("catalog summaries neither masquerade as loaded sessions nor roll live state backward", () => {
  const store = new AxiomStateStore();
  const stale = timelinePage(1).thread;
  store.setCatalog([stale]);
  assert.equal(store.snapshot().sessions.session, undefined);

  store.replaceTimeline([timelinePage(3)]);
  store.setCatalog([{ ...stale, title: "stale title" }]);
  const loaded = store.snapshot().sessions.session;
  assert.equal(loaded?.threadRevision, 3);
  assert.equal(loaded?.title, "Thread");
});

test("a global extension sequence gap marks every loaded thread for authoritative resync", () => {
  const store = new AxiomStateStore();
  store.replaceTimeline([timelinePage(1)]);
  const first = store.extensionEvent({
    runtimeInstanceId: "runtime-1",
    sequence: 1,
    occurredAt: "2026-08-24T00:00:02Z",
    event: { kind: "background_task", task_id: "one", state: "running" },
  });
  assert.equal(first?.fullResync, false);
  const gap = store.extensionEvent({
    runtimeInstanceId: "runtime-1",
    sequence: 3,
    occurredAt: "2026-08-24T00:00:03Z",
    event: { kind: "background_task", task_id: "two", state: "complete" },
  });
  assert.equal(gap?.fullResync, true);
  assert.equal(store.snapshot().sessions.session?.needsResync, true);
});

test("stable client item identity reconciles optimistic user messages", () => {
  const store = new AxiomStateStore();
  store.addSession("session", "/tmp");
  store.standardUpdate({
    sessionId: "session",
    _meta: {
      axiom: {
        threadRevision: 1,
        lastTimelineSequence: 2,
        clientItemId: "client-stable",
      },
    },
    update: {
      sessionUpdate: "user_message_chunk",
      messageId: "client-stable",
      content: { type: "text", text: "visible while verifying" },
    },
  });
  const user = store.snapshot().sessions.session?.timeline[0];
  assert.equal(user?.clientItemId, "client-stable");
  assert.equal(user?.text, "visible while verifying");
});

test("tool lifecycle updates preserve input and produce one tool row", () => {
  const store = new AxiomStateStore();
  store.addSession("session", "/tmp");
  store.standardUpdate({
    sessionId: "session",
    update: {
      sessionUpdate: "tool_call",
      toolCallId: "call-1",
      title: "Searching the web · web_search",
      kind: "search",
      status: "pending",
      rawInput: { query: "Axiom" },
    },
  });
  store.standardUpdate({
    sessionId: "session",
    update: {
      sessionUpdate: "tool_call_update",
      toolCallId: "call-1",
      status: "completed",
      content: [{ type: "text", text: "result" }],
    },
  });
  const rows = store.snapshot().sessions.session?.timeline ?? [];
  assert.equal(rows.length, 1);
  assert.deepEqual(rows[0]?.tool?.input, { query: "Axiom" });
  assert.equal(rows[0]?.tool?.status, "completed");
});

test("authoritative tool replacement preserves input, output, and terminal status once", () => {
  const store = new AxiomStateStore();
  const page = timelinePage(2);
  page.thread.lastTimelineSequence = 2;
  page.items.push({
    id: "tool-row",
    threadId: "session",
    turnId: "turn-1",
    sequence: 2,
    kind: "tool_call",
    status: "failed",
    externalId: "call-1",
    content: "bounded durable output",
    metadata: {
      name: "web_search",
      arguments: { query: "Axiom" },
      content: [{ type: "text", text: "bounded durable output" }],
      output_truncated: false,
    },
    createdAt: "2026-08-24T00:00:00Z",
    updatedAt: "2026-08-24T00:00:01Z",
  });
  store.replaceTimeline([page]);
  const tool = store.snapshot().sessions.session?.timeline[1]?.tool;
  assert.deepEqual(tool?.input, { query: "Axiom" });
  assert.equal(tool?.status, "failed");
  assert.deepEqual(tool?.content, [{ type: "text", text: "bounded durable output" }]);
});

test("desktop bootstrap owns chat cwd and fixes the web permission profile", async () => {
  const root = await mkdtemp(resolve(tmpdir(), "axiom-desktop-client-"));
  const client = new AxiomAcpClient({
    command: binary,
    args: ["acp", "--frontend", "desktop-chat"],
    env: {
      AXIOMCLI_TEST_RUNNER: "echo",
      XDG_CONFIG_HOME: resolve(root, "config"),
      XDG_DATA_HOME: resolve(root, "data"),
    },
  });
  try {
    await client.initialize();
    const bootstrap = await client.bootstrapDesktop();
    assert.equal(bootstrap.frontend, "desktop-chat");
    assert.equal(bootstrap.permissionProfile, "web");
    assert.equal(bootstrap.newThreadSettings.thinkingLevel, "medium");
    assert.ok(bootstrap.chatCwd.endsWith(join("axiom", "desktop", "chat")));
    const created = await client.newChat();
    const session = client.getState().sessions[created.sessionId];
    assert.equal(session?.cwd, bootstrap.chatCwd);
    assert.equal(session?.currentModeId, "web");
    assert.deepEqual(session?.modes.map((mode) => mode.id), ["web"]);
    await assert.rejects(client.setMode(created.sessionId, "observe"), /fixed to web/);
    await client.prompt(created.sessionId, "hello from desktop profile", "desktop-user-1");
    assert.ok(client.getState().sessions[created.sessionId]?.timeline.some(
      (item) => item.kind === "user" && item.clientItemId === "desktop-user-1",
    ));
  } finally {
    await client.close();
  }
});

test("timeline and collections survive a sidecar restart", async () => {
  const root = await mkdtemp(resolve(tmpdir(), "axiom-desktop-replay-"));
  const options = {
    command: binary,
    args: ["acp", "--frontend", "desktop-chat"],
    env: {
      AXIOMCLI_TEST_RUNNER: "echo",
      XDG_CONFIG_HOME: resolve(root, "config"),
      XDG_DATA_HOME: resolve(root, "data"),
    },
  };
  const first = new AxiomAcpClient(options);
  let sessionId = "";
  let collectionId = "";
  try {
    await first.initialize();
    await first.bootstrapDesktop();
    sessionId = (await first.newChat()).sessionId;
    await first.setSettings({ threadId: sessionId, thinkingLevel: "xhigh" });
    const createdCollection = await first.createCollection("Research");
    collectionId = createdCollection.state.collections[0]?.id ?? "";
    assert.ok(collectionId);
    await first.assignThreadCollection(sessionId, collectionId);
    await first.prompt(sessionId, "persist this user turn", "persistent-client-id");
  } finally {
    await first.close();
  }

  const second = new AxiomAcpClient(options);
  try {
    await second.initialize();
    const restoredBootstrap = await second.bootstrapDesktop();
    assert.equal(restoredBootstrap.newThreadSettings.thinkingLevel, "xhigh");
    await second.initializeDesktopState();
    assert.ok(second.getState().collections.collections.some((collection) =>
      collection.id === collectionId && collection.threadIds.includes(sessionId),
    ));
    await second.loadChat(sessionId);
    const userMessages = second.getState().sessions[sessionId]?.timeline.filter(
      (item) => item.kind === "user" && item.clientItemId === "persistent-client-id",
    );
    assert.equal(userMessages?.length, 1);
  } finally {
    await second.close();
  }
});
