//! Versioned authoritative semantic history, independent from presentation rows.
//! Version 1 stores typed semantic records plus exact model-message checkpoints.
//! Runtime envelope sequence/correlation metadata is reconstructed on read.
use super::codec::{
    decode_path, encode_path, origin_from_name, origin_name, parse_time, storage_error,
};
use super::{
    asset_codec::{Direction, MediaCodec},
    assets::{AssetManager, thread_owner},
};
use crate::{
    AxiomError, Result,
    app::{AppEvent, CorrelationId, EventEnvelope, SessionId},
};
use rusqlite::{Connection, OptionalExtension as _, Transaction, params};
use sha2::{Digest as _, Sha256};

pub(super) fn append(
    transaction: &Transaction<'_>,
    assets: &AssetManager,
    envelope: &EventEnvelope,
    turn: Option<&str>,
) -> Result<()> {
    // Historical response authentication is retained separately. A saved
    // attestation report or live provider state must never restore a lease.
    // Tool-call drafting is display-only progress with no replay meaning.
    if matches!(
        envelope.event,
        AppEvent::SecurityStatusChanged { .. }
            | AppEvent::SecurityEvidenceUpdated { .. }
            | AppEvent::CompactionProgressUpdated { .. }
            | AppEvent::ToolCallDrafting { .. }
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
            let digest = hex::encode(Sha256::digest(serde_json::to_string(message)?.as_bytes()));
            let mut message = message.clone();
            MediaCodec {
                assets,
                connection: transaction,
                owner: &thread_owner(&id),
                direction: Direction::Store,
            }
            .message(&mut message)?;
            let message = serde_json::to_string(&message)?;
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
    MediaCodec {
        assets,
        connection: transaction,
        owner: &thread_owner(&id),
        direction: Direction::Store,
    }
    .event(&mut payload)?;
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

pub(super) fn load(
    connection: &Connection,
    assets: &AssetManager,
    id: &SessionId,
) -> Result<Option<Vec<EventEnvelope>>> {
    load_range(connection, assets, id, i64::MAX, true)
}

pub(super) fn load_presentation(
    connection: &Connection,
    assets: &AssetManager,
    id: &SessionId,
) -> Result<Option<Vec<EventEnvelope>>> {
    load_range(connection, assets, id, i64::MAX, false)
}

pub(super) fn before_turn(
    connection: &Connection,
    assets: &AssetManager,
    id: &SessionId,
    turn: &str,
) -> Result<Option<Vec<EventEnvelope>>> {
    if !exists(connection, id)? {
        return Ok(None);
    }
    let boundary: i64 = connection.query_row("SELECT MIN(sequence) FROM model_history WHERE thread_id=?1 AND turn_id=?2 AND json_extract(payload,'$.kind')='prompt_accepted'", params![id.to_string(), turn], |row| row.get(0)).map_err(storage_error)?;
    load_range(connection, assets, id, boundary, true)
}

fn load_range(
    connection: &Connection,
    assets: &AssetManager,
    id: &SessionId,
    before: i64,
    selective: bool,
) -> Result<Option<Vec<EventEnvelope>>> {
    if !exists(connection, id)? {
        return Ok(None);
    }
    // Older model bodies are overwritten by the canonical checkpoint. Read
    // their small state/lifecycle projection only, and retain historical image
    // uploads/generations for the catalog. In particular, never deserialize old
    // text/tool-output bodies, file attachments, or obsolete checkpoint media.
    // This does not trim the presentation/export journal or revision history.
    let mut statement = connection
        .prepare(
            r"
        WITH latest AS (
            SELECT COALESCE(MAX(sequence),0) AS checkpoint FROM model_history
            WHERE thread_id=?1 AND sequence<?2
              AND json_extract(payload,'$.kind')='model_history_checkpoint'
        )
        SELECT version,turn_id,occurred_at,origin,
            CASE WHEN ?3 AND sequence < latest.checkpoint THEN
                CASE json_extract(payload,'$.kind')
                    WHEN 'prompt_accepted' THEN json_set(payload,'$.text','',
                        '$.attachments',json((SELECT json_group_array(json(
                            CASE WHEN json_extract(a.value,'$.kind')='image' THEN a.value
                            ELSE json_object('kind','text','name','Historical attachment',
                                'text','[Earlier attachment retained in saved history.]') END))
                            FROM json_each(payload,'$.attachments') a
                        )))
                    WHEN 'steering_applied' THEN json_set(payload,'$.text','')
                    WHEN 'context_compacted' THEN json_set(payload,'$.summary','')
                    ELSE payload
                END
            ELSE payload END, sequence
        FROM model_history,latest
        WHERE thread_id=?1 AND sequence<?2 AND (
            version<>1 OR sequence>=latest.checkpoint
            OR (NOT ?3 AND json_extract(payload,'$.kind')<>'model_history_checkpoint')
            OR json_extract(payload,'$.kind') IN (
                'session_created','session_resumed','model_changed','thinking_level_changed','permission_profile_changed',
                'desktop_agent_configured','session_closed',
                'workspace_changed','image_generated','prompt_accepted','steering_applied',
                'turn_started','turn_completed','turn_cancelled','error_raised',
                'tool_started','tool_completed','context_compacted'
            )
        ) ORDER BY sequence
    ",
        )
        .map_err(storage_error)?;
    let rows = statement
        .query_map(params![id.to_string(), before, selective], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, i64>(5)?,
            ))
        })
        .map_err(storage_error)?;
    let mut events = Vec::new();
    let mut omitted = 0;
    let mut checkpoint = None;
    for row in rows {
        let (version, turn, time, origin, payload, source_sequence) = row.map_err(storage_error)?;
        if version != 1 {
            return Err(AxiomError::Storage(
                "unsupported authoritative model history record version".into(),
            ));
        }
        let (event, count) = decode_event(connection, assets, id, &payload)?;
        if matches!(event, AppEvent::ModelHistoryCheckpoint { .. }) {
            checkpoint = Some((events.len(), source_sequence));
        }
        omitted += count;
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
            event,
        });
    }
    if selective
        && let Some((index, source_sequence)) = checkpoint
        && let Some(report) = checkpoint_usage(connection, id, source_sequence)?
    {
        // A terminal checkpoint can repeat exactly the state covered by the
        // previous report. Apply that metadata after the checkpoint, where its
        // message count is known, without replaying discarded model bodies.
        events.insert(index + 1, report);
        for (index, event) in events.iter_mut().enumerate() {
            event.sequence = index as u64 + 1;
        }
    }
    if omitted > 0
        && let Some(mut warning) = events.last().cloned()
    {
        warning.sequence += 1;
        warning.turn_id = None;
        warning.event = AppEvent::WarningRaised {
            message: super::asset_codec::MEDIA_OMITTED.into(),
        };
        events.push(warning);
    }
    Ok(Some(events))
}

fn checkpoint_usage(
    connection: &Connection,
    id: &SessionId,
    checkpoint: i64,
) -> Result<Option<EventEnvelope>> {
    let saved: Option<(String, Option<String>, String, String)> = connection.query_row(r"
        WITH report AS (
            SELECT sequence,payload,turn_id,occurred_at,origin FROM model_history WHERE thread_id=?1 AND sequence<?2
              AND json_extract(payload,'$.kind')='context_usage_updated'
            ORDER BY sequence DESC LIMIT 1
        )
        SELECT report.payload,report.turn_id,report.occurred_at,report.origin FROM report,model_history current
        WHERE current.thread_id=?1 AND current.sequence=?2
          AND NOT EXISTS (SELECT 1 FROM model_history h WHERE h.thread_id=?1
              AND h.sequence>report.sequence AND h.sequence<?2
              AND (json_extract(h.payload,'$.kind') IN (
                  'prompt_accepted','steering_applied','text_delta','reasoning_delta',
                  'tool_proposed','tool_output','tool_completed','image_generated','context_compacted'
              ) OR (json_extract(h.payload,'$.kind')='model_history_checkpoint' AND (
                  json_extract(h.payload,'$.messages') IS NOT json_extract(current.payload,'$.messages')
                  OR json_extract(h.payload,'$._axiom_message_refs') IS NOT json_extract(current.payload,'$._axiom_message_refs')
              ))))
    ", params![id.to_string(), checkpoint], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))).optional().map_err(storage_error)?;
    saved
        .map(|(payload, turn, time, origin)| {
            let event = serde_json::from_str(&payload).map_err(|error| {
                AxiomError::Storage(format!("invalid authoritative context usage: {error}"))
            })?;
            Ok(EventEnvelope {
                schema_version: 1,
                sequence: 0,
                turn_id: turn
                    .map(|id| id.parse())
                    .transpose()
                    .map_err(|_| AxiomError::Storage("invalid authoritative turn ID".into()))?,
                occurred_at: parse_time(&time),
                correlation_id: CorrelationId::new(),
                origin: origin_from_name(&origin)?,
                session_id: id.clone(),
                event,
            })
        })
        .transpose()
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

fn decode_event(
    connection: &Connection,
    assets: &AssetManager,
    id: &SessionId,
    payload: &str,
) -> Result<(AppEvent, usize)> {
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
        value
            .as_object_mut()
            .expect("event object")
            .remove("_axiom_message_refs");
    }
    let omitted = MediaCodec {
        assets,
        connection,
        owner: &thread_owner(&id.to_string()),
        direction: Direction::Load,
    }
    .event(&mut value)?;
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
    Ok((event, omitted))
}
