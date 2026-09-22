use std::{
    collections::BTreeMap,
    env,
    fs::{self},
    path::PathBuf,
};

use axiom_secure_client::SecurityEvidence;
use crossterm::{
    Command,
    event::{KeyCode, KeyEvent, KeyModifiers},
};
use ratatui::{
    Terminal,
    layout::Rect,
    style::{Color, Modifier},
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{
    agent::APP_EVENT_QUEUE_CAPACITY,
    app::{
        AppCommand, AppEvent, Origin, PermissionProfile, QuestionRequest, Runtime, ThinkingLevel,
        TurnId,
    },
    policy::{ApprovalHandler, ApprovalRequest, ApprovalResponse},
    session::{SessionStore, SessionSummary},
};

use super::{
    COMPOSER_MAX_CONTENT_ROWS, MIN_HEIGHT, MIN_WIDTH,
    attestation::*,
    brand,
    runtime::*,
    screens::*,
    state::*,
    text::*,
    theme::{Appearance, ColorMode, Theme},
};

use axiom_secure_client::{EvidenceCheck, EvidenceClaim, SecurityState};
use ratatui::backend::TestBackend;

mod input_routing;
mod proof_refresh;
mod provider_presentation;
mod resume;

#[test]
fn steering_keeps_completed_output_before_the_new_user_message() {
    let mut state = TuiState::new(
        PathBuf::from("/tmp/project"),
        "model".into(),
        PermissionProfile::Confirm,
    );
    let turn = TurnId::new();
    state.apply(&AppEvent::PromptAccepted {
        attachments: Vec::new(),
        turn_id: turn.clone(),
        text: "Initial request".into(),
    });
    state.apply(&AppEvent::TurnStarted {
        turn_id: turn.clone(),
    });
    state.apply(&AppEvent::TextDelta {
        turn_id: turn.clone(),
        text: "Original output".into(),
    });
    let first = state.task_index(&turn).unwrap();
    state.apply(&AppEvent::SteeringApplied {
        turn_id: turn.clone(),
        client_item_id: "input".into(),
        text: "New direction".into(),
    });
    let second = state.task_index(&turn).unwrap();
    assert!(second > first);
    state.apply(&AppEvent::TextDelta {
        turn_id: turn.clone(),
        text: "Revised output".into(),
    });
    assert!(state.running);
    let rendered = render_symbols(120, 42, &state);
    assert!(rendered.contains("Original output"));
    assert!(rendered.contains("New direction"));
    assert!(rendered.contains("Revised output"));
    assert!(rendered.find("Original output") < rendered.find("New direction"));
    assert!(rendered.find("New direction") < rendered.find("Revised output"));
    state.apply(&AppEvent::TurnCompleted { turn_id: turn });
    assert!(!state.running);
}

#[test]
fn live_tui_account_change_closes_old_store_and_requires_restart() {
    let root = tempfile::tempdir().expect("data root");
    let paths =
        crate::paths::AxiomPaths::from_roots(root.path().join("config"), root.path().join("data"));
    paths.prepare().expect("prepare paths");
    let store = SessionStore::account_routed(paths, crate::paths::FrontendKind::Cli);
    store.activate_account("account-a").expect("activate A");

    let error = activate_tui_account_store(&store, "account-b")
        .expect_err("live runtime cannot be relabeled to B");
    assert!(error.to_string().contains("restart AxiomCLI"));
    assert!(!store.is_active());
    assert_eq!(store.active_account_id(), None);
}

fn security_evidence() -> SecurityEvidence {
    let now = u64::try_from(chrono::Utc::now().timestamp()).unwrap();
    SecurityEvidence {
        state: SecurityState::Verified,
        provider_id: "near".into(),
        model_id: "deepseek-v4-flash".into(),
        attestation_protocol: "near-tdx-nvidia-v2".into(),
        e2ee_protocol: "near-v3".into(),
        e2ee_encryption_version: 2,
        trust_policy_version: "test-policy".into(),
        verified_at_unix_seconds: now - 1,
        attestation_generation: Some(4),
        hard_expires_at_unix_seconds: Some(now + 240),
        model_key_fingerprint: "11".repeat(32),
        tls_spki_fingerprint: Some("22".repeat(32)),
        checks: vec![EvidenceCheck {
            id: "intel_tdx".into(),
            label: "Intel TDX".into(),
            status: "UpToDate".into(),
            passed: true,
        }],
        provider_claims: vec![
            EvidenceClaim {
                name: "intel_mr_td".into(),
                value: "33".repeat(48),
            },
            EvidenceClaim {
                name: "workload_manifest_sha256".into(),
                value: "44".repeat(32),
            },
        ],
        workload_manifest: Some(
            serde_json::json!({
                "manifest_version": 2,
                "name": "dstack-nvidia-test",
                "runner": "docker-compose",
                "local_key_provider_enabled": false,
                "docker_compose_file": "services:\n  inference:\n    image: registry.example/inference@sha256:abcd\n  gateway:\n    image: registry.example/gateway@sha256:ef01\n",
            })
            .to_string(),
        ),
    }
}

#[test]
fn security_inspector_shows_current_summary_provider_evidence_and_raw_report() {
    let mut state = TuiState::new(
        PathBuf::from("/tmp/project"),
        "deepseek-v4-flash".into(),
        PermissionProfile::Confirm,
    );
    state.security = "SECURE";
    state.security_evidence = Some(security_evidence());
    state.open_security();

    let summary = render_symbols(120, 42, &state);
    assert!(summary.contains("Security"));
    assert!(summary.contains("TEE VERIFIED"));
    assert!(summary.contains("Intel TDX"));
    assert!(summary.contains("Expires"));
    assert!(!summary.contains("Axiom approval"));

    state.cycle_security_view();
    let workload = render_symbols(120, 42, &state);
    assert!(workload.contains("Provider evidence"));
    assert!(workload.contains("registry.example/inference@sha256:abcd"));

    state.cycle_security_view();
    let raw = render_symbols(120, 42, &state);
    assert!(raw.contains("Raw verification report"));
    assert!(raw.contains("attestation_protocol"));
    assert!(raw.contains("S save"));
}

#[test]
fn raw_attestation_view_opens_a_keyboard_save_picker() {
    let directory = tempfile::tempdir().expect("tempdir");
    fs::create_dir(directory.path().join("exports")).expect("exports directory");
    let mut state = TuiState::new(
        directory.path().to_path_buf(),
        "deepseek-v4-flash".into(),
        PermissionProfile::Confirm,
    );
    state.security = "SECURE";
    state.security_evidence = Some(security_evidence());
    state.overlay = Some(Overlay::Security(SecurityView::Raw));

    state.open_attestation_save_picker();
    let picker = render_symbols(120, 42, &state);
    assert!(picker.contains("Save attestation"));
    assert!(picker.contains("axiom-attestation-"));
    assert!(picker.contains("exports/"));
    assert!(picker.contains("Enter save"));

    state.close_attestation_save_picker();
    assert!(matches!(
        state.overlay,
        Some(Overlay::Security(SecurityView::Raw))
    ));
}

#[test]
fn attestation_export_is_complete_pretty_json_and_never_overwrites() {
    let directory = tempfile::tempdir().expect("tempdir");
    let evidence = security_evidence();
    let destination =
        write_attestation_export(directory.path(), "axiom-attestation.json", &evidence)
            .expect("save evidence");
    let encoded = fs::read_to_string(&destination).expect("read evidence");
    assert!(encoded.ends_with('\n'));
    assert!(encoded.contains("\n  \"workload_manifest\""));
    assert_eq!(
        serde_json::from_str::<SecurityEvidence>(&encoded).expect("decode evidence"),
        evidence
    );

    let overwrite = write_attestation_export(
        directory.path(),
        "axiom-attestation.json",
        &security_evidence(),
    )
    .expect_err("existing evidence must not be overwritten");
    assert!(overwrite.to_string().contains("already exists"));
    assert!(
        write_attestation_export(directory.path(), "../escape.json", &security_evidence()).is_err()
    );
}

#[test]
fn ready_status_is_neutral_until_work_completes() {
    let state = TuiState::new(
        PathBuf::from("/tmp/project"),
        "model".into(),
        PermissionProfile::Confirm,
    );
    assert_eq!(state.status, "Ready");
    assert_eq!(state.status_tone, StatusTone::Neutral);
}

#[test]
fn retained_evidence_display_bounds_unicode_without_changing_exports() {
    let oversized = "🌸".repeat(MAX_WORKLOAD_DISPLAY_CHARS + 1);
    let (bounded, truncated) = bounded_workload_display(&oversized);
    assert!(truncated);
    assert_eq!(bounded.chars().count(), MAX_WORKLOAD_DISPLAY_CHARS);
    assert!(bounded.ends_with('🌸'));
    let mut evidence = security_evidence();
    evidence.workload_manifest = Some(oversized.clone());
    let directory = tempfile::tempdir().unwrap();
    let path = write_attestation_export(directory.path(), "report.json", &evidence).unwrap();
    let exported: SecurityEvidence =
        serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(
        exported.workload_manifest.as_deref(),
        Some(oversized.as_str())
    );
}

#[test]
fn strips_terminal_control_sequences() {
    assert_eq!(sanitize_terminal_text("ok\u{1b}[31mred\u{7}"), "ok[31mred");
}

#[test]
fn explicit_color_mode_overrides_ambient_no_color() {
    assert_eq!(
        ColorMode::detect_from(Some("truecolor"), true, Some("xterm-256color"), None),
        ColorMode::TrueColor
    );
    assert_eq!(
        ColorMode::detect_from(None, true, Some("xterm-256color"), Some("truecolor")),
        ColorMode::NoColor
    );
}

#[test]
fn terminal_background_commands_cover_padding_and_restore_the_host_color() {
    assert_eq!(
        Appearance::Dark.terminal_background(ColorMode::TrueColor),
        Some("#141416")
    );
    assert_eq!(
        Appearance::Dark.terminal_background(ColorMode::Ansi16),
        Some("#000000")
    );
    assert_eq!(
        Appearance::Dark.terminal_background(ColorMode::NoColor),
        None
    );

    let mut start = String::new();
    SetAxiomTerminalBackground(Some("#141416"))
        .write_ansi(&mut start)
        .expect("set background sequence");
    assert_eq!(start, "\x1b]11;#141416\x1b\\");

    let mut reset = String::new();
    ResetAxiomTerminalBackground(true)
        .write_ansi(&mut reset)
        .expect("reset background sequence");
    assert_eq!(reset, "\x1b]111\x1b\\");

    let mut no_color = String::new();
    SetAxiomTerminalBackground(None)
        .write_ansi(&mut no_color)
        .expect("no-color sequence");
    ResetAxiomTerminalBackground(false)
        .write_ansi(&mut no_color)
        .expect("no-color reset sequence");
    assert!(no_color.is_empty());
}

#[test]
fn ctrl_f_is_search_and_slash_is_reserved_for_commands() {
    assert!(is_search_shortcut(&KeyEvent::new(
        KeyCode::Char('f'),
        KeyModifiers::CONTROL,
    )));
    assert!(!is_search_shortcut(&KeyEvent::new(
        KeyCode::Char('/'),
        KeyModifiers::NONE,
    )));
    assert!(!is_search_shortcut(&KeyEvent::new(
        KeyCode::Char('f'),
        KeyModifiers::NONE,
    )));
}

#[test]
fn slash_command_menu_is_visible_and_filtered_while_typing() {
    let mut state = TuiState::new(
        PathBuf::from("/tmp/project"),
        "model".into(),
        PermissionProfile::Confirm,
    );
    state.input = "/thi".into();
    let output = render_symbols(100, 30, &state);
    assert!(output.contains("/thinking"));
    assert!(output.contains("reasoning mode or effort"));
    assert!(!output.contains("/compact ["));
}

#[test]
fn permissions_picker_selects_tool_profiles() {
    let mut state = TuiState::new(
        PathBuf::from("/tmp/project"),
        "model".into(),
        PermissionProfile::Confirm,
    );
    state.open_permissions();
    let output = render_symbols(100, 30, &state);
    assert!(output.contains("Tools"));
    assert!(output.contains("Full Access"));
    assert!(output.contains("one confirmation per use"));
    assert!(!output.contains("Control which tools"));
    assert!(!output.contains("Coming soon"));
    assert!(output.contains(&format!("{} Confirm", state.glyph("✓", "+"))));
    assert!(!output.contains("Current"));
    assert_eq!(
        state.selected_permission_profile(),
        Some(PermissionProfile::Confirm)
    );
    let compact = render_symbols(MIN_WIDTH, MIN_HEIGHT, &state);
    assert!(compact.contains("Confirm"));

    state.move_permission_selection(1);
    assert_ne!(
        state.selected_permission_profile(),
        Some(PermissionProfile::Confirm)
    );
}

#[test]
fn slash_shortcut_focuses_composer_and_invalid_enter_can_complete() {
    let mut state = TuiState::new(
        PathBuf::from("/tmp/project"),
        "model".into(),
        PermissionProfile::Confirm,
    );
    state.focus = Focus::Transcript;
    state.begin_slash_input();
    assert_eq!(state.focus, Focus::Composer);
    assert_eq!(state.input, "/");

    state.input = "/permi".into();
    assert!(state.invalid_slash_has_completion());
    state.complete_slash();
    assert_eq!(state.input, "/permissions");
    assert!(!state.invalid_slash_has_completion());

    state.input = "/not-a-command".into();
    assert!(!state.invalid_slash_has_completion());
}

#[test]
fn repeated_tab_cycles_matching_options_in_declared_order() {
    let mut state = TuiState::new(
        PathBuf::from("/tmp/project"),
        "model".into(),
        PermissionProfile::Confirm,
    );
    state.input = "/thinking m".into();

    state.complete_slash();
    assert_eq!(state.input, "/thinking minimal");
    state.complete_slash();
    assert_eq!(state.input, "/thinking medium");
    state.complete_slash();
    assert_eq!(state.input, "/thinking minimal");
}

#[test]
fn arrow_keys_highlight_slash_commands_and_options_before_accepting() {
    let mut state = TuiState::new(
        PathBuf::from("/tmp/project"),
        "model".into(),
        PermissionProfile::Confirm,
    );
    state.input = "/".into();
    assert!(state.can_navigate_slash());
    state.move_slash_selection(1);
    assert_eq!(state.input, "/", "navigation must preserve the draft");
    assert!(state.should_accept_slash_selection());
    assert!(
        render_symbols(100, 30, &state).contains(&format!("{} /compact", state.glyph("›", ">")))
    );
    state.accept_slash_selection();
    assert_eq!(state.input, "/compact ");

    state.reset_slash_completion();
    state.input = "/thinking ".into();
    state.move_slash_selection(1);
    state.move_slash_selection(1);
    state.move_slash_selection(1);
    state.move_slash_selection(1);
    assert_eq!(state.input, "/thinking ");
    assert!(render_symbols(100, 30, &state).contains(&format!("{} medium", state.glyph("›", ">"))));
    state.accept_slash_selection();
    assert_eq!(state.input, "/thinking medium");
}

#[test]
fn model_picker_filters_navigates_and_selects_catalog_values() {
    let mut state = TuiState::new(
        PathBuf::from("/tmp/project"),
        "grok-code".into(),
        PermissionProfile::Confirm,
    );
    state.model_candidates = vec!["glm-5-2".into(), "grok-code".into(), "qwen-code".into()];
    state.model_details.insert(
        "grok-code".into(),
        axiom_inference::ModelInfo {
            id: "grok-code".into(),
            short_label: "Grok Code".into(),
            input_price_microusd_per_million_tokens: Some(200_000),
            output_price_microusd_per_million_tokens: Some(1_500_000),
            ..axiom_inference::ModelInfo::default()
        },
    );
    state.model_catalog_loaded = true;

    state.open_model_picker();
    assert_eq!(state.selected_model().as_deref(), Some("grok-code"));

    state.push_model_picker_search('g');
    assert_eq!(state.selected_model().as_deref(), Some("glm-5-2"));
    state.move_model_picker(1);
    assert_eq!(state.selected_model().as_deref(), Some("grok-code"));

    let output = render_symbols(100, 30, &state);
    assert!(output.contains("Select model"));
    assert!(output.contains("Search"));
    assert!(output.contains("glm-5-2"));
    assert!(output.contains("Grok Code"));
    assert!(output.contains("$0.2/$1.5 · 1M"));
    assert!(!output.contains("qwen-code"));
}

#[test]
fn resume_picker_filters_navigates_and_selects_saved_transcripts() {
    let mut state = TuiState::new(
        PathBuf::from("/tmp/project"),
        "grok-code".into(),
        PermissionProfile::Confirm,
    );
    let sessions = vec![
        SessionSummary {
            id: "11111111-1111-1111-1111-111111111111".into(),
            title: Some("Renderer repair".into()),
            cwd: PathBuf::from("/tmp/project"),
            origin: "tui".into(),
            profile: "confirm".into(),
            archived: false,
            updated_at: "2026-08-22T16:42:00Z".into(),
        },
        SessionSummary {
            id: "22222222-2222-2222-2222-222222222222".into(),
            title: Some("Release verification".into()),
            cwd: PathBuf::from("/tmp/project"),
            origin: "tui".into(),
            profile: "confirm".into(),
            archived: false,
            updated_at: "2026-08-21T10:15:00Z".into(),
        },
    ];

    state.open_resume_picker(sessions);
    assert_eq!(
        state
            .selected_resume_session()
            .expect("selected")
            .title
            .as_deref(),
        Some("Renderer repair")
    );
    state.push_resume_picker_search('r');
    state.push_resume_picker_search('e');
    state.move_resume_picker(1);
    assert_eq!(
        state
            .selected_resume_session()
            .expect("selected")
            .title
            .as_deref(),
        Some("Release verification")
    );

    let output = render_symbols(100, 30, &state);
    assert!(output.contains("Resume transcript"));
    assert!(output.contains("Renderer repair"));
    assert!(output.contains("Release verification"));
    assert!(output.contains("2026-08-22 16:42"));
}

#[test]
fn delete_picker_searches_multiselects_selects_all_and_confirms() {
    let mut state = TuiState::new(
        PathBuf::from("/tmp/project"),
        "grok-code".into(),
        PermissionProfile::Confirm,
    );
    let sessions = vec![
        SessionSummary {
            id: "11111111-1111-1111-1111-111111111111".into(),
            title: Some("Renderer repair".into()),
            cwd: PathBuf::from("/tmp/project"),
            origin: "tui".into(),
            profile: "confirm".into(),
            archived: false,
            updated_at: "2026-08-22T16:42:00Z".into(),
        },
        SessionSummary {
            id: "22222222-2222-2222-2222-222222222222".into(),
            title: Some("Release verification".into()),
            cwd: PathBuf::from("/tmp/other"),
            origin: "tui".into(),
            profile: "confirm".into(),
            archived: true,
            updated_at: "2026-08-21T10:15:00Z".into(),
        },
    ];

    state.open_delete_picker(sessions);
    state.toggle_delete_picker_session();
    state.move_delete_picker(1);
    state.toggle_delete_picker_session();
    assert_eq!(state.marked_delete_session_ids().expect("IDs").len(), 2);
    state.toggle_all_delete_picker_matches();
    assert!(
        state
            .marked_delete_session_ids()
            .expect("cleared IDs")
            .is_empty()
    );
    state.toggle_all_delete_picker_matches();
    state.begin_delete_confirmation();
    assert!(matches!(
        state.overlay,
        Some(Overlay::DeletePicker {
            confirming: true,
            ..
        })
    ));

    let output = render_symbols(100, 30, &state);
    assert!(output.contains("Delete transcripts"));
    assert!(output.contains("2 selected"));
    assert!(output.contains("cannot be undone"));
    assert!(output.contains("Renderer repair"));
    assert!(output.contains("Release verification"));
}

#[test]
fn composer_status_marker_is_static_and_uses_terminal_outcome_color() {
    let mut state = TuiState::new(
        PathBuf::from("/tmp/project"),
        "model".into(),
        PermissionProfile::Confirm,
    );
    let turn_id = TurnId::new();
    state.apply(&AppEvent::PromptAccepted {
        attachments: Vec::new(),
        turn_id: turn_id.clone(),
        text: "test status".into(),
    });
    state.apply(&AppEvent::TurnStarted {
        turn_id: turn_id.clone(),
    });
    let working = render_symbols(100, 30, &state);
    state.tick_animation();
    let next_frame = render_symbols(100, 30, &state);
    let status_marker = state.glyph("●", "*").to_owned();
    assert!(working.contains(&format!("{status_marker} Working")));
    assert!(next_frame.contains(&format!("{status_marker} Working")));

    state.apply(&AppEvent::TurnCompleted { turn_id });
    assert_eq!(state.status_tone, StatusTone::Success);
    assert!(render_symbols(100, 30, &state).contains(&format!("{status_marker} Task complete")));
    state.apply(&AppEvent::ErrorRaised {
        turn_id: None,
        message: "failed".into(),
    });
    assert_eq!(state.status_tone, StatusTone::Error);
    assert!(render_symbols(100, 30, &state).contains(&format!("{status_marker} Error")));
}

#[test]
fn compaction_has_a_visible_compression_animation_and_static_status_marker() {
    let mut state = TuiState::with_options(
        PathBuf::from("/tmp/project"),
        "model".into(),
        PermissionProfile::Confirm,
        TuiOptions {
            appearance: Appearance::Dark,
            color_mode: ColorMode::TrueColor,
            ascii: false,
            animation: true,
        },
    );
    state.running = true;
    state.compacting = true;
    state.status = "Compacting conversation context…".into();
    state.status_tone = StatusTone::Active;
    state.apply(&AppEvent::ProviderStatusChanged {
        connected: true,
        detail: "Attested end-to-end encrypted inference connected".into(),
    });

    state.animation_frame = 0;
    let spread = render_symbols(100, 30, &state);
    state.animation_frame = 4;
    let compressed = render_symbols(100, 30, &state);
    assert!(spread.contains("● [▰   ▰   ▰]  Compacting"));
    assert!(compressed.contains("● [    ◆    ]  Compacting"));
    assert!(!spread.contains("Attested end-to-end"));
    assert_ne!(spread, compressed);

    state.options.ascii = true;
    assert_eq!(state.compaction_frame(), "[    @    ]");
    state.apply(&AppEvent::ContextCompacted {
        summary: "successor".into(),
        messages_before: 4,
    });
    assert!(!state.compacting);
    assert!(
        state.running,
        "automatic compaction does not finish the active turn"
    );
    assert!(render_symbols(100, 30, &state).contains("* Context compacted"));
}

#[test]
fn provider_connection_details_do_not_replace_task_status_but_failures_do() {
    let mut state = TuiState::new(
        PathBuf::from("/tmp/project"),
        "model".into(),
        PermissionProfile::Confirm,
    );
    let turn_id = TurnId::new();
    state.apply(&AppEvent::TurnStarted { turn_id });
    state.apply(&AppEvent::ProviderStatusChanged {
        connected: true,
        detail: "Attested end-to-end encrypted inference connected".into(),
    });
    assert_eq!(state.status, "Working…");

    state.apply(&AppEvent::ProviderStatusChanged {
        connected: false,
        detail: "connection closed".into(),
    });
    assert_eq!(state.status, "Provider unavailable: connection closed");
    assert_eq!(state.status_tone, StatusTone::Error);
}

#[test]
fn slash_setting_events_update_the_header_and_compaction_transcript() {
    let mut state = TuiState::new(
        PathBuf::from("/tmp/project"),
        "old-model".into(),
        PermissionProfile::Confirm,
    );
    state.apply(&AppEvent::SecurityStatusChanged {
        state: crate::app::SecurityStatus::Verified,
    });
    state.security_evidence = Some(security_evidence());
    state.apply(&AppEvent::ModelChanged {
        model: "new-model".into(),
    });
    assert_eq!(state.security, "NOT VERIFIED");
    assert!(state.security_evidence.is_none());
    state.apply(&AppEvent::ThinkingLevelChanged {
        level: ThinkingLevel::High,
    });
    state.apply(&AppEvent::ContextCompacted {
        summary: "hidden successor summary".into(),
        messages_before: 12,
    });
    let output = render_symbols(100, 30, &state);
    for label in ["new-model", "Thinking High", "Permissions Ask"] {
        assert!(output.contains(label), "missing {label}: {output}");
    }
    assert!(output.contains("CONTEXT COMPACTED"));
    assert!(output.contains("12 conversation message(s)"));
    assert!(!output.contains("hidden successor summary"));
}

#[tokio::test]
async fn thinking_choice_survives_quit_before_the_next_prompt() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("state.sqlite3");
    let session_id;
    {
        let store = SessionStore::open(&path).expect("store");
        let runtime = Runtime::new(16);
        session_id = runtime.session_id();
        let created = runtime
            .dispatch(AppCommand::CreateSession {
                session_id: session_id.clone(),
                cwd: directory.path().canonicalize().expect("cwd"),
                origin: Origin::Tui,
                profile: PermissionProfile::Confirm,
            })
            .await
            .expect("create session");
        store.append_all(&created).expect("persist session");
        let initial = runtime
            .dispatch(AppCommand::ChangeModelSettings {
                session_id: session_id.clone(),
                model: "near/model".into(),
                thinking: ThinkingLevel::Medium,
                reset_security: false,
            })
            .await
            .expect("initial settings");
        store
            .append_all(&initial)
            .expect("persist initial settings");
        let changed = runtime
            .dispatch(AppCommand::ChangeThinkingLevel {
                session_id: session_id.clone(),
                level: ThinkingLevel::High,
            })
            .await
            .expect("change thinking");
        persist_tui_thinking_change(Some(&store), &changed, "near/model", ThinkingLevel::High)
            .expect("persist thinking and new-thread preference");
    }

    let reopened = SessionStore::open(&path).expect("reopen after quit");
    let preferences = reopened.profile_preferences().expect("preferences");
    assert_eq!(preferences.model.as_deref(), Some("near/model"));
    assert_eq!(preferences.thinking_level, ThinkingLevel::High);
    assert_eq!(
        reopened
            .thread_snapshot(&session_id, None, 1)
            .expect("thread")
            .thread
            .thinking_level,
        ThinkingLevel::High
    );
}

#[test]
fn armed_exit_displays_the_double_ctrl_c_confirmation() {
    let mut state = TuiState::new(
        PathBuf::from("/tmp/project"),
        "model".into(),
        PermissionProfile::Confirm,
    );
    state.exit_armed = true;

    assert!(render_symbols(100, 30, &state).contains("Press Ctrl+C again to exit"));
}

#[test]
fn startup_has_desktop_eagle_and_wordmark_at_responsive_sizes() {
    let mut state = TuiState::with_options(
        PathBuf::from("/tmp/project"),
        "model".into(),
        PermissionProfile::Confirm,
        TuiOptions {
            appearance: Appearance::Dark,
            color_mode: ColorMode::TrueColor,
            ascii: false,
            animation: false,
        },
    );
    state.splash_message = "Think freely.";
    let output = render_symbols(100, 30, &state);
    assert!(output.contains("AXIOM CLI"));
    assert!(output.contains(concat!("v", env!("CARGO_PKG_VERSION"))));
    assert!(output.contains("Think freely."));
    assert!(output.contains(brand::EAGLE[8]));
    assert!(!output.contains("AxiomCLI is ready"));

    let compact = render_symbols(64, 30, &state);
    assert!(compact.contains(brand::COMPACT_EAGLE[5]));
    assert!(compact.contains("Think freely."));

    let narrow = render_symbols(54, 16, &state);
    assert!(narrow.contains("AXIOM CLI"));
    assert!(!narrow.contains('✿'));
    assert!(narrow.contains("Think freely."));
}

#[test]
fn base_background_paints_axiom_black_at_every_outer_cell() {
    let state = TuiState::with_options(
        PathBuf::from("/tmp/project"),
        "model".into(),
        PermissionProfile::Confirm,
        TuiOptions {
            appearance: Appearance::Dark,
            color_mode: ColorMode::TrueColor,
            ascii: false,
            animation: false,
        },
    );
    let backend = TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal.draw(|frame| render(frame, &state)).expect("draw");
    let buffer = terminal.backend().buffer();
    for position in [(0, 0), (99, 0), (0, 29), (99, 29)] {
        assert_eq!(buffer[position].bg, Color::Rgb(20, 20, 22));
    }
}

#[test]
fn escape_stop_cancels_a_running_turn_and_updates_the_controls() {
    let mut state = TuiState::new(
        PathBuf::from("/tmp/project"),
        "model".into(),
        PermissionProfile::Confirm,
    );
    let turn_id = TurnId::new();
    state.apply(&AppEvent::PromptAccepted {
        attachments: Vec::new(),
        turn_id: turn_id.clone(),
        text: "keep working".into(),
    });
    state.apply(&AppEvent::TurnStarted {
        turn_id: turn_id.clone(),
    });
    state.overlay = Some(Overlay::Help);
    state.search_active = true;
    state.answer_prompt = Some("answer".into());
    state
        .pending_attention
        .insert("question:one".into(), "answer".into());
    let cancellation = CancellationToken::new();

    assert!(stop_running_turn(&mut state, Some(&cancellation)));
    assert!(cancellation.is_cancelled());
    assert_eq!(state.status, "Stopping task…");
    assert!(state.overlay.is_none());
    assert!(!state.search_active);
    assert!(state.answer_prompt.is_none());
    let rendered = render_symbols(100, 30, &state);
    assert!(rendered.contains("Esc stop"));
    assert!(!rendered.contains("Enter send/toggle"));

    state.apply(&AppEvent::SecurityStatusChanged {
        state: crate::app::SecurityStatus::Verifying,
    });
    state.apply(&AppEvent::TurnCancelled { turn_id });
    assert_ne!(state.security, "VERIFYING");
    assert_eq!(state.status, "Cancelled");
    assert!(state.pending_attention.is_empty());
}

#[test]
fn escape_clears_the_composer_draft_and_resets_completion() {
    let mut state = TuiState::new(
        PathBuf::from("/tmp/project"),
        "model".into(),
        PermissionProfile::Confirm,
    );
    state.input = "/thinking m".into();
    state.complete_slash();
    state.focus = Focus::Transcript;

    assert!(state.clear_composer_draft());
    assert!(state.input.is_empty());
    assert!(state.slash_completion.is_none());
    assert_eq!(state.focus, Focus::Composer);
    assert_eq!(state.status, "Draft cleared");
    assert!(!state.clear_composer_draft());
}

#[test]
fn pending_attention_is_ordered_and_does_not_replace_message_draft() {
    let mut state = TuiState::new(
        PathBuf::from("/tmp/project"),
        "model".into(),
        PermissionProfile::Confirm,
    );
    state.input = "unfinished draft".into();
    state.apply(&AppEvent::BackgroundTaskChanged {
        task_id: "z-task".into(),
        state: "running".into(),
    });
    state.apply(&AppEvent::QuestionsAsked {
        request: QuestionRequest {
            request_id: "a-question".into(),
            questions: vec![crate::app::QuestionSpec {
                id: "one".into(),
                prompt: "Choose".into(),
                options: vec!["yes".into(), "no".into()],
                multiple: false,
                required: true,
            }],
        },
    });
    assert_eq!(state.input, "unfinished draft");
    let keys = state.pending_attention.keys().cloned().collect::<Vec<_>>();
    assert_eq!(
        keys,
        vec![
            "background:z-task".to_owned(),
            "question:a-question".to_owned()
        ]
    );
    state.apply(&AppEvent::QuestionsAnswered {
        request_id: "a-question".into(),
        answers: BTreeMap::from([("one".into(), vec!["yes".into()])]),
    });
    assert_eq!(state.pending_attention.len(), 1);
}

#[test]
fn optional_questions_accept_a_blank_answer_but_required_questions_do_not() {
    let optional = crate::app::QuestionSpec {
        id: "details".into(),
        prompt: "Anything else?".into(),
        options: Vec::new(),
        multiple: false,
        required: false,
    };
    assert_eq!(
        parse_tui_answer("   ", &optional).expect("skip"),
        Vec::<String>::new()
    );

    let required = crate::app::QuestionSpec {
        required: true,
        ..optional
    };
    assert!(parse_tui_answer("", &required).is_err());
}

#[test]
fn renders_at_minimum_and_typical_sizes() {
    for mode in [
        ColorMode::TrueColor,
        ColorMode::Ansi256,
        ColorMode::Ansi16,
        ColorMode::NoColor,
    ] {
        for (width, height) in [(42, 10), (60, 14), (100, 30), (160, 50)] {
            let backend = TestBackend::new(width, height);
            let mut terminal = Terminal::new(backend).expect("terminal");
            let state = TuiState::with_options(
                PathBuf::from("/tmp/axiom-project/with/a/very/long/path"),
                "test-model".into(),
                PermissionProfile::Confirm,
                TuiOptions {
                    appearance: Appearance::Dark,
                    color_mode: mode,
                    ascii: false,
                    animation: false,
                },
            );
            terminal.draw(|frame| render(frame, &state)).expect("draw");
            let contents = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(ratatui::buffer::Cell::symbol)
                .collect::<String>();
            assert!(contents.contains("Axiom"));
            assert!(contents.contains("NOT VERIFIED"));
            if mode == ColorMode::NoColor {
                assert!(
                    terminal
                        .backend()
                        .buffer()
                        .content()
                        .iter()
                        .all(|cell| { cell.fg == Color::Reset && cell.bg == Color::Reset })
                );
            }
        }
    }
}

#[test]
fn undersized_terminal_has_a_plain_actionable_fallback() {
    let state = TuiState::with_options(
        PathBuf::from("/tmp/project"),
        "model".into(),
        PermissionProfile::Observe,
        TuiOptions {
            appearance: Appearance::Dark,
            color_mode: ColorMode::NoColor,
            ascii: true,
            animation: false,
        },
    );
    let output = render_symbols(24, 5, &state);
    assert!(output.contains("AxiomCLI needs at least"));
    assert!(output.contains("42x10"));
}

#[test]
fn an_error_returns_to_a_composable_recoverable_state() {
    let mut state = TuiState::new(
        PathBuf::from("/tmp/project"),
        "model".into(),
        PermissionProfile::Confirm,
    );
    state.running = true;
    state.input = "retry with narrower scope".into();
    state.apply(&AppEvent::ErrorRaised {
        turn_id: Some(TurnId::new()),
        message: "provider disconnected; retry is safe".into(),
    });
    assert!(!state.running);
    assert_eq!(state.focus, Focus::Composer);
    assert_eq!(state.input, "retry with narrower scope");
    assert!(
        state
            .entries
            .last()
            .is_some_and(|entry| entry.kind == EntryKind::Error)
    );
}

#[test]
fn insufficient_credit_failure_has_a_specific_top_up_message() {
    let mut state = TuiState::with_options(
        PathBuf::from("/tmp/project"),
        "model".into(),
        PermissionProfile::Confirm,
        TuiOptions {
            appearance: Appearance::Dark,
            color_mode: ColorMode::TrueColor,
            ascii: false,
            animation: false,
        },
    );
    let turn = TurnId::new();
    state.apply(&AppEvent::PromptAccepted {
        attachments: Vec::new(),
        turn_id: turn.clone(),
        text: "inspect the repository".into(),
    });
    state.apply(&AppEvent::TurnStarted {
        turn_id: turn.clone(),
    });
    let provider_detail = "provider error: InvalidRequest HTTP 402 Payment Required: \
        {\"error\":{\"message\":\"out of credit – top up your Axiom balance\",\
        \"type\":\"insufficient_credit\"}}";
    state.apply(&AppEvent::ErrorRaised {
        turn_id: Some(turn.clone()),
        message: provider_detail.into(),
    });

    let folded = render_symbols(100, 30, &state);
    assert!(folded.contains("Out of credits"));
    assert!(folded.contains("Use /topup"));
    assert!(!folded.contains("insufficient_credit"));
    let task = state.entries[state.task_index(&turn).expect("task")]
        .task
        .as_ref()
        .expect("structured task");
    assert!(
        task.error_detail
            .as_deref()
            .is_some_and(|detail| detail.contains("insufficient_credit"))
    );

    let unrelated = "provider error: HTTP 402 Payment Required: card authorization failed";
    assert_eq!(user_facing_provider_error(unrelated), unrelated);
}

#[test]
fn attestation_failure_is_actionable_and_keeps_raw_detail_folded() {
    let mut state = TuiState::new(
        PathBuf::from("/tmp/project"),
        "model".into(),
        PermissionProfile::Confirm,
    );
    let turn = TurnId::new();
    state.apply(&AppEvent::PromptAccepted {
        attachments: Vec::new(),
        turn_id: turn.clone(),
        text: "test secure inference".into(),
    });
    state.apply(&AppEvent::TurnStarted {
        turn_id: turn.clone(),
    });
    let raw = "provider error: AttestationRejected: Intel TDX status is rejected by policy";
    state.apply(&AppEvent::ErrorRaised {
        turn_id: Some(turn.clone()),
        message: raw.into(),
    });

    let rendered = render_symbols(100, 30, &state);
    assert!(rendered.contains("Secure connection could not be verified"));
    assert!(rendered.contains("Your request was not sent"));
    assert!(!rendered.contains("Intel TDX"));
    assert_eq!(state.status, "Attestation failed");
    let task = state.entries[state.task_index(&turn).expect("task")]
        .task
        .as_ref()
        .expect("task view");
    assert_eq!(task.error_detail.as_deref(), Some(raw));
}

#[test]
fn ascii_mode_and_every_semantic_category_render_without_controls() {
    let mut state = TuiState::with_options(
        PathBuf::from("/tmp/🌸/project"),
        "model".into(),
        PermissionProfile::Confirm,
        TuiOptions {
            appearance: Appearance::Dark,
            color_mode: ColorMode::Ansi16,
            ascii: true,
            animation: false,
        },
    );
    let turn = TurnId::new();
    for event in [
        AppEvent::SessionCreated {
            cwd: PathBuf::from("/tmp/project"),
            origin: Origin::Test,
            profile: PermissionProfile::Confirm,
        },
        AppEvent::SessionResumed {
            cwd: PathBuf::from("/tmp/project"),
            origin: Origin::Test,
            profile: PermissionProfile::Confirm,
        },
        AppEvent::PromptAccepted {
            attachments: Vec::new(),
            turn_id: turn.clone(),
            text: "hello".into(),
        },
        AppEvent::TurnStarted {
            turn_id: turn.clone(),
        },
        AppEvent::TextDelta {
            turn_id: turn.clone(),
            text: "answer".into(),
        },
        AppEvent::ReasoningDelta {
            turn_id: turn.clone(),
            text: "reason".into(),
        },
        AppEvent::ProgressUpdated {
            message: "progress".into(),
            completed: Some(1),
            total: Some(2),
        },
        AppEvent::TaskListUpdated {
            items: vec![crate::app::TaskItem {
                id: "inspect".into(),
                title: "Inspect blossom files".into(),
                status: crate::app::TaskStatus::InProgress,
            }],
        },
        AppEvent::ToolProposed {
            turn_id: turn.clone(),
            call_id: "c1".into(),
            name: "read_file".into(),
            arguments: serde_json::json!({}),
        },
        AppEvent::ToolStarted {
            call_id: "c1".into(),
            name: "read_file".into(),
        },
        AppEvent::ToolOutput {
            call_id: "c1".into(),
            content: "unsafe\u{1b}[31m".into(),
            truncated: false,
        },
        AppEvent::PermissionRequired {
            request_id: "permission".into(),
            explanation: "write src/main.rs".into(),
            effect: serde_json::json!({"kind":"file_write","path":"src/main.rs"}),
            choices: vec!["allow_once".into(), "deny".into()],
        },
        AppEvent::PermissionResolved {
            request_id: "permission".into(),
            allowed: false,
            choice: Some("deny".into()),
        },
        AppEvent::ToolCompleted {
            call_id: "c1".into(),
            success: true,
        },
        AppEvent::WorkspaceChanged {
            paths: vec![PathBuf::from("src/main.rs")],
        },
        AppEvent::DiffAvailable {
            call_id: "c2".into(),
            diff: "-old\n+new".into(),
            truncated: false,
            files: Vec::new(),
        },
        AppEvent::QuestionAsked {
            question_id: "q".into(),
            prompt: "Choose".into(),
            options: vec!["one".into(), "two".into()],
        },
        AppEvent::QuestionAnswered {
            question_id: "q".into(),
            answers: vec!["one".into()],
        },
        AppEvent::PlanProposed {
            plan_id: "p".into(),
            revision: 1,
            markdown: "1. inspect".into(),
        },
        AppEvent::PlanReviewed {
            plan_id: "p".into(),
            revision: 1,
            decision: "approved".into(),
        },
        AppEvent::BackgroundTaskChanged {
            task_id: "b".into(),
            state: "running".into(),
        },
        AppEvent::UsageUpdated {
            input_tokens: 10,
            output_tokens: 2,
        },
        AppEvent::PermissionProfileChanged {
            profile: PermissionProfile::Confirm,
        },
        AppEvent::ModelChanged {
            model: "next-model".into(),
        },
        AppEvent::ThinkingLevelChanged {
            level: ThinkingLevel::Low,
        },
        AppEvent::ContextCompacted {
            summary: "successor".into(),
            messages_before: 2,
        },
        AppEvent::ProviderStatusChanged {
            connected: true,
            detail: "connected".into(),
        },
        AppEvent::SecurityStatusChanged {
            state: crate::app::SecurityStatus::UnattestedDevelopment,
        },
        AppEvent::WarningRaised {
            message: "careful".into(),
        },
        AppEvent::TurnCompleted {
            turn_id: turn.clone(),
        },
        AppEvent::TurnCancelled {
            turn_id: turn.clone(),
        },
        AppEvent::ErrorRaised {
            turn_id: Some(turn),
            message: "failed".into(),
        },
        AppEvent::SessionClosed,
    ] {
        state.apply(&event);
    }
    let output = render_symbols(100, 30, &state);
    assert!(output.contains("Axiom"));
    assert!(state.entries.iter().any(|entry| {
        entry
            .task
            .as_ref()
            .is_some_and(|task| task.detail_text().contains("diff:"))
    }));
    assert!(output.contains("QUESTION"));
    assert!(!output.contains('\u{1b}'));
    assert!(!output.contains('✿'));
}

#[test]
fn search_selection_folding_and_viewer_are_operational() {
    let mut state = TuiState::new(
        PathBuf::from("/tmp/project"),
        "model".into(),
        PermissionProfile::Confirm,
    );
    state.push_entry(EntryKind::Tool, "first output".into(), false);
    state.push_entry(EntryKind::Tool, "needle output".into(), false);
    state.search_query = "needle".into();
    state.refresh_search();
    state.select_search_match();
    assert_eq!(state.selected, Some(1));
    state.toggle_selected();
    assert!(state.entries[1].collapsed);
    state.overlay = state.selected.map(Overlay::Entry);
    state.overlay_scroll = 0;
    assert!(render_symbols(80, 24, &state).contains("needle output"));
}

#[test]
fn streamed_content_does_not_steal_manual_scroll_or_selection() {
    let mut state = TuiState::new(
        PathBuf::from("/tmp/project"),
        "model".into(),
        PermissionProfile::Confirm,
    );
    state.follow_output = false;
    state.scroll = 4;
    state.selected = Some(0);
    state.apply(&AppEvent::TextDelta {
        turn_id: TurnId::new(),
        text: "stream".into(),
    });
    assert_eq!(state.scroll, 4);
    assert_eq!(state.selected, Some(0));
}

#[test]
fn first_scroll_step_leaves_follow_mode_at_the_real_bottom_offset() {
    let mut state = TuiState::new(
        PathBuf::from("/tmp/project"),
        "model".into(),
        PermissionProfile::Confirm,
    );
    for index in 0..40 {
        state.push_entry(
            EntryKind::Assistant,
            format!("scroll fixture line {index}"),
            false,
        );
    }
    let area = Rect::new(0, 0, 100, 30);
    let bottom = transcript_max_scroll(&state, area);
    assert!(bottom > 3);
    assert!(state.follow_output);
    assert_eq!(state.scroll, 0);

    scroll_transcript_up(&mut state, area, 3);
    assert!(!state.follow_output);
    assert_eq!(state.scroll, bottom - 3);

    scroll_transcript_down(&mut state, area, 3);
    assert!(state.follow_output);
    assert_eq!(state.scroll, 0);
}

#[test]
fn task_events_update_one_live_card_and_keep_raw_details_folded() {
    let mut state = TuiState::with_options(
        PathBuf::from("/tmp/project"),
        "model".into(),
        PermissionProfile::Confirm,
        TuiOptions {
            appearance: Appearance::Dark,
            color_mode: ColorMode::TrueColor,
            ascii: false,
            animation: false,
        },
    );
    let turn = TurnId::new();
    for event in [
        AppEvent::PromptAccepted {
            attachments: Vec::new(),
            turn_id: turn.clone(),
            text: "inspect the repository".into(),
        },
        AppEvent::TurnStarted {
            turn_id: turn.clone(),
        },
        AppEvent::TextDelta {
            turn_id: turn.clone(),
            text: "I’ll inspect the README first.".into(),
        },
        AppEvent::ReasoningDelta {
            turn_id: turn.clone(),
            text: "private scratch reasoning".into(),
        },
        AppEvent::ToolProposed {
            turn_id: turn.clone(),
            call_id: "read-1".into(),
            name: "read_file".into(),
            arguments: serde_json::json!({"path": "README.md"}),
        },
        AppEvent::ToolStarted {
            call_id: "read-1".into(),
            name: "read_file".into(),
        },
        AppEvent::ToolOutput {
            call_id: "read-1".into(),
            content: "raw README contents".into(),
            truncated: false,
        },
        AppEvent::DiffAvailable {
            call_id: "read-1".into(),
            diff: "-old\n+new".into(),
            truncated: false,
            files: Vec::new(),
        },
        AppEvent::ToolCompleted {
            call_id: "read-1".into(),
            success: true,
        },
        AppEvent::TextDelta {
            turn_id: turn.clone(),
            text: "This repository is AxiomCLI.".into(),
        },
    ] {
        state.apply(&event);
    }

    let task_entries = state
        .entries
        .iter()
        .filter(|entry| entry.kind == EntryKind::Task)
        .collect::<Vec<_>>();
    assert_eq!(task_entries.len(), 1);
    let task = task_entries[0].task.as_ref().expect("structured task");
    assert_eq!(task.tools.len(), 1);
    assert_eq!(task.tools[0].output, "raw README contents");
    assert_eq!(task.tools[0].diff, "-old\n+new");
    assert_eq!(task.timeline.len(), 3);

    let folded = render_symbols(100, 30, &state);
    assert!(folded.contains("Read `README.md`"));
    assert!(!folded.contains("private scratch reasoning"));
    assert!(!folded.contains("raw README contents"));
    let lines = folded.lines().collect::<Vec<_>>();
    let before = lines
        .iter()
        .position(|line| line.contains("I’ll inspect the README first."))
        .expect("response before tool");
    let tool = lines
        .iter()
        .position(|line| line.contains("Read `README.md`"))
        .expect("tool timeline row");
    let after = lines
        .iter()
        .position(|line| line.contains("This repository is AxiomCLI."))
        .expect("response after tool");
    assert!(before + 1 < tool, "tool should have spacing before it");
    assert!(tool + 1 < after, "tool should have spacing after it");

    state.selected = state.task_index(&turn);
    state.toggle_selected();
    let expanded = render_symbols(100, 40, &state);
    assert!(expanded.contains("private scratch reasoning"));
    assert!(expanded.contains("raw README contents"));
}

#[test]
fn tool_feedback_shows_live_commands_queries_and_workspace_relative_paths() {
    let cwd = std::env::current_dir().expect("absolute workspace");
    let mut state = TuiState::new(cwd.clone(), "model".into(), PermissionProfile::Confirm);
    let turn = TurnId::new();
    state.apply(&AppEvent::PromptAccepted {
        attachments: Vec::new(),
        turn_id: turn.clone(),
        text: "inspect and test".into(),
    });
    state.apply(&AppEvent::TurnStarted {
        turn_id: turn.clone(),
    });

    state.apply(&AppEvent::ToolProposed {
        turn_id: turn.clone(),
        call_id: "command".into(),
        name: "run_command".into(),
        arguments: serde_json::json!({
            "program": "cargo",
            "args": ["test", "-p", "axiomcli"]
        }),
    });
    assert_eq!(state.status, "Running a command: `cargo test -p axiomcli`");
    assert!(
        render_symbols(120, 34, &state).contains("Running a command: `cargo test -p axiomcli`")
    );
    state.apply(&AppEvent::ToolCompleted {
        call_id: "command".into(),
        success: true,
    });
    assert_eq!(state.status, "Ran a command: `cargo test -p axiomcli`");

    for (call_id, name, arguments, expected) in [
        (
            "read",
            "read_file",
            serde_json::json!({"path":cwd.join("src/lib.rs")}),
            "Read `src/lib.rs`",
        ),
        (
            "web",
            "web_search",
            serde_json::json!({"query":"Axiom TEE documentation"}),
            "Searched the web for `Axiom TEE documentation`",
        ),
        (
            "git",
            "run_command",
            serde_json::json!({"program":"git", "args":["diff", "--stat"]}),
            "Ran Git command: `git diff --stat`",
        ),
    ] {
        state.apply(&AppEvent::ToolProposed {
            turn_id: turn.clone(),
            call_id: call_id.into(),
            name: name.into(),
            arguments,
        });
        state.apply(&AppEvent::ToolCompleted {
            call_id: call_id.into(),
            success: true,
        });
        assert_eq!(state.status, expected);
    }

    let rendered = render_symbols(120, 42, &state);
    for expected in [
        "Ran a command: `cargo test -p axiomcli`",
        "Read `src/lib.rs`",
        "Searched the web for `Axiom TEE documentation`",
        "Ran Git command: `git diff --stat`",
    ] {
        assert!(
            rendered.contains(expected),
            "missing tool feedback: {expected}"
        );
    }
}

#[test]
fn activity_pulses_while_waiting_and_reduced_motion_stay_static() {
    let mut state = TuiState::with_options(
        PathBuf::from("/tmp/project"),
        "model".into(),
        PermissionProfile::Confirm,
        TuiOptions {
            appearance: Appearance::Dark,
            color_mode: ColorMode::TrueColor,
            ascii: false,
            animation: true,
        },
    );
    let frames = (0..12)
        .map(|tick| {
            state.animation_frame = tick;
            state.activity_glyph(false)
        })
        .collect::<Vec<_>>();
    assert_eq!(
        frames,
        vec!["·", "·", "•", "●", "●", "•", "·", "·", "·", "·", "•", "●"]
    );
    for tick in 0..12 {
        state.animation_frame = tick;
        assert_eq!(state.activity_glyph(true), "○");
    }
    state.options.animation = false;
    assert_eq!(state.activity_glyph(false), "●");
}

#[test]
fn assistant_markdown_is_rendered_but_user_text_stays_literal() {
    let mut state = TuiState::with_options(
        PathBuf::from("/tmp/project"),
        "model".into(),
        PermissionProfile::Confirm,
        TuiOptions {
            appearance: Appearance::Dark,
            color_mode: ColorMode::TrueColor,
            ascii: false,
            animation: false,
        },
    );
    state.push_entry(
        EntryKind::User,
        "please keep **these markers**".into(),
        false,
    );
    state.push_entry(
        EntryKind::Assistant,
        "## Result\n\nThe **markers disappear**, while `code` is highlighted.".into(),
        false,
    );
    let lines = state.transcript_lines(72);
    let plain = lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(plain.contains("please keep **these markers**"));
    assert!(plain.contains("The markers disappear, while code is highlighted."));
    assert!(!plain.contains("## Result"));
    let code_span = lines
        .iter()
        .flat_map(|line| &line.spans)
        .find(|span| span.content == "code")
        .expect("inline code span");
    assert_eq!(
        code_span.style.bg,
        Some(Theme::for_mode(ColorMode::TrueColor).surface)
    );
}

#[test]
fn typical_operational_document_snapshot() {
    let mut state = TuiState::with_options(
        PathBuf::from("/work/axiomcli"),
        "glm-test".into(),
        PermissionProfile::Confirm,
        TuiOptions {
            appearance: Appearance::Dark,
            color_mode: ColorMode::TrueColor,
            ascii: false,
            animation: false,
        },
    );
    let turn = "00000000-0000-0000-0000-000000000001"
        .parse::<TurnId>()
        .expect("turn");
    state.apply(&AppEvent::PromptAccepted {
        attachments: Vec::new(),
        turn_id: turn.clone(),
        text: "Inspect the repository".into(),
    });
    state.apply(&AppEvent::TurnStarted {
        turn_id: turn.clone(),
    });
    state.apply(&AppEvent::ProviderStatusChanged {
        connected: true,
        detail: "connected".into(),
    });
    state.apply(&AppEvent::TextDelta {
        turn_id: turn.clone(),
        text: "I’ll inspect the relevant files.".into(),
    });
    state.apply(&AppEvent::UsageUpdated {
        input_tokens: 40,
        output_tokens: 7,
    });
    state.apply(&AppEvent::ToolProposed {
        turn_id: turn.clone(),
        call_id: "tool-1".into(),
        name: "read_file".into(),
        arguments: serde_json::json!({"path": "src"}),
    });
    state.apply(&AppEvent::ToolStarted {
        call_id: "tool-1".into(),
        name: "read_file".into(),
    });
    state.apply(&AppEvent::ToolOutput {
        call_id: "tool-1".into(),
        content: "src/main.rs\nsrc/lib.rs".into(),
        truncated: false,
    });
    state.apply(&AppEvent::ToolCompleted {
        call_id: "tool-1".into(),
        success: true,
    });
    state.apply(&AppEvent::ProviderStatusChanged {
        connected: true,
        detail: "connected".into(),
    });
    state.apply(&AppEvent::TextDelta {
        turn_id: turn.clone(),
        text: "The repository contains a Rust coding agent.".into(),
    });
    state.apply(&AppEvent::UsageUpdated {
        input_tokens: 60,
        output_tokens: 10,
    });
    state.apply(&AppEvent::TurnCompleted { turn_id: turn });
    insta::assert_snapshot!(
        "tui_typical_operational_document",
        render_symbols(80, 20, &state)
    );
}

/// Dumps rendered frames (symbols plus resolved colours) to
/// `target/tui-preview/*.json` so the shell can be reviewed as an image
/// rather than as a plain-text buffer. Ignored by default:
/// `cargo test -p axiomcli --lib tui_preview_frames -- --ignored`.
#[test]
#[ignore = "writes preview frames for visual review rather than asserting"]
fn tui_preview_frames() {
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tui-preview");
    std::fs::create_dir_all(&directory).expect("preview directory");
    for (name, width, height, state) in preview_scenarios() {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal.draw(|frame| render(frame, &state)).expect("draw");
        let buffer = terminal.backend().buffer();
        let rows = (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| {
                        let cell = &buffer[(x, y)];
                        serde_json::json!({
                            "s": cell.symbol(),
                            "f": preview_rgb(cell.fg),
                            "b": preview_rgb(cell.bg),
                            "d": cell.modifier.contains(Modifier::BOLD),
                            "i": cell.modifier.contains(Modifier::ITALIC),
                            "u": cell.modifier.contains(Modifier::UNDERLINED),
                            "x": cell.modifier.contains(Modifier::CROSSED_OUT),
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        let frame = serde_json::json!({"width": width, "height": height, "rows": rows});
        std::fs::write(
            directory.join(format!("{name}.json")),
            serde_json::to_string(&frame).expect("frame json"),
        )
        .expect("write frame");
    }
}

fn preview_rgb(color: Color) -> Option<[u8; 3]> {
    match color {
        Color::Rgb(red, green, blue) => Some([red, green, blue]),
        _ => None,
    }
}

fn preview_state() -> TuiState {
    TuiState::with_options(
        PathBuf::from("/work/axiomai"),
        "glm-5-2".into(),
        PermissionProfile::Confirm,
        TuiOptions {
            appearance: Appearance::Dark,
            color_mode: ColorMode::TrueColor,
            ascii: false,
            animation: false,
        },
    )
}

fn activity_preview(frame: u64) -> TuiState {
    let mut state = preview_state();
    state.options.animation = true;
    state.animation_frame = frame;
    let turn = TurnId::new();
    for event in [
        AppEvent::PromptAccepted {
            attachments: Vec::new(),
            turn_id: turn.clone(),
            text: "inspect the renderer".into(),
        },
        AppEvent::TurnStarted { turn_id: turn },
        AppEvent::ProgressUpdated {
            message: "Inspecting the renderer".into(),
            completed: None,
            total: None,
        },
    ] {
        state.apply(&event);
    }
    state
}

fn preview_scenarios() -> Vec<(&'static str, u16, u16, TuiState)> {
    let mut startup = preview_state();
    startup.splash_message = "A quiet place to think.";
    let mut startup_compact = preview_state();
    startup_compact.splash_message = "A quiet place to think.";
    let mut conversation = preview_state();
    let conversation_turn = TurnId::new();
    for event in [
        AppEvent::PromptAccepted {attachments: Vec::new(),
            turn_id: conversation_turn.clone(),
            text: "hello axiom whats up".into(),
        },
        AppEvent::TurnStarted {
            turn_id: conversation_turn.clone(),
        },
        AppEvent::ReasoningDelta {
            turn_id: conversation_turn.clone(),
            text: "The user is greeting me casually; answer warmly and concisely.".into(),
        },
        AppEvent::TextDelta {
            turn_id: conversation_turn.clone(),
            text: "Hey! 👋 I’m here in your `axiomai` workspace and ready to work. What do you want to build?".into(),
        },
        AppEvent::TurnCompleted {
            turn_id: conversation_turn,
        },
    ] {
        conversation.apply(&event);
    }

    let mut working = preview_state();
    working.options.animation = true;
    working.animation_frame = 7;
    let working_turn = TurnId::new();
    for event in [
        AppEvent::PromptAccepted {
            attachments: Vec::new(),
            turn_id: working_turn.clone(),
            text: "run the release verification".into(),
        },
        AppEvent::TurnStarted {
            turn_id: working_turn.clone(),
        },
        AppEvent::TaskListUpdated {
            items: vec![
                crate::app::TaskItem {
                    id: "signatures".into(),
                    title: "Verify signatures".into(),
                    status: crate::app::TaskStatus::Completed,
                },
                crate::app::TaskItem {
                    id: "manifests".into(),
                    title: "Check release manifests".into(),
                    status: crate::app::TaskStatus::Completed,
                },
                crate::app::TaskItem {
                    id: "package".into(),
                    title: "Package the tarball".into(),
                    status: crate::app::TaskStatus::InProgress,
                },
            ],
        },
        AppEvent::ToolProposed {
            turn_id: working_turn.clone(),
            call_id: "read-release".into(),
            name: "read_file".into(),
            arguments: serde_json::json!({"path": "RELEASE.md"}),
        },
        AppEvent::ToolStarted {
            call_id: "read-release".into(),
            name: "read_file".into(),
        },
        AppEvent::ToolOutput {
            call_id: "read-release".into(),
            content: "signatures: valid\nmanifests: valid".into(),
            truncated: false,
        },
        AppEvent::ToolCompleted {
            call_id: "read-release".into(),
            success: true,
        },
        AppEvent::ToolProposed {
            turn_id: working_turn.clone(),
            call_id: "package".into(),
            name: "shell".into(),
            arguments: serde_json::json!({"command": "scripts/package-release"}),
        },
        AppEvent::ToolStarted {
            call_id: "package".into(),
            name: "shell".into(),
        },
        AppEvent::ProgressUpdated {
            message: "Packaging the release tarball".into(),
            completed: None,
            total: None,
        },
    ] {
        working.apply(&event);
    }

    let mut waiting = preview_state();
    waiting.options.animation = true;
    waiting.animation_frame = 9;
    let waiting_turn = TurnId::new();
    for event in [
        AppEvent::PromptAccepted {
            attachments: Vec::new(),
            turn_id: waiting_turn.clone(),
            text: "update the release manifest".into(),
        },
        AppEvent::TurnStarted {
            turn_id: waiting_turn.clone(),
        },
        AppEvent::ToolProposed {
            turn_id: waiting_turn,
            call_id: "write-manifest".into(),
            name: "apply_patch".into(),
            arguments: serde_json::json!({"path": "RELEASE.md"}),
        },
        AppEvent::PermissionRequired {
            request_id: "permission-1".into(),
            explanation: "Edit RELEASE.md".into(),
            effect: serde_json::json!({"kind":"file_write","path":"RELEASE.md"}),
            choices: vec!["allow_once".into(), "deny".into()],
        },
    ] {
        waiting.apply(&event);
    }
    waiting.interaction = Some(InteractionView::Approval(ApprovalRequest {
        request_id: "permission-1".into(),
        explanation: "Workspace policy requires confirmation before editing RELEASE.md".into(),
        effect: crate::policy::Effect::FileWrite {
            path: PathBuf::from("/work/axiomai/RELEASE.md"),
        },
        allow_session_grants: true,
        suggested_prefix_scope: Some("FileWrite within /work/axiomai".into()),
    }));

    let mut question = preview_state();
    let question_turn = TurnId::new();
    question.apply(&AppEvent::PromptAccepted {
        attachments: Vec::new(),
        turn_id: question_turn.clone(),
        text: "prepare the release notes".into(),
    });
    question.apply(&AppEvent::TurnStarted {
        turn_id: question_turn,
    });
    let question_request = QuestionRequest {
        request_id: "release-format".into(),
        questions: vec![crate::app::QuestionSpec {
            id: "audience".into(),
            prompt: "Who are these release notes for?".into(),
            options: vec!["Developers".into(), "End users".into(), "Both".into()],
            multiple: false,
            required: true,
        }],
    };
    question.apply(&AppEvent::QuestionsAsked {
        request: question_request.clone(),
    });
    question.interaction = Some(InteractionView::Questions {
        request: question_request,
        index: 0,
    });
    question.answer_prompt = Some("Question 1/1: Who are these release notes for?".into());
    question.focus = Focus::Composer;

    let mut details = preview_state();
    let details_turn = TurnId::new();
    for event in [
        AppEvent::PromptAccepted {attachments: Vec::new(),
            turn_id: details_turn.clone(),
            text: "fix the renderer regression".into(),
        },
        AppEvent::TurnStarted {
            turn_id: details_turn.clone(),
        },
        AppEvent::ReasoningDelta {
            turn_id: details_turn.clone(),
            text: "The regression begins in the task reducer. I should inspect the call identity path before editing.".into(),
        },
        AppEvent::ToolProposed {
            turn_id: details_turn.clone(),
            call_id: "search-reducer".into(),
            name: "search_files".into(),
            arguments: serde_json::json!({"query": "call_id", "path": "apps/axiomcli/src/tui"}),
        },
        AppEvent::ToolStarted {
            call_id: "search-reducer".into(),
            name: "search_files".into(),
        },
        AppEvent::ToolOutput {
            call_id: "search-reducer".into(),
            content: "apps/axiomcli/src/tui.rs:511\napps/axiomcli/src/tui/task.rs:142".into(),
            truncated: false,
        },
        AppEvent::ToolCompleted {
            call_id: "search-reducer".into(),
            success: true,
        },
        AppEvent::ToolProposed {
            turn_id: details_turn.clone(),
            call_id: "patch-reducer".into(),
            name: "apply_patch".into(),
            arguments: serde_json::json!({"path": "apps/axiomcli/src/tui.rs"}),
        },
        AppEvent::ToolStarted {
            call_id: "patch-reducer".into(),
            name: "apply_patch".into(),
        },
        AppEvent::DiffAvailable {
            call_id: "patch-reducer".into(),
            diff: "- push_entry(tool_output)\n+ update_tool(call_id)".into(),
            truncated: false,
            files: Vec::new(),
        },
        AppEvent::ToolCompleted {
            call_id: "patch-reducer".into(),
            success: true,
        },
        AppEvent::TextDelta {
            turn_id: details_turn.clone(),
            text: "The reducer now updates each tool call in place.".into(),
        },
        AppEvent::TurnCompleted {
            turn_id: details_turn.clone(),
        },
    ] {
        details.apply(&event);
    }
    details.focus = Focus::Transcript;
    details.selected = details.task_index(&details_turn);
    details.toggle_selected();

    let mut browsing = preview_state();
    browsing.focus = Focus::Transcript;
    browsing.push_entry(
        EntryKind::User,
        "where is the needle handled?".into(),
        false,
    );
    browsing.push_entry(
        EntryKind::Tool,
        "src/policy.rs:120\nsrc/policy.rs:214\nsrc/session.rs:88".into(),
        true,
    );
    browsing.push_entry(
        EntryKind::Error,
        "provider disconnected; retry is safe".into(),
        false,
    );
    browsing.search_query = "needle".into();
    browsing.refresh_search();
    browsing.selected = Some(1);
    browsing.follow_output = false;

    let mut out_of_credits = preview_state();
    let credits_turn = TurnId::new();
    for event in [
        AppEvent::PromptAccepted {
            attachments: Vec::new(),
            turn_id: credits_turn.clone(),
            text: "what is this repository about?".into(),
        },
        AppEvent::TurnStarted {
            turn_id: credits_turn.clone(),
        },
        AppEvent::ErrorRaised {
            turn_id: Some(credits_turn),
            message: "provider error: InvalidRequest HTTP 402 Payment Required: \
                {\"error\":{\"message\":\"out of credit – top up your Axiom balance\",\
                \"type\":\"insufficient_credit\"}}"
                .into(),
        },
    ] {
        out_of_credits.apply(&event);
    }

    let mut overlay = preview_state();
    overlay.push_entry(EntryKind::User, "help".into(), false);
    overlay.overlay = Some(Overlay::Help);

    let mut slash_commands = preview_state();
    slash_commands.input = "/".into();
    slash_commands.status = "Choose a local command".into();

    let mut slash_options = preview_state();
    slash_options.input = "/thinking h".into();
    slash_options.status = "Tab completes valid options".into();

    let mut model_picker = preview_state();
    model_picker.model_candidates = vec![
        "claude-sonnet-4-5".into(),
        "glm-5-2".into(),
        "gpt-5.2-codex".into(),
        "grok-code-fast-1".into(),
        "qwen3-coder".into(),
    ];
    model_picker.model_catalog_loaded = true;
    model_picker.open_model_picker();
    model_picker.push_model_picker_search('g');

    let mut resume_picker = preview_state();
    resume_picker.open_resume_picker(vec![
        SessionSummary {
            id: "d6bfde31-2742-45df-8744-72c08b2ce82c".into(),
            title: Some("Build transcript resume picker".into()),
            cwd: PathBuf::from("/work/axiomai"),
            origin: "tui".into(),
            profile: "full_access".into(),
            archived: false,
            updated_at: "2026-08-22T16:42:00Z".into(),
        },
        SessionSummary {
            id: "0b8b86dd-f972-42cf-a175-9b4bf73b0fa6".into(),
            title: Some("Repair Markdown rendering".into()),
            cwd: PathBuf::from("/work/axiomai"),
            origin: "tui".into(),
            profile: "confirm".into(),
            archived: false,
            updated_at: "2026-08-22T14:10:00Z".into(),
        },
        SessionSummary {
            id: "3aef7c2a-98df-4438-a18a-e158187f6f7a".into(),
            title: None,
            cwd: PathBuf::from("/work/axiomai"),
            origin: "tui".into(),
            profile: "confirm".into(),
            archived: false,
            updated_at: "2026-08-21T09:06:00Z".into(),
        },
    ]);

    let mut security = preview_state();
    security.security = "SECURE";
    security.security_evidence = Some(security_evidence());
    security.model = "deepseek-v4-flash".into();
    security.open_security();

    let mut security_workload = preview_state();
    security_workload.security = "SECURE";
    let mut provider_report = security_evidence();
    provider_report.provider_id = "future-gateway".into();
    provider_report.attestation_protocol = "gateway-evidence-v1".into();
    provider_report.e2ee_protocol = "gateway-e2ee-v1".into();
    provider_report.workload_manifest = None;
    provider_report.provider_claims = vec![
        EvidenceClaim {
            name: "gateway_source_repository".into(),
            value: "https://github.com/Dstack-TEE/private-ai-gateway".into(),
        },
        EvidenceClaim {
            name: "gateway_source_commit_full".into(),
            value: "abcdef0123456789abcdef0123456789abcdef0123".into(),
        },
        EvidenceClaim {
            name: "preverified_worker_sessions".into(),
            value: "2".into(),
        },
    ];
    security_workload
        .model
        .clone_from(&provider_report.model_id);
    security_workload.security_evidence = Some(provider_report);
    security_workload.overlay = Some(Overlay::Security(SecurityView::Workload));

    let mut security_expired = preview_state();
    security_expired.security = "SECURE";
    let mut expired = security_evidence();
    expired.verified_at_unix_seconds = 100;
    expired.hard_expires_at_unix_seconds = Some(200);
    security_expired.model.clone_from(&expired.model_id);
    security_expired.security_evidence = Some(expired);
    security_expired.open_security();

    let mut usage = preview_state();
    usage.context_usage = Some(axiom_acp_extension::ContextUsage {
        input_tokens: 4000,
        output_tokens: 1000,
        model_id: usage.model.clone(),
        reported_at: "2026-09-21T12:00:00Z".into(),
        context_window_tokens: Some(10000),
        auto_compact_threshold_tokens: Some(8500),
    });
    usage.overlay = Some(Overlay::Usage);

    let narrow = preview_state();
    let minimum = preview_state();

    let mut markdown = preview_state();
    markdown.entries.clear();
    markdown.push_entry(
        EntryKind::Assistant,
        "# Renderer ready\n\nAxiom now renders **strong**, *emphasis*, ~~removed text~~, and \
         [`styled links`](https://example.com) without exposing Markdown punctuation.\n\n\
         - [x] Preserve the raw response\n\
         - [x] Wrap styled Unicode like blossoms 🌸 correctly\n\
         - [ ] Add optional Mermaid rendering later\n\n\
         > [!NOTE]\n\
         > Security and tool output remain literal.\n\n\
         | Surface | Behavior |\n\
         |:--|:--|\n\
         | Assistant | Markdown |\n\
         | Tools | Literal |\n\n\
         ```rust\n\
         fn blossom(private: bool) -> &'static str {\n\
             if private { \"encrypted 🌸\" } else { \"blocked\" }\n\
         }\n\
         ```"
        .into(),
        false,
    );

    let mut markdown_narrow = preview_state();
    markdown_narrow.entries.clear();
    markdown_narrow.push_entry(
        EntryKind::Assistant,
        "## Narrow terminal\n\nStyled prose wraps without losing **weight** or its \
         [destination](https://example.com/a/long/path).\n\n\
         | Name | State | Owner | Scope | Mode |\n\
         |---|---|---|---|---|\n\
         | renderer | ready | axiom | terminal | safe |\n\n\
         ```sh\naxiomcli tui --cwd /a/very/long/project/path/that/is/clipped\n```"
            .into(),
        false,
    );

    vec![
        ("startup", 100, 30, startup),
        ("startup-compact", 64, 30, startup_compact),
        ("conversation", 100, 30, conversation),
        ("working", 100, 30, working),
        ("waiting", 100, 30, waiting),
        ("question", 100, 30, question),
        ("details", 100, 42, details),
        ("activity-rest", 84, 20, activity_preview(0)),
        ("activity-rise", 84, 20, activity_preview(5)),
        ("activity-peak", 84, 20, activity_preview(7)),
        ("activity-fall", 84, 20, activity_preview(11)),
        ("browsing", 100, 30, browsing),
        ("out-of-credits", 100, 30, out_of_credits),
        ("overlay", 100, 30, overlay),
        ("slash-commands", 100, 30, slash_commands),
        ("slash-options", 100, 30, slash_options),
        ("model-picker", 100, 30, model_picker),
        ("resume-picker", 100, 30, resume_picker),
        ("security", 120, 42, security),
        ("security-workload", 120, 42, security_workload),
        ("security-expired", 120, 42, security_expired),
        ("usage", 100, 30, usage),
        ("narrow", 54, 16, narrow),
        ("minimum", 42, 10, minimum),
        ("markdown", 100, 42, markdown),
        ("markdown-narrow", 60, 30, markdown_narrow),
    ]
}

fn render_symbols(width: u16, height: u16, state: &TuiState) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal.draw(|frame| render(frame, state)).expect("draw");
    let buffer = terminal.backend().buffer();
    (0..height)
        .map(|y| {
            let line = (0..width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>();
            line.trim_end().to_owned()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn wrapping_measures_display_width_and_breaks_unbreakable_words() {
    assert_eq!(
        wrap_to_width("the quick brown fox", 9),
        vec!["the quick", "brown fox"]
    );
    // A wave emoji is two columns wide, so only three fit in eight columns.
    assert_eq!(wrap_to_width("👋👋👋👋", 8), vec!["👋👋👋👋"]);
    assert_eq!(wrap_to_width("👋👋👋👋", 7), vec!["👋👋👋", "👋"]);
    assert_eq!(wrap_to_width("abcdefgh", 3), vec!["abc", "def", "gh"]);
    assert_eq!(wrap_to_width("    indented", 20), vec!["    indented"]);
    assert_eq!(wrap_to_width("", 10), vec![""]);
}

#[test]
fn single_line_fields_keep_their_caret_visible_for_overlong_input() {
    assert_eq!(tail_to_width("abcdef", 10), "abcdef");
    assert_eq!(tail_to_width("abcdef", 3), "def");
    assert_eq!(tail_to_width("a👋bc", 3), "bc");
}

#[test]
fn composer_grows_to_eight_rows_then_scrolls_with_the_editing_cursor() {
    let mut state = TuiState::new(
        PathBuf::from("/tmp/project"),
        "model".into(),
        PermissionProfile::Confirm,
    );
    state.has_prompt = true;
    state.entries.clear();
    let area = Rect::new(0, 0, 100, 42);
    assert_eq!(main_rows(area, &state)[3].height, 5);

    state.input.set_text(
        (0..20)
            .map(|index| format!("draft line {index:02}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    let rows = main_rows(area, &state);
    assert_eq!(rows[3].height, COMPOSER_MAX_CONTENT_ROWS + 4);
    assert!(
        rows[1].height >= 3,
        "the transcript must retain usable space"
    );

    let bottom = render_symbols(area.width, area.height, &state);
    assert!(bottom.contains("draft line 19"));
    assert!(!bottom.contains("draft line 00"));

    state.input.move_to_document_start();
    let top = render_symbols(area.width, area.height, &state);
    assert!(top.contains("draft line 00"));
    assert!(!top.contains("draft line 19"));
}

#[test]
fn multiline_composer_keeps_minimum_terminal_layout_usable() {
    let mut state = TuiState::new(
        PathBuf::from("/tmp/project"),
        "model".into(),
        PermissionProfile::Confirm,
    );
    state.input.set_text("one\ntwo\nthree\nfour");
    let area = Rect::new(0, 0, MIN_WIDTH, MIN_HEIGHT);
    let rows = main_rows(area, &state);
    assert_eq!(rows[1].height, 3);
    assert_eq!(rows[3].height, 3);
    assert!(render_symbols(area.width, area.height, &state).contains("four"));
}

#[test]
fn command_errors_are_transient_status_instead_of_transcript_entries() {
    let mut state = preview_state();
    let entries_before = state.entries.len();
    show_slash_error(&mut state, "/permissions does not accept arguments");
    assert_eq!(state.entries.len(), entries_before);
    assert!(state.status.starts_with("Command error:"));
    assert_eq!(state.status_tone, StatusTone::Error);

    state.apply(&AppEvent::PermissionProfileChanged {
        profile: PermissionProfile::FullAccess,
    });
    assert_eq!(state.status, "Tools permission changed to Full Access");
}

#[test]
fn verifying_attestation_animates_the_header_chip() {
    let mut state = preview_state();
    state.options.animation = true;
    state.apply(&AppEvent::SecurityStatusChanged {
        state: crate::app::SecurityStatus::Verifying,
    });
    state.animation_frame = 0;
    let resting = render_symbols(100, 30, &state);
    state.animation_frame = 4;
    let pulsing = render_symbols(100, 30, &state);
    assert!(state.should_animate());
    assert!(resting.contains("· VERIFYING"));
    assert!(pulsing.contains("● VERIFYING"));
}

#[test]
fn auth_overlays_expose_only_native_browser_sign_in() {
    let mut state = preview_state();
    state.open_login(Some("Authentication is required.".into()));
    let menu = render_symbols(90, 28, &state);
    assert!(menu.contains("Continue in system browser"));
    assert!(!menu.contains("API key"));
    assert!(!menu.contains("Not now"));
    assert!(!menu.contains("Press K"));

    state.overlay = Some(Overlay::Auth(AuthOverlay::Account {
        status: "Signed in successfully.\n\nYour refresh token is in the system credential store."
            .into(),
    }));
    let account = render_symbols(90, 28, &state);
    let account_lines = account.lines().collect::<Vec<_>>();
    let signed_row = account_lines
        .iter()
        .position(|line| line.contains("Signed in successfully."))
        .expect("signed-in row");
    let storage_row = account_lines
        .iter()
        .position(|line| line.contains("Your refresh token is in the system credential store."))
        .expect("storage row");
    assert!(storage_row >= signed_row + 2);
    assert!(account.contains("Your refresh token is in the system credential store."));
    assert!(!account.contains("Key ID:"));
}

#[test]
fn unattested_status_always_fails_closed() {
    let mut state = preview_state();
    state.apply(&AppEvent::SecurityStatusChanged {
        state: crate::app::SecurityStatus::UnattestedDevelopment,
    });
    assert_eq!(state.security, "SECURITY FAILED");
}

#[test]
fn transcript_text_never_crosses_the_right_gutter() {
    for (width, height) in [(42, 10), (60, 14), (100, 30), (160, 50)] {
        let mut state = TuiState::with_options(
            PathBuf::from("/tmp/axiom-project"),
            "test-model".into(),
            PermissionProfile::Confirm,
            TuiOptions {
                appearance: Appearance::Dark,
                color_mode: ColorMode::TrueColor,
                ascii: false,
                animation: false,
            },
        );
        state.push_entry(
            EntryKind::Assistant,
            "Hey! 👋 Not much, just hanging out in your workspace ready to work, with a \
             deliberately-long-unbreakable-token-that-cannot-fit-on-any-single-line-at-all \
             to force a hard split."
                .into(),
            false,
        );
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal.draw(|frame| render(frame, &state)).expect("draw");
        let buffer = terminal.backend().buffer();
        // Rows 0..2 hold the header and its full-bleed divider; the rest of
        // the shell must stay inside the two-column gutter.
        for y in 2..height {
            for x in [0, 1, width - 2, width - 1] {
                let cell = &buffer[(x, y)];
                assert!(
                    cell.symbol().trim().is_empty(),
                    "ink at ({x},{y}) in a {width}x{height} frame: {:?}",
                    cell.symbol()
                );
            }
        }
    }
}

#[test]
fn path_truncation_keeps_both_ends() {
    assert_eq!(truncate_middle("abcdefghij", 7), "abc…hij");
}

#[tokio::test]
async fn dropped_tui_approval_surface_fails_closed() {
    let (tx, mut rx) = mpsc::channel(APP_EVENT_QUEUE_CAPACITY);
    let handler = TuiApproval { tx };
    let request = ApprovalRequest {
        request_id: "request".into(),
        explanation: "delete /workspace/file".into(),
        effect: crate::policy::Effect::FileDelete {
            path: PathBuf::from("/workspace/file"),
        },
        allow_session_grants: true,
        suggested_prefix_scope: Some("FileDelete within /workspace".into()),
    };
    let task =
        tokio::spawn(async move { handler.request(request, CancellationToken::new()).await });
    let UiMessage::Approval(_, response) = rx.recv().await.expect("approval message") else {
        panic!("unexpected UI message");
    };
    drop(response);
    assert_eq!(
        task.await.expect("join").expect("closed response"),
        ApprovalResponse::deny()
    );
}
