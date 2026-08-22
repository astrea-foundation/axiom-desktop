use std::{collections::HashMap, path::PathBuf, sync::Arc};

use agent_client_protocol::schema::{ProtocolVersion, v1 as protocol};
use agent_client_protocol::{Agent, Client, ConnectionTo, Responder, Stdio};
use async_trait::async_trait;
use tokio::sync::{Mutex, mpsc};
use tokio_util::sync::CancellationToken;

use crate::{
    agent::TurnRunner,
    app::{AppCommand, AppEvent, Origin, PermissionProfile, Runtime, SessionId, TurnId},
    policy::{ApprovalChoice, ApprovalHandler, ApprovalRequest, ApprovalResponse},
    session::SessionStore,
};

#[derive(Clone)]
struct AcpSession {
    internal_id: SessionId,
    cwd: PathBuf,
    active_turn: Option<TurnId>,
    cancellation: Option<CancellationToken>,
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

/// Serve ACP v1 over stdio. Stdout is owned by the protocol transport.
pub async fn serve_stdio(
    runner: Arc<dyn TurnRunner>,
    profile: PermissionProfile,
    store: Option<SessionStore>,
) -> agent_client_protocol::Result<()> {
    let sessions: Sessions = Arc::new(Mutex::new(HashMap::new()));
    let runtime = Runtime::new(512);

    let new_sessions = sessions.clone();
    let new_runtime = runtime.clone();
    let prompt_sessions = sessions.clone();
    let prompt_runtime = runtime.clone();
    let cancel_sessions = sessions.clone();
    let new_store = store.clone();
    let prompt_store = store;

    Agent
        .builder()
        .name("axiomcli")
        .on_receive_request(
            async move |_request: protocol::InitializeRequest,
                        responder: Responder<protocol::InitializeResponse>,
                        _connection| {
                responder.respond(
                    protocol::InitializeResponse::new(ProtocolVersion::V1)
                    .agent_capabilities(protocol::AgentCapabilities::new())
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
                        _connection| {
                if !request.cwd.is_absolute() {
                    return responder
                        .respond_with_internal_error("ACP session cwd must be absolute");
                }
                let cwd = request.cwd.canonicalize().map_err(|error| {
                    agent_client_protocol::util::internal_error(format!(
                        "cannot open ACP workspace: {error}"
                    ))
                })?;
                let internal_id = SessionId::new();
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
                let external_id = protocol::SessionId::new(internal_id.to_string());
                new_sessions.lock().await.insert(
                    external_id.clone(),
                    AcpSession {
                        internal_id,
                        cwd,
                        active_turn: None,
                        cancellation: None,
                    },
                );
                responder.respond(protocol::NewSessionResponse::new(external_id))
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
                let turn_id = TurnId::new();
                let cancellation = CancellationToken::new();
                let (internal_id, cwd) = {
                    let mut sessions = prompt_sessions.lock().await;
                    let Some(session) = sessions.get_mut(&session_id) else {
                        return responder.respond_with_internal_error("unknown ACP session");
                    };
                    if session.active_turn.is_some() {
                        return responder
                            .respond_with_internal_error("session already has an active turn");
                    }
                    session.active_turn = Some(turn_id.clone());
                    session.cancellation = Some(cancellation.clone());
                    (session.internal_id.clone(), session.cwd.clone())
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
                }

                let task_sessions = prompt_sessions.clone();
                let task_runtime = prompt_runtime.clone();
                let task_runner = runner.clone();
                let task_connection = connection.clone();
                let task_store = prompt_store.clone();
                connection.spawn(async move {
                    let (events_tx, mut events_rx) = mpsc::unbounded_channel();
                    let run = task_runner.run(
                        crate::agent::TurnContext {
                            session_id: internal_id.clone(),
                            turn_id: turn_id.clone(),
                            cwd,
                            permission_profile: profile,
                            approval: Some(Arc::new(AcpApproval {
                                connection: task_connection.clone(),
                                session_id: session_id.clone(),
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
