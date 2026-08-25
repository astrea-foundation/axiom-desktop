/* Generated from protocol/axiom-acp-extension/v0.1/schema.json. Do not edit. */

export type SessionLifecycle =
  "ready" | "running" | "waiting_for_approval" | "waiting_for_answer" | "compacting" | "closed";
export type SecurityState = "unverified" | "verifying" | "unattested_development" | "verified" | "failed";
export type AccountState = "missing" | "valid" | "invalid" | "unavailable";
export type ActivityEvent =
  | {
      snapshot: SessionSnapshot;
      kind: "session_state";
      [k: string]: unknown;
    }
  | {
      title: string;
      kind: "title_changed";
      [k: string]: unknown;
    }
  | {
      settings: SessionSettings;
      kind: "settings_changed";
      [k: string]: unknown;
    }
  | {
      status: SecurityStatus;
      kind: "security_changed";
      [k: string]: unknown;
    }
  | {
      status: AccountStatus;
      kind: "account_changed";
      [k: string]: unknown;
    }
  | {
      message: string;
      completed?: number | null;
      total?: number | null;
      kind: "progress";
      [k: string]: unknown;
    }
  | {
      items: TaskItem[];
      kind: "task_list";
      [k: string]: unknown;
    }
  | {
      task_id: string;
      state: string;
      kind: "background_task";
      [k: string]: unknown;
    }
  | {
      paths: string[];
      kind: "workspace_changed";
      [k: string]: unknown;
    }
  | {
      phase: CompactionPhase;
      detail?: string | null;
      kind: "compaction";
      [k: string]: unknown;
    }
  | {
      message: string;
      kind: "warning";
      [k: string]: unknown;
    }
  | {
      message: string;
      kind: "error";
      [k: string]: unknown;
    };
export type CompactionPhase = "started" | "summarizing" | "replacing_context" | "completed" | "cancelled" | "failed";
export type ExtensionErrorCode =
  | "unsupported_feature"
  | "invalid_request"
  | "invalid_state"
  | "not_found"
  | "conflict"
  | "permission_denied"
  | "authentication_required"
  | "security_verification_failed"
  | "cancelled"
  | "expired"
  | "provider_unavailable"
  | "internal";

/**
 * Schema root used to generate the checked-in language-neutral contract.
 */
export interface ProtocolSchema {
  capabilities: ExtensionCapabilities;
  listSessionsRequest: ListSessionsRequest;
  listSessionsResponse: ListSessionsResponse;
  getSessionStateRequest: GetSessionStateRequest;
  getSessionStateResponse: GetSessionStateResponse;
  deletePreviewRequest: DeletePreviewRequest;
  deletePreviewResponse: DeletePreviewResponse;
  deleteConfirmRequest: DeleteConfirmRequest;
  deleteConfirmResponse: DeleteConfirmResponse;
  listModelsRequest: ListModelsRequest;
  listModelsResponse: ListModelsResponse;
  setSessionSettingsRequest: SetSessionSettingsRequest;
  setSessionSettingsResponse: SetSessionSettingsResponse;
  accountStatusRequest: AccountStatusRequest;
  accountStatusResponse: AccountStatusResponse;
  browserLoginStartRequest: BrowserLoginStartRequest;
  browserLoginStartResponse: BrowserLoginStartResponse;
  browserLoginCompleteRequest: BrowserLoginCompleteRequest;
  useApiKeyRequest: UseApiKeyRequest;
  logoutRequest: LogoutRequest;
  verifySecurityRequest: VerifySecurityRequest;
  verifySecurityResponse: VerifySecurityResponse;
  compactRequest: CompactRequest;
  compactResponse: CompactResponse;
  eventNotification: EventNotification;
  error: ExtensionError;
  metadata: {
    [k: string]: unknown;
  };
  [k: string]: unknown;
}
export interface ExtensionCapabilities {
  protocolVersion: string;
  features: FeatureVersions;
  runtimeInstanceId?: string | null;
  [k: string]: unknown;
}
export interface FeatureVersions {
  sessionCatalog: number;
  settings: number;
  account: number;
  securityEvidence: number;
  compaction: number;
  activity: number;
  [k: string]: unknown;
}
export interface ListSessionsRequest {
  query?: string | null;
  includeArchived?: boolean;
  limit?: number | null;
  /**
   * Opaque cursor returned by the previous page.
   */
  cursor?: string | null;
  [k: string]: unknown;
}
export interface ListSessionsResponse {
  sessions: SessionSummary[];
  nextCursor?: string | null;
  [k: string]: unknown;
}
export interface SessionSummary {
  sessionId: string;
  title?: string | null;
  cwd: string;
  updatedAt: string;
  settings: SessionSettings;
  archived: boolean;
  historyTruncated: boolean;
  lastOutcome?: string | null;
  [k: string]: unknown;
}
export interface SessionSettings {
  model: string;
  thinkingLevel: string;
  permissionProfile: string;
  [k: string]: unknown;
}
export interface GetSessionStateRequest {
  sessionId: string;
  [k: string]: unknown;
}
export interface GetSessionStateResponse {
  snapshot: SessionSnapshot;
  [k: string]: unknown;
}
export interface SessionSnapshot {
  runtimeInstanceId: string;
  sequence: number;
  sessionId: string;
  title?: string | null;
  cwd: string;
  lifecycle: SessionLifecycle;
  settings: SessionSettings;
  security: SecurityStatus;
  activeTurnId?: string | null;
  hasPrompt: boolean;
  [k: string]: unknown;
}
export interface SecurityStatus {
  state: SecurityState;
  detail?: string | null;
  [k: string]: unknown;
}
export interface DeletePreviewRequest {
  sessionIds: string[];
  [k: string]: unknown;
}
export interface DeletePreviewResponse {
  confirmationToken: string;
  expiresAtUnixSeconds: number;
  sessions: SessionSummary[];
  [k: string]: unknown;
}
export interface DeleteConfirmRequest {
  confirmationToken: string;
  sessionIds: string[];
  [k: string]: unknown;
}
export interface DeleteConfirmResponse {
  deleted: number;
  [k: string]: unknown;
}
export interface ListModelsRequest {
  [k: string]: unknown;
}
export interface ListModelsResponse {
  models: ModelInfo[];
  [k: string]: unknown;
}
export interface ModelInfo {
  id: string;
  label: string;
  thinkingLevels: string[];
  [k: string]: unknown;
}
export interface SetSessionSettingsRequest {
  sessionId: string;
  model?: string | null;
  thinkingLevel?: string | null;
  permissionProfile?: string | null;
  [k: string]: unknown;
}
export interface SetSessionSettingsResponse {
  settings: SessionSettings;
  [k: string]: unknown;
}
export interface AccountStatusRequest {
  [k: string]: unknown;
}
export interface AccountStatusResponse {
  status: AccountStatus;
  [k: string]: unknown;
}
export interface AccountStatus {
  keyId?: string | null;
  keyName?: string | null;
  state: AccountState;
  detail?: string | null;
  [k: string]: unknown;
}
export interface BrowserLoginStartRequest {
  [k: string]: unknown;
}
export interface BrowserLoginStartResponse {
  login: BrowserLoginState;
  [k: string]: unknown;
}
export interface BrowserLoginState {
  loginId: string;
  userCode: string;
  authorizationUrl: string;
  browserOpened: boolean;
  [k: string]: unknown;
}
export interface BrowserLoginCompleteRequest {
  loginId: string;
  [k: string]: unknown;
}
export interface UseApiKeyRequest {
  apiKey: string;
  [k: string]: unknown;
}
export interface LogoutRequest {
  [k: string]: unknown;
}
export interface VerifySecurityRequest {
  sessionId: string;
  [k: string]: unknown;
}
export interface VerifySecurityResponse {
  evidence?: SecurityEvidence | null;
  status: SecurityStatus;
  [k: string]: unknown;
}
export interface SecurityEvidence {
  status: SecurityStatus;
  providerId: string;
  modelId: string;
  attestationProtocol: string;
  e2eeProtocol: string;
  e2eeEncryptionVersion: number;
  trustPolicyVersion: string;
  verifiedAtUnixSeconds: number;
  modelKeyFingerprint: string;
  tlsSpkiFingerprint?: string | null;
  checks: EvidenceCheck[];
  providerClaims: EvidenceClaim[];
  workloadManifest?: string | null;
  workloadIdentityAllowlistConfigured: boolean;
  approvedWorkloadIdentity: boolean;
  [k: string]: unknown;
}
export interface EvidenceCheck {
  name: string;
  passed: boolean;
  detail: string;
  [k: string]: unknown;
}
export interface EvidenceClaim {
  name: string;
  value: string;
  [k: string]: unknown;
}
export interface CompactRequest {
  sessionId: string;
  focus?: string | null;
  [k: string]: unknown;
}
export interface CompactResponse {
  messagesBefore: number;
  [k: string]: unknown;
}
export interface EventNotification {
  runtimeInstanceId: string;
  sessionId?: string | null;
  sequence: number;
  /**
   * RFC 3339 time recorded by `AxiomCLI` when the outward event was created.
   */
  occurredAt: string;
  correlationId?: string | null;
  event: ActivityEvent;
  [k: string]: unknown;
}
export interface TaskItem {
  id: string;
  title: string;
  status: string;
  [k: string]: unknown;
}
export interface ExtensionError {
  code: ExtensionErrorCode;
  message: string;
  retryable: boolean;
  correlationId?: string | null;
  [k: string]: unknown;
}
