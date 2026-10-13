use anyhow::{anyhow, Result};
use chrono::{Datelike, NaiveDateTime, Utc};
use eframe::egui;
use image::DynamicImage;
use kamadak_exif::{In, Reader, Tag, Value};
use std::fs::{self, File};
use std::io::BufReader;
use std::path::Path;

use crate::classification::PhotoDate;
use crate::db::CachedThumbnail;

/// Extracts the day, month and year a photo was taken, and whether the date came
/// from EXIF metadata. Falls back to file creation/modification time if EXIF is
/// missing or unparseable.
///
/// The day used to be parsed and dropped, which left "taken" meaning "taken in the
/// same month as" — close enough to file photos by and useless for ordering a
/// folder of photos from one trip. It is in both sources, so it is read from both.
pub fn extract_date(path: &Path) -> (PhotoDate, bool) {
    if let Ok(file) = File::open(path) {
        let mut buf = BufReader::new(file);
        if let Ok(exif_data) = Reader::new().read_from_container(&mut buf) {
            if let Some(field) = exif_data.get_field(Tag::DateTimeOriginal, In::PRIMARY) {
                if let Value::Ascii(ref v) = field.value {
                    if let Some(bytes) = v.first() {
                        if let Ok(s) = std::str::from_utf8(bytes) {
                            if let Ok(dt) = NaiveDateTime::parse_from_str(s, "%Y:%m:%d %H:%M:%S") {
                                return (
                                    PhotoDate::new(dt.year() as u32, dt.month(), Some(dt.day())),
                                    true,
                                );
                            }
                        }
                    }
                }
            }
        }
    }
    let dt: chrono::DateTime<Utc> = fs::metadata(path)
        .and_then(|m| m.created().or_else(|_| m.modified()))
        .unwrap_or(std::time::SystemTime::now())
        .into();
    (
        PhotoDate::new(dt.year() as u32, dt.month(), Some(dt.day())),
        false,
    )
}

/// Loads an image from disk. Supports standard image formats as well as various
/// RAW camera formats (CR2, CR3, NEF, ARW, DNG, RAF, ORF, PEF) using `rawloader`.
pub fn load_image(path: &Path) -> Result<DynamicImage> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    let raw_extensions = ["cr2", "cr3", "nef", "arw", "dng", "raf", "orf", "pef"];

    if raw_extensions.contains(&ext.as_str()) {
        let raw =
            rawloader::decode_file(path).map_err(|e| anyhow!("RAW decoding failed: {:?}", e))?;

        let width = raw.width;
        let height = raw.height;
        let mut image_buf = image::RgbImage::new(width as u32, height as u32);

        if let rawloader::RawImageData::Integer(data) = raw.data {
            for (idx, pixel) in image_buf.pixels_mut().enumerate() {
                if idx < data.len() {
                    let val = (data[idx] >> 6) as u8; // Scale 14-bit raw down to 8-bit
                    *pixel = image::Rgb([val, val, val]);
                }
            }
            return Ok(DynamicImage::ImageRgb8(image_buf));
        }
        Err(anyhow!("Could not decode RAW frame"))
    } else {
        Ok(image::open(path)?)
    }
}

/// Longest edge the CLIP preprocessing stage needs to receive. Anything smaller
/// than this and the model input is being upscaled, which is never useful.
pub const CLIP_INPUT_EDGE: u32 = 224;

/// The pixels a scan actually needs, decoded at (or near) the size it needs them.
///
/// The full-resolution frame is only ever an intermediate here: the grid wants a
/// 200x140 thumbnail and CLIP wants a 224x224 square. Decoding a 12MP JPEG just
/// to feed those two consumers costs an order of magnitude more than everything
/// else in a scan put together, so JPEGs are decoded straight to a reduced size
/// via the inverse DCT's own scaling modes.
///
/// `original_width`/`original_height` carry the true frame size because the
/// classification heuristics key off resolution (a 1920x1080 PNG is a
/// screenshot, a 240x135 one is not).
pub struct ScanPreview {
    pub image: DynamicImage,
    pub original_width: u32,
    pub original_height: u32,
}

/// Loads the pixels a scan needs from `path`.
///
/// JPEGs take a scaled decode; every other format (PNG, WebP, TIFF, RAW, ...)
/// falls back to a full decode followed by a rescale, since those formats have
/// no cheap partial decode.
pub fn load_scan_preview(path: &Path) -> Result<ScanPreview> {
    if let Some(preview) = jpeg_scan_preview(path) {
        return Ok(preview);
    }

    let full = load_image(path)?;
    let (original_width, original_height) = (full.width(), full.height());
    Ok(ScanPreview {
        image: shrink_to_scan_preview(&full),
        original_width,
        original_height,
    })
}

/// Rescales a decoded image down to roughly what the pipeline consumes.
fn shrink_to_scan_preview(full: &DynamicImage) -> DynamicImage {
    // The preview edge budget is the CLIP input; anything already at or below it
    // is passed through untouched so small images are never blown up.
    if full.width() <= CLIP_INPUT_EDGE && full.height() <= CLIP_INPUT_EDGE {
        return full.clone();
    }
    full.thumbnail(SCAN_PREVIEW_EDGE, SCAN_PREVIEW_EDGE)
}

/// The edge size a decoded JPEG preview should land near: twice the CLIP input,
/// which leaves the 200x140 thumbnail with real detail to sample from.
const SCAN_PREVIEW_EDGE: u32 = CLIP_INPUT_EDGE * 2;

/// Attempts a DCT-scaled JPEG decode, returning `None` for anything that isn't a
/// JPEG `jpeg-decoder` can handle, so the caller can fall back to `load_image`.
fn jpeg_scan_preview(path: &Path) -> Option<ScanPreview> {
    let file = File::open(path).ok()?;
    let mut decoder = jpeg_decoder::Decoder::new(BufReader::with_capacity(256 * 1024, file));
    decoder.read_info().ok()?;

    // `info()` reflects the *output* size, so it has to be read before `scale()`
    // if the original dimensions are wanted.
    let info = decoder.info()?;
    if info.pixel_format != jpeg_decoder::PixelFormat::RGB24 {
        return None;
    }
    let (original_width, original_height) = (info.width as u32, info.height as u32);
    if original_width == 0 || original_height == 0 {
        return None;
    }
    // Already small enough that scaling down would only throw pixels away.
    if original_width <= CLIP_INPUT_EDGE && original_height <= CLIP_INPUT_EDGE {
        return None;
    }

    // `scale` picks the largest supported reduction (1/8, 1/4, 1/2) whose result
    // is at least the requested size on *at least one* axis. For a landscape or
    // portrait photo that leaves the short edge below the CLIP input, so the
    // request is bumped until both edges clear it and nothing gets upscaled.
    let mut request = CLIP_INPUT_EDGE as u16;
    let mut scaled = None;
    for _ in 0..3 {
        let (width, height) = decoder.scale(request, request).ok()?;
        if width as u32 >= CLIP_INPUT_EDGE && height as u32 >= CLIP_INPUT_EDGE {
            scaled = Some((width as u32, height as u32));
            break;
        }
        request = request.saturating_mul(2);
    }
    let (width, height) = scaled?;

    let pixels = decoder.decode().ok()?;
    let rgb = image::RgbImage::from_raw(width, height, pixels)?;

    Some(ScanPreview {
        image: DynamicImage::ImageRgb8(rgb),
        original_width,
        original_height,
    })
}

/// Converts a DynamicImage to an egui::ColorImage, scaling down to fit within
/// `max_edge` while preserving aspect ratio.
fn dynamic_to_color_image(img: &DynamicImage, max_edge: u32) -> egui::ColorImage {
    let (orig_w, orig_h) = (img.width(), img.height());
    let preview = if orig_w > max_edge || orig_h > max_edge {
        img.thumbnail(max_edge, max_edge).to_rgba8()
    } else {
        img.to_rgba8()
    };
    let width = preview.width() as usize;
    let height = preview.height() as usize;
    let rgba = preview.into_raw();
    egui::ColorImage::from_rgba_unmultiplied([width, height], &rgba)
}

/// Converts a DynamicImage to a CachedThumbnail and an egui::ColorImage.
pub fn dynamic_to_cached_thumb(img: &DynamicImage) -> (CachedThumbnail, egui::ColorImage) {
    let thumb = img.thumbnail(200, 140).to_rgba8();
    let width = thumb.width();
    let height = thumb.height();
    let rgba = thumb.into_raw();
    let size = [width as usize, height as usize];
    let color_img = egui::ColorImage::from_rgba_unmultiplied(size, &rgba);
    (
        CachedThumbnail {
            width,
            height,
            rgba,
        },
        color_img,
    )
}

/// Converts a CachedThumbnail to an egui::ColorImage.
pub fn cached_thumb_to_egui(thumb: &CachedThumbnail) -> egui::ColorImage {
    let size = [thumb.width as usize, thumb.height as usize];
    egui::ColorImage::from_rgba_unmultiplied(size, &thumb.rgba)
}

/// Converts a DynamicImage to an egui::ColorImage suitable for high-res modal preview,
/// scaling down to fit within `max_edge` while preserving aspect ratio.
pub fn dynamic_to_preview_color_image(img: &DynamicImage, max_edge: u32) -> egui::ColorImage {
    dynamic_to_color_image(img, max_edge)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_extract_date_fallback_to_metadata() {
        let temp_dir = std::env::temp_dir();
        let test_file = temp_dir.join(format!("test_extract_date_{}.tmp", std::process::id()));
        {
            let mut f = File::create(&test_file).expect("create test file");
            f.write_all(b"not an image").expect("write bytes");
        }

        let (date, is_exif) = extract_date(&test_file);
        let _ = fs::remove_file(&test_file);

        let now = Utc::now();
        assert!(!is_exif);
        assert_eq!(date.year, now.year() as u32);
        assert_eq!(date.month, now.month());
        // The fallback is a filesystem timestamp, which carries a day just as the
        // EXIF one does. It used to be dropped here, which is what left photos
        // from the same month indistinguishable.
        assert_eq!(date.day, Some(now.day()));
    }

    #[test]
    fn test_dynamic_to_cached_thumb_dimensions() {
        let img = DynamicImage::ImageRgb8(image::RgbImage::new(400, 300));
        let (thumb, color_img) = dynamic_to_cached_thumb(&img);
        // Thumbnail fits within 200x140 while preserving aspect ratio (400:300 -> 186x140 approx)
        assert!(thumb.width <= 200);
        assert!(thumb.height <= 140);
        assert_eq!(color_img.width(), thumb.width as usize);
        assert_eq!(color_img.height(), thumb.height as usize);
        assert_eq!(
            color_img.pixels.len(),
            color_img.width() * color_img.height()
        );
    }

    #[test]
    fn test_cached_thumbnail_roundtrip() {
        let img = DynamicImage::ImageRgb8(image::RgbImage::new(100, 100));
        let (cached, color_img) = dynamic_to_cached_thumb(&img);
        assert_eq!(cached.width as usize, color_img.width());
        assert_eq!(cached.height as usize, color_img.height());
        assert_eq!(
            cached.rgba.len(),
            color_img.width() * color_img.height() * 4
        );

        let restored_img = cached_thumb_to_egui(&cached);
        assert_eq!(restored_img.width(), color_img.width());
        assert_eq!(restored_img.height(), color_img.height());
        assert_eq!(restored_img.pixels, color_img.pixels);
    }

    #[test]
    fn test_dynamic_to_cached_thumb_aspect_ratios() {
        // Ultra wide panorama (1000x100)
        let wide_img = DynamicImage::ImageRgb8(image::RgbImage::new(1000, 100));
        let (wide_thumb, _) = dynamic_to_cached_thumb(&wide_img);
        assert!(wide_thumb.width <= 200);
        assert!(wide_thumb.height <= 140);
        assert!(wide_thumb.width > wide_thumb.height);

        // Ultra tall portrait (100x1000)
        let tall_img = DynamicImage::ImageRgb8(image::RgbImage::new(100, 1000));
        let (tall_thumb, _) = dynamic_to_cached_thumb(&tall_img);
        assert!(tall_thumb.width <= 200);
        assert!(tall_thumb.height <= 140);
        assert!(tall_thumb.height > tall_thumb.width);

        // 1x1 micro pixel
        let micro_img = DynamicImage::ImageRgb8(image::RgbImage::new(1, 1));
        let (micro_thumb, _) = dynamic_to_cached_thumb(&micro_img);
        assert!(micro_thumb.width >= 1);
        assert!(micro_thumb.height >= 1);
    }

    #[test]
    fn test_load_image_real_png() {
        let temp_dir = std::env::temp_dir();
        let test_file = temp_dir.join(format!("test_real_{}.png", std::process::id()));
        let mut img = image::RgbImage::new(50, 50);
        for pixel in img.pixels_mut() {
            *pixel = image::Rgb([120, 200, 50]);
        }
        img.save(&test_file).expect("save png");

        let loaded = load_image(&test_file).expect("load real image");
        assert_eq!(loaded.width(), 50);
        assert_eq!(loaded.height(), 50);

        let _ = fs::remove_file(&test_file);
    }

    #[test]
    fn test_scan_preview_jpeg_scales_down_and_keeps_original_size() {
        let test_file =
            std::env::temp_dir().join(format!("test_preview_{}.jpg", std::process::id()));
        image::DynamicImage::ImageRgb8(image::RgbImage::new(4000, 3000))
            .save(&test_file)
            .unwrap();

        let preview = load_scan_preview(&test_file).expect("load jpeg preview");

        // Decoded smaller, but never below what CLIP and the thumbnail need.
        assert!(
            preview.image.width() < 4000 && preview.image.height() < 3000,
            "expected a scaled decode, got {:?}",
            (preview.image.width(), preview.image.height())
        );
        assert!(preview.image.width() >= CLIP_INPUT_EDGE);
        assert!(preview.image.height() >= CLIP_INPUT_EDGE);

        // The heuristics depend on the true resolution, not the preview's.
        assert_eq!(preview.original_width, 4000);
        assert_eq!(preview.original_height, 3000);

        let _ = fs::remove_file(&test_file);
    }

    #[test]
    fn test_scan_preview_falls_back_for_non_jpeg() {
        let test_file =
            std::env::temp_dir().join(format!("test_preview_{}.png", std::process::id()));
        image::DynamicImage::ImageRgb8(image::RgbImage::new(1000, 800))
            .save(&test_file)
            .unwrap();

        let preview = load_scan_preview(&test_file).expect("load png preview");
        assert_eq!(preview.original_width, 1000);
        assert_eq!(preview.original_height, 800);
        assert!(preview.image.width() <= 1000);

        let _ = fs::remove_file(&test_file);
    }

    #[test]
    fn test_scan_preview_never_upsamples_small_images() {
        let test_file = std::env::temp_dir().join(format!("test_small_{}.jpg", std::process::id()));
        image::DynamicImage::ImageRgb8(image::RgbImage::new(120, 90))
            .save(&test_file)
            .unwrap();

        let preview = load_scan_preview(&test_file).expect("load small jpeg");
        assert_eq!(preview.image.width(), 120);
        assert_eq!(preview.image.height(), 90);

        let _ = fs::remove_file(&test_file);
    }

    #[test]
    fn test_load_image_corrupted_or_empty() {
        let temp_dir = std::env::temp_dir();
        let empty_file = temp_dir.join(format!("test_empty_{}.jpg", std::process::id()));
        {
            let _ = File::create(&empty_file).unwrap();
        }

        let result = load_image(&empty_file);
        assert!(result.is_err());

        let _ = fs::remove_file(&empty_file);
    }

    #[test]
    fn test_dynamic_to_preview_color_image() {
        // Large image (4000x2000) scaled to 1000 max edge
        let large_img = DynamicImage::ImageRgb8(image::RgbImage::new(4000, 2000));
        let preview = dynamic_to_preview_color_image(&large_img, 1000);
        assert_eq!(preview.size, [1000, 500]);

        // Small image (300x200) below max edge should retain original dimensions
        let small_img = DynamicImage::ImageRgb8(image::RgbImage::new(300, 200));
        let small_preview = dynamic_to_preview_color_image(&small_img, 1000);
        assert_eq!(small_preview.size, [300, 200]);
    }
}
