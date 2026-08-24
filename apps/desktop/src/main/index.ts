import { mkdir, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { isAbsolute } from "node:path";
import { fileURLToPath } from "node:url";
import { app, BrowserWindow, ipcMain, Menu, nativeImage, shell } from "electron";
import type { ElicitationOutcome, ListSessionsRequest, PermissionOutcome, SetSessionSettingsRequest } from "@axiom/axiom-acp-client";
import { AgentSidecar } from "./agent-sidecar";

const __dirname = dirname(fileURLToPath(import.meta.url));

const WINDOW_WIDTH = 1320;
const WINDOW_HEIGHT = 860;
const BACKGROUND = "#0e0e10";
const APP_NAME = "Axiom";
const APP_ID = "stream.axiom.desktop";
const LINUX_DESKTOP_FILE = `${APP_ID}.desktop`;
const sidecar = new AgentSidecar();
let shutdownStarted = false;

app.setName(APP_NAME);
app.setAppUserModelId(APP_ID);
if (process.platform === "linux") {
  const linuxApp = app as typeof app & { setDesktopName?: (name: string) => void };
  linuxApp.setDesktopName?.(LINUX_DESKTOP_FILE);
  app.commandLine.appendSwitch("class", APP_ID);
}

function boundedString(value: unknown, label: string, max = 4096): string {
  if (typeof value !== "string" || !value || value.length > max) {
    throw new Error(`${label} is invalid`);
  }
  return value;
}

function sessionIds(value: unknown): string[] {
  if (!Array.isArray(value) || value.length === 0 || value.length > 1_000) {
    throw new Error("session selection is invalid");
  }
  const ids = value.map((id) => boundedString(id, "session ID", 128));
  if (new Set(ids).size !== ids.length) throw new Error("session selection contains duplicates");
  return ids;
}

function installAgentIpc(): void {
  ipcMain.handle("agent:state", () => sidecar.state());
  ipcMain.handle("agent:choose-workspace", () => sidecar.chooseWorkspace());
  ipcMain.handle("agent:session-new", (_event, cwd: unknown) => {
    const path = boundedString(cwd, "workspace path", 16_384);
    if (!isAbsolute(path)) throw new Error("workspace path must be absolute");
    return sidecar.newSession(path);
  });
  ipcMain.handle("agent:session-load", (_event, sessionId: unknown, cwd: unknown) => {
    const path = boundedString(cwd, "workspace path", 16_384);
    if (!isAbsolute(path)) throw new Error("workspace path must be absolute");
    return sidecar.loadSession(boundedString(sessionId, "session ID", 128), path);
  });
  ipcMain.handle("agent:prompt", (_event, sessionId: unknown, prompt: unknown) =>
    sidecar.prompt(boundedString(sessionId, "session ID", 128), boundedString(prompt, "prompt", 4 * 1024 * 1024)),
  );
  ipcMain.handle("agent:cancel", (_event, sessionId: unknown) =>
    sidecar.cancel(boundedString(sessionId, "session ID", 128)),
  );
  ipcMain.handle("agent:set-mode", (_event, sessionId: unknown, modeId: unknown) =>
    sidecar.setMode(boundedString(sessionId, "session ID", 128), boundedString(modeId, "mode ID", 64)),
  );
  ipcMain.handle("agent:set-config", (_event, sessionId: unknown, configId: unknown, value: unknown) =>
    sidecar.setConfig(
      boundedString(sessionId, "session ID", 128),
      boundedString(configId, "config ID", 64),
      boundedString(value, "configuration value", 512),
    ),
  );
  ipcMain.handle("agent:sessions-list", (_event, value: unknown) => {
    if (value !== undefined && (!value || typeof value !== "object")) throw new Error("session query is invalid");
    const raw = (value ?? {}) as Record<string, unknown>;
    const request: ListSessionsRequest = {};
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
    return sidecar.listSessions(request);
  });
  ipcMain.handle("agent:models-list", () => sidecar.listModels());
  ipcMain.handle("agent:set-settings", (_event, value: unknown) => {
    if (!value || typeof value !== "object") throw new Error("settings are invalid");
    const raw = value as Record<string, unknown>;
    const settings: SetSessionSettingsRequest = {
      sessionId: boundedString(raw.sessionId, "session ID", 128),
    };
    if (raw.model !== undefined) settings.model = boundedString(raw.model, "model", 512);
    if (raw.thinkingLevel !== undefined) settings.thinkingLevel = boundedString(raw.thinkingLevel, "thinking level", 32);
    if (raw.permissionProfile !== undefined) settings.permissionProfile = boundedString(raw.permissionProfile, "permission profile", 32);
    return sidecar.setSettings(settings);
  });
  ipcMain.handle("agent:account-status", () => sidecar.accountStatus());
  ipcMain.handle("agent:browser-login-start", () => sidecar.browserLoginStart());
  ipcMain.handle("agent:browser-login-complete", (_event, loginId: unknown) =>
    sidecar.browserLoginComplete(boundedString(loginId, "login ID", 128)),
  );
  ipcMain.handle("agent:browser-login-cancel", (_event, loginId: unknown) => {
    sidecar.browserLoginCancel(boundedString(loginId, "login ID", 128));
  });
  ipcMain.handle("agent:use-api-key", (_event, apiKey: unknown) =>
    sidecar.useApiKey(boundedString(apiKey, "Axiom API key", 4096)),
  );
  ipcMain.handle("agent:logout", () => sidecar.logout());
  ipcMain.handle("agent:security-verify", (_event, sessionId: unknown) =>
    sidecar.verifySecurity(boundedString(sessionId, "session ID", 128)),
  );
  ipcMain.handle("agent:compact", (_event, sessionId: unknown, focus: unknown) =>
    sidecar.compact(
      boundedString(sessionId, "session ID", 128),
      focus === undefined ? undefined : boundedString(focus, "compaction focus", 4096),
    ),
  );
  ipcMain.handle("agent:delete-preview", (_event, ids: unknown) => sidecar.deletePreview(sessionIds(ids)));
  ipcMain.handle("agent:delete-confirm", (_event, token: unknown, ids: unknown) =>
    sidecar.deleteConfirm(boundedString(token, "confirmation token", 128), sessionIds(ids)),
  );
  ipcMain.handle("agent:permission-resolve", (_event, interactionId: unknown, outcome: PermissionOutcome) => {
    if (!outcome || (outcome.outcome !== "selected" && outcome.outcome !== "cancelled")) throw new Error("permission outcome is invalid");
    sidecar.resolvePermission(boundedString(interactionId, "interaction ID", 128), outcome);
  });
  ipcMain.handle("agent:elicitation-resolve", (_event, interactionId: unknown, outcome: ElicitationOutcome) => {
    if (!outcome || !["accept", "decline", "cancel"].includes(outcome.action)) throw new Error("elicitation outcome is invalid");
    sidecar.resolveElicitation(boundedString(interactionId, "interaction ID", 128), outcome);
  });
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

function createWindow(): BrowserWindow {
  const iconPath = join(__dirname, "../../resources/icon.png");
  const icon = nativeImage.createFromPath(iconPath);

  const window = new BrowserWindow({
    width: WINDOW_WIDTH,
    height: WINDOW_HEIGHT,
    minWidth: 960,
    minHeight: 640,
    backgroundColor: BACKGROUND,
    frame: false,
    autoHideMenuBar: true,
    title: "Axiom",
    show: false,
    icon: icon.isEmpty() ? undefined : icon,
    webPreferences: {
      preload: join(__dirname, "../preload/index.mjs"),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: false,
    },
  });

  window.setMenuBarVisibility(false);
  Menu.setApplicationMenu(null);

  window.on("ready-to-show", () => {
    if (!captureDirFromArgv(process.argv)) {
      window.show();
    }
  });

  window.webContents.setWindowOpenHandler((details) => {
    void shell.openExternal(details.url);
    return { action: "deny" };
  });

  const emitMaximized = () => {
    window.webContents.send("window:maximized-changed", window.isMaximized());
  };
  window.on("maximize", emitMaximized);
  window.on("unmaximize", emitMaximized);

  if (process.env.ELECTRON_RENDERER_URL) {
    void window.loadURL(process.env.ELECTRON_RENDERER_URL);
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
  await delay(1800);

  const shots: Array<{ name: string; script?: string; wait: number }> = [
    { name: "conversation", wait: 400 },
    { name: "welcome", script: `window.__axiomDemo?.showWelcome()`, wait: 1600 },
    { name: "proxy", script: `window.__axiomDemo?.showProxy()`, wait: 900 },
    { name: "folder", script: `window.__axiomDemo?.showFolder()`, wait: 700 },
  ];

  for (const shot of shots) {
    if (shot.script) {
      await window.webContents.executeJavaScript(shot.script);
    }
    await delay(shot.wait);
    const image = await window.capturePage();
    await writeFile(join(outputDir, `${shot.name}.png`), image.toPNG());
  }
}

app.whenReady().then(() => {
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
