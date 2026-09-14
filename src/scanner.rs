use crate::db::{CachedPhotoData, Database};
use crate::inference::{extract_embedding, init_clip_session};
use crate::media::{cached_thumb_to_egui, dynamic_to_cached_thumb, extract_date, load_image};
use crate::profile_store::ProfileStore;
use eframe::egui;
use rayon::prelude::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread;

pub struct ProcessedPayload {
    pub source_path: PathBuf,
    pub year: u32,
    pub month: u32,
    pub is_exif: bool,
    pub category: String,
    pub confidence: f32,
    pub image: egui::ColorImage,
}

pub enum ScanMessage {
    Item(ProcessedPayload),
    Update {
        source_path: PathBuf,
        year: u32,
        month: u32,
        is_exif: bool,
        category: String,
        confidence: f32,
    },
    Complete,
}

/// Checks if a file has a supported image or camera RAW extension.
pub fn is_supported_image(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    matches!(
        ext.as_str(),
        "jpg"
            | "jpeg"
            | "png"
            | "webp"
            | "bmp"
            | "gif"
            | "tiff"
            | "tif"
            | "cr2"
            | "cr3"
            | "nef"
            | "arw"
            | "dng"
            | "raf"
            | "orf"
            | "pef"
    )
}

/// Spawns a background thread that leverages a Rayon thread pool to scan the folder in parallel.
/// It immediately emits thumbnails to the UI as soon as they are ready (or from SQLite cache),
/// then asynchronously updates AI classifications in the background.
pub fn scan_folder(
    folder: PathBuf,
    profiles: ProfileStore,
    tx: Sender<ScanMessage>,
    ctx: egui::Context,
) {
    thread::spawn(move || {
        let mut paths = Vec::new();
        if let Ok(entries) = fs::read_dir(&folder) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() && is_supported_image(&path) {
                    paths.push(path);
                }
            }
        }

        if paths.is_empty() {
            let _ = tx.send(ScanMessage::Complete);
            ctx.request_repaint();
            return;
        }

        let db = Arc::new(Mutex::new(Database::init("photo_cache.db").ok()));
        let session = Arc::new(Mutex::new(
            init_clip_session("models/clip_visual.onnx").ok(),
        ));

        paths.par_iter().for_each(|path| {
            let file_hash = Database::compute_file_hash(path).unwrap_or_default();

            // 1. Check SQLite Cache
            let cached = {
                let db_guard = db.lock().unwrap();
                if let Some(ref db_conn) = *db_guard {
                    db_conn.get_cached(&file_hash).ok().flatten()
                } else {
                    None
                }
            };

            if let Some(c) = cached {
                let (category, confidence) = profiles.classify(&c.embedding);
                let image = if let Some(ref thumb) = c.thumbnail {
                    cached_thumb_to_egui(thumb)
                } else if let Ok(dyn_img) = load_image(path) {
                    let (cached_thumb, color_img) = dynamic_to_cached_thumb(&dyn_img);
                    let db_guard = db.lock().unwrap();
                    if let Some(ref db_conn) = *db_guard {
                        let _ = db_conn.insert_cache(
                            &file_hash,
                            &CachedPhotoData {
                                year: c.year,
                                month: c.month,
                                is_exif_date: c.is_exif_date,
                                embedding: c.embedding.clone(),
                                thumbnail: Some(cached_thumb),
                            },
                        );
                    }
                    color_img
                } else {
                    return;
                };

                let _ = tx.send(ScanMessage::Item(ProcessedPayload {
                    source_path: path.clone(),
                    year: c.year,
                    month: c.month,
                    is_exif: c.is_exif_date,
                    category,
                    confidence,
                    image,
                }));
                ctx.request_repaint();
                return;
            }

            // 2. Uncached image: Fast thumbnail generation
            let (date_info, dyn_img, cached_thumb, image) = match load_image(path) {
                Ok(img) => {
                    let (thumb, egui_img) = dynamic_to_cached_thumb(&img);
                    let date_info = extract_date(path);
                    (date_info, img, thumb, egui_img)
                }
                Err(_) => return,
            };

            // Immediately send thumbnail to UI so user sees the photo right away!
            let _ = tx.send(ScanMessage::Item(ProcessedPayload {
                source_path: path.clone(),
                year: date_info.0,
                month: date_info.1,
                is_exif: date_info.2,
                category: "Classifying...".to_string(),
                confidence: 0.0,
                image,
            }));
            ctx.request_repaint();

            // 3. Extract embedding in background
            let emb = {
                let mut sess_guard = session.lock().unwrap();
                if let Some(ref mut sess) = *sess_guard {
                    extract_embedding(sess, &dyn_img).unwrap_or_default()
                } else {
                    Vec::new()
                }
            };

            let (category, confidence) = profiles.classify(&emb);

            // 4. Save to cache with thumbnail
            {
                let db_guard = db.lock().unwrap();
                if let Some(ref db_conn) = *db_guard {
                    let _ = db_conn.insert_cache(
                        &file_hash,
                        &CachedPhotoData {
                            year: date_info.0,
                            month: date_info.1,
                            is_exif_date: date_info.2,
                            embedding: emb,
                            thumbnail: Some(cached_thumb),
                        },
                    );
                }
            }

            // 5. Update UI with final classification
            let _ = tx.send(ScanMessage::Update {
                source_path: path.clone(),
                year: date_info.0,
                month: date_info.1,
                is_exif: date_info.2,
                category,
                confidence,
            });
            ctx.request_repaint();
        });

        let _ = tx.send(ScanMessage::Complete);
        ctx.request_repaint();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_supported_image() {
        assert!(is_supported_image(Path::new("test.jpg")));
        assert!(is_supported_image(Path::new("test.JPEG")));
        assert!(is_supported_image(Path::new("photo.png")));
        assert!(is_supported_image(Path::new("raw.CR2")));
        assert!(is_supported_image(Path::new("raw.NEF")));
        assert!(is_supported_image(Path::new("raw.arw")));
        assert!(is_supported_image(Path::new("raw.DNG")));
        assert!(!is_supported_image(Path::new("doc.pdf")));
        assert!(!is_supported_image(Path::new("script.sh")));
        assert!(!is_supported_image(Path::new("no_extension")));
    }
}
