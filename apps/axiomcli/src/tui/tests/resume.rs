use super::*;
use crate::{
    agent::{EchoTurnRunner, TurnRunner},
    app::{CorrelationId, EventEnvelope, SessionId},
};
use std::sync::Arc;

const LEGACY_WARNING: &str = "Workspace changes recorded in this session: invoice.py, result.txt";
const REAL_WARNING: &str = "A real warning is retained.";

fn fixture(
    interrupted: bool,
    legacy_warning: bool,
) -> (tempfile::TempDir, SessionStore, SessionId) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.sqlite3");
    let session = SessionId::new();
    let turn = TurnId::new();
    let mut events = vec![
        AppEvent::SessionCreated {
            cwd: directory.path().canonicalize().unwrap(),
            origin: Origin::Tui,
            profile: PermissionProfile::Confirm,
        },
        AppEvent::PromptAccepted {
            turn_id: turn.clone(),
            text: "Fix the invoice".into(),
            attachments: Vec::new(),
        },
        AppEvent::TurnStarted {
            turn_id: turn.clone(),
        },
        AppEvent::ToolProposed {
            turn_id: turn.clone(),
            call_id: "edit-1".into(),
            name: "apply_patch".into(),
            arguments: serde_json::json!({}),
        },
        AppEvent::ToolStarted {
            call_id: "edit-1".into(),
            name: "apply_patch".into(),
        },
        AppEvent::WorkspaceChanged {
            paths: vec!["invoice.py".into(), "result.txt".into()],
        },
    ];
    if !interrupted {
        events.extend([
            AppEvent::ToolCompleted {
                call_id: "edit-1".into(),
                success: true,
            },
            AppEvent::TurnCompleted { turn_id: turn },
        ]);
    }
    events.push(AppEvent::WarningRaised {
        message: REAL_WARNING.into(),
    });
    if legacy_warning {
        events.push(AppEvent::WarningRaised {
            message: LEGACY_WARNING.into(),
        });
    }
    {
        let store = SessionStore::open(&path).unwrap();
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
    (directory, store, session)
}

async fn resume(store: &SessionStore, session: &SessionId) -> TuiState {
    let cwd = store.summary(session).unwrap().cwd;
    let runner: Arc<dyn TurnRunner> = Arc::new(EchoTurnRunner);
    restore_tui_session(
        &Runtime::new(32),
        &runner,
        store,
        &cwd,
        session,
        TuiState::new(
            cwd.clone(),
            "deepseek-v4-flash".into(),
            PermissionProfile::Confirm,
        ),
    )
    .await
    .unwrap()
}

fn persisted_warnings(store: &SessionStore, session: &SessionId) -> Vec<String> {
    store
        .load_recovering(session)
        .unwrap()
        .events
        .into_iter()
        .filter_map(|envelope| match envelope.event {
            AppEvent::WarningRaised { message } => Some(message),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn repeated_resume_keeps_workspace_history_without_adding_a_warning() {
    let (_directory, store, session) = fixture(false, false);
    for _ in 0..2 {
        let state = resume(&store, &session).await;
        assert!(
            !state
                .entries
                .iter()
                .any(|entry| entry.text.contains(LEGACY_WARNING))
        );
        assert!(
            state
                .entries
                .iter()
                .any(|entry| entry.text.contains(REAL_WARNING))
        );
        assert!(
            state
                .entries
                .iter()
                .filter_map(|entry| entry.task.as_ref())
                .any(|task| {
                    task.changed_paths.contains(&PathBuf::from("invoice.py"))
                        && task.changed_paths.contains(&PathBuf::from("result.txt"))
                })
        );
        assert_eq!(persisted_warnings(&store, &session), [REAL_WARNING]);
        assert_eq!(
            store.recovery_status(&session).unwrap().changed_paths.len(),
            2
        );
    }
}

#[tokio::test]
async fn resume_hides_legacy_edit_warnings_without_erasing_the_journal() {
    let (_directory, store, session) = fixture(false, true);
    let state = resume(&store, &session).await;
    assert!(
        !state
            .entries
            .iter()
            .any(|entry| entry.text.contains(LEGACY_WARNING))
    );
    assert!(
        state
            .entries
            .iter()
            .any(|entry| entry.text.contains(REAL_WARNING))
    );
    assert_eq!(
        persisted_warnings(&store, &session),
        [REAL_WARNING, LEGACY_WARNING]
    );
    assert_eq!(
        store.recovery_status(&session).unwrap().changed_paths.len(),
        2
    );
}

#[tokio::test]
async fn resume_still_warns_about_interrupted_turns_and_tools() {
    let (_directory, store, session) = fixture(true, false);
    let state = resume(&store, &session).await;
    let warning = "Recovered 1 interrupted turn(s) and 1 tool/task operation(s) with unknown status; nothing was replayed.";
    assert!(
        state
            .entries
            .iter()
            .any(|entry| entry.text.contains(warning))
    );
    assert_eq!(
        persisted_warnings(&store, &session),
        [REAL_WARNING, warning]
    );
    assert!(!state.running);
}
