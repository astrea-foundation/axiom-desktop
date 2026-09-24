#![forbid(unsafe_code)]

//! Versioned first-party extensions layered on top of standard ACP.
//!
//! These types are deliberately presentation-neutral. Standard ACP remains the
//! primary representation for prompts, tool calls, plans, permissions, modes,
//! configuration, and cancellation. This crate only describes Axiom-specific
//! control and state that standard ACP cannot faithfully carry.

use agent_client_protocol::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};
pub use axiom_inference::RequestUsage;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const PROTOCOL_VERSION: &str = "0.2";
const PROTOCOL_FAMILY: &str = "0";
pub const META_KEY: &str = "axiom";
pub const MAX_SESSION_IDS: usize = 1_000;
pub const MAX_QUERY_BYTES: usize = 512;
pub const MAX_FOCUS_BYTES: usize = 4 * 1024;
pub const MAX_EVIDENCE_CLAIMS: usize = 128;
pub const MAX_EVIDENCE_VALUE_BYTES: usize = 8 * 1024;
pub const MAX_WORKLOAD_MANIFEST_BYTES: usize = 1024 * 1024;
pub const MAX_IDENTIFIER_BYTES: usize = 512;
pub const MAX_COLLECTION_NAME_BYTES: usize = 256;
pub const MAX_TIMELINE_PAGE_SIZE: u32 = 500;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionCapabilities {
    pub protocol_version: String,
    pub features: FeatureVersions,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_instance_id: Option<String>,
}

impl ExtensionCapabilities {
    #[must_use]
    pub fn agent(runtime_instance_id: impl Into<String>) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION.into(),
            features: FeatureVersions::all(),
            runtime_instance_id: Some(runtime_instance_id.into()),
        }
    }

    #[must_use]
    pub fn client() -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION.into(),
            features: FeatureVersions::all(),
            runtime_instance_id: None,
        }
    }

    #[must_use]
    pub fn is_compatible(&self) -> bool {
        self.protocol_version
            .split('.')
            .next()
            .is_some_and(|family| family == PROTOCOL_FAMILY)
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct FeatureVersions {
    pub desktop_chat: u16,
    pub desktop_agent: u16,
    pub thread_catalog: u16,
    pub timeline: u16,
    pub model_catalog: u16,
    pub profile_preferences: u16,
    pub collections: u16,
    pub account: u16,
    pub billing: u16,
    pub usage: u16,
    pub security_evidence: u16,
    pub web_consent: u16,
    pub steering: u16,
    pub message_revision: u16,
    pub attachments: u16,
    pub compaction: u16,
    pub activity: u16,
}

impl FeatureVersions {
    #[must_use]
    pub const fn all() -> Self {
        Self {
            desktop_chat: 1,
            desktop_agent: 1,
            thread_catalog: 1,
            timeline: 2,
            model_catalog: 1,
            profile_preferences: 1,
            collections: 1,
            account: 2,
            billing: 3,
            usage: 1,
            security_evidence: 4,
            web_consent: 1,
            steering: 1,
            message_revision: 1,
            attachments: 2,
            compaction: 1,
            activity: 1,
        }
    }
}

#[derive(
    Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcRequest,
)]
#[request(method = "_axiom/desktop/bootstrap", response = DesktopBootstrapResponse)]
pub struct DesktopBootstrapRequest {}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct DesktopBootstrapResponse {
    /// Stable front-end identifier selected when the sidecar was launched.
    pub frontend: String,
    /// Canonical parent of application-owned, per-thread desktop workspaces.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_cwd: Option<String>,
    /// Default permission profile for a new desktop chat (Agent starts off).
    pub permission_profile: String,
    /// Settings `AxiomCLI` will apply to the next thread. On a clean profile the
    /// thinking level is `medium`; afterward these are the last-used values.
    pub new_thread_settings: ProfilePreferences,
}

/// Metadata Axiom Desktop attaches beneath the standard ACP `_meta.axiom`
/// namespace when submitting a user message.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PromptMetadata {
    pub client_item_id: String,
    /// Explicit per-message desktop consent to external web tools. Never
    /// inherited from an earlier turn, saved thread, or permission profile.
    #[serde(default)]
    pub web_enabled: bool,
    /// Bind queued input to the exact locally approved Agent configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_revision: Option<u64>,
    /// Replace local history starting at this user message before the normal
    /// native E2EE turn. Never interpreted by the hosted backend.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<PromptRevision>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<axiom_inference::PromptAttachment>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PromptRevision {
    pub user_item_id: String,
    pub expected_revision: u64,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DesktopAgentPermission {
    #[default]
    ApproveCommands,
    FullAccess,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DesktopAgentSettings {
    pub enabled: bool,
    pub permission: DesktopAgentPermission,
    pub working_directory: String,
    pub default_working_directory: String,
    pub uses_default_directory: bool,
    pub revision: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcRequest)]
#[request(method = "_axiom/desktop/agent/configure", response = ConfigureDesktopAgentResponse)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigureDesktopAgentRequest {
    pub thread_id: String,
    pub expected_revision: u64,
    pub enabled: bool,
    pub permission: DesktopAgentPermission,
    /// None resets to the application-owned per-thread directory. A custom
    /// directory must be an absolute, existing directory on this machine.
    #[serde(default)]
    pub working_directory: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct ConfigureDesktopAgentResponse {
    pub thread: ThreadSummary,
    pub agent: DesktopAgentSettings,
}

/// Durable identity attached beneath `_meta.axiom` on standard ACP
/// notifications. Content still belongs exclusively to standard ACP; this
/// metadata lets a client reconcile it with an authoritative timeline page.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryMetadata {
    pub thread_revision: u64,
    pub last_timeline_sequence: u64,
    /// Latest actual message activity; operational updates do not advance it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_message_at: Option<String>,
    /// Latest user submission. Streaming output does not advance sidebar order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_user_message_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeline_item_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_item_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionErrorCode {
    UnsupportedFeature,
    InvalidRequest,
    InvalidState,
    NotFound,
    Conflict,
    PermissionDenied,
    AuthenticationRequired,
    SecurityVerificationFailed,
    Cancelled,
    Expired,
    ProviderUnavailable,
    Internal,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionError {
    pub code: ExtensionErrorCode,
    pub message: String,
    pub retryable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ThreadLifecycle {
    Ready,
    Running,
    WaitingForApproval,
    WaitingForAnswer,
    Compacting,
    Closed,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ThreadSummary {
    pub thread_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub cwd: String,
    pub origin: String,
    pub profile: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_model: Option<String>,
    pub thinking_level: String,
    pub lifecycle: ThreadLifecycle,
    pub archived: bool,
    pub revision: u64,
    pub last_timeline_sequence: u64,
    pub created_at: String,
    pub updated_at: String,
    /// Latest actual message activity, absent for message-free threads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_message_at: Option<String>,
    /// Latest user submission. Streaming output does not advance sidebar order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_user_message_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TimelineItemKind {
    UserMessage,
    AssistantMessage,
    Reasoning,
    ToolCall,
    Plan,
    Notice,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TimelineItemStatus {
    Pending,
    InProgress,
    Completed,
    Cancelled,
    Failed,
    Interrupted,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TimelineItem {
    pub id: String,
    pub thread_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    pub sequence: u64,
    pub kind: TimelineItemKind,
    pub status: TimelineItemStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_item_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_id: Option<String>,
    pub content: String,
    pub metadata: serde_json::Value,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SecurityState {
    Unverified,
    Verifying,
    UnattestedDevelopment,
    Verified,
    Degraded,
    Outdated,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SecurityStatus {
    pub state: SecurityState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceCheck {
    pub name: String,
    pub passed: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceClaim {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SecurityEvidence {
    pub status: SecurityStatus,
    pub provider_id: String,
    pub model_id: String,
    pub attestation_protocol: String,
    pub e2ee_protocol: String,
    pub e2ee_encryption_version: u16,
    pub trust_policy_version: String,
    pub verified_at_unix_seconds: u64,
    /// Relay lease generation, when the provider protocol uses relay leases.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attestation_generation: Option<u64>,
    pub hard_expires_at_unix_seconds: u64,
    pub model_key_fingerprint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls_spki_fingerprint: Option<String>,
    pub checks: Vec<EvidenceCheck>,
    pub provider_claims: Vec<EvidenceClaim>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workload_manifest: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub id: String,
    pub label: String,
    pub short_label: String,
    pub provider_id: String,
    pub provider_label: String,
    pub upstream_model: String,
    pub thinking_levels: Vec<String>,
    pub context_window_tokens: u32,
    pub max_output_tokens: u32,
    #[serde(default)]
    pub supports_images: bool,
    #[serde(default)]
    pub file_mime_types: Vec<String>,
    pub auto_compact_threshold_tokens: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_price_microusd_per_million_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_price_microusd_per_million_tokens: Option<u64>,
}

/// Last provider-reported conversation request, not a measurement of the next
/// request or a cumulative billing total. Auxiliary title/compaction requests
/// never replace this report. Limits describe the model used by that request.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ContextUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub model_id: String,
    pub reported_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_compact_threshold_tokens: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProfilePreferences {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub thinking_level: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Collection {
    pub id: String,
    pub name: String,
    pub collapsed: bool,
    pub position: i64,
    pub thread_ids: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CollectionState {
    pub revision: u64,
    pub collections: Vec<Collection>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AccountStatus {
    /// Monotonic within one agent runtime. Clients must ignore account states
    /// whose revision is not newer than the state they already applied.
    pub revision: u64,
    pub state: AccountState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<AccountProfile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<NativeSession>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AccountState {
    Starting,
    SignedOut,
    Valid,
    Expired,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AccountProfile {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verified_email: Option<String>,
    pub linked_methods: Vec<LoginMethod>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LoginMethod {
    Passkey,
    Google,
    Password,
    EthereumWallet,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NativeSession {
    pub expires_at: String,
    pub credential_store: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NativeLoginState {
    pub login_id: String,
    pub user_code: String,
    pub authorization_url: String,
    pub browser_opened: bool,
    pub expires_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BillingStatus {
    /// Monotonic within one agent runtime; clients ignore older projections.
    pub revision: u64,
    pub posted_microusd: i64,
    pub available_microusd: i64,
    pub ledger_sequence: u64,
    pub trial_microusd: u64,
    pub paid_microusd: i64,
    #[serde(default)]
    pub payment_review_required: bool,
    pub currency: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payment_account: Option<PaymentAccount>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zec_usd_quote: Option<ZecUsdQuote>,
}

/// Indicative live market price; deposit credit uses its confirmation-time rate.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ZecUsdQuote {
    pub price_microusd_per_zec: String,
    pub source: String,
    pub as_of: String,
    pub expires_at: String,
}

/// Payment-service values remain exact decimal strings across Rust/ACP/JS.
/// Nested fields retain the backend's `snake_case` HTTP contract.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PaymentAccount {
    pub network: String,
    pub asset: String,
    pub conversion_status: String,
    #[serde(default)]
    pub valuation_enabled: bool,
    pub state: String,
    pub address: Option<String>,
    pub payment_uri: Option<String>,
    pub monitoring_status: String,
    pub required_confirmations: Option<String>,
    pub confirmed_zatoshis: String,
    pub confirming_zatoshis: String,
    pub review_required: bool,
    pub deposits: Vec<DepositReceipt>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DepositReceipt {
    pub id: String,
    pub amount_zatoshis: String,
    pub state: String,
    pub object_version: String,
    pub confirmations: String,
    pub required_confirmations: String,
    pub review_required: bool,
    pub observed_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valuation_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credit_microusd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price_microusd_per_zec: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priced_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ActivityEvent {
    ActiveTurnChanged {
        #[serde(rename = "turnId")]
        turn_id: Option<String>,
    },
    ContextUsageChanged {
        usage: ContextUsage,
    },
    RequestUsageChanged {
        usage: RequestUsage,
    },
    SecurityChanged {
        status: SecurityStatus,
    },
    AccountChanged {
        status: AccountStatus,
    },
    BillingChanged {
        status: BillingStatus,
    },
    BackgroundTask {
        task_id: String,
        state: String,
    },
    WorkspaceChanged {
        paths: Vec<String>,
    },
    Compaction {
        phase: CompactionPhase,
        detail: Option<String>,
    },
    CollectionsChanged {
        state: CollectionState,
    },
    ProfilePreferencesChanged {
        preferences: ProfilePreferences,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CompactionPhase {
    Started,
    Summarizing,
    ReplacingContext,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, JsonRpcNotification)]
#[notification(method = "_axiom/event")]
#[serde(rename_all = "camelCase")]
pub struct EventNotification {
    pub runtime_instance_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub sequence: u64,
    /// RFC 3339 time recorded by `AxiomCLI` when the outward event was created.
    pub occurred_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_timeline_sequence: Option<u64>,
    pub event: ActivityEvent,
}

#[derive(
    Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcRequest,
)]
#[request(method = "_axiom/thread/list", response = ListThreadsResponse)]
#[serde(rename_all = "camelCase")]
pub struct ListThreadsRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    #[serde(default)]
    pub include_archived: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// Opaque cursor returned by the previous page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct ListThreadsResponse {
    pub threads: Vec<ThreadSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcRequest)]
#[request(method = "_axiom/thread/rename", response = RenameThreadResponse)]
#[serde(rename_all = "camelCase")]
pub struct RenameThreadRequest {
    pub thread_id: String,
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct RenameThreadResponse {
    pub thread: ThreadSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcRequest)]
#[request(method = "_axiom/thread/timeline", response = GetThreadTimelineResponse)]
#[serde(rename_all = "camelCase")]
pub struct GetThreadTimelineRequest {
    pub thread_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_sequence: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct GetThreadTimelineResponse {
    pub thread: ThreadSummary,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub desktop_agent: Option<DesktopAgentSettings>,
    /// Active durable turn, including when a client missed its start event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_turn_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_usage: Option<ContextUsage>,
    #[serde(default)]
    pub request_usage: Vec<RequestUsage>,
    pub items: Vec<TimelineItem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_request_usage_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcRequest)]
#[request(method = "_axiom/thread/attachments", response = GetAttachmentsResponse)]
#[serde(rename_all = "camelCase")]
pub struct GetAttachmentsRequest {
    pub thread_id: String,
    pub user_item_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct GetAttachmentsResponse {
    pub attachments: Vec<axiom_inference::PromptAttachment>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcRequest)]
#[request(method = "_axiom/thread/delete_preview", response = DeletePreviewResponse)]
#[serde(rename_all = "camelCase")]
pub struct DeletePreviewRequest {
    pub thread_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct DeletePreviewResponse {
    pub confirmation_token: String,
    pub expires_at_unix_seconds: u64,
    pub threads: Vec<ThreadSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcRequest)]
#[request(method = "_axiom/thread/delete_confirm", response = DeleteConfirmResponse)]
#[serde(rename_all = "camelCase")]
pub struct DeleteConfirmRequest {
    pub confirmation_token: String,
    pub thread_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct DeleteConfirmResponse {
    pub deleted: u32,
}

#[derive(
    Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcRequest,
)]
#[request(method = "_axiom/models/list", response = ListModelsResponse)]
pub struct ListModelsRequest {}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct ListModelsResponse {
    pub models: Vec<ModelInfo>,
}

#[derive(
    Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcRequest,
)]
#[request(method = "_axiom/profile/preferences", response = ProfilePreferencesResponse)]
pub struct GetProfilePreferencesRequest {}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcRequest)]
#[request(method = "_axiom/profile/preferences/set", response = ProfilePreferencesResponse)]
#[serde(rename_all = "camelCase")]
pub struct SetProfilePreferencesRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_level: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct ProfilePreferencesResponse {
    pub preferences: ProfilePreferences,
}

#[derive(
    Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcRequest,
)]
#[request(method = "_axiom/collection/list", response = CollectionStateResponse)]
pub struct ListCollectionsRequest {}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcRequest)]
#[request(method = "_axiom/collection/create", response = CollectionStateResponse)]
#[serde(rename_all = "camelCase")]
pub struct CreateCollectionRequest {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcRequest)]
#[request(method = "_axiom/collection/rename", response = CollectionStateResponse)]
#[serde(rename_all = "camelCase")]
pub struct RenameCollectionRequest {
    pub collection_id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcRequest)]
#[request(method = "_axiom/collection/set_collapsed", response = CollectionStateResponse)]
#[serde(rename_all = "camelCase")]
pub struct SetCollectionCollapsedRequest {
    pub collection_id: String,
    pub collapsed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcRequest)]
#[request(method = "_axiom/collection/move", response = CollectionStateResponse)]
#[serde(rename_all = "camelCase")]
pub struct MoveCollectionRequest {
    pub collection_id: String,
    pub position: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcRequest)]
#[request(method = "_axiom/collection/delete", response = CollectionStateResponse)]
#[serde(rename_all = "camelCase")]
pub struct DeleteCollectionRequest {
    pub collection_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcRequest)]
#[request(method = "_axiom/collection/assign", response = AssignThreadCollectionResponse)]
#[serde(rename_all = "camelCase")]
pub struct AssignThreadCollectionRequest {
    pub thread_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collection_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct CollectionStateResponse {
    pub state: CollectionState,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct AssignThreadCollectionResponse {
    pub state: CollectionState,
    pub thread_revision: u64,
    pub last_timeline_sequence: u64,
}

#[derive(
    Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcRequest,
)]
#[request(method = "_axiom/account/status", response = AccountStatusResponse)]
pub struct AccountStatusRequest {}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct AccountStatusResponse {
    pub status: AccountStatus,
}

#[derive(
    Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcRequest,
)]
#[request(method = "_axiom/account/native_login_start", response = NativeLoginStartResponse)]
#[serde(rename_all = "camelCase")]
pub struct NativeLoginStartRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method_hint: Option<LoginMethod>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct NativeLoginStartResponse {
    pub login: NativeLoginState,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcRequest)]
#[request(method = "_axiom/account/native_login_complete", response = AccountStatusResponse)]
#[serde(rename_all = "camelCase")]
pub struct NativeLoginCompleteRequest {
    pub login_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcRequest)]
#[request(method = "_axiom/account/native_login_cancel", response = AccountStatusResponse)]
#[serde(rename_all = "camelCase")]
pub struct NativeLoginCancelRequest {
    pub login_id: String,
}

#[derive(
    Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcRequest,
)]
#[request(method = "_axiom/account/logout", response = AccountStatusResponse)]
pub struct LogoutRequest {}

#[derive(
    Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcRequest,
)]
#[request(method = "_axiom/billing/status", response = BillingStatusResponse)]
pub struct BillingStatusRequest {}

#[derive(Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcRequest)]
#[request(method = "_axiom/billing/redeem_gift_code", response = GiftCodeRedeemResponse)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GiftCodeRedeemRequest {
    #[schemars(length(min = 1, max = 64))]
    pub code: String,
}

impl std::fmt::Debug for GiftCodeRedeemRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("GiftCodeRedeemRequest { code: [REDACTED] }")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct GiftCodeRedeemResponse {
    pub credited_microusd: u64,
    pub already_redeemed: bool,
    pub status: BillingStatus,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UsagePeriod {
    Week,
    Month,
    #[default]
    AllTime,
}

impl UsagePeriod {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Week => "week",
            Self::Month => "month",
            Self::AllTime => "all_time",
        }
    }
}

#[derive(
    Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcRequest,
)]
#[request(method = "_axiom/usage/summary", response = UsageSummaryResponse)]
pub struct UsageSummaryRequest {
    #[serde(default)]
    pub period: UsagePeriod,
    /// IANA timezone for calendar boundaries. Omitted clients retain UTC/all-time behavior.
    #[schemars(length(min = 1, max = 128))]
    pub timezone: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ModelSpend {
    pub provider: String,
    pub model_id: Option<String>,
    pub model_name: Option<String>,
    /// Exact USD millionths; never an IEEE-754 amount.
    pub cost_microusd: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UsageSummary {
    pub period: String,
    pub total_cost_microusd: String,
    pub models: Vec<ModelSpend>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct UsageSummaryResponse {
    pub summary: UsageSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApiKeyUsage {
    #[serde(alias = "request_count")]
    pub request_count: String,
    #[serde(alias = "input_tokens")]
    pub input_tokens: String,
    #[serde(alias = "cached_input_tokens")]
    pub cached_input_tokens: String,
    #[serde(alias = "output_tokens")]
    pub output_tokens: String,
    #[serde(alias = "cost_microusd")]
    pub cost_microusd: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApiKeyRecord {
    pub id: String,
    pub name: String,
    pub scopes: Vec<String>,
    #[serde(alias = "created_at")]
    pub created_at: String,
    #[serde(alias = "last_used_at")]
    pub last_used_at: Option<String>,
    #[serde(alias = "expires_at")]
    pub expires_at: Option<String>,
    #[serde(alias = "revoked_at")]
    pub revoked_at: Option<String>,
    #[serde(alias = "usage_started_at")]
    pub usage_started_at: String,
    pub usage: ApiKeyUsage,
}

#[derive(
    Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcRequest,
)]
#[request(method = "_axiom/account/api_keys", response = ApiKeyListResponse)]
pub struct ApiKeyListRequest {}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcResponse)]
#[serde(deny_unknown_fields)]
pub struct ApiKeyListResponse {
    pub keys: Vec<ApiKeyRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcRequest)]
#[request(method = "_axiom/account/api_key_create", response = ApiKeyCreatedResponse)]
#[serde(deny_unknown_fields)]
pub struct ApiKeyCreateRequest {
    pub name: String,
}

// A newly issued key is intentionally shown once, never included in account state or Debug output.
#[derive(Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcResponse)]
pub struct ApiKeyCreatedResponse {
    pub key: ApiKeyRecord,
    pub token: String,
}

impl std::fmt::Debug for ApiKeyCreatedResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiKeyCreatedResponse")
            .field("key", &self.key)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcRequest)]
#[request(method = "_axiom/account/api_key_revoke", response = ApiKeyRevokeResponse)]
#[serde(deny_unknown_fields)]
pub struct ApiKeyRevokeRequest {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcResponse)]
pub struct ApiKeyRevokeResponse {}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct BillingStatusResponse {
    pub status: BillingStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcRequest)]
#[request(method = "_axiom/security/verify", response = VerifySecurityResponse)]
#[serde(rename_all = "camelCase")]
pub struct VerifySecurityRequest {
    pub thread_id: String,
    #[serde(default)]
    pub accept_outdated_tee: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
}

/// Model-only warmup: no thread, prompt, inference, or implicit consent.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcRequest)]
#[request(method = "_axiom/security/prewarm", response = VerifySecurityResponse)]
#[serde(rename_all = "camelCase")]
pub struct PrewarmSecurityRequest {
    pub model_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct VerifySecurityResponse {
    pub evidence: Option<SecurityEvidence>,
    pub status: SecurityStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcRequest)]
#[request(method = "_axiom/compaction/start", response = CompactResponse)]
#[serde(rename_all = "camelCase")]
pub struct CompactRequest {
    pub thread_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct CompactResponse {
    pub messages_before: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcRequest)]
#[request(method = "_axiom/turn/steer", response = SteerTurnResponse)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SteerTurnRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_revision: Option<u64>,
    pub thread_id: String,
    pub expected_turn_id: String,
    pub client_item_id: String,
    pub text: String,
    pub web_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct SteerTurnResponse {
    pub turn_id: String,
    pub client_item_id: String,
}

/// Schema root used to generate the checked-in language-neutral contract.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolSchema {
    pub configure_desktop_agent_request: ConfigureDesktopAgentRequest,
    pub configure_desktop_agent_response: ConfigureDesktopAgentResponse,
    pub capabilities: ExtensionCapabilities,
    pub desktop_bootstrap_request: DesktopBootstrapRequest,
    pub desktop_bootstrap_response: DesktopBootstrapResponse,
    pub prompt_metadata: PromptMetadata,
    pub get_attachments_request: GetAttachmentsRequest,
    pub get_attachments_response: GetAttachmentsResponse,
    pub delivery_metadata: DeliveryMetadata,
    pub list_threads_request: ListThreadsRequest,
    pub list_threads_response: ListThreadsResponse,
    pub rename_thread_request: RenameThreadRequest,
    pub rename_thread_response: RenameThreadResponse,
    pub get_thread_timeline_request: GetThreadTimelineRequest,
    pub get_thread_timeline_response: GetThreadTimelineResponse,
    pub delete_preview_request: DeletePreviewRequest,
    pub delete_preview_response: DeletePreviewResponse,
    pub delete_confirm_request: DeleteConfirmRequest,
    pub delete_confirm_response: DeleteConfirmResponse,
    pub list_models_request: ListModelsRequest,
    pub list_models_response: ListModelsResponse,
    pub get_profile_preferences_request: GetProfilePreferencesRequest,
    pub set_profile_preferences_request: SetProfilePreferencesRequest,
    pub profile_preferences_response: ProfilePreferencesResponse,
    pub list_collections_request: ListCollectionsRequest,
    pub create_collection_request: CreateCollectionRequest,
    pub rename_collection_request: RenameCollectionRequest,
    pub set_collection_collapsed_request: SetCollectionCollapsedRequest,
    pub move_collection_request: MoveCollectionRequest,
    pub delete_collection_request: DeleteCollectionRequest,
    pub assign_thread_collection_request: AssignThreadCollectionRequest,
    pub collection_state_response: CollectionStateResponse,
    pub assign_thread_collection_response: AssignThreadCollectionResponse,
    pub account_status_request: AccountStatusRequest,
    pub account_status_response: AccountStatusResponse,
    pub native_login_start_request: NativeLoginStartRequest,
    pub native_login_start_response: NativeLoginStartResponse,
    pub native_login_complete_request: NativeLoginCompleteRequest,
    pub native_login_cancel_request: NativeLoginCancelRequest,
    pub logout_request: LogoutRequest,
    pub billing_status_request: BillingStatusRequest,
    pub billing_status_response: BillingStatusResponse,
    pub gift_code_redeem_request: GiftCodeRedeemRequest,
    pub gift_code_redeem_response: GiftCodeRedeemResponse,
    pub usage_summary_request: UsageSummaryRequest,
    pub usage_summary_response: UsageSummaryResponse,
    pub api_key_list_request: ApiKeyListRequest,
    pub api_key_list_response: ApiKeyListResponse,
    pub api_key_create_request: ApiKeyCreateRequest,
    pub api_key_created_response: ApiKeyCreatedResponse,
    pub api_key_revoke_request: ApiKeyRevokeRequest,
    pub api_key_revoke_response: ApiKeyRevokeResponse,
    pub prewarm_security_request: PrewarmSecurityRequest,
    pub verify_security_request: VerifySecurityRequest,
    pub verify_security_response: VerifySecurityResponse,
    pub compact_request: CompactRequest,
    pub compact_response: CompactResponse,
    pub steer_turn_request: SteerTurnRequest,
    pub steer_turn_response: SteerTurnResponse,
    pub event_notification: EventNotification,
    pub error: ExtensionError,
    pub metadata: BTreeMap<String, serde_json::Value>,
}

#[must_use]
pub fn schema_json() -> String {
    serde_json::to_string_pretty(&schemars::schema_for!(ProtocolSchema))
        .expect("protocol schema must serialize")
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn capabilities_are_versioned_and_do_not_claim_a_runtime_for_clients() {
        let client = ExtensionCapabilities::client();
        assert!(client.is_compatible());
        assert!(client.runtime_instance_id.is_none());
        assert_eq!(client.features, FeatureVersions::all());

        let mut additive = client.clone();
        additive.protocol_version = "0.9".into();
        assert!(additive.is_compatible());
        additive.protocol_version = "1.0".into();
        assert!(!additive.is_compatible());
    }

    #[test]
    fn missing_and_unknown_feature_versions_are_additive() {
        let decoded: ExtensionCapabilities = serde_json::from_value(serde_json::json!({
            "protocolVersion": "0.2",
            "features": {
                "desktopChat": 1,
                "futureFeature": 4
            }
        }))
        .unwrap();
        assert!(decoded.is_compatible());
        assert_eq!(decoded.features.desktop_chat, 1);
        assert_eq!(decoded.features.timeline, 0);
    }

    #[test]
    fn schema_contains_every_extension_domain() {
        let schema = schema_json();
        for needle in [
            "ListThreadsRequest",
            "RenameThreadRequest",
            "DesktopBootstrapRequest",
            "GetThreadTimelineRequest",
            "SetProfilePreferencesRequest",
            "CreateCollectionRequest",
            "NativeLoginStartRequest",
            "VerifySecurityRequest",
            "PrewarmSecurityRequest",
            "CompactRequest",
            "EventNotification",
        ] {
            assert!(schema.contains(needle), "missing {needle}");
        }
    }

    #[test]
    fn checked_in_schema_matches_the_rust_contract() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../protocol/axiom-acp-extension/v0.2/schema.json");
        let checked_in = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("could not read {}: {error}", path.display()));
        assert_eq!(
            checked_in.trim(),
            schema_json().trim(),
            "regenerate with `cargo run -p axiom-acp-extension --example export_schema`"
        );
    }

    proptest! {
        #[test]
        fn capability_round_trips_for_bounded_runtime_ids(runtime in "[a-zA-Z0-9_-]{0,128}") {
            let capabilities = ExtensionCapabilities::agent(runtime);
            let json = serde_json::to_value(&capabilities).expect("serialize");
            let decoded: ExtensionCapabilities = serde_json::from_value(json).expect("deserialize");
            prop_assert_eq!(decoded, capabilities);
        }
    }
}
