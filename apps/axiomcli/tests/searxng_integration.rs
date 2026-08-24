use std::{
    collections::HashSet,
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use axiomcli::{
    Result,
    agent::{APP_EVENT_QUEUE_CAPACITY, AgentEngine, TurnContext, TurnRunner},
    app::{AppEvent, PermissionProfile, SessionId, TurnId},
    provider::{
        AssistantTurn, ChatRole, FunctionCall, InferenceProvider, InferenceRequest, ProviderEvent,
        ToolCall,
    },
    tools::default_registry,
    web::{SearchProvider, SearxngClient},
};
use tokio::sync::{Mutex, mpsc};
use tokio_util::sync::CancellationToken;

struct SearxngService {
    child: Option<Child>,
    endpoint: String,
}

impl SearxngService {
    async fn start(settings: &str) -> Self {
        let project = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("workspace root");
        let listener = TcpListener::bind("127.0.0.1:0").expect("reserve port");
        let port = listener.local_addr().expect("local address").port();
        drop(listener);
        let endpoint = format!("http://127.0.0.1:{port}");
        let child = Command::new(project.join("scripts/searxng-dev"))
            .arg("run")
            .env(
                "AXIOMCLI_SEARXNG_SETTINGS",
                project.join("dev/searxng").join(settings),
            )
            .env("AXIOMCLI_SEARXNG_PORT", port.to_string())
            .env("AXIOMCLI_SEARXNG_URL", &endpoint)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start real SearXNG");
        let mut service = Self {
            child: Some(child),
            endpoint,
        };
        let client = reqwest::Client::new();
        for _ in 0..120 {
            if service
                .child
                .as_mut()
                .expect("child")
                .try_wait()
                .expect("poll service")
                .is_some()
            {
                panic!("real SearXNG exited during startup for {settings}");
            }
            if client
                .get(format!("{}/", service.endpoint))
                .timeout(Duration::from_millis(250))
                .send()
                .await
                .is_ok_and(|response| response.status().is_success())
            {
                return service;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        service.stop();
        panic!("real SearXNG did not become healthy for {settings}");
    }

    fn client(&self) -> SearxngClient {
        SearxngClient::new(&self.endpoint, Duration::from_secs(20), 20).expect("SearXNG client")
    }

    fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for SearxngService {
    fn drop(&mut self) {
        self.stop();
    }
}

struct CitationProvider {
    step: Mutex<usize>,
}

#[async_trait]
impl InferenceProvider for CitationProvider {
    async fn stream(
        &self,
        request: InferenceRequest,
        events: mpsc::Sender<ProviderEvent>,
        _cancellation: CancellationToken,
    ) -> Result<AssistantTurn> {
        let mut step = self.step.lock().await;
        *step += 1;
        if *step == 1 {
            return Ok(AssistantTurn {
                text: String::new(),
                tool_calls: vec![ToolCall {
                    id: "real-search".into(),
                    kind: "function".into(),
                    function: FunctionCall {
                        name: "web_search".into(),
                        arguments: serde_json::json!({"query":"Rust programming language"})
                            .to_string(),
                    },
                }],
            });
        }
        let tool_content = request
            .messages
            .iter()
            .rev()
            .find(|message| message.role == ChatRole::Tool)
            .map(|message| message.content.as_str())
            .expect("real search tool result");
        let start = tool_content.find("https://").expect("source URL");
        let tail = &tool_content[start..];
        let end = tail.find(['"', '\n', ' ']).unwrap_or(tail.len());
        let citation = &tail[..end];
        let text = format!("Research complete. Source: {citation}");
        events
            .send(ProviderEvent::TextDelta(text.clone()))
            .await
            .expect("event receiver");
        Ok(AssistantTurn {
            text,
            tool_calls: Vec::new(),
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "starts the pinned real SearXNG development service and uses real upstream engines"]
async fn real_service_lifecycle_failure_modes_and_agent_citations() {
    let mut normal = SearxngService::start("settings.yml").await;
    let report = normal
        .client()
        .search("Rust programming language", CancellationToken::new())
        .await
        .expect("real upstream search");
    assert!(
        !report.results.is_empty(),
        "real engines returned no results"
    );
    let mut normalized = HashSet::new();
    for result in &report.results {
        assert!(matches!(result.url.scheme(), "http" | "https"));
        assert!(normalized.insert(result.url.to_string()));
        assert!(!result.source.is_empty());
    }

    let workspace = tempfile::tempdir().expect("research workspace");
    let provider = Arc::new(CitationProvider {
        step: Mutex::new(0),
    });
    let engine = AgentEngine::new(
        provider,
        Arc::new(default_registry(32 * 1024, &normal.endpoint).expect("tools")),
        "citation-fixture",
        4,
    );
    let (events_tx, mut events_rx) = mpsc::channel(APP_EVENT_QUEUE_CAPACITY);
    engine
        .run(
            TurnContext {
                session_id: SessionId::new(),
                turn_id: TurnId::new(),
                cwd: PathBuf::from(workspace.path()),
                permission_profile: PermissionProfile::FullAccess,
                approval: None,
                questions: None,
            },
            "Research Rust and cite a source".into(),
            events_tx,
            CancellationToken::new(),
        )
        .await
        .expect("research agent");
    let mut final_text = String::new();
    let mut proposed_tools = Vec::new();
    while let Ok(event) = events_rx.try_recv() {
        match event {
            AppEvent::TextDelta { text, .. } => final_text.push_str(&text),
            AppEvent::ToolProposed { name, .. } => proposed_tools.push(name),
            _ => {}
        }
    }
    assert!(final_text.contains("Source: https://"));
    assert_eq!(proposed_tools, vec!["web_search"]);

    let stopped_client = normal.client();
    normal.stop();
    assert!(
        stopped_client
            .search("after shutdown", CancellationToken::new())
            .await
            .is_err()
    );

    let empty = SearxngService::start("settings-empty.yml").await;
    let report = empty
        .client()
        .search("deliberately empty", CancellationToken::new())
        .await
        .expect("real empty engine");
    assert!(report.results.is_empty());
    assert!(
        report
            .warnings
            .iter()
            .any(|warning| warning.contains("no results"))
    );
    drop(empty);

    let disabled = SearxngService::start("settings-disabled.yml").await;
    let report = disabled
        .client()
        .search("disabled engine", CancellationToken::new())
        .await
        .expect("real disabled configuration");
    assert!(report.results.is_empty());
    assert!(!report.warnings.is_empty());
    drop(disabled);

    let timeout = SearxngService::start("settings-timeout.yml").await;
    let report = timeout
        .client()
        .search("timeout engine", CancellationToken::new())
        .await
        .expect("real timeout response");
    assert!(report.results.is_empty());
    assert!(
        report
            .warnings
            .iter()
            .any(|warning| warning.to_ascii_lowercase().contains("timeout")),
        "warnings: {:?}",
        report.warnings
    );
}
