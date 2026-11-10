//! First-party local MCP control; executable authority never comes from a prompt.
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
    };
    connection.spawn(async move {
        let _guard = guard;
        match operation.apply(request).await {
            Ok(state) => responder.respond(state),
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
        } = self;

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
            Action::List => unreachable!(),
            Action::Save { server } => save_input(&store, &mut servers, server).await?,
            Action::Import { servers: inputs } => {
                if inputs.len() > 32 {
                    return Err(AxiomError::Config(
                        "at most 32 local MCP servers can be imported".into(),
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
                    server.error = Some("Could not initialize this local server within 30 seconds. Check its command, arguments, dependencies and credentials.".into());
                }
            }
            Action::Select { tools } => {
                let id = thread.as_ref().ok_or_else(|| {
                    AxiomError::Config("a thread is required for MCP tool selection".into())
                })?;
                selection = Some((id, tools));
            }
        }
        if servers.len() > 32 {
            return Err(AxiomError::Config(
                "at most 32 local MCP servers are supported".into(),
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
    crate::desktop_mcp::validate_input(&input)?;
    let previous = servers.iter().find(|s| s.name == input.name);
    let credential_id = if let Some(env) = &input.env {
        if env.is_empty() {
            None
        } else {
            Some(uuid::Uuid::new_v4().to_string())
        }
    } else {
        previous.and_then(|s| s.credential_id.clone())
    };
    let environment_keys = if let Some(env) = &input.env {
        // Preserve an empty, credential-free configuration without requiring a keychain.
        if !env.is_empty() {
            let bound = store.clone();
            let name = format!(
                "{}:{}",
                input.name,
                credential_id.as_ref().expect("new credential ID")
            );
            let env = env.clone();
            crate::desktop_mcp::credential_task(move || {
                crate::desktop_mcp::write_environment(&bound, &name, &env)
            })
            .await?;
        }
        env.keys().cloned().collect()
    } else {
        previous
            .map(|s| s.environment_keys.clone())
            .unwrap_or_default()
    };
    let unchanged = previous
        .is_some_and(|s| s.command == input.command && s.args == input.args && input.env.is_none());
    let tools = if unchanged {
        previous.expect("previous").tools.clone()
    } else {
        Vec::new()
    };
    let status = if tools.is_empty() {
        "untested"
    } else {
        "tested"
    }
    .into();
    let server = DesktopMcpServer {
        name: input.name,
        command: input.command,
        args: input.args,
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
