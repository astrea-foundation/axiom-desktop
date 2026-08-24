use std::{
    collections::{BTreeSet, HashMap, HashSet},
    path::{Path, PathBuf},
    str::FromStr as _,
    sync::{Arc, Mutex, MutexGuard},
};

use chrono::{Duration, Utc};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::{
    AxiomError, Result,
    app::{AppEvent, EventEnvelope, PermissionProfile, SessionId, ThinkingLevel},
    audit::redact_value,
};

const SCHEMA_VERSION: i64 = 3;
const MAX_EVENTS_PER_LOAD: usize = 100_000;
const SNAPSHOT_INTERVAL: u64 = 64;
const MAX_SNAPSHOT_BYTES: usize = 256 * 1024;
const MAX_RECOVERY_DIAGNOSTIC_BYTES: usize = 480;

#[derive(Clone)]
pub struct SessionStore {
    connection: Arc<Mutex<Connection>>,
    path: Option<PathBuf>,
}

impl std::fmt::Debug for SessionStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SessionStore")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSummary {
    pub id: String,
    pub title: Option<String>,
    pub cwd: PathBuf,
    pub origin: String,
    pub profile: String,
    pub archived: bool,
    pub history_truncated: bool,
    pub updated_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryStatus {
    pub session_id: SessionId,
    pub interrupted_turns: usize,
    pub interrupted_tools: Vec<InterruptedTool>,
    pub changed_paths: Vec<PathBuf>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterruptedTool {
    pub call_id: String,
    pub name: String,
    pub status: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoverySnapshot {
    pub session_id: SessionId,
    pub through_sequence: u64,
    pub recovery: RecoveryStatus,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadOutcome {
    pub events: Vec<EventEnvelope>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Copy, Debug)]
pub struct ExportOptions {
    pub include_prompts: bool,
    pub include_tool_output: bool,
    pub max_events: usize,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            include_prompts: true,
            include_tool_output: true,
            max_events: 10_000,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct RetentionPolicy {
    pub max_events_per_session: usize,
    pub delete_archived_older_than_days: Option<i64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetentionReport {
    pub deleted_sessions: usize,
    pub deleted_events: usize,
}

impl SessionStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let connection = Connection::open(path).map_err(storage_error)?;
        let store = Self {
            connection: Arc::new(Mutex::new(connection)),
            path: Some(path.to_path_buf()),
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
        store.migrate()?;
        Ok(store)
    }

    pub fn in_memory() -> Result<Self> {
        let store = Self {
            connection: Arc::new(Mutex::new(
                Connection::open_in_memory().map_err(storage_error)?,
            )),
            path: None,
        };
        store.migrate()?;
        Ok(store)
    }

    pub fn append(&self, envelope: &EventEnvelope) -> Result<()> {
        let mut value = serde_json::to_value(envelope)?;
        redact_value(&mut value);
        let encoded = serde_json::to_string(&value)?;
        let now = envelope.occurred_at.to_rfc3339();
        let id = envelope.session_id.to_string();
        let mut connection = self.lock()?;
        let transaction = connection.transaction().map_err(storage_error)?;

        if let AppEvent::SessionCreated {
            cwd,
            origin,
            profile,
        } = &envelope.event
        {
            transaction
                .execute(
                    "INSERT INTO sessions(id, cwd, origin, profile, created_at, updated_at, archived)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?5, 0)
                     ON CONFLICT(id) DO UPDATE SET updated_at=excluded.updated_at",
                    params![id, cwd.to_string_lossy(), format!("{origin:?}").to_ascii_lowercase(), profile.to_string(), now],
                )
                .map_err(storage_error)?;
        } else {
            let exists: bool = transaction
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM sessions WHERE id=?1)",
                    [&id],
                    |row| row.get(0),
                )
                .map_err(storage_error)?;
            if !exists {
                return Err(AxiomError::Storage(format!(
                    "session {id} must be created before events are appended"
                )));
            }
        }

        let sequence = i64::try_from(envelope.sequence)
            .map_err(|_| AxiomError::Storage("event sequence exceeds SQLite range".into()))?;
        transaction
            .execute(
                "INSERT INTO events(session_id, sequence, occurred_at, payload)
                 VALUES (?1, ?2, ?3, ?4)",
                params![id, sequence, now, encoded],
            )
            .map_err(storage_error)?;
        transaction
            .execute(
                "UPDATE sessions SET updated_at=?2 WHERE id=?1",
                params![id, now],
            )
            .map_err(storage_error)?;
        if let AppEvent::PermissionProfileChanged { profile }
        | AppEvent::SessionResumed { profile, .. } = &envelope.event
        {
            transaction
                .execute(
                    "UPDATE sessions SET profile=?2 WHERE id=?1",
                    params![id, profile.to_string()],
                )
                .map_err(storage_error)?;
        }
        transaction.commit().map_err(storage_error)?;
        drop(connection);
        let snapshot_due = envelope.sequence.is_multiple_of(SNAPSHOT_INTERVAL)
            || matches!(
                &envelope.event,
                AppEvent::TurnCompleted { .. }
                    | AppEvent::TurnCancelled { .. }
                    | AppEvent::ErrorRaised { .. }
                    | AppEvent::SessionClosed
            );
        let snapshot_result = snapshot_due.then(|| self.create_snapshot(&envelope.session_id));
        if let Some(Err(error)) = snapshot_result {
            // The append already committed. Never return an ambiguous failure
            // which could tempt a caller to repeat an event or side effect;
            // snapshots are rebuildable from the journal.
            tracing::warn!(session_id = %envelope.session_id, %error, "session snapshot deferred");
        }
        Ok(())
    }

    pub fn append_all(&self, envelopes: &[EventEnvelope]) -> Result<()> {
        for envelope in envelopes {
            self.append(envelope)?;
        }
        Ok(())
    }

    /// The model used by the most recently submitted prompt in any adapter.
    /// This is an application preference, separate from per-session events.
    pub fn last_used_model(&self) -> Result<Option<String>> {
        self.lock()?
            .query_row(
                "SELECT value FROM app_settings WHERE key='last_used_model'",
                [],
                |row| row.get::<_, String>(0),
            )
            .map(Some)
            .or_else(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(storage_error(other)),
            })
    }

    pub fn set_last_used_model(&self, model: &str) -> Result<()> {
        let model = model.trim();
        if model.is_empty() || model.len() > 256 || model.chars().any(char::is_whitespace) {
            return Err(AxiomError::Storage(
                "last-used model must be a non-empty model ID without whitespace".into(),
            ));
        }
        self.lock()?
            .execute(
                "INSERT INTO app_settings(key, value, updated_at)
                 VALUES ('last_used_model', ?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value, updated_at=excluded.updated_at",
                params![model, Utc::now().to_rfc3339()],
            )
            .map_err(storage_error)?;
        Ok(())
    }

    /// The explicit reasoning level used by the most recently submitted
    /// prompt in any adapter.
    pub fn last_used_thinking(&self) -> Result<Option<ThinkingLevel>> {
        let value = self
            .lock()?
            .query_row(
                "SELECT value FROM app_settings WHERE key='last_used_thinking'",
                [],
                |row| row.get::<_, String>(0),
            )
            .map(Some)
            .or_else(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(storage_error(other)),
            })?;
        value
            .map(|value| {
                ThinkingLevel::from_str(&value).map_err(|_| {
                    AxiomError::Storage(format!(
                        "stored last-used thinking level `{value}` is invalid"
                    ))
                })
            })
            .transpose()
    }

    pub fn set_last_used_thinking(&self, level: ThinkingLevel) -> Result<()> {
        self.lock()?
            .execute(
                "INSERT INTO app_settings(key, value, updated_at)
                 VALUES ('last_used_thinking', ?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value, updated_at=excluded.updated_at",
                params![level.to_string(), Utc::now().to_rfc3339()],
            )
            .map_err(storage_error)?;
        Ok(())
    }

    pub fn load(&self, id: &SessionId) -> Result<Vec<EventEnvelope>> {
        let outcome = self.load_recovering(id)?;
        if let Some(warning) = outcome.warnings.first() {
            return Err(AxiomError::Storage(warning.clone()));
        }
        Ok(outcome.events)
    }

    pub fn load_recovering(&self, id: &SessionId) -> Result<LoadOutcome> {
        let connection = self.lock()?;
        let mut statement = connection
            .prepare(
                "SELECT payload FROM events WHERE session_id=?1 ORDER BY sequence ASC LIMIT ?2",
            )
            .map_err(storage_error)?;
        let encoded = statement
            .query_map(
                params![
                    id.to_string(),
                    i64::try_from(MAX_EVENTS_PER_LOAD).unwrap_or(i64::MAX)
                ],
                |row| row.get::<_, String>(0),
            )
            .map_err(storage_error)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(storage_error)?;
        if encoded.is_empty() {
            return Err(AxiomError::SessionNotFound(id.to_string()));
        }
        let mut events = Vec::with_capacity(encoded.len());
        let mut warnings = Vec::new();
        for (index, payload) in encoded.into_iter().enumerate() {
            match serde_json::from_str(&payload) {
                Ok(event) => events.push(event),
                Err(error) => {
                    warnings.push(bounded_diagnostic(format!(
                        "corrupt event at journal position {} in session {id}: {error}; recovery stopped before the damaged record and no records were deleted",
                        index + 1
                    )));
                    break;
                }
            }
        }
        Ok(LoadOutcome { events, warnings })
    }

    pub fn list(&self, include_archived: bool) -> Result<Vec<SessionSummary>> {
        let connection = self.lock()?;
        let mut statement = connection
            .prepare(
                "SELECT id, title, cwd, origin, profile, archived, history_truncated, updated_at
                 FROM sessions WHERE archived=0 OR ?1=1 ORDER BY updated_at DESC LIMIT 1000",
            )
            .map_err(storage_error)?;
        statement
            .query_map([i64::from(include_archived)], |row| {
                Ok(SessionSummary {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    cwd: PathBuf::from(row.get::<_, String>(2)?),
                    origin: row.get(3)?,
                    profile: row.get(4)?,
                    archived: row.get(5)?,
                    history_truncated: row.get(6)?,
                    updated_at: row.get(7)?,
                })
            })
            .map_err(storage_error)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(storage_error)
    }

    pub fn summary(&self, id: &SessionId) -> Result<SessionSummary> {
        self.lock()?
            .query_row(
                "SELECT id, title, cwd, origin, profile, archived, history_truncated, updated_at
                 FROM sessions WHERE id=?1",
                [id.to_string()],
                |row| {
                    Ok(SessionSummary {
                        id: row.get(0)?,
                        title: row.get(1)?,
                        cwd: PathBuf::from(row.get::<_, String>(2)?),
                        origin: row.get(3)?,
                        profile: row.get(4)?,
                        archived: row.get(5)?,
                        history_truncated: row.get(6)?,
                        updated_at: row.get(7)?,
                    })
                },
            )
            .map_err(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => AxiomError::SessionNotFound(id.to_string()),
                other => storage_error(other),
            })
    }

    pub fn rename(&self, id: &SessionId, title: &str) -> Result<()> {
        if title.trim().is_empty() {
            return Err(AxiomError::Storage("session title cannot be empty".into()));
        }
        let changed = self
            .lock()?
            .execute(
                "UPDATE sessions SET title=?2, updated_at=?3 WHERE id=?1",
                params![id.to_string(), title.trim(), Utc::now().to_rfc3339()],
            )
            .map_err(storage_error)?;
        ensure_changed(id, changed)
    }

    /// Set an automatically generated title without overwriting a title the
    /// user (or another adapter) already chose. Returns `true` when the title
    /// was stored and `false` when this session was already titled.
    pub fn set_title_if_absent(&self, id: &SessionId, title: &str) -> Result<bool> {
        let title = title.trim();
        if title.is_empty() {
            return Err(AxiomError::Storage("session title cannot be empty".into()));
        }
        let connection = self.lock()?;
        let changed = connection
            .execute(
                "UPDATE sessions SET title=?2, updated_at=?3 WHERE id=?1 AND title IS NULL",
                params![id.to_string(), title, Utc::now().to_rfc3339()],
            )
            .map_err(storage_error)?;
        if changed > 0 {
            return Ok(true);
        }
        let exists: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sessions WHERE id=?1)",
                [id.to_string()],
                |row| row.get(0),
            )
            .map_err(storage_error)?;
        if exists {
            Ok(false)
        } else {
            Err(AxiomError::SessionNotFound(id.to_string()))
        }
    }

    pub fn archive(&self, id: &SessionId, archived: bool) -> Result<()> {
        let changed = self
            .lock()?
            .execute(
                "UPDATE sessions SET archived=?2, updated_at=?3 WHERE id=?1",
                params![id.to_string(), archived, Utc::now().to_rfc3339()],
            )
            .map_err(storage_error)?;
        ensure_changed(id, changed)
    }

    /// Permanently remove explicitly selected transcript IDs. Callers are
    /// responsible for user confirmation and for excluding their active
    /// session. The transaction makes multi-selection all-or-nothing.
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
                    .execute("DELETE FROM sessions WHERE id=?1", [id.to_string()])
                    .map_err(storage_error)?,
            );
        }
        transaction.commit().map_err(storage_error)?;
        Ok(deleted)
    }

    pub fn export(&self, id: &SessionId) -> Result<String> {
        self.export_with_options(id, ExportOptions::default())
    }

    pub fn export_with_options(&self, id: &SessionId, options: ExportOptions) -> Result<String> {
        let mut events = self.load_recovering(id)?.events;
        if events.len() > options.max_events.max(1) {
            let creation = events.first().cloned();
            let keep = options.max_events.max(2).saturating_sub(1);
            let split = events.len().saturating_sub(keep);
            let mut retained = creation.into_iter().collect::<Vec<_>>();
            retained.extend(events.drain(split..));
            events = retained;
        }
        for envelope in &mut events {
            match &mut envelope.event {
                AppEvent::PromptAccepted { text, .. } if !options.include_prompts => {
                    *text = "[omitted by export policy]".into();
                }
                AppEvent::ToolOutput { content, .. } if !options.include_tool_output => {
                    *content = "[omitted by export policy]".into();
                }
                _ => {}
            }
        }
        let mut value = serde_json::json!({
            "session": self.summary(id)?,
            "recovery": self.recovery_status(id)?,
            "events": events,
        });
        redact_value(&mut value);
        serde_json::to_string_pretty(&value).map_err(Into::into)
    }

    pub fn recovery_status(&self, id: &SessionId) -> Result<RecoveryStatus> {
        let outcome = self.load_recovering(id)?;
        Ok(recovery_from_events(id, &outcome.events, outcome.warnings))
    }

    pub fn latest_snapshot(&self, id: &SessionId) -> Result<Option<RecoverySnapshot>> {
        let payload = self.lock()?.query_row(
            "SELECT payload FROM snapshots WHERE session_id=?1 ORDER BY sequence DESC LIMIT 1",
            [id.to_string()],
            |row| row.get::<_, String>(0),
        );
        match payload {
            Ok(payload) => serde_json::from_str(&payload).map(Some).map_err(|error| {
                AxiomError::Storage(bounded_diagnostic(format!(
                    "corrupt snapshot for session {id}: {error}"
                )))
            }),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(storage_error(error)),
        }
    }

    pub fn apply_retention(&self, policy: RetentionPolicy) -> Result<RetentionReport> {
        if policy.max_events_per_session < 2 {
            return Err(AxiomError::Storage(
                "retention must preserve at least creation plus one event".into(),
            ));
        }
        let mut connection = self.lock()?;
        let transaction = connection.transaction().map_err(storage_error)?;
        let deleted_sessions = if let Some(days) = policy.delete_archived_older_than_days {
            if days < 0 {
                return Err(AxiomError::Storage(
                    "retention age must not be negative".into(),
                ));
            }
            let cutoff = (Utc::now() - Duration::days(days)).to_rfc3339();
            transaction
                .execute(
                    "DELETE FROM sessions WHERE archived=1 AND updated_at < ?1",
                    [cutoff],
                )
                .map_err(storage_error)?
        } else {
            0
        };
        let ids = {
            let mut statement = transaction
                .prepare("SELECT id FROM sessions")
                .map_err(storage_error)?;
            statement
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(storage_error)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(storage_error)?
        };
        let mut deleted_events = 0;
        for id in ids {
            let keep =
                i64::try_from(policy.max_events_per_session.saturating_sub(1)).unwrap_or(i64::MAX);
            let deleted = transaction
                .execute(
                    "DELETE FROM events
                     WHERE session_id=?1
                       AND sequence != (SELECT MIN(sequence) FROM events WHERE session_id=?1)
                       AND sequence NOT IN (
                         SELECT sequence FROM events WHERE session_id=?1
                         ORDER BY sequence DESC LIMIT ?2
                       )",
                    params![id, keep],
                )
                .map_err(storage_error)?;
            if deleted > 0 {
                transaction
                    .execute("UPDATE sessions SET history_truncated=1 WHERE id=?1", [id])
                    .map_err(storage_error)?;
                deleted_events += deleted;
            }
        }
        transaction.commit().map_err(storage_error)?;
        Ok(RetentionReport {
            deleted_sessions,
            deleted_events,
        })
    }

    fn create_snapshot(&self, id: &SessionId) -> Result<()> {
        let outcome = self.load_recovering(id)?;
        let Some(last_sequence) = outcome.events.last().map(|event| event.sequence) else {
            return Ok(());
        };
        let snapshot = RecoverySnapshot {
            session_id: id.clone(),
            through_sequence: last_sequence,
            recovery: recovery_from_events(id, &outcome.events, outcome.warnings),
        };
        let mut value = serde_json::to_value(&snapshot)?;
        redact_value(&mut value);
        let payload = serde_json::to_string(&value)?;
        if payload.len() > MAX_SNAPSHOT_BYTES {
            return Err(AxiomError::Storage(format!(
                "recovery snapshot exceeds {MAX_SNAPSHOT_BYTES} bytes"
            )));
        }
        let mut connection = self.lock()?;
        let transaction = connection.transaction().map_err(storage_error)?;
        transaction
            .execute(
                "INSERT OR REPLACE INTO snapshots(session_id, sequence, created_at, payload)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    id.to_string(),
                    i64::try_from(last_sequence).unwrap_or(i64::MAX),
                    Utc::now().to_rfc3339(),
                    payload
                ],
            )
            .map_err(storage_error)?;
        transaction
            .execute(
                "DELETE FROM snapshots WHERE session_id=?1 AND sequence NOT IN (
                   SELECT sequence FROM snapshots WHERE session_id=?1 ORDER BY sequence DESC LIMIT 8
                 )",
                [id.to_string()],
            )
            .map_err(storage_error)?;
        transaction.commit().map_err(storage_error)
    }

    fn migrate(&self) -> Result<()> {
        let mut connection = self.lock()?;
        connection
            .execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
            .map_err(storage_error)?;
        let version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(storage_error)?;
        if version > SCHEMA_VERSION {
            return Err(AxiomError::Storage(format!(
                "session database schema {version} is newer than supported schema {SCHEMA_VERSION}"
            )));
        }
        if version == 0 {
            let transaction = connection.transaction().map_err(storage_error)?;
            transaction
                .execute_batch(
                    "CREATE TABLE IF NOT EXISTS sessions(
                    id TEXT PRIMARY KEY,
                    title TEXT,
                    cwd TEXT NOT NULL,
                    origin TEXT NOT NULL,
                    profile TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    archived INTEGER NOT NULL DEFAULT 0
                 );
                 CREATE TABLE IF NOT EXISTS events(
                    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                    sequence INTEGER NOT NULL,
                    occurred_at TEXT NOT NULL,
                    payload TEXT NOT NULL,
                    PRIMARY KEY(session_id, sequence)
                 );
                 CREATE INDEX IF NOT EXISTS idx_sessions_updated ON sessions(updated_at DESC);
                 PRAGMA user_version=1;",
                )
                .map_err(storage_error)?;
            transaction.commit().map_err(storage_error)?;
        }
        let version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(storage_error)?;
        if version == 1 {
            let transaction = connection.transaction().map_err(storage_error)?;
            transaction
                .execute_batch(
                    "ALTER TABLE sessions ADD COLUMN history_truncated INTEGER NOT NULL DEFAULT 0;
                     CREATE TABLE snapshots(
                       session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                       sequence INTEGER NOT NULL,
                       created_at TEXT NOT NULL,
                       payload TEXT NOT NULL,
                       PRIMARY KEY(session_id, sequence)
                     );
                     PRAGMA user_version=2;",
                )
                .map_err(storage_error)?;
            transaction.commit().map_err(storage_error)?;
        }
        let version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(storage_error)?;
        if version == 2 {
            let transaction = connection.transaction().map_err(storage_error)?;
            transaction
                .execute_batch(
                    "CREATE TABLE app_settings(
                       key TEXT PRIMARY KEY,
                       value TEXT NOT NULL,
                       updated_at TEXT NOT NULL
                     );
                     PRAGMA user_version=3;",
                )
                .map_err(storage_error)?;
            transaction.commit().map_err(storage_error)?;
        }
        Ok(())
    }

    fn lock(&self) -> Result<MutexGuard<'_, Connection>> {
        self.connection
            .lock()
            .map_err(|_| AxiomError::Storage("session database lock was poisoned".into()))
    }

    #[cfg(test)]
    fn corrupt_payload(&self, id: &SessionId) -> Result<()> {
        self.lock()?
            .execute(
                "UPDATE events SET payload='{' WHERE session_id=?1",
                [id.to_string()],
            )
            .map_err(storage_error)?;
        Ok(())
    }
}

/// Return the model that was active when the latest prompt was accepted.
/// Model selections made after that prompt are deliberately ignored. Sessions
/// without prompts retain their latest selected model.
#[must_use]
pub fn model_for_resume(events: &[EventEnvelope], fallback: &str) -> String {
    let mut active = fallback.to_owned();
    let mut last_used = None;
    for envelope in events {
        match &envelope.event {
            AppEvent::ModelChanged { model } => active.clone_from(model),
            AppEvent::PromptAccepted { .. } => last_used = Some(active.clone()),
            _ => {}
        }
    }
    last_used.unwrap_or(active)
}

/// Return the latest explicit reasoning level selected for this transcript.
#[must_use]
pub fn thinking_for_resume(events: &[EventEnvelope], fallback: ThinkingLevel) -> ThinkingLevel {
    events.iter().fold(fallback, |level, envelope| {
        if let AppEvent::ThinkingLevelChanged { level } = &envelope.event {
            *level
        } else {
            level
        }
    })
}

/// Return the permission profile active at the end of this transcript.
#[must_use]
pub fn permission_for_resume(
    events: &[EventEnvelope],
    fallback: PermissionProfile,
) -> PermissionProfile {
    events
        .iter()
        .fold(fallback, |profile, envelope| match &envelope.event {
            AppEvent::SessionCreated { profile, .. }
            | AppEvent::SessionResumed { profile, .. }
            | AppEvent::PermissionProfileChanged { profile } => *profile,
            _ => profile,
        })
}

fn recovery_from_events(
    id: &SessionId,
    events: &[EventEnvelope],
    warnings: Vec<String>,
) -> RecoveryStatus {
    let mut active_turns = HashSet::new();
    let mut active_tools = HashMap::new();
    let mut background = HashMap::new();
    let mut pending_permissions = HashSet::new();
    let mut pending_questions = HashSet::new();
    let mut changed_paths = BTreeSet::new();
    for envelope in events {
        match &envelope.event {
            AppEvent::TurnStarted { turn_id } => {
                active_turns.insert(turn_id.clone());
            }
            AppEvent::TurnCompleted { turn_id }
            | AppEvent::TurnCancelled { turn_id }
            | AppEvent::ErrorRaised {
                turn_id: Some(turn_id),
                ..
            } => {
                active_turns.remove(turn_id);
            }
            AppEvent::ToolStarted { call_id, name } => {
                active_tools.insert(call_id.clone(), name.clone());
            }
            AppEvent::ToolCompleted { call_id, .. } => {
                active_tools.remove(call_id);
            }
            AppEvent::WorkspaceChanged { paths } => {
                changed_paths.extend(paths.iter().cloned());
            }
            AppEvent::BackgroundTaskChanged { task_id, state } => {
                if matches!(state.as_str(), "completed" | "cancelled")
                    || state.starts_with("failed:")
                {
                    background.remove(task_id);
                } else {
                    background.insert(task_id.clone(), state.clone());
                }
            }
            AppEvent::PermissionRequired { request_id, .. } => {
                pending_permissions.insert(request_id.clone());
            }
            AppEvent::PermissionResolved { request_id, .. } => {
                pending_permissions.remove(request_id);
            }
            AppEvent::QuestionAsked { question_id, .. } => {
                pending_questions.insert(question_id.clone());
            }
            AppEvent::QuestionAnswered { question_id, .. } => {
                pending_questions.remove(question_id);
            }
            AppEvent::QuestionsAsked { request } => {
                pending_questions.insert(request.request_id.clone());
            }
            AppEvent::QuestionsAnswered { request_id, .. }
            | AppEvent::QuestionsFailed { request_id, .. } => {
                pending_questions.remove(request_id);
            }
            _ => {}
        }
    }
    let mut interrupted_tools = active_tools
        .into_iter()
        .map(|(call_id, name)| InterruptedTool {
            call_id,
            name,
            status: "unknown_after_restart".into(),
        })
        .collect::<Vec<_>>();
    interrupted_tools.extend(
        background
            .into_iter()
            .map(|(task_id, state)| InterruptedTool {
                call_id: task_id,
                name: "background_process".into(),
                status: format!("unknown_after_restart (last state: {state})"),
            }),
    );
    interrupted_tools.extend(
        pending_permissions
            .into_iter()
            .map(|request_id| InterruptedTool {
                call_id: request_id,
                name: "permission_request".into(),
                status: "cancelled_by_restart".into(),
            }),
    );
    interrupted_tools.extend(
        pending_questions
            .into_iter()
            .map(|request_id| InterruptedTool {
                call_id: request_id,
                name: "structured_question".into(),
                status: "cancelled_by_restart".into(),
            }),
    );
    interrupted_tools.sort_by(|left, right| left.call_id.cmp(&right.call_id));
    RecoveryStatus {
        session_id: id.clone(),
        interrupted_turns: active_turns.len(),
        interrupted_tools,
        changed_paths: changed_paths.into_iter().collect(),
        warnings,
    }
}

fn bounded_diagnostic(message: String) -> String {
    if message.len() <= MAX_RECOVERY_DIAGNOSTIC_BYTES {
        return message;
    }
    let mut end = MAX_RECOVERY_DIAGNOSTIC_BYTES;
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &message[..end])
}

fn ensure_changed(id: &SessionId, changed: usize) -> Result<()> {
    if changed == 0 {
        Err(AxiomError::SessionNotFound(id.to_string()))
    } else {
        Ok(())
    }
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "matches Result::map_err's owned error signature"
)]
fn storage_error(error: rusqlite::Error) -> AxiomError {
    AxiomError::Storage(error.to_string())
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use tempfile::tempdir;

    use crate::app::{CorrelationId, Origin, PermissionProfile, Runtime, TurnId};

    use super::*;

    fn created(id: &SessionId, sequence: u64) -> EventEnvelope {
        EventEnvelope {
            schema_version: 1,
            sequence,
            occurred_at: Utc::now(),
            correlation_id: CorrelationId::new(),
            origin: Origin::Test,
            session_id: id.clone(),
            event: AppEvent::SessionCreated {
                cwd: PathBuf::from("/tmp/project"),
                origin: Origin::Test,
                profile: PermissionProfile::Confirm,
            },
        }
    }

    fn event(id: &SessionId, sequence: u64, event: AppEvent) -> EventEnvelope {
        EventEnvelope {
            schema_version: 2,
            sequence,
            occurred_at: Utc::now(),
            correlation_id: CorrelationId::new(),
            origin: Origin::Test,
            session_id: id.clone(),
            event,
        }
    }

    #[test]
    fn persists_lists_and_recovers_without_replaying() {
        let store = SessionStore::in_memory().expect("store");
        let id = SessionId::new();
        store.append(&created(&id, 1)).expect("create");
        store
            .append(&EventEnvelope {
                schema_version: 1,
                sequence: 2,
                occurred_at: Utc::now(),
                correlation_id: CorrelationId::new(),
                origin: Origin::Test,
                session_id: id.clone(),
                event: AppEvent::TurnStarted {
                    turn_id: crate::app::TurnId::new(),
                },
            })
            .expect("turn");
        assert_eq!(store.list(false).expect("list").len(), 1);
        assert_eq!(
            store
                .recovery_status(&id)
                .expect("recovery")
                .interrupted_turns,
            1
        );
        store.rename(&id, "Cherry task").expect("rename");
        store.archive(&id, true).expect("archive");
        assert!(store.list(false).expect("list").is_empty());
    }

    #[test]
    fn generated_title_only_fills_an_untitled_session() {
        let store = SessionStore::in_memory().expect("store");
        let id = SessionId::new();
        store.append(&created(&id, 1)).expect("create");

        assert!(
            store
                .set_title_if_absent(&id, "  First useful title  ")
                .expect("set initial title")
        );
        assert_eq!(
            store.summary(&id).expect("summary").title.as_deref(),
            Some("First useful title")
        );
        assert!(
            !store
                .set_title_if_absent(&id, "Late replacement")
                .expect("preserve title")
        );
        assert_eq!(
            store.summary(&id).expect("summary").title.as_deref(),
            Some("First useful title")
        );
    }

    #[test]
    fn last_used_model_preference_is_validated_and_replaced() {
        let store = SessionStore::in_memory().expect("store");
        assert_eq!(store.last_used_model().expect("empty preference"), None);
        store.set_last_used_model("glm-5-2").expect("initial model");
        assert_eq!(
            store
                .last_used_model()
                .expect("stored preference")
                .as_deref(),
            Some("glm-5-2")
        );
        store
            .set_last_used_model("grok-code")
            .expect("replacement model");
        assert_eq!(
            store
                .last_used_model()
                .expect("replaced preference")
                .as_deref(),
            Some("grok-code")
        );
        assert!(store.set_last_used_model("not a model id").is_err());
    }

    #[test]
    fn last_used_thinking_is_explicit_and_replaced() {
        let store = SessionStore::in_memory().expect("store");
        assert_eq!(store.last_used_thinking().expect("empty preference"), None);
        store
            .set_last_used_thinking(ThinkingLevel::Medium)
            .expect("initial thinking");
        assert_eq!(
            store.last_used_thinking().expect("stored preference"),
            Some(ThinkingLevel::Medium)
        );
        store
            .set_last_used_thinking(ThinkingLevel::ExtraHigh)
            .expect("replacement thinking");
        assert_eq!(
            store.last_used_thinking().expect("replaced preference"),
            Some(ThinkingLevel::ExtraHigh)
        );
    }

    #[test]
    fn explicit_session_deletion_is_atomic_and_cascades_events() {
        let store = SessionStore::in_memory().expect("store");
        let first = SessionId::new();
        let second = SessionId::new();
        store.append(&created(&first, 1)).expect("first session");
        store.append(&created(&second, 1)).expect("second session");

        assert_eq!(
            store
                .delete_sessions(std::slice::from_ref(&first))
                .expect("delete selected session"),
            1
        );
        assert!(matches!(
            store.summary(&first),
            Err(AxiomError::SessionNotFound(_))
        ));
        assert_eq!(
            store.summary(&second).expect("preserved session").id,
            second.to_string()
        );
        let remaining_events: i64 = store
            .lock()
            .expect("lock")
            .query_row(
                "SELECT COUNT(*) FROM events WHERE session_id=?1",
                [first.to_string()],
                |row| row.get(0),
            )
            .expect("count cascaded event rows");
        assert_eq!(remaining_events, 0);
    }

    #[test]
    fn resume_model_tracks_the_latest_prompt_not_an_unused_selection() {
        let id = SessionId::new();
        let first_turn = crate::app::TurnId::new();
        let second_turn = crate::app::TurnId::new();
        let mut events = vec![
            created(&id, 1),
            event(
                &id,
                2,
                AppEvent::ModelChanged {
                    model: "first-model".into(),
                },
            ),
            event(
                &id,
                3,
                AppEvent::PromptAccepted {
                    turn_id: first_turn,
                    text: "first prompt".into(),
                },
            ),
            event(
                &id,
                4,
                AppEvent::ModelChanged {
                    model: "selected-but-unused".into(),
                },
            ),
        ];
        assert_eq!(model_for_resume(&events, "fallback"), "first-model");

        events.push(event(
            &id,
            5,
            AppEvent::PromptAccepted {
                turn_id: second_turn,
                text: "second prompt".into(),
            },
        ));
        assert_eq!(model_for_resume(&events, "fallback"), "selected-but-unused");
    }

    #[test]
    fn resume_settings_keep_latest_thinking_and_permissions() {
        let id = SessionId::new();
        let events = vec![
            created(&id, 1),
            event(
                &id,
                2,
                AppEvent::ThinkingLevelChanged {
                    level: ThinkingLevel::High,
                },
            ),
            event(
                &id,
                3,
                AppEvent::PermissionProfileChanged {
                    profile: PermissionProfile::Confirm,
                },
            ),
        ];
        assert_eq!(
            thinking_for_resume(&events, ThinkingLevel::Medium),
            ThinkingLevel::High
        );
        assert_eq!(
            permission_for_resume(&events, PermissionProfile::Confirm),
            PermissionProfile::Confirm
        );
        let store = SessionStore::in_memory().expect("store");
        store.append_all(&events).expect("persist settings");
        assert_eq!(
            store.summary(&id).expect("updated summary").profile,
            "confirm"
        );
    }

    #[test]
    fn recovery_reports_interrupted_permission_requests() {
        let store = SessionStore::in_memory().expect("store");
        let id = SessionId::new();
        store.append(&created(&id, 1)).expect("create");
        store
            .append(&event(
                &id,
                2,
                AppEvent::PermissionRequired {
                    request_id: "permission-1".into(),
                    explanation: "write fixture".into(),
                    effect: serde_json::json!({"kind":"file_write","path":"fixture"}),
                    choices: vec!["allow_once".into(), "deny".into()],
                },
            ))
            .expect("permission");

        let recovery = store.recovery_status(&id).expect("recovery");
        assert_eq!(recovery.interrupted_tools.len(), 1);
        assert_eq!(recovery.interrupted_tools[0].name, "permission_request");
        assert_eq!(recovery.interrupted_tools[0].status, "cancelled_by_restart");
    }

    #[test]
    fn corruption_is_bounded_and_secrets_are_redacted() {
        let store = SessionStore::in_memory().expect("store");
        let id = SessionId::new();
        store.append(&created(&id, 1)).expect("create");
        store
            .append(&EventEnvelope {
                schema_version: 1,
                sequence: 2,
                occurred_at: Utc::now(),
                correlation_id: CorrelationId::new(),
                origin: Origin::Test,
                session_id: id.clone(),
                event: AppEvent::PromptAccepted {
                    turn_id: crate::app::TurnId::new(),
                    text: "use axm_123456789abcdef".into(),
                },
            })
            .expect("prompt");
        assert!(!store.export(&id).expect("export").contains("axm_"));
        store.corrupt_payload(&id).expect("corrupt");
        assert!(
            store
                .load(&id)
                .expect_err("corrupt error")
                .to_string()
                .len()
                < 500
        );
    }

    #[test]
    fn usage_token_counters_remain_typed_and_resumable_after_redaction() {
        let store = SessionStore::in_memory().expect("store");
        let id = SessionId::new();
        store.append(&created(&id, 1)).expect("create");
        store
            .append(&event(
                &id,
                2,
                AppEvent::UsageUpdated {
                    input_tokens: 42,
                    output_tokens: 7,
                },
            ))
            .expect("persist usage");
        let loaded = store.load(&id).expect("load live-shaped journal");
        assert!(matches!(
            loaded[1].event,
            AppEvent::UsageUpdated {
                input_tokens: 42,
                output_tokens: 7
            }
        ));
    }

    #[tokio::test]
    async fn every_event_boundary_reopens_ready_without_replaying_unknown_work() {
        let id = SessionId::new();
        let turn = TurnId::new();
        let stream = vec![
            created(&id, 1),
            event(
                &id,
                2,
                AppEvent::SecurityStatusChanged {
                    state: crate::app::SecurityStatus::UnattestedDevelopment,
                },
            ),
            event(
                &id,
                3,
                AppEvent::PromptAccepted {
                    turn_id: turn.clone(),
                    text: "repair it".into(),
                },
            ),
            event(
                &id,
                4,
                AppEvent::TurnStarted {
                    turn_id: turn.clone(),
                },
            ),
            event(
                &id,
                5,
                AppEvent::ToolProposed {
                    turn_id: turn.clone(),
                    call_id: "write-1".into(),
                    name: "apply_patch".into(),
                    arguments: serde_json::json!({"path":"src/lib.rs"}),
                },
            ),
            event(
                &id,
                6,
                AppEvent::ToolStarted {
                    call_id: "write-1".into(),
                    name: "apply_patch".into(),
                },
            ),
            event(
                &id,
                7,
                AppEvent::ToolOutput {
                    call_id: "write-1".into(),
                    content: "possibly committed".into(),
                    truncated: false,
                },
            ),
            event(
                &id,
                8,
                AppEvent::WorkspaceChanged {
                    paths: vec![PathBuf::from("src/lib.rs")],
                },
            ),
            event(
                &id,
                9,
                AppEvent::ToolCompleted {
                    call_id: "write-1".into(),
                    success: true,
                },
            ),
            event(
                &id,
                10,
                AppEvent::TurnCompleted {
                    turn_id: turn.clone(),
                },
            ),
            event(&id, 11, AppEvent::SessionClosed),
        ];

        for boundary in 1..=stream.len() {
            let directory = tempdir().expect("state");
            let path = directory.path().join("sessions.sqlite3");
            {
                let store = SessionStore::open(&path).expect("open");
                store
                    .append_all(&stream[..boundary])
                    .expect("append prefix");
            }
            let reopened = SessionStore::open(&path).expect("reopen");
            let loaded = reopened.load_recovering(&id).expect("load");
            assert!(loaded.warnings.is_empty());
            let runtime = Runtime::new(16);
            let state = runtime.restore(&loaded.events).await.expect("restore");
            assert_eq!(state.lifecycle, crate::app::SessionLifecycle::Ready);
            assert!(state.active_turn.is_none());
            let recovery = reopened.recovery_status(&id).expect("recovery");
            if (6..9).contains(&boundary) {
                assert_eq!(recovery.interrupted_tools.len(), 1);
                assert_eq!(
                    recovery.interrupted_tools[0].status,
                    "unknown_after_restart"
                );
            }
        }
    }

    #[test]
    fn v0_v1_and_v2_migrate_transactionally_and_future_schema_is_rejected() {
        let directory = tempdir().expect("state");
        let fresh = directory.path().join("fresh.sqlite3");
        let store = SessionStore::open(&fresh).expect("v0 migration");
        assert_eq!(
            store
                .lock()
                .expect("lock")
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .expect("version"),
            SCHEMA_VERSION
        );

        let v1 = directory.path().join("v1.sqlite3");
        let connection = Connection::open(&v1).expect("v1 fixture");
        connection
            .execute_batch(
                "CREATE TABLE sessions(
                   id TEXT PRIMARY KEY, title TEXT, cwd TEXT NOT NULL, origin TEXT NOT NULL,
                   profile TEXT NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
                   archived INTEGER NOT NULL DEFAULT 0
                 );
                 CREATE TABLE events(
                   session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                   sequence INTEGER NOT NULL, occurred_at TEXT NOT NULL, payload TEXT NOT NULL,
                   PRIMARY KEY(session_id, sequence)
                 );
                 CREATE INDEX idx_sessions_updated ON sessions(updated_at DESC);
                 PRAGMA user_version=1;",
            )
            .expect("v1 schema");
        drop(connection);
        let migrated = SessionStore::open(&v1).expect("v1 migration");
        assert_eq!(
            migrated
                .lock()
                .expect("lock")
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .expect("version"),
            SCHEMA_VERSION
        );

        let v2 = directory.path().join("v2.sqlite3");
        let connection = Connection::open(&v2).expect("v2 fixture");
        connection
            .execute_batch(
                "CREATE TABLE sessions(
                   id TEXT PRIMARY KEY, title TEXT, cwd TEXT NOT NULL, origin TEXT NOT NULL,
                   profile TEXT NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
                   archived INTEGER NOT NULL DEFAULT 0,
                   history_truncated INTEGER NOT NULL DEFAULT 0
                 );
                 CREATE TABLE events(
                   session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                   sequence INTEGER NOT NULL, occurred_at TEXT NOT NULL, payload TEXT NOT NULL,
                   PRIMARY KEY(session_id, sequence)
                 );
                 CREATE TABLE snapshots(
                   session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                   sequence INTEGER NOT NULL, created_at TEXT NOT NULL, payload TEXT NOT NULL,
                   PRIMARY KEY(session_id, sequence)
                 );
                 CREATE INDEX idx_sessions_updated ON sessions(updated_at DESC);
                 PRAGMA user_version=2;",
            )
            .expect("v2 schema");
        drop(connection);
        let migrated = SessionStore::open(&v2).expect("v2 migration");
        assert_eq!(
            migrated
                .lock()
                .expect("lock")
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .expect("version"),
            SCHEMA_VERSION
        );
        migrated
            .set_last_used_model("migration-model")
            .expect("new settings table");

        let broken = directory.path().join("broken-v1.sqlite3");
        let connection = Connection::open(&broken).expect("broken fixture");
        connection
            .execute_batch("PRAGMA user_version=1;")
            .expect("broken schema");
        drop(connection);
        assert!(SessionStore::open(&broken).is_err());
        let connection = Connection::open(&broken).expect("inspect rollback");
        assert_eq!(
            connection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .expect("version"),
            1
        );

        let future = directory.path().join("future.sqlite3");
        let connection = Connection::open(&future).expect("future fixture");
        connection
            .execute_batch("PRAGMA user_version=99;")
            .expect("future schema");
        drop(connection);
        assert!(SessionStore::open(&future).is_err());
    }

    #[test]
    fn snapshots_retention_exports_and_disk_redaction_are_bounded() {
        let directory = tempdir().expect("state");
        let path = directory.path().join("sessions.sqlite3");
        let store = SessionStore::open(&path).expect("store");
        let id = SessionId::new();
        let turn = TurnId::new();
        store.append(&created(&id, 1)).expect("create");
        store
            .append(&event(
                &id,
                2,
                AppEvent::PromptAccepted {
                    turn_id: turn.clone(),
                    text: "token axm_storage_test_123456789".into(),
                },
            ))
            .expect("secret prompt");
        store
            .append(&event(
                &id,
                3,
                AppEvent::TurnStarted {
                    turn_id: turn.clone(),
                },
            ))
            .expect("start");
        for sequence in 4..10 {
            store
                .append(&event(
                    &id,
                    sequence,
                    AppEvent::WarningRaised {
                        message: format!("event {sequence}"),
                    },
                ))
                .expect("event");
        }
        store
            .append(&event(&id, 10, AppEvent::TurnCompleted { turn_id: turn }))
            .expect("finish");
        assert!(store.latest_snapshot(&id).expect("snapshot").is_some());
        let export = store
            .export_with_options(
                &id,
                ExportOptions {
                    include_prompts: false,
                    include_tool_output: false,
                    max_events: 10,
                },
            )
            .expect("export");
        assert!(export.contains("omitted by export policy"));
        assert!(!export.contains("axm_storage_test"));

        let report = store
            .apply_retention(RetentionPolicy {
                max_events_per_session: 4,
                delete_archived_older_than_days: None,
            })
            .expect("retention");
        assert!(report.deleted_events > 0);
        assert!(store.summary(&id).expect("summary").history_truncated);
        assert!(store.load(&id).expect("retained").len() <= 4);
        store
            .lock()
            .expect("lock")
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .expect("checkpoint");
        let bytes = std::fs::read(path).expect("database bytes");
        assert!(!String::from_utf8_lossy(&bytes).contains("axm_storage_test"));
    }
}
