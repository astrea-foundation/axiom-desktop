//! Explicit media slots only: never interpret tool arguments, text, or arbitrary
//! JSON as an image. Runtime objects are hydrated at the storage boundary.
use super::assets::{AssetLoad, AssetManager, AssetRef};
use crate::{AxiomError, Result};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use rusqlite::Connection;
use serde_json::Value;

pub(super) const MEDIA_OMITTED: &str =
    "Unavailable saved media was omitted; usable text and tool outcomes were preserved.";

#[derive(Clone, Copy)]
pub(super) enum Direction {
    Store,
    Load,
    /// Only the authenticated, transactional pre-v7 conversion may omit bad
    /// inline media. Ordinary writes never accept migration tombstones.
    Migrate,
}
#[derive(Clone, Copy)]
enum MediaKind {
    Image,
    File,
}
pub(super) struct MediaCodec<'a> {
    pub assets: &'a AssetManager,
    pub connection: &'a Connection,
    pub owner: &'a str,
    pub direction: Direction,
}
fn only_fields(value: &Value, allowed: &[&str]) -> Result<()> {
    let object = value
        .as_object()
        .ok_or_else(|| AxiomError::Storage("invalid stored object".into()))?;
    if object.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(AxiomError::Storage(
            "unsupported stored payload field".into(),
        ));
    }
    Ok(())
}
fn string_field(value: &Value, field: &str, required: bool) -> Result<()> {
    if value.get(field).is_some_and(|v| !v.is_string()) || (required && value.get(field).is_none())
    {
        return Err(AxiomError::Storage("invalid stored text field".into()));
    }
    Ok(())
}
impl MediaCodec<'_> {
    fn media(&self, value: &mut Value, kind: MediaKind) -> Result<bool> {
        // Only recognized media slots enter this function. Invalid references
        // can be omitted on read, but must never become bytes or be adopted by
        // a new write. Arbitrary JSON/text is not scanned for asset markers.
        if matches!(self.direction, Direction::Load) {
            let Some(object) = value.as_object_mut() else {
                return Ok(false);
            };
            if object.contains_key("data") || object.contains_key("_axiom_unavailable") {
                return Ok(false);
            }
            let Some(reference) = object.get("_axiom_asset") else {
                return Ok(false);
            };
            let Ok(reference) = serde_json::from_value::<AssetRef>(reference.clone()) else {
                return Ok(false);
            };
            return match self.assets.load(self.connection, self.owner, &reference)? {
                AssetLoad::Available(data) => {
                    object.remove("_axiom_asset");
                    object.insert("data".into(), Value::String(data));
                    Ok(true)
                }
                AssetLoad::Unavailable(_) => Ok(false),
            };
        }
        if matches!(self.direction, Direction::Migrate) && !valid_inline_media(value, kind) {
            // Keep a typed, byte-free unavailable slot for attachment indexes.
            // The original record is archived by the enclosing migration.
            *value = unavailable_media(kind);
            return Ok(false);
        }
        let object = value
            .as_object_mut()
            .ok_or_else(|| AxiomError::Storage("invalid saved media object".into()))?;
        match self.direction {
            Direction::Store | Direction::Migrate => {
                if object.contains_key("_axiom_unavailable") {
                    return Err(AxiomError::Storage(
                        "unavailable media cannot be written as an upload".into(),
                    ));
                }
                if let Some(data) = object.get("data") {
                    if object.contains_key("_axiom_asset") {
                        return Err(AxiomError::Storage(
                            "ambiguous saved media representation".into(),
                        ));
                    }
                    let data = data
                        .as_str()
                        .ok_or_else(|| AxiomError::Storage("invalid media data".into()))?;
                    let reference = self.assets.intern(self.connection, self.owner, data)?;
                    object.remove("data");
                    object.insert("_axiom_asset".into(), serde_json::to_value(reference)?);
                } else if let Some(reference) = object.get("_axiom_asset") {
                    let reference: AssetRef = serde_json::from_value(reference.clone())?;
                    self.assets
                        .resolve(self.connection, self.owner, &reference)?;
                } else {
                    return Err(AxiomError::Storage("saved media bytes are missing".into()));
                }
            }
            Direction::Load => unreachable!("handled above"),
        }
        Ok(true)
    }
    pub fn attachments(&self, value: &mut Value) -> Result<usize> {
        self.load_attachments(value, false)
    }
    pub fn replay_attachments(&self, value: &mut Value) -> Result<usize> {
        self.load_attachments(value, true)
    }
    fn load_attachments(&self, value: &mut Value, preserve_positions: bool) -> Result<usize> {
        if matches!(self.direction, Direction::Migrate) && !value.is_array() {
            *value = Value::Array(Vec::new());
            return Ok(1);
        }
        let values = value
            .as_array_mut()
            .ok_or_else(|| AxiomError::Storage("invalid saved attachments".into()))?;
        let mut omitted = 0;
        let mut usable = Vec::with_capacity(values.len());
        for mut attachment in std::mem::take(values) {
            if matches!(self.direction, Direction::Migrate)
                && matches!(attachment["kind"].as_str(), Some("image" | "file"))
                && serde_json::from_value::<axiom_inference::PromptAttachment>(attachment.clone())
                    .ok()
                    .is_none_or(|attachment| attachment.validate().is_err())
            {
                let (kind, field, media_kind) = if attachment["kind"] == "image" {
                    ("image", "image", MediaKind::Image)
                } else {
                    ("file", "file", MediaKind::File)
                };
                let name = attachment["name"]
                    .as_str()
                    .filter(|name| {
                        !name.is_empty() && name.len() <= 512 && !name.chars().any(char::is_control)
                    })
                    .unwrap_or("Unavailable attachment");
                attachment = serde_json::json!({"kind":kind,"name":name,field:unavailable_media(media_kind)});
            }
            string_field(&attachment, "name", true)?;
            match attachment["kind"].as_str() {
                Some("image") => {
                    only_fields(&attachment, &["kind", "name", "image"])?;
                    only_fields(
                        &attachment["image"],
                        &["mimeType", "data", "_axiom_asset", "_axiom_unavailable"],
                    )?;
                    string_field(&attachment["image"], "mimeType", true)?;
                    if !self.media(&mut attachment["image"], MediaKind::Image)? {
                        omitted += 1;
                        if matches!(self.direction, Direction::Migrate) {
                            usable.push(attachment);
                        } else if preserve_positions {
                            usable.push(omitted_attachment());
                        }
                        continue;
                    }
                }
                Some("file") => {
                    only_fields(&attachment, &["kind", "name", "file"])?;
                    only_fields(
                        &attachment["file"],
                        &[
                            "name",
                            "mimeType",
                            "data",
                            "_axiom_asset",
                            "_axiom_unavailable",
                        ],
                    )?;
                    string_field(&attachment["file"], "mimeType", true)?;
                    string_field(&attachment["file"], "name", true)?;
                    if !self.media(&mut attachment["file"], MediaKind::File)? {
                        omitted += 1;
                        if matches!(self.direction, Direction::Migrate) {
                            usable.push(attachment);
                        } else if preserve_positions {
                            usable.push(omitted_attachment());
                        }
                        continue;
                    }
                }
                Some("text") => {
                    only_fields(&attachment, &["kind", "name", "text"])?;
                    string_field(&attachment, "text", true)?;
                }
                _ => return Err(AxiomError::Storage("unknown saved attachment kind".into())),
            }
            usable.push(attachment);
        }
        *values = usable;
        Ok(omitted)
    }
    pub fn message(&self, value: &mut Value) -> Result<usize> {
        let mut omitted = 0;
        for field in ["images", "files"] {
            if let Some(values) = value.get_mut(field) {
                if matches!(self.direction, Direction::Migrate) && !values.is_array() {
                    *values = Value::Array(Vec::new());
                    omitted += 1;
                }
                let values = values
                    .as_array_mut()
                    .ok_or_else(|| AxiomError::Storage("invalid saved message media".into()))?;
                let mut usable = Vec::with_capacity(values.len());
                for mut media in std::mem::take(values) {
                    if self.media(
                        &mut media,
                        if field == "images" {
                            MediaKind::Image
                        } else {
                            MediaKind::File
                        },
                    )? {
                        usable.push(media);
                    } else {
                        omitted += 1;
                    }
                }
                *values = usable;
            }
        }
        if omitted > 0
            && value["content"]
                .as_str()
                .is_none_or(|text| text.trim().is_empty())
        {
            value["content"] = Value::String("[An earlier message's media is unavailable.]".into());
        }
        Ok(omitted)
    }
    pub fn event(&self, value: &mut Value) -> Result<usize> {
        let mut omitted = 0;
        match value["kind"].as_str() {
            Some("prompt_accepted") => {
                if let Some(attachments) = value.get_mut("attachments") {
                    // Upload IDs include their original attachment index. A
                    // text-only historical placeholder keeps surviving IDs
                    // stable without exposing unavailable bytes to the model.
                    omitted += self.replay_attachments(attachments)?;
                }
            }
            Some("image_generated") => {
                if !self.media(&mut value["image"], MediaKind::Image)? {
                    *value = serde_json::json!({"kind":"warning_raised", "message":MEDIA_OMITTED});
                    omitted += 1;
                }
            }
            Some("model_history_checkpoint") if value["_axiom_message_refs"] != true => {
                if matches!(self.direction, Direction::Migrate) && value["version"] != 1 {
                    return Err(AxiomError::Storage(
                        "unsupported model checkpoint version".into(),
                    ));
                }
                for message in value["messages"]
                    .as_array_mut()
                    .ok_or_else(|| AxiomError::Storage("invalid checkpoint messages".into()))?
                {
                    omitted += self.message(message)?;
                }
            }
            _ => {}
        }
        Ok(omitted)
    }
    pub fn timeline(&self, value: &mut Value) -> Result<usize> {
        let mut omitted = 0;
        if value.get("generated_image").is_some_and(Value::is_object) {
            if !matches!(self.direction, Direction::Store) && !value["content"].is_array() {
                value
                    .as_object_mut()
                    .expect("metadata object")
                    .remove("generated_image");
                value["recovery_warning"] = Value::String(MEDIA_OMITTED.into());
                return Ok(1);
            }
            let values = value["content"]
                .as_array_mut()
                .ok_or_else(|| AxiomError::Storage("saved image content is missing".into()))?;
            let mut usable = Vec::with_capacity(values.len());
            for mut block in std::mem::take(values) {
                let image = if block["type"] == "content" {
                    &mut block["content"]
                } else {
                    &mut block
                };
                if image["type"] == "image" {
                    // The lazy UI projection intentionally contains a descriptor.
                    if matches!(self.direction, Direction::Load)
                        && image.get("imageResource").is_some()
                    {
                        usable.push(block);
                        continue;
                    }
                    if !self.media(image, MediaKind::Image)? {
                        omitted += 1;
                        continue;
                    }
                }
                usable.push(block);
            }
            *values = usable;
        }
        if omitted > 0 {
            value
                .as_object_mut()
                .expect("metadata object")
                .remove("generated_image");
            value["recovery_warning"] = Value::String(MEDIA_OMITTED.into());
        }
        Ok(omitted)
    }
    pub fn local_payload(&self, value: &mut Value) -> Result<usize> {
        if value["version"] != 1 {
            return Err(AxiomError::Storage(
                "unsupported local payload version".into(),
            ));
        }
        match value["kind"].as_str() {
            Some("prompt") => {
                only_fields(value, &["version", "kind", "payload"])?;
                only_fields(&value["payload"], &["text", "attachments"])?;
                string_field(&value["payload"], "text", true)?;
                self.attachments(&mut value["payload"]["attachments"])
            }
            Some("queue") => {
                let mut omitted = 0;
                only_fields(value, &["version", "kind", "items", "legacyImported"])?;
                if value.get("legacyImported").is_some_and(|v| !v.is_boolean()) {
                    return Err(AxiomError::Storage("invalid queue migration marker".into()));
                }
                for item in value["items"]
                    .as_array_mut()
                    .ok_or_else(|| AxiomError::Storage("invalid saved queue".into()))?
                {
                    only_fields(
                        item,
                        &[
                            "id",
                            "threadId",
                            "text",
                            "attachments",
                            "payloadId",
                            "webEnabled",
                            "agentRevision",
                            "mcpRevision",
                            "status",
                            "preparing",
                            "error",
                        ],
                    )?;
                    for field in ["id", "threadId", "text", "payloadId", "status", "error"] {
                        string_field(item, field, field == "id")?;
                    }
                    for field in ["webEnabled", "preparing"] {
                        if item.get(field).is_some_and(|v| !v.is_boolean()) {
                            return Err(AxiomError::Storage("invalid queue flag".into()));
                        }
                    }
                    for field in ["agentRevision", "mcpRevision"] {
                        if item.get(field).is_some_and(|v| v.as_u64().is_none()) {
                            return Err(AxiomError::Storage("invalid queue revision".into()));
                        }
                    }
                    if let Some(attachments) = item.get_mut("attachments") {
                        let count = self.attachments(attachments)?;
                        omitted += count;
                        if count > 0 {
                            item["error"] = Value::String(MEDIA_OMITTED.into());
                            item["status"] = Value::String("queued".into());
                        }
                    }
                }
                Ok(omitted)
            }
            _ => Err(AxiomError::Storage("unknown local payload kind".into())),
        }
    }
}

fn omitted_attachment() -> Value {
    serde_json::json!({"kind":"text", "name":"Unavailable attachment", "text":"[Saved attachment is unavailable.]"})
}

fn unavailable_media(kind: MediaKind) -> Value {
    match kind {
        MediaKind::Image => serde_json::json!({"mimeType":"image/png","_axiom_unavailable":true}),
        MediaKind::File => {
            serde_json::json!({"name":"Unavailable attachment","mimeType":"application/octet-stream","_axiom_unavailable":true})
        }
    }
}

fn valid_inline_media(value: &Value, kind: MediaKind) -> bool {
    if value.get("_axiom_asset").is_some() || value.get("_axiom_unavailable").is_some() {
        return false;
    }
    let Some(data) = value["data"].as_str() else {
        return false;
    };
    let Some(mime_type) = value["mimeType"].as_str() else {
        return false;
    };
    let valid = match kind {
        MediaKind::Image => axiom_inference::ImageContent {
            data: data.into(),
            mime_type: mime_type.into(),
        }
        .validate_bounded(axiom_inference::MAX_GENERATED_IMAGE_BYTES)
        .is_ok(),
        MediaKind::File => value["name"].as_str().is_some_and(|name| {
            axiom_inference::FileContent {
                name: name.into(),
                data: data.into(),
                mime_type: mime_type.into(),
            }
            .validate()
            .is_ok()
        }),
    };
    valid
        && STANDARD
            .decode(data)
            .is_ok_and(|bytes| STANDARD.encode(bytes) == data)
}
