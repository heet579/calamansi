//! Built-in image decoding, encoding and simple edits.

use crate::error::{CoreError, Result};
use crate::format::Format;
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{DynamicImage, ImageFormat, ImageReader, Limits, Rgb, RgbImage, RgbaImage};
use std::io::{Seek, Write};
use std::path::Path;

/// Inputs above these limits are refused rather than risking memory exhaustion.
pub const MAX_INPUT_BYTES: u64 = 100 * 1024 * 1024;
pub const MAX_PIXELS: u64 = 40_000_000;
pub const ICO_MAX_SIDE: u32 = 256;

pub fn check_size(path: &Path) -> Result<()> {
    let len = std::fs::metadata(path)?.len();
    if len > MAX_INPUT_BYTES {
        return Err(CoreError::TooLarge(format!(
            "file is {} MB; the limit is {} MB",
            len / (1024 * 1024),
            MAX_INPUT_BYTES / (1024 * 1024)
        )));
    }
    Ok(())
}

pub fn load(path: &Path) -> Result<DynamicImage> {
    check_size(path)?;
    let (w, h) = ImageReader::open(path)?
        .with_guessed_format()?
        .into_dimensions()?;
    if u64::from(w) * u64::from(h) > MAX_PIXELS {
        return Err(CoreError::TooLarge(format!(
            "image is {w}×{h}; the limit is {} megapixels",
            MAX_PIXELS / 1_000_000
        )));
    }
    let mut reader = ImageReader::open(path)?.with_guessed_format()?;
    let mut limits = Limits::default();
    limits.max_alloc = Some(1024 * 1024 * 1024);
    reader.limits(limits);
    Ok(reader.decode()?)
}

/// Composite onto white, for formats without transparency.
pub fn flatten(img: &DynamicImage) -> RgbImage {
    if !img.color().has_alpha() {
        return img.to_rgb8();
    }
    let rgba = img.to_rgba8();
    let mut out = RgbImage::new(rgba.width(), rgba.height());
    for (x, y, p) in rgba.enumerate_pixels() {
        let a = u32::from(p[3]);
        let blend = |c: u8| ((u32::from(c) * a + 255 * (255 - a) + 127) / 255) as u8;
        out.put_pixel(x, y, Rgb([blend(p[0]), blend(p[1]), blend(p[2])]));
    }
    out
}

pub fn encode_jpeg<W: Write>(img: &DynamicImage, quality: u8, out: &mut W) -> Result<()> {
    let rgb = flatten(img);
    let mut enc = JpegEncoder::new_with_quality(out, quality.clamp(1, 100));
    enc.encode_image(&rgb)?;
    Ok(())
}

/// Encode `img` as `to`. `quality` (1–100) applies to JPEG.
pub fn encode<W: Write + Seek>(
    img: &DynamicImage,
    to: Format,
    quality: u8,
    out: &mut W,
) -> Result<()> {
    match to {
        Format::Jpeg => encode_jpeg(img, quality, out),
        Format::Png => Ok(img.write_to(out, ImageFormat::Png)?),
        // The built-in WebP encoder is lossless; it favours compatibility over size.
        Format::Webp => {
            Ok(DynamicImage::ImageRgba8(img.to_rgba8()).write_to(out, ImageFormat::WebP)?)
        }
        Format::Bmp => {
            Ok(DynamicImage::ImageRgba8(img.to_rgba8()).write_to(out, ImageFormat::Bmp)?)
        }
        Format::Tiff => {
            Ok(DynamicImage::ImageRgba8(img.to_rgba8()).write_to(out, ImageFormat::Tiff)?)
        }
        Format::Gif => {
            Ok(DynamicImage::ImageRgba8(img.to_rgba8()).write_to(out, ImageFormat::Gif)?)
        }
        Format::Ico => {
            let fitted = fit_within(img, ICO_MAX_SIDE);
            Ok(DynamicImage::ImageRgba8(fitted.to_rgba8()).write_to(out, ImageFormat::Ico)?)
        }
        other => Err(CoreError::InvalidInput(format!(
            "{other} isn't an image format the built-in encoder writes"
        ))),
    }
}

/// Scale down so neither side exceeds `max_side`. Never enlarges.
pub fn fit_within(img: &DynamicImage, max_side: u32) -> DynamicImage {
    if img.width() <= max_side && img.height() <= max_side {
        return img.clone();
    }
    img.resize(max_side, max_side, FilterType::Lanczos3)
}

pub fn rotate(img: &DynamicImage, degrees: u16) -> Result<DynamicImage> {
    Ok(match degrees % 360 {
        0 => img.clone(),
        90 => img.rotate90(),
        180 => img.rotate180(),
        270 => img.rotate270(),
        d => {
            return Err(CoreError::InvalidInput(format!(
                "rotation must be a multiple of 90°, not {d}°"
            )))
        }
    })
}

/// Convert a decoded image to RGBA for callers that need raw pixels.
pub fn to_rgba(img: &DynamicImage) -> RgbaImage {
    img.to_rgba8()
}

/// Arrange images in a grid on a white background, each scaled to fit a
/// `cell`-pixel square with a small margin.
pub fn collage(images: &[DynamicImage], cell: u32) -> DynamicImage {
    let n = images.len().max(1) as u32;
    let cols = (n as f32).sqrt().ceil() as u32;
    let rows = n.div_ceil(cols);
    let gap = (cell / 40).max(4);
    let (w, h) = (
        cols * cell + (cols + 1) * gap,
        rows * cell + (rows + 1) * gap,
    );
    let mut canvas = RgbaImage::from_pixel(w, h, image::Rgba([255, 255, 255, 255]));
    for (i, img) in images.iter().enumerate() {
        let (c, r) = (i as u32 % cols, i as u32 / cols);
        let fitted = img.resize(cell, cell, FilterType::Lanczos3).to_rgba8();
        let x = gap + c * (cell + gap) + (cell - fitted.width()) / 2;
        let y = gap + r * (cell + gap) + (cell - fitted.height()) / 2;
        image::imageops::overlay(&mut canvas, &fitted, i64::from(x), i64::from(y));
    }
    DynamicImage::ImageRgba8(canvas)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;
    use std::io::Cursor;

    fn sample(w: u32, h: u32) -> DynamicImage {
        let mut img = RgbaImage::new(w, h);
        for (x, y, p) in img.enumerate_pixels_mut() {
            *p = Rgba([
                (x * 255 / w) as u8,
                (y * 255 / h) as u8,
                128,
                if x < w / 2 { 255 } else { 0 },
            ]);
        }
        DynamicImage::ImageRgba8(img)
    }

    #[test]
    fn encodes_every_native_format_and_decodes_back() {
        let img = sample(64, 48);
        for f in [
            Format::Png,
            Format::Jpeg,
            Format::Webp,
            Format::Bmp,
            Format::Tiff,
            Format::Gif,
            Format::Ico,
        ] {
            let mut buf = Cursor::new(Vec::new());
            encode(&img, f, 85, &mut buf).unwrap();
            let bytes = buf.into_inner();
            assert_eq!(
                crate::format::detect_bytes(&bytes),
                Some(f),
                "{f:?} signature"
            );
            let back = image::load_from_memory(&bytes).unwrap();
            assert_eq!((back.width(), back.height()), (64, 48), "{f:?} dimensions");
        }
    }

    #[test]
    fn jpeg_flattens_transparency_to_white() {
        let img = sample(10, 10);
        let flat = flatten(&img);
        assert_eq!(flat.get_pixel(9, 0), &Rgb([255, 255, 255]));
    }

    #[test]
    fn ico_is_capped_at_256() {
        let img = sample(600, 300);
        let mut buf = Cursor::new(Vec::new());
        encode(&img, Format::Ico, 90, &mut buf).unwrap();
        let back = image::load_from_memory(&buf.into_inner()).unwrap();
        assert_eq!((back.width(), back.height()), (256, 128));
    }

    #[test]
    fn collage_grid() {
        let imgs = vec![sample(40, 20), sample(20, 40), sample(30, 30)];
        let c = collage(&imgs, 100);
        assert_eq!(c.width(), c.height(), "3 images make a 2x2 grid");
        assert_eq!(c.to_rgba8().get_pixel(0, 0)[0], 255, "white margin");
    }

    #[test]
    fn rotation() {
        let img = sample(30, 10);
        assert_eq!(rotate(&img, 90).unwrap().width(), 10);
        assert!(rotate(&img, 45).is_err());
    }
}
