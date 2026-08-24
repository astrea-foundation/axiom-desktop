import { ipcRenderer } from "electron";
import type {
  AccountStatusResponse,
  BrowserLoginStartResponse,
  ClientState,
  CompactResponse,
  DeleteConfirmResponse,
  DeletePreviewResponse,
  ElicitationOutcome,
  ListModelsResponse,
  ListSessionsRequest,
  ListSessionsResponse,
  PendingInteraction,
  PermissionOutcome,
  PromptResult,
  SessionStartResult,
  SetSessionSettingsRequest,
  SetSessionSettingsResponse,
  VerifySecurityResponse,
} from "@axiom/axiom-acp-client";

export interface AgentApi {
  getState: () => Promise<ClientState>;
  chooseWorkspace: () => Promise<string | null>;
  newSession: (cwd: string) => Promise<SessionStartResult>;
  loadSession: (sessionId: string, cwd: string) => Promise<SessionStartResult>;
  prompt: (sessionId: string, text: string) => Promise<PromptResult>;
  cancel: (sessionId: string) => Promise<void>;
  setMode: (sessionId: string, modeId: string) => Promise<void>;
  setConfig: (sessionId: string, configId: string, value: string) => Promise<void>;
  listSessions: (request?: ListSessionsRequest) => Promise<ListSessionsResponse>;
  listModels: () => Promise<ListModelsResponse>;
  setSettings: (settings: SetSessionSettingsRequest) => Promise<SetSessionSettingsResponse>;
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
  onState: (callback: (state: ClientState) => void) => () => void;
  onInteraction: (callback: (interaction: PendingInteraction) => void) => () => void;
}

function invoke<T>(method: string, ...args: unknown[]): Promise<T> {
  return ipcRenderer.invoke(`agent:${method}`, ...args) as Promise<T>;
}

export const agentApi: AgentApi = {
  getState: () => invoke("state"),
  chooseWorkspace: () => invoke("choose-workspace"),
  newSession: (cwd) => invoke("session-new", cwd),
  loadSession: (sessionId, cwd) => invoke("session-load", sessionId, cwd),
  prompt: (sessionId, text) => invoke("prompt", sessionId, text),
  cancel: (sessionId) => invoke("cancel", sessionId),
  setMode: (sessionId, modeId) => invoke("set-mode", sessionId, modeId),
  setConfig: (sessionId, configId, value) => invoke("set-config", sessionId, configId, value),
  listSessions: (request) => invoke("sessions-list", request),
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
