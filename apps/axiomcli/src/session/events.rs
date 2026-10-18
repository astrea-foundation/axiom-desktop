//! Atomic runtime-event translation into durable records.

use std::path::Path;

use chrono::Utc;
use rusqlite::{OptionalExtension as _, Transaction, params};
use serde_json::{Value, json};

use crate::{
    AxiomError, Result,
    app::{AppEvent, EventEnvelope, PermissionProfile, SessionId, ThinkingLevel},
};

use super::SessionStore;
use super::codec::{
    checked_i64, encode_metadata, encode_path, origin_name, redact_text, redacted_json,
    storage_error, validate_client_item_id, validate_model,
};
use super::preferences::query_profile_preferences;
use super::records::{
    ProfilePreferences, ThreadLifecycle, ThreadRevision, TimelineItemKind, TimelineItemStatus,
    TurnStatus,
};
use super::timeline::{
    NoticeWrite, TimelineItemWrite, active_turn_id, append_turn_content, ensure_external_item,
    ensure_thread, find_external_item, finish_turn, insert_notice, insert_timeline_item,
    item_metadata, mark_response_verified, persist_question, query_thread_revision,
    set_thread_lifecycle, truncate_for_revision, update_notice_resolution, upsert_timeline_item,
};

impl SessionStore {
    pub fn append(&self, envelope: &EventEnvelope) -> Result<ThreadRevision> {
        self.append_batch(std::slice::from_ref(envelope), None, None, None)
            .map(|(revision, _)| revision)
    }

    /// Atomically translate a runtime command's events into durable records.
    pub fn append_all(&self, envelopes: &[EventEnvelope]) -> Result<ThreadRevision> {
        self.append_batch(envelopes, None, None, None)
            .map(|(revision, _)| revision)
    }

    /// Persist one settings transition and the new-thread preference as one
    /// `SQLite` transaction so a crash cannot leave the thread and profile out
    /// of agreement.
    pub fn append_all_and_set_profile_preferences(
        &self,
        envelopes: &[EventEnvelope],
        model: &str,
        thinking: ThinkingLevel,
    ) -> Result<(ThreadRevision, ProfilePreferences)> {
        let (revision, preferences) =
            self.append_batch(envelopes, None, Some((model, thinking)), None)?;
        Ok((
            revision,
            preferences.expect("a requested profile update always returns its row"),
        ))
    }

    /// Persist a runtime command and bind its user timeline item to the
    /// renderer-generated ID used for optimistic reconciliation.
    pub fn append_all_with_client_item_id(
        &self,
        envelopes: &[EventEnvelope],
        client_item_id: Option<&str>,
    ) -> Result<ThreadRevision> {
        self.append_batch(envelopes, client_item_id, None, None)
            .map(|(revision, _)| revision)
    }

    pub fn append_prompt_revision(
        &self,
        envelopes: &[EventEnvelope],
        client_item_id: Option<&str>,
        revision: Option<&axiom_acp_extension::PromptRevision>,
    ) -> Result<ThreadRevision> {
        self.append_batch(envelopes, client_item_id, None, revision)
            .map(|(revision, _)| revision)
    }

    pub(super) fn append_batch(
        &self,
        envelopes: &[EventEnvelope],
        client_item_id: Option<&str>,
        preferences: Option<(&str, ThinkingLevel)>,
        replacement: Option<&axiom_acp_extension::PromptRevision>,
    ) -> Result<(ThreadRevision, Option<ProfilePreferences>)> {
        let first = envelopes
            .first()
            .ok_or_else(|| AxiomError::Storage("cannot persist an empty runtime update".into()))?;
        if envelopes
            .iter()
            .any(|envelope| envelope.session_id != first.session_id)
        {
            return Err(AxiomError::Storage(
                "one local-state transaction cannot span multiple threads".into(),
            ));
        }
        let client_item_id = client_item_id.map(validate_client_item_id).transpose()?;
        let preferences = preferences
            .map(|(model, thinking)| validate_model(model).map(|model| (model, thinking)))
            .transpose()?;
        if client_item_id.is_some()
            && !envelopes
                .iter()
                .any(|envelope| matches!(envelope.event, AppEvent::PromptAccepted { .. }))
        {
            return Err(AxiomError::Storage(
                "a client item ID is valid only for a prompt submission".into(),
            ));
        }

        let thread_id = first.session_id.to_string();
        let now = envelopes.last().map_or_else(
            || Utc::now().to_rfc3339(),
            |event| event.occurred_at.to_rfc3339(),
        );
        let mut connection = self.lock()?;
        let transaction = connection.transaction().map_err(storage_error)?;
        if let Some(replacement) = replacement {
            if !envelopes
                .iter()
                .any(|event| matches!(event.event, AppEvent::PromptAccepted { .. }))
            {
                return Err(AxiomError::InvalidTransition(
                    "A revision must include its replacement prompt.".into(),
                ));
            }
            truncate_for_revision(
                &transaction,
                &first.session_id,
                &replacement.user_item_id,
                replacement.expected_revision,
            )?;
        }
        let mut client_item_id = client_item_id.as_deref();
        for envelope in envelopes {
            persist_event(&transaction, envelope, client_item_id)?;
            let has_attachments = matches!(&envelope.event, AppEvent::PromptAccepted { attachments, .. } if !attachments.is_empty());
            if has_attachments
                || matches!(&envelope.event,
                AppEvent::PromptAccepted { text, .. }
                | AppEvent::SteeringApplied { text, .. }
                | AppEvent::TextDelta { text, .. }
                | AppEvent::ReasoningDelta { text, .. } if !text.is_empty())
            {
                let message_at = envelope
                    .occurred_at
                    .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
                transaction.execute(
                    "UPDATE threads SET last_message_at=MAX(COALESCE(last_message_at,''), ?2) WHERE id=?1",
                    params![thread_id, message_at],
                ).map_err(storage_error)?;
            }
            if has_attachments
                || matches!(&envelope.event,
                AppEvent::PromptAccepted { text, .. } | AppEvent::SteeringApplied { text, .. } if !text.is_empty())
            {
                transaction.execute(
                    "UPDATE threads SET last_user_message_at=MAX(COALESCE(last_user_message_at,''), ?2) WHERE id=?1",
                    params![thread_id, envelope.occurred_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)],
                ).map_err(storage_error)?;
            }
            if matches!(envelope.event, AppEvent::PromptAccepted { .. }) {
                client_item_id = None;
            }
        }
        let changed = transaction
            .execute(
                "UPDATE threads SET revision=revision+1, updated_at=?2 WHERE id=?1",
                params![thread_id, now],
            )
            .map_err(storage_error)?;
        if changed == 0 {
            return Err(AxiomError::SessionNotFound(first.session_id.to_string()));
        }
        let updated_preferences = if let Some((model, thinking)) = preferences {
            transaction
                .execute(
                    "UPDATE profile_preferences
                     SET selected_model=?1, thinking_level=?2, updated_at=?3 WHERE id=1",
                    params![model, thinking.to_string(), now],
                )
                .map_err(storage_error)?;
            Some(query_profile_preferences(&transaction)?)
        } else {
            None
        };
        let mut revision = query_thread_revision(&transaction, &first.session_id)?;
        // ACP retains its standard turn-scoped message ID, but Axiom clients
        // also receive the exact durable segment ID. A tool boundary must not
        // cause later deltas to be appended to the pre-tool assistant row.
        if let Some(envelope) = envelopes.last() {
            let message = match &envelope.event {
                AppEvent::TextDelta { turn_id, .. } => {
                    Some((turn_id, TimelineItemKind::AssistantMessage))
                }
                AppEvent::ReasoningDelta { turn_id, .. } => {
                    Some((turn_id, TimelineItemKind::Reasoning))
                }
                _ => None,
            };
            if let Some((turn_id, kind)) = message {
                revision.timeline_item_id = transaction
                    .query_row(
                        "SELECT id FROM timeline_items WHERE thread_id=?1 AND turn_id=?2
                     AND kind=?3 ORDER BY sequence DESC LIMIT 1",
                        params![thread_id, turn_id.to_string(), kind.to_string()],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(storage_error)?;
            }
        }
        transaction.commit().map_err(storage_error)?;
        Ok((revision, updated_preferences))
    }

    pub fn thread_revision(&self, id: &SessionId) -> Result<ThreadRevision> {
        query_thread_revision(&*self.lock()?, id)
    }
}

pub(super) fn persist_event(
    transaction: &Transaction<'_>,
    envelope: &EventEnvelope,
    client_item_id: Option<&str>,
) -> Result<()> {
    let thread_id = envelope.session_id.to_string();
    let now = envelope.occurred_at.to_rfc3339();
    match &envelope.event {
        AppEvent::SessionCreated {
            cwd,
            origin,
            profile,
        } => {
            let (cwd_encoding, cwd) = encode_path(cwd)?;
            transaction
                .execute(
                    "INSERT INTO threads(
                       id, cwd, cwd_encoding, origin, profile, thinking_level, lifecycle,
                       archived, revision, next_timeline_sequence, created_at, updated_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, 'medium', 'ready', 0, 0, 0, ?6, ?6)",
                    params![
                        thread_id,
                        cwd,
                        cwd_encoding,
                        origin_name(*origin),
                        profile.to_string(),
                        now
                    ],
                )
                .map_err(storage_error)?;
        }
        AppEvent::SessionResumed {
            cwd,
            origin,
            profile,
        } => {
            let (cwd_encoding, cwd) = encode_path(cwd)?;
            ensure_thread(transaction, &envelope.session_id)?;
            transaction
                .execute(
                    "UPDATE threads SET cwd=?2, cwd_encoding=?3, origin=?4, profile=?5,
                       lifecycle='ready' WHERE id=?1",
                    params![
                        thread_id,
                        cwd,
                        cwd_encoding,
                        origin_name(*origin),
                        profile.to_string()
                    ],
                )
                .map_err(storage_error)?;
        }
        AppEvent::SteeringApplied {
            turn_id,
            client_item_id,
            text,
        } => {
            let client_id = validate_client_item_id(client_item_id)?;
            insert_timeline_item(
                transaction,
                &thread_id,
                TimelineItemWrite {
                    turn_id: Some(&turn_id.to_string()),
                    kind: TimelineItemKind::UserMessage,
                    status: TimelineItemStatus::Completed,
                    client_item_id: Some(&client_id),
                    external_id: None,
                    content: &redact_text(text),
                    metadata: json!({"steering": true}),
                    now: &now,
                },
            )?;
        }
        AppEvent::PromptAccepted {
            turn_id,
            text,
            attachments,
        } => {
            let turn_id = turn_id.to_string();
            let user_content = redact_text(text);
            let (user_item_id, _) = insert_timeline_item(
                transaction,
                &thread_id,
                TimelineItemWrite {
                    turn_id: Some(&turn_id),
                    kind: TimelineItemKind::UserMessage,
                    status: TimelineItemStatus::Completed,
                    client_item_id,
                    external_id: None,
                    content: &user_content,
                    metadata: if attachments.is_empty() {
                        json!({})
                    } else {
                        json!({"attachments": attachments.iter().map(axiom_inference::PromptAttachment::summary).collect::<Vec<_>>()})
                    },
                    now: &now,
                },
            )?;
            if !attachments.is_empty() {
                axiom_inference::validate_prompt(text, attachments)
                    .map_err(|error| AxiomError::Storage(error.to_string()))?;
                transaction
                    .execute(
                        "INSERT INTO prompt_attachments(user_item_id,payload) VALUES (?1,?2)",
                        params![user_item_id, serde_json::to_string(attachments)?],
                    )
                    .map_err(storage_error)?;
            }
            let (assistant_item_id, _) = insert_timeline_item(
                transaction,
                &thread_id,
                TimelineItemWrite {
                    turn_id: Some(&turn_id),
                    kind: TimelineItemKind::AssistantMessage,
                    status: TimelineItemStatus::InProgress,
                    client_item_id: None,
                    external_id: None,
                    content: "",
                    metadata: json!({}),
                    now: &now,
                },
            )?;
            transaction
                .execute(
                    "INSERT INTO turns(
                       thread_id, id, status, user_item_id, assistant_item_id, started_at
                     ) VALUES (?1, ?2, 'running', ?3, ?4, ?5)",
                    params![thread_id, turn_id, user_item_id, assistant_item_id, now],
                )
                .map_err(storage_error)?;
            transaction
                .execute(
                    "UPDATE threads SET lifecycle='running' WHERE id=?1",
                    [&thread_id],
                )
                .map_err(storage_error)?;
        }
        AppEvent::TurnStarted { turn_id } => {
            ensure_thread(transaction, &envelope.session_id)?;
            transaction
                .execute(
                    "INSERT INTO turns(thread_id, id, status, started_at)
                     VALUES (?1, ?2, 'running', ?3)
                     ON CONFLICT(thread_id, id) DO UPDATE SET status='running'",
                    params![thread_id, turn_id.to_string(), now],
                )
                .map_err(storage_error)?;
            transaction
                .execute(
                    "UPDATE threads SET lifecycle='running' WHERE id=?1",
                    [&thread_id],
                )
                .map_err(storage_error)?;
        }
        AppEvent::TextDelta { turn_id, text } => {
            append_turn_content(
                transaction,
                &thread_id,
                &turn_id.to_string(),
                TimelineItemKind::AssistantMessage,
                &redact_text(text),
                &now,
            )?;
        }
        AppEvent::ReasoningDelta { turn_id, text } => {
            append_turn_content(
                transaction,
                &thread_id,
                &turn_id.to_string(),
                TimelineItemKind::Reasoning,
                &redact_text(text),
                &now,
            )?;
        }
        // Persist the last complete provider report atomically with its thread
        // revision, without adding transcript rows or advancing message order.
        AppEvent::RequestUsageUpdated { usage } => {
            if !usage.validate_counters() {
                return Err(AxiomError::Storage("invalid request accounting".into()));
            }
            transaction
                .execute(
                    "INSERT INTO request_usage(request_id, thread_id, record) VALUES (?1,?2,?3)
                 ON CONFLICT(request_id) DO UPDATE SET record=excluded.record
                 WHERE request_usage.thread_id=excluded.thread_id",
                    params![usage.request_id, thread_id, serde_json::to_string(usage)?],
                )
                .map_err(storage_error)?;
            if usage.purpose == axiom_inference::InvocationPurpose::Conversation
                && usage.state == axiom_inference::InvocationState::Running
            {
                // PromptAccepted creates this row before the request exists.
                // Bind it on the first request, including tool-only responses
                // that never emit a text delta into the placeholder.
                transaction.execute(
                    "UPDATE timeline_items SET metadata=json_set(metadata, '$.request_id', ?3)
                     WHERE thread_id=?1 AND turn_id=?2 AND kind='assistant_message'
                       AND id=(SELECT assistant_item_id FROM turns WHERE thread_id=?1 AND id=?2)
                       AND status='in_progress' AND content=''
                       AND json_extract(metadata, '$.request_id') IS NULL
                       AND NOT EXISTS(SELECT 1 FROM request_usage
                         WHERE thread_id=?1 AND json_extract(record, '$.turnId')=?2
                           AND json_extract(record, '$.purpose')='conversation' AND request_id<>?3)",
                    params![thread_id, usage.turn_id, usage.request_id],
                ).map_err(storage_error)?;
            }
            if usage.response_verified {
                transaction.execute(
                    "UPDATE timeline_items SET status='completed',
                      metadata=json_set(metadata, '$.terminal_verified', json('true'), '$.finish_reason', ?4), updated_at=?3
                     WHERE thread_id=?1 AND json_extract(metadata, '$.request_id')=?2
                       AND kind IN ('assistant_message','reasoning')",
                    params![thread_id, usage.request_id, now, usage.finish_reason],
                ).map_err(storage_error)?;
            }
        }
        AppEvent::ContextUsageUpdated { usage } => {
            transaction
                .execute(
                    "UPDATE threads SET last_reported_usage=?2 WHERE id=?1",
                    params![thread_id, serde_json::to_string(usage)?],
                )
                .map_err(storage_error)?;
        }
        AppEvent::UsageUpdated {
            input_tokens,
            output_tokens,
        } => {
            if let Some(turn_id) = active_turn_id(transaction, &thread_id)? {
                transaction
                    .execute(
                        "UPDATE turns SET input_tokens=?3, output_tokens=?4
                         WHERE thread_id=?1 AND id=?2",
                        params![
                            thread_id,
                            turn_id,
                            checked_i64(*input_tokens, "input token count")?,
                            checked_i64(*output_tokens, "output token count")?
                        ],
                    )
                    .map_err(storage_error)?;
            }
        }
        AppEvent::ProgressUpdated {
            message,
            completed,
            total,
        } => {
            let turn_id = active_turn_id(transaction, &thread_id)?;
            upsert_timeline_item(
                transaction,
                &thread_id,
                TimelineItemWrite {
                    turn_id: turn_id.as_deref(),
                    kind: TimelineItemKind::Notice,
                    status: TimelineItemStatus::InProgress,
                    client_item_id: None,
                    external_id: Some(&format!(
                        "progress:{}",
                        turn_id.as_deref().unwrap_or("thread")
                    )),
                    content: &redact_text(message),
                    metadata: json!({"code":"progress","completed":completed,"total":total}),
                    now: &now,
                },
            )?;
        }
        AppEvent::TaskListUpdated { items } => {
            let turn_id = active_turn_id(transaction, &thread_id)?;
            upsert_timeline_item(
                transaction,
                &thread_id,
                TimelineItemWrite {
                    turn_id: turn_id.as_deref(),
                    kind: TimelineItemKind::Plan,
                    status: TimelineItemStatus::InProgress,
                    client_item_id: None,
                    external_id: Some(&format!(
                        "task-list:{}",
                        turn_id.as_deref().unwrap_or("thread")
                    )),
                    content: "",
                    metadata: json!({"code":"task_list","items":redacted_json(items)?}),
                    now: &now,
                },
            )?;
        }
        AppEvent::ToolProposed {
            turn_id,
            call_id,
            name,
            arguments,
        } => {
            upsert_timeline_item(
                transaction,
                &thread_id,
                TimelineItemWrite {
                    turn_id: Some(&turn_id.to_string()),
                    kind: TimelineItemKind::ToolCall,
                    status: TimelineItemStatus::Pending,
                    client_item_id: None,
                    external_id: Some(call_id),
                    content: "",
                    metadata: json!({
                        "name": name,
                        "arguments": redacted_json(arguments)?,
                        "output_truncated": false,
                        "success": Value::Null,
                        "diff": Value::Null,
                        "files": []
                    }),
                    now: &now,
                },
            )?;
        }
        AppEvent::ToolStarted { call_id, name } => {
            let turn_id = active_turn_id(transaction, &thread_id)?;
            let item_id = ensure_external_item(
                transaction,
                &thread_id,
                turn_id.as_deref(),
                TimelineItemKind::ToolCall,
                call_id,
                json!({
                    "name": name,
                    "arguments": {},
                    "output_truncated": false,
                    "success": Value::Null,
                    "diff": Value::Null,
                    "files": []
                }),
                &now,
            )?;
            transaction
                .execute(
                    "UPDATE timeline_items SET status='in_progress', updated_at=?2 WHERE id=?1",
                    params![item_id, now],
                )
                .map_err(storage_error)?;
        }
        AppEvent::ToolOutput {
            call_id,
            content,
            truncated,
        } => {
            let turn_id = active_turn_id(transaction, &thread_id)?;
            let item_id = ensure_external_item(
                transaction,
                &thread_id,
                turn_id.as_deref(),
                TimelineItemKind::ToolCall,
                call_id,
                json!({
                    "name": "tool",
                    "arguments": {},
                    "output_truncated": false,
                    "success": Value::Null,
                    "diff": Value::Null,
                    "files": []
                }),
                &now,
            )?;
            let mut metadata = item_metadata(transaction, &item_id)?;
            metadata["output_truncated"] = Value::Bool(*truncated);
            transaction
                .execute(
                    "UPDATE timeline_items SET content=content || ?2, metadata=?3,
                       status='in_progress', updated_at=?4 WHERE id=?1",
                    params![
                        item_id,
                        redact_text(content),
                        encode_metadata(metadata)?,
                        now
                    ],
                )
                .map_err(storage_error)?;
        }
        AppEvent::ToolCompleted { call_id, success } => {
            if let Some(item_id) =
                find_external_item(transaction, &thread_id, TimelineItemKind::ToolCall, call_id)?
            {
                let mut metadata = item_metadata(transaction, &item_id)?;
                metadata["success"] = Value::Bool(*success);
                let status = if *success { "completed" } else { "failed" };
                transaction
                    .execute(
                        "UPDATE timeline_items SET status=?2, metadata=?3, updated_at=?4 WHERE id=?1",
                        params![item_id, status, encode_metadata(metadata)?, now],
                    )
                    .map_err(storage_error)?;
            }
        }
        AppEvent::DiffAvailable {
            call_id,
            diff,
            truncated,
            files,
        } => {
            let turn_id = active_turn_id(transaction, &thread_id)?;
            let item_id = ensure_external_item(
                transaction,
                &thread_id,
                turn_id.as_deref(),
                TimelineItemKind::ToolCall,
                call_id,
                json!({"name":"tool","arguments":{},"output_truncated":false,"success":Value::Null}),
                &now,
            )?;
            let mut metadata = item_metadata(transaction, &item_id)?;
            metadata["diff"] = Value::String(redact_text(diff));
            metadata["diff_truncated"] = Value::Bool(*truncated);
            metadata["files"] = redacted_json(files)?;
            transaction
                .execute(
                    "UPDATE timeline_items SET metadata=?2, updated_at=?3 WHERE id=?1",
                    params![item_id, encode_metadata(metadata)?, now],
                )
                .map_err(storage_error)?;
        }
        AppEvent::PermissionRequired {
            request_id,
            explanation,
            effect,
            choices,
        } => {
            let turn_id = active_turn_id(transaction, &thread_id)?;
            upsert_timeline_item(
                transaction,
                &thread_id,
                TimelineItemWrite {
                    turn_id: turn_id.as_deref(),
                    kind: TimelineItemKind::Notice,
                    status: TimelineItemStatus::Pending,
                    client_item_id: None,
                    external_id: Some(request_id),
                    content: &redact_text(explanation),
                    metadata: json!({"code":"permission","effect":redacted_json(effect)?,"choices":choices,"allowed":Value::Null}),
                    now: &now,
                },
            )?;
            set_thread_lifecycle(transaction, &thread_id, ThreadLifecycle::WaitingForApproval)?;
        }
        AppEvent::PermissionResolved {
            request_id,
            allowed,
            choice,
        } => {
            update_notice_resolution(
                transaction,
                &thread_id,
                request_id,
                json!({"allowed":allowed,"choice":choice}),
                if *allowed {
                    TimelineItemStatus::Completed
                } else {
                    TimelineItemStatus::Cancelled
                },
                &now,
            )?;
            set_thread_lifecycle(transaction, &thread_id, ThreadLifecycle::Running)?;
        }
        AppEvent::QuestionAsked {
            question_id,
            prompt,
            options,
        } => {
            persist_question(
                transaction,
                &thread_id,
                question_id,
                &redact_text(prompt),
                json!({"code":"question","options":options}),
                &now,
            )?;
        }
        AppEvent::QuestionAnswered {
            question_id,
            answers,
        } => {
            update_notice_resolution(
                transaction,
                &thread_id,
                question_id,
                json!({"answers":redacted_json(answers)?}),
                TimelineItemStatus::Completed,
                &now,
            )?;
            set_thread_lifecycle(transaction, &thread_id, ThreadLifecycle::Running)?;
        }
        AppEvent::QuestionsAsked { request } => {
            persist_question(
                transaction,
                &thread_id,
                &request.request_id,
                "",
                json!({"code":"questions","request":redacted_json(request)?}),
                &now,
            )?;
        }
        AppEvent::QuestionsAnswered {
            request_id,
            answers,
        } => {
            update_notice_resolution(
                transaction,
                &thread_id,
                request_id,
                json!({"answers":redacted_json(answers)?}),
                TimelineItemStatus::Completed,
                &now,
            )?;
            set_thread_lifecycle(transaction, &thread_id, ThreadLifecycle::Running)?;
        }
        AppEvent::QuestionsFailed { request_id, reason } => {
            update_notice_resolution(
                transaction,
                &thread_id,
                request_id,
                json!({"reason":redact_text(reason)}),
                TimelineItemStatus::Failed,
                &now,
            )?;
            set_thread_lifecycle(transaction, &thread_id, ThreadLifecycle::Running)?;
        }
        AppEvent::PlanProposed {
            plan_id,
            revision,
            markdown,
        } => {
            let turn_id = active_turn_id(transaction, &thread_id)?;
            upsert_timeline_item(
                transaction,
                &thread_id,
                TimelineItemWrite {
                    turn_id: turn_id.as_deref(),
                    kind: TimelineItemKind::Plan,
                    status: TimelineItemStatus::Pending,
                    client_item_id: None,
                    external_id: Some(plan_id),
                    content: &redact_text(markdown),
                    metadata: json!({"code":"plan","revision":revision,"decision":Value::Null}),
                    now: &now,
                },
            )?;
        }
        AppEvent::PlanReviewed {
            plan_id,
            revision,
            decision,
        } => {
            update_notice_resolution(
                transaction,
                &thread_id,
                plan_id,
                json!({"revision":revision,"decision":decision}),
                TimelineItemStatus::Completed,
                &now,
            )?;
        }
        AppEvent::BackgroundTaskChanged { task_id, state } => {
            let turn_id = active_turn_id(transaction, &thread_id)?;
            let status = if state == "completed" {
                TimelineItemStatus::Completed
            } else if state == "cancelled" {
                TimelineItemStatus::Cancelled
            } else if state.starts_with("failed:") {
                TimelineItemStatus::Failed
            } else {
                TimelineItemStatus::InProgress
            };
            upsert_timeline_item(
                transaction,
                &thread_id,
                TimelineItemWrite {
                    turn_id: turn_id.as_deref(),
                    kind: TimelineItemKind::ToolCall,
                    status,
                    client_item_id: None,
                    external_id: Some(task_id),
                    content: "",
                    metadata: json!({"name":"background_process","state":redact_text(state),"arguments":{},"success":status == TimelineItemStatus::Completed}),
                    now: &now,
                },
            )?;
        }
        AppEvent::WorkspaceChanged { paths } => {
            insert_notice(
                transaction,
                &thread_id,
                &NoticeWrite {
                    turn_id: active_turn_id(transaction, &thread_id)?.as_deref(),
                    status: TimelineItemStatus::Completed,
                    external_id: None,
                    code: "workspace_changed",
                    content: "",
                    details: json!({"paths":redacted_json(paths)?}),
                    now: &now,
                },
            )?;
        }
        AppEvent::DesktopAgentConfigured { settings } => {
            let (encoding, cwd) = encode_path(Path::new(&settings.working_directory))?;
            let changed = transaction.execute(
                "UPDATE threads SET cwd=?2, profile=?3, desktop_agent_settings=?4, cwd_encoding=?6 WHERE id=?1 AND lifecycle='ready' AND COALESCE(json_extract(desktop_agent_settings, '$.revision'), 0)=?5",
                params![thread_id, cwd, PermissionProfile::for_desktop_agent(settings).to_string(), serde_json::to_string(settings)?, i64::try_from(settings.revision.saturating_sub(1)).map_err(|_| AxiomError::Storage("Agent revision overflow".into()))?, encoding],
            ).map_err(storage_error)?;
            if changed != 1 {
                return Err(AxiomError::InvalidTransition(
                    "Agent settings changed or the thread is busy".into(),
                ));
            }
        }
        AppEvent::PermissionProfileChanged { profile } => {
            ensure_thread(transaction, &envelope.session_id)?;
            transaction
                .execute(
                    "UPDATE threads SET profile=?2 WHERE id=?1",
                    params![thread_id, profile.to_string()],
                )
                .map_err(storage_error)?;
        }
        AppEvent::ModelChanged { model } => {
            validate_model(model)?;
            transaction
                .execute(
                    "UPDATE threads SET selected_model=?2 WHERE id=?1",
                    params![thread_id, model],
                )
                .map_err(storage_error)?;
        }
        AppEvent::ThinkingLevelChanged { level } => {
            transaction
                .execute(
                    "UPDATE threads SET thinking_level=?2 WHERE id=?1",
                    params![thread_id, level.to_string()],
                )
                .map_err(storage_error)?;
        }
        AppEvent::ContextCompacted {
            summary,
            messages_before,
        } => {
            insert_notice(
                transaction,
                &thread_id,
                &NoticeWrite {
                    turn_id: None,
                    status: TimelineItemStatus::Completed,
                    external_id: None,
                    code: "context_compacted",
                    content: &redact_text(summary),
                    details: json!({"messages_before":messages_before}),
                    now: &now,
                },
            )?;
        }
        AppEvent::ProviderStatusChanged { connected, detail } => {
            upsert_timeline_item(
                transaction,
                &thread_id,
                TimelineItemWrite {
                    turn_id: None,
                    kind: TimelineItemKind::Notice,
                    status: if *connected {
                        TimelineItemStatus::Completed
                    } else {
                        TimelineItemStatus::Failed
                    },
                    client_item_id: None,
                    external_id: Some("provider-status"),
                    content: &redact_text(detail),
                    metadata: json!({"code":"provider_status","connected":connected}),
                    now: &now,
                },
            )?;
        }
        AppEvent::SecurityStatusChanged { state } => {
            upsert_timeline_item(
                transaction,
                &thread_id,
                TimelineItemWrite {
                    turn_id: None,
                    kind: TimelineItemKind::Notice,
                    status: TimelineItemStatus::Completed,
                    client_item_id: None,
                    external_id: Some("security-status"),
                    content: "",
                    metadata: json!({"code":"security_status","state":state}),
                    now: &now,
                },
            )?;
        }
        AppEvent::ResponseVerified { turn_id } => {
            mark_response_verified(transaction, &thread_id, &turn_id.to_string(), &now)?;
        }
        AppEvent::WarningRaised { message } => {
            insert_notice(
                transaction,
                &thread_id,
                &NoticeWrite {
                    turn_id: active_turn_id(transaction, &thread_id)?.as_deref(),
                    status: TimelineItemStatus::Completed,
                    external_id: None,
                    code: "warning",
                    content: &redact_text(message),
                    details: json!({}),
                    now: &now,
                },
            )?;
        }
        AppEvent::TurnCompleted { turn_id } => {
            finish_turn(
                transaction,
                &thread_id,
                &turn_id.to_string(),
                TurnStatus::Completed,
                None,
                &now,
            )?;
        }
        AppEvent::TurnCancelled { turn_id } => {
            finish_turn(
                transaction,
                &thread_id,
                &turn_id.to_string(),
                TurnStatus::Cancelled,
                None,
                &now,
            )?;
        }
        AppEvent::ErrorRaised { turn_id, message } => {
            if let Some(turn_id) = turn_id {
                finish_turn(
                    transaction,
                    &thread_id,
                    &turn_id.to_string(),
                    TurnStatus::Failed,
                    Some(&redact_text(message)),
                    &now,
                )?;
            }
            insert_notice(
                transaction,
                &thread_id,
                &NoticeWrite {
                    turn_id: turn_id.as_ref().map(ToString::to_string).as_deref(),
                    status: TimelineItemStatus::Failed,
                    external_id: None,
                    code: "error",
                    content: &redact_text(message),
                    details: json!({"terminal":turn_id.is_some()}),
                    now: &now,
                },
            )?;
        }
        AppEvent::SessionClosed => {
            set_thread_lifecycle(transaction, &thread_id, ThreadLifecycle::Closed)?;
        }
    }
    Ok(())
}
