use std::{path::Path, process::Stdio, time::Duration};

use axiomcli::planning::{PlanArtifact, PlanState};
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

impl AcpProcess {
    fn spawn(runner: &str) -> Self {
        let state = tempfile::tempdir().expect("isolated ACP state");
        let path = state.path().to_path_buf();
        Self::spawn_inner(runner, &path, Some(state))
    }

    fn spawn_with_state(runner: &str, state: &Path) -> Self {
        Self::spawn_inner(runner, state, None)
    }

    fn spawn_inner(runner: &str, state_path: &Path, state: Option<TempDir>) -> Self {
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
        if runner == "workspace_edit" {
            command.env("AXIOM_PERMISSION_PROFILE", "workspace");
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
            "help"
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
        "params":{"sessionId":session,"prompt":[{"type":"text","text":"/permissions observe"}]}
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
    assert!(transcript.contains("Permission profile changed to observe"));
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
    assert!(help.contains("not model prompts"));
    acp.close().await;
}

#[tokio::test]
async fn workspace_edit_reaches_the_expected_acp_semantics_and_final_workspace() {
    let workspace = tempfile::tempdir().expect("workspace");
    std::fs::write(workspace.path().join("frontend.txt"), "before\n").expect("fixture");
    let state = tempfile::tempdir().expect("state");
    let mut acp = AcpProcess::spawn_inner("workspace_edit", state.path(), None);
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
    loop {
        let value = acp.read().await;
        transcript.push_str(&value.to_string());
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
    assert!(transcript.contains("DIFF"));
    assert!(transcript.contains("Updated frontend.txt through the shared turn contract."));
    acp.close().await;
}

#[tokio::test]
async fn structured_questions_map_to_acp_elicitation_and_shared_updates() {
    let workspace = tempfile::tempdir().expect("workspace");
    let mut acp = AcpProcess::spawn("questions");
    acp.initialize_with_capabilities(json!({"elicitation":{"form":{}}}))
        .await;
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
        if value["method"] == "session/update" {
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
    assert!(saw_question_update);
    assert!(saw_answer);
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
    acp.initialize().await;
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
    assert!(saw_closed_attention);
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
async fn cancellation_finishes_the_active_prompt_once() {
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
        "jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":session}
    }))
    .await;
    let response = acp.response(3).await;
    assert_eq!(response["result"]["stopReason"], "cancelled");
    acp.close().await;
}
