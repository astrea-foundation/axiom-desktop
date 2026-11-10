//! Per-thread request usage persistence and reconciliation.

use rusqlite::{OptionalExtension as _, params};

use crate::{AxiomError, Result, app::SessionId};

use super::SessionStore;
use super::codec::storage_error;
use super::timeline::ensure_thread;

impl SessionStore {
    pub fn pending_accounting_ids(&self, id: &SessionId) -> Result<Vec<String>> {
        let connection = self.lock()?;
        let mut statement = connection
            .prepare(
                "SELECT request_id FROM request_usage WHERE thread_id=?1
             AND json_extract(record, '$.settled')=0
             AND json_extract(record, '$.state')!='running' ORDER BY request_id LIMIT 100",
            )
            .map_err(storage_error)?;
        statement
            .query_map([id.to_string()], |row| row.get(0))
            .map_err(storage_error)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(storage_error)
    }

    /// Persist one runtime update and return the resulting durable thread
    /// revision. The envelope itself is never serialized.
    pub fn record_request_usage(
        &self,
        id: &SessionId,
        usage: &axiom_inference::RequestUsage,
    ) -> Result<()> {
        if !usage.validate_counters() {
            return Err(AxiomError::Storage("invalid request accounting".into()));
        }
        let mut connection = self.lock()?;
        let tx = connection.transaction().map_err(storage_error)?;
        ensure_thread(&tx, id)?;
        tx.execute("INSERT INTO request_usage(request_id,thread_id,record) VALUES (?1,?2,?3)
            ON CONFLICT(request_id) DO UPDATE SET record=excluded.record WHERE request_usage.thread_id=excluded.thread_id",
            params![usage.request_id,id.to_string(),serde_json::to_string(usage)?]).map_err(storage_error)?;
        tx.execute(
            "UPDATE threads SET revision=revision+1 WHERE id=?1",
            [id.to_string()],
        )
        .map_err(storage_error)?;
        tx.commit().map_err(storage_error)
    }

    pub fn reconcile_request_usage(
        &self,
        id: &SessionId,
        snapshots: &[axiom_inference::RequestUsage],
    ) -> Result<()> {
        let mut connection = self.lock()?;
        let tx = connection.transaction().map_err(storage_error)?;
        ensure_thread(&tx, id)?;
        for snapshot in snapshots {
            if !snapshot.validate_counters() || snapshot.response_verified {
                return Err(AxiomError::Storage("invalid recovery record".into()));
            }
            let stored: Option<String> = tx
                .query_row(
                    "SELECT record FROM request_usage WHERE request_id=?1 AND thread_id=?2",
                    params![snapshot.request_id, id.to_string()],
                    |row| row.get(0),
                )
                .optional()
                .map_err(storage_error)?;
            let Some(stored) = stored else { continue };
            let mut local: axiom_inference::RequestUsage = serde_json::from_str(&stored)?;
            if local.model_id != snapshot.model_id
                || (!local.provider_id.is_empty() && local.provider_id != snapshot.provider_id)
            {
                return Err(AxiomError::Storage(
                    "accounting recovery identity mismatch".into(),
                ));
            }
            if local.settled || !snapshot.settled {
                continue;
            }
            local.provider_id.clone_from(&snapshot.provider_id);
            local.input_tokens.clone_from(&snapshot.input_tokens);
            local
                .cached_input_tokens
                .clone_from(&snapshot.cached_input_tokens);
            local.output_tokens.clone_from(&snapshot.output_tokens);
            local
                .reasoning_tokens
                .clone_from(&snapshot.reasoning_tokens);
            local.cost_microusd.clone_from(&snapshot.cost_microusd);
            if snapshot.error_code.as_deref() == Some("CANCELLED_USAGE_WAIVED") {
                local.error_code.clone_from(&snapshot.error_code);
            }
            local.completeness = snapshot.completeness;
            local.settled = snapshot.settled;
            if local.finished_at_ms.is_none() {
                local.finished_at_ms.clone_from(&snapshot.finished_at_ms);
            }
            // Recovery cannot recover a lost receipt or upgrade verification.
            tx.execute(
                "UPDATE request_usage SET record=?3 WHERE request_id=?1 AND thread_id=?2",
                params![
                    local.request_id,
                    id.to_string(),
                    serde_json::to_string(&local)?
                ],
            )
            .map_err(storage_error)?;
        }
        tx.commit().map_err(storage_error)
    }
}
