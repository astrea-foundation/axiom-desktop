/** Runtime-neutral, structural views projected by host adapters. No native transport. */
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

export type AccountState = "starting" | "signed_out" | "valid" | "expired" | "unavailable";

export type LoginMethod = "passkey" | "google" | "password" | "ethereum_wallet";

export type SecurityState =
  "unverified" | "verifying" | "unattested_development" | "verified" | "degraded" | "outdated" | "failed";

export interface DesktopMcpResponse {
  revision: number;
  servers: DesktopMcpServer[];
  selectedTools: string[];
  [k: string]: unknown;
}

export interface DesktopMcpServer {
  name: string;
  command: string;
  args: string[];
  enabled: boolean;
  environmentKeys: string[];
  credentialId?: string | null;
  tools: DesktopMcpTool[];
  status: string;
  error?: string | null;
  [k: string]: unknown;
}

export interface DesktopMcpTool {
  name: string;
  description: string;
  schemaHash: string;
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

export interface UsageSummaryRequest {
  period?: "week" | "month" | "all_time";
  /**
   * IANA timezone for calendar boundaries. Omitted clients retain UTC/all-time behavior.
   */
  timezone?: string | null;
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

export interface PendingInteraction {
  id: string;
  requestId: number | string;
  sessionId: string | null;
  kind: "permission" | "elicitation";
  payload: unknown;
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

export interface PromptResult {
  stopReason: "end_turn" | "cancelled" | "max_tokens" | "refusal" | string;
}

export type ToolActivity = {
  callId: string;
  name?: string;
  title: string;
  kind: string;
  status: string;
  input: unknown;
  content: unknown[];
  locations: unknown[];
};

export type ClientTimelineItem = {
  id: string;
  turnId?: string;
  clientItemId?: string;
  sequence?: number;
  kind: "user" | "assistant" | "reasoning" | "tool" | "plan" | "activity" | "error";
  text: string;
  status?: string;
  /// True only when the authoritative durable item carries evidence that its
  /// exact terminal provider response was verified. Session preflight state
  /// is deliberately not promoted to this per-response claim.
  terminalVerified?: boolean;
  finishReason?: string;
  raw?: unknown;
  tool?: ToolActivity;
};

export interface ThreadSettings {
  model: string;
  thinkingLevel: string;
  permissionProfile: string;
}

export interface ClientSessionState {
  sessionId: string;
  title: string | null;
  cwd: string;
  settings: ThreadSettings | null;
  security: SecurityStatus | null;
  /** Public report from the last explicit/local preflight; never response-receipt proof. */
  securityEvidence?: SecurityEvidence | null;
  securityVerificationPending?: boolean;
  securityVerificationError?: string | null;
  contextUsage?: ContextUsage | null;
  requestUsage?: RequestUsage[];
  modes: SessionMode[];
  currentModeId: string | null;
  configOptions: SessionConfigOption[];
  timeline: ClientTimelineItem[];
  interactions: PendingInteraction[];
  running: boolean;
  activeTurnId?: string | null;
  desktopAgent?: DesktopAgentSettings | null;
  desktopMcp?: Pick<DesktopMcpResponse, "revision" | "selectedTools"> | null;
  needsResync: boolean;
  threadRevision: number;
  lastTimelineSequence: number;
  lastMessageAt?: string | null;
  lastUserMessageAt?: string | null;
}

export interface ClientState {
  mcp?: DesktopMcpResponse | null;
  connected: boolean;
  runtimeInstanceId: string | null;
  lastSequence: number;
  sessions: Record<string, ClientSessionState>;
  catalog: ThreadSummary[];
  collections: CollectionState;
  preferences: ProfilePreferences | null;
  account: AccountStatus | null;
  billing: BillingStatus | null;
  diagnostic: string;
  error: string | null;
}

export type PermissionOutcome =
  | { outcome: "selected"; optionId: string }
  | { outcome: "cancelled" };

export type ElicitationOutcome =
  | { action: "accept"; content: Record<string, string | string[]> }
  | { action: "decline" }
  | { action: "cancel" };

