//! Disposable chat-model copies. Saved assets and generation references retain
//! their original local bytes; only the provider-bound request is changed.
use std::io::Cursor;

use axiom_inference::{ImageContent, InferenceRequest, ProviderFailureKind};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use image::{DynamicImage, ImageDecoder as _, ImageFormat, ImageReader, imageops::FilterType};
use tokio_util::sync::CancellationToken;

use crate::{Result, SecureClientError};

const MAX_VISION_PIXELS: u64 = 1024 * 1024;
const MAX_VISION_BYTES: usize = 512 * 1024;

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
    let mut reader = ImageReader::new(Cursor::new(raw))
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
    loop {
        // Reduce quality before resolution. Alpha remains available on inputs
        // with transparency; source metadata is never passed to the encoder.
        for quality in [85.0, 70.0, 55.0] {
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
        if width == 1 && height == 1 {
            return Err(SecureClientError::new(
                ProviderFailureKind::InvalidRequest,
                crate::request::REQUEST_TOO_LARGE,
            ));
        }
        rgba = image::imageops::resize(
            &rgba,
            (width * 3 / 4).max(1),
            (height * 3 / 4).max(1),
            FilterType::Triangle,
        );
    }
}

pub(crate) async fn prepare_request(
    mut request: InferenceRequest,
    image_byte_budget: usize,
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
                *image = encode(image, byte_limit, &worker_cancel)?;
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
        let prepared = prepare_request(request.clone(), 48 * 1024, &CancellationToken::new())
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
        assert!(total <= 48 * 1024);
        assert_eq!(prepared.messages[0].content, request.messages[0].content);
        assert_eq!(request.messages[0].images[0], original);
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
