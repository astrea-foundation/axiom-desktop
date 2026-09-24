//! Local user attachments. Only provider adapters serialize image bytes onto
//! their authenticated encrypted wire; attachments are never relay metadata.
use std::fmt::Write as _;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{ChatMessage, ChatRole, MAX_MESSAGE_TEXT_BYTES, ValidationError};

pub const MAX_ATTACHMENTS: usize = 8;
pub const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;
pub const MAX_FILE_BYTES: usize = 10 * 1024 * 1024;
pub const MAX_ATTACHMENT_BYTES: usize = 16 * 1024 * 1024;

pub const FILE_MIME_TYPES: &[&str] = &[
    "text/plain",
    "text/markdown",
    "text/csv",
    "text/html",
    "application/json",
    "application/xml",
    "application/pdf",
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    "application/vnd.openxmlformats-officedocument.presentationml.presentation",
    "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
];

/// Original file bytes. Extraction belongs to the attested provider, not Axiom.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FileContent {
    pub name: String,
    pub mime_type: String,
    pub data: String,
}

impl FileContent {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.name.is_empty()
            || self.name.len() > 512
            || self.name.chars().any(char::is_control)
            || !FILE_MIME_TYPES.contains(&self.mime_type.as_str())
            || self.data.is_empty()
            || self.data.len() > MAX_FILE_BYTES.div_ceil(3) * 4
        {
            return Err(ValidationError::invalid("file.content"));
        }
        let bytes = STANDARD
            .decode(&self.data)
            .map_err(|_| ValidationError::invalid("file.base64"))?;
        if bytes.len() > MAX_FILE_BYTES || STANDARD.encode(&bytes) != self.data {
            return Err(ValidationError::invalid("file.size"));
        }
        Ok(())
    }

    #[must_use]
    pub fn data_url(&self) -> String {
        format!("data:{};base64,{}", self.mime_type, self.data)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImageContent {
    pub mime_type: String,
    /// Canonical base64, without a data-URL prefix. Remote URLs are not images.
    pub data: String,
}

impl ImageContent {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.data.len() > MAX_IMAGE_BYTES.div_ceil(3) * 4 {
            return Err(ValidationError::invalid("image.size"));
        }
        let bytes = STANDARD
            .decode(&self.data)
            .map_err(|_| ValidationError::invalid("image.base64"))?;
        let matches = match self.mime_type.as_str() {
            "image/png" => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
            "image/jpeg" => bytes.starts_with(&[0xff, 0xd8, 0xff]),
            "image/webp" => bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP"),
            "image/gif" => bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"),
            _ => false,
        };
        if bytes.len() > MAX_IMAGE_BYTES || !matches {
            return Err(ValidationError::invalid("image.content"));
        }
        Ok(())
    }

    #[must_use]
    pub fn data_url(&self) -> String {
        format!("data:{};base64,{}", self.mime_type, self.data)
    }

    pub fn from_data_url(url: &str) -> Result<Self, ValidationError> {
        let (mime_type, data) = url
            .strip_prefix("data:")
            .and_then(|value| value.split_once(";base64,"))
            .ok_or_else(|| ValidationError::unsupported("image.url"))?;
        let image = Self {
            mime_type: mime_type.into(),
            data: data.into(),
        };
        image.validate()?;
        Ok(image)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PromptAttachment {
    // Readable only in historical local data. Never accepted as a new upload.
    Text { name: String, text: String },
    Image { name: String, image: ImageContent },
    File { name: String, file: FileContent },
}

impl PromptAttachment {
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::Text { name, .. } | Self::Image { name, .. } | Self::File { name, .. } => name,
        }
    }

    #[must_use]
    pub fn bytes(&self) -> usize {
        match self {
            Self::Text { text, .. } => text.len(),
            Self::Image { image, .. } => image.data.len(),
            Self::File { file, .. } => file.data.len(),
        }
    }

    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.name().is_empty()
            || self.name().len() > 512
            || self.name().chars().any(char::is_control)
        {
            return Err(ValidationError::invalid("attachment.name"));
        }
        match self {
            Self::Text { text, .. }
                if text.is_empty()
                    || text.len() > MAX_MESSAGE_TEXT_BYTES
                    || text.contains('\0') =>
            {
                Err(ValidationError::invalid("attachment.text"))
            }
            Self::Text { .. } => Ok(()),
            Self::Image { image, .. } => image.validate(),
            Self::File { name, file } => {
                if name != &file.name {
                    return Err(ValidationError::invalid("file.name"));
                }
                file.validate()
            }
        }
    }

    /// Small transcript projection. Original content stays in the local attachment table.
    #[must_use]
    pub fn summary(&self) -> serde_json::Value {
        serde_json::json!({"name": self.name(), "kind": match self { Self::Text { .. } => "text", Self::Image { .. } => "image", Self::File { .. } => "file" }, "bytes": self.bytes()})
    }
}

pub fn validate_prompt(
    text: &str,
    attachments: &[PromptAttachment],
) -> Result<(), ValidationError> {
    if attachments
        .iter()
        .any(|attachment| matches!(attachment, PromptAttachment::Text { .. }))
    {
        return Err(ValidationError::unsupported("attachment.extracted_text"));
    }
    validate_stored_prompt(text, attachments)
}

pub fn validate_stored_prompt(
    text: &str,
    attachments: &[PromptAttachment],
) -> Result<(), ValidationError> {
    if text.len() > MAX_MESSAGE_TEXT_BYTES || (text.trim().is_empty() && attachments.is_empty()) {
        return Err(ValidationError::invalid("prompt.text"));
    }
    if attachments.len() > MAX_ATTACHMENTS {
        return Err(ValidationError::count("prompt.attachments"));
    }
    let mut total = text.len();
    for attachment in attachments {
        attachment.validate()?;
        total = total.saturating_add(attachment.bytes());
    }
    if total > MAX_ATTACHMENT_BYTES
        || user_message(text, attachments).content.len() > MAX_MESSAGE_TEXT_BYTES
    {
        return Err(ValidationError::invalid("prompt.size"));
    }
    Ok(())
}

#[must_use]
pub fn user_message(text: &str, attachments: &[PromptAttachment]) -> ChatMessage {
    let mut message = ChatMessage::text(ChatRole::User, text);
    for attachment in attachments {
        // JSON quotes preserve filename boundaries. Attachments are user data,
        // with exactly the same instruction authority as the surrounding prompt.
        let name = serde_json::to_string(attachment.name()).expect("string serialization");
        match attachment {
            PromptAttachment::Text { text, .. } => {
                let _ = write!(
                    message.content,
                    "\n\n[Attached file {name}]\n{text}\n[End attached file]"
                );
            }
            PromptAttachment::Image { image, .. } => {
                let _ = write!(message.content, "\n[Attached image {name}]");
                message.images.push(image.clone());
            }
            PromptAttachment::File { file, .. } => message.files.push(file.clone()),
        }
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_images_are_validated_and_remote_urls_are_rejected() {
        let image = ImageContent {
            mime_type: "image/png".into(),
            data: STANDARD.encode(b"\x89PNG\r\n\x1a\nfixture"),
        };
        assert_eq!(
            ImageContent::from_data_url(&image.data_url()).unwrap(),
            image
        );
        assert!(ImageContent::from_data_url("https://example.com/private.png").is_err());
        assert!(
            ImageContent {
                mime_type: "image/jpeg".into(),
                ..image
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn text_attachments_are_bounded_together_with_the_prompt() {
        let attachments = vec![PromptAttachment::Text {
            name: "notes.md".into(),
            text: "important fact".into(),
        }];
        assert!(validate_prompt("", &attachments).is_err());
        assert!(validate_stored_prompt("", &attachments).is_ok());
        assert!(
            user_message("Question", &attachments)
                .content
                .contains("important fact")
        );
        assert!(validate_prompt(&"a".repeat(MAX_MESSAGE_TEXT_BYTES), &attachments).is_err());
        assert!(validate_prompt("", &[]).is_err());
    }
}
