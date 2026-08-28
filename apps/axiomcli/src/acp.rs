use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    str::FromStr as _,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use agent_client_protocol::schema::{ProtocolVersion, v1 as protocol};
use agent_client_protocol::{Agent, Client, ConnectionTo, Responder, Stdio};
use async_trait::async_trait;
use axiom_acp_extension as extension;
use axiom_acp_extension::{
    AccountState as ExtensionAccountState, AccountStatus as ExtensionAccountStatus, ActivityEvent,
    BrowserLoginCompleteRequest, BrowserLoginStartRequest, BrowserLoginStartResponse,
    BrowserLoginState, CollectionStateResponse, CompactRequest, CompactResponse, CompactionPhase,
    DeleteConfirmRequest, DeleteConfirmResponse, DeletePreviewRequest, DeletePreviewResponse,
    DesktopBootstrapRequest, DesktopBootstrapResponse, EventNotification, ExtensionCapabilities,
    ExtensionError, ExtensionErrorCode, FeatureVersions, GetThreadTimelineRequest,
    GetThreadTimelineResponse, ListModelsRequest, ListModelsResponse, ListThreadsRequest,
    ListThreadsResponse, LogoutRequest, ModelInfo, ProfilePreferencesResponse,
    SecurityEvidence as ExtensionSecurityEvidence, SecurityState as ExtensionSecurityState,
    SecurityStatus as ExtensionSecurityStatus, UseApiKeyRequest, VerifySecurityRequest,
    VerifySecurityResponse,
};
use tokio::sync::{Mutex, mpsc};
use tokio_util::sync::CancellationToken;

use crate::{
    agent::{
        APP_EVENT_QUEUE_CAPACITY, QuestionHandler, SecurityVerification, TurnRunner,
        reconcile_model_settings, reconcile_new_session_settings,
    },
    app::{
        AppCommand, AppEvent, Origin, PermissionProfile, QuestionRequest, Runtime, SessionId,
        TurnId,
    },
    auth::{AuthManager, ValidationStatus},
    config::Config,
    paths::FrontendKind,
    policy::{ApprovalChoice, ApprovalHandler, ApprovalRequest, ApprovalResponse},
    session::{SessionStore, model_for_resume, permission_for_resume, thinking_for_resume},
    slash::{self, SlashCommand},
    tool_display::{ToolDisplayPhase, describe_tool},
};

fn send_available_commands(
    connection: &ConnectionTo<Client>,
    session_id: &protocol::SessionId,
) -> agent_client_protocol::Result<()> {
    let commands = slash::COMMANDS
        .iter()
        .map(|spec| {
            let command = protocol::AvailableCommand::new(spec.name, spec.description);
            if matches!(
                spec.name,
                "help"
                    | "permissions"
                    | "model"
                    | "resume"
                    | "delete"
                    | "login"
                    | "logout"
                    | "account"
                    | "security"
                    | "refresh"
            ) {
                command
            } else {
                command.input(protocol::AvailableCommandInput::Unstructured(
                    protocol::UnstructuredCommandInput::new(spec.usage),
                ))
            }
        })
        .collect();
    connection.send_notification(protocol::SessionNotification::new(
        session_id.clone(),
        protocol::SessionUpdate::AvailableCommandsUpdate(protocol::AvailableCommandsUpdate::new(
            commands,
        )),
    ))
}

const MODEL_CONFIG_ID: &str = "model";
const THINKING_CONFIG_ID: &str = "thinking";

const THINKING_LEVELS: &[crate::app::ThinkingLevel] = &[
    crate::app::ThinkingLevel::Minimal,
    crate::app::ThinkingLevel::Low,
    crate::app::ThinkingLevel::Medium,
    crate::app::ThinkingLevel::High,
    crate::app::ThinkingLevel::ExtraHigh,
];

fn model_config_option(current: &str, models: &[String]) -> protocol::SessionConfigOption {
    let mut values = models.to_vec();
    if !values.iter().any(|model| model == current) {
        values.push(current.to_owned());
    }
    values.sort();
    values.dedup();
    protocol::SessionConfigOption::select(
        MODEL_CONFIG_ID,
        "Model",
        current.to_owned(),
        values
            .into_iter()
            .map(|model| protocol::SessionConfigSelectOption::new(model.clone(), model))
            .collect::<Vec<_>>(),
    )
    .description("Model used for subsequent AxiomCLI requests")
    .category(protocol::SessionConfigOptionCategory::Model)
}

fn thinking_config_option_for(
    current: crate::app::ThinkingLevel,
    levels: &[crate::app::ThinkingLevel],
) -> protocol::SessionConfigOption {
    protocol::SessionConfigOption::select(
        THINKING_CONFIG_ID,
        "Thinking",
        current.to_string(),
        levels
            .iter()
            .map(|level| {
                protocol::SessionConfigSelectOption::new(level.to_string(), level.to_string())
            })
            .collect::<Vec<_>>(),
    )
    .description("Reasoning effort used for subsequent AxiomCLI requests")
}

fn thinking_config_option(current: crate::app::ThinkingLevel) -> protocol::SessionConfigOption {
    thinking_config_option_for(current, THINKING_LEVELS)
}

fn supported_thinking_levels(model: &axiom_inference::ModelInfo) -> Vec<crate::app::ThinkingLevel> {
    model
        .supported_reasoning_efforts
        .iter()
        .copied()
        .map(|effort| match effort {
            axiom_inference::ReasoningEffort::Minimal => crate::app::ThinkingLevel::Minimal,
            axiom_inference::ReasoningEffort::Low => crate::app::ThinkingLevel::Low,
            axiom_inference::ReasoningEffort::Medium => crate::app::ThinkingLevel::Medium,
            axiom_inference::ReasoningEffort::High => crate::app::ThinkingLevel::High,
            axiom_inference::ReasoningEffort::ExtraHigh => crate::app::ThinkingLevel::ExtraHigh,
        })
        .collect()
}

fn permission_modes(
    profile: PermissionProfile,
    frontend: FrontendKind,
) -> protocol::SessionModeState {
    let profiles: &[PermissionProfile] = if frontend == FrontendKind::DesktopChat {
        &[PermissionProfile::Web]
    } else {
        &PermissionProfile::ALL
    };
    protocol::SessionModeState::new(
        profile.to_string(),
        profiles
            .iter()
            .copied()
            .map(|permission| {
                protocol::SessionMode::new(permission.to_string(), permission.label())
                    .description(permission.description())
            })
            .collect(),
    )
}

fn session_config_options(
    model: &str,
    models: &[String],
    thinking: crate::app::ThinkingLevel,
    supported_thinking: Option<&[crate::app::ThinkingLevel]>,
) -> Vec<protocol::SessionConfigOption> {
    let mut options = vec![model_config_option(model, models)];
    match supported_thinking {
        Some([]) => {}
        Some(levels) => options.push(thinking_config_option_for(thinking, levels)),
        None => options.push(thinking_config_option(thinking)),
    }
    options
}

fn tool_kind(name: &str) -> protocol::ToolKind {
    match name {
        "read_file" | "list_files" | "glob_files" | "inspect_metadata" | "inspect_git"
        | "select_context" | "view_session_diff" => protocol::ToolKind::Read,
        "apply_patch" | "replace_text" | "plan_create" | "plan_revise" => protocol::ToolKind::Edit,
        "search_text" | "web_search" | "fetch_url" => protocol::ToolKind::Search,
        "run_command" | "run_shell" | "start_background" | "background_wait"
        | "background_list" | "background_status" | "stop_background" => {
            protocol::ToolKind::Execute
        }
        "update_progress" | "plan_propose" | "ask_user_questions" => protocol::ToolKind::Think,
        _ => protocol::ToolKind::Other,
    }
}

fn send_model_config_update(
    connection: &ConnectionTo<Client>,
    session_id: &protocol::SessionId,
    current: &str,
    models: &[String],
) -> agent_client_protocol::Result<()> {
    connection.send_notification(protocol::SessionNotification::new(
        session_id.clone(),
        protocol::SessionUpdate::ConfigOptionUpdate(protocol::ConfigOptionUpdate::new(vec![
            model_config_option(current, models),
        ])),
    ))
}

fn send_config_update(
    connection: &ConnectionTo<Client>,
    session_id: &protocol::SessionId,
    model: &str,
    models: &[String],
    thinking: crate::app::ThinkingLevel,
) -> agent_client_protocol::Result<()> {
    connection.send_notification(protocol::SessionNotification::new(
        session_id.clone(),
        protocol::SessionUpdate::ConfigOptionUpdate(protocol::ConfigOptionUpdate::new(
            session_config_options(model, models, thinking, None),
        )),
    ))
}

fn slash_help_text() -> String {
    let lines = slash::COMMANDS
        .iter()
        .map(|spec| format!("{} — {}", spec.usage, spec.description))
        .collect::<Vec<_>>()
        .join("\n");
    format!("AxiomCLI commands:\n{lines}\nCommands run locally and are not model prompts.")
}

fn security_summary(verification: &SecurityVerification) -> String {
    verification.evidence.as_ref().map_or_else(
        || {
            format!(
                "Security state: {:?}. No attestation evidence is available for this transport.",
                verification.status
            )
        },
        |evidence| {
            let workload_hash = evidence
                .provider_claims
                .iter()
                .find(|claim| claim.name == "workload_manifest_sha256")
                .map_or("unavailable", |claim| claim.value.as_str());
            format!(
                "Security state: {:?}\nProvider: {}\nModel: {}\nAttestation: {}\nEncryption: {} v{}\nWorkload manifest SHA-256: {}",
                verification.status,
                evidence.provider_id,
                evidence.model_id,
                evidence.attestation_protocol,
                evidence.e2ee_protocol,
                evidence.e2ee_encryption_version,
                workload_hash,
            )
        },
    )
}

fn now_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn extension_meta(capabilities: ExtensionCapabilities) -> protocol::Meta {
    let mut meta = protocol::Meta::new();
    meta.insert(
        extension::META_KEY.into(),
        serde_json::to_value(capabilities).expect("extension capabilities serialize"),
    );
    meta
}

fn prompt_metadata(
    meta: Option<&protocol::Meta>,
) -> crate::Result<Option<extension::PromptMetadata>> {
    meta.and_then(|meta| meta.get(extension::META_KEY))
        .map(|value| {
            let metadata = serde_json::from_value::<extension::PromptMetadata>(value.clone())
                .map_err(|_| crate::AxiomError::Protocol("invalid Axiom prompt metadata".into()))?;
            if metadata.client_item_id.is_empty()
                || metadata.client_item_id.len() > extension::MAX_IDENTIFIER_BYTES
            {
                return Err(crate::AxiomError::Protocol(
                    "invalid Axiom client item ID".into(),
                ));
            }
            Ok(metadata)
        })
        .transpose()
}

fn delivery_meta(
    revision: Option<&crate::session::ThreadRevision>,
    timeline_item_id: Option<String>,
    client_item_id: Option<String>,
) -> Option<protocol::Meta> {
    revision.map(|revision| {
        let mut meta = protocol::Meta::new();
        meta.insert(
            extension::META_KEY.into(),
            serde_json::to_value(extension::DeliveryMetadata {
                thread_revision: revision.revision,
                last_timeline_sequence: revision.last_timeline_sequence,
                timeline_item_id,
                client_item_id,
            })
            .expect("delivery metadata serializes"),
        );
        meta
    })
}

fn send_session_update(
    connection: &ConnectionTo<Client>,
    session_id: &protocol::SessionId,
    update: protocol::SessionUpdate,
    revision: Option<&crate::session::ThreadRevision>,
    timeline_item_id: Option<String>,
    client_item_id: Option<String>,
) -> agent_client_protocol::Result<()> {
    connection.send_notification(
        protocol::SessionNotification::new(session_id.clone(), update).meta(delivery_meta(
            revision,
            timeline_item_id,
            client_item_id,
        )),
    )
}

fn negotiated_extension_features(meta: Option<&protocol::Meta>) -> Option<FeatureVersions> {
    meta.and_then(|meta| meta.get(extension::META_KEY))
        .and_then(|value| serde_json::from_value::<ExtensionCapabilities>(value.clone()).ok())
        .filter(ExtensionCapabilities::is_compatible)
        .map(|capabilities| capabilities.features)
}

fn extension_security_state(state: crate::app::SecurityStatus) -> ExtensionSecurityState {
    match state {
        crate::app::SecurityStatus::Unverified => ExtensionSecurityState::Unverified,
        crate::app::SecurityStatus::Verifying => ExtensionSecurityState::Verifying,
        crate::app::SecurityStatus::UnattestedDevelopment => {
            ExtensionSecurityState::UnattestedDevelopment
        }
        crate::app::SecurityStatus::Verified => ExtensionSecurityState::Verified,
        crate::app::SecurityStatus::Failed => ExtensionSecurityState::Failed,
    }
}

fn extension_security_status(state: crate::app::SecurityStatus) -> ExtensionSecurityStatus {
    ExtensionSecurityStatus {
        state: extension_security_state(state),
        detail: None,
    }
}

fn extension_security_evidence(
    status: crate::app::SecurityStatus,
    evidence: axiom_secure_client::SecurityEvidence,
) -> crate::Result<ExtensionSecurityEvidence> {
    let attestation_generation = evidence
        .attestation_generation
        .filter(|generation| *generation > 0)
        .ok_or_else(|| {
            crate::AxiomError::Protocol(
                "verified provider evidence is missing its attestation generation".into(),
            )
        })?;
    let hard_expires_at_unix_seconds = evidence
        .hard_expires_at_unix_seconds
        .filter(|expiry| *expiry > 0)
        .ok_or_else(|| {
            crate::AxiomError::Protocol(
                "verified provider evidence is missing its hard expiry".into(),
            )
        })?;
    Ok(ExtensionSecurityEvidence {
        status: extension_security_status(status),
        provider_id: evidence.provider_id,
        model_id: evidence.model_id,
        attestation_protocol: evidence.attestation_protocol,
        e2ee_protocol: evidence.e2ee_protocol,
        e2ee_encryption_version: evidence.e2ee_encryption_version,
        trust_policy_version: evidence.trust_policy_version,
        verified_at_unix_seconds: evidence.verified_at_unix_seconds,
        attestation_generation,
        hard_expires_at_unix_seconds,
        model_key_fingerprint: evidence.model_key_fingerprint,
        tls_spki_fingerprint: evidence.tls_spki_fingerprint,
        checks: evidence
            .checks
            .into_iter()
            .take(extension::MAX_EVIDENCE_CLAIMS)
            .map(|check| extension::EvidenceCheck {
                name: bounded_extension_text(check.label, 512),
                passed: check.passed,
                detail: bounded_extension_text(check.status, extension::MAX_EVIDENCE_VALUE_BYTES),
            })
            .collect(),
        provider_claims: evidence
            .provider_claims
            .into_iter()
            .take(extension::MAX_EVIDENCE_CLAIMS)
            .map(|claim| extension::EvidenceClaim {
                name: bounded_extension_text(claim.name, 512),
                value: bounded_extension_text(claim.value, extension::MAX_EVIDENCE_VALUE_BYTES),
            })
            .collect(),
        workload_manifest: evidence.workload_manifest.map(|manifest| {
            bounded_extension_text(manifest, extension::MAX_WORKLOAD_MANIFEST_BYTES)
        }),
        workload_identity_allowlist_configured: evidence.workload_identity_allowlist_configured,
        approved_workload_identity: evidence.approved_workload_identity,
    })
}

fn strict_extension_security_verification(
    verification: SecurityVerification,
) -> crate::Result<(
    crate::app::SecurityStatus,
    Option<ExtensionSecurityEvidence>,
)> {
    let status = verification.status;
    let evidence = verification
        .evidence
        .map(|evidence| extension_security_evidence(status, evidence))
        .transpose()?;
    if status == crate::app::SecurityStatus::Verified && evidence.is_none() {
        return Err(crate::AxiomError::Protocol(
            "verified provider returned no attestation evidence".into(),
        ));
    }
    Ok((status, evidence))
}

fn bounded_extension_text(mut value: String, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value;
    }
    let boundary = value
        .char_indices()
        .map(|(index, _)| index)
        .take_while(|index| *index <= max_bytes.saturating_sub(3))
        .last()
        .unwrap_or(0);
    value.truncate(boundary);
    value.push('…');
    value
}

fn extension_thread_lifecycle(
    value: crate::session::ThreadLifecycle,
) -> extension::ThreadLifecycle {
    match value {
        crate::session::ThreadLifecycle::Ready => extension::ThreadLifecycle::Ready,
        crate::session::ThreadLifecycle::Running => extension::ThreadLifecycle::Running,
        crate::session::ThreadLifecycle::WaitingForApproval => {
            extension::ThreadLifecycle::WaitingForApproval
        }
        crate::session::ThreadLifecycle::WaitingForAnswer => {
            extension::ThreadLifecycle::WaitingForAnswer
        }
        crate::session::ThreadLifecycle::Compacting => extension::ThreadLifecycle::Compacting,
        crate::session::ThreadLifecycle::Closed => extension::ThreadLifecycle::Closed,
    }
}

fn extension_thread_summary(summary: crate::session::ThreadSummary) -> extension::ThreadSummary {
    extension::ThreadSummary {
        thread_id: summary.id,
        title: summary.title,
        cwd: summary.cwd.display().to_string(),
        origin: summary.origin,
        profile: summary.profile,
        selected_model: summary.selected_model,
        thinking_level: summary.thinking_level.to_string(),
        lifecycle: extension_thread_lifecycle(summary.lifecycle),
        archived: summary.archived,
        revision: summary.revision,
        last_timeline_sequence: summary.last_timeline_sequence,
        created_at: summary.created_at,
        updated_at: summary.updated_at,
    }
}

fn extension_timeline_kind(value: crate::session::TimelineItemKind) -> extension::TimelineItemKind {
    match value {
        crate::session::TimelineItemKind::UserMessage => extension::TimelineItemKind::UserMessage,
        crate::session::TimelineItemKind::AssistantMessage => {
            extension::TimelineItemKind::AssistantMessage
        }
        crate::session::TimelineItemKind::Reasoning => extension::TimelineItemKind::Reasoning,
        crate::session::TimelineItemKind::ToolCall => extension::TimelineItemKind::ToolCall,
        crate::session::TimelineItemKind::Plan => extension::TimelineItemKind::Plan,
        crate::session::TimelineItemKind::Notice => extension::TimelineItemKind::Notice,
    }
}

fn extension_timeline_status(
    value: crate::session::TimelineItemStatus,
) -> extension::TimelineItemStatus {
    match value {
        crate::session::TimelineItemStatus::Pending => extension::TimelineItemStatus::Pending,
        crate::session::TimelineItemStatus::InProgress => extension::TimelineItemStatus::InProgress,
        crate::session::TimelineItemStatus::Completed => extension::TimelineItemStatus::Completed,
        crate::session::TimelineItemStatus::Cancelled => extension::TimelineItemStatus::Cancelled,
        crate::session::TimelineItemStatus::Failed => extension::TimelineItemStatus::Failed,
        crate::session::TimelineItemStatus::Interrupted => {
            extension::TimelineItemStatus::Interrupted
        }
    }
}

fn extension_timeline_item(item: crate::session::TimelineItem) -> extension::TimelineItem {
    extension::TimelineItem {
        id: item.id,
        thread_id: item.thread_id,
        turn_id: item.turn_id,
        sequence: item.sequence,
        kind: extension_timeline_kind(item.kind),
        status: extension_timeline_status(item.status),
        client_item_id: item.client_item_id,
        external_id: item.external_id,
        content: item.content,
        metadata: item.metadata,
        created_at: item.created_at,
        updated_at: item.updated_at,
    }
}

fn extension_preferences(
    value: crate::session::ProfilePreferences,
) -> extension::ProfilePreferences {
    extension::ProfilePreferences {
        model: value.model,
        thinking_level: value.thinking_level.to_string(),
        updated_at: value.updated_at,
    }
}

fn extension_collections(value: crate::session::CollectionState) -> extension::CollectionState {
    extension::CollectionState {
        revision: value.revision,
        collections: value
            .collections
            .into_iter()
            .map(|collection| extension::Collection {
                id: collection.id,
                name: collection.name,
                collapsed: collection.collapsed,
                position: collection.position,
                thread_ids: collection.thread_ids,
                created_at: collection.created_at,
                updated_at: collection.updated_at,
            })
            .collect(),
    }
}

fn extension_account_status(status: ValidationStatus) -> ExtensionAccountStatus {
    match status {
        ValidationStatus::Missing => ExtensionAccountStatus {
            key_id: None,
            key_name: None,
            state: ExtensionAccountState::Missing,
            detail: None,
        },
        ValidationStatus::Valid(account) => ExtensionAccountStatus {
            key_id: Some(account.key_id),
            key_name: Some(account.key_name),
            state: ExtensionAccountState::Valid,
            detail: Some(format!("stored in {}", account.source.label())),
        },
        ValidationStatus::Invalid => ExtensionAccountStatus {
            key_id: None,
            key_name: None,
            state: ExtensionAccountState::Invalid,
            detail: Some("The saved Axiom API key is invalid or revoked".into()),
        },
        ValidationStatus::Unavailable(detail) => ExtensionAccountStatus {
            key_id: None,
            key_name: None,
            state: ExtensionAccountState::Unavailable,
            detail: Some(detail),
        },
    }
}

fn send_extension_activity(
    connection: &ConnectionTo<Client>,
    extension_state: &ExtensionState,
    session_id: Option<&protocol::SessionId>,
    event: ActivityEvent,
) -> agent_client_protocol::Result<()> {
    send_extension_activity_correlated(connection, extension_state, session_id, None, event)
}

fn send_extension_activity_correlated(
    connection: &ConnectionTo<Client>,
    extension_state: &ExtensionState,
    session_id: Option<&protocol::SessionId>,
    correlation_id: Option<String>,
    event: ActivityEvent,
) -> agent_client_protocol::Result<()> {
    if !extension_state.feature_enabled(ExtensionFeature::Activity) {
        return Ok(());
    }
    connection.send_notification(EventNotification {
        runtime_instance_id: extension_state.runtime_instance_id.clone(),
        session_id: session_id.map(|id| id.0.to_string()),
        sequence: extension_state.next_sequence(),
        occurred_at: chrono::Utc::now().to_rfc3339(),
        correlation_id,
        thread_revision: None,
        last_timeline_sequence: None,
        event,
    })
}

fn extension_enabled(state: &ExtensionState, feature: ExtensionFeature) -> bool {
    state.feature_enabled(feature)
}

fn extension_rpc_error(
    json_rpc_code: i32,
    code: ExtensionErrorCode,
    message: impl Into<String>,
    retryable: bool,
) -> agent_client_protocol::Error {
    let message = message.into();
    let data = ExtensionError {
        code,
        message: message.clone(),
        retryable,
        correlation_id: None,
    };
    agent_client_protocol::Error::new(json_rpc_code, message)
        .data(serde_json::to_value(data).expect("Axiom extension error must serialize"))
}

fn extension_not_negotiated() -> agent_client_protocol::Error {
    extension_rpc_error(
        -32601,
        ExtensionErrorCode::UnsupportedFeature,
        "Axiom ACP extension feature was not negotiated during initialize",
        false,
    )
}

fn validate_session_ids(values: &[String]) -> crate::Result<Vec<SessionId>> {
    if values.is_empty() || values.len() > extension::MAX_SESSION_IDS {
        return Err(crate::AxiomError::Config(format!(
            "session selection must contain 1-{} IDs",
            extension::MAX_SESSION_IDS
        )));
    }
    let mut ids = values
        .iter()
        .map(|value| {
            SessionId::from_str(value)
                .map_err(|_| crate::AxiomError::Config("invalid session ID".into()))
        })
        .collect::<crate::Result<Vec<_>>>()?;
    ids.sort_by_key(ToString::to_string);
    ids.dedup();
    if ids.len() != values.len() {
        return Err(crate::AxiomError::Config(
            "session selection contains duplicate IDs".into(),
        ));
    }
    Ok(ids)
}

fn extension_activity(event: &AppEvent) -> Option<ActivityEvent> {
    match event {
        AppEvent::SecurityStatusChanged { state } => Some(ActivityEvent::SecurityChanged {
            status: extension_security_status(*state),
        }),
        AppEvent::BackgroundTaskChanged { task_id, state } => Some(ActivityEvent::BackgroundTask {
            task_id: task_id.clone(),
            state: state.clone(),
        }),
        AppEvent::WorkspaceChanged { paths } => Some(ActivityEvent::WorkspaceChanged {
            paths: paths
                .iter()
                .map(|path| path.display().to_string())
                .collect(),
        }),
        AppEvent::ContextCompacted {
            messages_before, ..
        } => Some(ActivityEvent::Compaction {
            phase: CompactionPhase::Completed,
            detail: Some(format!("compacted {messages_before} conversation messages")),
        }),
        _ => None,
    }
}

#[derive(Clone)]
struct AcpSession {
    internal_id: SessionId,
    cwd: PathBuf,
    profile: PermissionProfile,
    model: String,
    thinking: crate::app::ThinkingLevel,
    models: Vec<String>,
    active_turn: Option<TurnId>,
    cancellation: Option<CancellationToken>,
    has_prompt: bool,
    security: crate::app::SecurityStatus,
}

fn apply_session_model_settings(
    session: &mut AcpSession,
    model: &str,
    thinking: crate::app::ThinkingLevel,
    models: &[String],
    reset_security: bool,
) {
    model.clone_into(&mut session.model);
    session.thinking = thinking;
    session.models.clear();
    session.models.extend_from_slice(models);
    if reset_security {
        session.security = crate::app::SecurityStatus::Unverified;
    }
}

type Sessions = Arc<Mutex<HashMap<protocol::SessionId, AcpSession>>>;

struct PendingDelete {
    session_ids: Vec<SessionId>,
    expires_at_unix_seconds: u64,
}

#[derive(Clone, Copy)]
enum ExtensionFeature {
    ThreadCatalog = 1 << 0,
    Timeline = 1 << 1,
    ModelCatalog = 1 << 2,
    ProfilePreferences = 1 << 3,
    Collections = 1 << 4,
    Account = 1 << 5,
    SecurityEvidence = 1 << 6,
    Compaction = 1 << 7,
    Activity = 1 << 8,
    DesktopChat = 1 << 9,
}

struct ExtensionState {
    enabled_features: AtomicU64,
    runtime_instance_id: String,
    sequence: AtomicU64,
    pending_deletes: Mutex<HashMap<String, PendingDelete>>,
    browser_logins: Mutex<HashMap<String, crate::auth::BrowserLogin>>,
}

impl ExtensionState {
    fn new() -> Self {
        Self {
            enabled_features: AtomicU64::new(0),
            runtime_instance_id: uuid::Uuid::new_v4().to_string(),
            sequence: AtomicU64::new(0),
            pending_deletes: Mutex::new(HashMap::new()),
            browser_logins: Mutex::new(HashMap::new()),
        }
    }

    fn next_sequence(&self) -> u64 {
        self.sequence.fetch_add(1, Ordering::AcqRel) + 1
    }

    fn negotiate(&self, features: Option<FeatureVersions>) {
        let Some(features) = features else {
            self.enabled_features.store(0, Ordering::Release);
            return;
        };
        let mut mask = 0;
        if features.thread_catalog >= 1 {
            mask |= ExtensionFeature::ThreadCatalog as u64;
        }
        if features.timeline >= 1 {
            mask |= ExtensionFeature::Timeline as u64;
        }
        if features.model_catalog >= 1 {
            mask |= ExtensionFeature::ModelCatalog as u64;
        }
        if features.profile_preferences >= 1 {
            mask |= ExtensionFeature::ProfilePreferences as u64;
        }
        if features.collections >= 1 {
            mask |= ExtensionFeature::Collections as u64;
        }
        if features.account >= 1 {
            mask |= ExtensionFeature::Account as u64;
        }
        if features.security_evidence >= 1 {
            mask |= ExtensionFeature::SecurityEvidence as u64;
        }
        if features.compaction >= 1 {
            mask |= ExtensionFeature::Compaction as u64;
        }
        if features.activity >= 1 {
            mask |= ExtensionFeature::Activity as u64;
        }
        if features.desktop_chat >= 1 {
            mask |= ExtensionFeature::DesktopChat as u64;
        }
        self.enabled_features.store(mask, Ordering::Release);
    }

    fn feature_enabled(&self, feature: ExtensionFeature) -> bool {
        self.enabled_features.load(Ordering::Acquire) & feature as u64 != 0
    }
}

#[derive(Clone)]
struct AcpApproval {
    connection: ConnectionTo<Client>,
    session_id: protocol::SessionId,
}

#[async_trait]
impl ApprovalHandler for AcpApproval {
    async fn request(
        &self,
        request: ApprovalRequest,
        cancellation: CancellationToken,
    ) -> crate::Result<ApprovalResponse> {
        let tool_call = protocol::ToolCallUpdate::new(
            request.request_id.clone(),
            protocol::ToolCallUpdateFields::new()
                .title(request.explanation.clone())
                .kind(protocol::ToolKind::Other)
                .status(protocol::ToolCallStatus::Pending),
        );
        let mut options = vec![protocol::PermissionOption::new(
            "allow_once",
            "Allow once",
            protocol::PermissionOptionKind::AllowOnce,
        )];
        if request.allow_session_grants {
            options.push(protocol::PermissionOption::new(
                "allow_exact_session",
                "Allow this exact operation for this AxiomCLI session",
                protocol::PermissionOptionKind::AllowAlways,
            ));
        }
        if let Some(scope) = &request.suggested_prefix_scope {
            options.push(protocol::PermissionOption::new(
                "allow_prefix_session",
                format!("Allow {scope} for this AxiomCLI session"),
                protocol::PermissionOptionKind::AllowAlways,
            ));
        }
        options.push(protocol::PermissionOption::new(
            "reject_once",
            "Deny",
            protocol::PermissionOptionKind::RejectOnce,
        ));
        let response = tokio::select! {
            () = cancellation.cancelled() => return Err(crate::AxiomError::Cancelled),
            response = self.connection
                .send_request(protocol::RequestPermissionRequest::new(
                    self.session_id.clone(),
                    tool_call,
                    options,
                ))
                .block_task() => response.map_err(|error| crate::AxiomError::Protocol(error.to_string()))?,
        };
        let choice = match response.outcome {
            protocol::RequestPermissionOutcome::Selected(selected) => {
                match selected.option_id.0.as_ref() {
                    "allow_once" => ApprovalChoice::AllowOnce,
                    "allow_exact_session" if request.allow_session_grants => {
                        ApprovalChoice::AllowExactSession
                    }
                    "allow_prefix_session" if request.suggested_prefix_scope.is_some() => {
                        ApprovalChoice::AllowPrefixSession
                    }
                    _ => ApprovalChoice::Deny,
                }
            }
            _ => ApprovalChoice::Deny,
        };
        Ok(ApprovalResponse { choice })
    }
}

#[derive(Clone)]
struct AcpQuestions {
    connection: ConnectionTo<Client>,
    session_id: protocol::SessionId,
    supported: Arc<AtomicBool>,
}

#[async_trait]
impl QuestionHandler for AcpQuestions {
    async fn request(
        &self,
        request: QuestionRequest,
        cancellation: CancellationToken,
    ) -> crate::Result<BTreeMap<String, Vec<String>>> {
        if !self.supported.load(Ordering::Acquire) {
            return Err(crate::AxiomError::Protocol(
                "ACP client did not advertise structured elicitation support".into(),
            ));
        }
        let mut schema = protocol::ElicitationSchema::new().title("AxiomCLI questions");
        for question in &request.questions {
            if question.multiple {
                schema = schema.property(
                    question.id.clone(),
                    protocol::MultiSelectPropertySchema::new(question.options.clone())
                        .title(question.prompt.clone())
                        .min_items(1_u64)
                        .max_items(u64::try_from(question.options.len()).unwrap_or(u64::MAX)),
                    question.required,
                );
            } else {
                let mut property = protocol::StringPropertySchema::new()
                    .title(question.prompt.clone())
                    .min_length(1_u32);
                if !question.options.is_empty() {
                    property = property.enum_values(question.options.clone());
                }
                schema = schema.property(question.id.clone(), property, question.required);
            }
        }
        let mode = protocol::ElicitationFormMode::new(
            protocol::ElicitationSessionScope::new(self.session_id.clone()),
            schema,
        );
        let response = tokio::select! {
            () = cancellation.cancelled() => return Err(crate::AxiomError::Cancelled),
            response = self.connection.send_request(protocol::CreateElicitationRequest::new(
                mode,
                format!("AxiomCLI needs {} answer(s) to continue", request.questions.len()),
            )).block_task() => response.map_err(|error| crate::AxiomError::Protocol(error.to_string()))?,
        };
        let protocol::ElicitationAction::Accept(accepted) = response.action else {
            return Err(crate::AxiomError::Cancelled);
        };
        let content = accepted.content.ok_or_else(|| {
            crate::AxiomError::Protocol("ACP elicitation returned no content".into())
        })?;
        let mut answers = BTreeMap::new();
        for (id, value) in content {
            let values = match value {
                protocol::ElicitationContentValue::String(value) => vec![value],
                protocol::ElicitationContentValue::StringArray(values) => values,
                _ => {
                    return Err(crate::AxiomError::Protocol(format!(
                        "ACP elicitation field `{id}` returned a non-text value"
                    )));
                }
            };
            answers.insert(id, values);
        }
        Ok(answers)
    }
}

/// Serve ACP v1 over stdio. Stdout is owned by the protocol transport.
pub async fn serve_stdio(
    runner: Arc<dyn TurnRunner>,
    config: Config,
    store: Option<SessionStore>,
    auth: AuthManager,
    frontend: FrontendKind,
    desktop_cwd: Option<PathBuf>,
) -> agent_client_protocol::Result<()> {
    let sessions: Sessions = Arc::new(Mutex::new(HashMap::new()));
    let extension_state = Arc::new(ExtensionState::new());
    let runtime = Runtime::new(512);
    let default_model = config.model.clone();
    let default_thinking = crate::app::ThinkingLevel::Medium;

    let new_sessions = sessions.clone();
    let new_runtime = runtime.clone();
    let new_runner = runner.clone();
    let new_config = config;
    let new_frontend = frontend;
    let new_desktop_cwd = desktop_cwd.clone();
    let prompt_sessions = sessions.clone();
    let prompt_runtime = runtime.clone();
    let cancel_sessions = sessions.clone();
    let new_store = store.clone();
    let new_extension = extension_state.clone();
    let load_store = store.clone();
    let extension_store = store.clone();
    let prompt_store = store;
    let prompt_auth = auth.clone();
    let extension_auth = auth;
    let load_sessions = sessions.clone();
    let load_runtime = runtime.clone();
    let load_runner = runner.clone();
    let load_extension = extension_state.clone();
    let prompt_extension = extension_state.clone();
    let new_default_model = default_model.clone();
    let load_default_model = default_model;
    let new_default_thinking = default_thinking;
    let load_default_thinking = default_thinking;
    let config_sessions = sessions.clone();
    let config_runtime = runtime.clone();
    let config_runner = runner.clone();
    let config_store = prompt_store.clone();
    let config_extension = extension_state.clone();
    let mode_sessions = sessions.clone();
    let mode_runtime = runtime.clone();
    let mode_store = prompt_store.clone();
    let mode_frontend = frontend;
    let supports_load = load_store.is_some();
    let supports_elicitation = Arc::new(AtomicBool::new(false));
    let initialize_elicitation = supports_elicitation.clone();
    let initialize_extension = extension_state.clone();
    let list_extension = extension_state.clone();
    let list_extension_store = extension_store.clone();
    let timeline_extension = extension_state.clone();
    let timeline_extension_store = extension_store.clone();
    let preview_extension = extension_state.clone();
    let preview_extension_store = extension_store.clone();
    let preview_extension_sessions = sessions.clone();
    let delete_extension = extension_state.clone();
    let delete_extension_store = extension_store.clone();
    let delete_extension_sessions = sessions.clone();
    let models_extension = extension_state.clone();
    let models_extension_runner = runner.clone();
    let stored_preferences_extension = extension_state.clone();
    let stored_preferences_store = extension_store.clone();
    let update_preferences_extension = extension_state.clone();
    let update_preferences_store = extension_store.clone();
    let update_preferences_runner = runner.clone();
    let collections_list_extension = extension_state.clone();
    let collections_list_store = extension_store.clone();
    let collection_create_extension = extension_state.clone();
    let collection_create_store = extension_store.clone();
    let collection_rename_extension = extension_state.clone();
    let collection_rename_store = extension_store.clone();
    let collection_collapsed_extension = extension_state.clone();
    let collection_collapsed_store = extension_store.clone();
    let collection_move_extension = extension_state.clone();
    let collection_move_store = extension_store.clone();
    let collection_delete_extension = extension_state.clone();
    let collection_delete_store = extension_store.clone();
    let collection_assign_extension = extension_state.clone();
    let collection_assign_store = extension_store.clone();
    let account_status_extension = extension_state.clone();
    let account_status_auth = extension_auth.clone();
    let login_start_extension = extension_state.clone();
    let login_start_auth = extension_auth.clone();
    let login_complete_extension = extension_state.clone();
    let login_complete_auth = extension_auth.clone();
    let api_key_extension = extension_state.clone();
    let api_key_auth = extension_auth.clone();
    let logout_extension = extension_state.clone();
    let logout_auth = extension_auth;
    let security_extension = extension_state.clone();
    let security_extension_sessions = sessions.clone();
    let security_extension_runner = runner.clone();
    let compact_extension = extension_state.clone();
    let compact_extension_sessions = sessions.clone();
    let compact_extension_runner = runner.clone();
    let compact_extension_runtime = runtime.clone();
    let compact_extension_store = extension_store.clone();
    let bootstrap_extension = extension_state.clone();
    let bootstrap_cwd = desktop_cwd.clone();
    let bootstrap_store = prompt_store.clone();
    let bootstrap_runner = runner.clone();
    let bootstrap_default_model = new_default_model.clone();
    let bootstrap_default_thinking = default_thinking;
    let load_frontend = frontend;
    let load_desktop_cwd = desktop_cwd;

    Agent
        .builder()
        .name("axiomcli")
        .on_receive_request(
            async move |request: protocol::InitializeRequest,
                        responder: Responder<protocol::InitializeResponse>,
                        _connection| {
                initialize_elicitation.store(
                    request.client_capabilities.elicitation.as_ref().is_some_and(|value| value.form.is_some()),
                    Ordering::Release,
                );
                initialize_extension.negotiate(negotiated_extension_features(
                    request.client_capabilities.meta.as_ref(),
                ));
                let capabilities = protocol::AgentCapabilities::new()
                    .load_session(supports_load)
                    .meta(extension_meta(ExtensionCapabilities::agent(
                        initialize_extension.runtime_instance_id.clone(),
                    )));
                responder.respond(
                    protocol::InitializeResponse::new(ProtocolVersion::V1)
                    .agent_capabilities(capabilities)
                    .agent_info(
                        protocol::Implementation::new("axiomcli", env!("CARGO_PKG_VERSION"))
                            .title("AxiomCLI"),
                    ),
                )
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |_request: DesktopBootstrapRequest,
                        responder: Responder<DesktopBootstrapResponse>,
                        _connection| {
                if !extension_enabled(&bootstrap_extension, ExtensionFeature::DesktopChat) {
                    return responder.respond_with_error(extension_not_negotiated());
                }
                if frontend != FrontendKind::DesktopChat {
                    return responder.respond_with_internal_error(
                        "AxiomCLI was not launched with the desktop-chat frontend",
                    );
                }
                let Some(cwd) = &bootstrap_cwd else {
                    return responder.respond_with_internal_error(
                        "desktop chat workspace was not initialized",
                    );
                };
                let durable_preferences = bootstrap_store
                    .as_ref()
                    .map(SessionStore::profile_preferences)
                    .transpose()
                    .map_err(agent_error)?;
                let model_details = bootstrap_runner
                    .available_model_details(CancellationToken::new())
                    .await
                    .map_err(agent_error)?;
                let settings = reconcile_new_session_settings(
                    model_details,
                    durable_preferences
                        .as_ref()
                        .and_then(|preferences| preferences.model.as_deref()),
                    &bootstrap_default_model,
                    durable_preferences.as_ref().map_or(
                        bootstrap_default_thinking,
                        |preferences| preferences.thinking_level,
                    ),
                )
                .map_err(agent_error)?;
                let preferences = match (&bootstrap_store, durable_preferences) {
                    (Some(store), Some(preferences))
                        if preferences.model.as_deref() != Some(settings.model.as_str())
                            || preferences.thinking_level != settings.thinking =>
                    {
                        extension_preferences(
                            store
                                .set_profile_preferences(&settings.model, settings.thinking)
                                .map_err(agent_error)?,
                        )
                    }
                    (_, Some(preferences)) => extension_preferences(preferences),
                    _ => extension::ProfilePreferences {
                        model: Some(settings.model),
                        thinking_level: settings.thinking.to_string(),
                        updated_at: chrono::Utc::now().to_rfc3339(),
                    },
                };
                responder.respond(DesktopBootstrapResponse {
                    frontend: frontend.to_string(),
                    chat_cwd: cwd.display().to_string(),
                    permission_profile: PermissionProfile::Web.to_string(),
                    new_thread_settings: preferences,
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: protocol::NewSessionRequest,
                        responder: Responder<protocol::NewSessionResponse>,
                        connection: ConnectionTo<Client>| {
                if !request.cwd.is_absolute() {
                    return responder
                        .respond_with_internal_error("ACP session cwd must be absolute");
                }
                let cwd = request.cwd.canonicalize().map_err(|error| {
                    agent_client_protocol::util::internal_error(format!(
                        "cannot open ACP workspace: {error}"
                    ))
                })?;
                let profile = if new_frontend == FrontendKind::DesktopChat {
                    let Some(expected) = &new_desktop_cwd else {
                        return responder.respond_with_internal_error(
                            "desktop chat workspace was not initialized",
                        );
                    };
                    if &cwd != expected {
                        return responder.respond_with_internal_error(
                            "desktop chat sessions must use the application chat workspace",
                        );
                    }
                    PermissionProfile::Web
                } else {
                    new_config
                        .for_workspace(&cwd)
                        .map_err(agent_error)?
                        .permission_profile
                };
                let internal_id = SessionId::new();
                let preferences = new_store
                    .as_ref()
                    .map(SessionStore::profile_preferences)
                    .transpose()
                    .map_err(agent_error)?;
                let preferred_model = preferences
                    .as_ref()
                    .and_then(|preferences| preferences.model.as_deref());
                let preferred_thinking = preferences
                    .as_ref()
                    .map_or(new_default_thinking, |preferences| preferences.thinking_level);
                let model_details = new_runner
                    .available_model_details(CancellationToken::new())
                    .await
                    .map_err(agent_error)?;
                let settings = reconcile_new_session_settings(
                    model_details.clone(),
                    preferred_model,
                    &new_default_model,
                    preferred_thinking,
                )
                .map_err(agent_error)?;
                let correct_preferences = new_store.is_some()
                    && preferences.as_ref().is_none_or(|preferences| {
                        preferences.model.as_deref() != Some(settings.model.as_str())
                            || preferences.thinking_level != settings.thinking
                    });
                let session_model = settings.model.clone();
                let session_thinking = settings.thinking;
                let models = model_details
                    .iter()
                    .map(|model| model.id.clone())
                    .collect::<Vec<_>>();
                let supported_thinking = model_details
                    .iter()
                    .find(|model| model.id == session_model)
                    .map(supported_thinking_levels)
                    .ok_or_else(|| {
                        agent_client_protocol::util::internal_error(
                            "selected model disappeared from the provider catalog",
                        )
                    })?;
                let created = new_runtime
                    .dispatch(AppCommand::CreateSession {
                        session_id: internal_id.clone(),
                        cwd: cwd.clone(),
                        origin: Origin::Acp,
                        profile,
                    })
                    .await
                    .map_err(agent_error)?;
                if let Some(store) = &new_store {
                    store.append_all(&created).map_err(agent_error)?;
                }
                new_runner
                    .set_model_settings(&internal_id, &settings)
                    .await
                    .map_err(agent_error)?;
                let selected_settings = new_runtime
                    .dispatch(AppCommand::ChangeModelSettings {
                        session_id: internal_id.clone(),
                        model: session_model.clone(),
                        thinking: session_thinking,
                        reset_security: false,
                    })
                    .await
                    .map_err(agent_error)?;
                let corrected_preferences = if let Some(store) = &new_store {
                    if correct_preferences {
                        let (_, preferences) = store
                            .append_all_and_set_profile_preferences(
                                &selected_settings,
                                &session_model,
                                session_thinking,
                            )
                            .map_err(agent_error)?;
                        Some(extension_preferences(preferences))
                    } else {
                        store
                            .append_all(&selected_settings)
                            .map_err(agent_error)?;
                        None
                    }
                } else {
                    None
                };
                let external_id = protocol::SessionId::new(internal_id.to_string());
                new_sessions.lock().await.insert(
                    external_id.clone(),
                    AcpSession {
                        internal_id,
                        cwd,
                        profile,
                        model: session_model.clone(),
                        thinking: session_thinking,
                        models: models.clone(),
                        active_turn: None,
                        cancellation: None,
                        has_prompt: false,
                        security: crate::app::SecurityStatus::Unverified,
                    },
                );
                responder.respond(
                    protocol::NewSessionResponse::new(external_id.clone())
                        .modes(permission_modes(profile, new_frontend))
                        .config_options(session_config_options(
                            &session_model,
                            &models,
                            session_thinking,
                            Some(&supported_thinking),
                        )),
                )?;
                if let Some(preferences) = corrected_preferences {
                    send_extension_activity(
                        &connection,
                        &new_extension,
                        None,
                        ActivityEvent::ProfilePreferencesChanged { preferences },
                    )?;
                }
                send_available_commands(&connection, &external_id)
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: protocol::LoadSessionRequest,
                        responder: Responder<protocol::LoadSessionResponse>,
                        connection: ConnectionTo<Client>| {
                let Some(store) = &load_store else {
                    return responder.respond_with_internal_error(
                        "durable session loading is unavailable",
                    );
                };
                if !request.cwd.is_absolute() {
                    return responder
                        .respond_with_internal_error("ACP session cwd must be absolute");
                }
                let external_id = request.session_id.clone();
                if load_sessions.lock().await.contains_key(&external_id) {
                    return responder.respond_with_internal_error("ACP session is already loaded");
                }
                let internal_id = SessionId::from_str(request.session_id.0.as_ref())
                    .map_err(|_| agent_client_protocol::util::internal_error("invalid session ID"))?;
                let summary = store.summary(&internal_id).map_err(agent_error)?;
                let requested_cwd = request.cwd.canonicalize().map_err(|error| {
                    agent_client_protocol::util::internal_error(format!(
                        "cannot open ACP workspace: {error}"
                    ))
                })?;
                let saved_cwd = summary.cwd.canonicalize().map_err(|error| {
                    agent_client_protocol::util::internal_error(format!(
                        "saved ACP workspace is unavailable: {error}"
                    ))
                })?;
                if requested_cwd != saved_cwd {
                    return responder.respond_with_internal_error(
                        "ACP load cwd does not match the session workspace identity",
                    );
                }
                if load_frontend == FrontendKind::DesktopChat
                    && load_desktop_cwd.as_ref() != Some(&saved_cwd)
                {
                    return responder.respond_with_internal_error(
                        "desktop chat session has an invalid workspace identity",
                    );
                }
                let loaded = store.load_recovering(&internal_id).map_err(agent_error)?;
                if let Some(warning) = loaded.warnings.first() {
                    return responder.respond_with_internal_error(warning.clone());
                }
                let restored_model = model_for_resume(&loaded.events, &load_default_model);
                let restored_thinking =
                    thinking_for_resume(&loaded.events, load_default_thinking);
                let model_details = load_runner
                    .available_model_details(CancellationToken::new())
                    .await
                    .map_err(agent_error)?;
                let settings = reconcile_new_session_settings(
                    model_details.clone(),
                    Some(&restored_model),
                    &load_default_model,
                    restored_thinking,
                )
                .map_err(agent_error)?;
                let session_model = settings.model.clone();
                let session_thinking = settings.thinking;
                let models = model_details
                    .iter()
                    .map(|model| model.id.clone())
                    .collect::<Vec<_>>();
                let supported_thinking = model_details
                    .iter()
                    .find(|model| model.id == session_model)
                    .map(supported_thinking_levels)
                    .ok_or_else(|| {
                        agent_client_protocol::util::internal_error(
                            "selected model disappeared from the provider catalog",
                        )
                    })?;
                let restored = load_runtime
                    .restore(&loaded.events)
                    .await
                    .map_err(agent_error)?;
                load_runner
                    .restore_session(&internal_id, &loaded.events)
                    .await
                    .map_err(agent_error)?;
                load_runner
                    .set_model_settings(&internal_id, &settings)
                    .await
                    .map_err(agent_error)?;
                for envelope in &loaded.events {
                    send_event(
                        &connection,
                        &external_id,
                        envelope.event.clone(),
                        EventDelivery {
                            cwd: Some(&saved_cwd),
                            extension_state: None,
                            correlation_id: Some(envelope.correlation_id.to_string()),
                            revision: None,
                            client_item_id: None,
                        },
                    )?;
                }
                let resumed = load_runtime
                    .emit(
                        internal_id.clone(),
                        AppEvent::SessionResumed {
                            cwd: saved_cwd.clone(),
                            origin: Origin::Acp,
                            profile: restored.profile,
                        },
                    )
                    .await
                    .map_err(agent_error)?;
                store.append(&resumed).map_err(agent_error)?;
                let model_changed = restored_model != session_model;
                if model_changed || restored_thinking != session_thinking {
                    let corrected = load_runtime
                        .dispatch(AppCommand::ChangeModelSettings {
                            session_id: internal_id.clone(),
                            model: session_model.clone(),
                            thinking: session_thinking,
                            reset_security: model_changed,
                        })
                        .await
                        .map_err(agent_error)?;
                    let revision = store.append_all(&corrected).map_err(agent_error)?;
                    for envelope in corrected {
                        send_event(
                            &connection,
                            &external_id,
                            envelope.event,
                            EventDelivery {
                                cwd: Some(&saved_cwd),
                                extension_state: Some(&load_extension),
                                correlation_id: Some(envelope.correlation_id.to_string()),
                                revision: Some(&revision),
                                client_item_id: None,
                            },
                        )?;
                    }
                }
                let recovery = store.recovery_status(&internal_id).map_err(agent_error)?;
                if recovery.interrupted_turns > 0 || !recovery.interrupted_tools.is_empty() {
                    connection.send_notification(protocol::SessionNotification::new(
                        external_id.clone(),
                        protocol::SessionUpdate::AgentThoughtChunk(protocol::ContentChunk::new(
                            protocol::ContentBlock::Text(protocol::TextContent::new(format!(
                                "[recovery] {} interrupted turn(s), {} operation(s) unknown; nothing replayed",
                                recovery.interrupted_turns,
                                recovery.interrupted_tools.len()
                            ))),
                        )),
                    ))?;
                }
                let restored_profile = if load_frontend == FrontendKind::DesktopChat {
                    PermissionProfile::Web
                } else {
                    permission_for_resume(&loaded.events, restored.profile)
                };
                load_sessions.lock().await.insert(
                    external_id.clone(),
                    AcpSession {
                        internal_id,
                        cwd: saved_cwd,
                        profile: restored_profile,
                        model: session_model.clone(),
                        thinking: session_thinking,
                        models: models.clone(),
                        active_turn: None,
                        cancellation: None,
                        has_prompt: loaded.events.iter().any(|envelope| {
                            matches!(envelope.event, AppEvent::PromptAccepted { .. })
                        }),
                        security: crate::app::SecurityStatus::Unverified,
                    },
                );
                responder.respond(
                    protocol::LoadSessionResponse::new()
                        .modes(permission_modes(restored_profile, load_frontend))
                        .config_options(session_config_options(
                            &session_model,
                            &models,
                            session_thinking,
                            Some(&supported_thinking),
                        )),
                )?;
                send_available_commands(&connection, &external_id)
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: protocol::PromptRequest,
                        responder: Responder<protocol::PromptResponse>,
                        connection: ConnectionTo<Client>| {
                let client_item_id = prompt_metadata(request.meta.as_ref())
                    .map_err(agent_error)?
                    .map(|metadata| metadata.client_item_id);
                let prompt = match prompt_text(&request.prompt) {
                    Ok(prompt) => prompt,
                    Err(message) => return responder.respond_with_internal_error(message),
                };
                let session_id = request.session_id.clone();
                if let Some(command) = slash::parse(&prompt) {
                    let command = match command {
                        Ok(command) => command,
                        Err(error) => {
                            send_text(&connection, &session_id, format!("Command error: {error}"))?;
                            return responder.respond(protocol::PromptResponse::new(
                                protocol::StopReason::EndTurn,
                            ));
                        }
                    };
                    let internal_id = {
                        let sessions = prompt_sessions.lock().await;
                        let Some(session) = sessions.get(&session_id) else {
                            return responder.respond_with_internal_error("unknown ACP session");
                        };
                        if session.cancellation.is_some() {
                            return responder
                                .respond_with_internal_error("session already has active work");
                        }
                        session.internal_id.clone()
                    };
                    match command {
                        SlashCommand::Help => {
                            send_text(&connection, &session_id, slash_help_text())?;
                            return responder.respond(protocol::PromptResponse::new(
                                protocol::StopReason::EndTurn,
                            ));
                        }
                        SlashCommand::Permissions => {
                            send_text(
                                &connection,
                                &session_id,
                                "Open your ACP client's session mode selector to change Tools permissions. Crypto permissions are coming soon.".into(),
                            )?;
                            return responder.respond(protocol::PromptResponse::new(
                                protocol::StopReason::EndTurn,
                            ));
                        }
                        SlashCommand::Model => {
                            let models = runner
                                .available_models(CancellationToken::new())
                                .await
                                .map_err(agent_error)?;
                            let current = {
                                let mut sessions = prompt_sessions.lock().await;
                                let Some(session) = sessions.get_mut(&session_id) else {
                                    return responder
                                        .respond_with_internal_error("unknown ACP session");
                                };
                                session.models.clone_from(&models);
                                session.model.clone()
                            };
                            send_model_config_update(
                                &connection,
                                &session_id,
                                &current,
                                &models,
                            )?;
                            send_text(
                                &connection,
                                &session_id,
                                "Choose a model using the client model selector.".into(),
                            )?;
                            return responder.respond(protocol::PromptResponse::new(
                                protocol::StopReason::EndTurn,
                            ));
                        }
                        SlashCommand::Resume => {
                            send_text(
                                &connection,
                                &session_id,
                                "Use your ACP client's session list and session/load support to resume a transcript. The interactive /resume picker is available in the terminal UI."
                                    .into(),
                            )?;
                            return responder.respond(protocol::PromptResponse::new(
                                protocol::StopReason::EndTurn,
                            ));
                        }
                        SlashCommand::Delete => {
                            send_text(
                                &connection,
                                &session_id,
                                "The interactive multi-select /delete picker is available in the terminal UI. ACP transcript deletion will use a native client capability when the protocol defines one."
                                    .into(),
                            )?;
                            return responder.respond(protocol::PromptResponse::new(
                                protocol::StopReason::EndTurn,
                            ));
                        }
                        SlashCommand::Thinking(level) => {
                            runner
                                .set_thinking_level(&internal_id, level)
                                .await
                                .map_err(agent_error)?;
                            let events = prompt_runtime
                                .dispatch(AppCommand::ChangeThinkingLevel {
                                    session_id: internal_id,
                                    level,
                                })
                                .await
                                .map_err(agent_error)?;
                            if let Some(store) = &prompt_store {
                                store.append_all(&events).map_err(agent_error)?;
                            }
                            if let Some(session) = prompt_sessions.lock().await.get_mut(&session_id)
                            {
                                session.thinking = level;
                            }
                            let current = {
                                let sessions = prompt_sessions.lock().await;
                                sessions.get(&session_id).map(|session| {
                                    (session.model.clone(), session.models.clone())
                                })
                            };
                            if let Some((model, models)) = current {
                                send_config_update(
                                    &connection,
                                    &session_id,
                                    &model,
                                    &models,
                                    level,
                                )?;
                                if let Some(store) = &prompt_store {
                                    let preferences = extension_preferences(
                                        store
                                            .set_profile_preferences(&model, level)
                                            .map_err(agent_error)?,
                                    );
                                    send_extension_activity(
                                        &connection,
                                        &prompt_extension,
                                        None,
                                        ActivityEvent::ProfilePreferencesChanged { preferences },
                                    )?;
                                }
                            }
                            return responder.respond(protocol::PromptResponse::new(
                                protocol::StopReason::EndTurn,
                            ));
                        }
                        SlashCommand::Login => {
                            let login = prompt_auth
                                .start_browser_login()
                                .await
                                .map_err(agent_error)?;
                            send_text(
                                &connection,
                                &session_id,
                                format!(
                                    "Authorize AxiomCLI in your browser. Confirm code {}.\n{}",
                                    login.user_code(),
                                    login.authorization_url()
                                ),
                            )?;
                            let account = login
                                .complete(CancellationToken::new())
                                .await
                                .map_err(agent_error)?;
                            send_text(
                                &connection,
                                &session_id,
                                format!(
                                    "Signed in as credential {} (stored in {}).",
                                    account.key_name,
                                    account.source.label()
                                ),
                            )?;
                            return responder.respond(protocol::PromptResponse::new(
                                protocol::StopReason::EndTurn,
                            ));
                        }
                        SlashCommand::Logout => {
                            prompt_auth.logout().map_err(agent_error)?;
                            send_text(
                                &connection,
                                &session_id,
                                "Signed out. The saved Axiom authorization was removed.".into(),
                            )?;
                            return responder.respond(protocol::PromptResponse::new(
                                protocol::StopReason::EndTurn,
                            ));
                        }
                        SlashCommand::Account => {
                            let text = match prompt_auth.validate().await {
                                ValidationStatus::Missing => {
                                    "Not signed in. Run /login to connect AxiomCLI.".to_owned()
                                }
                                ValidationStatus::Valid(account) => format!(
                                    "Axiom authorization is valid: {} (stored in {}).",
                                    account.key_name,
                                    account.source.label()
                                ),
                                ValidationStatus::Invalid => {
                                    "The saved Axiom authorization is invalid or revoked."
                                        .to_owned()
                                }
                                ValidationStatus::Unavailable(message) => message,
                            };
                            send_text(&connection, &session_id, text)?;
                            return responder.respond(protocol::PromptResponse::new(
                                protocol::StopReason::EndTurn,
                            ));
                        }
                        SlashCommand::Security => {
                            let model = {
                                let sessions = prompt_sessions.lock().await;
                                let Some(session) = sessions.get(&session_id) else {
                                    return responder
                                        .respond_with_internal_error("unknown ACP session");
                                };
                                session.model.clone()
                            };
                            let verification = runner
                                .verify_security(&model, CancellationToken::new())
                                .await
                                .map_err(agent_error)?;
                            strict_extension_security_verification(verification.clone())
                                .map_err(agent_error)?;
                            send_text(
                                &connection,
                                &session_id,
                                security_summary(&verification),
                            )?;
                            return responder.respond(protocol::PromptResponse::new(
                                protocol::StopReason::EndTurn,
                            ));
                        }
                        SlashCommand::Refresh => {
                            let model = {
                                let mut sessions = prompt_sessions.lock().await;
                                let Some(session) = sessions.get_mut(&session_id) else {
                                    return responder
                                        .respond_with_internal_error("unknown ACP session");
                                };
                                session.security = crate::app::SecurityStatus::Verifying;
                                session.model.clone()
                            };
                            send_extension_activity(
                                &connection,
                                &prompt_extension,
                                Some(&session_id),
                                ActivityEvent::SecurityChanged {
                                    status: extension_security_status(
                                        crate::app::SecurityStatus::Verifying,
                                    ),
                                },
                            )?;
                            let verification = match runner
                                .verify_security(&model, CancellationToken::new())
                                .await
                            {
                                Ok(verification) => verification,
                                Err(error) => {
                                    if let Some(session) =
                                        prompt_sessions.lock().await.get_mut(&session_id)
                                    {
                                        session.security = crate::app::SecurityStatus::Failed;
                                    }
                                    send_extension_activity(
                                        &connection,
                                        &prompt_extension,
                                        Some(&session_id),
                                        ActivityEvent::SecurityChanged {
                                            status: ExtensionSecurityStatus {
                                                state: ExtensionSecurityState::Failed,
                                                detail: Some(error.to_string()),
                                            },
                                        },
                                    )?;
                                    return responder.respond_with_internal_error(error);
                                }
                            };
                            let verification_status =
                                match strict_extension_security_verification(verification.clone()) {
                                    Ok(result) => result,
                                    Err(error) => {
                                        if let Some(session) =
                                            prompt_sessions.lock().await.get_mut(&session_id)
                                        {
                                            session.security = crate::app::SecurityStatus::Failed;
                                        }
                                        send_extension_activity(
                                            &connection,
                                            &prompt_extension,
                                            Some(&session_id),
                                            ActivityEvent::SecurityChanged {
                                                status: ExtensionSecurityStatus {
                                                    state: ExtensionSecurityState::Failed,
                                                    detail: Some(error.to_string()),
                                                },
                                            },
                                        )?;
                                        return responder.respond_with_internal_error(error);
                                    }
                                }
                                .0;
                            if let Some(session) = prompt_sessions.lock().await.get_mut(&session_id) {
                                session.security = verification_status;
                            }
                            send_extension_activity(
                                &connection,
                                &prompt_extension,
                                Some(&session_id),
                                ActivityEvent::SecurityChanged {
                                    status: extension_security_status(verification_status),
                                },
                            )?;
                            send_text(
                                &connection,
                                &session_id,
                                format!(
                                    "Attestation refreshed.\n{}",
                                    security_summary(&verification)
                                ),
                            )?;
                            return responder.respond(protocol::PromptResponse::new(
                                protocol::StopReason::EndTurn,
                            ));
                        }
                        SlashCommand::Compact { focus } => {
                            let cancellation = CancellationToken::new();
                            if let Some(session) = prompt_sessions.lock().await.get_mut(&session_id)
                            {
                                session.cancellation = Some(cancellation.clone());
                            }
                            let task_sessions = prompt_sessions.clone();
                            let task_runtime = prompt_runtime.clone();
                            let task_runner = runner.clone();
                            let task_connection = connection.clone();
                            let task_store = prompt_store.clone();
                            let task_extension = prompt_extension.clone();
                            connection.spawn(async move {
                                let (events_tx, mut events_rx) =
                                    mpsc::channel(APP_EVENT_QUEUE_CAPACITY);
                                let compact = task_runner.compact(
                                    &internal_id,
                                    focus,
                                    events_tx,
                                    cancellation.clone(),
                                );
                                tokio::pin!(compact);
                                let result = loop {
                                    tokio::select! {
                                        result = &mut compact => break result,
                                        event = events_rx.recv() => {
                                            let Some(event) = event else { continue };
                                            let envelope = task_runtime
                                                .emit(internal_id.clone(), event.clone())
                                                .await
                                                .map_err(agent_error)?;
                                            let revision = match &task_store {
                                                Some(store) => Some(store.append(&envelope).map_err(agent_error)?),
                                                None => None,
                                            };
                                            send_event(
                                                &task_connection,
                                                &session_id,
                                                event,
                                                EventDelivery {
                                                    cwd: None,
                                                    extension_state: Some(&task_extension),
                                                    correlation_id: Some(envelope.correlation_id.to_string()),
                                                    revision: revision.as_ref(),
                                                    client_item_id: None,
                                                },
                                            )?;
                                        }
                                    }
                                };
                                while let Ok(event) = events_rx.try_recv() {
                                    let envelope = task_runtime
                                        .emit(internal_id.clone(), event.clone())
                                        .await
                                        .map_err(agent_error)?;
                                    let revision = match &task_store {
                                        Some(store) => Some(store.append(&envelope).map_err(agent_error)?),
                                        None => None,
                                    };
                                    send_event(
                                        &task_connection,
                                        &session_id,
                                        event,
                                        EventDelivery {
                                            cwd: None,
                                            extension_state: Some(&task_extension),
                                            correlation_id: Some(envelope.correlation_id.to_string()),
                                            revision: revision.as_ref(),
                                            client_item_id: None,
                                        },
                                    )?;
                                }
                                let stop_reason = match result {
                                    Ok(result) => {
                                        let events = task_runtime
                                            .dispatch(AppCommand::RecordContextCompaction {
                                                session_id: internal_id.clone(),
                                                summary: result.summary,
                                                messages_before: result.messages_before,
                                            })
                                            .await
                                            .map_err(agent_error)?;
                                        if let Some(store) = &task_store {
                                            store.append_all(&events).map_err(agent_error)?;
                                        }
                                        send_text(
                                            &task_connection,
                                            &session_id,
                                            format!(
                                                "Context compacted from {} conversation message(s).",
                                                result.messages_before
                                            ),
                                        )?;
                                        protocol::StopReason::EndTurn
                                    }
                                    Err(crate::AxiomError::Cancelled) => {
                                        protocol::StopReason::Cancelled
                                    }
                                    Err(error) => {
                                        send_text(
                                            &task_connection,
                                            &session_id,
                                            format!("AxiomCLI compaction error: {error}"),
                                        )?;
                                        protocol::StopReason::EndTurn
                                    }
                                };
                                if let Some(session) =
                                    task_sessions.lock().await.get_mut(&session_id)
                                {
                                    session.cancellation = None;
                                }
                                responder.respond(protocol::PromptResponse::new(stop_reason))
                            })?;
                            return Ok(());
                        }
                    }
                }
                let deterministic_test_runner = cfg!(debug_assertions)
                    && std::env::var("AXIOMCLI_TEST_RUNNER")
                        .is_ok_and(|value| !value.trim().is_empty());
                if !prompt_auth.has_credential() && !deterministic_test_runner {
                    send_text(
                        &connection,
                        &session_id,
                        "Authentication required. Run /login to connect an Axiom account."
                            .into(),
                    )?;
                    return responder.respond(protocol::PromptResponse::new(
                        protocol::StopReason::EndTurn,
                    ));
                }
                let turn_id = TurnId::new();
                let request_cancellation = responder.cancellation();
                let cancellation = CancellationToken::new();
                let (
                    internal_id,
                    cwd,
                    session_profile,
                    session_model,
                    session_thinking,
                    should_generate_title,
                ) = {
                    let mut sessions = prompt_sessions.lock().await;
                    let Some(session) = sessions.get_mut(&session_id) else {
                        return responder.respond_with_internal_error("unknown ACP session");
                    };
                    if session.cancellation.is_some() {
                        return responder
                            .respond_with_internal_error("session already has an active turn");
                    }
                    session.active_turn = Some(turn_id.clone());
                    session.cancellation = Some(cancellation.clone());
                    let should_generate_title = !session.has_prompt;
                    session.has_prompt = true;
                    (
                        session.internal_id.clone(),
                        session.cwd.clone(),
                        session.profile,
                        session.model.clone(),
                        session.thinking,
                        should_generate_title,
                    )
                };

                let submitted = prompt_runtime
                    .dispatch(AppCommand::SubmitPrompt {
                        session_id: internal_id.clone(),
                        turn_id: turn_id.clone(),
                        text: prompt.clone(),
                    })
                    .await
                    .map_err(agent_error)?;
                let submitted_revision = if let Some(store) = &prompt_store {
                    let revision = store
                        .append_all_with_client_item_id(&submitted, client_item_id.as_deref())
                        .map_err(agent_error)?;
                    let preferences = extension_preferences(
                        store
                            .set_profile_preferences(&session_model, session_thinking)
                            .map_err(agent_error)?,
                    );
                    send_extension_activity(
                        &connection,
                        &prompt_extension,
                        None,
                        ActivityEvent::ProfilePreferencesChanged { preferences },
                    )?;
                    Some(revision)
                } else {
                    None
                };
                for envelope in &submitted {
                    send_event(
                        &connection,
                        &session_id,
                        envelope.event.clone(),
                        EventDelivery {
                            cwd: Some(&cwd),
                            extension_state: Some(&prompt_extension),
                            correlation_id: Some(envelope.correlation_id.to_string()),
                            revision: submitted_revision.as_ref(),
                            client_item_id: client_item_id.as_deref(),
                        },
                    )?;
                }

                if should_generate_title
                    && let Some(title_store) = prompt_store.clone()
                {
                    let title_runner = runner.clone();
                    let title_session = internal_id.clone();
                    let title_prompt = prompt.clone();
                    let title_connection = connection.clone();
                    let title_external_id = session_id.clone();
                    tokio::spawn(async move {
                        match title_runner
                            .generate_title(
                                &title_session,
                                &title_prompt,
                                CancellationToken::new(),
                            )
                            .await
                        {
                            Ok(title) => match title_store
                                .set_title_if_absent(&title_session, &title)
                            {
                                Ok(true) => {
                                    if let Err(error) = title_connection.send_notification(
                                        protocol::SessionNotification::new(
                                            title_external_id.clone(),
                                            protocol::SessionUpdate::SessionInfoUpdate(
                                                protocol::SessionInfoUpdate::new()
                                                    .title(title.clone()),
                                            ),
                                        ),
                                    ) {
                                        tracing::warn!(
                                            session_id = %title_session,
                                            %error,
                                            "could not publish generated ACP session title"
                                        );
                                    }
                                }
                                Ok(false) => {}
                                Err(error) => tracing::warn!(
                                    session_id = %title_session,
                                    %error,
                                    "could not persist generated ACP session title"
                                ),
                            },
                            Err(error) => tracing::warn!(
                                session_id = %title_session,
                                %error,
                                "background ACP session title generation failed"
                            ),
                        }
                    });
                }

                let task_sessions = prompt_sessions.clone();
                let task_runtime = prompt_runtime.clone();
                let task_runner = runner.clone();
                let task_connection = connection.clone();
                let task_store = prompt_store.clone();
                let task_elicitation = supports_elicitation.clone();
                let task_extension = prompt_extension.clone();
                connection.spawn(async move {
                    let (events_tx, mut events_rx) = mpsc::channel(APP_EVENT_QUEUE_CAPACITY);
                    let display_cwd = cwd.clone();
                    let run = task_runner.run(
                        crate::agent::TurnContext {
                            session_id: internal_id.clone(),
                            turn_id: turn_id.clone(),
                            cwd,
                            permission_profile: session_profile,
                            approval: Some(Arc::new(AcpApproval {
                                connection: task_connection.clone(),
                                session_id: session_id.clone(),
                            })),
                            questions: Some(Arc::new(AcpQuestions {
                                connection: task_connection.clone(),
                                session_id: session_id.clone(),
                                supported: task_elicitation,
                            })),
                        },
                        prompt,
                        events_tx,
                        cancellation.clone(),
                    );
                    tokio::pin!(run);

                    let result = loop {
                        tokio::select! {
                            result = &mut run => break result,
                            event = events_rx.recv() => {
                                let Some(event) = event else { continue };
                                let envelope = task_runtime.emit(internal_id.clone(), event.clone()).await.map_err(agent_error)?;
                                let revision = match &task_store {
                                    Some(store) => Some(store.append(&envelope).map_err(agent_error)?),
                                    None => None,
                                };
                                send_event(
                                    &task_connection,
                                    &session_id,
                                    event,
                                    EventDelivery {
                                        cwd: Some(&display_cwd),
                                        extension_state: Some(&task_extension),
                                        correlation_id: Some(envelope.correlation_id.to_string()),
                                        revision: revision.as_ref(),
                                        client_item_id: None,
                                    },
                                )?;
                            }
                            () = request_cancellation.cancelled() => {
                                cancellation.cancel();
                                break Err(crate::AxiomError::Cancelled);
                            }
                        }
                    };
                    if result.is_err() {
                        cancellation.cancel();
                    }
                    while let Ok(event) = events_rx.try_recv() {
                        let envelope = task_runtime
                            .emit(internal_id.clone(), event.clone())
                            .await
                            .map_err(agent_error)?;
                        let revision = match &task_store {
                            Some(store) => Some(store.append(&envelope).map_err(agent_error)?),
                            None => None,
                        };
                        send_event(
                            &task_connection,
                            &session_id,
                            event,
                            EventDelivery {
                                cwd: Some(&display_cwd),
                                extension_state: Some(&task_extension),
                                correlation_id: Some(envelope.correlation_id.to_string()),
                                revision: revision.as_ref(),
                                client_item_id: None,
                            },
                        )?;
                    }

                    let stop_reason = match result {
                        Ok(()) => {
                            let completed = task_runtime
                                .dispatch(AppCommand::FinishTurn {
                                    session_id: internal_id,
                                    turn_id,
                                })
                                .await
                                .map_err(agent_error)?;
                            if let Some(store) = &task_store {
                                store.append_all(&completed).map_err(agent_error)?;
                            }
                            protocol::StopReason::EndTurn
                        }
                        Err(crate::AxiomError::Cancelled) => {
                            let completed = task_runtime
                                .dispatch(AppCommand::CancelTurn {
                                    session_id: internal_id,
                                    turn_id,
                                })
                                .await
                                .map_err(agent_error)?;
                            if let Some(store) = &task_store {
                                store.append_all(&completed).map_err(agent_error)?;
                            }
                            protocol::StopReason::Cancelled
                        }
                        Err(error) => {
                            let completed = task_runtime
                                .dispatch(AppCommand::FailTurn {
                                    session_id: internal_id,
                                    turn_id,
                                    message: error.to_string(),
                                })
                                .await
                                .map_err(agent_error)?;
                            if let Some(store) = &task_store {
                                store.append_all(&completed).map_err(agent_error)?;
                            }
                            send_text(
                                &task_connection,
                                &session_id,
                                format!("AxiomCLI error: {error}"),
                            )?;
                            protocol::StopReason::EndTurn
                        }
                    };
                    if let Some(session) = task_sessions.lock().await.get_mut(&session_id) {
                        session.active_turn = None;
                        session.cancellation = None;
                    }
                    responder.respond(protocol::PromptResponse::new(stop_reason))
                })?;
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: ListThreadsRequest,
                        responder: Responder<ListThreadsResponse>,
                        _connection| {
                if !extension_enabled(&list_extension, ExtensionFeature::ThreadCatalog) {
                    return responder.respond_with_error(extension_not_negotiated());
                }
                let Some(store) = &list_extension_store else {
                    return responder.respond_with_internal_error(
                        "durable session storage is unavailable",
                    );
                };
                if request
                    .query
                    .as_ref()
                    .is_some_and(|query| query.len() > extension::MAX_QUERY_BYTES)
                {
                    return responder.respond_with_internal_error("session query is too long");
                }
                let query = request.query.as_deref().map(str::trim).filter(|query| !query.is_empty());
                let limit = usize::try_from(request.limit.unwrap_or(200).clamp(1, 200))
                    .unwrap_or(200);
                let page = store
                    .thread_catalog(
                        query,
                        request.include_archived,
                        request.cursor.as_deref(),
                        limit,
                    )
                    .map_err(agent_error)?;
                responder.respond(ListThreadsResponse {
                    threads: page.threads.into_iter().map(extension_thread_summary).collect(),
                    next_cursor: page.next_cursor,
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: GetThreadTimelineRequest,
                        responder: Responder<GetThreadTimelineResponse>,
                        _connection| {
                if !extension_enabled(&timeline_extension, ExtensionFeature::Timeline) {
                    return responder.respond_with_error(extension_not_negotiated());
                }
                let Some(store) = &timeline_extension_store else {
                    return responder.respond_with_internal_error(
                        "durable thread storage is unavailable",
                    );
                };
                let thread_id = SessionId::from_str(&request.thread_id)
                    .map_err(|_| agent_client_protocol::util::internal_error("invalid thread ID"))?;
                let limit = usize::try_from(
                    request.limit.unwrap_or(250).clamp(1, extension::MAX_TIMELINE_PAGE_SIZE),
                )
                .unwrap_or(250);
                let snapshot = store
                    .thread_snapshot(&thread_id, request.after_sequence, limit)
                    .map_err(agent_error)?;
                responder.respond(GetThreadTimelineResponse {
                    thread: extension_thread_summary(snapshot.thread),
                    items: snapshot.items.into_iter().map(extension_timeline_item).collect(),
                    next_cursor: snapshot.next_cursor,
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: DeletePreviewRequest,
                        responder: Responder<DeletePreviewResponse>,
                        _connection| {
                if !extension_enabled(&preview_extension, ExtensionFeature::ThreadCatalog) {
                    return responder.respond_with_error(extension_not_negotiated());
                }
                let Some(store) = &preview_extension_store else {
                    return responder.respond_with_internal_error(
                        "durable session storage is unavailable",
                    );
                };
                let ids = validate_session_ids(&request.thread_ids).map_err(agent_error)?;
                {
                    let sessions = preview_extension_sessions.lock().await;
                    if ids.iter().any(|id| {
                        sessions.values().any(|session| {
                            &session.internal_id == id && session.cancellation.is_some()
                        })
                    }) {
                        return responder.respond_with_internal_error(
                            "sessions with active work must be cancelled before deletion",
                        );
                    }
                }
                let mut summaries = Vec::with_capacity(ids.len());
                for id in &ids {
                    summaries.push(extension_thread_summary(
                        store.thread_summary(id).map_err(agent_error)?,
                    ));
                }
                let token = uuid::Uuid::new_v4().to_string();
                let expires_at_unix_seconds = now_unix_seconds().saturating_add(60);
                let mut pending = preview_extension.pending_deletes.lock().await;
                pending.retain(|_, grant| grant.expires_at_unix_seconds > now_unix_seconds());
                pending.insert(
                    token.clone(),
                    PendingDelete {
                        session_ids: ids,
                        expires_at_unix_seconds,
                    },
                );
                responder.respond(DeletePreviewResponse {
                    confirmation_token: token,
                    expires_at_unix_seconds,
                    threads: summaries,
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: DeleteConfirmRequest,
                        responder: Responder<DeleteConfirmResponse>,
                        _connection| {
                if !extension_enabled(&delete_extension, ExtensionFeature::ThreadCatalog) {
                    return responder.respond_with_error(extension_not_negotiated());
                }
                let Some(store) = &delete_extension_store else {
                    return responder.respond_with_internal_error(
                        "durable session storage is unavailable",
                    );
                };
                let requested = match validate_session_ids(&request.thread_ids) {
                    Ok(requested) => requested,
                    Err(error) => return responder.respond_with_internal_error(error),
                };
                let Some(grant) = delete_extension
                    .pending_deletes
                    .lock()
                    .await
                    .remove(&request.confirmation_token)
                else {
                    return responder.respond_with_internal_error(
                        "delete confirmation is missing or already used",
                    );
                };
                if grant.expires_at_unix_seconds <= now_unix_seconds() {
                    return responder.respond_with_internal_error("delete confirmation expired");
                }
                if requested != grant.session_ids {
                    return responder.respond_with_internal_error(
                        "delete selection does not match the preview",
                    );
                }
                delete_extension_sessions
                    .lock()
                    .await
                    .retain(|_, session| !requested.contains(&session.internal_id));
                let deleted = match store.delete_sessions(&requested) {
                    Ok(deleted) => deleted,
                    Err(error) => return responder.respond_with_internal_error(error),
                };
                responder.respond(DeleteConfirmResponse {
                    deleted: u32::try_from(deleted).unwrap_or(u32::MAX),
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |_request: ListModelsRequest,
                        responder: Responder<ListModelsResponse>,
                        _connection| {
                if !extension_enabled(&models_extension, ExtensionFeature::ModelCatalog) {
                    return responder.respond_with_error(extension_not_negotiated());
                }
                let models = models_extension_runner
                    .available_model_details(CancellationToken::new())
                    .await
                    .map_err(agent_error)?
                    .into_iter()
                    .map(|model| {
                        if model.id.trim().is_empty()
                            || model.label.trim().is_empty()
                            || model.short_label.trim().is_empty()
                            || model.provider_id.trim().is_empty()
                            || model.provider_label.trim().is_empty()
                            || model.upstream_model.trim().is_empty()
                        {
                            return Err(agent_client_protocol::util::internal_error(
                                "provider catalog returned incomplete model metadata",
                            ));
                        }
                        Ok(ModelInfo {
                            label: model.label,
                            short_label: model.short_label,
                            provider_id: model.provider_id,
                            provider_label: model.provider_label,
                            upstream_model: model.upstream_model,
                            id: model.id,
                            thinking_levels: model
                                .supported_reasoning_efforts
                                .into_iter()
                                .map(|level| level.as_str().to_owned())
                                .collect(),
                        })
                    })
                    .collect::<agent_client_protocol::Result<Vec<_>>>()?;
                responder.respond(ListModelsResponse { models })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |_request: extension::GetProfilePreferencesRequest,
                        responder: Responder<ProfilePreferencesResponse>,
                        _connection| {
                if !extension_enabled(
                    &stored_preferences_extension,
                    ExtensionFeature::ProfilePreferences,
                ) {
                    return responder.respond_with_error(extension_not_negotiated());
                }
                let Some(store) = &stored_preferences_store else {
                    return responder.respond_with_internal_error(
                        "durable profile preferences are unavailable",
                    );
                };
                responder.respond(ProfilePreferencesResponse {
                    preferences: extension_preferences(
                        store.profile_preferences().map_err(agent_error)?,
                    ),
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: extension::SetProfilePreferencesRequest,
                        responder: Responder<ProfilePreferencesResponse>,
                        connection: ConnectionTo<Client>| {
                if !extension_enabled(
                    &update_preferences_extension,
                    ExtensionFeature::ProfilePreferences,
                ) {
                    return responder.respond_with_error(extension_not_negotiated());
                }
                let Some(store) = &update_preferences_store else {
                    return responder.respond_with_internal_error(
                        "durable profile preferences are unavailable",
                    );
                };
                if request.model.as_ref().is_some_and(|value| {
                    value.is_empty() || value.len() > extension::MAX_IDENTIFIER_BYTES
                })
                    || request
                        .thinking_level
                        .as_ref()
                        .is_some_and(|value| {
                            value.is_empty() || value.len() > extension::MAX_IDENTIFIER_BYTES
                        })
                {
                    return responder.respond_with_internal_error("profile preference is invalid");
                }
                let current = store.profile_preferences().map_err(agent_error)?;
                let Some(next_model) = request.model.or(current.model) else {
                    return responder.respond_with_internal_error(
                        "a model must be selected before saving profile preferences",
                    );
                };
                let next_thinking = request
                    .thinking_level
                    .as_deref()
                    .map(crate::app::ThinkingLevel::from_str)
                    .transpose()
                    .map_err(agent_error)?
                    .unwrap_or(current.thinking_level);
                let models = update_preferences_runner
                    .available_model_details(CancellationToken::new())
                    .await
                    .map_err(agent_error)?;
                let Some(model) = models.iter().find(|model| model.id == next_model) else {
                    return responder
                        .respond_with_internal_error("model is not in the provider catalog");
                };
                let next_thinking = reconcile_model_settings(model, next_thinking).thinking;
                let preferences = extension_preferences(
                    store
                        .set_profile_preferences(&next_model, next_thinking)
                        .map_err(agent_error)?,
                );
                send_extension_activity(
                    &connection,
                    &update_preferences_extension,
                    None,
                    ActivityEvent::ProfilePreferencesChanged {
                        preferences: preferences.clone(),
                    },
                )?;
                responder.respond(ProfilePreferencesResponse { preferences })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |_request: extension::ListCollectionsRequest,
                        responder: Responder<CollectionStateResponse>,
                        _connection| {
                if !extension_enabled(&collections_list_extension, ExtensionFeature::Collections) {
                    return responder.respond_with_error(extension_not_negotiated());
                }
                let Some(store) = &collections_list_store else {
                    return responder.respond_with_internal_error("collections are unavailable");
                };
                responder.respond(CollectionStateResponse {
                    state: extension_collections(store.list_collections().map_err(agent_error)?),
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: extension::CreateCollectionRequest,
                        responder: Responder<CollectionStateResponse>,
                        connection: ConnectionTo<Client>| {
                if !extension_enabled(&collection_create_extension, ExtensionFeature::Collections) {
                    return responder.respond_with_error(extension_not_negotiated());
                }
                let Some(store) = &collection_create_store else {
                    return responder.respond_with_internal_error("collections are unavailable");
                };
                if request.name.len() > extension::MAX_COLLECTION_NAME_BYTES {
                    return responder.respond_with_internal_error("collection name is too long");
                }
                let state = extension_collections(store.create_collection(&request.name).map_err(agent_error)?.state);
                send_extension_activity(&connection, &collection_create_extension, None, ActivityEvent::CollectionsChanged { state: state.clone() })?;
                responder.respond(CollectionStateResponse { state })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: extension::RenameCollectionRequest,
                        responder: Responder<CollectionStateResponse>,
                        connection: ConnectionTo<Client>| {
                if !extension_enabled(&collection_rename_extension, ExtensionFeature::Collections) {
                    return responder.respond_with_error(extension_not_negotiated());
                }
                let Some(store) = &collection_rename_store else {
                    return responder.respond_with_internal_error("collections are unavailable");
                };
                if request.name.len() > extension::MAX_COLLECTION_NAME_BYTES {
                    return responder.respond_with_internal_error("collection name is too long");
                }
                let state = extension_collections(store.rename_collection(&request.collection_id, &request.name).map_err(agent_error)?.state);
                send_extension_activity(&connection, &collection_rename_extension, None, ActivityEvent::CollectionsChanged { state: state.clone() })?;
                responder.respond(CollectionStateResponse { state })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: extension::SetCollectionCollapsedRequest,
                        responder: Responder<CollectionStateResponse>,
                        connection: ConnectionTo<Client>| {
                if !extension_enabled(&collection_collapsed_extension, ExtensionFeature::Collections) {
                    return responder.respond_with_error(extension_not_negotiated());
                }
                let Some(store) = &collection_collapsed_store else {
                    return responder.respond_with_internal_error("collections are unavailable");
                };
                let state = extension_collections(store.set_collection_collapsed(&request.collection_id, request.collapsed).map_err(agent_error)?.state);
                send_extension_activity(&connection, &collection_collapsed_extension, None, ActivityEvent::CollectionsChanged { state: state.clone() })?;
                responder.respond(CollectionStateResponse { state })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: extension::MoveCollectionRequest,
                        responder: Responder<CollectionStateResponse>,
                        connection: ConnectionTo<Client>| {
                if !extension_enabled(&collection_move_extension, ExtensionFeature::Collections) {
                    return responder.respond_with_error(extension_not_negotiated());
                }
                let Some(store) = &collection_move_store else {
                    return responder.respond_with_internal_error("collections are unavailable");
                };
                let state = extension_collections(store.move_collection(&request.collection_id, request.position).map_err(agent_error)?.state);
                send_extension_activity(&connection, &collection_move_extension, None, ActivityEvent::CollectionsChanged { state: state.clone() })?;
                responder.respond(CollectionStateResponse { state })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: extension::DeleteCollectionRequest,
                        responder: Responder<CollectionStateResponse>,
                        connection: ConnectionTo<Client>| {
                if !extension_enabled(&collection_delete_extension, ExtensionFeature::Collections) {
                    return responder.respond_with_error(extension_not_negotiated());
                }
                let Some(store) = &collection_delete_store else {
                    return responder.respond_with_internal_error("collections are unavailable");
                };
                let state = extension_collections(store.delete_collection(&request.collection_id).map_err(agent_error)?.state);
                send_extension_activity(&connection, &collection_delete_extension, None, ActivityEvent::CollectionsChanged { state: state.clone() })?;
                responder.respond(CollectionStateResponse { state })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: extension::AssignThreadCollectionRequest,
                        responder: Responder<extension::AssignThreadCollectionResponse>,
                        connection: ConnectionTo<Client>| {
                if !extension_enabled(&collection_assign_extension, ExtensionFeature::Collections) {
                    return responder.respond_with_error(extension_not_negotiated());
                }
                let Some(store) = &collection_assign_store else {
                    return responder.respond_with_internal_error("collections are unavailable");
                };
                let thread_id = SessionId::from_str(&request.thread_id)
                    .map_err(|_| agent_client_protocol::util::internal_error("invalid thread ID"))?;
                let state = extension_collections(store.assign_thread_collection(&thread_id, request.collection_id.as_deref()).map_err(agent_error)?.state);
                let revision = store.thread_revision(&thread_id).map_err(agent_error)?;
                send_extension_activity(&connection, &collection_assign_extension, None, ActivityEvent::CollectionsChanged { state: state.clone() })?;
                responder.respond(extension::AssignThreadCollectionResponse {
                    state,
                    thread_revision: revision.revision,
                    last_timeline_sequence: revision.last_timeline_sequence,
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |_request: extension::AccountStatusRequest,
                        responder: Responder<extension::AccountStatusResponse>,
                        _connection| {
                if !extension_enabled(&account_status_extension, ExtensionFeature::Account) {
                    return responder.respond_with_error(extension_not_negotiated());
                }
                let status = extension_account_status(account_status_auth.validate().await);
                responder.respond(extension::AccountStatusResponse { status })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |_request: BrowserLoginStartRequest,
                        responder: Responder<BrowserLoginStartResponse>,
                        _connection| {
                if !extension_enabled(&login_start_extension, ExtensionFeature::Account) {
                    return responder.respond_with_error(extension_not_negotiated());
                }
                let login = login_start_auth
                    .start_browser_login()
                    .await
                    .map_err(agent_error)?;
                let login_id = uuid::Uuid::new_v4().to_string();
                let state = BrowserLoginState {
                    login_id: login_id.clone(),
                    user_code: login.user_code().into(),
                    authorization_url: login.authorization_url().into(),
                    browser_opened: login.browser_opened(),
                };
                login_start_extension
                    .browser_logins
                    .lock()
                    .await
                    .insert(login_id, login);
                responder.respond(BrowserLoginStartResponse { login: state })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: BrowserLoginCompleteRequest,
                        responder: Responder<extension::AccountStatusResponse>,
                        connection: ConnectionTo<Client>| {
                if !extension_enabled(&login_complete_extension, ExtensionFeature::Account) {
                    return responder.respond_with_error(extension_not_negotiated());
                }
                let login = login_complete_extension
                    .browser_logins
                    .lock()
                    .await
                    .remove(&request.login_id)
                    .ok_or_else(|| {
                        agent_client_protocol::util::internal_error(
                            "browser login is missing, expired, or already completed",
                        )
                    })?;
                let request_cancellation = responder.cancellation();
                let cancellation = CancellationToken::new();
                let completion = login.complete(cancellation.clone());
                tokio::pin!(completion);
                let account = tokio::select! {
                    account = &mut completion => account.map_err(agent_error)?,
                    () = request_cancellation.cancelled() => {
                        cancellation.cancel();
                        return responder.respond_with_error(
                            agent_client_protocol::Error::request_cancelled(),
                        );
                    }
                };
                let status = extension_account_status(ValidationStatus::Valid(account));
                send_extension_activity(
                    &connection,
                    &login_complete_extension,
                    None,
                    ActivityEvent::AccountChanged {
                        status: status.clone(),
                    },
                )?;
                // Revalidate through the manager so a server-side revocation racing
                // completion still fails closed before the next prompt.
                let status = extension_account_status(login_complete_auth.validate().await);
                responder.respond(extension::AccountStatusResponse { status })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: UseApiKeyRequest,
                        responder: Responder<extension::AccountStatusResponse>,
                        connection: ConnectionTo<Client>| {
                if !extension_enabled(&api_key_extension, ExtensionFeature::Account) {
                    return responder.respond_with_error(extension_not_negotiated());
                }
                if request.api_key.len() > extension::MAX_API_KEY_BYTES {
                    return responder.respond_with_internal_error("Axiom API key is too long");
                }
                api_key_auth
                    .save_pasted_key(&request.api_key)
                    .map_err(agent_error)?;
                let status = extension_account_status(api_key_auth.validate().await);
                send_extension_activity(
                    &connection,
                    &api_key_extension,
                    None,
                    ActivityEvent::AccountChanged {
                        status: status.clone(),
                    },
                )?;
                responder.respond(extension::AccountStatusResponse { status })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |_request: LogoutRequest,
                        responder: Responder<extension::AccountStatusResponse>,
                        connection: ConnectionTo<Client>| {
                if !extension_enabled(&logout_extension, ExtensionFeature::Account) {
                    return responder.respond_with_error(extension_not_negotiated());
                }
                logout_auth.logout().map_err(agent_error)?;
                let status = extension_account_status(ValidationStatus::Missing);
                send_extension_activity(
                    &connection,
                    &logout_extension,
                    None,
                    ActivityEvent::AccountChanged {
                        status: status.clone(),
                    },
                )?;
                responder.respond(extension::AccountStatusResponse { status })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: VerifySecurityRequest,
                        responder: Responder<VerifySecurityResponse>,
                        connection: ConnectionTo<Client>| {
                if !extension_enabled(&security_extension, ExtensionFeature::SecurityEvidence) {
                    return responder.respond_with_error(extension_not_negotiated());
                }
                let external_id = protocol::SessionId::new(request.thread_id);
                let model = {
                    let mut sessions = security_extension_sessions.lock().await;
                    let Some(session) = sessions.get_mut(&external_id) else {
                        return responder.respond_with_internal_error("unknown ACP session");
                    };
                    session.security = crate::app::SecurityStatus::Verifying;
                    session.model.clone()
                };
                send_extension_activity(
                    &connection,
                    &security_extension,
                    Some(&external_id),
                    ActivityEvent::SecurityChanged {
                        status: extension_security_status(crate::app::SecurityStatus::Verifying),
                    },
                )?;
                let request_cancellation = responder.cancellation();
                let cancellation = CancellationToken::new();
                let verify = security_extension_runner.verify_security(&model, cancellation.clone());
                tokio::pin!(verify);
                let verification = tokio::select! {
                    verification = &mut verify => verification,
                    () = request_cancellation.cancelled() => {
                        cancellation.cancel();
                        if let Some(session) = security_extension_sessions.lock().await.get_mut(&external_id) {
                            session.security = crate::app::SecurityStatus::Unverified;
                        }
                        send_extension_activity(
                            &connection,
                            &security_extension,
                            Some(&external_id),
                            ActivityEvent::SecurityChanged {
                                status: extension_security_status(crate::app::SecurityStatus::Unverified),
                            },
                        )?;
                        return responder.respond_with_error(
                            agent_client_protocol::Error::request_cancelled(),
                        );
                    }
                };
                let verification = match verification {
                    Ok(verification) => verification,
                    Err(error) => {
                        if let Some(session) = security_extension_sessions.lock().await.get_mut(&external_id) {
                            session.security = crate::app::SecurityStatus::Failed;
                        }
                        send_extension_activity(
                            &connection,
                            &security_extension,
                            Some(&external_id),
                            ActivityEvent::SecurityChanged {
                                status: ExtensionSecurityStatus {
                                    state: ExtensionSecurityState::Failed,
                                    detail: Some(error.to_string()),
                                },
                            },
                        )?;
                        return responder.respond_with_internal_error(error);
                    }
                };
                let (verification_status, evidence) =
                    match strict_extension_security_verification(verification) {
                        Ok(result) => result,
                        Err(error) => {
                        if let Some(session) = security_extension_sessions
                            .lock()
                            .await
                            .get_mut(&external_id)
                        {
                            session.security = crate::app::SecurityStatus::Failed;
                        }
                        send_extension_activity(
                            &connection,
                            &security_extension,
                            Some(&external_id),
                            ActivityEvent::SecurityChanged {
                                status: ExtensionSecurityStatus {
                                    state: ExtensionSecurityState::Failed,
                                    detail: Some(error.to_string()),
                                },
                            },
                        )?;
                        return responder.respond_with_internal_error(error);
                        }
                    };
                if let Some(session) = security_extension_sessions.lock().await.get_mut(&external_id)
                {
                    session.security = verification_status;
                }
                let status = extension_security_status(verification_status);
                send_extension_activity(
                    &connection,
                    &security_extension,
                    Some(&external_id),
                    ActivityEvent::SecurityChanged {
                        status: status.clone(),
                    },
                )?;
                responder.respond(VerifySecurityResponse { evidence, status })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: CompactRequest,
                        responder: Responder<CompactResponse>,
                        connection: ConnectionTo<Client>| {
                if !extension_enabled(&compact_extension, ExtensionFeature::Compaction) {
                    return responder.respond_with_error(extension_not_negotiated());
                }
                if request
                    .focus
                    .as_ref()
                    .is_some_and(|focus| focus.len() > extension::MAX_FOCUS_BYTES)
                {
                    return responder.respond_with_internal_error("compaction focus is too long");
                }
                let external_id = protocol::SessionId::new(request.thread_id);
                let request_cancellation = responder.cancellation();
                let cancellation = CancellationToken::new();
                let internal_id = {
                    let mut sessions = compact_extension_sessions.lock().await;
                    let Some(session) = sessions.get_mut(&external_id) else {
                        return responder.respond_with_internal_error("unknown ACP session");
                    };
                    if session.cancellation.is_some() {
                        return responder
                            .respond_with_internal_error("session already has active work");
                    }
                    session.cancellation = Some(cancellation.clone());
                    session.internal_id.clone()
                };
                send_extension_activity(
                    &connection,
                    &compact_extension,
                    Some(&external_id),
                    ActivityEvent::Compaction {
                        phase: CompactionPhase::Started,
                        detail: Some("Compacting conversation context".into()),
                    },
                )?;
                let (events_tx, mut events_rx) = mpsc::channel(APP_EVENT_QUEUE_CAPACITY);
                let compact = compact_extension_runner.compact(
                    &internal_id,
                    request.focus,
                    events_tx,
                    cancellation.clone(),
                );
                tokio::pin!(compact);
                let result = loop {
                    tokio::select! {
                        result = &mut compact => break result,
                        event = events_rx.recv() => {
                            let Some(event) = event else { continue };
                            let envelope = compact_extension_runtime
                                .emit(internal_id.clone(), event.clone())
                                .await
                                .map_err(agent_error)?;
                            let revision = match &compact_extension_store {
                                Some(store) => Some(store.append(&envelope).map_err(agent_error)?),
                                None => None,
                            };
                            send_event(
                                &connection,
                                &external_id,
                                event,
                                EventDelivery {
                                    cwd: None,
                                    extension_state: Some(&compact_extension),
                                    correlation_id: Some(envelope.correlation_id.to_string()),
                                    revision: revision.as_ref(),
                                    client_item_id: None,
                                },
                            )?;
                        }
                        () = request_cancellation.cancelled() => {
                            cancellation.cancel();
                            break Err(crate::AxiomError::Cancelled);
                        }
                    }
                };
                if let Some(session) = compact_extension_sessions.lock().await.get_mut(&external_id)
                {
                    session.cancellation = None;
                }
                match result {
                    Ok(result) => {
                        let messages_before = result.messages_before;
                        let events = compact_extension_runtime
                            .dispatch(AppCommand::RecordContextCompaction {
                                session_id: internal_id.clone(),
                                summary: result.summary,
                                messages_before,
                            })
                            .await
                            .map_err(agent_error)?;
                        let revision = match &compact_extension_store {
                            Some(store) => Some(store.append_all(&events).map_err(agent_error)?),
                            None => None,
                        };
                        for event in events {
                            send_event(
                                &connection,
                                &external_id,
                                event.event,
                                EventDelivery {
                                    cwd: None,
                                    extension_state: Some(&compact_extension),
                                    correlation_id: Some(event.correlation_id.to_string()),
                                    revision: revision.as_ref(),
                                    client_item_id: None,
                                },
                            )?;
                        }
                        responder.respond(CompactResponse {
                            messages_before: u64::try_from(messages_before).unwrap_or(u64::MAX),
                        })
                    }
                    Err(error) => {
                        send_extension_activity(
                            &connection,
                            &compact_extension,
                            Some(&external_id),
                            ActivityEvent::Compaction {
                                phase: if matches!(error, crate::AxiomError::Cancelled) {
                                    CompactionPhase::Cancelled
                                } else {
                                    CompactionPhase::Failed
                                },
                                detail: Some(error.to_string()),
                            },
                        )?;
                        Err(agent_error(error))
                    }
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: protocol::SetSessionModeRequest,
                        responder: Responder<protocol::SetSessionModeResponse>,
                        connection: ConnectionTo<Client>| {
                let profile = PermissionProfile::from_str(request.mode_id.0.as_ref())
                    .map_err(agent_error)?;
                if mode_frontend == FrontendKind::DesktopChat && profile != PermissionProfile::Web {
                    return responder.respond_with_error(extension_rpc_error(
                        -32003,
                        ExtensionErrorCode::PermissionDenied,
                        "desktop chat permission profile is fixed to web",
                        false,
                    ));
                }
                let internal_id = {
                    let sessions = mode_sessions.lock().await;
                    let Some(session) = sessions.get(&request.session_id) else {
                        return responder.respond_with_internal_error("unknown ACP session");
                    };
                    if session.cancellation.is_some() {
                        return responder.respond_with_internal_error(
                            "cannot change permissions while the session has active work",
                        );
                    }
                    session.internal_id.clone()
                };
                let events = mode_runtime
                    .dispatch(AppCommand::ChangePermissionProfile {
                        session_id: internal_id,
                        profile,
                    })
                    .await
                    .map_err(agent_error)?;
                if let Some(store) = &mode_store {
                    store.append_all(&events).map_err(agent_error)?;
                }
                if let Some(session) = mode_sessions.lock().await.get_mut(&request.session_id) {
                    session.profile = profile;
                }
                connection.send_notification(protocol::SessionNotification::new(
                    request.session_id,
                    protocol::SessionUpdate::CurrentModeUpdate(protocol::CurrentModeUpdate::new(
                        profile.to_string(),
                    )),
                ))?;
                responder.respond(protocol::SetSessionModeResponse::new())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: protocol::SetSessionConfigOptionRequest,
                        responder: Responder<protocol::SetSessionConfigOptionResponse>,
                        connection: ConnectionTo<Client>| {
                let Some(value) = request.value.as_value_id() else {
                    return responder.respond_with_internal_error(
                        "session config requires a selected value ID",
                    );
                };
                let selected = value.0.to_string();
                let (internal_id, model, thinking) = {
                    let sessions = config_sessions.lock().await;
                    let Some(session) = sessions.get(&request.session_id) else {
                        return responder.respond_with_internal_error("unknown ACP session");
                    };
                    if session.cancellation.is_some() {
                        return responder.respond_with_internal_error(
                            "cannot change settings while the session has active work",
                        );
                    }
                    (
                        session.internal_id.clone(),
                        session.model.clone(),
                        session.thinking,
                    )
                };
                if !matches!(
                    request.config_id.0.as_ref(),
                    MODEL_CONFIG_ID | THINKING_CONFIG_ID
                ) {
                    return responder.respond_with_internal_error("unknown session config option");
                }
                let details = config_runner
                    .available_model_details(CancellationToken::new())
                    .await
                    .map_err(agent_error)?;
                let mut models = details
                    .iter()
                    .map(|candidate| candidate.id.clone())
                    .collect::<Vec<_>>();
                models.sort();
                models.dedup();
                let (settings, supported_thinking, events) = match request.config_id.0.as_ref() {
                    MODEL_CONFIG_ID => {
                        let Some(selected_model) =
                            details.iter().find(|candidate| candidate.id == selected)
                        else {
                            return responder.respond_with_internal_error(
                                "model is not in the provider catalog",
                            );
                        };
                        let model_changed = selected_model.id != model;
                        let settings = reconcile_model_settings(selected_model, thinking);
                        config_runner
                            .set_model_settings(&internal_id, &settings)
                            .await
                            .map_err(agent_error)?;
                        let events = config_runtime
                            .dispatch(AppCommand::ChangeModelSettings {
                                session_id: internal_id.clone(),
                                model: settings.model.clone(),
                                thinking: settings.thinking,
                                reset_security: model_changed,
                            })
                            .await
                            .map_err(agent_error)?;
                        (
                            settings,
                            supported_thinking_levels(selected_model),
                            events,
                        )
                    }
                    THINKING_CONFIG_ID => {
                        let level = crate::app::ThinkingLevel::from_str(&selected)
                            .map_err(agent_error)?;
                        let Some(selected_model) =
                            details.iter().find(|candidate| candidate.id == model)
                        else {
                            return responder.respond_with_internal_error(
                                "current model is not in the provider catalog",
                            );
                        };
                        let settings = reconcile_model_settings(selected_model, level);
                        if settings.supports_reasoning && settings.thinking != level {
                            return responder.respond_with_internal_error(
                                "thinking level is not supported by the selected model",
                            );
                        }
                        config_runner
                            .set_thinking_level(&internal_id, level)
                            .await
                            .map_err(agent_error)?;
                        let events = config_runtime
                            .dispatch(AppCommand::ChangeThinkingLevel {
                                session_id: internal_id.clone(),
                                level,
                            })
                            .await
                            .map_err(agent_error)?;
                        (
                            settings,
                            supported_thinking_levels(selected_model),
                            events,
                        )
                    }
                    _ => unreachable!("configuration ID was validated above"),
                };
                let reset_security = events.iter().any(|event| {
                    matches!(
                        &event.event,
                        AppEvent::SecurityStatusChanged {
                            state: crate::app::SecurityStatus::Unverified
                        }
                    )
                });
                let next_model = settings.model;
                let next_thinking = settings.thinking;
                if let Some(store) = &config_store {
                    let (_, stored) = store
                        .append_all_and_set_profile_preferences(
                            &events,
                            &next_model,
                            next_thinking,
                        )
                        .map_err(agent_error)?;
                    let preferences = extension_preferences(stored);
                    send_extension_activity(
                        &connection,
                        &config_extension,
                        None,
                        ActivityEvent::ProfilePreferencesChanged { preferences },
                    )?;
                }
                if let Some(session) = config_sessions.lock().await.get_mut(&request.session_id) {
                    apply_session_model_settings(
                        session,
                        &next_model,
                        next_thinking,
                        &models,
                        reset_security,
                    );
                }
                if reset_security {
                    send_extension_activity(
                        &connection,
                        &config_extension,
                        Some(&request.session_id),
                        ActivityEvent::SecurityChanged {
                            status: extension_security_status(
                                crate::app::SecurityStatus::Unverified,
                            ),
                        },
                    )?;
                }
                responder.respond(protocol::SetSessionConfigOptionResponse::new(
                    session_config_options(
                        &next_model,
                        &models,
                        next_thinking,
                        Some(&supported_thinking),
                    ),
                ))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_notification(
            async move |notification: protocol::CancelNotification, _connection| {
                if let Some(token) = cancel_sessions
                    .lock()
                    .await
                    .get(&notification.session_id)
                    .and_then(|session| session.cancellation.clone())
                {
                    token.cancel();
                }
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .connect_to(Stdio::new())
        .await
}

fn prompt_text(blocks: &[protocol::ContentBlock]) -> std::result::Result<String, &'static str> {
    let mut text = String::new();
    for block in blocks {
        let content = match block {
            protocol::ContentBlock::Text(content) => content.text.clone(),
            protocol::ContentBlock::ResourceLink(resource) => format!(
                "[Referenced resource: {}]({})",
                resource.title.as_deref().unwrap_or(&resource.name),
                resource.uri
            ),
            _ => return Err("ACP prompt contains a content type AxiomCLI did not advertise"),
        };
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&content);
    }
    if text.trim().is_empty() {
        Err("AxiomCLI currently requires a non-empty text prompt")
    } else {
        Ok(text)
    }
}

struct EventDelivery<'a> {
    cwd: Option<&'a Path>,
    extension_state: Option<&'a ExtensionState>,
    correlation_id: Option<String>,
    revision: Option<&'a crate::session::ThreadRevision>,
    client_item_id: Option<&'a str>,
}

fn send_event(
    connection: &ConnectionTo<Client>,
    session_id: &protocol::SessionId,
    event: AppEvent,
    delivery: EventDelivery<'_>,
) -> agent_client_protocol::Result<()> {
    let EventDelivery {
        cwd,
        extension_state,
        correlation_id,
        revision,
        client_item_id,
    } = delivery;
    if let Some(extension_state) = extension_state
        && let Some(activity) = extension_activity(&event)
    {
        send_extension_activity_correlated(
            connection,
            extension_state,
            Some(session_id),
            correlation_id.clone(),
            activity,
        )?;
    }
    match event {
        AppEvent::PromptAccepted { turn_id, text } => {
            let message_id =
                client_item_id.map_or_else(|| format!("user:{turn_id}"), ToOwned::to_owned);
            let chunk = protocol::ContentChunk::new(protocol::ContentBlock::Text(
                protocol::TextContent::new(text),
            ))
            .message_id(message_id.as_str());
            send_session_update(
                connection,
                session_id,
                protocol::SessionUpdate::UserMessageChunk(chunk),
                revision,
                None,
                client_item_id.map(ToOwned::to_owned),
            )
        }
        AppEvent::TextDelta { turn_id, text } => {
            let message_id = format!("assistant:{turn_id}");
            let chunk = protocol::ContentChunk::new(protocol::ContentBlock::Text(
                protocol::TextContent::new(text),
            ))
            .message_id(message_id.as_str());
            send_session_update(
                connection,
                session_id,
                protocol::SessionUpdate::AgentMessageChunk(chunk),
                revision,
                None,
                None,
            )
        }
        AppEvent::ReasoningDelta { turn_id, text } => {
            let message_id = format!("reasoning:{turn_id}");
            let chunk = protocol::ContentChunk::new(protocol::ContentBlock::Text(
                protocol::TextContent::new(text),
            ))
            .message_id(message_id.as_str());
            send_session_update(
                connection,
                session_id,
                protocol::SessionUpdate::AgentThoughtChunk(chunk),
                revision,
                None,
                None,
            )
        }
        AppEvent::PermissionProfileChanged { profile } => send_session_update(
            connection,
            session_id,
            protocol::SessionUpdate::CurrentModeUpdate(protocol::CurrentModeUpdate::new(
                profile.to_string(),
            )),
            revision,
            None,
            None,
        ),
        AppEvent::ModelChanged { model } => send_session_update(
            connection,
            session_id,
            protocol::SessionUpdate::ConfigOptionUpdate(protocol::ConfigOptionUpdate::new(vec![
                model_config_option(&model, std::slice::from_ref(&model)),
            ])),
            revision,
            None,
            None,
        ),
        AppEvent::ThinkingLevelChanged { level } => send_session_update(
            connection,
            session_id,
            protocol::SessionUpdate::ConfigOptionUpdate(protocol::ConfigOptionUpdate::new(vec![
                thinking_config_option(level),
            ])),
            revision,
            None,
            None,
        ),
        AppEvent::ProviderStatusChanged { connected, detail } => {
            let message_id = format!("provider:{}", revision.map_or(0, |value| value.revision));
            let chunk = protocol::ContentChunk::new(protocol::ContentBlock::Text(
                protocol::TextContent::new(format!(
                    "[{}] {detail}",
                    if connected {
                        "provider"
                    } else {
                        "provider status"
                    }
                )),
            ))
            .message_id(message_id.as_str());
            send_session_update(
                connection,
                session_id,
                protocol::SessionUpdate::AgentThoughtChunk(chunk),
                revision,
                None,
                None,
            )
        }
        AppEvent::ProgressUpdated {
            message,
            completed,
            total,
        } => {
            let text = match (completed, total) {
                (Some(completed), Some(total)) => {
                    format!("[progress {completed}/{total}] {message}")
                }
                _ => format!("[progress] {message}"),
            };
            let message_id = format!("progress:{}", revision.map_or(0, |value| value.revision));
            let chunk = protocol::ContentChunk::new(protocol::ContentBlock::Text(
                protocol::TextContent::new(text),
            ))
            .message_id(message_id.as_str());
            send_session_update(
                connection,
                session_id,
                protocol::SessionUpdate::AgentThoughtChunk(chunk),
                revision,
                None,
                None,
            )
        }
        AppEvent::TaskListUpdated { items } => send_session_update(
            connection,
            session_id,
            protocol::SessionUpdate::Plan(protocol::Plan::new(
                items
                    .into_iter()
                    .map(|item| {
                        protocol::PlanEntry::new(
                            format!("{} · {}", item.id, item.title),
                            protocol::PlanEntryPriority::Medium,
                            match item.status {
                                crate::app::TaskStatus::Pending => {
                                    protocol::PlanEntryStatus::Pending
                                }
                                crate::app::TaskStatus::InProgress => {
                                    protocol::PlanEntryStatus::InProgress
                                }
                                crate::app::TaskStatus::Completed => {
                                    protocol::PlanEntryStatus::Completed
                                }
                            },
                        )
                    })
                    .collect(),
            )),
            revision,
            None,
            None,
        ),
        AppEvent::ToolProposed {
            call_id,
            name,
            arguments,
            ..
        } => {
            let kind = tool_kind(&name);
            // ACP has no separate field for the provider-neutral tool name. Keep
            // that stable identity in the title for generic clients while making
            // the leading text useful to a person watching the task run.
            let title = format!(
                "{} · {name}",
                describe_tool(&name, &arguments, ToolDisplayPhase::Proposed, cwd)
            );
            send_session_update(
                connection,
                session_id,
                protocol::SessionUpdate::ToolCall(
                    protocol::ToolCall::new(call_id, title)
                        .kind(kind)
                        .status(protocol::ToolCallStatus::Pending)
                        .raw_input(arguments),
                ),
                revision,
                None,
                None,
            )
        }
        AppEvent::ToolStarted { call_id, .. } => send_tool_update(
            connection,
            session_id,
            call_id,
            protocol::ToolCallUpdateFields::new().status(protocol::ToolCallStatus::InProgress),
            revision,
        ),
        AppEvent::ToolOutput {
            call_id, content, ..
        } => send_tool_update(
            connection,
            session_id,
            call_id,
            protocol::ToolCallUpdateFields::new().content(vec![
                protocol::ContentBlock::Text(protocol::TextContent::new(content)).into(),
            ]),
            revision,
        ),
        AppEvent::ToolCompleted { call_id, success } => send_tool_update(
            connection,
            session_id,
            call_id,
            protocol::ToolCallUpdateFields::new().status(if success {
                protocol::ToolCallStatus::Completed
            } else {
                protocol::ToolCallStatus::Failed
            }),
            revision,
        ),
        AppEvent::DiffAvailable {
            call_id,
            diff,
            truncated,
            files,
        } => {
            let locations = files
                .iter()
                .map(|file| protocol::ToolCallLocation::new(file.path.clone()))
                .collect::<Vec<_>>();
            let content = if files.is_empty() {
                vec![
                    protocol::ContentBlock::Text(protocol::TextContent::new(format!(
                        "DIFF\n{diff}{}",
                        if truncated {
                            "\n… diff truncated"
                        } else {
                            ""
                        }
                    )))
                    .into(),
                ]
            } else {
                files
                    .into_iter()
                    .map(|file| {
                        protocol::Diff::new(file.path, file.new_text)
                            .old_text(file.old_text)
                            .into()
                    })
                    .collect()
            };
            send_tool_update(
                connection,
                session_id,
                call_id,
                protocol::ToolCallUpdateFields::new()
                    .content(content)
                    .locations(locations),
                revision,
            )
        }
        AppEvent::PlanProposed {
            plan_id,
            revision: plan_revision,
            markdown,
        } => send_session_update(
            connection,
            session_id,
            protocol::SessionUpdate::Plan(protocol::Plan::new(vec![protocol::PlanEntry::new(
                format!("{plan_id} r{plan_revision}\n{markdown}"),
                protocol::PlanEntryPriority::High,
                protocol::PlanEntryStatus::Pending,
            )])),
            revision,
            None,
            None,
        ),
        AppEvent::PlanReviewed {
            plan_id,
            revision: plan_revision,
            decision,
        } => send_session_update(
            connection,
            session_id,
            protocol::SessionUpdate::Plan(protocol::Plan::new(vec![protocol::PlanEntry::new(
                format!("{plan_id} r{plan_revision}: {decision}"),
                protocol::PlanEntryPriority::High,
                if decision == "approved" || decision == "abandoned" {
                    protocol::PlanEntryStatus::Completed
                } else {
                    protocol::PlanEntryStatus::InProgress
                },
            )])),
            revision,
            None,
            None,
        ),
        AppEvent::WarningRaised { message } | AppEvent::ErrorRaised { message, .. } => {
            let message_id = format!(
                "notice:{}:{}",
                revision.map_or(0, |value| value.revision),
                correlation_id.as_deref().unwrap_or("unknown")
            );
            let chunk = protocol::ContentChunk::new(protocol::ContentBlock::Text(
                protocol::TextContent::new(message),
            ))
            .message_id(message_id.as_str());
            send_session_update(
                connection,
                session_id,
                protocol::SessionUpdate::AgentMessageChunk(chunk),
                revision,
                None,
                None,
            )
        }
        _ => Ok(()),
    }
}

fn send_tool_update(
    connection: &ConnectionTo<Client>,
    session_id: &protocol::SessionId,
    call_id: String,
    fields: protocol::ToolCallUpdateFields,
    revision: Option<&crate::session::ThreadRevision>,
) -> agent_client_protocol::Result<()> {
    send_session_update(
        connection,
        session_id,
        protocol::SessionUpdate::ToolCallUpdate(protocol::ToolCallUpdate::new(call_id, fields)),
        revision,
        None,
        None,
    )
}

fn send_text(
    connection: &ConnectionTo<Client>,
    session_id: &protocol::SessionId,
    text: String,
) -> agent_client_protocol::Result<()> {
    connection.send_notification(protocol::SessionNotification::new(
        session_id.clone(),
        protocol::SessionUpdate::AgentMessageChunk(protocol::ContentChunk::new(
            protocol::ContentBlock::Text(protocol::TextContent::new(text)),
        )),
    ))
}

fn agent_error(error: crate::AxiomError) -> agent_client_protocol::Error {
    agent_client_protocol::util::internal_error(error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_blocks_are_joined_without_accepting_empty_prompts() {
        let blocks = vec![
            protocol::ContentBlock::Text(protocol::TextContent::new("one")),
            protocol::ContentBlock::Text(protocol::TextContent::new("two")),
        ];
        assert_eq!(prompt_text(&blocks).expect("text"), "one\ntwo");
        assert!(
            prompt_text(&[protocol::ContentBlock::Text(protocol::TextContent::new(
                "  "
            ))])
            .is_err()
        );
        assert_eq!(
            prompt_text(&[protocol::ContentBlock::ResourceLink(
                protocol::ResourceLink::new("source", "file:///tmp/source.rs")
            )])
            .expect("resource link"),
            "[Referenced resource: source](file:///tmp/source.rs)"
        );
    }

    #[test]
    fn every_builtin_tool_has_an_intentional_acp_kind() {
        for name in [
            "list_files",
            "glob_files",
            "inspect_metadata",
            "select_context",
            "inspect_git",
            "view_session_diff",
            "search_text",
            "read_file",
            "apply_patch",
            "replace_text",
            "run_command",
            "run_shell",
            "start_background",
            "background_list",
            "background_status",
            "background_wait",
            "web_search",
            "fetch_url",
            "stop_background",
            "plan_create",
            "plan_revise",
            "plan_propose",
            "ask_user_questions",
            "update_progress",
        ] {
            assert_ne!(tool_kind(name), protocol::ToolKind::Other, "{name}");
        }
    }

    #[test]
    fn verified_security_cannot_be_serialized_without_evidence() {
        let result = strict_extension_security_verification(SecurityVerification {
            status: crate::app::SecurityStatus::Verified,
            evidence: None,
        });
        assert!(result.is_err());
    }

    #[test]
    fn verified_security_rejects_incomplete_lease_evidence_even_for_fixtures() {
        let evidence = axiom_secure_client::SecurityEvidence {
            state: axiom_secure_client::SecurityState::Verified,
            provider_id: "fixture".into(),
            model_id: "fixture-model".into(),
            attestation_protocol: "fixture-attestation-v1".into(),
            e2ee_protocol: "fixture-e2ee-v1".into(),
            e2ee_encryption_version: 1,
            trust_policy_version: "fixture-policy".into(),
            verified_at_unix_seconds: 1,
            attestation_generation: None,
            hard_expires_at_unix_seconds: None,
            model_key_fingerprint: "00".repeat(32),
            tls_spki_fingerprint: None,
            checks: Vec::new(),
            provider_claims: Vec::new(),
            workload_manifest: None,
            workload_identity_allowlist_configured: false,
            approved_workload_identity: false,
        };
        assert!(
            extension_security_evidence(crate::app::SecurityStatus::Verified, evidence).is_err()
        );
    }

    #[test]
    fn applying_a_different_model_invalidates_verified_session_security() {
        let mut session = AcpSession {
            internal_id: SessionId::new(),
            cwd: PathBuf::from("/workspace"),
            profile: PermissionProfile::Web,
            model: "near/model-a".into(),
            thinking: crate::app::ThinkingLevel::High,
            models: vec!["near/model-a".into(), "phala/model-b".into()],
            active_turn: None,
            cancellation: None,
            has_prompt: false,
            security: crate::app::SecurityStatus::Verified,
        };

        apply_session_model_settings(
            &mut session,
            "phala/model-b",
            crate::app::ThinkingLevel::Medium,
            &["near/model-a".into(), "phala/model-b".into()],
            true,
        );
        assert_eq!(session.security, crate::app::SecurityStatus::Unverified);

        session.security = crate::app::SecurityStatus::Verified;
        apply_session_model_settings(
            &mut session,
            "phala/model-b",
            crate::app::ThinkingLevel::Medium,
            &["near/model-a".into(), "phala/model-b".into()],
            false,
        );
        assert_eq!(session.security, crate::app::SecurityStatus::Verified);
    }

    #[test]
    fn prompt_client_item_identity_is_bounded_and_nonempty() {
        let mut meta = protocol::Meta::new();
        meta.insert(
            extension::META_KEY.into(),
            serde_json::json!({"clientItemId": ""}),
        );
        assert!(prompt_metadata(Some(&meta)).is_err());
        meta.insert(
            extension::META_KEY.into(),
            serde_json::json!({"clientItemId": "x".repeat(extension::MAX_IDENTIFIER_BYTES + 1)}),
        );
        assert!(prompt_metadata(Some(&meta)).is_err());
    }
}
