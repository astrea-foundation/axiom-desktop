//! Forward migration of known media slots. Runs in the schema transaction;
//! crashes leave the old JSON authoritative and, at worst, orphan ciphertext.
use super::{
    asset_codec::{Direction, MediaCodec},
    assets::{AssetManager, thread_owner},
    codec::storage_error,
};
use crate::Result;
use crate::storage_progress::{self, Observer, Phase};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use rusqlite::{Connection, params};
use serde_json::Value;
use sha2::{Digest as _, Sha256};

pub(super) fn migrate(
    connection: &Connection,
    assets: &AssetManager,
    progress: &Observer<'_>,
) -> Result<()> {
    let mut groups = Vec::new();
    let mut total = 0;
    for (select, update, kind) in [
        (
            "SELECT p.user_item_id,i.thread_id FROM prompt_attachments p JOIN timeline_items i ON i.id=p.user_item_id",
            "UPDATE prompt_attachments SET payload=?2 WHERE user_item_id=?1",
            0,
        ),
        (
            "SELECT id,thread_id FROM timeline_items WHERE CASE WHEN json_valid(metadata) THEN json_type(metadata,'$.generated_image') END='object'",
            "UPDATE timeline_items SET metadata=?2 WHERE id=?1",
            1,
        ),
        (
            "SELECT CAST(sequence AS TEXT),thread_id FROM model_history",
            "UPDATE model_history SET payload=?2 WHERE sequence=?1",
            2,
        ),
        (
            "SELECT CAST(id AS TEXT),thread_id FROM model_messages",
            "UPDATE model_messages SET payload=?2 WHERE id=?1",
            3,
        ),
    ] {
        let mut statement = connection.prepare(select).map_err(storage_error)?;
        let rows = statement
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map_err(storage_error)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(storage_error)?;
        drop(statement);
        total += rows.len() as u64;
        groups.push((rows, update, kind));
    }
    let mut completed = 0;
    storage_progress::count(progress, Phase::MovingMedia, completed, total);
    for (rows, update, kind) in groups {
        for (id, thread) in rows {
            let read = match kind {
                0 => "SELECT payload FROM prompt_attachments WHERE user_item_id=?1",
                1 => "SELECT metadata FROM timeline_items WHERE id=?1",
                2 => "SELECT payload FROM model_history WHERE sequence=?1",
                _ => "SELECT payload FROM model_messages WHERE id=?1",
            };
            let payload: String = connection
                .query_row(read, [&id], |r| r.get(0))
                .map_err(storage_error)?;
            let mut value: Value = serde_json::from_str(&payload)?;
            let owner = thread_owner(&thread);
            let codec = MediaCodec {
                assets,
                connection,
                owner: &owner,
                direction: Direction::Migrate,
            };
            let omitted = match kind {
                0 => codec.attachments(&mut value)?,
                1 => codec.timeline(&mut value)?,
                2 => codec.event(&mut value)?,
                _ => codec.message(&mut value)?,
            };
            if omitted > 0 {
                // Preserve exact original records as ordinary encrypted assets
                // owned by the thread, never as media slots or SQL base64. Both
                // the archive ownership and omission commit with the migration.
                // Chunking also handles oversized malformed inline payloads.
                let original_digest: Option<String> = if kind == 3 {
                    Some(
                        connection
                            .query_row(
                                "SELECT digest FROM model_messages WHERE id=?1",
                                [&id],
                                |row| row.get(0),
                            )
                            .map_err(storage_error)?,
                    )
                } else {
                    None
                };
                preserve_record(
                    connection,
                    assets,
                    &owner,
                    kind,
                    &id,
                    &payload,
                    original_digest.as_deref(),
                )?;
                if let Some(digest) = original_digest {
                    // Preserve checkpoint IDs, but do not let the old digest
                    // collide with a later valid original message. Recovery
                    // keys are outside the canonical SHA-256 dedup namespace.
                    connection
                        .execute(
                            "UPDATE model_messages SET digest=?2 WHERE id=?1",
                            params![id, format!("legacy-media-omitted-v1:{digest}:{id}")],
                        )
                        .map_err(storage_error)?;
                }
                tracing::warn!(omitted, "{}", super::asset_codec::MEDIA_OMITTED);
            }
            let encoded = serde_json::to_string(&value)?;
            if encoded != payload {
                connection
                    .execute(update, params![id, encoded])
                    .map_err(storage_error)?;
            }
            // Unchanged model-message digests and all checkpoint IDs survive;
            // omitted messages retain their original digest in recovery data.
            completed += 1;
            storage_progress::count(progress, Phase::MovingMedia, completed, total);
        }
    }
    Ok(())
}

pub(super) fn preserve_record(
    connection: &Connection,
    assets: &AssetManager,
    owner: &str,
    kind: u8,
    id: &str,
    payload: &str,
    original_digest: Option<&str>,
) -> Result<()> {
    let source_sha256 = hex::encode(Sha256::digest(payload.as_bytes()));
    let chunks = payload.as_bytes().chunks(1024 * 1024);
    let total = chunks.len();
    for (part, chunk) in chunks.enumerate() {
        let recovery = serde_json::json!({"kind":"legacy_media_recovery","version":1,
            "sourceKind":kind,"sourceId":id,"sourceSha256":source_sha256,"part":part,"parts":total,
            "originalDigest":original_digest,"originalPayloadBase64":STANDARD.encode(chunk)});
        assets.intern(
            connection,
            owner,
            &STANDARD.encode(serde_json::to_vec(&recovery)?),
        )?;
    }
    Ok(())
}
