use super::*;
use axiom_inference::{FinishReason, ModelInfo, ProviderFailureKind};

struct RecoveryProvider {
    requests: Mutex<Vec<InferenceRequest>>,
    failures: usize,
    code: &'static str,
    allowed: bool,
    length: bool,
    authorized: tokio::sync::Notify,
}

impl RecoveryProvider {
    fn new(failures: usize) -> Self {
        Self {
            requests: Mutex::new(Vec::new()),
            failures,
            code: "PROVIDER_DISCONNECTED",
            allowed: true,
            length: false,
            authorized: tokio::sync::Notify::new(),
        }
    }
}

#[async_trait]
impl InferenceProvider for RecoveryProvider {
    async fn models(&self, _: CancellationToken) -> Result<Vec<ModelInfo>> {
        Ok(vec![ModelInfo {
            id: "test".into(),
            context_window_tokens: 32768,
            max_output_tokens: 4096,
            ..Default::default()
        }])
    }
    async fn authorize_recovery(&self, _: &str, _: &str, _: CancellationToken) -> Result<bool> {
        self.authorized.notify_one();
        Ok(self.allowed)
    }
    async fn stream(
        &self,
        request: InferenceRequest,
        events: mpsc::Sender<ProviderEvent>,
        _: CancellationToken,
    ) -> Result<AssistantTurn> {
        let mut requests = self.requests.lock().await;
        requests.push(request);
        let failed = requests.len() <= self.failures;
        drop(requests);
        let text = if failed {
            "unverified fragment"
        } else {
            "complete answer"
        };
        events
            .send(ProviderEvent::TextDelta(text.into()))
            .await
            .unwrap();
        if failed {
            return Err(AxiomError::SecureProvider {
                kind: ProviderFailureKind::Transient,
                code: self.code,
                message: "Connection interrupted",
            });
        }
        events
            .send(ProviderEvent::Finished(if self.length {
                FinishReason::Length
            } else {
                FinishReason::Stop
            }))
            .await
            .unwrap();
        events.send(ProviderEvent::ResponseVerified).await.unwrap();
        Ok(AssistantTurn {
            text: text.into(),
            ..Default::default()
        })
    }
}

fn context() -> TurnContext {
    TurnContext {
        mcp: None,
        session_id: SessionId::new(),
        turn_id: TurnId::new(),
        cwd: PathBuf::from("."),
        permission_profile: PermissionProfile::Confirm,
        web_enabled: false,
        image_tools_available: false,
        attachments: Vec::new(),
        steering: None,
        approval: None,
        questions: None,
    }
}

async fn run(provider: Arc<RecoveryProvider>) -> (Result<()>, Vec<AppEvent>, Vec<ChatMessage>) {
    let engine = AgentEngine::new(provider, Arc::new(ToolRegistry::new()), "test", 1);
    let context = context();
    let session = context.session_id.clone();
    let (tx, mut rx) = mpsc::channel(APP_EVENT_QUEUE_CAPACITY);
    let result = engine
        .run(context, "answer this".into(), tx, CancellationToken::new())
        .await;
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }
    let history = engine
        .histories
        .lock()
        .await
        .get(&session)
        .cloned()
        .unwrap_or_default();
    (result, events, history)
}

#[tokio::test(start_paused = true)]
async fn transport_recovery_is_bounded_and_never_replays_partial_text_as_history() {
    let provider = Arc::new(RecoveryProvider::new(2));
    let (result, events, history) = run(provider.clone()).await;
    result.unwrap();
    let requests = provider.requests.lock().await;
    assert_eq!(requests.len(), 3);
    assert!(
        requests
            .windows(2)
            .all(|r| r[0].request_id != r[1].request_id)
    );
    assert!(requests.iter().all(|r| {
        r.messages
            .iter()
            .all(|m| !m.content.contains("unverified fragment"))
    }));
    assert!(
        !history
            .iter()
            .any(|m| m.content.contains("unverified fragment"))
    );
    assert_eq!(events.iter().filter(|e| matches!(e, AppEvent::ProviderStatusChanged { detail, .. } if detail.starts_with("Connection interrupted. Retrying"))).count(), 2);
}

#[tokio::test(start_paused = true)]
async fn exhaustion_stops_after_three_attempts() {
    let provider = Arc::new(RecoveryProvider::new(10));
    let (result, _, _) = run(provider.clone()).await;
    assert!(result.is_err());
    assert_eq!(provider.requests.lock().await.len(), 3);
}

#[tokio::test(start_paused = true)]
async fn integrity_failures_and_unreconciled_charges_are_not_retried() {
    for (code, allowed) in [
        ("INVALID_RESPONSE", true),
        ("RECEIPT_UNAVAILABLE", true),
        ("PROVIDER_DISCONNECTED", false),
    ] {
        let provider = Arc::new(RecoveryProvider {
            code,
            allowed,
            ..RecoveryProvider::new(10)
        });
        assert!(run(provider.clone()).await.0.is_err());
        assert_eq!(provider.requests.lock().await.len(), 1);
    }
}

#[tokio::test(start_paused = true)]
async fn output_limit_continues_once_only_after_verified_completion() {
    let provider = Arc::new(RecoveryProvider {
        length: true,
        ..RecoveryProvider::new(0)
    });
    let (result, _, history) = run(provider.clone()).await;
    result.unwrap();
    assert_eq!(provider.requests.lock().await.len(), 2);
    assert_eq!(
        history
            .iter()
            .filter(|m| m
                .content
                .starts_with("[APPLICATION-GENERATED CONTINUATION]"))
            .count(),
        1
    );
}

#[tokio::test(start_paused = true)]
async fn stop_during_backoff_never_dispatches_the_successor() {
    let provider = Arc::new(RecoveryProvider::new(10));
    let engine = AgentEngine::new(provider.clone(), Arc::new(ToolRegistry::new()), "test", 1);
    let cancellation = CancellationToken::new();
    let child = cancellation.clone();
    let (tx, _rx) = mpsc::channel(APP_EVENT_QUEUE_CAPACITY);
    let work =
        tokio::spawn(async move { engine.run(context(), "answer this".into(), tx, child).await });
    provider.authorized.notified().await;
    cancellation.cancel();
    assert!(matches!(work.await.unwrap(), Err(AxiomError::Cancelled)));
    assert_eq!(provider.requests.lock().await.len(), 1);
}

#[tokio::test]
async fn restarting_from_a_checkpoint_never_promotes_interrupted_display_deltas() {
    let provider = Arc::new(RecoveryProvider {
        allowed: false,
        ..RecoveryProvider::new(1)
    });
    let (result, events, _) = run(provider.clone()).await;
    assert!(result.is_err());
    assert!(events.iter().any(
        |event| matches!(event, AppEvent::TextDelta { text, .. } if text == "unverified fragment")
    ));
    let turn = events
        .iter()
        .find_map(|event| event.turn_id().cloned())
        .unwrap();
    let session = SessionId::new();
    let journal = [
        AppEvent::PromptAccepted {
            turn_id: turn.clone(),
            text: "answer this".into(),
            attachments: vec![],
        },
        AppEvent::TurnStarted { turn_id: turn },
    ]
    .into_iter()
    .chain(events)
    .enumerate()
    .map(|(index, event)| EventEnvelope {
        schema_version: 1,
        sequence: index as u64 + 1,
        turn_id: event.turn_id().cloned(),
        occurred_at: chrono::Utc::now(),
        correlation_id: crate::app::CorrelationId::new(),
        origin: crate::app::Origin::Test,
        session_id: session.clone(),
        event,
    })
    .collect::<Vec<_>>();
    let engine = AgentEngine::new(provider.clone(), Arc::new(ToolRegistry::new()), "test", 1);
    engine.restore_session(&session, &journal).await.unwrap();
    let (tx, _rx) = mpsc::channel(APP_EVENT_QUEUE_CAPACITY);
    engine
        .run(
            TurnContext {
                session_id: session,
                ..context()
            },
            "Continue".into(),
            tx,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let requests = provider.requests.lock().await;
    assert_eq!(requests.len(), 2);
    assert!(
        requests[1]
            .messages
            .iter()
            .any(|message| message.role == ChatRole::User && message.content == "answer this")
    );
    assert!(
        !requests[1]
            .messages
            .iter()
            .any(|message| message.content.contains("unverified fragment"))
    );
}
