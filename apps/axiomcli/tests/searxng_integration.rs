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

struct SearchUpstream {
    state: tempfile::TempDir,
    server: tokio::task::JoinHandle<()>,
}

impl SearchUpstream {
    async fn start() -> Self {
        use axum::{Json, Router, routing::get};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let results = || async {
            Json(serde_json::json!({"results": [
                {"url":"https://www.rust-lang.org/", "title":"Rust", "content":"The Rust programming language"},
                {"url":"https://doc.rust-lang.org/book/", "title":"Rust Book", "content":"Learn Rust"}
            ]}))
        };
        let app = Router::new().route("/search", get(results)).route(
            "/slow",
            get(|| async {
                tokio::time::sleep(Duration::from_secs(2)).await;
                Json(serde_json::json!({"results": []}))
            }),
        );
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let state = tempfile::tempdir().unwrap();
        for (name, path, timeout) in [("normal", "search", 5.0), ("timeout", "slow", 0.1)] {
            std::fs::write(
                state.path().join(format!("{name}.yml")),
                format!(
                    r#"
use_default_settings:
  engines:
    keep_only: [axiom test upstream]
general:
  debug: false
  enable_metrics: false
search:
  formats: [html, json]
server:
  bind_address: "127.0.0.1"
  secret_key: "axiom-local-test-only"
  limiter: false
  public_instance: false
outgoing:
  request_timeout: {timeout}
  max_request_timeout: {timeout}
engines:
  - name: axiom test upstream
    engine: json_engine
    enable_http: true
    shortcut: axtest
    disabled: false
    search_url: "http://{address}/{path}?q={{query}}"
    results_query: results
    url_query: url
    title_query: title
    content_query: content
    timeout: {timeout}
"#
                ),
            )
            .unwrap();
        }
        Self { state, server }
    }

    fn settings(&self, name: &str) -> String {
        self.state
            .path()
            .join(format!("{name}.yml"))
            .to_str()
            .unwrap()
            .to_owned()
    }
}

impl Drop for SearchUpstream {
    fn drop(&mut self) {
        self.server.abort();
    }
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
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
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
                reasoning: None,
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
            reasoning: None,
            text,
            tool_calls: Vec::new(),
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "starts the pinned real SearXNG service with an isolated HTTP upstream"]
async fn real_service_lifecycle_failure_modes_and_agent_citations() {
    let upstream = SearchUpstream::start().await;
    let mut normal = SearxngService::start(&upstream.settings("normal")).await;
    let report = normal
        .client()
        .search("Rust programming language", CancellationToken::new())
        .await
        .expect("search through the real SearXNG service");
    assert!(
        !report.results.is_empty(),
        "the SearXNG JSON engine returned no fixture results; warnings: {:?}",
        report.warnings
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
                attachments: Vec::new(),
                session_id: SessionId::new(),
                turn_id: TurnId::new(),
                cwd: PathBuf::from(workspace.path()),
                permission_profile: PermissionProfile::FullAccess,
                web_enabled: true,
                steering: None,
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

    let timeout = SearxngService::start(&upstream.settings("timeout")).await;
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires live upstream search-engine availability"]
async fn live_upstream_search_returns_results() {
    let service = SearxngService::start("settings.yml").await;
    let report = service
        .client()
        .search("Rust programming language", CancellationToken::new())
        .await
        .expect("live upstream search");
    assert!(
        !report.results.is_empty(),
        "real engines returned no results; warnings: {:?}",
        report.warnings
    );
}
