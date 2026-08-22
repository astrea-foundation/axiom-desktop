use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
};

use chrono::Utc;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::{
    AxiomError, Result,
    app::{AppEvent, EventEnvelope, SessionId},
    audit::redact_value,
};

const SCHEMA_VERSION: i64 = 1;

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
    pub updated_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveryStatus {
    pub session_id: SessionId,
    pub interrupted_turns: usize,
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
        transaction.commit().map_err(storage_error)
    }

    pub fn append_all(&self, envelopes: &[EventEnvelope]) -> Result<()> {
        for envelope in envelopes {
            self.append(envelope)?;
        }
        Ok(())
    }

    pub fn load(&self, id: &SessionId) -> Result<Vec<EventEnvelope>> {
        let connection = self.lock()?;
        let mut statement = connection
            .prepare(
                "SELECT payload FROM events WHERE session_id=?1 ORDER BY sequence ASC LIMIT 100000",
            )
            .map_err(storage_error)?;
        let encoded = statement
            .query_map([id.to_string()], |row| row.get::<_, String>(0))
            .map_err(storage_error)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(storage_error)?;
        encoded
            .into_iter()
            .map(|payload| {
                serde_json::from_str(&payload).map_err(|error| {
                    AxiomError::Storage(format!("corrupt event in session {id}: {error}"))
                })
            })
            .collect()
    }

    pub fn list(&self, include_archived: bool) -> Result<Vec<SessionSummary>> {
        let connection = self.lock()?;
        let mut statement = connection
            .prepare(
                "SELECT id, title, cwd, origin, profile, archived, updated_at
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
                    updated_at: row.get(6)?,
                })
            })
            .map_err(storage_error)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(storage_error)
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

    pub fn export(&self, id: &SessionId) -> Result<String> {
        serde_json::to_string_pretty(&self.load(id)?).map_err(Into::into)
    }

    pub fn recovery_status(&self, id: &SessionId) -> Result<RecoveryStatus> {
        let events = self.load(id)?;
        let mut running = 0_usize;
        for envelope in events {
            match envelope.event {
                AppEvent::TurnStarted { .. } => running = running.saturating_add(1),
                AppEvent::TurnCompleted { .. }
                | AppEvent::TurnCancelled { .. }
                | AppEvent::ErrorRaised { .. } => running = running.saturating_sub(1),
                _ => {}
            }
        }
        Ok(RecoveryStatus {
            session_id: id.clone(),
            interrupted_turns: running,
        })
    }

    fn migrate(&self) -> Result<()> {
        let connection = self.lock()?;
        connection
            .execute_batch(&format!(
                "PRAGMA journal_mode=WAL;
                 PRAGMA foreign_keys=ON;
                 BEGIN IMMEDIATE;
                 CREATE TABLE IF NOT EXISTS sessions(
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
                 PRAGMA user_version={SCHEMA_VERSION};
                 COMMIT;"
            ))
            .map_err(storage_error)
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

    use crate::app::{CorrelationId, Origin, PermissionProfile};

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
}
