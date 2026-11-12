//! Forward migration of known media slots. Runs in the schema transaction;
//! crashes leave the old JSON authoritative and, at worst, orphan ciphertext.
use super::{
    asset_codec::{Direction, MediaCodec},
    assets::{AssetManager, thread_owner},
    codec::storage_error,
};
use crate::Result;
use crate::storage_progress::{self, Observer, Phase};
use rusqlite::{Connection, params};
use serde_json::Value;

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
                direction: Direction::Store,
            };
            match kind {
                0 => codec.attachments(&mut value)?,
                1 => codec.timeline(&mut value)?,
                2 => codec.event(&mut value)?,
                _ => codec.message(&mut value)?,
            }
            let encoded = serde_json::to_string(&value)?;
            if encoded != payload {
                connection
                    .execute(update, params![id, encoded])
                    .map_err(storage_error)?;
            }
            // model_messages.digest remains the digest of the hydrated runtime
            // message, preserving its identity and all checkpoint message IDs.
            completed += 1;
            storage_progress::count(progress, Phase::MovingMedia, completed, total);
        }
    }
    Ok(())
}
