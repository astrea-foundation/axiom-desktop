//! Conversation-local image generation and optional visual inspection.
use crate::{
    AxiomError, Result,
    app::AppEvent,
    policy::{Effect, ToolAccess},
    provider::InferenceProvider,
    tools::{Tool, ToolContext, ToolResult},
};
use async_trait::async_trait;
use axiom_inference::{ImageContent, PromptAttachment};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use image::{DynamicImage, ImageDecoder as _, ImageFormat, ImageReader, imageops::FilterType};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::Cursor,
    sync::{Arc, Mutex},
};
use tokio_util::sync::CancellationToken;

pub struct ImageGenerationTool {
    pub provider: Arc<dyn InferenceProvider>,
}

pub struct ViewImageTool;

pub(crate) fn configure_runtime_definition(
    tool: &mut axiom_inference::ToolDefinition,
    runtime: &axiom_inference::ImageRuntimePolicy,
) {
    if tool.function.name != "generate" {
        return;
    }
    let p = &runtime.image;
    tool.function.parameters["properties"]["reference_image_ids"]["maxItems"] =
        json!(p.max_references);
    tool.function.parameters["properties"]["aspect_ratio"]["enum"] = json!(p.aspect_ratios);
    tool.function.parameters["properties"]["aspect_ratio"]["default"] =
        json!(p.default_aspect_ratio);
    let size = runtime
        .dimensions(&p.default_aspect_ratio)
        .expect("validated runtime default");
    if p.max_references == 0 {
        tool.function.parameters["properties"]["reference_image_ids"]["description"] =
            json!("Reference editing is unavailable in this installation; omit this argument.");
    }
    tool.function.parameters["properties"]["aspect_ratio"]["description"] = json!(format!(
        "Choose one of the supported aspect ratios. New images default to {}.",
        p.default_aspect_ratio
    ));
    let reference_default = if runtime.references_use_default_size() {
        "Omit aspect_ratio to use the same default size for new images and reference-based generation."
    } else {
        "Omit aspect_ratio to retain a reference's shape."
    };
    tool.function.description = format!(
        "Create or change one image only when the user requests it. Images work best at higher resolutions; the native client computes the largest supported dimensions. Available aspect ratios: {}. New images default to {} at {}×{}. Use up to {} distinct conversation-local reference_image_ids; refer to them as <image1>, <image2>, etc. in order and describe what to preserve. {} Results appear automatically; use view_image if you want to inspect the result before answering.",
        p.aspect_ratios.join(", "),
        p.default_aspect_ratio,
        size.width,
        size.height,
        p.max_references,
        reference_default
    );
}

/// This catalog belongs to exactly one account runtime and conversation. Tool
/// arguments can select its IDs, never a path, URL, database row or other thread.
#[derive(Clone, Default)]
pub struct ImageCatalog(Arc<Mutex<BTreeMap<String, LocalImage>>>);

#[derive(Clone)]
pub struct LocalImage {
    pub id: String,
    pub name: String,
    pub origin: &'static str,
    pub position: usize,
    pub source_image_ids: Vec<String>,
    pub image: Arc<ImageContent>,
}

impl std::fmt::Debug for ImageCatalog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImageCatalog").finish_non_exhaustive()
    }
}

impl ImageCatalog {
    fn insert(&self, mut asset: LocalImage) -> Result<String> {
        asset
            .image
            .validate_bounded(if asset.origin == "uploaded" {
                axiom_inference::MAX_IMAGE_BYTES
            } else {
                axiom_inference::MAX_GENERATED_IMAGE_BYTES
            })
            .map_err(|e| AxiomError::Protocol(e.to_string()))?;
        let mut images = self
            .0
            .lock()
            .map_err(|_| AxiomError::Storage("image catalog unavailable".into()))?;
        if let Some(previous) = images.get(&asset.id) {
            if previous.image != asset.image {
                return Err(AxiomError::Storage("image identity changed".into()));
            }
            return Ok(asset.id);
        }
        let position = images
            .values()
            .map(|image| image.position)
            .max()
            .unwrap_or(0)
            + 1;
        let id = asset.id.clone();
        asset.position = position;
        images.insert(id.clone(), asset);
        Ok(id)
    }

    pub fn add_uploads(
        &self,
        session: &str,
        turn: &str,
        attachments: &[PromptAttachment],
    ) -> Result<()> {
        for (index, attachment) in attachments.iter().enumerate() {
            if let PromptAttachment::Image { name, image } = attachment {
                image
                    .validate()
                    .map_err(|e| AxiomError::Protocol(e.to_string()))?;
                let identity = serde_json::to_vec(&(
                    "axiom-local-image-v1",
                    session,
                    turn,
                    index,
                    &image.data,
                ))?;
                let id = format!("img_{}", &hex::encode(Sha256::digest(identity))[..32]);
                self.insert(LocalImage {
                    id,
                    name: name.clone(),
                    origin: "uploaded",
                    position: 0,
                    source_image_ids: Vec::new(),
                    image: Arc::new(image.clone()),
                })?;
            }
        }
        Ok(())
    }

    pub fn add_generated(&self, image: &axiom_secure_client::GeneratedImage) -> Result<String> {
        if image.request_id.len() != 32
            || !image
                .request_id
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            || !image.usage.response_verified
            || !image.usage.settled
        {
            return Err(AxiomError::Storage(
                "verified generated image required".into(),
            ));
        }
        let raw = STANDARD
            .decode(&image.data)
            .map_err(|_| AxiomError::Storage("invalid image bytes".into()))?;
        if hex::encode(Sha256::digest(raw)) != image.sha256 {
            return Err(AxiomError::Storage("image hash differs".into()));
        }
        self.insert(LocalImage {
            id: format!("img_{}", image.request_id),
            name: "Axiom image".into(),
            origin: if image.source_image_ids.is_empty() {
                "generated"
            } else {
                "edited"
            },
            position: 0,
            source_image_ids: image.source_image_ids.clone(),
            image: Arc::new(ImageContent {
                mime_type: image.mime_type.clone(),
                data: image.data.clone(),
            }),
        })
    }

    pub fn resolve(&self, id: &str) -> Result<LocalImage> {
        self.0.lock().map_err(|_| AxiomError::Storage("image catalog unavailable".into()))?
            .get(id).cloned().ok_or_else(|| AxiomError::Tool("Image is not available in this conversation. Use an image ID from this thread or attach the image here.".into()))
    }

    /// Make a bounded vision copy while retaining the original for editing,
    /// display and saving. No file paths or remote URLs are resolved here.
    pub async fn view(&self, id: &str, cancellation: CancellationToken) -> Result<ImageContent> {
        let source = self.resolve(id)?;
        tokio::select! {
            biased;
            () = cancellation.cancelled() => Err(AxiomError::Cancelled),
            result = tokio::task::spawn_blocking(move || vision_copy(&source.image)) => {
                result.map_err(|_| AxiomError::Tool("Could not prepare the image for viewing.".into()))?
            }
        }
    }

    pub fn manifest(&self) -> Result<String> {
        let images = self
            .0
            .lock()
            .map_err(|_| AxiomError::Storage("image catalog unavailable".into()))?;
        let mut ordered = images.values().collect::<Vec<_>>();
        ordered.sort_by_key(|a| a.position);
        Ok(serde_json::to_string(&ordered.into_iter().map(|a|json!({"image_id":a.id,"name":a.name,"origin":a.origin,"position":a.position,"source_image_ids":a.source_image_ids})).collect::<Vec<_>>())?)
    }
}

fn vision_copy(source: &ImageContent) -> Result<ImageContent> {
    let invalid = || AxiomError::Tool("This image could not be decoded for viewing.".into());
    let raw = STANDARD.decode(&source.data).map_err(|_| invalid())?;
    let format = ImageFormat::from_mime_type(&source.mime_type).ok_or_else(invalid)?;
    let mut reader = ImageReader::with_format(Cursor::new(raw), format);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().map_err(|_| invalid())?;
    let (width, height) = decoder.dimensions();
    if u64::from(width) * u64::from(height) > 16 * 1024 * 1024 {
        return Err(AxiomError::Tool(
            "This image exceeds the viewing size limit.".into(),
        ));
    }
    let orientation = decoder.orientation().map_err(|_| invalid())?;
    let mut raster = DynamicImage::from_decoder(decoder).map_err(|_| invalid())?;
    raster.apply_orientation(orientation);
    let pixels = u64::from(raster.width()) * u64::from(raster.height());
    if pixels > 1_048_576 {
        let root = pixels.isqrt();
        let divisor = root + u64::from(root * root < pixels);
        let width = u32::try_from(u64::from(raster.width()) * 1024 / divisor)
            .map_err(|_| invalid())?
            .max(1);
        let height = u32::try_from(u64::from(raster.height()) * 1024 / divisor)
            .map_err(|_| invalid())?
            .max(1);
        raster = raster.resize_exact(width, height, FilterType::Triangle);
    }
    // Eight-bit RGBA bounds even incompressible PNG output below 5 MiB and
    // keeps transparency. Re-encoding also strips private source metadata.
    let mut output = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(raster.to_rgba8())
        .write_to(&mut output, ImageFormat::Png)
        .map_err(|_| invalid())?;
    let image = ImageContent {
        mime_type: "image/png".into(),
        data: STANDARD.encode(output.into_inner()),
    };
    image.validate().map_err(|_| invalid())?;
    Ok(image)
}

#[async_trait]
impl Tool for ViewImageTool {
    fn name(&self) -> &'static str {
        "view_image"
    }
    fn description(&self) -> &'static str {
        "Inspect an uploaded, generated or edited image from this conversation. Use the exact image_id from the image inventory or an earlier generate result. The image is returned as visual input so you can check its appearance before answering or deciding on further edits. Generated images are not automatically visible to you. Viewing does not create or modify an image and does not require a separate user request for image creation."
    }
    fn parameters(&self) -> Value {
        json!({"type":"object", "properties":{
            "image_id":{"type":"string","pattern":"^img_[0-9a-f]{32}$",
            "description":"Exact ID of an uploaded or generated image in this conversation. Paths and URLs are not accepted."}},
            "required":["image_id"], "additionalProperties":false})
    }
    fn access(&self) -> ToolAccess {
        ToolAccess::Image
    }
    fn effects(&self, _: &ToolContext, _: &Value) -> Result<Vec<Effect>> {
        Ok(vec![])
    }
    async fn execute(
        &self,
        context: &ToolContext,
        arguments: Value,
        cancellation: CancellationToken,
    ) -> Result<ToolResult> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Arguments {
            image_id: String,
        }
        let args: Arguments = serde_json::from_value(arguments).map_err(|_| {
            AxiomError::Tool("view_image requires one valid conversation-local image_id.".into())
        })?;
        let id = args.image_id;
        if !valid_image_id(&id) {
            return Err(AxiomError::Tool(
                "view_image requires one valid conversation-local image_id.".into(),
            ));
        }
        let image = context.images.view(&id, cancellation).await?;
        let mut result = ToolResult::success(format!(
            "Viewed image {id}. The image is supplied as visual input after the tool responses."
        ));
        result.images.push(image);
        Ok(result)
    }
}

fn image_result(
    context: &ToolContext,
    image: axiom_secure_client::GeneratedImage,
) -> Result<ToolResult> {
    let id = context.images.add_generated(&image)?;
    let mut result = ToolResult::success(format!(
        "Created one {}×{} image. Image ID: {id}. It is displayed in this conversation. Cost: {} microUSD. Call view_image with this image_id to inspect the result, or generate with this image_id in reference_image_ids for further changes; do not invent a download URL.",
        image.width,
        image.height,
        image.usage.cost_microusd.as_deref().unwrap_or("unknown")
    ));
    result.events.push(AppEvent::RequestUsageUpdated {
        usage: image.usage.clone(),
    });
    result.events.push(AppEvent::ImageGenerated {
        call_id: String::new(),
        image: Box::new(image),
    });
    Ok(result)
}

#[async_trait]
impl Tool for ImageGenerationTool {
    fn name(&self) -> &'static str {
        "generate"
    }
    fn description(&self) -> &'static str {
        "Create one image with Axiom's encrypted image service only when the user requests image creation or changes. Provide a self-contained prompt and optional aspect_ratio such as 1:1, 16:9, 9:16, 3:2 or 4:3. Images work best at higher resolutions: the native client chooses the largest supported image with that exact aspect ratio on the 32-pixel grid, within 2,359,296 pixels. New images default to 1:1 square at 1536×1536. Available aspect ratios: 1:1, 2:3, 3:2, 3:4, 4:3, 9:16, 16:9, 1:2, 2:1, 9:21 and 21:9. To edit or use references, supply up to five exact conversation-local image IDs in reference_image_ids. In the prompt refer to them as <image1>, <image2>, etc. in that array's order; describe each reference's role and what to preserve. Without an aspect_ratio, reference-based calls preserve the first reference's output dimensions or aspect ratio within the limits. The result is displayed automatically. You may call view_image to inspect it before answering."
    }
    fn parameters(&self) -> Value {
        json!({"type":"object", "properties":{
            "prompt":{"type":"string", "minLength":1, "maxLength":8192},
            "reference_image_ids":{"type":"array", "maxItems":5,"uniqueItems":true,
                "items":{"type":"string","pattern":"^img_[0-9a-f]{32}$"},
                "description":"Ordered references from this conversation. First is <image1>, second <image2>, etc. Omit or pass [] for text-only generation."},
            "aspect_ratio":{"type":"string","enum":IMAGE_ASPECT_RATIOS,"default":"1:1",
                "description":"Width:height, for example 1:1, 16:9, 9:16, 3:2 or 4:3. The client chooses the largest supported dimensions at this ratio. Omit to use 1536×1536 square for a new image or the signed installation default for references."}},
            "required":["prompt"], "additionalProperties":false})
    }

    fn access(&self) -> ToolAccess {
        ToolAccess::Image
    }
    fn effects(&self, _: &ToolContext, _: &Value) -> Result<Vec<Effect>> {
        Ok(vec![])
    }
    async fn execute(
        &self,
        context: &ToolContext,
        arguments: Value,
        cancellation: CancellationToken,
    ) -> Result<ToolResult> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Arguments {
            prompt: String,
            #[serde(default)]
            reference_image_ids: Vec<String>,
            aspect_ratio: Option<String>,
            width: Option<u32>,
            height: Option<u32>,
        }
        let args: Arguments = serde_json::from_value(arguments).map_err(|_| {
            AxiomError::Tool(
                "generate requires a prompt and up to five ordered reference_image_ids".into(),
            )
        })?;
        if args.prompt.trim().is_empty() || args.prompt.len() > 8192 {
            return Err(AxiomError::Tool(
                "Image instructions must contain 1–8192 UTF-8 bytes.".into(),
            ));
        }
        let runtime = self
            .provider
            .image_capabilities(cancellation.child_token())
            .await?;
        runtime
            .validate()
            .map_err(|e| AxiomError::Provider(e.into()))?;
        // Resolve before dispatch. An invalid or foreign reference must never
        // silently fall back to a new generation or reach the provider.
        if args.reference_image_ids.len() > runtime.image.max_references {
            return Err(AxiomError::Tool(
                "Use at most five reference images.".into(),
            ));
        }
        let mut unique = std::collections::BTreeSet::new();
        let mut sources = Vec::new();
        for id in &args.reference_image_ids {
            if !valid_image_id(id) || !unique.insert(id) {
                return Err(AxiomError::Tool(
                    "Use distinct valid image IDs from this conversation.".into(),
                ));
            }
            sources.push(context.images.resolve(id)?);
        }
        for tail in args.prompt.split("<image").skip(1) {
            let Some((label, _)) = tail.split_once('>') else {
                continue;
            };
            if label.is_empty() || !label.bytes().all(|c| c.is_ascii_digit()) {
                continue;
            }
            let index = label.parse::<usize>().unwrap_or(0);
            if index == 0 || index > sources.len() || index.to_string() != label {
                return Err(AxiomError::Tool(
                    "Each <imageN> label must refer to the supplied reference array.".into(),
                ));
            }
        }
        let mut copies = Vec::new();
        for source in &sources {
            copies.push(
                context
                    .images
                    .view(&source.id, cancellation.clone())
                    .await?,
            );
        }
        let references = sources
            .iter()
            .zip(&copies)
            .map(|(source, image)| (source.id.as_str(), image))
            .collect::<Vec<_>>();
        if args.aspect_ratio.is_some() && (args.width.is_some() || args.height.is_some()) {
            return Err(AxiomError::Tool(
                "Provide aspect_ratio or legacy width and height, not both.".into(),
            ));
        }
        let dimensions = if let Some(ratio) = args.aspect_ratio.as_deref() {
            Some(
                runtime
                    .dimensions(ratio)
                    .map_err(|e| AxiomError::Tool(e.into()))?,
            )
        } else {
            match (args.width, args.height) {
                (None, None) if runtime.references_use_default_size() => Some(
                    runtime
                        .dimensions(&runtime.image.default_aspect_ratio)
                        .map_err(|e| AxiomError::Tool(e.into()))?,
                ),
                (None, None) => sources
                    .first()
                    .map(|source| {
                        reference_output_dimensions_with_limit(source, runtime.image.max_pixels)
                    })
                    .transpose()?
                    .flatten(),
                (Some(width), Some(height)) => {
                    let size = axiom_inference::ImageDimensions { width, height };
                    size.validate().map_err(|_| {
                        AxiomError::Tool(
                        "Output dimensions must use a 32-pixel grid, with area at most 1536×1536."
                            .into(),
                    )
                    })?;
                    Some(size)
                }
                _ => {
                    return Err(AxiomError::Tool(
                        "Provide width and height together.".into(),
                    ));
                }
            }
        };
        if dimensions
            .is_some_and(|d| u64::from(d.width) * u64::from(d.height) > runtime.image.max_pixels)
        {
            return Err(AxiomError::Tool(
                "Output dimensions exceed the current image service limit.".into(),
            ));
        }
        let (sender, mut receiver) = tokio::sync::mpsc::channel(32);
        let request = axiom_inference::ImageGenerationRequest {
            chat_model_id: &context.model_id,
            prompt: &args.prompt,
            references: &references,
            dimensions,
        };
        let operation = self
            .provider
            .image_request_with_progress(request, sender, cancellation);
        tokio::pin!(operation);
        let mut open = true;
        let image = loop {
            tokio::select! {
                result = &mut operation => {
                    while let Ok(progress) = receiver.try_recv() { report_progress(context, progress).await?; }
                    break result?;
                }
                update = receiver.recv(), if open => {
                    if let Some(progress) = update { report_progress(context, progress).await?; }
                    else { open = false; }
                }
            }
        };
        image_result(context, image)
    }
}

/// Exact ratios use reduced integer grid units; increasing the scale always
/// increases area, so the last valid scale is the largest supported image.
#[cfg(test)]
pub(crate) fn dimensions_for_aspect_ratio(ratio: &str) -> Result<axiom_inference::ImageDimensions> {
    let invalid = || {
        AxiomError::Tool("Use an aspect ratio such as 1:1, 16:9, 9:16, 3:2 or 4:3 that fits the supported image limits.".into())
    };
    if !IMAGE_ASPECT_RATIOS.contains(&ratio) {
        return Err(invalid());
    }
    let (width, height) = ratio.split_once(':').ok_or_else(invalid)?;
    if width.is_empty()
        || height.is_empty()
        || width.len() > 4
        || height.len() > 4
        || !width
            .bytes()
            .chain(height.bytes())
            .all(|byte| byte.is_ascii_digit())
    {
        return Err(invalid());
    }
    let width: u32 = width.parse().map_err(|_| invalid())?;
    let height: u32 = height.parse().map_err(|_| invalid())?;
    if width == 0 || height == 0 {
        return Err(invalid());
    }
    let (mut a, mut b) = (width, height);
    while b != 0 {
        (a, b) = (b, a % b);
    }
    let (width, height) = (width / a, height / a);
    for scale in (1..=(256 / width).min(256 / height)).rev() {
        let size = axiom_inference::ImageDimensions {
            width: width * 32 * scale,
            height: height * 32 * scale,
        };
        if size.validate().is_ok() {
            return Ok(size);
        }
    }
    Err(invalid())
}

const IMAGE_ASPECT_RATIOS: &[&str] = &[
    "1:1", "4:3", "3:4", "3:2", "2:3", "16:9", "9:16", "2:1", "1:2", "21:9", "9:21",
];

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "reference geometry is positive and bounded before grid rounding"
)]
fn reference_output_dimensions_with_limit(
    source: &LocalImage,
    max_pixels: u64,
) -> Result<Option<axiom_inference::ImageDimensions>> {
    let raw = STANDARD
        .decode(&source.image.data)
        .map_err(|_| AxiomError::Tool("Invalid reference bytes.".into()))?;
    let (width, height) = ImageReader::new(Cursor::new(raw))
        .with_guessed_format()
        .map_err(|_| AxiomError::Tool("Invalid reference image.".into()))?
        .into_dimensions()
        .map_err(|_| AxiomError::Tool("Invalid reference dimensions.".into()))?;
    // Small references retain their dimensions through the conditioning copy.
    // Larger references need an explicit output size to preserve editing size.
    if u64::from(width) * u64::from(height) <= 1_048_576.min(max_pixels) {
        return Ok(None);
    }
    let ratio = f64::from(width) / f64::from(height);
    if !(1.0 / 32.0..=32.0).contains(&ratio) {
        return Err(AxiomError::Tool(
            "Reference aspect ratio exceeds the image limits.".into(),
        ));
    }
    let scale = (f64::from(
        u32::try_from(max_pixels)
            .map_err(|_| AxiomError::Tool("Invalid image pixel limit.".into()))?,
    ) / (f64::from(width) * f64::from(height)))
    .sqrt()
    .min(1.0)
    .min(8192.0 / f64::from(width.max(height)))
    .max(256.0 / f64::from(width.min(height)));
    let mut width = ((f64::from(width) * scale / 32.0) as u32 * 32).max(256);
    let mut height = ((f64::from(height) * scale / 32.0) as u32 * 32).max(256);
    while u64::from(width) * u64::from(height) > max_pixels {
        if width >= height {
            width -= 32;
        } else {
            height -= 32;
        }
    }
    Ok(Some(axiom_inference::ImageDimensions { width, height }))
}

async fn report_progress(
    context: &ToolContext,
    progress: axiom_inference::ImageGenerationProgress,
) -> Result<()> {
    if let Some(output) = &context.progress {
        output
            .events
            .send(AppEvent::ImageGenerationProgress {
                call_id: output.call_id.clone(),
                progress,
            })
            .await
            .map_err(|_| AxiomError::Cancelled)?;
    }
    Ok(())
}

/// A missing reference means generation; an explicitly null/invalid reference
/// is an error, not permission to generate a different image.
fn valid_image_id(id: &str) -> bool {
    id.len() == 36
        && id.starts_with("img_")
        && id.as_bytes()[4..]
            .iter()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::app::{PermissionProfile, SessionId};
    use std::{
        path::PathBuf,
        sync::atomic::{AtomicUsize, Ordering},
    };

    #[test]
    fn signed_runtime_updates_the_advertised_tool_defaults_and_ratios() {
        let mut definition: axiom_inference::ToolDefinition = serde_json::from_value(json!({
            "type":"function", "function":{"name":"generate", "description":"legacy",
                "parameters":{"properties":{"reference_image_ids":{},"aspect_ratio":{}}}}
        }))
        .unwrap();
        let mut runtime = axiom_inference::ImageRuntimePolicy::default();
        runtime.image.default_aspect_ratio = "1:1".into();
        runtime.image.aspect_ratios = vec!["1:1".into(), "6:5".into()];
        runtime.image.max_references = 3;
        runtime.validate().unwrap();
        configure_runtime_definition(&mut definition, &runtime);
        assert_eq!(
            definition.function.parameters["properties"]["aspect_ratio"]["enum"],
            json!(["1:1", "6:5"])
        );
        assert_eq!(
            definition.function.parameters["properties"]["aspect_ratio"]["default"],
            "1:1"
        );
        assert_eq!(
            definition.function.parameters["properties"]["reference_image_ids"]["maxItems"],
            3
        );
        assert!(definition.function.description.contains("1536×1536"));
        assert!(definition.function.description.contains("6:5"));
        assert!(!definition.function.description.contains("0.9"));
    }

    #[test]
    fn aspect_ratios_select_the_largest_exact_supported_grid_and_reject_extremes() {
        for (ratio, width, height) in [
            ("1:1", 1536, 1536),
            ("2:3", 1216, 1824),
            ("3:2", 1824, 1216),
            ("16:9", 2048, 1152),
            ("9:16", 1152, 2048),
            ("21:9", 2240, 960),
        ] {
            assert_eq!(
                dimensions_for_aspect_ratio(ratio).unwrap(),
                axiom_inference::ImageDimensions { width, height }
            );
        }
        for ratio in IMAGE_ASPECT_RATIOS {
            let size = dimensions_for_aspect_ratio(ratio).unwrap();
            size.validate().unwrap();
            let (a, b) = ratio.split_once(':').unwrap();
            let (a, b) = (a.parse::<u32>().unwrap(), b.parse::<u32>().unwrap());
            assert_eq!(size.width * b, size.height * a);
            let (mut gcd, mut remainder) = (a, b);
            while remainder != 0 {
                (gcd, remainder) = (remainder, gcd % remainder);
            }
            let next = axiom_inference::ImageDimensions {
                width: size.width + 32 * (a / gcd),
                height: size.height + 32 * (b / gcd),
            };
            assert!(
                next.validate().is_err(),
                "{ratio} must already be at its largest exact size"
            );
        }
        for ratio in [
            "300:1", "1:300", "32:1", "0:1", "1:0", "100:101", "", "1.5:1", "1:1:1",
        ] {
            assert!(dimensions_for_aspect_ratio(ratio).is_err(), "{ratio}");
        }
    }

    fn png(width: u32, height: u32) -> ImageContent {
        let mut output = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            width,
            height,
            image::Rgba([20, 80, 140, 128]),
        ))
        .write_to(&mut output, ImageFormat::Png)
        .unwrap();
        ImageContent {
            mime_type: "image/png".into(),
            data: STANDARD.encode(output.into_inner()),
        }
    }

    pub(crate) fn generated_fixture() -> axiom_secure_client::GeneratedImage {
        let image = png(1536, 1536);
        let raw = STANDARD.decode(&image.data).unwrap();
        let request_id = "12".repeat(16);
        axiom_secure_client::GeneratedImage {
            request_id: request_id.clone(),
            data: image.data,
            mime_type: image.mime_type,
            width: 1536,
            height: 1536,
            sha256: hex::encode(Sha256::digest(raw)),
            source_image_ids: Vec::new(),
            chat_model_id: None,
            evidence: serde_json::from_value(json!({
                "state":"verified", "provider_id":"axiom-direct", "model_id":"direct:qwen3.8-27b",
                "attestation_protocol":"synthetic-unit-fixture", "e2ee_protocol":"synthetic-unit-fixture",
                "e2ee_encryption_version":1, "trust_policy_version":"unit-test", "verified_at_unix_seconds":1,
                "model_key_fingerprint":"00".repeat(32), "tls_spki_fingerprint":null, "checks":[], "provider_claims":[]
            })).unwrap(),
            usage: axiom_inference::RequestUsage {
                request_id,
                provider_id: "axiom-direct".into(),
                model_id: "direct:qwen3.8-27b".into(),
                settled: true,
                response_verified: true,
                ..Default::default()
            },
        }
    }

    #[test]
    fn old_image_references_remain_available_after_many_generations() {
        let catalog = ImageCatalog::default();
        let mut image = generated_fixture();
        for index in 0..270 {
            image.request_id = format!("{index:032x}");
            image.usage.request_id.clone_from(&image.request_id);
            catalog.add_generated(&image).unwrap();
        }
        assert!(catalog.resolve(&format!("img_{:032x}", 0)).is_ok());
        assert!(catalog.resolve(&format!("img_{:032x}", 269)).is_ok());
        assert_eq!(catalog.0.lock().unwrap().len(), 270);
    }

    #[test]
    fn old_image_references_remain_available_after_64_mib_of_encoded_assets() {
        let catalog = ImageCatalog::default();
        let mut raw = STANDARD.decode(png(64, 64).data).unwrap();
        // PNG permits trailing bytes; pad a valid small raster to exercise catalog
        // byte retention without spending the test on compressing large rasters.
        raw.resize(1024 * 1024, 0);
        let image = Arc::new(ImageContent {
            mime_type: "image/png".into(),
            data: STANDARD.encode(raw),
        });
        for index in 0..50 {
            catalog
                .insert(LocalImage {
                    id: format!("img_{index:032x}"),
                    name: format!("upload-{index}.png"),
                    origin: "uploaded",
                    position: 0,
                    source_image_ids: Vec::new(),
                    image: image.clone(),
                })
                .unwrap();
        }
        assert!(50 * image.data.len() > 64 * 1024 * 1024);
        for index in 0..50 {
            assert_eq!(
                catalog.resolve(&format!("img_{index:032x}")).unwrap().image,
                image
            );
        }
        assert_eq!(catalog.0.lock().unwrap().len(), 50);
    }

    #[tokio::test]
    async fn view_image_returns_visual_input_for_uploads_generated_images_and_edits() {
        let context = ToolContext {
            model_id: "vision".into(),
            session_id: SessionId::new(),
            cwd: PathBuf::from("."),
            permission_profile: PermissionProfile::Web,
            images: ImageCatalog::default(),
            progress: None,
        };
        let source = png(64, 32);
        context
            .images
            .add_uploads(
                "own",
                "turn",
                &[PromptAttachment::Image {
                    name: "uploaded.png".into(),
                    image: source.clone(),
                }],
            )
            .unwrap();
        let uploaded = id(&context.images);
        let generated = generated_fixture();
        let generated_id = context.images.add_generated(&generated).unwrap();
        let mut edited = generated.clone();
        edited.request_id = "34".repeat(16);
        edited.source_image_ids = vec![generated_id.clone()];
        let edited_id = context.images.add_generated(&edited).unwrap();
        for (image_id, dimensions) in [
            (uploaded.clone(), (64, 32)),
            (generated_id.clone(), (1024, 1024)),
            (edited_id, (1024, 1024)),
        ] {
            let result = ViewImageTool
                .execute(
                    &context,
                    json!({"image_id":image_id}),
                    CancellationToken::new(),
                )
                .await
                .unwrap();
            assert!(result.success);
            assert_eq!(result.images.len(), 1);
            assert!(
                result.events.is_empty(),
                "viewing never requests generation or changes the UI image"
            );
            let viewed = &result.images[0];
            viewed.validate().unwrap();
            let decoded = image::load_from_memory(&STANDARD.decode(&viewed.data).unwrap()).unwrap();
            assert_eq!((decoded.width(), decoded.height()), dimensions);
            assert_eq!(decoded.to_rgba8().get_pixel(0, 0).0, [20, 80, 140, 128]);
            assert!(!result.content.contains(&viewed.data));
            assert!(!format!("{result:?}").contains(&viewed.data));
        }
        assert_eq!(
            context.images.resolve(&uploaded).unwrap().image.as_ref(),
            &source
        );
        assert_eq!(
            context.images.resolve(&generated_id).unwrap().image.data,
            generated.data
        );
    }

    #[tokio::test]
    async fn view_image_rejects_malformed_foreign_corrupt_and_cancelled_inputs() {
        let context = ToolContext {
            model_id: "vision".into(),
            session_id: SessionId::new(),
            cwd: PathBuf::from("."),
            permission_profile: PermissionProfile::Web,
            images: ImageCatalog::default(),
            progress: None,
        };
        let foreign = ImageCatalog::default();
        foreign
            .add_uploads("other-thread", "turn", &[upload()])
            .unwrap();
        for arguments in [
            json!({}),
            json!({"image_id":null}),
            json!({"image_id":""}),
            json!({"image_id":"/tmp/private.png"}),
            json!({"image_id":"https://example.com/private.png"}),
            json!({"image_id":"img_ABCDEF0123456789abcdef0123456789"}),
            json!({"image_id":id(&foreign)}),
            json!({"image_id":id(&foreign),"path":"/tmp/private.png"}),
        ] {
            assert!(
                ViewImageTool
                    .execute(&context, arguments, CancellationToken::new())
                    .await
                    .is_err()
            );
        }
        context
            .images
            .add_uploads(
                "own",
                "turn",
                &[PromptAttachment::Image {
                    name: "corrupt.png".into(),
                    image: ImageContent {
                        mime_type: "image/png".into(),
                        data: STANDARD.encode(b"\x89PNG\r\n\x1a\ncorrupt"),
                    },
                }],
            )
            .unwrap();
        let own = id(&context.images);
        assert!(
            ViewImageTool
                .execute(&context, json!({"image_id":own}), CancellationToken::new())
                .await
                .is_err()
        );
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        assert!(matches!(
            ViewImageTool
                .execute(&context, json!({"image_id":own}), cancellation)
                .await,
            Err(AxiomError::Cancelled)
        ));
        assert!(vision_copy(&png(8193, 1)).is_err());
    }

    #[test]
    fn vision_copy_accepts_generated_pngs_above_the_upload_byte_limit() {
        use image::{
            ExtendedColorType, ImageEncoder as _,
            codecs::png::{CompressionType, FilterType as PngFilter, PngEncoder},
        };
        let mut state = 0x0123_4567_u32;
        let noise = (0..1536 * 1536 * 4)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state.to_le_bytes()[0]
            })
            .collect::<Vec<_>>();
        let mut raw = Vec::new();
        PngEncoder::new_with_quality(&mut raw, CompressionType::Fast, PngFilter::NoFilter)
            .write_image(&noise, 1536, 1536, ExtendedColorType::Rgba8)
            .unwrap();
        assert!(raw.len() > axiom_inference::MAX_IMAGE_BYTES);
        let source = ImageContent {
            mime_type: "image/png".into(),
            data: STANDARD.encode(raw),
        };
        source
            .validate_bounded(axiom_inference::MAX_GENERATED_IMAGE_BYTES)
            .unwrap();
        let viewed = vision_copy(&source).unwrap();
        viewed.validate().unwrap();
        let decoded = image::load_from_memory(&STANDARD.decode(viewed.data).unwrap()).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (1024, 1024));
    }

    fn upload() -> PromptAttachment {
        PromptAttachment::Image {
            name: "private-upload.png".into(),
            image: png(64, 64),
        }
    }
    fn id(catalog: &ImageCatalog) -> String {
        serde_json::from_str::<Value>(&catalog.manifest().unwrap()).unwrap()[0]["image_id"]
            .as_str()
            .unwrap()
            .into()
    }
    #[test]
    fn uploads_have_stable_thread_scoped_ids_without_bytes_in_the_model_inventory() {
        let own = ImageCatalog::default();
        let other = ImageCatalog::default();
        let attachment = upload();
        own.add_uploads(
            "account-one-thread",
            "turn-one",
            std::slice::from_ref(&attachment),
        )
        .unwrap();
        let first = id(&own);
        own.add_uploads(
            "account-one-thread",
            "turn-one",
            std::slice::from_ref(&attachment),
        )
        .unwrap();
        assert_eq!(id(&own), first);
        other
            .add_uploads("account-two-thread", "turn-one", &[attachment])
            .unwrap();
        assert_ne!(id(&other), first);
        assert!(other.resolve(&first).is_err());
        let source = own.resolve(&first).unwrap();
        assert!(!own.manifest().unwrap().contains(&source.image.data));
        assert!(!format!("{own:?}").contains(&source.image.data));
        assert!(own.resolve("https://example.com/image.png").is_err());
        assert!(own.resolve("/tmp/private.png").is_err());
    }

    struct Spy(AtomicUsize);

    #[derive(Default)]
    struct ReferenceSpy(Mutex<Vec<Vec<String>>>);

    #[async_trait]
    impl InferenceProvider for ReferenceSpy {
        async fn stream(
            &self,
            _: axiom_inference::InferenceRequest,
            _: tokio::sync::mpsc::Sender<axiom_inference::ProviderEvent>,
            _: CancellationToken,
        ) -> Result<axiom_inference::AssistantTurn> {
            unreachable!()
        }

        async fn image_request_with_progress(
            &self,
            request: axiom_inference::ImageGenerationRequest<'_>,
            _: tokio::sync::mpsc::Sender<axiom_inference::ImageGenerationProgress>,
            _: CancellationToken,
        ) -> Result<axiom_secure_client::GeneratedImage> {
            for (_, image) in request.references {
                image.validate().unwrap();
                let raw = STANDARD.decode(&image.data).unwrap();
                let image = image::load_from_memory(&raw).unwrap();
                assert!(u64::from(image.width()) * u64::from(image.height()) <= 1_048_576);
            }
            if !request.references.is_empty() {
                assert_eq!(
                    request.dimensions,
                    Some(axiom_inference::ImageDimensions {
                        width: 1536,
                        height: 1536
                    })
                );
            }
            self.0.lock().unwrap().push(
                request
                    .references
                    .iter()
                    .map(|(id, _)| (*id).into())
                    .collect(),
            );
            Err(AxiomError::Provider("synthetic reference capture".into()))
        }
    }

    #[tokio::test]
    async fn generate_keeps_five_references_ordered_and_drops_them_for_the_next_call() {
        let provider = Arc::new(ReferenceSpy::default());
        let tool = ImageGenerationTool {
            provider: provider.clone(),
        };
        let context = ToolContext {
            model_id: "vision".into(),
            session_id: SessionId::new(),
            cwd: PathBuf::from("."),
            permission_profile: PermissionProfile::Web,
            images: ImageCatalog::default(),
            progress: None,
        };
        for i in 0..6 {
            context
                .images
                .add_uploads(
                    "owned-thread",
                    &format!("turn-{i}"),
                    &[PromptAttachment::Image {
                        name: format!("synthetic-{i}.png"),
                        image: if i == 0 { png(1536, 1536) } else { png(64, 64) },
                    }],
                )
                .unwrap();
        }
        let manifest: Value = serde_json::from_str(&context.images.manifest().unwrap()).unwrap();
        let ids = manifest
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["image_id"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        let ordered = vec![
            ids[0].clone(),
            ids[4].clone(),
            ids[3].clone(),
            ids[2].clone(),
            ids[1].clone(),
        ];
        let original = context.images.resolve(&ids[0]).unwrap().image.data.clone();
        let error = tool.execute(&context, json!({"prompt":"Use <image1> with colors from <image5>", "reference_image_ids":ordered}), CancellationToken::new()).await.unwrap_err();
        assert!(error.to_string().contains("synthetic reference capture"));
        let error = tool
            .execute(
                &context,
                json!({"prompt":"A new cup"}),
                CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("synthetic reference capture"));
        assert_eq!(*provider.0.lock().unwrap(), vec![ordered, vec![]]);
        assert_eq!(
            context.images.resolve(&ids[0]).unwrap().image.data,
            original
        );
        for arguments in [
            json!({"prompt":"Cup", "reference_image_ids":ids}),
            json!({"prompt":"Cup", "reference_image_ids":[ids[0],ids[0]]}),
            json!({"prompt":"Use <image2>", "reference_image_ids":[ids[0]]}),
            json!({"prompt":"Cup", "reference_image_ids":null}),
            json!({"prompt":"Cup", "reference_image_ids":[format!("img_{}", "f".repeat(32))]}),
        ] {
            assert!(
                tool.execute(&context, arguments, CancellationToken::new())
                    .await
                    .is_err()
            );
        }
        assert_eq!(provider.0.lock().unwrap().len(), 2);
        assert_eq!(tool.name(), "generate");
        assert_eq!(
            tool.parameters()["properties"]["reference_image_ids"]["maxItems"],
            5
        );
    }

    #[derive(Default)]
    struct ModelSpy(Mutex<Vec<(String, &'static str)>>);
    #[async_trait]
    impl InferenceProvider for ModelSpy {
        async fn stream(
            &self,
            _: axiom_inference::InferenceRequest,
            _: tokio::sync::mpsc::Sender<axiom_inference::ProviderEvent>,
            _: CancellationToken,
        ) -> Result<axiom_inference::AssistantTurn> {
            unreachable!()
        }
        async fn generate_image(
            &self,
            model: &str,
            _: &str,
            _: CancellationToken,
        ) -> Result<axiom_secure_client::GeneratedImage> {
            self.0.lock().unwrap().push((model.into(), "generate"));
            Err(AxiomError::Provider("synthetic capture".into()))
        }
        async fn edit_image(
            &self,
            model: &str,
            _: &str,
            _: &str,
            _: &ImageContent,
            _: CancellationToken,
        ) -> Result<axiom_secure_client::GeneratedImage> {
            self.0.lock().unwrap().push((model.into(), "edit"));
            Err(AxiomError::Provider("synthetic capture".into()))
        }
    }

    #[tokio::test]
    async fn image_tools_use_the_current_native_model_and_reject_model_supplied_policy() {
        let provider = Arc::new(ModelSpy::default());
        let generate = ImageGenerationTool {
            provider: provider.clone(),
        };
        let mut context = ToolContext {
            model_id: String::new(),
            session_id: SessionId::new(),
            cwd: PathBuf::from("/tmp"),
            permission_profile: PermissionProfile::Web,
            images: ImageCatalog::default(),
            progress: None,
        };
        context
            .images
            .add_uploads(&context.session_id.to_string(), "turn", &[upload()])
            .unwrap();
        let image_id = id(&context.images);
        let mut expected = Vec::new();
        for model in ["direct:qwen3.8-27b", "near:glm", "tinfoil:qwen"] {
            context.model_id = model.into();
            for editing in [false, true] {
                let tool = &generate;
                let mut args = json!({"prompt":"A blue mug"});
                if editing {
                    args["reference_image_ids"] = json!([image_id]);
                }
                let error = tool
                    .execute(&context, args.clone(), CancellationToken::new())
                    .await
                    .unwrap_err();
                assert!(error.to_string().contains("synthetic capture"));
                expected.push((model.into(), if editing { "edit" } else { "generate" }));
                for extra in [
                    json!({"uncensored":true}),
                    json!({"lora":"uncensored.safetensors"}),
                    json!({"model_id":"direct:qwen3.8-27b"}),
                ] {
                    let mut overridden = args.clone();
                    overridden
                        .as_object_mut()
                        .unwrap()
                        .extend(extra.as_object().unwrap().clone());
                    assert!(
                        tool.execute(&context, overridden, CancellationToken::new())
                            .await
                            .is_err()
                    );
                }
                assert_eq!(*provider.0.lock().unwrap(), expected);
            }
        }
    }
    #[async_trait]
    impl InferenceProvider for Spy {
        async fn stream(
            &self,
            _: axiom_inference::InferenceRequest,
            _: tokio::sync::mpsc::Sender<axiom_inference::ProviderEvent>,
            _: CancellationToken,
        ) -> Result<axiom_inference::AssistantTurn> {
            unreachable!()
        }
        async fn edit_image(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: &ImageContent,
            _: CancellationToken,
        ) -> Result<axiom_secure_client::GeneratedImage> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err(AxiomError::Provider("synthetic capture".into()))
        }
    }
    #[tokio::test]
    async fn edit_resolution_rejects_another_conversations_image_before_provider_dispatch() {
        let provider = Arc::new(Spy(AtomicUsize::new(0)));
        let tool = ImageGenerationTool {
            provider: provider.clone(),
        };
        let context = ToolContext {
            model_id: "test-model".into(),
            session_id: SessionId::new(),
            cwd: PathBuf::from("/tmp"),
            permission_profile: PermissionProfile::Web,
            images: ImageCatalog::default(),
            progress: None,
        };
        for arguments in [
            json!({"prompt":"Make it blue", "image_id":null}),
            json!({"prompt":"Make it blue", "image_id":""}),
            json!({"prompt":"Make it blue", "image_id":"https://example.com/image.png"}),
            json!({"prompt":"Make it blue", "image_id":"img_ABCDEF0123456789abcdef0123456789"}),
            json!({"prompt":"   "}),
            json!({"prompt":"é".repeat(4097)}),
            json!({"prompt":"Make it blue", "image_ids":[]}),
        ] {
            assert!(
                tool.execute(&context, arguments, CancellationToken::new())
                    .await
                    .is_err()
            );
        }
        let foreign = ImageCatalog::default();
        foreign.add_uploads("foreign", "turn", &[upload()]).unwrap();
        assert!(
            tool.execute(
                &context,
                json!({"reference_image_ids":[id(&foreign)],"prompt":"Make it blue"}),
                CancellationToken::new()
            )
            .await
            .is_err()
        );
        assert_eq!(provider.0.load(Ordering::SeqCst), 0);
        context
            .images
            .add_uploads(&context.session_id.to_string(), "turn", &[upload()])
            .unwrap();
        let error = tool
            .execute(
                &context,
                json!({"reference_image_ids":[id(&context.images)],"prompt":"Make it blue"}),
                CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("synthetic capture"));
        assert_eq!(provider.0.load(Ordering::SeqCst), 1);
    }
}
