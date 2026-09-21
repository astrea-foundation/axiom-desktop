//! Interrupted-store recovery, loading and export.

use std::{
    collections::{BTreeSet, HashMap},
    path::PathBuf,
};

use chrono::Utc;
use rusqlite::{Connection, params};
use serde_json::{Value, json};

use crate::{
    AxiomError, Result,
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
        let transaction = connection.transaction().map_err(storage_error)?;
        let ids = {
            let mut statement = transaction
                .prepare("SELECT DISTINCT thread_id FROM turns WHERE status='running'")
                .map_err(storage_error)?;
            statement
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(storage_error)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(storage_error)?
        };
        transaction.execute("UPDATE request_usage SET record=json_set(record, '$.state','failed', '$.responseVerified',json('false'), '$.errorCode','CLIENT_INTERRUPTED')
            WHERE json_extract(record,'$.state')='running'", []).map_err(storage_error)?;
        if ids.is_empty() {
            transaction.commit().map_err(storage_error)?;
            return Ok(());
        }
        transaction
            .execute(
                "UPDATE turns SET status='interrupted', completed_at=?1,
                    error=COALESCE(error, 'The previous process ended during this turn')
                 WHERE status='running'",
                [&now],
            )
            .map_err(storage_error)?;
        transaction
            .execute(
                "UPDATE timeline_items SET status='interrupted', updated_at=?1
                 WHERE status IN ('pending','in_progress')
                   AND EXISTS(
                     SELECT 1 FROM turns
                     WHERE turns.thread_id=timeline_items.thread_id
                       AND turns.id=timeline_items.turn_id
                       AND turns.status='interrupted'
                   )",
                [&now],
            )
            .map_err(storage_error)?;
        for id in ids {
            transaction
                .execute(
                    "UPDATE threads SET lifecycle='ready', revision=revision+1, updated_at=?2
                     WHERE id=?1",
                    params![id, now],
                )
                .map_err(storage_error)?;
        }
        transaction.commit().map_err(storage_error)
    }
}

impl SessionStore {
    pub fn load(&self, id: &SessionId) -> Result<Vec<EventEnvelope>> {
        let outcome = self.load_recovering(id)?;
        if let Some(warning) = outcome.warnings.first() {
            return Err(AxiomError::Storage(warning.clone()));
        }
        Ok(outcome.events)
    }

    pub fn load_recovering(&self, id: &SessionId) -> Result<LoadOutcome> {
        let thread = self.thread_summary(id)?;
        let connection = self.lock()?;
        let turns = query_turns(&connection, id)?;
        let mut statement = connection
            .prepare(
                "SELECT id, thread_id, turn_id, sequence, kind, status, client_item_id,
                        external_id, content, metadata, created_at, updated_at
                 FROM timeline_items WHERE thread_id=?1 ORDER BY sequence ASC LIMIT ?2",
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
            items.pop();
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
        let thread = self.thread_summary(id)?;
        let connection = self.lock()?;
        let turns = query_turns(&connection, id)?;
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
            }
            if item.kind == TimelineItemKind::ToolCall && !options.include_tool_output {
                item.content = "[omitted by export policy]".into();
            }
        }
        drop(statement);
        drop(connection);
        let mut value = json!({
            "thread": thread,
            "turns": turns,
            "timeline_items": items,
            "recovery": self.recovery_status(id)?,
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
