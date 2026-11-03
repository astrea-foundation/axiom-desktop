//! Bounded inline images carried inside the authenticated request payload.
use anyhow::{Context, Result, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use image::{ImageFormat, ImageReader, Limits};
use serde::{Deserialize, Serialize};
use std::io::Cursor;

pub const MAX_IMAGES: usize = 8;
pub const MAX_IMAGE_EDGE: u32 = 1024;
pub const MAX_IMAGE_PIXELS: u64 = 1024 * 1024;
pub const MAX_INPUT_PIXELS: u64 = MAX_IMAGES as u64 * MAX_IMAGE_PIXELS;
pub const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InlineImage {
    pub mime_type: String,
    pub data: String,
    pub width: u32,
    pub height: u32,
}

fn decoded(data: &str) -> Result<Vec<u8>> {
    ensure!(
        data.len() <= MAX_IMAGE_BYTES.div_ceil(3) * 4,
        "input image byte limit"
    );
    let raw = STANDARD.decode(data)?;
    ensure!(
        raw.len() <= MAX_IMAGE_BYTES && STANDARD.encode(&raw) == data,
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
    /// Normalize a local upload before encryption. Large images are reduced to
    /// the installed vision bounds; animation uses the first decoded frame.
    pub fn from_upload(mime_type: &str, data: &str) -> Result<Self> {
        let raw = decoded(data)?;
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
        ensure!(
            self.width > 0
                && self.height > 0
                && self.width <= MAX_IMAGE_EDGE
                && self.height <= MAX_IMAGE_EDGE,
            "input image dimensions exceed capability"
        );
        let raw = decoded(&self.data)?;
        let expected = match self.mime_type.as_str() {
            "image/png" => ImageFormat::Png,
            "image/jpeg" => ImageFormat::Jpeg,
            _ => anyhow::bail!("normalized PNG or JPEG required"),
        };
        let source = reader(&raw, MAX_IMAGE_EDGE, 16 * 1024 * 1024)?;
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
}
