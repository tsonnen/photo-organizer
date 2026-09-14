use anyhow::{anyhow, Result};
use chrono::{Datelike, NaiveDateTime, Utc};
use eframe::egui;
use image::DynamicImage;
use kamadak_exif::{In, Reader, Tag, Value};
use std::fs::{self, File};
use std::io::BufReader;
use std::path::Path;

use crate::db::CachedThumbnail;

/// Extracts year, month, and whether the date was extracted from EXIF metadata.
/// Falls back to file creation/modification time if EXIF is missing or unparseable.
pub fn extract_date(path: &Path) -> (u32, u32, bool) {
    if let Ok(file) = File::open(path) {
        let mut buf = BufReader::new(file);
        if let Ok(exif_data) = Reader::new().read_from_container(&mut buf) {
            if let Some(field) = exif_data.get_field(Tag::DateTimeOriginal, In::PRIMARY) {
                if let Value::Ascii(ref v) = field.value {
                    if let Some(bytes) = v.first() {
                        if let Ok(s) = std::str::from_utf8(bytes) {
                            if let Ok(dt) = NaiveDateTime::parse_from_str(s, "%Y:%m:%d %H:%M:%S") {
                                return (dt.year() as u32, dt.month(), true);
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
    (dt.year() as u32, dt.month(), false)
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

        let (year, month, is_exif) = extract_date(&test_file);
        let _ = fs::remove_file(&test_file);

        let now = Utc::now();
        assert!(!is_exif);
        assert_eq!(year, now.year() as u32);
        assert_eq!(month, now.month());
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
}
