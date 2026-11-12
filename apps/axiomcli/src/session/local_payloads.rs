//! Account-fenced native persistence for renderer drafts and the durable outbox.
use super::{
    SessionStore,
    asset_codec::{Direction, MediaCodec},
    assets::payload_owner,
    codec::storage_error,
};
use crate::{AxiomError, Result};
use rusqlite::{OptionalExtension as _, params};
use serde_json::Value;
fn validate_id(id: &str) -> Result<()> {
    if id.is_empty() || id.len() > 2048 || id.chars().any(char::is_control) {
        return Err(AxiomError::Storage("invalid local payload ID".into()));
    }
    Ok(())
}
impl SessionStore {
    pub fn local_payload(&self, id: &str) -> Result<Option<Value>> {
        validate_id(id)?;
        let connection = self.lock()?;
        let payload: Option<String> = connection
            .query_row(
                "SELECT payload FROM local_payloads WHERE id=?1",
                [id],
                |r| r.get(0),
            )
            .optional()
            .map_err(storage_error)?;
        payload
            .map(|payload| {
                let mut value = serde_json::from_str(&payload)?;
                MediaCodec {
                    assets: connection.assets(),
                    connection: &connection,
                    owner: &payload_owner(id),
                    direction: Direction::Load,
                }
                .local_payload(&mut value)?;
                Ok(value)
            })
            .transpose()
    }
    pub fn set_local_payload(&self, id: &str, value: &Value) -> Result<()> {
        validate_id(id)?;
        let mut connection = self.lock()?;
        let assets = connection.assets().clone();
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let owner = payload_owner(id);
        let mut value = value.clone();
        // Rebuild ownership in the same transaction as the replacement. Any
        // validation/storage failure rolls back the original payload and links.
        let previous_assets = transaction
            .execute("DELETE FROM asset_owners WHERE owner=?1", [&owner])
            .map_err(storage_error)?;
        MediaCodec {
            assets: &assets,
            connection: &transaction,
            owner: &owner,
            direction: Direction::Store,
        }
        .local_payload(&mut value)?;
        transaction.execute("INSERT INTO local_payloads(id,payload) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET payload=excluded.payload",params![id,serde_json::to_string(&value)?]).map_err(storage_error)?;
        transaction.commit().map_err(storage_error)?;
        if previous_assets > 0 {
            assets.collect_after_commit(&mut connection);
        }
        Ok(())
    }
    pub fn remove_local_payload(&self, id: &str) -> Result<()> {
        validate_id(id)?;
        let mut connection = self.lock()?;
        let assets = connection.assets().clone();
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        transaction
            .execute("DELETE FROM local_payloads WHERE id=?1", [id])
            .map_err(storage_error)?;
        transaction
            .execute(
                "DELETE FROM asset_owners WHERE owner=?1",
                [payload_owner(id)],
            )
            .map_err(storage_error)?;
        transaction.commit().map_err(storage_error)?;
        assets.collect_after_commit(&mut connection);
        Ok(())
    }
}
