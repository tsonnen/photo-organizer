use crate::classification::{FrameSize, PhotoDate};
use anyhow::Result;
use rusqlite::{params, Connection};
use std::fs::File;
use std::io::Read;
use std::path::Path;

pub struct Database {
    conn: Connection,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CachedThumbnail {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// One cached photo: everything a rescan needs to classify and display it
/// without reading the file again.
#[derive(Debug, Clone)]
pub struct CachedPhotoData {
    pub date: PhotoDate,
    pub is_exif_date: bool,
    pub embedding: Vec<f32>,
    pub thumbnail: Option<CachedThumbnail>,
    /// The photo's true pixel dimensions, as `media::load_scan_preview` read
    /// them off the file.
    ///
    /// Stored because the rules key off resolution and the thumbnail is at most
    /// 200x140, so a rescan that used it classified a 1920x1080 PNG as
    /// Unsorted — the reverse of what the scan that read the file decided.
    /// `None` for rows written before these columns existed, which the scan
    /// backfills once.
    pub frame: Option<FrameSize>,
}

impl Database {
    pub fn init<P: AsRef<Path>>(db_path: P) -> Result<Self> {
        let conn = Connection::open(db_path)?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS photo_cache (
                hash TEXT PRIMARY KEY,
                year INTEGER NOT NULL,
                month INTEGER NOT NULL,
                is_exif_date INTEGER NOT NULL,
                embedding BLOB NOT NULL,
                thumb_width INTEGER,
                thumb_height INTEGER,
                thumbnail BLOB,
                original_width INTEGER,
                original_height INTEGER,
                day INTEGER
            )",
            [],
        )?;
        // Ensure columns exist if opened on older database version
        let _ = conn.execute("ALTER TABLE photo_cache ADD COLUMN thumb_width INTEGER", []);
        let _ = conn.execute(
            "ALTER TABLE photo_cache ADD COLUMN thumb_height INTEGER",
            [],
        );
        let _ = conn.execute("ALTER TABLE photo_cache ADD COLUMN thumbnail BLOB", []);
        let _ = conn.execute(
            "ALTER TABLE photo_cache ADD COLUMN original_width INTEGER",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE photo_cache ADD COLUMN original_height INTEGER",
            [],
        );
        // Nullable, unlike `year` and `month`: a row cached before this column
        // existed has no day, and zero is a day that never happened. The scan
        // backfills one from the file on the next rescan.
        let _ = conn.execute("ALTER TABLE photo_cache ADD COLUMN day INTEGER", []);

        // The cache takes one insert per photo during a scan and is read back on
        // every rescan. Under the default rollback journal with
        // synchronous=FULL that costs an fsync per insert (~3.6ms measured);
        // WAL with synchronous=NORMAL costs ~0.10ms and still survives a process
        // crash, which is all that matters for a cache that can be rebuilt from
        // the photos on disk. Both are best-effort: an unsupported journal mode
        // (e.g. `:memory:`) just leaves SQLite on its defaults.
        let _ = conn.pragma_update(None, "journal_mode", "WAL");
        let _ = conn.pragma_update(None, "synchronous", "NORMAL");

        Ok(Self { conn })
    }

    pub fn compute_file_hash<P: AsRef<Path>>(path: P) -> Result<String> {
        let mut file = File::open(path)?;
        let mut hasher = blake3::Hasher::new();
        let mut buffer = [0u8; 65536];
        loop {
            let count = file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            hasher.update(&buffer[..count]);
        }
        Ok(hasher.finalize().to_hex().to_string())
    }

    pub fn get_cached(&self, hash: &str) -> Result<Option<CachedPhotoData>> {
        // A rescan runs this once per file in the folder, so the statement is
        // cached rather than re-parsed every time. Safe because all access goes
        // through a single connection guarded by a mutex.
        let mut stmt = self.conn.prepare_cached(
            "SELECT year, month, day, is_exif_date, embedding, thumb_width, thumb_height, thumbnail, original_width, original_height FROM photo_cache WHERE hash = ?1",
        )?;
        let mut rows = stmt.query(params![hash])?;

        if let Some(row) = rows.next()? {
            let year: u32 = row.get(0)?;
            let month: u32 = row.get(1)?;
            // A row cached before the day column existed reports none, and the scan
            // re-reads it from the file on the next rescan. Zero is not a day.
            let day: Option<u32> = row.get(2)?;
            let is_exif: i32 = row.get(3)?;
            let blob: Vec<u8> = row.get(4)?;
            #[allow(clippy::chunks_exact_to_as_chunks)]
            let embedding: Vec<f32> = blob
                .chunks_exact(4)
                .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
                .collect();

            let thumb_w: Option<u32> = row.get(5)?;
            let thumb_h: Option<u32> = row.get(6)?;
            let thumb_blob: Option<Vec<u8>> = row.get(7)?;

            let thumbnail = match (thumb_w, thumb_h, thumb_blob) {
                (Some(w), Some(h), Some(rgba))
                    if w > 0 && h > 0 && rgba.len() == (w * h * 4) as usize =>
                {
                    Some(CachedThumbnail {
                        width: w,
                        height: h,
                        rgba,
                    })
                }
                _ => None,
            };

            // A row cached before the frame columns existed has no size to offer,
            // and the scan re-reads the header once to fill it in. Zero dimensions
            // count as unknown: not a frame any ratio rule could read.
            let original_w: Option<u32> = row.get(8)?;
            let original_h: Option<u32> = row.get(9)?;
            let frame = match (original_w, original_h) {
                (Some(w), Some(h)) if w > 0 && h > 0 => Some(FrameSize::new(w, h)),
                _ => None,
            };

            Ok(Some(CachedPhotoData {
                date: PhotoDate::new(year, month, day.filter(|d| *d > 0)),
                is_exif_date: is_exif != 0,
                embedding,
                thumbnail,
                frame,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn insert_cache(&self, hash: &str, data: &CachedPhotoData) -> Result<()> {
        let mut blob = Vec::with_capacity(data.embedding.len() * 4);
        for &val in &data.embedding {
            blob.extend_from_slice(&val.to_le_bytes());
        }

        let (tw, th, trgba) = match &data.thumbnail {
            Some(t) => (Some(t.width), Some(t.height), Some(t.rgba.as_slice())),
            None => (None, None, None),
        };
        let (frame_w, frame_h) = match data.frame {
            Some(frame) => (Some(frame.width), Some(frame.height)),
            None => (None, None),
        };

        self.conn
            .prepare_cached(
                "INSERT OR REPLACE INTO photo_cache (hash, year, month, day, is_exif_date, embedding, thumb_width, thumb_height, thumbnail, original_width, original_height)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            )?
            .execute(params![
                hash,
                data.date.year,
                data.date.month,
                data.date.day,
                data.is_exif_date as i32,
                blob,
                tw,
                th,
                trgba,
                frame_w,
                frame_h,
            ])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_db_insert_and_get() {
        let db = Database::init(":memory:").expect("init in-memory db");
        let hash = "abc123hash";
        let thumb_rgba = vec![255u8, 0, 0, 255, 0, 255, 0, 255]; // 2 pixels RGBA
        let photo_data = CachedPhotoData {
            date: PhotoDate::new(2025, 12, Some(12)),
            is_exif_date: true,
            embedding: vec![0.123, 0.456, -0.789, 1.0],
            thumbnail: Some(CachedThumbnail {
                width: 2,
                height: 1,
                rgba: thumb_rgba.clone(),
            }),
            frame: Some(FrameSize::new(4000, 3000)),
        };

        db.insert_cache(hash, &photo_data).expect("insert cache");
        let retrieved = db
            .get_cached(hash)
            .expect("query cache")
            .expect("found record");

        assert_eq!(retrieved.date, PhotoDate::new(2025, 12, Some(12)));
        assert!(retrieved.is_exif_date);
        assert_eq!(retrieved.frame, Some(FrameSize::new(4000, 3000)));
        assert_eq!(retrieved.embedding.len(), 4);
        for (a, b) in retrieved.embedding.iter().zip(&photo_data.embedding) {
            assert!((a - b).abs() < 1e-6);
        }
        assert_eq!(
            retrieved.thumbnail,
            Some(CachedThumbnail {
                width: 2,
                height: 1,
                rgba: thumb_rgba,
            })
        );
    }

    #[test]
    fn test_db_get_missing() {
        let db = Database::init(":memory:").expect("init in-memory db");
        let retrieved = db.get_cached("nonexistent").expect("query cache");
        assert!(retrieved.is_none());
    }

    #[test]
    fn test_compute_file_hash() {
        let temp_dir = std::env::temp_dir();
        let test_file = temp_dir.join(format!("test_hash_{}.tmp", std::process::id()));
        {
            let mut f = File::create(&test_file).expect("create file");
            f.write_all(b"hello photo organizer").expect("write bytes");
        }

        let hash1 = Database::compute_file_hash(&test_file).expect("compute hash");
        let hash2 = Database::compute_file_hash(&test_file).expect("compute hash again");
        let _ = std::fs::remove_file(&test_file);

        assert_eq!(hash1, hash2);
        assert!(!hash1.is_empty());
    }

    #[test]
    fn test_db_schema_migration_from_v1() {
        let temp_dir = std::env::temp_dir();
        let db_path = temp_dir.join(format!("test_migration_{}.db", std::process::id()));

        // Create legacy v1 schema without thumbnail columns
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute(
                "CREATE TABLE photo_cache (
                    hash TEXT PRIMARY KEY,
                    year INTEGER NOT NULL,
                    month INTEGER NOT NULL,
                    is_exif_date INTEGER NOT NULL,
                    embedding BLOB NOT NULL
                )",
                [],
            )
            .unwrap();

            let emb = vec![1.0f32, 2.0f32];
            let mut blob = Vec::new();
            for &v in &emb {
                blob.extend_from_slice(&v.to_le_bytes());
            }

            conn.execute(
                "INSERT INTO photo_cache (hash, year, month, is_exif_date, embedding)
                 VALUES ('legacy_hash', 2022, 5, 1, ?1)",
                params![blob],
            )
            .unwrap();
        }

        // Open with Database::init which performs migration
        let db = Database::init(&db_path).expect("initialize and migrate db");
        let cached = db
            .get_cached("legacy_hash")
            .expect("query legacy item")
            .expect("item exists");
        // A legacy row has no day column, so it reports none. The scan backfills one
        // from the file on the next rescan. What matters is that the reader reports
        // the absence rather than inventing a day, which would file the photo
        // somewhere it was never taken.
        assert_eq!(cached.date, PhotoDate::new(2022, 5, None));
        assert!(cached.is_exif_date);
        assert_eq!(cached.embedding, vec![1.0, 2.0]);
        assert_eq!(cached.thumbnail, None);
        // A legacy row has no frame size. What matters here is that the reader
        // reports the absence rather than inventing one from the thumbnail.
        assert_eq!(cached.frame, None);

        let _ = std::fs::remove_file(&db_path);
    }

    #[test]
    fn a_day_written_by_one_scan_is_read_back_by_the_next() {
        // The day is only useful if it survives the cache: without this the whole
        // change is invisible on any folder that has been scanned before, since
        // every row would come back with no day and every photo would still sort
        // as "some day in this month".
        let db = Database::init(":memory:").expect("init in-memory db");
        let hash = "day_hash";
        db.insert_cache(
            hash,
            &CachedPhotoData {
                date: PhotoDate::new(2021, 3, Some(14)),
                is_exif_date: true,
                embedding: vec![0.5],
                thumbnail: None,
                frame: None,
            },
        )
        .expect("insert");

        let read = db.get_cached(hash).expect("query").expect("present");
        assert_eq!(read.date, PhotoDate::new(2021, 3, Some(14)));
    }

    #[test]
    fn a_zero_day_is_treated_as_no_day() {
        // `day` is nullable, but a hand-edited or half-written row could hold 0,
        // and day zero is a date that never happened.
        let db = Database::init(":memory:").expect("init in-memory db");
        db.insert_cache(
            "zero_day",
            &CachedPhotoData {
                date: PhotoDate::new(2021, 3, Some(0)),
                is_exif_date: false,
                embedding: vec![0.5],
                thumbnail: None,
                frame: None,
            },
        )
        .expect("insert");

        let read = db.get_cached("zero_day").expect("query").expect("present");
        assert_eq!(read.date.day, None);
    }

    #[test]
    fn test_db_frame_size_survives_a_rescan() {
        // The rescan has to read the same resolution the first scan decided on,
        // out of the cache alone.
        let db = Database::init(":memory:").expect("init in-memory db");
        let hash = "frame_hash";
        db.insert_cache(
            hash,
            &CachedPhotoData {
                date: PhotoDate::new(2026, 9, Some(9)),
                is_exif_date: false,
                embedding: vec![0.1, 0.2],
                thumbnail: Some(CachedThumbnail {
                    width: 200,
                    height: 140,
                    rgba: vec![0u8; 200 * 140 * 4],
                }),
                frame: Some(FrameSize::new(1920, 1080)),
            },
        )
        .expect("insert");

        let cached = db.get_cached(hash).expect("query").expect("found");
        // The thumbnail is a different shape entirely, and reading the frame
        // columns is what stops a rescan using it.
        assert_eq!(cached.frame, Some(FrameSize::new(1920, 1080)));
        assert_eq!(
            cached.thumbnail.map(|t| (t.width, t.height)),
            Some((200, 140))
        );
    }

    #[test]
    fn test_db_zero_frame_dimensions_read_as_unknown() {
        // A corrupt row must not claim a 0x0 frame: the ratio rules would divide
        // by it, and the scan would treat it as needing a backfill.
        let db = Database::init(":memory:").expect("init in-memory db");
        db.conn
            .execute(
                "INSERT INTO photo_cache (hash, year, month, is_exif_date, embedding, original_width, original_height)
                 VALUES ('zero_frame', 2024, 1, 1, X'0000803f', 0, 0)",
                [],
            )
            .unwrap();

        let cached = db.get_cached("zero_frame").expect("query").expect("found");
        assert_eq!(cached.frame, None);
    }

    #[test]
    fn test_db_corrupted_or_truncated_thumbnail() {
        let db = Database::init(":memory:").expect("init in-memory db");
        // Insert a record where thumb_width * thumb_height * 4 does NOT match thumbnail blob length
        db.conn
            .execute(
                "INSERT INTO photo_cache (hash, year, month, is_exif_date, embedding, thumb_width, thumb_height, thumbnail)
                 VALUES ('corrupted_thumb', 2024, 1, 1, X'0000803f', 10, 10, X'123456')",
                [],
            )
            .unwrap();

        let cached = db
            .get_cached("corrupted_thumb")
            .expect("query item")
            .expect("item exists");
        // The row names no day column, so the day is absent — the same legacy path the
        // migration test covers, reached here by a raw insert.
        assert_eq!(cached.date, PhotoDate::new(2024, 1, None));
        // Thumbnail should be gracefully set to None when buffer is truncated/mismatched
        assert_eq!(cached.thumbnail, None);
    }

    #[test]
    fn test_db_update_existing_cache() {
        let db = Database::init(":memory:").expect("init in-memory db");
        let hash = "update_test_hash";

        let initial_data = CachedPhotoData {
            date: PhotoDate::new(2020, 1, Some(1)),
            is_exif_date: false,
            embedding: vec![0.1],
            thumbnail: None,
            frame: None,
        };
        db.insert_cache(hash, &initial_data).expect("insert");

        let updated_data = CachedPhotoData {
            date: PhotoDate::new(2021, 6, Some(6)),
            is_exif_date: true,
            embedding: vec![0.5, 0.9],
            thumbnail: Some(CachedThumbnail {
                width: 1,
                height: 1,
                rgba: vec![10, 20, 30, 40],
            }),
            frame: Some(FrameSize::new(3000, 4000)),
        };
        db.insert_cache(hash, &updated_data).expect("update");

        let retrieved = db.get_cached(hash).unwrap().unwrap();
        assert_eq!(retrieved.date, PhotoDate::new(2021, 6, Some(6)));
        assert!(retrieved.is_exif_date);
        assert_eq!(retrieved.embedding, vec![0.5, 0.9]);
        assert_eq!(retrieved.frame, Some(FrameSize::new(3000, 4000)));
        assert_eq!(
            retrieved.thumbnail,
            Some(CachedThumbnail {
                width: 1,
                height: 1,
                rgba: vec![10, 20, 30, 40],
            })
        );
    }

    #[test]
    fn test_db_backfilling_a_frame_size_into_a_legacy_row() {
        // What a rescan of a pre-frame-size cache does: the row comes back without
        // a frame, and writing the size the file reports fills it in.
        let db = Database::init(":memory:").expect("init in-memory db");
        let hash = "backfill_hash";
        db.insert_cache(
            hash,
            &CachedPhotoData {
                date: PhotoDate::new(2023, 4, Some(4)),
                is_exif_date: false,
                embedding: vec![0.3],
                thumbnail: Some(CachedThumbnail {
                    width: 2,
                    height: 1,
                    rgba: vec![1u8, 2, 3, 4, 5, 6, 7, 8],
                }),
                frame: None,
            },
        )
        .expect("insert");

        let mut row = db.get_cached(hash).unwrap().unwrap();
        assert_eq!(row.frame, None);
        row.frame = Some(FrameSize::new(1920, 1080));
        db.insert_cache(hash, &row).expect("backfill");

        let reread = db.get_cached(hash).unwrap().unwrap();
        assert_eq!(reread.frame, Some(FrameSize::new(1920, 1080)));
        assert_eq!(reread.date, PhotoDate::new(2023, 4, Some(4)));
        assert_eq!(reread.thumbnail.map(|t| t.rgba.len()), Some(8));
    }

    #[test]
    fn test_db_concurrent_access() {
        use std::sync::{Arc, Mutex};
        use std::thread;

        let db = Arc::new(Mutex::new(
            Database::init(":memory:").expect("init in-memory db"),
        ));
        let mut handles = Vec::new();

        for i in 0..10 {
            let db_clone = Arc::clone(&db);
            handles.push(thread::spawn(move || {
                let hash = format!("hash_{}", i);
                let data = CachedPhotoData {
                    date: PhotoDate::new(2000 + i as u32, (i % 12 + 1) as u32, Some(15)),
                    is_exif_date: true,
                    embedding: vec![i as f32],
                    thumbnail: Some(CachedThumbnail {
                        width: 1,
                        height: 1,
                        rgba: vec![i as u8, 0, 0, 255],
                    }),
                    frame: Some(FrameSize::new(1000 + i as u32, 800)),
                };
                let guard = db_clone.lock().unwrap();
                guard.insert_cache(&hash, &data).unwrap();
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        let guard = db.lock().unwrap();
        for i in 0..10 {
            let hash = format!("hash_{}", i);
            let item = guard.get_cached(&hash).unwrap().unwrap();
            assert_eq!(item.date.year, 2000 + i as u32);
            assert_eq!(item.frame, Some(FrameSize::new(1000 + i as u32, 800)));
        }
    }
}
