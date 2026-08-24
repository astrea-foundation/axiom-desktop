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
    BrowserLoginState, CompactRequest, CompactResponse, CompactionPhase, DeleteConfirmRequest,
    DeleteConfirmResponse, DeletePreviewRequest, DeletePreviewResponse, EventNotification,
    ExtensionCapabilities, ExtensionError, ExtensionErrorCode, FeatureVersions,
    GetSessionStateRequest, GetSessionStateResponse, ListModelsRequest, ListModelsResponse,
    ListSessionsRequest, ListSessionsResponse, LogoutRequest, ModelInfo,
    SecurityEvidence as ExtensionSecurityEvidence, SecurityState as ExtensionSecurityState,
    SecurityStatus as ExtensionSecurityStatus, SessionLifecycle as ExtensionSessionLifecycle,
    SessionSettings as ExtensionSessionSettings, SessionSnapshot,
    SessionSummary as ExtensionSessionSummary, SetSessionSettingsRequest,
    SetSessionSettingsResponse, UseApiKeyRequest, VerifySecurityRequest, VerifySecurityResponse,
};
use tokio::sync::{Mutex, mpsc};
use tokio_util::sync::CancellationToken;

use crate::{
    agent::{APP_EVENT_QUEUE_CAPACITY, QuestionHandler, SecurityVerification, TurnRunner},
    app::{
        AppCommand, AppEvent, Origin, PermissionProfile, QuestionRequest, Runtime, SessionId,
        TurnId,
    },
    auth::{AuthManager, ValidationStatus},
    config::Config,
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

fn thinking_config_option(current: crate::app::ThinkingLevel) -> protocol::SessionConfigOption {
    protocol::SessionConfigOption::select(
        THINKING_CONFIG_ID,
        "Thinking",
        current.to_string(),
        THINKING_LEVELS
            .iter()
            .map(|level| {
                protocol::SessionConfigSelectOption::new(level.to_string(), level.to_string())
            })
            .collect::<Vec<_>>(),
    )
    .description("Reasoning effort used for subsequent AxiomCLI requests")
}

fn permission_modes(profile: PermissionProfile) -> protocol::SessionModeState {
    protocol::SessionModeState::new(
        profile.to_string(),
        PermissionProfile::ALL
            .into_iter()
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
) -> Vec<protocol::SessionConfigOption> {
    vec![
        model_config_option(model, models),
        thinking_config_option(thinking),
    ]
}

fn tool_kind(name: &str) -> protocol::ToolKind {
    match name {
        "read_file" | "read_many" | "list_files" | "file_metadata" | "inspect_git" => {
            protocol::ToolKind::Read
        }
        "apply_patch" | "replace_text" | "plan_create" | "plan_revise" => protocol::ToolKind::Edit,
        "search_files" | "web_search" | "fetch_url" => protocol::ToolKind::Search,
        "run_command" | "run_shell" | "start_background" | "wait_background" => {
            protocol::ToolKind::Execute
        }
        "progress_update" | "plan_propose" => protocol::ToolKind::Think,
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
            session_config_options(model, models, thinking),
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
) -> ExtensionSecurityEvidence {
    ExtensionSecurityEvidence {
        status: extension_security_status(status),
        provider_id: evidence.provider_id,
        model_id: evidence.model_id,
        attestation_protocol: evidence.attestation_protocol,
        e2ee_protocol: evidence.e2ee_protocol,
        e2ee_encryption_version: evidence.e2ee_encryption_version,
        trust_policy_version: evidence.trust_policy_version,
        verified_at_unix_seconds: evidence.verified_at_unix_seconds,
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
    }
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

fn extension_settings(session: &AcpSession) -> ExtensionSessionSettings {
    ExtensionSessionSettings {
        model: session.model.clone(),
        thinking_level: session.thinking.to_string(),
        permission_profile: session.profile.to_string(),
    }
}

fn extension_snapshot(
    state: &ExtensionState,
    external_id: &protocol::SessionId,
    session: &AcpSession,
    title: Option<String>,
) -> SessionSnapshot {
    SessionSnapshot {
        runtime_instance_id: state.runtime_instance_id.clone(),
        sequence: state.sequence.load(Ordering::Acquire),
        session_id: external_id.0.to_string(),
        title,
        cwd: session.cwd.display().to_string(),
        lifecycle: if session.active_turn.is_some() {
            ExtensionSessionLifecycle::Running
        } else {
            ExtensionSessionLifecycle::Ready
        },
        settings: extension_settings(session),
        security: extension_security_status(session.security),
        active_turn_id: session.active_turn.as_ref().map(ToString::to_string),
        has_prompt: session.has_prompt,
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

fn extension_summary(
    store: &SessionStore,
    summary: crate::session::SessionSummary,
    default_model: &str,
    default_thinking: crate::app::ThinkingLevel,
) -> crate::Result<ExtensionSessionSummary> {
    let id = SessionId::from_str(&summary.id)
        .map_err(|_| crate::AxiomError::Config("invalid stored session ID".into()))?;
    let events = store.load_recovering(&id)?.events;
    let last_outcome = events
        .iter()
        .rev()
        .find_map(|envelope| match &envelope.event {
            AppEvent::TurnCompleted { .. } => Some("completed".to_owned()),
            AppEvent::TurnCancelled { .. } => Some("cancelled".to_owned()),
            AppEvent::ErrorRaised { turn_id, .. } if turn_id.is_some() => Some("failed".to_owned()),
            _ => None,
        });
    Ok(ExtensionSessionSummary {
        session_id: summary.id,
        title: summary.title,
        cwd: summary.cwd.display().to_string(),
        updated_at: summary.updated_at,
        settings: ExtensionSessionSettings {
            model: model_for_resume(&events, default_model),
            thinking_level: thinking_for_resume(&events, default_thinking).to_string(),
            permission_profile: permission_for_resume(
                &events,
                PermissionProfile::from_str(&summary.profile).unwrap_or(PermissionProfile::Confirm),
            )
            .to_string(),
        },
        archived: summary.archived,
        history_truncated: summary.history_truncated,
        last_outcome,
    })
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

fn security_for_resume(events: &[crate::app::EventEnvelope]) -> crate::app::SecurityStatus {
    events
        .iter()
        .rev()
        .find_map(|envelope| match &envelope.event {
            AppEvent::SecurityStatusChanged { state } => Some(*state),
            _ => None,
        })
        .unwrap_or(crate::app::SecurityStatus::Unverified)
}

fn extension_activity(event: &AppEvent) -> Option<ActivityEvent> {
    match event {
        AppEvent::SecurityStatusChanged { state } => Some(ActivityEvent::SecurityChanged {
            status: extension_security_status(*state),
        }),
        AppEvent::ProgressUpdated {
            message,
            completed,
            total,
        } => Some(ActivityEvent::Progress {
            message: message.clone(),
            completed: *completed,
            total: *total,
        }),
        AppEvent::TaskListUpdated { items } => Some(ActivityEvent::TaskList {
            items: items
                .iter()
                .map(|item| extension::TaskItem {
                    id: item.id.clone(),
                    title: item.title.clone(),
                    status: format!("{:?}", item.status).to_ascii_lowercase(),
                })
                .collect(),
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
        AppEvent::WarningRaised { message } => Some(ActivityEvent::Warning {
            message: message.clone(),
        }),
        AppEvent::ErrorRaised { message, .. } => Some(ActivityEvent::Error {
            message: message.clone(),
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

type Sessions = Arc<Mutex<HashMap<protocol::SessionId, AcpSession>>>;

struct PendingDelete {
    session_ids: Vec<SessionId>,
    expires_at_unix_seconds: u64,
}

#[derive(Clone, Copy)]
enum ExtensionFeature {
    SessionCatalog = 1 << 0,
    Settings = 1 << 1,
    Account = 1 << 2,
    SecurityEvidence = 1 << 3,
    Compaction = 1 << 4,
    Activity = 1 << 5,
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
        if features.session_catalog >= 1 {
            mask |= ExtensionFeature::SessionCatalog as u64;
        }
        if features.settings >= 1 {
            mask |= ExtensionFeature::Settings as u64;
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
) -> agent_client_protocol::Result<()> {
    let sessions: Sessions = Arc::new(Mutex::new(HashMap::new()));
    let extension_state = Arc::new(ExtensionState::new());
    let runtime = Runtime::new(512);
    let default_model = config.model.clone();
    let default_thinking = crate::app::ThinkingLevel::Medium;
    let extension_default_model = default_model.clone();
    let extension_default_thinking = default_thinking;
    let list_default_model = extension_default_model.clone();
    let preview_default_model = extension_default_model;

    let new_sessions = sessions.clone();
    let new_runtime = runtime.clone();
    let new_runner = runner.clone();
    let new_config = config;
    let prompt_sessions = sessions.clone();
    let prompt_runtime = runtime.clone();
    let cancel_sessions = sessions.clone();
    let new_store = store.clone();
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
    let mode_sessions = sessions.clone();
    let mode_runtime = runtime.clone();
    let mode_store = prompt_store.clone();
    let supports_load = load_store.is_some();
    let supports_elicitation = Arc::new(AtomicBool::new(false));
    let initialize_elicitation = supports_elicitation.clone();
    let initialize_extension = extension_state.clone();
    let list_extension = extension_state.clone();
    let list_extension_store = extension_store.clone();
    let state_extension = extension_state.clone();
    let state_extension_store = extension_store.clone();
    let state_extension_sessions = sessions.clone();
    let preview_extension = extension_state.clone();
    let preview_extension_store = extension_store.clone();
    let preview_extension_sessions = sessions.clone();
    let delete_extension = extension_state.clone();
    let delete_extension_store = extension_store.clone();
    let delete_extension_sessions = sessions.clone();
    let models_extension = extension_state.clone();
    let models_extension_runner = runner.clone();
    let settings_extension = extension_state.clone();
    let settings_extension_sessions = sessions.clone();
    let settings_extension_runtime = runtime.clone();
    let settings_extension_runner = runner.clone();
    let settings_extension_store = extension_store.clone();
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
    let compact_extension_store = extension_store;

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
                let profile = new_config
                    .for_workspace(&cwd)
                    .map_err(agent_error)?
                    .permission_profile;
                let internal_id = SessionId::new();
                let session_model = new_store
                    .as_ref()
                    .map(SessionStore::last_used_model)
                    .transpose()
                    .map_err(agent_error)?
                    .flatten()
                    .unwrap_or_else(|| new_default_model.clone());
                let session_thinking = new_store
                    .as_ref()
                    .map(SessionStore::last_used_thinking)
                    .transpose()
                    .map_err(agent_error)?
                    .flatten()
                    .unwrap_or(new_default_thinking);
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
                if session_model != new_default_model {
                    new_runner
                        .set_model(&internal_id, session_model.clone())
                        .await
                        .map_err(agent_error)?;
                }
                if session_thinking != new_default_thinking {
                    new_runner
                        .set_thinking_level(&internal_id, session_thinking)
                        .await
                        .map_err(agent_error)?;
                }
                let selected_model = new_runtime
                    .dispatch(AppCommand::ChangeModel {
                        session_id: internal_id.clone(),
                        model: session_model.clone(),
                    })
                    .await
                    .map_err(agent_error)?;
                if let Some(store) = &new_store {
                    store.append_all(&selected_model).map_err(agent_error)?;
                }
                let selected_thinking = new_runtime
                    .dispatch(AppCommand::ChangeThinkingLevel {
                        session_id: internal_id.clone(),
                        level: session_thinking,
                    })
                    .await
                    .map_err(agent_error)?;
                if let Some(store) = &new_store {
                    store
                        .append_all(&selected_thinking)
                        .map_err(agent_error)?;
                }
                let external_id = protocol::SessionId::new(internal_id.to_string());
                new_sessions.lock().await.insert(
                    external_id.clone(),
                    AcpSession {
                        internal_id,
                        cwd,
                        profile,
                        model: session_model.clone(),
                        thinking: session_thinking,
                        models: Vec::new(),
                        active_turn: None,
                        cancellation: None,
                        has_prompt: false,
                        security: crate::app::SecurityStatus::Unverified,
                    },
                );
                responder.respond(
                    protocol::NewSessionResponse::new(external_id.clone())
                        .modes(permission_modes(profile))
                        .config_options(session_config_options(
                            &session_model,
                            &[],
                            session_thinking,
                        )),
                )?;
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
                let loaded = store.load_recovering(&internal_id).map_err(agent_error)?;
                if let Some(warning) = loaded.warnings.first() {
                    return responder.respond_with_internal_error(warning.clone());
                }
                let restored = load_runtime
                    .restore(&loaded.events)
                    .await
                    .map_err(agent_error)?;
                load_runner
                    .restore_session(&internal_id, &loaded.events)
                    .await
                    .map_err(agent_error)?;
                for envelope in &loaded.events {
                    send_event(
                        &connection,
                        &external_id,
                        envelope.event.clone(),
                        Some(&saved_cwd),
                        Some(&load_extension),
                        Some(envelope.correlation_id.to_string()),
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
                let restored_profile = permission_for_resume(&loaded.events, restored.profile);
                let restored_model = model_for_resume(&loaded.events, &load_default_model);
                let restored_thinking =
                    thinking_for_resume(&loaded.events, load_default_thinking);
                load_sessions.lock().await.insert(
                    external_id.clone(),
                    AcpSession {
                        internal_id,
                        cwd: saved_cwd,
                        profile: restored_profile,
                        model: restored_model.clone(),
                        thinking: restored_thinking,
                        models: Vec::new(),
                        active_turn: None,
                        cancellation: None,
                        has_prompt: loaded.events.iter().any(|envelope| {
                            matches!(envelope.event, AppEvent::PromptAccepted { .. })
                        }),
                        security: security_for_resume(&loaded.events),
                    },
                );
                responder.respond(
                    protocol::LoadSessionResponse::new()
                        .modes(permission_modes(restored_profile))
                        .config_options(session_config_options(
                            &restored_model,
                            &[],
                            restored_thinking,
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
                            let settings = {
                                let sessions = prompt_sessions.lock().await;
                                sessions.get(&session_id).map(|session| {
                                    (
                                        extension_settings(session),
                                        session.model.clone(),
                                        session.models.clone(),
                                    )
                                })
                            };
                            if let Some((settings, model, models)) = settings {
                                send_config_update(
                                    &connection,
                                    &session_id,
                                    &model,
                                    &models,
                                    level,
                                )?;
                                send_extension_activity(
                                    &connection,
                                    &prompt_extension,
                                    Some(&session_id),
                                    ActivityEvent::SettingsChanged { settings },
                                )?;
                            }
                            send_text(
                                &connection,
                                &session_id,
                                format!("Thinking set to {level}."),
                            )?;
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
                            if let Some(session) =
                                prompt_sessions.lock().await.get_mut(&session_id)
                            {
                                session.security = verification.status;
                            }
                            send_extension_activity(
                                &connection,
                                &prompt_extension,
                                Some(&session_id),
                                ActivityEvent::SecurityChanged {
                                    status: extension_security_status(verification.status),
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
                                            if let Some(store) = &task_store {
                                                store.append(&envelope).map_err(agent_error)?;
                                            }
                                            send_event(
                                                &task_connection,
                                                &session_id,
                                                event,
                                                None,
                                                Some(&task_extension),
                                                Some(envelope.correlation_id.to_string()),
                                            )?;
                                        }
                                    }
                                };
                                while let Ok(event) = events_rx.try_recv() {
                                    let envelope = task_runtime
                                        .emit(internal_id.clone(), event.clone())
                                        .await
                                        .map_err(agent_error)?;
                                    if let Some(store) = &task_store {
                                        store.append(&envelope).map_err(agent_error)?;
                                    }
                                    send_event(
                                        &task_connection,
                                        &session_id,
                                        event,
                                        None,
                                        Some(&task_extension),
                                        Some(envelope.correlation_id.to_string()),
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
                if let Some(store) = &prompt_store {
                    store.append_all(&submitted).map_err(agent_error)?;
                    store
                        .set_last_used_model(&session_model)
                        .map_err(agent_error)?;
                    store
                        .set_last_used_thinking(session_thinking)
                        .map_err(agent_error)?;
                }

                if should_generate_title
                    && let Some(title_store) = prompt_store.clone()
                {
                    let title_runner = runner.clone();
                    let title_session = internal_id.clone();
                    let title_prompt = prompt.clone();
                    let title_connection = connection.clone();
                    let title_external_id = session_id.clone();
                    let title_extension = prompt_extension.clone();
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
                                    if let Err(error) = send_extension_activity(
                                        &title_connection,
                                        &title_extension,
                                        Some(&title_external_id),
                                        ActivityEvent::TitleChanged { title },
                                    ) {
                                        tracing::warn!(
                                            session_id = %title_session,
                                            %error,
                                            "could not publish generated Axiom session title"
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
                                if let Some(store) = &task_store {
                                    store.append(&envelope).map_err(agent_error)?;
                                }
                                send_event(
                                    &task_connection,
                                    &session_id,
                                    event,
                                    Some(&display_cwd),
                                    Some(&task_extension),
                                    Some(envelope.correlation_id.to_string()),
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
                        if let Some(store) = &task_store {
                            store.append(&envelope).map_err(agent_error)?;
                        }
                        send_event(
                            &task_connection,
                            &session_id,
                            event,
                            Some(&display_cwd),
                            Some(&task_extension),
                            Some(envelope.correlation_id.to_string()),
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
            async move |request: ListSessionsRequest,
                        responder: Responder<ListSessionsResponse>,
                        _connection| {
                if !extension_enabled(&list_extension, ExtensionFeature::SessionCatalog) {
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
                let query = request
                    .query
                    .as_deref()
                    .map(str::trim)
                    .filter(|query| !query.is_empty())
                    .map(str::to_ascii_lowercase);
                let limit = usize::try_from(request.limit.unwrap_or(200).clamp(1, 500))
                    .unwrap_or(200);
                let offset = match request.cursor.as_deref() {
                    None => 0,
                    Some(cursor) => match cursor.parse::<usize>() {
                        Ok(offset) => offset,
                        Err(_) => {
                            return responder
                                .respond_with_internal_error("session cursor is invalid");
                        }
                    },
                };
                let mut sessions = Vec::new();
                let mut matched_count = 0_usize;
                for summary in store.list(request.include_archived).map_err(agent_error)? {
                    if let Some(query) = &query {
                        let query_matches = summary
                            .title
                            .as_deref()
                            .is_some_and(|title| title.to_ascii_lowercase().contains(query))
                            || summary.cwd.display().to_string().to_ascii_lowercase().contains(query);
                        if !query_matches {
                            continue;
                        }
                    }
                    if matched_count < offset {
                        matched_count += 1;
                        continue;
                    }
                    matched_count += 1;
                    sessions.push(
                        extension_summary(
                            store,
                            summary,
                            &list_default_model,
                            extension_default_thinking,
                        )
                        .map_err(agent_error)?,
                    );
                    if sessions.len() > limit {
                        break;
                    }
                }
                let next_cursor = if sessions.len() > limit {
                    sessions.pop();
                    Some(offset.saturating_add(limit).to_string())
                } else {
                    None
                };
                responder.respond(ListSessionsResponse {
                    sessions,
                    next_cursor,
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: GetSessionStateRequest,
                        responder: Responder<GetSessionStateResponse>,
                        _connection| {
                if !extension_enabled(&state_extension, ExtensionFeature::SessionCatalog) {
                    return responder.respond_with_error(extension_not_negotiated());
                }
                let external_id = protocol::SessionId::new(request.session_id);
                let session = {
                    let sessions = state_extension_sessions.lock().await;
                    let Some(session) = sessions.get(&external_id) else {
                        return responder.respond_with_internal_error(
                            "session must be loaded before requesting live state",
                        );
                    };
                    session.clone()
                };
                let title = state_extension_store
                    .as_ref()
                    .and_then(|store| store.summary(&session.internal_id).ok())
                    .and_then(|summary| summary.title);
                responder.respond(GetSessionStateResponse {
                    snapshot: extension_snapshot(
                        &state_extension,
                        &external_id,
                        &session,
                        title,
                    ),
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: DeletePreviewRequest,
                        responder: Responder<DeletePreviewResponse>,
                        _connection| {
                if !extension_enabled(&preview_extension, ExtensionFeature::SessionCatalog) {
                    return responder.respond_with_error(extension_not_negotiated());
                }
                let Some(store) = &preview_extension_store else {
                    return responder.respond_with_internal_error(
                        "durable session storage is unavailable",
                    );
                };
                let ids = validate_session_ids(&request.session_ids).map_err(agent_error)?;
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
                    summaries.push(
                        extension_summary(
                            store,
                            store.summary(id).map_err(agent_error)?,
                            &preview_default_model,
                            extension_default_thinking,
                        )
                        .map_err(agent_error)?,
                    );
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
                    sessions: summaries,
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: DeleteConfirmRequest,
                        responder: Responder<DeleteConfirmResponse>,
                        _connection| {
                if !extension_enabled(&delete_extension, ExtensionFeature::SessionCatalog) {
                    return responder.respond_with_error(extension_not_negotiated());
                }
                let Some(store) = &delete_extension_store else {
                    return responder.respond_with_internal_error(
                        "durable session storage is unavailable",
                    );
                };
                let requested = match validate_session_ids(&request.session_ids) {
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
                if !extension_enabled(&models_extension, ExtensionFeature::Settings) {
                    return responder.respond_with_error(extension_not_negotiated());
                }
                let models = models_extension_runner
                    .available_models(CancellationToken::new())
                    .await
                    .map_err(agent_error)?
                    .into_iter()
                    .map(|id| ModelInfo {
                        label: id.clone(),
                        id,
                        thinking_levels: THINKING_LEVELS
                            .iter()
                            .map(ToString::to_string)
                            .collect(),
                    })
                    .collect();
                responder.respond(ListModelsResponse { models })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: SetSessionSettingsRequest,
                        responder: Responder<SetSessionSettingsResponse>,
                        connection: ConnectionTo<Client>| {
                if !extension_enabled(&settings_extension, ExtensionFeature::Settings) {
                    return responder.respond_with_error(extension_not_negotiated());
                }
                if request
                    .model
                    .as_ref()
                    .is_some_and(|value| value.len() > extension::MAX_IDENTIFIER_BYTES)
                    || request
                        .thinking_level
                        .as_ref()
                        .is_some_and(|value| value.len() > extension::MAX_IDENTIFIER_BYTES)
                    || request
                        .permission_profile
                        .as_ref()
                        .is_some_and(|value| value.len() > extension::MAX_IDENTIFIER_BYTES)
                {
                    return responder.respond_with_internal_error("session setting is too long");
                }
                let external_id = protocol::SessionId::new(request.session_id);
                let (internal_id, current_model, current_thinking, current_profile) = {
                    let sessions = settings_extension_sessions.lock().await;
                    let Some(session) = sessions.get(&external_id) else {
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
                        session.profile,
                    )
                };

                let next_model = request.model.unwrap_or_else(|| current_model.clone());
                let next_thinking = request
                    .thinking_level
                    .as_deref()
                    .map(crate::app::ThinkingLevel::from_str)
                    .transpose()
                    .map_err(agent_error)?
                    .unwrap_or(current_thinking);
                let next_profile = request
                    .permission_profile
                    .as_deref()
                    .map(PermissionProfile::from_str)
                    .transpose()
                    .map_err(agent_error)?
                    .unwrap_or(current_profile);

                let models = settings_extension_runner
                    .available_models(CancellationToken::new())
                    .await
                    .map_err(agent_error)?;
                if !models.iter().any(|model| model == &next_model) {
                    return responder
                        .respond_with_internal_error("model is not in the provider catalog");
                }
                if next_model != current_model {
                    settings_extension_runner
                        .set_model(&internal_id, next_model.clone())
                        .await
                        .map_err(agent_error)?;
                    let events = settings_extension_runtime
                        .dispatch(AppCommand::ChangeModel {
                            session_id: internal_id.clone(),
                            model: next_model.clone(),
                        })
                        .await
                        .map_err(agent_error)?;
                    if let Some(store) = &settings_extension_store {
                        store.append_all(&events).map_err(agent_error)?;
                    }
                }
                if next_thinking != current_thinking {
                    settings_extension_runner
                        .set_thinking_level(&internal_id, next_thinking)
                        .await
                        .map_err(agent_error)?;
                    let events = settings_extension_runtime
                        .dispatch(AppCommand::ChangeThinkingLevel {
                            session_id: internal_id.clone(),
                            level: next_thinking,
                        })
                        .await
                        .map_err(agent_error)?;
                    if let Some(store) = &settings_extension_store {
                        store.append_all(&events).map_err(agent_error)?;
                    }
                }
                if next_profile != current_profile {
                    let events = settings_extension_runtime
                        .dispatch(AppCommand::ChangePermissionProfile {
                            session_id: internal_id,
                            profile: next_profile,
                        })
                        .await
                        .map_err(agent_error)?;
                    if let Some(store) = &settings_extension_store {
                        store.append_all(&events).map_err(agent_error)?;
                    }
                }

                let settings = ExtensionSessionSettings {
                    model: next_model.clone(),
                    thinking_level: next_thinking.to_string(),
                    permission_profile: next_profile.to_string(),
                };
                if let Some(session) = settings_extension_sessions.lock().await.get_mut(&external_id)
                {
                    session.model.clone_from(&next_model);
                    session.thinking = next_thinking;
                    session.profile = next_profile;
                    session.models.clone_from(&models);
                }
                if let Some(store) = &settings_extension_store {
                    store
                        .set_last_used_model(&next_model)
                        .map_err(agent_error)?;
                    store
                        .set_last_used_thinking(next_thinking)
                        .map_err(agent_error)?;
                }
                connection.send_notification(protocol::SessionNotification::new(
                    external_id.clone(),
                    protocol::SessionUpdate::CurrentModeUpdate(protocol::CurrentModeUpdate::new(
                        next_profile.to_string(),
                    )),
                ))?;
                send_config_update(
                    &connection,
                    &external_id,
                    &next_model,
                    &models,
                    next_thinking,
                )?;
                send_extension_activity(
                    &connection,
                    &settings_extension,
                    Some(&external_id),
                    ActivityEvent::SettingsChanged {
                        settings: settings.clone(),
                    },
                )?;
                responder.respond(SetSessionSettingsResponse { settings })
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
                let external_id = protocol::SessionId::new(request.session_id);
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
                if let Some(session) = security_extension_sessions.lock().await.get_mut(&external_id)
                {
                    session.security = verification.status;
                }
                let status = extension_security_status(verification.status);
                send_extension_activity(
                    &connection,
                    &security_extension,
                    Some(&external_id),
                    ActivityEvent::SecurityChanged {
                        status: status.clone(),
                    },
                )?;
                let evidence = verification
                    .evidence
                    .map(|evidence| extension_security_evidence(verification.status, evidence));
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
                let external_id = protocol::SessionId::new(request.session_id);
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
                            if let Some(store) = &compact_extension_store {
                                store.append(&envelope).map_err(agent_error)?;
                            }
                            send_event(
                                &connection,
                                &external_id,
                                event,
                                None,
                                Some(&compact_extension),
                                Some(envelope.correlation_id.to_string()),
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
                        if let Some(store) = &compact_extension_store {
                            store.append_all(&events).map_err(agent_error)?;
                        }
                        for event in events {
                            send_event(
                                &connection,
                                &external_id,
                                event.event,
                                None,
                                Some(&compact_extension),
                                Some(event.correlation_id.to_string()),
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
                        _connection| {
                let Some(value) = request.value.as_value_id() else {
                    return responder.respond_with_internal_error(
                        "session config requires a selected value ID",
                    );
                };
                let selected = value.0.to_string();
                let (internal_id, model, thinking, models) = {
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
                        session.models.clone(),
                    )
                };
                let (next_model, next_thinking, events) = match request.config_id.0.as_ref() {
                    MODEL_CONFIG_ID => {
                        if !models.iter().any(|candidate| candidate == &selected) {
                            return responder.respond_with_internal_error(
                                "model is not in the provider catalog",
                            );
                        }
                        config_runner
                            .set_model(&internal_id, selected.clone())
                            .await
                            .map_err(agent_error)?;
                        let events = config_runtime
                            .dispatch(AppCommand::ChangeModel {
                                session_id: internal_id,
                                model: selected.clone(),
                            })
                            .await
                            .map_err(agent_error)?;
                        (selected, thinking, events)
                    }
                    THINKING_CONFIG_ID => {
                        let level = crate::app::ThinkingLevel::from_str(&selected)
                            .map_err(agent_error)?;
                        config_runner
                            .set_thinking_level(&internal_id, level)
                            .await
                            .map_err(agent_error)?;
                        let events = config_runtime
                            .dispatch(AppCommand::ChangeThinkingLevel {
                                session_id: internal_id,
                                level,
                            })
                            .await
                            .map_err(agent_error)?;
                        (model, level, events)
                    }
                    _ => {
                        return responder
                            .respond_with_internal_error("unknown session config option");
                    }
                };
                if let Some(store) = &config_store {
                    store.append_all(&events).map_err(agent_error)?;
                }
                if let Some(session) = config_sessions.lock().await.get_mut(&request.session_id) {
                    session.model.clone_from(&next_model);
                    session.thinking = next_thinking;
                }
                responder.respond(protocol::SetSessionConfigOptionResponse::new(
                    session_config_options(&next_model, &models, next_thinking),
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

fn send_event(
    connection: &ConnectionTo<Client>,
    session_id: &protocol::SessionId,
    event: AppEvent,
    cwd: Option<&Path>,
    extension_state: Option<&ExtensionState>,
    correlation_id: Option<String>,
) -> agent_client_protocol::Result<()> {
    if let Some(extension_state) = extension_state
        && let Some(activity) = extension_activity(&event)
    {
        send_extension_activity_correlated(
            connection,
            extension_state,
            Some(session_id),
            correlation_id,
            activity,
        )?;
    }
    match event {
        AppEvent::TextDelta { text, .. } => send_text(connection, session_id, text),
        AppEvent::ReasoningDelta { text, .. } => {
            connection.send_notification(protocol::SessionNotification::new(
                session_id.clone(),
                protocol::SessionUpdate::AgentThoughtChunk(protocol::ContentChunk::new(
                    protocol::ContentBlock::Text(protocol::TextContent::new(text)),
                )),
            ))
        }
        AppEvent::PermissionProfileChanged { profile } => {
            connection.send_notification(protocol::SessionNotification::new(
                session_id.clone(),
                protocol::SessionUpdate::CurrentModeUpdate(protocol::CurrentModeUpdate::new(
                    profile.to_string(),
                )),
            ))?;
            send_text(
                connection,
                session_id,
                format!("[settings] permission profile: {profile}"),
            )
        }
        AppEvent::ModelChanged { model } => {
            connection.send_notification(protocol::SessionNotification::new(
                session_id.clone(),
                protocol::SessionUpdate::ConfigOptionUpdate(protocol::ConfigOptionUpdate::new(
                    vec![model_config_option(&model, std::slice::from_ref(&model))],
                )),
            ))?;
            send_text(connection, session_id, format!("[settings] model: {model}"))
        }
        AppEvent::ThinkingLevelChanged { level } => {
            connection.send_notification(protocol::SessionNotification::new(
                session_id.clone(),
                protocol::SessionUpdate::ConfigOptionUpdate(protocol::ConfigOptionUpdate::new(
                    vec![thinking_config_option(level)],
                )),
            ))?;
            send_text(
                connection,
                session_id,
                format!("[settings] thinking: {level}"),
            )
        }
        AppEvent::ContextCompacted {
            messages_before, ..
        } => send_text(
            connection,
            session_id,
            format!("[context] compacted {messages_before} conversation message(s)"),
        ),
        AppEvent::ProviderStatusChanged { connected, detail } => {
            connection.send_notification(protocol::SessionNotification::new(
                session_id.clone(),
                protocol::SessionUpdate::AgentThoughtChunk(protocol::ContentChunk::new(
                    protocol::ContentBlock::Text(protocol::TextContent::new(format!(
                        "[{}] {detail}",
                        if connected {
                            "provider"
                        } else {
                            "provider status"
                        }
                    ))),
                )),
            ))
        }
        AppEvent::ProgressUpdated {
            message,
            completed,
            total,
        } => connection.send_notification(protocol::SessionNotification::new(
            session_id.clone(),
            protocol::SessionUpdate::AgentThoughtChunk(protocol::ContentChunk::new(
                protocol::ContentBlock::Text(protocol::TextContent::new(
                    match (completed, total) {
                        (Some(completed), Some(total)) => {
                            format!("[progress {completed}/{total}] {message}")
                        }
                        _ => format!("[progress] {message}"),
                    },
                )),
            )),
        )),
        AppEvent::TaskListUpdated { items } => {
            connection.send_notification(protocol::SessionNotification::new(
                session_id.clone(),
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
            ))
        }
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
            connection.send_notification(protocol::SessionNotification::new(
                session_id.clone(),
                protocol::SessionUpdate::ToolCall(
                    protocol::ToolCall::new(call_id, title)
                        .kind(kind)
                        .status(protocol::ToolCallStatus::Pending)
                        .raw_input(arguments),
                ),
            ))
        }
        AppEvent::ToolStarted { call_id, .. } => send_tool_update(
            connection,
            session_id,
            call_id,
            protocol::ToolCallUpdateFields::new().status(protocol::ToolCallStatus::InProgress),
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
            )
        }
        AppEvent::BackgroundTaskChanged { task_id, state } => {
            connection.send_notification(protocol::SessionNotification::new(
                session_id.clone(),
                protocol::SessionUpdate::AgentThoughtChunk(protocol::ContentChunk::new(
                    protocol::ContentBlock::Text(protocol::TextContent::new(format!(
                        "[background {task_id}] {state}"
                    ))),
                )),
            ))
        }
        AppEvent::QuestionsAsked { request } => {
            connection.send_notification(protocol::SessionNotification::new(
                session_id.clone(),
                protocol::SessionUpdate::AgentThoughtChunk(protocol::ContentChunk::new(
                    protocol::ContentBlock::Text(protocol::TextContent::new(format!(
                        "[attention {}] {} structured question(s)",
                        request.request_id,
                        request.questions.len()
                    ))),
                )),
            ))
        }
        AppEvent::QuestionsAnswered { request_id, .. } => {
            connection.send_notification(protocol::SessionNotification::new(
                session_id.clone(),
                protocol::SessionUpdate::AgentThoughtChunk(protocol::ContentChunk::new(
                    protocol::ContentBlock::Text(protocol::TextContent::new(format!(
                        "[attention {request_id}] answered"
                    ))),
                )),
            ))
        }
        AppEvent::QuestionsFailed { request_id, reason } => {
            connection.send_notification(protocol::SessionNotification::new(
                session_id.clone(),
                protocol::SessionUpdate::AgentThoughtChunk(protocol::ContentChunk::new(
                    protocol::ContentBlock::Text(protocol::TextContent::new(format!(
                        "[attention {request_id}] closed without answers: {reason}"
                    ))),
                )),
            ))
        }
        AppEvent::PlanProposed {
            plan_id,
            revision,
            markdown,
        } => connection.send_notification(protocol::SessionNotification::new(
            session_id.clone(),
            protocol::SessionUpdate::Plan(protocol::Plan::new(vec![protocol::PlanEntry::new(
                format!("{plan_id} r{revision}\n{markdown}"),
                protocol::PlanEntryPriority::High,
                protocol::PlanEntryStatus::Pending,
            )])),
        )),
        AppEvent::PlanReviewed {
            plan_id,
            revision,
            decision,
        } => connection.send_notification(protocol::SessionNotification::new(
            session_id.clone(),
            protocol::SessionUpdate::Plan(protocol::Plan::new(vec![protocol::PlanEntry::new(
                format!("{plan_id} r{revision}: {decision}"),
                protocol::PlanEntryPriority::High,
                if decision == "approved" || decision == "abandoned" {
                    protocol::PlanEntryStatus::Completed
                } else {
                    protocol::PlanEntryStatus::InProgress
                },
            )])),
        )),
        _ => Ok(()),
    }
}

fn send_tool_update(
    connection: &ConnectionTo<Client>,
    session_id: &protocol::SessionId,
    call_id: String,
    fields: protocol::ToolCallUpdateFields,
) -> agent_client_protocol::Result<()> {
    connection.send_notification(protocol::SessionNotification::new(
        session_id.clone(),
        protocol::SessionUpdate::ToolCallUpdate(protocol::ToolCallUpdate::new(call_id, fields)),
    ))
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
}
