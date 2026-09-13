use crate::db::{CachedPhotoData, Database};
use crate::inference::{extract_embedding, init_clip_session};
use crate::media::{dynamic_to_egui, extract_date, load_image};
use crate::profile_store::ProfileStore;
use eframe::egui;
use std::fs;
use std::path::PathBuf;
use std::sync::mpsc::Sender;
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

/// Spawns a background thread to scan the given folder, extract metadata and embeddings,
/// classify photos, and send processed payloads to the UI channel.
pub fn scan_folder(
    folder: PathBuf,
    profiles: ProfileStore,
    tx: Sender<ProcessedPayload>,
    ctx: egui::Context,
) {
    thread::spawn(move || {
        let db = Database::init("photo_cache.db").ok();
        let mut session = init_clip_session("models/clip_visual.onnx").ok();

        if let Ok(entries) = fs::read_dir(&folder) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }

                let mut cached = None;
                let file_hash = Database::compute_file_hash(&path).unwrap_or_default();

                if let Some(ref db_conn) = db {
                    if let Ok(Some(data)) = db_conn.get_cached(&file_hash) {
                        cached = Some(data);
                    }
                }

                let (year, month, is_exif, embedding) = match cached {
                    Some(c) => (c.year, c.month, c.is_exif_date, c.embedding),
                    None => {
                        let date_info = extract_date(&path);
                        let emb = if let (Some(ref mut sess), Ok(dyn_img)) =
                            (&mut session, load_image(&path))
                        {
                            extract_embedding(sess, &dyn_img).unwrap_or_default()
                        } else {
                            Vec::new()
                        };

                        if let Some(ref db_conn) = db {
                            let _ = db_conn.insert_cache(
                                &file_hash,
                                &CachedPhotoData {
                                    year: date_info.0,
                                    month: date_info.1,
                                    is_exif_date: date_info.2,
                                    embedding: emb.clone(),
                                },
                            );
                        }
                        (date_info.0, date_info.1, date_info.2, emb)
                    }
                };

                if let Ok(dyn_img) = load_image(&path) {
                    let (category, confidence) = profiles.classify(&embedding);
                    let image = dynamic_to_egui(&dyn_img);

                    let _ = tx.send(ProcessedPayload {
                        source_path: path,
                        year,
                        month,
                        is_exif,
                        category,
                        confidence,
                        image,
                    });
                    ctx.request_repaint();
                }
            }
        }
    });
}
