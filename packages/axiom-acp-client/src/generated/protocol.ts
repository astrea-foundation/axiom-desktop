/* Generated from protocol/axiom-acp-extension/v0.2/schema.json. Do not edit. */

export type DesktopAgentPermission = "approve_commands" | "full_access";
export type ThreadLifecycle =
  "ready" | "running" | "waiting_for_approval" | "waiting_for_answer" | "compacting" | "closed";
export type PromptAttachment =
  | {
      name: string;
      text: string;
      kind: "text";
    }
  | {
      name: string;
      image: ImageContent;
      kind: "image";
    }
  | {
      name: string;
      file: FileContent;
      kind: "file";
    };
export type TimelineItemKind = "user_message" | "assistant_message" | "reasoning" | "tool_call" | "plan" | "notice";
export type TimelineItemStatus = "pending" | "in_progress" | "completed" | "cancelled" | "failed" | "interrupted";
export type AccountState = "starting" | "signed_out" | "valid" | "expired" | "unavailable";
export type LoginMethod = "passkey" | "google" | "password" | "ethereum_wallet";
export type SecurityState =
  "unverified" | "verifying" | "unattested_development" | "verified" | "degraded" | "outdated" | "failed";
export type ActivityEvent =
  | {
      turnId?: string | null;
      kind: "active_turn_changed";
      [k: string]: unknown;
    }
  | {
      usage: ContextUsage;
      kind: "context_usage_changed";
      [k: string]: unknown;
    }
  | {
      usage: RequestUsage;
      kind: "request_usage_changed";
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
      status: BillingStatus;
      kind: "billing_changed";
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
  configureDesktopAgentRequest: ConfigureDesktopAgentRequest;
  configureDesktopAgentResponse: ConfigureDesktopAgentResponse;
  capabilities: ExtensionCapabilities;
  desktopBootstrapRequest: DesktopBootstrapRequest;
  desktopBootstrapResponse: DesktopBootstrapResponse;
  promptMetadata: PromptMetadata;
  getAttachmentsRequest: GetAttachmentsRequest;
  getAttachmentsResponse: GetAttachmentsResponse;
  deliveryMetadata: DeliveryMetadata;
  listThreadsRequest: ListThreadsRequest;
  listThreadsResponse: ListThreadsResponse;
  renameThreadRequest: RenameThreadRequest;
  renameThreadResponse: RenameThreadResponse;
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
  nativeLoginStartRequest: NativeLoginStartRequest;
  nativeLoginStartResponse: NativeLoginStartResponse;
  nativeLoginCompleteRequest: NativeLoginCompleteRequest;
  nativeLoginCancelRequest: NativeLoginCancelRequest;
  logoutRequest: LogoutRequest;
  billingStatusRequest: BillingStatusRequest;
  billingStatusResponse: BillingStatusResponse;
  giftCodeRedeemRequest: GiftCodeRedeemRequest;
  giftCodeRedeemResponse: GiftCodeRedeemResponse;
  usageSummaryRequest: UsageSummaryRequest;
  usageSummaryResponse: UsageSummaryResponse;
  apiKeyListRequest: ApiKeyListRequest;
  apiKeyListResponse: ApiKeyListResponse;
  apiKeyCreateRequest: ApiKeyCreateRequest;
  apiKeyCreatedResponse: ApiKeyCreatedResponse;
  apiKeyRevokeRequest: ApiKeyRevokeRequest;
  apiKeyRevokeResponse: ApiKeyRevokeResponse;
  prewarmSecurityRequest: PrewarmSecurityRequest;
  verifySecurityRequest: VerifySecurityRequest;
  verifySecurityResponse: VerifySecurityResponse;
  compactRequest: CompactRequest;
  compactResponse: CompactResponse;
  steerTurnRequest: SteerTurnRequest;
  steerTurnResponse: SteerTurnResponse;
  eventNotification: EventNotification;
  error: ExtensionError;
  metadata: {
    [k: string]: unknown;
  };
  [k: string]: unknown;
}
export interface ConfigureDesktopAgentRequest {
  threadId: string;
  expectedRevision: number;
  enabled: boolean;
  permission: DesktopAgentPermission;
  /**
   * None resets to the application-owned per-thread directory. A custom
   * directory must be an absolute, existing directory on this machine.
   */
  workingDirectory?: string | null;
}
export interface ConfigureDesktopAgentResponse {
  thread: ThreadSummary;
  agent: DesktopAgentSettings;
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
  /**
   * Latest actual message activity, absent for message-free threads.
   */
  lastMessageAt?: string | null;
  /**
   * Latest user submission. Streaming output does not advance sidebar order.
   */
  lastUserMessageAt?: string | null;
  [k: string]: unknown;
}
export interface DesktopAgentSettings {
  enabled: boolean;
  permission: DesktopAgentPermission;
  workingDirectory: string;
  defaultWorkingDirectory: string;
  usesDefaultDirectory: boolean;
  revision: number;
}
export interface ExtensionCapabilities {
  protocolVersion: string;
  features: FeatureVersions;
  runtimeInstanceId?: string | null;
  [k: string]: unknown;
}
export interface FeatureVersions {
  desktopChat?: number;
  desktopAgent?: number;
  threadCatalog?: number;
  timeline?: number;
  modelCatalog?: number;
  profilePreferences?: number;
  collections?: number;
  account?: number;
  billing?: number;
  usage?: number;
  securityEvidence?: number;
  webConsent?: number;
  steering?: number;
  messageRevision?: number;
  attachments?: number;
  compaction?: number;
  activity?: number;
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
   * Canonical parent of application-owned, per-thread desktop workspaces.
   */
  chatCwd?: string | null;
  /**
   * Default permission profile for a new desktop chat (Agent starts off).
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
  /**
   * Explicit per-message desktop consent to external web tools. Never
   * inherited from an earlier turn, saved thread, or permission profile.
   */
  webEnabled?: boolean;
  /**
   * Bind queued input to the exact locally approved Agent configuration.
   */
  agentRevision?: number | null;
  /**
   * Replace local history starting at this user message before the normal
   * native E2EE turn. Never interpreted by the hosted backend.
   */
  revision?: PromptRevision | null;
  attachments?: PromptAttachment[];
  [k: string]: unknown;
}
export interface PromptRevision {
  userItemId: string;
  expectedRevision: number;
}
export interface ImageContent {
  mimeType: string;
  /**
   * Canonical base64, without a data-URL prefix. Remote URLs are not images.
   */
  data: string;
}
/**
 * Original file bytes. Extraction belongs to the attested provider, not Axiom.
 */
export interface FileContent {
  name: string;
  mimeType: string;
  data: string;
}
export interface GetAttachmentsRequest {
  threadId: string;
  userItemId: string;
  [k: string]: unknown;
}
export interface GetAttachmentsResponse {
  attachments: PromptAttachment[];
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
  /**
   * Latest actual message activity; operational updates do not advance it.
   */
  lastMessageAt?: string | null;
  /**
   * Latest user submission. Streaming output does not advance sidebar order.
   */
  lastUserMessageAt?: string | null;
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
export interface RenameThreadRequest {
  threadId: string;
  title: string;
  [k: string]: unknown;
}
export interface RenameThreadResponse {
  thread: ThreadSummary;
  [k: string]: unknown;
}
export interface GetThreadTimelineRequest {
  threadId: string;
  afterSequence?: number | null;
  afterRequestId?: string | null;
  limit?: number | null;
  [k: string]: unknown;
}
export interface GetThreadTimelineResponse {
  thread: ThreadSummary;
  desktopAgent?: DesktopAgentSettings | null;
  /**
   * Active durable turn, including when a client missed its start event.
   */
  activeTurnId?: string | null;
  contextUsage?: ContextUsage | null;
  requestUsage?: RequestUsage[];
  items: TimelineItem[];
  nextCursor?: number | null;
  nextRequestUsageCursor?: string | null;
  [k: string]: unknown;
}
/**
 * Last provider-reported conversation request, not a measurement of the next
 * request or a cumulative billing total. Auxiliary title/compaction requests
 * never replace this report. Limits describe the model used by that request.
 */
export interface ContextUsage {
  inputTokens: number;
  outputTokens: number;
  modelId: string;
  reportedAt: string;
  contextWindowTokens?: number | null;
  autoCompactThresholdTokens?: number | null;
  [k: string]: unknown;
}
/**
 * Decimal strings remain exact in JavaScript, on disk, and over ACP.
 * A settled charge does not imply an authenticated/complete model response.
 */
export interface RequestUsage {
  requestId: string;
  modelId: string;
  providerId: string;
  contextWindowTokens?: number | null;
  autoCompactThresholdTokens?: number | null;
  turnId?: string | null;
  purpose?: "conversation" | "title" | "compaction";
  state?: "running" | "completed" | "failed" | "cancelled";
  completeness?: "unknown" | "live" | "final" | "partial";
  inputTokens?: string | null;
  cachedInputTokens?: string | null;
  outputTokens?: string | null;
  reasoningTokens?: string | null;
  costMicrousd?: string | null;
  settled?: boolean;
  responseVerified?: boolean;
  startedAtMs?: string;
  finishedAtMs?: string | null;
  errorCode?: string | null;
  finishReason?: string | null;
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
  contextWindowTokens: number;
  maxOutputTokens: number;
  supportsImages?: boolean;
  fileMimeTypes?: string[];
  autoCompactThresholdTokens: number;
  inputPriceMicrousdPerMillionTokens?: number | null;
  outputPriceMicrousdPerMillionTokens?: number | null;
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
  state: AccountState;
  account?: AccountProfile | null;
  session?: NativeSession | null;
  detail?: string | null;
  [k: string]: unknown;
}
export interface AccountProfile {
  id: string;
  displayName?: string | null;
  avatarUrl?: string | null;
  verifiedEmail?: string | null;
  linkedMethods: LoginMethod[];
  [k: string]: unknown;
}
export interface NativeSession {
  expiresAt: string;
  credentialStore: string;
  [k: string]: unknown;
}
export interface NativeLoginStartRequest {
  methodHint?: LoginMethod | null;
  [k: string]: unknown;
}
export interface NativeLoginStartResponse {
  login: NativeLoginState;
  [k: string]: unknown;
}
export interface NativeLoginState {
  loginId: string;
  userCode: string;
  authorizationUrl: string;
  browserOpened: boolean;
  expiresAt: string;
  [k: string]: unknown;
}
export interface NativeLoginCompleteRequest {
  loginId: string;
  [k: string]: unknown;
}
export interface NativeLoginCancelRequest {
  loginId: string;
  [k: string]: unknown;
}
export interface LogoutRequest {
  [k: string]: unknown;
}
export interface BillingStatusRequest {
  [k: string]: unknown;
}
export interface BillingStatusResponse {
  status: BillingStatus;
  [k: string]: unknown;
}
export interface BillingStatus {
  /**
   * Monotonic within one agent runtime; clients ignore older projections.
   */
  revision: number;
  postedMicrousd: number;
  availableMicrousd: number;
  ledgerSequence: number;
  trialMicrousd: number;
  paidMicrousd: number;
  paymentReviewRequired?: boolean;
  currency: string;
  paymentAccount?: PaymentAccount | null;
  zecUsdQuote?: ZecUsdQuote | null;
  [k: string]: unknown;
}
/**
 * Payment-service values remain exact decimal strings across Rust/ACP/JS.
 * Nested fields retain the backend's `snake_case` HTTP contract.
 */
export interface PaymentAccount {
  network: string;
  asset: string;
  conversion_status: string;
  valuation_enabled?: boolean;
  state: string;
  address?: string | null;
  payment_uri?: string | null;
  monitoring_status: string;
  required_confirmations?: string | null;
  confirmed_zatoshis: string;
  confirming_zatoshis: string;
  review_required: boolean;
  deposits: DepositReceipt[];
}
export interface DepositReceipt {
  id: string;
  amount_zatoshis: string;
  state: string;
  object_version: string;
  confirmations: string;
  required_confirmations: string;
  review_required: boolean;
  observed_at: string;
  valuation_status?: string | null;
  credit_microusd?: string | null;
  price_microusd_per_zec?: string | null;
  price_source?: string | null;
  priced_at?: string | null;
}
/**
 * Indicative live market price; deposit credit uses its confirmation-time rate.
 */
export interface ZecUsdQuote {
  price_microusd_per_zec: string;
  source: string;
  as_of: string;
  expires_at: string;
}
export interface GiftCodeRedeemRequest {
  code: string;
}
export interface GiftCodeRedeemResponse {
  creditedMicrousd: number;
  alreadyRedeemed: boolean;
  status: BillingStatus;
  [k: string]: unknown;
}
export interface UsageSummaryRequest {
  period?: "week" | "month" | "all_time";
  /**
   * IANA timezone for calendar boundaries. Omitted clients retain UTC/all-time behavior.
   */
  timezone?: string | null;
  [k: string]: unknown;
}
export interface UsageSummaryResponse {
  summary: UsageSummary;
  [k: string]: unknown;
}
export interface UsageSummary {
  period: string;
  totalCostMicrousd: string;
  models: ModelSpend[];
  [k: string]: unknown;
}
export interface ModelSpend {
  provider: string;
  modelId?: string | null;
  modelName?: string | null;
  /**
   * Exact USD millionths; never an IEEE-754 amount.
   */
  costMicrousd: string;
  [k: string]: unknown;
}
export interface ApiKeyListRequest {
  [k: string]: unknown;
}
export interface ApiKeyListResponse {
  keys: ApiKeyRecord[];
}
export interface ApiKeyRecord {
  id: string;
  name: string;
  scopes: string[];
  createdAt: string;
  lastUsedAt?: string | null;
  expiresAt?: string | null;
  revokedAt?: string | null;
  usageStartedAt: string;
  usage: ApiKeyUsage;
}
export interface ApiKeyUsage {
  requestCount: string;
  inputTokens: string;
  cachedInputTokens: string;
  outputTokens: string;
  costMicrousd: string;
}
export interface ApiKeyCreateRequest {
  name: string;
}
export interface ApiKeyCreatedResponse {
  key: ApiKeyRecord;
  token: string;
  [k: string]: unknown;
}
export interface ApiKeyRevokeRequest {
  id: string;
}
export interface ApiKeyRevokeResponse {
  [k: string]: unknown;
}
/**
 * Model-only warmup: no thread, prompt, inference, or implicit consent.
 */
export interface PrewarmSecurityRequest {
  modelId: string;
  [k: string]: unknown;
}
export interface VerifySecurityRequest {
  threadId: string;
  acceptOutdatedTee?: boolean;
  modelId?: string | null;
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
  /**
   * Relay lease generation, when the provider protocol uses relay leases.
   */
  attestationGeneration?: number | null;
  hardExpiresAtUnixSeconds: number;
  modelKeyFingerprint: string;
  tlsSpkiFingerprint?: string | null;
  checks: EvidenceCheck[];
  providerClaims: EvidenceClaim[];
  workloadManifest?: string | null;
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
export interface SteerTurnRequest {
  agentRevision?: number | null;
  threadId: string;
  expectedTurnId: string;
  clientItemId: string;
  text: string;
  webEnabled: boolean;
}
export interface SteerTurnResponse {
  turnId: string;
  clientItemId: string;
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
