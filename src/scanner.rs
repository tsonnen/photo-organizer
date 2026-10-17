use crate::classification::{Classification, FrameSize, PhotoFacts};
use crate::db::{CachedPhotoData, CachedThumbnail, Database};
use crate::inference::{extract_embedding, init_clip_session};
use crate::media::{dynamic_to_cached_thumb, extract_date, load_scan_preview};
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
    pub thumbnail: CachedThumbnail,
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
                    // A row written before the cache stored the day has none, and
                    // the date sort and the calendar both key off it: re-read the
                    // file's timestamp once and fill it in, the same way a missing
                    // frame size is repaired below. Without this a returning user's
                    // `photo_cache.db` keeps reporting `None` forever — every photo
                    // in a month stays tied under the sort, which is the complaint
                    // the day was added to answer. Costs one header read per legacy
                    // row, once.
                    //
                    // `is_exif_date` travels with the day because `extract_date`
                    // returns them as a pair, and a date read off the filesystem
                    // recorded under a row that claims EXIF is not a row anyone
                    // can reason about. It cannot disagree with what is already
                    // there: the hash matches, so the file has not changed, and the
                    // flag was written from the same call.
                    if c.date.day.is_none() {
                        let (date, is_exif_date) = extract_date(path);
                        c.date = date;
                        c.is_exif_date = is_exif_date;
                        let db_guard = db.lock().unwrap();
                        if let Some(ref db_conn) = *db_guard {
                            let _ = db_conn.insert_cache(&file_hash, &c);
                        }
                    }

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

                    // Ensure the cached row carries a thumbnail: rows written before
                    // the thumbnail column existed have none, so backfill once.
                    if c.thumbnail.is_none() {
                        if let Ok(preview) = load_scan_preview(path) {
                            let (cached_thumb, _color_img) =
                                dynamic_to_cached_thumb(&preview.image);
                            c.thumbnail = Some(cached_thumb);
                            let db_guard = db.lock().unwrap();
                            if let Some(ref db_conn) = *db_guard {
                                let _ = db_conn.insert_cache(&file_hash, &c);
                            }
                        } else {
                            return;
                        }
                    }

                    send(
                        &tx,
                        scan_id,
                        ScanMessage::Item(ProcessedPayload {
                            facts,
                            classification,
                            thumbnail: c
                                .thumbnail
                                .clone()
                                .expect("scan always carries a thumbnail"),
                        }),
                    );
                    ctx.request_repaint();
                    return;
                }

                // 2. Uncached image: decode once, at the size the pipeline consumes
                let (date_info, preview, cached_thumb) = match load_scan_preview(path) {
                    Ok(preview) => {
                        let (thumb, _color_img) = dynamic_to_cached_thumb(&preview.image);
                        let (date, is_exif_date) = extract_date(path);
                        ((date, is_exif_date), preview, thumb)
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
                        thumbnail: cached_thumb.clone(),
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
    use crate::classification::PhotoDate;
    use crate::media;

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

    /// Runs a scan to completion, returning the date each photo was staged with,
    /// keyed by file name.
    ///
    /// The `Item` message rather than the `Update`, so it reads the date the scan
    /// actually decided on: a cached photo is classified before it is staged and
    /// never sends an `Update` at all.
    fn scan_dates(folder: &Path, db_path: &Path) -> std::collections::HashMap<String, PhotoDate> {
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
        let mut dates = std::collections::HashMap::new();
        while let Ok(event) = rx.recv_timeout(std::time::Duration::from_secs(60)) {
            match event.message {
                ScanMessage::Item(payload) => {
                    dates.insert(name_of(&payload.facts.path), payload.facts.date);
                }
                ScanMessage::Complete => break,
                _ => {}
            }
        }
        dates
    }

    #[test]
    fn a_rescan_backfills_the_day_a_legacy_row_is_missing() {
        // The state every returning user is in: a `photo_cache.db` written before
        // the day column existed, so every row's `day` is NULL. Three comments and
        // a PR body said the next rescan would read the day back off the file.
        // Without the repair it never does — the reader reports `None`, the
        // scanner passed that through as the photo's date, and `insert_cache`
        // wrote the `None` straight back out — so every photo in a month stayed
        // tied under the date sort, which is the whole complaint the day was
        // added to answer.
        let temp_dir =
            std::env::temp_dir().join(format!("test_scan_day_backfill_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();
        let db_path = temp_dir.join("legacy_cache.db");
        let photo = temp_dir.join("holiday_pic.png");
        png_of_size(&photo, 1920, 1080, [10, 100, 150]);

        // The row a build without the day column would have left: the real hash,
        // so the scan finds it, but no day.
        let hash = Database::compute_file_hash(&photo).expect("hash the photo");
        let db = Database::init(&db_path).expect("open the cache");
        db.insert_cache(
            &hash,
            &CachedPhotoData {
                // Deliberately a different year and month from the file's, so a
                // "carry the old value through" bug cannot pass this by accident.
                date: PhotoDate::new(1999, 1, None),
                is_exif_date: false,
                embedding: vec![],
                thumbnail: None,
                frame: None,
            },
        )
        .expect("write the legacy row");

        let dates = scan_dates(&temp_dir, &db_path);
        let from_file = media::extract_date(&photo).0;
        assert!(
            from_file.day.is_some(),
            "the fixture itself has to carry a day, or this proves nothing"
        );
        assert_eq!(
            dates["holiday_pic.png"], from_file,
            "the rescan should have replaced the legacy row's date with the file's"
        );

        // And it is written back, so the repair costs one scan rather than one per
        // scan for the rest of the folder's life.
        let reread = db
            .get_cached(&hash)
            .expect("query the cache")
            .expect("the row is still there");
        assert_eq!(reread.date, from_file, "and persisted, not just reported");

        let _ = fs::remove_dir_all(&temp_dir);
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
