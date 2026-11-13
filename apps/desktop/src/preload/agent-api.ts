import { ipcRenderer } from "electron";
import type {
  AccountStatusResponse,
  BrowserLoginStartResponse,
  ClientState,
  CollectionStateResponse,
  CompactResponse,
  DeleteConfirmResponse,
  DeletePreviewResponse,
  DesktopBootstrapResponse,
  ElicitationOutcome,
  ListModelsResponse,
  ListThreadsRequest,
  ListThreadsResponse,
  PendingInteraction,
  PermissionOutcome,
  PromptResult,
  SessionStartResult,
  ThreadSettingsRequest,
  ThreadSettingsResult,
  VerifySecurityResponse,
} from "@axiom/axiom-acp-client";

export interface AgentApi {
  getState: () => Promise<ClientState>;
  desktopBootstrap: () => Promise<DesktopBootstrapResponse>;
  newChat: () => Promise<SessionStartResult>;
  loadChat: (sessionId: string) => Promise<SessionStartResult>;
  prompt: (sessionId: string, text: string, clientItemId: string) => Promise<PromptResult>;
  cancel: (sessionId: string) => Promise<void>;
  setConfig: (sessionId: string, configId: string, value: string) => Promise<void>;
  listThreads: (request?: ListThreadsRequest) => Promise<ListThreadsResponse>;
  listModels: () => Promise<ListModelsResponse>;
  setSettings: (settings: ThreadSettingsRequest) => Promise<ThreadSettingsResult>;
  accountStatus: () => Promise<AccountStatusResponse>;
  browserLoginStart: () => Promise<BrowserLoginStartResponse>;
  browserLoginComplete: (loginId: string) => Promise<AccountStatusResponse>;
  browserLoginCancel: (loginId: string) => Promise<void>;
  useApiKey: (apiKey: string) => Promise<AccountStatusResponse>;
  logout: () => Promise<AccountStatusResponse>;
  verifySecurity: (sessionId: string) => Promise<VerifySecurityResponse>;
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
  loadChat: (sessionId) => invoke("chat-load", sessionId),
  prompt: (sessionId, text, clientItemId) => invoke("prompt", sessionId, text, clientItemId),
  cancel: (sessionId) => invoke("cancel", sessionId),
  setConfig: (sessionId, configId, value) => invoke("set-config", sessionId, configId, value),
  listThreads: (request) => invoke("threads-list", request),
  listModels: () => invoke("models-list"),
  setSettings: (settings) => invoke("set-settings", settings),
  accountStatus: () => invoke("account-status"),
  browserLoginStart: () => invoke("browser-login-start"),
  browserLoginComplete: (loginId) => invoke("browser-login-complete", loginId),
  browserLoginCancel: (loginId) => invoke("browser-login-cancel", loginId),
  useApiKey: (apiKey) => invoke("use-api-key", apiKey),
  logout: () => invoke("logout"),
  verifySecurity: (sessionId) => invoke("security-verify", sessionId),
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
