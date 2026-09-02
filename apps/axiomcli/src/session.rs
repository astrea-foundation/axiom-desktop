//! Durable local conversation state.
//!
//! `SQLite` stores stable thread, turn, timeline, preference, and collection
//! records. [`AppEvent`] remains the runtime vocabulary, but event envelopes
//! are translated at this boundary and are never the on-disk contract.

use std::{
    collections::{BTreeSet, HashMap},
    ffi::OsString,
    path::{Path, PathBuf},
    str::FromStr as _,
    sync::{Arc, Mutex, MutexGuard},
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension as _, Row, Transaction, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    AxiomError, Result,
    app::{
        AppEvent, CorrelationId, EventEnvelope, Origin, PermissionProfile, SessionId,
        ThinkingLevel, TurnId,
    },
    audit::redact_value,
};

const SCHEMA_VERSION: i64 = 1;
const APPLICATION_ID: i64 = 0x4158_494f;
const MAX_TIMELINE_PAGE: usize = 1_000;
const MAX_THREAD_CATALOG_PAGE: usize = 200;
const MAX_THREAD_QUERY_BYTES: usize = 512;
const MAX_TIMELINE_ITEMS_PER_LOAD: usize = 100_000;
const DEFAULT_EXPORT_ITEMS: usize = 10_000;
const MAX_CLIENT_ITEM_ID_BYTES: usize = 512;
const MAX_COLLECTION_NAME_BYTES: usize = 120;

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

macro_rules! string_enum {
    ($name:ident { $($variant:ident => $wire:literal),+ $(,)? }) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name { $($variant),+ }

        impl $name {
            const fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $wire),+ }
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str(self.as_str())
            }
        }

        impl std::str::FromStr for $name {
            type Err = AxiomError;

            fn from_str(value: &str) -> Result<Self> {
                match value {
                    $($wire => Ok(Self::$variant),)+
                    _ => Err(AxiomError::Storage(format!(
                        "invalid {} value `{value}` in local state",
                        stringify!($name)
                    ))),
                }
            }
        }
    };
}

string_enum!(ThreadLifecycle {
    Ready => "ready",
    Running => "running",
    WaitingForApproval => "waiting_for_approval",
    WaitingForAnswer => "waiting_for_answer",
    Compacting => "compacting",
    Closed => "closed",
});

string_enum!(TurnStatus {
    Running => "running",
    Completed => "completed",
    Cancelled => "cancelled",
    Failed => "failed",
    Interrupted => "interrupted",
});

string_enum!(TimelineItemKind {
    UserMessage => "user_message",
    AssistantMessage => "assistant_message",
    Reasoning => "reasoning",
    ToolCall => "tool_call",
    Plan => "plan",
    Notice => "notice",
});

string_enum!(TimelineItemStatus {
    Pending => "pending",
    InProgress => "in_progress",
    Completed => "completed",
    Cancelled => "cancelled",
    Failed => "failed",
    Interrupted => "interrupted",
});

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadRevision {
    pub revision: u64,
    pub last_timeline_sequence: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadSummary {
    pub id: String,
    pub title: Option<String>,
    pub cwd: PathBuf,
    pub origin: String,
    pub profile: String,
    pub selected_model: Option<String>,
    pub thinking_level: ThinkingLevel,
    pub lifecycle: ThreadLifecycle,
    pub archived: bool,
    pub revision: u64,
    pub last_timeline_sequence: u64,
    pub created_at: String,
    pub updated_at: String,
}

/// Compact thread projection used by the terminal session picker.
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

impl From<ThreadSummary> for SessionSummary {
    fn from(thread: ThreadSummary) -> Self {
        Self {
            id: thread.id,
            title: thread.title,
            cwd: thread.cwd,
            origin: thread.origin,
            profile: thread.profile,
            archived: thread.archived,
            updated_at: thread.updated_at,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnRecord {
    pub id: String,
    pub thread_id: String,
    pub status: TurnStatus,
    pub user_item_id: Option<String>,
    pub assistant_item_id: Option<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub error: Option<String>,
    pub started_at: String,
    pub completed_at: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TimelineItem {
    pub id: String,
    pub thread_id: String,
    pub turn_id: Option<String>,
    pub sequence: u64,
    pub kind: TimelineItemKind,
    pub status: TimelineItemStatus,
    pub client_item_id: Option<String>,
    pub external_id: Option<String>,
    pub content: String,
    pub metadata: Value,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ThreadSnapshot {
    pub thread: ThreadSummary,
    pub turns: Vec<TurnRecord>,
    pub items: Vec<TimelineItem>,
    pub next_cursor: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadCatalogPage {
    pub threads: Vec<ThreadSummary>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfilePreferences {
    pub model: Option<String>,
    pub thinking_level: ThinkingLevel,
    pub updated_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Collection {
    pub id: String,
    pub name: String,
    pub collapsed: bool,
    pub position: i64,
    pub thread_ids: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollectionState {
    pub revision: u64,
    pub collections: Vec<Collection>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollectionChange {
    pub collection_id: Option<String>,
    pub state: CollectionState,
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadOutcome {
    pub events: Vec<EventEnvelope>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Copy, Debug)]
pub struct ExportOptions {
    pub include_prompts: bool,
    pub include_tool_output: bool,
    pub max_items: usize,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            include_prompts: true,
            include_tool_output: true,
            max_items: DEFAULT_EXPORT_ITEMS,
        }
    }
}

impl SessionStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let connection = Connection::open(path).map_err(storage_error)?;
        connection
            .busy_timeout(std::time::Duration::from_secs(5))
            .map_err(storage_error)?;
        let store = Self {
            connection: Arc::new(Mutex::new(connection)),
            path: Some(path.to_path_buf()),
        };
        store.initialize()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
        store.interrupt_incomplete_turns()?;
        Ok(store)
    }

    pub fn in_memory() -> Result<Self> {
        let store = Self {
            connection: Arc::new(Mutex::new(
                Connection::open_in_memory().map_err(storage_error)?,
            )),
            path: None,
        };
        store.initialize()?;
        Ok(store)
    }

    fn initialize(&self) -> Result<()> {
        let mut connection = self.lock()?;
        connection
            .execute_batch("PRAGMA foreign_keys=ON;")
            .map_err(storage_error)?;
        let version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(storage_error)?;
        if version > SCHEMA_VERSION {
            return Err(AxiomError::Storage(format!(
                "local state schema {version} is newer than supported schema {SCHEMA_VERSION}"
            )));
        }
        if version == 0 {
            let user_tables: i64 = connection
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
                    [],
                    |row| row.get(0),
                )
                .map_err(storage_error)?;
            if user_tables != 0 {
                return Err(AxiomError::Storage(
                    "unversioned or pre-release local state is not compatible with Axiom schema v1; reset the development database".into(),
                ));
            }
        }
        migrate_schema(&mut connection, version)?;
        let application_id: i64 = connection
            .query_row("PRAGMA application_id", [], |row| row.get(0))
            .map_err(storage_error)?;
        if application_id != APPLICATION_ID {
            return Err(AxiomError::Storage(
                "local state schema v1 does not belong to this Axiom storage contract; reset the development database".into(),
            ));
        }
        validate_schema_v1(&connection)?;
        connection
            .execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
            .map_err(storage_error)?;
        Ok(())
    }

    fn interrupt_incomplete_turns(&self) -> Result<()> {
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
        if ids.is_empty() {
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

    fn lock(&self) -> Result<MutexGuard<'_, Connection>> {
        self.connection
            .lock()
            .map_err(|_| AxiomError::Storage("local state database lock was poisoned".into()))
    }
}

struct SchemaMigration {
    from: i64,
    to: i64,
    sql: &'static str,
}

const SCHEMA_MIGRATIONS: &[SchemaMigration] = &[SchemaMigration {
    from: 0,
    to: 1,
    sql: SCHEMA_V1,
}];

fn migrate_schema(connection: &mut Connection, mut version: i64) -> Result<()> {
    while version < SCHEMA_VERSION {
        let migration = SCHEMA_MIGRATIONS
            .iter()
            .find(|migration| migration.from == version)
            .ok_or_else(|| {
                AxiomError::Storage(format!(
                    "no local state migration is available from schema {version}"
                ))
            })?;
        let transaction = connection.transaction().map_err(storage_error)?;
        transaction
            .execute_batch(migration.sql)
            .map_err(storage_error)?;
        let migrated_version: i64 = transaction
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(storage_error)?;
        if migrated_version != migration.to {
            return Err(AxiomError::Storage(format!(
                "local state migration from {} did not select schema {}",
                migration.from, migration.to
            )));
        }
        transaction.commit().map_err(storage_error)?;
        version = migration.to;
    }
    Ok(())
}

const SCHEMA_V1: &str = r"
CREATE TABLE threads(
  id TEXT PRIMARY KEY,
  title TEXT,
  cwd BLOB NOT NULL,
  cwd_encoding TEXT NOT NULL,
  origin TEXT NOT NULL,
  profile TEXT NOT NULL,
  selected_model TEXT,
  thinking_level TEXT NOT NULL DEFAULT 'medium',
  lifecycle TEXT NOT NULL DEFAULT 'ready',
  archived INTEGER NOT NULL DEFAULT 0 CHECK(archived IN (0,1)),
  revision INTEGER NOT NULL DEFAULT 0 CHECK(revision >= 0),
  next_timeline_sequence INTEGER NOT NULL DEFAULT 0 CHECK(next_timeline_sequence >= 0),
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE INDEX idx_threads_activity ON threads(archived, updated_at DESC, id);

CREATE TABLE turns(
  thread_id TEXT NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
  id TEXT NOT NULL,
  status TEXT NOT NULL,
  user_item_id TEXT,
  assistant_item_id TEXT,
  input_tokens INTEGER NOT NULL DEFAULT 0 CHECK(input_tokens >= 0),
  output_tokens INTEGER NOT NULL DEFAULT 0 CHECK(output_tokens >= 0),
  error TEXT,
  started_at TEXT NOT NULL,
  completed_at TEXT,
  PRIMARY KEY(thread_id, id)
);
CREATE INDEX idx_turns_thread_started ON turns(thread_id, started_at, id);

CREATE TABLE timeline_items(
  id TEXT PRIMARY KEY,
  thread_id TEXT NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
  turn_id TEXT,
  sequence INTEGER NOT NULL CHECK(sequence > 0),
  kind TEXT NOT NULL,
  status TEXT NOT NULL,
  client_item_id TEXT,
  external_id TEXT,
  content TEXT NOT NULL DEFAULT '',
  metadata TEXT NOT NULL DEFAULT '{}',
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  UNIQUE(thread_id, sequence),
  UNIQUE(thread_id, client_item_id)
);
CREATE INDEX idx_timeline_thread_sequence ON timeline_items(thread_id, sequence);
CREATE INDEX idx_timeline_turn ON timeline_items(thread_id, turn_id, sequence);
CREATE INDEX idx_timeline_external ON timeline_items(thread_id, external_id);

CREATE TABLE collections(
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  collapsed INTEGER NOT NULL DEFAULT 0 CHECK(collapsed IN (0,1)),
  position INTEGER NOT NULL CHECK(position >= 0),
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE INDEX idx_collections_position ON collections(position, id);

CREATE TABLE thread_collections(
  thread_id TEXT PRIMARY KEY REFERENCES threads(id) ON DELETE CASCADE,
  collection_id TEXT NOT NULL REFERENCES collections(id) ON DELETE CASCADE,
  assigned_at TEXT NOT NULL
);
CREATE INDEX idx_thread_collections_collection ON thread_collections(collection_id, thread_id);

CREATE TABLE profile_preferences(
  id INTEGER PRIMARY KEY CHECK(id=1),
  selected_model TEXT,
  thinking_level TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
INSERT INTO profile_preferences(id, selected_model, thinking_level, updated_at)
VALUES(1, NULL, 'medium', strftime('%Y-%m-%dT%H:%M:%fZ','now'));

CREATE TABLE collection_state(
  id INTEGER PRIMARY KEY CHECK(id=1),
  revision INTEGER NOT NULL CHECK(revision >= 0)
);
INSERT INTO collection_state(id, revision) VALUES(1, 0);

PRAGMA application_id=1096304975;
PRAGMA user_version=1;
";

fn validate_schema_v1(connection: &Connection) -> Result<()> {
    for table in [
        "threads",
        "turns",
        "timeline_items",
        "collections",
        "thread_collections",
        "profile_preferences",
        "collection_state",
    ] {
        let exists: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
                [table],
                |row| row.get(0),
            )
            .map_err(storage_error)?;
        if !exists {
            return Err(AxiomError::Storage(format!(
                "local state schema v1 is missing required table `{table}`"
            )));
        }
    }
    Ok(())
}

impl SessionStore {
    /// Persist one runtime update and return the resulting durable thread
    /// revision. The envelope itself is never serialized.
    pub fn append(&self, envelope: &EventEnvelope) -> Result<ThreadRevision> {
        self.append_batch(std::slice::from_ref(envelope), None, None)
            .map(|(revision, _)| revision)
    }

    /// Atomically translate a runtime command's events into durable records.
    pub fn append_all(&self, envelopes: &[EventEnvelope]) -> Result<ThreadRevision> {
        self.append_batch(envelopes, None, None)
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
            self.append_batch(envelopes, None, Some((model, thinking)))?;
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
        self.append_batch(envelopes, client_item_id, None)
            .map(|(revision, _)| revision)
    }

    fn append_batch(
        &self,
        envelopes: &[EventEnvelope],
        client_item_id: Option<&str>,
        preferences: Option<(&str, ThinkingLevel)>,
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
        let mut client_item_id = client_item_id.as_deref();
        for envelope in envelopes {
            persist_event(&transaction, envelope, client_item_id)?;
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
        let revision = query_thread_revision(&transaction, &first.session_id)?;
        transaction.commit().map_err(storage_error)?;
        Ok((revision, updated_preferences))
    }

    pub fn thread_revision(&self, id: &SessionId) -> Result<ThreadRevision> {
        query_thread_revision(&*self.lock()?, id)
    }
}

fn persist_event(
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
        AppEvent::PromptAccepted { turn_id, text } => {
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
                    metadata: json!({}),
                    now: &now,
                },
            )?;
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

fn ensure_thread(transaction: &Transaction<'_>, id: &SessionId) -> Result<()> {
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

fn query_thread_revision(connection: &Connection, id: &SessionId) -> Result<ThreadRevision> {
    connection
        .query_row(
            "SELECT revision, next_timeline_sequence FROM threads WHERE id=?1",
            [id.to_string()],
            |row| {
                Ok(ThreadRevision {
                    revision: checked_u64_sql(row.get::<_, i64>(0)?)?,
                    last_timeline_sequence: checked_u64_sql(row.get::<_, i64>(1)?)?,
                })
            },
        )
        .map_err(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => AxiomError::SessionNotFound(id.to_string()),
            other => storage_error(other),
        })
}

struct TimelineItemWrite<'a> {
    turn_id: Option<&'a str>,
    kind: TimelineItemKind,
    status: TimelineItemStatus,
    client_item_id: Option<&'a str>,
    external_id: Option<&'a str>,
    content: &'a str,
    metadata: Value,
    now: &'a str,
}

fn insert_timeline_item(
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

fn upsert_timeline_item(
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

fn append_turn_content(
    transaction: &Transaction<'_>,
    thread_id: &str,
    turn_id: &str,
    kind: TimelineItemKind,
    content: &str,
    now: &str,
) -> Result<()> {
    let existing = transaction
        .query_row(
            "SELECT id FROM timeline_items
             WHERE thread_id=?1 AND turn_id=?2 AND kind=?3 ORDER BY sequence LIMIT 1",
            params![thread_id, turn_id, kind.to_string()],
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
                metadata: json!({}),
                now,
            },
        )?
        .0
    };
    transaction
        .execute(
            "UPDATE timeline_items SET content=content || ?2, status='in_progress',
               updated_at=?3 WHERE id=?1",
            params![id, content, now],
        )
        .map_err(storage_error)?;
    Ok(())
}

fn active_turn_id(transaction: &Transaction<'_>, thread_id: &str) -> Result<Option<String>> {
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

fn find_external_item(
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

fn find_any_external_item(
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

fn ensure_external_item(
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

fn item_metadata(transaction: &Transaction<'_>, id: &str) -> Result<Value> {
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

fn mark_response_verified(
    transaction: &Transaction<'_>,
    thread_id: &str,
    turn_id: &str,
    now: &str,
) -> Result<()> {
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

fn update_notice_resolution(
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

fn persist_question(
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

struct NoticeWrite<'a> {
    turn_id: Option<&'a str>,
    status: TimelineItemStatus,
    external_id: Option<&'a str>,
    code: &'a str,
    content: &'a str,
    details: Value,
    now: &'a str,
}

fn insert_notice(
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

fn finish_turn(
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

fn set_thread_lifecycle(
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

fn origin_name(origin: Origin) -> &'static str {
    match origin {
        Origin::Tui => "tui",
        Origin::Acp => "acp",
        Origin::Headless => "headless",
        Origin::Test => "test",
    }
}

fn origin_from_name(value: &str) -> Result<Origin> {
    match value {
        "tui" => Ok(Origin::Tui),
        "acp" => Ok(Origin::Acp),
        "headless" => Ok(Origin::Headless),
        "test" => Ok(Origin::Test),
        _ => Err(AxiomError::Storage(format!(
            "invalid origin `{value}` in local state"
        ))),
    }
}

fn validate_model(model: &str) -> Result<&str> {
    let model = model.trim();
    if model.is_empty() || model.len() > 256 || model.chars().any(char::is_whitespace) {
        return Err(AxiomError::Storage(
            "model must be a non-empty model ID without whitespace".into(),
        ));
    }
    Ok(model)
}

fn validate_client_item_id(value: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > MAX_CLIENT_ITEM_ID_BYTES
        || value.chars().any(char::is_whitespace)
    {
        return Err(AxiomError::Storage(format!(
            "client item ID must contain 1 to {MAX_CLIENT_ITEM_ID_BYTES} non-whitespace bytes"
        )));
    }
    Ok(value.into())
}

fn redact_text(value: &str) -> String {
    let mut value = Value::String(value.into());
    redact_value(&mut value);
    value.as_str().unwrap_or_default().to_owned()
}

fn redacted_json(value: &impl Serialize) -> Result<Value> {
    let mut value = serde_json::to_value(value)?;
    redact_value(&mut value);
    Ok(value)
}

fn encode_metadata(mut metadata: Value) -> Result<String> {
    redact_value(&mut metadata);
    serde_json::to_string(&metadata).map_err(Into::into)
}

fn checked_i64(value: u64, label: &str) -> Result<i64> {
    i64::try_from(value)
        .map_err(|_| AxiomError::Storage(format!("{label} exceeds SQLite integer range")))
}

fn checked_u64(value: i64, label: &str) -> Result<u64> {
    u64::try_from(value).map_err(|_| AxiomError::Storage(format!("{label} is negative")))
}

fn checked_u64_sql(value: i64) -> rusqlite::Result<u64> {
    u64::try_from(value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Integer,
            Box::new(error),
        )
    })
}

#[derive(Debug)]
struct StoredThread {
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
}

impl StoredThread {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
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
        })
    }

    fn decode(self) -> Result<ThreadSummary> {
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
        })
    }
}

const THREAD_COLUMNS: &str = "id, title, cwd, cwd_encoding, origin, profile, selected_model, thinking_level, lifecycle, \
     archived, revision, next_timeline_sequence, created_at, updated_at";

fn query_thread_summary(connection: &Connection, id: &SessionId) -> Result<ThreadSummary> {
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
               AND (?3 IS NULL OR updated_at < ?3 OR (updated_at = ?3 AND id > ?4))
             ORDER BY updated_at DESC, id ASC LIMIT ?5"
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
                .map(|thread| encode_thread_cursor(&thread.updated_at, &thread.id))
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
             WHERE archived=0 OR ?1=1 ORDER BY updated_at DESC, id LIMIT 1000"
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

    fn update_thread_metadata(
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

    pub fn profile_preferences(&self) -> Result<ProfilePreferences> {
        let connection = self.lock()?;
        query_profile_preferences(&connection)
    }

    pub fn set_profile_preferences(
        &self,
        model: &str,
        thinking_level: ThinkingLevel,
    ) -> Result<ProfilePreferences> {
        let model = validate_model(model)?;
        let connection = self.lock()?;
        connection
            .execute(
                "UPDATE profile_preferences
                 SET selected_model=?1, thinking_level=?2, updated_at=?3 WHERE id=1",
                params![model, thinking_level.to_string(), Utc::now().to_rfc3339()],
            )
            .map_err(storage_error)?;
        query_profile_preferences(&connection)
    }

    pub fn last_used_model(&self) -> Result<Option<String>> {
        Ok(self.profile_preferences()?.model)
    }

    pub fn set_last_used_model(&self, model: &str) -> Result<()> {
        let model = validate_model(model)?;
        self.lock()?
            .execute(
                "UPDATE profile_preferences SET selected_model=?1, updated_at=?2 WHERE id=1",
                params![model, Utc::now().to_rfc3339()],
            )
            .map_err(storage_error)?;
        Ok(())
    }

    pub fn last_used_thinking(&self) -> Result<Option<ThinkingLevel>> {
        Ok(Some(self.profile_preferences()?.thinking_level))
    }

    pub fn set_last_used_thinking(&self, level: ThinkingLevel) -> Result<()> {
        self.lock()?
            .execute(
                "UPDATE profile_preferences SET thinking_level=?1, updated_at=?2 WHERE id=1",
                params![level.to_string(), Utc::now().to_rfc3339()],
            )
            .map_err(storage_error)?;
        Ok(())
    }
}

fn query_profile_preferences(connection: &Connection) -> Result<ProfilePreferences> {
    let (model, thinking, updated_at): (Option<String>, String, String) = connection
        .query_row(
            "SELECT selected_model, thinking_level, updated_at
             FROM profile_preferences WHERE id=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(storage_error)?;
    Ok(ProfilePreferences {
        model,
        thinking_level: ThinkingLevel::from_str(&thinking).map_err(|_| {
            AxiomError::Storage(format!(
                "invalid profile thinking level `{thinking}` in local state"
            ))
        })?,
        updated_at,
    })
}

#[derive(Debug)]
struct StoredTimelineItem {
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
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
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

    fn decode(self) -> Result<TimelineItem> {
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

fn query_turns(connection: &Connection, id: &SessionId) -> Result<Vec<TurnRecord>> {
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

fn ensure_thread_exists_connection(connection: &Connection, id: &SessionId) -> Result<()> {
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

fn validate_title(title: &str) -> Result<&str> {
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

fn encode_thread_cursor(updated_at: &str, id: &str) -> Result<String> {
    let encoded = serde_json::to_vec(&(updated_at, id))?;
    Ok(URL_SAFE_NO_PAD.encode(encoded))
}

fn decode_thread_cursor(cursor: &str) -> Result<(String, String)> {
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
    if updated_at.is_empty() || id.is_empty() {
        return Err(AxiomError::Storage(
            "thread catalog cursor is invalid".into(),
        ));
    }
    Ok((updated_at, id))
}

impl SessionStore {
    pub fn list_collections(&self) -> Result<CollectionState> {
        let connection = self.lock()?;
        collection_state(&connection)
    }

    pub fn create_collection(&self, name: &str) -> Result<CollectionChange> {
        let name = validate_collection_name(name)?;
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        let mut connection = self.lock()?;
        let transaction = connection.transaction().map_err(storage_error)?;
        let position: i64 = transaction
            .query_row(
                "SELECT COALESCE(MAX(position), -1) + 1 FROM collections",
                [],
                |row| row.get(0),
            )
            .map_err(storage_error)?;
        transaction
            .execute(
                "INSERT INTO collections(id, name, collapsed, position, created_at, updated_at)
                 VALUES (?1, ?2, 0, ?3, ?4, ?4)",
                params![id, name, position, now],
            )
            .map_err(storage_error)?;
        bump_collection_revision(&transaction)?;
        let state = collection_state(&transaction)?;
        transaction.commit().map_err(storage_error)?;
        Ok(CollectionChange {
            collection_id: Some(id),
            state,
        })
    }

    pub fn rename_collection(&self, id: &str, name: &str) -> Result<CollectionChange> {
        let name = validate_collection_name(name)?;
        self.update_collection(
            id,
            "UPDATE collections SET name=?2, updated_at=?3 WHERE id=?1",
            name,
        )
    }

    pub fn set_collection_collapsed(&self, id: &str, collapsed: bool) -> Result<CollectionChange> {
        let now = Utc::now().to_rfc3339();
        let mut connection = self.lock()?;
        let transaction = connection.transaction().map_err(storage_error)?;
        let changed = transaction
            .execute(
                "UPDATE collections SET collapsed=?2, updated_at=?3 WHERE id=?1",
                params![id, collapsed, now],
            )
            .map_err(storage_error)?;
        ensure_collection_changed(id, changed)?;
        bump_collection_revision(&transaction)?;
        let state = collection_state(&transaction)?;
        transaction.commit().map_err(storage_error)?;
        Ok(CollectionChange {
            collection_id: Some(id.into()),
            state,
        })
    }

    pub fn move_collection(&self, id: &str, position: i64) -> Result<CollectionChange> {
        let position = usize::try_from(position)
            .map_err(|_| AxiomError::Storage("collection position cannot be negative".into()))?;
        let now = Utc::now().to_rfc3339();
        let mut connection = self.lock()?;
        let transaction = connection.transaction().map_err(storage_error)?;
        let mut ids = {
            let mut statement = transaction
                .prepare("SELECT id FROM collections ORDER BY position, id")
                .map_err(storage_error)?;
            statement
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(storage_error)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(storage_error)?
        };
        let current = ids
            .iter()
            .position(|collection_id| collection_id == id)
            .ok_or_else(|| AxiomError::Storage(format!("collection {id} was not found")))?;
        if position >= ids.len() {
            return Err(AxiomError::Storage(format!(
                "collection position must be between 0 and {}",
                ids.len().saturating_sub(1)
            )));
        }
        let moved = ids.remove(current);
        ids.insert(position, moved);
        for (position, collection_id) in ids.iter().enumerate() {
            transaction
                .execute(
                    "UPDATE collections SET position=?2, updated_at=?3 WHERE id=?1",
                    params![
                        collection_id,
                        i64::try_from(position).unwrap_or(i64::MAX),
                        now
                    ],
                )
                .map_err(storage_error)?;
        }
        bump_collection_revision(&transaction)?;
        let state = collection_state(&transaction)?;
        transaction.commit().map_err(storage_error)?;
        Ok(CollectionChange {
            collection_id: Some(id.into()),
            state,
        })
    }

    pub fn delete_collection(&self, id: &str) -> Result<CollectionChange> {
        let now = Utc::now().to_rfc3339();
        let mut connection = self.lock()?;
        let transaction = connection.transaction().map_err(storage_error)?;
        let assigned = {
            let mut statement = transaction
                .prepare("SELECT thread_id FROM thread_collections WHERE collection_id=?1")
                .map_err(storage_error)?;
            statement
                .query_map([id], |row| row.get::<_, String>(0))
                .map_err(storage_error)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(storage_error)?
        };
        let changed = transaction
            .execute("DELETE FROM collections WHERE id=?1", [id])
            .map_err(storage_error)?;
        ensure_collection_changed(id, changed)?;
        let remaining = {
            let mut statement = transaction
                .prepare("SELECT id FROM collections ORDER BY position, id")
                .map_err(storage_error)?;
            statement
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(storage_error)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(storage_error)?
        };
        for (position, collection_id) in remaining.iter().enumerate() {
            transaction
                .execute(
                    "UPDATE collections SET position=?2, updated_at=?3 WHERE id=?1",
                    params![
                        collection_id,
                        i64::try_from(position).unwrap_or(i64::MAX),
                        now
                    ],
                )
                .map_err(storage_error)?;
        }
        for thread_id in assigned {
            transaction
                .execute(
                    "UPDATE threads SET revision=revision+1, updated_at=?2 WHERE id=?1",
                    params![thread_id, now],
                )
                .map_err(storage_error)?;
        }
        bump_collection_revision(&transaction)?;
        let state = collection_state(&transaction)?;
        transaction.commit().map_err(storage_error)?;
        Ok(CollectionChange {
            collection_id: Some(id.into()),
            state,
        })
    }

    pub fn assign_thread_collection(
        &self,
        thread_id: &SessionId,
        collection_id: Option<&str>,
    ) -> Result<CollectionChange> {
        let now = Utc::now().to_rfc3339();
        let mut connection = self.lock()?;
        let transaction = connection.transaction().map_err(storage_error)?;
        ensure_thread(&transaction, thread_id)?;
        if let Some(collection_id) = collection_id {
            let exists: bool = transaction
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM collections WHERE id=?1)",
                    [collection_id],
                    |row| row.get(0),
                )
                .map_err(storage_error)?;
            if !exists {
                return Err(AxiomError::Storage(format!(
                    "collection {collection_id} was not found"
                )));
            }
            transaction
                .execute(
                    "INSERT INTO thread_collections(thread_id, collection_id, assigned_at)
                     VALUES (?1, ?2, ?3)
                     ON CONFLICT(thread_id) DO UPDATE SET
                       collection_id=excluded.collection_id, assigned_at=excluded.assigned_at",
                    params![thread_id.to_string(), collection_id, now],
                )
                .map_err(storage_error)?;
        } else {
            transaction
                .execute(
                    "DELETE FROM thread_collections WHERE thread_id=?1",
                    [thread_id.to_string()],
                )
                .map_err(storage_error)?;
        }
        transaction
            .execute(
                "UPDATE threads SET revision=revision+1, updated_at=?2 WHERE id=?1",
                params![thread_id.to_string(), now],
            )
            .map_err(storage_error)?;
        bump_collection_revision(&transaction)?;
        let state = collection_state(&transaction)?;
        transaction.commit().map_err(storage_error)?;
        Ok(CollectionChange {
            collection_id: collection_id.map(str::to_owned),
            state,
        })
    }

    fn update_collection(
        &self,
        id: &str,
        statement: &str,
        value: &str,
    ) -> Result<CollectionChange> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction().map_err(storage_error)?;
        let changed = transaction
            .execute(statement, params![id, value, Utc::now().to_rfc3339()])
            .map_err(storage_error)?;
        ensure_collection_changed(id, changed)?;
        bump_collection_revision(&transaction)?;
        let state = collection_state(&transaction)?;
        transaction.commit().map_err(storage_error)?;
        Ok(CollectionChange {
            collection_id: Some(id.into()),
            state,
        })
    }
}

fn collection_state(connection: &Connection) -> Result<CollectionState> {
    let revision: i64 = connection
        .query_row(
            "SELECT revision FROM collection_state WHERE id=1",
            [],
            |row| row.get(0),
        )
        .map_err(storage_error)?;
    let mut statement = connection
        .prepare(
            "SELECT id, name, collapsed, position, created_at, updated_at
             FROM collections ORDER BY position, id",
        )
        .map_err(storage_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, bool>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        })
        .map_err(storage_error)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(storage_error)?;
    let mut collections = Vec::with_capacity(rows.len());
    for (id, name, collapsed, position, created_at, updated_at) in rows {
        let mut assignments = connection
            .prepare(
                "SELECT thread_id FROM thread_collections
                 WHERE collection_id=?1 ORDER BY assigned_at, thread_id",
            )
            .map_err(storage_error)?;
        let thread_ids = assignments
            .query_map([&id], |row| row.get::<_, String>(0))
            .map_err(storage_error)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(storage_error)?;
        collections.push(Collection {
            id,
            name,
            collapsed,
            position,
            thread_ids,
            created_at,
            updated_at,
        });
    }
    Ok(CollectionState {
        revision: checked_u64(revision, "collection revision")?,
        collections,
    })
}

fn bump_collection_revision(transaction: &Transaction<'_>) -> Result<()> {
    transaction
        .execute(
            "UPDATE collection_state SET revision=revision+1 WHERE id=1",
            [],
        )
        .map_err(storage_error)?;
    Ok(())
}

fn ensure_collection_changed(id: &str, changed: usize) -> Result<()> {
    if changed == 0 {
        Err(AxiomError::Storage(format!(
            "collection {id} was not found"
        )))
    } else {
        Ok(())
    }
}

fn validate_collection_name(name: &str) -> Result<&str> {
    let name = name.trim();
    if name.is_empty() || name.len() > MAX_COLLECTION_NAME_BYTES {
        return Err(AxiomError::Storage(format!(
            "collection name must contain 1 to {MAX_COLLECTION_NAME_BYTES} bytes"
        )));
    }
    Ok(name)
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
        drop(statement);
        drop(connection);
        let events = project_runtime_events(id, &thread, &turns, &items)?;
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

fn project_runtime_events(
    id: &SessionId,
    thread: &ThreadSummary,
    turns: &[TurnRecord],
    items: &[TimelineItem],
) -> Result<Vec<EventEnvelope>> {
    let origin = origin_from_name(&thread.origin)?;
    let profile = PermissionProfile::from_str(&thread.profile).map_err(|_| {
        AxiomError::Storage(format!(
            "invalid permission profile `{}` in local state",
            thread.profile
        ))
    })?;
    let mut projection = RuntimeProjection::new(id.clone(), origin);
    projection.push(
        parse_time(&thread.created_at),
        AppEvent::SessionCreated {
            cwd: thread.cwd.clone(),
            origin,
            profile,
        },
    );
    if let Some(model) = &thread.selected_model {
        projection.push(
            parse_time(&thread.created_at),
            AppEvent::ModelChanged {
                model: model.clone(),
            },
        );
    }
    projection.push(
        parse_time(&thread.created_at),
        AppEvent::ThinkingLevelChanged {
            level: thread.thinking_level,
        },
    );

    let turns = turns
        .iter()
        .map(|turn| (turn.id.as_str(), turn))
        .collect::<HashMap<_, _>>();
    let mut active_turn: Option<&str> = None;
    for item in items {
        if item.kind == TimelineItemKind::UserMessage {
            if let Some(previous) = active_turn.take()
                && Some(previous) != item.turn_id.as_deref()
            {
                project_turn_terminal(&mut projection, turns.get(previous).copied());
            }
            let turn_id = item.turn_id.as_deref().ok_or_else(|| {
                AxiomError::Storage(format!("user timeline item {} has no turn", item.id))
            })?;
            let parsed_turn = TurnId::from_str(turn_id).map_err(|error| {
                AxiomError::Storage(format!("invalid stored turn ID `{turn_id}`: {error}"))
            })?;
            projection.push(
                parse_time(&item.created_at),
                AppEvent::PromptAccepted {
                    turn_id: parsed_turn.clone(),
                    text: item.content.clone(),
                },
            );
            projection.push(
                parse_time(&item.created_at),
                AppEvent::TurnStarted {
                    turn_id: parsed_turn,
                },
            );
            active_turn = Some(turn_id);
            continue;
        }
        let Some(turn_id) = item
            .turn_id
            .as_deref()
            .map(|value| {
                TurnId::from_str(value).map_err(|error| {
                    AxiomError::Storage(format!("invalid stored turn ID `{value}`: {error}"))
                })
            })
            .transpose()?
        else {
            project_thread_item(&mut projection, item)?;
            continue;
        };
        match item.kind {
            TimelineItemKind::AssistantMessage if !item.content.is_empty() => projection.push(
                parse_time(&item.updated_at),
                AppEvent::TextDelta {
                    turn_id,
                    text: item.content.clone(),
                },
            ),
            TimelineItemKind::Reasoning if !item.content.is_empty() => projection.push(
                parse_time(&item.updated_at),
                AppEvent::ReasoningDelta {
                    turn_id,
                    text: item.content.clone(),
                },
            ),
            TimelineItemKind::ToolCall => project_tool(&mut projection, item, turn_id),
            TimelineItemKind::Plan => project_plan(&mut projection, item)?,
            TimelineItemKind::Notice => project_notice(&mut projection, item),
            TimelineItemKind::UserMessage
            | TimelineItemKind::AssistantMessage
            | TimelineItemKind::Reasoning => {}
        }
    }
    if let Some(turn_id) = active_turn {
        project_turn_terminal(&mut projection, turns.get(turn_id).copied());
    }
    Ok(projection.events)
}

struct RuntimeProjection {
    id: SessionId,
    origin: Origin,
    sequence: u64,
    events: Vec<EventEnvelope>,
}

impl RuntimeProjection {
    fn new(id: SessionId, origin: Origin) -> Self {
        Self {
            id,
            origin,
            sequence: 0,
            events: Vec::new(),
        }
    }

    fn push(&mut self, occurred_at: DateTime<Utc>, event: AppEvent) {
        self.sequence = self.sequence.saturating_add(1);
        self.events.push(EventEnvelope {
            schema_version: 1,
            sequence: self.sequence,
            occurred_at,
            correlation_id: CorrelationId::new(),
            origin: self.origin,
            session_id: self.id.clone(),
            event,
        });
    }
}

fn project_turn_terminal(projection: &mut RuntimeProjection, turn: Option<&TurnRecord>) {
    let Some(turn) = turn else { return };
    let Ok(turn_id) = TurnId::from_str(&turn.id) else {
        return;
    };
    let occurred_at = turn
        .completed_at
        .as_deref()
        .map_or_else(|| parse_time(&turn.started_at), parse_time);
    if turn.input_tokens > 0 || turn.output_tokens > 0 {
        projection.push(
            occurred_at,
            AppEvent::UsageUpdated {
                input_tokens: turn.input_tokens,
                output_tokens: turn.output_tokens,
            },
        );
    }
    let event = match turn.status {
        TurnStatus::Completed => AppEvent::TurnCompleted { turn_id },
        TurnStatus::Cancelled => AppEvent::TurnCancelled { turn_id },
        TurnStatus::Failed => AppEvent::ErrorRaised {
            turn_id: Some(turn_id),
            message: turn.error.clone().unwrap_or_else(|| "turn failed".into()),
        },
        TurnStatus::Interrupted | TurnStatus::Running => AppEvent::ErrorRaised {
            turn_id: Some(turn_id),
            message: turn.error.clone().unwrap_or_else(|| {
                "The previous process ended during this turn; unfinished work was not replayed"
                    .into()
            }),
        },
    };
    projection.push(occurred_at, event);
}

fn project_tool(projection: &mut RuntimeProjection, item: &TimelineItem, turn_id: TurnId) {
    let call_id = item.external_id.clone().unwrap_or_else(|| item.id.clone());
    let name = item
        .metadata
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("tool")
        .to_owned();
    let arguments = item
        .metadata
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    projection.push(
        parse_time(&item.created_at),
        AppEvent::ToolProposed {
            turn_id,
            call_id: call_id.clone(),
            name: name.clone(),
            arguments,
        },
    );
    if item.status != TimelineItemStatus::Pending {
        projection.push(
            parse_time(&item.created_at),
            AppEvent::ToolStarted {
                call_id: call_id.clone(),
                name,
            },
        );
    }
    if !item.content.is_empty() {
        projection.push(
            parse_time(&item.updated_at),
            AppEvent::ToolOutput {
                call_id: call_id.clone(),
                content: item.content.clone(),
                truncated: item
                    .metadata
                    .get("output_truncated")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            },
        );
    }
    if let Some(diff) = item.metadata.get("diff").and_then(Value::as_str) {
        projection.push(
            parse_time(&item.updated_at),
            AppEvent::DiffAvailable {
                call_id: call_id.clone(),
                diff: diff.into(),
                truncated: item
                    .metadata
                    .get("diff_truncated")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                files: serde_json::from_value(
                    item.metadata
                        .get("files")
                        .cloned()
                        .unwrap_or_else(|| json!([])),
                )
                .unwrap_or_default(),
            },
        );
    }
    if matches!(
        item.status,
        TimelineItemStatus::Completed | TimelineItemStatus::Failed
    ) {
        projection.push(
            parse_time(&item.updated_at),
            AppEvent::ToolCompleted {
                call_id,
                success: item.status == TimelineItemStatus::Completed,
            },
        );
    }
}

fn project_plan(projection: &mut RuntimeProjection, item: &TimelineItem) -> Result<()> {
    if item.metadata.get("code").and_then(Value::as_str) == Some("task_list") {
        if let Some(items) = item.metadata.get("items") {
            projection.push(
                parse_time(&item.updated_at),
                AppEvent::TaskListUpdated {
                    items: serde_json::from_value(items.clone()).map_err(|error| {
                        AxiomError::Storage(format!("invalid stored task list: {error}"))
                    })?,
                },
            );
        }
        return Ok(());
    }
    let Some(plan_id) = item.external_id.clone() else {
        return Ok(());
    };
    let revision = item
        .metadata
        .get("revision")
        .and_then(Value::as_u64)
        .unwrap_or(1);
    projection.push(
        parse_time(&item.created_at),
        AppEvent::PlanProposed {
            plan_id: plan_id.clone(),
            revision,
            markdown: item.content.clone(),
        },
    );
    if let Some(decision) = item.metadata.get("decision").and_then(Value::as_str) {
        projection.push(
            parse_time(&item.updated_at),
            AppEvent::PlanReviewed {
                plan_id,
                revision,
                decision: decision.into(),
            },
        );
    }
    Ok(())
}

fn project_thread_item(projection: &mut RuntimeProjection, item: &TimelineItem) -> Result<()> {
    match item.kind {
        TimelineItemKind::Plan => project_plan(projection, item),
        TimelineItemKind::Notice => {
            project_notice(projection, item);
            Ok(())
        }
        _ => Ok(()),
    }
}

fn project_notice(projection: &mut RuntimeProjection, item: &TimelineItem) {
    let code = item
        .metadata
        .get("code")
        .and_then(Value::as_str)
        .or_else(|| {
            item.metadata
                .get("details")
                .and_then(|details| details.get("code"))
                .and_then(Value::as_str)
        })
        .unwrap_or("notice");
    let details = item.metadata.get("details").unwrap_or(&item.metadata);
    let event = match code {
        "progress" => Some(AppEvent::ProgressUpdated {
            message: item.content.clone(),
            completed: item.metadata.get("completed").and_then(Value::as_u64),
            total: item.metadata.get("total").and_then(Value::as_u64),
        }),
        "workspace_changed" => Some(AppEvent::WorkspaceChanged {
            paths: serde_json::from_value(
                details.get("paths").cloned().unwrap_or_else(|| json!([])),
            )
            .unwrap_or_default(),
        }),
        "context_compacted" => Some(AppEvent::ContextCompacted {
            summary: item.content.clone(),
            messages_before: details
                .get("messages_before")
                .and_then(Value::as_u64)
                .and_then(|value| usize::try_from(value).ok())
                .unwrap_or(0),
        }),
        "warning" => Some(AppEvent::WarningRaised {
            message: item.content.clone(),
        }),
        "error"
            if !details
                .get("terminal")
                .and_then(Value::as_bool)
                .unwrap_or(false) =>
        {
            Some(AppEvent::ErrorRaised {
                turn_id: None,
                message: item.content.clone(),
            })
        }
        _ => None,
    };
    if let Some(event) = event {
        projection.push(parse_time(&item.updated_at), event);
    }
}

fn query_changed_paths(connection: &Connection, id: &SessionId) -> Result<Vec<PathBuf>> {
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

fn parse_time(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .map_or_else(|_| Utc::now(), |value| value.with_timezone(&Utc))
}

/// Return the model active for the latest submitted prompt. This projection
/// keeps the runtime restoration API independent from the `SQLite` schema.
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

#[cfg(unix)]
#[expect(
    clippy::unnecessary_wraps,
    reason = "the fallback platform implementation can reject non-Unicode paths"
)]
fn encode_path(path: &Path) -> Result<(String, Vec<u8>)> {
    use std::os::unix::ffi::OsStrExt as _;
    Ok(("unix_bytes".into(), path.as_os_str().as_bytes().to_vec()))
}

#[cfg(unix)]
fn decode_path(encoding: &str, bytes: &[u8]) -> Result<PathBuf> {
    use std::os::unix::ffi::OsStringExt as _;
    if encoding != "unix_bytes" {
        return Err(AxiomError::Storage(format!(
            "workspace path encoding `{encoding}` belongs to another platform"
        )));
    }
    Ok(PathBuf::from(OsString::from_vec(bytes.to_vec())))
}

#[cfg(windows)]
#[expect(
    clippy::unnecessary_wraps,
    reason = "the fallback platform implementation can reject non-Unicode paths"
)]
fn encode_path(path: &Path) -> Result<(String, Vec<u8>)> {
    use std::os::windows::ffi::OsStrExt as _;
    let bytes = path
        .as_os_str()
        .encode_wide()
        .flat_map(u16::to_le_bytes)
        .collect();
    Ok(("windows_wtf16le".into(), bytes))
}

#[cfg(windows)]
fn decode_path(encoding: &str, bytes: &[u8]) -> Result<PathBuf> {
    use std::os::windows::ffi::OsStringExt as _;
    if encoding != "windows_wtf16le" || !bytes.len().is_multiple_of(2) {
        return Err(AxiomError::Storage(format!(
            "invalid workspace path encoding `{encoding}`"
        )));
    }
    let wide = bytes
        .chunks_exact(2)
        .map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]]))
        .collect::<Vec<_>>();
    Ok(PathBuf::from(OsString::from_wide(&wide)))
}

#[cfg(not(any(unix, windows)))]
fn encode_path(path: &Path) -> Result<(String, Vec<u8>)> {
    let value = path.to_str().ok_or_else(|| {
        AxiomError::Storage("workspace path is not Unicode on this platform".into())
    })?;
    Ok(("utf8".into(), value.as_bytes().to_vec()))
}

#[cfg(not(any(unix, windows)))]
fn decode_path(encoding: &str, bytes: &[u8]) -> Result<PathBuf> {
    if encoding != "utf8" {
        return Err(AxiomError::Storage(format!(
            "invalid workspace path encoding `{encoding}`"
        )));
    }
    let value = std::str::from_utf8(bytes)
        .map_err(|error| AxiomError::Storage(format!("invalid stored workspace path: {error}")))?;
    Ok(PathBuf::from(value))
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
    use tempfile::tempdir;

    use super::*;

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
    fn fresh_and_current_databases_use_the_ordered_schema_migrations() {
        let directory = tempdir().expect("temp dir");
        let fresh = directory.path().join("state.sqlite3");
        let store = SessionStore::open(&fresh).expect("fresh store");
        let connection = store.lock().expect("lock");
        assert_eq!(
            connection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .expect("version"),
            1
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

        let future = directory.path().join("future.sqlite3");
        Connection::open(&future)
            .expect("future db")
            .execute_batch("PRAGMA user_version=2;")
            .expect("future schema");
        assert!(SessionStore::open(&future).is_err());
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
    fn reopening_marks_nonterminal_turns_and_items_interrupted() {
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
                        turn_id: turn,
                        text: "partial".into(),
                    },
                ))
                .expect("partial output");
        }
        let reopened = SessionStore::open(&path).expect("reopen");
        let snapshot = reopened.thread_snapshot(&id, None, 10).expect("snapshot");
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
}
