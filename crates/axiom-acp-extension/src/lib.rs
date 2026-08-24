#![forbid(unsafe_code)]

//! Versioned first-party extensions layered on top of standard ACP.
//!
//! These types are deliberately presentation-neutral. Standard ACP remains the
//! primary representation for prompts, tool calls, plans, permissions, modes,
//! configuration, and cancellation. This crate only describes Axiom-specific
//! control and state that standard ACP cannot faithfully carry.

use std::collections::BTreeMap;
use std::fmt;

use agent_client_protocol::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: &str = "0.1";
pub const META_KEY: &str = "axiom";
pub const MAX_SESSION_IDS: usize = 1_000;
pub const MAX_QUERY_BYTES: usize = 512;
pub const MAX_FOCUS_BYTES: usize = 4 * 1024;
pub const MAX_API_KEY_BYTES: usize = 4 * 1024;
pub const MAX_EVIDENCE_CLAIMS: usize = 128;
pub const MAX_EVIDENCE_VALUE_BYTES: usize = 8 * 1024;
pub const MAX_WORKLOAD_MANIFEST_BYTES: usize = 64 * 1024;
pub const MAX_IDENTIFIER_BYTES: usize = 512;

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
        self.protocol_version == PROTOCOL_VERSION
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FeatureVersions {
    pub session_catalog: u16,
    pub settings: u16,
    pub account: u16,
    pub security_evidence: u16,
    pub compaction: u16,
    pub activity: u16,
}

impl FeatureVersions {
    #[must_use]
    pub const fn all() -> Self {
        Self {
            session_catalog: 1,
            settings: 1,
            account: 1,
            security_evidence: 1,
            compaction: 1,
            activity: 1,
        }
    }
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
pub enum SessionLifecycle {
    Ready,
    Running,
    WaitingForApproval,
    WaitingForAnswer,
    Compacting,
    Closed,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SessionSettings {
    pub model: String,
    pub thinking_level: String,
    pub permission_profile: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub cwd: String,
    pub updated_at: String,
    pub settings: SessionSettings,
    pub archived: bool,
    pub history_truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_outcome: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SessionSnapshot {
    pub runtime_instance_id: String,
    pub sequence: u64,
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub cwd: String,
    pub lifecycle: SessionLifecycle,
    pub settings: SessionSettings,
    pub security: SecurityStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_turn_id: Option<String>,
    pub has_prompt: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SecurityState {
    Unverified,
    Verifying,
    UnattestedDevelopment,
    Verified,
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
    pub model_key_fingerprint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls_spki_fingerprint: Option<String>,
    pub checks: Vec<EvidenceCheck>,
    pub provider_claims: Vec<EvidenceClaim>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workload_manifest: Option<String>,
    pub workload_identity_allowlist_configured: bool,
    pub approved_workload_identity: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub id: String,
    pub label: String,
    pub thinking_levels: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AccountStatus {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_name: Option<String>,
    pub state: AccountState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AccountState {
    Missing,
    Valid,
    Invalid,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BrowserLoginState {
    pub login_id: String,
    pub user_code: String,
    pub authorization_url: String,
    pub browser_opened: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ActivityEvent {
    SessionState {
        snapshot: SessionSnapshot,
    },
    TitleChanged {
        title: String,
    },
    SettingsChanged {
        settings: SessionSettings,
    },
    SecurityChanged {
        status: SecurityStatus,
    },
    AccountChanged {
        status: AccountStatus,
    },
    Progress {
        message: String,
        completed: Option<u64>,
        total: Option<u64>,
    },
    TaskList {
        items: Vec<TaskItem>,
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
    Warning {
        message: String,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TaskItem {
    pub id: String,
    pub title: String,
    pub status: String,
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
    pub event: ActivityEvent,
}

#[derive(
    Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcRequest,
)]
#[request(method = "_axiom/session/list", response = ListSessionsResponse)]
#[serde(rename_all = "camelCase")]
pub struct ListSessionsRequest {
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
pub struct ListSessionsResponse {
    pub sessions: Vec<SessionSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcRequest)]
#[request(method = "_axiom/session/get_state", response = GetSessionStateResponse)]
#[serde(rename_all = "camelCase")]
pub struct GetSessionStateRequest {
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct GetSessionStateResponse {
    pub snapshot: SessionSnapshot,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcRequest)]
#[request(method = "_axiom/session/delete_preview", response = DeletePreviewResponse)]
#[serde(rename_all = "camelCase")]
pub struct DeletePreviewRequest {
    pub session_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct DeletePreviewResponse {
    pub confirmation_token: String,
    pub expires_at_unix_seconds: u64,
    pub sessions: Vec<SessionSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcRequest)]
#[request(method = "_axiom/session/delete_confirm", response = DeleteConfirmResponse)]
#[serde(rename_all = "camelCase")]
pub struct DeleteConfirmRequest {
    pub confirmation_token: String,
    pub session_ids: Vec<String>,
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

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcRequest)]
#[request(method = "_axiom/session/set_settings", response = SetSessionSettingsResponse)]
#[serde(rename_all = "camelCase")]
pub struct SetSessionSettingsRequest {
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_level: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_profile: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct SetSessionSettingsResponse {
    pub settings: SessionSettings,
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
#[request(method = "_axiom/account/browser_login_start", response = BrowserLoginStartResponse)]
pub struct BrowserLoginStartRequest {}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct BrowserLoginStartResponse {
    pub login: BrowserLoginState,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcRequest)]
#[request(method = "_axiom/account/browser_login_complete", response = AccountStatusResponse)]
#[serde(rename_all = "camelCase")]
pub struct BrowserLoginCompleteRequest {
    pub login_id: String,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcRequest)]
#[request(method = "_axiom/account/use_api_key", response = AccountStatusResponse)]
#[serde(rename_all = "camelCase")]
pub struct UseApiKeyRequest {
    pub api_key: String,
}

impl fmt::Debug for UseApiKeyRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UseApiKeyRequest")
            .field("api_key", &"[REDACTED]")
            .finish()
    }
}

#[derive(
    Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcRequest,
)]
#[request(method = "_axiom/account/logout", response = AccountStatusResponse)]
pub struct LogoutRequest {}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcRequest)]
#[request(method = "_axiom/security/verify", response = VerifySecurityResponse)]
#[serde(rename_all = "camelCase")]
pub struct VerifySecurityRequest {
    pub session_id: String,
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
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct CompactResponse {
    pub messages_before: u64,
}

/// Schema root used to generate the checked-in language-neutral contract.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolSchema {
    pub capabilities: ExtensionCapabilities,
    pub list_sessions_request: ListSessionsRequest,
    pub list_sessions_response: ListSessionsResponse,
    pub get_session_state_request: GetSessionStateRequest,
    pub get_session_state_response: GetSessionStateResponse,
    pub delete_preview_request: DeletePreviewRequest,
    pub delete_preview_response: DeletePreviewResponse,
    pub delete_confirm_request: DeleteConfirmRequest,
    pub delete_confirm_response: DeleteConfirmResponse,
    pub list_models_request: ListModelsRequest,
    pub list_models_response: ListModelsResponse,
    pub set_session_settings_request: SetSessionSettingsRequest,
    pub set_session_settings_response: SetSessionSettingsResponse,
    pub account_status_request: AccountStatusRequest,
    pub account_status_response: AccountStatusResponse,
    pub browser_login_start_request: BrowserLoginStartRequest,
    pub browser_login_start_response: BrowserLoginStartResponse,
    pub browser_login_complete_request: BrowserLoginCompleteRequest,
    pub use_api_key_request: UseApiKeyRequest,
    pub logout_request: LogoutRequest,
    pub verify_security_request: VerifySecurityRequest,
    pub verify_security_response: VerifySecurityResponse,
    pub compact_request: CompactRequest,
    pub compact_response: CompactResponse,
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
    }

    #[test]
    fn api_key_debug_is_redacted() {
        let request = UseApiKeyRequest {
            api_key: "axm_secret".into(),
        };
        let debug = format!("{request:?}");
        assert!(!debug.contains("axm_secret"));
        assert!(debug.contains("REDACTED"));
    }

    #[test]
    fn schema_contains_every_extension_domain() {
        let schema = schema_json();
        for needle in [
            "ListSessionsRequest",
            "SetSessionSettingsRequest",
            "BrowserLoginStartRequest",
            "VerifySecurityRequest",
            "CompactRequest",
            "EventNotification",
        ] {
            assert!(schema.contains(needle), "missing {needle}");
        }
    }

    #[test]
    fn checked_in_schema_matches_the_rust_contract() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../protocol/axiom-acp-extension/v0.1/schema.json");
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

        #[test]
        fn arbitrary_api_keys_never_appear_in_debug(secret in ".{1,1024}") {
            let request = UseApiKeyRequest { api_key: secret.clone() };
            let debug = format!("{request:?}");
            prop_assert!(!debug.contains(&secret));
        }
    }
}
