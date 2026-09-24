import {prepareUpdateRestart} from './update-lifecycle';
import { validatePrompt } from "@axiom/axiom-acp-client";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname, isAbsolute, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { spawn } from "node:child_process";
import { app, BrowserWindow, clipboard, dialog, ipcMain, Menu, nativeImage, nativeTheme, shell, type IpcMainInvokeEvent } from "electron";
import { createChildEnvironment, DESKTOP_SIDECAR_INHERITED_ENV } from "@axiom/axiom-acp-client";
import type { ElicitationOutcome, ListThreadsRequest, LoginMethod, PermissionOutcome, ThreadSettingsRequest } from "@axiom/axiom-acp-client";
import { AgentSidecar, resolveSidecar } from "./agent-sidecar";
import { ProxySupervisor } from "./proxy-supervisor";
import { agentRevision, desktopAgentRequest } from "./desktop-agent-settings";
import { requirePaymentAccount, resolveServiceOrigins, STAGING_API_ORIGIN } from "./service-origins";
import { platformWindowChrome } from "./window-chrome";
import { installMacCli } from "./install-cli";
import { APP_ID, APP_NAME, setLinuxDesktopIdentity } from "../../scripts/desktop-identity.mjs";
import { UpdateService } from "./update-service";
import { updateIdentity } from "./update-identity";
import { nativeUpdates } from "./native-updates";

const __dirname = dirname(fileURLToPath(import.meta.url));

const WINDOW_WIDTH = 1320;
const WINDOW_HEIGHT = 860;
const BACKGROUND = "#f2f2f4";
const BACKGROUND_DARK = "#141416";
const IS_STAGING_PROFILE = !app.isPackaged && process.env.AXIOM_DESKTOP_STAGING_PROFILE === "1";
// TODO(brand): confirm the legal line before release.
const COPYRIGHT = "© 2026 Astrea Foundation";
const proxy = new ProxySupervisor(async (port, accountId, token) => {
  const command = await resolveSidecar();
  const origins = resolveServiceOrigins({ packaged: app.isPackaged, authUrl: process.env.AXIOM_AUTH_URL, apiUrl: process.env.AXIOM_BASE_URL });
  return spawn(command, ["desktop-proxy", "--account-id", accountId, "--bind", `127.0.0.1:${port}`, "--axiom-base-url", origins.api], {
    cwd: app.getPath("home"), windowsHide: true, stdio: "pipe",
    env: createChildEnvironment(process.env, {
      AXIOM_AUTH_URL: origins.auth, AXIOM_BASE_URL: origins.api, AXIOM_PROXY_TOKEN: token,
      ...(IS_STAGING_PROFILE ? {
        XDG_CONFIG_HOME: join(app.getPath("home"), ".axiom-staging", "config"),
        XDG_DATA_HOME: join(app.getPath("home"), ".axiom-staging", "data"),
      } : {}),
    }, ["AXIOM_API_KEY"], DESKTOP_SIDECAR_INHERITED_ENV),
  });
}, (state) => {
  for (const window of BrowserWindow.getAllWindows()) window.webContents.send("proxy:state-changed", state);
});
const sidecar = new AgentSidecar((state) => {
  proxy.setAccount(state.connected && state.account?.state === "valid" ? state.account.account?.id ?? null : null, state.runtimeInstanceId);
});
let shutdownStarted = false;
let updates: UpdateService | undefined;
// Dev-only: preview another platform's chrome (AXIOM_DESKTOP_PLATFORM_PREVIEW=win32).
const PLATFORM_PREVIEW = app.isPackaged ? undefined : process.env.AXIOM_DESKTOP_PLATFORM_PREVIEW;

// Separate Electron storage and the single-instance lock before either is used.
// Packaged releases cannot opt in to development staging via environment variables.
if (IS_STAGING_PROFILE) {
  const origins = resolveServiceOrigins({ packaged: false,
    authUrl: process.env.AXIOM_AUTH_URL, apiUrl: process.env.AXIOM_BASE_URL });
  if (origins.api !== STAGING_API_ORIGIN) throw new Error("Staging profile requires staging service origins");
  app.setPath("userData", join(app.getPath("appData"), "Axiom Staging"));
}
// Electron 37 derives X11 WM_CLASS from this name, not --class. Keep it stable
// across profiles; staging already has isolated userData above.
app.setName(APP_NAME);
app.setAppUserModelId(APP_ID);
// Electron 37 reads CHROME_DESKTOP for its Wayland app_id; setDesktopName is
// unavailable. Also cover packaged launches outside our development script.
setLinuxDesktopIdentity(process.env, process.platform);
const primaryInstance = app.requestSingleInstanceLock();
if (!primaryInstance) app.quit();

function developmentRendererUrl(): string | undefined {
  return app.isPackaged ? undefined : process.env.ELECTRON_RENDERER_URL;
}

function boundedString(value: unknown, label: string, max = 4096): string {
  if (typeof value !== "string" || !value || value.length > max) {
    throw new Error(`${label} is invalid`);
  }
  return value;
}

function threadIds(value: unknown): string[] {
  if (!Array.isArray(value) || value.length === 0 || value.length > 1_000) {
    throw new Error("session selection is invalid");
  }
  const ids = value.map((id) => boundedString(id, "session ID", 128));
  if (new Set(ids).size !== ids.length) throw new Error("session selection contains duplicates");
  return ids;
}

const LOGIN_METHODS = new Set<LoginMethod>(["passkey", "google", "password", "ethereum_wallet"]);

function loginMethod(value: unknown): LoginMethod | undefined {
  if (value === undefined) return undefined;
  if (typeof value !== "string" || !LOGIN_METHODS.has(value as LoginMethod)) {
    throw new Error("login method is invalid");
  }
  return value as LoginMethod;
}

type AgentHandler = (event: IpcMainInvokeEvent, ...args: unknown[]) => unknown;

function agentHandle(channel: string, handler: AgentHandler): void {
  ipcMain.handle(channel, (event, ...args) => {
    if (event.senderFrame !== event.sender.mainFrame) throw new Error("agent IPC requires the main renderer frame");
    const actual = new URL(event.senderFrame.url);
    const development = developmentRendererUrl();
    const trusted = development
      ? actual.origin === new URL(development).origin
      : actual.href === pathToFileURL(join(__dirname, "../renderer/index.html")).href;
    if (!trusted || !BrowserWindow.fromWebContents(event.sender)) {
      throw new Error("agent IPC rejected an untrusted renderer");
    }
    if (updates?.snapshot().status === 'installing' && (channel.startsWith('agent:prompt') || channel === 'proxy:start')) {
      throw new Error('Axiom is installing an update. Wait for it to restart.');
    }
    return handler(event, ...args);
  });
}

function installAgentIpc(): void {
  agentHandle("proxy:state", () => proxy.snapshot());
  agentHandle("proxy:start", (_event, port: unknown, accountId: unknown, runtimeId: unknown) => {
    if (!Number.isInteger(port) || (port as number) < 1 || (port as number) > 65535) throw new Error("Port must be between 1 and 65535");
    return proxy.start(port as number, boundedString(accountId, "account ID", 128), boundedString(runtimeId, "runtime ID", 128));
  });
  agentHandle("proxy:stop", () => proxy.stop());
  agentHandle("proxy:copy-token", (_event, accountId: unknown, runtimeId: unknown) => {
    clipboard.writeText(proxy.copyToken(boundedString(accountId, "account ID", 128), boundedString(runtimeId, "runtime ID", 128)));
  });
  agentHandle("agent:state", () => sidecar.state());
  agentHandle("agent:usage-summary", async (_event, accountId: unknown, period: unknown = "all_time", timezone: unknown = "UTC") => {
    const id = boundedString(accountId, "account ID", 128);
    if (period !== "week" && period !== "month" && period !== "all_time") throw new Error("Invalid usage period");
    const zone = boundedString(timezone, "timezone", 128);
    requirePaymentAccount(id, sidecar.state());
    const result = await sidecar.usageSummary({ period, timezone: zone });
    requirePaymentAccount(id, sidecar.state());
    return result;
  });
  agentHandle("agent:desktop-bootstrap", () => sidecar.desktopBootstrap());
  agentHandle("agent:chat-new", () => sidecar.newChat());
  agentHandle("agent:chat-load", (_event, sessionId: unknown) =>
    sidecar.loadChat(boundedString(sessionId, "session ID", 128)),
  );
  // Do not alias the legacy channel: mismatched renderer/preload/main builds
  // must fail before a prompt can reach a runtime without Web consent support.
  agentHandle("agent:prompt-with-agent-context", (_event, sessionId: unknown, prompt: unknown, clientItemId: unknown, webEnabled: unknown, revision: unknown) => {
    if (typeof webEnabled !== "boolean") throw new Error("webEnabled must be a boolean");
    return sidecar.prompt(
      boundedString(sessionId, "thread ID", 128),
      boundedString(prompt, "prompt", 4 * 1024 * 1024),
      boundedString(clientItemId, "client item ID", 512),
      webEnabled,
      agentRevision(revision),
    );
  });
  agentHandle("agent:prompt-with-attachments", (_event, sessionId: unknown, text: unknown, clientItemId: unknown, webEnabled: unknown, revision: unknown, attachments: unknown) => {
    if (typeof webEnabled !== "boolean" || typeof text !== "string") throw new Error("Invalid prompt input");
    validatePrompt(text, attachments);
    return sidecar.prompt(boundedString(sessionId, "thread ID", 128), text, boundedString(clientItemId, "client item ID", 512), webEnabled, agentRevision(revision), undefined, attachments);
  });
  agentHandle("agent:attachments", (_event, threadId: unknown, userItemId: unknown) =>
    sidecar.getAttachments(boundedString(threadId, "thread ID", 128), boundedString(userItemId, "user message ID", 512)));
  agentHandle("agent:prompt-revise", (_event, sessionId: unknown, text: unknown, userItemId: unknown, expectedRevision: unknown, webEnabled: unknown, revision: unknown) => {
    if (typeof webEnabled !== "boolean") throw new Error("webEnabled must be a boolean");
    if (typeof text !== "string") throw new Error("Invalid prompt input");
    validatePrompt(text, [], true);
    return sidecar.prompt(boundedString(sessionId, "thread ID", 128), text,
      crypto.randomUUID(), webEnabled, agentRevision(revision), {
        userItemId: boundedString(userItemId, "user message ID", 512), expectedRevision: agentRevision(expectedRevision),
      });
  });
  agentHandle("agent:cancel", (_event, sessionId: unknown) =>
    sidecar.cancel(boundedString(sessionId, "session ID", 128)),
  );
  agentHandle("agent:steer", (_event, value: unknown) => {
    if (!value || typeof value !== "object") throw new Error("Invalid steering request");
    const request = value as Record<string, unknown>;
    if (typeof request.webEnabled !== "boolean") throw new Error("webEnabled must be a boolean");
    return sidecar.steer({
      threadId: boundedString(request.threadId, "thread ID", 128),
      expectedTurnId: boundedString(request.expectedTurnId, "turn ID", 128),
      clientItemId: boundedString(request.clientItemId, "client item ID", 128),
      text: boundedString(request.text, "steering input", 64 * 1024),
      webEnabled: request.webEnabled,
      agentRevision: agentRevision(request.agentRevision ?? 0),
    });
  });
  agentHandle("agent:desktop-agent-configure", (_event, value: unknown) => sidecar.configureDesktopAgent(desktopAgentRequest(value)));
  agentHandle("agent:working-directory-choose", async (event, current: unknown) => {
    const state = sidecar.state();
    if (!state.connected || state.account?.state !== "valid") throw new Error("Sign in before choosing a working directory");
    const owner = BrowserWindow.fromWebContents(event.sender);
    if (!owner) throw new Error("Desktop window is unavailable");
    const defaultPath = current == null ? undefined : boundedString(current, "working directory", 32_768);
    if (defaultPath !== undefined && (!isAbsolute(defaultPath) || defaultPath.includes("\0"))) throw new Error("Working directory must be an absolute path");
    const selected = await dialog.showOpenDialog(owner, { title: "Choose Agent working directory", properties: ["openDirectory", "createDirectory"], defaultPath });
    const latest = sidecar.state();
    if (latest.runtimeInstanceId !== state.runtimeInstanceId || latest.account?.state !== "valid"
      || latest.account.account?.id !== state.account.account?.id) throw new Error("The account changed while choosing a directory");
    return selected.canceled ? null : selected.filePaths[0] ?? null;
  });
  agentHandle("agent:set-config", (_event, sessionId: unknown, configId: unknown, value: unknown) =>
    sidecar.setConfig(
      boundedString(sessionId, "session ID", 128),
      boundedString(configId, "config ID", 64),
      boundedString(value, "configuration value", 512),
    ),
  );
  agentHandle("agent:threads-list", (_event, value: unknown) => {
    if (value !== undefined && (!value || typeof value !== "object")) throw new Error("thread query is invalid");
    const raw = (value ?? {}) as Record<string, unknown>;
    const request: ListThreadsRequest = {};
    if (raw.query !== undefined) request.query = boundedString(raw.query, "query", 512);
    if (raw.cursor !== undefined) request.cursor = boundedString(raw.cursor, "cursor", 64);
    if (raw.includeArchived !== undefined) {
      if (typeof raw.includeArchived !== "boolean") throw new Error("includeArchived is invalid");
      request.includeArchived = raw.includeArchived;
    }
    if (raw.limit !== undefined) {
      if (!Number.isInteger(raw.limit) || (raw.limit as number) < 1 || (raw.limit as number) > 500) throw new Error("session limit is invalid");
      request.limit = raw.limit as number;
    }
    return sidecar.listThreads(request);
  });
  agentHandle("agent:models-list", () => sidecar.listModels());
  agentHandle("agent:thread-rename", (_event, threadId: unknown, title: unknown) =>
    sidecar.renameThread(boundedString(threadId, "thread ID", 128), boundedString(title, "thread title", 512)),
  );
  agentHandle("agent:set-settings", (_event, value: unknown) => {
    if (!value || typeof value !== "object") throw new Error("settings are invalid");
    const raw = value as Record<string, unknown>;
    const settings: ThreadSettingsRequest = {
      threadId: boundedString(raw.threadId, "thread ID", 128),
    };
    if (raw.model !== undefined) settings.model = boundedString(raw.model, "model", 512);
    if (raw.thinkingLevel !== undefined) settings.thinkingLevel = boundedString(raw.thinkingLevel, "thinking level", 32);
    if (raw.permissionProfile !== undefined) {
      throw new Error("desktop chat permission profile cannot be changed");
    }
    return sidecar.setSettings(settings);
  });
  agentHandle("agent:account-status", () => sidecar.accountStatus());
  agentHandle("agent:api-keys", async (_event, accountId: unknown) => {
    const id = boundedString(accountId, "account ID", 128);
    requirePaymentAccount(id, sidecar.state());
    const result = await sidecar.apiKeys();
    requirePaymentAccount(id, sidecar.state());
    return result;
  });
  agentHandle("agent:api-key-create", async (_event, name: unknown, accountId: unknown) => {
    const id = boundedString(accountId, "account ID", 128);
    requirePaymentAccount(id, sidecar.state());
    const result = await sidecar.createApiKey(boundedString(name, "API key name", 480));
    requirePaymentAccount(id, sidecar.state());
    return result;
  });
  agentHandle("agent:api-key-revoke", async (_event, keyId: unknown, accountId: unknown) => {
    const id = boundedString(accountId, "account ID", 128);
    requirePaymentAccount(id, sidecar.state());
    const result = await sidecar.revokeApiKey(boundedString(keyId, "API key ID", 128));
    requirePaymentAccount(id, sidecar.state());
    return result;
  });
  agentHandle("agent:billing-status", () => sidecar.billingStatus());
  agentHandle("agent:redeem-gift-code", async (_event, code: unknown, accountId: unknown) => {
    const id = boundedString(accountId, "account ID", 128);
    requirePaymentAccount(id, sidecar.state());
    const result = await sidecar.redeemGiftCode(boundedString(code, "gift code", 64));
    requirePaymentAccount(id, sidecar.state());
    return result;
  });
  agentHandle("agent:native-login-start", (_event, methodHint: unknown) =>
    sidecar.nativeLoginStart(loginMethod(methodHint)),
  );
  agentHandle("agent:native-login-complete", (_event, loginId: unknown) =>
    sidecar.nativeLoginComplete(boundedString(loginId, "login ID", 128)),
  );
  agentHandle("agent:native-login-cancel", (_event, loginId: unknown) =>
    sidecar.nativeLoginCancel(boundedString(loginId, "login ID", 128)),
  );
  agentHandle("agent:account-open", () => sidecar.openAccountPortal());
  agentHandle("agent:logout", async () => { await proxy.stop(); return sidecar.logout(); });
  agentHandle("agent:security-prewarm", (_event, modelId: unknown) =>
    sidecar.prewarmSecurity(boundedString(modelId, "model ID", 512)),
  );
  agentHandle("agent:security-verify", (_event, sessionId: unknown, acceptOutdatedTee: unknown, modelId: unknown) =>
    sidecar.verifySecurity(boundedString(sessionId, "session ID", 128), acceptOutdatedTee === true,
      acceptOutdatedTee === true ? boundedString(modelId, "model ID", 512) : undefined),
  );
  agentHandle("agent:compact", (_event, sessionId: unknown, focus: unknown) =>
    sidecar.compact(
      boundedString(sessionId, "session ID", 128),
      focus === undefined ? undefined : boundedString(focus, "compaction focus", 4096),
    ),
  );
  agentHandle("agent:delete-preview", (_event, ids: unknown) => sidecar.deletePreview(threadIds(ids)));
  agentHandle("agent:delete-confirm", (_event, token: unknown, ids: unknown) =>
    sidecar.deleteConfirm(boundedString(token, "confirmation token", 128), threadIds(ids)),
  );
  agentHandle("agent:permission-resolve", (_event, interactionId: unknown, outcome: unknown) => {
    if (!outcome || typeof outcome !== "object") throw new Error("permission outcome is invalid");
    const permission = outcome as PermissionOutcome;
    if (permission.outcome !== "selected" && permission.outcome !== "cancelled") throw new Error("permission outcome is invalid");
    sidecar.resolvePermission(boundedString(interactionId, "interaction ID", 128), permission);
  });
  agentHandle("agent:elicitation-resolve", (_event, interactionId: unknown, outcome: unknown) => {
    if (!outcome || typeof outcome !== "object") throw new Error("elicitation outcome is invalid");
    const elicitation = outcome as ElicitationOutcome;
    if (!["accept", "decline", "cancel"].includes(elicitation.action)) throw new Error("elicitation outcome is invalid");
    sidecar.resolveElicitation(boundedString(interactionId, "interaction ID", 128), elicitation);
  });
  agentHandle("agent:collections-list", () => sidecar.listCollections());
  agentHandle("agent:collection-create", (_event, name: unknown) =>
    sidecar.createCollection(boundedString(name, "collection name", 256)),
  );
  agentHandle("agent:collection-rename", (_event, collectionId: unknown, name: unknown) =>
    sidecar.renameCollection(
      boundedString(collectionId, "collection ID", 128),
      boundedString(name, "collection name", 256),
    ),
  );
  agentHandle("agent:collection-collapsed", (_event, collectionId: unknown, collapsed: unknown) => {
    if (typeof collapsed !== "boolean") throw new Error("collection collapsed state is invalid");
    return sidecar.setCollectionCollapsed(
      boundedString(collectionId, "collection ID", 128),
      collapsed,
    );
  });
  agentHandle("agent:collection-move", (_event, collectionId: unknown, position: unknown) => {
    if (!Number.isSafeInteger(position)) throw new Error("collection position is invalid");
    return sidecar.moveCollection(
      boundedString(collectionId, "collection ID", 128),
      position as number,
    );
  });
  agentHandle("agent:collection-delete", (_event, collectionId: unknown) =>
    sidecar.deleteCollection(boundedString(collectionId, "collection ID", 128)),
  );
  agentHandle("agent:collection-assign", (_event, threadId: unknown, collectionId: unknown) =>
    sidecar.assignThreadCollection(
      boundedString(threadId, "thread ID", 128),
      collectionId === null ? null : boundedString(collectionId, "collection ID", 128),
    ),
  );
}

function captureDirFromArgv(argv: string[]): string | null {
  const flag = argv.find((arg) => arg === "--capture" || arg.startsWith("--capture="));
  if (!flag) return null;
  if (flag === "--capture") {
    return join(app.getPath("home"), "axiom-desktop-captures");
  }
  return flag.slice("--capture=".length) || join(app.getPath("home"), "axiom-desktop-captures");
}

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

/**
 * macOS gets a minimal application menu: it is where "About Axiom" lives, and
 * without an Edit menu the system text-editing shortcuts don't reach the
 * renderer. Windows/Linux keep no menu — the window hides its menu bar anyway.
 */
function installApplicationMenu(): void {
  if (process.platform !== "darwin") {
    Menu.setApplicationMenu(null);
    return;
  }
  app.setAboutPanelOptions({
    applicationName: APP_NAME,
    applicationVersion: app.getVersion(),
    version: "",
    copyright: COPYRIGHT,
    iconPath: join(__dirname, "../../resources/icon-mac.png"),
  });
  Menu.setApplicationMenu(Menu.buildFromTemplate([
    {
      label: APP_NAME,
      submenu: [
        { role: "about", label: `About ${APP_NAME}` },
        { label: "Install Command-Line Tool…", enabled: app.isPackaged, click: () => { void installMacCli(); } },
        { type: "separator" },
        { role: "hide", label: `Hide ${APP_NAME}` },
        { role: "hideOthers" },
        { role: "unhide" },
        { type: "separator" },
        { role: "quit", label: `Quit ${APP_NAME}` },
      ],
    },
    { role: "editMenu" },
    { role: "windowMenu" },
  ]));
}

function createWindow(): BrowserWindow {
  const iconPath = join(__dirname, "../../resources/icon.png");
  const icon = nativeImage.createFromPath(iconPath);
  if (!app.isPackaged && process.platform === "darwin") {
    // Packaged builds carry icon.icns; in dev the Dock would show Electron's.
    const dockIcon = nativeImage.createFromPath(join(__dirname, "../../resources/icon-mac.png"));
    if (!dockIcon.isEmpty()) app.dock?.setIcon(dockIcon);
  }

  const window = new BrowserWindow({
    width: WINDOW_WIDTH,
    height: WINDOW_HEIGHT,
    minWidth: 960,
    minHeight: 640,
    backgroundColor: nativeTheme.shouldUseDarkColors ? BACKGROUND_DARK : BACKGROUND,
    ...platformWindowChrome(process.platform),
    autoHideMenuBar: true,
    title: "Axiom",
    show: false,
    icon: icon.isEmpty() ? undefined : icon,
    webPreferences: {
      preload: join(__dirname, "../preload/index.cjs"),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
      additionalArguments: PLATFORM_PREVIEW ? [`--axiom-platform-preview=${PLATFORM_PREVIEW}`] : [],
    },
  });

  window.setMenuBarVisibility(false);
  installApplicationMenu();
  if (PLATFORM_PREVIEW && process.platform === "darwin") window.setWindowButtonVisibility(false);

  window.on("ready-to-show", () => {
    if (!captureDirFromArgv(process.argv)) {
      window.show();
    }
  });

  window.webContents.setWindowOpenHandler((details) => {
    const target = new URL(details.url);
    if (target.protocol === "https:" || target.protocol === "http:") {
      void shell.openExternal(target.toString());
    }
    return { action: "deny" };
  });
  window.webContents.on("will-navigate", (event, url) => {
    if (url !== window.webContents.getURL()) event.preventDefault();
  });

  const emitMaximized = () => {
    window.webContents.send("window:maximized-changed", window.isMaximized());
  };
  window.on("maximize", emitMaximized);
  window.on("unmaximize", emitMaximized);

  const emitFullScreen = () => {
    window.webContents.send("window:fullscreen-changed", window.isFullScreen());
  };
  window.on("enter-full-screen", emitFullScreen);
  window.on("leave-full-screen", emitFullScreen);

  const emitActive = () => {
    window.webContents.send("window:active-changed", window.isFocused());
  };
  window.on("focus", emitActive);
  window.on("blur", emitActive);



  const development = developmentRendererUrl();
  if (development) {
    void window.loadURL(development);
  } else {
    void window.loadFile(join(__dirname, "../renderer/index.html"));
  }

  return window;
}

async function captureStates(window: BrowserWindow, outputDir: string): Promise<void> {
  await mkdir(outputDir, { recursive: true });
  window.setSize(WINDOW_WIDTH, WINDOW_HEIGHT);
  window.show();
  await window.webContents.executeJavaScript(
    `document.fonts?.ready ?? Promise.resolve()`,
  );
  // capturePage never includes OS-drawn window controls, so render the
  // fullscreen layout: no reserved insets, no dead notch in the shots. Sent
  // after fonts settle so the renderer's listener is guaranteed registered.
  window.webContents.send("window:fullscreen-changed", true);
  await delay(1800);

  const shots: Array<{ name: string; script?: string; wait: number; chrome?: boolean }> = [
    { name: "conversation", wait: 400 },
    { name: "welcome", script: `window.__axiomDemo?.showWelcome()`, wait: 1600 },
    { name: "models", script: `window.__axiomDemo?.openModelPicker()`, wait: 400 },
    { name: "signin", script: `window.__axiomDemo?.showSignIn()`, wait: 400 },
    { name: "proxy", script: `window.__axiomDemo?.showProxy()`, wait: 900 },
    { name: "folder", script: `window.__axiomDemo?.showFolder()`, wait: 700 },
    { name: "markdown", script: `window.__axiomDemo?.showThread("thread-markdown")`, wait: 900 },
    {
      name: "markdown-top",
      script: `[...document.querySelectorAll("div")].find((el) => getComputedStyle(el).overflowY === "auto" && el.scrollHeight > el.clientHeight)?.scrollTo(0, 0)`,
      wait: 500,
    },
    {
      name: "markdown-open",
      script: `document.querySelectorAll('.title-fade button[aria-expanded="false"]').forEach((el) => el.click())`,
      wait: 500,
    },
    { name: "errors", script: `window.__axiomDemo?.showThread("thread-states")`, wait: 900 },
    { name: "settings", script: `window.__axiomDemo?.showSettings(true)`, wait: 700 },
    // Windowed, inactive, sidebar collapsed: the traffic-light notch with its ghosts.
    { name: "collapsed", script: `window.__axiomDemo?.showSettings(false); window.__axiomDemo?.setSidebar(false)`, wait: 700, chrome: true },
  ];

  for (const theme of ["light", "dark"] as const) {
    await window.webContents.executeJavaScript(`window.__axiomDemo?.setTheme(${JSON.stringify(theme)})`);
    window.webContents.send("window:fullscreen-changed", true);
    window.webContents.send("window:active-changed", true);
    await window.webContents.executeJavaScript(`window.__axiomDemo?.showSettings(false); window.__axiomDemo?.setSidebar(true)`);
    await window.webContents.executeJavaScript(`window.__axiomDemo?.showThread()`);
    await delay(600);
    for (const shot of shots) {
      await window.webContents.executeJavaScript(`window.__axiomDemo?.showSignIn(false)`);
      if (shot.chrome) {
        window.webContents.send("window:fullscreen-changed", false);
        window.webContents.send("window:active-changed", false);
      }
      if (shot.script) {
        await window.webContents.executeJavaScript(shot.script);
      }
      await delay(shot.wait);
      const image = await window.capturePage();
      await writeFile(join(outputDir, `${theme}-${shot.name}.png`), image.toPNG());
    }
  }
}

if (primaryInstance) app.on("second-instance", () => {
  const window = BrowserWindow.getAllWindows()[0];
  if (!window) return;
  if (window.isMinimized()) window.restore();
  window.show();
  window.focus();
});

if (primaryInstance) void app.whenReady().then(async () => {
  const metadata = app.isPackaged
    ? JSON.parse(await readFile(join(app.getAppPath(), "package.json"), "utf8")) as { axiomUpdateFormat?: unknown; axiomUpdateChannel?: string }
    : {};
  updates = new UpdateService(updateIdentity({ version: app.getVersion(), platform: process.platform,
    arch: process.arch, packaged: app.isPackaged && metadata.axiomUpdateChannel === 'stable', packageFormat: metadata.axiomUpdateFormat, appImage: process.env.APPIMAGE }), {
    run: nativeUpdates(resolveSidecar),
    parentPid: process.pid,
    prepareRestart: prepareUpdateRestart,
    canRestart: () => !Object.values(sidecar.state().sessions).some(session => session.running)
      && !['running', 'starting', 'stopping'].includes(proxy.snapshot().status),
    restart: async () => {
      await Promise.all([proxy.dispose(), sidecar.stop()]);
      app.quit();
    },
    emit: (state) => {
      for (const window of BrowserWindow.getAllWindows()) window.webContents.send("updates:state-changed", state);
    },
  });
  agentHandle("updates:state", () => updates!.snapshot());
  agentHandle("updates:check", () => updates!.check());
  agentHandle("updates:install", () => updates!.install());
  agentHandle("updates:cancel", () => updates!.cancel());
  installAgentIpc();
  ipcMain.on("window:minimize", (event) => {
    BrowserWindow.fromWebContents(event.sender)?.minimize();
  });
  ipcMain.on("window:maximize", (event) => {
    const window = BrowserWindow.fromWebContents(event.sender);
    if (!window) return;
    if (window.isMaximized()) window.unmaximize();
    else window.maximize();
  });
  ipcMain.on("window:close", (event) => {
    BrowserWindow.fromWebContents(event.sender)?.close();
  });
  ipcMain.handle("window:isMaximized", (event) => {
    return BrowserWindow.fromWebContents(event.sender)?.isMaximized() ?? false;
  });
  ipcMain.handle("window:isFullScreen", (event) => {
    return BrowserWindow.fromWebContents(event.sender)?.isFullScreen() ?? false;
  });
  ipcMain.handle("window:isFocused", (event) => {
    return BrowserWindow.fromWebContents(event.sender)?.isFocused() ?? true;
  });
  ipcMain.on("window:set-theme", (event, preference: unknown, resolved: unknown) => {
    if (preference === "light" || preference === "dark" || preference === "system") {
      nativeTheme.themeSource = preference;
    }
    const window = BrowserWindow.fromWebContents(event.sender);
    if (window && (resolved === "light" || resolved === "dark")) {
      window.setBackgroundColor(resolved === "dark" ? BACKGROUND_DARK : BACKGROUND);
    }
  });

  const window = createWindow();
  updates.start();
  void sidecar.start().catch((error: unknown) => {
    console.error("Could not start AxiomCLI sidecar", error);
  });
  const captureDir = captureDirFromArgv(process.argv);

  if (captureDir) {
    window.webContents.once("did-finish-load", () => {
      void captureStates(window, captureDir)
        .then(() => {
          console.log(`Wrote captures to ${captureDir}`);
          app.quit();
        })
        .catch((error: unknown) => {
          console.error(error);
          app.exit(1);
        });
    });
  }

  app.on("activate", () => {
    if (BrowserWindow.getAllWindows().length === 0) createWindow();
  });
});

app.on("window-all-closed", () => {
  if (process.platform !== "darwin") app.quit();
});

app.on("before-quit", (event) => {
  if (shutdownStarted) return;
  event.preventDefault();
  shutdownStarted = true;
  void Promise.all([proxy.dispose(), sidecar.stop(), updates?.dispose()]).finally(() => app.quit());
});
