use std::{path::Path, process::Stdio, time::Duration};

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
        let mut child = Command::new(env!("CARGO_BIN_EXE_axiomcli"))
            .arg("acp")
            .env("AXIOMCLI_TEST_RUNNER", runner)
            .env("XDG_CONFIG_HOME", state_path.join("config"))
            .env("XDG_DATA_HOME", state_path.join("data"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn ACP");
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
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": 1,
                "clientCapabilities": {},
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
