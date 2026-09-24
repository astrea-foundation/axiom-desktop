import type { PromptAttachment } from "@axiom/axiom-acp-client";
import { access, stat } from "node:fs/promises";
import { constants } from "node:fs";
import { join, resolve } from "node:path";
import { app, BrowserWindow, shell } from "electron";
import {
  AxiomAcpClient,
  DESKTOP_SIDECAR_INHERITED_ENV,
  type ClientState,
  type DesktopBootstrapResponse,
  type ConfigureDesktopAgentRequest,
  type ElicitationOutcome,
  type ListThreadsRequest,
  type LoginMethod,
  type PendingInteraction,
  type PermissionOutcome,
  type ThreadSettingsRequest,
  type SteerTurnRequest,
  type UsageSummaryRequest,
} from "@axiom/axiom-acp-client";
import { RestartPolicy } from "./restart-policy";
import { actionableAcpError } from "./agent-errors";
import { resolveServiceOrigins, trustedNativeAuthorizationUrl } from "./service-origins";

export const AGENT_STATE_CHANNEL = "agent:state-changed";
export const AGENT_INTERACTION_CHANNEL = "agent:interaction";

function broadcast(channel: string, value: unknown): void {
  for (const window of BrowserWindow.getAllWindows()) window.webContents.send(channel, value);
}

async function executable(path: string): Promise<boolean> {
  try {
    const metadata = await stat(path);
    if (!metadata.isFile()) return false;
    if (process.platform === "win32") return true;
    await access(path, constants.X_OK);
    return true;
  } catch {
    return false;
  }
}

export async function resolveSidecar(): Promise<string> {
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
  constructor(private readonly stateChanged: (state: ClientState) => void = () => {}) {}
  private client: AxiomAcpClient | null = null;
  private initializingClient: AxiomAcpClient | null = null;
  private lastState: ClientState = {
    connected: false,
    runtimeInstanceId: null,
    lastSequence: 0,
    sessions: {},
    catalog: [],
    collections: { revision: 0, collections: [] },
    preferences: null,
    account: null,
    billing: null,
    diagnostic: "",
    error: null,
  };
  private starting: Promise<void> | null = null;
  private shuttingDown = false;
  private bootstrap: DesktopBootstrapResponse | null = null;
  private readonly restartPolicy = new RestartPolicy();
  private readonly loginControllers = new Map<string, AbortController>();
  private readonly origins = resolveServiceOrigins({
    packaged: app.isPackaged,
    authUrl: process.env.AXIOM_AUTH_URL,
    apiUrl: process.env.AXIOM_BASE_URL,
  });

  start(): Promise<void> {
    if (this.shuttingDown) return Promise.reject(new Error("Axiom Desktop sidecar is shutting down"));
    if (this.starting) return this.starting;
    if (this.client && this.bootstrap) return Promise.resolve();

    const pending = this.startOnce();
    const tracked = pending.finally(() => {
      if (this.starting === tracked) this.starting = null;
    });
    this.starting = tracked;
    return tracked;
  }

  private async startOnce(): Promise<void> {
    let client: AxiomAcpClient | null = null;
    try {
      const command = await resolveSidecar();
      client = new AxiomAcpClient({
        command,
        args: ["acp", "--frontend", "desktop-chat"],
        cwd: app.getPath("home"),
        env: {
          AXIOM_AUTH_URL: this.origins.auth,
          AXIOM_BASE_URL: this.origins.api,
          // Scope only native account storage, not Electron's OS browser lookup.
          ...(!app.isPackaged && process.env.AXIOM_DESKTOP_STAGING_PROFILE === "1" ? {
            XDG_CONFIG_HOME: join(app.getPath("home"), ".axiom-staging", "config"),
            XDG_DATA_HOME: join(app.getPath("home"), ".axiom-staging", "data"),
          } : {}),
        },
        inheritEnv: DESKTOP_SIDECAR_INHERITED_ENV,
        // API-key automation belongs to the standalone CLI, never the
        // sandboxed Desktop account surface.
        unsetEnv: ["AXIOM_API_KEY"],
      });
      this.initializingClient = client;
      client.on("state", (state: ClientState) => {
        const visibleState = this.client === client
          ? state
          : { ...state, connected: false };
        this.lastState = visibleState;
        this.stateChanged(visibleState);
        broadcast(AGENT_STATE_CHANNEL, visibleState);
      });
      client.on("interaction", (interaction: PendingInteraction) => {
        broadcast(AGENT_INTERACTION_CHANNEL, interaction);
      });
      client.process.once("exit", () => {
        const owned = this.client === client || this.initializingClient === client;
        if (this.client === client) {
          this.client = null;
          this.bootstrap = null;
        }
        if (this.initializingClient === client) this.initializingClient = null;
        if (owned) this.scheduleRestart();
      });
      await client.initialize(app.getVersion());
      const bootstrap = await client.bootstrapDesktop();
      await client.initializeDesktopState();
      if (this.shuttingDown || this.initializingClient !== client) {
        throw new Error("AxiomCLI exited before Desktop initialization completed");
      }
      this.initializingClient = null;
      this.client = client;
      this.bootstrap = bootstrap;
      const readyState = client.getState();
      this.lastState = readyState;
      this.stateChanged(readyState);
      broadcast(AGENT_STATE_CHANNEL, readyState);
      this.restartPolicy.markReady(() => !this.shuttingDown && this.client === client);
      this.refreshAccountInBackground(client);
    } catch (error) {
      const owned = client !== null && (this.client === client || this.initializingClient === client);
      if (client && this.client === client) this.client = null;
      if (client && this.initializingClient === client) this.initializingClient = null;
      this.bootstrap = null;
      if (client && owned) await client.close().catch(() => undefined);
      this.lastState = {
        ...this.lastState,
        connected: false,
        error: error instanceof Error ? error.message : String(error),
      };
      this.stateChanged(this.lastState);
      broadcast(AGENT_STATE_CHANNEL, this.lastState);
      this.scheduleRestart();
      throw error;
    }
  }

  private scheduleRestart(): void {
    if (this.shuttingDown) return;
    this.restartPolicy.scheduleRestart(() => {
      const pending = this.starting?.catch(() => undefined) ?? Promise.resolve();
      void pending
        .then(() => this.start())
        .catch(() => undefined);
    });
  }

  private refreshAccountInBackground(client: AxiomAcpClient): void {
    void client.accountStatus().catch((error: unknown) => {
      if (this.shuttingDown || this.client !== client) return;
      const detail = error instanceof Error ? error.message : String(error);
      const revision = client.state.snapshot().account?.revision ?? 0;
      client.state.setAccount({
        revision,
        state: "unavailable",
        detail: `Could not refresh the Axiom account: ${detail}`,
      });
    });
  }

  async stop(): Promise<void> {
    this.shuttingDown = true;
    this.restartPolicy.cancelAll();
    for (const controller of this.loginControllers.values()) controller.abort();
    this.loginControllers.clear();
    const clients = new Set(
      [this.client, this.initializingClient].filter((client): client is AxiomAcpClient => client !== null),
    );
    this.client = null;
    this.initializingClient = null;
    this.bootstrap = null;
    await Promise.all([...clients].map((client) => client.close().catch(() => undefined)));
    await this.starting?.catch(() => undefined);
  }

  state(): ClientState {
    return this.client?.getState() ?? structuredClone(this.lastState);
  }

  async desktopBootstrap(): Promise<DesktopBootstrapResponse> {
    await this.start();
    if (!this.client || !this.bootstrap) throw new Error("Axiom Desktop sidecar did not finish bootstrapping");
    return structuredClone(this.bootstrap);
  }

  newChat() {
    return this.requireClient().newChat();
  }

  loadChat(sessionId: string) {
    return this.requireClient().loadChat(sessionId);
  }

  configureDesktopAgent(request: ConfigureDesktopAgentRequest) { return this.requireClient().configureDesktopAgent(request); }

  getAttachments(threadId: string, userItemId: string) { return this.requireClient().getAttachments(threadId, userItemId); }

  prompt(sessionId: string, text: string, clientItemId: string, webEnabled = false, agentRevision = 0, revision?: { userItemId: string; expectedRevision: number }, attachments: PromptAttachment[] = []) {
    return this.requireClient().prompt(sessionId, text, clientItemId, webEnabled, agentRevision, revision, attachments);
  }

  steer(request: SteerTurnRequest) { return this.requireClient().steer(request); }

  cancel(sessionId: string) {
    return this.requireClient().cancel(sessionId);
  }

  setConfig(sessionId: string, configId: string, value: string) {
    return this.requireClient().setConfig(sessionId, configId, value);
  }

  listThreads(request: ListThreadsRequest = {}) {
    return this.requireClient().listThreads(request);
  }

  async renameThread(threadId: string, title: string) {
    try {
      return await this.requireClient().renameThread(threadId, title);
    } catch (error) {
      throw actionableAcpError(error);
    }
  }

  async listModels() {
    try {
      return await this.requireClient().listModels();
    } catch (error) {
      throw actionableAcpError(error);
    }
  }

  setSettings(settings: ThreadSettingsRequest) {
    return this.requireClient().setSettings(settings);
  }

  accountStatus() {
    return this.requireClient().accountStatus();
  }

  billingStatus() {
    return this.requireClient().billingStatus().catch((error: unknown) => {
      throw actionableAcpError(error);
    });
  }

  redeemGiftCode(code: string) {
    return this.requireClient().redeemGiftCode(code).catch((error: unknown) => {
      throw actionableAcpError(error);
    });
  }

  apiKeys() {
    return this.requireClient().apiKeys().catch((error: unknown) => { throw actionableAcpError(error); });
  }

  createApiKey(name: string) {
    return this.requireClient().createApiKey(name).catch((error: unknown) => { throw actionableAcpError(error); });
  }

  revokeApiKey(id: string) {
    return this.requireClient().revokeApiKey(id).catch((error: unknown) => { throw actionableAcpError(error); });
  }

  usageSummary(request: UsageSummaryRequest = {}) {
    return this.requireClient().usageSummary(request).catch((error: unknown) => {
      throw actionableAcpError(error);
    });
  }

  async nativeLoginStart(methodHint?: LoginMethod) {
    const result = await this.requireClient().nativeLoginStart(methodHint).catch((error: unknown) => {
      throw actionableAcpError(error);
    });
    const authorizationUrl = trustedNativeAuthorizationUrl(
      result.login.authorizationUrl,
      this.origins.auth,
    );
    if (!result.login.browserOpened) await shell.openExternal(authorizationUrl);
    return { ...result, login: { ...result.login, browserOpened: true } };
  }

  async nativeLoginComplete(loginId: string) {
    const controller = new AbortController();
    this.loginControllers.set(loginId, controller);
    try {
      return await this.requireClient().nativeLoginComplete(loginId, controller.signal);
    } catch (error) {
      throw actionableAcpError(error);
    } finally {
      this.loginControllers.delete(loginId);
    }
  }

  nativeLoginCancel(loginId: string) {
    this.loginControllers.get(loginId)?.abort();
    return this.requireClient().nativeLoginCancel(loginId);
  }

  async openAccountPortal(): Promise<void> {
    await shell.openExternal(`${this.origins.auth}/account`);
  }

  logout() {
    return this.requireClient().logout();
  }

  prewarmSecurity(modelId: string) {
    return this.requireClient().prewarmSecurity(modelId);
  }

  verifySecurity(sessionId: string, acceptOutdatedTee = false, modelId?: string) {
    return this.requireClient().verifySecurity(sessionId, acceptOutdatedTee, modelId);
  }

  compact(sessionId: string, focus?: string) {
    return this.requireClient().compact(sessionId, focus);
  }

  async deletePreview(sessionIds: string[]) {
    try {
      return await this.requireClient().deletePreview(sessionIds, { cancelActiveWork: true });
    } catch (error) {
      throw actionableAcpError(error);
    }
  }

  async deleteConfirm(confirmationToken: string, sessionIds: string[]) {
    try {
      return await this.requireClient().deleteConfirm(confirmationToken, sessionIds);
    } catch (error) {
      throw actionableAcpError(error);
    }
  }

  listCollections() {
    return this.requireClient().listCollections();
  }

  createCollection(name: string) {
    return this.requireClient().createCollection(name);
  }

  async renameCollection(collectionId: string, name: string) {
    try {
      return await this.requireClient().renameCollection(collectionId, name);
    } catch (error) {
      throw actionableAcpError(error);
    }
  }

  setCollectionCollapsed(collectionId: string, collapsed: boolean) {
    return this.requireClient().setCollectionCollapsed(collectionId, collapsed);
  }

  moveCollection(collectionId: string, position: number) {
    return this.requireClient().moveCollection(collectionId, position);
  }

  deleteCollection(collectionId: string) {
    return this.requireClient().deleteCollection(collectionId);
  }

  assignThreadCollection(threadId: string, collectionId: string | null) {
    return this.requireClient().assignThreadCollection(threadId, collectionId);
  }

  resolvePermission(interactionId: string, outcome: PermissionOutcome): void {
    this.requireClient().resolvePermission(interactionId, outcome);
  }

  resolveElicitation(interactionId: string, outcome: ElicitationOutcome): void {
    this.requireClient().resolveElicitation(interactionId, outcome);
  }

  private requireClient(): AxiomAcpClient {
    if (!this.client || !this.bootstrap) throw new Error("AxiomCLI sidecar is not ready");
    return this.client;
  }
}
