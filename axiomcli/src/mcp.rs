use std::{collections::HashSet, process::Stdio, sync::Arc};

use async_trait::async_trait;
use rmcp::{
    RoleClient, ServiceExt as _,
    model::{CallToolRequestParams, JsonObject},
    service::RunningService,
    transport::TokioChildProcess,
};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::{
    AxiomError, Result,
    config::McpServerConfig,
    policy::Effect,
    tools::{Tool, ToolContext, ToolResult},
};

type McpService = RunningService<RoleClient, ()>;

pub async fn connect_tools(
    configs: &[McpServerConfig],
    max_output_bytes: usize,
) -> Result<Vec<Arc<dyn Tool>>> {
    let mut adapters: Vec<Arc<dyn Tool>> = Vec::new();
    let mut public_names = HashSet::new();
    for config in configs {
        validate_server_config(config)?;
        let service = Arc::new(connect(config).await?);
        let tools = service
            .list_all_tools()
            .await
            .map_err(|error| mcp_error(&config.name, error))?;
        for tool in tools {
            let remote_name = tool.name.into_owned();
            let public_name = format!(
                "mcp__{}__{}",
                sanitize_name(&config.name),
                sanitize_name(&remote_name)
            );
            if !public_names.insert(public_name.clone()) {
                return Err(AxiomError::Protocol(format!(
                    "duplicate MCP tool namespace `{public_name}`"
                )));
            }
            let schema = Value::Object((*tool.input_schema).clone());
            let description = format!(
                "MCP {} / {}: {}",
                config.name,
                remote_name,
                tool.description.as_deref().unwrap_or("No description")
            );
            adapters.push(Arc::new(McpTool {
                public_name,
                remote_name: remote_name.clone(),
                server_name: config.name.clone(),
                description,
                schema,
                trusted_read_only: config
                    .read_only_tools
                    .iter()
                    .any(|name| name == &remote_name),
                service: service.clone(),
                max_output_bytes: max_output_bytes.max(1024),
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
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|error| {
            AxiomError::Protocol(format!(
                "cannot start MCP server `{}`: {error}",
                config.name
            ))
        })?
        .0;
    ().serve(transport)
        .await
        .map_err(|error| mcp_error(&config.name, error))
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

struct McpTool {
    public_name: String,
    remote_name: String,
    server_name: String,
    description: String,
    schema: Value,
    trusted_read_only: bool,
    service: Arc<McpService>,
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
        let request =
            CallToolRequestParams::new(self.remote_name.clone()).with_arguments(arguments);
        let result = tokio::select! {
            () = cancellation.cancelled() => return Err(AxiomError::Cancelled),
            result = self.service.call_tool(request) => result.map_err(|error| mcp_error(&self.server_name, error))?,
        };
        let serialized = serde_json::to_string_pretty(&serde_json::json!({
            "server": self.server_name,
            "tool": self.remote_name,
            "content_trust": "untrusted MCP result",
            "result": result,
        }))?;
        let (content, truncated) = truncate(&serialized, self.max_output_bytes);
        Ok(ToolResult {
            content,
            success: result.is_error != Some(true),
            truncated,
            changed_paths: Vec::new(),
            diff: None,
            background: None,
        })
    }
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

fn mcp_error(server: &str, error: impl std::fmt::Display) -> AxiomError {
    AxiomError::Protocol(format!("MCP server `{server}`: {error}"))
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
}
