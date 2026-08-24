import { access } from "node:fs/promises";
import { constants } from "node:fs";
import { resolve } from "node:path";
import { app, BrowserWindow, dialog, shell } from "electron";
import {
  AxiomAcpClient,
  type ClientState,
  type ElicitationOutcome,
  type ListSessionsRequest,
  type PendingInteraction,
  type PermissionOutcome,
  type SetSessionSettingsRequest,
} from "@axiom/axiom-acp-client";

export const AGENT_STATE_CHANNEL = "agent:state-changed";
export const AGENT_INTERACTION_CHANNEL = "agent:interaction";

function broadcast(channel: string, value: unknown): void {
  for (const window of BrowserWindow.getAllWindows()) window.webContents.send(channel, value);
}

async function executable(path: string): Promise<boolean> {
  try {
    await access(path, constants.X_OK);
    return true;
  } catch {
    return false;
  }
}

async function resolveSidecar(): Promise<string> {
  const packaged = resolve(process.resourcesPath, "bin", process.platform === "win32" ? "axiomcli.exe" : "axiomcli");
  if (app.isPackaged) {
    if (!(await executable(packaged))) throw new Error(`Packaged AxiomCLI sidecar is missing: ${packaged}`);
    return packaged;
  }
  const override = process.env.AXIOMCLI_SIDECAR;
  const development = override
    ? resolve(override)
    : resolve(app.getAppPath(), "../../target/debug", process.platform === "win32" ? "axiomcli.exe" : "axiomcli");
  if (!(await executable(development))) {
    throw new Error(`Build AxiomCLI before starting desktop: cargo build -p axiomcli (${development})`);
  }
  return development;
}

export class AgentSidecar {
  private client: AxiomAcpClient | null = null;
  private lastState: ClientState = {
    connected: false,
    runtimeInstanceId: null,
    lastSequence: 0,
    sessions: {},
    catalog: [],
    account: null,
    diagnostic: "",
    error: null,
  };
  private starting: Promise<void> | null = null;
  private shuttingDown = false;
  private restartAttempt = 0;
  private readonly loginControllers = new Map<string, AbortController>();

  async start(): Promise<void> {
    if (this.client) return;
    if (this.starting) return this.starting;
    this.starting = this.startOnce().finally(() => {
      this.starting = null;
    });
    return this.starting;
  }

  private async startOnce(): Promise<void> {
    const command = await resolveSidecar();
    const client = new AxiomAcpClient({ command, args: ["acp"], cwd: app.getPath("home") });
    this.client = client;
    client.on("state", (state: ClientState) => {
      this.lastState = state;
      broadcast(AGENT_STATE_CHANNEL, state);
    });
    client.on("interaction", (interaction: PendingInteraction) => {
      broadcast(AGENT_INTERACTION_CHANNEL, interaction);
    });
    client.process.once("exit", () => {
      if (this.client === client) this.client = null;
      if (!this.shuttingDown && this.restartAttempt < 3) {
        const delay = 250 * 2 ** this.restartAttempt++;
        setTimeout(() => void this.start().catch(() => undefined), delay);
      }
    });
    try {
      await client.initialize();
      this.restartAttempt = 0;
      await client.accountStatus();
      await client.listSessions();
    } catch (error) {
      if (this.client === client) this.client = null;
      await client.close().catch(() => undefined);
      this.lastState = {
        ...this.lastState,
        connected: false,
        error: error instanceof Error ? error.message : String(error),
      };
      broadcast(AGENT_STATE_CHANNEL, this.lastState);
      throw error;
    }
  }

  async stop(): Promise<void> {
    this.shuttingDown = true;
    for (const controller of this.loginControllers.values()) controller.abort();
    this.loginControllers.clear();
    const client = this.client;
    this.client = null;
    if (client) await client.close();
  }

  state(): ClientState {
    return this.client?.getState() ?? structuredClone(this.lastState);
  }

  async chooseWorkspace(): Promise<string | null> {
    const result = await dialog.showOpenDialog({ properties: ["openDirectory", "createDirectory"] });
    return result.canceled ? null : (result.filePaths[0] ?? null);
  }

  newSession(cwd: string) {
    return this.requireClient().newSession(cwd);
  }

  loadSession(sessionId: string, cwd: string) {
    return this.requireClient().loadSession(sessionId, cwd);
  }

  prompt(sessionId: string, text: string) {
    return this.requireClient().prompt(sessionId, text);
  }

  cancel(sessionId: string) {
    return this.requireClient().cancel(sessionId);
  }

  setMode(sessionId: string, modeId: string) {
    return this.requireClient().setMode(sessionId, modeId);
  }

  setConfig(sessionId: string, configId: string, value: string) {
    return this.requireClient().setConfig(sessionId, configId, value);
  }

  listSessions(request: ListSessionsRequest = {}) {
    return this.requireClient().listSessions(request);
  }

  listModels() {
    return this.requireClient().listModels();
  }

  setSettings(settings: SetSessionSettingsRequest) {
    return this.requireClient().setSettings(settings);
  }

  accountStatus() {
    return this.requireClient().accountStatus();
  }

  async browserLoginStart() {
    const result = await this.requireClient().browserLoginStart();
    if (!result.login.browserOpened) await shell.openExternal(result.login.authorizationUrl);
    return result;
  }

  async browserLoginComplete(loginId: string) {
    const controller = new AbortController();
    this.loginControllers.set(loginId, controller);
    try {
      return await this.requireClient().browserLoginComplete(loginId, controller.signal);
    } finally {
      this.loginControllers.delete(loginId);
    }
  }

  browserLoginCancel(loginId: string): void {
    this.loginControllers.get(loginId)?.abort();
  }

  useApiKey(apiKey: string) {
    return this.requireClient().useApiKey(apiKey);
  }

  logout() {
    return this.requireClient().logout();
  }

  verifySecurity(sessionId: string) {
    return this.requireClient().verifySecurity(sessionId);
  }

  compact(sessionId: string, focus?: string) {
    return this.requireClient().compact(sessionId, focus);
  }

  deletePreview(sessionIds: string[]) {
    return this.requireClient().deletePreview(sessionIds);
  }

  deleteConfirm(confirmationToken: string, sessionIds: string[]) {
    return this.requireClient().deleteConfirm(confirmationToken, sessionIds);
  }

  resolvePermission(interactionId: string, outcome: PermissionOutcome): void {
    this.requireClient().resolvePermission(interactionId, outcome);
  }

  resolveElicitation(interactionId: string, outcome: ElicitationOutcome): void {
    this.requireClient().resolveElicitation(interactionId, outcome);
  }

  private requireClient(): AxiomAcpClient {
    if (!this.client) throw new Error("AxiomCLI sidecar is not connected");
    return this.client;
  }
}
