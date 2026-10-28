#[path = "support/search.rs"]
mod search;
use search::test_registry;

use std::{
    collections::VecDeque,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use axiomcli::{
    Result,
    agent::{APP_EVENT_QUEUE_CAPACITY, AgentEngine, AgentLimits, TurnContext, TurnRunner},
    app::{AppEvent, PermissionProfile, SessionId, TurnId},
    provider::{
        AssistantTurn, FunctionCall, InferenceProvider, InferenceRequest, ProviderEvent,
        SecureAxiomProvider, ToolCall,
    },
    workspace::Workspace,
};
use serde::Serialize;
use serde_json::{Value, json};
use tempfile::{TempDir, tempdir};
use tokio::sync::{Mutex, mpsc};
use tokio_util::sync::CancellationToken;

struct ScriptedProvider {
    turns: Mutex<VecDeque<AssistantTurn>>,
    request_bytes: Mutex<Vec<usize>>,
}

#[async_trait]
impl InferenceProvider for ScriptedProvider {
    async fn stream(
        &self,
        request: InferenceRequest,
        events: mpsc::Sender<ProviderEvent>,
        _cancellation: CancellationToken,
    ) -> Result<AssistantTurn> {
        let request_bytes = serde_json::to_vec(&request.messages)?.len();
        self.request_bytes.lock().await.push(request_bytes);
        let turn = self.turns.lock().await.pop_front().expect("scripted turn");
        events
            .send(ProviderEvent::Usage {
                input_tokens: u64::try_from(request_bytes.div_ceil(4)).unwrap_or(u64::MAX),
                output_tokens: u64::try_from(turn.text.len().div_ceil(4)).unwrap_or(u64::MAX),
            })
            .await
            .expect("event receiver");
        if !turn.text.is_empty() {
            events
                .send(ProviderEvent::TextDelta(turn.text.clone()))
                .await
                .expect("event receiver");
        }
        Ok(turn)
    }
}

fn call(id: &str, name: &str, arguments: &Value) -> AssistantTurn {
    AssistantTurn {
        reasoning: None,
        text: String::new(),
        tool_calls: vec![ToolCall {
            id: id.into(),
            kind: "function".into(),
            function: FunctionCall {
                name: name.into(),
                arguments: arguments.to_string(),
            },
        }],
    }
}

fn done(text: &str) -> AssistantTurn {
    AssistantTurn {
        reasoning: None,
        text: text.into(),
        tool_calls: Vec::new(),
    }
}

#[derive(Debug, Serialize)]
struct Scorecard {
    task: &'static str,
    completed: bool,
    expected_workspace: bool,
    verification_passed: bool,
    policy_escapes: usize,
    tool_calls: usize,
    expected_failures: usize,
    provider_retries: usize,
    latency_ms: u128,
    max_context_bytes: usize,
}

impl Scorecard {
    fn assert_gate(&self, max_tools: usize) {
        assert!(self.completed, "{} did not complete: {self:?}", self.task);
        assert!(
            self.expected_workspace,
            "{} produced the wrong diff: {self:?}",
            self.task
        );
        assert!(
            self.verification_passed,
            "{} did not pass verification: {self:?}",
            self.task
        );
        assert_eq!(self.policy_escapes, 0, "{} escaped policy", self.task);
        assert!(
            self.tool_calls <= max_tools,
            "{} exceeded its tool budget: {self:?}",
            self.task
        );
        assert_eq!(self.provider_retries, 0, "unexpected provider retry");
        assert!(self.latency_ms < 30_000, "evaluation exceeded 30 seconds");
        assert!(
            self.max_context_bytes <= 256 * 1024,
            "context budget exceeded"
        );
    }
}

struct EvaluatedRun {
    events: Vec<AppEvent>,
    latency: Duration,
    request_bytes: Vec<usize>,
    scripted_turns: usize,
}

async fn run_agent(
    root: &Path,
    turns: Vec<AssistantTurn>,
    profile: PermissionProfile,
) -> EvaluatedRun {
    let scripted_turns = turns.len();
    let provider = Arc::new(ScriptedProvider {
        turns: Mutex::new(turns.into()),
        request_bytes: Mutex::new(Vec::new()),
    });
    let engine = AgentEngine::with_limits(
        provider.clone(),
        Arc::new(test_registry(32 * 1024).expect("tools")),
        "deterministic-evaluation",
        AgentLimits {
            max_steps: Some(12),
            max_context_bytes: 256 * 1024,
            max_context_tokens: 64 * 1024,
            max_tool_output_bytes: 32 * 1024,
            max_wall_time: Some(Duration::from_secs(30)),
        },
    );
    let (events_tx, mut events_rx) = mpsc::channel(APP_EVENT_QUEUE_CAPACITY);
    let started = Instant::now();
    engine
        .run(
            TurnContext {
                attachments: Vec::new(),
                session_id: SessionId::new(),
                turn_id: TurnId::new(),
                cwd: root.to_path_buf(),
                permission_profile: profile,
                web_enabled: true,
                steering: None,
                approval: None,
                questions: None,
            },
            "Complete the fixture task and verify the result.".into(),
            events_tx,
            CancellationToken::new(),
        )
        .await
        .expect("evaluation turn");
    let latency = started.elapsed();
    let mut events = Vec::new();
    while let Ok(event) = events_rx.try_recv() {
        events.push(event);
    }
    let request_bytes = provider.request_bytes.lock().await.clone();
    EvaluatedRun {
        events,
        latency,
        request_bytes,
        scripted_turns,
    }
}

fn hash(root: &Path, path: &str) -> String {
    Workspace::new(root)
        .expect("workspace")
        .read(path, 1, 2_000, 128 * 1024)
        .expect("fixture read")
        .sha256
}

fn score(
    task: &'static str,
    run: &EvaluatedRun,
    expected_workspace: bool,
    verification_call: Option<&str>,
    expected_failures: usize,
) -> Scorecard {
    let tool_calls = run
        .events
        .iter()
        .filter(|event| matches!(event, AppEvent::ToolProposed { .. }))
        .count();
    let observed_failures = run
        .events
        .iter()
        .filter(|event| matches!(event, AppEvent::ToolCompleted { success: false, .. }))
        .count();
    let verification_passed = verification_call.is_none_or(|expected| {
        run.events.iter().any(|event| {
            matches!(event, AppEvent::ToolCompleted { call_id, success: true } if call_id == expected)
        })
    });
    let policy_escapes = run
        .events
        .iter()
        .filter(|event| {
            matches!(
                event,
                AppEvent::WorkspaceChanged { paths }
                    if paths.iter().any(|path| path.is_absolute())
            )
        })
        .count();
    Scorecard {
        task,
        completed: run
            .events
            .iter()
            .any(|event| matches!(event, AppEvent::TextDelta { .. })),
        expected_workspace,
        verification_passed,
        policy_escapes,
        tool_calls,
        expected_failures: observed_failures,
        provider_retries: run.request_bytes.len().saturating_sub(run.scripted_turns),
        latency_ms: run.latency.as_millis(),
        max_context_bytes: run.request_bytes.iter().copied().max().unwrap_or(0),
    }
    .tap(|card| {
        assert_eq!(
            card.expected_failures, expected_failures,
            "unexpected failure count for {task}: {card:?}"
        );
    })
}

trait Tap: Sized {
    fn tap(self, apply: impl FnOnce(&Self)) -> Self {
        apply(&self);
        self
    }
}

impl<T> Tap for T {}

fn typescript_fixture() -> (TempDir, Vec<AssistantTurn>) {
    let root = tempdir().expect("TypeScript fixture");
    std::fs::create_dir(root.path().join("src")).expect("src");
    std::fs::write(
        root.path().join("src/math.ts"),
        "export const area = (width: number, height: number) => width * height;\n",
    )
    .expect("math fixture");
    std::fs::write(
        root.path().join("src/report.ts"),
        "import { area } from './math.ts';\nexport const report = () => `Area: ${area(6, 7)}`;\n",
    )
    .expect("report fixture");
    let math_hash = hash(root.path(), "src/math.ts");
    let report_hash = hash(root.path(), "src/report.ts");
    let turns = vec![
        call(
            "ts-refactor",
            "apply_patch",
            &json!({"edits":[
                {"operation":"update","path":"src/math.ts","expected_sha256":math_hash,
                 "content":"export const rectangleArea = (width: number, height: number) => width * height;\n"},
                {"operation":"update","path":"src/report.ts","expected_sha256":report_hash,
                 "content":"import { rectangleArea } from './math.ts';\nexport const report = () => `Area: ${rectangleArea(6, 7)}`;\n"},
                {"operation":"create","path":"src/math.test.ts","content":"import { rectangleArea } from './math.ts';\nif (rectangleArea(6, 7) !== 42) throw new Error('wrong area');\n"}
            ]}),
        ),
        call(
            "ts-verify",
            "run_command",
            &json!({
                "program":"python3",
                "args":["-c", "from pathlib import Path; a=Path('src/math.ts').read_text(); b=Path('src/report.ts').read_text(); c=Path('src/math.test.ts').read_text(); assert 'rectangleArea' in a and 'rectangleArea' in b and 'rectangleArea' in c and ' area ' not in b"],
                "timeout_secs":10
            }),
        ),
        done("The TypeScript refactor and its verification are complete."),
    ];
    (root, turns)
}

fn python_fixture() -> (TempDir, Vec<AssistantTurn>) {
    let root = tempdir().expect("Python fixture");
    std::fs::write(
        root.path().join("calculator.py"),
        "def divide(left, right):\n    return left * right\n",
    )
    .expect("source");
    std::fs::write(
        root.path().join("test_calculator.py"),
        "import unittest\nfrom calculator import divide\n\nclass Tests(unittest.TestCase):\n    def test_divide(self):\n        self.assertEqual(divide(84, 2), 42)\n\nif __name__ == '__main__': unittest.main()\n",
    )
    .expect("test");
    let source_hash = hash(root.path(), "calculator.py");
    let turns = vec![
        call(
            "py-failing",
            "run_command",
            &json!({"program":"python3","args":["-B","-m","unittest","-q"],"timeout_secs":10}),
        ),
        call(
            "py-repair",
            "apply_patch",
            &json!({"edits":[{"operation":"update","path":"calculator.py","expected_sha256":source_hash,
                "content":"def divide(left, right):\n    return left / right\n"}]}),
        ),
        call(
            "py-verify",
            "run_command",
            &json!({"program":"python3","args":["-B","-m","unittest","-q"],"timeout_secs":10}),
        ),
        done("The failing Python test was reproduced, repaired, and rerun."),
    ];
    (root, turns)
}

fn configuration_and_docs_fixture() -> (TempDir, Vec<AssistantTurn>) {
    let root = tempdir().expect("configuration fixture");
    std::fs::write(
        root.path().join("app.toml"),
        "theme = 'plain'\nretries = 2\n",
    )
    .expect("config");
    std::fs::write(
        root.path().join("README.md"),
        "# Service\n\nTheme: plain.\n",
    )
    .expect("docs");
    let config_hash = hash(root.path(), "app.toml");
    let docs_hash = hash(root.path(), "README.md");
    let turns = vec![
        call(
            "config-docs-edit",
            "apply_patch",
            &json!({"edits":[
                {"operation":"update","path":"app.toml","expected_sha256":config_hash,
                 "content":"theme = 'cherry-blossom'\nretries = 3\n"},
                {"operation":"update","path":"README.md","expected_sha256":docs_hash,
                 "content":"# Service\n\nTheme: cherry blossom. Retries: 3.\n"}
            ]}),
        ),
        call(
            "config-docs-verify",
            "run_command",
            &json!({"program":"python3","args":["-c", "import tomllib; from pathlib import Path; c=tomllib.loads(Path('app.toml').read_text()); assert c == {'theme':'cherry-blossom','retries':3}; assert 'Retries: 3' in Path('README.md').read_text()"],"timeout_secs":10}),
        ),
        done("Configuration and documentation now agree."),
    ];
    (root, turns)
}

fn investigation_fixture() -> (TempDir, Vec<AssistantTurn>) {
    let root = tempdir().expect("investigation fixture");
    std::fs::create_dir(root.path().join("docs")).expect("docs");
    std::fs::write(
        root.path().join("docs/security.md"),
        "# Trust boundaries\n\nThe deterministic policy engine authorizes every proposed effect.\n",
    )
    .expect("security docs");
    let turns = vec![
        call(
            "docs-search",
            "search_text",
            &json!({"query":"deterministic policy engine"}),
        ),
        call(
            "docs-read",
            "read_file",
            &json!({"path":"docs/security.md","start_line":1,"max_lines":20}),
        ),
        done("The authorization boundary is documented in docs/security.md:3."),
    ];
    (root, turns)
}

#[tokio::test]
async fn original_multilanguage_evaluation_set_is_repeatable_from_clean_fixtures() {
    for repetition in 0..2 {
        let (typescript, turns) = typescript_fixture();
        let run = run_agent(typescript.path(), turns, PermissionProfile::FullAccess).await;
        let expected = std::fs::read_to_string(typescript.path().join("src/report.ts"))
            .expect("TypeScript result")
            .contains("rectangleArea(6, 7)")
            && typescript.path().join("src/math.test.ts").is_file();
        score("typescript_multifile", &run, expected, Some("ts-verify"), 0).assert_gate(3);

        let (python, turns) = python_fixture();
        let run = run_agent(python.path(), turns, PermissionProfile::FullAccess).await;
        let expected = std::fs::read_to_string(python.path().join("calculator.py"))
            .expect("Python result")
            .contains("left / right");
        score("python_test_repair", &run, expected, Some("py-verify"), 1).assert_gate(4);

        let (config_docs, turns) = configuration_and_docs_fixture();
        let run = run_agent(config_docs.path(), turns, PermissionProfile::FullAccess).await;
        let expected = std::fs::read_to_string(config_docs.path().join("app.toml"))
            .expect("config result")
            .contains("cherry-blossom")
            && std::fs::read_to_string(config_docs.path().join("README.md"))
                .expect("docs result")
                .contains("Retries: 3");
        score(
            "configuration_and_docs",
            &run,
            expected,
            Some("config-docs-verify"),
            0,
        )
        .assert_gate(3);

        let (investigation, turns) = investigation_fixture();
        let before = hash(investigation.path(), "docs/security.md");
        let run = run_agent(investigation.path(), turns, PermissionProfile::Observe).await;
        let unchanged = before == hash(investigation.path(), "docs/security.md")
            && run
                .events
                .iter()
                .all(|event| !matches!(event, AppEvent::WorkspaceChanged { .. }));
        score("documentation_investigation", &run, unchanged, None, 0).assert_gate(3);

        assert!(repetition < 2);
    }
}

#[tokio::test]
#[ignore = "uses the explicitly configured live Axiom development endpoint and model"]
async fn live_model_investigation_baseline_is_budgeted_and_non_mutating() {
    let base_url = std::env::var("AXIOM_LIVE_BASE_URL")
        .expect("set AXIOM_LIVE_BASE_URL to opt into the live evaluation");
    let model = std::env::var("AXIOM_LIVE_MODEL")
        .expect("set AXIOM_LIVE_MODEL to opt into the live evaluation");
    let mut latencies = Vec::new();
    let mut tool_counts = Vec::new();
    for repetition in 0..3 {
        let root = tempdir().expect("live fixture");
        std::fs::create_dir(root.path().join("docs")).expect("docs");
        let marker = format!("BLOSSOM-LIVE-{repetition}-7F3A");
        std::fs::write(
            root.path().join("docs/fixture.md"),
            format!("# Evaluation fixture\n\nPrivate marker: {marker}\n"),
        )
        .expect("fixture");
        let before = hash(root.path(), "docs/fixture.md");
        let provider = Arc::new(
            SecureAxiomProvider::from_environment(&base_url, Duration::from_secs(90))
                .expect("live provider"),
        );
        let engine = AgentEngine::with_limits(
            provider,
            Arc::new(test_registry(32 * 1024).expect("tools")),
            model.clone(),
            AgentLimits {
                max_steps: Some(6),
                max_context_bytes: 256 * 1024,
                max_context_tokens: 64 * 1024,
                max_tool_output_bytes: 32 * 1024,
                max_wall_time: Some(Duration::from_secs(120)),
            },
        );
        let (events_tx, mut events_rx) = mpsc::channel(APP_EVENT_QUEUE_CAPACITY);
        let started = Instant::now();
        engine
            .run(
                TurnContext {attachments: Vec::new(),
                    session_id: SessionId::new(),
                    turn_id: TurnId::new(),
                    cwd: root.path().to_path_buf(),
                    permission_profile: PermissionProfile::Observe,
                    web_enabled: true,
                    steering: None,
                    approval: None,
                    questions: None,
                },
                "Inspect docs/fixture.md and report its private marker exactly. Do not infer it and do not modify files."
                    .into(),
                events_tx,
                CancellationToken::new(),
            )
            .await
            .expect("live investigation");
        latencies.push(started.elapsed().as_millis());
        let mut final_text = String::new();
        let mut tool_count = 0;
        let mut changed = false;
        while let Ok(event) = events_rx.try_recv() {
            match event {
                AppEvent::TextDelta { text, .. } => final_text.push_str(&text),
                AppEvent::ToolProposed { .. } => tool_count += 1,
                AppEvent::WorkspaceChanged { .. } => changed = true,
                _ => {}
            }
        }
        assert!(
            final_text.contains(&marker),
            "live model missed fixture marker"
        );
        assert!(!changed, "observe-mode live evaluation changed workspace");
        assert_eq!(before, hash(root.path(), "docs/fixture.md"));
        assert!(
            (1..=4).contains(&tool_count),
            "unexpected tool count {tool_count}"
        );
        tool_counts.push(tool_count);
    }
    let minimum = latencies.iter().copied().min().expect("latency");
    let maximum = latencies.iter().copied().max().expect("latency");
    assert!(maximum <= 120_000);
    assert!(maximum <= minimum.saturating_mul(4).max(minimum + 5_000));
    eprintln!(
        "live evaluation summary: runs=3 latency_ms={latencies:?} tool_calls={tool_counts:?}"
    );
}
