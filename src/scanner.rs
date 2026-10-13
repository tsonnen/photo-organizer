use crate::classification::{Classification, FrameSize, PhotoFacts};
use crate::db::{CachedPhotoData, Database};
use crate::inference::{extract_embedding, init_clip_session};
use crate::media::{
    cached_thumb_to_egui, dynamic_to_cached_thumb, extract_date, load_scan_preview,
};
use crate::profile_store::ProfileStore;
use eframe::egui;
use rayon::prelude::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread;

/// A photo as the scan has it so far: its facts, whatever has been decided, and
/// the thumbnail to show while the rest is still working.
pub struct ProcessedPayload {
    pub facts: PhotoFacts,
    pub classification: Classification,
    pub image: egui::ColorImage,
}

/// What a scan tells the UI.
pub enum ScanMessage {
    /// A photo to stage now, with its thumbnail.
    Item(ProcessedPayload),
    /// A decision about a photo already staged, carrying the facts the scan holds
    /// for it alongside it — two records rather than the eight fields this used to
    /// declare, destructure into eight bindings and pass to an eight-argument
    /// function.
    Update {
        facts: PhotoFacts,
        classification: Classification,
    },
    /// Every photo in the folder has been sent.
    Complete,
}

/// A message together with the scan it came from.
///
/// `start_scan` clears the staged items but cannot un-send what a superseded scan
/// already put on the shared channel, and a late `Update` from that scan was
/// decided against the profile store as it was then — it would otherwise overwrite
/// a decision the current scan is still working towards. The receiver drops
/// anything but the newest scan's.
pub struct ScanEvent {
    pub scan_id: u64,
    pub message: ScanMessage,
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
    tx: Sender<ScanEvent>,
    ctx: egui::Context,
    scan_id: u64,
) {
    scan_folder_with_db(
        folder,
        profiles,
        config,
        tx,
        ctx,
        PathBuf::from("photo_cache.db"),
        scan_id,
    );
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

/// Stamps the scan tag here rather than at each call site, so a message cannot be
/// sent without one.
fn send(tx: &Sender<ScanEvent>, scan_id: u64, message: ScanMessage) {
    let _ = tx.send(ScanEvent { scan_id, message });
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
    tx: Sender<ScanEvent>,
    ctx: egui::Context,
    db_path: PathBuf,
    scan_id: u64,
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
            send(&tx, scan_id, ScanMessage::Complete);
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
                    // A row written before the cache stored the frame size has
                    // none, and the rules key off resolution: re-read the header
                    // once and fill it in, so this photo is decided the same way on
                    // every later scan. Costs one header read per legacy row.
                    if c.frame.is_none() {
                        if let Ok(preview) = load_scan_preview(path) {
                            c.frame = Some(FrameSize::new(
                                preview.original_width,
                                preview.original_height,
                            ));
                            let db_guard = db.lock().unwrap();
                            if let Some(ref db_conn) = *db_guard {
                                let _ = db_conn.insert_cache(&file_hash, &c);
                            }
                        }
                    }

                    // If photo was previously cached without embedding, backfill it now
                    if c.embedding.is_empty() {
                        if let Some(ref sess) = *session {
                            if let Ok(preview) = load_scan_preview(path) {
                                if let Ok(emb) = extract_embedding(sess, &preview.image) {
                                    if !emb.is_empty() {
                                        c.embedding = emb;
                                        let db_guard = db.lock().unwrap();
                                        if let Some(ref db_conn) = *db_guard {
                                            let _ = db_conn.insert_cache(&file_hash, &c);
                                        }
                                    }
                                }
                            }
                        }
                    }

                    let facts = PhotoFacts {
                        path: path.clone(),
                        date: c.date,
                        frame: c.frame,
                        is_exif: c.is_exif_date,
                        embedding: c.embedding.clone(),
                    };
                    let classification = profiles.classify(&facts, threshold);

                    let image = if let Some(ref thumb) = c.thumbnail {
                        cached_thumb_to_egui(thumb)
                    } else if let Ok(preview) = load_scan_preview(path) {
                        let (cached_thumb, color_img) = dynamic_to_cached_thumb(&preview.image);
                        c.thumbnail = Some(cached_thumb);
                        let db_guard = db.lock().unwrap();
                        if let Some(ref db_conn) = *db_guard {
                            let _ = db_conn.insert_cache(&file_hash, &c);
                        }
                        color_img
                    } else {
                        return;
                    };

                    send(
                        &tx,
                        scan_id,
                        ScanMessage::Item(ProcessedPayload {
                            facts,
                            classification,
                            image,
                        }),
                    );
                    ctx.request_repaint();
                    return;
                }

                // 2. Uncached image: decode once, at the size the pipeline consumes
                let (date_info, preview, cached_thumb, image) = match load_scan_preview(path) {
                    Ok(preview) => {
                        let (thumb, egui_img) = dynamic_to_cached_thumb(&preview.image);
                        let (date, is_exif_date) = extract_date(path);
                        ((date, is_exif_date), preview, thumb, egui_img)
                    }
                    Err(_) => return,
                };

                let mut facts = PhotoFacts {
                    path: path.clone(),
                    date: date_info.0,
                    // The heuristics read real resolution (a 1920x1080 PNG is a
                    // screenshot), so the facts carry the source dimensions
                    // rather than the preview's.
                    frame: Some(FrameSize::new(
                        preview.original_width,
                        preview.original_height,
                    )),
                    is_exif: date_info.1,
                    embedding: Vec::new(),
                };

                // Thumbnail first, so the photo is on screen while the model
                // works. `Pending` rather than a "Classifying..." category: there
                // is no category yet, so there is nothing to edit into one.
                send(
                    &tx,
                    scan_id,
                    ScanMessage::Item(ProcessedPayload {
                        facts: facts.clone(),
                        classification: Classification::Pending,
                        image,
                    }),
                );
                ctx.request_repaint();

                // 3. Extract embedding in background
                facts.embedding = if let Some(ref sess) = *session {
                    extract_embedding(sess, &preview.image).unwrap_or_default()
                } else {
                    Vec::new()
                };

                // `facts.frame` carries the source dimensions rather than the
                // preview's, which is what the resolution rule reads.
                let classification = profiles.classify(&facts, threshold);

                // 4. Save to cache with thumbnail and frame size, so the next
                // scan classifies this photo from the same resolution.
                {
                    let db_guard = db.lock().unwrap();
                    if let Some(ref db_conn) = *db_guard {
                        let _ = db_conn.insert_cache(
                            &file_hash,
                            &CachedPhotoData {
                                date: facts.date,
                                is_exif_date: facts.is_exif,
                                embedding: facts.embedding.clone(),
                                thumbnail: Some(cached_thumb),
                                frame: facts.frame,
                            },
                        );
                    }
                }

                // 5. Update UI with final classification
                send(
                    &tx,
                    scan_id,
                    ScanMessage::Update {
                        facts,
                        classification,
                    },
                );
                ctx.request_repaint();
            });
        });

        send(&tx, scan_id, ScanMessage::Complete);
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

    fn png_of_size(path: &Path, width: u32, height: u32, rgb: [u8; 3]) {
        let mut img = image::RgbImage::new(width, height);
        for p in img.pixels_mut() {
            *p = image::Rgb(rgb);
        }
        img.save(path).unwrap();
    }

    /// What one scan of a folder produced, keyed by file name: the category each
    /// photo was staged with, and whether it was still pending.
    type ScanResult = std::collections::HashMap<String, (String, bool)>;

    /// Runs a scan to completion, returning what it staged and what it later
    /// updated.
    fn scan_once(folder: &Path, db_path: &Path) -> (ScanResult, ScanResult) {
        let (tx, rx) = std::sync::mpsc::channel();
        scan_folder_with_db(
            folder.to_path_buf(),
            ProfileStore::default(),
            rules_only(),
            tx,
            egui::Context::default(),
            db_path.to_path_buf(),
            1,
        );

        let name_of = |p: &Path| p.file_name().unwrap().to_string_lossy().into_owned();
        let mut items = ScanResult::new();
        let mut updates = ScanResult::new();
        while let Ok(event) = rx.recv_timeout(std::time::Duration::from_secs(60)) {
            match event.message {
                ScanMessage::Item(payload) => {
                    let decision = payload.classification.decided();
                    let category = decision.map(|d| d.category.as_str()).unwrap_or_default();
                    items.insert(
                        name_of(&payload.facts.path),
                        (category.to_string(), decision.is_none()),
                    );
                }
                ScanMessage::Update {
                    facts,
                    classification,
                } => {
                    let category = classification.decided().map(|d| d.category.as_str());
                    updates.insert(
                        name_of(&facts.path),
                        (category.unwrap_or_default().to_string(), false),
                    );
                }
                ScanMessage::Complete => break,
            }
        }
        (items, updates)
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
            1,
        );

        let mut received_complete = false;
        while let Ok(event) = rx.recv_timeout(std::time::Duration::from_secs(2)) {
            if let ScanMessage::Complete = event.message {
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

        // No screenshot keyword in either name, so only the frame size can make
        // them a screenshot.
        png_of_size(
            &temp_dir.join("holiday_pic.png"),
            1920,
            1080,
            [10, 100, 150],
        );
        png_of_size(&temp_dir.join("small_pic.png"), 640, 360, [50, 100, 150]);
        fs::write(temp_dir.join("readme.txt"), b"not a photo").unwrap();

        // 1st scan: uncached. Staged as Pending, decided in an Update.
        let (items, updates) = scan_once(&temp_dir, &test_db_path);
        assert_eq!(items.len(), 2);
        assert_eq!(updates.len(), 2);
        assert!(
            items.values().all(|(_, pending)| *pending),
            "every freshly decoded photo is staged before it is classified: {items:?}"
        );
        // The rule needs width >= 800 and a 16:9 frame; only the photo read at its
        // true size passes both.
        assert_eq!(
            updates["holiday_pic.png"].0, "Screenshots",
            "a 1920x1080 PNG is a screenshot: {updates:?}"
        );
        assert_eq!(updates["small_pic.png"].0, "Unsorted", "{updates:?}");

        // 2nd scan: from the cache, and it must reach the same answer — out of the
        // cached frame size rather than the 200x140 thumbnail.
        let (cached_items, cached_updates) = scan_once(&temp_dir, &test_db_path);
        assert_eq!(cached_items.len(), 2);
        assert!(cached_updates.is_empty(), "a cached photo needs no update");
        assert!(
            cached_items.values().all(|(_, pending)| !*pending),
            "a cached photo is classified before it is staged: {cached_items:?}"
        );
        for name in ["holiday_pic.png", "small_pic.png"] {
            assert_eq!(
                cached_items[name].0, updates[name].0,
                "{name} classified differently on a cached rescan: {cached_items:?} vs {updates:?}"
            );
        }

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
