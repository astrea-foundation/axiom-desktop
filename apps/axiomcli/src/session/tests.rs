//! Storage, account ownership and recovery regressions.

use std::{ffi::OsString, path::PathBuf, sync::Arc};

use chrono::Utc;
use rusqlite::{Connection, params};
use serde_json::{Value, json};

use crate::{
    AxiomError,
    app::{
        AppEvent, CorrelationId, EventEnvelope, Origin, PermissionProfile, SessionId,
        ThinkingLevel, TurnId,
    },
    paths::{AxiomPaths, FrontendKind},
};

use super::SessionStore;
use super::records::{
    ThreadLifecycle, ThreadRevision, TimelineItemKind, TimelineItemStatus, TurnStatus,
};
use super::schema::{APPLICATION_ID, SCHEMA_BASELINE, SCHEMA_VERSION};

use std::sync::Barrier;

use tempfile::tempdir;

fn envelope(id: &SessionId, sequence: u64, event: AppEvent) -> EventEnvelope {
    EventEnvelope {
        schema_version: 1,
        sequence,
        occurred_at: Utc::now(),
        correlation_id: CorrelationId::new(),
        origin: Origin::Test,
        session_id: id.clone(),
        event,
    }
}

fn create(store: &SessionStore, id: &SessionId) {
    store
        .append(&envelope(
            id,
            1,
            AppEvent::SessionCreated {
                cwd: PathBuf::from("/tmp/axiom-state-test"),
                origin: Origin::Test,
                profile: PermissionProfile::Confirm,
            },
        ))
        .expect("create thread");
}

fn routed_store() -> (tempfile::TempDir, SessionStore) {
    let root = tempdir().expect("temporary account root");
    let paths = AxiomPaths::from_roots(root.path().join("config"), root.path().join("data"));
    paths.prepare().expect("prepare account paths");
    let store = SessionStore::account_routed(paths, FrontendKind::DesktopChat);
    (root, store)
}

#[test]
fn title_generation_survives_activity_but_never_overwrites_a_manual_rename() {
    let store = SessionStore::in_memory().unwrap();
    let id = SessionId::new();
    create(&store, &id);
    let generation = store
        .begin_title_generation(&id, "First prompt")
        .unwrap()
        .unwrap();
    assert!(
        store
            .begin_title_generation(&id, "Duplicate")
            .unwrap()
            .is_none()
    );
    store
        .record_request_usage(
            &id,
            &axiom_inference::RequestUsage {
                request_id: "title-request".into(),
                purpose: axiom_inference::InvocationPurpose::Title,
                ..Default::default()
            },
        )
        .unwrap();
    assert!(
        store
            .finish_title_generation(&id, &generation, Some("Generated title"))
            .unwrap()
    );
    assert_eq!(
        store.thread_summary(&id).unwrap().title.as_deref(),
        Some("Generated title")
    );
    assert!(
        !store
            .finish_title_generation(&id, &generation, Some("Duplicate result"))
            .unwrap()
    );

    for manual in ["First prompt", "My custom title"] {
        let id = SessionId::new();
        create(&store, &id);
        let generation = store
            .begin_title_generation(&id, "First prompt")
            .unwrap()
            .unwrap();
        store.rename(&id, manual).unwrap();
        assert!(!store.title_generation_pending(&id, &generation).unwrap());
        assert!(
            !store
                .finish_title_generation(&id, &generation, Some("Late result"))
                .unwrap()
        );
        assert_eq!(
            store.thread_summary(&id).unwrap().title.as_deref(),
            Some(manual)
        );
    }
}

#[test]
fn title_generation_is_account_bound_and_cannot_recreate_a_deleted_thread() {
    let (_root, store) = routed_store();
    store.activate_account("account-a").unwrap();
    let id = SessionId::new();
    create(&store, &id);
    let bound = store.bind_active_account().unwrap();
    let generation = bound
        .begin_title_generation(&id, "Fallback")
        .unwrap()
        .unwrap();
    store.activate_account("account-b").unwrap();
    assert!(
        bound
            .finish_title_generation(&id, &generation, Some("Late title"))
            .is_err()
    );
    store.activate_account("account-a").unwrap();
    assert_eq!(
        store.thread_summary(&id).unwrap().title.as_deref(),
        Some("Fallback")
    );
    store.delete_sessions(std::slice::from_ref(&id)).unwrap();
    assert!(
        !store
            .finish_title_generation(&id, &generation, Some("Late title"))
            .unwrap()
    );
    assert!(store.thread_summary(&id).is_err());
}

#[test]
fn version_two_store_upgrades_preserving_titles_and_history() {
    let root = tempdir().unwrap();
    let path = root.path().join("history.sqlite3");
    let store = SessionStore::open(&path).unwrap();
    let id = SessionId::new();
    create(&store, &id);
    store.rename(&id, "Existing name").unwrap();
    let history = store.load(&id).unwrap();
    // Reproduce a schema-2 database with its original data, then reopen it.
    store
        .lock()
        .unwrap()
        .execute_batch(
            "ALTER TABLE threads DROP COLUMN title_generation_id; PRAGMA user_version=2;",
        )
        .unwrap();
    drop(store);
    let upgraded = SessionStore::open(&path).unwrap();
    assert_eq!(
        upgraded.thread_summary(&id).unwrap().title.as_deref(),
        Some("Existing name")
    );
    assert_eq!(upgraded.load(&id).unwrap().len(), history.len());
    assert!(
        upgraded
            .begin_title_generation(&id, "Replacement")
            .unwrap()
            .is_none()
    );
    let fresh = SessionId::new();
    create(&upgraded, &fresh);
    assert!(
        upgraded
            .begin_title_generation(&fresh, "Fresh prompt")
            .unwrap()
            .is_some()
    );
}

#[test]
fn request_accounting_recovery_is_idempotent_account_scoped_and_never_verifies() {
    use axiom_inference::{InvocationState, RequestUsage, UsageCompleteness};
    let (_root, store) = routed_store();
    store.activate_account("account-a").unwrap();
    let id = SessionId::new();
    create(&store, &id);
    let local = RequestUsage {
        request_id: "a".repeat(32),
        model_id: "m".into(),
        provider_id: "near".into(),
        state: InvocationState::Failed,
        turn_id: Some("turn-a".into()),
        started_at_ms: "1000".into(),
        error_code: Some("CLIENT_INTERRUPTED".into()),
        ..RequestUsage::default()
    };
    store.record_request_usage(&id, &local).unwrap();
    let remote = RequestUsage {
        state: InvocationState::Completed,
        completeness: UsageCompleteness::Final,
        input_tokens: Some("9007199254740993".into()),
        output_tokens: Some("55".into()),
        reasoning_tokens: Some("40".into()),
        cost_microusd: Some("112".into()),
        settled: true,
        finished_at_ms: Some("8000".into()),
        ..local.clone()
    };
    store
        .reconcile_request_usage(&id, &[remote.clone(), remote.clone()])
        .unwrap();
    let recovered = store.thread_snapshot(&id, None, 100).unwrap().request_usage;
    assert_eq!(recovered.len(), 1);
    assert_eq!(
        recovered[0].input_tokens.as_deref(),
        Some("9007199254740993")
    );
    assert_eq!(recovered[0].state, InvocationState::Failed);
    assert!(!recovered[0].response_verified);
    assert_eq!(recovered[0].finished_at_ms.as_deref(), Some("8000"));
    assert_eq!(recovered[0].turn_id, local.turn_id);
    assert!(
        store
            .reconcile_request_usage(
                &id,
                &[RequestUsage {
                    response_verified: true,
                    ..remote.clone()
                }]
            )
            .is_err()
    );
    store.activate_account("account-b").unwrap();
    create(&store, &id);
    store.reconcile_request_usage(&id, &[remote]).unwrap();
    assert!(
        store
            .thread_snapshot(&id, None, 100)
            .unwrap()
            .request_usage
            .is_empty()
    );
    store.activate_account("account-a").unwrap();
    assert_eq!(
        store.thread_snapshot(&id, None, 100).unwrap().request_usage,
        recovered
    );
}

#[test]
fn message_revision_is_idle_account_scoped_and_preserves_request_accounting() {
    let (_root, store) = routed_store();
    store.activate_account("a").unwrap();
    let id = SessionId::new();
    let first = TurnId::new();
    let second = TurnId::new();
    create(&store, &id);
    begin_turn(&store, &id, &first, None);
    store
        .append(&envelope(
            &id,
            4,
            AppEvent::TurnCompleted {
                turn_id: first.clone(),
            },
        ))
        .unwrap();
    begin_turn(&store, &id, &second, None);
    let snapshot = store.thread_snapshot(&id, None, 100).unwrap();
    let item = snapshot
        .items
        .iter()
        .rev()
        .find(|item| item.kind == TimelineItemKind::UserMessage)
        .unwrap()
        .id
        .clone();
    let revision = store.thread_summary(&id).unwrap().revision;
    assert!(
        store
            .truncate_before_user_message(&id, &item, revision)
            .is_err(),
        "active turns cannot be revised"
    );
    store
        .append(&envelope(
            &id,
            6,
            AppEvent::TurnCancelled {
                turn_id: second.clone(),
            },
        ))
        .unwrap();
    let usage = axiom_inference::RequestUsage {
        request_id: "c".repeat(32),
        model_id: "m".into(),
        provider_id: "tinfoil".into(),
        turn_id: Some(second.to_string()),
        ..Default::default()
    };
    store.record_request_usage(&id, &usage).unwrap();
    let revision = store.thread_summary(&id).unwrap().revision;
    assert!(
        store
            .truncate_before_user_message(&id, &item, revision - 1)
            .is_err()
    );
    assert!(
        store
            .truncate_before_user_message(&id, "foreign-message", revision)
            .is_err()
    );
    let before = store.thread_snapshot(&id, None, 100).unwrap();
    let invalid_replacement = [
        envelope(
            &id,
            7,
            AppEvent::PromptAccepted {
                attachments: Vec::new(),
                turn_id: first.clone(),
                text: "replacement".into(),
            },
        ),
        envelope(&id, 8, AppEvent::TurnStarted { turn_id: first }),
    ];
    assert!(
        store
            .append_prompt_revision(
                &invalid_replacement,
                None,
                Some(&axiom_acp_extension::PromptRevision {
                    user_item_id: item.clone(),
                    expected_revision: revision,
                })
            )
            .is_err(),
        "a replacement using an existing turn ID must roll back both truncation and insertion"
    );
    assert_eq!(store.thread_snapshot(&id, None, 100).unwrap(), before);
    store
        .truncate_before_user_message(&id, &item, revision)
        .unwrap();
    let after = store.thread_snapshot(&id, None, 100).unwrap();
    assert_eq!(after.items.len(), 2);
    assert_eq!(after.request_usage, vec![usage]);
    let events = store.load(&id).unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.event, AppEvent::PromptAccepted { .. }))
            .count(),
        1
    );
    assert!(
        store
            .truncate_before_user_message(&id, &item, revision)
            .is_err(),
        "cannot replay an edit"
    );
    store.activate_account("b").unwrap();
    create(&store, &id);
    assert!(
        store
            .truncate_before_user_message(&id, &item, store.thread_summary(&id).unwrap().revision)
            .is_err()
    );
}

#[test]
fn cancelling_a_live_turn_preserves_pending_usage_for_recovery_without_restart() {
    use axiom_inference::{InvocationState, RequestUsage, UsageCompleteness};
    let (_root, store) = routed_store();
    store.activate_account("account-a").unwrap();
    let id = SessionId::new();
    let turn = TurnId::new();
    create(&store, &id);
    begin_turn(&store, &id, &turn, None);
    let running = RequestUsage {
        request_id: "b".repeat(32),
        model_id: "m".into(),
        provider_id: "tinfoil".into(),
        state: InvocationState::Running,
        turn_id: Some(turn.to_string()),
        started_at_ms: "1000".into(),
        ..RequestUsage::default()
    };
    store.record_request_usage(&id, &running).unwrap();
    store
        .append(&envelope(&id, 4, AppEvent::TurnCancelled { turn_id: turn }))
        .unwrap();
    let usage = store.thread_snapshot(&id, None, 100).unwrap().request_usage;
    assert_eq!(usage[0].state, InvocationState::Cancelled);
    assert!(!usage[0].settled && !usage[0].response_verified);
    assert!(usage[0].finished_at_ms.is_some());
    let remote = RequestUsage {
        state: InvocationState::Completed,
        settled: true,
        completeness: UsageCompleteness::Final,
        input_tokens: Some("10".into()),
        output_tokens: Some("20".into()),
        cost_microusd: Some("30".into()),
        ..running
    };
    store.reconcile_request_usage(&id, &[remote]).unwrap();
    let usage = store.thread_snapshot(&id, None, 100).unwrap().request_usage;
    assert_eq!(usage[0].state, InvocationState::Cancelled);
    assert!(usage[0].settled);
    assert!(!usage[0].response_verified);
    assert_eq!(usage[0].cost_microusd.as_deref(), Some("30"));
}

#[test]
fn desktop_agent_settings_are_atomic_idle_only_and_account_scoped() {
    use axiom_acp_extension::{ConfigureDesktopAgentRequest, DesktopAgentPermission};
    let (root, store) = routed_store();
    store.activate_account("account-a").unwrap();
    let id = SessionId::new();
    create(&store, &id);
    let original = store.desktop_agent_settings(&id).unwrap();
    assert!(!original.enabled);
    let chosen = root.path().join("My Project 日本語");
    std::fs::create_dir(&chosen).unwrap();
    let request = ConfigureDesktopAgentRequest {
        thread_id: id.to_string(),
        expected_revision: 0,
        enabled: true,
        permission: DesktopAgentPermission::FullAccess,
        working_directory: Some(chosen.to_str().unwrap().into()),
    };
    let settings = store.prepare_desktop_agent_settings(&id, &request).unwrap();
    let event = envelope(
        &id,
        2,
        AppEvent::DesktopAgentConfigured {
            settings: settings.clone(),
        },
    );
    store.append(&event).unwrap();
    assert_eq!(store.desktop_agent_settings(&id).unwrap(), settings);
    let summary = store.thread_summary(&id).unwrap();
    assert_eq!(summary.profile, PermissionProfile::FullAccess.to_string());
    assert_eq!(summary.cwd, chosen.canonicalize().unwrap());
    assert!(summary.last_message_at.is_none());
    assert!(
        store.append(&event).is_err(),
        "stale revisions cannot apply twice"
    );
    assert!(store.prepare_desktop_agent_settings(&id, &request).is_err());
    let mut update = request.clone();
    update.expected_revision = 1;
    update.enabled = false;
    let off = store.prepare_desktop_agent_settings(&id, &update).unwrap();
    let turn = TurnId::new();
    begin_turn(&store, &id, &turn, None);
    assert!(
        store
            .append(&envelope(
                &id,
                5,
                AppEvent::DesktopAgentConfigured { settings: off }
            ))
            .is_err()
    );
    assert_eq!(store.desktop_agent_settings(&id).unwrap(), settings);
    assert_eq!(
        store.thread_summary(&id).unwrap().profile,
        PermissionProfile::FullAccess.to_string()
    );
    let account_a = store.bind_active_account().unwrap();
    store.activate_account("account-b").unwrap();
    assert!(account_a.desktop_agent_settings(&id).is_err());
    assert!(store.desktop_agent_settings(&id).is_err());
    store.activate_account("account-a").unwrap();
    assert_eq!(store.desktop_agent_settings(&id).unwrap(), settings);
}

#[test]
fn account_bound_read_paused_before_store_lock_cannot_disclose_the_next_account() {
    let (_root, store) = routed_store();
    store.activate_account("account-a").expect("activate A");
    let thread_a = SessionId::new();
    create(&store, &thread_a);
    store
        .rename(&thread_a, "account-a-private")
        .expect("name A thread");
    let account_a_store = store.bind_active_account().expect("bind A store");

    let paused_before_lock = Arc::new(Barrier::new(2));
    let resume = Arc::new(Barrier::new(2));
    let reader_paused = paused_before_lock.clone();
    let reader_resume = resume.clone();
    let reader = std::thread::spawn(move || {
        reader_paused.wait();
        reader_resume.wait();
        account_a_store.thread_catalog(None, true, None, 200)
    });

    paused_before_lock.wait();
    store.activate_account("account-b").expect("activate B");
    let thread_b = SessionId::new();
    create(&store, &thread_b);
    store
        .rename(&thread_b, "account-b-private")
        .expect("name B thread");
    resume.wait();

    let error = reader
        .join()
        .expect("reader thread")
        .expect_err("the A binding must be stale");
    assert!(
        matches!(error, AxiomError::InvalidTransition(ref detail) if detail.contains("changed"))
    );
    let account_b_threads = store.list_threads(true).expect("B catalog");
    assert_eq!(account_b_threads.len(), 1);
    assert_eq!(
        account_b_threads[0].title.as_deref(),
        Some("account-b-private")
    );
}

#[test]
fn account_bound_mutation_paused_before_store_lock_cannot_write_the_next_account() {
    let (_root, store) = routed_store();
    store.activate_account("account-a").expect("activate A");
    let account_a_store = store.bind_active_account().expect("bind A store");

    let paused_before_lock = Arc::new(Barrier::new(2));
    let resume = Arc::new(Barrier::new(2));
    let writer_paused = paused_before_lock.clone();
    let writer_resume = resume.clone();
    let writer = std::thread::spawn(move || {
        writer_paused.wait();
        writer_resume.wait();
        account_a_store.create_collection("must-not-reach-account-b")
    });

    paused_before_lock.wait();
    store.activate_account("account-b").expect("activate B");
    resume.wait();

    let error = writer
        .join()
        .expect("writer thread")
        .expect_err("the A binding must be stale");
    assert!(
        matches!(error, AxiomError::InvalidTransition(ref detail) if detail.contains("changed"))
    );
    assert!(
        store
            .list_collections()
            .expect("B collections")
            .collections
            .is_empty()
    );
}

#[test]
fn account_binding_generation_rejects_a_reactivated_copy_of_the_same_account() {
    let (_root, store) = routed_store();
    store.activate_account("account-a").expect("activate A");
    let original_activation = store.bind_active_account().expect("bind first A");

    store.deactivate_account().expect("deactivate A");
    store.activate_account("account-a").expect("reactivate A");

    assert!(store.profile_preferences().is_ok());
    assert!(matches!(
        original_activation.profile_preferences(),
        Err(AxiomError::InvalidTransition(detail)) if detail.contains("changed")
    ));
}

fn begin_turn(
    store: &SessionStore,
    id: &SessionId,
    turn: &TurnId,
    client_item_id: Option<&str>,
) -> ThreadRevision {
    store
        .append_all_with_client_item_id(
            &[
                envelope(
                    id,
                    2,
                    AppEvent::PromptAccepted {
                        attachments: Vec::new(),
                        turn_id: turn.clone(),
                        text: "hello".into(),
                    },
                ),
                envelope(
                    id,
                    3,
                    AppEvent::TurnStarted {
                        turn_id: turn.clone(),
                    },
                ),
            ],
            client_item_id,
        )
        .expect("begin turn")
}

#[test]
fn timeline_sequence_uniqueness_also_supplies_the_pagination_index() {
    let store = SessionStore::in_memory().unwrap();
    let id = SessionId::new();
    create(&store, &id);
    let connection = store.lock().unwrap();
    let insert = "INSERT INTO timeline_items
            (id, thread_id, sequence, kind, status, created_at, updated_at)
            VALUES (?1, ?2, 1, 'user_message', 'completed', 'now', 'now')";
    connection
        .execute(insert, params!["first", id.to_string()])
        .unwrap();
    assert!(
        connection
            .execute(insert, params!["duplicate", id.to_string()])
            .is_err()
    );
    let duplicate_index: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='idx_timeline_thread_sequence')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(!duplicate_index);
    let detail: String = connection
        .query_row(
            "EXPLAIN QUERY PLAN SELECT id FROM timeline_items
             WHERE thread_id=?1 AND sequence>?2 ORDER BY sequence LIMIT 10",
            params![id.to_string(), 0],
            |row| row.get(3),
        )
        .unwrap();
    assert!(
        detail.contains("USING INDEX"),
        "pagination must remain indexed: {detail}"
    );
}

#[test]
fn fresh_and_current_databases_use_the_clean_baseline() {
    let directory = tempdir().expect("temp dir");
    let fresh = directory.path().join("state.sqlite3");
    let store = SessionStore::open(&fresh).expect("fresh store");
    assert!(
        store.is_active(),
        "explicit non-routed stores remain usable by tests and automation"
    );
    let connection = store.lock().expect("lock");
    assert_eq!(
        connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .expect("version"),
        SCHEMA_VERSION
    );
    assert_eq!(
        connection
            .query_row("PRAGMA application_id", [], |row| row.get::<_, i64>(0))
            .expect("application ID"),
        APPLICATION_ID
    );
    for retired in ["sessions", "events", "snapshots", "app_settings"] {
        let exists: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
                [retired],
                |row| row.get(0),
            )
            .expect("schema lookup");
        assert!(!exists, "retired table {retired} must not exist");
    }
    drop(connection);
    drop(store);
    SessionStore::open(&fresh).expect("current schema reopens without migration");

    let old = directory.path().join("old.sqlite3");
    Connection::open(&old)
        .expect("old db")
        .execute_batch("CREATE TABLE events(payload TEXT); PRAGMA user_version=0;")
        .expect("old schema");
    assert!(SessionStore::open(&old).is_err());

    // Even schema v1 from the old application contract must be reset.
    let retired = directory.path().join("retired.sqlite3");
    Connection::open(&retired).unwrap().execute_batch(
            "CREATE TABLE threads(id TEXT); PRAGMA application_id=1096302898; PRAGMA user_version=1;"
        ).unwrap();
    assert!(SessionStore::open(&retired).is_err());

    let future = directory.path().join("future.sqlite3");
    Connection::open(&future)
        .expect("future db")
        .pragma_update(None, "user_version", SCHEMA_VERSION + 1)
        .expect("future schema");
    assert!(SessionStore::open(&future).is_err());
}

fn usage_report(input_tokens: u64) -> axiom_acp_extension::ContextUsage {
    axiom_acp_extension::ContextUsage {
        input_tokens,
        output_tokens: 1_000,
        model_id: "original-model".into(),
        reported_at: "2026-09-07T00:00:00Z".into(),
        context_window_tokens: Some(100_000),
        auto_compact_threshold_tokens: Some(85_000),
    }
}

#[test]
fn provider_reports_persist_across_reload_without_auxiliary_usage_or_compaction_overwriting_them() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("state.sqlite3");
    let store = SessionStore::open(&path).unwrap();
    let id = SessionId::new();
    let other = SessionId::new();
    let turn = TurnId::new();
    create(&store, &id);
    create(&store, &other);
    begin_turn(&store, &id, &turn, None);
    let before = store.thread_snapshot(&id, None, 100).unwrap();
    for (sequence, input) in [(4, 70_000), (5, 75_000)] {
        store
            .append(&envelope(
                &id,
                sequence,
                AppEvent::ContextUsageUpdated {
                    usage: usage_report(input),
                },
            ))
            .unwrap();
    }
    let after = store.thread_snapshot(&id, None, 100).unwrap();
    assert_eq!(
        after.context_usage,
        Some(usage_report(75_000)),
        "last request, never a sum"
    );
    assert_eq!(
        after.items, before.items,
        "no report rows or altered transcript"
    );
    assert_eq!(after.thread.last_message_at, before.thread.last_message_at);
    assert!(after.thread.revision > before.thread.revision);
    store
        .append(&envelope(
            &id,
            6,
            AppEvent::UsageUpdated {
                input_tokens: 9,
                output_tokens: 8,
            },
        ))
        .unwrap();
    store
        .append(&envelope(
            &id,
            7,
            AppEvent::ContextCompacted {
                summary: "Compacted context".into(),
                messages_before: 2,
            },
        ))
        .unwrap();
    store
        .append(&envelope(
            &id,
            8,
            AppEvent::ModelChanged {
                model: "different-model".into(),
            },
        ))
        .unwrap();
    store
        .append(&envelope(&id, 9, AppEvent::TurnCompleted { turn_id: turn }))
        .unwrap();
    store.rename(&id, "Renamed thread").unwrap();
    drop(store);
    let reopened = SessionStore::open(&path).unwrap();
    assert_eq!(
        reopened
            .thread_snapshot(&id, None, 100)
            .unwrap()
            .context_usage,
        Some(usage_report(75_000))
    );
    assert!(
        reopened
            .thread_snapshot(&other, None, 100)
            .unwrap()
            .context_usage
            .is_none()
    );
    reopened.delete_sessions(std::slice::from_ref(&id)).unwrap();
    assert!(reopened.thread_snapshot(&id, None, 100).is_err());
    assert!(reopened.thread_snapshot(&other, None, 100).is_ok());
}

#[test]
fn provider_report_is_account_scoped_even_when_the_same_thread_id_is_reused() {
    let (_root, store) = routed_store();
    let id = SessionId::new();
    store.activate_account("account-a").unwrap();
    create(&store, &id);
    store
        .append(&envelope(
            &id,
            2,
            AppEvent::ContextUsageUpdated {
                usage: usage_report(75_000),
            },
        ))
        .unwrap();
    store.activate_account("account-b").unwrap();
    create(&store, &id);
    assert!(
        store
            .thread_snapshot(&id, None, 10)
            .unwrap()
            .context_usage
            .is_none()
    );
    store.activate_account("account-a").unwrap();
    assert_eq!(
        store.thread_snapshot(&id, None, 10).unwrap().context_usage,
        Some(usage_report(75_000))
    );
}

#[test]
fn prompt_transaction_creates_turn_user_and_assistant_with_client_identity() {
    let store = SessionStore::in_memory().expect("store");
    let id = SessionId::new();
    let turn = TurnId::new();
    create(&store, &id);
    let revision = begin_turn(&store, &id, &turn, Some("renderer-message-1"));
    assert_eq!(revision.revision, 2);
    assert_eq!(revision.last_timeline_sequence, 2);

    let snapshot = store.thread_snapshot(&id, None, 10).expect("snapshot");
    assert_eq!(snapshot.turns.len(), 1);
    assert_eq!(snapshot.turns[0].status, TurnStatus::Running);
    assert_eq!(snapshot.items.len(), 2);
    assert_eq!(snapshot.items[0].kind, TimelineItemKind::UserMessage);
    assert_eq!(
        snapshot.items[0].client_item_id.as_deref(),
        Some("renderer-message-1")
    );
    assert_eq!(snapshot.items[1].kind, TimelineItemKind::AssistantMessage);
    assert_eq!(snapshot.items[1].status, TimelineItemStatus::InProgress);
}

#[test]
fn streaming_updates_mutate_single_items_and_terminal_commit_finishes_them() {
    let store = SessionStore::in_memory().expect("store");
    let id = SessionId::new();
    let turn = TurnId::new();
    create(&store, &id);
    begin_turn(&store, &id, &turn, None);
    for (sequence, text) in [(4, "one "), (5, "two"), (6, " three")] {
        store
            .append(&envelope(
                &id,
                sequence,
                AppEvent::TextDelta {
                    turn_id: turn.clone(),
                    text: text.into(),
                },
            ))
            .expect("stream checkpoint");
    }
    store
        .append(&envelope(
            &id,
            7,
            AppEvent::TurnCompleted {
                turn_id: turn.clone(),
            },
        ))
        .expect("complete");
    let snapshot = store.thread_snapshot(&id, None, 10).expect("snapshot");
    assert_eq!(snapshot.items.len(), 2);
    assert_eq!(snapshot.items[1].content, "one two three");
    assert_eq!(snapshot.items[1].status, TimelineItemStatus::Completed);
    assert_eq!(snapshot.turns[0].status, TurnStatus::Completed);
    assert_eq!(snapshot.thread.lifecycle, ThreadLifecycle::Ready);
}

#[test]
fn steering_preserves_same_turn_chronology_after_restart() {
    let directory = tempdir().expect("temp dir");
    let path = directory.path().join("state.sqlite3");
    let store = SessionStore::open(&path).expect("store");
    let id = SessionId::new();
    let turn = TurnId::new();
    create(&store, &id);
    begin_turn(&store, &id, &turn, Some("original-input"));
    let before = store
        .append(&envelope(
            &id,
            4,
            AppEvent::TextDelta {
                turn_id: turn.clone(),
                text: "Before steering.".into(),
            },
        ))
        .unwrap();
    let input = AppEvent::SteeringApplied {
        turn_id: turn.clone(),
        client_item_id: "steer-input".into(),
        text: "New direction".into(),
    };
    store.append(&envelope(&id, 5, input.clone())).unwrap();
    let after = store
        .append(&envelope(
            &id,
            6,
            AppEvent::TextDelta {
                turn_id: turn.clone(),
                text: "After steering.".into(),
            },
        ))
        .unwrap();
    assert_ne!(before.timeline_item_id, after.timeline_item_id);
    assert!(
        store.append(&envelope(&id, 7, input)).is_err(),
        "duplicate input IDs never create another message"
    );
    store
        .append(&envelope(
            &id,
            8,
            AppEvent::ResponseVerified {
                turn_id: turn.clone(),
            },
        ))
        .unwrap();
    store
        .append(&envelope(
            &id,
            9,
            AppEvent::TurnCompleted {
                turn_id: turn.clone(),
            },
        ))
        .unwrap();
    let snapshot = store.thread_snapshot(&id, None, 100).unwrap();
    assert_eq!(snapshot.turns.len(), 1);
    assert_eq!(snapshot.turns[0].status, TurnStatus::Completed);
    assert_eq!(
        snapshot
            .items
            .iter()
            .map(|item| item.content.as_str())
            .collect::<Vec<_>>(),
        vec![
            "hello",
            "Before steering.",
            "New direction",
            "After steering."
        ]
    );
    assert!(
        snapshot
            .items
            .iter()
            .all(|item| item.turn_id.as_deref() == Some(turn.to_string().as_str()))
    );
    assert_eq!(
        snapshot.items[2].client_item_id.as_deref(),
        Some("steer-input")
    );
    assert_eq!(snapshot.items[2].metadata["steering"], true);
    drop(store);
    let reopened = SessionStore::open(&path).unwrap();
    assert_eq!(snapshot, reopened.thread_snapshot(&id, None, 100).unwrap());
    let replay = reopened.load(&id).unwrap();
    assert_eq!(
        replay
            .iter()
            .filter(|event| matches!(event.event, AppEvent::TurnStarted { .. }))
            .count(),
        1
    );
    assert_eq!(
        replay
            .iter()
            .filter(|event| matches!(event.event, AppEvent::PromptAccepted { .. }))
            .count(),
        1
    );
    assert_eq!(
        replay
            .iter()
            .filter(|event| matches!(event.event, AppEvent::SteeringApplied { .. }))
            .count(),
        1
    );
    let mut kernel = crate::app::Kernel::default();
    kernel
        .restore_from_events(&replay)
        .expect("valid same-turn replay");
}

#[test]
fn tool_boundaries_preserve_message_segments_through_restart_and_replay() {
    let directory = tempdir().expect("temp dir");
    let path = directory.path().join("state.sqlite3");
    let store = SessionStore::open(&path).expect("store");
    let id = SessionId::new();
    let turn = TurnId::new();
    create(&store, &id);
    begin_turn(&store, &id, &turn, None);
    let mut sequence = 4;
    let mut append = |event| {
        sequence += 1;
        store
            .append(&envelope(&id, sequence, event))
            .expect("append")
    };
    let intro = append(AppEvent::TextDelta {
        turn_id: turn.clone(),
        text: "Let me check 🌍: ".into(),
    });
    for (call, text) in [
        ("search-1", "One result. "),
        ("search-2", "The news is xyz."),
    ] {
        append(AppEvent::ToolProposed {
            turn_id: turn.clone(),
            call_id: call.into(),
            name: "web_search".into(),
            arguments: json!({"query":"current news"}),
        });
        append(AppEvent::ToolStarted {
            call_id: call.into(),
            name: "web_search".into(),
        });
        append(AppEvent::ToolCompleted {
            call_id: call.into(),
            success: true,
        });
        append(AppEvent::ReasoningDelta {
            turn_id: turn.clone(),
            text: "Check the sources.".into(),
        });
        let delta = append(AppEvent::TextDelta {
            turn_id: turn.clone(),
            text: text.into(),
        });
        assert_ne!(delta.timeline_item_id, intro.timeline_item_id);
        let continuation = append(AppEvent::TextDelta {
            turn_id: turn.clone(),
            text: "More. ".into(),
        });
        assert_eq!(delta.timeline_item_id, continuation.timeline_item_id);
    }
    append(AppEvent::ResponseVerified {
        turn_id: turn.clone(),
    });
    append(AppEvent::TurnCompleted { turn_id: turn });
    let before = store.thread_snapshot(&id, None, 100).expect("snapshot");
    drop(store);
    let reopened = SessionStore::open(&path).expect("reopen");
    let after = reopened.thread_snapshot(&id, None, 100).expect("snapshot");
    assert_eq!(before.items, after.items);
    let visible = after
        .items
        .iter()
        .filter(|item| item.kind != TimelineItemKind::Reasoning)
        .collect::<Vec<_>>();
    assert_eq!(
        visible.iter().map(|item| item.kind).collect::<Vec<_>>(),
        vec![
            TimelineItemKind::UserMessage,
            TimelineItemKind::AssistantMessage,
            TimelineItemKind::ToolCall,
            TimelineItemKind::AssistantMessage,
            TimelineItemKind::ToolCall,
            TimelineItemKind::AssistantMessage,
        ]
    );
    assert_eq!(visible[1].content, "Let me check 🌍: ");
    assert_eq!(visible[3].content, "One result. More. ");
    assert_eq!(visible[5].content, "The news is xyz.More. ");
    assert!(
        after
            .items
            .iter()
            .filter(|item| matches!(
                item.kind,
                TimelineItemKind::AssistantMessage | TimelineItemKind::Reasoning
            ))
            .all(|item| item.metadata["terminal_verified"] == true)
    );
    let replay = reopened.load(&id).expect("runtime projection");
    let order = replay
        .iter()
        .filter_map(|event| match &event.event {
            AppEvent::TextDelta { text, .. } => Some(text.as_str()),
            AppEvent::ToolProposed { call_id, .. } => Some(call_id.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        order,
        vec![
            "Let me check 🌍: ",
            "search-1",
            "One result. More. ",
            "search-2",
            "The news is xyz.More. "
        ]
    );
}

#[test]
fn tool_first_response_does_not_fill_the_pre_tool_placeholder() {
    let store = SessionStore::in_memory().expect("store");
    let id = SessionId::new();
    let turn = TurnId::new();
    create(&store, &id);
    begin_turn(&store, &id, &turn, None);
    store
        .append(&envelope(
            &id,
            4,
            AppEvent::ToolProposed {
                turn_id: turn.clone(),
                call_id: "search".into(),
                name: "web_search".into(),
                arguments: json!({"query":"news"}),
            },
        ))
        .expect("tool");
    let delta = store
        .append(&envelope(
            &id,
            5,
            AppEvent::TextDelta {
                turn_id: turn.clone(),
                text: "Unverified partial".into(),
            },
        ))
        .expect("delta");
    store
        .append(&envelope(&id, 6, AppEvent::TurnCancelled { turn_id: turn }))
        .expect("cancel");
    let snapshot = store.thread_snapshot(&id, None, 100).expect("snapshot");
    assert_eq!(snapshot.items[1].content, "");
    assert_eq!(snapshot.items[2].kind, TimelineItemKind::ToolCall);
    assert_eq!(snapshot.items[3].content, "Unverified partial");
    assert_eq!(
        delta.timeline_item_id.as_deref(),
        Some(snapshot.items[3].id.as_str())
    );
    assert_eq!(snapshot.items[3].status, TimelineItemStatus::Cancelled);
    assert_ne!(snapshot.items[3].metadata["terminal_verified"], true);
}

#[test]
fn request_receipt_binds_initial_assistant_and_reasoning_through_restart() {
    use axiom_inference::{InvocationState, RequestUsage};
    let directory = tempdir().unwrap();
    let path = directory.path().join("state.sqlite3");
    let store = SessionStore::open(&path).unwrap();
    let id = SessionId::new();
    let turn = TurnId::new();
    create(&store, &id);
    begin_turn(&store, &id, &turn, None);
    let mut usage = RequestUsage {
        request_id: "a".repeat(32),
        turn_id: Some(turn.to_string()),
        ..RequestUsage::default()
    };
    let mut sequence = 3;
    let mut append = |event| {
        sequence += 1;
        store.append(&envelope(&id, sequence, event)).unwrap()
    };
    append(AppEvent::RequestUsageUpdated {
        usage: usage.clone(),
    });
    append(AppEvent::ReasoningDelta {
        turn_id: turn.clone(),
        text: "Synthetic reasoning".into(),
    });
    append(AppEvent::TextDelta {
        turn_id: turn.clone(),
        text: "Synthetic reply".into(),
    });
    let before = store.thread_snapshot(&id, None, 100).unwrap();
    for item in &before.items[1..] {
        assert_eq!(item.metadata["request_id"], usage.request_id);
        assert_ne!(item.metadata["terminal_verified"], true);
    }
    usage.state = InvocationState::Completed;
    usage.response_verified = true;
    usage.finish_reason = Some("length".into());
    append(AppEvent::RequestUsageUpdated {
        usage: usage.clone(),
    });
    append(AppEvent::ResponseVerified {
        turn_id: turn.clone(),
    });
    append(AppEvent::TurnCompleted { turn_id: turn });
    drop(store);
    let reopened = SessionStore::open(&path).unwrap();
    let after = reopened.thread_snapshot(&id, None, 100).unwrap();
    assert_eq!(after.items.len(), 3);
    for item in &after.items[1..] {
        assert_eq!(item.metadata["request_id"], usage.request_id);
        assert_eq!(item.metadata["terminal_verified"], true);
        assert_eq!(item.metadata["finish_reason"], "length");
        assert_eq!(item.status, TimelineItemStatus::Completed);
    }
}

#[test]
fn later_verified_request_cannot_verify_an_earlier_partial_in_the_same_turn() {
    use axiom_inference::{InvocationPurpose, InvocationState, RequestUsage};
    let store = SessionStore::in_memory().unwrap();
    let id = SessionId::new();
    let turn = TurnId::new();
    create(&store, &id);
    begin_turn(&store, &id, &turn, None);
    let mut sequence = 3;
    let mut append = |event| {
        sequence += 1;
        store.append(&envelope(&id, sequence, event)).unwrap()
    };
    let mut first = RequestUsage {
        request_id: "a".repeat(32),
        turn_id: Some(turn.to_string()),
        ..RequestUsage::default()
    };
    append(AppEvent::RequestUsageUpdated {
        usage: first.clone(),
    });
    append(AppEvent::TextDelta {
        turn_id: turn.clone(),
        text: "Unverified partial".into(),
    });
    first.state = InvocationState::Cancelled;
    append(AppEvent::RequestUsageUpdated {
        usage: first.clone(),
    });
    let mut second = RequestUsage {
        request_id: "b".repeat(32),
        state: InvocationState::Running,
        ..first.clone()
    };
    append(AppEvent::RequestUsageUpdated {
        usage: second.clone(),
    });
    append(AppEvent::RequestUsageUpdated {
        usage: RequestUsage {
            request_id: "c".repeat(32),
            purpose: InvocationPurpose::Compaction,
            ..second.clone()
        },
    });
    append(AppEvent::RequestUsageUpdated {
        usage: RequestUsage {
            request_id: "d".repeat(32),
            turn_id: Some(TurnId::new().to_string()),
            ..second.clone()
        },
    });
    append(AppEvent::TextDelta {
        turn_id: turn.clone(),
        text: "Verified new response".into(),
    });
    append(AppEvent::ReasoningDelta {
        turn_id: turn.clone(),
        text: "Verified new reasoning".into(),
    });
    second.state = InvocationState::Completed;
    second.response_verified = true;
    append(AppEvent::RequestUsageUpdated {
        usage: second.clone(),
    });
    append(AppEvent::ResponseVerified {
        turn_id: turn.clone(),
    });
    append(AppEvent::TurnCompleted { turn_id: turn });
    let snapshot = store.thread_snapshot(&id, None, 100).unwrap();
    assert_eq!(snapshot.items.len(), 4);
    assert_eq!(snapshot.items[1].content, "Unverified partial");
    assert_eq!(snapshot.items[1].metadata["request_id"], first.request_id);
    assert_ne!(snapshot.items[1].metadata["terminal_verified"], true);
    for item in &snapshot.items[2..] {
        assert_eq!(item.metadata["request_id"], second.request_id);
        assert_eq!(item.metadata["terminal_verified"], true);
    }
}

#[test]
fn verified_tool_only_request_binds_its_empty_assistant_placeholder() {
    use axiom_inference::{InvocationState, RequestUsage};
    let store = SessionStore::in_memory().unwrap();
    let id = SessionId::new();
    let turn = TurnId::new();
    create(&store, &id);
    begin_turn(&store, &id, &turn, None);
    let usage = RequestUsage {
        request_id: "a".repeat(32),
        turn_id: Some(turn.to_string()),
        ..RequestUsage::default()
    };
    store
        .append(&envelope(
            &id,
            4,
            AppEvent::RequestUsageUpdated {
                usage: usage.clone(),
            },
        ))
        .unwrap();
    store
        .append(&envelope(
            &id,
            5,
            AppEvent::ToolProposed {
                turn_id: turn,
                call_id: "search".into(),
                name: "web_search".into(),
                arguments: json!({"query":"synthetic"}),
            },
        ))
        .unwrap();
    store
        .append(&envelope(
            &id,
            6,
            AppEvent::RequestUsageUpdated {
                usage: RequestUsage {
                    state: InvocationState::Completed,
                    response_verified: true,
                    ..usage.clone()
                },
            },
        ))
        .unwrap();
    let snapshot = store.thread_snapshot(&id, None, 100).unwrap();
    assert_eq!(snapshot.items[1].content, "");
    assert_eq!(snapshot.items[1].metadata["request_id"], usage.request_id);
    assert_eq!(snapshot.items[1].metadata["terminal_verified"], true);
}

#[test]
fn terminal_response_verification_survives_authoritative_restart() {
    let directory = tempdir().expect("temp dir");
    let path = directory.path().join("state.sqlite3");
    let id = SessionId::new();
    let turn = TurnId::new();
    {
        let store = SessionStore::open(&path).expect("store");
        create(&store, &id);
        begin_turn(&store, &id, &turn, None);
        store
            .append(&envelope(
                &id,
                4,
                AppEvent::TextDelta {
                    turn_id: turn.clone(),
                    text: "verified answer".into(),
                },
            ))
            .expect("answer");
        store
            .append(&envelope(
                &id,
                5,
                AppEvent::ReasoningDelta {
                    turn_id: turn.clone(),
                    text: "verified reasoning".into(),
                },
            ))
            .expect("reasoning");
        store
            .append(&envelope(
                &id,
                6,
                AppEvent::ResponseVerified {
                    turn_id: turn.clone(),
                },
            ))
            .expect("terminal verification");
        store
            .append(&envelope(
                &id,
                7,
                AppEvent::TurnCompleted {
                    turn_id: turn.clone(),
                },
            ))
            .expect("complete");
    }

    let reopened = SessionStore::open(&path).expect("reopen");
    let snapshot = reopened.thread_snapshot(&id, None, 10).expect("snapshot");
    let response_items = snapshot
        .items
        .iter()
        .filter(|item| {
            matches!(
                item.kind,
                TimelineItemKind::AssistantMessage | TimelineItemKind::Reasoning
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(response_items.len(), 2);
    assert!(response_items.iter().all(|item| {
        item.status == TimelineItemStatus::Completed
            && item
                .metadata
                .get("terminal_verified")
                .and_then(Value::as_bool)
                == Some(true)
    }));
}

#[test]
fn timeline_pages_are_forward_only_and_carry_authoritative_revision() {
    let store = SessionStore::in_memory().expect("store");
    let id = SessionId::new();
    let turn = TurnId::new();
    create(&store, &id);
    begin_turn(&store, &id, &turn, None);
    store
        .append(&envelope(
            &id,
            4,
            AppEvent::ReasoningDelta {
                turn_id: turn.clone(),
                text: "checking".into(),
            },
        ))
        .expect("reasoning");
    let first = store.thread_snapshot(&id, None, 2).expect("first page");
    assert_eq!(first.items.len(), 2);
    assert_eq!(first.next_cursor, Some(2));
    let second = store
        .thread_snapshot(&id, first.next_cursor, 2)
        .expect("second page");
    assert_eq!(second.items.len(), 1);
    assert_eq!(second.items[0].kind, TimelineItemKind::Reasoning);
    assert_eq!(second.thread.revision, first.thread.revision);
    assert_eq!(second.thread.last_timeline_sequence, 3);
}

#[test]
fn thread_catalog_search_and_cursor_are_authoritative() {
    let store = SessionStore::in_memory().expect("store");
    let mut ids = Vec::new();
    for title in ["Alpha review", "Beta review", "Gamma notes"] {
        let id = SessionId::new();
        create(&store, &id);
        store.rename(&id, title).expect("title");
        ids.push(id);
    }
    let first = store
        .thread_catalog(Some("review"), false, None, 1)
        .expect("first page");
    assert_eq!(first.threads.len(), 1);
    let cursor = first.next_cursor.expect("next cursor");
    let second = store
        .thread_catalog(Some("review"), false, Some(&cursor), 1)
        .expect("second page");
    assert_eq!(second.threads.len(), 1);
    assert_ne!(first.threads[0].id, second.threads[0].id);
    assert!(second.next_cursor.is_none());
    assert!(
        store
            .thread_catalog(None, false, Some("not-a-cursor"), 10)
            .is_err()
    );
    assert_eq!(ids.len(), 3);
}

#[test]
fn message_order_ignores_operational_updates_and_paginates_empty_threads() {
    let store = SessionStore::in_memory().expect("store");
    let older = SessionId::new();
    let newer = SessionId::new();
    let empty = SessionId::new();
    let empty_two = SessionId::new();
    for id in [&older, &newer, &empty, &empty_two] {
        create(&store, id);
    }
    let turn = TurnId::new();
    let at = |id: &SessionId, time: &str, event| {
        let mut message = envelope(id, 2, event);
        message.occurred_at = time.parse().expect("timestamp");
        store.append(&message).expect("persist")
    };
    at(
        &older,
        "2026-01-01T01:00:00Z",
        AppEvent::PromptAccepted {
            attachments: Vec::new(),
            turn_id: turn.clone(),
            text: "old".into(),
        },
    );
    at(
        &newer,
        "2026-01-01T02:00:00Z",
        AppEvent::PromptAccepted {
            attachments: Vec::new(),
            turn_id: TurnId::new(),
            text: "new".into(),
        },
    );
    store.rename(&older, "Renamed today").expect("rename");
    let folder = store
        .create_collection("Folder")
        .expect("folder")
        .collection_id
        .expect("id");
    store
        .assign_thread_collection(&older, Some(&folder))
        .expect("assign");
    at(
        &older,
        "2026-01-02T00:00:00Z",
        AppEvent::BackgroundTaskChanged {
            task_id: "preflight".into(),
            state: "completed".into(),
        },
    );
    at(
        &older,
        "2026-01-02T00:00:01Z",
        AppEvent::SessionResumed {
            cwd: PathBuf::from("/tmp"),
            origin: Origin::Test,
            profile: PermissionProfile::Confirm,
        },
    );
    at(
        &older,
        "2026-01-02T00:00:02Z",
        AppEvent::TextDelta {
            turn_id: turn.clone(),
            text: String::new(),
        },
    );
    assert_eq!(
        store.list_threads(false).expect("threads")[0].id,
        newer.to_string()
    );
    assert_eq!(
        store
            .thread_summary(&older)
            .expect("summary")
            .last_message_at
            .as_deref(),
        Some("2026-01-01T01:00:00.000Z")
    );
    let revision = at(
        &older,
        "2026-01-02T03:00:00Z",
        AppEvent::TextDelta {
            turn_id: turn,
            text: "new reply".into(),
        },
    );
    assert_eq!(
        revision.last_message_at.as_deref(),
        Some("2026-01-02T03:00:00.000Z")
    );
    assert_eq!(
        store.list_threads(false).expect("threads")[0].id,
        newer.to_string()
    );
    assert_eq!(
        revision.last_user_message_at.as_deref(),
        Some("2026-01-01T01:00:00.000Z")
    );
    at(
        &older,
        "2026-01-03T01:00:00Z",
        AppEvent::PromptAccepted {
            attachments: Vec::new(),
            turn_id: TurnId::new(),
            text: "follow up".into(),
        },
    );
    assert_eq!(store.list_threads(false).unwrap()[0].id, older.to_string());
    let mut cursor = None;
    let mut ids = Vec::new();
    loop {
        let page = store
            .thread_catalog(None, false, cursor.as_deref(), 1)
            .expect("page");
        ids.extend(page.threads.into_iter().map(|thread| thread.id));
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    let mut empty_ids = vec![empty.to_string(), empty_two.to_string()];
    empty_ids.sort();
    assert_eq!(
        ids,
        [vec![older.to_string(), newer.to_string()], empty_ids].concat()
    );
}

#[test]
fn reopening_marks_nonterminal_turns_and_items_interrupted() {
    let directory = tempdir().expect("temp dir");
    let path = directory.path().join("state.sqlite3");
    let id = SessionId::new();
    let turn = TurnId::new();
    let message_at;
    {
        let store = SessionStore::open(&path).expect("store");
        create(&store, &id);
        begin_turn(&store, &id, &turn, None);
        store
            .append(&envelope(
                &id,
                4,
                AppEvent::TextDelta {
                    turn_id: turn,
                    text: "partial".into(),
                },
            ))
            .expect("partial output");
        message_at = store.thread_summary(&id).expect("summary").last_message_at;
    }
    let reopened = SessionStore::open(&path).expect("reopen");
    let snapshot = reopened.thread_snapshot(&id, None, 10).expect("snapshot");
    assert_eq!(
        snapshot.thread.last_message_at, message_at,
        "recovery must not look like a new message"
    );
    assert_eq!(snapshot.turns[0].status, TurnStatus::Interrupted);
    assert_eq!(snapshot.items[1].status, TimelineItemStatus::Interrupted);
    assert_eq!(snapshot.items[1].content, "partial");
    assert_eq!(
        reopened
            .recovery_status(&id)
            .expect("recovery")
            .interrupted_turns,
        1
    );
}

#[test]
fn profile_preferences_default_to_medium_and_update_together() {
    let store = SessionStore::in_memory().expect("store");
    let defaults = store.profile_preferences().expect("defaults");
    assert_eq!(defaults.model, None);
    assert_eq!(defaults.thinking_level, ThinkingLevel::Medium);
    let updated = store
        .set_profile_preferences("near/model", ThinkingLevel::High)
        .expect("update");
    assert_eq!(updated.model.as_deref(), Some("near/model"));
    assert_eq!(updated.thinking_level, ThinkingLevel::High);

    let id = SessionId::new();
    create(&store, &id);
    let (_, preferences) = store
        .append_all_and_set_profile_preferences(
            &[
                envelope(
                    &id,
                    2,
                    AppEvent::ModelChanged {
                        model: "near/medium-only".into(),
                    },
                ),
                envelope(
                    &id,
                    3,
                    AppEvent::ThinkingLevelChanged {
                        level: ThinkingLevel::Medium,
                    },
                ),
            ],
            "near/medium-only",
            ThinkingLevel::Medium,
        )
        .expect("atomic settings update");
    let thread = store.thread_snapshot(&id, None, 1).expect("thread").thread;
    assert_eq!(thread.selected_model.as_deref(), Some("near/medium-only"));
    assert_eq!(thread.thinking_level, ThinkingLevel::Medium);
    assert_eq!(preferences.model.as_deref(), Some("near/medium-only"));
    assert_eq!(preferences.thinking_level, ThinkingLevel::Medium);
}

#[test]
fn collection_mutations_return_authoritative_state_and_bump_thread_revision() {
    let store = SessionStore::in_memory().expect("store");
    let id = SessionId::new();
    create(&store, &id);
    let created = store.create_collection("Work").expect("collection");
    assert_eq!(created.state.revision, 1);
    let collection_id = created.collection_id.expect("collection ID");
    let assigned = store
        .assign_thread_collection(&id, Some(&collection_id))
        .expect("assign");
    assert_eq!(assigned.state.revision, 2);
    assert_eq!(
        assigned.state.collections[0].thread_ids,
        vec![id.to_string()]
    );
    assert_eq!(store.thread_revision(&id).expect("revision").revision, 2);
    let removed = store
        .delete_collection(&collection_id)
        .expect("delete collection");
    assert_eq!(removed.state.revision, 3);
    assert!(removed.state.collections.is_empty());
    assert_eq!(store.thread_revision(&id).expect("revision").revision, 3);
}

#[test]
fn collection_moves_and_deletes_keep_positions_dense() {
    let store = SessionStore::in_memory().expect("store");
    let first = store
        .create_collection("First")
        .expect("first collection")
        .collection_id
        .expect("first ID");
    let second = store
        .create_collection("Second")
        .expect("second collection")
        .collection_id
        .expect("second ID");
    let third = store
        .create_collection("Third")
        .expect("third collection")
        .collection_id
        .expect("third ID");

    let moved = store.move_collection(&third, 0).expect("move collection");
    assert_eq!(
        moved
            .state
            .collections
            .iter()
            .map(|collection| (&collection.id, collection.position))
            .collect::<Vec<_>>(),
        vec![(&third, 0), (&first, 1), (&second, 2)]
    );
    assert!(store.move_collection(&third, 3).is_err());

    let deleted = store.delete_collection(&first).expect("delete collection");
    assert_eq!(
        deleted
            .state
            .collections
            .iter()
            .map(|collection| (&collection.id, collection.position))
            .collect::<Vec<_>>(),
        vec![(&third, 0), (&second, 1)]
    );
}

#[test]
fn runtime_projection_restores_messages_without_replaying_work() {
    let store = SessionStore::in_memory().expect("store");
    let id = SessionId::new();
    let turn = TurnId::new();
    create(&store, &id);
    begin_turn(&store, &id, &turn, None);
    store
        .append(&envelope(
            &id,
            4,
            AppEvent::TextDelta {
                turn_id: turn.clone(),
                text: "answer".into(),
            },
        ))
        .expect("answer");
    store
        .append(&envelope(&id, 5, AppEvent::TurnCompleted { turn_id: turn }))
        .expect("complete");
    let loaded = store.load(&id).expect("load projection");
    assert!(loaded.iter().any(|event| matches!(
        &event.event,
        AppEvent::PromptAccepted { text, .. } if text == "hello"
    )));
    assert!(loaded.iter().any(|event| matches!(
        &event.event,
        AppEvent::TextDelta { text, .. } if text == "answer"
    )));
    assert!(matches!(
        loaded.last().map(|event| &event.event),
        Some(AppEvent::TurnCompleted { .. })
    ));
}

#[cfg(unix)]
#[test]
fn unix_workspace_paths_round_trip_without_unicode_loss() {
    use std::os::unix::ffi::OsStringExt as _;

    let store = SessionStore::in_memory().expect("store");
    let id = SessionId::new();
    let path = PathBuf::from(OsString::from_vec(b"/tmp/axiom-\xff".to_vec()));
    store
        .append(&envelope(
            &id,
            1,
            AppEvent::SessionCreated {
                cwd: path.clone(),
                origin: Origin::Test,
                profile: PermissionProfile::Confirm,
            },
        ))
        .expect("create");
    assert_eq!(store.thread_summary(&id).expect("thread").cwd, path);
}

#[test]
fn attachments_survive_reopen_and_revision_and_are_deleted_with_their_message() {
    let (_root, store) = routed_store();
    store.activate_account("a").unwrap();
    let id = SessionId::new();
    let turn = TurnId::new();
    create(&store, &id);
    let files = vec![axiom_inference::PromptAttachment::File {
        name: "notes.txt".into(),
        file: axiom_inference::FileContent {
            name: "notes.txt".into(),
            mime_type: "text/plain".into(),
            data: "bG9jYWwgZmlsZSBjb250ZW50cw==".into(),
        },
    }];
    store
        .append_prompt_revision(
            &[envelope(
                &id,
                2,
                AppEvent::PromptAccepted {
                    turn_id: turn.clone(),
                    text: String::new(),
                    attachments: files.clone(),
                },
            )],
            Some("client-id"),
            None,
        )
        .unwrap();
    store
        .append(&envelope(&id, 3, AppEvent::TurnCompleted { turn_id: turn }))
        .unwrap();
    let snapshot = store.thread_snapshot(&id, None, 100).unwrap();
    let user = snapshot
        .items
        .iter()
        .find(|item| item.kind == TimelineItemKind::UserMessage)
        .unwrap();
    assert!(snapshot.thread.last_user_message_at.is_some());
    assert!(
        !serde_json::to_string(&snapshot.items)
            .unwrap()
            .contains("local file contents")
    );
    assert_eq!(store.prompt_attachments(&id, "client-id").unwrap(), files);
    store.activate_account("b").unwrap();
    assert!(store.prompt_attachments(&id, &user.id).is_err());
    store.activate_account("a").unwrap();
    assert!(store.load(&id).unwrap().iter().any(|event| matches!(&event.event, AppEvent::PromptAccepted { attachments, .. } if attachments == &files)));
    let revision = axiom_acp_extension::PromptRevision {
        user_item_id: user.id.clone(),
        expected_revision: store.thread_summary(&id).unwrap().revision,
    };
    let retained = store.prompt_attachments(&id, &user.id).unwrap();
    let new_turn = TurnId::new();
    store
        .append_prompt_revision(
            &[envelope(
                &id,
                4,
                AppEvent::PromptAccepted {
                    turn_id: new_turn.clone(),
                    text: "edited".into(),
                    attachments: retained,
                },
            )],
            Some("edited-id"),
            Some(&revision),
        )
        .unwrap();
    assert!(store.prompt_attachments(&id, &user.id).is_err());
    assert_eq!(store.prompt_attachments(&id, "edited-id").unwrap(), files);
    store
        .append(&envelope(
            &id,
            5,
            AppEvent::TurnCompleted { turn_id: new_turn },
        ))
        .unwrap();
    let count: i64 = store
        .lock()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM prompt_attachments", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(count, 1, "revision cascades away the replaced payload");
}

#[test]
fn version_one_store_upgrades_without_replacing_existing_history() {
    let root = tempdir().unwrap();
    let path = root.path().join("state.sqlite3");
    let connection = Connection::open(&path).unwrap();
    connection.execute_batch(SCHEMA_BASELINE).unwrap();
    drop(connection);
    let store = SessionStore::open(&path).unwrap();
    let id = SessionId::new();
    create(&store, &id);
    let connection = store.lock().unwrap();
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, SCHEMA_VERSION);
    drop(connection);
    assert!(!store.load(&id).unwrap().is_empty());
}

#[test]
fn local_history_pages_bound_escaped_bytes_and_paginate_accounting_independently() {
    use axiom_inference::{InvocationState, RequestUsage};
    let store = SessionStore::in_memory().unwrap();
    let id = SessionId::new();
    create(&store, &id);
    let mut sequence = 1;
    // Three legal 4 MiB messages expand beyond the transport's 64 MiB ceiling.
    let text = "\u{0001}".repeat(4 * 1024 * 1024);
    for _ in 0..3 {
        let turn_id = TurnId::new();
        for event in [
            AppEvent::PromptAccepted {
                turn_id: turn_id.clone(),
                text: text.clone(),
                attachments: vec![],
            },
            AppEvent::TurnStarted {
                turn_id: turn_id.clone(),
            },
            AppEvent::TurnCompleted { turn_id },
        ] {
            sequence += 1;
            store.append(&envelope(&id, sequence, event)).unwrap();
        }
    }
    for index in 0..205 {
        store
            .record_request_usage(
                &id,
                &RequestUsage {
                    request_id: format!("{index:032x}"),
                    model_id: "model".into(),
                    provider_id: "near".into(),
                    state: InvocationState::Failed,
                    ..RequestUsage::default()
                },
            )
            .unwrap();
    }
    let mut cursor = None;
    let mut usage_cursor = None;
    let mut items = vec![];
    let mut usage_ids = vec![];
    let mut page_count = 0;
    loop {
        let page = store
            .thread_page(&id, cursor, usage_cursor.as_deref(), 250)
            .unwrap();
        assert!(serde_json::to_vec(&page).unwrap().len() < 33 * 1024 * 1024);
        assert!(page.turns.is_empty());
        let more = page.next_cursor.is_some() || page.next_request_usage_cursor.is_some();
        cursor = page
            .next_cursor
            .or(Some(page.thread.last_timeline_sequence));
        usage_cursor = page
            .next_request_usage_cursor
            .or_else(|| page.request_usage.last().map(|r| r.request_id.clone()))
            .or(usage_cursor);
        items.extend(page.items);
        usage_ids.extend(page.request_usage.into_iter().map(|r| r.request_id));
        page_count += 1;
        assert!(page_count <= 4);
        if !more {
            break;
        }
    }
    assert_eq!(page_count, 3);
    assert_eq!(items.iter().filter(|item| item.content == text).count(), 3);
    assert_eq!(usage_ids.len(), 205);
    assert!(usage_ids.windows(2).all(|pair| pair[0] < pair[1]));
    // Internal recovery consumers retain their complete accounting snapshot.
    assert_eq!(
        store
            .thread_snapshot(&id, None, 1)
            .unwrap()
            .request_usage
            .len(),
        205
    );
}
