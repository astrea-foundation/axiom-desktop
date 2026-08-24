use std::{
    collections::{BTreeMap, HashSet},
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

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
}

struct McpConnection {
    config: McpServerConfig,
    state: RwLock<ConnectionState>,
    reconnect: Mutex<()>,
    next_generation: AtomicU64,
}

impl McpConnection {
    fn new(config: McpServerConfig, service: McpService) -> Self {
        Self {
            config,
            state: RwLock::new(ConnectionState {
                service: Arc::new(service),
                generation: 1,
            }),
            reconnect: Mutex::new(()),
            next_generation: AtomicU64::new(2),
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

        let service = Arc::new(connect(&self.config).await?);
        verify_reconnected_tool(&service, &self.config.name, expected_name, expected_schema)
            .await?;
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed);
        let mut state = self.state.write().await;
        *state = ConnectionState {
            service: service.clone(),
            generation,
        };
        Ok((service, generation))
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
        let service = connect(config).await?;
        let descriptors = discover_tools(&service, &config.name).await?;
        let connection = Arc::new(McpConnection::new(config.clone(), service));

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
            }));
        }
    }
    Ok(adapters)
}

async fn connect(config: &McpServerConfig) -> Result<McpService> {
    let mut command = tokio::process::Command::new(&config.command);
    command.args(&config.args).env_clear().stdin(Stdio::null());
    for variable in ["PATH", "LANG", "LC_ALL"] {
        if let Ok(value) = std::env::var(variable) {
            command.env(variable, value);
        }
    }
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
    tokio::time::timeout(MCP_REQUEST_TIMEOUT, ().serve(transport))
        .await
        .map_err(|_| bounded_mcp_error(&config.name, "initialization timed out"))?
        .map_err(|error| bounded_mcp_error(&config.name, error))
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

        let first = call_with_limits(&service, request(), &cancellation, &self.server_name).await;
        let result = match first {
            Ok(result) => result,
            Err(_error) if cancellation.is_cancelled() => return Err(AxiomError::Cancelled),
            Err(_error) if connection_closed(&service) && self.trusted_read_only => {
                (service, generation) = self
                    .connection
                    .reconnect_if_current(&service, &self.remote_name, &self.schema)
                    .await?;
                call_with_limits(&service, request(), &cancellation, &self.server_name)
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
) -> Result<rmcp::model::CallToolResult> {
    tokio::select! {
        () = cancellation.cancelled() => Err(AxiomError::Cancelled),
        result = tokio::time::timeout(MCP_REQUEST_TIMEOUT, service.call_tool(request)) => {
            result
                .map_err(|_| bounded_mcp_error(server_name, "tool call timed out"))?
                .map_err(|error| bounded_mcp_error(server_name, error))
        }
    }
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
}
