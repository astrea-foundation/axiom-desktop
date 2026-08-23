use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
    str::FromStr as _,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use agent_client_protocol::schema::{ProtocolVersion, v1 as protocol};
use agent_client_protocol::{Agent, Client, ConnectionTo, Responder, Stdio};
use async_trait::async_trait;
use tokio::sync::{Mutex, mpsc};
use tokio_util::sync::CancellationToken;

use crate::{
    agent::{APP_EVENT_QUEUE_CAPACITY, QuestionHandler, TurnRunner},
    app::{
        AppCommand, AppEvent, Origin, PermissionProfile, QuestionRequest, Runtime, SessionId,
        TurnId,
    },
    config::Config,
    policy::{ApprovalChoice, ApprovalHandler, ApprovalRequest, ApprovalResponse},
    session::{SessionStore, model_for_resume, permission_for_resume, thinking_for_resume},
    slash::{self, SlashCommand},
};

fn send_available_commands(
    connection: &ConnectionTo<Client>,
    session_id: &protocol::SessionId,
) -> agent_client_protocol::Result<()> {
    let commands = slash::COMMANDS
        .iter()
        .map(|spec| {
            let command = protocol::AvailableCommand::new(spec.name, spec.description);
            if matches!(spec.name, "help" | "model" | "resume" | "delete") {
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

fn slash_help_text() -> String {
    let lines = slash::COMMANDS
        .iter()
        .map(|spec| format!("{} — {}", spec.usage, spec.description))
        .collect::<Vec<_>>()
        .join("\n");
    format!("AxiomCLI commands:\n{lines}\nCommands run locally and are not model prompts.")
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
}

type Sessions = Arc<Mutex<HashMap<protocol::SessionId, AcpSession>>>;

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
) -> agent_client_protocol::Result<()> {
    let sessions: Sessions = Arc::new(Mutex::new(HashMap::new()));
    let runtime = Runtime::new(512);
    let default_model = config.model.clone();
    let default_thinking = crate::app::ThinkingLevel::Medium;

    let new_sessions = sessions.clone();
    let new_runtime = runtime.clone();
    let new_runner = runner.clone();
    let new_config = config;
    let prompt_sessions = sessions.clone();
    let prompt_runtime = runtime.clone();
    let cancel_sessions = sessions.clone();
    let new_store = store.clone();
    let load_store = store.clone();
    let prompt_store = store;
    let load_sessions = sessions.clone();
    let load_runtime = runtime.clone();
    let load_runner = runner.clone();
    let new_default_model = default_model.clone();
    let load_default_model = default_model;
    let new_default_thinking = default_thinking;
    let load_default_thinking = default_thinking;
    let config_sessions = sessions.clone();
    let config_runtime = runtime.clone();
    let config_runner = runner.clone();
    let config_store = prompt_store.clone();
    let supports_load = load_store.is_some();
    let supports_elicitation = Arc::new(AtomicBool::new(false));
    let initialize_elicitation = supports_elicitation.clone();

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
                responder.respond(
                    protocol::InitializeResponse::new(ProtocolVersion::V1)
                    .agent_capabilities(
                        protocol::AgentCapabilities::new().load_session(supports_load),
                    )
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
                        model: session_model,
                        thinking: session_thinking,
                        models: Vec::new(),
                        active_turn: None,
                        cancellation: None,
                        has_prompt: false,
                    },
                );
                responder.respond(protocol::NewSessionResponse::new(external_id.clone()))?;
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
                load_sessions.lock().await.insert(
                    external_id.clone(),
                    AcpSession {
                        internal_id,
                        cwd: saved_cwd,
                        profile: permission_for_resume(&loaded.events, restored.profile),
                        model: model_for_resume(&loaded.events, &load_default_model),
                        thinking: thinking_for_resume(
                            &loaded.events,
                            load_default_thinking,
                        ),
                        models: Vec::new(),
                        active_turn: None,
                        cancellation: None,
                        has_prompt: loaded.events.iter().any(|envelope| {
                            matches!(envelope.event, AppEvent::PromptAccepted { .. })
                        }),
                    },
                );
                responder.respond(protocol::LoadSessionResponse::new())?;
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
                        SlashCommand::Permission(profile) => {
                            let events = prompt_runtime
                                .dispatch(AppCommand::ChangePermissionProfile {
                                    session_id: internal_id,
                                    profile,
                                })
                                .await
                                .map_err(agent_error)?;
                            if let Some(store) = &prompt_store {
                                store.append_all(&events).map_err(agent_error)?;
                            }
                            if let Some(session) = prompt_sessions.lock().await.get_mut(&session_id)
                            {
                                session.profile = profile;
                            }
                            send_text(
                                &connection,
                                &session_id,
                                format!("Permission profile changed to {profile}."),
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
                            send_text(
                                &connection,
                                &session_id,
                                format!(
                                    "Thinking set to {level}; development proxy transport is pending."
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
                                            send_event(&task_connection, &session_id, event)?;
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
                                    send_event(&task_connection, &session_id, event)?;
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
                let turn_id = TurnId::new();
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
                    tokio::spawn(async move {
                        match title_runner
                            .generate_title(
                                &title_session,
                                &title_prompt,
                                CancellationToken::new(),
                            )
                            .await
                        {
                            Ok(title) => {
                                if let Err(error) =
                                    title_store.set_title_if_absent(&title_session, &title)
                                {
                                    tracing::warn!(
                                        session_id = %title_session,
                                        %error,
                                        "could not persist generated ACP session title"
                                    );
                                }
                            }
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
                connection.spawn(async move {
                    let (events_tx, mut events_rx) = mpsc::channel(APP_EVENT_QUEUE_CAPACITY);
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
                                send_event(&task_connection, &session_id, event)?;
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
                        send_event(&task_connection, &session_id, event)?;
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
            async move |request: protocol::SetSessionConfigOptionRequest,
                        responder: Responder<protocol::SetSessionConfigOptionResponse>,
                        _connection| {
                if request.config_id.0.as_ref() != MODEL_CONFIG_ID {
                    return responder.respond_with_internal_error("unknown session config option");
                }
                let Some(value) = request.value.as_value_id() else {
                    return responder
                        .respond_with_internal_error("model config requires a selected value ID");
                };
                let model = value.0.to_string();
                let (internal_id, models) = {
                    let sessions = config_sessions.lock().await;
                    let Some(session) = sessions.get(&request.session_id) else {
                        return responder.respond_with_internal_error("unknown ACP session");
                    };
                    if session.cancellation.is_some() {
                        return responder.respond_with_internal_error(
                            "cannot change model while the session has active work",
                        );
                    }
                    if !session.models.iter().any(|candidate| candidate == &model) {
                        return responder.respond_with_internal_error(
                            "model is not in the provider catalog",
                        );
                    }
                    (session.internal_id.clone(), session.models.clone())
                };
                config_runner
                    .set_model(&internal_id, model.clone())
                    .await
                    .map_err(agent_error)?;
                let events = config_runtime
                    .dispatch(AppCommand::ChangeModel {
                        session_id: internal_id,
                        model: model.clone(),
                    })
                    .await
                    .map_err(agent_error)?;
                if let Some(store) = &config_store {
                    store.append_all(&events).map_err(agent_error)?;
                }
                if let Some(session) = config_sessions.lock().await.get_mut(&request.session_id) {
                    session.model.clone_from(&model);
                }
                responder.respond(protocol::SetSessionConfigOptionResponse::new(vec![
                    model_config_option(&model, &models),
                ]))
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
) -> agent_client_protocol::Result<()> {
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
        AppEvent::PermissionProfileChanged { profile } => send_text(
            connection,
            session_id,
            format!("[settings] permission profile: {profile}"),
        ),
        AppEvent::ModelChanged { model } => {
            send_text(connection, session_id, format!("[settings] model: {model}"))
        }
        AppEvent::ThinkingLevelChanged { level } => send_text(
            connection,
            session_id,
            format!("[settings] thinking: {level} (proxy transport pending)"),
        ),
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
        } => connection.send_notification(protocol::SessionNotification::new(
            session_id.clone(),
            protocol::SessionUpdate::ToolCall(
                protocol::ToolCall::new(call_id, name)
                    .status(protocol::ToolCallStatus::Pending)
                    .raw_input(arguments),
            ),
        )),
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
        } => send_tool_update(
            connection,
            session_id,
            call_id,
            protocol::ToolCallUpdateFields::new().content(vec![
                protocol::ContentBlock::Text(protocol::TextContent::new(format!(
                    "DIFF\n{diff}{}",
                    if truncated {
                        "\n… diff truncated"
                    } else {
                        ""
                    }
                )))
                .into(),
            ]),
        ),
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
