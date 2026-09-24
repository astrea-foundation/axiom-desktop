import { ipcRenderer } from "electron";
import type { PromptAttachment, GetAttachmentsResponse } from "@axiom/axiom-acp-client";
import type {
  AccountStatusResponse,
  BillingStatusResponse,
  GiftCodeRedeemResponse,
  UsageSummaryResponse,
  UsageSummaryRequest,
  ApiKeyListResponse,
  ApiKeyCreatedResponse,
  ApiKeyRevokeResponse,
  ClientState,
  CollectionStateResponse,
  CompactResponse,
  DeleteConfirmResponse,
  DeletePreviewResponse,
  DesktopBootstrapResponse,
  ConfigureDesktopAgentRequest,
  ConfigureDesktopAgentResponse,
  ElicitationOutcome,
  ListModelsResponse,
  ListThreadsRequest,
  ListThreadsResponse,
  LoginMethod,
  NativeLoginStartResponse,
  PendingInteraction,
  PermissionOutcome,
  PromptResult,
  RenameThreadResponse,
  SessionStartResult,
  SteerTurnRequest,
  SteerTurnResponse,
  ThreadSettingsRequest,
  ThreadSettingsResult,
  VerifySecurityResponse,
} from "@axiom/axiom-acp-client";

export interface AgentApi {
  getState: () => Promise<ClientState>;
  desktopBootstrap: () => Promise<DesktopBootstrapResponse>;
  newChat: () => Promise<SessionStartResult>;
  configureDesktopAgent: (request: ConfigureDesktopAgentRequest) => Promise<ConfigureDesktopAgentResponse>;
  chooseWorkingDirectory: (current?: string) => Promise<string | null>;
  loadChat: (sessionId: string) => Promise<SessionStartResult>;
  // Deliberately distinct from the legacy prompt API: a hot-reloaded renderer
  // must never send through a preload/main process that ignores Web consent.
  promptWithWebConsent: (sessionId: string, text: string, clientItemId: string, webEnabled: boolean, agentRevision?: number) => Promise<PromptResult>;
  promptWithAttachments: (sessionId: string, text: string, clientItemId: string, webEnabled: boolean, agentRevision: number, attachments: PromptAttachment[]) => Promise<PromptResult>;
  getAttachments: (threadId: string, userItemId: string) => Promise<GetAttachmentsResponse>;
  revisePrompt: (sessionId: string, text: string, userItemId: string, expectedRevision: number, webEnabled: boolean, agentRevision: number) => Promise<PromptResult>;
  steer: (request: SteerTurnRequest) => Promise<SteerTurnResponse>;
  cancel: (sessionId: string) => Promise<void>;
  setConfig: (sessionId: string, configId: string, value: string) => Promise<void>;
  listThreads: (request?: ListThreadsRequest) => Promise<ListThreadsResponse>;
  renameThread: (threadId: string, title: string) => Promise<RenameThreadResponse>;
  listModels: () => Promise<ListModelsResponse>;
  setSettings: (settings: ThreadSettingsRequest) => Promise<ThreadSettingsResult>;
  accountStatus: () => Promise<AccountStatusResponse>;
  billingStatus: () => Promise<BillingStatusResponse>;
  redeemGiftCode: (code: string, accountId: string) => Promise<GiftCodeRedeemResponse>;
  usageSummary: (accountId: string, request?: UsageSummaryRequest) => Promise<UsageSummaryResponse>;
  apiKeys: (accountId: string) => Promise<ApiKeyListResponse>;
  createApiKey: (name: string, accountId: string) => Promise<ApiKeyCreatedResponse>;
  revokeApiKey: (id: string, accountId: string) => Promise<ApiKeyRevokeResponse>;
  nativeLoginStart: (methodHint?: LoginMethod) => Promise<NativeLoginStartResponse>;
  nativeLoginComplete: (loginId: string) => Promise<AccountStatusResponse>;
  nativeLoginCancel: (loginId: string) => Promise<AccountStatusResponse>;
  openAccountPortal: () => Promise<void>;
  logout: () => Promise<AccountStatusResponse>;
  prewarmSecurity: (modelId: string) => Promise<VerifySecurityResponse>;
  verifySecurity: (sessionId: string, acceptOutdatedTee?: boolean, modelId?: string) => Promise<VerifySecurityResponse>;
  compact: (sessionId: string, focus?: string) => Promise<CompactResponse>;
  deletePreview: (sessionIds: string[]) => Promise<DeletePreviewResponse>;
  deleteConfirm: (confirmationToken: string, sessionIds: string[]) => Promise<DeleteConfirmResponse>;
  resolvePermission: (interactionId: string, outcome: PermissionOutcome) => Promise<void>;
  resolveElicitation: (interactionId: string, outcome: ElicitationOutcome) => Promise<void>;
  listCollections: () => Promise<CollectionStateResponse>;
  createCollection: (name: string) => Promise<CollectionStateResponse>;
  renameCollection: (collectionId: string, name: string) => Promise<CollectionStateResponse>;
  setCollectionCollapsed: (collectionId: string, collapsed: boolean) => Promise<CollectionStateResponse>;
  moveCollection: (collectionId: string, position: number) => Promise<CollectionStateResponse>;
  deleteCollection: (collectionId: string) => Promise<CollectionStateResponse>;
  assignThreadCollection: (threadId: string, collectionId: string | null) => Promise<unknown>;
  onState: (callback: (state: ClientState) => void) => () => void;
  onInteraction: (callback: (interaction: PendingInteraction) => void) => () => void;
}

function invoke<T>(method: string, ...args: unknown[]): Promise<T> {
  return ipcRenderer.invoke(`agent:${method}`, ...args) as Promise<T>;
}

export const agentApi: AgentApi = {
  getState: () => invoke("state"),
  desktopBootstrap: () => invoke("desktop-bootstrap"),
  newChat: () => invoke("chat-new"),
  configureDesktopAgent: (request) => invoke("desktop-agent-configure", request),
  chooseWorkingDirectory: (current) => invoke("working-directory-choose", current),
  loadChat: (sessionId) => invoke("chat-load", sessionId),
  promptWithWebConsent: (sessionId, text, clientItemId, webEnabled, agentRevision = 0) => invoke("prompt-with-agent-context", sessionId, text, clientItemId, webEnabled, agentRevision),
  promptWithAttachments: (sessionId, text, clientItemId, webEnabled, agentRevision, attachments) => invoke("prompt-with-attachments", sessionId, text, clientItemId, webEnabled, agentRevision, attachments),
  getAttachments: (threadId, userItemId) => invoke("attachments", threadId, userItemId),
  revisePrompt: (sessionId, text, userItemId, expectedRevision, webEnabled, agentRevision) => invoke("prompt-revise", sessionId, text, userItemId, expectedRevision, webEnabled, agentRevision),
  steer: (request) => invoke("steer", request),
  cancel: (sessionId) => invoke("cancel", sessionId),
  setConfig: (sessionId, configId, value) => invoke("set-config", sessionId, configId, value),
  listThreads: (request) => invoke("threads-list", request),
  renameThread: (threadId, title) => invoke("thread-rename", threadId, title),
  listModels: () => invoke("models-list"),
  setSettings: (settings) => invoke("set-settings", settings),
  accountStatus: () => invoke("account-status"),
  billingStatus: () => invoke("billing-status"),
  redeemGiftCode: (code, accountId) => invoke("redeem-gift-code", code, accountId),
  usageSummary: (accountId, request = {}) => invoke("usage-summary", accountId, request.period ?? "all_time", request.timezone ?? "UTC"),
  apiKeys: (accountId) => invoke("api-keys", accountId),
  createApiKey: (name, accountId) => invoke("api-key-create", name, accountId),
  revokeApiKey: (id, accountId) => invoke("api-key-revoke", id, accountId),
  nativeLoginStart: (methodHint) => invoke("native-login-start", methodHint),
  nativeLoginComplete: (loginId) => invoke("native-login-complete", loginId),
  nativeLoginCancel: (loginId) => invoke("native-login-cancel", loginId),
  openAccountPortal: () => invoke("account-open"),
  logout: () => invoke("logout"),
  prewarmSecurity: (modelId) => invoke("security-prewarm", modelId),
  verifySecurity: (sessionId, acceptOutdatedTee, modelId) => invoke("security-verify", sessionId, acceptOutdatedTee, modelId),
  compact: (sessionId, focus) => invoke("compact", sessionId, focus),
  deletePreview: (sessionIds) => invoke("delete-preview", sessionIds),
  deleteConfirm: (confirmationToken, sessionIds) => invoke("delete-confirm", confirmationToken, sessionIds),
  resolvePermission: (interactionId, outcome) => invoke("permission-resolve", interactionId, outcome),
  resolveElicitation: (interactionId, outcome) => invoke("elicitation-resolve", interactionId, outcome),
  listCollections: () => invoke("collections-list"),
  createCollection: (name) => invoke("collection-create", name),
  renameCollection: (collectionId, name) => invoke("collection-rename", collectionId, name),
  setCollectionCollapsed: (collectionId, collapsed) => invoke("collection-collapsed", collectionId, collapsed),
  moveCollection: (collectionId, position) => invoke("collection-move", collectionId, position),
  deleteCollection: (collectionId) => invoke("collection-delete", collectionId),
  assignThreadCollection: (threadId, collectionId) => invoke("collection-assign", threadId, collectionId),
  onState: (callback) => {
    const listener = (_event: unknown, state: ClientState) => callback(state);
    ipcRenderer.on("agent:state-changed", listener);
    return () => ipcRenderer.removeListener("agent:state-changed", listener);
  },
  onInteraction: (callback) => {
    const listener = (_event: unknown, interaction: PendingInteraction) => callback(interaction);
    ipcRenderer.on("agent:interaction", listener);
    return () => ipcRenderer.removeListener("agent:interaction", listener);
  },
};
