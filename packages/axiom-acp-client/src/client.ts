import { EventEmitter } from "node:events";
import type {
  AccountStatusResponse,
  PromptAttachment,
  GetAttachmentsResponse,
  AssignThreadCollectionResponse,
  BillingStatusResponse,
  GiftCodeRedeemResponse,
  UsageSummaryResponse,
  UsageSummaryRequest,
  ApiKeyListResponse,
  ApiKeyCreatedResponse,
  ApiKeyRevokeResponse,
  CollectionStateResponse,
  CompactResponse,
  DeleteConfirmResponse,
  DeletePreviewResponse,
  DesktopBootstrapResponse,
  ConfigureDesktopAgentRequest,
  ConfigureDesktopAgentResponse,
  EventNotification,
  GetThreadTimelineResponse,
  ListModelsResponse,
  ListThreadsRequest,
  ListThreadsResponse,
  LoginMethod,
  NativeLoginStartResponse,
  ProfilePreferencesResponse,
  RenameThreadResponse,
  SetProfilePreferencesRequest,
  SteerTurnRequest,
  SteerTurnResponse,
  VerifySecurityResponse,
} from "./generated/protocol.js";
import { validatePrompt } from "./attachments.js";
import { AcpError, ProtocolError } from "./errors.js";
import { AcpProcess, type IncomingRequest, type SidecarProcessOptions } from "./process.js";
import { AxiomStateStore, type ClientState } from "./state.js";
import { publicSecurityEvidence } from "./security-evidence.js";
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
  protocolVersion: "0.2",
  features: {
    threadCatalog: 1,
    timeline: 2,
    modelCatalog: 1,
    profilePreferences: 1,
    collections: 1,
    desktopChat: 1,
    desktopAgent: 1,
    account: 2,
    billing: 3,
    usage: 1,
    securityEvidence: 4,
    webConsent: 1,
    steering: 1,
    messageRevision: 1,
    attachments: 2,
    compaction: 1,
    activity: 1,
  },
} as const;

export const AXIOM_ACP_CLIENT_VERSION = "0.2.0";

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
  "billing",
  "securityEvidence",
  // Older sidecars exposed web tools unconditionally and cannot honor off.
  "webConsent",
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
const DELETE_DRAIN_TIMEOUT_MS = 10_000;

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
  private readonly securityWarmups = new Map<string, {
    context: number; promise: Promise<VerifySecurityResponse>; pending: boolean; validUntil: number; retryAt: number; failures: number;
  }>();
  private readonly securityVerifications = new Map<string, { context: object; promise: Promise<VerifySecurityResponse> }>();
  private readonly pendingPrompts = new Set<{ threadId: string; cancelled: boolean }>();
  private fullResync: Promise<void> | null = null;
  private fullResyncRequired = false;
  private initialized = false;
  private supportsMessageRevision = false;
  private supportsAttachments = false;
  private desktopBootstrap: DesktopBootstrapResponse | null = null;
  private closing = false;
  private accountingTimer: ReturnType<typeof setTimeout> | null = null;
  private readonly accountingRecoveryAttempts = new Map<string, number>();

  constructor(options: SidecarProcessOptions) {
    super();
    this.process = new AcpProcess(options);
    this.process.on("notification", (notification: RpcNotification) => this.onNotification(notification));
    this.process.on("request", (request: IncomingRequest) => this.onRequest(request));
    this.process.on("diagnostic", (diagnostic: string) => this.state.setDiagnostic(diagnostic));
    this.process.on("exit", (error: Error) => this.state.setConnected(false, error.message));
    this.state.on("change", (state: ClientState) => { if (!state.connected) this.securityWarmups.clear(); this.emit("state", state); this.scheduleAccountingRecovery(state); });
  }

  private scheduleAccountingRecovery(state: ClientState): void {
    if (this.closing || this.accountingTimer || !state.connected) return;
    const context = this.state.accountContextToken();
    const requestKey = (id: string) => context + ":" + id;
    const unresolved = new Set(Object.values(state.sessions).flatMap((session) =>
      session.requestUsage?.filter((request) => !request.settled).map((request) => requestKey(request.requestId)) ?? []));
    for (const key of this.accountingRecoveryAttempts.keys()) {
      if (!unresolved.has(key)) this.accountingRecoveryAttempts.delete(key);
    }
    const pending = Object.values(state.sessions).map((session) => ({
      threadId: session.sessionId,
      requests: session.requestUsage?.filter((request) => !request.settled && request.state !== "running"
        && (this.accountingRecoveryAttempts.get(requestKey(request.requestId)) ?? 0) < 80) ?? [],
    })).filter((session) => session.requests.length);
    if (!pending.length) return;
    this.accountingTimer = setTimeout(() => {
      this.accountingTimer = null;
      if (this.closing) return;
      if (this.state.accountContextToken() !== context) { this.scheduleAccountingRecovery(this.state.snapshot()); return; }
      for (const session of pending) {
        for (const request of session.requests) {
          const key = requestKey(request.requestId);
          this.accountingRecoveryAttempts.set(key, (this.accountingRecoveryAttempts.get(key) ?? 0) + 1);
        }
        // Metadata-only reconciliation. Never retry the inference POST, steal
        // focus, show a modal or promote a lost receipt to verified.
        this.runBackground(this.refreshThread(session.threadId).catch(() => undefined)
          .finally(() => this.scheduleAccountingRecovery(this.state.snapshot())));
      }
    }, 15_000);
    this.accountingTimer.unref();
  }

  async initialize(expectedProductVersion?: string): Promise<InitializeResult> {
    const result = await this.process.request<InitializeResult>("initialize", {
      protocolVersion: 1,
      clientCapabilities: {
        elicitation: { form: {} },
        _meta: { axiom: EXTENSION_CAPABILITIES },
      },
      clientInfo: {
        name: "axiom-desktop",
        version: AXIOM_ACP_CLIENT_VERSION,
        title: "Axiom Desktop",
      },
    });
    if (expectedProductVersion !== undefined && result.agentInfo?.version !== expectedProductVersion) {
      throw new ProtocolError(`Axiom Desktop ${expectedProductVersion} requires AxiomCLI ${expectedProductVersion}; received ${result.agentInfo?.version ?? 'no version'}. Quit Axiom and all AxiomCLI sessions, then reinstall the matching release (or rebuild the development sidecar).`);
    }
    if (result.protocolVersion !== 1) throw new ProtocolError("AxiomCLI selected an unsupported ACP version");
    const meta = result.agentCapabilities._meta?.axiom as {
      protocolVersion?: string;
      features?: Record<string, number>;
    } | undefined;
    validateDesktopExtension(meta);
    this.supportsMessageRevision = (meta?.features?.messageRevision ?? 0) >= 1;
    this.supportsAttachments = (meta?.features?.attachments ?? 0) >= 2;
    this.initialized = true;
    this.state.setConnected(true);
    return result;
  }

  getState(): ClientState {
    return this.state.snapshot();
  }

  async bootstrapDesktop(): Promise<DesktopBootstrapResponse> {
    this.assertInitialized();
    const accountContext = this.state.accountContextToken();
    const result = await this.process.request<DesktopBootstrapResponse>("_axiom/desktop/bootstrap", {});
    this.assertAccountContext(accountContext);
    if (
      result.frontend !== "desktop-chat"
      || result.permissionProfile !== "web"
      || (result.chatCwd != null && (typeof result.chatCwd !== "string" || result.chatCwd.length === 0))
      || !result.newThreadSettings
      || !["provider_default", "enabled", "disabled", "minimal", "low", "medium", "high", "xhigh"].includes(result.newThreadSettings.thinkingLevel)
    ) {
      throw new ProtocolError("AxiomCLI returned an invalid desktop chat bootstrap contract");
    }
    this.desktopBootstrap = result;
    this.state.setPreferences(result.newThreadSettings);
    return result;
  }

  async initializeDesktopState(): Promise<void> {
    if (!this.desktopBootstrap?.chatCwd) return;
    const initialization: Promise<unknown>[] = [
      this.listThreads(),
      this.listCollections(),
      this.getProfilePreferences(),
    ];
    if (this.state.snapshot().account?.state === "valid") {
      // Billing is remote, optional presentation state. An outage must not
      // turn a successfully stored native session into a failed sign-in or
      // prevent account-scoped local history from opening. Explicit billing
      // requests still reject and the balance remains unknown until loaded.
      this.runBackground(this.billingStatus());
    }
    await Promise.all(initialization);
  }

  async newChat(): Promise<SessionStartResult> {
    return this.newSession(this.requireDesktopChatCwd());
  }

  async loadChat(threadId: string): Promise<SessionStartResult> {
    this.requireDesktopChatCwd();
    const accountContext = this.state.accountContextToken();
    // Read the persisted workspace without presenting an unloaded thread as
    // ready. The native load handler validates this exact directory identity.
    const page = await this.process.request<GetThreadTimelineResponse>("_axiom/thread/timeline", { threadId, limit: 1 });
    this.assertAccountContext(accountContext);
    return this.loadSession(threadId, page.thread.cwd);
  }

  async configureDesktopAgent(request: ConfigureDesktopAgentRequest): Promise<ConfigureDesktopAgentResponse> {
    const accountContext = this.state.accountContextToken();
    await this.waitForSecurityPreflight(request.threadId);
    this.assertAccountContext(accountContext);
    const result = await this.process.request<ConfigureDesktopAgentResponse>("_axiom/desktop/agent/configure", request);
    this.assertAccountContext(accountContext);
    this.state.setDesktopAgent(result);
    await this.refreshThread(request.threadId);
    return result;
  }

  async newSession(cwd: string): Promise<SessionStartResult> {
    this.assertInitialized();
    const accountContext = this.state.accountContextToken();
    const result = await this.process.request<SessionStartResult>("session/new", { cwd, mcpServers: [] });
    this.assertAccountContext(accountContext);
    this.state.addSession(result.sessionId, cwd);
    if (result.modes) this.state.setModes(result.sessionId, result.modes.currentModeId, result.modes.availableModes);
    if (result.configOptions) this.state.setConfig(result.sessionId, result.configOptions);
    await this.refreshThread(result.sessionId);
    return result;
  }

  async loadSession(threadId: string, cwd: string): Promise<SessionStartResult> {
    this.assertInitialized();
    const accountContext = this.state.accountContextToken();
    const result = await this.process.request<SessionStartResult>("session/load", { sessionId: threadId, cwd, mcpServers: [] });
    this.assertAccountContext(accountContext);
    this.state.addSession(threadId, cwd);
    if (result.modes) this.state.setModes(threadId, result.modes.currentModeId, result.modes.availableModes);
    if (result.configOptions) this.state.setConfig(threadId, result.configOptions);
    await this.refreshThread(threadId);
    return { ...result, sessionId: threadId };
  }

  async prompt(threadId: string, text: string, clientItemId: string, webEnabled = false, agentRevision = this.state.snapshot().sessions[threadId]?.desktopAgent?.revision ?? 0, revision?: { userItemId: string; expectedRevision: number }, attachments: PromptAttachment[] = []): Promise<PromptResult> {
    if (revision && (!this.supportsMessageRevision || !revision.userItemId || revision.userItemId.length > 512
      || !Number.isSafeInteger(revision.expectedRevision) || revision.expectedRevision < 0)) {
      throw new ProtocolError("Restart Axiom before editing or regenerating messages.");
    }
    if (revision && this.state.snapshot().sessions[threadId]?.running) throw new ProtocolError("Stop the current reply first.");
    if (typeof webEnabled !== "boolean") throw new ProtocolError("webEnabled must be a boolean");
    if (!Number.isSafeInteger(agentRevision) || agentRevision < 0) throw new ProtocolError("invalid Agent settings revision");
    validatePrompt(text, attachments, Boolean(revision));
    if (attachments.length && !this.supportsAttachments) throw new ProtocolError("Restart Axiom to enable attachments. No message was sent.");
    if (!clientItemId || clientItemId.length > 512) throw new ProtocolError("invalid client item ID");
    const accountContext = this.state.accountContextToken();
    const pending = { threadId, cancelled: false };
    this.pendingPrompts.add(pending);
    try {
      await this.waitForSecurityPreflight(threadId);
    } finally {
      this.pendingPrompts.delete(pending);
    }
    this.assertAccountContext(accountContext);
    if (pending.cancelled) return { stopReason: "cancelled" };
    this.state.setRunning(threadId, true);
    try {
      const result = await this.process.request<PromptResult>(
        "session/prompt",
        {
          sessionId: threadId,
          prompt: [{ type: "text", text }],
          _meta: { axiom: { clientItemId, webEnabled, agentRevision, ...(revision ? { revision } : {}), ...(attachments.length ? { attachments } : {}) } },
        },
        0,
      );
      this.assertAccountContext(accountContext);
      return result;
    } catch (error) {
      if (revision && error instanceof AcpError && typeof error.data === "string") throw new ProtocolError(error.data);
      throw error;
    } finally {
      if (this.state.accountContextToken() === accountContext) {
        this.state.setRunning(threadId, false);
        await this.refreshThread(threadId).catch(() => undefined);
      }
    }
  }

  async getAttachments(threadId: string, userItemId: string): Promise<GetAttachmentsResponse> {
    if (!this.supportsAttachments) throw new ProtocolError("Restart Axiom to view attachments.");
    const context = this.state.accountContextToken();
    const result = await this.process.request<GetAttachmentsResponse>("_axiom/thread/attachments", { threadId, userItemId });
    this.assertAccountContext(context);
    return result;
  }

  async cancel(threadId: string): Promise<void> {
    // A prompt waiting for attestation must not start after Stop was sent.
    for (const pending of this.pendingPrompts) if (pending.threadId === threadId) pending.cancelled = true;
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

  async steer(request: SteerTurnRequest): Promise<SteerTurnResponse> {
    if (!request.text.trim() || new TextEncoder().encode(request.text).length > 64 * 1024
      || !request.clientItemId || request.clientItemId.length > 128 || !request.expectedTurnId
      || typeof request.webEnabled !== "boolean") throw new ProtocolError("invalid steering input");
    const context = this.state.accountContextToken();
    const result = await this.process.request<SteerTurnResponse>("_axiom/turn/steer", request, 0);
    this.assertAccountContext(context);
    return result;
  }

  async setMode(threadId: string, modeId: string): Promise<void> {
    await this.waitForSecurityPreflight(threadId);
    await this.process.request("session/set_mode", { sessionId: threadId, modeId });
  }

  async setConfig(threadId: string, configId: string, value: string): Promise<SessionConfigOption[]> {
    await this.waitForSecurityPreflight(threadId);
    const accountContext = this.state.accountContextToken();
    const result = await this.process.request<{ configOptions: SessionConfigOption[] }>("session/set_config_option", {
      sessionId: threadId,
      configId,
      value,
    });
    this.assertAccountContext(accountContext);
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
    const accountContext = this.state.accountContextToken();
    const threads = [] as ListThreadsResponse["threads"];
    let cursor = request.cursor ?? undefined;
    for (let pageIndex = 0; pageIndex < 10_000; pageIndex += 1) {
      const page = await this.process.request<ListThreadsResponse>("_axiom/thread/list", {
        ...request,
        cursor,
        limit: request.limit ?? 200,
      });
      this.assertAccountContext(accountContext);
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

  async renameThread(threadId: string, title: string): Promise<RenameThreadResponse> {
    const accountContext = this.state.accountContextToken();
    const result = await this.process.request<RenameThreadResponse>("_axiom/thread/rename", { threadId, title });
    this.assertAccountContext(accountContext);
    await this.listThreads();
    this.assertAccountContext(accountContext);
    this.state.applyThreadRename(result.thread);
    return result;
  }

  refreshThread(threadId: string): Promise<GetThreadTimelineResponse> {
    const accountContext = this.state.accountContextToken();
    const refreshKey = `${accountContext}:${threadId}`;
    const existing = this.refreshingThreads.get(refreshKey);
    if (existing) return existing;
    const refresh = this.fetchTimeline(threadId, accountContext)
      .finally(() => this.refreshingThreads.delete(refreshKey));
    this.refreshingThreads.set(refreshKey, refresh);
    return refresh;
  }

  listModels(): Promise<ListModelsResponse> {
    return this.process.request("_axiom/models/list", {});
  }

  async getProfilePreferences(): Promise<ProfilePreferencesResponse> {
    const accountContext = this.state.accountContextToken();
    const result = await this.process.request<ProfilePreferencesResponse>("_axiom/profile/preferences", {});
    this.assertAccountContext(accountContext);
    this.state.setPreferences(result.preferences);
    return result;
  }

  async setProfilePreferences(request: SetProfilePreferencesRequest): Promise<ProfilePreferencesResponse> {
    const accountContext = this.state.accountContextToken();
    const result = await this.process.request<ProfilePreferencesResponse>("_axiom/profile/preferences/set", request);
    this.assertAccountContext(accountContext);
    this.state.setPreferences(result.preferences);
    return result;
  }

  async listCollections(): Promise<CollectionStateResponse> {
    const accountContext = this.state.accountContextToken();
    const result = await this.process.request<CollectionStateResponse>("_axiom/collection/list", {});
    this.assertAccountContext(accountContext);
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
    const accountContext = this.state.accountContextToken();
    const result = await this.process.request<AssignThreadCollectionResponse>("_axiom/collection/assign", {
      threadId,
      collectionId,
    });
    this.assertAccountContext(accountContext);
    this.state.setCollections(result.state);
    await this.refreshThread(threadId);
    return result;
  }

  accountStatus(): Promise<AccountStatusResponse> {
    return this.accountRequest("_axiom/account/status", {});
  }

  nativeLoginStart(methodHint?: LoginMethod): Promise<NativeLoginStartResponse> {
    return this.process.request("_axiom/account/native_login_start", { methodHint });
  }

  nativeLoginComplete(loginId: string, signal?: AbortSignal): Promise<AccountStatusResponse> {
    if (!loginId || loginId.length > 512) throw new ProtocolError("invalid native login ID");
    return this.accountRequest("_axiom/account/native_login_complete", { loginId }, 0, signal);
  }

  nativeLoginCancel(loginId: string): Promise<AccountStatusResponse> {
    if (!loginId || loginId.length > 512) throw new ProtocolError("invalid native login ID");
    return this.accountRequest("_axiom/account/native_login_cancel", { loginId });
  }

  logout(): Promise<AccountStatusResponse> {
    return this.accountRequest("_axiom/account/logout", {});
  }

  async usageSummary(request: UsageSummaryRequest = {}): Promise<UsageSummaryResponse> {
    const accountContext = this.state.accountContextToken();
    const result = await this.process.request<UsageSummaryResponse>("_axiom/usage/summary", request);
    this.assertAccountContext(accountContext);
    if (result.summary.period !== (request.period ?? "all_time")) {
      throw new Error("Usage is unavailable for the selected period");
    }
    return result;
  }

  async apiKeys(): Promise<ApiKeyListResponse> {
    const context = this.state.accountContextToken();
    const result = await this.process.request<ApiKeyListResponse>("_axiom/account/api_keys", {});
    this.assertAccountContext(context);
    return result;
  }

  async createApiKey(name: string): Promise<ApiKeyCreatedResponse> {
    if (!name.trim() || [...name].length > 120 || /[\u0000-\u001f\u007f]/.test(name)) throw new ProtocolError("Invalid API key name");
    const context = this.state.accountContextToken();
    const result = await this.process.request<ApiKeyCreatedResponse>("_axiom/account/api_key_create", {name: name.normalize("NFC").trim()});
    this.assertAccountContext(context);
    return result;
  }

  async revokeApiKey(id: string): Promise<ApiKeyRevokeResponse> {
    if (!/^[A-Za-z0-9_-]{1,128}$/.test(id)) throw new ProtocolError("Invalid API key ID");
    const context = this.state.accountContextToken();
    const result = await this.process.request<ApiKeyRevokeResponse>("_axiom/account/api_key_revoke", {id});
    this.assertAccountContext(context);
    return result;
  }

  async billingStatus(): Promise<BillingStatusResponse> {
    const accountContext = this.state.accountContextToken();
    const result = await this.process.request<BillingStatusResponse>("_axiom/billing/status", {});
    this.assertAccountContext(accountContext);
    this.state.setBilling(result.status);
    return result;
  }

  async redeemGiftCode(code: string): Promise<GiftCodeRedeemResponse> {
    if (typeof code !== "string" || !code.trim() || code.length > 64 || /[^\x20-\x7e]/.test(code)) {
      throw new ProtocolError("Enter a valid Axiom gift code.");
    }
    const accountContext = this.state.accountContextToken();
    const result = await this.process.request<GiftCodeRedeemResponse>("_axiom/billing/redeem_gift_code", {code});
    this.assertAccountContext(accountContext);
    this.state.setBilling(result.status);
    return result;
  }

  /** Called only by composition. Native code still enforces freshness at send. */
  prewarmSecurity(modelId: string): Promise<VerifySecurityResponse> {
    const accountContext = this.state.accountContextToken();
    const cached = this.securityWarmups.get(modelId);
    const previous = cached?.context === accountContext ? cached : undefined;
    const now = Date.now();
    // Keystrokes with reusable work need no RPC or full conversation snapshot.
    // Account/runtime transitions invalidate the context; disconnect clears the map.
    if (previous && (previous.pending || now < previous.validUntil || now < previous.retryAt)) return previous.promise;
    const snapshot = this.state.snapshot();
    if (!snapshot.connected || !modelId || modelId.length > 512 || snapshot.account?.state !== "valid") {
      return Promise.reject(new ProtocolError("Sign in and select a model before verifying"));
    }
    for (const [threadId, verification] of this.securityVerifications) {
      if (snapshot.sessions[threadId]?.settings?.model === modelId
        && this.state.securityContextToken(threadId) === verification.context) return verification.promise;
    }
    const contexts = Object.values(snapshot.sessions)
      .filter((session) => session.settings?.model === modelId && !session.running)
      .map((session) => [session.sessionId, this.state.securityContextToken(session.sessionId)] as const);
    const entry = { context: accountContext, promise: null! as Promise<VerifySecurityResponse>, pending: true, validUntil: 0, retryAt: 0, failures: previous?.failures ?? 0 };
    entry.promise = Promise.resolve().then(async () => {
      try {
        const result = await this.process.request<VerifySecurityResponse>("_axiom/security/prewarm", { modelId }, 0);
        this.assertAccountContext(accountContext);
        if (this.securityWarmups.get(modelId) !== entry) throw new ProtocolError("Verification context changed");
        const evidence = publicSecurityEvidence(result, modelId);
        if (!evidence || evidence.hardExpiresAtUnixSeconds * 1000 <= Date.now()) throw new ProtocolError("Verification returned no fresh evidence");
        entry.validUntil = evidence.hardExpiresAtUnixSeconds * 1000;
        entry.failures = 0;
        for (const [threadId, context] of contexts) {
          const session = this.state.snapshot().sessions[threadId];
          if (session && !session.running && !this.securityVerifications.has(threadId)
            && this.state.securityContextToken(threadId) === context) {
            this.state.setSecurityVerification(threadId, { pending: false, evidence, status: result.status });
          }
        }
        return { status: result.status, evidence };
      } catch (error) {
        entry.failures = Math.min(entry.failures + 1, 5);
        entry.retryAt = Date.now() + Math.min(300_000, 30_000 * 2 ** (entry.failures - 1));
        throw error;
      } finally { entry.pending = false; }
    });
    this.securityWarmups.set(modelId, entry);
    this.runBackground(entry.promise);
    return entry.promise;
  }

  verifySecurity(threadId: string, acceptOutdatedTee = false, expectedModelId?: string): Promise<VerifySecurityResponse> {
    const context = this.state.securityContextToken(threadId);
    const accountContext = this.state.accountContextToken();
    const session = this.state.snapshot().sessions[threadId];
    const modelId = session?.settings?.model;
    if (!context || !modelId) return Promise.reject(new ProtocolError("Load this thread before verifying its model"));
    if (acceptOutdatedTee && expectedModelId !== modelId) return Promise.reject(new ProtocolError("The selected model changed; review its warning before continuing."));
    const current = () => this.state.accountContextToken() === accountContext
      && this.state.securityContextToken(threadId) === context;
    const existing = this.securityVerifications.get(threadId);
    if (existing?.context === context) return acceptOutdatedTee
      ? existing.promise.catch(() => undefined).then(() => {
        if (!current()) throw new ProtocolError("The verification context changed; review its warning before continuing.");
        return this.verifySecurity(threadId, true, expectedModelId);
      })
      : existing.promise;
    if (session.running) return Promise.reject(new ProtocolError("Wait for the current reply before refreshing verification"));
    this.securityWarmups.delete(modelId);
    this.state.setSecurityVerification(threadId, { pending: true });
    // Defer dispatch until the map entry exists, including synchronous bridges.
    const promise = Promise.resolve().then(async () => {
      try {
        if (!current()) throw new ProtocolError("The verification context changed; review its warning before continuing.");
        const result = await this.process.request<VerifySecurityResponse>("_axiom/security/verify", { threadId, ...(acceptOutdatedTee ? { acceptOutdatedTee: true, modelId } : {}) }, 0);
        if (!current()) throw new ProtocolError("The verification context changed; refresh the selected model");
        const evidence = publicSecurityEvidence(result, modelId);
        this.state.setSecurityVerification(threadId, { pending: false, evidence });
        return { status: result.status, evidence };
      } catch (error) {
        if (current()) this.state.setSecurityVerification(threadId, {
          pending: false, error: error instanceof AcpError && error.code === -32001
            ? "Refresh your Axiom account or sign in again before verifying this connection."
            : "Couldn’t refresh the verification report. Try again.",
        });
        throw error;
      } finally {
        if (this.securityVerifications.get(threadId)?.context === context) this.securityVerifications.delete(threadId);
      }
    });
    this.securityVerifications.set(threadId, { context, promise });
    return promise;
  }

  compact(threadId: string, focus?: string): Promise<CompactResponse> {
    return this.waitForSecurityPreflight(threadId).then(() =>
      this.process.request("_axiom/compaction/start", { threadId, focus }, 0)
    );
  }

  async deletePreview(
    threadIds: string[],
    options: { cancelActiveWork?: boolean } = {},
  ): Promise<DeletePreviewResponse> {
    const accountContext = this.state.accountContextToken();
    if (!options.cancelActiveWork) {
      const preview = await this.process.request<DeletePreviewResponse>("_axiom/thread/delete_preview", { threadIds });
      this.assertAccountContext(accountContext);
      return preview;
    }
    // The desktop opts in only after the user confirms deletion. session/cancel
    // requests cancellation but does not acknowledge cleanup; a successful
    // preview is the native guard's acknowledgment that work has drained.
    const deadline = Date.now() + DELETE_DRAIN_TIMEOUT_MS;
    while (Date.now() < deadline) {
      this.assertAccountContext(accountContext);
      await Promise.all(threadIds.map((threadId) => this.cancel(threadId)));
      this.assertAccountContext(accountContext);
      try {
        const preview = await this.process.request<DeletePreviewResponse>(
          "_axiom/thread/delete_preview", { threadIds }, Math.max(1, deadline - Date.now()),
        );
        this.assertAccountContext(accountContext);
        return preview;
      } catch (error) {
        // Never retry arbitrary errors or send delete-confirm more than once.
        if (!(error instanceof AcpError)
          || error.code !== -32603
          || error.data !== "sessions with active work must be cancelled before deletion") throw error;
        await new Promise<void>((resolve) => setTimeout(resolve, Math.min(100, Math.max(0, deadline - Date.now()))));
      }
    }
    throw new ProtocolError("This thread is still stopping. Wait a moment and try deleting it again.");
  }

  async deleteConfirm(confirmationToken: string, threadIds: string[]): Promise<DeleteConfirmResponse> {
    const accountContext = this.state.accountContextToken();
    const result = await this.process.request<DeleteConfirmResponse>("_axiom/thread/delete_confirm", {
      confirmationToken,
      threadIds,
    });
    this.assertAccountContext(accountContext);
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
    if (this.accountingTimer) clearTimeout(this.accountingTimer);
    this.accountingTimer = null;
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

  private async fetchTimeline(
    threadId: string,
    accountContext: number,
  ): Promise<GetThreadTimelineResponse> {
    for (let attempt = 0; attempt < TIMELINE_RESTART_LIMIT; attempt += 1) {
      const pages: GetThreadTimelineResponse[] = [];
      let cursor: number | undefined;
      let usageCursor: string | undefined;
      let hasMore: boolean;
      let restart = false;
      do {
        const page = await this.process.request<GetThreadTimelineResponse>("_axiom/thread/timeline", {
          threadId,
          afterSequence: cursor,
          afterRequestId: usageCursor,
          limit: TIMELINE_PAGE_SIZE,
        });
        this.assertAccountContext(accountContext);
        const first = pages[0];
        if (first && (
          page.thread.threadId !== first.thread.threadId
          || page.thread.revision !== first.thread.revision
          || page.thread.lastTimelineSequence !== first.thread.lastTimelineSequence
        )) {
          restart = true;
          break;
        }
        if (page.thread.threadId !== threadId
          || (page.nextCursor != null && (page.nextCursor <= (cursor ?? 0)
            || page.nextCursor !== page.items.at(-1)?.sequence))
          || (page.nextRequestUsageCursor != null && (page.nextRequestUsageCursor <= (usageCursor ?? "")
            || page.nextRequestUsageCursor !== page.requestUsage?.at(-1)?.requestId))) {
          throw new ProtocolError("AxiomCLI returned a non-progressing timeline cursor");
        }
        pages.push(page);
        hasMore = page.nextCursor != null || page.nextRequestUsageCursor != null;
        cursor = page.nextCursor ?? page.thread.lastTimelineSequence;
        usageCursor = page.nextRequestUsageCursor ?? page.requestUsage?.at(-1)?.requestId ?? usageCursor;
      } while (hasMore);
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
      const accountContext = this.state.accountContextToken();
      const result = this.state.extensionEvent(notification.params as EventNotification);
      const accountChanged = accountContext !== this.state.accountContextToken();
      if (accountChanged) this.resetClientAccountContext();
      if (result?.fullResync && !accountChanged) this.fullResyncRequired = true;
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

  private requireDesktopChatCwd(): string {
    const cwd = this.requireDesktopBootstrap().chatCwd;
    if (!cwd) {
      throw new ProtocolError("Sign in to an Axiom account before opening local threads");
    }
    return cwd;
  }

  private assertAccountContext(expected: number): void {
    if (this.state.accountContextToken() !== expected) {
      throw new ProtocolError("Axiom account changed while the request was in progress");
    }
  }

  private resetClientAccountContext(): void {
    this.desktopBootstrap = null;
    this.refreshingThreads.clear();
    this.securityWarmups.clear();
    this.securityVerifications.clear();
    this.fullResync = null;
    this.fullResyncRequired = false;
    for (const [interactionId, pending] of this.interactions) {
      pending.respond(
        pending.interaction.kind === "permission"
          ? { outcome: { outcome: "cancelled" } }
          : { action: "cancel" },
      );
      this.interactions.delete(interactionId);
    }
  }

  private runBackground(task: Promise<unknown>): void {
    let tracked: Promise<void>;
    tracked = task.then(() => undefined, () => undefined).finally(() => this.background.delete(tracked));
    this.background.add(tracked);
  }

  private async waitForSecurityPreflight(threadId: string): Promise<void> {
    const model = this.state.snapshot().sessions[threadId]?.settings?.model;
    if (model) await this.securityWarmups.get(model)?.promise.catch(() => undefined);
    await this.securityVerifications.get(threadId)?.promise.catch(() => undefined);
  }

  private resyncAll(): Promise<void> {
    if (this.fullResync) return this.fullResync;
    this.fullResync = (async () => {
      const requests: Promise<unknown>[] = [
        this.listThreads(),
        this.listCollections(),
        this.getProfilePreferences(),
      ];
      if (this.state.snapshot().account?.state === "valid") this.runBackground(this.billingStatus());
      await Promise.all(requests);
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
    const accountContext = this.state.accountContextToken();
    const result = await this.process.request<CollectionStateResponse>(method, params);
    this.assertAccountContext(accountContext);
    this.state.setCollections(result.state);
    return result;
  }

  private async accountRequest(
    method: string,
    params: unknown,
    timeoutMs = 120_000,
    signal?: AbortSignal,
  ): Promise<AccountStatusResponse> {
    const accountContext = this.state.accountContextToken();
    const result = await this.process.request<AccountStatusResponse>(method, params, timeoutMs, signal);
    this.state.setAccount(result.status);
    if (accountContext !== this.state.accountContextToken()) {
      this.resetClientAccountContext();
      if (result.status.state === "valid") {
        await this.bootstrapDesktop();
        await this.initializeDesktopState();
      }
    }
    return result;
  }
}
