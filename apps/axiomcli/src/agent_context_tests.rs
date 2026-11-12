//! Synthetic regressions for image context accounting and staged compaction.
use super::*;
use axiom_inference::{ImageContent, ModelInfo, PromptAttachment};
use std::collections::VecDeque;

struct ContextProvider {
    model: ModelInfo,
    turns: Mutex<VecDeque<AssistantTurn>>,
    requests: Mutex<Vec<InferenceRequest>>,
    usage: (u64, u64),
    compaction_reasoning_tokens: u32,
}

impl ContextProvider {
    fn new(model_id: &str) -> Self {
        Self {
            model: ModelInfo {
                id: model_id.into(),
                provider_id: if model_id == "direct:qwen3.8-27b" {
                    "axiom-direct".into()
                } else {
                    String::new()
                },
                supports_images: true,
                context_window_tokens: 32_768,
                max_output_tokens: 4096,
                ..Default::default()
            },
            turns: Mutex::new(VecDeque::new()),
            requests: Mutex::new(Vec::new()),
            usage: (8000, 100),
            compaction_reasoning_tokens: 0,
        }
    }
}

fn is_compaction(request: &InferenceRequest) -> bool {
    request.messages[0]
        .content
        .starts_with("Create a faithful successor summary")
}

#[async_trait]
impl InferenceProvider for ContextProvider {
    async fn models(&self, _: CancellationToken) -> Result<Vec<ModelInfo>> {
        Ok(vec![self.model.clone()])
    }

    async fn stream(
        &self,
        request: InferenceRequest,
        events: mpsc::Sender<ProviderEvent>,
        _: CancellationToken,
    ) -> Result<AssistantTurn> {
        request
            .validate()
            .map_err(|error| AxiomError::Provider(format!("invalid synthetic request: {error}")))?;
        let compacting = is_compaction(&request);
        let reasoning_exhausts_output = compacting
            && request.max_output_tokens.unwrap_or(0) <= self.compaction_reasoning_tokens;
        let fits = model_context_tokens(&self.model, &request.messages)
            + u64::from(request.max_output_tokens.unwrap_or(0))
            <= u64::from(self.model.context_window_tokens);
        self.requests.lock().await.push(request);
        if !fits {
            return Err(AxiomError::Provider(
                "synthetic provider: image inputs exceed the model context".into(),
            ));
        }
        events
            .send(ProviderEvent::Usage {
                input_tokens: self.usage.0,
                output_tokens: self.usage.1,
            })
            .await
            .unwrap();
        events
            .send(ProviderEvent::Finished(if reasoning_exhausts_output {
                axiom_inference::FinishReason::Length
            } else {
                axiom_inference::FinishReason::Stop
            }))
            .await
            .unwrap();
        Ok(if compacting {
            AssistantTurn {
                text: if reasoning_exhausts_output {
                    String::new()
                } else {
                    "Synthetic summary preserving image observations.".into()
                },
                reasoning: None,
                tool_calls: vec![],
            }
        } else {
            self.turns
                .lock()
                .await
                .pop_front()
                .unwrap_or(AssistantTurn {
                    text: "Synthetic image analysis complete.".into(),
                    reasoning: None,
                    tool_calls: vec![],
                })
        })
    }
}

fn context() -> TurnContext {
    TurnContext {
        mcp: None,
        attachments: vec![],
        session_id: SessionId::new(),
        turn_id: TurnId::new(),
        cwd: PathBuf::from("."),
        permission_profile: PermissionProfile::Web,
        web_enabled: false,
        image_tools_available: false,
        steering: None,
        approval: None,
        questions: None,
    }
}

fn image() -> ImageContent {
    ImageContent {
        mime_type: "image/png".into(),
        data: crate::image_tool::tests::generated_fixture().data,
    }
}

fn uploads(count: usize) -> Vec<PromptAttachment> {
    let image = image();
    (0..count)
        .map(|index| PromptAttachment::Image {
            name: format!("synthetic-{index}.png"),
            image: image.clone(),
        })
        .collect()
}

async fn compact_fixture(
    provider: ContextProvider,
    history: Vec<ChatMessage>,
    known_tokens: Option<u64>,
) -> (Arc<ContextProvider>, Vec<AppEvent>) {
    let provider = Arc::new(provider);
    let mut limits = AgentLimits::development(4);
    limits.max_context_tokens = provider.model.context_window_tokens as usize;
    limits.max_context_bytes = 16 * 1024 * 1024;
    let engine = AgentEngine::with_limits(
        provider.clone(),
        Arc::new(ToolRegistry::new()),
        &provider.model.id,
        limits,
    );
    let id = SessionId::new();
    engine.histories.lock().await.insert(id.clone(), history);
    if let Some(tokens) = known_tokens {
        engine.update_context_tokens(&id, tokens).await;
    }
    let (tx, mut rx) = mpsc::channel(APP_EVENT_QUEUE_CAPACITY);
    engine
        .compact(&id, None, tx, CancellationToken::new())
        .await
        .unwrap();
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }
    (provider, events)
}

#[tokio::test]
async fn interleaved_images_and_text_compact_in_one_native_request() {
    let mut provider = ContextProvider::new("synthetic-vision");
    provider.model.context_window_tokens = 1_048_576;
    let mut history = vec![ChatMessage::text(ChatRole::System, "original system")];
    for index in 0..14 {
        let mut message = ChatMessage::text(ChatRole::User, format!("Image marker {index}"));
        message.images.push(image());
        history.push(message);
        history.push(ChatMessage::text(
            ChatRole::Assistant,
            format!("Observation marker {index}"),
        ));
    }
    history.push(ChatMessage::text(
        ChatRole::System,
        "Historical system note",
    ));
    let original = history.clone();
    let (provider, events) = compact_fixture(provider, history, None).await;
    let requests = provider.requests.lock().await;
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    request.validate().unwrap();
    assert_eq!(
        request
            .messages
            .iter()
            .map(|m| m.images.len())
            .sum::<usize>(),
        14
    );
    assert_eq!(
        &request.messages[1..original.len() - 1],
        &original[1..original.len() - 1]
    );
    assert_eq!(request.messages[original.len() - 1].role, ChatRole::User);
    assert!(
        request.messages[original.len() - 1]
            .content
            .contains("HISTORICAL SYSTEM NOTE")
    );
    assert!(
        request
            .messages
            .last()
            .unwrap()
            .content
            .contains("APPLICATION-GENERATED COMPACTION REQUEST")
    );
    assert!(events.iter().any(|event| matches!(event, AppEvent::CompactionProgressUpdated { progress } if progress.total == 1 && progress.completed == 1)));
    assert!(!events.iter().any(|event| matches!(
        event,
        AppEvent::TextDelta { .. } | AppEvent::ReasoningDelta { .. }
    )));
}

#[tokio::test]
async fn a_million_token_model_near_its_threshold_needs_only_one_summary_request() {
    let mut provider = ContextProvider::new("synthetic-vision");
    provider.model.context_window_tokens = 1_048_576;
    provider.model.max_output_tokens = 8192;
    let context_tokens = u64::from(provider.model.auto_compact_threshold_tokens());
    let history = vec![
        ChatMessage::text(ChatRole::System, "system"),
        ChatMessage::text(
            ChatRole::User,
            "abcd".repeat(usize::try_from(context_tokens).unwrap()),
        ),
    ];
    let (provider, _) = compact_fixture(provider, history.clone(), Some(context_tokens)).await;
    let requests = provider.requests.lock().await;
    assert_eq!(
        requests.len(),
        1,
        "85% context fits together with summary generation space"
    );
    assert_eq!(requests[0].max_output_tokens, Some(8192));
    assert_eq!(requests[0].messages[1], history[1]);
}

#[tokio::test]
async fn single_pass_keeps_reasoning_and_refusals_as_historical_context() {
    let provider = ContextProvider::new("synthetic-vision");
    let mut assistant = ChatMessage::text(ChatRole::Assistant, "Recorded result");
    assistant.reasoning_content = Some("Historical reasoning reference: route B".into());
    assistant.refusal = Some("Historical refusal reference: request C".into());
    let history = vec![
        ChatMessage::text(ChatRole::System, "system"),
        ChatMessage::text(ChatRole::User, "Earlier request"),
        assistant,
    ];
    let (provider, _) = compact_fixture(provider, history, None).await;
    let requests = provider.requests.lock().await;
    assert_eq!(requests.len(), 1);
    let historical = &requests[0].messages[2];
    assert!(historical.reasoning_content.is_none());
    assert!(historical.refusal.is_none());
    assert!(historical.content.contains("route B"));
    assert!(historical.content.contains("request C"));
    assert!(historical.content.contains("Recorded result"));
}

#[tokio::test]
async fn token_limited_fallback_batches_interleaved_messages_into_two_requests() {
    let provider = ContextProvider::new("direct:qwen3.8-27b");
    let mut history = vec![ChatMessage::text(ChatRole::System, "system")];
    for index in 0..30 {
        let mut message = ChatMessage::text(ChatRole::User, format!("Image marker {index}"));
        message.images.push(image());
        history.push(message);
        history.push(ChatMessage::text(
            ChatRole::Assistant,
            format!("Observation marker {index}"),
        ));
    }
    let (provider, events) = compact_fixture(provider, history, None).await;
    let requests = provider.requests.lock().await;
    assert_eq!(requests.len(), 2);
    let image_counts = requests
        .iter()
        .map(|r| r.messages.iter().map(|m| m.images.len()).sum::<usize>())
        .collect::<Vec<_>>();
    assert_eq!(image_counts.iter().sum::<usize>(), 30);
    assert!(image_counts.iter().all(|count| *count > 8));
    let transcript = requests
        .iter()
        .flat_map(|r| &r.messages)
        .map(|m| m.content.as_str())
        .collect::<String>();
    let positions = (0..30)
        .map(|i| transcript.find(&format!("Image marker {i}")).unwrap())
        .collect::<Vec<_>>();
    assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));
    assert!(
        requests[1].messages[1]
            .content
            .contains("Synthetic summary preserving image observations.")
    );
    assert!(events.iter().any(|event| matches!(event, AppEvent::CompactionProgressUpdated { progress } if progress.total == 2 && progress.completed == 2)));
}

#[tokio::test]
async fn compaction_reserves_output_space_before_choosing_one_request() {
    let provider = ContextProvider::new("synthetic-vision");
    let history = vec![
        ChatMessage::text(ChatRole::System, "system"),
        ChatMessage::text(ChatRole::User, "a".repeat(30_000 * 4)),
    ];
    let (provider, _) = compact_fixture(provider, history, Some(30_000)).await;
    let requests = provider.requests.lock().await;
    assert!(
        requests.len() > 1,
        "summary output must fit alongside its input"
    );
    assert!(
        requests
            .iter()
            .all(|r| r.max_output_tokens.unwrap_or(0) > 1)
    );
}

#[tokio::test]
async fn provider_payload_limit_also_splits_a_token_sized_image_history() {
    use base64::Engine as _;
    let provider = ContextProvider::new("direct:qwen3.8-27b");
    let mut bytes = vec![0_u8; 4 * 1024 * 1024];
    bytes[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
    let large = ImageContent {
        mime_type: "image/png".into(),
        data: base64::engine::general_purpose::STANDARD.encode(bytes),
    };
    let mut history = vec![ChatMessage::text(ChatRole::System, "system")];
    for _ in 0..8 {
        let mut message = ChatMessage::text(ChatRole::User, "Encoded image fixture");
        message.images.push(large.clone());
        history.push(message);
    }
    let (provider, _) = compact_fixture(provider, history, None).await;
    let requests = provider.requests.lock().await;
    assert_eq!(
        requests.len(),
        2,
        "token and image counts fit, but the provider request payload does not"
    );
    assert_eq!(
        requests
            .iter()
            .flat_map(|r| &r.messages)
            .map(|m| m.images.len())
            .sum::<usize>(),
        8
    );
    for request in requests.iter() {
        assert!(compaction_request_fits(&provider.model, request).unwrap());
    }
}

#[tokio::test]
async fn restoring_images_uses_the_selected_models_visual_token_allowance() {
    let provider = Arc::new(ContextProvider::new("direct:qwen3.8-27b"));
    let engine = AgentEngine::new(
        provider.clone(),
        Arc::new(ToolRegistry::new()),
        &provider.model.id,
        4,
    );
    let ctx = context();
    let journal = vec![EventEnvelope {
        schema_version: 1,
        turn_id: None,
        sequence: 1,
        occurred_at: chrono::Utc::now(),
        correlation_id: crate::app::CorrelationId::new(),
        origin: crate::app::Origin::Test,
        session_id: ctx.session_id.clone(),
        event: AppEvent::PromptAccepted {
            turn_id: ctx.turn_id.clone(),
            text: "Analyze these synthetic images".into(),
            attachments: uploads(4),
        },
    }];
    // Restore must also invalidate a previous in-memory count for this session.
    engine.update_context_tokens(&ctx.session_id, 80_000).await;
    engine
        .restore_session(&ctx.session_id, &journal)
        .await
        .unwrap();
    let (tx, _rx) = mpsc::channel(APP_EVENT_QUEUE_CAPACITY);
    engine
        .run(
            ctx,
            "Describe them again".into(),
            tx,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let requests = provider.requests.lock().await;
    assert_eq!(
        requests.len(),
        1,
        "restoration must not cause premature compaction"
    );
    assert!(!is_compaction(&requests[0]));
    assert_eq!(
        requests[0]
            .messages
            .iter()
            .map(|m| m.images.len())
            .sum::<usize>(),
        4
    );
}

#[tokio::test]
async fn context_guard_uses_the_same_visual_token_allowance_as_compaction() {
    let provider = Arc::new(ContextProvider::new("direct:qwen3.8-27b"));
    let mut limits = AgentLimits::development(4);
    limits.max_context_tokens = 12_000;
    let engine = AgentEngine::with_limits(
        provider.clone(),
        Arc::new(ToolRegistry::new()),
        &provider.model.id,
        limits,
    );
    let ctx = TurnContext {
        attachments: uploads(8),
        ..context()
    };
    let (tx, _rx) = mpsc::channel(APP_EVENT_QUEUE_CAPACITY);
    engine
        .run(
            ctx,
            "Analyze the images".into(),
            tx,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let requests = provider.requests.lock().await;
    assert_eq!(requests.len(), 1);
    assert!(!is_compaction(&requests[0]));
}

#[tokio::test]
async fn new_uploads_compact_existing_images_when_the_token_budget_requires_it() {
    let provider = Arc::new(ContextProvider::new("direct:qwen3.8-27b"));
    let engine = AgentEngine::new(
        provider.clone(),
        Arc::new(ToolRegistry::new()),
        &provider.model.id,
        4,
    );
    let ctx = TurnContext {
        attachments: uploads(10),
        ..context()
    };
    let message = axiom_inference::user_message("Analyze the earlier images", &uploads(10));
    engine.histories.lock().await.insert(
        ctx.session_id.clone(),
        vec![
            ChatMessage::text(ChatRole::System, "system"),
            message.clone(),
            message,
        ],
    );
    let (tx, _rx) = mpsc::channel(APP_EVENT_QUEUE_CAPACITY);
    engine
        .run(
            ctx,
            "Analyze this new image too".into(),
            tx,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let requests = provider.requests.lock().await;
    assert!(is_compaction(&requests[0]));
    let final_request = requests.last().unwrap();
    assert!(!is_compaction(final_request));
    assert_eq!(
        final_request
            .messages
            .iter()
            .map(|m| m.images.len())
            .sum::<usize>(),
        10
    );
}

#[tokio::test]
async fn astreaai_viewing_after_ten_uploads_reaches_the_provider() {
    use crate::image_tool::{ImageCatalog, ImageGenerationTool, ViewImageTool};
    let mut provider = ContextProvider::new("direct:qwen3.8-27b");
    let catalog = ImageCatalog::default();
    let image_id = catalog
        .add_generated(&crate::image_tool::tests::generated_fixture())
        .unwrap();
    provider.turns.get_mut().push_back(AssistantTurn {
        text: String::new(),
        reasoning: None,
        tool_calls: vec![axiom_inference::ToolCall {
            id: "synthetic-view-at-limit".into(),
            kind: "function".into(),
            function: axiom_inference::FunctionCall {
                name: "view_image".into(),
                arguments: serde_json::json!({"image_id":image_id}).to_string(),
            },
        }],
    });
    let provider = Arc::new(provider);
    let mut tools = ToolRegistry::new();
    tools
        .register(Arc::new(ImageGenerationTool {
            provider: provider.clone(),
        }))
        .unwrap();
    tools.register(Arc::new(ViewImageTool)).unwrap();
    let engine = AgentEngine::new(provider.clone(), Arc::new(tools), &provider.model.id, 4);
    let ctx = TurnContext {
        image_tools_available: true,
        attachments: uploads(10),
        ..context()
    };
    engine
        .image_catalogs
        .lock()
        .await
        .insert(ctx.session_id.clone(), catalog);
    let (tx, mut rx) = mpsc::channel(APP_EVENT_QUEUE_CAPACITY);
    engine
        .run(
            ctx,
            "Inspect the earlier result".into(),
            tx,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let requests = provider.requests.lock().await;
    assert_eq!(requests.len(), 2);
    for (request, expected) in requests.iter().zip([10, 11]) {
        assert_eq!(
            request
                .messages
                .iter()
                .map(|message| message.images.len())
                .sum::<usize>(),
            expected
        );
    }
    let mut viewed = false;
    while let Ok(event) = rx.try_recv() {
        if let AppEvent::ToolCompleted { success, .. } = event {
            assert!(success);
            viewed = true;
        }
    }
    assert!(viewed);
}

#[tokio::test]
async fn astreaai_compaction_preserves_multiple_ten_image_messages() {
    let provider = Arc::new(ContextProvider::new("direct:qwen3.8-27b"));
    let engine = AgentEngine::new(
        provider.clone(),
        Arc::new(ToolRegistry::new()),
        &provider.model.id,
        4,
    );
    let session_id = SessionId::new();
    let mut message = ChatMessage::text(ChatRole::User, "Keep these existing image observations.");
    message.images = vec![image(); 10];
    engine.histories.lock().await.insert(
        session_id.clone(),
        vec![
            ChatMessage::text(ChatRole::System, "system"),
            message.clone(),
            message,
        ],
    );
    let (tx, _rx) = mpsc::channel(APP_EVENT_QUEUE_CAPACITY);
    engine
        .compact(&session_id, None, tx, CancellationToken::new())
        .await
        .unwrap();
    let requests = provider.requests.lock().await;
    assert_eq!(requests.len(), 1, "all twenty images fit the token window");
    assert_eq!(
        requests
            .iter()
            .flat_map(|r| &r.messages)
            .map(|m| m.images.len())
            .sum::<usize>(),
        20
    );
    assert!(
        requests
            .iter()
            .flat_map(|r| &r.messages)
            .all(|message| message.images.len() <= 10)
    );
}

#[tokio::test]
async fn one_hundred_ten_images_fit_one_summary_when_the_token_window_allows() {
    let mut provider = ContextProvider::new("direct:qwen3.8-27b");
    provider.model.context_window_tokens = 1_048_576;
    let mut history = vec![ChatMessage::text(ChatRole::System, "system")];
    for batch in 0..11 {
        let mut message = ChatMessage::text(ChatRole::User, format!("Image batch {batch}"));
        message.images = vec![image(); 10];
        history.push(message);
    }
    let original = history.clone();
    let (provider, events) = compact_fixture(provider, history, None).await;
    let requests = provider.requests.lock().await;
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    request.validate().unwrap();
    assert_eq!(
        request
            .messages
            .iter()
            .map(|message| message.images.len())
            .sum::<usize>(),
        110
    );
    assert_eq!(&request.messages[1..original.len()], &original[1..]);
    assert!(events.iter().any(|event| matches!(event, AppEvent::CompactionProgressUpdated { progress } if progress.total == 1 && progress.completed == 1)));
}

#[tokio::test]
async fn viewing_an_image_adds_tokens_to_the_last_provider_report() {
    use crate::image_tool::{ImageCatalog, ImageGenerationTool, ViewImageTool};
    let mut provider = ContextProvider::new("direct:qwen3.8-27b");
    provider.usage = (26_900, 200);
    let catalog = ImageCatalog::default();
    let image_id = catalog
        .add_generated(&crate::image_tool::tests::generated_fixture())
        .unwrap();
    provider.turns.get_mut().push_back(AssistantTurn {
        text: String::new(),
        reasoning: None,
        tool_calls: vec![axiom_inference::ToolCall {
            id: "synthetic-view".into(),
            kind: "function".into(),
            function: axiom_inference::FunctionCall {
                name: "view_image".into(),
                arguments: serde_json::json!({"image_id":image_id}).to_string(),
            },
        }],
    });
    let provider = Arc::new(provider);
    let mut tools = ToolRegistry::new();
    tools
        .register(Arc::new(ImageGenerationTool {
            provider: provider.clone(),
        }))
        .unwrap();
    tools.register(Arc::new(ViewImageTool)).unwrap();
    let engine = AgentEngine::new(provider.clone(), Arc::new(tools), &provider.model.id, 4);
    let ctx = TurnContext {
        image_tools_available: true,
        ..context()
    };
    engine
        .image_catalogs
        .lock()
        .await
        .insert(ctx.session_id.clone(), catalog);
    let (tx, _rx) = mpsc::channel(APP_EVENT_QUEUE_CAPACITY);
    engine
        .run(
            ctx,
            "Inspect the synthetic image".into(),
            tx,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let requests = provider.requests.lock().await;
    assert!(!is_compaction(&requests[0]));
    assert!(
        is_compaction(&requests[1]),
        "the new observation crosses the compaction threshold"
    );
    assert!(!is_compaction(requests.last().unwrap()));
}

#[tokio::test]
async fn compaction_splits_image_inputs_to_leave_room_for_a_summary() {
    let provider = Arc::new(ContextProvider::new("synthetic-vision"));
    let engine = AgentEngine::new(
        provider.clone(),
        Arc::new(ToolRegistry::new()),
        &provider.model.id,
        4,
    );
    let session_id = SessionId::new();
    let mut message = ChatMessage::text(ChatRole::User, "Compare these synthetic images in order.");
    message.images = vec![image(); 10];
    engine.histories.lock().await.insert(
        session_id.clone(),
        vec![ChatMessage::text(ChatRole::System, "system"), message],
    );
    let (tx, _rx) = mpsc::channel(APP_EVENT_QUEUE_CAPACITY);
    engine
        .compact(&session_id, None, tx, CancellationToken::new())
        .await
        .unwrap();
    let requests = provider.requests.lock().await;
    assert!(
        requests.len() > 1,
        "the image inputs need multiple bounded stages"
    );
    assert_eq!(
        requests
            .iter()
            .flat_map(|r| &r.messages)
            .map(|m| m.images.len())
            .sum::<usize>(),
        10
    );
    assert!(
        requests
            .iter()
            .all(|r| r.max_output_tokens.unwrap() > 1 && r.tools.is_empty())
    );
    let image = image();
    assert!(
        requests
            .iter()
            .all(|r| { r.messages.iter().all(|m| !m.content.contains(&image.data)) })
    );
}

#[tokio::test]
async fn image_compaction_uses_available_context_beyond_the_ordinary_text_fragment_size() {
    let mut provider = ContextProvider::new("synthetic-vision");
    provider.model.context_window_tokens = 16_384;
    provider.model.supported_reasoning_efforts = vec![axiom_inference::ReasoningEffort::Medium];
    let provider = Arc::new(provider);
    let engine = AgentEngine::new(
        provider.clone(),
        Arc::new(ToolRegistry::new()),
        &provider.model.id,
        4,
    );
    let session_id = SessionId::new();
    let mut message = ChatMessage::text(ChatRole::User, "Keep the image observation.");
    message.images.push(image());
    engine.histories.lock().await.insert(
        session_id.clone(),
        vec![ChatMessage::text(ChatRole::System, "system"), message],
    );
    let (tx, _rx) = mpsc::channel(APP_EVENT_QUEUE_CAPACITY);
    engine
        .compact(&session_id, None, tx, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(
        provider
            .requests
            .lock()
            .await
            .iter()
            .flat_map(|r| &r.messages)
            .map(|m| m.images.len())
            .sum::<usize>(),
        1
    );
}

#[tokio::test]
async fn compaction_leaves_output_room_for_reasoning_before_the_visible_summary() {
    let mut provider = ContextProvider::new("direct:qwen3.8-27b");
    provider.model.supported_reasoning_efforts = vec![axiom_inference::ReasoningEffort::Medium];
    provider.model.supported_thinking_modes = vec![axiom_inference::ThinkingMode::Enabled];
    provider.compaction_reasoning_tokens = 1024;
    let provider = Arc::new(provider);
    let mut limits = AgentLimits::development(4);
    limits.max_context_bytes = 4096;
    let engine = AgentEngine::with_limits(
        provider.clone(),
        Arc::new(ToolRegistry::new()),
        &provider.model.id,
        limits,
    );
    let session_id = SessionId::new();
    let mut message = ChatMessage::text(ChatRole::User, "Keep the image observation.");
    message.images.push(image());
    engine.histories.lock().await.insert(
        session_id.clone(),
        vec![ChatMessage::text(ChatRole::System, "system"), message],
    );
    let (tx, _rx) = mpsc::channel(APP_EVENT_QUEUE_CAPACITY);
    engine
        .compact(&session_id, None, tx, CancellationToken::new())
        .await
        .unwrap();
    let requests = provider.requests.lock().await;
    assert!(requests.iter().all(|r| r.max_output_tokens.unwrap() > 1024));
}

#[tokio::test]
async fn compaction_honors_advertised_output_limits_when_normal_requests_use_a_default() {
    let mut provider = ContextProvider::new("synthetic-tinfoil-vision");
    provider.model.provider_id = "tinfoil".into();
    provider.model.max_output_tokens = 512;
    provider.model.supported_reasoning_efforts = vec![axiom_inference::ReasoningEffort::Medium];
    let provider = Arc::new(provider);
    let engine = AgentEngine::new(
        provider.clone(),
        Arc::new(ToolRegistry::new()),
        &provider.model.id,
        4,
    );
    let session_id = SessionId::new();
    let mut message = ChatMessage::text(ChatRole::User, "Keep the image observation.");
    message.images.push(image());
    engine.histories.lock().await.insert(
        session_id.clone(),
        vec![ChatMessage::text(ChatRole::System, "system"), message],
    );
    let (tx, _rx) = mpsc::channel(APP_EVENT_QUEUE_CAPACITY);
    engine
        .compact(&session_id, None, tx, CancellationToken::new())
        .await
        .unwrap();
    assert!(
        provider
            .requests
            .lock()
            .await
            .iter()
            .all(|r| r.max_output_tokens.unwrap() <= 512)
    );
}

#[tokio::test]
async fn encoded_image_bytes_never_trigger_compaction_below_the_token_threshold() {
    let provider = Arc::new(ContextProvider::new("direct:qwen3.8-27b"));
    let mut limits = AgentLimits::development(4);
    limits.max_context_bytes = 128 * 1024;
    let engine = AgentEngine::with_limits(
        provider.clone(),
        Arc::new(ToolRegistry::new()),
        &provider.model.id,
        limits,
    );
    let ctx = context();
    let message = axiom_inference::user_message("Analyze the synthetic images", &uploads(4));
    engine.histories.lock().await.insert(
        ctx.session_id.clone(),
        vec![ChatMessage::text(ChatRole::System, "system"), message],
    );
    let (tx, mut rx) = mpsc::channel(APP_EVENT_QUEUE_CAPACITY);
    engine
        .run(
            ctx,
            "Continue the analysis".into(),
            tx,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let requests = provider.requests.lock().await;
    assert_eq!(requests.len(), 1);
    assert!(!is_compaction(&requests[0]));
    assert!(!is_compaction(requests.last().unwrap()));
    assert_eq!(
        requests
            .iter()
            .flat_map(|r| &r.messages)
            .map(|m| m.images.len())
            .sum::<usize>(),
        4
    );
    let mut reports = vec![];
    let mut compacted = false;
    while let Ok(event) = rx.try_recv() {
        match event {
            AppEvent::ContextCompacted { .. } => compacted = true,
            AppEvent::ContextUsageUpdated { usage } => reports.push(usage),
            _ => {}
        }
    }
    assert!(!compacted);
    assert_eq!(
        reports.len(),
        1,
        "summary requests must not replace conversation usage"
    );
    assert_eq!(reports[0].input_tokens, 8000);
}

#[test]
fn compaction_bounds_encoded_attachments_including_summary_request_overhead() {
    use base64::Engine as _;
    let model = ContextProvider::new("direct:qwen3.8-27b").model;
    let mut bytes = vec![0_u8; 4 * 1024 * 1024 - 256];
    bytes[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
    let image = ImageContent {
        mime_type: "image/png".into(),
        data: base64::engine::general_purpose::STANDARD.encode(bytes),
    };
    let mut message = ChatMessage::text(ChatRole::User, "Inspect these synthetic images.");
    message.images = vec![image; 3];
    InferenceRequest::streaming(model.id.clone(), vec![message.clone()], vec![])
        .validate()
        .unwrap();
    let overhead = "s".repeat(32 * 1024);
    let parts = compaction_inputs(&model, &[message], 16 * 1024, 20_000, overhead.len()).unwrap();
    assert!(
        parts.len() > 1,
        "summary metadata requires splitting nearly-full image inputs"
    );
    assert_eq!(parts.iter().map(|p| p.images.len()).sum::<usize>(), 3);
    for part in parts {
        let mut input = ChatMessage::text(ChatRole::User, format!("{overhead}{}", part.transcript));
        input.images = part.images;
        InferenceRequest::streaming(model.id.clone(), vec![input], vec![])
            .validate()
            .unwrap();
    }
}

#[test]
fn compaction_text_stays_within_the_message_limit_after_summary_overhead() {
    let model = ContextProvider::new("synthetic-vision").model;
    let message = ChatMessage::text(
        ChatRole::User,
        "x".repeat(axiom_inference::MAX_MESSAGE_TEXT_BYTES),
    );
    let overhead = "s".repeat(64 * 1024);
    let parts = compaction_inputs(
        &model,
        &[message],
        16 * 1024 * 1024,
        4 * 1024 * 1024,
        overhead.len(),
    )
    .unwrap();
    assert!(parts.len() > 1);
    for part in parts {
        let input = ChatMessage::text(ChatRole::User, format!("{overhead}{}", part.transcript));
        InferenceRequest::streaming(model.id.clone(), vec![input], vec![])
            .validate()
            .unwrap();
    }
}

#[test]
fn attachment_compaction_keeps_text_order_and_bounds_each_stage() {
    use base64::Engine as _;
    let model = ContextProvider::new("synthetic-vision").model;
    let mut message = ChatMessage::text(ChatRole::User, "synthetic 字😀 ".repeat(4000));
    message.images = (0..3)
        .map(|index| ImageContent {
            mime_type: "image/png".into(),
            data: base64::engine::general_purpose::STANDARD.encode(
                [
                    b"\x89PNG\r\n\x1a\n".as_slice(),
                    format!("fixture-{index}").as_bytes(),
                ]
                .concat(),
            ),
        })
        .collect();
    message.files = (0..2)
        .map(|index| axiom_inference::FileContent {
            name: format!("synthetic-{index}.txt"),
            mime_type: "text/plain".into(),
            data: "aGVsbG8=".into(),
        })
        .collect();
    let parts = compaction_inputs(&model, &[message.clone()], 4096, 10_000, 1024).unwrap();
    assert_eq!(
        parts
            .iter()
            .flat_map(|p| &p.images)
            .cloned()
            .collect::<Vec<_>>(),
        message.images
    );
    assert_eq!(
        parts
            .iter()
            .flat_map(|p| &p.files)
            .cloned()
            .collect::<Vec<_>>(),
        message.files
    );
    let transcript = parts
        .iter()
        .map(|p| {
            assert!(p.transcript.len() <= 4096);
            assert!(
                p.transcript.len().div_ceil(COMPACTION_BYTES_PER_TOKEN)
                    + p.images.len() * 8192
                    + p.files.len() * 8192
                    <= 10_000
            );
            if p.transcript.starts_with("Native attachments") {
                p.transcript.split_once('\n').unwrap().1
            } else {
                p.transcript.as_str()
            }
        })
        .collect::<String>();
    let mut text_message = message;
    text_message.images.clear();
    text_message.files.clear();
    assert_eq!(transcript, serde_json::to_string(&[text_message]).unwrap());
}
