//! Provider-neutral image generation results shared by image-capable providers.
use crate::SecurityEvidence;
use serde::Deserialize;

/// A verified artifact, kept out of model history and scoped to its native tool.
#[derive(Clone, PartialEq, Eq, serde::Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeneratedImage {
    pub request_id: String,
    pub data: String,
    pub mime_type: String,
    pub width: u32,
    pub height: u32,
    pub sha256: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_image_ids: Vec<String>,
    /// Native-selected origin, retained so regeneration keeps the same policy
    /// after the user switches the thread's chat model. Never a tool argument.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_model_id: Option<String>,
    pub evidence: SecurityEvidence,
    pub usage: axiom_inference::RequestUsage,
}

impl std::fmt::Debug for GeneratedImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GeneratedImage")
            .field("request_id", &self.request_id)
            .field("data", &"[image bytes omitted]")
            .field("mime_type", &self.mime_type)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("sha256", &self.sha256)
            .field("source_image_ids", &self.source_image_ids)
            .field("chat_model_id", &self.chat_model_id)
            .field("evidence", &self.evidence)
            .field("usage", &self.usage)
            .finish()
    }
}
