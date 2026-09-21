import assert from "node:assert/strict";
import { after, before, test } from "node:test";
import { fileURLToPath } from "node:url";
import { chromium, type Browser, type Page } from "playwright";
import { createServer, type ViteDevServer } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import type { ApiKeyRecord, AccountStatus, BillingStatus, ClientSessionState, ClientTimelineItem, ContextUsage, ConfigureDesktopAgentRequest, DesktopAgentSettings } from "@axiom/axiom-acp-client";

// Real renderer/DOM focus tests, but no Electron window, OS input, account,
// sidecar, or provider request. The bridge below operates only on fake threads.
let browser: Browser;
let server: ViteDevServer;
let origin: string;

function depositBilling(postedMicrousd: number, revision = 1): BillingStatus {
  const address = `u1${"a".repeat(180)}`;
  return {
    revision, postedMicrousd, availableMicrousd: postedMicrousd,
    trialMicrousd: 0, paidMicrousd: postedMicrousd, ledgerSequence: revision,
    currency: "microUSD", paymentReviewRequired: false, zecUsdQuote: null,
    paymentAccount: {
      network: "mainnet", asset: "ZEC", conversion_status: "none",
      state: "ready", valuation_enabled: true, address, payment_uri: `zcash:${address}`,
      monitoring_status: "ready", required_confirmations: "10",
      confirmed_zatoshis: "0", confirming_zatoshis: "0", review_required: false, deposits: [],
    },
  };
}

before(async () => {
  server = await createServer({
    configFile: false,
    root: fileURLToPath(new URL("../../src/renderer", import.meta.url)),
    plugins: [react(), tailwindcss()],
    server: { host: "127.0.0.1", port: 0 },
    logLevel: "error",
  });
  await server.listen();
  origin = server.resolvedUrls!.local[0]!;
  browser = await chromium.launch({ headless: true });
});

after(async () => {
  await browser?.close();
  await server?.close();
});

async function openApp(platform = "linux", failDelete = false, thinkingLevels: string[] = [], rememberedThinking = "medium", supportsImages = false, bypassCSP = true) {
  const page = await browser.newPage({ viewport: { width: 1320, height: 900 }, bypassCSP });
  page.setDefaultTimeout(8_000);
  const errors: string[] = [];
  const nativeDialogs: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  page.on("dialog", async (dialog) => {
    nativeDialogs.push(dialog.type());
    await dialog.dismiss();
  });
  await page.route("**/*", (route) => {
    if (route.request().url().startsWith(origin)) return route.continue();
    return route.abort();
  });
  const installBridge = ({ platform, failDelete, thinkingLevels, rememberedThinking, supportsImages }: { platform: string; failDelete: boolean; thinkingLevels: string[]; rememberedThinking: string; supportsImages: boolean }) => {
    const savedThinking = () => localStorage.getItem("test-thinking") ?? rememberedThinking;
    const settingsCalls: { threadId: string; model?: string; thinkingLevel?: string }[] = [];
    const defaultDirectory = (id: string) => platform === "win32" ? `C:\\Axiom\\chat\\${id}` : `/axiom/chat/${id}`;
    const savedAgent = (id: string): DesktopAgentSettings => JSON.parse(localStorage.getItem(`test-agent:${id}`) || "null") || {
      enabled: false, permission: "approve_commands", revision: 0, workingDirectory: defaultDirectory(id),
      defaultWorkingDirectory: defaultDirectory(id), usesDefaultDirectory: true,
    };
    const session = (id: string) => ({
      sessionId: id, title: id === "old-thread" ? "Existing thread" : "New test thread",
      cwd: "", settings: { model: "test-model", thinkingLevel: savedThinking() },
      security: { state: "idle" }, modes: [], currentModeId: null, configOptions: [],
      contextUsage: null as ContextUsage | null,
      requestUsage: [] as NonNullable<ClientSessionState["requestUsage"]>,
      desktopAgent: savedAgent(id),
      timeline: [] as ClientTimelineItem[],
      interactions: [], running: false, activeTurnId: null as string | null, needsResync: false,
      threadRevision: 1, lastTimelineSequence: 0,
      lastMessageAt: null as string | null,
      lastUserMessageAt: null as string | null,
    });
    const summary = (id: string) => ({
      threadId: id, title: session(id).title, cwd: "", updatedAt: new Date().toISOString(),
      archived: false, revision: 1, lifecycle: "ready",
      lastMessageAt: null as string | null,
      lastUserMessageAt: null as string | null,
    });
    const state = {
      connected: true, runtimeInstanceId: "test-runtime", lastSequence: 0,
      sessions: {} as Record<string, ReturnType<typeof session>>,
      catalog: [summary("old-thread")],
      collections: { revision: 1, collections: [{ id: "folder", name: "Test folder", collapsed: false, threadIds: [] }] },
      preferences: null, account: { revision: 1, state: "valid", account: { id: "test-account", displayName: "Test User", verifiedEmail: "test@example.invalid", linkedMethods: ["password"] } } as AccountStatus,
      billing: null as BillingStatus | null, diagnostic: "", error: null,
    };
    const calls: { method: string; id?: string; text?: string; webEnabled?: boolean; userItemId?: string; expectedRevision?: number }[] = [];
    let failRename = false;
    let failAgent = false;
    let failBalance = false;
    let newThreadCount = 0;
    let loginCount = 0;
    const pendingLogins = new Map<string, { resolve: (value: unknown) => void; reject: (error: Error) => void }>();
    const setupGates = new Map<string, { promise: Promise<void>; resolve: () => void; reject: (error: Error) => void }>();
    const waitForSetup = async (stage: string) => { await setupGates.get(stage)?.promise; };
    const pendingSteers = new Map<string, { thread: string; text: string; resolve: (value: unknown) => void }>();
    const listeners = new Set<(state: unknown) => void>();
    const emit = () => {
      for (const listener of listeners) listener(structuredClone(state));
    };
    const agent = {
      onState: (listener: (state: unknown) => void) => { listeners.add(listener); return () => listeners.delete(listener); },
      getState: async () => structuredClone(state),
      desktopBootstrap: async () => ({ newThreadSettings: { model: "test-model", thinkingLevel: savedThinking() } }),
      listModels: async () => ({ models: [{
        id: "test-model", label: "Test model", shortLabel: "Test model", providerId: "test",
        providerLabel: "Test provider", upstreamModel: "test-model", thinkingLevels,
        contextWindowTokens: 100_000, autoCompactThresholdTokens: 85_000, supportsImages,
      }] }),
      loadChat: async (id: string) => {
        calls.push({ method: "load", id });
        state.sessions[id] = session(id); emit();
      },
      deletePreview: async (ids: string[]) => {
        calls.push({ method: "preview", id: ids[0] });
        if (failDelete) throw new Error("Deletion test failure");
        return { confirmationToken: "test-token" };
      },
      deleteConfirm: async (_token: string, ids: string[]) => {
        calls.push({ method: "delete", id: ids[0] });
        for (const id of ids) delete state.sessions[id];
        state.catalog = state.catalog.filter((entry) => !ids.includes(entry.threadId));
        // Match the SDK race: removal is published BEFORE the IPC resolves.
        emit();
        await new Promise((resolve) => setTimeout(resolve, 40));
        emit();
        return { deleted: ids.length };
      },
      deleteCollection: async (id: string) => {
        calls.push({ method: "deleteFolder", id });
        state.collections.collections = state.collections.collections.filter((entry) => entry.id !== id);
        emit();
      },
      renameThread: async (id: string, title: string) => {
        calls.push({ method: "renameThread", id, text: title });
        if (failRename) throw new Error("Rename test failure");
        state.catalog.find((thread) => thread.threadId === id)!.title = title;
        if (state.sessions[id]) state.sessions[id]!.title = title;
        emit();
      },
      renameCollection: async (id: string, name: string) => {
        calls.push({ method: "renameFolder", id, text: name });
        if (failRename) throw new Error("Rename test failure");
        state.collections.collections.find((folder) => folder.id === id)!.name = name;
        emit();
      },
      setCollectionCollapsed: async (id: string, collapsed: boolean) => {
        calls.push({ method: "collapseFolder", id });
        state.collections.collections.find((folder) => folder.id === id)!.collapsed = collapsed;
        emit();
      },
      newChat: async () => {
        calls.push({ method: "new" });
        const id = ++newThreadCount === 1 ? "new-thread" : `new-thread-${newThreadCount}`;
        await waitForSetup("new");
        state.sessions[id] = session(id);
        state.catalog.push(summary(id)); emit();
        return { sessionId: id };
      },
      setSettings: async (request: { threadId: string; model?: string; thinkingLevel?: string }) => {
        settingsCalls.push(request);
        await waitForSetup("settings");
        const selected = state.sessions[request.threadId]!;
        selected.settings = { ...selected.settings, ...(request.model ? { model: request.model } : {}), ...(request.thinkingLevel ? { thinkingLevel: request.thinkingLevel } : {}) };
        localStorage.setItem("test-thinking", selected.settings.thinkingLevel);
        emit();
        return { threadId: request.threadId, settings: selected.settings };
      },
      configureDesktopAgent: async (request: ConfigureDesktopAgentRequest) => {
        calls.push({ method: "configureAgent", id: request.threadId });
        await waitForSetup("agent");
        if (failAgent) throw new Error("Working directory is unavailable");
        const selected = state.sessions[request.threadId]!;
        if (selected.running || selected.desktopAgent.revision !== request.expectedRevision) throw new Error("Settings changed or thread busy");
        selected.desktopAgent = { enabled: request.enabled, permission: request.permission, revision: request.expectedRevision + 1,
          defaultWorkingDirectory: defaultDirectory(request.threadId), workingDirectory: request.workingDirectory || defaultDirectory(request.threadId),
          usesDefaultDirectory: !request.workingDirectory };
        localStorage.setItem(`test-agent:${request.threadId}`, JSON.stringify(selected.desktopAgent));
        emit(); return { agent: selected.desktopAgent, thread: summary(request.threadId) };
      },
      chooseWorkingDirectory: async () => {
        calls.push({ method: "chooseDirectory" });
        return platform === "win32" ? "C:\\Projects\\My Project 日本語" : "/Projects/My Project 日本語";
      },
      promptWithWebConsent: async (id: string, text: string, clientItemId: string, webEnabled: boolean) => {
        calls.push({ method: "prompt", id, text, webEnabled });
        await waitForSetup("prompt");
        state.sessions[id]!.timeline.push({ id: clientItemId, clientItemId, kind: "user", text, status: "completed" });
        emit();
        return { stopReason: "end_turn" };
      },
      promptWithAttachments: async (id: string, text: string, clientItemId: string, webEnabled: boolean, _revision: number, attachments: any[]) => {
        calls.push({ method: "attachments", id, text: JSON.stringify({ text, attachments }), webEnabled });
        state.sessions[id]!.timeline.push({ id: clientItemId, clientItemId, kind: "user", text, status: "completed", raw: { metadata: { attachments: attachments.map((file) => ({ name: file.name, kind: file.kind })) } } });
        emit(); return { stopReason: "end_turn" };
      },
      revisePrompt: async (id: string, text: string, userItemId: string, expectedRevision: number, webEnabled: boolean) => {
        calls.push({ method: "revise", id, text, userItemId, expectedRevision, webEnabled });
        return { stopReason: "end_turn" };
      },
      steer: async (request: { threadId: string; clientItemId: string; text: string; expectedTurnId: string; webEnabled: boolean }) => {
        calls.push({ method: "steer", id: request.clientItemId, text: request.text, webEnabled: request.webEnabled });
        if (state.sessions[request.threadId]?.activeTurnId !== request.expectedTurnId) throw new Error("stale turn");
        return new Promise((resolve) => pendingSteers.set(request.clientItemId, { thread: request.threadId, text: request.text, resolve }));
      },
      cancel: async (id: string) => { calls.push({ method: "cancel", id }); state.sessions[id]!.running = false; emit(); },
      listThreads: async () => state.catalog,
      apiKeys: async () => ({keys: []}),
      createApiKey: async () => { throw new Error("API keys not configured in this fixture"); },
      revokeApiKey: async () => ({}),
      accountStatus: async () => { calls.push({ method: "accountStatus" }); return { account: state.account }; },
      billingStatus: async () => { calls.push({ method: "billingStatus" }); if (failBalance) throw new Error("Balance test failure"); return { billing: state.billing }; },
      openAccountPortal: async () => { calls.push({ method: "accountPortal" }); },
      nativeLoginStart: async (...hints: unknown[]) => {
        const loginId = `test-login-${++loginCount}`;
        calls.push({ method: "nativeLoginStart", id: loginId, text: JSON.stringify(hints) });
        await waitForSetup("loginStart");
        return { login: { loginId, userCode: "ABCD-1234", authorizationUrl: "https://auth.axiom.stream/native/authorize?test=1", browserOpened: true } };
      },
      nativeLoginComplete: async (id: string) => {
        calls.push({ method: "nativeLoginComplete", id });
        return new Promise((resolve, reject) => pendingLogins.set(id, { resolve, reject }));
      },
      nativeLoginCancel: async (id: string) => {
        calls.push({ method: "nativeLoginCancel", id });
        pendingLogins.get(id)?.reject(new Error("Sign-in cancelled"));
        pendingLogins.delete(id);
      },
      logout: async () => { calls.push({ method: "logout" }); state.account = { revision: 2, state: "signed_out" }; state.billing = null; emit(); return { account: state.account }; },
    };
    Object.assign(window, {
      __deletionTest: {
        calls,
        settingsCalls,
        finishLogin: (id: string, error?: string) => {
          const pending = pendingLogins.get(id);
          if (!pending) throw new Error(`No pending login: ${id}`);
          pendingLogins.delete(id);
          if (error) { pending.reject(new Error(error)); return; }
          state.account = { revision: 3, state: "valid", account: { id: "test-account", displayName: "Test User", linkedMethods: ["passkey"] } };
          emit();
          pending.resolve({ status: state.account });
        },
        holdSetup: (stage: string) => {
          let resolve!: () => void;
          let reject!: (error: Error) => void;
          const promise = new Promise<void>((yes, no) => { resolve = yes; reject = no; });
          setupGates.set(stage, { promise, resolve, reject });
        },
        releaseSetup: (stage: string, error?: string) => {
          const gate = setupGates.get(stage);
          setupGates.delete(stage);
          if (error) gate?.reject(new Error(error)); else gate?.resolve();
        },
        setAccount: (id: string) => { state.account.account!.id = id; emit(); },
        setAccountState: (account: AccountStatus) => { state.account = account; state.sessions = {}; state.catalog = []; emit(); },
        setBilling: (billing: BillingStatus) => { state.billing = billing; emit(); },
        failBalance: (fail: boolean) => { failBalance = fail; },
        setRuntime: (id: string, connected = true) => { state.runtimeInstanceId = id; state.connected = connected; emit(); },
        finishSteers: () => {
          for (const [id, pending] of pendingSteers) {
            const session = state.sessions[pending.thread]!;
            session.timeline.push({ id, clientItemId: id, kind: "user", text: pending.text });
            session.timeline.push({ id: `reply-${id}`, kind: "assistant", text: "Continuing with your update", status: "streaming" });
            pending.resolve({ clientItemId: id, turnId: session.activeTurnId });
          }
          pendingSteers.clear(); emit();
        },
        finishTurn: () => { state.sessions["old-thread"]!.running = false; state.sessions["old-thread"]!.activeTurnId = null; emit(); },
        failRename: (fail: boolean) => { failRename = fail; },
        failAgent: (fail: boolean) => { failAgent = fail; },
        setReply: (text: string) => {
          state.sessions["old-thread"]!.timeline = [{ id: "reply", kind: "assistant", text, status: "completed" }];
          emit();
        },
        setTimeline: (timeline: ClientTimelineItem[], running = false) => {
          state.sessions["old-thread"]!.timeline = timeline;
          state.sessions["old-thread"]!.running = running;
          state.sessions["old-thread"]!.activeTurnId = running ? "active-test-turn" : null;
          emit();
        },
        setRequestUsage: (usage: NonNullable<ClientSessionState["requestUsage"]>) => { state.sessions["old-thread"]!.requestUsage = usage; emit(); },
        setContextUsage: (usage: ContextUsage | null) => {
          state.sessions["old-thread"]!.contextUsage = usage;
          emit();
        },
        setOrdering: (threads: { id: string; lastMessageAt: string | null; folder: string | null }[]) => {
          state.catalog = threads.map((thread) => ({ ...summary(thread.id), title: thread.id, lastMessageAt: thread.lastMessageAt, lastUserMessageAt: thread.lastMessageAt }));
          state.collections.collections = ["Empty", "Older", "Newer"].map((name) => ({
            id: name, name, collapsed: false, threadIds: threads.filter((thread) => thread.folder === name).map((thread) => thread.id),
          }));
          emit();
        },
        bumpLiveMessage: (id: string, time: string) => {
          state.sessions[id] = { ...session(id), title: id, lastMessageAt: time };
          emit();
        },
        bumpUserMessage: (id: string, time: string) => {
          state.sessions[id] = { ...session(id), title: id, lastMessageAt: time, lastUserMessageAt: time };
          emit();
        },
        refreshMetadata: () => {
          state.catalog = state.catalog.map((thread) => ({ ...thread, updatedAt: new Date().toISOString(), revision: thread.revision + 1 }));
          emit();
        },
      },
      axiomDesktop: {
        platform, agent, setTheme: () => {},
        isFullScreen: async () => false, onFullScreenChange: () => () => {},
        isFocused: async () => true, onActiveChange: () => () => {},
        isMaximized: async () => false, onMaximizedChange: () => () => {},
      },
    });
  };
  // tsx adds a function-name helper to nested callbacks; serialized browser
  // fixtures must define that helper in their own JavaScript realm.
  await page.addInitScript({ content: `window.__name = (fn) => fn; (${installBridge.toString()})(${JSON.stringify({ platform, failDelete, thinkingLevels, rememberedThinking, supportsImages })});` });
  await page.goto(origin);
  try {
    await page.getByRole("button", { name: "Test model" }).waitFor();
  } catch (error) {
    console.error("Renderer startup failed", errors, await page.locator("body").innerText());
    await page.close();
    throw error;
  }
  return { page, errors, nativeDialogs };
}

for (const platform of ["linux", "win32", "darwin"]) {
  test(`stale thinking preferences become an offered value and persist after sending (${platform})`, async () => {
    const { page, errors } = await openApp(platform, false, ["provider_default", "enabled", "disabled"], "provider_default");
    try {
      const selector = page.getByTitle("Thinking level", { exact: true });
      assert.equal(await selector.innerText(), "On");
      await selector.click();
      assert.equal(await page.getByRole("menuitemradio", { name: "On", exact: true }).getAttribute("aria-checked"), "true");
      assert.equal(await page.getByRole("menuitemradio", { name: "Medium", exact: true }).count(), 0);
      assert.equal(await page.getByRole("menuitemradio", { name: "Provider default", exact: true }).count(), 0);
      await page.getByRole("menuitemradio", { name: "Off", exact: true }).click();
      assert.equal(await selector.innerText(), "Off");
      await page.locator("textarea[data-chat-input]").fill("Keep thinking off");
      await page.locator("textarea[data-chat-input]").press("Control+Enter");
      await page.waitForFunction(() => (window as any).__deletionTest.calls.some((call: any) => call.method === "prompt"));
      const settings = await page.evaluate(() => (window as any).__deletionTest.settingsCalls);
      assert.ok(settings.length > 0);
      assert.ok(settings.every((request: any) => request.thinkingLevel === "disabled"));
      await page.reload();
      await selector.waitFor();
      assert.equal(await selector.innerText(), "Off");
      assert.deepEqual(errors, []);
    } finally { await page.close(); }
  });
}

test("a loaded thread also corrects stale thinking before its next message", async () => {
  const { page, errors } = await openApp("linux", false, ["provider_default", "enabled"]);
  try {
    await page.getByRole("button", { name: "Viewed Existing thread", exact: true }).click();
    const selector = page.getByTitle("Thinking level", { exact: true });
    assert.equal(await selector.innerText(), "On");
    await selector.click();
    assert.equal(await page.getByRole("menuitemradio", { name: "Off", exact: true }).count(), 0);
    await page.getByRole("menuitemradio", { name: "On", exact: true }).click();
    await page.locator("textarea[data-chat-input]").fill("Continue");
    await page.locator("textarea[data-chat-input]").press("Control+Enter");
    await page.waitForFunction(() => (window as any).__deletionTest.calls.some((call: any) => call.method === "prompt"));
    const settings = await page.evaluate(() => (window as any).__deletionTest.settingsCalls);
    assert.ok(settings.some((request: any) => request.threadId === "old-thread" && request.thinkingLevel === "enabled"));
    assert.deepEqual(errors, []);
  } finally { await page.close(); }
});

async function enableDraftAgent(page: Page) {
  await page.getByRole("button", { name: "Agent", exact: true }).click();
  await page.getByRole("switch", { name: "Enable agent mode" }).click();
  await page.getByRole("button", { name: "Save changes", exact: true }).click();
  await page.getByRole("dialog", { name: "Agent settings" }).waitFor({ state: "hidden" });
}

for (const platform of ["linux", "win32", "darwin"]) {
  test(`new Agent threads show the message immediately throughout setup and send it only once (${platform})`, async () => {
    const { page, errors } = await openApp(platform);
    try {
      await enableDraftAgent(page);
      await page.evaluate(() => {
        for (const stage of ["new", "settings", "agent", "prompt"]) (window as any).__deletionTest.holdSetup(stage);
      });
      await page.locator("textarea[data-chat-input]").fill("Work on my project");
      await page.locator("textarea[data-chat-input]").press("Enter");
      const messages = page.getByRole("region", { name: "Chat messages", exact: true });
      await messages.getByRole("status").waitFor();
      assert.equal(await messages.getByRole("status").innerText(), "Sending…");
      assert.equal(await messages.locator('[data-timeline-kind="user"]').count(), 1);
      assert.match(await messages.innerText(), /Work on my project/);
      assert.equal(await messages.locator('[data-timeline-kind="user"] > .group').evaluate((element) => getComputedStyle(element).opacity), "1");
      assert.equal(await page.locator("textarea[data-chat-input]").count(), 0);
      await page.keyboard.press("Enter");
      assert.equal((await calls(page)).filter((call) => call.method === "new").length, 1);
      assert.equal((await calls(page)).some((call) => call.method === "prompt"), false);
      if (platform === "linux" && process.env.AXIOM_PREPARING_SCREENSHOT) {
        await page.screenshot({ path: process.env.AXIOM_PREPARING_SCREENSHOT, animations: "disabled" });
        await page.emulateMedia({ colorScheme: "dark" });
        await page.waitForFunction(() => document.documentElement.dataset.theme === "dark");
        await page.screenshot({ path: `${process.env.AXIOM_PREPARING_SCREENSHOT}.dark.png`, animations: "disabled" });
      }
      await page.evaluate(() => (window as any).__deletionTest.releaseSetup("new"));
      await page.waitForFunction(() => (window as any).__deletionTest.settingsCalls.length === 1);
      assert.equal(await messages.getByRole("status").innerText(), "Sending…");
      await page.evaluate(() => (window as any).__deletionTest.releaseSetup("settings"));
      await page.waitForFunction(() => (window as any).__deletionTest.calls.some((call: any) => call.method === "configureAgent"));
      assert.equal(await messages.getByRole("status").innerText(), "Sending…");
      assert.equal((await calls(page)).some((call) => call.method === "prompt"), false);
      await page.evaluate(() => (window as any).__deletionTest.releaseSetup("agent"));
      await page.waitForFunction(() => (window as any).__deletionTest.calls.some((call: any) => call.method === "prompt"));
      await page.locator("textarea[data-chat-input]").waitFor();
      assert.equal(await messages.locator('[data-timeline-kind="user"]').count(), 1);
      assert.equal(await page.getByRole("status", { name: "Sending message" }).count(), 1);
      assert.equal(await page.getByRole("region", { name: "Message queue" }).count(), 0);
      assert.equal(await page.getByRole("button", { name: "Agent", exact: true }).getAttribute("aria-pressed"), "true");
      await page.evaluate(() => (window as any).__deletionTest.releaseSetup("prompt"));
      await page.getByRole("status", { name: "Sending message" }).waitFor({ state: "hidden" });
      assert.equal(await messages.locator('[data-timeline-kind="user"]').count(), 1);
      assert.equal((await calls(page)).filter((call) => call.method === "prompt").length, 1);
      assert.equal(await page.locator("textarea[data-chat-input]").inputValue(), "");
      assert.deepEqual(errors, []);
    } finally { await page.close(); }
  });
}

for (const stage of ["new", "settings", "agent"]) {
  test(`failed ${stage} setup restores the durable draft and requested Agent mode without sending`, async () => {
    const { page, errors } = await openApp();
    try {
      await enableDraftAgent(page);
      await page.evaluate((stage) => (window as any).__deletionTest.holdSetup(stage), stage);
      await page.locator("textarea[data-chat-input]").fill("Keep the complete draft");
      await page.locator("textarea[data-chat-input]").press("Enter");
      await page.getByRole("status", { name: "Sending message" }).waitFor();
      await page.waitForFunction((stage) => stage === "settings"
        ? (window as any).__deletionTest.settingsCalls.length > 0
        : (window as any).__deletionTest.calls.some((call: any) => call.method === (stage === "agent" ? "configureAgent" : "new")), stage);
      await page.evaluate((stage) => (window as any).__deletionTest.releaseSetup(stage, "Setup could not finish"), stage);
      await page.getByText("Setup could not finish", { exact: true }).waitFor();
      assert.equal(await page.locator("textarea[data-chat-input]").inputValue(), "Keep the complete draft");
      assert.equal(await page.getByRole("button", { name: "Agent", exact: true }).getAttribute("aria-pressed"), "true");
      assert.equal((await calls(page)).some((call) => call.method === "prompt"), false);
      await page.locator("textarea[data-chat-input]").press("Enter");
      await page.waitForFunction(() => (window as any).__deletionTest.calls.some((call: any) => call.method === "prompt"));
      assert.equal((await calls(page)).filter((call) => call.method === "prompt").length, 1);
      assert.deepEqual(errors, []);
    } finally { await page.close(); }
  });
}

for (const destination of ["new", "existing", "account", "runtime"]) {
  test(`leaving thread preparation for ${destination} preserves the message without stealing focus`, async () => {
    const { page, errors } = await openApp();
    try {
      await page.evaluate(() => (window as any).__deletionTest.holdSetup("new"));
      await page.locator("textarea[data-chat-input]").fill("Private pending message");
      await page.locator("textarea[data-chat-input]").press("Enter");
      await page.getByRole("status", { name: "Sending message" }).waitFor();
      if (destination === "new") await page.getByRole("button", { name: "New thread", exact: true }).click();
      else if (destination === "existing") await page.getByRole("button", { name: "Viewed Existing thread", exact: true }).click();
      else if (destination === "account") await page.evaluate(() => (window as any).__deletionTest.setAccount("another-account"));
      else await page.evaluate(() => (window as any).__deletionTest.setRuntime("another-runtime"));
      await page.locator("textarea[data-chat-input]").waitFor();
      if (destination === "account") assert.equal(await page.locator("textarea[data-chat-input]").inputValue(), "");
      await page.locator("textarea[data-chat-input]").fill("A different draft");
      await page.evaluate(() => (window as any).__deletionTest.releaseSetup("new"));
      await page.waitForFunction(() => document.querySelector('[data-thread-id="new-thread"]'));
      assert.equal(await page.getByRole("status", { name: "Sending message" }).count(), 0);
      assert.equal(await page.locator("textarea[data-chat-input]").inputValue(), "A different draft");
      if (destination === "new" || destination === "existing") {
        await page.waitForFunction(() => (window as any).__deletionTest.calls.some((call: any) => call.method === "prompt"));
        assert.equal((await calls(page)).filter((call) => call.method === "prompt").length, 1);
        assert.equal(await page.locator("textarea[data-chat-input]").inputValue(), "A different draft");
      } else {
        assert.equal((await calls(page)).some((call) => call.method === "prompt"), false);
        assert.equal(await page.evaluate(() => (window as any).__deletionTest.settingsCalls.length), 0);
        const saved = await page.evaluate(() => JSON.parse(localStorage.getItem("axiom.message-queue.v1:test-account")!));
        assert.equal(saved[0].text, "", "large payloads are not stored in localStorage");
        const payload = await page.evaluate(async (id: string) => {
          const { localPayloads } = await import(`${location.origin}/src/localPayloads.ts`);
          return localPayloads.get("test-account", id);
        }, saved[0].payloadId);
        assert.equal(payload.text, "Private pending message");
        assert.equal(saved[0].preparing, true);
      }
      assert.deepEqual(errors, []);
    } finally { await page.close(); }
  });
}

test("a long draft keeps its sending indicator in view during thread preparation", async () => {
  const { page, errors } = await openApp();
  try {
    await page.evaluate(() => (window as any).__deletionTest.holdSetup("new"));
    await page.locator("textarea[data-chat-input]").fill(Array.from({ length: 80 }, (_, index) => `Project requirement ${index}\n`).join("\n"));
    await page.locator("textarea[data-chat-input]").press("Control+Enter");
    await page.getByRole("status", { name: "Sending message" }).waitFor();
    await page.waitForFunction(() => {
      const viewport = document.querySelector("[data-chat-scroll]")!.getBoundingClientRect();
      const status = document.querySelector('[aria-label="Sending message"]')!.getBoundingClientRect();
      return status.top >= viewport.top && status.bottom <= viewport.bottom;
    });
    assert.deepEqual(errors, []);
  } finally { await page.close(); }
});

test("an unconfirmed first message leaves sending state and remains queued for an explicit retry", async () => {
  const { page, errors } = await openApp();
  try {
    await page.evaluate(() => (window as any).__deletionTest.holdSetup("prompt"));
    await page.locator("textarea[data-chat-input]").fill("Do not lose this message");
    await page.locator("textarea[data-chat-input]").press("Enter");
    await page.waitForFunction(() => (window as any).__deletionTest.calls.some((call: any) => call.method === "prompt"));
    await page.getByRole("status", { name: "Sending message" }).waitFor();
    await page.evaluate(() => (window as any).__deletionTest.releaseSetup("prompt", "Delivery unavailable"));
    const queue = page.getByRole("region", { name: "Message queue" });
    await queue.waitFor();
    await page.getByRole("status", { name: "Sending message" }).waitFor({ state: "hidden" });
    assert.match(await queue.innerText(), /Do not lose this message/);
    assert.equal(await page.locator('[data-timeline-kind="user"]').count(), 0);
    await queue.getByRole("button", { name: "Send now", exact: true }).click();
    await page.waitForFunction(() => (window as any).__deletionTest.calls.filter((call: any) => call.method === "prompt").length === 2);
    await queue.waitFor({ state: "hidden" });
    assert.equal(await page.locator('[data-timeline-kind="user"]').count(), 1);
    assert.deepEqual(errors, []);
  } finally { await page.close(); }
});

for (const platform of ["linux", "win32", "darwin"]) {
  test(`trailing orange dots distinguish unread and working threads without marking metadata unread (${platform})`, async () => {
    const { page, errors } = await openApp(platform);
    try {
      const row = page.locator('[data-thread-id="old-thread"]');
      const title = row.locator(".overflow-title");
      const initialX = (await title.boundingBox())!.x;
      assert.equal(await row.locator(".thread-dot").count(), 0);
      await page.evaluate(() => (window as any).__deletionTest.refreshMetadata());
      assert.equal(await row.locator(".thread-dot").count(), 0);
      await page.evaluate(() => (window as any).__deletionTest.bumpLiveMessage("old-thread", "2026-09-08T12:00:00Z"));
      await row.locator(".thread-dot").waitFor();
      assert.equal((await title.boundingBox())!.x, initialX, "dot visibility must not move the title");
      assert.ok((await row.locator(".thread-dot").boundingBox())!.x > (await title.boundingBox())!.x + (await title.boundingBox())!.width);
      assert.equal(await row.locator(".thread-dot").evaluate((element) => getComputedStyle(element).backgroundColor), "rgb(255, 118, 83)");
      await row.click();
      await row.locator(".thread-dot").waitFor({ state: "detached" });
      await page.evaluate(() => (window as any).__deletionTest.bumpLiveMessage("old-thread", "2026-09-08T12:01:00Z"));
      assert.equal(await row.locator(".thread-dot").count(), 0, "messages in the open chat are already read");
      await page.getByRole("button", { name: "New thread", exact: true }).click();
      await page.evaluate(() => (window as any).__deletionTest.setTimeline([], true));
      await page.waitForFunction(() => document.querySelector('[data-thread-id="old-thread"]')?.getAttribute("aria-label")?.startsWith("Replying"));
      assert.equal(await row.locator(".thread-dot-working").count(), 1, "working uses the trailing pulsing dot");
      assert.equal(await row.locator(".thread-dot-working").evaluate((element) => getComputedStyle(element).animationName), "dot-working");
      await page.emulateMedia({ reducedMotion: "reduce" });
      assert.equal(await row.locator(".thread-dot-working").evaluate((element) => getComputedStyle(element).animationName), "none");
      await page.evaluate(() => (window as any).__deletionTest.setTimeline([], false));
      assert.equal(await row.locator(".thread-dot").count(), 0);
      await page.evaluate(() => (window as any).__deletionTest.bumpLiveMessage("old-thread", "2026-09-08T12:02:00Z"));
      await row.locator(".thread-dot").waitFor();
      await row.click();
      await row.locator(".thread-dot").waitFor({ state: "detached" });
      assert.equal((await title.boundingBox())!.x, initialX);
      assert.deepEqual(errors, []);
    } finally { await page.close(); }
  });
}

for (const platform of ["linux", "win32", "darwin"]) {
  test(`overflowing titles fade and reveal the entire title on hover or keyboard focus (${platform})`, async () => {
    const { page, errors } = await openApp(platform);
    const fullTitle = "Investigating how the desktop app handles very long conversation titles and preserves the final words";
    try {
      await page.evaluate((title) => (window as any).axiomDesktop.agent.renameThread("old-thread", title), fullTitle);
      const row = page.locator('[data-thread-id="old-thread"]');
      const viewport = row.locator(".overflow-title");
      const track = row.locator(".overflow-title-track");
      await page.waitForFunction(() => document.querySelector('[data-thread-id="old-thread"] .overflow-title')?.getAttribute("data-overflow") === "true");
      assert.equal(await viewport.innerText(), fullTitle);
      assert.equal(await viewport.evaluate((element) => getComputedStyle(element).textOverflow), "clip");
      assert.match(await viewport.evaluate((element) => getComputedStyle(element).maskImage), /linear-gradient/);
      assert.equal(await track.evaluate((element) => getComputedStyle(element).animationName), "none");
      assert.equal(await row.getAttribute("aria-label"), `Viewed ${fullTitle}`);
      // Hover the trailing empty slot, not just the text.
      await row.locator("[data-thread-indicator]").hover();
      await page.waitForFunction(() => document.querySelector('[data-thread-id="old-thread"] .overflow-title-track')!.getAnimations().length > 0);
      const atEnd = await track.evaluate((element) => {
        const animation = element.getAnimations()[0]!;
        animation.pause();
        animation.currentTime = Number(animation.effect!.getTiming().duration) * 0.99;
        const viewport = element.parentElement!.getBoundingClientRect();
        const text = element.querySelector(".overflow-title-text")!.getBoundingClientRect();
        return { start: text.left, end: text.right, left: viewport.left, right: viewport.right };
      });
      assert.ok(atEnd.start < atEnd.left);
      assert.ok(atEnd.end <= atEnd.right - 20 && atEnd.end > atEnd.left, "the final characters must be outside the fading edge");
      await page.getByPlaceholder("Search threads").hover();
      assert.equal(await track.evaluate((element) => getComputedStyle(element).transform), "none");
      await row.focus();
      await page.keyboard.press("Tab");
      await page.keyboard.press("Shift+Tab");
      assert.equal(await row.evaluate((element) => element.matches(":focus-visible")), true);
      assert.equal(await track.evaluate((element) => getComputedStyle(element).animationName), "title-carousel");
      await page.emulateMedia({ reducedMotion: "reduce" });
      await page.waitForFunction(() => document.querySelector('[data-thread-id="old-thread"] .overflow-title')?.hasAttribute("title"));
      assert.equal(await viewport.getAttribute("title"), fullTitle);
      assert.equal(await track.evaluate((element) => getComputedStyle(element).animationName), "none");
      await page.evaluate(() => (window as any).axiomDesktop.agent.renameThread("old-thread", "Short title"));
      await page.waitForFunction(() => !document.querySelector('[data-thread-id="old-thread"] .overflow-title')?.hasAttribute("data-overflow"));
      assert.equal(await viewport.evaluate((element) => getComputedStyle(element).maskImage), "none");
      assert.equal(await viewport.getAttribute("title"), null);
      assert.deepEqual(errors, []);
    } finally { await page.close(); }
  });
}

test("title fades update with available width, and cover folder and chat header titles", async () => {
  const { page, errors } = await openApp();
  const fullTitle = "A detailed investigation into desktop application behavior ".repeat(5).trim();
  try {
    await page.evaluate(async (title) => {
      await (window as any).axiomDesktop.agent.renameThread("old-thread", title);
      await (window as any).axiomDesktop.agent.renameCollection("folder", title);
    }, fullTitle);
    const row = page.locator('[data-thread-id="old-thread"]');
    const folder = page.locator('[data-folder-id="folder"] .overflow-title');
    await page.waitForFunction(() => document.querySelector('[data-folder-id="folder"] .overflow-title')?.getAttribute("data-overflow") === "true");
    assert.match(await folder.evaluate((element) => getComputedStyle(element).maskImage), /linear-gradient/);
    await row.click();
    const header = page.locator("header .overflow-title").first();
    await page.waitForFunction(() => document.querySelector("header .overflow-title")?.getAttribute("data-overflow") === "true");
    assert.equal(await header.innerText(), fullTitle);
    const before = await header.evaluate((element) => Math.abs(parseFloat(element.style.getPropertyValue("--title-scroll-distance"))));
    await page.setViewportSize({ width: 850, height: 900 });
    await page.waitForFunction((before) => Math.abs(parseFloat((document.querySelector("header .overflow-title") as HTMLElement).style.getPropertyValue("--title-scroll-distance"))) > before, before);
    if (process.env.AXIOM_TITLE_SCREENSHOT) {
      await page.setViewportSize({ width: 1320, height: 900 });
      await page.getByPlaceholder("Search threads").hover();
      await page.screenshot({ path: process.env.AXIOM_TITLE_SCREENSHOT, animations: "disabled" });
      await page.emulateMedia({ colorScheme: "dark" });
      await page.waitForFunction(() => document.documentElement.dataset.theme === "dark");
      await page.screenshot({ path: `${process.env.AXIOM_TITLE_SCREENSHOT}.dark.png`, animations: "disabled" });
    }
    assert.deepEqual(errors, []);
  } finally { await page.close(); }
});

async function openDeleteDialog(page: Page) {
  await page.getByRole("button", { name: "Viewed Existing thread", exact: true }).click();
  await page.locator("textarea").waitFor();
  await page.locator('[data-thread-id="old-thread"]').click({ button: "right" });
  await page.getByRole("menuitem", { name: "Delete", exact: true }).click();
  await page.getByRole("alertdialog").waitFor();
}

async function calls(page: Page) {
  return page.evaluate(() => (window as unknown as { __deletionTest: { calls: { method: string; id?: string; text?: string }[] } }).__deletionTest.calls);
}

async function assertComposerFocused(page: Page) {
  await page.waitForFunction(() => document.activeElement?.matches("textarea[data-chat-input]"));
  assert.equal(await page.locator("dialog:modal").count(), 0);
}

for (const platform of ["linux", "win32", "darwin"]) {
  test(`chat links request an external window without navigating or losing the chat (${platform})`, async () => {
    const { page, errors } = await openApp(platform);
    try {
      // Only the destination is stubbed. Exercise the real Markdown, anchor,
      // and browser new-window behavior without contacting external websites.
      await page.context().route("**://example.invalid/**", (route) => route.fulfill({ contentType: "text/html", body: "<title>External page fixture</title>" }));
      await page.getByRole("button", { name: "Viewed Existing thread", exact: true }).click();
      await page.evaluate(() => (window as any).__deletionTest.setReply("[Cat resources](https://example.invalid/cats)\n\n[More resources](http://example.invalid/more)"));
      await page.locator("textarea[data-chat-input]").fill("Keep this draft");
      for (const [name, url, keyboard] of [["Cat resources", "https://example.invalid/cats", false], ["More resources", "http://example.invalid/more", true]] as const) {
        const link = page.getByRole("link", { name, exact: true });
        const opened = page.waitForEvent("popup");
        if (keyboard) { await link.focus(); await link.press("Enter"); }
        else await link.click();
        const popup = await opened;
        await popup.waitForLoadState("domcontentloaded");
        assert.equal(popup.url(), url);
        assert.equal(await popup.evaluate(() => window.opener === null), true);
        await popup.close();
        assert.equal(page.url(), origin);
        assert.equal(await page.locator("textarea[data-chat-input]").inputValue(), "Keep this draft");
      }
      assert.deepEqual(errors, []);
    } finally { await page.close(); }
  });

  test(`Agent controls save per-thread permissions and folders, with safe new-thread defaults (${platform})`, async () => {
    const { page, errors, nativeDialogs } = await openApp(platform);
    const agent = page.getByRole("button", { name: "Agent", exact: true });
    try {
      assert.equal(await agent.getAttribute("aria-pressed"), "false");
      await agent.click();
      const dialog = page.getByRole("dialog", { name: "Agent settings" });
      await page.setViewportSize({ width: 960, height: 640 });
      await page.waitForFunction(() => {
        const rect = document.querySelector('[aria-label="Agent settings"]')!.getBoundingClientRect();
        return rect.top >= 0 && rect.left >= 0 && rect.right <= innerWidth && rect.bottom <= innerHeight
          && Math.abs(rect.left + rect.width / 2 - innerWidth / 2) < 2
          && Math.abs(rect.top + rect.height / 2 - innerHeight / 2) < 2;
      });
      if (platform === "linux" && process.env.AXIOM_AGENT_SCREENSHOT) {
        await page.screenshot({ path: `${process.env.AXIOM_AGENT_SCREENSHOT}.light.png` });
        await page.emulateMedia({ colorScheme: "dark" });
        await page.waitForFunction(() => document.documentElement.dataset.theme === "dark");
        await page.waitForTimeout(350); // Settle the theme transition for visual inspection.
        await page.screenshot({ path: process.env.AXIOM_AGENT_SCREENSHOT });
      }
      assert.equal(await page.locator("dialog:modal").count(), 1);
      // The native HTML modal makes the composer inert, rather than merely
      // placing a visual overlay above it.
      await page.locator("textarea[data-chat-input]").evaluate((element: HTMLTextAreaElement) => element.focus());
      assert.equal(await dialog.evaluate((element) => element.contains(document.activeElement)), true);
      const close = dialog.getByRole("button", { name: "Close Agent settings" });
      const save = dialog.getByRole("button", { name: "Save changes" });
      await close.focus(); await page.keyboard.press("Shift+Tab");
      assert.equal(await save.evaluate((element) => element === document.activeElement), true);
      await page.keyboard.press("Tab");
      assert.equal(await close.evaluate((element) => element === document.activeElement), true);
      await page.setViewportSize({ width: 1320, height: 900 });
      assert.equal(await dialog.getByRole("switch", { name: "Enable agent mode" }).getAttribute("aria-checked"), "false");
      assert.equal(await dialog.getByLabel("Approve commands", { exact: true }).isChecked(), true);
      await dialog.getByRole("switch", { name: "Enable agent mode" }).click();
      await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
      assert.equal(await agent.getAttribute("aria-pressed"), "false");
      await page.getByRole("button", { name: "Viewed Existing thread", exact: true }).click();
      await agent.click();
      await dialog.getByRole("switch", { name: "Enable agent mode" }).click();
      await dialog.getByText("Full access", { exact: true }).click();
      await dialog.getByRole("button", { name: "Choose working directory" }).click();
      const custom = platform === "win32" ? "C:\\Projects\\My Project 日本語" : "/Projects/My Project 日本語";
      assert.equal(await dialog.getByLabel("Working directory", { exact: true }).inputValue(), custom);
      await dialog.getByRole("button", { name: "Save changes", exact: true }).click();
      await dialog.waitFor({ state: "hidden" });
      assert.equal(await agent.getAttribute("aria-pressed"), "true");
      await page.reload();
      await page.getByRole("button", { name: "Viewed Existing thread", exact: true }).click();
      await agent.click();
      assert.equal(await dialog.getByLabel("Full access", { exact: true }).isChecked(), true);
      assert.equal(await dialog.getByLabel("Working directory", { exact: true }).inputValue(), custom);
      await dialog.getByRole("button", { name: "Use default thread folder" }).click();
      await dialog.getByRole("switch", { name: "Enable agent mode" }).click();
      await dialog.getByRole("button", { name: "Save changes", exact: true }).click();
      await dialog.waitFor({ state: "hidden" });
      assert.equal(await agent.getAttribute("aria-pressed"), "false");
      await agent.click();
      await page.keyboard.press("Escape");
      assert.equal(await dialog.count(), 0);
      await agent.click();
      await page.mouse.click(12, 12);
      assert.equal(await dialog.count(), 0, "clicking the modal backdrop closes settings");
      assert.equal(await agent.evaluate((element) => element === document.activeElement), true);
      await page.getByRole("button", { name: "New thread", exact: true }).click();
      await agent.click();
      assert.equal(await dialog.getByRole("switch", { name: "Enable agent mode" }).getAttribute("aria-checked"), "false");
      assert.equal(await dialog.getByLabel("Approve commands", { exact: true }).isChecked(), true);
      await dialog.getByRole("switch", { name: "Enable agent mode" }).click();
      await dialog.getByRole("button", { name: "Save changes", exact: true }).click();
      await dialog.waitFor({ state: "hidden" });
      await page.locator("textarea[data-chat-input]").fill("Work in this thread folder");
      await page.locator("textarea[data-chat-input]").press("Enter");
      await page.waitForFunction(() => (window as any).__deletionTest.calls.some((call: any) => call.method === "prompt"));
      const actions = await calls(page);
      assert.ok(actions.findIndex((call) => call.method === "configureAgent") < actions.findIndex((call) => call.method === "prompt"));
      assert.equal(await agent.getAttribute("aria-pressed"), "true");
      assert.deepEqual(nativeDialogs, []);
      assert.deepEqual(errors, []);
    } finally { await page.close(); }
  });

  test(`Agent configuration failures retain the welcome draft and never send with wrong permissions (${platform})`, async () => {
    const { page, errors } = await openApp(platform);
    try {
      await page.getByRole("button", { name: "Agent", exact: true }).click();
      await page.getByRole("switch", { name: "Enable agent mode" }).click();
      await page.getByRole("button", { name: "Save changes", exact: true }).click();
      await page.getByRole("dialog", { name: "Agent settings" }).waitFor({ state: "hidden" });
      await page.evaluate(() => (window as any).__deletionTest.failAgent(true));
      await page.locator("textarea[data-chat-input]").fill("Keep my draft");
      await page.locator("textarea[data-chat-input]").press("Enter");
      await page.getByText("Working directory is unavailable", { exact: true }).waitFor();
      assert.equal(await page.locator("textarea[data-chat-input]").inputValue(), "Keep my draft");
      assert.equal((await calls(page)).some((call) => call.method === "prompt"), false);
      assert.equal(await page.getByRole("button", { name: "Agent", exact: true }).getAttribute("aria-pressed"), "true");
      await page.evaluate(() => (window as any).__deletionTest.failAgent(false));
      await page.locator("textarea[data-chat-input]").press("Enter");
      await page.waitForFunction(() => (window as any).__deletionTest.calls.some((call: any) => call.method === "prompt"));
      assert.deepEqual(errors, []);
    } finally { await page.close(); }
  });

  test(`Agent modal keeps failed edits local and restores focus on cancellation (${platform})`, async () => {
    const { page, errors } = await openApp(platform);
    try {
      await page.getByRole("button", { name: "Viewed Existing thread", exact: true }).click();
      await page.getByRole("button", { name: "Agent", exact: true }).click();
      const dialog = page.getByRole("dialog", { name: "Agent settings" });
      await dialog.getByRole("switch", { name: "Enable agent mode" }).click();
      await dialog.getByText("Full access", { exact: true }).click();
      const input = dialog.getByLabel("Working directory", { exact: true });
      await input.fill("/unavailable/project");
      await page.evaluate(() => (window as any).__deletionTest.failAgent(true));
      await input.press("Enter");
      await dialog.getByRole("alert").waitFor();
      assert.equal(await page.locator("dialog:modal").count(), 1);
      assert.equal(await input.inputValue(), "/unavailable/project");
      assert.equal(await dialog.getByLabel("Full access", { exact: true }).isChecked(), true);
      assert.equal(await page.locator("[data-agent-toggle]").getAttribute("aria-pressed"), "false");
      assert.equal((await calls(page)).some((call) => call.method === "prompt"), false);
      await page.keyboard.press("Escape");
      assert.equal(await page.locator("dialog:modal").count(), 0);
      assert.equal(await page.locator("[data-agent-toggle]").evaluate((element) => document.activeElement === element), true);
      await page.keyboard.press("Enter");
      assert.equal(await dialog.getByRole("switch", { name: "Enable agent mode" }).getAttribute("aria-checked"), "false");
      assert.equal(await dialog.getByLabel("Approve commands", { exact: true }).isChecked(), true);
      assert.equal(await dialog.getByRole("alert").count(), 0);
      assert.deepEqual(errors, []);
    } finally { await page.close(); }
  });

  test(`stopped partial replies stay readable and copyable without cancellation warnings (${platform})`, async () => {
    const { page, errors } = await openApp(platform);
    try {
      await page.getByRole("button", { name: "Viewed Existing thread", exact: true }).click();
      await page.evaluate(() => {
        (window as any).__copiedPartialReplies = [];
        Object.defineProperty(navigator, "clipboard", { configurable: true, value: {
          writeText: async (text: string) => { (window as any).__copiedPartialReplies.push(text); },
        } });
      });
      for (const status of ["cancelled", "interrupted"]) {
        await page.evaluate((status) => (window as any).__deletionTest.setTimeline([
          { id: `partial-${status}`, turnId: "stopped-turn", kind: "assistant", text: `Keep the ${status} partial answer.`, status, terminalVerified: false },
        ], false), status);
        const transcript = page.getByRole("region", { name: "Chat messages" });
        await transcript.getByText(`Keep the ${status} partial answer.`, { exact: true }).waitFor();
        assert.doesNotMatch(await transcript.innerText(), /before verified completion|unverified|cannot be copied|Generating|E2EE/);
        const copy = transcript.getByRole("button", { name: "Copy", exact: true });
        assert.equal(await copy.isEnabled(), true);
        await copy.click();
        await transcript.getByRole("button", { name: "Copied", exact: true }).waitFor();
      }
      assert.deepEqual(await page.evaluate(() => (window as any).__copiedPartialReplies), [
        "Keep the cancelled partial answer.", "Keep the interrupted partial answer.",
      ]);
      assert.deepEqual(errors, []);
    } finally { await page.close(); }
  });

  test(`messages queue during a reply and Send now waits for a safe boundary (${platform})`, async () => {
    const { page, errors } = await openApp(platform);
    try {
      await page.getByRole("button", { name: "Viewed Existing thread", exact: true }).click();
      await page.evaluate(() => (window as any).__deletionTest.setTimeline([
        { id: "intro", kind: "assistant", text: "I am checking that now.", status: "streaming" },
      ], true));
      const queue = page.getByRole("region", { name: "Message queue", exact: true });
      await page.locator("textarea").fill("After that, summarize the result.");
      await page.getByRole("button", { name: "Stop generating", exact: true }).waitFor();
      await page.getByRole("button", { name: "Queue message", exact: true }).click();
      await queue.getByText("After that, summarize the result.", { exact: true }).waitFor();
      await page.locator("textarea").fill("Use the revised requirements instead.");
      await page.locator("textarea").press("Enter");
      await page.waitForFunction(() => document.querySelectorAll("[data-queued-message]").length === 2);
      assert.equal(await page.locator("textarea").inputValue(), "");
      assert.equal((await calls(page)).filter((call) => ["prompt", "steer", "cancel"].includes(call.method)).length, 0);
      await queue.getByRole("button", { name: "Send now", exact: true }).last().click();
      await queue.getByText("Waiting for current step to finish…", { exact: true }).waitFor();
      assert.deepEqual((await calls(page)).filter((call) => ["prompt", "steer", "cancel"].includes(call.method)).map((call) => [call.method, call.text]), [
        ["steer", "Use the revised requirements instead."],
      ]);
      assert.equal(await page.getByRole("region", { name: "Chat messages" }).getByText("Use the revised requirements instead.", { exact: true }).count(), 0,
        "steering is not presented as delivered before the boundary acknowledgement");
      if (platform === "linux") await page.screenshot({ path: "/tmp/axiom-message-queue.png" });
      await page.evaluate(() => (window as any).__deletionTest.finishSteers());
      await page.getByRole("region", { name: "Chat messages" }).getByText("Use the revised requirements instead.", { exact: true }).waitFor();
      await page.waitForFunction(() => document.querySelectorAll("[data-queued-message]").length === 1);
      const transcript = await page.getByRole("region", { name: "Chat messages" }).innerText();
      assert.ok(transcript.indexOf("I am checking") < transcript.indexOf("Use the revised"));
      assert.ok(transcript.indexOf("Use the revised") < transcript.indexOf("Continuing with your update"));
      assert.equal((await calls(page)).filter((call) => call.method === "prompt").length, 0);
      await page.evaluate(() => (window as any).__deletionTest.finishTurn());
      await queue.waitFor({ state: "hidden" });
      assert.deepEqual((await calls(page)).filter((call) => call.method === "prompt").map((call) => call.text), ["After that, summarize the result."]);
      assert.deepEqual(errors, []);
    } finally { await page.close(); }
  });

  test(`queued messages recover paused after reload and can be removed (${platform})`, async () => {
    const { page, errors } = await openApp(platform);
    try {
      await page.getByRole("button", { name: "Viewed Existing thread", exact: true }).click();
      await page.evaluate(() => (window as any).__deletionTest.setTimeline([], true));
      await page.locator("textarea").fill("Keep this pending across restart.");
      await page.locator("textarea").press("Enter");
      await page.getByRole("region", { name: "Message queue", exact: true }).waitFor();
      await page.reload();
      await page.getByRole("button", { name: "Viewed Existing thread", exact: true }).click();
      const queue = page.getByRole("region", { name: "Message queue", exact: true });
      await queue.getByText("Restored after reconnect. Review and send when ready.", { exact: true }).waitFor();
      assert.equal((await calls(page)).filter((call) => call.method === "prompt").length, 0);
      await queue.getByRole("button", { name: "Remove queued message", exact: true }).click();
      await queue.waitFor({ state: "hidden" });
      assert.deepEqual(errors, []);
    } finally { await page.close(); }
  });

  test(`streaming follows through hover and layout changes, pausing only for user interaction (${platform})`, async () => {
    const { page, errors } = await openApp(platform);
    const feed = (count: number, running = true) => page.evaluate(({ count, running }) => {
      (window as any).__deletionTest.setTimeline([{
        id: "streaming-reply", kind: "assistant", turnId: "scroll-turn",
        text: Array.from({ length: count }, (_, index) => `Paragraph ${index + 1}: more streamed text arrives here.`).join("\n\n"),
        status: running ? "streaming" : "completed",
      }], running);
    }, { count, running });
    const atBottom = () => page.waitForFunction(() => {
      const el = document.querySelector<HTMLElement>("[data-chat-scroll]")!;
      return el.scrollHeight > el.clientHeight && el.scrollHeight - el.clientHeight - el.scrollTop <= 2;
    });
    const settle = () => page.evaluate(() => new Promise<void>((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))));
    try {
      await page.getByRole("button", { name: "Viewed Existing thread", exact: true }).click();
      const viewport = page.getByRole("region", { name: "Chat messages" });
      const jump = page.getByRole("button", { name: "Scroll to latest", exact: true });
      await feed(25);
      await atBottom();
      await page.locator(".markdown-prose p").last().hover();
      for (const count of [30, 40, 50]) {
        await feed(count);
        await atBottom();
        assert.equal(await jump.count(), 0, "hover must never disable following");
      }
      // A layout/anchoring scroll event can arrive with a newly opened gap,
      // before the next follow frame. It is not an intentional upward scroll.
      await viewport.evaluate((el) => {
        (el.firstElementChild as HTMLElement).style.paddingBottom = "350px";
        el.dispatchEvent(new Event("scroll"));
      });
      await atBottom();
      assert.equal(await jump.count(), 0);
      await viewport.evaluate((el) => { (el.firstElementChild as HTMLElement).style.paddingBottom = "32px"; });
      await atBottom();

      const beforeWheel = await viewport.evaluate((el) => el.scrollTop);
      await page.mouse.wheel(0, -300);
      await jump.waitFor();
      await page.waitForFunction((before) => document.querySelector("[data-chat-scroll]")!.scrollTop <= before - 299, beforeWheel);
      const pausedTop = await viewport.evaluate((el) => el.scrollTop);
      await feed(60);
      await settle();
      assert.equal(await viewport.evaluate((el) => el.scrollTop), pausedTop, "scrolling up preserves the reading position");

      await page.mouse.wheel(0, 10_000);
      await atBottom();
      await jump.waitFor({ state: "hidden" });
      await feed(65);
      await atBottom();

      await page.locator(".markdown-prose p").last().click();
      await jump.waitFor();
      const clickedTop = await viewport.evaluate((el) => el.scrollTop);
      await feed(70);
      await settle();
      assert.equal(await viewport.evaluate((el) => el.scrollTop), clickedTop, "clicking text pauses even while already at the bottom");
      await jump.click();
      await atBottom();
      await jump.waitFor({ state: "hidden" });
      await feed(75, false);
      await atBottom();

      await viewport.dispatchEvent("pointerdown", { button: 0, pointerType: "mouse" });
      await viewport.evaluate((el) => { el.scrollTop -= 200; });
      await jump.waitFor();
      await viewport.dispatchEvent("pointerup", { button: 0, pointerType: "mouse" });
      const draggedTop = await viewport.evaluate((el) => el.scrollTop);
      await feed(80);
      await settle();
      assert.equal(await viewport.evaluate((el) => el.scrollTop), draggedTop, "scrollbar scrolling upward also pauses following");
      await jump.click();
      await atBottom();

      await viewport.evaluate((el) => {
        const touch = (y: number) => new Touch({ identifier: 1, target: el, clientX: 100, clientY: y });
        el.dispatchEvent(new TouchEvent("touchstart", { bubbles: true, touches: [touch(100)] }));
        el.dispatchEvent(new TouchEvent("touchmove", { bubbles: true, touches: [touch(200)] }));
      });
      await jump.waitFor();
      await jump.click();
      await atBottom();
      // Keyboard scrolling has its own compositor animation; exercise it last
      // so it cannot continue moving the viewport during the drag assertion.
      await viewport.focus();
      await viewport.press("ArrowUp");
      await jump.waitFor();
      await jump.click();
      await atBottom();
      assert.deepEqual(errors, []);
    } finally { await page.close(); }
  });

  test(`a stale desktop bridge cannot bypass Web-off after a renderer update (${platform})`, async () => {
    const { page, errors } = await openApp(platform);
    try {
      await page.evaluate(() => {
        const agent = (window as any).axiomDesktop.agent;
        agent.prompt = agent.promptWithWebConsent; // The pre-consent preload API.
        delete agent.promptWithWebConsent;
      });
      assert.equal(await page.getByRole("button", { name: "Web", exact: true }).getAttribute("aria-pressed"), "false");
      await page.locator("textarea").fill("Search the web for the latest news");
      await page.locator("textarea").press("Enter");
      await page.getByText("Restart Axiom to apply the Web privacy controls. No message was sent.", { exact: true }).waitFor();
      assert.equal(await page.locator("textarea").inputValue(), "Search the web for the latest news");
      assert.deepEqual((await calls(page)).filter((call) => call.method === "prompt" || call.method === "new"), []);
      assert.deepEqual(errors, []);
    } finally { await page.close(); }
  });

  test(`Web is off by default, requires consent, and sends an explicit per-message choice (${platform})`, async () => {
    const { page, errors, nativeDialogs } = await openApp(platform);
    try {
      const web = page.getByRole("button", { name: "Web", exact: true });
      assert.equal(await web.getAttribute("aria-pressed"), "false");
      await page.locator("textarea").fill("Keep my draft");
      await web.click();
      const warning = page.getByRole("alertdialog", { name: "Turn on Web?" });
      await warning.waitFor();
      assert.match(await warning.innerText(), /Web searches run outside Axiom’s verified private environment/);
      assert.doesNotMatch(await warning.innerText(), /Decodo|SearXNG/i);
      if (platform === "linux") {
        await page.emulateMedia({ colorScheme: "dark" });
        await page.waitForFunction(() => getComputedStyle(document.querySelector("dialog")!).color === "rgb(244, 244, 246)");
        await page.screenshot({ path: "/tmp/axiom-web-consent.png" });
      }
      assert.equal(await page.getByRole("button", { name: "No thanks", exact: true }).evaluate((el) => el === document.activeElement), true);
      await page.getByRole("checkbox", { name: "Don’t show this warning again" }).check();
      await page.getByRole("button", { name: "No thanks", exact: true }).click();
      assert.equal(await web.getAttribute("aria-pressed"), "false");
      assert.equal(await page.locator("textarea").inputValue(), "Keep my draft");
      assert.equal(await page.evaluate(() => Object.keys(localStorage).filter((key) => key.startsWith("axiom.web-warning")).length), 0);
      await web.click();
      await warning.waitFor();
      await page.keyboard.press("Escape");
      await warning.waitFor({ state: "detached" });
      assert.equal(await web.getAttribute("aria-pressed"), "false");
      await web.click();
      await page.getByRole("button", { name: "Yes, I understand", exact: true }).click();
      assert.equal(await web.getAttribute("aria-pressed"), "true");
      await page.getByRole("button", { name: "Send", exact: true }).click();
      await page.waitForFunction(() => (window as unknown as { __deletionTest: { calls: { method: string }[] } }).__deletionTest.calls.some((call) => call.method === "prompt"));
      const flags = () => page.evaluate(() => (window as unknown as { __deletionTest: { calls: { method: string; webEnabled?: boolean }[] } }).__deletionTest.calls.filter((call) => call.method === "prompt").map((call) => call.webEnabled));
      assert.deepEqual(await flags(), [true]);
      assert.equal(await web.getAttribute("aria-pressed"), "true", "welcome opt-in transfers to the created chat");
      await web.click();
      assert.equal(await web.getAttribute("aria-pressed"), "false");
      await page.locator("textarea").fill("No web this time");
      await page.getByRole("button", { name: "Send", exact: true }).click();
      await page.waitForFunction(() => (window as unknown as { __deletionTest: { calls: { method: string }[] } }).__deletionTest.calls.filter((call) => call.method === "prompt").length === 2);
      assert.deepEqual(await flags(), [true, false]);
      await web.click();
      await warning.waitFor(); // Not remembered: every off -> on asks again.
      await page.getByRole("checkbox", { name: "Don’t show this warning again" }).check();
      await page.getByRole("button", { name: "Yes, I understand", exact: true }).click();
      await page.getByRole("button", { name: "New thread", exact: true }).click();
      assert.equal(await web.getAttribute("aria-pressed"), "false", "new chat always starts off");
      await web.click();
      assert.equal(await web.getAttribute("aria-pressed"), "true");
      assert.equal(await warning.count(), 0, "remembering the warning does not enable a new draft without clicking Web");
      if (platform === "linux") {
        await page.reload();
        await web.waitFor();
        assert.equal(await web.getAttribute("aria-pressed"), "false", "a fresh welcome draft always starts off");
        await web.click();
        assert.equal(await web.getAttribute("aria-pressed"), "true");
        assert.equal(await warning.count(), 0, "warning preference survives reload");
      }
      assert.deepEqual(errors, []);
      assert.deepEqual(nativeDialogs, []);
    } finally { await page.close(); }
  });
}

test("an account switch dismisses web consent and never transfers the previous account's preference", async () => {
  const { page, errors } = await openApp();
  try {
    const web = page.getByRole("button", { name: "Web", exact: true });
    await web.click();
    await page.getByRole("alertdialog", { name: "Turn on Web?" }).waitFor();
    await page.evaluate(() => (window as unknown as { __deletionTest: { setAccount: (id: string) => void } }).__deletionTest.setAccount("account-b"));
    await page.getByRole("alertdialog").waitFor({ state: "detached" });
    assert.equal(await web.getAttribute("aria-pressed"), "false");
    await web.click();
    await page.getByRole("alertdialog", { name: "Turn on Web?" }).waitFor();
    await page.getByRole("button", { name: "No thanks", exact: true }).click();
    await page.locator("textarea").fill("Still usable");
    assert.deepEqual(errors, []);
  } finally { await page.close(); }
});

for (const platform of ["linux", "win32", "darwin"]) {
  test(`Web remembers both on and off per chat across renderer and sidecar restarts (${platform})`, async () => {
    const { page, errors } = await openApp(platform);
    try {
      const web = page.getByRole("button", { name: "Web", exact: true });
      const warning = page.getByRole("alertdialog", { name: "Turn on Web?" });
      const openThread = async () => {
        await page.locator('[data-thread-id="old-thread"]').click();
        await page.locator("[data-chat-scroll]").waitFor();
      };
      const expectWeb = (enabled: boolean) => page.waitForFunction((enabled) =>
        document.querySelector("[data-web-toggle]")?.getAttribute("aria-pressed") === String(enabled), enabled);
      await openThread();
      await expectWeb(false);
      await web.click();
      await page.getByRole("button", { name: "Yes, I understand", exact: true }).click();
      await expectWeb(true);
      assert.equal((await calls(page)).filter((call) => call.method === "prompt").length, 0, "toggle saves without a message");

      await page.reload();
      await web.waitFor();
      await expectWeb(false); // New welcome draft, not an account-wide opt-in.
      await openThread();
      await expectWeb(true);
      assert.equal(await warning.count(), 0, "restoring explicit consent does not re-prompt");
      await page.locator("textarea").fill("Restored Web choice");
      await page.getByRole("button", { name: "Send", exact: true }).click();
      await page.waitForFunction(() => (window as any).__deletionTest.calls.some((call: any) => call.method === "prompt" && call.webEnabled === true));

      await page.evaluate(() => (window as any).__deletionTest.setRuntime("restarted-runtime", false));
      await expectWeb(false); // A disconnected runtime cannot inherit permission.
      await page.evaluate(() => (window as any).__deletionTest.setRuntime("restarted-runtime"));
      await expectWeb(true);
      await page.evaluate(() => (window as any).__deletionTest.setAccount("another-account"));
      await expectWeb(false);
      await page.evaluate(() => (window as any).__deletionTest.setAccount("test-account"));
      await expectWeb(true);

      await web.click();
      await expectWeb(false); // Persist off without sending another message.
      await page.reload();
      await web.waitFor();
      await openThread();
      await expectWeb(false);
      await page.locator("textarea").fill("Restored off choice");
      await page.getByRole("button", { name: "Send", exact: true }).click();
      await page.waitForFunction(() => (window as any).__deletionTest.calls.some((call: any) => call.method === "prompt" && call.webEnabled === false));
      await web.click();
      await warning.waitFor(); // No warning-checkbox opt-in was saved above.
      await page.getByRole("button", { name: "No thanks", exact: true }).click();
      await expectWeb(false);
      assert.deepEqual(errors, []);
    } finally { await page.close(); }
  });
}

test("saved Web choices from the welcome draft stay with its new thread and are removed on deletion", async () => {
  const { page, errors } = await openApp();
  try {
    const web = page.getByRole("button", { name: "Web", exact: true });
    await web.click();
    await page.getByRole("button", { name: "Yes, I understand", exact: true }).click();
    await page.locator("textarea").fill("Create with Web on");
    await page.getByRole("button", { name: "Send", exact: true }).click();
    await page.waitForFunction(() => (window as any).__deletionTest.calls.some((call: any) => call.method === "prompt"));
    assert.equal(await page.evaluate(() => localStorage.getItem('axiom.thread-web.v1:["test-account","new-thread"]')), "enabled");
    assert.equal(await page.evaluate(() => localStorage.getItem('axiom.thread-web.v1:["test-account","new"]')), null);
    await page.getByRole("button", { name: "New thread", exact: true }).click();
    assert.equal(await web.getAttribute("aria-pressed"), "false");
    await page.locator('[data-thread-id="new-thread"]').click();
    await page.waitForFunction(() => document.querySelector("[data-web-toggle]")?.getAttribute("aria-pressed") === "true");
    await page.locator('[data-thread-id="new-thread"]').click({ button: "right" });
    await page.getByRole("menuitem", { name: "Delete", exact: true }).click();
    await page.getByRole("alertdialog").getByRole("button", { name: "Delete thread", exact: true }).click();
    await page.waitForFunction(() => localStorage.getItem('axiom.thread-web.v1:["test-account","new-thread"]') === null);
    assert.deepEqual(errors, []);
  } finally { await page.close(); }
});

test("Web storage failures never silently enable Web or claim an opt-out was saved", async () => {
  const { page, errors } = await openApp();
  try {
    const web = page.getByRole("button", { name: "Web", exact: true });
    await page.locator('[data-thread-id="old-thread"]').click();
    await page.locator("[data-chat-scroll]").waitFor();
    await page.evaluate(() => {
      const setItem = Storage.prototype.setItem;
      Object.assign(window, { __restoreWebStorage: () => { Storage.prototype.setItem = setItem; } });
      Storage.prototype.setItem = () => { throw new Error("Storage full"); };
    });
    await web.click();
    await page.getByRole("button", { name: "Yes, I understand", exact: true }).click();
    await page.getByText("Web could not be saved and remains off. Please try again.", { exact: true }).waitFor();
    assert.equal(await web.getAttribute("aria-pressed"), "false");
    await page.evaluate(() => (window as any).__restoreWebStorage());
    await web.click();
    await page.getByRole("button", { name: "Yes, I understand", exact: true }).click();
    assert.equal(await web.getAttribute("aria-pressed"), "true");
    await page.evaluate(() => { Storage.prototype.setItem = () => { throw new Error("Storage full"); }; });
    await web.click();
    await page.getByText("Web is off, but could not be saved. It may turn back on after restarting.", { exact: true }).waitFor();
    assert.equal(await web.getAttribute("aria-pressed"), "false");
    await page.evaluate(() => (window as any).__restoreWebStorage());
    assert.deepEqual(errors, []);
  } finally { await page.close(); }
});

test("usage distinguishes a failed partial request, exact turn totals, and auxiliary charges", async () => {
  const { page, errors } = await openApp();
  try {
    await page.locator('[data-thread-id="old-thread"]').click();
    await page.locator("textarea").waitFor();
    await page.evaluate(() => {
      const base = { providerId:"near", modelId:"test-model", purpose:"conversation", turnId:"t",
        state:"completed", completeness:"final", settled:true, responseVerified:true,
        startedAtMs:"1000", finishedAtMs:"2000", contextWindowTokens:1000000, autoCompactThresholdTokens:850000,
        inputTokens:"77326", outputTokens:"266", costMicrousd:"1000" };
      (window as any).__deletionTest.setRequestUsage([
        { ...base, requestId:"a".repeat(32) },
        { ...base, requestId:"b".repeat(32), inputTokens:"81173", outputTokens:"3495", costMicrousd:"9285",
          state:"failed", completeness:"partial", responseVerified:false, startedAtMs:"2100", finishedAtMs:"4000" },
        { ...base, requestId:"c".repeat(32), purpose:"title", turnId:null, inputTokens:"10", outputTokens:"5", costMicrousd:"100" },
      ]);
    });
    await page.locator("[data-context-usage]").hover();
    const tooltip = page.getByRole("tooltip");
    await tooltip.waitFor();
    const text = await tooltip.innerText();
    assert.match(text, /Partial request usage/);
    assert.match(text, /84,668 tokens/);
    assert.match(text, /158,499 input · 3,761 output/);
    assert.match(text, /This turn · 2 model requests/);
    assert.match(text, /\$0\.010285 settled/);
    assert.match(text, /Titles · \$0\.000100 settled/);
    assert.deepEqual(errors, []);
  } finally { await page.close(); }
});

for (const platform of ["linux", "win32", "darwin"]) {
  test(`context ring replaces shortcut hint and exposes live usage on hover and focus (${platform})`, async () => {
    const { page, errors } = await openApp(platform);
    try {
      const indicator = page.locator("[data-context-usage]");
      await indicator.waitFor();
      assert.equal(await indicator.innerText(), "—");
      assert.doesNotMatch(await page.locator(".composer-shell").innerText(), /Ctrl|⌘|↵/);
      await page.locator('[data-thread-id="old-thread"]').click();
      await page.locator("textarea").waitFor();
      assert.equal(await indicator.innerText(), "—");
      const setUsage = (total: number, modelId = "test-model") => page.evaluate((usage) => (window as any).__deletionTest.setContextUsage(usage), {
        inputTokens: total - 1_000, outputTokens: 1_000, modelId,
        reportedAt: "2026-09-07T00:00:00Z", contextWindowTokens: 100_000, autoCompactThresholdTokens: 85_000,
      });
      await setUsage(76_000);
      await page.waitForFunction(() => document.querySelector("[data-context-usage]")?.getAttribute("aria-valuenow") === "76");
      await indicator.hover();
      const tooltip = page.getByRole("tooltip");
      await tooltip.waitFor();
      assert.equal(await indicator.getAttribute("aria-label"), "Usage");
      assert.equal(await tooltip.getByText("Usage", { exact: true }).count(), 1);
      assert.doesNotMatch(await tooltip.innerText(), /Last reported usage|Provider counts from|Edits and compaction/);
      assert.match(await tooltip.innerText(), /76,000 tokens/);
      assert.match(await tooltip.innerText(), /75,000 input · 1,000 output/);
      assert.doesNotMatch(await tooltip.innerText(), /About |Estimated/);
      assert.match(await tooltip.innerText(), /100,000 token context window/);
      assert.match(await tooltip.innerText(), /76% used/);
      assert.match(await tooltip.innerText(), /85,000 tokens \(85%\)/);
      const tooltipBox = await tooltip.boundingBox();
      assert.ok(tooltipBox && tooltipBox.x >= 0 && tooltipBox.y >= 0 && tooltipBox.x + tooltipBox.width <= 1320);
      // Moving onto the tooltip keeps it open; it contains no OS/native dialog.
      await tooltip.hover();
      assert.equal(await tooltip.isVisible(), true);
      await page.locator("textarea").hover();
      await tooltip.waitFor({ state: "detached" });
      await indicator.focus();
      await tooltip.waitFor();
      await indicator.press("Escape");
      await tooltip.waitFor({ state: "detached" });
      await indicator.blur();
      await indicator.focus();
      await page.locator("textarea").fill("Unsent changes");
      await page.evaluate(() => (window as any).__deletionTest.setTimeline([{ id: "compact", kind: "activity", text: "Context compacted" }], true));
      assert.equal(await indicator.getAttribute("aria-valuenow"), "76");
      await indicator.hover();
      await tooltip.waitFor();
      assert.doesNotMatch(await tooltip.innerText(), /Provider counts from|Edits and compaction|Updates when the current request finishes/);
      await setUsage(3_000); // new provider report after compaction
      await page.getByText("3,000 tokens", { exact: true }).waitFor();
      assert.equal(await indicator.getAttribute("aria-valuenow"), "3");
      await page.evaluate(() => (window as any).__deletionTest.setTimeline([]));
      await setUsage(76_000, "previous-model");
      // Finishing the turn removes Stop beside a nonempty draft, moving the
      // meter away from the pointer. Hover its new position explicitly.
      await indicator.hover();
      await page.getByText("Reported for previous-model", { exact: true }).waitFor();
      if (platform === "linux") {
        await setUsage(76_000);
        await page.evaluate(() => { document.documentElement.dataset.theme = "dark"; });
        await page.waitForTimeout(350); // allow the theme's color transition to settle
        await page.screenshot({ path: "/tmp/axiom-context-usage.png" });
      }
      await page.getByRole("button", { name: "New thread", exact: true }).click();
      assert.equal(await indicator.innerText(), "—");
      await page.locator("textarea").fill("Composer still works");
      assert.deepEqual(errors, []);
    } finally { await page.close(); }
  });

  test(`fetches and errors show readable, non-expandable summaries (${platform})`, async () => {
    const { page, errors } = await openApp(platform);
    try {
      await page.locator('[data-thread-id="old-thread"]').click();
      await page.locator("textarea").waitFor();
      const fetch = (id: string, url: string, status: string, output: string): ClientTimelineItem => ({
        id, turnId: "turn", kind: "tool", text: "fetch_url", status, tool: {
          callId: id, title: "fetch_url", name: "fetch_url", kind: "fetch", status,
          input: { url }, locations: [], content: [{ type: "text", text: output }],
        },
      });
      const timeline = [
        fetch("ok", "https://en.wikipedia.org/wiki/UTC", "completed", '{"content":"RAW_FETCHED_HTML_AND_SCRIPT"}'),
        fetch("blocked", "https://www.timeanddate.com/time/time-zones-interesting.html", "failed", "tool error: fetch returned HTTP 403 Forbidden"),
        fetch("missing", "https://example.com/missing", "failed", "tool error: fetch returned HTTP 404 Not Found"),
        fetch("timeout", "https://example.com/slow", "failed", "tool error: operation timed out"),
        { id: "search", turnId: "turn", kind: "tool", text: "web_search", status: "in_progress", tool: {
          callId: "search", name: "web_search", title: "web_search", kind: "search", status: "in_progress",
          input: { query: "time zones" }, locations: [], content: [],
        } } as ClientTimelineItem,
      ];
      const setTimeline = (items: ClientTimelineItem[]) => page.evaluate((items) => (window as any).__deletionTest.setTimeline(items), items);
      await setTimeline(timeline);
      await page.getByText("Fetched en.wikipedia.org/wiki/UTC", { exact: true }).waitFor();
      await page.getByText("Blocked by website · www.timeanddate.com/time/time-zones-interesting.html", { exact: true }).waitFor();
      await page.getByText("Page not found · example.com/missing", { exact: true }).waitFor();
      await page.getByText("Request timed out · example.com/slow", { exact: true }).waitFor();
      for (const id of ["ok", "blocked", "missing", "timeout"]) {
        const row = page.locator(`[data-tool-call-id="${id}"]`);
        assert.equal(await row.locator("button, pre, [aria-expanded]").count(), 0);
        await row.click();
        assert.equal(await row.locator("pre").count(), 0);
      }
      assert.doesNotMatch(await page.locator(".chat-timeline").innerText(), /RAW_FETCHED|tool error:|HTTP 403|Forbidden/);
      // An already-expanded running tool must drop its raw details on failure.
      await page.getByRole("button", { name: /Searching the web for “time zones”/ }).click();
      await page.locator('[data-tool-call-id="search"] pre').waitFor();
      timeline[4]!.status = "failed";
      timeline[4]!.tool!.status = "failed";
      timeline[4]!.tool!.content = [{ type: "text", text: "Search limit reached (60 per minute per account). Retry after 20 seconds." }];
      await setTimeline(timeline);
      await page.getByText("Search limit reached · Web search for “time zones”", { exact: true }).waitFor();
      assert.equal(await page.locator(".chat-timeline button, .chat-timeline pre, .chat-timeline [aria-expanded]").count(), 0);
      if (platform === "linux") {
        await page.emulateMedia({ colorScheme: "dark" });
        await page.evaluate(() => document.getAnimations().forEach((animation) => animation.finish()));
        await page.screenshot({ path: "/tmp/axiom-readable-tools.png" });
      }
      assert.deepEqual(errors, []);
    } finally { await page.close(); }
  });

  test(`inline tools remain compact, chronological, and keyboard-expandable (${platform})`, async () => {
    const { page, errors } = await openApp(platform);
    try {
      await page.locator('[data-thread-id="old-thread"]').click();
      await page.locator("textarea").waitFor();
      const text = (id: string, content: string): ClientTimelineItem => ({
        id, turnId: "turn", kind: "assistant", text: content, status: "completed", terminalVerified: true,
      });
      const tool = (id: string, query: string): ClientTimelineItem => ({
        id, turnId: "turn", kind: "tool", text: "web_search", status: "completed", tool: {
          callId: id, title: "web_search", name: "web_search", kind: "search", status: "completed",
          input: { query }, locations: [], content: [{ type: "text", text: JSON.stringify({ results: [
            { title: "Current news — source article", url: "https://example.com/news", snippet: "A short summary of the latest headlines." },
            { title: "<script>unsafe()</script>", url: "javascript:alert(1)" },
          ] }) }],
        },
      });
      const timeline = [
        { ...text("user", "What's in the news?"), kind: "user" as const },
        text("intro", "Let me check the web for that:"),
        tool("search-1", "current news"), tool("search-2", "current news source confirmation"),
        text("answer", "The current news is xyz. Here are the details from the sources above."),
      ];
      await page.evaluate((timeline) => {
        (window as unknown as { __deletionTest: { setTimeline: (items: ClientTimelineItem[], running?: boolean) => void } }).__deletionTest.setTimeline(timeline);
      }, timeline);
      const cards = page.locator('[data-timeline-kind="tool"]');
      await cards.nth(1).waitFor();
      await page.evaluate(() => document.getAnimations().forEach((animation) => animation.finish()));
      assert.deepEqual(await page.locator(".chat-timeline > [data-timeline-kind]").evaluateAll((nodes) => nodes.map((node) => node.getAttribute("data-timeline-kind"))), [
        "user", "assistant", "tool", "tool", "assistant",
      ]);
      const first = (await cards.nth(0).boundingBox())!;
      const second = (await cards.nth(1).boundingBox())!;
      assert.ok(first.height >= 28 && first.height <= 34, `compact tool height ${first.height}`);
      assert.ok(second.y - first.y - first.height <= 3, "adjacent tools only have a 2px gap");
      assert.equal(await page.getByRole("button", { name: "Copy", exact: true }).count(), 1);
      const search = page.getByRole("button", { name: "Searched the web for “current news”; show details", exact: true });
      await search.focus();
      await page.keyboard.press("Enter");
      assert.equal(await search.getAttribute("aria-expanded"), "true");
      const source = page.getByRole("link", { name: "Current news — source article" });
      await source.waitFor();
      assert.equal(await source.getAttribute("href"), "https://example.com/news");
      assert.equal(await page.getByRole("link", { name: /unsafe/ }).count(), 0);
      if (platform === "linux") {
        await page.emulateMedia({ colorScheme: "dark" });
        await page.waitForFunction(() => {
          const button = document.querySelector('[data-tool-call-id="search-1"] button');
          return button && getComputedStyle(button).color === "rgb(180, 180, 186)";
        });
        await search.evaluate((element) => (element as HTMLElement).blur());
        await page.evaluate(() => document.getAnimations().forEach((animation) => animation.finish()));
        await page.screenshot({ path: "/tmp/axiom-inline-tools.png" });
      }
      await search.focus();
      await page.keyboard.press("Space");
      assert.equal(await search.getAttribute("aria-expanded"), "false");
      assert.equal(await source.count(), 0);
      // Tool-only running phase owns the indicator, without a stale blank reply.
      const running = timeline.slice(0, 3);
      running[1] = { ...text("placeholder", ""), status: "in_progress" };
      running[2]!.tool!.status = "in_progress";
      await page.evaluate((timeline) => {
        (window as unknown as { __deletionTest: { setTimeline: (items: ClientTimelineItem[], running: boolean) => void } }).__deletionTest.setTimeline(timeline, true);
      }, running);
      await page.getByRole("button", { name: /Searching the web for/ }).waitFor();
      assert.equal(await page.getByText("Generating", { exact: true }).count(), 0);
      assert.deepEqual(errors, []);
    } finally { await page.close(); }
  });
}

test("right-click menus dismiss outside, on Escape/Tab, scroll, and replacement without selecting rows", async () => {
  const { page, errors, nativeDialogs } = await openApp();
  try {
    const thread = page.locator('[data-thread-id="old-thread"]');
    const folder = page.locator('[data-folder-id="folder"]');
    await thread.click({ button: "right" });
    assert.equal(await page.getByRole("menu", { name: "Thread actions" }).count(), 1);
    assert.deepEqual(await page.getByRole("menuitem").allTextContents(), ["Rename", "Delete"]);
    assert.deepEqual(await calls(page), [], "right click must not load a thread");
    await page.locator("textarea").click();
    await page.getByRole("menu").waitFor({ state: "detached" });
    await assertComposerFocused(page);
    await thread.click({ button: "right" });
    await folder.click({ button: "right" });
    assert.equal(await page.getByRole("menu").count(), 1);
    assert.equal(await page.getByRole("menu", { name: "Folder actions" }).count(), 1);
    assert.deepEqual(await calls(page), [], "right click must not toggle a folder");
    await page.keyboard.press("Escape");
    await page.getByRole("menu").waitFor({ state: "detached" });
    assert.equal(await folder.evaluate((element) => element === document.activeElement), true);
    await page.keyboard.press("Shift+F10");
    await page.getByRole("menu").waitFor();
    await page.keyboard.press("ArrowUp");
    assert.equal(await page.evaluate(() => document.activeElement?.textContent), "Delete");
    await page.keyboard.press("Home");
    assert.equal(await page.evaluate(() => document.activeElement?.textContent), "Rename");
    await page.keyboard.press("Tab");
    await page.getByRole("menu").waitFor({ state: "detached" });
    await thread.click({ button: "right" });
    await page.locator("aside").dispatchEvent("scroll");
    await page.getByRole("menu").waitFor({ state: "detached" });
    await thread.evaluate((element) => element.dispatchEvent(new MouseEvent("contextmenu", {
      bubbles: true, cancelable: true, clientX: 1318, clientY: 898, button: 2,
    })));
    const box = (await page.getByRole("menu").boundingBox())!;
    assert.ok(box.x >= 8 && box.x + box.width <= 1312 && box.y + box.height <= 892, `menu stays inside viewport: ${JSON.stringify(box)}`);
    await page.keyboard.press("Escape");
    await folder.dblclick();
    assert.equal(await page.locator("aside input").count(), 1, "no double-click rename input (only search)");
    assert.equal(await page.getByRole("dialog").count(), 0);
    assert.equal(await page.getByRole("button", { name: /Open actions for|Delete folder/ }).count(), 0);
    assert.deepEqual(errors, []);
    assert.deepEqual(nativeDialogs, []);
  } finally { await page.close(); }
});

for (const kind of ["thread", "folder"] as const) {
  test(`${kind} right-click rename saves, validates, cancels, and recovers from failure`, async () => {
    const { page, errors, nativeDialogs } = await openApp();
    try {
      const row = page.locator(kind === "thread" ? '[data-thread-id="old-thread"]' : '[data-folder-id="folder"]');
      if (kind === "thread") await row.click();
      await page.locator("textarea").fill("Keep my draft");
      const openRename = async () => {
        await row.click({ button: "right" });
        await page.getByRole("menuitem", { name: "Rename", exact: true }).click();
        await page.getByRole("dialog", { name: `Rename ${kind}` }).waitFor();
        assert.equal(await page.getByRole("menu").count(), 0);
      };
      await openRename();
      const input = page.getByRole("textbox", { name: "Name", exact: true });
      assert.equal(await input.inputValue(), kind === "thread" ? "Existing thread" : "Test folder");
      assert.equal(await input.evaluate((element: HTMLInputElement) => element.selectionEnd! - element.selectionStart!), (await input.inputValue()).length);
      await page.keyboard.press("Shift+Tab");
      assert.equal(await page.evaluate(() => document.activeElement?.textContent), "Save");
      await page.keyboard.press("Tab");
      assert.equal(await input.evaluate((element) => element === document.activeElement), true);
      await input.fill("Do not save");
      await page.keyboard.press("Escape");
      await assertComposerFocused(page);
      assert.equal((await calls(page)).filter((call) => call.method.startsWith("rename")).length, 0);
      await openRename();
      await input.fill("  ");
      assert.equal(await page.getByRole("button", { name: "Save", exact: true }).isDisabled(), true);
      await input.fill("🔒".repeat(130));
      assert.equal(await page.getByRole("button", { name: "Save", exact: true }).isDisabled(), true);
      await input.fill("  Renamed item  ");
      await page.evaluate(() => (window as any).__deletionTest.failRename(true));
      await page.getByRole("button", { name: "Save", exact: true }).click();
      await page.getByRole("alert").getByText("Rename test failure").waitFor();
      await page.evaluate(() => (window as any).__deletionTest.failRename(false));
      await page.getByRole("button", { name: "Save", exact: true }).click();
      await page.getByRole("dialog").waitFor({ state: "detached" });
      assert.equal(await row.innerText(), "Renamed item");
      await assertComposerFocused(page);
      assert.equal(await page.locator("textarea").inputValue(), "Keep my draft");
      assert.deepEqual((await calls(page)).filter((call) => call.method.startsWith("rename")), [0, 1].map(() => ({
        method: kind === "thread" ? "renameThread" : "renameFolder", id: kind === "thread" ? "old-thread" : "folder", text: "Renamed item",
      })));
      assert.deepEqual(errors, []);
      assert.deepEqual(nativeDialogs, []);
    } finally { await page.close(); }
  });
}

for (const platform of ["linux", "win32", "darwin"]) {
  test(`IME candidate confirmation preserves the draft until a separate Enter (${platform})`, async () => {
    const { page, errors } = await openApp(platform);
    try {
      const input = page.locator("textarea");
      await input.fill("日本語の下書き");
      for (const composing of [
        { isComposing: true, keyCode: 13 },
        // Some IMEs report the confirming key after compositionend.
        { isComposing: false, keyCode: 229 },
      ]) {
        const prevented = await input.evaluate((element, state) => {
          const event = new KeyboardEvent("keydown", {
            key: "Enter", code: "Enter", bubbles: true, cancelable: true, ...state,
          });
          element.dispatchEvent(event);
          return event.defaultPrevented;
        }, composing);
        assert.equal(prevented, false, "candidate confirmation must reach the input method");
        assert.equal(await input.inputValue(), "日本語の下書き");
        assert.equal((await calls(page)).some((call) => call.method === "new" || call.method === "prompt"), false);
      }
      await input.press("Shift+Enter");
      assert.equal(await input.inputValue(), "日本語の下書き\n");
      await input.press("Enter");
      await page.waitForFunction(() => (window as any).__deletionTest.calls.some((call: any) => call.method === "prompt"));
      const prompts = (await calls(page)).filter((call) => call.method === "prompt");
      assert.equal(prompts.length, 1);
      assert.equal(prompts[0]!.text, "日本語の下書き");
      assert.deepEqual(errors, []);
    } finally { await page.close(); }
  });

  test(`${platform} layout: open, delete, new thread, click, type, and submit`, async () => {
    const { page, errors, nativeDialogs } = await openApp(platform);
    try {
      await openDeleteDialog(page);
      await page.getByRole("alertdialog").getByRole("button", { name: "Delete thread", exact: true }).click();
      await page.getByRole("alertdialog").waitFor({ state: "detached" });
      await assertComposerFocused(page);
      await page.getByRole("button", { name: "New thread", exact: true }).click();
      await page.locator("textarea").click();
      await page.keyboard.type("Typing after deletion");
      assert.equal(await page.locator("textarea").inputValue(), "Typing after deletion");
      await page.keyboard.press("Enter");
      await page.waitForFunction(() => (window as any).__deletionTest.calls.some((call: any) => call.method === "prompt"));
      const recorded = await calls(page);
      assert.equal(recorded.filter((call) => call.method === "delete").length, 1);
      assert.equal(recorded.filter((call) => call.method === "load").length, 1, "deleted thread must not reload");
      assert.deepEqual(recorded.find((call) => call.method === "prompt"), { method: "prompt", id: "new-thread", text: "Typing after deletion", webEnabled: false });
      assert.deepEqual(errors, []);
      assert.deepEqual(nativeDialogs, [], "must not invoke OS confirmation dialogs");
    } finally { await page.close(); }
  });
}

for (const cancelWith of ["button", "Escape"]) {
  test(`cancel using ${cancelWith} preserves the draft, restores input, and never starts deletion`, async () => {
    const { page, errors, nativeDialogs } = await openApp();
    try {
      await page.getByRole("button", { name: "Viewed Existing thread", exact: true }).click();
      await page.locator("textarea").fill("Keep this draft");
      await page.locator('[data-thread-id="old-thread"]').click({ button: "right" });
      await page.getByRole("menuitem", { name: "Delete", exact: true }).click();
      const dialog = page.getByRole("alertdialog");
      await dialog.waitFor();
      assert.equal(await page.evaluate(() => document.activeElement?.textContent), "Cancel");
      await page.keyboard.press("Shift+Tab");
      assert.equal(await page.evaluate(() => document.activeElement?.textContent), "Delete thread");
      await page.keyboard.press("Tab");
      assert.equal(await page.evaluate(() => document.activeElement?.textContent), "Cancel");
      if (cancelWith === "button") await dialog.getByRole("button", { name: "Cancel" }).click();
      else await page.keyboard.press("Escape");
      await dialog.waitFor({ state: "detached" });
      await assertComposerFocused(page);
      await page.keyboard.type(" still editable");
      assert.equal(await page.locator("textarea").inputValue(), "Keep this draft still editable");
      assert.equal((await calls(page)).filter((call) => ["preview", "delete"].includes(call.method)).length, 0);
      assert.deepEqual(errors, []);
      assert.deepEqual(nativeDialogs, []);
    } finally { await page.close(); }
  });
}

test("deletion failure leaves the new-thread composer usable and the old thread available", async () => {
  const { page, errors, nativeDialogs } = await openApp("linux", true);
  try {
    await openDeleteDialog(page);
    await page.getByRole("alertdialog").getByRole("button", { name: "Delete thread", exact: true }).click();
    await page.getByText("Deletion test failure", { exact: true }).waitFor();
    await assertComposerFocused(page);
    await page.keyboard.type("Still works");
    assert.equal(await page.locator("textarea").inputValue(), "Still works");
    assert.equal((await calls(page)).filter((call) => call.method === "load").length, 1);
    await page.getByRole("button", { name: "Viewed Existing thread", exact: true }).click();
    await page.locator("textarea").fill("Old thread works too");
    assert.deepEqual(errors, []);
    assert.deepEqual(nativeDialogs, []);
  } finally { await page.close(); }
});

test("folder deletion uses the same modal without deleting its threads or blocking the draft", async () => {
  const { page, errors, nativeDialogs } = await openApp();
  try {
    await page.locator("textarea").fill("Folder draft");
    await page.locator('[data-folder-id="folder"]').click({ button: "right" });
    await page.getByRole("menuitem", { name: "Delete", exact: true }).click();
    await page.getByRole("alertdialog").getByRole("button", { name: "Delete folder", exact: true }).click();
    await assertComposerFocused(page);
    await page.keyboard.type(" remains editable");
    assert.equal(await page.locator("textarea").inputValue(), "Folder draft remains editable");
    await page.getByRole("button", { name: "Viewed Existing thread", exact: true }).waitFor();
    assert.deepEqual(await calls(page), [{ method: "deleteFolder", id: "folder" }]);
    assert.deepEqual(errors, []);
    assert.deepEqual(nativeDialogs, []);
  } finally { await page.close(); }
});

test("LaTeX renders in the real chat with local fonts, safe streamed updates, and scrollable wide equations", async () => {
  const { page, errors } = await openApp();
  const remoteRequests: string[] = [];
  page.on("request", (request) => {
    if (!request.url().startsWith(origin)) remoteRequests.push(request.url());
  });
  const reply = async (text: string) => page.evaluate((text) => (window as any).__deletionTest.setReply(text), text);
  try {
    await page.getByRole("button", { name: "Viewed Existing thread", exact: true }).click();
    await page.locator("textarea").waitFor();
    await reply(String.raw`Energy $E = mc^2$. Matrix:

$$
\begin{bmatrix}1 & 2 \\ 3 & 4\end{bmatrix}
$$`);
    await page.locator(".katex-display").waitFor();
    assert.equal(await page.locator(".katex").count(), 2);
    assert.equal(await page.locator(".katex-error").count(), 0);
    assert.equal(await page.evaluate(async () => {
      await document.fonts.ready;
      return (await document.fonts.load("16px KaTeX_Main")).length > 0;
    }), true, "KaTeX fonts must load from the app bundle");
    const box = await page.locator(".katex-display").boundingBox();
    assert.ok(box && box.height > 30 && box.height < 200, "matrix must be visibly typeset");
    for (const theme of ["light", "dark"]) {
      await page.evaluate((theme) => { document.documentElement.dataset.theme = theme; }, theme);
      assert.equal(await page.locator(".katex").first().evaluate((node) => getComputedStyle(node).color),
        await page.locator(".markdown-prose").evaluate((node) => getComputedStyle(node).color));
    }
    for (const partial of [String.raw`Working $\frac{1}{`, String.raw`Working $\frac{1}{$`, String.raw`Working $\frac{1}{2}$`]) {
      await reply(partial);
      await page.getByText("Working", { exact: false }).first().waitFor();
    }
    await reply(`$$\n${"a + ".repeat(150)}z\n$$`);
    await page.locator(".katex-display").waitFor();
    assert.equal(await page.locator(".katex-display").evaluate((node) => node.scrollWidth > node.clientWidth), true);
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), true);
    assert.deepEqual(remoteRequests, []);
    assert.deepEqual(errors, []);
  } finally { await page.close(); }
});

async function openAccountScreen(page: Page, name: "Settings" | "Balance") {
  await page.getByRole("button", { name: "Account menu", exact: true }).click();
  await page.getByRole("menuitem", { name: name === "Settings" ? "Settings" : /^Balance/ }).click();
  await page.getByRole("region", { name, exact: true }).waitFor();
}

for (const platform of ["linux", "win32", "darwin"]) {
  test(`settings categories animate, work with keyboard, and preserve the chat (${platform})`, async () => {
    const { page, errors } = await openApp(platform);
    try {
      await page.locator("textarea[data-chat-input]").fill("Keep my draft");
      await openAccountScreen(page, "Settings");
      const settings = page.getByRole("region", { name: "Settings", exact: true });
      const appearance = settings.getByRole("tab", { name: "Appearance", exact: true });
      const account = settings.getByRole("tab", { name: "Account", exact: true });
      const usage = settings.getByRole("tab", { name: "Usage", exact: true });
      const updates = settings.getByRole("tab", { name: "Updates", exact: true });
      assert.equal(await settings.evaluate((element) => getComputedStyle(element).animationName), "account-screen-enter");
      assert.equal(await settings.getByRole("tab").count(), 5);
      assert.equal(await settings.getByRole("tabpanel").count(), 1);
      assert.equal(await appearance.getAttribute("aria-selected"), "true");
      assert.equal(await settings.getByText("Inference balance").count(), 0);
      assert.equal(await settings.getByRole("button", { name: /Top up|Refresh balance/ }).count(), 0);
      await page.waitForFunction(() => document.activeElement?.getAttribute("data-category") === "appearance");
      assert.equal(await page.getByRole("textbox", { name: "Search threads" }).count(), 0);
      await appearance.press("ArrowDown");
      assert.equal(await account.getAttribute("aria-selected"), "true");
      await settings.getByRole("button", { name: "Refresh account" }).click();
      await settings.getByRole("button", { name: "Manage account", exact: true }).click();
      const beforeReturn = await page.evaluate(() => (window as any).__deletionTest.calls.filter((call: any) => call.method === "accountStatus").length);
      await page.evaluate(() => window.dispatchEvent(new Event("focus")));
      await page.waitForFunction((before) => (window as any).__deletionTest.calls.filter((call: any) => call.method === "accountStatus").length === before + 1, beforeReturn);
      assert.equal(await settings.getByText("TU", {exact: true}).count(), 1);
      await account.focus();
      await account.press("ArrowDown");
      assert.equal(await usage.getAttribute("aria-selected"), "true");
      await usage.press("ArrowDown");
      const apiKeys = settings.getByRole("tab", {name: "API keys", exact: true});
      assert.equal(await apiKeys.getAttribute("aria-selected"), "true");
      await apiKeys.press("ArrowDown");
      assert.equal(await updates.getAttribute("aria-selected"), "true");
      await updates.press("ArrowDown");
      assert.equal(await appearance.getAttribute("aria-selected"), "true");
      await appearance.press("End");
      assert.equal(await updates.getAttribute("aria-selected"), "true");
      await updates.press("Home");
      await settings.getByRole("radio", { name: "Dark", exact: true }).click();
      assert.equal(await page.evaluate(() => document.documentElement.dataset.theme), "dark");
      await settings.getByRole("radio", { name: "Dark", exact: true }).press("ArrowRight");
      assert.equal(await settings.getByRole("radio", { name: "System", exact: true }).getAttribute("aria-checked"), "true");
      await account.click();
      const nav = await settings.getByRole("tablist").boundingBox();
      const panel = await settings.getByRole("tabpanel").boundingBox();
      assert.ok(nav && panel && nav.x + nav.width <= panel.x);
      if (platform !== "darwin") {
        assert.equal(await page.getByRole("button", { name: "Minimize", exact: true }).evaluate((button) => {
          const rect = button.getBoundingClientRect();
          return button.contains(document.elementFromPoint(rect.x + rect.width / 2, rect.y + rect.height / 2));
        }), true, "window controls stay above the settings page");
      }
      await page.keyboard.press("Escape");
      const exiting = page.locator('.account-screen-motion[data-closing="true"]');
      assert.equal(await exiting.evaluate((element) => getComputedStyle(element).animationName), "account-screen-exit");
      assert.equal(await page.getByRole("textbox", { name: "Search threads" }).count(), 0, "chat stays inert until exit completes");
      await settings.waitFor({ state: "detached" });
      assert.equal(await page.locator("textarea[data-chat-input]").inputValue(), "Keep my draft");
      await page.waitForFunction(() => document.activeElement?.getAttribute("aria-label") === "Account menu");
      const calls = await page.evaluate(() => (window as any).__deletionTest.calls.map((call: any) => call.method));
      assert.ok(calls.includes("accountStatus") && calls.includes("accountPortal"));
      await openAccountScreen(page, "Settings");
      await settings.getByRole("button", { name: "Back", exact: true }).click();
      await settings.waitFor({ state: "detached" });
      assert.deepEqual(errors, []);
    } finally { await page.close(); }
  });

  test(`account menu opens upwards, shows live balance, and dismisses safely (${platform})`, async () => {
    const { page, errors } = await openApp(platform);
    try {
      await page.evaluate((billing) => (window as any).__deletionTest.setBilling(billing), depositBilling(12_345_678));
      const trigger = page.getByRole("button", { name: "Account menu", exact: true });
      assert.equal(await trigger.getByText("TU", {exact: true}).count(), 1, "Avatar comes from the account username");
      assert.equal(await page.getByRole("button", { name: "Open settings" }).count(), 0, "no separate settings gear");
      await trigger.click();
      const menu = page.getByRole("menu", { name: "Account actions", exact: true });
      const bounds = await menu.boundingBox(), anchor = await trigger.boundingBox();
      assert.ok(bounds && anchor && bounds.y + bounds.height <= anchor.y);
      assert.match(await menu.getByRole("menuitem", { name: /^Balance/ }).innerText(), /\$12\.35$/);
      await page.getByRole("menuitem", { name: "Settings", exact: true }).press("ArrowDown");
      assert.equal(await page.evaluate(() => document.activeElement?.textContent?.startsWith("Balance")), true);
      await page.keyboard.press("Escape");
      assert.equal(await menu.count(), 0);
      assert.equal(await trigger.getAttribute("aria-expanded"), "false");
      await trigger.press("ArrowUp");
      await page.keyboard.press("Tab");
      assert.equal(await menu.count(), 0);
      await trigger.click();
      await page.getByPlaceholder("Search threads").click();
      assert.equal(await menu.count(), 0);
      await trigger.click();
      await trigger.click();
      assert.equal(await menu.count(), 0, "clicking the account again toggles the menu closed");
      await trigger.click();
      await page.evaluate(() => (window as any).__deletionTest.setAccount("different-account"));
      await menu.waitFor({ state: "detached" });
      await page.evaluate(() => (window as any).__deletionTest.failBalance(true));
      await trigger.click();
      await menu.getByRole("menuitem", { name: "Balance Unavailable", exact: true }).waitFor();
      const logout = menu.getByRole("menuitem", { name: "Log out", exact: true });
      if (platform === "linux") {
        await logout.click();
      } else {
        await page.keyboard.press("End");
        assert.equal(await logout.evaluate((element) => element === document.activeElement), true);
        await page.keyboard.press("Enter");
      }
      await menu.waitFor({ state: "detached" });
      await trigger.getByText("Sign in", { exact: true }).waitFor();
      assert.equal(await trigger.evaluate((element) => element === document.activeElement), true);
      assert.equal(await page.evaluate(() => (window as any).__deletionTest.calls.filter((call: any) => call.method === "logout").length), 1);
      await trigger.click();
      await menu.getByRole("menuitem", { name: "Sign in", exact: true }).waitFor();
      assert.equal(await menu.getByRole("menuitem", { name: "Log out", exact: true }).count(), 0);
      assert.deepEqual(errors, []);
    } finally { await page.close(); }
  });
}

test("balance is a separate screen and payments stay hidden after sign-out", async () => {
  const { page, errors } = await openApp();
  try {
    const billing = depositBilling(25_000_000);
    await page.evaluate((billing) => (window as any).__deletionTest.setBilling(billing), billing);
    await openAccountScreen(page, "Balance");
    assert.equal(await page.getByRole("region", { name: "Settings", exact: true }).count(), 0);
    const address = page.getByLabel("Mainnet deposit address", { exact: true });
    assert.equal(await address.innerText(), billing.paymentAccount!.address);
    assert.equal(await page.getByRole("button", { name: "Copy address", exact: true }).isEnabled(), true);
    const refreshes = await page.evaluate(() => (window as any).__deletionTest.calls.filter((call: any) => call.method === "billingStatus").length);
    await page.getByRole("button", { name: "Refresh balance" }).click();
    await page.waitForFunction((previous) => (window as any).__deletionTest.calls.filter((call: any) => call.method === "billingStatus").length > previous, refreshes);
    assert.equal(await address.innerText(), billing.paymentAccount!.address);
    await page.getByRole("button", { name: "Back", exact: true }).click();
    await openAccountScreen(page, "Settings");
    await page.getByRole("tab", { name: "Account", exact: true }).click();
    await page.getByRole("button", { name: "Sign out", exact: true }).click();
    await page.getByRole("button", { name: "Sign in", exact: true }).waitFor();
    await page.getByRole("button", { name: "Back", exact: true }).click();
    await openAccountScreen(page, "Balance");
    await page.getByText("Sign in to view your balance and payment options.").waitFor();
    assert.equal(await page.getByRole("region", { name: "Zcash deposits", exact: true }).count(), 0);
    assert.equal(await address.count(), 0);
    await page.getByRole("button", { name: "Sign in", exact: true }).click();
    await page.getByRole("region", { name: "Browser sign-in", exact: true }).waitFor();
    assert.equal(await page.getByRole("dialog").count(), 0);
    assert.deepEqual(await page.evaluate(() => (window as any).__deletionTest.calls.filter((call: any) => call.method === "nativeLoginStart").map((call: any) => call.text)), ["[]"]);
    assert.equal(await page.getByRole("region", { name: "Balance", exact: true }).count(), 0);
    assert.deepEqual(errors, []);
  } finally { await page.close(); }
});

test("balance shows debt and a reusable deposit address without displaying a reservation", async () => {
  const { page, errors } = await openApp();
  try {
    await page.evaluate((billing) => (window as any).__deletionTest.setBilling(billing), depositBilling(-230_534));
    await openAccountScreen(page, "Balance");
    const panel = page.getByRole("region", { name: "Balance and payments", exact: true });
    await panel.getByText("-$0.23", { exact: true }).waitFor();
    await panel.getByText("Top up to send another message.", { exact: true }).waitFor();
    await page.getByLabel("Mainnet deposit address", { exact: true }).waitFor();
    assert.equal(await page.getByRole("button", { name: "Copy address", exact: true }).isEnabled(), true);
    assert.doesNotMatch(await panel.innerText(), /temporarily held|reserved/i);
    await page.evaluate((billing) => (window as any).__deletionTest.setBilling(billing), depositBilling(100_000, 2));
    await panel.getByText("$0.10", { exact: true }).first().waitFor();
    assert.equal(await panel.getByText("Top up to send another message.", { exact: true }).count(), 0);
    assert.deepEqual(errors, []);
  } finally { await page.close(); }
});

test("sign in opens browser authorization directly and leaves the composer usable", async () => {
  const { page, errors } = await openApp();
  try {
    await page.evaluate(() => (window as any).__deletionTest.setAccountState({ revision: 2, state: "signed_out" }));
    await page.getByRole("button", { name: "Sign in", exact: true }).first().click();
    const status = page.getByRole("region", { name: "Browser sign-in", exact: true });
    await status.getByText("ABCD-1234", { exact: true }).waitFor();
    assert.equal(await page.getByRole("dialog").count(), 0);
    assert.deepEqual(await page.evaluate(() => (window as any).__deletionTest.calls.filter((call: any) => call.method === "nativeLoginStart").map((call: any) => call.text)), ["[]"]);
    assert.equal(await status.getByRole("link", { name: "Reopen browser" }).getAttribute("href"), "https://auth.axiom.stream/native/authorize?test=1");
    await page.locator("textarea").fill("Keep this draft while I sign in");
    await page.setViewportSize({ width: 751, height: 791 });
    await page.emulateMedia({ reducedMotion: "reduce" });
    for (const theme of ["light", "dark"] as const) {
      await page.emulateMedia({ colorScheme: theme });
      await page.waitForFunction((expected) => document.documentElement.dataset.theme === expected, theme);
      await page.screenshot({ path: `/tmp/axiom-sign-in-${theme}.png` });
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth && document.documentElement.scrollHeight <= window.innerHeight), true);
    }
    await page.evaluate(() => (window as any).__deletionTest.finishLogin("test-login-1"));
    await page.getByRole("button", { name: "Test model", exact: true }).waitFor();
    await status.waitFor({ state: "detached" });
    assert.equal(await page.locator("textarea").inputValue(), "Keep this draft while I sign in");
    assert.deepEqual(errors, []);
  } finally { await page.close(); }
});

test("sign in ignores repeated starts and cancels authorizations that arrive after dismissal", async () => {
  const { page, errors } = await openApp();
  try {
    await page.evaluate(() => {
      (window as any).__deletionTest.setAccountState({ revision: 2, state: "signed_out" });
      (window as any).__deletionTest.holdSetup("loginStart");
    });
    const signIn = page.getByRole("button", { name: "Sign in", exact: true }).first();
    await signIn.evaluate((button) => { (button as HTMLButtonElement).click(); (button as HTMLButtonElement).click(); });
    await page.getByText("Opening your browser…").waitFor();
    assert.equal(await page.evaluate(() => (window as any).__deletionTest.calls.filter((call: any) => call.method === "nativeLoginStart").length), 1);
    await page.getByRole("button", { name: "Cancel sign-in", exact: true }).click();
    await page.evaluate(() => (window as any).__deletionTest.releaseSetup("loginStart"));
    await page.waitForFunction(() => (window as any).__deletionTest.calls.some((call: any) => call.method === "nativeLoginCancel" && call.id === "test-login-1"));
    assert.equal(await page.getByRole("region", { name: "Browser sign-in", exact: true }).count(), 0);
    assert.equal(await page.evaluate(() => (window as any).__deletionTest.calls.some((call: any) => call.method === "nativeLoginComplete")), false);
    await signIn.click();
    await page.getByText("ABCD-1234", { exact: true }).waitFor();
    await page.getByRole("button", { name: "Cancel sign-in", exact: true }).click();
    assert.deepEqual(await page.evaluate(() => (window as any).__deletionTest.calls.filter((call: any) => call.method === "nativeLoginCancel").map((call: any) => call.id)), ["test-login-1", "test-login-2"]);
    assert.equal(await page.getByRole("region", { name: "Browser sign-in", exact: true }).count(), 0);
    assert.deepEqual(errors, []);
  } finally { await page.close(); }
});

test("sign in retries a failed browser authorization without a method selector", async () => {
  const { page, errors } = await openApp();
  try {
    await page.evaluate(() => (window as any).__deletionTest.setAccountState({ revision: 2, state: "signed_out" }));
    await page.getByRole("button", { name: "Sign in", exact: true }).first().click();
    const status = page.getByRole("region", { name: "Browser sign-in", exact: true });
    await status.getByText("ABCD-1234", { exact: true }).waitFor();
    await page.evaluate(() => (window as any).__deletionTest.finishLogin("test-login-1", "Authorization expired"));
    await status.getByText("Authorization expired", { exact: true }).waitFor();
    await status.getByRole("button", { name: "Try again", exact: true }).click();
    await status.getByText("ABCD-1234", { exact: true }).waitFor();
    assert.deepEqual(await page.evaluate(() => (window as any).__deletionTest.calls.filter((call: any) => call.method === "nativeLoginStart").map((call: any) => call.text)), ["[]", "[]"]);
    assert.equal(await page.getByRole("dialog").count(), 0);
    assert.deepEqual(errors, []);
  } finally { await page.close(); }
});

test("balance deposit controls fit the reference window, respect reduced motion, and match both themes", async () => {
  const { page, errors } = await openApp();
  try {
    await page.clock.install();
    await page.emulateMedia({ reducedMotion: "reduce" });
    await page.evaluate(() => {
      const address = `u1${"a".repeat(180)}`;
      (window as any).__deletionTest.setBilling({
        revision: 1, postedMicrousd: 2_893_999, availableMicrousd: 2_893_999,
        trialMicrousd: 999_676, paidMicrousd: 1_894_323,
        ledgerSequence: 0, currency: "microUSD",
        zecUsdQuote: { source: "coinbase", price_microusd_per_zec: "123456789",
          as_of: new Date().toISOString(), expires_at: new Date(Date.now() + 60_000).toISOString() },
        paymentAccount: { network: "mainnet", asset: "ZEC", conversion_status: "none", state: "ready", valuation_enabled: true,
          address, payment_uri: `zcash:${address}`, monitoring_status: "ready", required_confirmations: "10",
          confirmed_zatoshis: "100000001", confirming_zatoshis: "1", review_required: false, deposits: [],
        },
      });
    });
    const screenshot = process.env.AXIOM_SETTINGS_SCREENSHOT;
    for (const theme of ["light", "dark"] as const) {
      await page.emulateMedia({ colorScheme: theme });
      await page.waitForFunction((expected) => document.documentElement.dataset.theme === expected, theme);
      await page.getByRole("button", { name: "Account menu", exact: true }).click();
      if (screenshot) await page.screenshot({ path: `${screenshot}-${theme}-menu.png`, animations: "disabled" });
      assert.equal(await page.getByRole("menu", { name: "Account actions" }).evaluate((element) => getComputedStyle(element).animationName), "none");
      await page.getByRole("menuitem", { name: "Settings", exact: true }).click();
      for (const category of ["Appearance", "Account"]) {
        await page.getByRole("tab", { name: category, exact: true }).click();
        if (screenshot) await page.screenshot({ path: `${screenshot}-${theme}-${category}.png`, animations: "disabled" });
      }
      await page.getByRole("button", { name: "Back", exact: true }).click();
      await page.setViewportSize({ width: 751, height: 791 });
      await openAccountScreen(page, "Balance");
      assert.equal(await page.getByRole("region", { name: "Balance", exact: true }).evaluate((element) => getComputedStyle(element).animationName), "none");
      const panel = page.getByRole("region", { name: "Balance and payments", exact: true });
      if (screenshot) await page.screenshot({ path: `${screenshot}-${theme}-balance.png`, animations: "disabled" });
      assert.equal(await panel.evaluate((element) => element.scrollHeight <= element.clientHeight), true, "the empty deposit screen fits without scrolling");
      const action = await page.getByRole("button", { name: "Copy address", exact: true }).boundingBox();
      const panelBounds = await panel.boundingBox();
      assert.ok(action && panelBounds && action.y + action.height <= panelBounds.y + panelBounds.height);
      assert.equal(await page.getByLabel("Current ZEC exchange rate").innerText(), "1 ZEC ≈ $123.46\n\nRate at confirmation applies");
      await page.getByRole("button", { name: "Back", exact: true }).click();
    }
    await openAccountScreen(page, "Balance");
    for (const width of [800, 560]) {
      await page.setViewportSize({ width, height: 600 });
      const panel = page.getByRole("region", { name: "Balance and payments", exact: true });
      const back = page.getByRole("button", { name: "Back", exact: true });
      const backBefore = await back.boundingBox();
      assert.equal(await panel.evaluate((element) => element.scrollWidth <= element.clientWidth), true);
      await page.getByRole("button", { name: "Copy address", exact: true }).scrollIntoViewIfNeeded();
      assert.deepEqual(await back.boundingBox(), backBefore);
      assert.equal(await page.getByLabel("Mainnet deposit address", { exact: true }).innerText(), `u1${"a".repeat(180)}`);
      if (screenshot) await page.screenshot({ path: `${screenshot}-small-${width}.png`, animations: "disabled" });
    }
    await page.evaluate(() => {
      Object.defineProperty(navigator.clipboard, "writeText", { configurable: true, value: async (text: string) => { (window as any).__copiedAddress = text; } });
    });
    await page.getByRole("button", { name: "Copy address", exact: true }).click();
    await page.getByRole("button", { name: "Copied", exact: true }).waitFor();
    assert.equal(await page.evaluate(() => (window as any).__copiedAddress), `u1${"a".repeat(180)}`);
    await page.clock.runFor(61_000);
    assert.match(await page.getByLabel("Current ZEC exchange rate").innerText(), /Rate unavailable/);
    await page.getByRole("button", { name: "Back", exact: true }).click();
    await openAccountScreen(page, "Settings");
    const count = await page.evaluate(() => (window as any).__deletionTest.calls.filter((call: any) => call.method === "billingStatus").length);
    await page.clock.runFor(6_000);
    assert.equal(await page.evaluate(() => (window as any).__deletionTest.calls.filter((call: any) => call.method === "billingStatus").length), count);
    assert.deepEqual(errors, []);
  } finally { await page.close(); }
});

test("deposit progress keeps ZEC primary and changes estimated USD to credited USD", async () => {
  const { page, errors } = await openApp();
  try {
    await page.clock.install();
    const showDeposit = async (confirmations: string, credited = false) => page.evaluate(({ confirmations, credited }) => {
      const address = `u1${"a".repeat(180)}`;
      const quoteTime = Date.now();
      (window as any).__deletionTest.setBilling({
        revision: Number(confirmations), postedMicrousd: credited ? 100001 : 0,
        availableMicrousd: credited ? 100001 : 0, trialMicrousd: 0, paidMicrousd: credited ? 100001 : 0,
        ledgerSequence: 0, currency: "microUSD",
        zecUsdQuote: { source: "coinbase", price_microusd_per_zec: "123456789",
          as_of: new Date(quoteTime).toISOString(), expires_at: new Date(quoteTime + 60_000).toISOString() },
        paymentAccount: { network: "mainnet", asset: "ZEC", conversion_status: "none", state: "ready", valuation_enabled: true,
          address, payment_uri: `zcash:${address}`, monitoring_status: "ready", required_confirmations: "10",
          confirmed_zatoshis: credited ? "80000" : "0", confirming_zatoshis: credited ? "0" : "80000", review_required: false,
          deposits: [{ id: "deposit-progress", amount_zatoshis: "80000", state: credited ? "confirmed" : "confirming",
            object_version: credited ? "2" : "1", confirmations, required_confirmations: "10", review_required: false,
            observed_at: new Date().toISOString(), valuation_status: credited ? "credited" : "unpriced",
            ...(credited ? { credit_microusd: "100001", price_microusd_per_zec: "125001250", price_source: "coinbase", priced_at: new Date().toISOString() } : {}),
          }],
        },
      });
    }, { confirmations, credited });
    await showDeposit("3");
    await openAccountScreen(page, "Balance");
    const row = page.getByRole("region", { name: "Zcash deposits" }).locator("li summary");
    await row.getByText("~$0.10", { exact: true }).waitFor();
    const zecBounds = (await row.getByText("0.0008 ZEC", { exact: true }).boundingBox())!;
    const usdBounds = (await row.getByText("~$0.10", { exact: true }).boundingBox())!;
    assert.ok(usdBounds.x > zecBounds.x + zecBounds.width);
    assert.ok(Math.abs(usdBounds.y - zecBounds.y) < 4);
    assert.match(await row.innerText(), /3\/10 confirmations/);
    assert.notEqual(await row.locator("svg.lucide-loader-circle").evaluate((el) => getComputedStyle(el).animationName), "none");
    await page.emulateMedia({ reducedMotion: "reduce" });
    assert.equal(await row.locator("svg.lucide-loader-circle").evaluate((el) => getComputedStyle(el).animationName), "none");
    await showDeposit("7");
    await row.getByText("· 7/10 confirmations", { exact: true }).waitFor();
    await page.setViewportSize({ width: 560, height: 650 });
    await row.scrollIntoViewIfNeeded();
    const panel = page.getByRole("region", { name: "Balance and payments", exact: true });
    assert.equal(await panel.evaluate(el => el.scrollWidth <= el.clientWidth), true);
    const screenshot = process.env.AXIOM_DEPOSIT_SCREENSHOT;
    if (screenshot) await page.screenshot({ path: `${screenshot}-confirming.png`, animations: "disabled" });
    await page.clock.runFor(60_001);
    await row.getByText("0.0008 ZEC", { exact: true }).waitFor();
    assert.equal(await row.getByText("~$0.10", { exact: true }).count(), 0);
    await showDeposit("12", true);
    await row.getByText("$0.10", { exact: true }).waitFor();
    assert.match(await row.innerText(), /Confirmed.*10\/10 confirmations/s);
    assert.doesNotMatch(await row.innerText(), /~/);
    assert.equal(await row.locator("svg.lucide-loader-circle").count(), 0);
    assert.equal(await row.locator("svg.lucide-check").count(), 1);
    if (screenshot) await page.screenshot({ path: `${screenshot}-confirmed.png`, animations: "disabled" });
    assert.deepEqual(errors, []);
  } finally { await page.close(); }
});

test("sidebar order stays stable during live output and advances on user submissions", async () => {
  const { page, errors } = await openApp();
  try {
    await page.evaluate(() => (window as any).__deletionTest.setOrdering([
      { id: "older-1", lastMessageAt: "2026-01-01T00:00:00Z", folder: "Older" },
      { id: "older-2", lastMessageAt: "2026-01-02T00:00:00Z", folder: "Older" },
      { id: "newer-1", lastMessageAt: "2026-01-03T00:00:00Z", folder: "Newer" },
      { id: "loose-old", lastMessageAt: "2026-01-01T00:00:00Z", folder: null },
      { id: "loose-new", lastMessageAt: "2026-01-02T00:00:00Z", folder: null },
    ]));
    const folders = page.locator('button[data-folder-id]');
    const rows = page.locator('button[data-thread-id]');
    await page.locator('[data-folder-id="Newer"]').waitFor();
    const order = async () => ({
      folders: await folders.evaluateAll((buttons) => buttons.map((button) => button.getAttribute("data-folder-id"))),
      threads: await rows.evaluateAll((buttons) => buttons.map((button) => button.getAttribute("data-thread-id"))),
    });
    const initial = await order();
    assert.deepEqual(initial.folders, ["Newer", "Older", "Empty"]);
    assert.deepEqual(initial.threads, ["newer-1", "older-2", "older-1", "loose-new", "loose-old"]);
    await page.evaluate(() => (window as any).__deletionTest.refreshMetadata());
    assert.deepEqual(await order(), initial);
    await page.evaluate(() => (window as any).__deletionTest.bumpLiveMessage("older-1", "2026-01-04T00:00:00Z"));
    assert.deepEqual(await order(), initial, "streamed output must not reorder rows or folders");
    await page.evaluate(() => (window as any).__deletionTest.bumpUserMessage("older-1", "2026-01-04T00:00:00Z"));
    await page.waitForFunction(() => document.querySelector('button[data-folder-id]')?.getAttribute("data-folder-id") === "Older");
    const updated = await order();
    assert.deepEqual(updated.folders, ["Older", "Newer", "Empty"]);
    assert.deepEqual(updated.threads, ["older-1", "older-2", "newer-1", "loose-new", "loose-old"]);
    await page.getByPlaceholder("Search threads").fill("older-2");
    assert.deepEqual((await order()).folders, updated.folders, "search must not change folder ranking");
    assert.deepEqual(errors, []);
  } finally { await page.close(); }
});

test("new-chat drafts clear on account switch and sign-out", async () => {
  const { page, errors } = await openApp();
  try {
    const input = page.locator("textarea[data-chat-input]");
    await input.fill("Private account A draft");
    await page.evaluate(() => (window as any).__deletionTest.setAccount("account-b"));
    await page.waitForFunction(() => (document.querySelector("textarea[data-chat-input]") as HTMLTextAreaElement)?.value === "");
    await input.fill("Private account B draft");
    await page.evaluate(() => (window as any).axiomDesktop.agent.logout());
    await page.getByRole("button", { name: "Sign in", exact: true }).waitFor();
    assert.equal(await input.inputValue(), "");
    assert.deepEqual(errors, []);
  } finally { await page.close(); }
});

test("Edit and Regenerate confirm native revisions and disable while a reply is running", async () => {
  const { page, errors } = await openApp();
  try {
    await page.getByRole("button", { name: "Viewed Existing thread", exact: true }).click();
    await page.evaluate(() => (window as any).__deletionTest.setTimeline([
      { id: "user", kind: "user", text: "Original message", status: "completed" },
      { id: "assistant", kind: "assistant", text: "Original response", status: "completed" },
    ]));
    await page.getByRole("button", { name: "Edit message", exact: true }).click();
    await page.getByRole("textbox", { name: "Edited message" }).fill("Changed message");
    assert.match(await page.locator("body").innerText(), /Files already changed stay as they are/);
    assert.equal(await page.evaluate(() => (window as any).__deletionTest.calls.filter((call: any) => call.method === "revise").length), 0);
    await page.getByRole("button", { name: "Save and send", exact: true }).click();
    await page.getByRole("textbox", { name: "Edited message" }).waitFor({ state: "hidden" });
    await page.getByRole("button", { name: "Regenerate", exact: true }).click();
    await page.getByRole("button", { name: "Regenerate from here", exact: true }).click();
    await page.getByRole("button", { name: "Regenerate from here", exact: true }).waitFor({ state: "hidden" });
    const calls = await page.evaluate(() => (window as any).__deletionTest.calls.filter((call: any) => call.method === "revise"));
    assert.deepEqual(calls.map(({ id, text, userItemId, expectedRevision }: any) => ({ id, text, userItemId, expectedRevision })), [
      { id: "old-thread", text: "Changed message", userItemId: "user", expectedRevision: 1 },
      { id: "old-thread", text: "Original message", userItemId: "user", expectedRevision: 1 },
    ]);
    await page.evaluate(() => (window as any).__deletionTest.setTimeline([
      { id: "user", kind: "user", text: "Original message", status: "completed" },
      { id: "assistant", kind: "assistant", text: "Original response", status: "completed" },
    ], true));
    await page.waitForFunction(() => (document.querySelector('button[aria-label="Edit message"]') as HTMLButtonElement)?.disabled);
    assert.equal(await page.getByRole("button", { name: "Edit message", exact: true }).isDisabled(), true);
    assert.equal(await page.getByRole("button", { name: "Regenerate", exact: true }).evaluateAll((buttons) => buttons.every((button) => (button as HTMLButtonElement).disabled)), true);
    assert.deepEqual(errors, []);
  } finally { await page.close(); }
});

test("proxy view follows native state and sends start, restart, stop and clipboard intents", async () => {
  const { page, errors } = await openApp();
  try {
    await page.evaluate(() => {
      const listeners = new Set<(state: unknown) => void>();
      let state = { revision: 1, status: "stopped", accountId: "test-account", port: 8484, baseUrl: null as string | null,
        completedRequests: 0, totalTokens: 0, errors: [], evidence: null, error: null };
      const calls: unknown[] = [];
      const emit = () => { for (const listener of listeners) listener(structuredClone(state)); };
      (window as any).__proxyCalls = calls;
      (window as any).axiomDesktop.proxy = {
        getState: async () => structuredClone(state),
        onState: (callback: (state: unknown) => void) => { listeners.add(callback); return () => listeners.delete(callback); },
        start: async (port: number, accountId: string, runtimeId: string) => {
          calls.push(["start", port, accountId, runtimeId]);
          state = { ...state, revision: state.revision + 1, status: "running", port, baseUrl: `http://127.0.0.1:${port}/v1` };
          emit(); return structuredClone(state);
        },
        stop: async () => { calls.push(["stop"]); state = { ...state, revision: state.revision + 1, status: "stopped", baseUrl: null }; emit(); return structuredClone(state); },
        copyToken: async (account: string, runtime: string) => { calls.push(["copyToken", account, runtime]); },
      };
    });
    await page.getByRole("button", { name: "Proxy", exact: true }).click();
    await page.getByText("Stopped", { exact: true }).waitFor();
    assert.equal(await page.getByRole("button", { name: "Copy token", exact: true }).isDisabled(), true);
    assert.doesNotMatch(await page.locator("body").innerText(), /12\.4k|axiom-local/);
    await page.getByRole("button", { name: "Start proxy", exact: true }).click();
    await page.getByText("Running", { exact: true }).waitFor();
    await page.getByRole("button", { name: "Copy token", exact: true }).click();
    await page.getByRole("button", { name: "Copied", exact: true }).waitFor();
    await page.getByRole("textbox", { name: "Port", exact: true }).fill("8585");
    await page.getByRole("button", { name: "Restart proxy", exact: true }).click();
    await page.getByText("http://127.0.0.1:8585/v1", { exact: true }).waitFor();
    await page.getByRole("button", { name: "Stop proxy", exact: true }).click();
    await page.getByText("Stopped", { exact: true }).waitFor();
    assert.deepEqual(await page.evaluate(() => (window as any).__proxyCalls), [
      ["start", 8484, "test-account", "test-runtime"], ["copyToken", "test-account", "test-runtime"],
      ["start", 8585, "test-account", "test-runtime"], ["stop"],
    ]);
    assert.deepEqual(errors, []);
  } finally { await page.close(); }
});

test("an anonymous draft follows sign-in and each account recovers only its own draft", async () => {
  const { page, errors } = await openApp();
  try {
    const input = page.locator("textarea[data-chat-input]");
    await input.fill("Old account secret");
    await page.evaluate(() => (window as any).axiomDesktop.agent.logout());
    await page.getByRole("button", { name: "Sign in", exact: true }).waitFor();
    assert.equal(await input.inputValue(), "");
    await input.fill("New draft written while signed out");
    await page.evaluate(() => (window as any).__deletionTest.setAccountState({ revision: 3, state: "valid", account: { id: "account-b", displayName: "B", linkedMethods: ["password"] } }));
    await page.getByRole("button", { name: "Test model", exact: true }).waitFor();
    assert.equal(await input.inputValue(), "New draft written while signed out");
    await page.evaluate(() => (window as any).__deletionTest.setAccount("test-account"));
    await page.waitForFunction(() => (document.querySelector("textarea[data-chat-input]") as HTMLTextAreaElement)?.value === "Old account secret");
    assert.deepEqual(errors, []);
  } finally { await page.close(); }
});

test("usage shows model shares in both themes, exact small charges, and an honest empty state", async () => {
  const { page, errors } = await openApp();
  try {
    await page.evaluate(() => {
      (window as any).axiomDesktop.agent.usageSummary = async () => ({ summary: {
        period: "all_time", totalCostMicrousd: "48650000", models: [
          { provider: "phala", modelId: "qwen3", modelName: "Qwen3", costMicrousd: "24500000" },
          { provider: "near", modelId: "gpt-oss-120b", modelName: "GPT-OSS 120B", costMicrousd: "15800000" },
          { provider: "phala", modelId: "deepseek", modelName: "DeepSeek V3.2", costMicrousd: "8350000" },
        ],
      } });
    });
    await openAccountScreen(page, "Settings");
    await page.getByRole("tab", { name: "Usage", exact: true }).click();
    await page.locator("[data-usage-total]").waitFor();
    assert.equal(await page.locator("[data-usage-total]").innerText(), "$48.65");
    const chart = page.getByRole("img", { name: /Spending by model/ });
    assert.equal(await chart.locator("path").count(), 3);
    await page.getByRole("button", { name: /Qwen3, phala/ }).focus();
    assert.equal(await chart.locator('path[opacity="0.35"]').count(), 2);
    await page.getByRole("tab", { name: "Usage", exact: true }).focus();
    for (const theme of ["light", "dark"] as const) {
      await page.emulateMedia({ colorScheme: theme, reducedMotion: "reduce" });
      await page.waitForFunction(value => document.documentElement.dataset.theme === value, theme);
      if (process.env.AXIOM_USAGE_SCREENSHOT) await page.screenshot({ path: `${process.env.AXIOM_USAGE_SCREENSHOT}-${theme}.png`, animations: "disabled" });
    }
    await page.setViewportSize({ width: 800, height: 700 });
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
    if (process.env.AXIOM_USAGE_SCREENSHOT) await page.screenshot({ path: `${process.env.AXIOM_USAGE_SCREENSHOT}-narrow.png`, animations: "disabled" });
    await page.evaluate(() => {
      (window as any).axiomDesktop.agent.usageSummary = async () => ({ summary: {
        period: "all_time", totalCostMicrousd: "1", models: [
          { provider: "test", modelId: "one", modelName: "One model", costMicrousd: "1" },
        ],
      } });
    });
    await page.getByRole("button", { name: "Refresh usage" }).click();
    await page.getByText("$0.000001", { exact: true }).first().waitFor();
    assert.equal(await chart.locator("circle").count(), 1);
    await page.evaluate(() => {
      (window as any).axiomDesktop.agent.usageSummary = async () => ({ summary: { period: "all_time", totalCostMicrousd: "0", models: [] } });
    });
    await page.getByRole("button", { name: "Refresh usage" }).click();
    await page.getByRole("heading", { name: "No spending yet" }).waitFor();
    assert.equal(await page.locator("[data-usage-total]").innerText(), "$0.00");
    assert.equal(await chart.count(), 0);
    assert.deepEqual(errors, []);
  } finally { await page.close(); }
});

test("usage errors never become zero spend and old account requests cannot populate a new account", async () => {
  const { page, errors } = await openApp();
  try {
    await page.evaluate(() => {
      (window as any).axiomDesktop.agent.usageSummary = async () => { throw new Error("Unavailable"); };
    });
    await openAccountScreen(page, "Settings");
    await page.getByRole("tab", { name: "Usage", exact: true }).click();
    await page.getByRole("alert").filter({ hasText: "Couldn’t load your usage." }).waitFor();
    assert.equal(await page.locator("[data-usage-total]").count(), 0);
    await page.evaluate(() => {
      (window as any).axiomDesktop.agent.usageSummary = () => new Promise(resolve => { (window as any).__oldUsage = resolve; });
    });
    await page.getByRole("button", { name: "Try again" }).click();
    await page.waitForFunction(() => !!(window as any).__oldUsage);
    await page.evaluate(() => {
      (window as any).axiomDesktop.agent.usageSummary = async () => ({ summary: { period: "all_time", totalCostMicrousd: "0", models: [] } });
      (window as any).__deletionTest.setAccount("new-usage-account");
    });
    await page.getByRole("heading", { name: "No spending yet" }).waitFor();
    await page.evaluate(() => (window as any).__oldUsage({ summary: { period: "all_time", totalCostMicrousd: "999000000", models: [{ provider: "old", modelId: "old", modelName: "Old account model", costMicrousd: "999000000" }] } }));
    assert.equal(await page.locator("[data-usage-total]").innerText(), "$0.00");
    assert.equal(await page.getByText("Old account model").count(), 0);
    await page.evaluate(() => (window as any).axiomDesktop.agent.logout());
    await page.getByText("Sign in to see your spending.").waitFor();
    assert.equal(await page.locator("[data-usage-total]").count(), 0);
    assert.deepEqual(errors, []);
  } finally { await page.close(); }
});


test("API keys show usage, reveal a new key once, and preserve revoked history", async () => {
  const {page, errors} = await openApp();
  try {
    await page.evaluate(() => {
      let keys: ApiKeyRecord[] = [];
      const api = window.axiomDesktop!.agent;
      api.apiKeys = async () => ({keys});
      api.createApiKey = async (name, accountId) => {
        if (accountId !== "test-account") throw new Error("Wrong account");
        const key: ApiKeyRecord = {id: "key-1", name, scopes: ["inference"], createdAt: "2026-09-10T12:00:00Z", lastUsedAt: null, expiresAt: null, revokedAt: null, usageStartedAt: "2026-09-10T12:00:00Z",
          usage: {requestCount: "123", inputTokens: "12345", cachedInputTokens: "1000", outputTokens: "4000", costMicrousd: "123456"}};
        keys = [key];
        return {key, token: "axm_desktop_fixture_1234567890"};
      };
      api.revokeApiKey = async id => {keys = keys.map(key => key.id === id ? {...key, revokedAt: "2026-09-10T13:00:00Z"} : key); return {};};
    });
    await openAccountScreen(page, "Settings");
    await page.getByRole("tab", {name: "API keys", exact: true}).click();
    const panel = page.getByRole("tabpanel", {name: "API keys", exact: true});
    await panel.getByText("No API keys yet").waitFor();
    await panel.getByRole("button", {name: "Create key", exact: true}).click();
    await panel.getByLabel("Key name", {exact: true}).fill("Coding tools");
    await panel.getByRole("form", {name: "Create API key"}).getByRole("button", {name: "Create key", exact: true}).click();
    const secret = panel.getByRole("textbox", {name: "New API key", exact: true});
    await secret.waitFor();
    assert.equal(await secret.inputValue(), "axm_desktop_fixture_1234567890");
    await page.context().grantPermissions(["clipboard-read", "clipboard-write"], {origin});
    await panel.getByRole("button", {name: "Copy key", exact: true}).click();
    await panel.getByRole("button", {name: "Copied", exact: true}).waitFor();
    assert.equal(await page.evaluate(() => navigator.clipboard.readText()), "axm_desktop_fixture_1234567890");
    await panel.getByRole("button", {name: "Done", exact: true}).click();
    assert.equal(await secret.count(), 0);
    const row = panel.getByRole("article", {name: "API key Coding tools"});
    await row.getByText("$0.12", {exact: true}).waitFor();
    await row.getByText("Usage details", {exact: true}).click();
    await row.getByText("12,345", {exact: true}).waitFor();
    for (const theme of ["light", "dark"]) {
      await page.evaluate(theme => {document.documentElement.dataset.theme = theme;}, theme);
      await panel.screenshot({path: `/tmp/axiom-api-keys-${theme}.png`, animations: "disabled"});
    }
    await row.getByRole("button", {name: "Revoke", exact: true}).click();
    await page.getByRole("dialog").getByRole("button", {name: "Cancel", exact: true}).click();
    await row.getByRole("button", {name: "Revoke", exact: true}).click();
    await page.getByRole("dialog").getByRole("button", {name: "Revoke key", exact: true}).click();
    await row.waitFor({state: "hidden"});
    await panel.getByRole("button", {name: "Show revoked and expired keys"}).click();
    await row.getByText("Revoked", {exact: true}).waitFor();
    await row.getByText("$0.12", {exact: true}).waitFor();
    await page.setViewportSize({width: 750, height: 790});
    assert.equal(await panel.evaluate(element => element.scrollWidth > element.clientWidth), false);
    await panel.screenshot({path: "/tmp/axiom-api-keys-small.png", animations: "disabled"});
    await page.evaluate(() => (window as any).__deletionTest.setAccount("other-account"));
    assert.equal(await secret.count(), 0);
    assert.deepEqual(errors, []);
  } finally {await page.close();}
});

test("API keys discard a creation response after account switching", async () => {
  const {page, errors} = await openApp();
  try {
    await page.evaluate(() => {
      window.axiomDesktop!.agent.createApiKey = () => new Promise(resolve => {(window as any).__finishKey = resolve;});
    });
    await openAccountScreen(page, "Settings");
    await page.getByRole("tab", {name: "API keys", exact: true}).click();
    const panel = page.getByRole("tabpanel", {name: "API keys", exact: true});
    await panel.getByRole("button", {name: "Create key", exact: true}).click();
    await panel.getByLabel("Key name", {exact: true}).fill("Old account");
    await panel.getByRole("form", {name: "Create API key"}).getByRole("button", {name: "Create key", exact: true}).click();
    await page.waitForFunction(() => typeof (window as any).__finishKey === "function");
    await page.evaluate(() => (window as any).__deletionTest.setAccount("other-account"));
    await page.evaluate(() => (window as any).__finishKey({key: {id: "old", name: "Old account"}, token: "axm_old_secret_do_not_show"}));
    assert.equal(await page.getByRole("textbox", {name: "New API key", exact: true}).count(), 0);
    assert.equal(await page.getByText("Old account", {exact: true}).count(), 0);
    assert.deepEqual(errors, []);
  } finally {await page.close();}
});


test("drafts survive reselecting, switching threads, reloading, and account changes", async () => {
  const { page, errors } = await openApp();
  try {
    const input = page.locator("textarea[data-chat-input]");
    const existing = page.locator('[data-thread-id="old-thread"]');
    await input.fill("New conversation draft");
    await existing.click();
    assert.equal(await input.inputValue(), "");
    await input.fill("Existing conversation draft");
    await existing.click();
    assert.equal(await input.inputValue(), "Existing conversation draft");
    await page.getByRole("button", { name: "New thread", exact: true }).click();
    assert.equal(await input.inputValue(), "New conversation draft");
    await page.reload();
    await input.waitFor();
    await page.waitForFunction(() => (document.querySelector("textarea[data-chat-input]") as HTMLTextAreaElement)?.value === "New conversation draft");
    await existing.click();
    assert.equal(await input.inputValue(), "Existing conversation draft");
    await page.evaluate(() => (window as any).__deletionTest.setAccount("account-b"));
    await page.waitForFunction(() => (document.querySelector("textarea[data-chat-input]") as HTMLTextAreaElement)?.value === "");
    await input.fill("Account B draft");
    await page.evaluate(() => (window as any).__deletionTest.setAccount("test-account"));
    await page.waitForFunction(() => (document.querySelector("textarea[data-chat-input]") as HTMLTextAreaElement)?.value === "Existing conversation draft");
    assert.deepEqual(errors, []);
  } finally { await page.close(); }
});

test("interrupted first-message setup survives reload without automatic dispatch", async () => {
  const { page, errors } = await openApp();
  try {
    await page.evaluate(() => (window as any).__deletionTest.holdSetup("new"));
    await page.locator("textarea[data-chat-input]").fill("Durable first message");
    await page.locator("textarea[data-chat-input]").press("Enter");
    await page.getByRole("status", { name: "Sending message" }).waitFor();
    await page.reload();
    const queue = page.getByRole("region", { name: "Message queue" });
    await queue.waitFor();
    assert.match(await queue.innerText(), /Durable first message/);
    assert.equal((await calls(page)).some((call) => call.method === "prompt"), false);
    await queue.getByRole("button", { name: "Review draft" }).click();
    await page.waitForFunction(() => (document.querySelector("textarea[data-chat-input]") as HTMLTextAreaElement)?.value === "Durable first message");
    await page.locator("textarea[data-chat-input]").press("Enter");
    await page.waitForFunction(() => (window as any).__deletionTest.calls.some((call: any) => call.method === "prompt"));
    assert.equal((await calls(page)).filter((call) => call.method === "prompt").length, 1);
    assert.deepEqual(errors, []);
  } finally { await page.close(); }
});

for (const width of [1320, 640]) {
  test(`code-only responses and large ordered-list markers fit at ${width}px`, async () => {
    const { page, errors } = await openApp();
    try {
      await page.setViewportSize({ width, height: 800 });
      await page.locator('[data-thread-id="old-thread"]').click();
      const text = "```typescript\nconst answer = 42;\n" + "// wide code ".repeat(80) + "\n```";
      await page.evaluate((text) => (window as any).__deletionTest.setTimeline([{ id: "code", kind: "assistant", text, status: "completed" }]), text);
      const code = page.locator(".markdown-prose pre");
      await code.waitFor();
      const dimensions = await code.evaluate((el) => ({ width: el.getBoundingClientRect().width, scroll: el.scrollWidth, viewport: el.clientWidth }));
      assert.ok(dimensions.width > 180, JSON.stringify(dimensions));
      assert.ok(dimensions.scroll > dimensions.viewport, "wide code scrolls inside its code block");
      const list = "999. First item\n1000. Second item\n1001. Third item";
      await page.evaluate((text) => (window as any).__deletionTest.setTimeline([{ id: "list", kind: "assistant", text, status: "completed" }]), list);
      const geometry = await page.locator(".markdown-prose ol").evaluate((el) => {
        const style = getComputedStyle(el);
        const canvas = document.createElement("canvas");
        const ctx = canvas.getContext("2d")!; ctx.font = style.font;
        return { padding: parseFloat(style.paddingInlineStart), marker: ctx.measureText("1001. ").width,
          documentWidth: document.documentElement.scrollWidth, viewport: innerWidth };
      });
      assert.ok(geometry.padding >= geometry.marker, JSON.stringify(geometry));
      assert.ok(geometry.documentWidth <= geometry.viewport);
      assert.deepEqual(errors, []);
    } finally { await page.close(); }
  });
}


test("two pending first messages continue independently while a third draft stays selected", async () => {
  const { page, errors } = await openApp();
  try {
    await page.evaluate(() => (window as any).__deletionTest.holdSetup("new"));
    for (const text of ["First parallel message", "Second parallel message"]) {
      await page.locator("textarea[data-chat-input]").fill(text);
      await page.locator("textarea[data-chat-input]").press("Enter");
      await page.getByRole("status", { name: "Sending message" }).waitFor();
      await page.getByRole("button", { name: "New thread", exact: true }).click();
    }
    await page.locator('[data-thread-id="old-thread"]').click();
    await page.locator("textarea[data-chat-input]").fill("Third independent draft");
    await page.evaluate(() => (window as any).__deletionTest.releaseSetup("new"));
    await page.waitForFunction(() => (window as any).__deletionTest.calls.filter((call: any) => call.method === "prompt").length === 2);
    const prompts = (await calls(page)).filter((call) => call.method === "prompt");
    assert.deepEqual(prompts.map((call) => call.text).sort(), ["First parallel message", "Second parallel message"]);
    assert.equal(new Set(prompts.map((call) => call.id)).size, 2);
    assert.equal(await page.locator("textarea[data-chat-input]").inputValue(), "Third independent draft");
    assert.deepEqual(errors, []);
  } finally { await page.close(); }
});


test("large file text is sent intact through the attachment-aware bridge and survives transcript display", async () => {
  const { page, errors } = await openApp();
  try {
    const content = "local private content\n".repeat(10_000);
    await page.locator('input[type="file"]').setInputFiles({ name: "notes.txt", mimeType: "text/plain", buffer: Buffer.from(content) });
    await page.getByRole("button", { name: "Remove notes.txt" }).waitFor();
    await page.getByRole("button", { name: "Send", exact: true }).click();
    await page.waitForFunction(() => (window as any).__deletionTest.calls.some((call: any) => call.method === "attachments"));
    const call = (await calls(page)).find((call) => call.method === "attachments")!;
    assert.deepEqual(JSON.parse(call.text!), { text: "", attachments: [{ kind: "text", name: "notes.txt", text: content }] });
    await page.getByRole("region", { name: "Chat messages", exact: true }).getByText("notes.txt", { exact: true }).waitFor();
    assert.equal(await page.getByRole("button", { name: "Remove notes.txt" }).count(), 0);
    assert.deepEqual(errors, []);
  } finally { await page.close(); }
});

const tinyPng = Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGP4z8AAAAMBAQDJ/pLvAAAAAElFTkSuQmCC", "base64");
for (const vision of [false, true]) {
  test(`image-only input respects model capability (${vision})`, async () => {
    const { page, errors } = await openApp("linux", false, [], "medium", vision);
    try {
      await page.locator('input[type="file"]').setInputFiles({ name: "pixel.png", mimeType: "image/png", buffer: tinyPng });
      await page.getByRole("button", { name: "Remove pixel.png" }).waitFor();
      const send = page.getByRole("button", { name: "Send", exact: true });
      if (!vision) {
        assert.ok(await send.isDisabled());
        await page.getByText("Choose a model with image support to send these images.").waitFor();
      } else {
        await send.click();
        await page.waitForFunction(() => (window as any).__deletionTest.calls.some((call: any) => call.method === "attachments"));
        const call = (await calls(page)).find((call) => call.method === "attachments")!;
        assert.equal(JSON.parse(call.text!).attachments[0].image.data, tinyPng.toString("base64"));
      }
      assert.deepEqual(errors, []);
    } finally { await page.close(); }
  });
}

function textPdf(text: string): Buffer {
  const stream = `BT /F1 12 Tf 72 720 Td (${text}) Tj ET`;
  const objects = ["<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [3 0 R] /Count 1 >>", "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>", "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>", `<< /Length ${stream.length} >>\nstream\n${stream}\nendstream`];
  let pdf = "%PDF-1.4\n";
  const offsets = [0];
  for (const [index, object] of objects.entries()) { offsets.push(pdf.length); pdf += `${index + 1} 0 obj\n${object}\nendobj\n`; }
  const xref = pdf.length;
  pdf += `xref\n0 6\n0000000000 65535 f \n${offsets.slice(1).map((offset) => String(offset).padStart(10, "0") + " 00000 n \n").join("")}trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n${xref}\n%%EOF`;
  return Buffer.from(pdf);
}

test("PDF text extraction uses the local bundled worker and unsupported documents keep the draft", async () => {
  const { page, errors } = await openApp("linux", false, [], "medium", false, false);
  try {
    await page.locator('input[type="file"]').setInputFiles({ name: "notes.pdf", mimeType: "application/pdf", buffer: textPdf("Local PDF fact") });
    await page.getByRole("button", { name: "Remove notes.pdf" }).waitFor();
    await page.getByRole("button", { name: "Send", exact: true }).click();
    await page.waitForFunction(() => (window as any).__deletionTest.calls.some((call: any) => call.method === "attachments"));
    const call = (await calls(page)).find((call) => call.method === "attachments")!;
    assert.match(JSON.parse(call.text!).attachments[0].text, /Local PDF fact/);
    await page.locator("textarea[data-chat-input]").fill("Keep this draft");
    await page.locator('input[type="file"]').setInputFiles({ name: "binary.docx", mimeType: "application/octet-stream", buffer: Buffer.from("binary") });
    await page.getByRole("alert").filter({ hasText: "Export Office documents" }).waitFor();
    assert.equal(await page.locator("textarea[data-chat-input]").inputValue(), "Keep this draft");
    assert.deepEqual(errors, []);
  } finally { await page.close(); }
});


test("gift redemption is masked, updates balance, retries safely and clears on account switch", async () => {
  const {page, errors} = await openApp();
  try {
    await page.evaluate(() => {
      (window as any).__giftCalls = [];
      (window as any).axiomDesktop.agent.redeemGiftCode = async (code: string, accountId: string) => {
        (window as any).__giftCalls.push({code, accountId});
        if ((window as any).__giftFail) throw new Error("Gift code is invalid or unavailable.");
        if ((window as any).__giftHold) await new Promise<void>(resolve => { (window as any).__giftRelease = resolve; });
        const status = {revision: 2, ledgerSequence: 2, currency: "microUSD", postedMicrousd: 25_000_000,
          availableMicrousd: 25_000_000, paidMicrousd: 25_000_000, trialMicrousd: 0,
          paymentReviewRequired: false, paymentAccount: null, zecUsdQuote: null};
        (window as any).__deletionTest.setBilling(status);
        return {creditedMicrousd: 25_000_000, alreadyRedeemed: false, status};
      };
    });
    await openAccountScreen(page, "Balance");
    const code = "AXG-AAAA-AAAA-AAAA-AAAA-AAAA-AAAA-AAAA-AAAA";
    const field = page.getByLabel("Gift code", {exact: true});
    await field.fill(code);
    assert.equal(await field.getAttribute("type"), "password");
    if (process.env.AXIOM_GIFT_CAPTURE_PATH) await page.screenshot({path: process.env.AXIOM_GIFT_CAPTURE_PATH});
    await page.getByRole("button", {name: "Redeem", exact: true}).click();
    await page.getByRole("status").filter({hasText: "$25.00 added to your account."}).waitFor();
    assert.equal(await field.inputValue(), "");
    const calls = await page.evaluate(() => (window as any).__giftCalls);
    assert.deepEqual(calls, [{code, accountId: "test-account"}]);
    assert(!await page.locator("body").innerText().then(text => text.includes(code)));
    assert.equal(await page.evaluate(() => (window as any).__deletionTest.calls.filter((c: any) => c.method === "prompt").length), 0);
    await page.evaluate(() => { (window as any).__giftFail = true; });
    await field.fill(code);
    await page.getByRole("button", {name: "Redeem", exact: true}).click();
    await page.getByRole("alert").filter({hasText: "retry the same code safely"}).waitFor();
    assert.equal(await field.inputValue(), code, "retain the code for a safe retry");
    await page.evaluate(() => { (window as any).__giftFail = false; (window as any).__giftHold = true; });
    await page.getByRole("button", {name: "Redeem", exact: true}).click();
    await page.waitForFunction(() => !!(window as any).__giftRelease);
    assert.equal(await page.getByRole("button", {name: "Redeeming…", exact: true}).isDisabled(), true);
    await page.evaluate(() => (window as any).__deletionTest.setAccount("another-account"));
    await page.evaluate(() => (window as any).__giftRelease());
    assert.equal(await page.getByText("$25.00 added to your account.", {exact: true}).count(), 0);
    // Account navigation may dismiss Balance; whichever screen remains cannot retain the code.
    assert.equal(await page.locator('input[name="gift-code"]').evaluateAll(inputs => inputs.some(input => (input as HTMLInputElement).value.length > 0)), false);
    assert.deepEqual(errors, []);
  } finally { await page.close(); }
});
