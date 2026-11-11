//! Interrupted-store recovery, loading and export.

use std::{
    collections::{BTreeSet, HashMap},
    path::PathBuf,
};

use chrono::Utc;
use rusqlite::{Connection, params};
use serde_json::{Value, json};

use crate::{
    Result,
    app::{EventEnvelope, SessionId},
    audit::redact_value,
};

use super::codec::storage_error;
use super::projection::project_runtime_events;
use super::queries::{
    StoredTimelineItem, ensure_thread_exists_connection, query_prompt_attachments, query_turns,
};
use super::records::{
    ExportOptions, InterruptedTool, LoadOutcome, RecoveryStatus, TimelineItemKind,
};
use super::{MAX_TIMELINE_ITEMS_PER_LOAD, SessionStore};

impl SessionStore {
    pub(super) fn interrupt_incomplete_turns(&self) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        let mut connection = self.lock()?;
        let path = connection.guard.path.clone();
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let candidates = {
            let mut statement = transaction
                .prepare("SELECT DISTINCT thread_id FROM turns WHERE status='running'")
                .map_err(storage_error)?;
            statement
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(storage_error)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(storage_error)?
        };
        let mut ids = Vec::new();
        // Keep recovery locks through commit. Another opener or executor cannot
        // race recovery, and a live owner's OS lock is never stolen on timeout.
        let mut recovery_locks = Vec::new();
        for id in candidates {
            if let Some(path) = &path {
                let Some(lease) = super::execution::try_execution_lock(path, &id)? else {
                    continue;
                };
                recovery_locks.push(lease);
            }
            ids.push(id);
        }
        if ids.is_empty() {
            transaction.commit().map_err(storage_error)?;
            return Ok(());
        }
        for id in ids {
            let abandoned = {
                let mut statement = transaction
                    .prepare("SELECT id FROM turns WHERE thread_id=?1 AND status='running'")
                    .map_err(storage_error)?;
                statement
                    .query_map([&id], |row| row.get::<_, String>(0))
                    .map_err(storage_error)?
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(storage_error)?
            };
            for turn in abandoned {
                // Legacy histories have no canonical marker yet: retain their
                // compatible projection until their next atomic write.
                if super::model_history::exists(
                    &transaction,
                    &id.parse().map_err(|_| {
                        crate::AxiomError::Storage("invalid saved thread ID".into())
                    })?,
                )? {
                    let turn_id = turn
                        .parse()
                        .map_err(|_| crate::AxiomError::Storage("invalid saved turn ID".into()))?;
                    super::model_history::append(
                        &transaction,
                        &EventEnvelope {
                            schema_version: 1,
                            sequence: 0,
                            turn_id: Some(turn_id),
                            occurred_at: Utc::now(),
                            correlation_id: crate::app::CorrelationId::new(),
                            origin: crate::app::Origin::Test,
                            session_id: id.parse().map_err(|_| {
                                crate::AxiomError::Storage("invalid saved thread ID".into())
                            })?,
                            event: crate::app::AppEvent::ErrorRaised {
                                turn_id: Some(turn.parse().map_err(|_| {
                                    crate::AxiomError::Storage("invalid saved turn ID".into())
                                })?),
                                message: "The previous process ended during this turn".into(),
                            },
                        },
                        Some(&turn),
                    )?;
                }
            }
            transaction.execute("UPDATE request_usage SET record=json_set(record, '$.state','failed', '$.responseVerified',json('false'), '$.errorCode','CLIENT_INTERRUPTED') WHERE thread_id=?1 AND json_extract(record,'$.state')='running'", [&id]).map_err(storage_error)?;
            transaction.execute("UPDATE turns SET status='interrupted', completed_at=?2, error=COALESCE(error, 'The previous process ended during this turn') WHERE thread_id=?1 AND status='running'", params![id, now]).map_err(storage_error)?;
            transaction.execute("UPDATE timeline_items SET status='interrupted', updated_at=?2 WHERE thread_id=?1 AND status IN ('pending','in_progress') AND EXISTS(SELECT 1 FROM turns WHERE turns.thread_id=timeline_items.thread_id AND turns.id=timeline_items.turn_id AND turns.status='interrupted')", params![id, now]).map_err(storage_error)?;
            transaction.execute("UPDATE threads SET lifecycle='ready', revision=revision+1, updated_at=?2 WHERE id=?1", params![id, now]).map_err(storage_error)?;
        }
        transaction.commit().map_err(storage_error)
    }
}

impl SessionStore {
    pub fn load(&self, id: &SessionId) -> Result<Vec<EventEnvelope>> {
        let outcome = self.load_recovering(id)?;
        for warning in &outcome.warnings {
            tracing::warn!(%id, %warning, "saved conversation recovered with partial context");
        }
        Ok(outcome.events)
    }

    pub fn load_recovering(&self, id: &SessionId) -> Result<LoadOutcome> {
        let thread = self.thread_summary(id)?;
        let connection = self.lock()?;
        if let Some(events) = super::model_history::load(&connection, id)? {
            return Ok(LoadOutcome {
                events,
                warnings: Vec::new(),
            });
        }
        let turns = query_turns(&connection, id)?;
        let mut statement = connection
            .prepare(
                "SELECT id, thread_id, turn_id, sequence, kind, status, client_item_id,
                        external_id, content, metadata, created_at, updated_at
                 FROM (SELECT * FROM timeline_items WHERE thread_id=?1 ORDER BY sequence DESC LIMIT ?2)
                 ORDER BY sequence ASC",
            )
            .map_err(storage_error)?;
        let stored = statement
            .query_map(
                params![
                    id.to_string(),
                    i64::try_from(MAX_TIMELINE_ITEMS_PER_LOAD + 1).unwrap_or(i64::MAX)
                ],
                StoredTimelineItem::from_row,
            )
            .map_err(storage_error)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(storage_error)?;
        let mut items = stored
            .into_iter()
            .map(StoredTimelineItem::decode)
            .collect::<Result<Vec<_>>>()?;
        let mut warnings = Vec::new();
        if items.len() > MAX_TIMELINE_ITEMS_PER_LOAD {
            items.remove(0);
            warnings.push(format!(
                "thread {id} exceeds the {MAX_TIMELINE_ITEMS_PER_LOAD}-item runtime restore limit"
            ));
        }
        let mut attachments = HashMap::new();
        for item in &items {
            if item.kind == TimelineItemKind::UserMessage
                && item.metadata.get("attachments").is_some()
            {
                attachments.insert(
                    item.id.clone(),
                    query_prompt_attachments(&connection, id, &item.id)?,
                );
            }
        }
        drop(statement);
        drop(connection);
        let events = project_runtime_events(id, &thread, &turns, &items, &attachments)?;
        Ok(LoadOutcome { events, warnings })
    }

    pub fn recovery_status(&self, id: &SessionId) -> Result<RecoveryStatus> {
        let connection = self.lock()?;
        ensure_thread_exists_connection(&connection, id)?;
        let interrupted_turns: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM turns WHERE thread_id=?1 AND status='interrupted'",
                [id.to_string()],
                |row| row.get(0),
            )
            .map_err(storage_error)?;
        let mut tools_statement = connection
            .prepare(
                "SELECT external_id, metadata FROM timeline_items
                 WHERE thread_id=?1 AND kind='tool_call' AND status='interrupted'
                 ORDER BY sequence",
            )
            .map_err(storage_error)?;
        let tools = tools_statement
            .query_map([id.to_string()], |row| {
                Ok((row.get::<_, Option<String>>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(storage_error)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(storage_error)?;
        let interrupted_tools = tools
            .into_iter()
            .map(|(call_id, metadata)| {
                let metadata: Value = serde_json::from_str(&metadata).unwrap_or_default();
                InterruptedTool {
                    call_id: call_id.unwrap_or_else(|| "unknown".into()),
                    name: metadata
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("tool")
                        .to_owned(),
                    status: "unknown_after_restart".into(),
                }
            })
            .collect();
        let changed_paths = query_changed_paths(&connection, id)?;
        Ok(RecoveryStatus {
            session_id: id.clone(),
            interrupted_turns: usize::try_from(interrupted_turns).unwrap_or(usize::MAX),
            interrupted_tools,
            changed_paths,
            warnings: Vec::new(),
        })
    }

    pub fn export(&self, id: &SessionId) -> Result<String> {
        self.export_with_options(id, ExportOptions::default())
    }

    pub fn export_with_options(&self, id: &SessionId, options: ExportOptions) -> Result<String> {
        let mut thread = self.thread_summary(id)?;
        let connection = self.lock()?;
        let mut turns = query_turns(&connection, id)?;
        let maximum = options.max_items.max(1);
        let mut statement = connection
            .prepare(
                "SELECT id, thread_id, turn_id, sequence, kind, status, client_item_id,
                        external_id, content, metadata, created_at, updated_at
                 FROM timeline_items WHERE thread_id=?1 ORDER BY sequence ASC LIMIT ?2",
            )
            .map_err(storage_error)?;
        let stored = statement
            .query_map(
                params![id.to_string(), i64::try_from(maximum).unwrap_or(i64::MAX)],
                StoredTimelineItem::from_row,
            )
            .map_err(storage_error)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(storage_error)?;
        let mut items = stored
            .into_iter()
            .map(StoredTimelineItem::decode)
            .collect::<Result<Vec<_>>>()?;
        for item in &mut items {
            if item.kind == TimelineItemKind::UserMessage && !options.include_prompts {
                item.content = "[omitted by export policy]".into();
                item.metadata = json!({});
            }
            if item.kind == TimelineItemKind::ToolCall {
                if !options.include_tool_output {
                    item.content = "[omitted by export policy]".into();
                    // Allowlist structural fields: new output-bearing metadata
                    // must not silently bypass an omission policy.
                    item.metadata = json!({"name": item.metadata.get("name"),
                        "success": item.metadata.get("success")});
                } else if !options.include_prompts
                    && let Some(fields) = item.metadata.as_object_mut()
                {
                    fields.remove("arguments");
                }
            }
            // Summaries, questions, plans, progress and notices can quote both
            // prompts and tool results. They have no separable safe payload.
            if (!options.include_prompts || !options.include_tool_output)
                && matches!(item.kind, TimelineItemKind::Notice | TimelineItemKind::Plan)
            {
                item.content = "[omitted by export policy]".into();
                item.metadata = json!({"code": item.metadata.get("code")});
            }
        }
        if !options.include_prompts {
            thread.title = None;
        }
        if !options.include_prompts || !options.include_tool_output {
            for turn in &mut turns {
                turn.error = None;
            }
        }
        drop(statement);
        drop(connection);
        let mut recovery = self.recovery_status(id)?;
        if !options.include_tool_output {
            recovery.changed_paths.clear();
        }
        let mut value = json!({
            "thread": thread,
            "turns": turns,
            "timeline_items": items,
            "recovery": recovery,
        });
        redact_value(&mut value);
        serde_json::to_string_pretty(&value).map_err(Into::into)
    }
}

pub(super) fn query_changed_paths(connection: &Connection, id: &SessionId) -> Result<Vec<PathBuf>> {
    let mut statement = connection
        .prepare(
            "SELECT metadata FROM timeline_items
             WHERE thread_id=?1 AND kind='notice' ORDER BY sequence",
        )
        .map_err(storage_error)?;
    let values = statement
        .query_map([id.to_string()], |row| row.get::<_, String>(0))
        .map_err(storage_error)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(storage_error)?;
    let mut paths = BTreeSet::new();
    for value in values {
        let Ok(value) = serde_json::from_str::<Value>(&value) else {
            continue;
        };
        let details = value.get("details").unwrap_or(&value);
        if let Some(stored) = details.get("paths")
            && let Ok(stored) = serde_json::from_value::<Vec<PathBuf>>(stored.clone())
        {
            paths.extend(stored);
        }
    }
    Ok(paths.into_iter().collect())
}
