use std::{path::Path, process::Stdio, time::Duration};

use axiomcli::{
    app::{AppCommand, Origin, PermissionProfile, Runtime, SessionId, ThinkingLevel},
    planning::{PlanArtifact, PlanState},
    session::{SessionStore, model_for_resume, thinking_for_resume},
};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::{
    io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader, Lines},
    process::{Child, ChildStdin, ChildStdout, Command},
};

struct AcpProcess {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: Lines<BufReader<ChildStdout>>,
    _state: Option<TempDir>,
}

async fn seed_session_settings(
    store: &SessionStore,
    cwd: &Path,
    model: &str,
    thinking: ThinkingLevel,
) -> SessionId {
    let runtime = Runtime::new(32);
    let session_id = runtime.session_id();
    let created = runtime
        .dispatch(AppCommand::CreateSession {
            session_id: session_id.clone(),
            cwd: cwd.to_path_buf(),
            origin: Origin::Acp,
            profile: PermissionProfile::Confirm,
        })
        .await
        .expect("create seeded session");
    store.append_all(&created).expect("persist seeded session");
    let settings = runtime
        .dispatch(AppCommand::ChangeModelSettings {
            session_id: session_id.clone(),
            model: model.into(),
            thinking,
            reset_security: false,
        })
        .await
        .expect("seed model settings");
    store
        .append_all(&settings)
        .expect("persist seeded model settings");
    session_id
}

impl AcpProcess {
    fn spawn(runner: &str) -> Self {
        let state = tempfile::tempdir().expect("isolated ACP state");
        let path = state.path().to_path_buf();
        Self::spawn_inner(runner, &path, Some(state), None, None)
    }

    fn spawn_with_state(runner: &str, state: &Path) -> Self {
        Self::spawn_inner(runner, state, None, None, None)
    }

    fn spawn_with_persistence_failure(runner: &str, scope: &str) -> Self {
        let state = tempfile::tempdir().expect("isolated ACP state");
        let path = state.path().to_path_buf();
        Self::spawn_inner(runner, &path, Some(state), None, Some(scope))
    }

    fn spawn_desktop_without_provider() -> Self {
        Self::spawn_desktop_with_provider_base("https://127.0.0.1:1")
    }

    fn spawn_desktop_with_provider_base(base_url: &str) -> Self {
        let state = tempfile::tempdir().expect("isolated desktop ACP state");
        let path = state.path().to_path_buf();
        Self::spawn_inner(
            &format!("unreachable-provider:{base_url}"),
            &path,
            Some(state),
            Some("desktop-chat"),
            None,
        )
    }

    fn spawn_desktop_with_blocked_auth(base_url: &str) -> Self {
        let state = tempfile::tempdir().expect("isolated desktop ACP state");
        let path = state.path().to_path_buf();
        Self::spawn_inner(
            &format!("blocked-auth:{base_url}"),
            &path,
            Some(state),
            Some("desktop-chat"),
            None,
        )
    }

    fn spawn_inner(
        runner: &str,
        state_path: &Path,
        state: Option<TempDir>,
        frontend: Option<&str>,
        persistence_failure: Option<&str>,
    ) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_axiomcli"));
        command
            .arg("acp")
            .env("AXIOMCLI_TEST_RUNNER", runner)
            .env("XDG_CONFIG_HOME", state_path.join("config"))
            .env("XDG_DATA_HOME", state_path.join("data"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(frontend) = frontend {
            command.args(["--frontend", frontend]);
        }
        if let Some(scope) = persistence_failure {
            command.env("AXIOMCLI_TEST_FAIL_PERSISTENCE", scope);
        }
        if runner == "workspace_edit" {
            command.env("AXIOM_PERMISSION_PROFILE", "full_access");
        }
        if let Some(base_url) = runner.strip_prefix("unreachable-provider:") {
            command.env("AXIOM_BASE_URL", base_url);
        }
        if let Some(base_url) = runner.strip_prefix("blocked-auth:") {
            command
                .env("AXIOM_BASE_URL", base_url)
                .env("AXIOM_API_KEY", "axm_0123456789abcdef");
        }
        let mut child = command.spawn().expect("spawn ACP");
        let stdin = child.stdin.take().expect("stdin");
        let stdout = BufReader::new(child.stdout.take().expect("stdout")).lines();
        Self {
            child,
            stdin: Some(stdin),
            stdout,
            _state: state,
        }
    }

    async fn send(&mut self, value: &Value) {
        let stdin = self.stdin.as_mut().expect("open stdin");
        stdin
            .write_all(format!("{value}\n").as_bytes())
            .await
            .expect("write ACP message");
        stdin.flush().await.expect("flush ACP message");
    }

    async fn send_raw(&mut self, value: &str) {
        let stdin = self.stdin.as_mut().expect("open stdin");
        stdin.write_all(value.as_bytes()).await.expect("write raw");
        stdin.write_all(b"\n").await.expect("newline");
        stdin.flush().await.expect("flush raw");
    }

    async fn read(&mut self) -> Value {
        let line = tokio::time::timeout(Duration::from_secs(5), self.stdout.next_line())
            .await
            .expect("ACP output timeout")
            .expect("read ACP output")
            .expect("unexpected ACP EOF");
        serde_json::from_str(&line)
            .unwrap_or_else(|error| panic!("ACP stdout was not pure JSON: {error}: {line:?}"))
    }

    async fn response(&mut self, id: i64) -> Value {
        loop {
            let value = self.read().await;
            if value.get("id").and_then(Value::as_i64) == Some(id) {
                return value;
            }
        }
    }

    async fn initialize(&mut self) {
        self.initialize_with_capabilities(json!({})).await;
    }

    async fn initialize_extension(&mut self) {
        self.initialize_with_capabilities(json!({
            "_meta": {
                "axiom": {
                    "protocolVersion": "0.2",
                    "features": {
                        "desktopChat": 1,
                        "threadCatalog": 1,
                        "timeline": 2,
                        "modelCatalog": 1,
                        "profilePreferences": 1,
                        "collections": 1,
                        "account": 2,
                        "billing": 2,
                        "usage": 1,
                        "securityEvidence": 4,
                        "compaction": 1,
                        "activity": 1
                    }
                }
            },
            "elicitation": {"form": {}}
        }))
        .await;
    }

    async fn initialize_with_capabilities(&mut self, capabilities: Value) {
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": 1,
                "clientCapabilities": capabilities,
                "clientInfo": {"name": "axiomcli-conformance", "version": "1"}
            }
        }))
        .await;
        let response = self.response(1).await;
        assert_eq!(response["result"]["protocolVersion"], 1);
        assert_eq!(response["result"]["agentCapabilities"]["loadSession"], true);
    }

    async fn new_session(&mut self, id: i64, cwd: &Path) -> String {
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "session/new",
            "params": {"cwd": cwd, "mcpServers": []}
        }))
        .await;
        self.response(id).await["result"]["sessionId"]
            .as_str()
            .expect("session ID")
            .to_owned()
    }

    async fn close(mut self) {
        drop(self.stdin.take());
        let status = tokio::time::timeout(Duration::from_secs(5), self.child.wait())
            .await
            .expect("ACP EOF shutdown timeout")
            .expect("wait ACP");
        assert!(status.success());
    }
}

#[tokio::test]
async fn request_failure_returns_an_error_and_keeps_the_connection_usable() {
    let mut acp = AcpProcess::spawn("echo");
    acp.initialize_extension().await;

    acp.send(&json!({
        "jsonrpc":"2.0","id":2,"method":"_axiom/thread/timeline",
        "params":{"threadId":"not-a-session-id","limit":10}
    }))
    .await;
    let failed = acp.response(2).await;
    assert!(failed.get("error").is_some());
    assert!(failed.get("result").is_none());

    acp.send(&json!({
        "jsonrpc":"2.0","id":3,"method":"_axiom/thread/list","params":{}
    }))
    .await;
    let following = acp.response(3).await;
    assert_eq!(following["result"]["threads"], json!([]));
    acp.close().await;
}

#[tokio::test]
async fn desktop_bootstrap_is_local_and_does_not_require_provider_or_auth() {
    let mut acp = AcpProcess::spawn_desktop_without_provider();
    acp.initialize_extension().await;

    acp.send(&json!({
        "jsonrpc":"2.0","id":2,"method":"_axiom/desktop/bootstrap","params":{}
    }))
    .await;
    let bootstrap = acp.response(2).await;
    assert!(bootstrap.get("error").is_none(), "{bootstrap}");
    assert_eq!(bootstrap["result"]["frontend"], "desktop-chat");
    assert_eq!(bootstrap["result"]["permissionProfile"], "web");
    assert_eq!(bootstrap["result"]["newThreadSettings"]["model"], "auto");
    assert_eq!(
        bootstrap["result"]["newThreadSettings"]["thinkingLevel"],
        "medium"
    );
    assert!(
        Path::new(
            bootstrap["result"]["chatCwd"]
                .as_str()
                .expect("desktop chat cwd")
        )
        .is_absolute()
    );
    acp.close().await;
}

#[tokio::test]
async fn blocked_external_request_does_not_block_local_requests_and_is_cancellable() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("blocked provider listener");
    let provider_url = format!(
        "https://{}",
        listener.local_addr().expect("listener address")
    );
    let mut acp = AcpProcess::spawn_desktop_with_blocked_auth(&provider_url);
    acp.initialize_extension().await;

    acp.send(&json!({
        "jsonrpc":"2.0","id":2,"method":"_axiom/account/status","params":{}
    }))
    .await;
    let (_blocked_connection, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept())
        .await
        .expect("provider request was not started")
        .expect("accept provider request");

    acp.send(&json!({
        "jsonrpc":"2.0","id":3,"method":"_axiom/thread/list","params":{}
    }))
    .await;
    let local = acp.response(3).await;
    assert_eq!(local["result"]["threads"], json!([]));

    acp.send(&json!({
        "jsonrpc":"2.0","method":"$/cancel_request","params":{"requestId":2}
    }))
    .await;
    let cancelled = acp.response(2).await;
    assert!(cancelled.get("error").is_some(), "{cancelled}");
    assert_eq!(cancelled["error"]["code"], -32800, "{cancelled}");
    acp.close().await;
}

#[tokio::test]
async fn security_warmup_requires_negotiation_and_never_creates_a_thread() {
    let mut unnegotiated = AcpProcess::spawn("echo");
    unnegotiated.initialize().await;
    unnegotiated
        .send(
            &json!({"jsonrpc":"2.0","id":2,"method":"_axiom/security/prewarm",
        "params":{"modelId":"test-model"}}),
        )
        .await;
    assert!(unnegotiated.response(2).await.get("error").is_some());
    unnegotiated.close().await;

    let mut acp = AcpProcess::spawn("echo");
    acp.initialize_extension().await;
    acp.send(
        &json!({"jsonrpc":"2.0","id":2,"method":"_axiom/security/prewarm",
        "params":{"modelId":"test-model"}}),
    )
    .await;
    let response = acp.response(2).await;
    assert_eq!(
        response["result"]["status"]["state"], "unattested_development",
        "{response}"
    );
    assert!(
        response["result"]["evidence"].is_null(),
        "test transport cannot manufacture verified evidence"
    );
    acp.send(&json!({"jsonrpc":"2.0","id":3,"method":"_axiom/thread/list","params":{}}))
        .await;
    assert_eq!(acp.response(3).await["result"]["threads"], json!([]));
    acp.close().await;
}

#[tokio::test]
async fn axiom_extension_negotiates_and_exposes_typed_settings_security_and_compaction() {
    let workspace = tempfile::tempdir().expect("workspace");
    let mut acp = AcpProcess::spawn("echo");
    acp.initialize_extension().await;
    let session = acp.new_session(2, workspace.path()).await;

    acp.send(&json!({
        "jsonrpc":"2.0","id":3,"method":"_axiom/models/list","params":{}
    }))
    .await;
    let models = acp.response(3).await;
    assert_eq!(models["result"]["models"][0]["id"], "alternate-model");
    assert_eq!(models["result"]["models"][0]["providerId"], "axiom");
    assert_eq!(models["result"]["models"][0]["providerLabel"], "Axiom");
    assert_eq!(
        models["result"]["models"][0]["shortLabel"],
        "alternate-model"
    );
    assert_eq!(models["result"]["models"][0]["contextWindowTokens"], 8192);
    assert_eq!(models["result"]["models"][0]["maxOutputTokens"], 2048);
    assert_eq!(
        models["result"]["models"][0]["inputPriceMicrousdPerMillionTokens"],
        440_000
    );
    assert_eq!(
        models["result"]["models"][0]["outputPriceMicrousdPerMillionTokens"],
        1_320_000
    );
    assert_eq!(
        models["result"]["models"][0]["autoCompactThresholdTokens"],
        6963
    );

    acp.send(&json!({
        "jsonrpc":"2.0","id":4,"method":"session/set_config_option",
        "params":{"sessionId":session,"configId":"model","value":"alternate-model"}
    }))
    .await;
    assert!(acp.response(4).await.get("result").is_some());

    acp.send(&json!({
        "jsonrpc":"2.0","id":5,"method":"session/set_config_option",
        "params":{"sessionId":session,"configId":"thinking","value":"high"}
    }))
    .await;
    assert!(acp.response(5).await.get("result").is_some());

    acp.send(&json!({
        "jsonrpc":"2.0","id":6,"method":"session/set_mode",
        "params":{"sessionId":session,"modeId":"observe"}
    }))
    .await;
    assert!(acp.response(6).await.get("result").is_some());

    acp.send(&json!({
        "jsonrpc":"2.0","id":7,"method":"_axiom/profile/preferences","params":{}
    }))
    .await;
    let preferences = acp.response(7).await;
    assert_eq!(
        preferences["result"]["preferences"]["model"],
        "alternate-model"
    );
    assert_eq!(
        preferences["result"]["preferences"]["thinkingLevel"],
        "high"
    );

    acp.send(&json!({
        "jsonrpc":"2.0","id":8,"method":"_axiom/thread/timeline",
        "params":{"threadId":session,"limit":100}
    }))
    .await;
    let state = acp.response(8).await;
    assert_eq!(state["result"]["thread"]["lifecycle"], "ready");
    assert_eq!(state["result"]["thread"]["profile"], "observe");

    acp.send(&json!({
        "jsonrpc":"2.0","id":9,"method":"_axiom/security/verify",
        "params":{"threadId":session}
    }))
    .await;
    let security = acp.response(9).await;
    assert_eq!(
        security["result"]["status"]["state"],
        "unattested_development"
    );
    assert!(security["result"]["evidence"].is_null());

    acp.send(&json!({
        "jsonrpc":"2.0","id":10,"method":"_axiom/compaction/start",
        "params":{"threadId":session,"focus":"extension coverage"}
    }))
    .await;
    let compacted = acp.response(10).await;
    assert_eq!(compacted["result"]["messagesBefore"], 2);

    acp.send(&json!({
        "jsonrpc":"2.0","id":11,"method":"session/prompt",
        "params":{"sessionId":session,"prompt":[{"type":"text","text":"desktop title notification"}]}
    }))
    .await;
    let mut saw_title_notification = false;
    loop {
        let value = acp.read().await;
        saw_title_notification |= value.to_string().contains("title_changed")
            || value.to_string().contains("session_info_update");
        if value.get("id").and_then(Value::as_i64) == Some(11) {
            break;
        }
    }
    assert!(
        saw_title_notification,
        "initial titles must be published before the prompt completes"
    );
    acp.close().await;
}

#[tokio::test]
async fn generated_title_arrives_after_the_first_prompt_and_survives_reload() {
    let workspace = tempfile::tempdir().unwrap();
    let mut acp = AcpProcess::spawn("echo");
    acp.initialize_extension().await;
    let session = acp.new_session(2, workspace.path()).await;
    acp.send(&json!({"jsonrpc":"2.0","id":3,"method":"session/prompt",
        "params":{"sessionId":session,"prompt":[{"type":"text","text":"Please fix my login redirect"}]}})).await;
    let mut completed = false;
    let mut saw_fallback = false;
    loop {
        let value = acp.read().await;
        if value["id"] == 3 {
            assert_eq!(value["result"]["stopReason"], "end_turn");
            completed = true;
        }
        if value["params"]["update"]["sessionUpdate"] == "session_info_update" {
            let title = value["params"]["update"]["title"].as_str().unwrap();
            if title == "Please fix my login redirect" {
                saw_fallback = true;
            }
            if title == "Generated conversation title" {
                assert!(saw_fallback);
                assert!(
                    completed,
                    "title work must outlive a successfully completed prompt"
                );
                break;
            }
        }
    }
    acp.send(&json!({"jsonrpc":"2.0","id":4,"method":"_axiom/thread/list","params":{"limit":20}}))
        .await;
    let listed = acp.response(4).await;
    let thread = listed["result"]["threads"]
        .as_array()
        .unwrap()
        .iter()
        .find(|thread| thread["threadId"] == session)
        .unwrap();
    assert_eq!(thread["title"], "Generated conversation title");
    acp.close().await;
}

#[tokio::test]
async fn model_changes_reconcile_disjoint_reasoning_capabilities_atomically() {
    let workspace = tempfile::tempdir().expect("workspace");
    let mut acp = AcpProcess::spawn("echo");
    acp.initialize_extension().await;
    let session = acp.new_session(2, workspace.path()).await;

    for (id, config_id, value) in [(3, "model", "alternate-model"), (4, "thinking", "xhigh")] {
        acp.send(&json!({
            "jsonrpc":"2.0","id":id,"method":"session/set_config_option",
            "params":{"sessionId":session,"configId":config_id,"value":value}
        }))
        .await;
        assert!(acp.response(id).await.get("result").is_some());
    }

    acp.send(&json!({
        "jsonrpc":"2.0","id":5,"method":"session/set_config_option",
        "params":{"sessionId":session,"configId":"model","value":"grok-code"}
    }))
    .await;
    let mut reset_security = false;
    let nonreasoning = loop {
        let value = acp.read().await;
        if value["method"] == "_axiom/event"
            && value["params"]["event"]["kind"] == "security_changed"
            && value["params"]["event"]["status"]["state"] == "unverified"
        {
            reset_security = true;
        }
        if value.get("id").and_then(Value::as_i64) == Some(5) {
            break value;
        }
    };
    assert!(
        reset_security,
        "a different model must invalidate the prior model-bound security status"
    );
    let options = nonreasoning["result"]["configOptions"]
        .as_array()
        .expect("configuration options");
    assert!(options.iter().all(|option| option["id"] != "thinking"));

    acp.send(&json!({
        "jsonrpc":"2.0","id":6,"method":"_axiom/profile/preferences","params":{}
    }))
    .await;
    let retained = acp.response(6).await;
    assert_eq!(retained["result"]["preferences"]["model"], "grok-code");
    assert_eq!(
        retained["result"]["preferences"]["thinkingLevel"],
        "provider_default"
    );

    acp.send(&json!({
        "jsonrpc":"2.0","id":7,"method":"session/set_config_option",
        "params":{"sessionId":session,"configId":"model","value":"medium-only"}
    }))
    .await;
    let reconciled = acp.response(7).await;
    let thinking = reconciled["result"]["configOptions"]
        .as_array()
        .and_then(|options| options.iter().find(|option| option["id"] == "thinking"))
        .expect("reasoning-capable model exposes thinking");
    assert_eq!(thinking["currentValue"], "medium");
    assert_eq!(
        thinking["options"].as_array().map(Vec::len),
        Some(1),
        "only supported values are advertised"
    );

    acp.send(&json!({
        "jsonrpc":"2.0","id":8,"method":"_axiom/thread/timeline",
        "params":{"threadId":session,"limit":100}
    }))
    .await;
    let thread = acp.response(8).await;
    assert_eq!(thread["result"]["thread"]["selectedModel"], "medium-only");
    assert_eq!(thread["result"]["thread"]["thinkingLevel"], "medium");

    acp.send(&json!({
        "jsonrpc":"2.0","id":9,"method":"_axiom/profile/preferences","params":{}
    }))
    .await;
    let preferences = acp.response(9).await;
    assert_eq!(preferences["result"]["preferences"]["model"], "medium-only");
    assert_eq!(
        preferences["result"]["preferences"]["thinkingLevel"],
        "medium"
    );
    acp.close().await;
}

#[tokio::test]
async fn config_persistence_failure_rolls_live_settings_back_before_next_request() {
    let workspace = tempfile::tempdir().expect("workspace");
    let mut acp = AcpProcess::spawn_with_persistence_failure("echo", "config");
    acp.initialize_extension().await;
    let session = acp.new_session(2, workspace.path()).await;

    acp.send(&json!({
        "jsonrpc":"2.0","id":3,"method":"session/set_config_option",
        "params":{"sessionId":session,"configId":"thinking","value":"high"}
    }))
    .await;
    assert!(acp.response(3).await.get("error").is_some());

    acp.send(&json!({
        "jsonrpc":"2.0","id":4,"method":"session/set_config_option",
        "params":{"sessionId":session,"configId":"model","value":"alternate-model"}
    }))
    .await;
    let following = acp.response(4).await;
    let thinking = following["result"]["configOptions"]
        .as_array()
        .and_then(|options| options.iter().find(|option| option["id"] == "thinking"))
        .expect("thinking config after rollback");
    assert_eq!(thinking["currentValue"], "medium", "{following}");

    acp.send(&json!({
        "jsonrpc":"2.0","id":5,"method":"_axiom/profile/preferences","params":{}
    }))
    .await;
    assert_eq!(
        acp.response(5).await["result"]["preferences"]["thinkingLevel"],
        "medium"
    );
    acp.close().await;
}

#[tokio::test]
async fn slash_thinking_persistence_failure_rolls_back_before_standard_config() {
    let workspace = tempfile::tempdir().expect("workspace");
    let mut acp = AcpProcess::spawn_with_persistence_failure("echo", "slash-thinking");
    acp.initialize().await;
    let session = acp.new_session(2, workspace.path()).await;

    acp.send(&json!({
        "jsonrpc":"2.0","id":3,"method":"session/prompt",
        "params":{"sessionId":session,"prompt":[{"type":"text","text":"/thinking high"}]}
    }))
    .await;
    assert!(acp.response(3).await.get("error").is_some());

    acp.send(&json!({
        "jsonrpc":"2.0","id":4,"method":"session/set_config_option",
        "params":{"sessionId":session,"configId":"model","value":"alternate-model"}
    }))
    .await;
    let following = acp.response(4).await;
    let thinking = following["result"]["configOptions"]
        .as_array()
        .and_then(|options| options.iter().find(|option| option["id"] == "thinking"))
        .expect("thinking config after slash rollback");
    assert_eq!(thinking["currentValue"], "medium", "{following}");
    acp.close().await;
}

#[tokio::test]
async fn mode_persistence_failure_restores_the_permission_used_by_the_next_turn() {
    let workspace = tempfile::tempdir().expect("workspace");
    std::fs::write(workspace.path().join("frontend.txt"), "before\n").expect("fixture");
    let mut acp = AcpProcess::spawn_with_persistence_failure("workspace_edit", "mode");
    acp.initialize().await;
    let session = acp.new_session(2, workspace.path()).await;

    acp.send(&json!({
        "jsonrpc":"2.0","id":3,"method":"session/set_mode",
        "params":{"sessionId":session,"modeId":"observe"}
    }))
    .await;
    assert!(acp.response(3).await.get("error").is_some());

    acp.send(&json!({
        "jsonrpc":"2.0","id":4,"method":"session/prompt",
        "params":{"sessionId":session,"prompt":[{"type":"text","text":"edit fixture"}]}
    }))
    .await;
    assert_eq!(acp.response(4).await["result"]["stopReason"], "end_turn");
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("frontend.txt")).expect("result"),
        "after\n"
    );
    acp.close().await;
}

#[tokio::test]
async fn new_acp_sessions_reconcile_stale_catalog_preferences_before_use() {
    let workspace = tempfile::tempdir().expect("workspace");
    let state = tempfile::tempdir().expect("shared state");
    let database = state
        .path()
        .join("data")
        .join("axiom")
        .join("accounts")
        .join("local-test-account")
        .join("cli")
        .join("state.sqlite3");

    {
        let store = SessionStore::open(&database).expect("seed durable preferences");
        store
            .set_profile_preferences("retired-model", ThinkingLevel::High)
            .expect("retired preference");
    }
    let mut retired = AcpProcess::spawn_with_state("echo", state.path());
    retired.initialize_extension().await;
    retired
        .send(&json!({
            "jsonrpc":"2.0","id":2,"method":"session/new",
            "params":{"cwd":workspace.path(),"mcpServers":[]}
        }))
        .await;
    let created = retired.response(2).await;
    let options = created["result"]["configOptions"]
        .as_array()
        .expect("new-session config options");
    assert!(
        options.iter().any(|option| {
            option["id"] == "model" && option["currentValue"] == "alternate-model"
        })
    );
    retired
        .send(&json!({
            "jsonrpc":"2.0","id":3,"method":"_axiom/profile/preferences","params":{}
        }))
        .await;
    let corrected = retired.response(3).await;
    assert_eq!(
        corrected["result"]["preferences"]["model"],
        "alternate-model"
    );
    assert_eq!(corrected["result"]["preferences"]["thinkingLevel"], "high");
    retired.close().await;

    {
        let store = SessionStore::open(&database).expect("reopen durable preferences");
        store
            .set_profile_preferences("medium-only", ThinkingLevel::ExtraHigh)
            .expect("stale reasoning preference");
    }
    let mut changed_reasoning = AcpProcess::spawn_with_state("echo", state.path());
    changed_reasoning.initialize_extension().await;
    changed_reasoning
        .send(&json!({
            "jsonrpc":"2.0","id":2,"method":"session/new",
            "params":{"cwd":workspace.path(),"mcpServers":[]}
        }))
        .await;
    let created = changed_reasoning.response(2).await;
    let thinking = created["result"]["configOptions"]
        .as_array()
        .and_then(|options| options.iter().find(|option| option["id"] == "thinking"))
        .expect("reasoning config option");
    assert_eq!(thinking["currentValue"], "medium");
    assert_eq!(thinking["options"].as_array().map(Vec::len), Some(1));
    changed_reasoning
        .send(&json!({
            "jsonrpc":"2.0","id":3,"method":"_axiom/profile/preferences","params":{}
        }))
        .await;
    let corrected = changed_reasoning.response(3).await;
    assert_eq!(
        corrected["result"]["preferences"]["thinkingLevel"],
        "medium"
    );
    changed_reasoning.close().await;
}

#[tokio::test]
async fn axiom_extension_is_silent_until_negotiated_and_rejects_custom_requests() {
    let mut acp = AcpProcess::spawn("echo");
    acp.initialize().await;
    acp.send(&json!({
        "jsonrpc":"2.0","id":2,"method":"_axiom/models/list","params":{}
    }))
    .await;
    let response = acp.response(2).await;
    assert_eq!(response["error"]["code"], -32601);
    assert_eq!(response["error"]["data"]["code"], "unsupported_feature");
    acp.close().await;
}

#[tokio::test]
async fn gift_redemption_requires_billing_three_and_an_interactive_account() {
    for version in [2, 3] {
        let mut acp = AcpProcess::spawn("echo");
        acp.initialize_with_capabilities(json!({"_meta": {"axiom": {
            "protocolVersion": "0.2", "features": {"billing": version}
        }}}))
        .await;
        acp.send(
            &json!({"jsonrpc":"2.0", "id":2, "method":"_axiom/billing/redeem_gift_code",
            "params":{"code":"AXG-AAAA-AAAA-AAAA-AAAA-AAAA-AAAA-AAAA-AAAA"}}),
        )
        .await;
        let response = acp.response(2).await;
        assert!(response.get("error").is_some());
        if version == 2 {
            assert_eq!(response["error"]["code"], -32601);
        } else {
            assert_ne!(response["error"]["code"], -32601);
        }
        assert!(!response.to_string().contains("AXG-AAAA"));
        acp.close().await;
    }
}

#[tokio::test]
async fn axiom_extension_negotiates_each_feature_independently() {
    let mut acp = AcpProcess::spawn("echo");
    acp.initialize_with_capabilities(json!({
        "_meta": {
            "axiom": {
                "protocolVersion": "0.2",
                "features": {
                    "desktopChat": 0,
                    "threadCatalog": 1,
                    "timeline": 0,
                    "modelCatalog": 0,
                    "profilePreferences": 0,
                    "collections": 0,
                    "account": 0,
                    "billing": 1,
                    "securityEvidence": 0,
                    "compaction": 0,
                    "activity": 0
                }
            }
        }
    }))
    .await;

    acp.send(&json!({
        "jsonrpc":"2.0","id":2,"method":"_axiom/thread/list","params":{}
    }))
    .await;
    assert!(acp.response(2).await.get("result").is_some());

    acp.send(&json!({
        "jsonrpc":"2.0","id":3,"method":"_axiom/models/list","params":{}
    }))
    .await;
    let rejected = acp.response(3).await;
    assert_eq!(rejected["error"]["code"], -32601);
    assert_eq!(rejected["error"]["data"]["code"], "unsupported_feature");

    acp.send(&json!({
        "jsonrpc":"2.0","id":4,"method":"_axiom/billing/status","params":{}
    }))
    .await;
    let rejected_billing = acp.response(4).await;
    assert_eq!(rejected_billing["error"]["code"], -32601);
    assert_eq!(
        rejected_billing["error"]["data"]["code"],
        "unsupported_feature"
    );
    acp.close().await;
}

#[tokio::test]
async fn billing_extension_requires_native_account_and_rejects_removed_invoice_methods() {
    let mut acp = AcpProcess::spawn("echo");
    acp.initialize_extension().await;

    acp.send(&json!({
        "jsonrpc":"2.0","id":2,"method":"_axiom/billing/status","params":{}
    }))
    .await;
    let signed_out = acp.response(2).await;
    assert!(signed_out.get("error").is_some(), "{signed_out}");
    assert!(!signed_out.to_string().contains("axm_"), "{signed_out}");

    acp.send(&json!({
        "jsonrpc":"2.0","id":3,"method":"_axiom/billing/invoice_create",
        "params":{"amountUsdCents":0,"clientRequestId":"../bad"}
    }))
    .await;
    let invalid_create = acp.response(3).await;
    assert_eq!(invalid_create["error"]["code"], -32601, "{invalid_create}");

    acp.send(&json!({
        "jsonrpc":"2.0","id":4,"method":"_axiom/billing/invoice_status",
        "params":{"invoiceId":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"}
    }))
    .await;
    let invalid_id = acp.response(4).await;
    assert_eq!(invalid_id["error"]["code"], -32601, "{invalid_id}");

    acp.send(&json!({
        "jsonrpc":"2.0","id":5,"method":"_axiom/thread/list","params":{}
    }))
    .await;
    assert!(acp.response(5).await.get("result").is_some());
    acp.close().await;
}

#[tokio::test]
async fn usage_extension_requires_negotiation_and_a_native_account() {
    let mut acp = AcpProcess::spawn("echo");
    acp.initialize().await;
    acp.send(&json!({"jsonrpc":"2.0","id":2,"method":"_axiom/usage/summary","params":{}}))
        .await;
    assert_eq!(
        acp.response(2).await["error"]["data"]["code"],
        "unsupported_feature"
    );
    acp.close().await;

    let mut acp = AcpProcess::spawn("echo");
    acp.initialize_extension().await;
    acp.send(&json!({"jsonrpc":"2.0","id":2,"method":"_axiom/usage/summary","params":{}}))
        .await;
    let response = acp.response(2).await;
    assert!(response.get("error").is_some(), "{response}");
    assert_ne!(response["error"]["data"]["code"], "unsupported_feature");
    acp.close().await;
}

#[tokio::test]
async fn axiom_extension_lists_and_deletes_durable_sessions_with_single_use_confirmation() {
    let workspace = tempfile::tempdir().expect("workspace");
    let state = tempfile::tempdir().expect("shared state");
    let session = {
        let mut first = AcpProcess::spawn_with_state("echo", state.path());
        first.initialize_extension().await;
        let session = first.new_session(2, workspace.path()).await;
        first
            .send(&json!({
                "jsonrpc":"2.0","id":3,"method":"session/prompt",
                "params":{"sessionId":session,"prompt":[{"type":"text","text":"catalog title fixture"}]}
            }))
            .await;
        assert_eq!(first.response(3).await["result"]["stopReason"], "end_turn");
        first.close().await;
        session
    };

    let mut second = AcpProcess::spawn_with_state("echo", state.path());
    second.initialize_extension().await;
    second
        .send(&json!({
            "jsonrpc":"2.0","id":2,"method":"_axiom/thread/list",
            "params":{"limit":20}
        }))
        .await;
    let listed = second.response(2).await;
    assert!(
        listed["result"]["threads"]
            .as_array()
            .is_some_and(|threads| threads.iter().any(|item| item["threadId"] == session))
    );

    second
        .send(&json!({
            "jsonrpc":"2.0","id":3,"method":"_axiom/thread/delete_preview",
            "params":{"threadIds":[session]}
        }))
        .await;
    let preview = second.response(3).await;
    let token = preview["result"]["confirmationToken"]
        .as_str()
        .expect("confirmation token")
        .to_owned();
    second
        .send(&json!({
            "jsonrpc":"2.0","id":4,"method":"_axiom/thread/delete_confirm",
            "params":{"confirmationToken":token,"threadIds":[session]}
        }))
        .await;
    assert_eq!(second.response(4).await["result"]["deleted"], 1);

    second
        .send(&json!({
            "jsonrpc":"2.0","id":5,"method":"_axiom/thread/delete_confirm",
            "params":{"confirmationToken":token,"threadIds":[session]}
        }))
        .await;
    assert!(second.response(5).await.get("error").is_some());
    second.close().await;
}

#[tokio::test]
async fn resumed_acp_sessions_restore_model_thinking_and_permissions() {
    let workspace = tempfile::tempdir().expect("workspace");
    let state = tempfile::tempdir().expect("shared state");
    let session = {
        let mut first = AcpProcess::spawn_with_state("echo", state.path());
        first.initialize_extension().await;
        let session = first.new_session(2, workspace.path()).await;
        first
            .send(&json!({
                "jsonrpc":"2.0","id":3,"method":"session/set_config_option",
                "params":{"sessionId":session,"configId":"model","value":"alternate-model"}
            }))
            .await;
        assert!(first.response(3).await.get("result").is_some());
        first
            .send(&json!({
                "jsonrpc":"2.0","id":4,"method":"session/set_config_option",
                "params":{"sessionId":session,"configId":"thinking","value":"xhigh"}
            }))
            .await;
        assert!(first.response(4).await.get("result").is_some());
        first
            .send(&json!({
                "jsonrpc":"2.0","id":5,"method":"session/set_mode",
                "params":{"sessionId":session,"modeId":"full_access"}
            }))
            .await;
        assert!(first.response(5).await.get("result").is_some());
        first.close().await;
        session
    };

    let mut resumed = AcpProcess::spawn_with_state("echo", state.path());
    resumed.initialize_extension().await;
    resumed
        .send(&json!({
            "jsonrpc":"2.0","id":2,"method":"session/load",
            "params":{"sessionId":session,"cwd":workspace.path(),"mcpServers":[]}
        }))
        .await;
    let loaded = resumed.response(2).await;
    assert_eq!(loaded["result"]["modes"]["currentModeId"], "full_access");
    let options = loaded["result"]["configOptions"]
        .as_array()
        .expect("config options");
    assert!(
        options.iter().any(|option| {
            option["id"] == "model" && option["currentValue"] == "alternate-model"
        })
    );
    assert!(
        options
            .iter()
            .any(|option| { option["id"] == "thinking" && option["currentValue"] == "xhigh" })
    );
    resumed.close().await;
}

#[tokio::test]
async fn resumed_acp_sessions_retain_retired_models_and_reconcile_changed_reasoning_support() {
    let workspace = tempfile::tempdir().expect("workspace");
    let state = tempfile::tempdir().expect("shared state");
    let database = state
        .path()
        .join("data")
        .join("axiom")
        .join("accounts")
        .join("local-test-account")
        .join("cli")
        .join("state.sqlite3");
    let (retired_id, changed_reasoning_id) = {
        let store = SessionStore::open(&database).expect("state store");
        let retired = seed_session_settings(
            &store,
            workspace.path(),
            "retired-model",
            ThinkingLevel::High,
        )
        .await;
        let changed_reasoning = seed_session_settings(
            &store,
            workspace.path(),
            "medium-only",
            ThinkingLevel::ExtraHigh,
        )
        .await;
        (retired, changed_reasoning)
    };

    let mut acp = AcpProcess::spawn_with_state("echo", state.path());
    acp.initialize_extension().await;
    for (request_id, session_id, expected_model, expected_thinking) in [
        (2, &retired_id, "retired-model", None),
        (3, &changed_reasoning_id, "medium-only", Some("medium")),
    ] {
        acp.send(&json!({
            "jsonrpc":"2.0","id":request_id,"method":"session/load",
            "params":{
                "sessionId":session_id.to_string(),
                "cwd":workspace.path(),
                "mcpServers":[]
            }
        }))
        .await;
        let loaded = acp.response(request_id).await;
        let options = loaded["result"]["configOptions"]
            .as_array()
            .expect("load config options");
        assert!(
            options.iter().any(|option| {
                option["id"] == "model" && option["currentValue"] == expected_model
            })
        );
        if let Some(expected_thinking) = expected_thinking {
            assert!(options.iter().any(|option| {
                option["id"] == "thinking" && option["currentValue"] == expected_thinking
            }));
        } else {
            assert!(options.iter().all(|option| option["id"] != "thinking"));
        }
    }
    acp.close().await;

    let store = SessionStore::open(&database).expect("reopen preserved state");
    let retired = store
        .load_recovering(&retired_id)
        .expect("load retired-model correction");
    assert_eq!(
        model_for_resume(&retired.events, "fallback"),
        "retired-model"
    );
    assert_eq!(
        thinking_for_resume(&retired.events, ThinkingLevel::Medium),
        ThinkingLevel::High
    );
    let changed_reasoning = store
        .load_recovering(&changed_reasoning_id)
        .expect("load reasoning correction");
    assert_eq!(
        model_for_resume(&changed_reasoning.events, "fallback"),
        "medium-only"
    );
    assert_eq!(
        thinking_for_resume(&changed_reasoning.events, ThinkingLevel::ExtraHigh),
        ThinkingLevel::Medium
    );
    drop(store);
    let mut replacement = AcpProcess::spawn_with_state("echo", state.path());
    replacement.initialize_extension().await;
    replacement
        .send(&json!({"jsonrpc":"2.0","id":2,"method":"session/load",
        "params":{"sessionId":retired_id.to_string(),"cwd":workspace.path(),"mcpServers":[]}}))
        .await;
    assert!(replacement.response(2).await.get("result").is_some());
    replacement.send(&json!({"jsonrpc":"2.0","id":3,"method":"session/set_config_option",
        "params":{"sessionId":retired_id.to_string(),"configId":"model","value":"alternate-model"}})).await;
    let selected = replacement.response(3).await;
    assert!(selected.get("error").is_none(), "{selected}");
    replacement.close().await;
}

#[tokio::test]
async fn slash_commands_are_advertised_and_execute_outside_model_turns() {
    let workspace = tempfile::tempdir().expect("workspace");
    let mut acp = AcpProcess::spawn("echo");
    acp.initialize().await;
    let session = acp.new_session(2, workspace.path()).await;
    let update = acp.read().await;
    assert_eq!(update["method"], "session/update");
    let commands = update["params"]["update"]["availableCommands"]
        .as_array()
        .expect("available commands");
    let names = commands
        .iter()
        .filter_map(|command| command["name"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        vec![
            "permissions",
            "compact",
            "model",
            "resume",
            "delete",
            "thinking",
            "login",
            "logout",
            "account",
            "security",
            "refresh",
            "help",
            "balance",
            "topup",
            "redeem",
            "update"
        ]
    );
    let model_command = commands
        .iter()
        .find(|command| command["name"] == "model")
        .expect("model command");
    assert!(
        model_command.get("input").is_none(),
        "/model must not advertise a typed ID argument"
    );
    let permissions_command = commands
        .iter()
        .find(|command| command["name"] == "permissions")
        .expect("permissions command");
    assert!(
        permissions_command.get("input").is_none(),
        "/permissions must not advertise a typed profile argument"
    );
    let resume_command = commands
        .iter()
        .find(|command| command["name"] == "resume")
        .expect("resume command");
    assert!(
        resume_command.get("input").is_none(),
        "/resume must not advertise a typed argument"
    );
    let delete_command = commands
        .iter()
        .find(|command| command["name"] == "delete")
        .expect("delete command");
    assert!(
        delete_command.get("input").is_none(),
        "/delete must not advertise a typed argument"
    );

    acp.send(&json!({
        "jsonrpc":"2.0","id":3,"method":"session/prompt",
        "params":{"sessionId":session,"prompt":[{"type":"text","text":"/permissions"}]}
    }))
    .await;
    let mut transcript = String::new();
    loop {
        let value = acp.read().await;
        transcript.push_str(&value.to_string());
        if value.get("id").and_then(Value::as_i64) == Some(3) {
            assert_eq!(value["result"]["stopReason"], "end_turn");
            break;
        }
    }
    assert!(transcript.contains("session mode selector"));
    assert!(transcript.contains("session mode selector to change Tools permissions"));
    assert!(!transcript.contains("Echo:"));

    acp.send(&json!({
        "jsonrpc":"2.0","id":4,"method":"session/prompt",
        "params":{"sessionId":session,"prompt":[{"type":"text","text":"/model"}]}
    }))
    .await;
    let mut model_picker = String::new();
    loop {
        let value = acp.read().await;
        model_picker.push_str(&value.to_string());
        if value.get("id").and_then(Value::as_i64) == Some(4) {
            assert_eq!(value["result"]["stopReason"], "end_turn");
            break;
        }
    }
    assert!(model_picker.contains("configOptions"));
    assert!(model_picker.contains("alternate-model"));
    assert!(model_picker.contains("Choose a model using the client model selector"));

    acp.send(&json!({
        "jsonrpc":"2.0","id":5,"method":"session/set_config_option",
        "params":{"sessionId":session,"configId":"model","value":"alternate-model"}
    }))
    .await;
    let selected = acp.response(5).await;
    assert_eq!(
        selected["result"]["configOptions"][0]["currentValue"],
        "alternate-model"
    );

    acp.send(&json!({
        "jsonrpc":"2.0","id":6,"method":"session/prompt",
        "params":{"sessionId":session,"prompt":[{"type":"text","text":"/model arbitrary-id"}]}
    }))
    .await;
    let mut invalid_model = String::new();
    loop {
        let value = acp.read().await;
        invalid_model.push_str(&value.to_string());
        if value.get("id").and_then(Value::as_i64) == Some(6) {
            break;
        }
    }
    assert!(invalid_model.contains("Command error"));
    assert!(invalid_model.contains("/model"));

    acp.send(&json!({
        "jsonrpc":"2.0","id":7,"method":"session/prompt",
        "params":{"sessionId":session,"prompt":[{"type":"text","text":"/help"}]}
    }))
    .await;
    let mut help = String::new();
    loop {
        let value = acp.read().await;
        help.push_str(&value.to_string());
        if value.get("id").and_then(Value::as_i64) == Some(7) {
            break;
        }
    }
    assert!(help.contains("/compact"));
    assert!(help.contains("/refresh"));
    assert!(help.contains("not model prompts"));

    acp.send(&json!({
        "jsonrpc":"2.0","id":8,"method":"session/prompt",
        "params":{"sessionId":session,"prompt":[{"type":"text","text":"/refresh"}]}
    }))
    .await;
    let mut refreshed = String::new();
    loop {
        let value = acp.read().await;
        refreshed.push_str(&value.to_string());
        if value.get("id").and_then(Value::as_i64) == Some(8) {
            break;
        }
    }
    assert!(refreshed.contains("Attestation refreshed"));
    assert!(!refreshed.contains("Echo:"));
    acp.send(&json!({"jsonrpc":"2.0","id":9,"method":"session/prompt",
        "params":{"sessionId":session,"prompt":[{"type":"text","text":"/update"}]}}))
        .await;
    let mut update_help = String::new();
    loop {
        let value = acp.read().await;
        update_help.push_str(&value.to_string());
        if value.get("id").and_then(Value::as_i64) == Some(9) {
            assert_eq!(value["result"]["stopReason"], "end_turn");
            break;
        }
    }
    assert!(update_help.contains("axiomcli update"));
    assert!(!update_help.contains("Echo:"));
    acp.close().await;
}

#[tokio::test]
async fn workspace_edit_reaches_the_expected_acp_semantics_and_final_workspace() {
    let workspace = tempfile::tempdir().expect("workspace");
    std::fs::write(workspace.path().join("frontend.txt"), "before\n").expect("fixture");
    let state = tempfile::tempdir().expect("state");
    let mut acp = AcpProcess::spawn_inner("workspace_edit", state.path(), None, None, None);
    // This fixture exercises a pre-authorized shared turn; it still rejects
    // profiles without workspace write authority.
    acp.initialize().await;
    let session = acp.new_session(2, workspace.path()).await;
    acp.send(&json!({
        "jsonrpc":"2.0","id":3,"method":"session/prompt",
        "params":{"sessionId":session,"prompt":[{"type":"text","text":"edit fixture"}]}
    }))
    .await;
    let mut transcript = String::new();
    let mut updates = Vec::new();
    loop {
        let value = acp.read().await;
        transcript.push_str(&value.to_string());
        if value["method"] == "session/update" {
            updates.push(value["params"]["update"].clone());
        }
        if value.get("id").and_then(Value::as_i64) == Some(3) {
            assert_eq!(value["result"]["stopReason"], "end_turn");
            break;
        }
    }
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("frontend.txt")).expect("result"),
        "after\n"
    );
    assert!(transcript.contains("replace_text"));
    assert!(transcript.contains("frontend-edit"));
    let diff_update = updates
        .iter()
        .find(|update| {
            update["sessionUpdate"] == "tool_call_update"
                && update["toolCallId"] == "frontend-edit"
                && update["content"][0]["type"] == "diff"
        })
        .expect("workspace edit diff update");
    let content = diff_update["content"].as_array().expect("diff content");
    assert_eq!(content.len(), 1, "one edited file");
    let diff = &content[0];
    assert_eq!(diff["oldText"], "before\n");
    assert_eq!(diff["newText"], "after\n");
    let locations = diff_update["locations"].as_array().expect("diff locations");
    assert_eq!(locations.len(), 1, "one edited file location");
    // Decode JSON escapes (including Windows separators) before normalizing
    // filesystem aliases such as macOS /var and /private/var.
    let expected_path = workspace
        .path()
        .join("frontend.txt")
        .canonicalize()
        .expect("canonical edited file");
    for (kind, path) in [("diff", &diff["path"]), ("location", &locations[0]["path"])] {
        let path = Path::new(path.as_str().expect("file path"));
        assert!(path.is_absolute(), "{kind} path must be absolute: {path:?}");
        assert_eq!(
            path.canonicalize().expect("canonical ACP file path"),
            expected_path,
            "{kind} path must identify the edited file"
        );
    }
    assert!(transcript.contains("Updated frontend.txt through the shared turn contract."));
    acp.close().await;
}

#[tokio::test]
async fn structured_questions_map_to_acp_elicitation_and_shared_updates() {
    let workspace = tempfile::tempdir().expect("workspace");
    let mut acp = AcpProcess::spawn("questions");
    acp.initialize_extension().await;
    let session = acp.new_session(2, workspace.path()).await;
    acp.send(&json!({
        "jsonrpc":"2.0","id":3,"method":"session/prompt",
        "params":{"sessionId":session,"prompt":[{"type":"text","text":"ask me"}]}
    }))
    .await;
    let mut saw_question_update = false;
    let mut saw_answer = false;
    loop {
        let value = acp.read().await;
        if value["method"] == "session/update"
            && value["params"]["update"]["sessionUpdate"] == "agent_thought_chunk"
        {
            saw_question_update |= value.to_string().contains("structured question");
            saw_answer |= value.to_string().contains("cherry");
        }
        if value["method"] == "elicitation/create" {
            let id = value["id"].clone();
            assert_eq!(
                value["params"]["requestedSchema"]["required"]
                    .as_array()
                    .expect("required")
                    .len(),
                2
            );
            acp.send(&json!({
                "jsonrpc":"2.0","id":id,
                "result":{"action":"accept","content":{"theme":"cherry","note":"keep it compact"}}
            }))
            .await;
        }
        if value.get("id").and_then(Value::as_i64) == Some(3) {
            assert_eq!(value["result"]["stopReason"], "end_turn");
            break;
        }
    }
    assert!(
        !saw_question_update && !saw_answer,
        "question state must not be duplicated as standard transcript content"
    );
    acp.send(&json!({
        "jsonrpc":"2.0","id":4,"method":"_axiom/thread/timeline",
        "params":{"threadId":session,"limit":100}
    }))
    .await;
    let timeline = acp.response(4).await;
    assert!(
        timeline["result"]["items"]
            .as_array()
            .is_some_and(|items| items.iter().any(|item| {
                item["kind"] == "notice"
                    && item["status"] == "completed"
                    && item["metadata"].to_string().contains("cherry")
            })),
        "structured answers must be durable in the authoritative timeline"
    );
    acp.close().await;
}

#[tokio::test]
async fn approval_maps_all_narrow_choices_and_resumes_the_acp_turn() {
    let workspace = tempfile::tempdir().expect("workspace");
    let mut acp = AcpProcess::spawn("approval");
    acp.initialize().await;
    let session = acp.new_session(2, workspace.path()).await;
    acp.send(&json!({
        "jsonrpc":"2.0","id":3,"method":"session/prompt",
        "params":{"sessionId":session,"prompt":[{"type":"text","text":"request approval"}]}
    }))
    .await;
    let mut saw_result = false;
    loop {
        let value = acp.read().await;
        if value["method"] == "session/request_permission" {
            let options = value["params"]["options"]
                .as_array()
                .expect("permission options");
            let ids = options
                .iter()
                .filter_map(|option| option["optionId"].as_str())
                .collect::<Vec<_>>();
            assert_eq!(
                ids,
                vec![
                    "allow_once",
                    "allow_exact_session",
                    "allow_prefix_session",
                    "reject_once"
                ]
            );
            acp.send(&json!({
                "jsonrpc":"2.0","id":value["id"],
                "result":{"outcome":{"outcome":"selected","optionId":"allow_once"}}
            }))
            .await;
        }
        if value["method"] == "session/update" {
            saw_result |= value.to_string().contains("Approval response: allow_once");
        }
        if value.get("id").and_then(Value::as_i64) == Some(3) {
            assert_eq!(value["result"]["stopReason"], "end_turn");
            break;
        }
    }
    assert!(saw_result);
    acp.close().await;
}

#[tokio::test]
async fn unsupported_structured_questions_close_cleanly_instead_of_sticking() {
    let workspace = tempfile::tempdir().expect("workspace");
    let mut acp = AcpProcess::spawn("questions");
    acp.initialize_with_capabilities(json!({
        "_meta": {
            "axiom": {
                "protocolVersion": "0.2",
                "features": {
                    "desktopChat": 0,
                    "threadCatalog": 0,
                    "timeline": 2,
                    "modelCatalog": 0,
                    "profilePreferences": 0,
                    "collections": 0,
                    "account": 0,
                    "billing": 1,
                    "securityEvidence": 0,
                    "compaction": 0,
                    "activity": 0
                }
            }
        }
    }))
    .await;
    let session = acp.new_session(2, workspace.path()).await;
    acp.send(&json!({
        "jsonrpc":"2.0","id":3,"method":"session/prompt",
        "params":{"sessionId":session,"prompt":[{"type":"text","text":"ask me"}]}
    }))
    .await;
    let mut saw_closed_attention = false;
    loop {
        let value = acp.read().await;
        if value["method"] == "session/update" {
            saw_closed_attention |= value.to_string().contains("closed without answers");
        }
        if value.get("id").and_then(Value::as_i64) == Some(3) {
            assert_eq!(value["result"]["stopReason"], "end_turn");
            break;
        }
    }
    assert!(
        !saw_closed_attention,
        "failed question state must not be duplicated as standard transcript content"
    );
    acp.send(&json!({
        "jsonrpc":"2.0","id":4,"method":"_axiom/thread/timeline",
        "params":{"threadId":session,"limit":100}
    }))
    .await;
    let timeline = acp.response(4).await;
    assert!(
        timeline["result"]["items"]
            .as_array()
            .is_some_and(|items| items.iter().any(|item| {
                item["kind"] == "notice"
                    && item["status"] == "failed"
                    && item["metadata"]
                        .to_string()
                        .contains("structured elicitation support")
            })),
        "unsupported elicitation must close the durable notice with a reason"
    );
    acp.send(&json!({
        "jsonrpc":"2.0","id":5,"method":"session/prompt",
        "params":{"sessionId":session,"prompt":[{"type":"text","text":"ask again"}]}
    }))
    .await;
    loop {
        let value = acp.read().await;
        if value.get("id").and_then(Value::as_i64) == Some(5) {
            assert_eq!(value["result"]["stopReason"], "end_turn", "{value}");
            break;
        }
    }
    acp.close().await;
}

#[tokio::test]
async fn plan_review_uses_standard_acp_plan_updates_and_persists_line_comments() {
    let workspace = tempfile::tempdir().expect("workspace");
    let mut acp = AcpProcess::spawn("plan_review");
    acp.initialize_with_capabilities(json!({"elicitation":{"form":{}}}))
        .await;
    let session = acp.new_session(2, workspace.path()).await;
    acp.send(&json!({
        "jsonrpc":"2.0","id":3,"method":"session/prompt",
        "params":{"sessionId":session,"prompt":[{"type":"text","text":"review plan"}]}
    }))
    .await;
    let mut plan_updates = 0;
    loop {
        let value = acp.read().await;
        if value["method"] == "session/update"
            && value["params"]["update"]["sessionUpdate"] == "plan"
        {
            plan_updates += 1;
            assert!(
                value["params"]["update"]["entries"]
                    .as_array()
                    .is_some_and(|entries| !entries.is_empty())
            );
        }
        if value["method"] == "elicitation/create" {
            let id = value["id"].clone();
            acp.send(&json!({
                "jsonrpc":"2.0","id":id,
                "result":{"action":"accept","content":{
                    "decision":"request_revision",
                    "line_range":"4",
                    "comment":"Add a verification step"
                }}
            }))
            .await;
        }
        if value.get("id").and_then(Value::as_i64) == Some(3) {
            assert_eq!(value["result"]["stopReason"], "end_turn");
            break;
        }
    }
    assert_eq!(
        plan_updates, 2,
        "proposal and review must both use ACP plans"
    );
    let plans = PlanArtifact::list(workspace.path()).expect("persisted plans");
    assert_eq!(plans.len(), 1);
    assert_eq!(plans[0].state, PlanState::RevisionRequested);
    assert_eq!(plans[0].comments[0].start_line, 4);
    assert_eq!(plans[0].comments[0].text, "Add a verification step");
    acp.close().await;
}

#[tokio::test]
async fn plan_approval_does_not_require_revision_only_fields() {
    let workspace = tempfile::tempdir().expect("workspace");
    let mut acp = AcpProcess::spawn("plan_review");
    acp.initialize_with_capabilities(json!({"elicitation":{"form":{}}}))
        .await;
    let session = acp.new_session(2, workspace.path()).await;
    acp.send(&json!({
        "jsonrpc":"2.0","id":3,"method":"session/prompt",
        "params":{"sessionId":session,"prompt":[{"type":"text","text":"approve plan"}]}
    }))
    .await;
    loop {
        let value = acp.read().await;
        if value["method"] == "elicitation/create" {
            let required = value["params"]["requestedSchema"]["required"]
                .as_array()
                .expect("required fields");
            assert_eq!(required, &[json!("decision")]);
            acp.send(&json!({
                "jsonrpc":"2.0","id":value["id"],
                "result":{"action":"accept","content":{"decision":"approve"}}
            }))
            .await;
        }
        if value.get("id").and_then(Value::as_i64) == Some(3) {
            assert_eq!(value["result"]["stopReason"], "end_turn");
            break;
        }
    }
    let plans = PlanArtifact::list(workspace.path()).expect("persisted plans");
    assert_eq!(plans.len(), 1);
    assert_eq!(plans[0].state, PlanState::Approved);
    assert!(plans[0].comments.is_empty());
    acp.close().await;
}

#[tokio::test]
async fn durable_session_load_keeps_identity_replays_history_and_continues() {
    let workspace = tempfile::tempdir().expect("workspace");
    let state = tempfile::tempdir().expect("shared state");
    let session = {
        let mut first = AcpProcess::spawn_with_state("echo", state.path());
        first.initialize().await;
        let session = first.new_session(2, workspace.path()).await;
        first
            .send(&json!({
                "jsonrpc":"2.0","id":3,"method":"session/prompt",
                "params":{"sessionId":session,"prompt":[{"type":"text","text":"persist me"}]}
            }))
            .await;
        let response = first.response(3).await;
        assert_eq!(response["result"]["stopReason"], "end_turn");
        first.close().await;
        session
    };

    let mut resumed = AcpProcess::spawn_with_state("echo", state.path());
    resumed.initialize().await;
    resumed
        .send(&json!({
            "jsonrpc":"2.0","id":2,"method":"session/load",
            "params":{"sessionId":session,"cwd":workspace.path(),"mcpServers":[]}
        }))
        .await;
    let mut replayed = false;
    loop {
        let value = resumed.read().await;
        replayed |= value["method"] == "session/update" && value.to_string().contains("persist me");
        if value.get("id").and_then(Value::as_i64) == Some(2) {
            assert!(value.get("result").is_some(), "load failed: {value}");
            break;
        }
    }
    assert!(replayed, "load must replay journaled transcript updates");
    resumed
        .send(&json!({
            "jsonrpc":"2.0","id":3,"method":"session/prompt",
            "params":{"sessionId":session,"prompt":[{"type":"text","text":"continue"}]}
        }))
        .await;
    assert_eq!(
        resumed.response(3).await["result"]["stopReason"],
        "end_turn"
    );
    resumed.close().await;
}

#[tokio::test]
async fn concurrent_loads_share_a_canonical_in_flight_reservation() {
    let workspace = tempfile::tempdir().expect("workspace");
    let state = tempfile::tempdir().expect("shared state");
    let session = {
        let mut first = AcpProcess::spawn_with_state("echo", state.path());
        first.initialize().await;
        let session = first.new_session(2, workspace.path()).await;
        first.close().await;
        session
    };

    let mut acp = AcpProcess::spawn_with_state("echo", state.path());
    acp.initialize().await;
    for (id, session_id) in [(2, session.clone()), (3, session.to_uppercase())] {
        acp.send(&json!({
            "jsonrpc":"2.0","id":id,"method":"session/load",
            "params":{"sessionId":session_id,"cwd":workspace.path(),"mcpServers":[]}
        }))
        .await;
    }
    let mut responses = Vec::new();
    while responses.len() < 2 {
        let value = acp.read().await;
        if matches!(value.get("id").and_then(Value::as_i64), Some(2 | 3)) {
            responses.push(value);
        }
    }
    assert_eq!(
        responses
            .iter()
            .filter(|value| value.get("result").is_some())
            .count(),
        1,
        "{responses:?}"
    );
    assert_eq!(
        responses
            .iter()
            .filter(|value| value.get("error").is_some())
            .count(),
        1,
        "{responses:?}"
    );
    acp.close().await;
}

#[tokio::test]
async fn creates_prompts_streams_and_exits_with_protocol_only_stdout() {
    let workspace = tempfile::tempdir().expect("workspace");
    let mut acp = AcpProcess::spawn("echo");
    acp.initialize().await;
    let session = acp.new_session(2, workspace.path()).await;
    acp.send(&json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "session/prompt",
        "params": {
            "sessionId": session,
            "prompt": [{"type": "text", "text": "contract probe"}]
        }
    }))
    .await;
    let mut saw_update = false;
    loop {
        let value = acp.read().await;
        saw_update |= value["method"] == "session/update";
        if value.get("id").and_then(Value::as_i64) == Some(3) {
            assert_eq!(value["result"]["stopReason"], "end_turn");
            break;
        }
    }
    assert!(saw_update);
    acp.close().await;
}

#[tokio::test]
async fn malformed_unknown_and_duplicate_ids_are_bounded() {
    let workspace = tempfile::tempdir().expect("workspace");
    let mut acp = AcpProcess::spawn("echo");
    // A malformed frame must not become a request or contaminate stdout. The
    // SDK may return a parse error or discard it; the next valid request must
    // still complete.
    acp.send_raw("{not-json").await;
    acp.send(&json!({"jsonrpc":"2.0","id":7,"method":"unknown/method","params":{}}))
        .await;
    let unknown = acp.response(7).await;
    assert_eq!(unknown["error"]["code"], -32601);
    acp.initialize().await;

    acp.send(&json!({"jsonrpc":"2.0","id":9,"method":"session/new","params":{"cwd":workspace.path(),"mcpServers":[]}})).await;
    acp.send(&json!({"jsonrpc":"2.0","id":9,"method":"session/new","params":{"cwd":workspace.path(),"mcpServers":[]}})).await;
    let first = acp.response(9).await;
    let second = acp.response(9).await;
    assert!(first.get("result").is_some());
    assert!(second.get("result").is_some() || second.get("error").is_some());
    acp.close().await;
}

#[tokio::test]
async fn cancelled_permission_response_cancels_the_prompt_without_a_notification() {
    let workspace = tempfile::tempdir().expect("workspace");
    let mut acp = AcpProcess::spawn("approval");
    acp.initialize().await;
    let session = acp.new_session(2, workspace.path()).await;
    acp.send(&json!({
        "jsonrpc":"2.0","id":3,"method":"session/prompt",
        "params":{"sessionId":session,"prompt":[{"type":"text","text":"wait for approval"}]}
    }))
    .await;
    loop {
        let value = acp.read().await;
        if value["method"] == "session/request_permission" {
            acp.send(&json!({
                "jsonrpc":"2.0","id":value["id"],
                "result":{"outcome":{"outcome":"cancelled"}}
            }))
            .await;
            break;
        }
    }
    assert_eq!(acp.response(3).await["result"]["stopReason"], "cancelled");
    acp.close().await;
}

#[tokio::test]
async fn cancellation_finishes_the_active_prompt_once() {
    let workspace = tempfile::tempdir().expect("workspace");
    let mut acp = AcpProcess::spawn("blocking");
    acp.initialize().await;
    let session = acp.new_session(2, workspace.path()).await;
    for request_id in 3..11 {
        acp.send(&json!({
            "jsonrpc":"2.0","id":request_id,"method":"session/prompt",
            "params":{"sessionId":session,"prompt":[{"type":"text","text":"wait"}]}
        }))
        .await;
        acp.send(&json!({
            "jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":session}
        }))
        .await;
        let response = acp.response(request_id).await;
        assert_eq!(response["result"]["stopReason"], "cancelled", "{response}");
    }
    acp.close().await;
}

#[tokio::test]
async fn active_prompt_rejects_config_mutation_until_owner_cleanup() {
    let workspace = tempfile::tempdir().expect("workspace");
    let mut acp = AcpProcess::spawn("blocking");
    acp.initialize().await;
    let session = acp.new_session(2, workspace.path()).await;
    acp.send(&json!({
        "jsonrpc":"2.0","id":3,"method":"session/prompt",
        "params":{"sessionId":session,"prompt":[{"type":"text","text":"wait"}]}
    }))
    .await;
    acp.send(&json!({
        "jsonrpc":"2.0","id":4,"method":"session/set_config_option",
        "params":{"sessionId":session,"configId":"thinking","value":"high"}
    }))
    .await;
    let busy = acp.response(4).await;
    assert!(busy.get("error").is_some(), "{busy}");
    acp.send(&json!({
        "jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":session}
    }))
    .await;
    assert_eq!(acp.response(3).await["result"]["stopReason"], "cancelled");
    acp.close().await;
}

#[tokio::test]
async fn delete_confirmation_rechecks_active_work_after_preview() {
    let workspace = tempfile::tempdir().expect("workspace");
    let mut acp = AcpProcess::spawn("blocking");
    acp.initialize_extension().await;
    let session = acp.new_session(2, workspace.path()).await;

    acp.send(&json!({
        "jsonrpc":"2.0","id":3,"method":"_axiom/thread/delete_preview",
        "params":{"threadIds":[session]}
    }))
    .await;
    let preview = acp.response(3).await;
    let token = preview["result"]["confirmationToken"]
        .as_str()
        .expect("confirmation token")
        .to_owned();

    acp.send(&json!({
        "jsonrpc":"2.0","id":4,"method":"session/prompt",
        "params":{"sessionId":session,"prompt":[{"type":"text","text":"wait"}]}
    }))
    .await;
    acp.send(&json!({
        "jsonrpc":"2.0","id":5,"method":"_axiom/thread/delete_confirm",
        "params":{"confirmationToken":token,"threadIds":[session]}
    }))
    .await;
    let rejected = acp.response(5).await;
    assert!(rejected.get("error").is_some(), "{rejected}");

    acp.send(&json!({
        "jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":session}
    }))
    .await;
    assert_eq!(acp.response(4).await["result"]["stopReason"], "cancelled");
    acp.close().await;
}

#[tokio::test]
async fn protocol_level_request_cancellation_stops_the_active_prompt() {
    let workspace = tempfile::tempdir().expect("workspace");
    let mut acp = AcpProcess::spawn("blocking");
    acp.initialize().await;
    let session = acp.new_session(2, workspace.path()).await;
    acp.send(&json!({
        "jsonrpc":"2.0","id":3,"method":"session/prompt",
        "params":{"sessionId":session,"prompt":[{"type":"text","text":"wait"}]}
    }))
    .await;
    acp.send(&json!({
        "jsonrpc":"2.0","method":"$/cancel_request","params":{"requestId":3}
    }))
    .await;
    let response = acp.response(3).await;
    assert_eq!(response["result"]["stopReason"], "cancelled");
    acp.close().await;
}
