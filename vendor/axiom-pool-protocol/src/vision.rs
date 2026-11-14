//! Bounded inline images carried inside the authenticated request payload.
use anyhow::{Context, Result, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use image::{ImageDecoder, ImageFormat, ImageReader, Limits};
use serde::{Deserialize, Serialize};
use std::io::Cursor;

pub const MAX_IMAGE_EDGE: u32 = 1024;
pub const MAX_IMAGE_PIXELS: u64 = 1024 * 1024;
pub const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;
pub const MAX_REFERENCES: usize = 5;
pub const MAX_REFERENCE_PIXELS: u64 = 2048 * 2048;
pub const MAX_REFERENCE_BYTES: usize = 20 * 1024 * 1024;
pub const MAX_GENERATED_PIXELS: u64 = 2048 * 2048;
pub const MAX_GENERATED_BYTES: usize = 20 * 1024 * 1024;

/// Generated output has a different bound from the smaller vision copy.
pub fn validate_generated_png(raw: &[u8], width: u32, height: u32) -> Result<()> {
    ensure!(
        raw.len() <= MAX_GENERATED_BYTES
            && (256..=8192).contains(&width)
            && (256..=8192).contains(&height)
            && width.is_multiple_of(32)
            && height.is_multiple_of(32)
            && u64::from(width) * u64::from(height) <= MAX_GENERATED_PIXELS,
        "generated image resource limit"
    );
    let source = reader(raw, 8192, 64 * 1024 * 1024)?;
    ensure!(
        source.format() == Some(ImageFormat::Png),
        "generated PNG required"
    );
    let image = source.decode().context("generated image decoding failed")?;
    ensure!(
        image.width() == width && image.height() == height,
        "generated image dimensions differ"
    );
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InlineImage {
    pub mime_type: String,
    pub data: String,
    pub width: u32,
    pub height: u32,
}

fn decoded(data: &str, max_bytes: usize) -> Result<Vec<u8>> {
    ensure!(
        data.len() <= max_bytes.div_ceil(3) * 4,
        "input image byte limit"
    );
    let raw = STANDARD.decode(data)?;
    ensure!(
        raw.len() <= max_bytes && STANDARD.encode(&raw) == data,
        "canonical bounded input image required"
    );
    Ok(raw)
}

fn reader(raw: &[u8], edge: u32, allocation: u64) -> Result<ImageReader<Cursor<&[u8]>>> {
    let mut reader = ImageReader::new(Cursor::new(raw)).with_guessed_format()?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(edge);
    limits.max_image_height = Some(edge);
    limits.max_alloc = Some(allocation);
    reader.limits(limits);
    Ok(reader)
}

impl InlineImage {
    /// Generation references retain up to 2048² pixels, independently of chat
    /// vision thumbnails. Resize once onto Comfy's grid and strip metadata.
    pub fn from_reference_upload(mime_type: &str, data: &str) -> Result<Self> {
        let raw = decoded(data, MAX_REFERENCE_BYTES)?;
        let format =
            ImageFormat::from_mime_type(mime_type).context("unsupported reference type")?;
        ensure!(
            matches!(
                format,
                ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP | ImageFormat::Gif
            ),
            "unsupported reference type"
        );
        let source = reader(&raw, 8192, 128 * 1024 * 1024)?;
        ensure!(
            source.format() == Some(format),
            "reference MIME/content mismatch"
        );
        let (width, height) = source.into_dimensions()?;
        ensure!(
            width > 0 && height > 0 && u64::from(width) * u64::from(height) <= 32 * 1024 * 1024,
            "source reference pixel limit"
        );
        let mut decoder = reader(&raw, 8192, 128 * 1024 * 1024)?.into_decoder()?;
        let orientation = decoder.orientation()?;
        let mut pixels = image::DynamicImage::from_decoder(decoder)?;
        pixels.apply_orientation(orientation);
        let scale = (MAX_REFERENCE_PIXELS as f64
            / (f64::from(pixels.width()) * f64::from(pixels.height())))
        .sqrt()
        .min(1.0);
        // Round down to avoid upscaling or overshooting the pixel budget. Tiny
        // inputs use Comfy's minimum 32-pixel dimension.
        let width = ((f64::from(pixels.width()) * scale / 32.0).floor() as u32 * 32).max(32);
        let height = ((f64::from(pixels.height()) * scale / 32.0).floor() as u32 * 32).max(32);
        let pixels = resize_rgba(&pixels, width, height);
        let mut output = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(pixels).write_to(&mut output, ImageFormat::Png)?;
        let image = Self {
            mime_type: "image/png".into(),
            data: STANDARD.encode(output.into_inner()),
            width,
            height,
        };
        image.validate_reference()?;
        Ok(image)
    }
    /// Normalize a local upload before encryption. Large images are reduced to
    /// the installed vision bounds; animation uses the first decoded frame.
    pub fn from_upload(mime_type: &str, data: &str) -> Result<Self> {
        let raw = decoded(data, MAX_REFERENCE_BYTES)?;
        let format = match mime_type {
            "image/png" => ImageFormat::Png,
            "image/jpeg" => ImageFormat::Jpeg,
            "image/webp" => ImageFormat::WebP,
            "image/gif" => ImageFormat::Gif,
            _ => anyhow::bail!("unsupported input image format"),
        };
        let source = reader(&raw, 8192, 128 * 1024 * 1024)?;
        ensure!(
            source.format() == Some(format),
            "input image MIME/content mismatch"
        );
        let (width, height) = source.into_dimensions()?;
        ensure!(
            width > 0 && height > 0 && u64::from(width) * u64::from(height) <= 32 * 1024 * 1024,
            "source image pixel limit"
        );
        let image = reader(&raw, 8192, 128 * 1024 * 1024)?
            .decode()
            .context("input image decoding failed")?;
        let image = if image.width() > MAX_IMAGE_EDGE || image.height() > MAX_IMAGE_EDGE {
            image.thumbnail(MAX_IMAGE_EDGE, MAX_IMAGE_EDGE)
        } else {
            image
        };
        let format = if format == ImageFormat::Jpeg {
            ImageFormat::Jpeg
        } else {
            ImageFormat::Png
        };
        let mut output = Cursor::new(Vec::new());
        image.write_to(&mut output, format)?;
        let image = Self {
            mime_type: if format == ImageFormat::Jpeg {
                "image/jpeg"
            } else {
                "image/png"
            }
            .into(),
            data: STANDARD.encode(output.into_inner()),
            width: image.width(),
            height: image.height(),
        };
        image.validate()?;
        Ok(image)
    }

    /// Recheck dimensions, complete decoding and byte bounds in the worker.
    /// A client-supplied width, MIME label or base64 prefix is not trusted.
    pub fn validate(&self) -> Result<u64> {
        self.validate_bounded(MAX_IMAGE_EDGE, MAX_IMAGE_PIXELS, MAX_IMAGE_BYTES)
    }

    pub fn validate_reference(&self) -> Result<u64> {
        ensure!(self.mime_type == "image/png", "PNG reference required");
        self.validate_bounded(8192, MAX_REFERENCE_PIXELS, MAX_REFERENCE_BYTES)
    }

    fn validate_bounded(&self, edge: u32, pixels: u64, max_bytes: usize) -> Result<u64> {
        ensure!(
            self.width > 0
                && self.height > 0
                && self.width <= edge
                && self.height <= edge
                && self.pixels() <= pixels,
            "input image dimensions exceed capability"
        );
        let raw = decoded(&self.data, max_bytes)?;
        let expected = match self.mime_type.as_str() {
            "image/png" => ImageFormat::Png,
            "image/jpeg" => ImageFormat::Jpeg,
            _ => anyhow::bail!("normalized PNG or JPEG required"),
        };
        let source = reader(&raw, edge, 128 * 1024 * 1024)?;
        ensure!(
            source.format() == Some(expected),
            "input image MIME/content mismatch"
        );
        let image = source.decode().context("input image decoding failed")?;
        ensure!(
            image.width() == self.width && image.height() == self.height,
            "input image dimensions differ from authenticated metadata"
        );
        Ok(u64::from(self.width) * u64::from(self.height))
    }

    pub fn pixels(&self) -> u64 {
        u64::from(self.width) * u64::from(self.height)
    }

    pub fn data_url(&self) -> String {
        format!("data:{};base64,{}", self.mime_type, self.data)
    }
}

/// Lanczos filtering expects linear light and premultiplied alpha. Filtering
/// straight RGBA mixes hidden colors into edges and creates dark/color halos.
fn resize_rgba(source: &image::DynamicImage, width: u32, height: u32) -> image::RgbaImage {
    let source = source.to_rgba8();
    if source.dimensions() == (width, height) {
        return source;
    }
    let linear: [f32; 256] = std::array::from_fn(|i| {
        let value = i as f32 / 255.0;
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    });
    let premultiplied = image::Rgba32FImage::from_fn(source.width(), source.height(), |x, y| {
        let pixel = source.get_pixel(x, y).0;
        let alpha = f32::from(pixel[3]) / 255.0;
        image::Rgba([
            linear[usize::from(pixel[0])] * alpha,
            linear[usize::from(pixel[1])] * alpha,
            linear[usize::from(pixel[2])] * alpha,
            alpha,
        ])
    });
    let resized = image::imageops::resize(
        &premultiplied,
        width,
        height,
        image::imageops::FilterType::Lanczos3,
    );
    image::RgbaImage::from_fn(width, height, |x, y| {
        let pixel = resized.get_pixel(x, y).0;
        let alpha = pixel[3].clamp(0.0, 1.0);
        if alpha <= 1e-6 {
            return image::Rgba([0, 0, 0, 0]);
        }
        let srgb = |value: f32| {
            let value = (value / alpha).clamp(0.0, 1.0);
            let value = if value <= 0.0031308 {
                value * 12.92
            } else {
                1.055 * value.powf(1.0 / 2.4) - 0.055
            };
            (value * 255.0).round() as u8
        };
        image::Rgba([
            srgb(pixel[0]),
            srgb(pixel[1]),
            srgb(pixel[2]),
            (alpha * 255.0).round() as u8,
        ])
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn png(width: u32, height: u32) -> String {
        let mut out = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            width,
            height,
            image::Rgb([0, 0, 255]),
        ))
        .write_to(&mut out, ImageFormat::Png)
        .unwrap();
        STANDARD.encode(out.into_inner())
    }

    #[test]
    fn uploads_are_normalized_with_aspect_ratio_and_rechecked() {
        let image = InlineImage::from_upload("image/png", &png(1280, 720)).unwrap();
        assert_eq!((image.width, image.height), (1024, 576));
        assert_eq!(image.validate().unwrap(), 1024 * 576);
        assert!(image.data_url().starts_with("data:image/png;base64,"));
    }

    #[test]
    fn remote_urls_wrong_mime_truncation_and_forged_dimensions_fail() {
        assert!(InlineImage::from_upload("image/png", "https://attacker.example/image").is_err());
        let raw = png(16, 16);
        assert_eq!(
            InlineImage::from_upload("image/png", &raw).unwrap().width,
            16
        );
        assert!(InlineImage::from_upload("image/jpeg", &raw).is_err());
        let mut image = InlineImage::from_upload("image/png", &raw).unwrap();
        image.width = 32;
        assert!(image.validate().is_err());
        image.width = 16;
        image.data.truncate(16);
        assert!(image.validate().is_err());
    }

    #[test]
    fn references_keep_2k_detail_without_expanding_chat_vision_bounds() {
        let reference = InlineImage::from_reference_upload("image/png", &png(2048, 2048)).unwrap();
        assert_eq!((reference.width, reference.height), (2048, 2048));
        assert_eq!(
            reference.validate_reference().unwrap(),
            MAX_REFERENCE_PIXELS
        );
        assert!(reference.validate().is_err());
        let chat = InlineImage::from_upload("image/png", &reference.data).unwrap();
        assert_eq!((chat.width, chat.height), (1024, 1024));
        let raw = STANDARD.decode(&reference.data).unwrap();
        validate_generated_png(&raw, 2048, 2048).unwrap();
        assert!(validate_generated_png(&raw, 2080, 2048).is_err());
    }

    #[test]
    fn reference_resize_preserves_aspect_and_stays_on_the_model_grid() {
        let reference = InlineImage::from_reference_upload("image/png", &png(3000, 4000)).unwrap();
        assert_eq!((reference.width, reference.height), (1760, 2336));
        assert!(reference.pixels() <= MAX_REFERENCE_PIXELS);
        assert!(reference.width.is_multiple_of(32) && reference.height.is_multiple_of(32));
        let small = InlineImage::from_reference_upload("image/png", &png(512, 256)).unwrap();
        assert_eq!((small.width, small.height), (512, 256));
    }

    #[test]
    fn lanczos_resize_uses_linear_light_and_ignores_hidden_transparent_colors() {
        let source = image::DynamicImage::ImageRgba8(image::RgbaImage::from_fn(2, 1, |x, _| {
            if x == 0 {
                image::Rgba([255, 0, 0, 0])
            } else {
                image::Rgba([0, 0, 255, 255])
            }
        }));
        let result = resize_rgba(&source, 1, 1);
        let pixel = result.get_pixel(0, 0).0;
        assert_eq!(&pixel[..3], &[0, 0, 255]);
        assert!((127..=128).contains(&pixel[3]));
        let source = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(2, 1, |x, _| {
            image::Rgb([if x == 0 { 0 } else { 255 }; 3])
        }));
        let result = resize_rgba(&source, 1, 1);
        assert!((187..=189).contains(&result.get_pixel(0, 0)[0]));
    }
}
