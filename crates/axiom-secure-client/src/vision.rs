//! Disposable chat-model copies. Saved assets and generation references retain
//! their original local bytes; only the provider-bound request is changed.
use std::{
    collections::BTreeMap,
    io::Cursor,
    sync::{Arc, Mutex},
};

use axiom_inference::{ImageContent, InferenceRequest, ProviderFailureKind};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use image::{DynamicImage, ImageDecoder as _, ImageFormat, ImageReader, imageops::FilterType};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use crate::{Result, SecureClientError};

const MAX_VISION_PIXELS: u64 = 1024 * 1024;
pub const MAX_VISION_BYTES: usize = 512 * 1024;
const MIN_VISION_PIXELS: u64 = 256 * 256;
const CACHE_BYTES: usize = 16 * 1024 * 1024;
const CACHE_ENTRIES: usize = 64;

/// In-memory, bounded image variants owned by one account runtime or catalog.
/// Keys include the encoding policy and source content; no paths or URLs.
#[derive(Default)]
pub struct VisionCache(Mutex<CacheState>);

#[derive(Default)]
struct CacheState {
    entries: BTreeMap<[u8; 32], (u64, ImageContent)>,
    clock: u64,
    bytes: usize,
}

impl VisionCache {
    pub fn clear(&self) {
        if let Ok(mut cache) = self.0.lock() {
            *cache = CacheState::default();
        }
    }

    pub fn prepare(&self, source: &ImageContent) -> Result<ImageContent> {
        self.prepare_bounded(source, MAX_VISION_BYTES, &CancellationToken::new())
    }

    fn prepare_bounded(
        &self,
        source: &ImageContent,
        limit: usize,
        cancel: &CancellationToken,
    ) -> Result<ImageContent> {
        if cancel.is_cancelled() {
            return Err(SecureClientError::cancelled());
        }
        let mut digest = Sha256::new();
        digest.update(b"axiom-vision-webp-v2-q85-70-floor256\0");
        digest.update(limit.to_le_bytes());
        digest.update(source.mime_type.as_bytes());
        digest.update([0]);
        digest.update(source.data.as_bytes());
        let key: [u8; 32] = digest.finalize().into();
        if let Ok(mut cache) = self.0.lock() {
            cache.clock = cache.clock.wrapping_add(1);
            let clock = cache.clock;
            if let Some((used, copy)) = cache.entries.get_mut(&key) {
                *used = clock;
                return Ok(copy.clone());
            }
        }
        let copy = encode(source, limit, cancel)?;
        if let Ok(mut cache) = self.0.lock() {
            if let Some((_, previous)) = cache.entries.remove(&key) {
                cache.bytes -= previous.data.len();
            }
            while cache.bytes.saturating_add(copy.data.len()) > CACHE_BYTES
                || cache.entries.len() >= CACHE_ENTRIES
            {
                let Some(oldest) = cache
                    .entries
                    .iter()
                    .min_by_key(|(_, (used, _))| used)
                    .map(|(key, _)| *key)
                else {
                    break;
                };
                if let Some((_, old)) = cache.entries.remove(&oldest) {
                    cache.bytes -= old.data.len();
                }
            }
            cache.clock = cache.clock.wrapping_add(1);
            let clock = cache.clock;
            cache.bytes += copy.data.len();
            cache.entries.insert(key, (clock, copy.clone()));
        }
        Ok(copy)
    }
}

fn invalid_image() -> SecureClientError {
    SecureClientError::new(
        ProviderFailureKind::InvalidRequest,
        "This image could not be prepared for the model. Try attaching it again.",
    )
}

/// Prepare a bounded, metadata-free lossy WebP for visual inspection without
/// changing the source used for saving, display or image generation.
pub fn vision_image(source: &ImageContent) -> Result<ImageContent> {
    encode(source, MAX_VISION_BYTES, &CancellationToken::new())
}

fn encode(
    source: &ImageContent,
    byte_limit: usize,
    cancellation: &CancellationToken,
) -> Result<ImageContent> {
    source
        .validate_bounded(axiom_inference::MAX_GENERATED_IMAGE_BYTES)
        .map_err(|_| invalid_image())?;
    let raw = STANDARD.decode(&source.data).map_err(|_| invalid_image())?;
    let format = ImageFormat::from_mime_type(&source.mime_type).ok_or_else(invalid_image)?;
    let mut reader = ImageReader::new(Cursor::new(raw.as_slice()))
        .with_guessed_format()
        .map_err(|_| invalid_image())?;
    if reader.format() != Some(format) {
        return Err(invalid_image());
    }
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().map_err(|_| invalid_image())?;
    let (width, height) = decoder.dimensions();
    if u64::from(width) * u64::from(height) > 16 * 1024 * 1024 {
        return Err(invalid_image());
    }
    let orientation = decoder.orientation().map_err(|_| invalid_image())?;
    let mut raster = DynamicImage::from_decoder(decoder).map_err(|_| invalid_image())?;
    raster.apply_orientation(orientation);
    let pixels = u64::from(raster.width()) * u64::from(raster.height());
    if format == ImageFormat::WebP
        && pixels <= MAX_VISION_PIXELS
        && raw.len() <= byte_limit
        && clean_webp(&raw)
    {
        return Ok(source.clone());
    }
    if pixels > MAX_VISION_PIXELS {
        let root = pixels.isqrt();
        let divisor = root + u64::from(root * root < pixels);
        let width = u32::try_from(u64::from(raster.width()) * 1024 / divisor)
            .map_err(|_| invalid_image())?
            .max(1);
        let height = u32::try_from(u64::from(raster.height()) * 1024 / divisor)
            .map_err(|_| invalid_image())?
            .max(1);
        raster = raster.resize_exact(width, height, FilterType::Triangle);
    }
    let mut rgba = raster.into_rgba8();
    let minimum_pixels = MIN_VISION_PIXELS.min(u64::from(rgba.width()) * u64::from(rgba.height()));
    loop {
        // Reduce quality before resolution. Alpha remains available on inputs
        // with transparency; source metadata is never passed to the encoder.
        for quality in [85.0, 70.0] {
            if cancellation.is_cancelled() {
                return Err(SecureClientError::cancelled());
            }
            let bytes = webp::Encoder::from_rgba(&rgba, rgba.width(), rgba.height())
                .encode_simple(false, quality)
                .map_err(|_| invalid_image())?;
            if !bytes.is_empty() && bytes.len() <= byte_limit {
                return Ok(ImageContent {
                    mime_type: "image/webp".into(),
                    data: STANDARD.encode(&*bytes),
                });
            }
        }
        let (width, height) = (rgba.width(), rgba.height());
        let next_width = (width * 3 / 4).max(1);
        let next_height = (height * 3 / 4).max(1);
        if u64::from(next_width) * u64::from(next_height) < minimum_pixels {
            return Err(SecureClientError::new(
                ProviderFailureKind::InvalidRequest,
                crate::request::REQUEST_TOO_LARGE,
            ));
        }
        rgba = image::imageops::resize(&rgba, next_width, next_height, FilterType::Triangle);
    }
}

fn clean_webp(bytes: &[u8]) -> bool {
    let mut offset = 12;
    while let Some(header) = bytes.get(offset..offset + 8) {
        if !matches!(&header[..4], b"VP8 " | b"VP8X" | b"ALPH") {
            return false;
        }
        let size = u32::from_le_bytes(header[4..8].try_into().expect("chunk size")) as usize;
        offset = offset
            .saturating_add(8)
            .saturating_add(size)
            .saturating_add(size % 2);
    }
    offset == bytes.len()
}

/// Prepare useful, fixed-policy copies before planning the model request.
/// Byte pressure must trigger compaction, never progressively unreadable copies.
pub async fn prepare_vision_request(
    request: InferenceRequest,
    cache: Arc<VisionCache>,
    cancellation: &CancellationToken,
) -> Result<InferenceRequest> {
    prepare_with_cache(request, usize::MAX, cache, cancellation).await
}

pub(crate) async fn prepare_near_request(
    model: &axiom_inference::ModelInfo,
    request: InferenceRequest,
    cache: Arc<VisionCache>,
    cancellation: &CancellationToken,
) -> Result<InferenceRequest> {
    let current = request.messages.last().cloned();
    let mut prepared = prepare_vision_request(request, Arc::clone(&cache), cancellation).await?;
    let Some(source) = current.filter(|message| !message.images.is_empty()) else {
        return Ok(prepared);
    };
    let mut messages = prepared
        .messages
        .iter()
        .filter(|message| {
            matches!(
                message.role,
                axiom_inference::ChatRole::System | axiom_inference::ChatRole::Developer
            )
        })
        .cloned()
        .collect::<Vec<_>>();
    messages.push(prepared.messages.last().expect("current message").clone());
    let mut fresh = InferenceRequest {
        model: prepared.model.clone(),
        request_id: prepared.request_id.clone(),
        messages,
        tools: prepared.tools.clone(),
        tool_choice: prepared.tool_choice.clone(),
        parallel_tool_calls: prepared.parallel_tool_calls,
        reasoning_effort: prepared.reasoning_effort,
        thinking_mode: prepared.thinking_mode,
        max_output_tokens: prepared.max_output_tokens,
        sampling: prepared.sampling.clone(),
        response_format: prepared.response_format.clone(),
        stream: prepared.stream,
    };
    if crate::near_request_bytes(model, &fresh)? <= crate::NEAR_REQUEST_BUDGET_BYTES {
        return Ok(prepared);
    }
    // Plan only the current upload against its actual text/tool overhead.
    // Older images keep stable variants and trigger history compaction instead
    // of taking an ever-smaller equal share of a global allowance.
    let last = fresh.messages.last_mut().expect("current message");
    let sizes = last
        .images
        .iter()
        .map(|image| image.data.len() / 4 * 3)
        .collect::<Vec<_>>();
    for image in &mut last.images {
        image.data.clear();
    }
    let base = crate::near_request_bytes(model, &fresh)?;
    let mut remaining = crate::NEAR_REQUEST_BUDGET_BYTES
        .saturating_sub(base)
        .saturating_sub(4096)
        / 2
        * 3
        / 4;
    let mut order = (0..sizes.len()).collect::<Vec<_>>();
    order.sort_by_key(|index| sizes[*index]);
    let mut limits = vec![0; sizes.len()];
    for (position, index) in order.into_iter().enumerate() {
        let budget = (remaining / (sizes.len() - position))
            .min(sizes[index])
            .min(MAX_VISION_BYTES);
        limits[index] = budget;
        remaining = remaining.saturating_sub(budget);
    }
    let worker_cancel = cancellation.child_token();
    let _cancel_on_drop = worker_cancel.clone().drop_guard();
    let task = tokio::task::spawn_blocking(move || {
        let images = source
            .images
            .iter()
            .zip(limits)
            .map(|(image, limit)| cache.prepare_bounded(image, limit, &worker_cancel))
            .collect::<Result<Vec<_>>>()?;
        prepared
            .messages
            .last_mut()
            .expect("current message")
            .images = images;
        Ok(prepared)
    });
    tokio::select! {
        biased;
        () = cancellation.cancelled() => Err(SecureClientError::cancelled()),
        result = task => result.map_err(|_| invalid_image())?,
    }
}

pub(crate) async fn prepare_request(
    request: InferenceRequest,
    image_byte_budget: usize,
    cancellation: &CancellationToken,
) -> Result<InferenceRequest> {
    prepare_with_cache(
        request,
        image_byte_budget,
        Arc::new(VisionCache::default()),
        cancellation,
    )
    .await
}

async fn prepare_with_cache(
    mut request: InferenceRequest,
    image_byte_budget: usize,
    cache: Arc<VisionCache>,
    cancellation: &CancellationToken,
) -> Result<InferenceRequest> {
    request.validate().map_err(|_| {
        SecureClientError::new(
            ProviderFailureKind::InvalidRequest,
            "Invalid inference request.",
        )
    })?;
    let count = request
        .messages
        .iter()
        .map(|m| m.images.len())
        .sum::<usize>();
    if cancellation.is_cancelled() {
        return Err(SecureClientError::cancelled());
    }
    if count == 0 {
        return Ok(request);
    }
    let byte_limit = (image_byte_budget / count).min(MAX_VISION_BYTES);
    if byte_limit < 1024 {
        return Err(SecureClientError::new(
            ProviderFailureKind::InvalidRequest,
            crate::request::REQUEST_TOO_LARGE,
        ));
    }
    let worker_cancel = cancellation.child_token();
    let _cancel_on_drop = worker_cancel.clone().drop_guard();
    let task = tokio::task::spawn_blocking(move || {
        // Decode and encode sequentially so image count does not multiply peak
        // raster memory. Do not mutate the caller's stored history or catalog.
        for message in &mut request.messages {
            for image in &mut message.images {
                *image = cache.prepare_bounded(image, byte_limit, &worker_cancel)?;
            }
        }
        Ok(request)
    });
    tokio::select! {
        biased;
        () = cancellation.cancelled() => Err(SecureClientError::cancelled()),
        result = task => result.map_err(|_| invalid_image())?,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use axiom_inference::{ChatMessage, ChatRole};
    use image::{ExtendedColorType, ImageEncoder as _, codecs::png::PngEncoder};

    pub(crate) fn noisy_png(width: u32, height: u32) -> ImageContent {
        let mut state = 0x0123_4567_u32;
        let pixels = (0..width * height * 3)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state.to_le_bytes()[0]
            })
            .collect::<Vec<_>>();
        let mut raw = Vec::new();
        PngEncoder::new(&mut raw)
            .write_image(&pixels, width, height, ExtendedColorType::Rgb8)
            .unwrap();
        ImageContent {
            mime_type: "image/png".into(),
            data: STANDARD.encode(raw),
        }
    }

    #[test]
    fn lossy_webp_preserves_shape_alpha_and_the_source_image() {
        let pixels = image::RgbaImage::from_pixel(2048, 1024, image::Rgba([20, 80, 140, 128]));
        let mut raw = Vec::new();
        PngEncoder::new(&mut raw)
            .write_image(&pixels, 2048, 1024, ExtendedColorType::Rgba8)
            .unwrap();
        let source = ImageContent {
            mime_type: "image/png".into(),
            data: STANDARD.encode(&raw),
        };
        let copy = vision_image(&source).unwrap();
        assert_eq!(copy.mime_type, "image/webp");
        let bytes = STANDARD.decode(copy.data).unwrap();
        assert!(bytes.len() <= MAX_VISION_BYTES);
        assert!(
            bytes.windows(4).any(|chunk| chunk == b"VP8 "),
            "vision copy must use lossy VP8"
        );
        let decoded = image::load_from_memory(&bytes).unwrap();
        assert!(u64::from(decoded.width()) * u64::from(decoded.height()) <= MAX_VISION_PIXELS);
        assert!(decoded.width().abs_diff(2 * decoded.height()) <= 1);
        assert_eq!(decoded.to_rgba8().get_pixel(0, 0).0[3], 128);
        assert_eq!(STANDARD.decode(source.data).unwrap(), raw);
    }

    #[tokio::test]
    async fn request_budget_covers_all_images_in_history_without_dropping_any() {
        let source = noisy_png(384, 256);
        let original = source.clone();
        let mut message = ChatMessage::text(ChatRole::User, "Keep these references.");
        message.images = vec![source; 3];
        let request = InferenceRequest::streaming("vision", vec![message.clone(), message], vec![]);
        let prepared =
            prepare_vision_request(request.clone(), Arc::default(), &CancellationToken::new())
                .await
                .unwrap();
        let images = prepared
            .messages
            .iter()
            .flat_map(|m| &m.images)
            .collect::<Vec<_>>();
        assert_eq!(images.len(), 6);
        assert!(images.iter().all(|i| i.mime_type == "image/webp"));
        let total = images
            .iter()
            .map(|i| STANDARD.decode(&i.data).unwrap().len())
            .sum::<usize>();
        assert!(total <= 6 * MAX_VISION_BYTES);
        assert_eq!(prepared.messages[0].content, request.messages[0].content);
        assert_eq!(request.messages[0].images[0], original);
    }

    #[tokio::test]
    async fn near_current_upload_uses_actual_overhead_without_shrinking_older_images() {
        let source = noisy_png(1024, 768);
        let cache = Arc::new(VisionCache::default());
        let standard = cache.prepare(&source).unwrap();
        let model = axiom_inference::ModelInfo {
            id: "near-vision".into(),
            provider_id: "near".into(),
            upstream_model: "provider/vision".into(),
            supports_images: true,
            ..Default::default()
        };
        let mut older = ChatMessage::text(ChatRole::User, "An earlier reference.");
        older.images = vec![standard.clone(); 10];
        let mut current = ChatMessage::text(ChatRole::User, "Compare these new references.");
        current.images = vec![source.clone(); 10];
        let request = InferenceRequest::streaming(&model.id, vec![older, current], vec![]);
        let prepared =
            prepare_near_request(&model, request.clone(), cache, &CancellationToken::new())
                .await
                .unwrap();
        assert!(
            prepared.messages[0]
                .images
                .iter()
                .all(|copy| copy == &standard)
        );
        let fresh =
            InferenceRequest::streaming(&model.id, vec![prepared.messages[1].clone()], vec![]);
        assert!(
            crate::near_request_bytes(&model, &fresh).unwrap() <= crate::NEAR_REQUEST_BUDGET_BYTES
        );
        assert_eq!(fresh.messages[0].images.len(), 10);
        for copy in &fresh.messages[0].images {
            let decoded = image::load_from_memory(&STANDARD.decode(&copy.data).unwrap()).unwrap();
            assert!(u64::from(decoded.width()) * u64::from(decoded.height()) >= MIN_VISION_PIXELS);
        }
        assert!(
            request.messages[1]
                .images
                .iter()
                .all(|image| image == &source)
        );
        // History remains above the wire budget so the caller must summarize
        // it, rather than reducing its preview quality again on every turn.
        assert!(
            crate::near_request_bytes(&model, &prepared).unwrap()
                > crate::NEAR_REQUEST_BUDGET_BYTES
        );
    }

    #[test]
    fn quality_floor_refuses_unreadable_variants_and_cache_reuses_exact_pixels() {
        let source = noisy_png(384, 256);
        let cache = VisionCache::default();
        let first = cache.prepare(&source).unwrap();
        let second = cache.prepare(&source).unwrap();
        assert_eq!(first, second);
        assert_eq!(cache.0.lock().unwrap().entries.len(), 1);
        // Passing a prepared image to another provider/cache must not add a
        // second lossy generation or change the current visual identity.
        assert_eq!(VisionCache::default().prepare(&first).unwrap(), first);
        assert!(encode(&source, 1024, &CancellationToken::new()).is_err());
        cache.clear();
        assert!(cache.0.lock().unwrap().entries.is_empty());
    }

    #[test]
    fn image_caches_are_bounded_and_isolated() {
        let cache = VisionCache::default();
        for value in 0..80_u8 {
            let pixels = image::RgbImage::from_pixel(16, 16, image::Rgb([value, 80, 140]));
            let mut raw = Vec::new();
            PngEncoder::new(&mut raw)
                .write_image(&pixels, 16, 16, ExtendedColorType::Rgb8)
                .unwrap();
            cache
                .prepare(&ImageContent {
                    mime_type: "image/png".into(),
                    data: STANDARD.encode(raw),
                })
                .unwrap();
        }
        let state = cache.0.lock().unwrap();
        assert!(state.entries.len() <= CACHE_ENTRIES);
        assert!(state.bytes <= CACHE_BYTES);
        assert!(VisionCache::default().0.lock().unwrap().entries.is_empty());
    }

    #[tokio::test]
    async fn malformed_and_cancelled_images_fail_before_provider_dispatch() {
        let source = ImageContent {
            mime_type: "image/png".into(),
            data: STANDARD.encode(b"\x89PNG\r\n\x1a\ncorrupt"),
        };
        assert!(vision_image(&source).is_err());
        let mut message = ChatMessage::text(ChatRole::User, "Inspect this.");
        message.images.push(source);
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let error = prepare_request(
            InferenceRequest::streaming("vision", vec![message], vec![]),
            1024,
            &cancellation,
        )
        .await
        .unwrap_err();
        assert_eq!(error.kind(), ProviderFailureKind::Cancelled);
    }
}
