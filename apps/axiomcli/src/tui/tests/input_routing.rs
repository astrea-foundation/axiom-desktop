use super::super::input::{approval_choice, command_allowed_while_running, is_text_input};
use super::*;
use crate::slash::SlashCommand;
use std::collections::BTreeSet;

fn state() -> TuiState {
    TuiState::new(
        PathBuf::from("/tmp/project"),
        "model".into(),
        PermissionProfile::Confirm,
    )
}

#[test]
fn gift_code_paste_stays_in_the_masked_billing_input() {
    use super::super::billing::{BillingAction, BillingView};
    let mut state = state();
    state.input.insert_str("Keep this chat draft");
    state.overlay = Some(Overlay::Billing(Box::new(BillingView::new(true))));
    state.paste_text("AXG-PRIVATE-TEST-CODE");
    assert_eq!(state.input.as_str(), "Keep this chat draft");
    assert!(!render_symbols(100, 30, &state).contains("AXG-PRIVATE-TEST-CODE"));
    let Some(Overlay::Billing(view)) = &mut state.overlay else {
        panic!("gift entry must remain visible");
    };
    let BillingAction::Redeem(code) = view.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
    else {
        panic!("pasted code must be redeemable");
    };
    assert_eq!(code, "AXG-PRIVATE-TEST-CODE");
}

#[test]
fn text_input_accepts_shift_but_not_control_or_alt_shortcuts() {
    for modifiers in [KeyModifiers::NONE, KeyModifiers::SHIFT] {
        assert!(is_text_input(modifiers));
    }
    for modifiers in [
        KeyModifiers::CONTROL,
        KeyModifiers::ALT,
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    ] {
        assert!(!is_text_input(modifiers));
    }
}

#[test]
fn approval_requires_an_explicit_control_chord_and_respects_grant_scope() {
    use crate::policy::{ApprovalChoice, Effect};
    let mut request = ApprovalRequest {
        request_id: "approval".into(),
        explanation: "Read the test fixture".into(),
        effect: Effect::FileRead {
            path: PathBuf::from("/tmp/project/invoice.py"),
        },
        allow_session_grants: true,
        suggested_prefix_scope: Some("read workspace".into()),
    };
    for (character, choice) in [
        ('y', ApprovalChoice::AllowOnce),
        ('n', ApprovalChoice::Deny),
        ('g', ApprovalChoice::AllowExactSession),
        ('p', ApprovalChoice::AllowPrefixSession),
    ] {
        for code in [character, character.to_ascii_uppercase()] {
            for modifiers in [
                KeyModifiers::NONE,
                KeyModifiers::SHIFT,
                KeyModifiers::ALT,
                KeyModifiers::CONTROL | KeyModifiers::ALT,
            ] {
                assert_eq!(
                    approval_choice(KeyEvent::new(KeyCode::Char(code), modifiers), &request),
                    None
                );
            }
            assert_eq!(
                approval_choice(
                    KeyEvent::new(KeyCode::Char(code), KeyModifiers::CONTROL),
                    &request
                ),
                Some(choice)
            );
        }
    }
    assert_eq!(
        approval_choice(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &request),
        None
    );
    // In particular, the composer's Ctrl+E end-of-line shortcut cannot grant
    // permissions if an approval arrives just before that editing keystroke.
    assert_eq!(
        approval_choice(
            KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL),
            &request
        ),
        None
    );
    request.allow_session_grants = false;
    for code in ['g', 'p'] {
        assert_eq!(
            approval_choice(
                KeyEvent::new(KeyCode::Char(code), KeyModifiers::CONTROL),
                &request
            ),
            None
        );
    }
    request.allow_session_grants = true;
    request.suggested_prefix_scope = None;
    assert_eq!(
        approval_choice(
            KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL),
            &request
        ),
        None
    );

    let mut state = state();
    state.running = true;
    state.input = "keep draft".into();
    state.interaction = Some(InteractionView::Approval(request));
    state.paste_text("yNeP\n");
    assert_eq!(state.input, "keep draft");
    let rendered = render_symbols(120, 40, &state);
    assert!(rendered.contains("Ctrl+Y"));
    assert!(rendered.contains("Ctrl+N"));
}

#[test]
fn running_commands_allow_inspection_but_not_session_mutation() {
    for command in [
        SlashCommand::Help,
        SlashCommand::Usage,
        SlashCommand::Security,
        SlashCommand::Theme(Appearance::Light),
    ] {
        assert!(command_allowed_while_running(&command));
    }
    for command in [
        SlashCommand::Model,
        SlashCommand::Logout,
        SlashCommand::Login,
        SlashCommand::Account,
        SlashCommand::Resume,
        SlashCommand::Delete,
        SlashCommand::Refresh,
        SlashCommand::Permissions,
        SlashCommand::Thinking(ThinkingLevel::High),
        SlashCommand::Web(true),
        SlashCommand::Compact { focus: None },
    ] {
        assert!(!command_allowed_while_running(&command));
    }
    let mut state = state();
    state.running = true;
    state.input = "/usa".into();
    assert!(state.can_navigate_slash());
    assert!(state.should_accept_slash_selection());
    assert!(render_symbols(100, 30, &state).contains("/usage"));
    state.accept_slash_selection();
    assert_eq!(state.input, "/usage");
}

#[test]
fn paste_preserves_multiline_steering_and_disarms_exit() {
    let mut state = state();
    state.running = true;
    state.exit_armed = true;
    state.paste_text("aBcD\nFollowUp\x1b");
    assert_eq!(state.input, "aBcD\nFollowUp");
    assert!(!state.exit_armed);
}

#[test]
fn paste_filters_the_visible_picker_without_changing_the_draft() {
    let mut state = state();
    state.input = "keep this draft".into();
    let overlays = [
        Overlay::ModelPicker {
            query: String::new(),
            selected: 3,
        },
        Overlay::ResumePicker {
            query: String::new(),
            selected: 3,
            sessions: Vec::new(),
        },
        Overlay::DeletePicker {
            query: String::new(),
            selected: 3,
            sessions: Vec::new(),
            marked: BTreeSet::new(),
            confirming: false,
        },
    ];
    for overlay in overlays {
        state.overlay = Some(overlay);
        state.paste_text("Model\nName");
        match state.overlay.as_ref().unwrap() {
            Overlay::ModelPicker { query, selected }
            | Overlay::ResumePicker {
                query, selected, ..
            }
            | Overlay::DeletePicker {
                query, selected, ..
            } => {
                assert_eq!(query, "Model Name");
                assert_eq!(*selected, 0);
            }
            _ => panic!("unexpected picker"),
        }
        assert_eq!(state.input, "keep this draft");
    }
}

#[test]
fn paste_cannot_reach_a_hidden_composer_or_confirm_a_deletion() {
    let mut state = state();
    state.input = "draft".into();
    for overlay in [
        Overlay::Help,
        Overlay::Usage,
        Overlay::Security(SecurityView::Summary),
        Overlay::Permissions { selected: 0 },
        Overlay::Auth(AuthOverlay::Menu { message: None }),
        Overlay::DeletePicker {
            query: String::new(),
            selected: 0,
            sessions: Vec::new(),
            marked: BTreeSet::new(),
            confirming: true,
        },
    ] {
        state.overlay = Some(overlay.clone());
        state.paste_text("yes");
        assert_eq!(state.overlay, Some(overlay));
        assert_eq!(state.input, "draft");
    }
}

#[test]
fn paste_uses_search_question_and_filename_inputs() {
    let mut state = state();
    state.input = "draft".into();
    state.search_active = true;
    state.paste_text("Search\nTerm");
    assert_eq!(state.search_query, "Search Term");
    state.interaction = Some(InteractionView::Questions {
        request: QuestionRequest {
            request_id: "question".into(),
            questions: Vec::new(),
        },
        index: 0,
    });
    state.paste_text("Answer\nText");
    assert_eq!(state.answer_input, "Answer Text");
    state.interaction = None;
    state.overlay = Some(Overlay::SaveAttestation(SaveAttestationPicker {
        directory: state.cwd.clone(),
        filename: String::new(),
        selected: 0,
        focus: SavePickerFocus::Filename,
        error: None,
    }));
    state.paste_text("report\n.json");
    assert!(
        matches!(&state.overlay, Some(Overlay::SaveAttestation(picker)) if picker.filename == "report.json")
    );
    assert_eq!(state.input, "draft");
}

#[tokio::test]
async fn login_reconciles_provisional_model_before_saving_the_session() {
    use crate::agent::{EchoTurnRunner, TurnRunner};
    use std::sync::Arc;
    let directory = tempfile::tempdir().unwrap();
    let store = SessionStore::open(directory.path().join("state.sqlite3")).unwrap();
    store.set_last_used_model("medium-only").unwrap();
    store.set_last_used_thinking(ThinkingLevel::High).unwrap();
    let runtime = Runtime::new(32);
    let session = runtime.session_id();
    let mut state = state();
    state.model = "unavailable-provisional-model".into();
    let mut pending = runtime
        .dispatch(AppCommand::CreateSession {
            session_id: session.clone(),
            cwd: directory.path().to_path_buf(),
            origin: Origin::Tui,
            profile: PermissionProfile::Confirm,
        })
        .await
        .unwrap();
    let runner: Arc<dyn TurnRunner> = Arc::new(EchoTurnRunner);
    finish_authenticated_bootstrap(
        &runtime,
        &runner,
        &store,
        &session,
        &mut state,
        &mut pending,
    )
    .await
    .unwrap();
    assert!(pending.is_empty());
    assert_eq!(state.model, "medium-only");
    assert_eq!(state.thinking, ThinkingLevel::Medium);
    assert!(state.model_details.contains_key("medium-only"));
    assert_eq!(
        store.profile_preferences().unwrap().model.as_deref(),
        Some("medium-only")
    );
    assert_eq!(
        store.profile_preferences().unwrap().thinking_level,
        ThinkingLevel::Medium
    );
    assert_eq!(state.report_status(), ReportStatus::Unverified);
}

#[test]
fn agent_interactions_remain_visible_above_an_open_inspector() {
    let mut state = state();
    state.running = true;
    state.overlay = Some(Overlay::Usage);
    state.interaction = Some(InteractionView::Approval(ApprovalRequest {
        request_id: "approval".into(),
        explanation: "Approve test file change".into(),
        effect: crate::policy::Effect::FileWrite {
            path: PathBuf::from("/tmp/project/result.txt"),
        },
        allow_session_grants: false,
        suggested_prefix_scope: None,
    }));
    let approval = render_symbols(100, 30, &state);
    assert!(approval.contains("Permission required"));
    assert!(approval.contains("Approve test file change"));
    state.paste_text("y");
    assert!(
        state.input.is_empty(),
        "paste must not approve or alter a hidden draft"
    );
    state.interaction = Some(InteractionView::Questions {
        request: QuestionRequest {
            request_id: "question".into(),
            questions: vec![crate::app::QuestionSpec {
                id: "choice".into(),
                prompt: "Choose the test output".into(),
                options: Vec::new(),
                multiple: false,
                required: true,
            }],
        },
        index: 0,
    });
    let question = render_symbols(100, 30, &state);
    assert!(question.contains("Axiom needs your input"));
    assert!(question.contains("Choose the test output"));
}
