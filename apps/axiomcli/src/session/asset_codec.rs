//! Explicit media slots only: never interpret tool arguments, text, or arbitrary
//! JSON as an image. Runtime objects are hydrated at the storage boundary.
use super::assets::{AssetManager, AssetRef};
use crate::{AxiomError, Result};
use rusqlite::Connection;
use serde_json::Value;

#[derive(Clone, Copy)]
pub(super) enum Direction {
    Store,
    Load,
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
    fn media(&self, value: &mut Value) -> Result<()> {
        let object = value
            .as_object_mut()
            .ok_or_else(|| AxiomError::Storage("invalid saved media object".into()))?;
        match self.direction {
            Direction::Store => {
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
            Direction::Load => {
                if object.contains_key("data") {
                    return Err(AxiomError::Storage(
                        "inline media in external-asset storage".into(),
                    ));
                }
                let reference: AssetRef =
                    serde_json::from_value(object.get("_axiom_asset").cloned().ok_or_else(
                        || AxiomError::Storage("saved media reference is missing".into()),
                    )?)?;
                let data = self
                    .assets
                    .resolve(self.connection, self.owner, &reference)?;
                object.remove("_axiom_asset");
                object.insert("data".into(), Value::String(data));
            }
        }
        Ok(())
    }
    pub fn attachments(&self, value: &mut Value) -> Result<()> {
        for attachment in value
            .as_array_mut()
            .ok_or_else(|| AxiomError::Storage("invalid saved attachments".into()))?
        {
            string_field(attachment, "name", true)?;
            match attachment["kind"].as_str() {
                Some("image") => {
                    only_fields(attachment, &["kind", "name", "image"])?;
                    only_fields(&attachment["image"], &["mimeType", "data", "_axiom_asset"])?;
                    string_field(&attachment["image"], "mimeType", true)?;
                    self.media(&mut attachment["image"])?;
                }
                Some("file") => {
                    only_fields(attachment, &["kind", "name", "file"])?;
                    only_fields(
                        &attachment["file"],
                        &["name", "mimeType", "data", "_axiom_asset"],
                    )?;
                    string_field(&attachment["file"], "mimeType", true)?;
                    string_field(&attachment["file"], "name", true)?;
                    self.media(&mut attachment["file"])?;
                }
                Some("text") => {
                    only_fields(attachment, &["kind", "name", "text"])?;
                    string_field(attachment, "text", true)?;
                }
                _ => return Err(AxiomError::Storage("unknown saved attachment kind".into())),
            }
        }
        Ok(())
    }
    pub fn message(&self, value: &mut Value) -> Result<()> {
        for field in ["images", "files"] {
            if let Some(values) = value.get_mut(field) {
                for media in values
                    .as_array_mut()
                    .ok_or_else(|| AxiomError::Storage("invalid saved message media".into()))?
                {
                    self.media(media)?;
                }
            }
        }
        Ok(())
    }
    pub fn event(&self, value: &mut Value) -> Result<()> {
        match value["kind"].as_str() {
            Some("prompt_accepted") => {
                if let Some(attachments) = value.get_mut("attachments") {
                    self.attachments(attachments)?;
                }
            }
            Some("image_generated") => self.media(&mut value["image"])?,
            Some("model_history_checkpoint") if value["_axiom_message_refs"] != true => {
                for message in value["messages"]
                    .as_array_mut()
                    .ok_or_else(|| AxiomError::Storage("invalid checkpoint messages".into()))?
                {
                    self.message(message)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    pub fn timeline(&self, value: &mut Value) -> Result<()> {
        if value.get("generated_image").is_some() {
            for block in value["content"]
                .as_array_mut()
                .ok_or_else(|| AxiomError::Storage("saved image content is missing".into()))?
            {
                let image = if block["type"] == "content" {
                    &mut block["content"]
                } else {
                    block
                };
                if image["type"] == "image" {
                    // The lazy UI projection intentionally contains a descriptor.
                    if matches!(self.direction, Direction::Load)
                        && image.get("imageResource").is_some()
                    {
                        continue;
                    }
                    self.media(image)?;
                }
            }
        }
        Ok(())
    }
    pub fn local_payload(&self, value: &mut Value) -> Result<()> {
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
                        self.attachments(attachments)?;
                    }
                }
                Ok(())
            }
            _ => Err(AxiomError::Storage("unknown local payload kind".into())),
        }
    }
}
