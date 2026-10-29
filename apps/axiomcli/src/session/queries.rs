//! Thread catalog, snapshots, edits and metadata queries.

use std::str::FromStr as _;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::Utc;
use rusqlite::{Connection, OptionalExtension as _, Row, params};
use serde_json::Value;

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
        self.read_snapshot(id, after_sequence, limit, None, false)
    }

    /// A bounded page for local IPC. Accounting has its own stable cursor.
    pub fn thread_page(
        &self,
        id: &SessionId,
        after_sequence: Option<u64>,
        after_request_id: Option<&str>,
        limit: usize,
    ) -> Result<ThreadSnapshot> {
        self.read_snapshot(id, after_sequence, limit, after_request_id, true)
    }

    fn read_snapshot(
        &self,
        id: &SessionId,
        after_sequence: Option<u64>,
        limit: usize,
        after_request_id: Option<&str>,
        paged: bool,
    ) -> Result<ThreadSnapshot> {
        if limit == 0 || limit > MAX_TIMELINE_PAGE {
            return Err(AxiomError::Storage(format!(
                "timeline page size must be between 1 and {MAX_TIMELINE_PAGE}"
            )));
        }
        let mut connection = self.lock()?;
        let transaction = connection.transaction().map_err(storage_error)?;
        let thread = query_thread_summary(&transaction, id)?;
        let turns = query_turns_filtered(&transaction, id, paged)?;
        let mut next_request_usage_cursor = None;
        let request_usage = {
            let sql = if paged {
                "SELECT record FROM request_usage WHERE thread_id=?1 AND request_id>?2 ORDER BY request_id LIMIT 101"
            } else {
                "SELECT record FROM request_usage WHERE thread_id=?1 AND request_id>?2 ORDER BY rowid"
            };
            let mut statement = transaction.prepare(sql).map_err(storage_error)?;
            let mut rows = statement
                .query(params![id.to_string(), after_request_id.unwrap_or("")])
                .map_err(storage_error)?;
            let mut records: Vec<axiom_inference::RequestUsage> = Vec::new();
            let mut bytes = 0;
            while let Some(row) = rows.next().map_err(storage_error)? {
                let json: String = row.get(0).map_err(storage_error)?;
                if paged && (records.len() == 100 || bytes + json.len() > 512 * 1024) {
                    next_request_usage_cursor = Some(
                        records
                            .last()
                            .ok_or_else(|| {
                                AxiomError::Storage(
                                    "accounting record exceeds local page limit".into(),
                                )
                            })?
                            .request_id
                            .clone(),
                    );
                    break;
                }
                bytes += json.len();
                records.push(serde_json::from_str(&json)?);
            }
            records
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
        let mut rows = statement
            .query(params![id.to_string(), after, fetch])
            .map_err(storage_error)?;
        let mut items: Vec<TimelineItem> = Vec::new();
        let mut bytes = 0;
        let mut next_cursor = None;
        while let Some(row) = rows.next().map_err(storage_error)? {
            let item = StoredTimelineItem::from_row(row)
                .map_err(storage_error)?
                .decode()?;
            // Count JSON bytes, including escaping, rather than raw message text.
            let item_bytes = if paged {
                serde_json::to_vec(&item)?.len()
            } else {
                0
            };
            if items.len() == limit || (paged && bytes + item_bytes > 32 * 1024 * 1024) {
                next_cursor = Some(
                    items
                        .last()
                        .ok_or_else(|| {
                            AxiomError::Storage("timeline item exceeds local page limit".into())
                        })?
                        .sequence,
                );
                break;
            }
            bytes += item_bytes;
            items.push(item);
        }
        drop(rows);
        drop(statement);
        transaction.commit().map_err(storage_error)?;
        Ok(ThreadSnapshot {
            thread,
            context_usage,
            request_usage,
            turns,
            items,
            next_cursor,
            next_request_usage_cursor,
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
            "UPDATE threads SET title=?2, title_generation_id=NULL, revision=revision+1, updated_at=?3 WHERE id=?1",
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

    pub fn prompt_attachments(
        &self,
        id: &SessionId,
        item_id: &str,
    ) -> Result<Vec<axiom_inference::PromptAttachment>> {
        query_prompt_attachments(&*self.lock()?, id, item_id)
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

    /// Reserve one automatic title without treating a later manual rename as a fallback.
    pub fn begin_title_generation(&self, id: &SessionId, fallback: &str) -> Result<Option<String>> {
        let fallback = validate_title(fallback)?;
        let generation = uuid::Uuid::new_v4().to_string();
        let changed = self.lock()?.execute(
            "UPDATE threads SET title=?2, title_generation_id=?3, revision=revision+1, updated_at=?4
             WHERE id=?1 AND title IS NULL",
            params![id.to_string(), fallback, generation, Utc::now().to_rfc3339()],
        ).map_err(storage_error)?;
        Ok((changed > 0).then_some(generation))
    }

    pub fn title_generation_pending(&self, id: &SessionId, generation: &str) -> Result<bool> {
        self.lock()?
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM threads WHERE id=?1 AND title_generation_id=?2)",
                params![id.to_string(), generation],
                |row| row.get(0),
            )
            .map_err(storage_error)
    }

    /// Compare and replace only this job's fallback; deletion and every rename revoke it.
    pub fn finish_title_generation(
        &self,
        id: &SessionId,
        generation: &str,
        title: Option<&str>,
    ) -> Result<bool> {
        let title = title.map(validate_title).transpose()?;
        let changed = self
            .lock()?
            .execute(
                "UPDATE threads SET title=COALESCE(?3,title), title_generation_id=NULL,
             revision=revision+1, updated_at=?4 WHERE id=?1 AND title_generation_id=?2",
                params![id.to_string(), generation, title, Utc::now().to_rfc3339()],
            )
            .map_err(storage_error)?;
        Ok(changed > 0)
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
    query_turns_filtered(connection, id, false)
}

fn query_turns_filtered(
    connection: &Connection,
    id: &SessionId,
    active_only: bool,
) -> Result<Vec<TurnRecord>> {
    let predicate = if active_only {
        " AND status='running' ORDER BY started_at, id LIMIT 1"
    } else {
        " ORDER BY started_at, id"
    };
    let mut statement = connection
        .prepare(&format!(
            "SELECT id, thread_id, status, user_item_id, assistant_item_id,
                    input_tokens, output_tokens, error, started_at, completed_at
             FROM turns WHERE thread_id=?1{predicate}",
        ))
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

pub(super) fn query_prompt_attachments(
    connection: &Connection,
    id: &SessionId,
    item_id: &str,
) -> Result<Vec<axiom_inference::PromptAttachment>> {
    let (payload, metadata, text): (Option<String>, String, String) = connection.query_row(
        "SELECT p.payload,i.metadata,i.content FROM timeline_items i LEFT JOIN prompt_attachments p ON p.user_item_id=i.id WHERE i.thread_id=?1 AND (i.id=?2 OR i.client_item_id=?2) AND i.kind='user_message'",
        params![id.to_string(), item_id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
    ).map_err(storage_error)?;
    if let Some(payload) = payload {
        let attachments: Vec<axiom_inference::PromptAttachment> = serde_json::from_str(&payload)?;
        axiom_inference::validate_stored_prompt(&text, &attachments)
            .map_err(|error| AxiomError::Storage(error.to_string()))?;
        Ok(attachments)
    } else if serde_json::from_str::<Value>(&metadata)?
        .get("attachments")
        .is_some()
    {
        Err(AxiomError::Storage(
            "Stored attachment contents are missing; the message cannot be replayed.".into(),
        ))
    } else {
        Ok(Vec::new())
    }
}
