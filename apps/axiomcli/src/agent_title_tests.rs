use super::*;
use crate::{
    app::{AppCommand, Origin, Runtime},
    session::SessionStore,
    session_title::TitleGeneration,
};
use axiom_inference::{InvocationPurpose, InvocationState, ModelInfo, RequestUsage, ThinkingMode};
use axiom_secure_client::ASTREAAI_QWEN_MODEL;
use tokio::sync::Notify;

struct TitleProvider {
    output: &'static str,
    fail: bool,
    block: bool,
    direct: bool,
    started: Notify,
    requests: Mutex<Vec<InferenceRequest>>,
}

impl TitleProvider {
    fn new(output: &'static str, fail: bool, block: bool) -> Self {
        Self {
            output,
            fail,
            block,
            direct: false,
            started: Notify::new(),
            requests: Mutex::new(vec![]),
        }
    }
}

#[async_trait]
impl InferenceProvider for TitleProvider {
    async fn models(&self, _: CancellationToken) -> Result<Vec<ModelInfo>> {
        Ok(vec![ModelInfo {
            id: if self.direct {
                ASTREAAI_QWEN_MODEL
            } else {
                "test"
            }
            .into(),
            provider_id: if self.direct {
                "axiom-direct".into()
            } else {
                String::new()
            },
            ..Default::default()
        }])
    }

    async fn stream(
        &self,
        request: InferenceRequest,
        events: mpsc::Sender<ProviderEvent>,
        cancellation: CancellationToken,
    ) -> Result<AssistantTurn> {
        let model_id = request.model.clone();
        self.requests.lock().await.push(request);
        for event in [
            ProviderEvent::TextDelta("provisional title".into()),
            ProviderEvent::ReasoningDelta("private reasoning".into()),
            ProviderEvent::Usage {
                input_tokens: 30,
                output_tokens: 8,
            },
        ] {
            events.send(event).await.unwrap();
        }
        self.started.notify_one();
        if self.block {
            cancellation.cancelled().await;
        }
        events
            .send(ProviderEvent::Accounting(Box::new(RequestUsage {
                request_id: "a".repeat(32),
                model_id,
                provider_id: if self.direct {
                    "axiom-direct"
                } else {
                    "fixture"
                }
                .into(),
                state: if self.fail || self.block {
                    InvocationState::Failed
                } else {
                    InvocationState::Completed
                },
                cost_microusd: Some("42".into()),
                input_tokens: Some("30".into()),
                output_tokens: Some("8".into()),
                ..Default::default()
            })))
            .await
            .unwrap();
        if self.fail {
            return Err(AxiomError::Provider(
                "terminal authentication failed".into(),
            ));
        }
        if self.block {
            return Err(AxiomError::Cancelled);
        }
        Ok(AssistantTurn {
            text: self.output.into(),
            reasoning: None,
            tool_calls: vec![],
        })
    }
}

fn engine(provider: Arc<TitleProvider>) -> AgentEngine {
    AgentEngine::new(provider, Arc::new(ToolRegistry::new()), "test", 2)
}

#[tokio::test]
async fn qwen_generates_a_title_after_its_first_reply() {
    let mut provider = TitleProvider::new("Coastal hiking routes", false, false);
    provider.direct = true;
    let provider = Arc::new(provider);
    let engine = engine(provider.clone());
    let (store, id) = thread().await;
    let prompt = "Where should I hike on the coast?";
    let mut job = TitleGeneration::prepare(&store, &id, prompt, ASTREAAI_QWEN_MODEL)
        .unwrap()
        .unwrap();
    let reply_finished = job.defer_until_first_reply().unwrap();
    let title = job.run(&engine, CancellationToken::new());
    tokio::pin!(title);
    assert!(futures::poll!(&mut title).is_pending());
    assert!(provider.requests.lock().await.is_empty());
    assert_eq!(
        store.thread_summary(&id).unwrap().title.as_deref(),
        Some("Where should I hike on the coast")
    );
    reply_finished.send(()).unwrap();
    assert_eq!(
        title.await.unwrap().as_deref(),
        Some("Coastal hiking routes")
    );
    let snapshot = store.thread_snapshot(&id, None, 10).unwrap();
    assert_eq!(
        snapshot.thread.title.as_deref(),
        Some("Coastal hiking routes")
    );
    assert_eq!(snapshot.request_usage.len(), 1);
    assert_eq!(snapshot.request_usage[0].purpose, InvocationPurpose::Title);
    let requests = provider.requests.lock().await;
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].model, ASTREAAI_QWEN_MODEL);
    assert!(
        requests[0].messages[1]
            .content
            .contains(&serde_json::json!(prompt).to_string())
    );
    assert!(
        requests[0].messages[1]
            .content
            .ends_with("Return only a 3-8 word topic label.")
    );
    assert!(requests[0].tools.is_empty());
}

#[tokio::test]
async fn qwen_skips_title_inference_after_a_stopped_turn_or_manual_rename() {
    let mut provider = TitleProvider::new("Unused", false, false);
    provider.direct = true;
    let provider = Arc::new(provider);
    let engine = engine(provider.clone());
    for renamed in [false, true] {
        let (store, id) = thread().await;
        let mut job = TitleGeneration::prepare(&store, &id, "First message", ASTREAAI_QWEN_MODEL)
            .unwrap()
            .unwrap();
        let reply_finished = job.defer_until_first_reply().unwrap();
        if renamed {
            store.rename(&id, "My title").unwrap();
            reply_finished.send(()).unwrap();
        } else {
            drop(reply_finished);
        }
        assert_eq!(
            job.run(&engine, CancellationToken::new()).await.unwrap(),
            None
        );
        assert_eq!(
            store.thread_summary(&id).unwrap().title.as_deref(),
            Some(if renamed { "My title" } else { "First message" })
        );
    }
    assert!(provider.requests.lock().await.is_empty());
}

#[test]
fn text_only_model_omits_tools_without_changing_supported_models() {
    let tools = vec![crate::provider::ToolDefinition {
        kind: "function".into(),
        function: axiom_inference::FunctionDefinition {
            name: "read".into(),
            description: "Read a file".into(),
            parameters: serde_json::json!({"type":"object","properties":{}}),
            strict: None,
        },
    }];
    let mut model = ModelInfo {
        id: "direct:qwen3.8-27b".into(),
        ..Default::default()
    };
    let request = inference_request_for_model(
        &model,
        model.id.clone(),
        vec![],
        tools.clone(),
        ThinkingLevel::ProviderDefault,
    );
    assert!(request.tools.is_empty());
    model.supports_tools = true;
    let request = inference_request_for_model(
        &model,
        model.id.clone(),
        vec![],
        tools.clone(),
        ThinkingLevel::ProviderDefault,
    );
    assert_eq!(request.tools, tools);
}

#[test]
fn title_requests_respect_reasoning_controls_and_output_limits() {
    use axiom_inference::ReasoningEffort;
    let mut model = ModelInfo {
        id: "test".into(),
        context_window_tokens: 32_768,
        max_output_tokens: 8_192,
        supported_thinking_modes: vec![ThinkingMode::Enabled, ThinkingMode::Disabled],
        supported_reasoning_efforts: vec![ReasoningEffort::High, ReasoningEffort::Low],
        ..Default::default()
    };
    let request = title_request(&model, &"界".repeat(2_000));
    assert_eq!(request.thinking_mode, ThinkingMode::Disabled);
    assert_eq!(request.reasoning_effort, ReasoningEffort::Low);
    assert_eq!(request.max_output_tokens, Some(256));
    assert!(request.tools.is_empty());
    assert_eq!(request.messages.len(), 2);
    assert!(request.messages[1].content.len() <= 4_000);
    model.supported_thinking_modes = vec![ThinkingMode::Enabled];
    assert_eq!(
        title_request(&model, "hello").max_output_tokens,
        Some(4_096)
    );
    assert_eq!(
        title_request(&model, "hello").thinking_mode,
        ThinkingMode::Enabled
    );
    model.supported_thinking_modes.clear();
    model.supported_reasoning_efforts.clear();
    let request = title_request(&model, "hello");
    assert_eq!(request.thinking_mode, ThinkingMode::ProviderDefault);
    assert_eq!(request.max_output_tokens, Some(4_096));
    model.provider_id = "tinfoil".into();
    model.max_output_tokens = 512;
    assert_eq!(title_request(&model, "hello").max_output_tokens, Some(512));
}

#[tokio::test]
async fn title_generation_only_publishes_final_text_and_separate_accounting() {
    for (output, fail, expected) in [
        (
            "\"Fixing login redirects.\"",
            false,
            Some("Fixing login redirects"),
        ),
        ("", false, None),
        ("***?!***", false, None),
        ("Forged title", true, None),
    ] {
        let provider = Arc::new(TitleProvider::new(output, fail, false));
        let engine = engine(provider.clone());
        let (tx, mut rx) = mpsc::channel(16);
        let cancellation = CancellationToken::new();
        let result = engine
            .generate_title("test", "Please fix login", tx, cancellation.clone())
            .await;
        assert_eq!(result.ok().as_deref(), expected);
        assert!(
            !cancellation.is_cancelled(),
            "the caller owns the parent cancellation token"
        );
        assert!(engine.histories.lock().await.is_empty());
        assert!(engine.context_tokens.lock().await.is_empty());
        let AppEvent::RequestUsageUpdated { usage } = rx.recv().await.unwrap() else {
            panic!("only accounting may escape title inference")
        };
        assert_eq!(usage.purpose, InvocationPurpose::Title);
        assert_eq!(usage.cost_microusd.as_deref(), Some("42"));
        assert!(rx.recv().await.is_none());
        assert_eq!(provider.requests.lock().await.len(), 1);
    }
}

async fn thread() -> (SessionStore, SessionId) {
    let store = SessionStore::in_memory().unwrap();
    let runtime = Runtime::new(32);
    let id = runtime.session_id();
    let events = runtime
        .dispatch(AppCommand::CreateSession {
            session_id: id.clone(),
            cwd: PathBuf::from("/tmp"),
            origin: Origin::Test,
            profile: PermissionProfile::Confirm,
        })
        .await
        .unwrap();
    store.append_all(&events).unwrap();
    (store, id)
}

#[tokio::test]
async fn background_title_keeps_an_immediate_fallback_and_records_success_or_failure_cost() {
    for fail in [false, true] {
        let (store, id) = thread().await;
        let original_items = store.thread_snapshot(&id, None, 10).unwrap().items;
        let mut job = TitleGeneration::prepare(
            &store,
            &id,
            "Please help me repair the login redirect",
            "test",
        )
        .unwrap()
        .unwrap();
        let fallback = store.thread_summary(&id).unwrap().title.unwrap();
        assert_eq!(fallback, "Please help me repair the login redirect");
        let engine = engine(Arc::new(TitleProvider::new(
            "Fixing login redirects",
            fail,
            false,
        )));
        assert!(job.defer_until_first_reply().is_none());
        let result = job.run(&engine, CancellationToken::new()).await;
        assert_eq!(result.is_err(), fail);
        let snapshot = store.thread_snapshot(&id, None, 10).unwrap();
        assert_eq!(
            snapshot.thread.title.as_deref(),
            Some(if fail {
                &fallback
            } else {
                "Fixing login redirects"
            })
        );
        assert_eq!(snapshot.request_usage.len(), 1);
        assert_eq!(snapshot.request_usage[0].purpose, InvocationPurpose::Title);
        assert_eq!(
            snapshot.request_usage[0].cost_microusd.as_deref(),
            Some("42")
        );
        assert_eq!(snapshot.items, original_items);
        assert!(
            TitleGeneration::prepare(&store, &id, "Another prompt", "test")
                .unwrap()
                .is_none()
        );
    }
}

#[tokio::test]
async fn manual_rename_cancels_background_inference_without_losing_its_usage() {
    let (store, id) = thread().await;
    let job = TitleGeneration::prepare(&store, &id, "First prompt", "test")
        .unwrap()
        .unwrap();
    let provider = Arc::new(TitleProvider::new("Late title", false, true));
    let engine = engine(provider.clone());
    let task = tokio::spawn(async move { job.run(&engine, CancellationToken::new()).await });
    provider.started.notified().await;
    store.rename(&id, "First prompt").unwrap();
    let result = tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(result, Err(AxiomError::Cancelled)));
    let snapshot = store.thread_snapshot(&id, None, 10).unwrap();
    assert_eq!(snapshot.thread.title.as_deref(), Some("First prompt"));
    assert_eq!(snapshot.request_usage.len(), 1);
}
