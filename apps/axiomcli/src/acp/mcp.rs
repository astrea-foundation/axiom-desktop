//! First-party MCP control; connection authority never comes from a prompt.
use super::*;
use crate::{AxiomError, Result};
use extension::{
    DesktopMcpAction as Action, DesktopMcpRequest, DesktopMcpResponse, DesktopMcpServer,
    DesktopMcpServerInput, DesktopMcpTool,
};
use std::collections::BTreeMap;

pub(super) async fn manage(
    context: &ServerContext,
    request: DesktopMcpRequest,
    responder: Responder<DesktopMcpResponse>,
    connection: &ConnectionTo<Client>,
) -> agent_client_protocol::Result<()> {
    if context.frontend != FrontendKind::DesktopChat
        || !extension_enabled(&context.extension_state, ExtensionFeature::DesktopMcp)
    {
        return responder.respond_with_error(extension_not_negotiated());
    }
    let remote_supported = extension_enabled(&context.extension_state, ExtensionFeature::RemoteMcp);
    if !remote_supported
        && match &request.action {
            Action::Connect { .. } | Action::Disconnect { .. } | Action::CancelConnect { .. } => {
                true
            }
            Action::Save { server } => server.remote.is_some(),
            Action::Import { servers } => servers.iter().any(|s| s.remote.is_some()),
            _ => false,
        }
    {
        return responder.respond_with_error(extension_not_negotiated());
    }
    let store = respond_or_return!(
        responder,
        context
            .store
            .as_ref()
            .ok_or_else(|| agent_client_protocol::util::internal_error(
                "account storage unavailable"
            ))
            .and_then(|s| s.bind_active_account().map_err(agent_error))
    );
    let thread = respond_or_return!(
        responder,
        request
            .thread_id
            .as_deref()
            .map(SessionId::from_str)
            .transpose()
            .map_err(|_| agent_client_protocol::util::internal_error("invalid thread ID"))
    );
    let sessions = context.sessions.clone();
    let runner = context.runner.clone();
    let lock = context.profile_settings_lock.clone();
    let cancellation = responder.cancellation();
    let work_cancellation = CancellationToken::new();
    let guard = respond_or_return!(
        responder,
        context
            .account_work
            .register(work_cancellation.clone())
            .map_err(agent_error)
    );
    let operation = McpOperation {
        store,
        thread,
        sessions,
        runner,
        lock,
        cancellation,
        work_cancellation,
        remote_supported,
    };
    connection.spawn(async move {
        let _guard = guard;
        match Box::pin(operation.apply(request)).await {
            Ok(mut state) => {
                state.remote_supported = remote_supported;
                if !remote_supported {
                    state.servers.retain(|server| server.remote.is_none());
                    state.selected_tools.retain(|name| {
                        state
                            .servers
                            .iter()
                            .any(|server| server.tools.iter().any(|tool| &tool.name == name))
                    });
                }
                responder.respond(state)
            }
            Err(error) => responder.respond_with_error(agent_error(error)),
        }
    })
}

struct McpOperation {
    store: SessionStore,
    thread: Option<SessionId>,
    sessions: Sessions,
    runner: Arc<dyn TurnRunner>,
    lock: Arc<Mutex<()>>,
    cancellation: agent_client_protocol::RequestCancellation,
    work_cancellation: CancellationToken,
    remote_supported: bool,
}

impl McpOperation {
    async fn apply(self, request: DesktopMcpRequest) -> Result<DesktopMcpResponse> {
        let Self {
            store,
            thread,
            sessions,
            runner,
            lock,
            cancellation,
            work_cancellation,
            remote_supported,
        } = self;

        if let Action::CancelConnect { name } = &request.action {
            crate::desktop_mcp::remote::cancel_authorization(&store, name)?;
            return store.desktop_mcp_snapshot(thread.as_ref());
        }
        let _lock = lock.lock().await;
        let registry = sessions.lock().await;
        let snapshot = store.desktop_mcp_snapshot(thread.as_ref())?;
        if matches!(request.action, Action::List) {
            return Ok(snapshot);
        }
        if request.expected_revision != Some(snapshot.revision) {
            return Err(AxiomError::InvalidTransition(
                "MCP settings changed; refresh before saving".into(),
            ));
        }
        // Account-wide executable edits cannot race any active prompt. Tool selection
        // is also idle-only, and remains independent of Agent's builtin permissions.
        if !registry.active_work.is_empty() {
            return Err(AxiomError::InvalidTransition(
                "wait for active operations before changing MCP settings".into(),
            ));
        }
        if let Some(id) = &thread
            && !registry.contains_key(&protocol::SessionId::new(id.to_string()))
        {
            return Err(AxiomError::InvalidTransition(
                "open the thread before selecting MCP tools".into(),
            ));
        }
        let revision = snapshot.revision;
        let mut servers = snapshot.servers;
        let mut selection = None;
        match request.action {
            Action::List | Action::CancelConnect { .. } => unreachable!(),
            Action::Save { server } => save_input(&store, &mut servers, *server).await?,
            Action::Import { servers: inputs } => {
                if inputs.len() > 32 {
                    return Err(AxiomError::Config(
                        "at most 32 MCP connections can be imported".into(),
                    ));
                }
                let mut names = std::collections::BTreeSet::new();
                for input in &inputs {
                    crate::desktop_mcp::validate_input(input)?;
                    if !names.insert(&input.name) {
                        return Err(AxiomError::Config(
                            "duplicate imported MCP server name".into(),
                        ));
                    }
                }
                for mut input in inputs {
                    input.enabled = false;
                    save_input(&store, &mut servers, input).await?;
                }
            }
            Action::Delete { name } => {
                let Some(index) = servers.iter().position(|s| s.name == name) else {
                    return Err(AxiomError::Config("unknown MCP connection".into()));
                };
                servers.remove(index);
            }
            Action::Connect { name } => {
                let server = servers
                    .iter_mut()
                    .find(|s| s.name == name)
                    .ok_or_else(|| AxiomError::Config("unknown MCP connection".into()))?;
                let (token, _authorization) =
                    crate::desktop_mcp::remote::begin_authorization(&store, &name)?;
                let saved = server.clone();
                let authorization =
                    crate::desktop_mcp::remote::authorize(&store, &saved, token.clone());
                tokio::pin!(authorization);
                let id = tokio::select! {
                    () = cancellation.cancelled() => { token.cancel(); authorization.await? },
                    () = work_cancellation.cancelled() => { token.cancel(); authorization.await? },
                    result = &mut authorization => result?,
                };
                server.credential_id = Some(id);
                server.tools.clear();
                server.status = "untested".into();
                server.error = None;
            }
            Action::Disconnect { name } => {
                let server = servers
                    .iter_mut()
                    .find(|s| s.name == name)
                    .ok_or_else(|| AxiomError::Config("unknown MCP connection".into()))?;
                if server.remote.is_none() {
                    return Err(AxiomError::Config(
                        "only remote connections have a service sign-in".into(),
                    ));
                }
                server.credential_id = None;
                server.enabled = false;
                server.tools.clear();
                server.status = "needs_auth".into();
                server.error = None;
            }
            Action::Test { name } => {
                let server = servers
                    .iter_mut()
                    .find(|s| s.name == name)
                    .ok_or_else(|| AxiomError::Config("unknown MCP connection".into()))?;
                let bound = store.clone();
                let saved = server.clone();
                let config = crate::desktop_mcp::credential_task(move || {
                    crate::desktop_mcp::launch_config(&bound, &saved)
                })
                .await?;
                let configs = [config];
                let result = tokio::select! {
                    () = cancellation.cancelled() => return Err(AxiomError::Cancelled),
                    () = work_cancellation.cancelled() => return Err(AxiomError::Cancelled),
                    result = tokio::time::timeout(std::time::Duration::from_secs(30), crate::mcp::connect_tools(&configs, 64 * 1024)) => result,
                };
                if let Ok(Ok(tools)) = result {
                    server.tools = tools
                        .iter()
                        .map(|t| DesktopMcpTool {
                            name: t.name().into(),
                            description: crate::mcp::redact_environment_text(
                                t.description(),
                                &configs[0].env,
                            ),
                            schema_hash: crate::desktop_mcp::schema_hash(&t.parameters()),
                        })
                        .collect();
                    server.tools.sort_by(|a, b| a.name.cmp(&b.name));
                    for tool in tools {
                        tool.shutdown().await;
                    }
                    server.status = "tested".into();
                    server.error = None;
                } else {
                    // Process/server errors can echo credentials. Keep the public
                    // failure useful without forwarding untrusted stderr or values.
                    server.tools.clear();
                    server.status = "error".into();
                    server.error = Some(if server.remote.is_some() {
                        "Could not initialize this remote service within 30 seconds. Check its URL, availability and sign-in."
                    } else {
                        "Could not initialize this local server within 30 seconds. Check its command, arguments, dependencies and credentials."
                    }.into());
                }
            }
            Action::Select { tools } => {
                if !remote_supported
                    && tools.iter().any(|name| {
                        servers.iter().any(|server| {
                            server.remote.is_some()
                                && server.tools.iter().any(|tool| &tool.name == name)
                        })
                    })
                {
                    return Err(AxiomError::Config(
                        "restart the updated Desktop app to select remote tools".into(),
                    ));
                }
                let id = thread.as_ref().ok_or_else(|| {
                    AxiomError::Config("a thread is required for MCP tool selection".into())
                })?;
                selection = Some((id, tools));
            }
        }
        if servers.len() > 32 {
            return Err(AxiomError::Config(
                "at most 32 MCP connections are supported".into(),
            ));
        }
        if cancellation.is_cancelled() || work_cancellation.is_cancelled() {
            return Err(AxiomError::Cancelled);
        }
        for session in registry.values() {
            runner
                .reset_session_permissions(&session.internal_id)
                .await?;
        }
        let old_servers = store.desktop_mcp_snapshot(None)?.servers;
        store.save_desktop_mcp(revision, servers, selection)?;
        let new_servers = store.desktop_mcp_snapshot(None)?.servers;
        let bound = store.clone();
        if crate::desktop_mcp::credential_task(move || {
                for old in old_servers {
                    if let Some(id) = &old.credential_id && !new_servers.iter().any(|s| s.credential_id.as_ref() == Some(id)) && crate::desktop_mcp::write_environment(&bound, &format!("{}:{id}", old.name), &BTreeMap::default()).is_err() {
                        tracing::warn!(server = %old.name, "could not clear retired MCP credentials from the system credential store");
                    }
                }
                Ok(())
            }).await.is_err() {
                tracing::warn!("MCP retired credential cleanup did not complete");
            }
        store.desktop_mcp_snapshot(thread.as_ref())
    }
}

async fn save_input(
    store: &SessionStore,
    servers: &mut Vec<DesktopMcpServer>,
    input: DesktopMcpServerInput,
) -> Result<()> {
    use extension::McpAuthentication;
    crate::desktop_mcp::validate_input(&input)?;
    let previous = servers.iter().find(|s| s.name == input.name);
    let same_transport = previous.is_some_and(|s| s.remote == input.remote);
    let mut environment_keys = Vec::new();
    let mut secrets = None;
    let credential_id = if let Some(remote) = &input.remote {
        match remote.auth {
            McpAuthentication::None => None,
            McpAuthentication::Bearer => {
                if let Some(token) = &input.bearer_token {
                    if token.is_empty() {
                        None
                    } else {
                        secrets = Some(BTreeMap::from([(
                            crate::desktop_mcp::remote::BEARER_KEY.into(),
                            token.clone(),
                        )]));
                        Some(uuid::Uuid::new_v4().to_string())
                    }
                } else if same_transport {
                    previous.and_then(|s| s.credential_id.clone())
                } else {
                    None
                }
            }
            McpAuthentication::Oauth => {
                if same_transport {
                    previous.and_then(|s| s.credential_id.clone())
                } else {
                    None
                }
            }
        }
    } else if let Some(env) = &input.env {
        environment_keys = env.keys().cloned().collect();
        if env.is_empty() {
            None
        } else {
            secrets = Some(env.clone());
            Some(uuid::Uuid::new_v4().to_string())
        }
    } else if same_transport {
        environment_keys = previous
            .map(|s| s.environment_keys.clone())
            .unwrap_or_default();
        previous.and_then(|s| s.credential_id.clone())
    } else {
        None
    };
    if let Some(secrets) = secrets {
        let bound = store.clone();
        let name = format!(
            "{}:{}",
            input.name,
            credential_id.as_ref().expect("new credential ID")
        );
        crate::desktop_mcp::credential_task(move || {
            crate::desktop_mcp::write_environment(&bound, &name, &secrets)
        })
        .await?;
    }
    let unchanged = previous.is_some_and(|s| {
        same_transport
            && s.command == input.command
            && s.args == input.args
            && s.credential_id == credential_id
    });
    let tools = if unchanged {
        previous.expect("previous").tools.clone()
    } else {
        Vec::new()
    };
    let needs_auth = input
        .remote
        .as_ref()
        .is_some_and(|r| r.auth != McpAuthentication::None)
        && credential_id.is_none();
    let status = if needs_auth {
        "needs_auth"
    } else if tools.is_empty() {
        "untested"
    } else {
        "tested"
    }
    .into();
    let server = DesktopMcpServer {
        display_name: input
            .display_name
            .or_else(|| previous.and_then(|s| s.display_name.clone())),
        name: input.name,
        command: input.command,
        args: input.args,
        remote: input.remote,
        enabled: input.enabled,
        environment_keys,
        credential_id,
        tools,
        status,
        error: None,
    };
    servers.retain(|s| s.name != server.name);
    servers.push(server);
    servers.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(())
}
