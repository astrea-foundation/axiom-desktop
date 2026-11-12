//! Fresh schema initialization and structural checks.

use rusqlite::Connection;

use crate::storage_progress::{self, Observer, Phase};
use crate::{AxiomError, Result};

use super::SessionStore;
use super::codec::storage_error;

pub(super) const SCHEMA_VERSION: i64 = 7;
// Clean-slate account-scoped storage contract (`AXA3`). Installation-scoped
// pre-release databases use a different application ID and are never opened.
pub(super) const APPLICATION_ID: i64 = 0x4158_4133;
pub(crate) fn needs_maintenance(connection: &Connection) -> Result<bool> {
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(storage_error)?;
    if version < SCHEMA_VERSION {
        return Ok(true);
    }
    if version > SCHEMA_VERSION {
        return Ok(false);
    } // ordinary schema check rejects newer versions
    connection
        .query_row(
            "SELECT purge_pending FROM asset_maintenance WHERE id=1",
            [],
            |row| row.get(0),
        )
        .map_err(storage_error)
}

impl SessionStore {
    pub(super) fn initialize(&self) -> Result<()> {
        self.initialize_with_progress(&|_| {})
    }

    pub(super) fn initialize_with_progress(&self, progress: &Observer<'_>) -> Result<()> {
        let mut connection = self.lock()?;
        connection
            .execute_batch(
                "PRAGMA foreign_keys=ON; PRAGMA secure_delete=ON; PRAGMA temp_store=MEMORY;",
            )
            .map_err(storage_error)?;
        let assets = connection.assets().clone();
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let version: i64 = transaction
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(storage_error)?;
        if version > SCHEMA_VERSION {
            return Err(AxiomError::Storage(format!(
                "local state schema {version} is newer than supported schema {SCHEMA_VERSION}"
            )));
        }
        if version == 0 {
            let user_tables: i64 = transaction
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
        if version > 0 {
            let application_id: i64 = transaction
                .query_row("PRAGMA application_id", [], |row| row.get(0))
                .map_err(storage_error)?;
            if application_id != APPLICATION_ID {
                return Err(AxiomError::Storage(
                    "local state does not belong to this Axiom storage contract".into(),
                ));
            }
        }
        initialize_schema(&transaction, version)?;
        let application_id: i64 = transaction
            .query_row("PRAGMA application_id", [], |row| row.get(0))
            .map_err(storage_error)?;
        if application_id != APPLICATION_ID {
            return Err(AxiomError::Storage(
                "local state schema v1 does not belong to this Axiom storage contract; reset the development database".into(),
            ));
        }
        validate_schema(&transaction)?;
        if version < 7 {
            super::asset_migration::migrate(
                &transaction,
                &assets,
                if version > 0 { progress } else { &|_| {} },
            )?;
            if version > 0 {
                transaction
                    .execute(
                        "UPDATE asset_maintenance SET purge_pending=1 WHERE id=1",
                        [],
                    )
                    .map_err(storage_error)?;
            }
        }
        let pending: bool = transaction
            .query_row(
                "SELECT purge_pending FROM asset_maintenance WHERE id=1",
                [],
                |row| row.get(0),
            )
            .map_err(storage_error)?;
        assets.verify_and_clean_with_progress(
            &transaction,
            if pending { progress } else { &|_| {} },
        )?;
        transaction.commit().map_err(storage_error)?;
        if pending {
            storage_progress::phase(progress, Phase::Finalizing);
            // Remove historical free-page copies as well as old WAL frames.
            // The marker survives errors/crashes and forces a retry on open.
            connection.execute_batch("VACUUM;").map_err(storage_error)?;
            let busy: i64 = connection
                .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))
                .map_err(storage_error)?;
            if busy != 0 {
                return Err(AxiomError::Storage("Close other Axiom processes to finish the image storage migration; saved data was retained".into()));
            }
            connection
                .execute(
                    "UPDATE asset_maintenance SET purge_pending=0 WHERE id=1",
                    [],
                )
                .map_err(storage_error)?;
        }
        assets.collect_after_commit(&mut connection);
        connection
            .execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
            .map_err(storage_error)?;
        Ok(())
    }
}

pub(super) fn initialize_schema(connection: &Connection, version: i64) -> Result<()> {
    if version == 0 {
        connection
            .execute_batch(SCHEMA_BASELINE)
            .map_err(storage_error)?;
    }
    if version < 2 {
        connection.execute_batch("CREATE TABLE prompt_attachments (user_item_id TEXT PRIMARY KEY REFERENCES timeline_items(id) ON DELETE CASCADE, payload TEXT NOT NULL); PRAGMA user_version=2;").map_err(storage_error)?;
    }
    if version < 3 {
        connection
            .execute_batch(
                "ALTER TABLE threads ADD COLUMN title_generation_id TEXT; PRAGMA user_version=3;",
            )
            .map_err(storage_error)?;
    }
    if version < 4 {
        connection.execute_batch("CREATE TABLE desktop_mcp_state(id INTEGER PRIMARY KEY CHECK(id=1), payload TEXT NOT NULL); INSERT INTO desktop_mcp_state VALUES(1, '{\"revision\":0,\"servers\":{}}'); CREATE TABLE desktop_mcp_threads(thread_id TEXT PRIMARY KEY REFERENCES threads(id) ON DELETE CASCADE, tools TEXT NOT NULL); PRAGMA user_version=4;").map_err(storage_error)?;
    }
    if version < 5 {
        connection
            .execute_batch("ALTER TABLE turns ADD COLUMN owner_id TEXT; PRAGMA user_version=5;")
            .map_err(storage_error)?;
    }
    if version < 6 {
        connection.execute_batch("CREATE TABLE model_history_threads(thread_id TEXT PRIMARY KEY REFERENCES threads(id) ON DELETE CASCADE, version INTEGER NOT NULL); CREATE TABLE model_history(sequence INTEGER PRIMARY KEY AUTOINCREMENT, thread_id TEXT NOT NULL REFERENCES threads(id) ON DELETE CASCADE, version INTEGER NOT NULL, turn_id TEXT, occurred_at TEXT NOT NULL, origin TEXT NOT NULL, payload TEXT NOT NULL); CREATE INDEX model_history_thread ON model_history(thread_id, sequence); CREATE TABLE model_messages(id INTEGER PRIMARY KEY, thread_id TEXT NOT NULL REFERENCES threads(id) ON DELETE CASCADE, digest TEXT NOT NULL, payload TEXT NOT NULL, UNIQUE(thread_id,digest)); PRAGMA user_version=6;").map_err(storage_error)?;
    }
    if version < 7 {
        connection.execute_batch("CREATE TABLE binary_assets(id TEXT PRIMARY KEY, sha256 TEXT NOT NULL UNIQUE, byte_length INTEGER NOT NULL CHECK(byte_length>=0)); CREATE TABLE asset_owners(owner TEXT NOT NULL, asset_id TEXT NOT NULL REFERENCES binary_assets(id) ON DELETE CASCADE, PRIMARY KEY(owner,asset_id)); CREATE INDEX asset_owners_asset ON asset_owners(asset_id); CREATE TABLE local_payloads(id TEXT PRIMARY KEY, payload TEXT NOT NULL); CREATE TABLE asset_maintenance(id INTEGER PRIMARY KEY CHECK(id=1), purge_pending INTEGER NOT NULL CHECK(purge_pending IN (0,1))); INSERT INTO asset_maintenance VALUES(1,0); PRAGMA user_version=7;").map_err(storage_error)?;
    }
    Ok(())
}

pub(super) const SCHEMA_BASELINE: &str = include_str!("schema.sql");

pub(super) fn validate_schema(connection: &Connection) -> Result<()> {
    for table in [
        "threads",
        "desktop_mcp_state",
        "desktop_mcp_threads",
        "turns",
        "timeline_items",
        "prompt_attachments",
        "collections",
        "thread_collections",
        "profile_preferences",
        "collection_state",
        "request_usage",
        "model_history",
        "model_history_threads",
        "model_messages",
        "binary_assets",
        "asset_owners",
        "local_payloads",
        "asset_maintenance",
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
