//! Collection membership, ordering and revision transactions.

use chrono::Utc;
use rusqlite::{Connection, Transaction, params};
use uuid::Uuid;

use crate::{AxiomError, Result, app::SessionId};

use super::codec::{checked_u64, storage_error};
use super::records::{Collection, CollectionChange, CollectionState};
use super::timeline::ensure_thread;
use super::{MAX_COLLECTION_NAME_BYTES, SessionStore};

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

    pub(super) fn update_collection(
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

pub(super) fn collection_state(connection: &Connection) -> Result<CollectionState> {
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

pub(super) fn bump_collection_revision(transaction: &Transaction<'_>) -> Result<()> {
    transaction
        .execute(
            "UPDATE collection_state SET revision=revision+1 WHERE id=1",
            [],
        )
        .map_err(storage_error)?;
    Ok(())
}

pub(super) fn ensure_collection_changed(id: &str, changed: usize) -> Result<()> {
    if changed == 0 {
        Err(AxiomError::Storage(format!(
            "collection {id} was not found"
        )))
    } else {
        Ok(())
    }
}

pub(super) fn validate_collection_name(name: &str) -> Result<&str> {
    let name = name.trim();
    if name.is_empty() || name.len() > MAX_COLLECTION_NAME_BYTES {
        return Err(AxiomError::Storage(format!(
            "collection name must contain 1 to {MAX_COLLECTION_NAME_BYTES} bytes"
        )));
    }
    Ok(name)
}
