/* Generated from protocol/axiom-acp-extension/v0.1/schema.json. Do not edit. */

export type ThreadLifecycle =
  "ready" | "running" | "waiting_for_approval" | "waiting_for_answer" | "compacting" | "closed";
export type TimelineItemKind = "user_message" | "assistant_message" | "reasoning" | "tool_call" | "plan" | "notice";
export type TimelineItemStatus = "pending" | "in_progress" | "completed" | "cancelled" | "failed" | "interrupted";
export type AccountState = "missing" | "valid" | "invalid" | "unavailable";
export type SecurityState = "unverified" | "verifying" | "unattested_development" | "verified" | "failed";
export type ActivityEvent =
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
      state: CollectionState;
      kind: "collections_changed";
      [k: string]: unknown;
    }
  | {
      preferences: ProfilePreferences1;
      kind: "profile_preferences_changed";
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
  desktopBootstrapRequest: DesktopBootstrapRequest;
  desktopBootstrapResponse: DesktopBootstrapResponse;
  promptMetadata: PromptMetadata;
  deliveryMetadata: DeliveryMetadata;
  listThreadsRequest: ListThreadsRequest;
  listThreadsResponse: ListThreadsResponse;
  getThreadTimelineRequest: GetThreadTimelineRequest;
  getThreadTimelineResponse: GetThreadTimelineResponse;
  deletePreviewRequest: DeletePreviewRequest;
  deletePreviewResponse: DeletePreviewResponse;
  deleteConfirmRequest: DeleteConfirmRequest;
  deleteConfirmResponse: DeleteConfirmResponse;
  listModelsRequest: ListModelsRequest;
  listModelsResponse: ListModelsResponse;
  getProfilePreferencesRequest: GetProfilePreferencesRequest;
  setProfilePreferencesRequest: SetProfilePreferencesRequest;
  profilePreferencesResponse: ProfilePreferencesResponse;
  listCollectionsRequest: ListCollectionsRequest;
  createCollectionRequest: CreateCollectionRequest;
  renameCollectionRequest: RenameCollectionRequest;
  setCollectionCollapsedRequest: SetCollectionCollapsedRequest;
  moveCollectionRequest: MoveCollectionRequest;
  deleteCollectionRequest: DeleteCollectionRequest;
  assignThreadCollectionRequest: AssignThreadCollectionRequest;
  collectionStateResponse: CollectionStateResponse;
  assignThreadCollectionResponse: AssignThreadCollectionResponse;
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
  desktopChat: number;
  threadCatalog: number;
  timeline: number;
  modelCatalog: number;
  profilePreferences: number;
  collections: number;
  account: number;
  securityEvidence: number;
  compaction: number;
  activity: number;
  [k: string]: unknown;
}
export interface DesktopBootstrapRequest {
  [k: string]: unknown;
}
export interface DesktopBootstrapResponse {
  /**
   * Stable front-end identifier selected when the sidecar was launched.
   */
  frontend: string;
  /**
   * Canonical workspace used for every desktop chat thread.
   */
  chatCwd: string;
  /**
   * Effective, immutable permission profile for desktop chat.
   */
  permissionProfile: string;
  newThreadSettings: ProfilePreferences;
  [k: string]: unknown;
}
/**
 * Settings `AxiomCLI` will apply to the next thread. On a clean profile the
 * thinking level is `medium`; afterward these are the last-used values.
 */
export interface ProfilePreferences {
  model?: string | null;
  thinkingLevel: string;
  updatedAt: string;
  [k: string]: unknown;
}
/**
 * Metadata Axiom Desktop attaches beneath the standard ACP `_meta.axiom`
 * namespace when submitting a user message.
 */
export interface PromptMetadata {
  clientItemId: string;
  [k: string]: unknown;
}
/**
 * Durable identity attached beneath `_meta.axiom` on standard ACP
 * notifications. Content still belongs exclusively to standard ACP; this
 * metadata lets a client reconcile it with an authoritative timeline page.
 */
export interface DeliveryMetadata {
  threadRevision: number;
  lastTimelineSequence: number;
  timelineItemId?: string | null;
  clientItemId?: string | null;
  [k: string]: unknown;
}
export interface ListThreadsRequest {
  query?: string | null;
  includeArchived?: boolean;
  limit?: number | null;
  /**
   * Opaque cursor returned by the previous page.
   */
  cursor?: string | null;
  [k: string]: unknown;
}
export interface ListThreadsResponse {
  threads: ThreadSummary[];
  nextCursor?: string | null;
  [k: string]: unknown;
}
export interface ThreadSummary {
  threadId: string;
  title?: string | null;
  cwd: string;
  origin: string;
  profile: string;
  selectedModel?: string | null;
  thinkingLevel: string;
  lifecycle: ThreadLifecycle;
  archived: boolean;
  revision: number;
  lastTimelineSequence: number;
  createdAt: string;
  updatedAt: string;
  [k: string]: unknown;
}
export interface GetThreadTimelineRequest {
  threadId: string;
  afterSequence?: number | null;
  limit?: number | null;
  [k: string]: unknown;
}
export interface GetThreadTimelineResponse {
  thread: ThreadSummary;
  items: TimelineItem[];
  nextCursor?: number | null;
  [k: string]: unknown;
}
export interface TimelineItem {
  id: string;
  threadId: string;
  turnId?: string | null;
  sequence: number;
  kind: TimelineItemKind;
  status: TimelineItemStatus;
  clientItemId?: string | null;
  externalId?: string | null;
  content: string;
  metadata: unknown;
  createdAt: string;
  updatedAt: string;
  [k: string]: unknown;
}
export interface DeletePreviewRequest {
  threadIds: string[];
  [k: string]: unknown;
}
export interface DeletePreviewResponse {
  confirmationToken: string;
  expiresAtUnixSeconds: number;
  threads: ThreadSummary[];
  [k: string]: unknown;
}
export interface DeleteConfirmRequest {
  confirmationToken: string;
  threadIds: string[];
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
  shortLabel: string;
  providerId: string;
  providerLabel: string;
  upstreamModel: string;
  thinkingLevels: string[];
  [k: string]: unknown;
}
export interface GetProfilePreferencesRequest {
  [k: string]: unknown;
}
export interface SetProfilePreferencesRequest {
  model?: string | null;
  thinkingLevel?: string | null;
  [k: string]: unknown;
}
export interface ProfilePreferencesResponse {
  preferences: ProfilePreferences1;
  [k: string]: unknown;
}
export interface ProfilePreferences1 {
  model?: string | null;
  thinkingLevel: string;
  updatedAt: string;
  [k: string]: unknown;
}
export interface ListCollectionsRequest {
  [k: string]: unknown;
}
export interface CreateCollectionRequest {
  name: string;
  [k: string]: unknown;
}
export interface RenameCollectionRequest {
  collectionId: string;
  name: string;
  [k: string]: unknown;
}
export interface SetCollectionCollapsedRequest {
  collectionId: string;
  collapsed: boolean;
  [k: string]: unknown;
}
export interface MoveCollectionRequest {
  collectionId: string;
  position: number;
  [k: string]: unknown;
}
export interface DeleteCollectionRequest {
  collectionId: string;
  [k: string]: unknown;
}
export interface AssignThreadCollectionRequest {
  threadId: string;
  collectionId?: string | null;
  [k: string]: unknown;
}
export interface CollectionStateResponse {
  state: CollectionState;
  [k: string]: unknown;
}
export interface CollectionState {
  revision: number;
  collections: Collection[];
  [k: string]: unknown;
}
export interface Collection {
  id: string;
  name: string;
  collapsed: boolean;
  position: number;
  threadIds: string[];
  createdAt: string;
  updatedAt: string;
  [k: string]: unknown;
}
export interface AssignThreadCollectionResponse {
  state: CollectionState;
  threadRevision: number;
  lastTimelineSequence: number;
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
  /**
   * Monotonic within one agent runtime. Clients must ignore account states
   * whose revision is not newer than the state they already applied.
   */
  revision: number;
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
  threadId: string;
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
  attestationGeneration: number;
  hardExpiresAtUnixSeconds: number;
  modelKeyFingerprint: string;
  tlsSpkiFingerprint?: string | null;
  checks: EvidenceCheck[];
  providerClaims: EvidenceClaim[];
  workloadManifest?: string | null;
  workloadIdentityAllowlistConfigured: boolean;
  approvedWorkloadIdentity: boolean;
  [k: string]: unknown;
}
export interface SecurityStatus {
  state: SecurityState;
  detail?: string | null;
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
  threadId: string;
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
  threadRevision?: number | null;
  lastTimelineSequence?: number | null;
  event: ActivityEvent;
  [k: string]: unknown;
}
export interface ExtensionError {
  code: ExtensionErrorCode;
  message: string;
  retryable: boolean;
  correlationId?: string | null;
  [k: string]: unknown;
}
