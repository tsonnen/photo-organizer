use crate::db::{CachedPhotoData, Database};
use crate::inference::{extract_embedding, init_clip_session};
use crate::media::{
    cached_thumb_to_egui, dynamic_to_cached_thumb, extract_date, load_scan_preview,
};
use crate::profile_store::{ClassificationSource, ProfileStore};
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
    pub source: ClassificationSource,
    pub embedding: Vec<f32>,
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
        source: ClassificationSource,
        embedding: Vec<f32>,
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
/// then asynchronously updates AI / heuristic classifications in the background.
pub fn scan_folder(
    folder: PathBuf,
    profiles: ProfileStore,
    config: ScanConfig,
    tx: Sender<ScanMessage>,
    ctx: egui::Context,
) {
    scan_folder_with_db(
        folder,
        profiles,
        config,
        tx,
        ctx,
        PathBuf::from("photo_cache.db"),
    )
}

/// Everything the scan needs from [`crate::settings::Settings`].
///
/// Passed as one value rather than as loose parameters so the scan cannot be
/// wired to a threshold from one place and a model from another: the two are
/// read by the same workers in the same pass, and a mismatch between them
/// classifies photos against a bar the UI never showed.
pub struct ScanConfig {
    /// Cosine similarity a centroid match must reach to beat the rules.
    pub threshold: f32,
    /// Resolved CLIP checkpoint. `None` means no model, so the scan runs
    /// rules-only rather than failing.
    pub model_path: Option<PathBuf>,
}

/// Number of worker threads the scan runs on.
///
/// The scan is not compute-bound in the way the core count suggests: the CLIP
/// forward pass dominates and it re-reads its entire ~350 MB of f32 weights from
/// RAM on every photo, so throughput is set by memory bandwidth rather than by
/// how many cores can multiply. Measured on a 22-core machine, 8-12 concurrent
/// forwards were ~1.4x *faster* than 22, because the surplus threads spend their
/// time stalled on cores they cannot keep fed.
///
/// Roughly half the machine is therefore the better default, which also leaves
/// the rest of the box responsive while a scan is running. Override with
/// `PHOTO_ORGANIZER_SCAN_THREADS`.
fn scan_thread_count() -> usize {
    if let Ok(raw) = std::env::var("PHOTO_ORGANIZER_SCAN_THREADS") {
        if let Ok(requested) = raw.trim().parse::<usize>() {
            if requested > 0 {
                return requested;
            }
        }
    }
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    (cores / 2).clamp(2, 12)
}

pub fn scan_folder_with_db(
    folder: PathBuf,
    profiles: ProfileStore,
    config: ScanConfig,
    tx: Sender<ScanMessage>,
    ctx: egui::Context,
    db_path: PathBuf,
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

        let db = Arc::new(Mutex::new(Database::init(&db_path).ok()));
        // The model path is resolved by the caller from the user's setting, so
        // the workers never re-run the search: every one of them would stat the
        // same candidate list to reach the same answer.
        let model_path = config.model_path;
        let threshold = config.threshold;
        let session = Arc::new(model_path.and_then(|p| init_clip_session(p).ok()));

        // Deliberately not the global rayon pool: the scan wants a smaller,
        // bandwidth-bound worker count than "one per core", and running it here
        // keeps that pool out of the rest of the app's work.
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(scan_thread_count())
            .thread_name(|i| format!("scan-{i}"))
            .build()
            .unwrap_or_else(|_| rayon::ThreadPoolBuilder::new().build().unwrap());

        pool.install(|| {
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

                if let Some(mut c) = cached {
                    // If photo was previously cached without embedding, backfill it now
                    if c.embedding.is_empty() {
                        if let Some(ref sess) = *session {
                            if let Ok(preview) = load_scan_preview(path) {
                                if let Ok(emb) = extract_embedding(sess, &preview.image) {
                                    if !emb.is_empty() {
                                        c.embedding = emb.clone();
                                        let db_guard = db.lock().unwrap();
                                        if let Some(ref db_conn) = *db_guard {
                                            let _ = db_conn.insert_cache(
                                                &file_hash,
                                                &CachedPhotoData {
                                                    year: c.year,
                                                    month: c.month,
                                                    is_exif_date: c.is_exif_date,
                                                    embedding: emb,
                                                    thumbnail: c.thumbnail.clone(),
                                                },
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    }

                    let (w, h) = if let Some(ref thumb) = c.thumbnail {
                        (thumb.width, thumb.height)
                    } else {
                        (1920, 1080)
                    };
                    let class_res = profiles.classify_with_heuristics(
                        &c.embedding,
                        path,
                        c.is_exif_date,
                        w,
                        h,
                        threshold,
                    );

                    let image = if let Some(ref thumb) = c.thumbnail {
                        cached_thumb_to_egui(thumb)
                    } else if let Ok(preview) = load_scan_preview(path) {
                        let (cached_thumb, color_img) = dynamic_to_cached_thumb(&preview.image);
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
                        // The staged item holds the name as the user sees it; it
                        // is sanitised again at the transfer seam.
                        category: class_res.category.into_string(),
                        confidence: class_res.confidence,
                        source: class_res.source,
                        embedding: c.embedding,
                        image,
                    }));
                    ctx.request_repaint();
                    return;
                }

                // 2. Uncached image: decode once, at the size the pipeline consumes
                let (date_info, preview, cached_thumb, image) = match load_scan_preview(path) {
                    Ok(preview) => {
                        let (thumb, egui_img) = dynamic_to_cached_thumb(&preview.image);
                        let date_info = extract_date(path);
                        (date_info, preview, thumb, egui_img)
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
                    source: ClassificationSource::UnsortedFallback,
                    embedding: Vec::new(),
                    image,
                }));
                ctx.request_repaint();

                // 3. Extract embedding in background
                let emb = if let Some(ref sess) = *session {
                    extract_embedding(sess, &preview.image).unwrap_or_default()
                } else {
                    Vec::new()
                };

                // The heuristics read real resolution (a 1920x1080 PNG is a
                // screenshot), so they get the source dimensions rather than the
                // preview's.
                let class_res = profiles.classify_with_heuristics(
                    &emb,
                    path,
                    date_info.2,
                    preview.original_width,
                    preview.original_height,
                    threshold,
                );

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
                                embedding: emb.clone(),
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
                    category: class_res.category.into_string(),
                    confidence: class_res.confidence,
                    source: class_res.source,
                    embedding: emb,
                });
                ctx.request_repaint();
            });
        });

        let _ = tx.send(ScanMessage::Complete);
        ctx.request_repaint();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A rules-only scan config: no model, so these tests exercise the
    /// heuristic path without depending on a CLIP checkpoint being present.
    fn rules_only() -> ScanConfig {
        ScanConfig {
            threshold: crate::settings::DEFAULT_CONFIDENCE_THRESHOLD,
            model_path: None,
        }
    }

    // One test, because the env var is process-global and cargo runs tests in
    // parallel threads.
    #[test]
    fn test_scan_thread_count_default_and_env_override() {
        let previous = std::env::var("PHOTO_ORGANIZER_SCAN_THREADS").ok();
        std::env::remove_var("PHOTO_ORGANIZER_SCAN_THREADS");

        let cores = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        assert_eq!(scan_thread_count(), (cores / 2).clamp(2, 12));

        std::env::set_var("PHOTO_ORGANIZER_SCAN_THREADS", " 3 ");
        assert_eq!(scan_thread_count(), 3);

        // Nonsense values fall back to the default rather than a broken pool.
        for bad in ["not-a-number", "0", ""] {
            std::env::set_var("PHOTO_ORGANIZER_SCAN_THREADS", bad);
            assert!((2..=12).contains(&scan_thread_count()));
        }

        match previous {
            Some(value) => std::env::set_var("PHOTO_ORGANIZER_SCAN_THREADS", value),
            None => std::env::remove_var("PHOTO_ORGANIZER_SCAN_THREADS"),
        }
    }

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

    #[test]
    fn test_scan_folder_empty() {
        let temp_dir = std::env::temp_dir().join(format!("test_scan_empty_{}", std::process::id()));
        fs::create_dir_all(&temp_dir).unwrap();

        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = egui::Context::default();
        scan_folder_with_db(
            temp_dir.clone(),
            ProfileStore::default(),
            rules_only(),
            tx,
            ctx,
            temp_dir.join("empty_cache.db"),
        );

        let mut received_complete = false;
        while let Ok(msg) = rx.recv_timeout(std::time::Duration::from_secs(2)) {
            if let ScanMessage::Complete = msg {
                received_complete = true;
                break;
            }
        }
        assert!(received_complete);

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_scan_folder_with_real_images_and_rescan_cache() {
        let temp_dir =
            std::env::temp_dir().join(format!("test_scan_images_{}", std::process::id()));
        fs::create_dir_all(&temp_dir).unwrap();
        let test_db_path = temp_dir.join("isolated_cache.db");

        // Create 2 test PNG images (one screenshot-like naming, one generic)
        let img_path1 = temp_dir.join("screenshot_test.png");
        let mut img1 = image::RgbImage::new(40, 40);
        for p in img1.pixels_mut() {
            *p = image::Rgb([10, 100, 150]);
        }
        img1.save(&img_path1).unwrap();

        let img_path2 = temp_dir.join("test_pic_1.png");
        let mut img2 = image::RgbImage::new(40, 40);
        for p in img2.pixels_mut() {
            *p = image::Rgb([50, 100, 150]);
        }
        img2.save(&img_path2).unwrap();

        let txt_path = temp_dir.join("readme.txt");
        fs::write(&txt_path, b"not a photo").unwrap();

        // 1st scan: uncached
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = egui::Context::default();
        scan_folder_with_db(
            temp_dir.clone(),
            ProfileStore::default(),
            rules_only(),
            tx,
            ctx,
            test_db_path.clone(),
        );

        let mut items = Vec::new();
        let mut updates = Vec::new();
        let mut completed = false;

        while let Ok(msg) = rx.recv_timeout(std::time::Duration::from_secs(60)) {
            match msg {
                ScanMessage::Item(payload) => items.push(payload),
                ScanMessage::Update {
                    source_path,
                    category,
                    ..
                } => updates.push((source_path, category)),
                ScanMessage::Complete => {
                    completed = true;
                    break;
                }
            }
        }

        assert!(completed);
        assert_eq!(items.len(), 2);
        assert_eq!(updates.len(), 2);

        // Verify screenshot got heuristic category
        let screenshot_update = updates
            .iter()
            .find(|(p, _)| p.file_name().unwrap() == "screenshot_test.png");
        assert!(screenshot_update.is_some());
        assert_eq!(screenshot_update.unwrap().1, "Screenshots");

        // 2nd scan: should hit cache and complete
        let (tx2, rx2) = std::sync::mpsc::channel();
        let ctx2 = egui::Context::default();
        scan_folder_with_db(
            temp_dir.clone(),
            ProfileStore::default(),
            rules_only(),
            tx2,
            ctx2,
            test_db_path,
        );

        let mut cached_items = Vec::new();
        let mut cached_completed = false;

        while let Ok(msg) = rx2.recv_timeout(std::time::Duration::from_secs(60)) {
            match msg {
                ScanMessage::Item(payload) => cached_items.push(payload),
                ScanMessage::Complete => {
                    cached_completed = true;
                    break;
                }
                _ => {}
            }
        }

        assert!(cached_completed);
        assert_eq!(cached_items.len(), 2);

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
