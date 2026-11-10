//! Versioned authoritative semantic history, independent from presentation rows.
//! Version 1 stores typed semantic records plus exact model-message checkpoints.
//! Runtime envelope sequence/correlation metadata is reconstructed on read.
use super::codec::{
    decode_path, encode_path, origin_from_name, origin_name, parse_time, storage_error,
};
use crate::{
    AxiomError, Result,
    app::{AppEvent, CorrelationId, EventEnvelope, SessionId},
};
use rusqlite::{Connection, OptionalExtension as _, Transaction, params};
use sha2::{Digest as _, Sha256};

pub(super) fn append(
    transaction: &Transaction<'_>,
    envelope: &EventEnvelope,
    turn: Option<&str>,
) -> Result<()> {
    // Historical response authentication is retained separately. A saved
    // attestation report or live provider state must never restore a lease.
    if matches!(
        envelope.event,
        AppEvent::SecurityStatusChanged { .. } | AppEvent::SecurityEvidenceUpdated { .. }
    ) {
        return Ok(());
    }
    let id = envelope.session_id.to_string();
    transaction
        .execute(
            "INSERT OR IGNORE INTO model_history_threads(thread_id,version) VALUES (?1,1)",
            [&id],
        )
        .map_err(storage_error)?;
    let mut payload = encode_event(&envelope.event)?;
    if let AppEvent::ModelHistoryCheckpoint { version, messages } = &envelope.event {
        if *version != 1 {
            return Err(AxiomError::Storage(
                "unsupported model checkpoint version".into(),
            ));
        }
        let messages = messages.as_array().ok_or_else(|| {
            AxiomError::Storage("model checkpoint must be a message array".into())
        })?;
        let mut references = Vec::with_capacity(messages.len());
        for message in messages {
            let message = serde_json::to_string(message)?;
            let digest = hex::encode(Sha256::digest(message.as_bytes()));
            transaction.execute("INSERT OR IGNORE INTO model_messages(thread_id,digest,payload) VALUES (?1,?2,?3)", params![id, digest, message]).map_err(storage_error)?;
            let (reference, stored): (i64, String) = transaction
                .query_row(
                    "SELECT id,payload FROM model_messages WHERE thread_id=?1 AND digest=?2",
                    params![id, digest],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(storage_error)?;
            if stored != message {
                return Err(AxiomError::Storage(
                    "canonical message digest collision".into(),
                ));
            }
            references.push(reference);
        }
        payload["messages"] = serde_json::to_value(references)?;
        payload["_axiom_message_refs"] = serde_json::Value::Bool(true);
        // Retain exact state for earlier turn revisions. Each message/media
        // payload is stored once; checkpoints contain only local message IDs.
        transaction.execute("DELETE FROM model_history WHERE thread_id=?1 AND turn_id IS ?2 AND json_extract(payload,'$.kind')='model_history_checkpoint'", params![id, turn]).map_err(storage_error)?;
    }
    transaction.execute("INSERT INTO model_history(thread_id,version,turn_id,occurred_at,origin,payload) VALUES (?1,1,?2,?3,?4,?5)", params![id, turn, envelope.occurred_at.to_rfc3339(), origin_name(envelope.origin), serde_json::to_string(&payload)?]).map_err(storage_error)?;
    Ok(())
}

pub(super) fn exists(connection: &Connection, id: &SessionId) -> Result<bool> {
    let version: Option<i64> = connection
        .query_row(
            "SELECT version FROM model_history_threads WHERE thread_id=?1",
            [id.to_string()],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage_error)?;
    match version {
        None => Ok(false),
        Some(1) => Ok(true),
        Some(version) => Err(AxiomError::Storage(format!(
            "unsupported authoritative model history version {version}"
        ))),
    }
}

pub(super) fn load(connection: &Connection, id: &SessionId) -> Result<Option<Vec<EventEnvelope>>> {
    load_range(connection, id, i64::MAX)
}

pub(super) fn before_turn(
    connection: &Connection,
    id: &SessionId,
    turn: &str,
) -> Result<Option<Vec<EventEnvelope>>> {
    if !exists(connection, id)? {
        return Ok(None);
    }
    let boundary: i64 = connection.query_row("SELECT MIN(sequence) FROM model_history WHERE thread_id=?1 AND turn_id=?2 AND json_extract(payload,'$.kind')='prompt_accepted'", params![id.to_string(), turn], |row| row.get(0)).map_err(storage_error)?;
    load_range(connection, id, boundary)
}

fn load_range(
    connection: &Connection,
    id: &SessionId,
    before: i64,
) -> Result<Option<Vec<EventEnvelope>>> {
    if !exists(connection, id)? {
        return Ok(None);
    }
    let mut statement = connection.prepare("SELECT version,turn_id,occurred_at,origin,payload FROM model_history WHERE thread_id=?1 AND sequence<?2 AND (json_extract(payload,'$.kind')<>'model_history_checkpoint' OR sequence=(SELECT MAX(sequence) FROM model_history WHERE thread_id=?1 AND sequence<?2 AND json_extract(payload,'$.kind')='model_history_checkpoint')) ORDER BY sequence").map_err(storage_error)?;
    let rows = statement
        .query_map(params![id.to_string(), before], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })
        .map_err(storage_error)?;
    let mut events = Vec::new();
    for row in rows {
        let (version, turn, time, origin, payload) = row.map_err(storage_error)?;
        if version != 1 {
            return Err(AxiomError::Storage(
                "unsupported authoritative model history record version".into(),
            ));
        }
        events.push(EventEnvelope {
            schema_version: 1,
            sequence: events.len() as u64 + 1,
            turn_id: turn
                .map(|id| id.parse())
                .transpose()
                .map_err(|_| AxiomError::Storage("invalid authoritative turn ID".into()))?,
            occurred_at: parse_time(&time),
            correlation_id: CorrelationId::new(),
            origin: origin_from_name(&origin)?,
            session_id: id.clone(),
            event: decode_event(connection, id, &payload)?,
        });
    }
    Ok(Some(events))
}

fn encode_event(event: &AppEvent) -> Result<serde_json::Value> {
    let mut event = event.clone();
    let cwd = match &mut event {
        AppEvent::SessionCreated { cwd, .. } | AppEvent::SessionResumed { cwd, .. } => {
            let encoded = encode_path(cwd)?;
            *cwd = std::path::PathBuf::new();
            Some(encoded)
        }
        _ => None,
    };
    let mut value = serde_json::to_value(&event)?;
    if let Some((encoding, bytes)) = cwd {
        value["_axiom_cwd"] = serde_json::json!({"encoding":encoding,"bytes":bytes});
    }
    Ok(value)
}

fn decode_event(connection: &Connection, id: &SessionId, payload: &str) -> Result<AppEvent> {
    let mut value: serde_json::Value = serde_json::from_str(payload).map_err(|error| {
        AxiomError::Storage(format!("invalid authoritative model history: {error}"))
    })?;
    if value
        .get("_axiom_message_refs")
        .and_then(serde_json::Value::as_bool)
        == Some(true)
    {
        let references: Vec<i64> =
            serde_json::from_value(value["messages"].clone()).map_err(|error| {
                AxiomError::Storage(format!("invalid canonical message references: {error}"))
            })?;
        let mut messages = Vec::with_capacity(references.len());
        for reference in references {
            let payload: String = connection
                .query_row(
                    "SELECT payload FROM model_messages WHERE thread_id=?1 AND id=?2",
                    params![id.to_string(), reference],
                    |row| row.get(0),
                )
                .map_err(storage_error)?;
            messages.push(serde_json::from_str::<serde_json::Value>(&payload).map_err(
                |error| AxiomError::Storage(format!("invalid canonical message: {error}")),
            )?);
        }
        value["messages"] = serde_json::Value::Array(messages);
    }
    let cwd = value
        .as_object_mut()
        .and_then(|object| object.remove("_axiom_cwd"));
    let mut event: AppEvent = serde_json::from_value(value).map_err(|error| {
        AxiomError::Storage(format!("invalid authoritative model history: {error}"))
    })?;
    if let Some(cwd) = cwd {
        let encoding = cwd["encoding"]
            .as_str()
            .ok_or_else(|| AxiomError::Storage("invalid canonical cwd encoding".into()))?;
        let bytes: Vec<u8> = serde_json::from_value(cwd["bytes"].clone())
            .map_err(|error| AxiomError::Storage(format!("invalid canonical cwd: {error}")))?;
        match &mut event {
            AppEvent::SessionCreated { cwd, .. } | AppEvent::SessionResumed { cwd, .. } => {
                *cwd = decode_path(encoding, &bytes)?;
            }
            _ => return Err(AxiomError::Storage("unexpected canonical cwd".into())),
        }
    }
    Ok(event)
}
