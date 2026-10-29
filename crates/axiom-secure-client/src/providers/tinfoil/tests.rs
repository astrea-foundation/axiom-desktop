use super::*;
use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead as _, KeyInit as _},
};
use axiom_inference::{ChatMessage, ChatRole, FinishReason, ReasoningEffort};
use tinfoil_ehbp::{SessionRecoveryToken, compute_nonce, derive_response_keys};

fn model() -> ModelInfo {
    serde_json::from_value(json!({"id":"tinfoil-gpt-oss-120b","provider_id":"tinfoil",
        "upstream_model":"gpt-oss-120b","provider_base_url":BASE_URL,
        "e2ee_protocol":PROTOCOL,"e2ee_encryption_version":1,"attestation_protocol":ATTESTATION,
        "context_window_tokens":131_072,"max_output_tokens":8192,
        "supports_tools":true, "supported_reasoning_efforts":["low","medium","high"],
        "reasoning_parameters":{"low":{"reasoning_effort":"low"},"medium":{"reasoning_effort":"medium"},"high":{"reasoning_effort":"high"}}}))
    .unwrap()
}
#[allow(clippy::needless_pass_by_value)] // Fixture callers use temporary json! values.
fn sse(value: Value) -> Vec<u8> {
    format!("data: {value}\n\n").into_bytes()
}
#[allow(clippy::needless_pass_by_value)]
fn chunk(delta: Value, finish: Value) -> Vec<u8> {
    sse(
        json!({"id":"request-a","model":"deployment-alias","choices":[{"index":0,"delta":delta,"finish_reason":finish}]}),
    )
}
fn usage() -> Vec<u8> {
    sse(
        json!({"id":"request-a","model":"deployment-alias","choices":[],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}),
    )
}
fn stream() -> Vec<Vec<u8>> {
    vec![
        chunk(json!({"role":"assistant","content":"héllo"}), Value::Null),
        chunk(json!({}), json!("stop")),
        usage(),
        b"data: [DONE]\n\n".to_vec(),
    ]
}
fn parse(chunks: &[Vec<u8>]) -> Result<InferenceResponse> {
    let mut parser = response::StreamParser::new("gpt-oss-120b", 4096);
    for bytes in chunks {
        parser.push(bytes)?;
    }
    parser.finish()
}

#[test]
fn encrypted_terminal_is_mandatory_even_at_valid_frame_boundaries() {
    let chunks = stream();
    assert_eq!(parse(&chunks).unwrap().assistant.text, "héllo");
    for end in 0..chunks.len() {
        assert!(parse(&chunks[..end]).is_err());
    }
    assert!(parse(&[chunks[0].clone(), chunks[1].clone(), chunks[3].clone()]).is_err());
    assert!(parse(&[chunks[0].clone(), chunks[2].clone(), chunks[3].clone()]).is_err());
    let mut extra = chunks.clone();
    extra.push(chunks[0].clone());
    assert!(parse(&extra).is_err());
    // Every possible byte split, including within a multibyte UTF-8 character.
    let all = chunks.concat();
    for split in 0..all.len() {
        assert_eq!(
            parse(&[all[..split].to_vec(), all[split..].to_vec()])
                .unwrap()
                .assistant
                .text,
            "héllo"
        );
    }
}

#[test]
fn tool_arguments_accumulate_and_incomplete_calls_never_succeed() {
    let mut chunks = vec![
        chunk(
            json!({"tool_calls":[{"index":0,"id":"call-1","type":"function","function":{"name":"lookup","arguments":"{\"key\":"}}]}),
            Value::Null,
        ),
        chunk(
            json!({"tool_calls":[{"index":0,"function":{"arguments":"\"alpha\"}"}}]}),
            Value::Null,
        ),
        chunk(json!({}), json!("tool_calls")),
        usage(),
        b"data: [DONE]\n\n".to_vec(),
    ];
    let output = parse(&chunks).unwrap();
    assert_eq!(output.finish_reason, Some(FinishReason::ToolCalls));
    assert_eq!(
        output.assistant.tool_calls[0].function.arguments,
        r#"{"key":"alpha"}"#
    );
    chunks[2] = chunk(json!({}), json!("stop"));
    assert_eq!(
        parse(&chunks).unwrap().finish_reason,
        Some(FinishReason::ToolCalls)
    );
    chunks.remove(1);
    assert!(parse(&chunks).is_err());
}

#[test]
fn identity_usage_and_limits_fail_closed() {
    let chunks = stream();
    let mut mutated = chunks.clone();
    mutated[2] = String::from_utf8(chunks[2].clone())
        .unwrap()
        .replace("request-a", "other")
        .into_bytes();
    assert!(parse(&mutated).is_err());
    mutated[2] = sse(
        json!({"id":"request-a","model":"deployment-alias","choices":[],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":16}}),
    );
    assert!(parse(&mutated).is_err());
    let mut parser = response::StreamParser::new("gpt-oss-120b", 16);
    assert!(parser.push(&chunks[0]).is_err());
    let mut parser = response::StreamParser::new("gpt-oss-120b", 4096);
    assert!(parser.push(b"data: \xff\n\n").is_err());
}

fn encrypted_frames(
    chunks: &[Vec<u8>],
    token: &SessionRecoveryToken,
    nonce: &[u8],
) -> Vec<Vec<u8>> {
    let material = derive_response_keys(&token.exported_secret, &token.request_enc, nonce).unwrap();
    let cipher = Aes256Gcm::new_from_slice(&material.key).unwrap();
    chunks
        .iter()
        .enumerate()
        .map(|(index, plaintext)| {
            let nonce = compute_nonce(&material.nonce_base, u64::try_from(index).unwrap());
            let encrypted = cipher
                .encrypt(Nonce::from_slice(&nonce), plaintext.as_slice())
                .unwrap();
            let mut framed = u32::try_from(encrypted.len())
                .unwrap()
                .to_be_bytes()
                .to_vec();
            framed.extend(encrypted);
            framed
        })
        .collect()
}
fn open(
    frames: &[Vec<u8>],
    token: &SessionRecoveryToken,
    nonce: &[u8],
) -> anyhow::Result<InferenceResponse> {
    let mut decryptor = token.response_decryptor(nonce)?;
    let mut parser = response::StreamParser::new("gpt-oss-120b", 4096);
    for frame in frames {
        // Deliberately split the wire below both frame header and tag sizes.
        for bytes in frame.chunks(3) {
            for plaintext in decryptor.push(bytes)? {
                parser.push(&plaintext)?;
            }
        }
    }
    decryptor.finish()?;
    Ok(parser.finish()?)
}

#[test]
fn aead_binds_order_completion_and_request_context() {
    let token = SessionRecoveryToken::new(vec![1; 32], vec![2; 32]).unwrap();
    let nonce = [3; 32];
    let frames = encrypted_frames(&stream(), &token, &nonce);
    assert_eq!(
        open(&frames, &token, &nonce).unwrap().assistant.text,
        "héllo"
    );
    for end in 0..frames.len() {
        assert!(open(&frames[..end], &token, &nonce).is_err());
    }
    let mut damaged = frames.clone();
    damaged[0][8] ^= 1;
    assert!(open(&damaged, &token, &nonce).is_err());
    let mut reordered = frames.clone();
    reordered.swap(0, 1);
    assert!(open(&reordered, &token, &nonce).is_err());
    let mut duplicated = frames.clone();
    duplicated.insert(1, frames[0].clone());
    assert!(open(&duplicated, &token, &nonce).is_err());
    let wrong_request = SessionRecoveryToken::new(vec![4; 32], vec![5; 32]).unwrap();
    assert!(open(&frames, &wrong_request, &nonce).is_err());
    assert!(open(&frames, &token, &[9; 32]).is_err());
    let mut truncated = frames.clone();
    truncated.last_mut().unwrap().pop();
    assert!(open(&truncated, &token, &nonce).is_err());
}

#[test]
fn request_contract_preserves_tools_and_private_message_fields() {
    let model = model();
    let tools = vec![serde_json::from_value(json!({"type":"function","function":{"name":"lookup","description":"private description","parameters":{"type":"object"}}})).unwrap()];
    let mut request = InferenceRequest::streaming(
        &model.id,
        vec![
            ChatMessage::text(ChatRole::System, "private system"),
            ChatMessage::text(ChatRole::User, "private user"),
        ],
        tools,
    );
    request.tool_choice = ToolChoice::Named {
        name: "lookup".into(),
    };
    request.parallel_tool_calls = Some(false);
    request.response_format = ResponseFormat::JsonObject;
    let body = request_body(&model, &request, true).unwrap();
    assert_eq!(body["messages"][0]["content"], "private system");
    assert_eq!(
        body["tools"][0]["function"]["description"],
        "private description"
    );
    assert_eq!(body["tool_choice"]["function"]["name"], "lookup");
    assert_eq!(body["parallel_tool_calls"], false);
    request.parallel_tool_calls = Some(true);
    assert!(request_body(&model, &request, true).is_err());
    request.parallel_tool_calls = Some(false);
    assert_eq!(body["stream_options"]["include_usage"], true);
    request.tool_choice = ToolChoice::Named {
        name: "undefined".into(),
    };
    assert!(request_body(&model, &request, true).is_err());
    request.max_output_tokens = Some(8193);
    assert!(request_body(&model, &request, true).is_err());
}

#[test]
fn catalog_cannot_redirect_attestation_or_expand_compiled_contract() {
    let provider = TinfoilProvider::new(
        SecureClientConfig::new("https://relay.example").unwrap(),
        Arc::new(ApiCredential::new("test")),
    );
    let valid = model();
    provider.supports(&valid).unwrap();
    let mut discovered = valid.clone();
    discovered.id = "tinfoil-new-model".into();
    discovered.upstream_model = "new-model.2".into();
    provider.supports(&discovered).unwrap();
    for field in [
        "provider_id",
        "provider_base_url",
        "e2ee_protocol",
        "attestation_protocol",
    ] {
        let mut value = serde_json::to_value(&valid).unwrap();
        value[field] = json!("attacker");
        assert!(
            provider
                .supports(&serde_json::from_value(value).unwrap())
                .is_err()
        );
    }
}

#[test]
fn authenticated_completion_still_must_honor_tool_choice() {
    let response = json!({"id":"answer","model":"gpt-oss-120b", "choices":[{
        "index":0,"message":{"role":"assistant","content":"done"},"finish_reason":"stop"}],
        "usage":{"prompt_tokens":10,"completion_tokens":1,"total_tokens":11}});
    let bytes = serde_json::to_vec(&response).unwrap();
    let output = response::complete(&bytes, "gpt-oss-120b").unwrap();
    assert!(response::complete(&bytes[..bytes.len() - 1], "gpt-oss-120b").is_err());
    let mut request = InferenceRequest::streaming(
        "tinfoil-gpt-oss-120b",
        vec![ChatMessage::text(ChatRole::User, "use the tool")],
        vec![],
    );
    validate_requested_tools(&request, &output).unwrap();
    request.tool_choice = ToolChoice::Required;
    assert!(validate_requested_tools(&request, &output).is_err());
    request.tool_choice = ToolChoice::Named {
        name: "lookup".into(),
    };
    assert!(validate_requested_tools(&request, &output).is_err());
}

#[test]
fn discovered_reasoning_controls_map_levels_without_model_names() {
    for name in ["future-top-level", "future-template"] {
        let mut model = model();
        model.id = format!("tinfoil-{name}");
        model.upstream_model = name.into();
        for (effort, expected) in [
            (ReasoningEffort::Low, "low"),
            (ReasoningEffort::Medium, "high"),
            (ReasoningEffort::High, "max"),
        ] {
            let mut request = InferenceRequest::streaming(
                &model.id,
                vec![ChatMessage::text(ChatRole::User, "test")],
                vec![],
            );
            model.reasoning_parameters.insert(
                effort.as_str().into(),
                if name == "future-top-level" {
                    json!({"reasoning_effort":expected})
                } else {
                    json!({"chat_template_kwargs":{"reasoning_effort":expected}})
                },
            );
            request.reasoning_effort = effort;
            let body = request_body(&model, &request, true).unwrap();
            let actual = if name == "future-top-level" {
                &body["reasoning_effort"]
            } else {
                &body["chat_template_kwargs"]["reasoning_effort"]
            };
            assert_eq!(actual, expected);
        }
    }
}

#[test]
fn image_parts_are_supported_only_inside_the_encrypted_body_of_a_vision_model() {
    let mut vision = model();
    vision.upstream_model = "future-vision-model".into();
    vision.supports_images = true;
    let image = axiom_inference::ImageContent {
        mime_type: "image/png".into(),
        data: "iVBORw0KGgpmaXh0dXJl".into(),
    };
    let mut message = ChatMessage::text(ChatRole::User, "describe this private image");
    message.images.push(image.clone());
    let request = InferenceRequest::streaming(&vision.id, vec![message], vec![]);
    let body = request_body(&vision, &request, true).unwrap();
    assert_eq!(
        body["messages"][0]["content"][1]["image_url"]["url"],
        image.data_url()
    );
    assert!(body["messages"][0].get("images").is_none());
    assert!(
        body.get("max_tokens").is_none(),
        "proxy omission leaves the upstream default intact"
    );
    assert!(request_body(&model(), &request, true).is_err());
}

#[test]
fn direct_files_stay_content_parts_and_capability_checks_fail_closed() {
    use base64::Engine as _;
    let mut catalog_model = model();
    catalog_model.file_mime_types = vec!["application/pdf".into()];
    let file = axiom_inference::FileContent {
        name: "private-report.pdf".into(),
        mime_type: "application/pdf".into(),
        data: base64::engine::general_purpose::STANDARD.encode(b"%PDF-1.7 private file bytes"),
    };
    let mut message = ChatMessage::text(ChatRole::User, "Summarize");
    message.files.push(file.clone());
    let request = InferenceRequest::streaming(&catalog_model.id, vec![message], vec![]);
    let body = request_body(&catalog_model, &request, true).unwrap();
    assert_eq!(
        body["messages"][0]["content"][1],
        json!({"type":"file","file":{"filename":file.name,"file_data":file.data_url()}})
    );
    assert!(body["messages"][0].get("files").is_none());
    assert!(
        !body["messages"][0]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains(&file.data)
    );
    catalog_model.file_mime_types.clear();
    assert!(request_body(&catalog_model, &request, true).is_err());
}

#[test]
fn new_catalog_models_use_the_same_verified_protocol_and_reject_injected_controls() {
    let provider = TinfoilProvider::new(
        SecureClientConfig::new("https://relay.example").unwrap(),
        Arc::new(ApiCredential::new("test")),
    );
    let mut discovered = model();
    discovered.upstream_model = "future-model".into();
    discovered.id = "tinfoil-future-model".into();
    provider.supports(&discovered).unwrap();
    discovered.reasoning_parameters.insert(
        "medium".into(),
        json!({"messages":[{"content":"injected"}]}),
    );
    assert!(provider.supports(&discovered).is_err());
}

#[test]
fn discovered_controls_honor_selected_effort_and_explicit_thinking_disable() {
    let mut model = model();
    model.upstream_model = "new-generation.2".into();
    model.id = "tinfoil-new-generation-2".into();
    model
        .thinking_parameters
        .insert("enabled".into(), json!({"reasoning_effort":"medium"}));
    model
        .thinking_parameters
        .insert("disabled".into(), json!({"reasoning_effort":"none"}));
    let mut request = InferenceRequest::streaming(
        &model.id,
        vec![ChatMessage::text(ChatRole::User, "hello")],
        vec![],
    );
    request.reasoning_effort = ReasoningEffort::High;
    request.thinking_mode = ThinkingMode::Enabled;
    assert_eq!(
        request_body(&model, &request, true).unwrap()["reasoning_effort"],
        "high"
    );
    request.thinking_mode = ThinkingMode::Disabled;
    assert_eq!(
        request_body(&model, &request, true).unwrap()["reasoning_effort"],
        "none"
    );
    model
        .reasoning_parameters
        .insert("high".into(), json!({"messages":[]}));
    request.thinking_mode = ThinkingMode::Enabled;
    assert!(request_body(&model, &request, true).is_err());
}
