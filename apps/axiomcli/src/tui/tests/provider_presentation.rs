use super::*;

use axiom_inference::{InvocationPurpose, InvocationState, RequestUsage};

fn state() -> TuiState {
    TuiState::new(
        PathBuf::from("/tmp/project"),
        "deepseek-v4-flash".into(),
        PermissionProfile::Confirm,
    )
}

fn lines_text(lines: Vec<ratatui::text::Line<'static>>) -> String {
    lines
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn security_inspector_retains_failure_details_independently_of_the_status_line() {
    let mut state = state();
    state.security_evidence = Some(security_evidence());
    state.fail_security("Provider TDX status: OutOfDate\nUpdate required\x1b");
    state.status = "Unrelated status".into();
    assert_eq!(state.report_status(), ReportStatus::Failed);
    assert!(state.security_evidence.is_none());
    for view in [
        SecurityView::Summary,
        SecurityView::Workload,
        SecurityView::Raw,
    ] {
        let text = lines_text(security_overlay_lines(&state, view, state.options.theme()));
        assert!(text.contains("Failure reason:"));
        assert!(text.contains("OutOfDate"));
        assert!(text.contains("Update required"));
        assert!(!text.contains('\x1b'));
    }
    state.open_security();
    assert!(render_symbols(100, 30, &state).contains("OutOfDate"));
    for status in [
        crate::app::SecurityStatus::Verifying,
        crate::app::SecurityStatus::Verified,
        crate::app::SecurityStatus::Unverified,
    ] {
        state.apply(&AppEvent::SecurityStatusChanged { state: status });
        assert!(state.security_error.is_none());
        state.fail_security("previous failure");
    }
    state.reset_security();
    assert!(state.security_error.is_none());
}

#[test]
fn report_freshness_is_model_bound_and_never_promotes_missing_or_failed_evidence() {
    let mut evidence = security_evidence();
    evidence.verified_at_unix_seconds = 100;
    evidence.hard_expires_at_unix_seconds = Some(200);
    let model = evidence.model_id.clone();
    assert_eq!(
        report_status("SECURE", &model, Some(&evidence), 199),
        ReportStatus::Verified
    );
    assert_eq!(
        report_status("SECURE", &model, Some(&evidence), 200),
        ReportStatus::Expired
    );
    assert_eq!(
        report_status("SECURE", &model, Some(&evidence), 99),
        ReportStatus::Unavailable
    );
    assert_eq!(
        report_status("SECURE", "different-model", Some(&evidence), 150),
        ReportStatus::Unavailable
    );
    assert_eq!(
        report_status("SECURE", &model, None, 150),
        ReportStatus::Unavailable
    );
    assert_eq!(
        report_status("SECURITY FAILED", &model, Some(&evidence), 150),
        ReportStatus::Failed
    );
    assert_eq!(
        report_status("VERIFYING", &model, Some(&evidence), 150),
        ReportStatus::Verifying
    );
    evidence.hard_expires_at_unix_seconds = None;
    assert_eq!(
        report_status("SECURE", &model, Some(&evidence), 150),
        ReportStatus::Unavailable
    );
    evidence.hard_expires_at_unix_seconds = Some(200);
    evidence.checks[0].passed = false;
    assert_eq!(
        report_status("SECURE", &model, Some(&evidence), 150),
        ReportStatus::Failed
    );
    evidence.checks[0].passed = true;
    for rejected in [
        SecurityState::Rejected,
        SecurityState::Cancelled,
        SecurityState::Unverified,
        SecurityState::Expired,
    ] {
        evidence.state = rejected;
        assert_ne!(
            report_status("SECURE", &model, Some(&evidence), 150),
            ReportStatus::Verified
        );
    }
}

#[test]
fn idle_expired_report_has_no_current_verified_claim_even_without_animation() {
    let mut state = state();
    state.options.animation = false;
    state.security = "SECURE";
    let mut evidence = security_evidence();
    evidence.verified_at_unix_seconds = 100;
    evidence.hard_expires_at_unix_seconds = Some(200);
    state.security_evidence = Some(evidence);
    let header = render_symbols(100, 30, &state);
    assert!(header.contains("PROOF EXPIRED"));
    assert!(!header.contains("TEE VERIFIED"));
    state.open_security();
    let overlay = render_symbols(120, 42, &state);
    assert!(overlay.contains("report has expired"));
    assert!(overlay.contains("Expires"));
    assert!(!overlay.contains("TEE VERIFIED"));
    state.security = "SECURITY FAILED";
    let failed = render_symbols(120, 42, &state);
    assert!(failed.contains("Verification failed"));
    assert!(!failed.contains("TEE VERIFIED"));
}

#[test]
fn evidence_views_support_current_and_unknown_provider_shapes() {
    let cases = [
        (
            "near",
            "near-tdx-nvidia-v2",
            "near-v3",
            "intel_mr_td",
            "td-measurement",
            Some("{\"docker_compose_file\":\"services: {}\"}"),
        ),
        (
            "future-gateway",
            "gateway-evidence-v1",
            "gateway-e2ee-v1",
            "gateway_source_commit_full",
            "abcdef0123456789abcdef0123456789abcdef0123",
            None,
        ),
        (
            "tinfoil",
            "tinfoil-snp-sigstore-v1",
            "tinfoil-ehbp-v1",
            "worker_verification",
            "Model workers verified by the measured Tinfoil router",
            Some("{\"release_tag\":\"release-123\"}"),
        ),
        (
            "future-provider",
            "different-evidence-v1",
            "different-e2ee-v1",
            "custom_identity",
            "opaque-public-identity",
            Some("A non-JSON provider document"),
        ),
    ];
    for (provider, protocol, encryption, key, value, document) in cases {
        let mut state = state();
        let mut evidence = security_evidence();
        evidence.provider_id = provider.into();
        evidence.attestation_protocol = protocol.into();
        evidence.e2ee_protocol = encryption.into();
        evidence.provider_claims = vec![
            EvidenceClaim {
                name: key.into(),
                value: value.into(),
            },
            EvidenceClaim {
                name: "source_repository".into(),
                value: "https://example.org/source".into(),
            },
        ];
        evidence.workload_manifest = document.map(str::to_owned);
        state.security = "SECURE";
        state.security_evidence = Some(evidence.clone());
        let theme = state.options.theme();
        let summary = lines_text(security_overlay_lines(&state, SecurityView::Summary, theme));
        assert!(summary.contains(protocol), "{provider}");
        assert!(summary.contains(&humanize_evidence_key(key)), "{provider}");
        assert!(summary.contains(value), "{provider}");
        assert!(summary.contains("https://example.org/source"));
        let fields = lines_text(security_overlay_lines(
            &state,
            SecurityView::Workload,
            theme,
        ));
        assert!(fields.contains(value));
        if let Some(document) = document {
            assert!(fields.contains(document));
        } else {
            assert!(fields.contains("no separate workload document"));
        }
        let raw = lines_text(workload_lines(&evidence, true, theme));
        let encoded = serde_json::to_string_pretty(&evidence).unwrap();
        assert!(
            raw.contains(&encoded),
            "raw view must include the entire report for {provider}"
        );
        assert!(!raw.contains("app_compose bytes"));
        assert!(!raw.contains("configuration committed to by the quote"));
    }
}

#[test]
fn provider_fields_and_checks_cannot_emit_terminal_controls() {
    let mut evidence = security_evidence();
    evidence.provider_id = "custom\x1b]0;title\x07".into();
    evidence.checks[0].label = "check\x1b[31m".into();
    evidence.checks[0].status = "verified\x07".into();
    evidence.provider_claims.push(EvidenceClaim {
        name: "claim\x1b".into(),
        value: "value\x07".into(),
    });
    let text = lines_text(security_summary_lines(
        &evidence,
        ReportStatus::Verified,
        Theme::for_mode(ColorMode::TrueColor),
    ));
    assert!(!text.contains(['\x1b', '\x07']));
    assert!(text.contains("custom"));
    assert!(text.contains("value"));
}

fn context_usage() -> axiom_acp_extension::ContextUsage {
    axiom_acp_extension::ContextUsage {
        input_tokens: 400,
        output_tokens: 100,
        model_id: "deepseek-v4-flash".into(),
        reported_at: "2026-09-21T12:00:00Z".into(),
        context_window_tokens: Some(1000),
        auto_compact_threshold_tokens: Some(850),
    }
}

#[test]
fn context_feedback_uses_reported_capacity_and_survives_model_switches() {
    let mut state = state();
    assert!(state.usage_details().contains("No completed request usage"));
    assert!(render_symbols(42, 10, &state).contains("Context unknown"));
    state.apply(&AppEvent::ContextUsageUpdated {
        usage: context_usage(),
    });
    for (width, height) in [(42, 10), (80, 24), (120, 40)] {
        assert!(render_symbols(width, height, &state).contains("Context 50%"));
    }
    let details = state.usage_details();
    assert!(details.contains("Input: 400"));
    assert!(details.contains("Auto-compacts at: 850 tokens (85%)"));
    state.apply(&AppEvent::ModelChanged {
        model: "different-model".into(),
    });
    assert_eq!(state.context_summary(), "Context 50%");
    assert!(state.usage_details().contains("previous model"));
    state.overlay = Some(Overlay::Usage);
    assert!(render_symbols(100, 35, &state).contains("Last reported conversation request"));
    let mut unknown = context_usage();
    unknown.context_window_tokens = None;
    state.apply(&AppEvent::ContextUsageUpdated { usage: unknown });
    assert_eq!(state.context_summary(), "Context size unknown");
    assert!(state.usage_details().contains("threshold unavailable"));
}

#[test]
fn reasoning_controls_follow_catalog_capabilities_without_provider_names() {
    let mut state = state();
    state.model_details.insert(
        state.model.clone(),
        axiom_inference::ModelInfo {
            id: state.model.clone(),
            supported_thinking_modes: vec![
                axiom_inference::ThinkingMode::Enabled,
                axiom_inference::ThinkingMode::Disabled,
            ],
            ..axiom_inference::ModelInfo::default()
        },
    );
    state.thinking = ThinkingLevel::Enabled;
    assert!(render_symbols(120, 30, &state).contains("Thinking On"));
    state.input = "/thinking ".into();
    assert_eq!(
        state
            .slash_candidates()
            .iter()
            .map(|item| item.label.as_str())
            .collect::<Vec<_>>(),
        ["enabled", "disabled"]
    );
    state.complete_slash();
    assert_eq!(state.input, "/thinking enabled");
    state.complete_slash();
    assert_eq!(state.input, "/thinking disabled");

    state.model_details.insert(
        "effort-model".into(),
        axiom_inference::ModelInfo {
            id: "effort-model".into(),
            supported_reasoning_efforts: vec![
                axiom_inference::ReasoningEffort::Low,
                axiom_inference::ReasoningEffort::High,
            ],
            ..axiom_inference::ModelInfo::default()
        },
    );
    state.apply(&AppEvent::ModelChanged {
        model: "effort-model".into(),
    });
    assert!(state.slash_completion.is_none());
    state.input = "/reasoning ".into();
    assert_eq!(
        state
            .slash_candidates()
            .iter()
            .map(|item| item.label.as_str())
            .collect::<Vec<_>>(),
        ["low", "high"]
    );
    state.input = "/thinking h".into();
    assert!(state.invalid_slash_has_completion());
    state.complete_slash();
    assert_eq!(state.input, "/thinking high");
    state
        .model_details
        .get_mut("effort-model")
        .unwrap()
        .supported_reasoning_efforts
        .clear();
    state.reset_slash_completion();
    state.input = "/thinking ".into();
    assert!(state.slash_candidates().is_empty());
    state.input.clear();
    assert!(!render_symbols(120, 30, &state).contains("Thinking On"));
}

fn request(turn: &TurnId) -> RequestUsage {
    RequestUsage {
        request_id: "request-1".into(),
        turn_id: Some(turn.to_string()),
        model_id: "deepseek-v4-flash".into(),
        provider_id: "arbitrary-provider".into(),
        state: InvocationState::Completed,
        finish_reason: Some("length".into()),
        response_verified: true,
        started_at_ms: "1000".into(),
        ..RequestUsage::default()
    }
}

#[test]
fn output_limit_notice_requires_authenticated_completion_not_settlement() {
    let mut state = state();
    let turn = TurnId::new();
    state.apply(&AppEvent::PromptAccepted {
        turn_id: turn.clone(),
        text: "Write a long answer".into(),
        attachments: Vec::new(),
    });
    state.apply(&AppEvent::TextDelta {
        turn_id: turn.clone(),
        text: "An answer".into(),
    });
    let mut usage = request(&turn);
    state.apply(&AppEvent::RequestUsageUpdated {
        usage: usage.clone(),
    });
    assert!(
        !render_symbols(120, 42, &state).contains("output limit"),
        "running output stays provisional"
    );
    state.apply(&AppEvent::TurnCompleted {
        turn_id: turn.clone(),
    });
    assert!(render_symbols(120, 42, &state).contains("The model reached its output limit"));
    usage.response_verified = false;
    usage.settled = true;
    state.apply(&AppEvent::RequestUsageUpdated {
        usage: usage.clone(),
    });
    assert!(!render_symbols(120, 42, &state).contains("output limit"));
    usage.response_verified = true;
    usage.state = InvocationState::Cancelled;
    state.apply(&AppEvent::RequestUsageUpdated {
        usage: usage.clone(),
    });
    assert!(!render_symbols(120, 42, &state).contains("output limit"));
    usage.state = InvocationState::Completed;
    state.apply(&AppEvent::RequestUsageUpdated { usage });
    state.apply(&AppEvent::TurnCancelled {
        turn_id: turn.clone(),
    });
    assert!(!render_symbols(120, 42, &state).contains("output limit"));
}

#[test]
fn output_limit_notice_uses_last_conversation_request_and_excludes_compaction() {
    let mut state = state();
    let turn = TurnId::new();
    let first = request(&turn);
    state.apply(&AppEvent::RequestUsageUpdated {
        usage: first.clone(),
    });
    assert!(state.response_hit_output_limit(&turn));
    let mut next = first.clone();
    next.request_id = "request-2".into();
    next.started_at_ms = "2000".into();
    next.finish_reason = Some("stop".into());
    state.apply(&AppEvent::RequestUsageUpdated { usage: next });
    assert!(!state.response_hit_output_limit(&turn));
    let mut compact = first;
    compact.request_id = "compaction-3".into();
    compact.started_at_ms = "3000".into();
    compact.purpose = InvocationPurpose::Compaction;
    state.apply(&AppEvent::RequestUsageUpdated { usage: compact });
    assert!(!state.response_hit_output_limit(&turn));
}

#[test]
fn slash_help_describes_current_controls_and_usage_is_a_local_command() {
    let help = slash_help_text();
    assert!(help.contains("/usage"));
    assert!(!help.contains("crypto permission"));
    assert!(!help.contains("or use an Axiom API key"));
    assert!(matches!(
        crate::slash::parse("/usage"),
        Some(Ok(crate::slash::SlashCommand::Usage))
    ));
    assert!(matches!(crate::slash::parse("/usage extra"), Some(Err(_))));
    assert_eq!(crate::slash::completions("/usa")[0].input, "/usage");
}

#[tokio::test]
async fn resume_restores_context_and_authenticated_finish_without_restoring_live_proof() {
    use crate::{
        agent::{EchoTurnRunner, TurnRunner},
        app::{CorrelationId, EventEnvelope, SessionId},
    };
    use std::sync::Arc;

    let directory = tempfile::tempdir().unwrap();
    let cwd = directory.path().canonicalize().unwrap();
    let path = directory.path().join("state.sqlite3");
    let session = SessionId::new();
    let turn = TurnId::new();
    {
        let store = SessionStore::open(&path).unwrap();
        let events = [
            AppEvent::SessionCreated {
                cwd: cwd.clone(),
                origin: Origin::Tui,
                profile: PermissionProfile::Confirm,
            },
            AppEvent::ModelChanged {
                model: "deepseek-v4-flash".into(),
            },
            AppEvent::PromptAccepted {
                turn_id: turn.clone(),
                text: "Long response".into(),
                attachments: Vec::new(),
            },
            AppEvent::TurnStarted {
                turn_id: turn.clone(),
            },
            AppEvent::TextDelta {
                turn_id: turn.clone(),
                text: "Retained answer".into(),
            },
            AppEvent::RequestUsageUpdated {
                usage: request(&turn),
            },
            AppEvent::ContextUsageUpdated {
                usage: context_usage(),
            },
            AppEvent::ResponseVerified {
                turn_id: turn.clone(),
            },
            AppEvent::TurnCompleted { turn_id: turn },
        ];
        for (index, event) in events.into_iter().enumerate() {
            store
                .append(&EventEnvelope {
                    schema_version: 1,
                    sequence: u64::try_from(index + 1).unwrap(),
                    occurred_at: chrono::Utc::now(),
                    correlation_id: CorrelationId::new(),
                    origin: Origin::Tui,
                    session_id: session.clone(),
                    event,
                })
                .unwrap();
        }
    }
    let store = SessionStore::open(&path).unwrap();
    let runner: Arc<dyn TurnRunner> = Arc::new(EchoTurnRunner);
    let state = restore_tui_session(
        &Runtime::new(32),
        &runner,
        &store,
        &cwd,
        &session,
        TuiState::new(
            cwd.clone(),
            "deepseek-v4-flash".into(),
            PermissionProfile::Confirm,
        ),
    )
    .await
    .unwrap();
    assert_eq!(state.context_summary(), "Context 50%");
    assert!(
        state
            .usage_details()
            .contains("Auto-compacts at: 850 tokens")
    );
    assert!(render_symbols(120, 42, &state).contains("The model reached its output limit"));
    assert_eq!(state.report_status(), ReportStatus::Unverified);
    assert!(state.security_evidence.is_none());
}
