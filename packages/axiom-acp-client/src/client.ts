import { EventEmitter } from "node:events";
import type {
  AccountStatusResponse,
  AssignThreadCollectionResponse,
  BrowserLoginStartResponse,
  CollectionStateResponse,
  CompactResponse,
  DeleteConfirmResponse,
  DeletePreviewResponse,
  DesktopBootstrapResponse,
  EventNotification,
  GetThreadTimelineResponse,
  ListModelsResponse,
  ListThreadsRequest,
  ListThreadsResponse,
  ProfilePreferencesResponse,
  SetProfilePreferencesRequest,
  VerifySecurityResponse,
} from "./generated/protocol.js";
import { ProtocolError } from "./errors.js";
import { AcpProcess, type IncomingRequest, type SidecarProcessOptions } from "./process.js";
import { AxiomStateStore, type ClientState } from "./state.js";
import type {
  ElicitationOutcome,
  InitializeResult,
  PendingInteraction,
  PermissionOutcome,
  PromptResult,
  RpcNotification,
  SessionConfigOption,
  SessionStartResult,
} from "./types.js";

const EXTENSION_CAPABILITIES = {
  protocolVersion: "0.1",
  features: {
    threadCatalog: 1,
    timeline: 1,
    modelCatalog: 1,
    profilePreferences: 1,
    collections: 1,
    desktopChat: 1,
    account: 1,
    securityEvidence: 1,
    compaction: 1,
    activity: 1,
  },
} as const;

type ExtensionFeature = keyof typeof EXTENSION_CAPABILITIES.features;

// Adding a new optional extension feature must not make a newer Desktop
// unable to talk to an otherwise usable older sidecar. Keep this list limited
// to features required for the core desktop and its security indicator.
const REQUIRED_DESKTOP_FEATURES: readonly ExtensionFeature[] = [
  "desktopChat",
  "threadCatalog",
  "timeline",
  "modelCatalog",
  "profilePreferences",
  "collections",
  "account",
  "securityEvidence",
];

function compatibleExtensionFamily(version: string | undefined): boolean {
  return version?.split(".", 1)[0] === EXTENSION_CAPABILITIES.protocolVersion.split(".", 1)[0];
}

export function validateDesktopExtension(meta: {
  protocolVersion?: string;
  features?: Record<string, number>;
} | undefined): void {
  if (!compatibleExtensionFamily(meta?.protocolVersion)) {
    throw new ProtocolError("AxiomCLI does not support the required Axiom desktop extension");
  }
  for (const feature of REQUIRED_DESKTOP_FEATURES) {
    const requiredVersion = EXTENSION_CAPABILITIES.features[feature];
    if ((meta?.features?.[feature] ?? 0) < requiredVersion) {
      throw new ProtocolError(`AxiomCLI does not support required desktop feature ${feature}@${requiredVersion}`);
    }
  }
}

const TIMELINE_PAGE_SIZE = 250;
const TIMELINE_RESTART_LIMIT = 3;

export interface ThreadSettingsRequest {
  threadId: string;
  model?: string;
  thinkingLevel?: string;
}

export interface ThreadSettingsResult extends ProfilePreferencesResponse {
  threadId: string;
  settings: {
    model: string;
    thinkingLevel: string;
    permissionProfile: string;
  };
  configOptions: SessionConfigOption[];
}

type InteractionResolver = {
  interaction: PendingInteraction;
  respond: (result: unknown) => void;
};

export class AxiomAcpClient extends EventEmitter {
  readonly process: AcpProcess;
  readonly state = new AxiomStateStore();
  private readonly interactions = new Map<string, InteractionResolver>();
  private readonly background = new Set<Promise<void>>();
  private readonly refreshingThreads = new Map<string, Promise<GetThreadTimelineResponse>>();
  private fullResync: Promise<void> | null = null;
  private fullResyncRequired = false;
  private initialized = false;
  private desktopBootstrap: DesktopBootstrapResponse | null = null;
  private closing = false;

  constructor(options: SidecarProcessOptions) {
    super();
    this.process = new AcpProcess(options);
    this.process.on("notification", (notification: RpcNotification) => this.onNotification(notification));
    this.process.on("request", (request: IncomingRequest) => this.onRequest(request));
    this.process.on("diagnostic", (diagnostic: string) => this.state.setDiagnostic(diagnostic));
    this.process.on("exit", (error: Error) => this.state.setConnected(false, error.message));
    this.state.on("change", (state: ClientState) => this.emit("state", state));
  }

  async initialize(): Promise<InitializeResult> {
    const result = await this.process.request<InitializeResult>("initialize", {
      protocolVersion: 1,
      clientCapabilities: {
        elicitation: { form: {} },
        _meta: { axiom: EXTENSION_CAPABILITIES },
      },
      clientInfo: { name: "axiom-desktop", version: "0.1.0", title: "Axiom Desktop" },
    });
    if (result.protocolVersion !== 1) throw new ProtocolError("AxiomCLI selected an unsupported ACP version");
    const meta = result.agentCapabilities._meta?.axiom as {
      protocolVersion?: string;
      features?: Record<string, number>;
    } | undefined;
    validateDesktopExtension(meta);
    this.initialized = true;
    this.state.setConnected(true);
    return result;
  }

  getState(): ClientState {
    return this.state.snapshot();
  }

  async bootstrapDesktop(): Promise<DesktopBootstrapResponse> {
    this.assertInitialized();
    const result = await this.process.request<DesktopBootstrapResponse>("_axiom/desktop/bootstrap", {});
    if (
      result.frontend !== "desktop-chat"
      || result.permissionProfile !== "web"
      || !result.chatCwd
      || !result.newThreadSettings
      || !["minimal", "low", "medium", "high", "xhigh"].includes(result.newThreadSettings.thinkingLevel)
    ) {
      throw new ProtocolError("AxiomCLI returned an invalid desktop chat bootstrap contract");
    }
    this.desktopBootstrap = result;
    this.state.setPreferences(result.newThreadSettings);
    return result;
  }

  async initializeDesktopState(): Promise<void> {
    await Promise.all([this.listThreads(), this.listCollections(), this.getProfilePreferences()]);
  }

  async newChat(): Promise<SessionStartResult> {
    return this.newSession(this.requireDesktopBootstrap().chatCwd);
  }

  async loadChat(threadId: string): Promise<SessionStartResult> {
    return this.loadSession(threadId, this.requireDesktopBootstrap().chatCwd);
  }

  async newSession(cwd: string): Promise<SessionStartResult> {
    this.assertInitialized();
    const result = await this.process.request<SessionStartResult>("session/new", { cwd, mcpServers: [] });
    this.state.addSession(result.sessionId, cwd);
    if (result.modes) this.state.setModes(result.sessionId, result.modes.currentModeId, result.modes.availableModes);
    if (result.configOptions) this.state.setConfig(result.sessionId, result.configOptions);
    await this.refreshThread(result.sessionId);
    this.runBackground(this.verifySecurity(result.sessionId));
    return result;
  }

  async loadSession(threadId: string, cwd: string): Promise<SessionStartResult> {
    this.assertInitialized();
    const result = await this.process.request<SessionStartResult>("session/load", { sessionId: threadId, cwd, mcpServers: [] });
    this.state.addSession(threadId, cwd);
    if (result.modes) this.state.setModes(threadId, result.modes.currentModeId, result.modes.availableModes);
    if (result.configOptions) this.state.setConfig(threadId, result.configOptions);
    await this.refreshThread(threadId);
    this.runBackground(this.verifySecurity(threadId));
    return { ...result, sessionId: threadId };
  }

  async prompt(threadId: string, text: string, clientItemId: string): Promise<PromptResult> {
    if (!text.trim()) throw new ProtocolError("prompt cannot be empty");
    if (!clientItemId || clientItemId.length > 512) throw new ProtocolError("invalid client item ID");
    this.state.setRunning(threadId, true);
    try {
      return await this.process.request<PromptResult>(
        "session/prompt",
        {
          sessionId: threadId,
          prompt: [{ type: "text", text }],
          _meta: { axiom: { clientItemId } },
        },
        0,
      );
    } finally {
      this.state.setRunning(threadId, false);
      await this.refreshThread(threadId).catch(() => undefined);
    }
  }

  async cancel(threadId: string): Promise<void> {
    try {
      await this.process.notify("session/cancel", { sessionId: threadId });
    } finally {
      for (const [interactionId, pending] of this.interactions) {
        if (pending.interaction.sessionId !== threadId) continue;
        this.resolveInteraction(
          interactionId,
          pending.interaction.kind === "permission" ? { outcome: { outcome: "cancelled" } } : { action: "cancel" },
        );
      }
    }
  }

  async setMode(threadId: string, modeId: string): Promise<void> {
    await this.process.request("session/set_mode", { sessionId: threadId, modeId });
  }

  async setConfig(threadId: string, configId: string, value: string): Promise<SessionConfigOption[]> {
    const result = await this.process.request<{ configOptions: SessionConfigOption[] }>("session/set_config_option", {
      sessionId: threadId,
      configId,
      value,
    });
    this.state.setConfig(threadId, result.configOptions);
    return result.configOptions;
  }

  async setSettings(request: ThreadSettingsRequest): Promise<ThreadSettingsResult> {
    if (!request.model && !request.thinkingLevel) throw new ProtocolError("no settings were provided");
    let configOptions = request.model
      ? await this.setConfig(request.threadId, "model", request.model)
      : null;
    if (request.thinkingLevel) {
      const thinking = configOptions?.find((option) => option.id === "thinking");
      const requestedIsSupported = thinking?.options?.some(
        (option) => option.value === request.thinkingLevel,
      ) ?? false;
      if (!request.model || requestedIsSupported) {
        if (thinking?.currentValue !== request.thinkingLevel) {
          configOptions = await this.setConfig(request.threadId, "thinking", request.thinkingLevel);
        }
      }
    }
    await this.refreshThread(request.threadId);
    const preferences = await this.getProfilePreferences();
    const session = this.state.snapshot().sessions[request.threadId];
    if (!session?.settings?.model) {
      throw new ProtocolError("AxiomCLI returned no authoritative active-thread settings");
    }
    return {
      ...preferences,
      threadId: request.threadId,
      settings: session.settings,
      configOptions: configOptions ?? session.configOptions,
    };
  }

  async listThreads(request: ListThreadsRequest = {}): Promise<ListThreadsResponse> {
    const threads = [] as ListThreadsResponse["threads"];
    let cursor = request.cursor ?? undefined;
    for (let pageIndex = 0; pageIndex < 10_000; pageIndex += 1) {
      const page = await this.process.request<ListThreadsResponse>("_axiom/thread/list", {
        ...request,
        cursor,
        limit: request.limit ?? 200,
      });
      threads.push(...page.threads);
      cursor = page.nextCursor ?? undefined;
      if (cursor === undefined) {
        const result = { threads, nextCursor: undefined };
        this.state.setCatalog(threads);
        return result;
      }
    }
    throw new ProtocolError("thread catalog pagination exceeded its safety limit");
  }

  refreshThread(threadId: string): Promise<GetThreadTimelineResponse> {
    const existing = this.refreshingThreads.get(threadId);
    if (existing) return existing;
    const refresh = this.fetchTimeline(threadId).finally(() => this.refreshingThreads.delete(threadId));
    this.refreshingThreads.set(threadId, refresh);
    return refresh;
  }

  listModels(): Promise<ListModelsResponse> {
    return this.process.request("_axiom/models/list", {});
  }

  async getProfilePreferences(): Promise<ProfilePreferencesResponse> {
    const result = await this.process.request<ProfilePreferencesResponse>("_axiom/profile/preferences", {});
    this.state.setPreferences(result.preferences);
    return result;
  }

  async setProfilePreferences(request: SetProfilePreferencesRequest): Promise<ProfilePreferencesResponse> {
    const result = await this.process.request<ProfilePreferencesResponse>("_axiom/profile/preferences/set", request);
    this.state.setPreferences(result.preferences);
    return result;
  }

  async listCollections(): Promise<CollectionStateResponse> {
    const result = await this.process.request<CollectionStateResponse>("_axiom/collection/list", {});
    this.state.setCollections(result.state);
    return result;
  }

  createCollection(name: string): Promise<CollectionStateResponse> {
    return this.collectionRequest("_axiom/collection/create", { name });
  }

  renameCollection(collectionId: string, name: string): Promise<CollectionStateResponse> {
    return this.collectionRequest("_axiom/collection/rename", { collectionId, name });
  }

  setCollectionCollapsed(collectionId: string, collapsed: boolean): Promise<CollectionStateResponse> {
    return this.collectionRequest("_axiom/collection/set_collapsed", { collectionId, collapsed });
  }

  moveCollection(collectionId: string, position: number): Promise<CollectionStateResponse> {
    return this.collectionRequest("_axiom/collection/move", { collectionId, position });
  }

  deleteCollection(collectionId: string): Promise<CollectionStateResponse> {
    return this.collectionRequest("_axiom/collection/delete", { collectionId });
  }

  async assignThreadCollection(threadId: string, collectionId: string | null): Promise<AssignThreadCollectionResponse> {
    const result = await this.process.request<AssignThreadCollectionResponse>("_axiom/collection/assign", {
      threadId,
      collectionId,
    });
    this.state.setCollections(result.state);
    await this.refreshThread(threadId);
    return result;
  }

  accountStatus(): Promise<AccountStatusResponse> {
    return this.accountRequest("_axiom/account/status", {});
  }

  browserLoginStart(): Promise<BrowserLoginStartResponse> {
    return this.process.request("_axiom/account/browser_login_start", {});
  }

  browserLoginComplete(loginId: string, signal?: AbortSignal): Promise<AccountStatusResponse> {
    return this.accountRequest("_axiom/account/browser_login_complete", { loginId }, 0, signal);
  }

  useApiKey(apiKey: string): Promise<AccountStatusResponse> {
    if (!apiKey || apiKey.length > 4096) throw new ProtocolError("invalid Axiom API key length");
    return this.accountRequest("_axiom/account/use_api_key", { apiKey });
  }

  logout(): Promise<AccountStatusResponse> {
    return this.accountRequest("_axiom/account/logout", {});
  }

  async verifySecurity(threadId: string): Promise<VerifySecurityResponse> {
    const result = await this.process.request<VerifySecurityResponse>("_axiom/security/verify", { threadId }, 0);
    if (
      result.status.state === "verified"
      && (
        !result.evidence
        || result.evidence.attestationGeneration <= 0
        || result.evidence.hardExpiresAtUnixSeconds <= 0
      )
    ) {
      throw new ProtocolError("AxiomCLI returned verified security without complete lease-bound evidence");
    }
    return result;
  }

  compact(threadId: string, focus?: string): Promise<CompactResponse> {
    return this.process.request("_axiom/compaction/start", { threadId, focus }, 0);
  }

  deletePreview(threadIds: string[]): Promise<DeletePreviewResponse> {
    return this.process.request("_axiom/thread/delete_preview", { threadIds });
  }

  async deleteConfirm(confirmationToken: string, threadIds: string[]): Promise<DeleteConfirmResponse> {
    const result = await this.process.request<DeleteConfirmResponse>("_axiom/thread/delete_confirm", {
      confirmationToken,
      threadIds,
    });
    for (const threadId of threadIds) this.state.removeSession(threadId);
    await Promise.all([this.listThreads(), this.listCollections()]);
    return result;
  }

  resolvePermission(interactionId: string, outcome: PermissionOutcome): void {
    const pending = this.interactions.get(interactionId);
    if (!pending || pending.interaction.kind !== "permission") {
      throw new ProtocolError("permission request is stale or has the wrong kind");
    }
    if (outcome.outcome === "selected") {
      const payload = pending.interaction.payload as { options?: Array<{ optionId?: string }> };
      if (!payload.options?.some((option) => option.optionId === outcome.optionId)) {
        throw new ProtocolError("permission option was not offered by AxiomCLI");
      }
    }
    this.resolveInteraction(interactionId, { outcome });
  }

  resolveElicitation(interactionId: string, outcome: ElicitationOutcome): void {
    const pending = this.interactions.get(interactionId);
    if (!pending || pending.interaction.kind !== "elicitation") {
      throw new ProtocolError("elicitation request is stale or has the wrong kind");
    }
    this.resolveInteraction(interactionId, outcome);
  }

  async close(): Promise<void> {
    this.closing = true;
    for (const pending of this.interactions.values()) {
      pending.respond(pending.interaction.kind === "permission" ? { outcome: { outcome: "cancelled" } } : { action: "cancel" });
    }
    this.interactions.clear();
    try {
      await this.process.close();
    } catch (error) {
      throw new ProtocolError(`could not close AxiomCLI: ${error instanceof Error ? error.message : String(error)}`);
    }
    this.background.clear();
  }

  private async fetchTimeline(threadId: string): Promise<GetThreadTimelineResponse> {
    for (let attempt = 0; attempt < TIMELINE_RESTART_LIMIT; attempt += 1) {
      const pages: GetThreadTimelineResponse[] = [];
      let cursor: number | undefined;
      let restart = false;
      do {
        const page = await this.process.request<GetThreadTimelineResponse>("_axiom/thread/timeline", {
          threadId,
          afterSequence: cursor,
          limit: TIMELINE_PAGE_SIZE,
        });
        const first = pages[0];
        if (first && (
          page.thread.threadId !== first.thread.threadId
          || page.thread.revision !== first.thread.revision
          || page.thread.lastTimelineSequence !== first.thread.lastTimelineSequence
        )) {
          restart = true;
          break;
        }
        pages.push(page);
        cursor = page.nextCursor ?? undefined;
      } while (cursor !== undefined);
      if (restart) continue;
      if (!this.state.replaceTimeline(pages)) continue;
      return pages.at(-1) ?? (() => { throw new ProtocolError("AxiomCLI returned no timeline page"); })();
    }
    throw new ProtocolError("thread changed repeatedly while its timeline was being synchronized");
  }

  private onNotification(notification: RpcNotification): void {
    if (this.closing) return;
    if (notification.method === "session/update") {
      const threadId = this.state.standardUpdate(notification.params);
      if (threadId) this.runBackground(this.refreshThread(threadId));
    }
    if (notification.method === "_axiom/event") {
      const result = this.state.extensionEvent(notification.params as EventNotification);
      if (result?.fullResync) this.fullResyncRequired = true;
      if (this.fullResyncRequired) this.runBackground(this.resyncAll());
      else if (result?.threadId) this.runBackground(this.refreshThread(result.threadId));
    }
  }

  private onRequest(incoming: IncomingRequest): void {
    const { request } = incoming;
    const params = request.params as Record<string, unknown> | undefined;
    const mode = params?.mode && typeof params.mode === "object" ? params.mode as Record<string, unknown> : undefined;
    const sessionId = typeof params?.sessionId === "string"
      ? params.sessionId
      : typeof mode?.sessionId === "string"
        ? mode.sessionId
        : null;
    let kind: PendingInteraction["kind"];
    if (request.method === "session/request_permission") kind = "permission";
    else if (request.method === "elicitation/create") kind = "elicitation";
    else {
      incoming.reject(-32601, "unsupported client method");
      return;
    }
    const interaction: PendingInteraction = {
      id: crypto.randomUUID(),
      requestId: request.id,
      sessionId,
      kind,
      payload: request.params,
    };
    this.interactions.set(interaction.id, { interaction, respond: incoming.respond });
    this.state.addInteraction(interaction);
    this.emit("interaction", interaction);
  }

  private resolveInteraction(interactionId: string, result: unknown): void {
    const pending = this.interactions.get(interactionId);
    if (!pending) throw new ProtocolError("interaction is stale or already answered");
    this.interactions.delete(interactionId);
    this.state.removeInteraction(interactionId);
    pending.respond(result);
  }

  private assertInitialized(): void {
    if (!this.initialized) throw new ProtocolError("ACP client is not initialized");
  }

  private requireDesktopBootstrap(): DesktopBootstrapResponse {
    this.assertInitialized();
    if (!this.desktopBootstrap) throw new ProtocolError("desktop bootstrap has not completed");
    return this.desktopBootstrap;
  }

  private runBackground(task: Promise<unknown>): void {
    let tracked: Promise<void>;
    tracked = task.then(() => undefined, () => undefined).finally(() => this.background.delete(tracked));
    this.background.add(tracked);
  }

  private resyncAll(): Promise<void> {
    if (this.fullResync) return this.fullResync;
    this.fullResync = (async () => {
      await Promise.all([this.listThreads(), this.listCollections(), this.getProfilePreferences()]);
      const staleThreads = Object.values(this.state.snapshot().sessions)
        .filter((session) => session.needsResync)
        .map((session) => session.sessionId);
      await Promise.all(staleThreads.map((threadId) => this.refreshThread(threadId)));
      this.fullResyncRequired = false;
    })().finally(() => {
      this.fullResync = null;
    });
    return this.fullResync;
  }

  private async collectionRequest(method: string, params: unknown): Promise<CollectionStateResponse> {
    const result = await this.process.request<CollectionStateResponse>(method, params);
    this.state.setCollections(result.state);
    return result;
  }

  private async accountRequest(
    method: string,
    params: unknown,
    timeoutMs = 120_000,
    signal?: AbortSignal,
  ): Promise<AccountStatusResponse> {
    const result = await this.process.request<AccountStatusResponse>(method, params, timeoutMs, signal);
    this.state.setAccount(result.status);
    return result;
  }
}
