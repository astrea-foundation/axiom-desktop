export type JsonObject = Record<string, unknown>;

export interface RpcRequest {
  jsonrpc: "2.0";
  id: number | string;
  method: string;
  params?: unknown;
}

export interface RpcNotification {
  jsonrpc: "2.0";
  method: string;
  params?: unknown;
}

export type PermissionOutcome =
  | { outcome: "selected"; optionId: string }
  | { outcome: "cancelled" };

export type ElicitationOutcome =
  | { action: "accept"; content: Record<string, string | string[]> }
  | { action: "decline" }
  | { action: "cancel" };

export interface PendingInteraction {
  id: string;
  requestId: number | string;
  sessionId: string | null;
  kind: "permission" | "elicitation";
  payload: unknown;
}

export interface StandardSessionUpdate {
  sessionId: string;
  update: Record<string, unknown>;
}

export interface AgentCapabilities {
  loadSession?: boolean;
  _meta?: Record<string, unknown>;
}

export interface InitializeResult {
  protocolVersion: number;
  agentCapabilities: AgentCapabilities;
  agentInfo?: { name: string; version: string; title?: string };
}

export interface SessionConfigOption {
  id: string;
  name: string;
  currentValue: string;
  options?: Array<{ value: string; name: string }>;
}

export interface SessionMode {
  id: string;
  name: string;
  description?: string;
}

export interface SessionStartResult {
  sessionId: string;
  modes?: { currentModeId: string; availableModes: SessionMode[] };
  configOptions?: SessionConfigOption[];
}

export interface PromptResult {
  stopReason: "end_turn" | "cancelled" | "max_tokens" | "refusal" | string;
}
