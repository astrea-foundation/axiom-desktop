import { EventEmitter } from "node:events";
import type {
  AccountStatusResponse,
  BrowserLoginStartResponse,
  CompactResponse,
  DeleteConfirmResponse,
  DeletePreviewResponse,
  EventNotification,
  GetSessionStateResponse,
  ListModelsResponse,
  ListSessionsRequest,
  ListSessionsResponse,
  SetSessionSettingsRequest,
  SetSessionSettingsResponse,
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
  SessionStartResult,
} from "./types.js";

const EXTENSION_CAPABILITIES = {
  protocolVersion: "0.1",
  features: {
    sessionCatalog: 1,
    settings: 1,
    account: 1,
    securityEvidence: 1,
    compaction: 1,
    activity: 1,
  },
} as const;

type InteractionResolver = {
  interaction: PendingInteraction;
  respond: (result: unknown) => void;
};

export class AxiomAcpClient extends EventEmitter {
  readonly process: AcpProcess;
  readonly state = new AxiomStateStore();
  private readonly interactions = new Map<string, InteractionResolver>();
  private initialized = false;

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
      features?: Partial<Record<keyof typeof EXTENSION_CAPABILITIES.features, number>>;
    } | undefined;
    if (meta?.protocolVersion !== EXTENSION_CAPABILITIES.protocolVersion) {
      throw new ProtocolError("AxiomCLI does not support the required Axiom desktop extension");
    }
    for (const [feature, requiredVersion] of Object.entries(EXTENSION_CAPABILITIES.features)) {
      if ((meta.features?.[feature as keyof typeof EXTENSION_CAPABILITIES.features] ?? 0) < requiredVersion) {
        throw new ProtocolError(`AxiomCLI does not support required desktop feature ${feature}@${requiredVersion}`);
      }
    }
    this.initialized = true;
    this.state.setConnected(true);
    return result;
  }

  getState(): ClientState {
    return this.state.snapshot();
  }

  async newSession(cwd: string): Promise<SessionStartResult> {
    this.assertInitialized();
    const result = await this.process.request<SessionStartResult>("session/new", { cwd, mcpServers: [] });
    this.state.addSession(result.sessionId, cwd);
    if (result.modes) this.state.setModes(result.sessionId, result.modes.currentModeId, result.modes.availableModes);
    if (result.configOptions) this.state.setConfig(result.sessionId, result.configOptions);
    await this.refreshSession(result.sessionId);
    void this.verifySecurity(result.sessionId).catch(() => undefined);
    return result;
  }

  async loadSession(sessionId: string, cwd: string): Promise<SessionStartResult> {
    this.assertInitialized();
    const result = await this.process.request<SessionStartResult>("session/load", { sessionId, cwd, mcpServers: [] });
    this.state.addSession(sessionId, cwd);
    if (result.modes) this.state.setModes(sessionId, result.modes.currentModeId, result.modes.availableModes);
    if (result.configOptions) this.state.setConfig(sessionId, result.configOptions);
    await this.refreshSession(sessionId);
    void this.verifySecurity(sessionId).catch(() => undefined);
    return { ...result, sessionId };
  }

  async prompt(sessionId: string, text: string): Promise<PromptResult> {
    if (!text.trim()) throw new ProtocolError("prompt cannot be empty");
    this.state.addUserMessage(sessionId, text);
    this.state.setRunning(sessionId, true);
    try {
      return await this.process.request<PromptResult>(
        "session/prompt",
        { sessionId, prompt: [{ type: "text", text }] },
        0,
      );
    } finally {
      this.state.setRunning(sessionId, false);
      await this.refreshSession(sessionId).catch(() => undefined);
    }
  }

  async cancel(sessionId: string): Promise<void> {
    // Interactions are client-owned ACP requests. Resolve them while cancelling
    // the turn so a renderer disappearing or pressing Stop can never leave the
    // trusted runtime waiting indefinitely (and can never imply approval). Send
    // the cancellation first on the ordered pipe so denial cannot let the turn
    // race ahead to a normal completion.
    try {
      await this.process.notify("session/cancel", { sessionId });
    } finally {
      for (const [interactionId, pending] of this.interactions) {
        if (pending.interaction.sessionId !== sessionId) continue;
        this.resolveInteraction(
          interactionId,
          pending.interaction.kind === "permission"
            ? { outcome: { outcome: "cancelled" } }
            : { action: "cancel" },
        );
      }
    }
  }

  async setMode(sessionId: string, modeId: string): Promise<void> {
    await this.process.request("session/set_mode", { sessionId, modeId });
  }

  async setConfig(sessionId: string, configId: string, value: string): Promise<void> {
    const result = await this.process.request<{ configOptions: [] }>("session/set_config_option", {
      sessionId,
      configId,
      value,
    });
    this.state.setConfig(sessionId, result.configOptions);
  }

  async listSessions(request: ListSessionsRequest = {}): Promise<ListSessionsResponse> {
    const result = await this.process.request<ListSessionsResponse>("_axiom/session/list", request);
    this.state.setCatalog(result.sessions);
    return result;
  }

  async refreshSession(sessionId: string): Promise<GetSessionStateResponse> {
    const result = await this.process.request<GetSessionStateResponse>("_axiom/session/get_state", { sessionId });
    this.state.setSnapshot(result.snapshot);
    return result;
  }

  listModels(): Promise<ListModelsResponse> {
    return this.process.request("_axiom/models/list", {});
  }

  async setSettings(request: SetSessionSettingsRequest): Promise<SetSessionSettingsResponse> {
    const result = await this.process.request<SetSessionSettingsResponse>("_axiom/session/set_settings", request);
    await this.refreshSession(request.sessionId);
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

  async verifySecurity(sessionId: string): Promise<VerifySecurityResponse> {
    const result = await this.process.request<VerifySecurityResponse>("_axiom/security/verify", { sessionId }, 0);
    await this.refreshSession(sessionId);
    return result;
  }

  compact(sessionId: string, focus?: string): Promise<CompactResponse> {
    return this.process.request("_axiom/compaction/start", { sessionId, focus }, 0);
  }

  deletePreview(sessionIds: string[]): Promise<DeletePreviewResponse> {
    return this.process.request("_axiom/session/delete_preview", { sessionIds });
  }

  async deleteConfirm(confirmationToken: string, sessionIds: string[]): Promise<DeleteConfirmResponse> {
    const result = await this.process.request<DeleteConfirmResponse>("_axiom/session/delete_confirm", {
      confirmationToken,
      sessionIds,
    });
    for (const sessionId of sessionIds) this.state.removeSession(sessionId);
    await this.listSessions();
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

  close(): Promise<void> {
    for (const pending of this.interactions.values()) {
      pending.respond(
        pending.interaction.kind === "permission"
          ? { outcome: { outcome: "cancelled" } }
          : { action: "cancel" },
      );
    }
    this.interactions.clear();
    return this.process.close();
  }

  private onNotification(notification: RpcNotification): void {
    if (notification.method === "session/update") this.state.standardUpdate(notification.params);
    if (notification.method === "_axiom/event") {
      const sessionId = this.state.extensionEvent(notification.params as EventNotification);
      const state = sessionId ? this.state.snapshot().sessions[sessionId] : undefined;
      if (sessionId && state?.needsResync) void this.refreshSession(sessionId);
    }
  }

  private onRequest(incoming: IncomingRequest): void {
    const { request } = incoming;
    const params = request.params as Record<string, unknown> | undefined;
    const mode = params?.mode && typeof params.mode === "object"
      ? (params.mode as Record<string, unknown>)
      : undefined;
    const sessionId =
      typeof params?.sessionId === "string"
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
