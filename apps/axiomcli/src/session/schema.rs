//! Fresh schema initialization and structural checks.

use rusqlite::Connection;

use crate::{AxiomError, Result};

use super::SessionStore;
use super::codec::storage_error;

pub(super) const SCHEMA_VERSION: i64 = 3;
// Clean-slate account-scoped storage contract (`AXA3`). Installation-scoped
// pre-release databases use a different application ID and are never opened.
pub(super) const APPLICATION_ID: i64 = 0x4158_4133;
impl SessionStore {
    pub(super) fn initialize(&self) -> Result<()> {
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
        if version > 0 {
            let application_id: i64 = connection
                .query_row("PRAGMA application_id", [], |row| row.get(0))
                .map_err(storage_error)?;
            if application_id != APPLICATION_ID {
                return Err(AxiomError::Storage(
                    "local state does not belong to this Axiom storage contract".into(),
                ));
            }
        }
        initialize_schema(&mut connection, version)?;
        let application_id: i64 = connection
            .query_row("PRAGMA application_id", [], |row| row.get(0))
            .map_err(storage_error)?;
        if application_id != APPLICATION_ID {
            return Err(AxiomError::Storage(
                "local state schema v1 does not belong to this Axiom storage contract; reset the development database".into(),
            ));
        }
        validate_schema(&connection)?;
        connection
            .execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
            .map_err(storage_error)?;
        Ok(())
    }
}

pub(super) fn initialize_schema(connection: &mut Connection, version: i64) -> Result<()> {
    if version == 0 {
        let transaction = connection.transaction().map_err(storage_error)?;
        transaction
            .execute_batch(SCHEMA_BASELINE)
            .map_err(storage_error)?;
        transaction.commit().map_err(storage_error)?;
    }
    if version < 2 {
        let transaction = connection.transaction().map_err(storage_error)?;
        transaction.execute_batch("CREATE TABLE prompt_attachments (user_item_id TEXT PRIMARY KEY REFERENCES timeline_items(id) ON DELETE CASCADE, payload TEXT NOT NULL); PRAGMA user_version=2;").map_err(storage_error)?;
        transaction.commit().map_err(storage_error)?;
    }
    if version < 3 {
        let transaction = connection.transaction().map_err(storage_error)?;
        transaction
            .execute_batch(
                "ALTER TABLE threads ADD COLUMN title_generation_id TEXT; PRAGMA user_version=3;",
            )
            .map_err(storage_error)?;
        transaction.commit().map_err(storage_error)?;
    }
    Ok(())
}

pub(super) const SCHEMA_BASELINE: &str = include_str!("schema.sql");

pub(super) fn validate_schema(connection: &Connection) -> Result<()> {
    for table in [
        "threads",
        "turns",
        "timeline_items",
        "prompt_attachments",
        "collections",
        "thread_collections",
        "profile_preferences",
        "collection_state",
        "request_usage",
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
