//! Timeline writes, verification commits and turn transitions.

use chrono::Utc;
use rusqlite::{Connection, OptionalExtension as _, Transaction, params};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{AxiomError, Result, app::SessionId};

use super::codec::{checked_u64, checked_u64_sql, encode_metadata, storage_error};
use super::records::{
    ThreadLifecycle, ThreadRevision, TimelineItemKind, TimelineItemStatus, TurnStatus,
};

pub(super) fn ensure_thread(transaction: &Transaction<'_>, id: &SessionId) -> Result<()> {
    let exists: bool = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM threads WHERE id=?1)",
            [id.to_string()],
            |row| row.get(0),
        )
        .map_err(storage_error)?;
    if exists {
        Ok(())
    } else {
        Err(AxiomError::SessionNotFound(id.to_string()))
    }
}

pub(super) fn query_thread_revision(
    connection: &Connection,
    id: &SessionId,
) -> Result<ThreadRevision> {
    connection
        .query_row(
            "SELECT revision, next_timeline_sequence, last_message_at, last_user_message_at FROM threads WHERE id=?1",
            [id.to_string()],
            |row| {
                Ok(ThreadRevision {
                    revision: checked_u64_sql(row.get::<_, i64>(0)?)?,
                    last_timeline_sequence: checked_u64_sql(row.get::<_, i64>(1)?)?,
                    last_message_at: row.get(2)?,
                    last_user_message_at: row.get(3)?,
                    timeline_item_id: None,
                })
            },
        )
        .map_err(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => AxiomError::SessionNotFound(id.to_string()),
            other => storage_error(other),
        })
}

pub(super) struct TimelineItemWrite<'a> {
    pub(super) turn_id: Option<&'a str>,
    pub(super) kind: TimelineItemKind,
    pub(super) status: TimelineItemStatus,
    pub(super) client_item_id: Option<&'a str>,
    pub(super) external_id: Option<&'a str>,
    pub(super) content: &'a str,
    pub(super) metadata: Value,
    pub(super) now: &'a str,
}

pub(super) fn insert_timeline_item(
    transaction: &Transaction<'_>,
    thread_id: &str,
    item: TimelineItemWrite<'_>,
) -> Result<(String, u64)> {
    let changed = transaction
        .execute(
            "UPDATE threads SET next_timeline_sequence=next_timeline_sequence+1 WHERE id=?1",
            [thread_id],
        )
        .map_err(storage_error)?;
    if changed == 0 {
        return Err(AxiomError::SessionNotFound(thread_id.into()));
    }
    let sequence: i64 = transaction
        .query_row(
            "SELECT next_timeline_sequence FROM threads WHERE id=?1",
            [thread_id],
            |row| row.get(0),
        )
        .map_err(storage_error)?;
    let id = Uuid::new_v4().to_string();
    transaction
        .execute(
            "INSERT INTO timeline_items(
               id, thread_id, turn_id, sequence, kind, status, client_item_id,
               external_id, content, metadata, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11)",
            params![
                id,
                thread_id,
                item.turn_id,
                sequence,
                item.kind.to_string(),
                item.status.to_string(),
                item.client_item_id,
                item.external_id,
                item.content,
                encode_metadata(item.metadata)?,
                item.now
            ],
        )
        .map_err(storage_error)?;
    Ok((id, checked_u64(sequence, "timeline sequence")?))
}

pub(super) fn upsert_timeline_item(
    transaction: &Transaction<'_>,
    thread_id: &str,
    item: TimelineItemWrite<'_>,
) -> Result<String> {
    if let Some(external_id) = item.external_id
        && let Some(id) = find_external_item(transaction, thread_id, item.kind, external_id)?
    {
        transaction
            .execute(
                "UPDATE timeline_items SET turn_id=COALESCE(turn_id, ?2), status=?3,
                   content=?4, metadata=?5, updated_at=?6 WHERE id=?1",
                params![
                    id,
                    item.turn_id,
                    item.status.to_string(),
                    item.content,
                    encode_metadata(item.metadata)?,
                    item.now
                ],
            )
            .map_err(storage_error)?;
        return Ok(id);
    }
    insert_timeline_item(transaction, thread_id, item).map(|(id, _)| id)
}

pub(super) fn append_turn_content(
    transaction: &Transaction<'_>,
    thread_id: &str,
    turn_id: &str,
    kind: TimelineItemKind,
    content: &str,
    now: &str,
) -> Result<()> {
    let request_id: Option<String> = transaction
        .query_row(
            "SELECT request_id FROM request_usage WHERE thread_id=?1
               AND json_extract(record, '$.turnId')=?2
               AND json_extract(record, '$.purpose')='conversation'
               AND json_extract(record, '$.state')='running'
             ORDER BY rowid DESC LIMIT 1",
            params![thread_id, turn_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage_error)?;
    let existing = transaction
        .query_row(
            "SELECT id FROM timeline_items
             WHERE thread_id=?1 AND turn_id=?2 AND kind=?3
               AND COALESCE(json_extract(metadata, '$.terminal_verified'), 0)=0
               AND status IN ('pending','in_progress')
               AND json_extract(metadata, '$.request_id') IS ?4
               AND sequence > COALESCE((SELECT MAX(sequence) FROM timeline_items
                 WHERE thread_id=?1 AND turn_id=?2 AND kind IN ('tool_call', 'user_message')), 0)
             ORDER BY sequence DESC LIMIT 1",
            params![thread_id, turn_id, kind.to_string(), request_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage_error)?;
    let id = if let Some(id) = existing {
        id
    } else {
        insert_timeline_item(
            transaction,
            thread_id,
            TimelineItemWrite {
                turn_id: Some(turn_id),
                kind,
                status: TimelineItemStatus::InProgress,
                client_item_id: None,
                external_id: None,
                content: "",
                metadata: json!({"request_id": request_id}),
                now,
            },
        )?
        .0
    };
    transaction
        .execute(
            "UPDATE timeline_items SET content=content || ?2, status='in_progress',
               metadata=json_remove(metadata, '$.terminal_verified', '$.terminalVerified'),
               updated_at=?3 WHERE id=?1",
            params![id, content, now],
        )
        .map_err(storage_error)?;
    Ok(())
}

pub(super) fn active_turn_id(
    transaction: &Transaction<'_>,
    thread_id: &str,
) -> Result<Option<String>> {
    transaction
        .query_row(
            "SELECT id FROM turns WHERE thread_id=?1 AND status='running'
             ORDER BY started_at DESC, id DESC LIMIT 1",
            [thread_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage_error)
}

pub(super) fn find_external_item(
    transaction: &Transaction<'_>,
    thread_id: &str,
    kind: TimelineItemKind,
    external_id: &str,
) -> Result<Option<String>> {
    transaction
        .query_row(
            "SELECT id FROM timeline_items
             WHERE thread_id=?1 AND kind=?2 AND external_id=?3 ORDER BY sequence DESC LIMIT 1",
            params![thread_id, kind.to_string(), external_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage_error)
}

pub(super) fn find_any_external_item(
    transaction: &Transaction<'_>,
    thread_id: &str,
    external_id: &str,
) -> Result<Option<String>> {
    transaction
        .query_row(
            "SELECT id FROM timeline_items
             WHERE thread_id=?1 AND external_id=?2 ORDER BY sequence DESC LIMIT 1",
            params![thread_id, external_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage_error)
}

pub(super) fn ensure_external_item(
    transaction: &Transaction<'_>,
    thread_id: &str,
    turn_id: Option<&str>,
    kind: TimelineItemKind,
    external_id: &str,
    metadata: Value,
    now: &str,
) -> Result<String> {
    if let Some(id) = find_external_item(transaction, thread_id, kind, external_id)? {
        return Ok(id);
    }
    insert_timeline_item(
        transaction,
        thread_id,
        TimelineItemWrite {
            turn_id,
            kind,
            status: TimelineItemStatus::Pending,
            client_item_id: None,
            external_id: Some(external_id),
            content: "",
            metadata,
            now,
        },
    )
    .map(|(id, _)| id)
}

pub(super) fn item_metadata(transaction: &Transaction<'_>, id: &str) -> Result<Value> {
    let encoded: String = transaction
        .query_row(
            "SELECT metadata FROM timeline_items WHERE id=?1",
            [id],
            |row| row.get(0),
        )
        .map_err(storage_error)?;
    serde_json::from_str(&encoded)
        .map_err(|error| AxiomError::Storage(format!("invalid timeline metadata: {error}")))
}

pub(super) fn mark_response_verified(
    transaction: &Transaction<'_>,
    thread_id: &str,
    turn_id: &str,
    now: &str,
) -> Result<()> {
    let has_requests: bool = transaction.query_row("SELECT EXISTS(SELECT 1 FROM request_usage WHERE thread_id=?1 AND json_extract(record,'$.turnId')=?2)",
        params![thread_id, turn_id], |row| row.get(0)).map_err(storage_error)?;
    // New requests are verified one at a time. A turn-wide event must never
    // upgrade an interrupted request left earlier in that turn by steering.
    if has_requests {
        return Ok(());
    }
    let mut statement = transaction
        .prepare(
            "SELECT id, metadata FROM timeline_items
             WHERE thread_id=?1 AND turn_id=?2
               AND kind IN ('assistant_message','reasoning')",
        )
        .map_err(storage_error)?;
    let rows = statement
        .query_map(params![thread_id, turn_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(storage_error)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(storage_error)?;
    drop(statement);

    if rows.is_empty() {
        return Err(AxiomError::Storage(format!(
            "verified response has no assistant timeline item for turn {turn_id} in thread {thread_id}"
        )));
    }
    for (id, encoded) in rows {
        let mut metadata: Value = serde_json::from_str(&encoded)
            .map_err(|error| AxiomError::Storage(format!("invalid timeline metadata: {error}")))?;
        let Some(metadata) = metadata.as_object_mut() else {
            return Err(AxiomError::Storage(
                "timeline metadata must be a JSON object".into(),
            ));
        };
        metadata.insert("terminal_verified".into(), Value::Bool(true));
        transaction
            .execute(
                "UPDATE timeline_items SET metadata=?2, updated_at=?3 WHERE id=?1",
                params![id, encode_metadata(Value::Object(metadata.clone()))?, now],
            )
            .map_err(storage_error)?;
    }
    Ok(())
}

pub(super) fn update_notice_resolution(
    transaction: &Transaction<'_>,
    thread_id: &str,
    external_id: &str,
    fields: Value,
    status: TimelineItemStatus,
    now: &str,
) -> Result<()> {
    let Some(id) = find_any_external_item(transaction, thread_id, external_id)? else {
        return Ok(());
    };
    let mut metadata = item_metadata(transaction, &id)?;
    let Some(target) = metadata.as_object_mut() else {
        return Err(AxiomError::Storage(
            "timeline metadata must be a JSON object".into(),
        ));
    };
    let Value::Object(fields) = fields else {
        return Err(AxiomError::Storage(
            "timeline resolution must be a JSON object".into(),
        ));
    };
    target.extend(fields);
    transaction
        .execute(
            "UPDATE timeline_items SET status=?2, metadata=?3, updated_at=?4 WHERE id=?1",
            params![id, status.to_string(), encode_metadata(metadata)?, now],
        )
        .map_err(storage_error)?;
    Ok(())
}

pub(super) fn persist_question(
    transaction: &Transaction<'_>,
    thread_id: &str,
    request_id: &str,
    content: &str,
    metadata: Value,
    now: &str,
) -> Result<()> {
    let turn_id = active_turn_id(transaction, thread_id)?;
    upsert_timeline_item(
        transaction,
        thread_id,
        TimelineItemWrite {
            turn_id: turn_id.as_deref(),
            kind: TimelineItemKind::Notice,
            status: TimelineItemStatus::Pending,
            client_item_id: None,
            external_id: Some(request_id),
            content,
            metadata,
            now,
        },
    )?;
    set_thread_lifecycle(transaction, thread_id, ThreadLifecycle::WaitingForAnswer)
}

pub(super) struct NoticeWrite<'a> {
    pub(super) turn_id: Option<&'a str>,
    pub(super) status: TimelineItemStatus,
    pub(super) external_id: Option<&'a str>,
    pub(super) code: &'a str,
    pub(super) content: &'a str,
    pub(super) details: Value,
    pub(super) now: &'a str,
}

pub(super) fn insert_notice(
    transaction: &Transaction<'_>,
    thread_id: &str,
    notice: &NoticeWrite<'_>,
) -> Result<String> {
    insert_timeline_item(
        transaction,
        thread_id,
        TimelineItemWrite {
            turn_id: notice.turn_id,
            kind: TimelineItemKind::Notice,
            status: notice.status,
            client_item_id: None,
            external_id: notice.external_id,
            content: notice.content,
            metadata: json!({"code":notice.code,"details":notice.details}),
            now: notice.now,
        },
    )
    .map(|(id, _)| id)
}

pub(super) fn truncate_for_revision(
    transaction: &Transaction<'_>,
    id: &SessionId,
    item_id: &str,
    expected_revision: u64,
) -> Result<()> {
    let revision = query_thread_revision(transaction, id)?;
    if revision.revision != expected_revision {
        return Err(AxiomError::InvalidTransition(
            "The conversation changed. Reload it before editing or regenerating.".into(),
        ));
    }
    let running: bool = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM turns WHERE thread_id=?1 AND status='running')",
            [id.to_string()],
            |row| row.get(0),
        )
        .map_err(storage_error)?;
    if running {
        return Err(AxiomError::InvalidTransition(
            "Stop the current reply before editing or regenerating.".into(),
        ));
    }
    let sequence: Option<i64> = transaction.query_row(
            "SELECT i.sequence FROM timeline_items i JOIN turns t ON t.thread_id=i.thread_id AND t.user_item_id=i.id
             WHERE i.thread_id=?1 AND i.id=?2 AND i.kind='user_message'",
            params![id.to_string(), item_id], |row| row.get(0)).optional().map_err(storage_error)?;
    let sequence = sequence.ok_or_else(|| {
        AxiomError::InvalidTransition("Choose the message that started this turn.".into())
    })?;
    transaction
        .execute(
            "DELETE FROM turns WHERE thread_id=?1 AND user_item_id IN
             (SELECT id FROM timeline_items WHERE thread_id=?1 AND sequence>=?2)",
            params![id.to_string(), sequence],
        )
        .map_err(storage_error)?;
    transaction
        .execute(
            "DELETE FROM timeline_items WHERE thread_id=?1 AND sequence>=?2",
            params![id.to_string(), sequence],
        )
        .map_err(storage_error)?;
    transaction.execute(
            "UPDATE threads SET revision=revision+1,last_reported_usage=NULL,updated_at=?2 WHERE id=?1",
            params![id.to_string(), Utc::now().to_rfc3339()]).map_err(storage_error)?;
    Ok(())
}

pub(super) fn finish_turn(
    transaction: &Transaction<'_>,
    thread_id: &str,
    turn_id: &str,
    status: TurnStatus,
    error: Option<&str>,
    now: &str,
) -> Result<()> {
    let changed = transaction
        .execute(
            "UPDATE turns SET status=?3, error=?4, completed_at=?5
             WHERE thread_id=?1 AND id=?2 AND status='running'",
            params![thread_id, turn_id, status.to_string(), error, now],
        )
        .map_err(storage_error)?;
    if changed == 0 {
        return Err(AxiomError::Storage(format!(
            "turn {turn_id} is not running in thread {thread_id}"
        )));
    }
    if matches!(
        status,
        TurnStatus::Cancelled | TurnStatus::Failed | TurnStatus::Interrupted
    ) {
        // A cancelled ACP future can be dropped before its provider emits final
        // accounting. End execution atomically with the turn; billing remains
        // independently recoverable and must never promote response verification.
        let (state, code) = if status == TurnStatus::Cancelled {
            ("cancelled", "CANCELLED")
        } else {
            ("failed", "CLIENT_INTERRUPTED")
        };
        transaction
            .execute(
                "UPDATE request_usage SET record=json_set(record,
                '$.state', ?3, '$.errorCode', ?4, '$.finishedAtMs', ?5)
             WHERE thread_id=?1 AND json_extract(record,'$.turnId')=?2
               AND json_extract(record,'$.state')='running'",
                params![
                    thread_id,
                    turn_id,
                    state,
                    code,
                    Utc::now().timestamp_millis().to_string()
                ],
            )
            .map_err(storage_error)?;
    }
    let item_status = match status {
        TurnStatus::Running => TimelineItemStatus::InProgress,
        TurnStatus::Completed => TimelineItemStatus::Completed,
        TurnStatus::Cancelled => TimelineItemStatus::Cancelled,
        TurnStatus::Failed => TimelineItemStatus::Failed,
        TurnStatus::Interrupted => TimelineItemStatus::Interrupted,
    };
    transaction
        .execute(
            "UPDATE timeline_items SET status=?3, updated_at=?4
             WHERE thread_id=?1 AND turn_id=?2 AND status IN ('pending','in_progress')",
            params![thread_id, turn_id, item_status.to_string(), now],
        )
        .map_err(storage_error)?;
    set_thread_lifecycle(transaction, thread_id, ThreadLifecycle::Ready)
}

pub(super) fn set_thread_lifecycle(
    transaction: &Transaction<'_>,
    thread_id: &str,
    lifecycle: ThreadLifecycle,
) -> Result<()> {
    let changed = transaction
        .execute(
            "UPDATE threads SET lifecycle=?2 WHERE id=?1",
            params![thread_id, lifecycle.to_string()],
        )
        .map_err(storage_error)?;
    if changed == 0 {
        Err(AxiomError::SessionNotFound(thread_id.into()))
    } else {
        Ok(())
    }
}
