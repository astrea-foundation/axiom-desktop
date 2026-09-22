use std::{collections::BTreeMap, sync::Arc, time::Duration};

use async_trait::async_trait;
use axiomcli::{
    Result,
    agent::{APP_EVENT_QUEUE_CAPACITY, AgentEngine, AgentLimits, TurnContext, TurnRunner},
    app::{AppCommand, AppEvent, Origin, PermissionProfile, Runtime, SessionId, TurnId},
    provider::{AssistantTurn, InferenceProvider, InferenceRequest, ProviderEvent},
    session::{ExportOptions, SessionStore},
    tools::ToolRegistry,
    workspace::{ProcessManager, ProcessRequest, Workspace},
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[test]
#[ignore = "performance benchmark; run scripts/axiomcli-eval deterministic explicitly"]
fn repeated_cli_startup_stays_within_the_development_budget() {
    let started = std::time::Instant::now();
    for _ in 0..10 {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_axiomcli"))
            .arg("--version")
            .output()
            .expect("start axiomcli");
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8(output.stdout).expect("version output"),
            format!("axiomcli {}\n", env!("CARGO_PKG_VERSION"))
        );
    }
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_secs(3),
        "ten debug CLI startups exceeded three seconds"
    );
    eprintln!(
        "startup summary: runs=10 elapsed_ms={}",
        elapsed.as_millis()
    );
}

struct LongStreamProvider {
    chunks: usize,
    chunk: String,
}

#[async_trait]
impl InferenceProvider for LongStreamProvider {
    async fn stream(
        &self,
        _request: InferenceRequest,
        events: mpsc::Sender<ProviderEvent>,
        cancellation: CancellationToken,
    ) -> Result<AssistantTurn> {
        let mut text = String::with_capacity(self.chunks * self.chunk.len());
        for _ in 0..self.chunks {
            if cancellation.is_cancelled() {
                return Err(axiomcli::AxiomError::Cancelled);
            }
            text.push_str(&self.chunk);
            events
                .send(ProviderEvent::TextDelta(self.chunk.clone()))
                .await
                .map_err(|_| axiomcli::AxiomError::Cancelled)?;
        }
        Ok(AssistantTurn {
            reasoning: None,
            text,
            tool_calls: Vec::new(),
        })
    }
}

#[tokio::test]
async fn stream_deltas_merge_into_one_bounded_persisted_message() {
    check_stream_and_transcript(64, 32, false).await;
}

#[tokio::test]
#[ignore = "durable-write soak benchmark; run scripts/axiomcli-eval deterministic explicitly"]
async fn long_stream_and_many_transcript_entries_stay_within_recorded_budgets() {
    check_stream_and_transcript(10_000, 5_000, true).await;
}

async fn check_stream_and_transcript(chunks: usize, checkpoints: usize, benchmark: bool) {
    let workspace = tempfile::tempdir().expect("workspace");
    let engine = AgentEngine::with_limits(
        Arc::new(LongStreamProvider {
            chunks,
            chunk: "petal-stream-0123456789abcdef\n".into(),
        }),
        Arc::new(ToolRegistry::new()),
        "soak",
        AgentLimits {
            max_steps: Some(1),
            max_context_bytes: 2 * 1024 * 1024,
            max_context_tokens: 512 * 1024,
            max_tool_output_bytes: 4096,
            max_wall_time: Some(Duration::from_secs(10)),
        },
    );
    let (events_tx, mut events_rx) = mpsc::channel(APP_EVENT_QUEUE_CAPACITY);
    let collector = tokio::spawn(async move {
        let mut deltas = 0;
        let mut retained_bytes = 0;
        while let Some(event) = events_rx.recv().await {
            if let AppEvent::TextDelta { text, .. } = event {
                deltas += 1;
                retained_bytes += text.len();
            }
        }
        (deltas, retained_bytes)
    });
    let started = std::time::Instant::now();
    engine
        .run(
            TurnContext {
                attachments: Vec::new(),
                session_id: SessionId::new(),
                turn_id: TurnId::new(),
                cwd: workspace.path().to_path_buf(),
                permission_profile: PermissionProfile::Observe,
                web_enabled: true,
                steering: None,
                approval: None,
                questions: None,
            },
            "stream a bounded long response".into(),
            events_tx,
            CancellationToken::new(),
        )
        .await
        .expect("long stream");
    let (deltas, retained_bytes) = collector.await.expect("event collector");
    assert_eq!(deltas, chunks);
    assert_eq!(retained_bytes, chunks * 30);
    if benchmark {
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    let state = tempfile::tempdir().expect("state");
    let database = state.path().join("state.sqlite3");
    let store = SessionStore::open(&database).expect("store");
    let runtime = Runtime::new(8);
    let session_id = runtime.session_id();
    let created = runtime
        .dispatch(AppCommand::CreateSession {
            session_id: session_id.clone(),
            cwd: workspace.path().to_path_buf(),
            origin: Origin::Test,
            profile: PermissionProfile::Observe,
        })
        .await
        .expect("create session");
    store.append_all(&created).expect("persist creation");
    let turn_id = TurnId::new();
    let submitted = runtime
        .dispatch(AppCommand::SubmitPrompt {
            attachments: Vec::new(),
            session_id: session_id.clone(),
            turn_id: turn_id.clone(),
            text: "stream a long response".into(),
        })
        .await
        .expect("submit prompt");
    store.append_all(&submitted).expect("persist prompt");
    let persistence_started = std::time::Instant::now();
    for index in 0..checkpoints {
        let event = runtime
            .emit(
                session_id.clone(),
                AppEvent::TextDelta {
                    turn_id: turn_id.clone(),
                    text: format!("entry-{index:04}-{}", "x".repeat(64)),
                },
            )
            .await
            .expect("emit transcript event");
        store.append(&event).expect("persist transcript event");
    }
    let export = store
        .export_with_options(
            &session_id,
            ExportOptions {
                include_prompts: false,
                include_tool_output: false,
                max_items: 1_000,
            },
        )
        .expect("bounded export");
    let value: serde_json::Value = serde_json::from_str(&export).expect("export JSON");
    let timeline_items = value["timeline_items"].as_array().expect("timeline items");
    assert_eq!(timeline_items.len(), 3);
    assert_eq!(
        timeline_items
            .iter()
            .filter(|item| item["kind"] == "assistant_message")
            .count(),
        1,
        "streaming deltas must merge into one assistant timeline item"
    );
    assert!(export.len() < 512 * 1024);
    let persistence_elapsed = persistence_started.elapsed();
    let database_bytes = std::fs::metadata(database).expect("database").len();
    eprintln!(
        "session summary: stream_chunks={chunks} transcript_checkpoints={checkpoints} persistence_ms={} database_bytes={database_bytes}",
        persistence_elapsed.as_millis()
    );
    // Hosted Windows disks have substantially higher per-commit flush latency:
    // 5,000 separate durable commits measured 103 seconds on windows-latest.
    // Keep the same commit count and storage bounds with platform I/O budgets.
    let persistence_budget = Duration::from_secs(if cfg!(windows) { 180 } else { 30 });
    if benchmark {
        assert!(
            persistence_elapsed < persistence_budget,
            "{checkpoints} durable checkpoints took {persistence_elapsed:?}, budget {persistence_budget:?}"
        );
    }
    assert!(database_bytes < 16 * 1024 * 1024);
}

#[tokio::test]
async fn repeated_background_batches_are_ordered_bounded_and_fully_reaped() {
    let root = tempfile::tempdir().expect("workspace");
    let workspace = Workspace::new(root.path()).expect("workspace");
    let manager = ProcessManager::new(1024);
    let parent = CancellationToken::new();
    let started = std::time::Instant::now();
    let mut ids = Vec::new();
    for batch in 0..6 {
        let mut current = Vec::new();
        for item in 0..4 {
            current.push(
                manager
                    .start(
                        workspace.clone(),
                        ProcessRequest {
                            program: "python3".into(),
                            args: vec!["-c".into(), format!("print('batch-{batch}-item-{item}')")],
                            cwd: root.path().to_path_buf(),
                            env: BTreeMap::new(),
                            timeout_secs: 5,
                        },
                        parent.clone(),
                        "full_access".into(),
                    )
                    .await
                    .expect("start background task"),
            );
        }
        for id in &current {
            let result = manager.wait(id, 5).await.expect("wait for task");
            assert_eq!(result.state, "completed");
            assert_eq!(result.outcome.expect("outcome").exit_code, Some(0));
        }
        ids.extend(current);
    }
    let snapshots = manager.list().await;
    assert_eq!(snapshots.len(), 24);
    assert!(
        snapshots
            .windows(2)
            .all(|pair| pair[0].sequence < pair[1].sequence)
    );
    assert!(snapshots.iter().all(|snapshot| {
        snapshot.state == "completed"
            && snapshot.permission_profile == "full_access"
            && snapshot.stdout.len() <= 1024
            && snapshot.stderr.len() <= 1024
    }));
    assert_eq!(ids.len(), 24);
    let elapsed = started.elapsed();
    assert!(elapsed < Duration::from_secs(15));
    eprintln!(
        "background summary: tasks=24 elapsed_ms={}",
        elapsed.as_millis()
    );
}
