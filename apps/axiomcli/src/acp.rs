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
    BillingInvoiceCreateRequest, BillingInvoiceResponse, BillingInvoiceStatusRequest,
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
    billing::{BillingClient, InvoiceState as BackendInvoiceState},
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
        .filter(|spec| !matches!(spec.name, "theme" | "web"))
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
            options.push(thinking_config_option(thinking))
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
        .filter(|spec| !matches!(spec.name, "theme" | "web"))
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
        last_message_at: summary.last_message_at,
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
        held_microusd: status.held_microusd,
        available_microusd: status.available_microusd,
        ledger_sequence: status.ledger_sequence,
        trial_microusd: status.trial_microusd,
        paid_microusd: status.paid_microusd,
        payment_review_required: status.payment_review_required,
        currency: status.currency,
        invoices_enabled: status.invoices_enabled,
        invoice_min_usd_cents: status.invoice_min_usd_cents,
        invoice_max_usd_cents: status.invoice_max_usd_cents,
        settlement_asset: status.settlement_asset,
        payment_account: status.payment_account,
        zec_usd_quote: status.zec_usd_quote,
    }
}

fn extension_invoice(invoice: crate::billing::Invoice, revision: u64) -> extension::BillingInvoice {
    extension::BillingInvoice {
        revision,
        id: invoice.id,
        amount_usd_cents: invoice.amount_usd_cents,
        state: match invoice.state {
            BackendInvoiceState::Creating => extension::InvoiceState::Creating,
            BackendInvoiceState::Pending => extension::InvoiceState::Pending,
            BackendInvoiceState::Paid => extension::InvoiceState::Paid,
            BackendInvoiceState::Expired => extension::InvoiceState::Expired,
            BackendInvoiceState::Failed => extension::InvoiceState::Failed,
            BackendInvoiceState::Review => extension::InvoiceState::Review,
        },
        expires_at: invoice.expires_at,
        paid_at: invoice.paid_at,
        pay_url: invoice.pay_url,
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
        if features.steering >= 1 {
            mask |= ExtensionFeature::Steering as u64;
        }
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
        if features.account >= 2 {
            mask |= ExtensionFeature::Account as u64;
        }
        if features.billing >= 1 {
            mask |= ExtensionFeature::Billing as u64;
        }
        if features.usage >= 1 {
            mask |= ExtensionFeature::Usage as u64;
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
) -> agent_client_protocol::Result<()> {
    // Electron validates and opens desktop sign-in URLs in the user's actual
    // desktop session. The CLI's isolated staging storage must not select a
    // different browser or browser profile.
    let auth = if frontend == FrontendKind::DesktopChat {
        auth.without_browser_launch()
    } else {
        auth
    };
    let sessions: Sessions = Arc::new(Mutex::new(SessionRegistry::default()));
    let account_work = Arc::new(AccountWorkTracker::default());
    let account_transition = Arc::new(Mutex::new(()));
    let extension_state = Arc::new(ExtensionState::new());
    let billing = BillingClient::new(&config.base_url, auth.clone(), config.request_timeout())
        .map_err(agent_error)?;
    let profile_settings_lock = Arc::new(Mutex::new(()));
    let persistence_failpoint = Arc::new(PersistenceFailpoint::from_environment());
    let runtime = Runtime::new(512);
    let default_model = config.model.clone();
    let default_thinking = crate::app::ThinkingLevel::Medium;
    let shutdown_runner = runner.clone();

    let new_sessions = sessions.clone();
    let new_account_work = account_work.clone();
    let new_runtime = runtime.clone();
    let new_runner = runner.clone();
    let new_config = config;
    let new_frontend = frontend;
    let prompt_sessions = sessions.clone();
    let steer_sessions = sessions.clone();
    let steer_extension = extension_state.clone();
    let prompt_runtime = runtime.clone();
    let cancel_sessions = sessions.clone();
    let new_store = store.clone();
    let agent_settings_store = store.clone();
    let agent_settings_sessions = sessions.clone();
    let agent_settings_runtime = runtime.clone();
    let agent_settings_runner = runner.clone();
    let agent_settings_extension = extension_state.clone();
    let agent_settings_account_work = account_work.clone();
    let new_extension = extension_state.clone();
    let load_store = store.clone();
    let extension_store = store.clone();
    let prompt_store = store;
    let prompt_auth = auth.clone();
    let extension_auth = auth;
    let load_sessions = sessions.clone();
    let load_account_work = account_work.clone();
    let load_runtime = runtime.clone();
    let load_runner = runner.clone();
    let load_extension = extension_state.clone();
    let prompt_extension = extension_state.clone();
    let prompt_account_work = account_work.clone();
    let prompt_account_transition = account_transition.clone();
    let prompt_profile_settings_lock = profile_settings_lock.clone();
    let prompt_persistence_failpoint = persistence_failpoint.clone();
    let new_default_model = default_model.clone();
    let load_default_model = default_model;
    let new_default_thinking = default_thinking;
    let load_default_thinking = default_thinking;
    let config_sessions = sessions.clone();
    let config_runtime = runtime.clone();
    let config_runner = runner.clone();
    let config_store = prompt_store.clone();
    let config_extension = extension_state.clone();
    let config_profile_settings_lock = profile_settings_lock.clone();
    let config_persistence_failpoint = persistence_failpoint.clone();
    let mode_sessions = sessions.clone();
    let mode_runtime = runtime.clone();
    let mode_store = prompt_store.clone();
    let mode_persistence_failpoint = persistence_failpoint;
    let mode_frontend = frontend;
    let supports_load = load_store.is_some();
    let supports_elicitation = Arc::new(AtomicBool::new(false));
    let initialize_elicitation = supports_elicitation.clone();
    let initialize_extension = extension_state.clone();
    let list_extension = extension_state.clone();
    let list_extension_store = extension_store.clone();
    let list_account_work = account_work.clone();
    let rename_thread_extension = extension_state.clone();
    let rename_thread_store = extension_store.clone();
    let rename_thread_account_work = account_work.clone();
    let timeline_extension = extension_state.clone();
    let timeline_extension_store = extension_store.clone();
    let timeline_account_work = account_work.clone();
    let timeline_runner = runner.clone();
    let preview_extension = extension_state.clone();
    let preview_extension_store = extension_store.clone();
    let preview_extension_sessions = sessions.clone();
    let preview_account_work = account_work.clone();
    let delete_extension = extension_state.clone();
    let delete_extension_store = extension_store.clone();
    let delete_extension_sessions = sessions.clone();
    let delete_account_work = account_work.clone();
    let models_extension = extension_state.clone();
    let models_extension_runner = runner.clone();
    let stored_preferences_extension = extension_state.clone();
    let stored_preferences_store = extension_store.clone();
    let stored_preferences_account_work = account_work.clone();
    let update_preferences_extension = extension_state.clone();
    let update_preferences_store = extension_store.clone();
    let update_preferences_runner = runner.clone();
    let update_preferences_lock = profile_settings_lock;
    let update_preferences_account_work = account_work.clone();
    let collections_list_extension = extension_state.clone();
    let collections_list_store = extension_store.clone();
    let collections_list_account_work = account_work.clone();
    let collection_create_extension = extension_state.clone();
    let collection_create_store = extension_store.clone();
    let collection_create_account_work = account_work.clone();
    let collection_rename_extension = extension_state.clone();
    let collection_rename_store = extension_store.clone();
    let collection_rename_account_work = account_work.clone();
    let collection_collapsed_extension = extension_state.clone();
    let collection_collapsed_store = extension_store.clone();
    let collection_collapsed_account_work = account_work.clone();
    let collection_move_extension = extension_state.clone();
    let collection_move_store = extension_store.clone();
    let collection_move_account_work = account_work.clone();
    let collection_delete_extension = extension_state.clone();
    let collection_delete_store = extension_store.clone();
    let collection_delete_account_work = account_work.clone();
    let collection_assign_extension = extension_state.clone();
    let collection_assign_store = extension_store.clone();
    let collection_assign_account_work = account_work.clone();
    let account_status_extension = extension_state.clone();
    let account_status_auth = extension_auth.clone();
    let account_status_store = extension_store.clone();
    let account_status_sessions = sessions.clone();
    let account_status_work = account_work.clone();
    let account_status_transition = account_transition.clone();
    let login_start_extension = extension_state.clone();
    let login_start_auth = extension_auth.clone();
    let login_complete_extension = extension_state.clone();
    let login_complete_auth = extension_auth.clone();
    let login_complete_store = extension_store.clone();
    let login_complete_sessions = sessions.clone();
    let login_complete_work = account_work.clone();
    let login_complete_transition = account_transition.clone();
    let login_cancel_extension = extension_state.clone();
    let login_cancel_auth = extension_auth.clone();
    let logout_extension = extension_state.clone();
    let logout_auth = extension_auth;
    let logout_store = extension_store.clone();
    let logout_sessions = sessions.clone();
    let logout_work = account_work.clone();
    let logout_transition = account_transition;
    let billing_status_extension = extension_state.clone();
    let billing_status_client = billing.clone();
    let billing_status_work = account_work.clone();
    let usage_extension = extension_state.clone();
    let usage_client = billing.clone();
    let usage_work = account_work.clone();
    let keys_list_extension = extension_state.clone();
    let keys_list_client = billing.clone();
    let keys_list_work = account_work.clone();
    let keys_create_extension = extension_state.clone();
    let keys_create_client = billing.clone();
    let keys_create_work = account_work.clone();
    let keys_revoke_extension = extension_state.clone();
    let keys_revoke_client = billing.clone();
    let keys_revoke_work = account_work.clone();
    let billing_create_extension = extension_state.clone();
    let billing_create_client = billing.clone();
    let billing_create_work = account_work.clone();
    let billing_invoice_extension = extension_state.clone();
    let billing_invoice_client = billing;
    let billing_invoice_work = account_work.clone();
    let security_extension = extension_state.clone();
    let security_extension_sessions = sessions.clone();
    let security_extension_runner = runner.clone();
    let security_account_work = account_work.clone();
    let compact_extension = extension_state.clone();
    let compact_extension_sessions = sessions.clone();
    let compact_extension_runner = runner.clone();
    let compact_extension_runtime = runtime.clone();
    let compact_extension_store = extension_store.clone();
    let compact_account_work = account_work.clone();
    let bootstrap_extension = extension_state.clone();
    let bootstrap_store = prompt_store.clone();
    let bootstrap_default_model = new_default_model.clone();
    let bootstrap_default_thinking = default_thinking;
    let load_frontend = frontend;
    let mode_account_work = account_work.clone();
    let config_account_work = account_work;

    let connection_result = Box::pin(
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
                let cwd = bootstrap_store
                    .as_ref()
                    .and_then(|store| store.active_desktop_cwd().ok());
                let durable_preferences = if let Some(store) = &bootstrap_store
                    && store.is_active()
                {
                    Some(respond_or_return!(
                        responder,
                        store.profile_preferences().map_err(agent_error)
                    ))
                } else {
                    None
                };
                let preferences = match durable_preferences {
                    Some(preferences) => extension::ProfilePreferences {
                        model: Some(
                            preferences
                                .model
                                .unwrap_or_else(|| bootstrap_default_model.clone()),
                        ),
                        thinking_level: preferences.thinking_level.to_string(),
                        updated_at: preferences.updated_at,
                    },
                    None => extension::ProfilePreferences {
                        model: Some(bootstrap_default_model.clone()),
                        thinking_level: bootstrap_default_thinking.to_string(),
                        updated_at: chrono::Utc::now().to_rfc3339(),
                    },
                };
                responder.respond(DesktopBootstrapResponse {
                    frontend: frontend.to_string(),
                    chat_cwd: cwd.map(|path| path.display().to_string()),
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
                let new_config = new_config.clone();
                let new_store = new_store.clone();
                let new_runner = new_runner.clone();
                let new_default_model = new_default_model.clone();
                let new_runtime = new_runtime.clone();
                let new_sessions = new_sessions.clone();
                let new_extension = new_extension.clone();
                let account_cancellation = CancellationToken::new();
                let account_guard = respond_or_return!(
                    responder,
                    new_account_work
                        .register(account_cancellation.clone())
                        .map_err(agent_error)
                );
                let task_connection = connection.clone();
                connection.spawn(async move {
                let _account_guard = account_guard;
                let connection = task_connection;
                if !request.cwd.is_absolute() {
                    return responder
                        .respond_with_internal_error("ACP session cwd must be absolute");
                }
                let mut cwd = respond_or_return!(
                    responder,
                    request.cwd.canonicalize().map_err(|error| {
                        agent_client_protocol::util::internal_error(format!(
                            "cannot open ACP workspace: {error}"
                        ))
                    })
                );
                let profile = if new_frontend == FrontendKind::DesktopChat {
                    let expected = respond_or_return!(
                        responder,
                        new_store
                            .as_ref()
                            .ok_or_else(|| agent_client_protocol::util::internal_error(
                                "authentication required; account storage is unavailable"
                            ))
                            .and_then(|store| store.active_desktop_cwd().map_err(agent_error))
                    );
                    if cwd != expected {
                        return responder.respond_with_internal_error(
                            "desktop chat sessions must use the application chat workspace",
                        );
                    }
                    PermissionProfile::Web
                } else {
                    respond_or_return!(
                        responder,
                        new_config.for_workspace(&cwd).map_err(agent_error)
                    )
                    .permission_profile
                };
                let internal_id = SessionId::new();
                if new_frontend == FrontendKind::DesktopChat {
                    cwd = respond_or_return!(responder, new_store.as_ref().expect("desktop store checked").active_desktop_thread_cwd(&internal_id).map_err(agent_error));
                }
                let preferences = respond_or_return!(
                    responder,
                    new_store
                        .as_ref()
                        .map(SessionStore::profile_preferences)
                        .transpose()
                        .map_err(agent_error)
                );
                let preferred_model = preferences
                    .as_ref()
                    .and_then(|preferences| preferences.model.as_deref());
                let preferred_thinking = preferences
                    .as_ref()
                    .map_or(new_default_thinking, |preferences| preferences.thinking_level);
                    let request_cancellation = responder.cancellation();
                    let discovery_cancellation = account_cancellation.clone();
                    let discovery = new_runner
                        .available_model_details(discovery_cancellation.clone());
                    tokio::pin!(discovery);
                    let model_details = tokio::select! {
                        result = &mut discovery => respond_or_return!(responder, result.map_err(agent_error)),
                        () = request_cancellation.cancelled() => {
                            discovery_cancellation.cancel();
                            return responder.respond_with_error(
                                agent_client_protocol::Error::request_cancelled(),
                            );
                        }
                        () = account_cancellation.cancelled() => {
                            return responder.respond_with_error(
                                agent_client_protocol::Error::request_cancelled(),
                            );
                        }
                    };
                let settings = respond_or_return!(
                    responder,
                    reconcile_new_session_settings(
                        model_details.clone(),
                        preferred_model,
                        &new_default_model,
                        preferred_thinking,
                    )
                    .map_err(agent_error)
                );
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
                let supported_thinking = respond_or_return!(
                    responder,
                    model_details
                        .iter()
                        .find(|model| model.id == session_model)
                        .map(supported_thinking_levels)
                        .ok_or_else(|| {
                            agent_client_protocol::util::internal_error(
                                "selected model disappeared from the provider catalog",
                            )
                        })
                );
                let created = respond_or_return!(
                    responder,
                    new_runtime
                        .dispatch(AppCommand::CreateSession {
                            session_id: internal_id.clone(),
                            cwd: cwd.clone(),
                            origin: Origin::Acp,
                            profile,
                        })
                        .await
                        .map_err(agent_error)
                );
                if let Some(store) = &new_store {
                    respond_or_return!(responder, store.append_all(&created).map_err(agent_error));
                }
                respond_or_return!(
                    responder,
                    new_runner
                        .restore_session(&internal_id, &created)
                        .await
                        .map_err(agent_error)
                );
                respond_or_return!(
                    responder,
                    new_runner
                        .set_model_settings(&internal_id, &settings)
                        .await
                        .map_err(agent_error)
                );
                let selected_settings = respond_or_return!(
                    responder,
                    new_runtime
                        .dispatch(AppCommand::ChangeModelSettings {
                            session_id: internal_id.clone(),
                            model: session_model.clone(),
                            thinking: session_thinking,
                            reset_security: false,
                        })
                        .await
                        .map_err(agent_error)
                );
                let corrected_preferences = if let Some(store) = &new_store {
                    if correct_preferences {
                        let (_, preferences) = respond_or_return!(
                            responder,
                            store
                                .append_all_and_set_profile_preferences(
                                    &selected_settings,
                                    &session_model,
                                    session_thinking,
                                )
                                .map_err(agent_error)
                        );
                        Some(extension_preferences(preferences))
                    } else {
                        respond_or_return!(
                            responder,
                            store.append_all(&selected_settings).map_err(agent_error)
                        );
                        None
                    }
                } else {
                    None
                };
                let external_id = protocol::SessionId::new(internal_id.to_string());
                new_sessions.lock().await.insert(
                    external_id.clone(),
                    AcpSession {
                        internal_id: internal_id.clone(),
                        cwd,
                        profile,
                        model: session_model.clone(),
                        thinking: session_thinking,
                        models: models.clone(),
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
                })?;
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: protocol::LoadSessionRequest,
                        responder: Responder<protocol::LoadSessionResponse>,
                        connection: ConnectionTo<Client>| {
                let load_store = load_store.clone();
                let load_sessions = load_sessions.clone();
                let load_default_model = load_default_model.clone();
                let load_runner = load_runner.clone();
                let load_runtime = load_runtime.clone();
                let load_extension = load_extension.clone();
                let Some(store) = load_store else {
                    return responder.respond_with_internal_error(
                        "durable session loading is unavailable",
                    );
                };
                if !request.cwd.is_absolute() {
                    return responder
                        .respond_with_internal_error("ACP session cwd must be absolute");
                }
                let external_id = request.session_id.clone();
                let internal_id = respond_or_return!(
                    responder,
                    SessionId::from_str(request.session_id.0.as_ref()).map_err(|_| {
                        agent_client_protocol::util::internal_error("invalid session ID")
                    })
                );
                let work = SessionWork::new(SessionWorkKind::Loading);
                {
                    let mut registry = load_sessions.lock().await;
                    if registry
                        .values()
                        .any(|session| session.internal_id == internal_id)
                    {
                        return responder
                            .respond_with_internal_error("ACP session is already loaded");
                    }
                    if registry.active_work.contains_key(&internal_id) {
                        return responder.respond_with_internal_error(
                            "ACP session is already loading or being deleted",
                        );
                    }
                    registry
                        .active_work
                        .insert(internal_id.clone(), work.clone());
                }
                let request_cancellation = responder.cancellation();
                let task_connection = connection.clone();
                let rollback_sessions = load_sessions.clone();
                let rollback_internal_id = internal_id.clone();
                let rollback_work = work.clone();
                let account_guard = match load_account_work.register(work.cancellation.clone()) {
                    Ok(guard) => guard,
                    Err(error) => {
                        release_session_work(&load_sessions, &internal_id, &work).await;
                        return responder.respond_with_error(agent_error(error));
                    }
                };
                let spawn_result = connection.spawn(async move {
                let _account_guard = account_guard;
                let connection = task_connection;
                let task_result: agent_client_protocol::Result<_> = async {
                let durable_store = store.clone();
                let durable_internal_id = internal_id.clone();
                let durable_load = tokio::task::spawn_blocking(move || {
                    let summary = durable_store.summary(&durable_internal_id)?;
                    let loaded = durable_store.load_recovering(&durable_internal_id)?;
                    Ok::<_, crate::AxiomError>((summary, loaded))
                });
                let (summary, loaded) = await_session_future(
                    &request_cancellation,
                    &work,
                    durable_load,
                )
                .await?
                .map_err(|error| {
                    agent_client_protocol::util::internal_error(format!(
                        "durable session load worker failed: {error}"
                    ))
                })?
                .map_err(agent_error)?;
                // Desktop must be able to reopen local history even when its
                // project folder moved, so the user can select a replacement.
                // Local Agent execution revalidates the saved identity below.
                let requested_cwd = if load_frontend == FrontendKind::DesktopChat {
                    request.cwd.clone()
                } else { request.cwd.canonicalize().map_err(|error| {
                        agent_client_protocol::util::internal_error(format!(
                            "cannot open ACP workspace: {error}"
                        ))
                    })? };
                let saved_cwd = if load_frontend == FrontendKind::DesktopChat {
                    summary.cwd.clone()
                } else { summary.cwd.canonicalize().map_err(|error| {
                        agent_client_protocol::util::internal_error(format!(
                            "saved ACP workspace is unavailable: {error}"
                        ))
                    })? };
                if requested_cwd != saved_cwd {
                    return Err(agent_client_protocol::util::internal_error(
                        "ACP load cwd does not match the session workspace identity",
                    ));
                }
                if load_frontend == FrontendKind::DesktopChat {
                    let agent = store.desktop_agent_settings(&internal_id).map_err(agent_error)?;
                    let legacy = store.active_desktop_cwd().map_err(agent_error)?;
                    if saved_cwd != std::path::Path::new(&agent.working_directory)
                        && !(agent.revision == 0 && saved_cwd == legacy) {
                        return Err(agent_client_protocol::util::internal_error(
                            "desktop chat session has an invalid workspace identity",
                        ));
                    }
                }
                if let Some(warning) = loaded.warnings.first() {
                    return Err(agent_client_protocol::util::internal_error(warning.clone()));
                }
                let restored_model = model_for_resume(&loaded.events, &load_default_model);
                let restored_thinking =
                    thinking_for_resume(&loaded.events, load_default_thinking);
                let model_details = await_session_future(
                    &request_cancellation,
                    &work,
                    load_runner.available_model_details(work.cancellation.clone()),
                )
                .await?
                .map_err(agent_error)?;
                let settings = crate::agent::reconcile_resumed_session_settings(
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
                        })
                        ?;
                await_session_future(
                    &request_cancellation,
                    &work,
                    load_runner
                        .restore_session(&internal_id, &loaded.events),
                )
                .await?
                .map_err(agent_error)?;
                await_session_future(
                    &request_cancellation,
                    &work,
                    load_runner
                        .set_model_settings(&internal_id, &settings),
                )
                .await?
                .map_err(agent_error)?;
                // Completing the runner settings is the load commit point.
                // Cancellation before it leaves no registered ACP session;
                // after it, finish restoring Runtime/storage/session state so
                // the runner and ACP mirror cannot diverge.
                let restored = load_runtime
                    .restore(&loaded.events)
                    .await
                    .map_err(agent_error)?;
                for (index, envelope) in loaded.events.iter().enumerate() {
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
                    if (index + 1) % 128 == 0 {
                        tokio::task::yield_now().await;
                    }
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
                    PermissionProfile::for_desktop_agent(&store.desktop_agent_settings(&internal_id).map_err(agent_error)?)
                } else {
                    permission_for_resume(&loaded.events, restored.profile)
                };
                let response = protocol::LoadSessionResponse::new()
                    .modes(permission_modes(restored_profile, load_frontend))
                    .config_options(session_config_options(
                        &session_model,
                        &models,
                        session_thinking,
                        Some(&supported_thinking),
                    ));
                {
                    let mut registry = load_sessions.lock().await;
                    if !work_matches(&registry, &internal_id, &work) {
                        return Err(agent_client_protocol::Error::request_cancelled());
                    }
                    if registry
                        .values()
                        .any(|session| session.internal_id == internal_id)
                    {
                        return Err(agent_client_protocol::util::internal_error(
                            "ACP session became loaded while restoring",
                        ));
                    }
                    registry.insert(external_id.clone(), AcpSession {
                        internal_id: internal_id.clone(),
                        cwd: saved_cwd,
                        profile: restored_profile,
                        model: session_model.clone(),
                        thinking: session_thinking,
                        models: models.clone(),
                        has_prompt: loaded.events.iter().any(|envelope| {
                            matches!(envelope.event, AppEvent::PromptAccepted { .. })
                        }),
                        security: crate::app::SecurityStatus::Unverified,
                    });
                }
                Ok(response)
                }.await;
                release_session_work(&load_sessions, &internal_id, &work).await;
                match task_result {
                    Ok(response) => {
                        responder.respond(response)?;
                        send_available_commands(&connection, &external_id)
                    }
                    Err(error) => responder.respond_with_error(error),
                }
                });
                if let Err(error) = spawn_result {
                    release_session_work(
                        &rollback_sessions,
                        &rollback_internal_id,
                        &rollback_work,
                    )
                    .await;
                    return Err(error);
                }
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: protocol::PromptRequest,
                        responder: Responder<protocol::PromptResponse>,
                        connection: ConnectionTo<Client>| {
                let runner = runner.clone();
                let prompt_sessions = prompt_sessions.clone();
                let prompt_runtime = prompt_runtime.clone();
                let prompt_store = prompt_store.clone();
                let prompt_auth = prompt_auth.clone();
                let prompt_extension = prompt_extension.clone();
                let prompt_account_work = prompt_account_work.clone();
                let prompt_account_transition = prompt_account_transition.clone();
                let prompt_profile_settings_lock = prompt_profile_settings_lock.clone();
                let prompt_persistence_failpoint = prompt_persistence_failpoint.clone();
                let supports_elicitation = supports_elicitation.clone();
                let prompt_metadata = respond_or_return!(
                    responder,
                    prompt_metadata(request.meta.as_ref()).map_err(agent_error)
                );
                let web_enabled = prompt_web_enabled(frontend, prompt_metadata.as_ref());
                let agent_revision = prompt_metadata.as_ref().and_then(|metadata| metadata.agent_revision).unwrap_or(0);
                let message_revision = prompt_metadata.as_ref().and_then(|metadata| metadata.revision.clone());
                let client_item_id = prompt_metadata.map(|metadata| metadata.client_item_id);
                let prompt = match prompt_text(&request.prompt) {
                    Ok(prompt) => prompt,
                    Err(message) => return responder.respond_with_internal_error(message),
                };
                let session_id = request.session_id.clone();
                // Edited user text is data, including a leading slash.
                let command = match if message_revision.is_some() { None } else { slash::parse(&prompt) } {
                    Some(Ok(command)) => Some(command),
                    Some(Err(error)) => {
                        send_text(&connection, &session_id, format!("Command error: {error}"))?;
                        return responder.respond(protocol::PromptResponse::new(
                            protocol::StopReason::EndTurn,
                        ));
                    }
                    None => None,
                };
                let deterministic_test_runner = cfg!(debug_assertions)
                    && std::env::var("AXIOMCLI_TEST_RUNNER")
                        .is_ok_and(|value| !value.trim().is_empty());
                if command.is_none()
                    && !prompt_auth.has_credential()
                    && !deterministic_test_runner
                {
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
                let turn_id = command.is_none().then(TurnId::new);
                let mut work = SessionWork::new(turn_id.as_ref().map_or(
                    SessionWorkKind::SlashCommand,
                    |turn_id| SessionWorkKind::Prompt(turn_id.clone()),
                ));
                work.web_enabled = Some(web_enabled);
                let (
                    internal_id,
                    cwd,
                    session_profile,
                    session_model,
                    session_thinking,
                    should_generate_title,
                ) = {
                    let mut registry = prompt_sessions.lock().await;
                    let Some(session) = registry.get(&session_id) else {
                        return responder.respond_with_internal_error("unknown ACP session");
                    };
                    let internal_id = session.internal_id.clone();
                    if registry.active_work.contains_key(&internal_id) {
                        return responder
                            .respond_with_internal_error("session already has active work");
                    }
                    if frontend == FrontendKind::DesktopChat {
                        let agent = respond_or_return!(responder, prompt_store.as_ref().expect("desktop store").desktop_agent_settings(&internal_id).map_err(agent_error));
                        if agent.revision != agent_revision {
                            return responder.respond_with_internal_error("Agent settings changed since this message was queued. Review and resend it.");
                        }
                        if agent.enabled && (!session.cwd.is_dir() || session.cwd.canonicalize().ok().as_ref() != Some(&session.cwd)) {
                            return responder.respond_with_internal_error("The Agent working directory is unavailable or has moved. Choose a folder in Agent settings before sending.");
                        }
                        work.agent_revision = agent.revision;
                    }
                    registry
                        .active_work
                        .insert(internal_id.clone(), work.clone());
                    let session = registry
                        .get_mut(&session_id)
                        .expect("session remained loaded while reserving work");
                    let should_generate_title = turn_id.is_some() && !session.has_prompt;
                    if turn_id.is_some() {
                        session.has_prompt = true;
                    }
                    (
                        internal_id,
                        session.cwd.clone(),
                        session.profile,
                        session.model.clone(),
                        session.thinking,
                        should_generate_title,
                    )
                };
                let request_cancellation = responder.cancellation();
                let task_connection = connection.clone();
                let rollback_sessions = prompt_sessions.clone();
                let rollback_session_id = session_id.clone();
                let rollback_internal_id = internal_id.clone();
                let rollback_work = work.clone();
                let rollback_has_prompt = should_generate_title;
                let account_guard = match prompt_account_work.register(work.cancellation.clone()) {
                    Ok(guard) => Some(guard),
                    Err(error) => {
                        release_session_work(&prompt_sessions, &internal_id, &work).await;
                        return responder.respond_with_error(agent_error(error));
                    }
                };
                let spawn_result = connection.spawn(async move {
                let account_guard = account_guard;
                let connection = task_connection;
                if let Some(command) = command {
                    let slash_result: agent_client_protocol::Result<_> = async {
                    match command {
                        SlashCommand::Theme(_) | SlashCommand::Web(_) => {
                            Err(agent_client_protocol::Error::invalid_params()
                                .data("Use the desktop appearance settings and Web control; /theme and /web are terminal-only commands."))
                        }
                        SlashCommand::Help => {
                            send_text(&connection, &session_id, slash_help_text())?;
                            Ok(protocol::StopReason::EndTurn)
                        }
                        SlashCommand::Permissions => {
                            send_text(
                                &connection,
                                &session_id,
                                "Open your ACP client's session mode selector to change Tools permissions. Crypto permissions are coming soon.".into(),
                            )?;
                            Ok(protocol::StopReason::EndTurn)
                        }
                        SlashCommand::Model => {
                            let models = await_session_future(
                                &request_cancellation,
                                &work,
                                runner.available_models(work.cancellation.clone()),
                            )
                            .await?
                            .map_err(agent_error)?;
                            let current = {
                                let mut registry = prompt_sessions.lock().await;
                                if !work_matches(&registry, &internal_id, &work) {
                                    return Err(agent_client_protocol::Error::request_cancelled());
                                }
                                let session = registry.get_mut(&session_id).ok_or_else(|| {
                                    agent_client_protocol::util::internal_error(
                                        "unknown ACP session",
                                    )
                                })?;
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
                            Ok(protocol::StopReason::EndTurn)
                        }
                        SlashCommand::Resume => {
                            send_text(
                                &connection,
                                &session_id,
                                "Use your ACP client's session list and session/load support to resume a transcript. The interactive /resume picker is available in the terminal UI."
                                    .into(),
                            )?;
                            Ok(protocol::StopReason::EndTurn)
                        }
                        SlashCommand::Delete => {
                            send_text(
                                &connection,
                                &session_id,
                                "The interactive multi-select /delete picker is available in the terminal UI. ACP transcript deletion will use a native client capability when the protocol defines one."
                                    .into(),
                            )?;
                            Ok(protocol::StopReason::EndTurn)
                        }
                        SlashCommand::Thinking(level) => {
                            let details = await_session_future(&request_cancellation, &work,
                                runner.available_model_details(work.cancellation.clone())).await?.map_err(agent_error)?;
                            let model = details.iter().find(|model| model.id == session_model)
                                .ok_or_else(|| agent_client_protocol::util::internal_error("current model is not in the provider catalog"))?;
                            let level = reconcile_model_settings(model, level).thinking;
                            let _profile_settings_guard = await_session_future(
                                &request_cancellation,
                                &work,
                                prompt_profile_settings_lock.lock(),
                            )
                            .await?;
                            await_session_future(
                                &request_cancellation,
                                &work,
                                runner.set_thinking_level(&internal_id, level),
                            )
                            .await?
                            .map_err(agent_error)?;
                            let events = match prompt_runtime
                                .dispatch(AppCommand::ChangeThinkingLevel {
                                    session_id: internal_id.clone(),
                                    level,
                                })
                                .await
                            {
                                Ok(events) => events,
                                Err(error) => {
                                    if let Err(rollback_error) = runner
                                        .set_thinking_level(&internal_id, session_thinking)
                                        .await
                                    {
                                        tracing::warn!(
                                            %rollback_error,
                                            %internal_id,
                                            "could not roll back runner thinking after Runtime rejected slash command"
                                        );
                                    }
                                    return Err(agent_error(error));
                                }
                            };
                            let current = {
                                let mut registry = prompt_sessions.lock().await;
                                if !work_matches(&registry, &internal_id, &work) {
                                    return Err(agent_client_protocol::Error::request_cancelled());
                                }
                                let session = registry.get_mut(&session_id).ok_or_else(|| {
                                    agent_client_protocol::util::internal_error(
                                        "unknown ACP session",
                                    )
                                })?;
                                session.thinking = level;
                                Some((session.model.clone(), session.models.clone()))
                            };
                            let stored_preferences = if let Some(store) = &prompt_store {
                                let persistence = prompt_persistence_failpoint
                                    .check("slash-thinking")
                                    .and_then(|()| {
                                        store
                                            .append_all_and_set_profile_preferences(
                                                &events,
                                                &session_model,
                                                level,
                                            )
                                            .map(|(_, preferences)| preferences)
                                    });
                                match persistence {
                                    Ok(preferences) => {
                                        Some(extension_preferences(preferences))
                                    }
                                    Err(error) => {
                                        if let Err(rollback_error) = runner
                                            .set_thinking_level(&internal_id, session_thinking)
                                            .await
                                        {
                                            tracing::error!(
                                                %rollback_error,
                                                %internal_id,
                                                "could not roll back runner after slash thinking persistence failed"
                                            );
                                        }
                                        if let Err(rollback_error) = prompt_runtime
                                            .dispatch(AppCommand::ChangeThinkingLevel {
                                                session_id: internal_id.clone(),
                                                level: session_thinking,
                                            })
                                            .await
                                        {
                                            tracing::error!(
                                                %rollback_error,
                                                %internal_id,
                                                "could not roll back Runtime after slash thinking persistence failed"
                                            );
                                        }
                                        let mut registry = prompt_sessions.lock().await;
                                        if work_matches(&registry, &internal_id, &work)
                                            && let Some(session) = registry.get_mut(&session_id)
                                        {
                                            session.thinking = session_thinking;
                                        }
                                        return Err(agent_error(error));
                                    }
                                }
                            } else {
                                None
                            };
                            if let Some((model, models)) = current {
                                send_config_update(
                                    &connection,
                                    &session_id,
                                    &model,
                                    &models,
                                    level,
                                )?;
                                if let Some(preferences) = stored_preferences {
                                    send_extension_activity(
                                        &connection,
                                        &prompt_extension,
                                        None,
                                        ActivityEvent::ProfilePreferencesChanged { preferences },
                                    )?;
                                }
                            }
                            Ok(protocol::StopReason::EndTurn)
                        }
                        SlashCommand::Login => {
                            let login = await_session_future(
                                &request_cancellation,
                                &work,
                                prompt_auth.start_native_login(None),
                            )
                            .await?
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
                            let completion =
                                login.complete(work.cancellation.clone());
                            tokio::pin!(completion);
                            let completion_result = tokio::select! {
                                biased;
                                result = &mut completion => result,
                                () = request_cancellation.cancelled() => {
                                    work.cancellation.cancel();
                                    // NativeLogin may already be committing
                                    // the credential. Keep polling it so a
                                    // successful commit is always announced.
                                    completion.await
                                }
                                () = work.cancellation.cancelled() => {
                                    // session/cancel only requests cancellation;
                                    // NativeLogin decides whether the commit
                                    // point has already been crossed.
                                    completion.await
                                }
                            };
                            let account = completion_result.map_err(agent_error)?;
                            let _account_transition = prompt_account_transition.lock().await;
                            begin_account_switch(
                                &prompt_sessions,
                                &prompt_account_work,
                                Some(&work),
                                account_guard.as_ref(),
                            )
                            .await
                            .map_err(agent_error)?;
                            publish_account_switch_started(
                                &connection,
                                &prompt_extension,
                                &prompt_auth,
                            )
                            .await?;
                            commit_account_store_switch(
                                &prompt_sessions,
                                &prompt_account_work,
                                prompt_store.as_ref(),
                                Some(&account.account.id),
                            )
                            .await
                            .map_err(agent_error)?;
                            // The credential is now persisted. Finish account
                            // validation and publish it without observing later
                            // request/session cancellation.
                            let validation = validation_status_if_current(
                                &prompt_auth,
                                prompt_auth.validate_with_revision().await,
                            );
                            if !matches!(&validation.status, ValidationStatus::Valid(_))
                            {
                                begin_account_switch(
                                    &prompt_sessions,
                                    &prompt_account_work,
                                    Some(&work),
                                    account_guard.as_ref(),
                                )
                                .await
                                .map_err(agent_error)?;
                                commit_account_store_switch(
                                    &prompt_sessions,
                                    &prompt_account_work,
                                    prompt_store.as_ref(),
                                    None,
                                )
                                .await
                                .map_err(agent_error)?;
                            }
                            let status = extension_account_status(validation);
                            send_extension_activity(
                                &connection,
                                &prompt_extension,
                                None,
                                ActivityEvent::AccountChanged {
                                    status: status.clone(),
                                },
                            )?;
                            Ok(protocol::StopReason::EndTurn)
                        }
                        SlashCommand::Logout => {
                            let _account_transition = prompt_account_transition.lock().await;
                            begin_account_switch(
                                &prompt_sessions,
                                &prompt_account_work,
                                Some(&work),
                                account_guard.as_ref(),
                            )
                            .await
                            .map_err(agent_error)?;
                            publish_account_switch_started(
                                &connection,
                                &prompt_extension,
                                &prompt_auth,
                            )
                            .await?;
                            commit_account_store_switch(
                                &prompt_sessions,
                                &prompt_account_work,
                                prompt_store.as_ref(),
                                None,
                            )
                            .await
                            .map_err(agent_error)?;
                            let logout = prompt_auth
                                .logout_with_cancellation(&work.cancellation);
                            tokio::pin!(logout);
                            let logout_result = tokio::select! {
                                biased;
                                result = &mut logout => result,
                                () = request_cancellation.cancelled() => {
                                    work.cancellation.cancel();
                                    logout.await
                                }
                                () = work.cancellation.cancelled() => logout.await,
                            };
                            let revision = logout_result.map_err(agent_error)?;
                            send_extension_activity(
                                &connection,
                                &prompt_extension,
                                None,
                                ActivityEvent::AccountChanged {
                                    status: extension_account_status(VersionedValidationStatus {
                                        status: ValidationStatus::Missing,
                                        revision,
                                    }),
                                },
                            )?;
                            Ok(protocol::StopReason::EndTurn)
                        }
                        SlashCommand::Account => {
                            let validation = await_session_future(
                                &request_cancellation,
                                &work,
                                prompt_auth.validate(),
                            )
                            .await?;
                            let text = match validation {
                                ValidationStatus::Missing => {
                                    "Not signed in. Run /login to connect AxiomCLI.".to_owned()
                                }
                                ValidationStatus::Valid(account) => format!(
                                    "Axiom account session is valid: {} (stored in {}).",
                                    account.account.display_name.as_deref().unwrap_or(&account.account.id),
                                    account.source.label()
                                ),
                                ValidationStatus::Expired => {
                                    "The saved Axiom account session expired or was revoked."
                                        .to_owned()
                                }
                                ValidationStatus::Unavailable(message) => message,
                            };
                            send_text(&connection, &session_id, text)?;
                            Ok(protocol::StopReason::EndTurn)
                        }
                        SlashCommand::Security => {
                            let verification = await_session_future(
                                &request_cancellation,
                                &work,
                                runner.verify_security(
                                    &session_model,
                                    work.cancellation.clone(),
                                ),
                            )
                            .await?
                            .map_err(agent_error)?;
                            strict_extension_security_verification(verification.clone())
                                .map_err(agent_error)?;
                            send_text(
                                &connection,
                                &session_id,
                                security_summary(&verification),
                            )?;
                            Ok(protocol::StopReason::EndTurn)
                        }
                        SlashCommand::Refresh => {
                            {
                                let mut registry = prompt_sessions.lock().await;
                                if !work_matches(&registry, &internal_id, &work) {
                                    return Err(agent_client_protocol::Error::request_cancelled());
                                }
                                let session = registry.get_mut(&session_id).ok_or_else(|| {
                                    agent_client_protocol::util::internal_error(
                                        "unknown ACP session",
                                    )
                                })?;
                                if session.model != session_model {
                                    return Err(agent_client_protocol::Error::request_cancelled());
                                }
                                session.security = crate::app::SecurityStatus::Verifying;
                            }
                            if let Err(error) = send_extension_activity(
                                &connection,
                                &prompt_extension,
                                Some(&session_id),
                                ActivityEvent::SecurityChanged {
                                    status: extension_security_status(
                                        crate::app::SecurityStatus::Verifying,
                                    ),
                                },
                            ) {
                                set_session_security_if_work(
                                    &prompt_sessions,
                                    &session_id,
                                    &internal_id,
                                    &work,
                                    &session_model,
                                    crate::app::SecurityStatus::Unverified,
                                )
                                .await;
                                return Err(error);
                            }
                            let verification = match await_session_future(
                                &request_cancellation,
                                &work,
                                runner.verify_security(
                                    &session_model,
                                    work.cancellation.clone(),
                                ),
                            )
                            .await
                            {
                                Ok(Ok(verification)) => verification,
                                Err(error) => {
                                    if set_session_security_if_work(
                                        &prompt_sessions,
                                        &session_id,
                                        &internal_id,
                                        &work,
                                        &session_model,
                                        crate::app::SecurityStatus::Unverified,
                                    )
                                    .await
                                    {
                                        send_extension_activity(
                                            &connection,
                                            &prompt_extension,
                                            Some(&session_id),
                                            ActivityEvent::SecurityChanged {
                                                status: extension_security_status(
                                                    crate::app::SecurityStatus::Unverified,
                                                ),
                                            },
                                        )?;
                                    }
                                    return Err(error);
                                }
                                Ok(Err(crate::AxiomError::Cancelled)) => {
                                    work.cancellation.cancel();
                                    if set_session_security_if_work(
                                        &prompt_sessions,
                                        &session_id,
                                        &internal_id,
                                        &work,
                                        &session_model,
                                        crate::app::SecurityStatus::Unverified,
                                    )
                                    .await
                                    {
                                        send_extension_activity(
                                            &connection,
                                            &prompt_extension,
                                            Some(&session_id),
                                            ActivityEvent::SecurityChanged {
                                                status: extension_security_status(
                                                    crate::app::SecurityStatus::Unverified,
                                                ),
                                            },
                                        )?;
                                    }
                                    return Err(
                                        agent_client_protocol::Error::request_cancelled(),
                                    );
                                }
                                Ok(Err(error)) => {
                                    if set_session_security_if_work(
                                        &prompt_sessions,
                                        &session_id,
                                        &internal_id,
                                        &work,
                                        &session_model,
                                        crate::app::SecurityStatus::Failed,
                                    )
                                    .await
                                    {
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
                                    }
                                    return Err(agent_error(error));
                                }
                            };
                            let verification_status =
                                match strict_extension_security_verification(verification.clone()) {
                                    Ok(result) => result,
                                    Err(error) => {
                                        if set_session_security_if_work(
                                            &prompt_sessions,
                                            &session_id,
                                            &internal_id,
                                            &work,
                                            &session_model,
                                            crate::app::SecurityStatus::Failed,
                                        )
                                        .await
                                        {
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
                                        }
                                        return Err(agent_error(error));
                                    }
                                }
                                .0;
                            if !set_session_security_if_work(
                                &prompt_sessions,
                                &session_id,
                                &internal_id,
                                &work,
                                &session_model,
                                verification_status,
                            )
                            .await
                            {
                                return Err(agent_client_protocol::Error::request_cancelled());
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
                            Ok(protocol::StopReason::EndTurn)
                        }
                        SlashCommand::Compact { focus } => {
                            let task_runtime = prompt_runtime.clone();
                            let task_runner = runner.clone();
                            let task_connection = connection.clone();
                            let task_store = prompt_store.clone();
                            let task_extension = prompt_extension.clone();
                                let (events_tx, mut events_rx) =
                                    mpsc::channel(APP_EVENT_QUEUE_CAPACITY);
                                let task_result: agent_client_protocol::Result<_> = async {
                                    let compact = task_runner.compact(
                                        &internal_id,
                                        focus,
                                        events_tx,
                                        work.cancellation.clone(),
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
                                            () = request_cancellation.cancelled() => {
                                                work.cancellation.cancel();
                                                break Err(crate::AxiomError::Cancelled);
                                            }
                                            () = work.cancellation.cancelled() => {
                                                break Err(crate::AxiomError::Cancelled);
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
                                    match result {
                                        Ok(result) => {
                                            let messages_before = result.messages_before;
                                            let events = task_runtime
                                                .dispatch(AppCommand::RecordContextCompaction {
                                                    session_id: internal_id.clone(),
                                                    summary: result.summary,
                                                    messages_before,
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
                                                    "Context compacted from {messages_before} conversation message(s)."
                                                ),
                                            )?;
                                            Ok(protocol::StopReason::EndTurn)
                                        }
                                        Err(crate::AxiomError::Cancelled) => {
                                            Ok(protocol::StopReason::Cancelled)
                                        }
                                        Err(error) => {
                                            send_text(
                                                &task_connection,
                                                &session_id,
                                                format!("AxiomCLI compaction error: {error}"),
                                            )?;
                                            Ok(protocol::StopReason::EndTurn)
                                        }
                                    }
                                }
                                .await;
                            task_result
                        }
                    }
                    }
                    .await;
                    release_session_work(&prompt_sessions, &internal_id, &work).await;
                    return responder.respond_with_result(
                        slash_result.map(protocol::PromptResponse::new),
                    );
                }
                let turn_id = turn_id.expect("non-slash prompt reserves a turn ID");

                let revision_history = if let Some(revision) = &message_revision {
                    let prepared = (|| {
                        if work.cancellation.is_cancelled() || request_cancellation.is_cancelled() {
                            return Err(crate::AxiomError::Cancelled);
                        }
                        let store = prompt_store.as_ref().ok_or_else(|| crate::AxiomError::Storage("durable thread storage is unavailable".into()))?;
                        store.history_before_user_message(&internal_id, revision)
                    })();
                    match prepared {
                        Ok(history) => Some(history),
                        Err(error) => {
                            release_session_work(&prompt_sessions, &internal_id, &work).await;
                            return responder.respond_with_error(agent_error(error));
                        }
                    }
                } else { None };

                let submitted = match prompt_runtime
                    .dispatch(AppCommand::SubmitPrompt {
                        session_id: internal_id.clone(),
                        turn_id: turn_id.clone(),
                        text: prompt.clone(),
                    })
                    .await
                {
                    Ok(submitted) => submitted,
                    Err(error) => {
                        if should_generate_title {
                            let mut registry = prompt_sessions.lock().await;
                            if work_matches(&registry, &internal_id, &work)
                                && let Some(session) = registry.get_mut(&session_id)
                            {
                                session.has_prompt = false;
                            }
                        }
                        release_session_work(&prompt_sessions, &internal_id, &work).await;
                        return responder.respond_with_error(agent_error(error));
                    }
                };
                let preparation: agent_client_protocol::Result<_> = async {
                    let submitted_revision = if let Some(store) = &prompt_store {
                        let revision = store
                            .append_prompt_revision(
                                &submitted,
                                client_item_id.as_deref(),
                                message_revision.as_ref(),
                            )
                            .map_err(agent_error)?;
                        if let Some(history) = &revision_history {
                            runner.restore_session(&internal_id, history).await.map_err(agent_error)?;
                        }
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
                    Ok(submitted_revision)
                }
                .await;
                let _submitted_revision = match preparation {
                    Ok(revision) => revision,
                    Err(error) => {
                        fail_active_prompt(
                            &prompt_sessions,
                            &prompt_runtime,
                            prompt_store.as_ref(),
                            &internal_id,
                            &work,
                            &error.to_string(),
                        )
                        .await;
                        return responder.respond_with_error(error);
                    }
                };

                if should_generate_title
                    && let Some(title_store) = prompt_store.clone()
                {
                    let title_cancellation = CancellationToken::new();
                    let title_guard = prompt_account_work
                        .register(title_cancellation.clone())
                        .ok();
                    let title_runner = runner.clone();
                    let title_session = internal_id.clone();
                    let title_prompt = prompt.clone();
                    let title_connection = connection.clone();
                    let title_external_id = session_id.clone();
                    if let Some(title_guard) = title_guard {
                    tokio::spawn(async move {
                        let _account_guard = title_guard;
                        match title_runner
                            .generate_title(
                                &title_session,
                                &title_prompt,
                                title_cancellation.clone(),
                                Some(&title_store),
                            )
                            .await
                        {
                            Ok(_) if title_cancellation.is_cancelled() => {}
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
                }

                let task_sessions = prompt_sessions.clone();
                let task_runtime = prompt_runtime.clone();
                let task_runner = runner.clone();
                let task_connection = connection.clone();
                let task_store = prompt_store.clone();
                let task_elicitation = supports_elicitation.clone();
                let task_extension = prompt_extension.clone();
                    let (events_tx, mut events_rx) = mpsc::channel(APP_EVENT_QUEUE_CAPACITY);
                    let display_cwd = cwd.clone();
                    let mut terminalized = false;
                    let task_result: agent_client_protocol::Result<_> = async {
                        let run = task_runner.run(
                            crate::agent::TurnContext {
                                session_id: internal_id.clone(),
                                turn_id: turn_id.clone(),
                                cwd,
                                permission_profile: session_profile,
                                web_enabled,
                                steering: work.steering.clone(),
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
                            work.cancellation.clone(),
                        );
                        tokio::pin!(run);

                        let result = loop {
                            tokio::select! {
                                result = &mut run => break result,
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
                                    if let AppEvent::ThinkingLevelChanged { level } = &event {
                                        if let Some(session) = task_sessions.lock().await.sessions.get_mut(&session_id) { session.thinking = *level; }
                                    }
                                    if let AppEvent::SteeringApplied { client_item_id, .. } = &event {
                                        if let Some(inbox) = &work.steering { inbox.acknowledge(client_item_id); }
                                    }
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
                                    work.cancellation.cancel();
                                    break Err(crate::AxiomError::Cancelled);
                                }
                                () = work.cancellation.cancelled() => {
                                    break Err(crate::AxiomError::Cancelled);
                                }
                            }
                        };
                        if result.is_err() {
                            work.cancellation.cancel();
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
                            if let AppEvent::ThinkingLevelChanged { level } = &event {
                                if let Some(session) = task_sessions.lock().await.sessions.get_mut(&session_id) { session.thinking = *level; }
                            }
                            if let AppEvent::SteeringApplied { client_item_id, .. } = &event {
                                if let Some(inbox) = &work.steering { inbox.acknowledge(client_item_id); }
                            }
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

                        let (command, stop_reason, display_error) = match result {
                            Ok(()) => (
                                AppCommand::FinishTurn {
                                    session_id: internal_id.clone(),
                                    turn_id: turn_id.clone(),
                                },
                                protocol::StopReason::EndTurn,
                                None,
                            ),
                            Err(crate::AxiomError::Cancelled) => (
                                AppCommand::CancelTurn {
                                    session_id: internal_id.clone(),
                                    turn_id: turn_id.clone(),
                                },
                                protocol::StopReason::Cancelled,
                                None,
                            ),
                            Err(error) => {
                                let message = error.to_string();
                                (
                                    AppCommand::FailTurn {
                                        session_id: internal_id.clone(),
                                        turn_id: turn_id.clone(),
                                        message: message.clone(),
                                    },
                                    protocol::StopReason::EndTurn,
                                    Some(message),
                                )
                            }
                        };
                        let completed = task_runtime
                            .dispatch(command)
                            .await
                            .map_err(agent_error)?;
                        terminalized = true;
                        if let Some(store) = &task_store {
                            store.append_all(&completed).map_err(agent_error)?;
                        }
                        if let Some(error) = display_error {
                            send_text(
                                &task_connection,
                                &session_id,
                                format!("AxiomCLI error: {error}"),
                            )?;
                        }
                        Ok(stop_reason)
                    }
                    .await;

                    if let Err(error) = &task_result
                        && !terminalized
                    {
                        fail_active_prompt(
                            &task_sessions,
                            &task_runtime,
                            task_store.as_ref(),
                            &internal_id,
                            &work,
                            &error.to_string(),
                        )
                        .await;
                    } else {
                        release_session_work(&task_sessions, &internal_id, &work).await;
                    }
                    responder.respond_with_result(
                        task_result.map(protocol::PromptResponse::new),
                    )
                });
                if let Err(error) = spawn_result {
                    if rollback_has_prompt {
                        let mut registry = rollback_sessions.lock().await;
                        if work_matches(
                            &registry,
                            &rollback_internal_id,
                            &rollback_work,
                        ) && let Some(session) = registry.get_mut(&rollback_session_id)
                        {
                            session.has_prompt = false;
                        }
                    }
                    release_session_work(
                        &rollback_sessions,
                        &rollback_internal_id,
                        &rollback_work,
                    )
                    .await;
                    return Err(error);
                }
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
                let account_store = respond_or_return!(
                    responder,
                    list_account_work
                        .register_store(store)
                        .map_err(agent_error)
                );
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
                let page = respond_or_return!(
                    responder,
                    account_store
                        .read(|store| store.thread_catalog(
                            query,
                            request.include_archived,
                            request.cursor.as_deref(),
                            limit,
                        ))
                        .map_err(agent_error)
                );
                account_store.publish(|| {
                    responder.respond(ListThreadsResponse {
                        threads: page
                            .threads
                            .into_iter()
                            .map(extension_thread_summary)
                            .collect(),
                        next_cursor: page.next_cursor,
                    })
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: extension::RenameThreadRequest,
                        responder: Responder<extension::RenameThreadResponse>,
                        _connection| {
                if !extension_enabled(&rename_thread_extension, ExtensionFeature::ThreadCatalog) {
                    return responder.respond_with_error(extension_not_negotiated());
                }
                let Some(store) = &rename_thread_store else {
                    return responder.respond_with_internal_error("durable thread storage is unavailable");
                };
                let account_store = respond_or_return!(
                    responder,
                    rename_thread_account_work.register_store(store).map_err(agent_error)
                );
                let thread_id = respond_or_return!(
                    responder,
                    SessionId::from_str(&request.thread_id).map_err(|_| {
                        agent_client_protocol::util::internal_error("invalid thread ID")
                    })
                );
                let thread = respond_or_return!(
                    responder,
                    account_store.mutate(|store| {
                        store.rename(&thread_id, &request.title)?;
                        store.thread_summary(&thread_id)
                    }).map_err(agent_error)
                );
                account_store.publish(|| responder.respond(extension::RenameThreadResponse {
                    thread: extension_thread_summary(thread),
                }))
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
                let account_store = respond_or_return!(
                    responder,
                    timeline_account_work
                        .register_store(store)
                        .map_err(agent_error)
                );
                let thread_id = respond_or_return!(
                    responder,
                    SessionId::from_str(&request.thread_id).map_err(|_| {
                        agent_client_protocol::util::internal_error("invalid thread ID")
                    })
                );
                let limit = usize::try_from(
                    request.limit.unwrap_or(250).clamp(1, extension::MAX_TIMELINE_PAGE_SIZE),
                )
                .unwrap_or(250);
                let mut snapshot = respond_or_return!(
                    responder,
                    account_store
                        .read(|store| {
                            store.thread_snapshot(&thread_id, request.after_sequence, limit)
                        })
                        .map_err(agent_error)
                );
                let ids: Vec<String> = snapshot.request_usage.iter().filter(|record| !record.settled && record.state != axiom_inference::InvocationState::Running)
                    .take(100).map(|record| record.request_id.clone()).collect();
                if !ids.is_empty() {
                    if let Ok(Ok(records)) = tokio::time::timeout(std::time::Duration::from_secs(2), timeline_runner.recover_accounting(&ids, CancellationToken::new())).await {
                        let _ = account_store.read(|store| store.reconcile_request_usage(&thread_id, &records));
                        snapshot = respond_or_return!(responder, account_store.read(|store| store.thread_snapshot(&thread_id, request.after_sequence, limit)).map_err(agent_error));
                    }
                }
                account_store.publish(|| {
                    responder.respond(GetThreadTimelineResponse {
                        desktop_agent: if frontend == FrontendKind::DesktopChat {
                            Some(account_store.read(|store| store.desktop_agent_settings(&thread_id)).map_err(agent_error)?)
                        } else { None },
                        active_turn_id: snapshot.turns.iter()
                            .find(|turn| turn.status == crate::session::TurnStatus::Running)
                            .map(|turn| turn.id.clone()),
                        thread: extension_thread_summary(snapshot.thread),
                        context_usage: snapshot.context_usage,
                        request_usage: snapshot.request_usage,
                        items: snapshot
                            .items
                            .into_iter()
                            .map(extension_timeline_item)
                            .collect(),
                        next_cursor: snapshot.next_cursor,
                    })
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
                let account_store = respond_or_return!(
                    responder,
                    preview_account_work
                        .register_store(store)
                        .map_err(agent_error)
                );
                let ids = respond_or_return!(
                    responder,
                    validate_session_ids(&request.thread_ids).map_err(agent_error)
                );
                {
                    let registry = preview_extension_sessions.lock().await;
                    if ids
                        .iter()
                        .any(|id| registry.active_work.contains_key(id))
                    {
                        return responder.respond_with_internal_error(
                            "sessions with active work must be cancelled before deletion",
                        );
                    }
                }
                let summaries = respond_or_return!(
                    responder,
                    account_store
                        .read(|store| {
                            ids.iter()
                                .map(|id| {
                                    store.thread_summary(id).map(extension_thread_summary)
                                })
                                .collect::<crate::Result<Vec<_>>>()
                        })
                        .map_err(agent_error)
                );
                let token = uuid::Uuid::new_v4().to_string();
                let expires_at_unix_seconds = now_unix_seconds().saturating_add(60);
                let mut pending = preview_extension.pending_deletes.lock().await;
                account_store.publish(|| {
                    pending.retain(|_, grant| {
                        grant.expires_at_unix_seconds > now_unix_seconds()
                    });
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
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: DeleteConfirmRequest,
                        responder: Responder<DeleteConfirmResponse>,
                        connection: ConnectionTo<Client>| {
                if !extension_enabled(&delete_extension, ExtensionFeature::ThreadCatalog) {
                    return responder.respond_with_error(extension_not_negotiated());
                }
                let Some(store) = delete_extension_store.clone() else {
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
                let delete_work = SessionWork::new(SessionWorkKind::Deletion);
                {
                    let mut registry = delete_extension_sessions.lock().await;
                    if requested
                        .iter()
                        .any(|id| registry.active_work.contains_key(id))
                    {
                        return responder.respond_with_internal_error(
                            "sessions with active work must be cancelled before deletion",
                        );
                    }
                    for id in &requested {
                        registry.active_work.insert(id.clone(), delete_work.clone());
                    }
                }
                let request_cancellation = responder.cancellation();
                let task_sessions = delete_extension_sessions.clone();
                let rollback_sessions = delete_extension_sessions.clone();
                let rollback_requested = requested.clone();
                let rollback_work = delete_work.clone();
                let account_store = match delete_account_work.register_store_with_cancellation(
                    &store,
                    delete_work.cancellation.clone(),
                )
                {
                    Ok(account_store) => account_store,
                    Err(error) => {
                        release_session_works(
                            &delete_extension_sessions,
                            &requested,
                            &delete_work,
                        )
                        .await;
                        return responder.respond_with_error(agent_error(error));
                    }
                };
                let account_store = Arc::new(account_store);
                let spawn_result = connection.spawn(async move {
                    let task_result: agent_client_protocol::Result<_> = async {
                        if request_cancellation.is_cancelled()
                            || delete_work.cancellation.is_cancelled()
                        {
                            return Err(agent_client_protocol::Error::request_cancelled());
                        }
                        let deletion_account_store = account_store.clone();
                        let deletion_ids = requested.clone();
                        let deleted = tokio::task::spawn_blocking(move || {
                            deletion_account_store
                                .mutate(|store| store.delete_sessions(&deletion_ids))
                        })
                        .await
                        .map_err(|error| {
                            agent_client_protocol::util::internal_error(format!(
                                "session deletion worker failed: {error}"
                            ))
                        })?
                        .map_err(agent_error)?;
                        task_sessions
                            .lock()
                            .await
                            .retain(|_, session| !requested.contains(&session.internal_id));
                        Ok(DeleteConfirmResponse {
                            deleted: u32::try_from(deleted).unwrap_or(u32::MAX),
                        })
                    }
                    .await;
                    let response_result = match task_result {
                        Ok(response) => account_store.publish(|| responder.respond(response)),
                        Err(error) => responder.respond_with_error(error),
                    };
                    release_session_works(&task_sessions, &requested, &delete_work).await;
                    response_result
                });
                if let Err(error) = spawn_result {
                    release_session_works(
                        &rollback_sessions,
                        &rollback_requested,
                        &rollback_work,
                    )
                    .await;
                    return Err(error);
                }
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |_request: ListModelsRequest,
                        responder: Responder<ListModelsResponse>,
                        connection: ConnectionTo<Client>| {
                let models_extension = models_extension.clone();
                let models_extension_runner = models_extension_runner.clone();
                connection.spawn(async move {
                if !extension_enabled(&models_extension, ExtensionFeature::ModelCatalog) {
                    return responder.respond_with_error(extension_not_negotiated());
                }
                    let request_cancellation = responder.cancellation();
                    let discovery_cancellation = CancellationToken::new();
                    let discovery = models_extension_runner
                        .available_model_details(discovery_cancellation.clone());
                    tokio::pin!(discovery);
                    let models = tokio::select! {
                        result = &mut discovery => respond_or_return!(responder, result.map_err(agent_error)),
                        () = request_cancellation.cancelled() => {
                            discovery_cancellation.cancel();
                            return responder.respond_with_error(
                                agent_client_protocol::Error::request_cancelled(),
                            );
                        }
                    }
                    .into_iter()
                    .map(|model| {
                        if model.id.trim().is_empty()
                            || model.label.trim().is_empty()
                            || model.short_label.trim().is_empty()
                            || model.provider_id.trim().is_empty()
                            || model.provider_label.trim().is_empty()
                            || model.upstream_model.trim().is_empty()
                            || model.context_window_tokens == 0
                            || model.max_output_tokens == 0
                            || model.max_output_tokens > model.context_window_tokens
                            || !matches!(
                                (
                                    model.input_price_microusd_per_million_tokens,
                                    model.output_price_microusd_per_million_tokens,
                                ),
                                (None, None) | (Some(1..), Some(1..))
                            )
                        {
                            return Err(agent_client_protocol::util::internal_error(
                                "provider catalog returned incomplete model metadata",
                            ));
                        }
                        let thinking_levels = supported_thinking_levels(&model).into_iter().map(|level| level.to_string()).collect();
                        Ok(ModelInfo {
                            auto_compact_threshold_tokens: models_extension_runner
                                .auto_compact_threshold_tokens(&model),
                            label: model.label,
                            short_label: model.short_label,
                            provider_id: model.provider_id,
                            provider_label: model.provider_label,
                            upstream_model: model.upstream_model,
                            id: model.id,
                            context_window_tokens: model.context_window_tokens,
                            max_output_tokens: model.max_output_tokens,
                            input_price_microusd_per_million_tokens: model
                                .input_price_microusd_per_million_tokens,
                            output_price_microusd_per_million_tokens: model
                                .output_price_microusd_per_million_tokens,
                            thinking_levels,
                        })
                    })
                    .collect::<agent_client_protocol::Result<Vec<_>>>();
                let models = respond_or_return!(responder, models);
                responder.respond(ListModelsResponse { models })
                })?;
                Ok(())
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
                let account_store = respond_or_return!(
                    responder,
                    stored_preferences_account_work
                        .register_store(store)
                        .map_err(agent_error)
                );
                let preferences = extension_preferences(respond_or_return!(
                    responder,
                    account_store
                        .read(SessionStore::profile_preferences)
                        .map_err(agent_error)
                ));
                account_store.publish(|| {
                    responder.respond(ProfilePreferencesResponse { preferences })
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
                let Some(store) = update_preferences_store.clone() else {
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
                let task_extension = update_preferences_extension.clone();
                let task_runner = update_preferences_runner.clone();
                let task_lock = update_preferences_lock.clone();
                let task_connection = connection.clone();
                let request_cancellation = responder.cancellation();
                let account_cancellation = CancellationToken::new();
                let account_store = respond_or_return!(
                    responder,
                    update_preferences_account_work
                        .register_store_with_cancellation(&store, account_cancellation.clone())
                        .map_err(agent_error)
                );
                connection.spawn(async move {
                let task_result: agent_client_protocol::Result<_> = async {
                if request_cancellation.is_cancelled() {
                    return Err(agent_client_protocol::Error::request_cancelled());
                }
                let lock = task_lock.lock();
                tokio::pin!(lock);
                let _guard = tokio::select! {
                    guard = &mut lock => guard,
                    () = request_cancellation.cancelled() => {
                        return Err(agent_client_protocol::Error::request_cancelled());
                    }
                    () = account_cancellation.cancelled() => {
                        return Err(agent_client_protocol::Error::request_cancelled());
                    }
                };
                let current = account_store
                    .read(SessionStore::profile_preferences)
                    .map_err(agent_error)?;
                let Some(next_model) = request.model.or(current.model) else {
                    return Err(agent_client_protocol::util::internal_error(
                        "a model must be selected before saving profile preferences",
                    ));
                };
                let next_thinking = request
                        .thinking_level
                        .as_deref()
                        .map(crate::app::ThinkingLevel::from_str)
                        .transpose()
                        .map_err(agent_error)?
                    .unwrap_or(current.thinking_level);
                let cancellation = account_cancellation;
                let models_future = task_runner.available_model_details(cancellation.clone());
                tokio::pin!(models_future);
                let models = tokio::select! {
                    result = &mut models_future => result.map_err(agent_error)?,
                    () = request_cancellation.cancelled() => {
                        cancellation.cancel();
                        return Err(agent_client_protocol::Error::request_cancelled());
                    }
                    () = cancellation.cancelled() => {
                        return Err(agent_client_protocol::Error::request_cancelled());
                    }
                };
                let Some(model) = models.iter().find(|model| model.id == next_model) else {
                    return Err(agent_client_protocol::util::internal_error(
                        "model is not in the provider catalog",
                    ));
                };
                let next_thinking = reconcile_model_settings(model, next_thinking).thinking;
                if request_cancellation.is_cancelled() || cancellation.is_cancelled() {
                    return Err(agent_client_protocol::Error::request_cancelled());
                }
                let preferences = extension_preferences(
                    account_store
                        .mutate(|store| {
                            store.set_profile_preferences(&next_model, next_thinking)
                        })
                        .map_err(agent_error)?,
                );
                Ok(preferences)
                }
                .await;
                match task_result {
                    Ok(preferences) => account_store.publish(|| {
                        send_extension_activity(
                            &task_connection,
                            &task_extension,
                            None,
                            ActivityEvent::ProfilePreferencesChanged {
                                preferences: preferences.clone(),
                            },
                        )?;
                        responder.respond(ProfilePreferencesResponse { preferences })
                    }),
                    Err(error) => responder.respond_with_error(error),
                }
                })?;
                Ok(())
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
                let account_store = respond_or_return!(
                    responder,
                    collections_list_account_work
                        .register_store(store)
                        .map_err(agent_error)
                );
                let state = extension_collections(respond_or_return!(
                    responder,
                    account_store
                        .read(SessionStore::list_collections)
                        .map_err(agent_error)
                ));
                account_store.publish(|| responder.respond(CollectionStateResponse { state }))
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
                let account_store = respond_or_return!(
                    responder,
                    collection_create_account_work
                        .register_store(store)
                        .map_err(agent_error)
                );
                let state = extension_collections(
                    respond_or_return!(
                        responder,
                        account_store
                            .mutate(|store| store.create_collection(&request.name))
                            .map_err(agent_error)
                    )
                    .state,
                );
                account_store.publish(|| {
                    send_extension_activity(&connection, &collection_create_extension, None, ActivityEvent::CollectionsChanged { state: state.clone() })?;
                    responder.respond(CollectionStateResponse { state })
                })
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
                let account_store = respond_or_return!(
                    responder,
                    collection_rename_account_work
                        .register_store(store)
                        .map_err(agent_error)
                );
                let state = extension_collections(
                    respond_or_return!(
                        responder,
                        account_store
                            .mutate(|store| {
                                store.rename_collection(&request.collection_id, &request.name)
                            })
                            .map_err(agent_error)
                    )
                    .state,
                );
                account_store.publish(|| {
                    send_extension_activity(&connection, &collection_rename_extension, None, ActivityEvent::CollectionsChanged { state: state.clone() })?;
                    responder.respond(CollectionStateResponse { state })
                })
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
                let account_store = respond_or_return!(
                    responder,
                    collection_collapsed_account_work
                        .register_store(store)
                        .map_err(agent_error)
                );
                let state = extension_collections(
                    respond_or_return!(
                        responder,
                        account_store
                            .mutate(|store| {
                                store.set_collection_collapsed(
                                    &request.collection_id,
                                    request.collapsed,
                                )
                            })
                            .map_err(agent_error)
                    )
                    .state,
                );
                account_store.publish(|| {
                    send_extension_activity(&connection, &collection_collapsed_extension, None, ActivityEvent::CollectionsChanged { state: state.clone() })?;
                    responder.respond(CollectionStateResponse { state })
                })
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
                let account_store = respond_or_return!(
                    responder,
                    collection_move_account_work
                        .register_store(store)
                        .map_err(agent_error)
                );
                let state = extension_collections(
                    respond_or_return!(
                        responder,
                        account_store
                            .mutate(|store| {
                                store.move_collection(&request.collection_id, request.position)
                            })
                            .map_err(agent_error)
                    )
                    .state,
                );
                account_store.publish(|| {
                    send_extension_activity(&connection, &collection_move_extension, None, ActivityEvent::CollectionsChanged { state: state.clone() })?;
                    responder.respond(CollectionStateResponse { state })
                })
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
                let account_store = respond_or_return!(
                    responder,
                    collection_delete_account_work
                        .register_store(store)
                        .map_err(agent_error)
                );
                let state = extension_collections(
                    respond_or_return!(
                        responder,
                        account_store
                            .mutate(|store| store.delete_collection(&request.collection_id))
                            .map_err(agent_error)
                    )
                    .state,
                );
                account_store.publish(|| {
                    send_extension_activity(&connection, &collection_delete_extension, None, ActivityEvent::CollectionsChanged { state: state.clone() })?;
                    responder.respond(CollectionStateResponse { state })
                })
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
                let thread_id = respond_or_return!(
                    responder,
                    SessionId::from_str(&request.thread_id).map_err(|_| {
                        agent_client_protocol::util::internal_error("invalid thread ID")
                    })
                );
                let account_store = respond_or_return!(
                    responder,
                    collection_assign_account_work
                        .register_store(store)
                        .map_err(agent_error)
                );
                let (change, revision) = respond_or_return!(
                    responder,
                    account_store
                        .mutate(|store| {
                            let change = store.assign_thread_collection(
                                &thread_id,
                                request.collection_id.as_deref(),
                            )?;
                            let revision = store.thread_revision(&thread_id)?;
                            Ok((change, revision))
                        })
                        .map_err(agent_error)
                );
                let state = extension_collections(change.state);
                account_store.publish(|| {
                    send_extension_activity(&connection, &collection_assign_extension, None, ActivityEvent::CollectionsChanged { state: state.clone() })?;
                    responder.respond(extension::AssignThreadCollectionResponse {
                        state,
                        thread_revision: revision.revision,
                        last_timeline_sequence: revision.last_timeline_sequence,
                    })
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |_request: extension::AccountStatusRequest,
                        responder: Responder<extension::AccountStatusResponse>,
                        connection: ConnectionTo<Client>| {
                let account_status_extension = account_status_extension.clone();
                let account_status_auth = account_status_auth.clone();
                let account_status_store = account_status_store.clone();
                let account_status_sessions = account_status_sessions.clone();
                let account_status_work = account_status_work.clone();
                let account_status_transition = account_status_transition.clone();
                let task_connection = connection.clone();
                connection.spawn(async move {
                let connection = task_connection;
                if !extension_enabled(&account_status_extension, ExtensionFeature::Account) {
                    return responder.respond_with_error(extension_not_negotiated());
                }
                let _account_transition = account_status_transition.lock().await;
                let request_cancellation = responder.cancellation();
                let validation = respond_or_return!(
                    responder,
                    await_request_future(
                        &request_cancellation,
                        account_status_auth.validate_with_revision(),
                    )
                    .await
                );
                let mut validation = validation_status_if_current(
                    &account_status_auth,
                    validation,
                );
                let switch_target = match (&validation.status, &account_status_store) {
                    (ValidationStatus::Valid(account), Some(store))
                        if store.active_account_id().as_deref()
                            != Some(account.account.id.as_str()) =>
                    {
                        Some(Some(account.account.id.clone()))
                    }
                    (ValidationStatus::Missing | ValidationStatus::Expired, Some(store))
                        if store.is_active() =>
                    {
                        Some(None)
                    }
                    _ => None,
                };
                if let Some(target) = &switch_target {
                    respond_or_return!(
                        responder,
                        begin_account_switch(
                            &account_status_sessions,
                            &account_status_work,
                            None,
                            None,
                        )
                        .await
                        .map_err(agent_error)
                    );
                    publish_account_switch_started(
                        &connection,
                        &account_status_extension,
                        &account_status_auth,
                    )
                    .await?;
                    respond_or_return!(
                        responder,
                        commit_account_store_switch(
                            &account_status_sessions,
                            &account_status_work,
                            account_status_store.as_ref(),
                            target.as_deref(),
                        )
                        .await
                        .map_err(agent_error)
                    );
                    validation = validation_status_if_current(
                        &account_status_auth,
                        account_status_auth.validate_with_revision().await,
                    );
                }
                let status = extension_account_status(validation);
                if switch_target.is_some() {
                    send_extension_activity(
                        &connection,
                        &account_status_extension,
                        None,
                        ActivityEvent::AccountChanged {
                            status: status.clone(),
                        },
                    )?;
                }
                responder.respond(extension::AccountStatusResponse { status })
                })?;
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: NativeLoginStartRequest,
                        responder: Responder<NativeLoginStartResponse>,
                        connection: ConnectionTo<Client>| {
                let login_start_extension = login_start_extension.clone();
                let login_start_auth = login_start_auth.clone();
                connection.spawn(async move {
                if !extension_enabled(&login_start_extension, ExtensionFeature::Account) {
                    return responder.respond_with_error(extension_not_negotiated());
                }
                // One desktop process owns at most one pending browser
                // authorization. Superseded capabilities are cancelled at the
                // backend before another approval page is created.
                let superseded = {
                    let mut logins = login_start_extension.native_logins.lock().await;
                    logins.drain().map(|(_, login)| login).collect::<Vec<_>>()
                };
                for superseded_login in superseded {
                    let _ = superseded_login.cancel().await;
                }
                let request_cancellation = responder.cancellation();
                let login = respond_or_return!(
                    responder,
                    await_request_future(
                        &request_cancellation,
                        login_start_auth.start_native_login(request.method_hint.map(auth_login_method)),
                    )
                    .await
                    .and_then(|result| result.map_err(agent_error))
                );
                if request_cancellation.is_cancelled() {
                    let _ = login.cancel().await;
                    return responder.respond_with_error(
                        agent_client_protocol::Error::request_cancelled(),
                    );
                }
                let login_id = uuid::Uuid::new_v4().to_string();
                let state = NativeLoginState {
                    login_id: login_id.clone(),
                    user_code: login.user_code().into(),
                    authorization_url: login.authorization_url().into(),
                    browser_opened: login.browser_opened(),
                    expires_at: login.expires_at().into(),
                };
                login_start_extension
                    .native_logins
                    .lock()
                    .await
                    .insert(login_id, login);
                responder.respond(NativeLoginStartResponse { login: state })
                })?;
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: NativeLoginCompleteRequest,
                        responder: Responder<extension::AccountStatusResponse>,
                        connection: ConnectionTo<Client>| {
                let login_complete_extension = login_complete_extension.clone();
                let login_complete_auth = login_complete_auth.clone();
                let login_complete_store = login_complete_store.clone();
                let login_complete_sessions = login_complete_sessions.clone();
                let login_complete_work = login_complete_work.clone();
                let login_complete_transition = login_complete_transition.clone();
                let task_connection = connection.clone();
                connection.spawn(async move {
                let connection = task_connection;
                if !extension_enabled(&login_complete_extension, ExtensionFeature::Account) {
                    return responder.respond_with_error(extension_not_negotiated());
                }
                if request.login_id.is_empty()
                    || request.login_id.len() > extension::MAX_IDENTIFIER_BYTES
                {
                    return responder.respond_with_internal_error("native login ID is invalid");
                }
                let request_cancellation = responder.cancellation();
                if request_cancellation.is_cancelled() {
                    return responder.respond_with_error(
                        agent_client_protocol::Error::request_cancelled(),
                    );
                }
                let login = respond_or_return!(
                    responder,
                    login_complete_extension
                        .native_logins
                        .lock()
                        .await
                        .remove(&request.login_id)
                        .ok_or_else(|| {
                            agent_client_protocol::util::internal_error(
                                "native login is missing, expired, or already completed",
                            )
                        })
                );
                let cancellation = CancellationToken::new();
                let completion = login.complete(cancellation.clone());
                tokio::pin!(completion);
                let completion_result = tokio::select! {
                    biased;
                    account = &mut completion => account,
                    () = request_cancellation.cancelled() => {
                        cancellation.cancel();
                        // NativeLogin owns the persistence commit boundary. Do
                        // not drop it after cancellation: it may already be in
                        // the uninterruptible credential-save phase.
                        completion.await
                    }
                };
                let account = respond_or_return!(responder, completion_result.map_err(agent_error));
                let _account_transition = login_complete_transition.lock().await;
                respond_or_return!(
                    responder,
                    begin_account_switch(
                        &login_complete_sessions,
                        &login_complete_work,
                        None,
                        None,
                    )
                    .await
                    .map_err(agent_error)
                );
                publish_account_switch_started(
                    &connection,
                    &login_complete_extension,
                    &login_complete_auth,
                )
                .await?;
                respond_or_return!(
                    responder,
                    commit_account_store_switch(
                        &login_complete_sessions,
                        &login_complete_work,
                        login_complete_store.as_ref(),
                        Some(&account.account.id),
                    )
                    .await
                    .map_err(agent_error)
                );
                // login.complete persisted the credential. This is the commit
                // point: finish validation and publish the resulting account
                // state even if request cancellation arrives afterward.
                // Revalidate through the manager so a server-side revocation racing
                // completion still fails closed before the next prompt.
                let committed_account_id = account.account.id.clone();
                let mut validation = validation_status_if_current(
                    &login_complete_auth,
                    login_complete_auth.validate_with_revision().await,
                );
                if matches!(
                    &validation.status,
                    ValidationStatus::Valid(validated)
                        if validated.account.id != committed_account_id
                ) {
                    validation.status = ValidationStatus::Unavailable(
                        "Axiom returned a different account while validating the completed native session"
                            .into(),
                    );
                }
                if !matches!(&validation.status, ValidationStatus::Valid(_)) {
                    respond_or_return!(
                        responder,
                        begin_account_switch(
                            &login_complete_sessions,
                            &login_complete_work,
                            None,
                            None,
                        )
                        .await
                        .map_err(agent_error)
                    );
                    respond_or_return!(
                        responder,
                        commit_account_store_switch(
                            &login_complete_sessions,
                            &login_complete_work,
                            login_complete_store.as_ref(),
                            None,
                        )
                        .await
                        .map_err(agent_error)
                    );
                }
                let status = extension_account_status(validation);
                send_extension_activity(
                    &connection,
                    &login_complete_extension,
                    None,
                    ActivityEvent::AccountChanged {
                        status: status.clone(),
                    },
                )?;
                responder.respond(extension::AccountStatusResponse { status })
                })?;
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: NativeLoginCancelRequest,
                        responder: Responder<extension::AccountStatusResponse>,
                        connection: ConnectionTo<Client>| {
                if !extension_enabled(&login_cancel_extension, ExtensionFeature::Account) {
                    return responder.respond_with_error(extension_not_negotiated());
                }
                if request.login_id.is_empty()
                    || request.login_id.len() > extension::MAX_IDENTIFIER_BYTES
                {
                    return responder.respond_with_internal_error("native login ID is invalid");
                }
                let login_cancel_extension = login_cancel_extension.clone();
                let login_cancel_auth = login_cancel_auth.clone();
                connection.spawn(async move {
                let login = login_cancel_extension
                    .native_logins
                    .lock()
                    .await
                    .remove(&request.login_id);
                if let Some(login) = login {
                    respond_or_return!(responder, login.cancel().await.map_err(agent_error));
                }
                let validation = login_cancel_auth.validate_with_revision().await;
                let status = extension_account_status(validation_status_if_current(
                    &login_cancel_auth,
                    validation,
                ));
                responder.respond(extension::AccountStatusResponse { status })
                })?;
                Ok(())
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
                let task_extension = logout_extension.clone();
                let task_auth = logout_auth.clone();
                let task_store = logout_store.clone();
                let task_sessions = logout_sessions.clone();
                let task_work = logout_work.clone();
                let task_transition = logout_transition.clone();
                let task_connection = connection.clone();
                let request_cancellation = responder.cancellation();
                connection.spawn(async move {
                let task_result: agent_client_protocol::Result<_> = async {
                let _account_transition = task_transition.lock().await;
                begin_account_switch(&task_sessions, &task_work, None, None)
                    .await
                    .map_err(agent_error)?;
                publish_account_switch_started(
                    &task_connection,
                    &task_extension,
                    &task_auth,
                )
                .await?;
                commit_account_store_switch(
                    &task_sessions,
                    &task_work,
                    task_store.as_ref(),
                    None,
                )
                .await
                .map_err(agent_error)?;
                let cancellation = CancellationToken::new();
                let logout = task_auth.logout_with_cancellation(&cancellation);
                tokio::pin!(logout);
                let logout_result = tokio::select! {
                    biased;
                    result = &mut logout => result,
                    () = request_cancellation.cancelled() => {
                        cancellation.cancel();
                        logout.await
                    }
                };
                let revision = logout_result.map_err(agent_error)?;
                // Credential removal is committed; always publish the new state.
                let status = extension_account_status(VersionedValidationStatus {
                    status: ValidationStatus::Missing,
                    revision,
                });
                send_extension_activity(
                    &task_connection,
                    &task_extension,
                    None,
                    ActivityEvent::AccountChanged {
                        status: status.clone(),
                    },
                )?;
                Ok(extension::AccountStatusResponse { status })
                }
                .await;
                responder.respond_with_result(task_result)
                })?;
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )

        .on_receive_request(
            async move |_request: extension::ApiKeyListRequest, responder: Responder<extension::ApiKeyListResponse>, connection: ConnectionTo<Client>| {
                if !extension_enabled(&keys_list_extension, ExtensionFeature::Account) {
                    return responder.respond_with_error(extension_not_negotiated());
                }
                let client = keys_list_client.clone();
                let cancellation = CancellationToken::new();
                let guard = respond_or_return!(responder, keys_list_work.register(cancellation.clone()).map_err(agent_error));
                let request_cancellation = responder.cancellation();
                connection.spawn(async move {
                    let _guard = guard;
                    let operation = client.api_keys(&cancellation);
                    tokio::pin!(operation);
                    let result = tokio::select! {
                        biased;
                        result = &mut operation => result,
                        () = request_cancellation.cancelled() => { cancellation.cancel(); operation.await }
                    };
                    responder.respond_with_result(result.map_err(agent_error))
                })?;
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: extension::ApiKeyCreateRequest, responder: Responder<extension::ApiKeyCreatedResponse>, connection: ConnectionTo<Client>| {
                if !extension_enabled(&keys_create_extension, ExtensionFeature::Account) {
                    return responder.respond_with_error(extension_not_negotiated());
                }
                let client = keys_create_client.clone();
                let cancellation = CancellationToken::new();
                let guard = respond_or_return!(responder, keys_create_work.register(cancellation.clone()).map_err(agent_error));
                let request_cancellation = responder.cancellation();
                connection.spawn(async move {
                    let _guard = guard;
                    let operation = client.create_api_key(&request.name, &cancellation);
                    tokio::pin!(operation);
                    let result = tokio::select! {
                        biased;
                        result = &mut operation => result,
                        () = request_cancellation.cancelled() => { cancellation.cancel(); operation.await }
                    };
                    responder.respond_with_result(result.map_err(agent_error))
                })?;
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: extension::ApiKeyRevokeRequest, responder: Responder<extension::ApiKeyRevokeResponse>, connection: ConnectionTo<Client>| {
                if !extension_enabled(&keys_revoke_extension, ExtensionFeature::Account) {
                    return responder.respond_with_error(extension_not_negotiated());
                }
                let client = keys_revoke_client.clone();
                let cancellation = CancellationToken::new();
                let guard = respond_or_return!(responder, keys_revoke_work.register(cancellation.clone()).map_err(agent_error));
                let request_cancellation = responder.cancellation();
                connection.spawn(async move {
                    let _guard = guard;
                    let operation = client.revoke_api_key(&request.id, &cancellation);
                    tokio::pin!(operation);
                    let result = tokio::select! {
                        biased;
                        result = &mut operation => result,
                        () = request_cancellation.cancelled() => { cancellation.cancel(); operation.await }
                    };
                    responder.respond_with_result(result.map_err(agent_error))
                })?;
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |_request: extension::UsageSummaryRequest,
                        responder: Responder<extension::UsageSummaryResponse>,
                        connection: ConnectionTo<Client>| {
                if !extension_enabled(&usage_extension, ExtensionFeature::Usage) {
                    return responder.respond_with_error(extension_not_negotiated());
                }
                let client = usage_client.clone();
                let cancellation = CancellationToken::new();
                let guard = respond_or_return!(responder,
                    usage_work.register(cancellation.clone()).map_err(agent_error));
                let request_cancellation = responder.cancellation();
                connection.spawn(async move {
                    let _guard = guard;
                    let summary = client.usage_summary(&cancellation);
                    tokio::pin!(summary);
                    let result = tokio::select! {
                        biased;
                        result = &mut summary => result,
                        () = request_cancellation.cancelled() => {
                            cancellation.cancel();
                            summary.await
                        }
                    };
                    let summary = respond_or_return!(responder, result.map_err(agent_error));
                    responder.respond(extension::UsageSummaryResponse { summary })
                })?;
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |_request: BillingStatusRequest,
                        responder: Responder<BillingStatusResponse>,
                        connection: ConnectionTo<Client>| {
                if !extension_enabled(&billing_status_extension, ExtensionFeature::Billing) {
                    return responder.respond_with_error(extension_not_negotiated());
                }
                let task_extension = billing_status_extension.clone();
                let task_client = billing_status_client.clone();
                let task_connection = connection.clone();
                let cancellation = CancellationToken::new();
                let account_guard = respond_or_return!(
                    responder,
                    billing_status_work
                        .register(cancellation.clone())
                        .map_err(agent_error)
                );
                let request_cancellation = responder.cancellation();
                connection.spawn(async move {
                    let _account_guard = account_guard;
                    let status = task_client.status(&cancellation);
                    tokio::pin!(status);
                    let result = tokio::select! {
                        biased;
                        result = &mut status => result,
                        () = request_cancellation.cancelled() => {
                            cancellation.cancel();
                            status.await
                        }
                    };
                    let status = respond_or_return!(responder, result.map_err(agent_error));
                    let status = extension_billing_status(
                        status,
                        task_extension.next_billing_revision(),
                    );
                    send_extension_activity(
                        &task_connection,
                        &task_extension,
                        None,
                        ActivityEvent::BillingChanged {
                            status: status.clone(),
                        },
                    )?;
                    responder.respond(BillingStatusResponse { status })
                })?;
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: BillingInvoiceCreateRequest,
                        responder: Responder<BillingInvoiceResponse>,
                        connection: ConnectionTo<Client>| {
                if !extension_enabled(&billing_create_extension, ExtensionFeature::Billing) {
                    return responder.respond_with_error(extension_not_negotiated());
                }
                if request.amount_usd_cents == 0
                    || request.amount_usd_cents > extension::MAX_INVOICE_USD_CENTS
                    || request.client_request_id.is_empty()
                    || request.client_request_id.len() > extension::MAX_CLIENT_REQUEST_ID_BYTES
                    || !request.client_request_id.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric() || b"_.:-".contains(&byte)
                    })
                {
                    return responder.respond_with_internal_error(
                        "invoice creation request is invalid",
                    );
                }
                let task_extension = billing_create_extension.clone();
                let task_client = billing_create_client.clone();
                let task_connection = connection.clone();
                let cancellation = CancellationToken::new();
                let account_guard = respond_or_return!(
                    responder,
                    billing_create_work
                        .register(cancellation.clone())
                        .map_err(agent_error)
                );
                let request_cancellation = responder.cancellation();
                connection.spawn(async move {
                    let _account_guard = account_guard;
                    let invoice = task_client.create_invoice(
                        request.amount_usd_cents,
                        &request.client_request_id,
                        &cancellation,
                    );
                    tokio::pin!(invoice);
                    let result = tokio::select! {
                        biased;
                        result = &mut invoice => result,
                        () = request_cancellation.cancelled() => {
                            cancellation.cancel();
                            invoice.await
                        }
                    };
                    let invoice = respond_or_return!(responder, result.map_err(agent_error));
                    let invoice = extension_invoice(
                        invoice,
                        task_extension.next_billing_revision(),
                    );
                    send_extension_activity(
                        &task_connection,
                        &task_extension,
                        None,
                        ActivityEvent::InvoiceChanged {
                            invoice: invoice.clone(),
                        },
                    )?;
                    responder.respond(BillingInvoiceResponse { invoice })
                })?;
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: BillingInvoiceStatusRequest,
                        responder: Responder<BillingInvoiceResponse>,
                        connection: ConnectionTo<Client>| {
                if !extension_enabled(&billing_invoice_extension, ExtensionFeature::Billing) {
                    return responder.respond_with_error(extension_not_negotiated());
                }
                if request.invoice_id.len() != 32
                    || !request.invoice_id.bytes().all(|byte| {
                        byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
                    })
                {
                    return responder.respond_with_internal_error("invoice ID is invalid");
                }
                let task_extension = billing_invoice_extension.clone();
                let task_client = billing_invoice_client.clone();
                let task_connection = connection.clone();
                let cancellation = CancellationToken::new();
                let account_guard = respond_or_return!(
                    responder,
                    billing_invoice_work
                        .register(cancellation.clone())
                        .map_err(agent_error)
                );
                let request_cancellation = responder.cancellation();
                connection.spawn(async move {
                    let _account_guard = account_guard;
                    let invoice = task_client.invoice_status(&request.invoice_id, &cancellation);
                    tokio::pin!(invoice);
                    let result = tokio::select! {
                        biased;
                        result = &mut invoice => result,
                        () = request_cancellation.cancelled() => {
                            cancellation.cancel();
                            invoice.await
                        }
                    };
                    let invoice = respond_or_return!(responder, result.map_err(agent_error));
                    let invoice = extension_invoice(
                        invoice,
                        task_extension.next_billing_revision(),
                    );
                    send_extension_activity(
                        &task_connection,
                        &task_extension,
                        None,
                        ActivityEvent::InvoiceChanged {
                            invoice: invoice.clone(),
                        },
                    )?;
                    responder.respond(BillingInvoiceResponse { invoice })
                })?;
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: VerifySecurityRequest,
                        responder: Responder<VerifySecurityResponse>,
                        connection: ConnectionTo<Client>| {
                let security_extension = security_extension.clone();
                let security_extension_sessions = security_extension_sessions.clone();
                let security_extension_runner = security_extension_runner.clone();
                if !extension_enabled(&security_extension, ExtensionFeature::SecurityEvidence) {
                    return responder.respond_with_error(extension_not_negotiated());
                }
                let requested_internal_id = respond_or_return!(
                    responder,
                    SessionId::from_str(&request.thread_id).map_err(|_| {
                        agent_client_protocol::util::internal_error("invalid thread ID")
                    })
                );
                let work = SessionWork::new(SessionWorkKind::SecurityVerification);
                let (external_id, internal_id, model) = {
                    let mut registry = security_extension_sessions.lock().await;
                    let Some((external_id, session)) = registry
                        .iter()
                        .find(|(_, session)| session.internal_id == requested_internal_id)
                    else {
                        return responder.respond_with_internal_error("unknown ACP session");
                    };
                    let external_id = external_id.clone();
                    let internal_id = session.internal_id.clone();
                    let model = session.model.clone();
                    if registry.active_work.contains_key(&internal_id) {
                        return responder
                            .respond_with_internal_error("session already has active work");
                    }
                    registry
                        .active_work
                        .insert(internal_id.clone(), work.clone());
                    registry
                        .get_mut(&external_id)
                        .expect("session remained loaded while reserving security verification")
                        .security = crate::app::SecurityStatus::Verifying;
                    (external_id, internal_id, model)
                };
                let request_cancellation = responder.cancellation();
                let task_connection = connection.clone();
                let rollback_sessions = security_extension_sessions.clone();
                let rollback_external_id = external_id.clone();
                let rollback_internal_id = internal_id.clone();
                let rollback_model = model.clone();
                let rollback_work = work.clone();
                let account_guard = match security_account_work
                    .register(work.cancellation.clone())
                {
                    Ok(guard) => guard,
                    Err(error) => {
                        set_session_security_if_work(
                            &security_extension_sessions,
                            &external_id,
                            &internal_id,
                            &work,
                            &model,
                            crate::app::SecurityStatus::Unverified,
                        )
                        .await;
                        release_session_work(&security_extension_sessions, &internal_id, &work)
                            .await;
                        return responder.respond_with_error(agent_error(error));
                    }
                };
                let spawn_result = connection.spawn(async move {
                let _account_guard = account_guard;
                let connection = task_connection;
                let task_result: agent_client_protocol::Result<_> = async {
                    send_extension_activity(
                    &connection,
                    &security_extension,
                    Some(&external_id),
                    ActivityEvent::SecurityChanged {
                        status: extension_security_status(crate::app::SecurityStatus::Verifying),
                    },
                )?;
                    let verification = await_session_future(
                        &request_cancellation,
                        &work,
                        security_extension_runner
                            .verify_security(&model, work.cancellation.clone()),
                    )
                    .await?;
                    let verification = match verification {
                        Ok(verification) => verification,
                        Err(crate::AxiomError::Cancelled) => {
                            work.cancellation.cancel();
                            return Err(agent_client_protocol::Error::request_cancelled());
                        }
                        Err(error) => return Err(agent_error(error)),
                    };
                    let (verification_status, evidence) =
                        strict_extension_security_verification(verification)
                            .map_err(agent_error)?;
                    if !set_session_security_if_work(
                        &security_extension_sessions,
                        &external_id,
                        &internal_id,
                        &work,
                        &model,
                        verification_status,
                    )
                    .await
                    {
                        return Err(agent_client_protocol::Error::request_cancelled());
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
                    Ok(VerifySecurityResponse { evidence, status })
                }
                .await;
                if let Err(error) = &task_result {
                    let verification_status = if work.cancellation.is_cancelled() || error.code == (-32001).into() {
                        crate::app::SecurityStatus::Unverified
                    } else {
                        crate::app::SecurityStatus::Failed
                    };
                    if set_session_security_if_work(
                        &security_extension_sessions,
                        &external_id,
                        &internal_id,
                        &work,
                        &model,
                        verification_status,
                    )
                    .await
                    {
                        let status = if verification_status
                            == crate::app::SecurityStatus::Failed
                        {
                            ExtensionSecurityStatus {
                                state: ExtensionSecurityState::Failed,
                                detail: Some(error.to_string()),
                            }
                        } else {
                            extension_security_status(verification_status)
                        };
                        let _ = send_extension_activity(
                            &connection,
                            &security_extension,
                            Some(&external_id),
                            ActivityEvent::SecurityChanged { status },
                        );
                    }
                }
                release_session_work(&security_extension_sessions, &internal_id, &work).await;
                responder.respond_with_result(task_result)
                });
                if let Err(error) = spawn_result {
                    set_session_security_if_work(
                        &rollback_sessions,
                        &rollback_external_id,
                        &rollback_internal_id,
                        &rollback_work,
                        &rollback_model,
                        crate::app::SecurityStatus::Unverified,
                    )
                    .await;
                    release_session_work(
                        &rollback_sessions,
                        &rollback_internal_id,
                        &rollback_work,
                    )
                    .await;
                    return Err(error);
                }
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: extension::SteerTurnRequest,
                        responder: Responder<extension::SteerTurnResponse>, connection: ConnectionTo<Client>| {
                if !extension_enabled(&steer_extension, ExtensionFeature::Steering) {
                    return responder.respond_with_error(extension_not_negotiated());
                }
                let turn = respond_or_return!(responder, TurnId::from_str(&request.expected_turn_id).map_err(|_| agent_client_protocol::util::internal_error("invalid expected turn ID")));
                let registry = steer_sessions.lock().await;
                let external = protocol::SessionId::new(request.thread_id.clone());
                let Some(session) = registry.get(&external) else { return responder.respond_with_internal_error("unknown ACP session"); };
                let Some(work) = registry.active_work.get(&session.internal_id) else { return responder.respond_with_internal_error("turn is no longer active"); };
                if work.web_enabled != Some(request.web_enabled) || work.cancellation.is_cancelled()
                    || work.agent_revision != request.agent_revision.unwrap_or(0) {
                    return responder.respond_with_internal_error("steering cannot change the active turn's Web access or Agent settings, or resume cancellation");
                }
                let Some(inbox) = &work.steering else { return responder.respond_with_internal_error("current work does not accept steering"); };
                let receiver = respond_or_return!(responder, inbox.submit(&turn, request.client_item_id.clone(), request.text).map_err(agent_error));
                let cancellation = work.cancellation.clone();
                drop(registry);
                connection.spawn(async move {
                    tokio::select! {
                        biased;
                        result = receiver => match result {
                            Ok(()) => responder.respond(extension::SteerTurnResponse { turn_id: request.expected_turn_id, client_item_id: request.client_item_id }),
                            Err(_) => responder.respond_with_internal_error("steering was not applied; message remains queued"),
                        },
                        () = cancellation.cancelled() => responder.respond_with_internal_error("turn stopped before steering was confirmed; check the saved transcript before retrying"),
                    }
                })?;
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: CompactRequest,
                        responder: Responder<CompactResponse>,
                        connection: ConnectionTo<Client>| {
                let compact_extension = compact_extension.clone();
                let compact_extension_sessions = compact_extension_sessions.clone();
                let compact_extension_runner = compact_extension_runner.clone();
                let compact_extension_runtime = compact_extension_runtime.clone();
                let compact_extension_store = compact_extension_store.clone();
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
                let requested_internal_id = respond_or_return!(
                    responder,
                    SessionId::from_str(&request.thread_id).map_err(|_| {
                        agent_client_protocol::util::internal_error("invalid thread ID")
                    })
                );
                let work = SessionWork::new(SessionWorkKind::Compaction);
                let (external_id, internal_id) = {
                    let mut registry = compact_extension_sessions.lock().await;
                    let Some((external_id, session)) = registry
                        .iter()
                        .find(|(_, session)| session.internal_id == requested_internal_id)
                    else {
                        return responder.respond_with_internal_error("unknown ACP session");
                    };
                    let external_id = external_id.clone();
                    let internal_id = session.internal_id.clone();
                    if registry.active_work.contains_key(&internal_id) {
                        return responder
                            .respond_with_internal_error("session already has active work");
                    }
                    registry
                        .active_work
                        .insert(internal_id.clone(), work.clone());
                    (external_id, internal_id)
                };
                let request_cancellation = responder.cancellation();
                let task_connection = connection.clone();
                let rollback_sessions = compact_extension_sessions.clone();
                let rollback_internal_id = internal_id.clone();
                let rollback_work = work.clone();
                let account_guard = match compact_account_work
                    .register(work.cancellation.clone())
                {
                    Ok(guard) => guard,
                    Err(error) => {
                        release_session_work(&compact_extension_sessions, &internal_id, &work)
                            .await;
                        return responder.respond_with_error(agent_error(error));
                    }
                };
                let spawn_result = connection.spawn(async move {
                let _account_guard = account_guard;
                let connection = task_connection;
                let task_result: agent_client_protocol::Result<_> = async {
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
                        work.cancellation.clone(),
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
                                work.cancellation.cancel();
                                break Err(crate::AxiomError::Cancelled);
                            }
                            () = work.cancellation.cancelled() => {
                                break Err(crate::AxiomError::Cancelled);
                            }
                        }
                    };
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
                            Ok(CompactResponse {
                                messages_before: u64::try_from(messages_before)
                                    .unwrap_or(u64::MAX),
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
                }
                .await;
                release_session_work(&compact_extension_sessions, &internal_id, &work).await;
                responder.respond_with_result(task_result)
                });
                if let Err(error) = spawn_result {
                    release_session_work(
                        &rollback_sessions,
                        &rollback_internal_id,
                        &rollback_work,
                    )
                    .await;
                    return Err(error);
                }
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: extension::ConfigureDesktopAgentRequest,
                        responder: Responder<extension::ConfigureDesktopAgentResponse>, connection: ConnectionTo<Client>| {
                if frontend != FrontendKind::DesktopChat || !extension_enabled(&agent_settings_extension, ExtensionFeature::DesktopAgent) {
                    return responder.respond_with_error(extension_not_negotiated());
                }
                let Some(store) = &agent_settings_store else { return responder.respond_with_internal_error("account storage unavailable"); };
                let store = respond_or_return!(responder, store.bind_active_account().map_err(agent_error));
                let internal_id = respond_or_return!(responder, SessionId::from_str(&request.thread_id).map_err(|_| agent_client_protocol::util::internal_error("invalid thread ID")));
                let external_id = protocol::SessionId::new(request.thread_id.clone());
                let work = SessionWork::new(SessionWorkKind::Configuration);
                let account_guard = respond_or_return!(responder, agent_settings_account_work.register(work.cancellation.clone()).map_err(agent_error));
                {
                    let mut registry = agent_settings_sessions.lock().await;
                    if !registry.contains_key(&external_id) { return responder.respond_with_internal_error("open the thread before changing Agent settings"); }
                    if registry.active_work.contains_key(&internal_id) { return responder.respond_with_internal_error("wait for the current operation before changing Agent settings"); }
                    registry.active_work.insert(internal_id.clone(), work.clone());
                }
                let sessions = agent_settings_sessions.clone();
                let runtime = agent_settings_runtime.clone();
                let runner = agent_settings_runner.clone();
                let rollback_sessions = sessions.clone(); let rollback_id = internal_id.clone(); let rollback_work = work.clone();
                let request_cancellation = responder.cancellation();
                let spawn_result = connection.spawn(async move {
                    let _account_guard = account_guard;
                    let result: agent_client_protocol::Result<_> = async {
                        let settings = store.prepare_desktop_agent_settings(&internal_id, &request).map_err(agent_error)?;
                        let mut previous = store.desktop_agent_settings(&internal_id).map_err(agent_error)?;
                        runner.reset_session_permissions(&internal_id).await.map_err(agent_error)?;
                        let mut registry = sessions.lock().await;
                        if work.cancellation.is_cancelled() || request_cancellation.is_cancelled() || !work_matches(&registry, &internal_id, &work) {
                            return Err(agent_client_protocol::Error::request_cancelled());
                        }
                        let session = registry.get_mut(&external_id).ok_or_else(|| agent_client_protocol::util::internal_error("thread closed"))?;
                        previous.working_directory = session.cwd.to_string_lossy().into_owned();
                        let events = runtime.dispatch(AppCommand::ConfigureDesktopAgent { session_id: internal_id.clone(), settings: settings.clone() }).await.map_err(agent_error)?;
                        if let Err(error) = store.append_all(&events) {
                            runtime.dispatch(AppCommand::ConfigureDesktopAgent { session_id: internal_id.clone(), settings: previous }).await.map_err(agent_error)?;
                            return Err(agent_error(error));
                        }
                        session.cwd = PathBuf::from(&settings.working_directory);
                        session.profile = PermissionProfile::for_desktop_agent(&settings);
                        Ok(extension::ConfigureDesktopAgentResponse { thread: extension_thread_summary(store.thread_summary(&internal_id).map_err(agent_error)?), agent: settings })
                    }.await;
                    release_session_work(&sessions, &internal_id, &work).await;
                    responder.respond_with_result(result)
                });
                if let Err(error) = spawn_result {
                    release_session_work(&rollback_sessions, &rollback_id, &rollback_work).await;
                    return Err(error);
                }
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: protocol::SetSessionModeRequest,
                        responder: Responder<protocol::SetSessionModeResponse>,
                        connection: ConnectionTo<Client>| {
                let mode_sessions = mode_sessions.clone();
                let mode_runtime = mode_runtime.clone();
                let mode_store = mode_store.clone();
                let mode_persistence_failpoint = mode_persistence_failpoint.clone();
                let profile = respond_or_return!(
                    responder,
                    PermissionProfile::from_str(request.mode_id.0.as_ref()).map_err(agent_error)
                );
                if mode_frontend == FrontendKind::DesktopChat {
                    return responder.respond_with_error(extension_rpc_error(
                        -32003,
                        ExtensionErrorCode::PermissionDenied,
                        "desktop permissions must be changed through the Agent controls",
                        false,
                    ));
                }
                let work = SessionWork::new(SessionWorkKind::PermissionMode);
                let (internal_id, previous_profile) = {
                    let mut registry = mode_sessions.lock().await;
                    let Some(session) = registry.get(&request.session_id) else {
                        return responder.respond_with_internal_error("unknown ACP session");
                    };
                    let internal_id = session.internal_id.clone();
                    let previous_profile = session.profile;
                    if registry.active_work.contains_key(&internal_id) {
                        return responder.respond_with_internal_error(
                            "cannot change permissions while the session has active work",
                        );
                    }
                    registry
                        .active_work
                        .insert(internal_id.clone(), work.clone());
                    (internal_id, previous_profile)
                };
                let task_sessions = mode_sessions.clone();
                let rollback_sessions = mode_sessions.clone();
                let rollback_internal_id = internal_id.clone();
                let rollback_work = work.clone();
                let task_connection = connection.clone();
                let request_cancellation = responder.cancellation();
                let account_guard = match mode_account_work.register(work.cancellation.clone()) {
                    Ok(guard) => guard,
                    Err(error) => {
                        release_session_work(&mode_sessions, &internal_id, &work).await;
                        return responder.respond_with_error(agent_error(error));
                    }
                };
                let spawn_result = connection.spawn(async move {
                    let _account_guard = account_guard;
                    let task_result: agent_client_protocol::Result<_> = async {
                    let mut registry = task_sessions.lock().await;
                    if request_cancellation.is_cancelled()
                        || work.cancellation.is_cancelled()
                        || !work_matches(&registry, &internal_id, &work)
                    {
                        work.cancellation.cancel();
                        return Err(agent_client_protocol::Error::request_cancelled());
                    }
                    // Permission changes are local and short. Holding the registry
                    // lock makes this the atomic commit point: session/cancel can
                    // win before dispatch, but cannot split Runtime, storage, and
                    // the ACP session mirror after dispatch starts.
                    let events = mode_runtime
                        .dispatch(AppCommand::ChangePermissionProfile {
                            session_id: internal_id.clone(),
                            profile,
                        })
                        .await
                        .map_err(agent_error)?;
                    let Some(session) = registry.get_mut(&request.session_id) else {
                        return Err(agent_client_protocol::util::internal_error(
                            "unknown ACP session",
                        ));
                    };
                    session.profile = profile;
                    drop(registry);
                    if let Some(store) = &mode_store
                        && let Err(error) = mode_persistence_failpoint
                            .check("mode")
                            .and_then(|()| store.append_all(&events).map(|_| ()))
                    {
                        if let Err(rollback_error) = mode_runtime
                            .dispatch(AppCommand::ChangePermissionProfile {
                                session_id: internal_id.clone(),
                                profile: previous_profile,
                            })
                            .await
                        {
                            tracing::error!(
                                %rollback_error,
                                %internal_id,
                                "could not roll back Runtime after permission persistence failed"
                            );
                        }
                        let mut registry = task_sessions.lock().await;
                        if work_matches(&registry, &internal_id, &work)
                            && let Some(session) = registry.get_mut(&request.session_id)
                        {
                            session.profile = previous_profile;
                        }
                        return Err(agent_error(error));
                    }
                    task_connection.send_notification(protocol::SessionNotification::new(
                        request.session_id.clone(),
                        protocol::SessionUpdate::CurrentModeUpdate(
                            protocol::CurrentModeUpdate::new(profile.to_string()),
                        ),
                    ))?;
                    Ok(protocol::SetSessionModeResponse::new())
                    }
                    .await;
                    release_session_work(&task_sessions, &internal_id, &work).await;
                    responder.respond_with_result(task_result)
                });
                if let Err(error) = spawn_result {
                    release_session_work(
                        &rollback_sessions,
                        &rollback_internal_id,
                        &rollback_work,
                    )
                    .await;
                    return Err(error);
                }
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: protocol::SetSessionConfigOptionRequest,
                        responder: Responder<protocol::SetSessionConfigOptionResponse>,
                        connection: ConnectionTo<Client>| {
                let config_sessions = config_sessions.clone();
                let config_runner = config_runner.clone();
                let config_runtime = config_runtime.clone();
                let config_store = config_store.clone();
                let config_extension = config_extension.clone();
                let config_profile_settings_lock = config_profile_settings_lock.clone();
                let config_persistence_failpoint = config_persistence_failpoint.clone();
                let Some(value) = request.value.as_value_id() else {
                    return responder.respond_with_internal_error(
                        "session config requires a selected value ID",
                    );
                };
                let selected = value.0.to_string();
                if !matches!(
                    request.config_id.0.as_ref(),
                    MODEL_CONFIG_ID | THINKING_CONFIG_ID
                ) {
                    return responder.respond_with_internal_error("unknown session config option");
                }
                let work = SessionWork::new(SessionWorkKind::Configuration);
                let (internal_id, previous_session) = {
                    let mut registry = config_sessions.lock().await;
                    let Some(session) = registry.get(&request.session_id) else {
                        return responder.respond_with_internal_error("unknown ACP session");
                    };
                    let internal_id = session.internal_id.clone();
                    let previous_session = session.clone();
                    if registry.active_work.contains_key(&internal_id) {
                        return responder.respond_with_internal_error(
                            "cannot change settings while the session has active work",
                        );
                    }
                    registry
                        .active_work
                        .insert(internal_id.clone(), work.clone());
                    (internal_id, previous_session)
                };
                let model = previous_session.model.clone();
                let thinking = previous_session.thinking;
                let request_cancellation = responder.cancellation();
                let task_sessions = config_sessions.clone();
                let rollback_sessions = config_sessions.clone();
                let rollback_internal_id = internal_id.clone();
                let rollback_work = work.clone();
                let task_connection = connection.clone();
                let account_guard = match config_account_work.register(work.cancellation.clone()) {
                    Ok(guard) => guard,
                    Err(error) => {
                        release_session_work(&config_sessions, &internal_id, &work).await;
                        return responder.respond_with_error(agent_error(error));
                    }
                };
                let spawn_result = connection.spawn(async move {
                let _account_guard = account_guard;
                let task_result: agent_client_protocol::Result<_> = async {
                let _profile_settings_guard = await_session_future(
                    &request_cancellation,
                    &work,
                    config_profile_settings_lock.lock(),
                )
                .await?;
                let details = await_session_future(
                    &request_cancellation,
                    &work,
                    config_runner.available_model_details(work.cancellation.clone()),
                )
                .await?
                .map_err(agent_error)?;
                let mut models = details
                    .iter()
                    .map(|candidate| candidate.id.clone())
                    .collect::<Vec<_>>();
                models.sort();
                models.dedup();
                let Some(previous_model) = details.iter().find(|candidate| candidate.id == model)
                else {
                    return Err(agent_client_protocol::util::internal_error(
                        "current model is not in the provider catalog",
                    ));
                };
                let previous_settings = reconcile_model_settings(previous_model, thinking);
                let (settings, supported_thinking, events) = match request.config_id.0.as_ref() {
                    MODEL_CONFIG_ID => {
                        let Some(selected_model) =
                            details.iter().find(|candidate| candidate.id == selected)
                        else {
                            return Err(agent_client_protocol::util::internal_error(
                                "model is not in the provider catalog",
                            ));
                        };
                        let model_changed = selected_model.id != model;
                        let settings = reconcile_model_settings(selected_model, thinking);
                        await_session_future(
                            &request_cancellation,
                            &work,
                            config_runner
                                .set_model_settings(&internal_id, &settings),
                        )
                        .await?
                        .map_err(agent_error)?;
                        // A successful runner setter is the configuration
                        // commit point. Finish Runtime, durable storage, and the
                        // ACP mirror even if cancellation arrives afterward.
                        let events = match config_runtime
                            .dispatch(AppCommand::ChangeModelSettings {
                                    session_id: internal_id.clone(),
                                    model: settings.model.clone(),
                                    thinking: settings.thinking,
                                    reset_security: model_changed,
                                })
                            .await
                        {
                            Ok(events) => events,
                            Err(error) => {
                                if let Err(rollback_error) = config_runner
                                    .set_model_settings(&internal_id, &previous_settings)
                                    .await
                                {
                                    tracing::warn!(
                                        %rollback_error,
                                        %internal_id,
                                        "could not roll back runner model after Runtime rejected ACP config"
                                    );
                                }
                                return Err(agent_error(error));
                            }
                        };
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
                            return Err(agent_client_protocol::util::internal_error(
                                "current model is not in the provider catalog",
                            ));
                        };
                        let settings = reconcile_model_settings(selected_model, level);
                        let level = settings.thinking;
                        await_session_future(
                            &request_cancellation,
                            &work,
                            config_runner
                                .set_thinking_level(&internal_id, level),
                        )
                        .await?
                        .map_err(agent_error)?;
                        // Keep the same post-setter commit rule as model changes
                        // so runner and durable session settings stay aligned.
                        let events = match config_runtime
                            .dispatch(AppCommand::ChangeThinkingLevel {
                                    session_id: internal_id.clone(),
                                    level,
                                })
                            .await
                        {
                            Ok(events) => events,
                            Err(error) => {
                                if let Err(rollback_error) = config_runner
                                    .set_thinking_level(&internal_id, thinking)
                                    .await
                                {
                                    tracing::warn!(
                                        %rollback_error,
                                        %internal_id,
                                        "could not roll back runner thinking after Runtime rejected ACP config"
                                    );
                                }
                                return Err(agent_error(error));
                            }
                        };
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
                {
                    let mut registry = task_sessions.lock().await;
                    if !work_matches(&registry, &internal_id, &work) {
                        return Err(agent_client_protocol::Error::request_cancelled());
                    }
                    let Some(session) = registry.get_mut(&request.session_id) else {
                        return Err(agent_client_protocol::util::internal_error(
                            "unknown ACP session",
                        ));
                    };
                    apply_session_model_settings(
                        session,
                        &next_model,
                        next_thinking,
                        &models,
                        reset_security,
                    );
                }
                let updated_preferences = if let Some(store) = &config_store {
                    let persistence = config_persistence_failpoint
                        .check("config")
                        .and_then(|()| {
                            store.append_all_and_set_profile_preferences(
                                &events,
                                &next_model,
                                next_thinking,
                            )
                        });
                    match persistence {
                        Ok((_, stored)) => Some(extension_preferences(stored)),
                        Err(error) => {
                            if let Err(rollback_error) = config_runner
                                .set_model_settings(&internal_id, &previous_settings)
                                .await
                            {
                                tracing::error!(
                                    %rollback_error,
                                    %internal_id,
                                    "could not roll back runner after ACP config persistence failed"
                                );
                            }
                            if let Err(rollback_error) = config_runtime
                                .dispatch(AppCommand::ChangeModelSettings {
                                    session_id: internal_id.clone(),
                                    model: previous_session.model.clone(),
                                    thinking: previous_session.thinking,
                                    reset_security: false,
                                })
                                .await
                            {
                                tracing::error!(
                                    %rollback_error,
                                    %internal_id,
                                    "could not roll back Runtime after ACP config persistence failed"
                                );
                            }
                            if reset_security
                                && let Err(rollback_error) = config_runtime
                                    .emit(
                                        internal_id.clone(),
                                        AppEvent::SecurityStatusChanged {
                                            state: previous_session.security,
                                        },
                                    )
                                    .await
                            {
                                tracing::error!(
                                    %rollback_error,
                                    %internal_id,
                                    "could not restore Runtime security after ACP config persistence failed"
                                );
                            }
                            let mut registry = task_sessions.lock().await;
                            if work_matches(&registry, &internal_id, &work)
                                && let Some(session) = registry.get_mut(&request.session_id)
                            {
                                *session = previous_session.clone();
                            }
                            return Err(agent_error(error));
                        }
                    }
                } else {
                    None
                };
                if let Some(preferences) = updated_preferences {
                    send_extension_activity(
                        &task_connection,
                        &config_extension,
                        None,
                        ActivityEvent::ProfilePreferencesChanged { preferences },
                    )?;
                }
                if reset_security {
                    send_extension_activity(
                        &task_connection,
                        &config_extension,
                        Some(&request.session_id),
                        ActivityEvent::SecurityChanged {
                            status: extension_security_status(
                                crate::app::SecurityStatus::Unverified,
                            ),
                        },
                    )?;
                }
                Ok(protocol::SetSessionConfigOptionResponse::new(
                    session_config_options(
                        &next_model,
                        &models,
                        next_thinking,
                        Some(&supported_thinking),
                    ),
                ))
                }
                .await;
                release_session_work(&task_sessions, &internal_id, &work).await;
                responder.respond_with_result(task_result)
                });
                if let Err(error) = spawn_result {
                    release_session_work(
                        &rollback_sessions,
                        &rollback_internal_id,
                        &rollback_work,
                    )
                    .await;
                    return Err(error);
                }
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_notification(
            async move |notification: protocol::CancelNotification, _connection| {
                let token = {
                    let registry = cancel_sessions.lock().await;
                    registry
                        .get(&notification.session_id)
                        .and_then(|session| registry.active_work.get(&session.internal_id))
                        .map(|work| work.cancellation.clone())
                };
                if let Some(token) = token {
                    token.cancel();
                }
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        // Use Tokio-native stdin/stdout streams for the Tokio-hosted sidecar.
            .connect_to(ByteStreams::new(
                tokio::io::stdout().compat_write(),
                tokio::io::stdin().compat(),
            )),
    )
    .await;
    // ACP EOF is an explicit ownership boundary. Drain tool and MCP process
    // groups while Tokio is still alive; synchronous guard Drop remains the
    // fallback if the runtime or an individual request future is aborted.
    shutdown_runner.shutdown().await;
    connection_result
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
                    "inputTokens": 75000, "outputTokens": 1000, "modelId": "test",
                    "reportedAt": "2026-09-07T00:00:00Z", "contextWindowTokens": 100000,
                    "autoCompactThresholdTokens": 85000
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
            workload_identity_allowlist_configured: false,
            approved_workload_identity: false,
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
            workload_identity_allowlist_configured: false,
            approved_workload_identity: false,
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
            models: vec!["near/model-a".into(), "phala/model-b".into()],
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
