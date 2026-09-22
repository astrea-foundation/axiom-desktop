use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use axiomcli::{
    app::PermissionProfile, config::McpServerConfig, mcp::connect_tools, policy::Effect,
    tools::ToolContext,
};
use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler, ServiceExt,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ListToolsResult,
        PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool, ToolAnnotations,
    },
    service::RequestContext,
};
use serde_json::{Map, Value, json};
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio_util::sync::CancellationToken;

fn object_schema(properties: &Value) -> Arc<Map<String, Value>> {
    Arc::new(
        json!({
            "type": "object",
            "properties": properties,
            "additionalProperties": false
        })
        .as_object()
        .expect("object schema")
        .clone(),
    )
}

struct FixtureServer {
    mode: String,
    state: Option<PathBuf>,
}

impl FixtureServer {
    fn tool(&self) -> Tool {
        let drifted =
            self.mode == "schema_drift" && self.state.as_ref().is_some_and(|path| path.exists());
        let property = if drifted {
            json!({"value": {"type": "integer"}})
        } else {
            json!({"value": {"type": "string"}})
        };
        Tool::new(
            "echo",
            "fixture tool with misleading server annotations",
            object_schema(&property),
        )
        .with_annotations(ToolAnnotations::new().read_only(true).destructive(false))
    }
}

impl ServerHandler for FixtureServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let mut tools = vec![self.tool()];
        if self.mode == "duplicate" {
            tools.push(self.tool());
        }
        Ok(ListToolsResult {
            tools,
            ..Default::default()
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        match self.mode.as_str() {
            "crash_once" | "schema_drift" => {
                let state = self.state.as_ref().expect("state path");
                if !state.exists() {
                    std::fs::write(state, b"called").expect("write marker");
                    std::process::exit(86);
                }
            }
            "side_effect_crash" => {
                let state = self.state.as_ref().expect("state path");
                let count = std::fs::read_to_string(state)
                    .ok()
                    .and_then(|value| value.parse::<u32>().ok())
                    .unwrap_or(0);
                std::fs::write(state, (count + 1).to_string()).expect("write count");
                std::process::exit(87);
            }
            "delayed" => {
                if let Some(path) = &self.state {
                    let count = std::fs::read_to_string(path)
                        .ok()
                        .and_then(|value| value.parse::<u32>().ok())
                        .unwrap_or(0);
                    std::fs::write(path, (count + 1).to_string()).expect("call count");
                }
                tokio::time::sleep(Duration::from_millis(1200)).await;
            }
            "slow" => context.ct.cancelled().await,
            _ => {}
        }
        let value = request
            .arguments
            .as_ref()
            .and_then(|arguments| arguments.get("value"))
            .cloned()
            .unwrap_or(Value::Null);
        let body = if self.mode == "long" {
            "x".repeat(64 * 1024)
        } else {
            format!("echo={value}; secret=axm_1234567890abcdefghijklmnop")
        };
        Ok(CallToolResult::success(vec![ContentBlock::text(body)]).into())
    }
}

#[tokio::test]
async fn mcp_fixture_helper() -> anyhow::Result<()> {
    let Ok(mode) = std::env::var("AXIOMCLI_MCP_FIXTURE_MODE") else {
        return Ok(());
    };
    if mode == "malformed" {
        return serve_malformed_stdio().await;
    }
    let state = std::env::var_os("AXIOMCLI_MCP_FIXTURE_STATE").map(PathBuf::from);
    let server = FixtureServer { mode, state }
        .serve(rmcp::transport::stdio())
        .await?;
    server.waiting().await?;
    Ok(())
}

async fn serve_malformed_stdio() -> anyhow::Result<()> {
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut stdout = tokio::io::stdout();
    while let Some(line) = lines.next_line().await? {
        let request: Value = serde_json::from_str(&line)?;
        let Some(id) = request.get("id").cloned() else {
            continue;
        };
        let response = match request.get("method").and_then(Value::as_str) {
            Some("initialize") => json!({
                "jsonrpc":"2.0",
                "id":id,
                "result":{
                    "protocolVersion": request["params"]["protocolVersion"],
                    "capabilities":{"tools":{}},
                    "serverInfo":{"name":"malformed-fixture","version":"0.1.0"}
                }
            }),
            Some("tools/list") => json!({
                "jsonrpc":"2.0",
                "id":id,
                "result":{"tools":[{
                    "name":"echo",
                    "description":"returns a malformed result",
                    "inputSchema":{"type":"object","properties":{}}
                }]}
            }),
            Some("tools/call") => json!({
                "jsonrpc":"2.0",
                "id":id,
                "result":{"content":"this must be an array","isError":false}
            }),
            _ => json!({"jsonrpc":"2.0","id":id,"result":{}}),
        };
        stdout
            .write_all(format!("{}\n", serde_json::to_string(&response)?).as_bytes())
            .await?;
        stdout.flush().await?;
    }
    Ok(())
}

fn config(mode: &str, state: Option<&Path>, read_only: bool) -> McpServerConfig {
    let executable = std::env::current_exe().expect("test executable");
    let mut args = vec![format!("AXIOMCLI_MCP_FIXTURE_MODE={mode}")];
    if let Some(state) = state {
        args.push(format!("AXIOMCLI_MCP_FIXTURE_STATE={}", state.display()));
    }
    args.extend([
        executable.to_string_lossy().into_owned(),
        "--exact".into(),
        "mcp_fixture_helper".into(),
        "--quiet".into(),
        "--nocapture".into(),
        "--test-threads".into(),
        "1".into(),
    ]);
    McpServerConfig {
        name: "Blossom Fixture".into(),
        command: "env".into(),
        args,
        read_only_tools: read_only.then(|| "echo".into()).into_iter().collect(),
        tool_timeout_secs: axiomcli::config::default_mcp_tool_timeout_secs(),
    }
}

fn context(temp: &TempDir) -> ToolContext {
    ToolContext {
        session_id: axiomcli::app::SessionId::new(),
        cwd: temp.path().to_path_buf(),
        permission_profile: PermissionProfile::Confirm,
    }
}

#[tokio::test]
async fn discovers_real_stdio_tools_and_preserves_policy_provenance() {
    let temp = TempDir::new().expect("temp");
    let tools = connect_tools(&[config("normal", None, false)], 4096)
        .await
        .expect("connect");
    assert_eq!(tools.len(), 1);
    let tool = &tools[0];
    assert_eq!(tool.name(), "mcp__blossom_fixture__echo");
    assert!(matches!(
        tool.effects(&context(&temp), &json!({"value":"pink"}))
            .expect("effects")
            .as_slice(),
        [Effect::Mcp { server, tool, side_effecting: true }]
            if server == "Blossom Fixture" && tool == "echo"
    ));

    let result = tool
        .execute(
            &context(&temp),
            json!({"value":"pink"}),
            CancellationToken::new(),
        )
        .await
        .expect("call");
    assert!(result.success);
    assert!(result.content.contains("Blossom Fixture"));
    assert!(result.content.contains("untrusted MCP result"));
    assert!(!result.content.contains("axm_"));
    assert!(result.content.contains("[REDACTED]"));
}

#[tokio::test]
async fn trusted_read_only_call_reconnects_once_after_process_crash() {
    let temp = TempDir::new().expect("temp");
    let marker = temp.path().join("called");
    let tools = connect_tools(&[config("crash_once", Some(&marker), true)], 4096)
        .await
        .expect("connect");
    let result = tools[0]
        .execute(
            &context(&temp),
            json!({"value":"again"}),
            CancellationToken::new(),
        )
        .await
        .expect("reconnected call");
    assert!(result.content.contains("\"connection_generation\": 2"));
    assert!(marker.exists());
}

#[tokio::test]
async fn side_effecting_call_is_never_replayed_after_disconnect() {
    let temp = TempDir::new().expect("temp");
    let counter = temp.path().join("count");
    let tools = connect_tools(&[config("side_effect_crash", Some(&counter), false)], 4096)
        .await
        .expect("connect");
    let error = tools[0]
        .execute(
            &context(&temp),
            json!({"value":"once"}),
            CancellationToken::new(),
        )
        .await
        .expect_err("ambiguous side effect");
    assert!(error.to_string().contains("outcome is unknown"));
    assert_eq!(std::fs::read_to_string(counter).expect("counter"), "1");
}

#[tokio::test]
async fn schema_drift_blocks_read_only_retry() {
    let temp = TempDir::new().expect("temp");
    let marker = temp.path().join("schema-version");
    let tools = connect_tools(&[config("schema_drift", Some(&marker), true)], 4096)
        .await
        .expect("connect");
    let error = tools[0]
        .execute(
            &context(&temp),
            json!({"value":"old schema"}),
            CancellationToken::new(),
        )
        .await
        .expect_err("schema drift");
    assert!(error.to_string().contains("changed the schema"));
}

#[tokio::test]
async fn duplicate_discovery_and_namespace_collisions_fail_closed() {
    let duplicate = connect_tools(&[config("duplicate", None, false)], 4096)
        .await
        .err()
        .expect("duplicate remote name");
    assert!(duplicate.to_string().contains("duplicate tool name"));

    let mut second = config("normal", None, false);
    second.name = "Blossom-Fixture".into();
    let collision = connect_tools(&[config("normal", None, false), second], 4096)
        .await
        .err()
        .expect("sanitized collision");
    assert!(
        collision
            .to_string()
            .contains("duplicate MCP tool namespace")
    );
}

#[tokio::test]
async fn long_results_are_bounded_and_calls_cancel_promptly() {
    let temp = TempDir::new().expect("temp");
    let tools = connect_tools(&[config("long", None, true)], 1024)
        .await
        .expect("connect");
    let result = tools[0]
        .execute(
            &context(&temp),
            json!({"value":"long"}),
            CancellationToken::new(),
        )
        .await
        .expect("long call");
    assert!(result.truncated);
    assert!(result.content.len() <= 1024);

    let slow = connect_tools(&[config("slow", None, true)], 4096)
        .await
        .expect("connect");
    let cancellation = CancellationToken::new();
    let trigger = cancellation.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        trigger.cancel();
    });
    let outcome = tokio::time::timeout(
        Duration::from_secs(2),
        slow[0].execute(&context(&temp), json!({}), cancellation),
    )
    .await
    .expect("bounded cancellation");
    assert!(matches!(outcome, Err(axiomcli::AxiomError::Cancelled)));
}

#[tokio::test]
async fn malformed_protocol_results_fail_closed_without_panicking() {
    let temp = TempDir::new().expect("temp");
    let tools = connect_tools(&[config("malformed", None, true)], 4096)
        .await
        .expect("discovery is valid");
    let error = tools[0]
        .execute(&context(&temp), json!({}), CancellationToken::new())
        .await
        .expect_err("malformed tool result");
    let message = error.to_string();
    assert!(message.contains("MCP server `Blossom Fixture`"));
    assert!(message.len() < 5000);
}

#[tokio::test]
async fn per_server_deadlines_allow_slow_tools_and_never_replay_a_timeout() {
    let temp = TempDir::new().unwrap();
    for (seconds, succeeds) in [(2, true), (1, false)] {
        let marker = temp.path().join(format!("calls-{seconds}"));
        let mut cfg = config("delayed", Some(&marker), true);
        cfg.tool_timeout_secs = seconds;
        let tools = connect_tools(&[cfg], 4096).await.unwrap();
        let result = tools[0]
            .execute(&context(&temp), json!({}), CancellationToken::new())
            .await;
        if succeeds {
            assert!(result.is_ok(), "{result:?}");
        } else {
            assert!(result.unwrap_err().to_string().contains("timed out"));
        }
        assert_eq!(std::fs::read_to_string(marker).unwrap(), "1");
    }
}

#[tokio::test]
async fn invalid_deadlines_are_rejected_before_starting_a_server() {
    for seconds in [0, 86_401] {
        let mut cfg = config("normal", None, true);
        cfg.tool_timeout_secs = seconds;
        assert!(connect_tools(&[cfg], 4096).await.is_err());
    }
    let cfg: McpServerConfig = toml::from_str("name = 'fixture'\ncommand = 'fixture'").unwrap();
    assert_eq!(cfg.tool_timeout_secs, 300);
}
