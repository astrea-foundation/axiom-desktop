import { mkdir, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { app, BrowserWindow, ipcMain, Menu, nativeImage, nativeTheme, shell, type IpcMainInvokeEvent } from "electron";
import type { ElicitationOutcome, ListThreadsRequest, PermissionOutcome, ThreadSettingsRequest } from "@axiom/axiom-acp-client";
import { AgentSidecar } from "./agent-sidecar";
import { platformWindowChrome, windowControlsOverlay } from "./window-chrome";

const __dirname = dirname(fileURLToPath(import.meta.url));

const WINDOW_WIDTH = 1320;
const WINDOW_HEIGHT = 860;
const BACKGROUND = "#f2f2f4";
const BACKGROUND_DARK = "#141416";
const APP_NAME = "Axiom";
const APP_ID = "stream.axiom.desktop";
// TODO(brand): confirm the legal line before release.
const COPYRIGHT = "© 2026 Astrea Foundation";
const LINUX_DESKTOP_FILE = `${APP_ID}.desktop`;
const sidecar = new AgentSidecar();
let shutdownStarted = false;
// Dev-only: preview another platform's chrome (AXIOM_DESKTOP_PLATFORM_PREVIEW=win32).
const PLATFORM_PREVIEW = app.isPackaged ? undefined : process.env.AXIOM_DESKTOP_PLATFORM_PREVIEW;

app.setName(APP_NAME);
app.setAppUserModelId(APP_ID);
if (process.platform === "linux") {
  const linuxApp = app as typeof app & { setDesktopName?: (name: string) => void };
  linuxApp.setDesktopName?.(LINUX_DESKTOP_FILE);
  app.commandLine.appendSwitch("class", APP_ID);
}
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
    return handler(event, ...args);
  });
}

function installAgentIpc(): void {
  agentHandle("agent:state", () => sidecar.state());
  agentHandle("agent:desktop-bootstrap", () => sidecar.desktopBootstrap());
  agentHandle("agent:chat-new", () => sidecar.newChat());
  agentHandle("agent:chat-load", (_event, sessionId: unknown) =>
    sidecar.loadChat(boundedString(sessionId, "session ID", 128)),
  );
  agentHandle("agent:prompt", (_event, sessionId: unknown, prompt: unknown, clientItemId: unknown) =>
    sidecar.prompt(
      boundedString(sessionId, "thread ID", 128),
      boundedString(prompt, "prompt", 4 * 1024 * 1024),
      boundedString(clientItemId, "client item ID", 512),
    ),
  );
  agentHandle("agent:cancel", (_event, sessionId: unknown) =>
    sidecar.cancel(boundedString(sessionId, "session ID", 128)),
  );
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
  agentHandle("agent:browser-login-start", () => sidecar.browserLoginStart());
  agentHandle("agent:browser-login-complete", (_event, loginId: unknown) =>
    sidecar.browserLoginComplete(boundedString(loginId, "login ID", 128)),
  );
  agentHandle("agent:browser-login-cancel", (_event, loginId: unknown) => {
    sidecar.browserLoginCancel(boundedString(loginId, "login ID", 128));
  });
  agentHandle("agent:use-api-key", (_event, apiKey: unknown) =>
    sidecar.useApiKey(boundedString(apiKey, "Axiom API key", 4096)),
  );
  agentHandle("agent:logout", () => sidecar.logout());
  agentHandle("agent:security-verify", (_event, sessionId: unknown) =>
    sidecar.verifySecurity(boundedString(sessionId, "session ID", 128)),
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
    ...platformWindowChrome(process.platform, nativeTheme.shouldUseDarkColors ? "dark" : "light"),
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

if (primaryInstance) void app.whenReady().then(() => {
  installAgentIpc();
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
      if (process.platform !== "darwin") {
        window.setTitleBarOverlay(windowControlsOverlay(resolved));
      }
    }
  });

  const window = createWindow();
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
  void sidecar.stop().finally(() => app.quit());
});
