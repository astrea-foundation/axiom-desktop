use std::{
    collections::{BTreeMap, HashMap, HashSet},
    ops::{Deref, DerefMut},
    path::{Path, PathBuf},
    str::FromStr as _,
    sync::{
        Arc, Mutex as StdMutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use agent_client_protocol::schema::{ProtocolVersion, v1 as protocol};
use agent_client_protocol::{Agent, ByteStreams, Client, ConnectionTo, Responder};
use async_trait::async_trait;
use axiom_acp_extension as extension;
use axiom_acp_extension::{
    AccountState as ExtensionAccountState, AccountStatus as ExtensionAccountStatus, ActivityEvent,
    BillingStatusRequest, BillingStatusResponse, CollectionStateResponse, CompactRequest,
    CompactResponse, CompactionPhase, DeleteConfirmRequest, DeleteConfirmResponse,
    DeletePreviewRequest, DeletePreviewResponse, DesktopBootstrapRequest, DesktopBootstrapResponse,
    EventNotification, ExtensionCapabilities, ExtensionError, ExtensionErrorCode, FeatureVersions,
    GetThreadTimelineRequest, GetThreadTimelineResponse, ListModelsRequest, ListModelsResponse,
    ListThreadsRequest, ListThreadsResponse, LogoutRequest, ModelInfo, NativeLoginCancelRequest,
    NativeLoginCompleteRequest, NativeLoginStartRequest, NativeLoginStartResponse,
    NativeLoginState, ProfilePreferencesResponse, SecurityEvidence as ExtensionSecurityEvidence,
    SecurityState as ExtensionSecurityState, SecurityStatus as ExtensionSecurityStatus,
    VerifySecurityRequest, VerifySecurityResponse,
};
use tokio::sync::{Mutex, Notify, mpsc};
use tokio_util::compat::{TokioAsyncReadCompatExt as _, TokioAsyncWriteCompatExt as _};
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
    auth::{AuthManager, LoginMethod, ValidationStatus, VersionedValidationStatus},
    billing::BillingClient,
    config::Config,
    paths::FrontendKind,
    policy::{ApprovalChoice, ApprovalHandler, ApprovalRequest, ApprovalResponse},
    session::{SessionStore, model_for_resume, permission_for_resume, thinking_for_resume},
    slash::{self, SlashCommand},
    tool_display::{ToolDisplayPhase, describe_tool},
};

/// Complete an expected request failure through its JSON-RPC responder instead
/// of returning it from the handler and terminating the ACP connection task.
/// The only errors allowed to escape a request handler are transport failures
/// produced while sending responses, notifications, or spawned work.
macro_rules! respond_or_return {
    ($responder:ident, $result:expr) => {
        match $result {
            Ok(value) => value,
            Err(error) => return $responder.respond_with_error(error),
        }
    };
}

mod account;
mod catalog;
mod collections;
mod prompts;
mod security;
mod server;
mod sessions;
mod settings;
mod threads;

use server::ServerContext;
pub use server::serve_stdio;

#[derive(Default)]
struct PersistenceFailpoint {
    scope: StdMutex<Option<String>>,
}

impl PersistenceFailpoint {
    fn from_environment() -> Self {
        let enabled = cfg!(debug_assertions)
            && std::env::var("AXIOMCLI_TEST_RUNNER").is_ok()
            && std::env::var("AXIOMCLI_TEST_FAIL_PERSISTENCE")
                .ok()
                .is_some_and(|value| !value.trim().is_empty());
        Self {
            scope: StdMutex::new(enabled.then(|| {
                std::env::var("AXIOMCLI_TEST_FAIL_PERSISTENCE")
                    .expect("checked test persistence failpoint")
            })),
        }
    }

    fn check(&self, scope: &str) -> crate::Result<()> {
        let mut configured = self
            .scope
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if configured.as_deref() == Some(scope) {
            configured.take();
            return Err(crate::AxiomError::Storage(format!(
                "injected {scope} persistence failure"
            )));
        }
        Ok(())
    }
}

fn send_available_commands(
    connection: &ConnectionTo<Client>,
    session_id: &protocol::SessionId,
) -> agent_client_protocol::Result<()> {
    let commands = slash::COMMANDS
        .iter()
        .filter(|spec| !matches!(spec.name, "theme" | "web" | "usage"))
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
                protocol::SessionConfigSelectOption::new(
                    level.to_string(),
                    match level {
                        crate::app::ThinkingLevel::ProviderDefault => "Provider default".to_owned(),
                        crate::app::ThinkingLevel::Enabled => "On".to_owned(),
                        crate::app::ThinkingLevel::Disabled => "Off".to_owned(),
                        _ => level.to_string(),
                    },
                )
            })
            .collect::<Vec<_>>(),
    )
    .description("Reasoning effort used for subsequent AxiomCLI requests")
}

fn thinking_config_option(current: crate::app::ThinkingLevel) -> protocol::SessionConfigOption {
    // This event carries a value, not a new catalog. Do not invent supported
    // alternatives; full config snapshots carry the actual advertised list.
    thinking_config_option_for(current, &[current])
}

fn supported_thinking_levels(model: &axiom_inference::ModelInfo) -> Vec<crate::app::ThinkingLevel> {
    crate::agent::supported_thinking_levels(model)
}

fn permission_modes(
    profile: PermissionProfile,
    frontend: FrontendKind,
) -> protocol::SessionModeState {
    let profiles: &[PermissionProfile] = if frontend == FrontendKind::DesktopChat {
        &[
            PermissionProfile::Web,
            PermissionProfile::Confirm,
            PermissionProfile::FullAccess,
        ]
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
        None if thinking != crate::app::ThinkingLevel::ProviderDefault => {
            options.push(thinking_config_option(thinking));
        }
        None => {}
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
        .filter(|spec| !matches!(spec.name, "theme" | "web" | "usage"))
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

fn prompt_web_enabled(
    frontend: FrontendKind,
    metadata: Option<&extension::PromptMetadata>,
) -> bool {
    frontend != FrontendKind::DesktopChat || metadata.is_some_and(|metadata| metadata.web_enabled)
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
                last_message_at: revision.last_message_at.clone(),
                last_user_message_at: revision.last_user_message_at.clone(),
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
        crate::app::SecurityStatus::Degraded => ExtensionSecurityState::Degraded,
        crate::app::SecurityStatus::Outdated => ExtensionSecurityState::Outdated,
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
    // Tinfoil authenticates its router directly and has no relay lease generation.
    // Keep the generation requirement for protocols whose relay leases use it.
    let direct_tinfoil = evidence.provider_id == "tinfoil"
        && evidence.attestation_protocol == "tinfoil-snp-sigstore-v1"
        && evidence.e2ee_protocol == "tinfoil-ehbp-v1";
    let attestation_generation = evidence.attestation_generation;
    if attestation_generation == Some(0) || (attestation_generation.is_none() && !direct_tinfoil) {
        return Err(crate::AxiomError::Protocol(
            "verified provider evidence is missing its attestation generation".into(),
        ));
    }
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
    if matches!(
        status,
        crate::app::SecurityStatus::Verified | crate::app::SecurityStatus::Degraded
    ) && evidence.is_none()
    {
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
        last_message_at: summary.last_message_at,
        last_user_message_at: summary.last_user_message_at,
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

fn extension_timeline_item(mut item: crate::session::TimelineItem) -> extension::TimelineItem {
    if item.kind == crate::session::TimelineItemKind::ToolCall {
        let name = item
            .metadata
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("tool");
        let arguments = item
            .metadata
            .get("arguments")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let phase = match item.status {
            crate::session::TimelineItemStatus::Completed => ToolDisplayPhase::Completed,
            crate::session::TimelineItemStatus::Failed
            | crate::session::TimelineItemStatus::Cancelled
            | crate::session::TimelineItemStatus::Interrupted => ToolDisplayPhase::Failed,
            crate::session::TimelineItemStatus::Pending => ToolDisplayPhase::Proposed,
            crate::session::TimelineItemStatus::InProgress => ToolDisplayPhase::Running,
        };
        let title = describe_tool(name, &arguments, phase, None);
        let kind = serde_json::to_value(tool_kind(name)).unwrap_or(serde_json::Value::Null);
        item.metadata["title"] = serde_json::Value::String(title);
        item.metadata["kind"] = kind;
    }
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

fn extension_account_status(status: VersionedValidationStatus) -> ExtensionAccountStatus {
    let revision = status.revision;
    match status.status {
        ValidationStatus::Missing => ExtensionAccountStatus {
            revision,
            state: ExtensionAccountState::SignedOut,
            account: None,
            session: None,
            detail: None,
        },
        ValidationStatus::Valid(account) => ExtensionAccountStatus {
            revision,
            state: ExtensionAccountState::Valid,
            account: Some(extension::AccountProfile {
                id: account.account.id,
                display_name: account.account.display_name,
                avatar_url: account.account.avatar_url,
                verified_email: account.account.verified_email,
                linked_methods: account
                    .account
                    .linked_methods
                    .into_iter()
                    .map(extension_login_method)
                    .collect(),
            }),
            session: Some(extension::NativeSession {
                expires_at: account.session.expires_at,
                credential_store: account.source.label().into(),
            }),
            detail: None,
        },
        ValidationStatus::Expired => ExtensionAccountStatus {
            revision,
            state: ExtensionAccountState::Expired,
            account: None,
            session: None,
            detail: Some("The native account session expired or was revoked".into()),
        },
        ValidationStatus::Unavailable(detail) => ExtensionAccountStatus {
            revision,
            state: ExtensionAccountState::Unavailable,
            account: None,
            session: None,
            detail: Some(detail),
        },
    }
}

fn extension_account_switching(revision: u64) -> ExtensionAccountStatus {
    ExtensionAccountStatus {
        revision,
        state: ExtensionAccountState::Unavailable,
        account: None,
        session: None,
        detail: Some("Switching the local Axiom account context".into()),
    }
}

async fn publish_account_switch_started(
    connection: &ConnectionTo<Client>,
    extension_state: &ExtensionState,
    auth: &AuthManager,
) -> agent_client_protocol::Result<()> {
    // Delete grants are account-local capabilities. Invalidate them at the
    // same publication boundary that tells clients the old account context is
    // no longer usable, before the replacement store can be opened.
    extension_state
        .clear_pending_deletes_for_account_switch()
        .await;
    send_extension_activity(
        connection,
        extension_state,
        None,
        ActivityEvent::AccountChanged {
            status: extension_account_switching(auth.reserve_account_transition_revision()),
        },
    )
}

fn extension_login_method(method: LoginMethod) -> extension::LoginMethod {
    match method {
        LoginMethod::Passkey => extension::LoginMethod::Passkey,
        LoginMethod::Google => extension::LoginMethod::Google,
        LoginMethod::Password => extension::LoginMethod::Password,
        LoginMethod::EthereumWallet => extension::LoginMethod::EthereumWallet,
    }
}

fn extension_billing_status(
    status: crate::billing::BillingStatus,
    revision: u64,
) -> extension::BillingStatus {
    extension::BillingStatus {
        revision,
        posted_microusd: status.posted_microusd,
        available_microusd: status.available_microusd,
        ledger_sequence: status.ledger_sequence,
        trial_microusd: status.trial_microusd,
        paid_microusd: status.paid_microusd,
        payment_review_required: status.payment_review_required,
        currency: status.currency,
        payment_account: status.payment_account,
        zec_usd_quote: status.zec_usd_quote,
    }
}

fn auth_login_method(method: extension::LoginMethod) -> LoginMethod {
    match method {
        extension::LoginMethod::Passkey => LoginMethod::Passkey,
        extension::LoginMethod::Google => LoginMethod::Google,
        extension::LoginMethod::Password => LoginMethod::Password,
        extension::LoginMethod::EthereumWallet => LoginMethod::EthereumWallet,
    }
}

fn validation_status_if_current(
    auth: &AuthManager,
    status: VersionedValidationStatus,
) -> VersionedValidationStatus {
    let revision = status.revision;
    match status.status {
        ValidationStatus::Valid(account) if !auth.account_status_is_current(&account) => {
            let status = if auth.has_credential() {
                ValidationStatus::Unavailable(
                    "Axiom authorization changed while account status was being published; refresh account status"
                        .into(),
                )
            } else {
                ValidationStatus::Missing
            };
            VersionedValidationStatus { status, revision }
        }
        account_status => VersionedValidationStatus {
            status: account_status,
            revision,
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
        AppEvent::TurnStarted { turn_id } => Some(ActivityEvent::ActiveTurnChanged {
            turn_id: Some(turn_id.to_string()),
        }),
        AppEvent::TurnCompleted { .. }
        | AppEvent::TurnCancelled { .. }
        | AppEvent::ErrorRaised {
            turn_id: Some(_), ..
        } => Some(ActivityEvent::ActiveTurnChanged { turn_id: None }),
        AppEvent::RequestUsageUpdated { usage } => Some(ActivityEvent::RequestUsageChanged {
            usage: usage.clone(),
        }),
        AppEvent::ContextUsageUpdated { usage } => Some(ActivityEvent::ContextUsageChanged {
            usage: usage.clone(),
        }),
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
    has_prompt: bool,
    security: crate::app::SecurityStatus,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum SessionWorkKind {
    Prompt(TurnId),
    SlashCommand,
    Compaction,
    SecurityVerification,
    Configuration,
    PermissionMode,
    Loading,
    Deletion,
}

#[derive(Clone)]
struct SessionWork {
    id: uuid::Uuid,
    kind: SessionWorkKind,
    cancellation: CancellationToken,
    steering: Option<Arc<crate::steering::TurnSteering>>,
    web_enabled: Option<bool>,
    agent_revision: u64,
}

impl SessionWork {
    fn new(kind: SessionWorkKind) -> Self {
        Self {
            id: uuid::Uuid::new_v4(),
            steering: match &kind {
                SessionWorkKind::Prompt(turn) => {
                    Some(Arc::new(crate::steering::TurnSteering::new(turn.clone())))
                }
                _ => None,
            },
            web_enabled: None,
            agent_revision: 0,
            kind,
            cancellation: CancellationToken::new(),
        }
    }

    fn matches(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

async fn await_request_future<T>(
    request_cancellation: &agent_client_protocol::RequestCancellation,
    future: impl std::future::Future<Output = T>,
) -> agent_client_protocol::Result<T> {
    if request_cancellation.is_cancelled() {
        return Err(agent_client_protocol::Error::request_cancelled());
    }
    tokio::pin!(future);
    tokio::select! {
        biased;
        result = &mut future => Ok(result),
        () = request_cancellation.cancelled() => {
            Err(agent_client_protocol::Error::request_cancelled())
        }
    }
}

async fn await_session_future<T>(
    request_cancellation: &agent_client_protocol::RequestCancellation,
    work: &SessionWork,
    future: impl std::future::Future<Output = T>,
) -> agent_client_protocol::Result<T> {
    if request_cancellation.is_cancelled() || work.cancellation.is_cancelled() {
        work.cancellation.cancel();
        return Err(agent_client_protocol::Error::request_cancelled());
    }
    tokio::pin!(future);
    tokio::select! {
        // A completed operation is its commit point. Prefer that result when
        // completion and cancellation become ready in the same scheduler poll,
        // so callers never report cancellation after a setter already committed.
        biased;
        result = &mut future => Ok(result),
        () = request_cancellation.cancelled() => {
            work.cancellation.cancel();
            Err(agent_client_protocol::Error::request_cancelled())
        }
        () = work.cancellation.cancelled() => {
            Err(agent_client_protocol::Error::request_cancelled())
        }
    }
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

#[derive(Default)]
struct SessionRegistry {
    sessions: HashMap<protocol::SessionId, AcpSession>,
    active_work: HashMap<SessionId, SessionWork>,
    account_switching: bool,
}

impl Deref for SessionRegistry {
    type Target = HashMap<protocol::SessionId, AcpSession>;

    fn deref(&self) -> &Self::Target {
        &self.sessions
    }
}

impl DerefMut for SessionRegistry {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.sessions
    }
}

type Sessions = Arc<Mutex<SessionRegistry>>;

#[derive(Default)]
struct AccountWorkState {
    switching: bool,
    switch_excluded: Option<uuid::Uuid>,
    active: HashMap<uuid::Uuid, CancellationToken>,
}

#[derive(Default)]
struct AccountWorkTracker {
    state: StdMutex<AccountWorkState>,
    notify: Notify,
}

struct AccountWorkGuard {
    id: uuid::Uuid,
    tracker: Arc<AccountWorkTracker>,
}

/// One ACP request's reservation of the current account store. The tracker
/// prevents a switch from completing while the work is alive, while the bound
/// store independently rejects a delayed operation if the generation or
/// account changed before it acquired the database mutex.
struct AccountStoreWork {
    store: SessionStore,
    cancellation: CancellationToken,
    guard: AccountWorkGuard,
}

impl Drop for AccountWorkGuard {
    fn drop(&mut self) {
        let removed = self
            .tracker
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .active
            .remove(&self.id)
            .is_some();
        if removed {
            self.tracker.notify.notify_waiters();
        }
    }
}

impl AccountWorkTracker {
    fn register(
        self: &Arc<Self>,
        cancellation: CancellationToken,
    ) -> crate::Result<AccountWorkGuard> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| crate::AxiomError::Storage("account work lock was poisoned".into()))?;
        if state.switching {
            return Err(crate::AxiomError::InvalidTransition(
                "the Axiom account is changing; retry after account status settles".into(),
            ));
        }
        let id = uuid::Uuid::new_v4();
        state.active.insert(id, cancellation);
        Ok(AccountWorkGuard {
            id,
            tracker: self.clone(),
        })
    }

    fn register_store(self: &Arc<Self>, store: &SessionStore) -> crate::Result<AccountStoreWork> {
        self.register_store_with_cancellation(store, CancellationToken::new())
    }

    fn register_store_with_cancellation(
        self: &Arc<Self>,
        store: &SessionStore,
        cancellation: CancellationToken,
    ) -> crate::Result<AccountStoreWork> {
        let guard = self.register(cancellation.clone())?;
        let store = store.bind_active_account()?;
        if cancellation.is_cancelled() {
            return Err(crate::AxiomError::Cancelled);
        }
        Ok(AccountStoreWork {
            store,
            cancellation,
            guard,
        })
    }

    fn start_switch(&self, excluding: Option<uuid::Uuid>) -> crate::Result<()> {
        let cancellations = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| crate::AxiomError::Storage("account work lock was poisoned".into()))?;
            if state.switching {
                return Err(crate::AxiomError::InvalidTransition(
                    "an Axiom account transition is already in progress".into(),
                ));
            }
            state.switching = true;
            state.switch_excluded = excluding;
            state
                .active
                .iter()
                .filter(|(id, _)| Some(**id) != excluding)
                .map(|(_, cancellation)| cancellation.clone())
                .collect::<Vec<_>>()
        };
        for cancellation in cancellations {
            cancellation.cancel();
        }
        Ok(())
    }

    async fn drain_switch(&self) -> crate::Result<()> {
        let drain = async {
            loop {
                let notified = self.notify.notified();
                let drained = {
                    let state = self
                        .state
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    state
                        .active
                        .keys()
                        .all(|id| Some(*id) == state.switch_excluded)
                };
                if drained {
                    break;
                }
                notified.await;
            }
        };
        tokio::time::timeout(ACCOUNT_WORK_DRAIN_TIMEOUT, drain)
            .await
            .map_err(|_| {
                crate::AxiomError::InvalidTransition(
                    "previous-account work did not stop; local account switching remains locked"
                        .into(),
                )
            })
    }

    fn finish_switch(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.switching = false;
        state.switch_excluded = None;
    }
}

impl AccountStoreWork {
    fn ensure_current(&self) -> crate::Result<()> {
        if self.cancellation.is_cancelled() {
            return Err(crate::AxiomError::Cancelled);
        }
        self.store.ensure_active_account_binding()
    }

    fn read<T>(
        &self,
        operation: impl FnOnce(&SessionStore) -> crate::Result<T>,
    ) -> crate::Result<T> {
        self.ensure_current()?;
        let value = operation(&self.store)?;
        self.ensure_current()?;
        Ok(value)
    }

    fn mutate<T>(
        &self,
        operation: impl FnOnce(&SessionStore) -> crate::Result<T>,
    ) -> crate::Result<T> {
        self.ensure_current()?;
        let value = operation(&self.store)?;
        self.ensure_current()?;
        Ok(value)
    }

    /// Serialize the final account-derived publication against the switch
    /// start boundary. If this closure runs, its notification/response is
    /// enqueued before `start_switch` can publish the account reset; otherwise
    /// stale account data is dropped with a cancellation error.
    fn publish<T>(
        &self,
        publication: impl FnOnce() -> agent_client_protocol::Result<T>,
    ) -> agent_client_protocol::Result<T> {
        let state = self.guard.tracker.state.lock().map_err(|_| {
            agent_client_protocol::util::internal_error("account work lock was poisoned")
        })?;
        if state.switching
            || !state.active.contains_key(&self.guard.id)
            || self.cancellation.is_cancelled()
        {
            return Err(agent_client_protocol::Error::request_cancelled());
        }
        self.store
            .ensure_active_account_binding()
            .map_err(agent_error)?;
        publication()
    }
}

async fn release_session_work(sessions: &Sessions, internal_id: &SessionId, work: &SessionWork) {
    if let Some(inbox) = &work.steering {
        inbox.close();
    }
    work.cancellation.cancel();
    let mut registry = sessions.lock().await;
    if registry
        .active_work
        .get(internal_id)
        .is_some_and(|active| active.matches(work))
    {
        registry.active_work.remove(internal_id);
    }
}

async fn release_session_works(
    sessions: &Sessions,
    internal_ids: &[SessionId],
    work: &SessionWork,
) {
    work.cancellation.cancel();
    let mut registry = sessions.lock().await;
    for internal_id in internal_ids {
        if work_matches(&registry, internal_id, work) {
            registry.active_work.remove(internal_id);
        }
    }
}

const ACCOUNT_WORK_DRAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Cancel and drain all previous-account work before the caller closes or
/// opens the dynamic store. While this barrier is active, work reservations
/// and `work_matches` both fail, so stale tasks cannot enter a new commit.
async fn begin_account_switch(
    sessions: &Sessions,
    work_tracker: &AccountWorkTracker,
    excluding_work: Option<&SessionWork>,
    excluding_task: Option<&AccountWorkGuard>,
) -> crate::Result<()> {
    let excluded_id = excluding_work.map(|work| work.id);
    {
        let mut registry = sessions.lock().await;
        if registry.account_switching {
            return Err(crate::AxiomError::InvalidTransition(
                "an Axiom account transition is already in progress".into(),
            ));
        }
        registry.account_switching = true;
        let mut seen = HashSet::new();
        for work in registry
            .active_work
            .values()
            .filter(|work| Some(work.id) != excluded_id && seen.insert(work.id))
        {
            work.cancellation.cancel();
        }
    }
    if let Err(error) = work_tracker.start_switch(excluding_task.map(|guard| guard.id)) {
        sessions.lock().await.account_switching = false;
        return Err(error);
    }
    Ok(())
}

async fn drain_account_switch(
    sessions: &Sessions,
    work_tracker: &AccountWorkTracker,
) -> crate::Result<()> {
    work_tracker.drain_switch().await?;
    let mut registry = sessions.lock().await;
    registry.active_work.clear();
    registry.sessions.clear();
    Ok(())
}

async fn finish_account_switch(sessions: &Sessions, work_tracker: &AccountWorkTracker) {
    let mut registry = sessions.lock().await;
    registry.account_switching = false;
    drop(registry);
    work_tracker.finish_switch();
}

async fn commit_account_store_switch(
    sessions: &Sessions,
    work_tracker: &AccountWorkTracker,
    store: Option<&SessionStore>,
    account_id: Option<&str>,
) -> crate::Result<()> {
    drain_account_switch(sessions, work_tracker).await?;
    let result = match (store, account_id) {
        (Some(store), Some(account_id)) => store.activate_account(account_id),
        (Some(store), None) => store.deactivate_account(),
        (None, _) => Ok(()),
    };
    finish_account_switch(sessions, work_tracker).await;
    result
}

fn work_matches(registry: &SessionRegistry, internal_id: &SessionId, work: &SessionWork) -> bool {
    !registry.account_switching
        && registry
            .active_work
            .get(internal_id)
            .is_some_and(|active| active.matches(work))
}

async fn set_session_security_if_work(
    sessions: &Sessions,
    external_id: &protocol::SessionId,
    internal_id: &SessionId,
    work: &SessionWork,
    expected_model: &str,
    status: crate::app::SecurityStatus,
) -> bool {
    let mut registry = sessions.lock().await;
    if !work_matches(&registry, internal_id, work) {
        return false;
    }
    let Some(session) = registry.get_mut(external_id) else {
        return false;
    };
    if session.internal_id != *internal_id || session.model != expected_model {
        return false;
    }
    session.security = status;
    true
}

async fn fail_active_prompt(
    sessions: &Sessions,
    runtime: &Runtime,
    store: Option<&SessionStore>,
    internal_id: &SessionId,
    work: &SessionWork,
    message: &str,
) {
    work.cancellation.cancel();
    let SessionWorkKind::Prompt(turn_id) = &work.kind else {
        tracing::warn!(%internal_id, "non-prompt work passed to prompt finalizer");
        release_session_work(sessions, internal_id, work).await;
        return;
    };
    match runtime
        .dispatch(AppCommand::FailTurn {
            session_id: internal_id.clone(),
            turn_id: turn_id.clone(),
            message: message.to_owned(),
        })
        .await
    {
        Ok(events) => {
            if let Some(store) = store
                && let Err(error) = store.append_all(&events)
            {
                tracing::warn!(%error, %internal_id, "could not persist failed ACP turn");
            }
        }
        Err(error) => {
            tracing::warn!(%error, %internal_id, "could not terminalize failed ACP turn");
        }
    }
    let mut registry = sessions.lock().await;
    if work_matches(&registry, internal_id, work) {
        registry.active_work.remove(internal_id);
    }
}

struct PendingDelete {
    session_ids: Vec<SessionId>,
    expires_at_unix_seconds: u64,
}

#[derive(Clone, Copy)]
enum ExtensionFeature {
    Steering = 1 << 11,
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
    DesktopAgent = 1 << 12,
    Billing = 1 << 10,
    Usage = 1 << 13,
    Attachments = 1 << 14,
    GiftCodes = 1 << 15,
}

struct ExtensionState {
    enabled_features: AtomicU64,
    runtime_instance_id: String,
    sequence: AtomicU64,
    billing_revision: AtomicU64,
    pending_deletes: Mutex<HashMap<String, PendingDelete>>,
    native_logins: Mutex<HashMap<String, crate::auth::NativeLogin>>,
}

impl ExtensionState {
    fn new() -> Self {
        Self {
            enabled_features: AtomicU64::new(0),
            runtime_instance_id: uuid::Uuid::new_v4().to_string(),
            sequence: AtomicU64::new(0),
            billing_revision: AtomicU64::new(0),
            pending_deletes: Mutex::new(HashMap::new()),
            native_logins: Mutex::new(HashMap::new()),
        }
    }

    fn next_sequence(&self) -> u64 {
        self.sequence.fetch_add(1, Ordering::AcqRel) + 1
    }

    fn next_billing_revision(&self) -> u64 {
        self.billing_revision.fetch_add(1, Ordering::AcqRel) + 1
    }

    async fn clear_pending_deletes_for_account_switch(&self) {
        self.pending_deletes.lock().await.clear();
    }

    fn negotiate(&self, features: Option<FeatureVersions>) {
        let Some(features) = features else {
            self.enabled_features.store(0, Ordering::Release);
            return;
        };
        let mut mask = 0;
        if features.attachments >= 2 {
            mask |= ExtensionFeature::Attachments as u64;
        }
        if features.steering >= 1 {
            mask |= ExtensionFeature::Steering as u64;
        }
        if features.thread_catalog >= 1 {
            mask |= ExtensionFeature::ThreadCatalog as u64;
        }
        if features.timeline >= 2 {
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
        if features.account >= 2 {
            mask |= ExtensionFeature::Account as u64;
        }
        if features.billing >= 2 {
            mask |= ExtensionFeature::Billing as u64;
        }
        if features.billing >= 3 {
            mask |= ExtensionFeature::GiftCodes as u64;
        }
        if features.usage >= 1 {
            mask |= ExtensionFeature::Usage as u64;
        }
        if features.security_evidence >= 3 {
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
        if features.desktop_agent >= 1 {
            mask |= ExtensionFeature::DesktopAgent as u64;
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
            protocol::RequestPermissionOutcome::Cancelled => {
                return Err(crate::AxiomError::Cancelled);
            }
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

#[cfg(test)]
fn prompt_text(blocks: &[protocol::ContentBlock]) -> std::result::Result<String, &'static str> {
    let (text, attachments) = prompt_input(blocks)?;
    if text.trim().is_empty() && attachments.is_empty() {
        Err("AxiomCLI currently requires a non-empty text prompt")
    } else {
        Ok(text)
    }
}

fn prompt_input(
    blocks: &[protocol::ContentBlock],
) -> std::result::Result<(String, Vec<axiom_inference::PromptAttachment>), &'static str> {
    let mut text = String::new();
    let mut attachments = Vec::new();
    for block in blocks {
        let content = match block {
            protocol::ContentBlock::Text(content) => content.text.clone(),
            protocol::ContentBlock::ResourceLink(resource) => format!(
                "[Referenced resource: {}]({})",
                resource.title.as_deref().unwrap_or(&resource.name),
                resource.uri
            ),
            protocol::ContentBlock::Image(image) => {
                attachments.push(axiom_inference::PromptAttachment::Image {
                    name: image.uri.clone().unwrap_or_else(|| "Attached image".into()),
                    image: axiom_inference::ImageContent {
                        mime_type: image.mime_type.clone(),
                        data: image.data.clone(),
                    },
                });
                continue;
            }
            protocol::ContentBlock::Resource(_) => {
                return Err(
                    "Use direct file attachments; embedded resource extraction is unsupported",
                );
            }
            _ => return Err("ACP prompt contains a content type AxiomCLI did not advertise"),
        };
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&content);
    }
    Ok((text, attachments))
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
        AppEvent::SteeringApplied {
            client_item_id,
            text,
            ..
        } => {
            let chunk = protocol::ContentChunk::new(protocol::ContentBlock::Text(
                protocol::TextContent::new(text),
            ))
            .message_id(client_item_id.as_str());
            send_session_update(
                connection,
                session_id,
                protocol::SessionUpdate::UserMessageChunk(chunk),
                revision,
                None,
                Some(client_item_id),
            )
        }
        AppEvent::PromptAccepted {
            turn_id,
            text,
            attachments,
        } => {
            let message_id =
                client_item_id.map_or_else(|| format!("user:{turn_id}"), ToOwned::to_owned);
            let mut chunk = protocol::ContentChunk::new(protocol::ContentBlock::Text(
                protocol::TextContent::new(text),
            ))
            .message_id(message_id.as_str());
            if !attachments.is_empty()
                && extension_state
                    .is_some_and(|state| state.feature_enabled(ExtensionFeature::Attachments))
            {
                let mut meta = protocol::Meta::new();
                meta.insert(
                    "axiomAttachments".into(),
                    serde_json::json!(
                        attachments
                            .iter()
                            .map(axiom_inference::PromptAttachment::summary)
                            .collect::<Vec<_>>()
                    ),
                );
                chunk = chunk.meta(meta);
            }
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
                revision.and_then(|value| value.timeline_item_id.clone()),
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
                revision.and_then(|value| value.timeline_item_id.clone()),
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
        AppEvent::ThinkingLevelChanged {
            level: crate::app::ThinkingLevel::ProviderDefault,
        } => Ok(()),
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
    match error {
        crate::AxiomError::SecureProvider {
            code: "PROVIDER_TDX_OUT_OF_DATE",
            message,
            ..
        } => agent_client_protocol::Error::new(-32010, message),
        crate::AxiomError::Cancelled => agent_client_protocol::Error::request_cancelled(),
        crate::AxiomError::SecureProvider {
            kind:
                axiom_inference::ProviderFailureKind::LocalAuthentication
                | axiom_inference::ProviderFailureKind::Authentication,
            ..
        } => agent_client_protocol::Error::new(
            -32001,
            "Refresh your Axiom account or sign in again before verifying this connection.",
        ),
        error => agent_client_protocol::util::internal_error(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn independently_paged_accounting_requires_timeline_version_two() {
        let state = ExtensionState::new();
        let mut features = FeatureVersions::all();
        features.timeline = 1;
        state.negotiate(Some(features));
        assert!(!state.feature_enabled(ExtensionFeature::Timeline));
        features.timeline = 2;
        state.negotiate(Some(features));
        assert!(state.feature_enabled(ExtensionFeature::Timeline));
    }

    #[test]
    fn context_usage_activity_carries_only_replacement_token_metadata() {
        let usage = extension::ContextUsage {
            input_tokens: 75_000,
            output_tokens: 1_000,
            model_id: "test".into(),
            reported_at: "2026-09-07T00:00:00Z".into(),
            context_window_tokens: Some(100_000),
            auto_compact_threshold_tokens: Some(85_000),
        };
        let event = extension_activity(&AppEvent::ContextUsageUpdated { usage }).unwrap();
        assert_eq!(
            serde_json::to_value(event).unwrap(),
            serde_json::json!({
                "kind": "context_usage_changed", "usage": {
                    "inputTokens": 75_000, "outputTokens": 1000, "modelId": "test",
                    "reportedAt": "2026-09-07T00:00:00Z", "contextWindowTokens": 100_000,
                    "autoCompactThresholdTokens": 85_000
                }
            })
        );
    }

    #[tokio::test]
    async fn account_switch_drains_work_paused_before_the_store_lock_before_opening_b() {
        let root = tempfile::tempdir().expect("temporary account root");
        let paths = crate::paths::AxiomPaths::from_roots(
            root.path().join("config"),
            root.path().join("data"),
        );
        paths.prepare().expect("prepare paths");
        let store = SessionStore::account_routed(paths, FrontendKind::DesktopChat);
        store.activate_account("account-a").expect("activate A");
        let sessions: Sessions = Arc::new(Mutex::new(SessionRegistry::default()));
        let tracker = Arc::new(AccountWorkTracker::default());
        let cancellation = CancellationToken::new();
        let guard = tracker
            .register(cancellation.clone())
            .expect("register A work");
        let paused_before_store_lock = Arc::new(Notify::new());
        let resume_store_work = Arc::new(Notify::new());
        let delayed_paused = paused_before_store_lock.clone();
        let delayed_resume = resume_store_work.clone();
        let delayed_store = store.clone();
        let delayed = tokio::spawn(async move {
            let _guard = guard;
            delayed_paused.notify_one();
            cancellation.cancelled().await;
            delayed_resume.notified().await;
            let runtime = Runtime::new(8);
            let session_id = SessionId::new();
            let events = runtime
                .dispatch(AppCommand::CreateSession {
                    session_id,
                    cwd: PathBuf::from("/account-a/private"),
                    origin: Origin::Acp,
                    profile: PermissionProfile::Web,
                })
                .await
                .expect("create delayed A session");
            delayed_store
                .append_all(&events)
                .expect("the drained task finishes against A");
        });

        paused_before_store_lock.notified().await;
        begin_account_switch(&sessions, &tracker, None, None)
            .await
            .expect("begin switch");
        assert!(
            tracker.register(CancellationToken::new()).is_err(),
            "new account work must not enter while the switch barrier is active"
        );
        let switch_sessions = sessions.clone();
        let switch_tracker = tracker.clone();
        let switch_store = store.clone();
        let switch_started = Arc::new(Notify::new());
        let task_started = switch_started.clone();
        let account_switch = tokio::spawn(async move {
            task_started.notify_one();
            commit_account_store_switch(
                &switch_sessions,
                &switch_tracker,
                Some(&switch_store),
                Some("account-b"),
            )
            .await
        });
        switch_started.notified().await;
        tokio::task::yield_now().await;
        assert!(
            !account_switch.is_finished(),
            "B must not open while paused A work still owns its account reservation"
        );

        resume_store_work.notify_one();
        delayed.await.expect("delayed A task");
        account_switch
            .await
            .expect("account switch task")
            .expect("commit B switch");

        assert_eq!(store.active_account_id().as_deref(), Some("account-b"));
        assert!(store.list(true).expect("B catalog").is_empty());
        store.activate_account("account-a").expect("reactivate A");
        assert_eq!(store.list(true).expect("A catalog").len(), 1);
    }

    #[tokio::test]
    async fn bound_acp_store_work_paused_before_lock_fails_instead_of_crossing_accounts() {
        let root = tempfile::tempdir().expect("temporary account root");
        let paths = crate::paths::AxiomPaths::from_roots(
            root.path().join("config"),
            root.path().join("data"),
        );
        paths.prepare().expect("prepare paths");
        let store = SessionStore::account_routed(paths, FrontendKind::DesktopChat);
        store.activate_account("account-a").expect("activate A");
        let sessions: Sessions = Arc::new(Mutex::new(SessionRegistry::default()));
        let tracker = Arc::new(AccountWorkTracker::default());
        let account_store = tracker.register_store(&store).expect("bind ACP work to A");
        let paused_before_store_lock = Arc::new(Notify::new());
        let resume_store_work = Arc::new(Notify::new());
        let delayed_paused = paused_before_store_lock.clone();
        let delayed_resume = resume_store_work.clone();
        let delayed = tokio::spawn(async move {
            delayed_paused.notify_one();
            delayed_resume.notified().await;
            account_store.read(|store| store.thread_catalog(None, true, None, 200))
        });

        paused_before_store_lock.notified().await;
        begin_account_switch(&sessions, &tracker, None, None)
            .await
            .expect("begin switch");
        let switch_sessions = sessions.clone();
        let switch_tracker = tracker.clone();
        let switch_store = store.clone();
        let account_switch = tokio::spawn(async move {
            commit_account_store_switch(
                &switch_sessions,
                &switch_tracker,
                Some(&switch_store),
                Some("account-b"),
            )
            .await
        });

        resume_store_work.notify_one();
        let error = delayed
            .await
            .expect("delayed ACP store task")
            .expect_err("cancelled A work must not return a catalog");
        assert!(matches!(error, crate::AxiomError::Cancelled));
        account_switch
            .await
            .expect("account switch task")
            .expect("commit B switch");
        assert_eq!(store.active_account_id().as_deref(), Some("account-b"));
        assert!(store.list(true).expect("B catalog").is_empty());
    }

    #[tokio::test]
    async fn account_derived_publication_finishes_before_switch_start_or_is_dropped() {
        let store = SessionStore::in_memory().expect("store");
        let tracker = Arc::new(AccountWorkTracker::default());
        let account_store = tracker.register_store(&store).expect("register store work");
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let publication = tokio::task::spawn_blocking(move || {
            account_store.publish(|| {
                entered_tx.send(()).expect("signal publication");
                release_rx.recv().expect("release publication");
                Ok(())
            })
        });
        entered_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("publication entered");

        let switch_tracker = tracker.clone();
        let (switch_done_tx, switch_done_rx) = std::sync::mpsc::channel();
        let switch = tokio::task::spawn_blocking(move || {
            let result = switch_tracker.start_switch(None);
            switch_done_tx.send(()).expect("signal switch result");
            result
        });
        assert!(matches!(
            switch_done_rx.recv_timeout(std::time::Duration::from_millis(50)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ));

        release_tx.send(()).expect("release publication");
        publication
            .await
            .expect("publication task")
            .expect("account A publication");
        switch
            .await
            .expect("switch task")
            .expect("switch starts after publication");

        let stale = tracker
            .register_store(&store)
            .err()
            .expect("new publication work is rejected during the switch");
        assert!(matches!(stale, crate::AxiomError::InvalidTransition(_)));
    }

    #[tokio::test]
    async fn account_switch_publication_boundary_clears_pending_delete_grants() {
        let extension = ExtensionState::new();
        extension.pending_deletes.lock().await.insert(
            "account-a-grant".into(),
            PendingDelete {
                session_ids: vec![SessionId::new()],
                expires_at_unix_seconds: u64::MAX,
            },
        );

        extension.clear_pending_deletes_for_account_switch().await;

        assert!(extension.pending_deletes.lock().await.is_empty());
    }

    #[test]
    fn cancelled_operations_use_the_standard_json_rpc_cancellation_code() {
        let error = agent_error(crate::AxiomError::Cancelled);
        assert_eq!(i32::from(error.code), -32800);
    }

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
    fn workload_manifest_projection_preserves_one_mib_and_bounds_unicode_truncation() {
        let limit = extension::MAX_WORKLOAD_MANIFEST_BYTES;
        assert_eq!(limit, 1024 * 1024);
        for value in ["x".repeat(limit), "é".repeat(limit / 2)] {
            assert_eq!(bounded_extension_text(value.clone(), limit), value);
        }
        for value in ["x".repeat(limit + 1), "é".repeat(limit / 2 + 1)] {
            let projected = bounded_extension_text(value.clone(), limit);
            assert!(projected.len() <= limit);
            assert!(projected.ends_with('…'));
            assert!(value.starts_with(projected.trim_end_matches('…')));
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
        };
        assert!(
            extension_security_evidence(crate::app::SecurityStatus::Verified, evidence).is_err()
        );
    }

    #[test]
    fn tinfoil_security_report_does_not_require_a_relay_lease_generation() {
        let evidence = axiom_secure_client::SecurityEvidence {
            state: axiom_secure_client::SecurityState::Verified,
            provider_id: "tinfoil".into(),
            model_id: "tinfoil-gpt-oss-120b".into(),
            attestation_protocol: "tinfoil-snp-sigstore-v1".into(),
            e2ee_protocol: "tinfoil-ehbp-v1".into(),
            e2ee_encryption_version: 1,
            trust_policy_version: "test-policy".into(),
            verified_at_unix_seconds: 1,
            attestation_generation: None,
            hard_expires_at_unix_seconds: Some(241),
            model_key_fingerprint: "00".repeat(32),
            tls_spki_fingerprint: None,
            checks: Vec::new(),
            provider_claims: Vec::new(),
            workload_manifest: None,
        };
        let projected =
            extension_security_evidence(crate::app::SecurityStatus::Verified, evidence.clone())
                .expect("Tinfoil evidence has no relay lease");
        assert_eq!(projected.attestation_generation, None);
        assert_eq!(projected.hard_expires_at_unix_seconds, 241);
        for index in 0..5 {
            let mut invalid = evidence.clone();
            match index {
                0 => invalid.provider_id = "near".into(),
                1 => invalid.attestation_protocol = "other".into(),
                2 => invalid.e2ee_protocol = "other".into(),
                3 => invalid.attestation_generation = Some(0),
                _ => invalid.hard_expires_at_unix_seconds = None,
            }
            assert!(
                extension_security_evidence(crate::app::SecurityStatus::Verified, invalid,)
                    .is_err()
            );
        }
    }

    #[test]
    fn applying_a_different_model_invalidates_verified_session_security() {
        let mut session = AcpSession {
            internal_id: SessionId::new(),
            cwd: PathBuf::from("/workspace"),
            profile: PermissionProfile::Web,
            model: "near/model-a".into(),
            thinking: crate::app::ThinkingLevel::High,
            models: vec!["near/model-a".into(), "tinfoil/model-b".into()],
            has_prompt: false,
            security: crate::app::SecurityStatus::Verified,
        };

        apply_session_model_settings(
            &mut session,
            "tinfoil/model-b",
            crate::app::ThinkingLevel::Medium,
            &["near/model-a".into(), "tinfoil/model-b".into()],
            true,
        );
        assert_eq!(session.security, crate::app::SecurityStatus::Unverified);

        session.security = crate::app::SecurityStatus::Verified;
        apply_session_model_settings(
            &mut session,
            "tinfoil/model-b",
            crate::app::ThinkingLevel::Medium,
            &["near/model-a".into(), "tinfoil/model-b".into()],
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

    #[test]
    fn desktop_web_requires_an_explicit_boolean_opt_in_on_every_prompt() {
        assert!(!prompt_web_enabled(FrontendKind::DesktopChat, None));
        assert!(prompt_web_enabled(FrontendKind::Cli, None));
        for (value, expected) in [
            (serde_json::json!({"clientItemId":"test"}), false),
            (
                serde_json::json!({"clientItemId":"test","webEnabled":false}),
                false,
            ),
            (
                serde_json::json!({"clientItemId":"test","webEnabled":true}),
                true,
            ),
        ] {
            let meta = serde_json::from_value(serde_json::json!({"axiom":value})).expect("meta");
            let decoded = prompt_metadata(Some(&meta)).expect("valid metadata");
            assert_eq!(
                prompt_web_enabled(FrontendKind::DesktopChat, decoded.as_ref()),
                expected
            );
        }
        for invalid in [
            serde_json::json!("true"),
            serde_json::json!(1),
            serde_json::Value::Null,
        ] {
            let meta = serde_json::from_value(serde_json::json!({
                "axiom":{"clientItemId":"test","webEnabled":invalid}
            }))
            .expect("meta");
            assert!(prompt_metadata(Some(&meta)).is_err());
        }
    }
}
