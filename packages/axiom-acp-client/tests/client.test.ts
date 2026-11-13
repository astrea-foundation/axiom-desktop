import assert from "node:assert/strict";
import { once } from "node:events";
import { readFileSync } from "node:fs";
import { mkdtemp, mkdir, writeFile, rename, realpath } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve, toNamespacedPath } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";
import {
  AxiomAcpClient,
  AXIOM_ACP_CLIENT_VERSION,
  AxiomStateStore,
  type GetThreadTimelineResponse,
  validateDesktopExtension,
} from "../src/index.js";
import { AcpError } from "../src/errors.js";

const here = dirname(fileURLToPath(import.meta.url));
const binary = resolve(
  here,
  "../../../target/debug",
  process.platform === "win32" ? "axiomcli.exe" : "axiomcli",
);

test("ACP client implementation metadata matches its package version", () => {
  const manifest = JSON.parse(
    readFileSync(new URL("../package.json", import.meta.url), "utf8"),
  ) as { version?: unknown };
  assert.equal(AXIOM_ACP_CLIENT_VERSION, manifest.version);
});

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

test("extension negotiation accepts additive families and optional features only", () => {
  const required = {
    desktopChat: 1,
    threadCatalog: 1,
    timeline: 2,
    modelCatalog: 1,
    profilePreferences: 1,
    collections: 1,
    account: 2,
    billing: 3,
    securityEvidence: 4,
    webConsent: 1,
    futureFeature: 7,
  };
  assert.doesNotThrow(() => validateDesktopExtension({
    protocolVersion: "0.9",
    features: required,
  }));
  assert.throws(() => validateDesktopExtension({
    protocolVersion: "1.0",
    features: required,
  }));
  assert.throws(() => validateDesktopExtension({
    protocolVersion: "0.2",
    features: { ...required, timeline: 1 },
  }));
  assert.throws(() => validateDesktopExtension({
    protocolVersion: "0.2",
    features: { ...required, securityEvidence: 1 },
  }), /securityEvidence/);
  const { timeline: _, ...missingTimeline } = required;
  assert.throws(() => validateDesktopExtension({
    protocolVersion: "0.2",
    features: missingTimeline,
  }));
  const { webConsent: _webConsent, ...missingConsent } = required;
  assert.throws(() => validateDesktopExtension({
    protocolVersion: "0.2", features: missingConsent,
  }), /webConsent/);
});

test("production client negotiates, streams, and controls standard ACP settings", async () => {
  const { client, workspace } = await fixture();
  try {
    const created = await client.newSession(workspace);
    const models = await client.listModels();
    assert.ok(models.models.some((model) => model.id === "alternate-model"));
    assert.ok(models.models.every((model) =>
      model.providerId && model.providerLabel && model.shortLabel && model.upstreamModel
      && model.contextWindowTokens > 0 && model.maxOutputTokens > 0
      && model.autoCompactThresholdTokens === Math.floor(model.contextWindowTokens * 0.85),
    ));
    assert.ok(models.models.every((model) =>
      (model.inputPriceMicrousdPerMillionTokens ?? 0) > 0
      && (model.outputPriceMicrousdPerMillionTokens ?? 0) > 0,
    ));
    await client.setSettings({
      threadId: created.sessionId,
      model: "alternate-model",
      thinkingLevel: "high",
    });
    await client.setMode(created.sessionId, "observe");
    assert.equal(client.getState().sessions[created.sessionId]?.lastMessageAt, null);
    const result = await client.prompt(created.sessionId, "hello from desktop", "client-message-1");
    assert.equal(result.stopReason, "end_turn");
    const state = client.getState().sessions[created.sessionId];
    assert.ok(state?.lastMessageAt && Number.isFinite(Date.parse(state.lastMessageAt)));
    const catalog = await client.listThreads();
    assert.equal(catalog.threads.find((thread) => thread.threadId === created.sessionId)?.lastMessageAt, state.lastMessageAt);
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

test("every SDK prompt sends a strict explicit web choice without inheriting previous consent", async () => {
  const { client, workspace } = await fixture();
  try {
    const created = await client.newSession(workspace);
    const sent: boolean[] = [];
    const request = client.process.request.bind(client.process);
    client.process.request = async <T>(method: string, params: unknown, timeoutMs?: number, signal?: AbortSignal): Promise<T> => {
      if (method === "session/prompt") sent.push((params as { _meta: { axiom: { webEnabled: boolean } } })._meta.axiom.webEnabled);
      return request<T>(method, params, timeoutMs, signal);
    };
    await client.prompt(created.sessionId, "Default off", "web-off");
    await client.prompt(created.sessionId, "Explicit opt-in", "web-on", true);
    await client.prompt(created.sessionId, "No inherited opt-in", "web-off-again");
    await assert.rejects(client.prompt(created.sessionId, "Invalid opt-in", "web-invalid", "true" as unknown as boolean), /must be a boolean/);
    assert.deepEqual(sent, [false, true, false]);
  } finally { await client.close(); }
});

test("thread rename persists for loaded and unloaded threads without changing message order", async () => {
  const { client, workspace } = await fixture();
  try {
    const older = await client.newSession(workspace);
    await client.prompt(older.sessionId, "older message", "rename-older-message");
    const newer = await client.newSession(workspace);
    await client.prompt(newer.sessionId, "newer message", "rename-newer-message");
    const before = (await client.listThreads()).threads;
    const original = before.find((thread) => thread.threadId === older.sessionId)!;
    const result = await client.renameThread(older.sessionId, "  My renamed thread  ");
    assert.equal(result.thread.title, "My renamed thread");
    assert.equal(result.thread.lastMessageAt, original.lastMessageAt);
    assert.equal(result.thread.revision, original.revision + 1);
    assert.equal(client.getState().sessions[older.sessionId]?.title, "My renamed thread");
    assert.deepEqual((await client.listThreads()).threads.map((thread) => thread.threadId), before.map((thread) => thread.threadId));
    // Discard the SDK projection: renaming an unloaded row must not load its chat.
    client.state.removeSession(older.sessionId);
    await client.renameThread(older.sessionId, "Saved without loading");
    assert.equal(client.getState().sessions[older.sessionId], undefined);
    await client.refreshThread(older.sessionId);
    assert.equal(client.getState().sessions[older.sessionId]?.title, "Saved without loading");
    await assert.rejects(client.renameThread(older.sessionId, "  "));
    await assert.rejects(client.renameThread(older.sessionId, "🔒".repeat(130)));
    assert.equal((await client.refreshThread(older.sessionId)).thread.title, "Saved without loading");
  } finally { await client.close(); }
});

test("renaming updates a live title without rolling back newer stream metadata", () => {
  const store = new AxiomStateStore();
  const page = timelinePage(5);
  store.replaceTimeline([page]);
  store.setCatalog([page.thread]);
  const session = store.addSession(page.thread.threadId, page.thread.cwd);
  session.threadRevision = 8;
  session.running = true;
  session.lastMessageAt = "2026-08-24T00:00:03Z";
  store.applyThreadRename({ ...page.thread, revision: 6, title: "Renamed while streaming" });
  const updated = store.snapshot().sessions[page.thread.threadId]!;
  assert.equal(updated.title, "Renamed while streaming");
  assert.equal(updated.threadRevision, 8);
  assert.equal(updated.running, true);
  assert.equal(updated.lastMessageAt, "2026-08-24T00:00:03Z");
  assert.equal(updated.timeline.length, 1);
  store.setCatalog([{ ...page.thread, revision: 9, title: "Newer rename" }]);
  store.applyThreadRename({ ...page.thread, revision: 6, title: "Late response" });
  assert.equal(store.snapshot().sessions[page.thread.threadId]?.title, "Newer rename");
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
    assert.equal(nonreasoning.preferences.thinkingLevel, "provider_default");
    assert.equal(nonreasoning.settings.model, "grok-code");
    assert.equal(nonreasoning.settings.thinkingLevel, "provider_default");
    assert.ok(nonreasoning.configOptions.every((option) => option.id !== "thinking"));
    assert.equal(client.getState().sessions[created.sessionId]?.settings?.thinkingLevel, "provider_default");

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

test("older account responses and activities cannot overwrite newer auth state", () => {
  const store = new AxiomStateStore();
  store.setAccount({
    revision: 4,
    state: "valid",
    account: {
      id: "account-b",
      displayName: "Current account",
      linkedMethods: ["passkey"],
    },
    session: {
      expiresAt: "2026-09-28T00:00:00Z",
      credentialStore: "system credential store",
    },
  });
  store.setAccount({
    revision: 2,
    state: "unavailable",
    detail: "stale validation failed",
  });
  store.extensionEvent({
    runtimeInstanceId: "runtime",
    sequence: 1,
    occurredAt: "2026-08-28T00:00:00Z",
    event: {
      kind: "account_changed",
      status: {
        revision: 3,
        state: "signed_out",
      },
    },
  });

  assert.equal(store.snapshot().account?.state, "valid");
  assert.equal(store.snapshot().account?.revision, 4);

  store.extensionEvent({
    runtimeInstanceId: "runtime",
    sequence: 2,
    occurredAt: "2026-08-28T00:00:01Z",
    event: {
      kind: "account_changed",
      status: {
        revision: 5,
        state: "signed_out",
      },
    },
  });
  assert.equal(store.snapshot().account?.state, "signed_out");
  assert.equal(store.snapshot().account?.revision, 5);
});

test("an account-context change atomically clears every previous account projection", () => {
  const store = new AxiomStateStore();
  store.setAccount({
    revision: 1,
    state: "valid",
    account: { id: "account-a", linkedMethods: ["google"] },
    session: { expiresAt: "2026-09-28T00:00:00Z", credentialStore: "system credential store" },
  });
  store.addSession("account-a-thread", "/account-a");
  store.setCollections({
    revision: 9,
    collections: [{
      id: "private-a",
      name: "Account A",
      collapsed: false,
      position: 0,
      threadIds: ["account-a-thread"],
      createdAt: "2026-09-01T00:00:00Z",
      updatedAt: "2026-09-01T00:00:00Z",
    }],
  });
  store.setBilling({
    revision: 3,
    postedMicrousd: 10_000_000, trialMicrousd: 0, paidMicrousd: 10_000_000,
    availableMicrousd: 10_000_000,
    ledgerSequence: 7,
    currency: "microUSD",
    paymentAccount: {
      network: "mainnet", asset: "ZEC", conversion_status: "none", state: "ready",
      address: "u1account-a", payment_uri: "zcash:u1account-a", monitoring_status: "ready",
      required_confirmations: "10", confirmed_zatoshis: "9007199254740993",
      confirming_zatoshis: "0", review_required: false, deposits: [],
    },
  });
  const accountAContext = store.accountContextToken();

  store.extensionEvent({
    runtimeInstanceId: "runtime-account-switch",
    sequence: 1,
    occurredAt: "2026-09-01T00:00:01Z",
    event: {
      kind: "account_changed",
      status: {
        revision: 2,
        state: "valid",
        account: { id: "account-b", linkedMethods: ["ethereum_wallet"] },
        session: {
          expiresAt: "2026-09-28T00:00:00Z",
          credentialStore: "system credential store",
        },
      },
    },
  });

  const snapshot = store.snapshot();
  assert.deepEqual(snapshot.sessions, {});
  assert.deepEqual(snapshot.catalog, []);
  assert.deepEqual(snapshot.collections, { revision: 0, collections: [] });
  assert.equal(snapshot.preferences, null);
  assert.equal(snapshot.billing, null);
  assert.equal(snapshot.account?.account?.id, "account-b");
  assert.ok(store.accountContextToken() > accountAContext);
});

test("billing projections are monotonic within an account and clear on account switch", () => {
  const store = new AxiomStateStore();
  store.setAccount({
    revision: 1,
    state: "valid",
    account: { id: "account-a", linkedMethods: ["passkey"] },
    session: {
      expiresAt: "2026-10-04T00:00:00Z",
      credentialStore: "system credential store",
    },
  });
  const newest = {
    revision: 2,
    postedMicrousd: 12_000_000, trialMicrousd: 0, paidMicrousd: 12_000_000,
    availableMicrousd: 12_000_000,
    ledgerSequence: 9,
    currency: "microUSD",
  } as const;
  store.setBilling(newest);
  store.setBilling({ ...newest, revision: 3, ledgerSequence: 8, availableMicrousd: 1 });
  assert.equal(store.snapshot().billing?.availableMicrousd, 12_000_000);

  store.setAccount({
    revision: 2,
    state: "valid",
    account: { id: "account-b", linkedMethods: ["google"] },
    session: {
      expiresAt: "2026-10-04T00:00:00Z",
      credentialStore: "system credential store",
    },
  });
  assert.equal(store.snapshot().billing, null);
});

test("switching state rejects delayed old-account events before account B becomes valid", () => {
  const store = new AxiomStateStore();
  store.setAccount({
    revision: 1,
    state: "valid",
    account: { id: "account-a", linkedMethods: ["passkey"] },
    session: {
      expiresAt: "2026-09-28T00:00:00Z",
      credentialStore: "system credential store",
    },
  });
  store.addSession("account-a-thread", "/account-a/private");

  store.extensionEvent({
    runtimeInstanceId: "runtime-switch-barrier",
    sequence: 1,
    occurredAt: "2026-09-04T00:00:00Z",
    event: {
      kind: "account_changed",
      status: {
        revision: 2,
        state: "unavailable",
        detail: "Switching the local Axiom account context",
      },
    },
  });
  store.standardUpdate({
    sessionId: "account-a-thread",
    update: {
      sessionUpdate: "agent_message_chunk",
      content: { type: "text", text: "private account A completion" },
    },
  });
  store.extensionEvent({
    runtimeInstanceId: "runtime-switch-barrier",
    sequence: 2,
    occurredAt: "2026-09-04T00:00:01Z",
    sessionId: "account-a-thread",
    event: {
      kind: "profile_preferences_changed",
      preferences: {
        model: "account-a-private-model",
        thinkingLevel: "high",
        updatedAt: "2026-09-04T00:00:01Z",
      },
    },
  });
  store.extensionEvent({
    runtimeInstanceId: "runtime-switch-barrier",
    sequence: 3,
    occurredAt: "2026-09-04T00:00:02Z",
    event: {
      kind: "account_changed",
      status: {
        revision: 3,
        state: "valid",
        account: { id: "account-b", linkedMethods: ["google"] },
        session: {
          expiresAt: "2026-10-04T00:00:00Z",
          credentialStore: "system credential store",
        },
      },
    },
  });

  const snapshot = store.snapshot();
  assert.equal(snapshot.account?.account?.id, "account-b");
  assert.deepEqual(snapshot.sessions, {});
  assert.equal(snapshot.preferences, null);
  assert.ok(!JSON.stringify(snapshot).includes("private account A"));
  assert.ok(!JSON.stringify(snapshot).includes("account-a-private-model"));
});

test("a sidecar runtime change drops account projections before any resync", () => {
  const store = new AxiomStateStore();
  store.setAccount({
    revision: 8,
    state: "valid",
    account: { id: "account-a", linkedMethods: ["passkey"] },
    session: {
      expiresAt: "2026-09-28T00:00:00Z",
      credentialStore: "system credential store",
    },
  });
  store.addSession("private-thread", "/account-a");
  store.extensionEvent({
    runtimeInstanceId: "old-runtime",
    sequence: 1,
    occurredAt: "2026-09-01T00:00:00Z",
    event: { kind: "background_task", task_id: "one", state: "done" },
  });
  const oldContext = store.accountContextToken();

  const event = store.extensionEvent({
    runtimeInstanceId: "new-runtime",
    sequence: 1,
    occurredAt: "2026-09-01T00:00:01Z",
    event: { kind: "background_task", task_id: "two", state: "starting" },
  });

  assert.equal(event?.fullResync, true);
  assert.deepEqual(store.snapshot().sessions, {});
  assert.equal(store.snapshot().account, null);
  assert.ok(store.accountContextToken() > oldContext);
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

test("cancelling during verification prevents a waiting prompt from starting later", async () => {
  const { client, workspace } = await fixture();
  try {
    const created = await client.newSession(workspace);
    await client.verifySecurity(created.sessionId);
    const request = client.process.request.bind(client.process);
    let rejectVerification!: (error: Error) => void;
    let prompts = 0;
    client.process.request = <T>(method: string, params: unknown, timeoutMs?: number, signal?: AbortSignal): Promise<T> => {
      if (method === "_axiom/security/verify") return new Promise<T>((_resolve, reject) => { rejectVerification = reject; });
      if (method === "session/prompt") prompts++;
      return request<T>(method, params, timeoutMs, signal);
    };
    const verification = assert.rejects(client.verifySecurity(created.sessionId), /stopped/);
    await new Promise((resolve) => setImmediate(resolve));
    const pending = client.prompt(created.sessionId, "cancel before sending", "cancel-preflight");
    await client.cancel(created.sessionId);
    rejectVerification(new Error("verification stopped"));
    await verification;
    assert.equal((await pending).stopReason, "cancelled");
    assert.equal(prompts, 0);
    assert.equal(client.getState().sessions[created.sessionId]?.running, false);
    client.process.request = request;
    assert.equal((await client.prompt(created.sessionId, "new instruction", "after-cancel")).stopReason, "end_turn");
    assert.equal(client.getState().sessions[created.sessionId]?.timeline.some((item) => item.clientItemId === "cancel-preflight"), false);
  } finally { await client.close(); }
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

test("unknown durable timeline kinds remain visible as generic activity", () => {
  const store = new AxiomStateStore();
  const page = timelinePage(2);
  page.items[0] = {
    ...page.items[0],
    kind: "future_checkpoint",
    content: "Checkpoint created by a newer sidecar",
  } as unknown as GetThreadTimelineResponse["items"][number];
  store.replaceTimeline([page]);

  const item = store.snapshot().sessions.session?.timeline[0];
  assert.equal(item?.kind, "activity");
  assert.equal(item?.text, "Checkpoint created by a newer sidecar");
  assert.equal((item?.raw as { kind?: string })?.kind, "future_checkpoint");
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

test("desktop Agent settings own per-thread workspaces and persist permissions across restart", async () => {
  const root = await mkdtemp(resolve(tmpdir(), "axiom-desktop-client-"));
  const options = {
    command: binary,
    args: ["acp", "--frontend", "desktop-chat"],
    env: {
      AXIOMCLI_TEST_RUNNER: "echo",
      XDG_CONFIG_HOME: resolve(root, "config"),
      XDG_DATA_HOME: resolve(root, "data"),
    },
  };
  let client = new AxiomAcpClient(options);
  try {
    await client.initialize();
    const bootstrap = await client.bootstrapDesktop();
    assert.equal(bootstrap.frontend, "desktop-chat");
    assert.equal(bootstrap.permissionProfile, "web");
    assert.equal(bootstrap.newThreadSettings.thinkingLevel, "medium");
    assert.ok(bootstrap.chatCwd?.endsWith(
      join("axiom", "accounts", "local-test-account", "desktop", "chat"),
    ));
    const created = await client.newChat();
    const session = client.getState().sessions[created.sessionId];
    const defaultCwd = join(bootstrap.chatCwd!, created.sessionId);
    assert.equal(session?.cwd, defaultCwd);
    assert.equal(session?.currentModeId, "web");
    assert.deepEqual(session?.modes.map((mode) => mode.id), ["web", "confirm", "full_access"]);
    assert.equal(session?.desktopAgent?.enabled, false);
    assert.equal(session?.desktopAgent?.revision, 0);
    await assert.rejects(client.setMode(created.sessionId, "full_access"), /Agent controls/);
    const other = await client.newChat();
    assert.notEqual(client.getState().sessions[other.sessionId]?.cwd, defaultCwd);
    await client.prompt(created.sessionId, "hello from desktop profile", "desktop-user-1");
    assert.ok(client.getState().sessions[created.sessionId]?.timeline.some(
      (item) => item.kind === "user" && item.clientItemId === "desktop-user-1",
    ));
    const project = join(root, "project with spaces 日本語");
    await mkdir(project);
    const customCwd = toNamespacedPath(await realpath(project));
    const initial = { threadId: created.sessionId, expectedRevision: 0, enabled: true, permission: "approve_commands" as const, workingDirectory: customCwd };
    for (const invalid of ["relative/path", join(root, "missing"), "bad\0path"]) {
      await assert.rejects(client.configureDesktopAgent({ ...initial, workingDirectory: invalid }));
    }
    const file = join(root, "not-a-folder.txt");
    await writeFile(file, "untouched");
    await assert.rejects(client.configureDesktopAgent({ ...initial, workingDirectory: file }));
    const configured = await client.configureDesktopAgent(initial);
    assert.equal(configured.agent.revision, 1);
    assert.equal(configured.agent.usesDefaultDirectory, false);
    assert.equal(configured.thread.profile, "confirm");
    assert.equal(configured.thread.cwd, customCwd);
    assert.equal(client.getState().sessions[created.sessionId]?.desktopAgent?.enabled, true);
    await assert.rejects(client.configureDesktopAgent(initial), (error: unknown) => error instanceof AcpError && /changed/.test(String(error.data)));
    await assert.rejects(client.prompt(created.sessionId, "stale queued input", "stale-input", false, 0), (error: unknown) => error instanceof AcpError && /queued/.test(String(error.data)));
    assert.ok(!client.getState().sessions[created.sessionId]?.timeline.some((item) => item.clientItemId === "stale-input"));
    await client.prompt(created.sessionId, "uses current revision", "agent-input");
    await client.close();
    client = new AxiomAcpClient(options);
    await client.initialize();
    await client.bootstrapDesktop();
    await client.loadChat(created.sessionId);
    const restored = client.getState().sessions[created.sessionId]!;
    assert.equal(restored.cwd, customCwd);
    assert.equal(restored.currentModeId, "confirm");
    assert.deepEqual(restored.desktopAgent, configured.agent);
    await rename(project, `${project}-moved`);
    await client.close();
    client = new AxiomAcpClient(options);
    await client.initialize(); await client.bootstrapDesktop(); await client.loadChat(created.sessionId);
    await assert.rejects(client.prompt(created.sessionId, "missing folder must not execute", "moved-folder"),
      (error: unknown) => error instanceof AcpError && /working directory/.test(String(error.data)));
    const full = await client.configureDesktopAgent({ ...initial, expectedRevision: 1, permission: "full_access", workingDirectory: null });
    assert.equal(full.thread.profile, "full_access");
    assert.equal(full.thread.cwd, defaultCwd);
    assert.equal(full.agent.usesDefaultDirectory, true);
    const off = await client.configureDesktopAgent({ ...initial, expectedRevision: 2, permission: "full_access", enabled: false, workingDirectory: null });
    assert.equal(off.thread.profile, "web");
    assert.equal(off.agent.permission, "full_access");
    assert.equal(off.agent.revision, 3);
    await client.loadChat(other.sessionId);
    assert.equal(client.getState().sessions[other.sessionId]?.desktopAgent?.revision, 0);
    assert.equal(client.getState().sessions[other.sessionId]?.desktopAgent?.enabled, false);
    assert.equal(readFileSync(file, "utf8"), "untouched");
  } finally {
    await client.close();
  }
});

test("desktop Agent changes are rejected while a tool approval or turn is active", async () => {
  const root = await mkdtemp(resolve(tmpdir(), "axiom-agent-busy-"));
  const client = new AxiomAcpClient({ command: binary, args: ["acp", "--frontend", "desktop-chat"], env: {
    AXIOMCLI_TEST_RUNNER: "approval", XDG_CONFIG_HOME: resolve(root, "config"), XDG_DATA_HOME: resolve(root, "data"),
  } });
  try {
    await client.initialize(); await client.bootstrapDesktop();
    const { sessionId } = await client.newChat();
    await client.configureDesktopAgent({ threadId: sessionId, expectedRevision: 0, enabled: true, permission: "approve_commands", workingDirectory: null });
    const interaction = once(client, "interaction");
    const prompt = client.prompt(sessionId, "approval fixture", "busy-prompt");
    const [pending] = await interaction;
    await assert.rejects(client.configureDesktopAgent({ threadId: sessionId, expectedRevision: 1, enabled: true, permission: "full_access", workingDirectory: null }));
    assert.equal(client.getState().sessions[sessionId]?.desktopAgent?.permission, "approve_commands");
    client.resolvePermission(pending.id, { outcome: "selected", optionId: "allow_once" });
    await prompt;
    const result = await client.configureDesktopAgent({ threadId: sessionId, expectedRevision: 1, enabled: false, permission: "approve_commands", workingDirectory: null });
    assert.equal(result.agent.enabled, false);
  } finally { await client.close(); }
});

test("confirmed deletion removes a loaded thread without affecting another thread", async () => {
  const { client, workspace } = await fixture();
  try {
    const removed = await client.newSession(workspace);
    const kept = await client.newSession(workspace);
    const preview = await client.deletePreview([removed.sessionId]);
    const result = await client.deleteConfirm(preview.confirmationToken, [removed.sessionId]);
    assert.equal(result.deleted, 1);
    const state = client.getState();
    assert.equal(state.sessions[removed.sessionId], undefined);
    assert.equal(state.catalog.some((thread) => thread.threadId === removed.sessionId), false);
    assert.ok(state.sessions[kept.sessionId]);
    await assert.rejects(client.loadSession(removed.sessionId, workspace));
    assert.equal(client.getState().sessions[removed.sessionId], undefined);
    await client.refreshThread(kept.sessionId);
    assert.ok(client.getState().sessions[kept.sessionId]);
  } finally {
    await client.close();
  }
});

test("confirmed desktop deletion cancels and drains active work before deleting", async () => {
  const { client, workspace } = await fixture("approval");
  try {
    const created = await client.newSession(workspace);
    const interaction = once(client, "interaction");
    const prompt = client.prompt(created.sessionId, "wait for approval", "delete-active");
    await interaction;
    // The ordinary preview remains read-only and cannot cancel a live turn.
    await assert.rejects(client.deletePreview([created.sessionId]), (error: unknown) =>
      error instanceof AcpError && error.data === "sessions with active work must be cancelled before deletion",
    );
    assert.equal(client.getState().sessions[created.sessionId]?.interactions.length, 1);
    const preview = await client.deletePreview([created.sessionId], { cancelActiveWork: true });
    assert.equal((await prompt).stopReason, "cancelled");
    assert.equal(client.getState().sessions[created.sessionId]?.interactions.length, 0);
    const deleted = await client.deleteConfirm(preview.confirmationToken, [created.sessionId]);
    assert.equal(deleted.deleted, 1);
    assert.equal(client.getState().sessions[created.sessionId], undefined);
  } finally {
    await client.close();
  }
});

test("desktop deletion retries only the native work-draining condition", async (t) => {
  const { client, workspace } = await fixture();
  try {
    const created = await client.newSession(workspace);
    const request = client.process.request.bind(client.process);
    let previews = 0;
    t.mock.method(client.process, "request", async (method: string, ...args: unknown[]) => {
      if (method === "_axiom/thread/delete_preview" && ++previews <= 2) {
        throw new AcpError(-32603, "Internal error", "sessions with active work must be cancelled before deletion");
      }
      return request(method, ...args as [unknown, number?]);
    });
    const preview = await client.deletePreview([created.sessionId], { cancelActiveWork: true });
    assert.equal(previews, 3);
    assert.ok(preview.confirmationToken);
    await client.deleteConfirm(preview.confirmationToken, [created.sessionId]);
    previews = 0;
    await assert.rejects(client.deletePreview([created.sessionId], { cancelActiveWork: true }), (error: unknown) =>
      error instanceof AcpError && String(error.data).includes("session not found"),
    );
    // Two simulated busy responses, then the real missing-thread error: it is
    // surfaced immediately instead of being retried until the deadline.
    assert.equal(previews, 3);
  } finally {
    await client.close();
  }
});

test("a billing outage does not fail local desktop initialization after sign-in", async () => {
  const root = await mkdtemp(resolve(tmpdir(), "axiom-desktop-billing-outage-"));
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
    client.state.setAccount({
      revision: 1000,
      state: "valid",
      account: { id: "local-test-account", linkedMethods: ["passkey"] },
    });
    await client.bootstrapDesktop();
    let billingRequests = 0;
    client.billingStatus = async () => {
      billingRequests += 1;
      throw new Error("billing temporarily unavailable");
    };
    await client.initializeDesktopState();
    assert.equal(billingRequests, 1);
    assert.equal(client.getState().account?.state, "valid");
    assert.equal(client.getState().billing, null, "do not fabricate a zero balance");
    assert.ok(client.getState().preferences);
    await assert.rejects(client.billingStatus(), /billing temporarily unavailable/);

    // Only optional billing refresh is isolated: mandatory local-store
    // failures must still fail closed rather than exposing another account.
    client.listThreads = async () => { throw new Error("local store unavailable"); };
    await assert.rejects(client.initializeDesktopState(), /local store unavailable/);
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
    await first.renameThread(sessionId, "Persistent custom title");
    await first.renameCollection(collectionId, "Renamed research");
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
      collection.id === collectionId && collection.name === "Renamed research" && collection.threadIds.includes(sessionId),
    ));
    await second.loadChat(sessionId);
    assert.equal(second.getState().sessions[sessionId]?.title, "Persistent custom title");
    const userMessages = second.getState().sessions[sessionId]?.timeline.filter(
      (item) => item.kind === "user" && item.clientItemId === "persistent-client-id",
    );
    assert.equal(userMessages?.length, 1);
  } finally {
    await second.close();
  }
});

test("usage summary preserves exact values and rejects responses from the previous account", async () => {
  const { client } = await fixture();
  const summary = { period: "all_time", totalCostMicrousd: "9007199254740993", models: [
    { provider: "test", modelId: "test", modelName: "Test", costMicrousd: "9007199254740993" },
  ] };
  try {
    const state = (client as unknown as { state: AxiomStateStore }).state;
    state.setAccount({ revision: 1, state: "valid", account: { id: "account-a", linkedMethods: ["password"] } });
    const request = client.process.request.bind(client.process);
    const requests: unknown[] = [];
    let complete: ((value: unknown) => void) | undefined;
    client.process.request = <T>(method: string, params: unknown, timeoutMs?: number, signal?: AbortSignal): Promise<T> => {
      if (method === "_axiom/usage/summary") {
        requests.push(params);
        return new Promise<T>(resolve => { complete = resolve as (value: unknown) => void; });
      }
      return request<T>(method, params, timeoutMs, signal);
    };
    const first = client.usageSummary();
    complete!({ summary });
    assert.deepEqual(await first, { summary });
    const week = client.usageSummary({ period: "week", timezone: "America/Los_Angeles" });
    complete!({ summary: { ...summary, period: "week" } });
    assert.equal((await week).summary.period, "week");
    assert.deepEqual(requests, [{}, { period: "week", timezone: "America/Los_Angeles" }]);
    const wrongPeriod = client.usageSummary({ period: "month", timezone: "America/Los_Angeles" });
    complete!({ summary }); // An old server/sidecar may ignore the period argument.
    await assert.rejects(wrongPeriod, /selected period/i);
    const stale = client.usageSummary();
    state.setAccount({ revision: 2, state: "valid", account: { id: "account-b", linkedMethods: ["password"] } });
    complete!({ summary });
    await assert.rejects(stale, /account changed/i);
  } finally { await client.close(); }
});

test("API key operations reject responses after the account changes and never update shared state", async () => {
  const {client} = await fixture();
  try {
    const state = (client as unknown as {state: AxiomStateStore}).state;
    let revision = 0;
    const setAccount = (id: string) => state.setAccount({revision: ++revision, state: "valid", account: {id, linkedMethods: ["password"]}});
    let complete: ((value: unknown) => void) | undefined;
    const methods: string[] = [];
    const original = client.process.request.bind(client.process);
    client.process.request = <T>(method: string, params: unknown, timeoutMs?: number, signal?: AbortSignal): Promise<T> => {
      if (method.startsWith("_axiom/account/api_key")) {
        methods.push(method);
        return new Promise<T>(resolve => {complete = resolve as (value: unknown) => void;});
      }
      return original<T>(method, params, timeoutMs, signal);
    };
    for (const operation of [() => client.apiKeys(), () => client.createApiKey("Coding tools"), () => client.revokeApiKey("key-1")]) {
      setAccount("account-a");
      const pending = operation();
      setAccount("account-b");
      complete!({keys: [], key: {id: "key-1"}, token: "axm_never_share_old_account_key"});
      await assert.rejects(pending, /account changed/i);
    }
    assert.deepEqual(methods, ["_axiom/account/api_keys", "_axiom/account/api_key_create", "_axiom/account/api_key_revoke"]);
    assert(!JSON.stringify(client.getState()).includes("axm_never_share"));
    await assert.rejects(client.createApiKey("bad\nname"), /Invalid API key name/);
    await assert.rejects(client.revokeApiKey("../another-key"), /Invalid API key ID/);
  } finally {await client.close();}
});

test("gift redemption updates billing and discards a receipt after an account switch", async (t) => {
  const { client } = await fixture();
  t.after(() => client.close());
  const state = (client as unknown as { state: AxiomStateStore }).state;
  const code = "AXG-AAAA-AAAA-AAAA-AAAA-AAAA-AAAA-AAAA-AAAA";
  state.setAccount({revision: 1, state: "valid", account: {id: "account-a", linkedMethods: ["password"]}});
  let complete!: (value: unknown) => void;
  const original = client.process.request.bind(client.process);
  client.process.request = <T>(method: string, params: unknown, timeoutMs?: number, signal?: AbortSignal): Promise<T> => {
    if (method === "_axiom/billing/redeem_gift_code") {
      assert.deepEqual(params, {code});
      return new Promise<T>(resolve => { complete = resolve as (value: unknown) => void; });
    }
    return original<T>(method, params, timeoutMs, signal);
  };
  const receipt = {creditedMicrousd: 5_000_000, alreadyRedeemed: false, status: {
    revision: 1, ledgerSequence: 1, currency: "microUSD", postedMicrousd: 5_000_000,
    availableMicrousd: 5_000_000, paidMicrousd: 5_000_000, trialMicrousd: 0,
    paymentReviewRequired: false, paymentAccount: null, zecUsdQuote: null,
  }};
  const first = client.redeemGiftCode(code);
  complete(receipt);
  assert.deepEqual(await first, receipt);
  assert.equal(client.getState().billing?.paidMicrousd, 5_000_000);
  assert(!JSON.stringify(client.getState()).includes(code));
  const stale = client.redeemGiftCode(code);
  state.setAccount({revision: 2, state: "valid", account: {id: "account-b", linkedMethods: ["password"]}});
  complete(receipt);
  await assert.rejects(stale, /account changed/i);
  assert.equal(client.getState().billing, null);
  await assert.rejects(client.redeemGiftCode("x".repeat(65)), /valid Axiom gift code/);
});


test("native message revision replaces local history, rejects stale edits, and survives reload", async () => {
  let { client, workspace } = await fixture();
  try {
    const created = await client.newSession(workspace);
    await client.prompt(created.sessionId, "first original", "revision-first");
    await client.prompt(created.sessionId, "later original", "revision-later");
    const prior = client.getState().sessions[created.sessionId]!;
    const user = prior.timeline.find((item) => item.kind === "user")!;
    await client.prompt(created.sessionId, "replacement", "revision-replacement", false, 0,
      { userItemId: user.id, expectedRevision: prior.threadRevision });
    const revised = client.getState().sessions[created.sessionId]!;
    assert.deepEqual(revised.timeline.filter((item) => item.kind === "user").map((item) => item.text), ["replacement"]);
    assert.ok(revised.timeline.some((item) => item.kind === "assistant" && item.text.includes("replacement")));
    await assert.rejects(client.prompt(created.sessionId, "stale", "revision-stale", false, 0,
      { userItemId: user.id, expectedRevision: prior.threadRevision }), /conversation changed/i);
    await client.close();
    client = new AxiomAcpClient({ command: binary, args: ["acp"], env: {
      AXIOMCLI_TEST_RUNNER: "echo", XDG_CONFIG_HOME: resolve(dirname(workspace), "config"),
      XDG_DATA_HOME: resolve(dirname(workspace), "data"),
    } });
    await client.initialize();
    await client.loadSession(created.sessionId, workspace);
    assert.deepEqual(client.getState().sessions[created.sessionId]!.timeline.filter((item) => item.kind === "user").map((item) => item.text), ["replacement"]);
  } finally { await client.close(); }
});

test("real ACP attachment submission, inspection, reload, and text revision retain the same local input", async () => {
  let { client, workspace } = await fixture();
  try {
    const { sessionId } = await client.newSession(workspace);
    const attachments = [{ kind: "file" as const, name: "notes.txt", file: { name: "notes.txt", mimeType: "text/plain", data: Buffer.from("private local attachment\n".repeat(4000)).toString("base64") } }];
    await client.prompt(sessionId, "", "attached-client-id", false, 0, undefined, attachments);
    let session = client.getState().sessions[sessionId]!;
    let user = session.timeline.find((item) => item.clientItemId === "attached-client-id")!;
    assert.ok(user);
    assert.deepEqual((await client.getAttachments(sessionId, user.id)).attachments, attachments);
    assert.ok(!JSON.stringify(session.timeline).includes("private local attachment"), "state only broadcasts attachment summaries");
    await client.close();
    const root = dirname(workspace);
    client = new AxiomAcpClient({ command: binary, args: ["acp"], env: {
      AXIOMCLI_TEST_RUNNER: "echo", XDG_CONFIG_HOME: resolve(root, "config"), XDG_DATA_HOME: resolve(root, "data"),
    } });
    await client.initialize();
    await client.loadSession(sessionId, workspace);
    session = client.getState().sessions[sessionId]!;
    user = session.timeline.find((item) => item.clientItemId === "attached-client-id")!;
    await client.prompt(sessionId, "Read the attached notes again", "attachment-edit", false, 0, { userItemId: user.id, expectedRevision: session.threadRevision });
    session = client.getState().sessions[sessionId]!;
    user = session.timeline.find((item) => item.clientItemId === "attachment-edit")!;
    assert.equal(user.text, "Read the attached notes again");
    assert.deepEqual((await client.getAttachments(sessionId, user.id)).attachments, attachments);
  } finally { await client.close(); }
});

test("timeline assembly independently drains accounting pages and rejects repeated cursors", async (t) => {
  const first = timelinePage();
  const a = { requestId: "a".repeat(32), modelId: "model", providerId: "near" };
  const b = { ...a, requestId: "b".repeat(32) };
  first.requestUsage = [a];
  first.nextRequestUsageCursor = a.requestId;
  const second = { ...first, items: [], requestUsage: [b], nextRequestUsageCursor: undefined };
  const client = new AxiomAcpClient({ command: binary, args: ["acp"] });
  t.after(() => client.close());
  const internal = client as unknown as {
    process: { request: (method: string, params: Record<string, unknown>) => Promise<GetThreadTimelineResponse> };
    state: AxiomStateStore;
    fetchTimeline: (id: string, context: number) => Promise<GetThreadTimelineResponse>;
  };
  let calls = 0;
  internal.process.request = async (_, params) => {
    if (calls++ === 0) return first;
    assert.equal(params.afterSequence, 1);
    assert.equal(params.afterRequestId, a.requestId);
    return second;
  };
  await internal.fetchTimeline("session", internal.state.accountContextToken());
  assert.equal(calls, 2);
  assert.deepEqual(client.getState().sessions.session!.requestUsage, [a, b]);
  calls = 0;
  internal.process.request = async () => { calls++; return first; };
  await assert.rejects(internal.fetchTimeline("session", internal.state.accountContextToken()), /non-progressing/);
  assert.equal(calls, 2);
  assert.deepEqual(client.getState().sessions.session!.requestUsage, [a, b]);
  assert.throws(() => internal.state.replaceTimeline([first, { ...second, requestUsage: [a] }]), /identity/);
  assert.throws(() => internal.state.replaceTimeline([first, { ...second, thread: { ...first.thread, revision: 2 } }]), /changed/);
});
