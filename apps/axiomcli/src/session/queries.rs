//! Thread catalog, snapshots, edits and metadata queries.

use std::str::FromStr as _;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::Utc;
use rusqlite::{Connection, OptionalExtension as _, Row, params};

use crate::{
    AxiomError, Result,
    app::{AppEvent, EventEnvelope, SessionId, ThinkingLevel},
};

use super::codec::{checked_i64, checked_u64, decode_path, storage_error};
use super::records::{
    SessionSummary, ThreadCatalogPage, ThreadLifecycle, ThreadRevision, ThreadSnapshot,
    ThreadSummary, TimelineItem, TimelineItemKind, TimelineItemStatus, TurnRecord, TurnStatus,
};
use super::timeline::query_thread_revision;
use super::{MAX_THREAD_CATALOG_PAGE, MAX_THREAD_QUERY_BYTES, MAX_TIMELINE_PAGE, SessionStore};

#[derive(Debug)]
pub(super) struct StoredThread {
    id: String,
    title: Option<String>,
    cwd: Vec<u8>,
    cwd_encoding: String,
    origin: String,
    profile: String,
    selected_model: Option<String>,
    thinking_level: String,
    lifecycle: String,
    archived: bool,
    revision: i64,
    last_sequence: i64,
    created_at: String,
    updated_at: String,
    last_message_at: Option<String>,
    last_user_message_at: Option<String>,
}

impl StoredThread {
    pub(super) fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            title: row.get(1)?,
            cwd: row.get(2)?,
            cwd_encoding: row.get(3)?,
            origin: row.get(4)?,
            profile: row.get(5)?,
            selected_model: row.get(6)?,
            thinking_level: row.get(7)?,
            lifecycle: row.get(8)?,
            archived: row.get(9)?,
            revision: row.get(10)?,
            last_sequence: row.get(11)?,
            created_at: row.get(12)?,
            updated_at: row.get(13)?,
            last_message_at: row.get(14)?,
            last_user_message_at: row.get(15)?,
        })
    }

    pub(super) fn decode(self) -> Result<ThreadSummary> {
        Ok(ThreadSummary {
            id: self.id,
            title: self.title,
            cwd: decode_path(&self.cwd_encoding, &self.cwd)?,
            origin: self.origin,
            profile: self.profile,
            selected_model: self.selected_model,
            thinking_level: ThinkingLevel::from_str(&self.thinking_level).map_err(|_| {
                AxiomError::Storage(format!(
                    "invalid thinking level `{}` in local state",
                    self.thinking_level
                ))
            })?,
            lifecycle: ThreadLifecycle::from_str(&self.lifecycle)?,
            archived: self.archived,
            revision: checked_u64(self.revision, "thread revision")?,
            last_timeline_sequence: checked_u64(self.last_sequence, "last timeline sequence")?,
            created_at: self.created_at,
            updated_at: self.updated_at,
            last_message_at: self.last_message_at,
            last_user_message_at: self.last_user_message_at,
        })
    }
}

pub(super) const THREAD_COLUMNS: &str = "id, title, cwd, cwd_encoding, origin, profile, selected_model, thinking_level, lifecycle, \
     archived, revision, next_timeline_sequence, created_at, updated_at, last_message_at, last_user_message_at";

pub(super) fn query_thread_summary(
    connection: &Connection,
    id: &SessionId,
) -> Result<ThreadSummary> {
    let sql = format!("SELECT {THREAD_COLUMNS} FROM threads WHERE id=?1");
    match connection.query_row(&sql, [id.to_string()], StoredThread::from_row) {
        Ok(stored) => stored.decode(),
        Err(rusqlite::Error::QueryReturnedNoRows) => {
            Err(AxiomError::SessionNotFound(id.to_string()))
        }
        Err(error) => Err(storage_error(error)),
    }
}

impl SessionStore {
    pub fn thread_catalog(
        &self,
        query: Option<&str>,
        include_archived: bool,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<ThreadCatalogPage> {
        if limit == 0 || limit > MAX_THREAD_CATALOG_PAGE {
            return Err(AxiomError::Storage(format!(
                "thread catalog page size must be between 1 and {MAX_THREAD_CATALOG_PAGE}"
            )));
        }
        let query = query.map(str::trim).filter(|value| !value.is_empty());
        if query.is_some_and(|value| value.len() > MAX_THREAD_QUERY_BYTES) {
            return Err(AxiomError::Storage(format!(
                "thread catalog query cannot exceed {MAX_THREAD_QUERY_BYTES} bytes"
            )));
        }
        let (cursor_time, cursor_id) = cursor
            .map(decode_thread_cursor)
            .transpose()?
            .map_or((None, None), |(time, id)| (Some(time), Some(id)));
        let connection = self.lock()?;
        let sql = format!(
            "SELECT {THREAD_COLUMNS} FROM threads
             WHERE (archived=0 OR ?1=1)
               AND (?2 IS NULL OR instr(lower(COALESCE(title,'')), lower(?2))>0
                                OR instr(lower(id), lower(?2))>0)
               AND (?3 IS NULL OR COALESCE(last_user_message_at,'') < ?3 OR (COALESCE(last_user_message_at,'') = ?3 AND id > ?4))
             ORDER BY COALESCE(last_user_message_at,'') DESC, id ASC LIMIT ?5"
        );
        let mut statement = connection.prepare(&sql).map_err(storage_error)?;
        let stored = statement
            .query_map(
                params![
                    i64::from(include_archived),
                    query,
                    cursor_time,
                    cursor_id,
                    i64::try_from(limit.saturating_add(1)).unwrap_or(i64::MAX)
                ],
                StoredThread::from_row,
            )
            .map_err(storage_error)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(storage_error)?;
        let mut threads = stored
            .into_iter()
            .map(StoredThread::decode)
            .collect::<Result<Vec<_>>>()?;
        let has_more = threads.len() > limit;
        if has_more {
            threads.pop();
        }
        let next_cursor = if has_more {
            threads
                .last()
                .map(|thread| {
                    encode_thread_cursor(
                        thread.last_user_message_at.as_deref().unwrap_or(""),
                        &thread.id,
                    )
                })
                .transpose()?
        } else {
            None
        };
        Ok(ThreadCatalogPage {
            threads,
            next_cursor,
        })
    }

    pub fn list_threads(&self, include_archived: bool) -> Result<Vec<ThreadSummary>> {
        let connection = self.lock()?;
        let sql = format!(
            "SELECT {THREAD_COLUMNS} FROM threads
             WHERE archived=0 OR ?1=1 ORDER BY COALESCE(last_user_message_at,'') DESC, id LIMIT 1000"
        );
        let mut statement = connection.prepare(&sql).map_err(storage_error)?;
        let stored = statement
            .query_map([i64::from(include_archived)], StoredThread::from_row)
            .map_err(storage_error)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(storage_error)?;
        stored.into_iter().map(StoredThread::decode).collect()
    }

    pub fn thread_summary(&self, id: &SessionId) -> Result<ThreadSummary> {
        let connection = self.lock()?;
        query_thread_summary(&connection, id)
    }

    pub fn thread_snapshot(
        &self,
        id: &SessionId,
        after_sequence: Option<u64>,
        limit: usize,
    ) -> Result<ThreadSnapshot> {
        if limit == 0 || limit > MAX_TIMELINE_PAGE {
            return Err(AxiomError::Storage(format!(
                "timeline page size must be between 1 and {MAX_TIMELINE_PAGE}"
            )));
        }
        let mut connection = self.lock()?;
        let transaction = connection.transaction().map_err(storage_error)?;
        let thread = query_thread_summary(&transaction, id)?;
        let turns = query_turns(&transaction, id)?;
        let request_usage = {
            let mut statement = transaction
                .prepare("SELECT record FROM request_usage WHERE thread_id=?1 ORDER BY rowid")
                .map_err(storage_error)?;
            let records = statement
                .query_map([id.to_string()], |row| row.get::<_, String>(0))
                .map_err(storage_error)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(storage_error)?;
            records
                .into_iter()
                .map(|json| serde_json::from_str(&json))
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        let context_usage: Option<String> = transaction
            .query_row(
                "SELECT last_reported_usage FROM threads WHERE id=?1",
                [id.to_string()],
                |row| row.get(0),
            )
            .map_err(storage_error)?;
        let context_usage = context_usage
            .map(|json| serde_json::from_str(&json))
            .transpose()?;
        let after = checked_i64(after_sequence.unwrap_or(0), "timeline cursor")?;
        let fetch = i64::try_from(limit.saturating_add(1)).unwrap_or(i64::MAX);
        let mut statement = transaction
            .prepare(
                "SELECT id, thread_id, turn_id, sequence, kind, status, client_item_id,
                        external_id, content, metadata, created_at, updated_at
                 FROM timeline_items
                 WHERE thread_id=?1 AND sequence>?2 ORDER BY sequence ASC LIMIT ?3",
            )
            .map_err(storage_error)?;
        let stored = statement
            .query_map(
                params![id.to_string(), after, fetch],
                StoredTimelineItem::from_row,
            )
            .map_err(storage_error)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(storage_error)?;
        let mut items = stored
            .into_iter()
            .map(StoredTimelineItem::decode)
            .collect::<Result<Vec<_>>>()?;
        let has_more = items.len() > limit;
        if has_more {
            items.pop();
        }
        let next_cursor = has_more
            .then(|| items.last().map(|item| item.sequence))
            .flatten();
        drop(statement);
        transaction.commit().map_err(storage_error)?;
        Ok(ThreadSnapshot {
            thread,
            context_usage,
            request_usage,
            turns,
            items,
            next_cursor,
        })
    }

    pub fn list(&self, include_archived: bool) -> Result<Vec<SessionSummary>> {
        self.list_threads(include_archived)
            .map(|threads| threads.into_iter().map(Into::into).collect())
    }

    pub fn summary(&self, id: &SessionId) -> Result<SessionSummary> {
        self.thread_summary(id).map(Into::into)
    }

    pub fn rename(&self, id: &SessionId, title: &str) -> Result<ThreadRevision> {
        let title = validate_title(title)?;
        self.update_thread_metadata(
            id,
            "UPDATE threads SET title=?2, revision=revision+1, updated_at=?3 WHERE id=?1",
            title,
        )
    }

    /// Revise a locally stored conversation. Usage records are immutable history
    /// of paid requests and survive removal of their displayed messages.
    #[cfg(test)]
    pub(super) fn truncate_before_user_message(
        &self,
        id: &SessionId,
        item_id: &str,
        expected_revision: u64,
    ) -> Result<()> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction().map_err(storage_error)?;
        super::timeline::truncate_for_revision(&transaction, id, item_id, expected_revision)?;
        transaction.commit().map_err(storage_error)
    }

    pub fn history_before_user_message(
        &self,
        id: &SessionId,
        revision: &axiom_acp_extension::PromptRevision,
    ) -> Result<Vec<EventEnvelope>> {
        if self.thread_summary(id)?.revision != revision.expected_revision {
            return Err(AxiomError::InvalidTransition(
                "The conversation changed. Reload it before editing or regenerating.".into(),
            ));
        }
        let turn: Option<String> = self
            .lock()?
            .query_row(
                "SELECT id FROM turns WHERE thread_id=?1 AND user_item_id=?2",
                params![id.to_string(), revision.user_item_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage_error)?;
        let turn = turn.ok_or_else(|| {
            AxiomError::InvalidTransition("Choose the message that started this turn.".into())
        })?;
        let mut events = self.load(id)?;
        let boundary = events.iter().position(|event| matches!(&event.event, AppEvent::PromptAccepted { turn_id, .. } if turn_id.to_string() == turn))
            .ok_or_else(|| AxiomError::Storage("The original prompt is unavailable.".into()))?;
        events.truncate(boundary);
        Ok(events)
    }

    pub fn set_title_if_absent(&self, id: &SessionId, title: &str) -> Result<bool> {
        let title = validate_title(title)?;
        let now = Utc::now().to_rfc3339();
        let connection = self.lock()?;
        let changed = connection
            .execute(
                "UPDATE threads SET title=?2, revision=revision+1, updated_at=?3
                 WHERE id=?1 AND title IS NULL",
                params![id.to_string(), title, now],
            )
            .map_err(storage_error)?;
        if changed > 0 {
            return Ok(true);
        }
        ensure_thread_exists_connection(&connection, id).map(|()| false)
    }

    pub fn archive(&self, id: &SessionId, archived: bool) -> Result<ThreadRevision> {
        let now = Utc::now().to_rfc3339();
        let connection = self.lock()?;
        let changed = connection
            .execute(
                "UPDATE threads SET archived=?2, revision=revision+1, updated_at=?3 WHERE id=?1",
                params![id.to_string(), archived, now],
            )
            .map_err(storage_error)?;
        if changed == 0 {
            return Err(AxiomError::SessionNotFound(id.to_string()));
        }
        query_thread_revision(&connection, id)
    }

    pub fn delete_sessions(&self, ids: &[SessionId]) -> Result<usize> {
        if ids.is_empty() {
            return Ok(0);
        }
        let mut connection = self.lock()?;
        let transaction = connection.transaction().map_err(storage_error)?;
        let mut deleted = 0_usize;
        for id in ids {
            deleted = deleted.saturating_add(
                transaction
                    .execute("DELETE FROM threads WHERE id=?1", [id.to_string()])
                    .map_err(storage_error)?,
            );
        }
        transaction.commit().map_err(storage_error)?;
        Ok(deleted)
    }

    pub(super) fn update_thread_metadata(
        &self,
        id: &SessionId,
        statement: &str,
        value: &str,
    ) -> Result<ThreadRevision> {
        let connection = self.lock()?;
        let changed = connection
            .execute(
                statement,
                params![id.to_string(), value, Utc::now().to_rfc3339()],
            )
            .map_err(storage_error)?;
        if changed == 0 {
            return Err(AxiomError::SessionNotFound(id.to_string()));
        }
        query_thread_revision(&connection, id)
    }
}

#[derive(Debug)]
pub(super) struct StoredTimelineItem {
    id: String,
    thread_id: String,
    turn_id: Option<String>,
    sequence: i64,
    kind: String,
    status: String,
    client_item_id: Option<String>,
    external_id: Option<String>,
    content: String,
    metadata: String,
    created_at: String,
    updated_at: String,
}

impl StoredTimelineItem {
    pub(super) fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            thread_id: row.get(1)?,
            turn_id: row.get(2)?,
            sequence: row.get(3)?,
            kind: row.get(4)?,
            status: row.get(5)?,
            client_item_id: row.get(6)?,
            external_id: row.get(7)?,
            content: row.get(8)?,
            metadata: row.get(9)?,
            created_at: row.get(10)?,
            updated_at: row.get(11)?,
        })
    }

    pub(super) fn decode(self) -> Result<TimelineItem> {
        Ok(TimelineItem {
            id: self.id,
            thread_id: self.thread_id,
            turn_id: self.turn_id,
            sequence: checked_u64(self.sequence, "timeline sequence")?,
            kind: TimelineItemKind::from_str(&self.kind)?,
            status: TimelineItemStatus::from_str(&self.status)?,
            client_item_id: self.client_item_id,
            external_id: self.external_id,
            content: self.content,
            metadata: serde_json::from_str(&self.metadata).map_err(|error| {
                AxiomError::Storage(format!("invalid timeline metadata: {error}"))
            })?,
            created_at: self.created_at,
            updated_at: self.updated_at,
        })
    }
}

pub(super) fn query_turns(connection: &Connection, id: &SessionId) -> Result<Vec<TurnRecord>> {
    let mut statement = connection
        .prepare(
            "SELECT id, thread_id, status, user_item_id, assistant_item_id,
                    input_tokens, output_tokens, error, started_at, completed_at
             FROM turns WHERE thread_id=?1 ORDER BY started_at, id",
        )
        .map_err(storage_error)?;
    let stored = statement
        .query_map([id.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, Option<String>>(9)?,
            ))
        })
        .map_err(storage_error)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(storage_error)?;
    stored
        .into_iter()
        .map(
            |(
                id,
                thread_id,
                status,
                user_item_id,
                assistant_item_id,
                input_tokens,
                output_tokens,
                error,
                started_at,
                completed_at,
            )| {
                Ok(TurnRecord {
                    id,
                    thread_id,
                    status: TurnStatus::from_str(&status)?,
                    user_item_id,
                    assistant_item_id,
                    input_tokens: checked_u64(input_tokens, "input token count")?,
                    output_tokens: checked_u64(output_tokens, "output token count")?,
                    error,
                    started_at,
                    completed_at,
                })
            },
        )
        .collect()
}

pub(super) fn ensure_thread_exists_connection(
    connection: &Connection,
    id: &SessionId,
) -> Result<()> {
    let exists: bool = connection
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

pub(super) fn validate_title(title: &str) -> Result<&str> {
    let title = title.trim();
    if title.is_empty() {
        return Err(AxiomError::Storage("thread title cannot be empty".into()));
    }
    if title.len() > 512 {
        return Err(AxiomError::Storage(
            "thread title cannot exceed 512 bytes".into(),
        ));
    }
    Ok(title)
}

pub(super) fn encode_thread_cursor(updated_at: &str, id: &str) -> Result<String> {
    let encoded = serde_json::to_vec(&(updated_at, id))?;
    Ok(URL_SAFE_NO_PAD.encode(encoded))
}

pub(super) fn decode_thread_cursor(cursor: &str) -> Result<(String, String)> {
    if cursor.len() > 2_048 {
        return Err(AxiomError::Storage(
            "thread catalog cursor is too long".into(),
        ));
    }
    let decoded = URL_SAFE_NO_PAD
        .decode(cursor)
        .map_err(|_| AxiomError::Storage("thread catalog cursor is invalid".into()))?;
    let (updated_at, id): (String, String) = serde_json::from_slice(&decoded)
        .map_err(|_| AxiomError::Storage("thread catalog cursor is invalid".into()))?;
    if id.is_empty() {
        return Err(AxiomError::Storage(
            "thread catalog cursor is invalid".into(),
        ));
    }
    Ok((updated_at, id))
}
