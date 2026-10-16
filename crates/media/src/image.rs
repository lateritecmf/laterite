//! Pixels: what is read from an upload before it is stored (its size, its
//! orientation, with the metadata gone), and how a variant is rendered from
//! it under decode limits.

use bytes::Bytes;
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::codecs::webp::WebPEncoder;
use image::metadata::Orientation;
use image::{DynamicImage, ExtendedColorType, ImageDecoder, ImageEncoder, ImageReader, Limits};
use img_parts::ImageEXIF;
use std::io::Cursor;

use crate::preset::{Format, Mode, Preset};

/// The widest or tallest image that is decoded at all.
pub const MAX_SIDE: u32 = 12_000;
/// The most pixels an image may have and still be decoded.
pub const MAX_PIXELS: u64 = 50_000_000;
/// The most memory one decode may take.
pub const MAX_ALLOC: u64 = 512 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum ImageError {
    #[error("the file is not an image this build can decode")]
    Unsupported,
    #[error("the image is {width} by {height}, past the decode limits")]
    TooLarge { width: u32, height: u32 },
    #[error("decoding failed: {0}")]
    Decode(String),
    #[error("encoding failed: {0}")]
    Encode(String),
}

/// An upload as it is stored: its metadata stripped, its size and orientation
/// read. `orientation` is the EXIF value, 1 to 8, with 1 meaning none.
#[derive(Debug, Clone)]
pub struct Prepared {
    pub bytes: Bytes,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub orientation: u8,
}

/// Reads an upload's size and orientation and strips its EXIF segment,
/// losslessly, before it is hashed and stored. A file that is not a JPEG, PNG
/// or WebP passes through untouched, its size read when it is an image.
pub fn prepare(bytes: Bytes) -> Prepared {
    let kind = infer::get(&bytes).map(|t| t.mime_type()).unwrap_or("");
    let orientation = read_orientation(&bytes);
    let (width, height) = match header_dimensions(&bytes) {
        Some((w, h)) if matches!(orientation, 5..=8) => (Some(h), Some(w)),
        Some((w, h)) => (Some(w), Some(h)),
        None => (None, None),
    };
    let stripped = match kind {
        "image/jpeg" => img_parts::jpeg::Jpeg::from_bytes(bytes.clone())
            .ok()
            .map(|mut j| {
                j.set_exif(None);
                j.encoder().bytes()
            }),
        "image/png" => img_parts::png::Png::from_bytes(bytes.clone())
            .ok()
            .map(|mut p| {
                p.set_exif(None);
                p.encoder().bytes()
            }),
        "image/webp" => img_parts::webp::WebP::from_bytes(bytes.clone())
            .ok()
            .map(|mut w| {
                w.set_exif(None);
                w.encoder().bytes()
            }),
        _ => None,
    };
    Prepared {
        bytes: stripped.unwrap_or(bytes),
        width,
        height,
        orientation,
    }
}

/// The size in the file's header, without decoding the pixels.
pub fn header_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
}

fn read_orientation(bytes: &[u8]) -> u8 {
    let Ok(reader) = ImageReader::new(Cursor::new(bytes)).with_guessed_format() else {
        return 1;
    };
    let Ok(mut decoder) = reader.into_decoder() else {
        return 1;
    };
    decoder.orientation().map(|o| o.to_exif()).unwrap_or(1)
}

/// A rendered variant.
#[derive(Debug, Clone)]
pub struct Rendered {
    pub bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub content_type: &'static str,
    pub extension: &'static str,
}

/// Decodes `bytes` under the limits, bakes `orientation` in, fits the image to
/// the preset and encodes it. CPU-bound: call it from a blocking thread.
pub fn render(bytes: &[u8], preset: &Preset, orientation: u8) -> Result<Rendered, ImageError> {
    let (w, h) = header_dimensions(bytes).ok_or(ImageError::Unsupported)?;
    if w > MAX_SIDE || h > MAX_SIDE || u64::from(w) * u64::from(h) > MAX_PIXELS {
        return Err(ImageError::TooLarge {
            width: w,
            height: h,
        });
    }
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| ImageError::Decode(e.to_string()))?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(MAX_ALLOC);
    reader.limits(limits);
    let decoder = reader
        .into_decoder()
        .map_err(|e| ImageError::Decode(e.to_string()))?;
    let mut img =
        DynamicImage::from_decoder(decoder).map_err(|e| ImageError::Decode(e.to_string()))?;
    if let Some(o) = Orientation::from_exif(orientation) {
        img.apply_orientation(o);
    }
    let img = fit(img, preset);
    encode(&img, preset)
}

/// A rectangle of the source, in its oriented pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Crops `bytes` to `rect` (clamped to the image) in the source's own format:
/// PNG stays PNG, JPEG stays JPEG, WebP stays WebP, anything else becomes
/// PNG. Orientation is baked in first, so the rectangle is what the viewer
/// saw. CPU-bound: call it from a blocking thread.
pub fn crop(bytes: &[u8], rect: Rect, orientation: u8) -> Result<Rendered, ImageError> {
    let (w, h) = header_dimensions(bytes).ok_or(ImageError::Unsupported)?;
    if w > MAX_SIDE || h > MAX_SIDE || u64::from(w) * u64::from(h) > MAX_PIXELS {
        return Err(ImageError::TooLarge {
            width: w,
            height: h,
        });
    }
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| ImageError::Decode(e.to_string()))?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(MAX_ALLOC);
    reader.limits(limits);
    let format = reader.format();
    let decoder = reader
        .into_decoder()
        .map_err(|e| ImageError::Decode(e.to_string()))?;
    let mut img =
        DynamicImage::from_decoder(decoder).map_err(|e| ImageError::Decode(e.to_string()))?;
    if let Some(o) = Orientation::from_exif(orientation) {
        img.apply_orientation(o);
    }
    let x = rect.x.min(img.width().saturating_sub(1));
    let y = rect.y.min(img.height().saturating_sub(1));
    let width = rect.width.clamp(1, img.width() - x);
    let height = rect.height.clamp(1, img.height() - y);
    let cropped = img.crop_imm(x, y, width, height);
    let keep = match format {
        Some(image::ImageFormat::Jpeg) => Format::Jpeg,
        Some(image::ImageFormat::WebP) => Format::WebP,
        _ => Format::Png,
    };
    encode(&cropped, &Preset::new("crop").format(keep).quality(90))
}

fn fit(img: DynamicImage, preset: &Preset) -> DynamicImage {
    use image::imageops::FilterType::Lanczos3;
    let (sw, sh) = (img.width(), img.height());
    match preset.mode {
        Mode::Fit => {
            let bw = preset.width.unwrap_or(sw);
            let bh = preset.height.unwrap_or(sh);
            if sw <= bw && sh <= bh {
                img
            } else {
                img.resize(bw, bh, Lanczos3)
            }
        }
        Mode::Crop => {
            let bw = preset.width.unwrap_or(sw).min(sw.max(1));
            let bh = preset.height.unwrap_or(sh).min(sh.max(1));
            img.resize_to_fill(bw, bh, Lanczos3)
        }
        Mode::Exact => {
            let bw = preset.width.unwrap_or(sw);
            let bh = preset.height.unwrap_or(sh);
            img.resize_exact(bw, bh, Lanczos3)
        }
    }
}

fn encode(img: &DynamicImage, preset: &Preset) -> Result<Rendered, ImageError> {
    let (width, height) = (img.width(), img.height());
    let alpha = img.color().has_alpha();
    let format = match preset.format {
        Format::Auto if alpha => Format::Png,
        Format::Auto => Format::Jpeg,
        other => other,
    };
    let mut out = Vec::new();
    let encode_error = |e: image::ImageError| ImageError::Encode(e.to_string());
    let (content_type, extension) = match format {
        Format::Jpeg | Format::Auto => {
            let rgb = img.to_rgb8();
            JpegEncoder::new_with_quality(&mut out, preset.quality)
                .write_image(rgb.as_raw(), width, height, ExtendedColorType::Rgb8)
                .map_err(encode_error)?;
            ("image/jpeg", "jpg")
        }
        Format::Png => {
            let rgba = img.to_rgba8();
            PngEncoder::new(&mut out)
                .write_image(rgba.as_raw(), width, height, ExtendedColorType::Rgba8)
                .map_err(encode_error)?;
            ("image/png", "png")
        }
        Format::WebP => {
            let rgba = img.to_rgba8();
            WebPEncoder::new_lossless(&mut out)
                .write_image(rgba.as_raw(), width, height, ExtendedColorType::Rgba8)
                .map_err(encode_error)?;
            ("image/webp", "webp")
        }
    };
    Ok(Rendered {
        bytes: out,
        width,
        height,
        content_type,
        extension,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgb, Rgba};

    fn png(w: u32, h: u32) -> Bytes {
        let img = ImageBuffer::from_fn(w, h, |x, _| Rgb([x as u8, 0, 0]));
        let mut out = Cursor::new(Vec::new());
        DynamicImage::ImageRgb8(img)
            .write_to(&mut out, image::ImageFormat::Png)
            .unwrap();
        Bytes::from(out.into_inner())
    }

    fn png_with_alpha(w: u32, h: u32) -> Bytes {
        let img = ImageBuffer::from_fn(w, h, |x, _| Rgba([x as u8, 0, 0, 128]));
        let mut out = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(img)
            .write_to(&mut out, image::ImageFormat::Png)
            .unwrap();
        Bytes::from(out.into_inner())
    }

    #[test]
    fn a_png_is_measured_and_a_text_file_passes_through() {
        let p = prepare(png(40, 30));
        assert_eq!((p.width, p.height, p.orientation), (Some(40), Some(30), 1));
        let text = Bytes::from_static(b"hello");
        let p = prepare(text.clone());
        assert_eq!(p.bytes, text);
        assert_eq!(p.width, None);
    }

    #[test]
    fn fit_never_upscales_and_crop_fills_the_box() {
        let src = png(400, 200);
        let fit = render(&src, &Preset::new("t").width(100).height(100), 1).unwrap();
        assert_eq!((fit.width, fit.height), (100, 50));
        let small = render(&src, &Preset::new("t").width(800).height(800), 1).unwrap();
        assert_eq!((small.width, small.height), (400, 200));
        let crop = render(&src, &Preset::new("t").width(100).height(100).crop(), 1).unwrap();
        assert_eq!((crop.width, crop.height), (100, 100));
        let exact = render(&src, &Preset::new("t").width(50).height(80).exact(), 1).unwrap();
        assert_eq!((exact.width, exact.height), (50, 80));
        assert_eq!(fit.content_type, "image/jpeg", "opaque: JPEG");
        let clear = render(&png_with_alpha(40, 40), &Preset::new("t").width(10), 1).unwrap();
        assert_eq!(clear.content_type, "image/png", "transparent: PNG");
        let webp = render(&src, &Preset::new("t").width(10).format(Format::WebP), 1).unwrap();
        assert_eq!(webp.content_type, "image/webp");
    }

    #[test]
    fn a_crop_keeps_the_format_and_clamps_to_the_image() {
        let src = png(400, 200);
        let out = crop(
            &src,
            Rect {
                x: 100,
                y: 50,
                width: 200,
                height: 100,
            },
            1,
        )
        .unwrap();
        assert_eq!(
            (out.width, out.height, out.content_type),
            (200, 100, "image/png")
        );
        let past = crop(
            &src,
            Rect {
                x: 350,
                y: 150,
                width: 500,
                height: 500,
            },
            1,
        )
        .unwrap();
        assert_eq!((past.width, past.height), (50, 50));
    }

    #[test]
    fn an_orientation_is_baked_in() {
        let src = png(400, 200);
        let turned = render(&src, &Preset::new("t").width(1000).height(1000), 6).unwrap();
        assert_eq!((turned.width, turned.height), (200, 400));
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &b in data {
            crc ^= u32::from(b);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    0xEDB8_8320 ^ (crc >> 1)
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    #[test]
    fn a_bomb_is_refused_from_its_header() {
        let mut big = png(8, 8).to_vec();
        // The IHDR width and height fields, rewritten to claim 40k by 40k,
        // with the chunk's checksum made good again.
        big[16..20].copy_from_slice(&40_000u32.to_be_bytes());
        big[20..24].copy_from_slice(&40_000u32.to_be_bytes());
        let crc = crc32(&big[12..29]);
        big[29..33].copy_from_slice(&crc.to_be_bytes());
        assert!(matches!(
            render(&big, &Preset::new("t").width(10), 1),
            Err(ImageError::TooLarge { .. })
        ));
    }
}
