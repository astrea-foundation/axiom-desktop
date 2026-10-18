use std::{
    collections::{BTreeMap, HashSet},
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

#[cfg(any(windows, test))]
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use rmcp::{
    RoleClient, ServiceExt as _,
    model::{CallToolRequestParams, JsonObject, Tool as RemoteTool},
    service::RunningService,
    transport::TokioChildProcess,
};
use serde_json::Value;
use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;

use crate::{
    AxiomError, Result,
    audit::{redact_text, redact_value},
    config::McpServerConfig,
    policy::{Effect, ToolAccess},
    tools::{Tool, ToolContext, ToolResult},
};

type McpService = RunningService<RoleClient, ()>;

struct ConnectedMcp {
    service: McpService,
    process_group: crate::process_env::ProcessGroupGuard,
}

const MIN_OUTPUT_BYTES: usize = 1024;
const MAX_SCHEMA_BYTES: usize = 128 * 1024;
const MAX_DESCRIPTION_BYTES: usize = 4 * 1024;
const MCP_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, PartialEq)]
struct ToolDescriptor {
    remote_name: String,
    description: String,
    schema: Value,
}

struct ConnectionState {
    service: Arc<McpService>,
    generation: u64,
    _process_group: crate::process_env::ProcessGroupGuard,
}

struct McpConnection {
    config: McpServerConfig,
    state: RwLock<ConnectionState>,
    reconnect: Mutex<()>,
    next_generation: AtomicU64,
    process_groups: crate::process_env::ProcessGroupRegistry,
    shutting_down: AtomicBool,
}

impl McpConnection {
    fn new(
        config: McpServerConfig,
        connected: ConnectedMcp,
        process_groups: crate::process_env::ProcessGroupRegistry,
    ) -> Self {
        Self {
            config,
            state: RwLock::new(ConnectionState {
                service: Arc::new(connected.service),
                generation: 1,
                _process_group: connected.process_group,
            }),
            reconnect: Mutex::new(()),
            next_generation: AtomicU64::new(2),
            process_groups,
            shutting_down: AtomicBool::new(false),
        }
    }

    async fn current(&self) -> (Arc<McpService>, u64) {
        let state = self.state.read().await;
        (state.service.clone(), state.generation)
    }

    async fn healthy_service(
        &self,
        expected_name: &str,
        expected_schema: &Value,
    ) -> Result<(Arc<McpService>, u64)> {
        if self.shutting_down.load(Ordering::Acquire) {
            return Err(AxiomError::Cancelled);
        }
        let current = self.current().await;
        if !connection_closed(&current.0) {
            return Ok(current);
        }
        self.reconnect_if_current(&current.0, expected_name, expected_schema)
            .await
    }

    async fn reconnect_if_current(
        &self,
        failed: &Arc<McpService>,
        expected_name: &str,
        expected_schema: &Value,
    ) -> Result<(Arc<McpService>, u64)> {
        let _guard = self.reconnect.lock().await;
        if self.shutting_down.load(Ordering::Acquire) {
            return Err(AxiomError::Cancelled);
        }
        let current = self.current().await;
        if !Arc::ptr_eq(&current.0, failed) && !connection_closed(&current.0) {
            verify_reconnected_tool(
                &current.0,
                &self.config.name,
                expected_name,
                expected_schema,
            )
            .await?;
            return Ok(current);
        }

        let connected = connect(&self.config, &self.process_groups).await?;
        let service = Arc::new(connected.service);
        verify_reconnected_tool(&service, &self.config.name, expected_name, expected_schema)
            .await?;
        if self.shutting_down.load(Ordering::Acquire) {
            return Err(AxiomError::Cancelled);
        }
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed);
        let mut state = self.state.write().await;
        *state = ConnectionState {
            service: service.clone(),
            generation,
            _process_group: connected.process_group,
        };
        Ok((service, generation))
    }

    async fn shutdown(&self) {
        if self.shutting_down.swap(true, Ordering::AcqRel) {
            return;
        }
        // Serialize against reconnect so no replacement process can be
        // installed after the final registry drain.
        let _reconnect = self.reconnect.lock().await;
        self.state
            .read()
            .await
            .service
            .cancellation_token()
            .cancel();
        self.process_groups.terminate_all();
    }
}

pub async fn connect_tools(
    configs: &[McpServerConfig],
    max_output_bytes: usize,
) -> Result<Vec<Arc<dyn Tool>>> {
    let mut adapters: Vec<Arc<dyn Tool>> = Vec::new();
    let mut public_names = HashSet::new();
    for config in configs {
        validate_server_config(config)?;
        let process_groups = crate::process_env::ProcessGroupRegistry::default();
        let connected = connect(config, &process_groups).await?;
        let descriptors = discover_tools(&connected.service, &config.name).await?;
        let connection = Arc::new(McpConnection::new(
            config.clone(),
            connected,
            process_groups,
        ));

        for descriptor in descriptors.into_values() {
            let public_name = format!(
                "mcp__{}__{}",
                sanitize_name(&config.name),
                sanitize_name(&descriptor.remote_name)
            );
            if !public_names.insert(public_name.clone()) {
                return Err(AxiomError::Protocol(format!(
                    "duplicate MCP tool namespace `{public_name}`"
                )));
            }
            let trusted_read_only = config
                .read_only_tools
                .iter()
                .any(|name| name == &descriptor.remote_name);
            adapters.push(Arc::new(McpTool {
                public_name,
                remote_name: descriptor.remote_name,
                server_name: config.name.clone(),
                description: descriptor.description,
                schema: descriptor.schema,
                trusted_read_only,
                connection: connection.clone(),
                max_output_bytes: max_output_bytes.max(MIN_OUTPUT_BYTES),
                tool_timeout: Duration::from_secs(config.tool_timeout_secs),
            }));
        }
    }
    Ok(adapters)
}

async fn connect(
    config: &McpServerConfig,
    process_groups: &crate::process_env::ProcessGroupRegistry,
) -> Result<ConnectedMcp> {
    let mut command = configured_command(config)?;
    crate::process_env::apply_sanitized_environment(&mut command);
    command.stdin(Stdio::null());
    #[cfg(unix)]
    command.process_group(0);
    let transport = TokioChildProcess::builder(command)
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| {
            bounded_mcp_error(
                &config.name,
                format!("cannot start configured process: {error}"),
            )
        })?
        .0;
    // The lease is armed before initialization can await. Runtime teardown,
    // initialization timeout, reconnect replacement, and ACP EOF therefore
    // all terminate the MCP server's complete process group.
    let process_group = process_groups.register(transport.id());
    let service = tokio::time::timeout(MCP_REQUEST_TIMEOUT, ().serve(transport))
        .await
        .map_err(|_| bounded_mcp_error(&config.name, "initialization timed out"))?
        .map_err(|error| bounded_mcp_error(&config.name, error))?;
    Ok(ConnectedMcp {
        service,
        process_group,
    })
}

#[cfg(not(windows))]
// Keep the platform implementations type-identical: Windows resolution is
// fallible even though spawning a direct POSIX executable is not.
#[allow(clippy::unnecessary_wraps)]
fn configured_command(config: &McpServerConfig) -> Result<tokio::process::Command> {
    let mut command = tokio::process::Command::new(&config.command);
    command.args(&config.args);
    Ok(command)
}

#[cfg(windows)]
fn configured_command(config: &McpServerConfig) -> Result<tokio::process::Command> {
    let cwd = std::env::current_dir().map_err(AxiomError::Io)?;
    let program =
        crate::process_env::resolve_windows_program(&config.command, &cwd).ok_or_else(|| {
            AxiomError::Config(format!(
                "MCP server `{}` executable `{}` was not found on the sanitized Windows PATH",
                config.name, config.command
            ))
        })?;
    let system_root = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .ok_or_else(|| AxiomError::Config("Windows SystemRoot is unavailable".into()))?;
    let (launcher, arguments) = windows_mcp_launch(&program, &config.args, &system_root)?;
    if !launcher.is_file() {
        return Err(AxiomError::Config(format!(
            "Windows MCP launcher is missing: {}",
            launcher.display()
        )));
    }
    let mut command = tokio::process::Command::new(launcher);
    command.args(arguments);
    Ok(command)
}

#[cfg(any(windows, test))]
fn windows_mcp_launch(
    program: &Path,
    arguments: &[String],
    system_root: &Path,
) -> Result<(PathBuf, Vec<String>)> {
    let extension = program
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "cmd" | "bat" => {
            let mut tokens = Vec::with_capacity(arguments.len() + 1);
            tokens.push(cmd_literal(program.as_os_str().to_string_lossy().as_ref())?);
            for argument in arguments {
                tokens.push(cmd_literal(argument)?);
            }
            // `/S /C` has one documented outer quote pair around the complete command. Every
            // inner token is separately quoted, and expansion-bearing `%`/quotes/newlines are
            // rejected so configured arguments cannot escape their token boundary.
            let invocation = format!("\"{}\"", tokens.join(" "));
            Ok((
                system_root.join("System32").join("cmd.exe"),
                vec![
                    "/D".into(),
                    "/V:OFF".into(),
                    "/S".into(),
                    "/C".into(),
                    invocation,
                ],
            ))
        }
        "ps1" => {
            let mut launch_arguments = vec![
                "-NoLogo".into(),
                "-NoProfile".into(),
                "-NonInteractive".into(),
                "-File".into(),
                program.as_os_str().to_string_lossy().into_owned(),
            ];
            launch_arguments.extend(arguments.iter().cloned());
            Ok((
                system_root
                    .join("System32")
                    .join("WindowsPowerShell")
                    .join("v1.0")
                    .join("powershell.exe"),
                launch_arguments,
            ))
        }
        _ => Ok((program.to_path_buf(), arguments.to_vec())),
    }
}

#[cfg(any(windows, test))]
fn cmd_literal(value: &str) -> Result<String> {
    if value.is_empty()
        || value
            .chars()
            .any(|character| matches!(character, '\0' | '\r' | '\n' | '"' | '%'))
    {
        return Err(AxiomError::Config(
            "Windows .cmd/.bat MCP paths and arguments cannot contain quotes, percent signs, or line breaks; use an executable wrapper for those values"
                .into(),
        ));
    }
    Ok(format!("\"{value}\""))
}

async fn discover_tools(
    service: &McpService,
    server_name: &str,
) -> Result<BTreeMap<String, ToolDescriptor>> {
    let tools = tokio::time::timeout(MCP_REQUEST_TIMEOUT, service.list_all_tools())
        .await
        .map_err(|_| bounded_mcp_error(server_name, "tool discovery timed out"))?
        .map_err(|error| bounded_mcp_error(server_name, error))?;
    let mut descriptors = BTreeMap::new();
    for tool in tools {
        let descriptor = normalize_descriptor(server_name, tool)?;
        if descriptors
            .insert(descriptor.remote_name.clone(), descriptor)
            .is_some()
        {
            return Err(AxiomError::Protocol(format!(
                "MCP server `{server_name}` advertised a duplicate tool name"
            )));
        }
    }
    Ok(descriptors)
}

fn normalize_descriptor(server_name: &str, tool: RemoteTool) -> Result<ToolDescriptor> {
    let remote_name = tool.name.into_owned();
    if remote_name.trim().is_empty() || sanitize_name(&remote_name).is_empty() {
        return Err(AxiomError::Protocol(format!(
            "MCP server `{server_name}` advertised an invalid tool name"
        )));
    }
    let schema = Value::Object((*tool.input_schema).clone());
    let schema_bytes = serde_json::to_vec(&schema)?.len();
    if schema_bytes > MAX_SCHEMA_BYTES {
        return Err(AxiomError::Protocol(format!(
            "MCP server `{server_name}` tool `{remote_name}` schema exceeds {MAX_SCHEMA_BYTES} bytes"
        )));
    }
    if schema
        .get("type")
        .is_some_and(|kind| kind.as_str() != Some("object"))
    {
        return Err(AxiomError::Protocol(format!(
            "MCP server `{server_name}` tool `{remote_name}` has a non-object input schema"
        )));
    }
    let raw_description = tool.description.as_deref().unwrap_or("No description");
    let description = clean_and_truncate(raw_description, MAX_DESCRIPTION_BYTES);
    Ok(ToolDescriptor {
        remote_name: remote_name.clone(),
        description: format!("MCP {server_name} / {remote_name}: {description}"),
        schema,
    })
}

async fn verify_reconnected_tool(
    service: &McpService,
    server_name: &str,
    expected_name: &str,
    expected_schema: &Value,
) -> Result<()> {
    let tools = discover_tools(service, server_name).await?;
    let Some(tool) = tools.get(expected_name) else {
        return Err(AxiomError::Protocol(format!(
            "MCP server `{server_name}` no longer advertises tool `{expected_name}` after reconnect"
        )));
    };
    if &tool.schema != expected_schema {
        return Err(AxiomError::Protocol(format!(
            "MCP server `{server_name}` changed the schema for tool `{expected_name}`; restart AxiomCLI to review it"
        )));
    }
    Ok(())
}

fn validate_server_config(config: &McpServerConfig) -> Result<()> {
    if !(1..=86_400).contains(&config.tool_timeout_secs) {
        return Err(AxiomError::Config(
            "MCP tool_timeout_secs must be between 1 and 86400".into(),
        ));
    }
    if config.name.trim().is_empty() || config.command.trim().is_empty() {
        return Err(AxiomError::Config(
            "MCP server name and command cannot be empty".into(),
        ));
    }
    if sanitize_name(&config.name).is_empty() {
        return Err(AxiomError::Config(format!(
            "MCP server name `{}` has no safe namespace characters",
            config.name
        )));
    }
    Ok(())
}

fn sanitize_name(input: &str) -> String {
    input
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' {
                character.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .to_owned()
}

fn connection_closed(service: &McpService) -> bool {
    service.is_closed() || service.peer().is_transport_closed()
}

struct McpTool {
    public_name: String,
    remote_name: String,
    server_name: String,
    description: String,
    schema: Value,
    trusted_read_only: bool,
    connection: Arc<McpConnection>,
    max_output_bytes: usize,
    tool_timeout: Duration,
}

#[async_trait]
impl Tool for McpTool {
    fn name(&self) -> &str {
        &self.public_name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn access(&self) -> ToolAccess {
        if self.trusted_read_only {
            ToolAccess::Read
        } else {
            ToolAccess::Write
        }
    }

    fn parameters(&self) -> Value {
        self.schema.clone()
    }

    fn effects(&self, _context: &ToolContext, arguments: &Value) -> Result<Vec<Effect>> {
        if !arguments.is_object() {
            return Err(AxiomError::Tool(format!(
                "MCP tool `{}` arguments must be an object",
                self.public_name
            )));
        }
        Ok(vec![Effect::Mcp {
            server: self.server_name.clone(),
            tool: self.remote_name.clone(),
            side_effecting: !self.trusted_read_only,
        }])
    }

    async fn shutdown(&self) {
        self.connection.shutdown().await;
    }

    async fn execute(
        &self,
        _context: &ToolContext,
        arguments: Value,
        cancellation: CancellationToken,
    ) -> Result<ToolResult> {
        let arguments: JsonObject = arguments
            .as_object()
            .cloned()
            .ok_or_else(|| AxiomError::Tool("MCP arguments must be an object".into()))?;
        let (mut service, mut generation) = self
            .connection
            .healthy_service(&self.remote_name, &self.schema)
            .await?;
        let request = || {
            CallToolRequestParams::new(self.remote_name.clone()).with_arguments(arguments.clone())
        };

        let first = call_with_limits(
            &service,
            request(),
            &cancellation,
            &self.server_name,
            self.tool_timeout,
        )
        .await;
        let result = match first {
            Ok(result) => result,
            Err(_error) if cancellation.is_cancelled() => return Err(AxiomError::Cancelled),
            // Deadline expiry has an unknown outcome even if the connection
            // also closed. It is never eligible for the read-only reconnect retry.
            Err(error @ AxiomError::Tool(_)) => return Err(error),
            Err(_error) if connection_closed(&service) && self.trusted_read_only => {
                (service, generation) = self
                    .connection
                    .reconnect_if_current(&service, &self.remote_name, &self.schema)
                    .await?;
                call_with_limits(
                    &service,
                    request(),
                    &cancellation,
                    &self.server_name,
                    self.tool_timeout,
                )
                .await
                .map_err(|retry_error| {
                    bounded_mcp_error(
                        &self.server_name,
                        format!("read-only retry failed after reconnect: {retry_error}"),
                    )
                })?
            }
            Err(error) if connection_closed(&service) => {
                return Err(bounded_mcp_error(
                    &self.server_name,
                    format!(
                        "connection closed during side-effecting tool `{}`; outcome is unknown and the call was not retried: {error}",
                        self.remote_name
                    ),
                ));
            }
            Err(error) => return Err(error),
        };

        let success = result.is_error != Some(true);
        let mut envelope = serde_json::json!({
            "server": self.server_name,
            "tool": self.remote_name,
            "connection_generation": generation,
            "content_trust": "untrusted MCP result",
            "result": result,
        });
        redact_value(&mut envelope);
        let serialized = serde_json::to_string_pretty(&envelope)?;
        let (content, truncated) = truncate(&serialized, self.max_output_bytes);
        Ok(ToolResult {
            content,
            success,
            truncated,
            changed_paths: Vec::new(),
            diff: None,
            file_diffs: Vec::new(),
            background: None,
            events: Vec::new(),
            questions: None,
        })
    }
}

async fn call_with_limits(
    service: &McpService,
    request: CallToolRequestParams,
    cancellation: &CancellationToken,
    server_name: &str,
    timeout: Duration,
) -> Result<rmcp::model::CallToolResult> {
    let mut handle = tokio::select! {
        biased;
        () = cancellation.cancelled() => return Err(AxiomError::Cancelled),
        result = service.peer().send_cancellable_request(
            rmcp::model::ClientRequest::CallToolRequest(rmcp::model::CallToolRequest::new(request)),
            rmcp::service::PeerRequestOptions::no_options(),
        ) => result.map_err(|error| bounded_mcp_error(server_name, error))?,
    };
    let result = tokio::select! {
        biased;
        () = cancellation.cancelled() => Err(AxiomError::Cancelled),
        result = tokio::time::timeout(timeout, &mut handle.rx) => match result {
            Ok(Ok(Ok(rmcp::model::ServerResult::CallToolResult(result)))) => return Ok(result),
            Ok(Ok(Ok(_))) => return Err(bounded_mcp_error(server_name, "unsupported tool response")),
            Ok(Ok(Err(error))) => return Err(bounded_mcp_error(server_name, error)),
            Ok(Err(_)) => return Err(bounded_mcp_error(server_name, "tool connection closed")),
            Err(_) => Err(AxiomError::Tool(format!("MCP server `{server_name}`: tool call timed out after {} seconds; increase this server's tool_timeout_secs for longer operations", timeout.as_secs()))),
        },
    };
    // Notify the server instead of merely abandoning the local future. A
    // timeout never authorizes replay of an operation with an unknown outcome.
    let _ = tokio::time::timeout(
        Duration::from_secs(1),
        handle.cancel(Some("Axiom stopped waiting for this tool call".into())),
    )
    .await;
    result
}

fn clean_and_truncate(input: &str, max_bytes: usize) -> String {
    let clean: String = input
        .chars()
        .filter(|character| !character.is_control() || matches!(character, '\n' | '\t'))
        .collect();
    truncate(&redact_text(&clean), max_bytes).0
}

fn truncate(input: &str, max_bytes: usize) -> (String, bool) {
    if input.len() <= max_bytes {
        return (input.into(), false);
    }
    let mut end = max_bytes;
    while !input.is_char_boundary(end) {
        end -= 1;
    }
    (input[..end].into(), true)
}

fn bounded_mcp_error(server: &str, error: impl std::fmt::Display) -> AxiomError {
    AxiomError::Protocol(format!(
        "MCP server `{server}`: {}",
        clean_and_truncate(&error.to_string(), MAX_DESCRIPTION_BYTES)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespaces_are_stable_and_restricted() {
        assert_eq!(sanitize_name("GitHub Server"), "github_server");
        assert_eq!(sanitize_name("../../bad"), "bad");
    }

    #[test]
    fn output_truncation_preserves_utf8() {
        let (output, truncated) = truncate("cherry 🌸 tree", 10);
        assert!(truncated);
        assert!(std::str::from_utf8(output.as_bytes()).is_ok());
    }

    #[test]
    fn errors_are_bounded_and_redacted() {
        let error = bounded_mcp_error("fixture", format!("axm_{}", "x".repeat(10_000)));
        let rendered = error.to_string();
        assert!(!rendered.contains("axm_"));
        assert!(rendered.len() < MAX_DESCRIPTION_BYTES + 100);
    }

    #[test]
    fn windows_cmd_mcp_shim_uses_the_absolute_system_launcher() {
        let root = tempfile::tempdir().expect("temporary root");
        let system_root = root.path().join("Windows");
        let shim = root.path().join("node-bin").join("npx.cmd");
        let arguments = vec![
            "-y".into(),
            "@modelcontextprotocol/server-filesystem".into(),
        ];

        let (launcher, launcher_arguments) =
            windows_mcp_launch(&shim, &arguments, &system_root).expect("build Windows launch");

        assert_eq!(launcher, system_root.join("System32").join("cmd.exe"));
        assert_eq!(&launcher_arguments[..4], &["/D", "/V:OFF", "/S", "/C"]);
        assert_eq!(
            launcher_arguments[4],
            format!(
                "\"\"{}\" \"-y\" \"@modelcontextprotocol/server-filesystem\"\"",
                shim.display()
            )
        );
    }

    #[test]
    fn windows_cmd_mcp_shim_rejects_expansion_bearing_arguments() {
        let error = windows_mcp_launch(
            Path::new(r"C:\nodejs\npx.cmd"),
            &["%PATH%".into()],
            Path::new(r"C:\Windows"),
        )
        .expect_err("percent expansion must be rejected");

        assert!(error.to_string().contains("cannot contain quotes"));
    }
}
