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

#[derive(Debug, Clone)]
pub struct CachedPhotoData {
    pub year: u32,
    pub month: u32,
    pub is_exif_date: bool,
    pub embedding: Vec<f32>,
    pub thumbnail: Option<CachedThumbnail>,
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
                thumbnail BLOB
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
            "SELECT year, month, is_exif_date, embedding, thumb_width, thumb_height, thumbnail FROM photo_cache WHERE hash = ?1",
        )?;
        let mut rows = stmt.query(params![hash])?;

        if let Some(row) = rows.next()? {
            let year: u32 = row.get(0)?;
            let month: u32 = row.get(1)?;
            let is_exif: i32 = row.get(2)?;
            let blob: Vec<u8> = row.get(3)?;
            #[allow(clippy::chunks_exact_to_as_chunks)]
            let embedding: Vec<f32> = blob
                .chunks_exact(4)
                .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
                .collect();

            let thumb_w: Option<u32> = row.get(4)?;
            let thumb_h: Option<u32> = row.get(5)?;
            let thumb_blob: Option<Vec<u8>> = row.get(6)?;

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

            Ok(Some(CachedPhotoData {
                year,
                month,
                is_exif_date: is_exif != 0,
                embedding,
                thumbnail,
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

        self.conn
            .prepare_cached(
                "INSERT OR REPLACE INTO photo_cache (hash, year, month, is_exif_date, embedding, thumb_width, thumb_height, thumbnail)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )?
            .execute(params![hash, data.year, data.month, data.is_exif_date as i32, blob, tw, th, trgba])?;
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
            year: 2025,
            month: 12,
            is_exif_date: true,
            embedding: vec![0.123, 0.456, -0.789, 1.0],
            thumbnail: Some(CachedThumbnail {
                width: 2,
                height: 1,
                rgba: thumb_rgba.clone(),
            }),
        };

        db.insert_cache(hash, &photo_data).expect("insert cache");
        let retrieved = db
            .get_cached(hash)
            .expect("query cache")
            .expect("found record");

        assert_eq!(retrieved.year, 2025);
        assert_eq!(retrieved.month, 12);
        assert!(retrieved.is_exif_date);
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
        assert_eq!(cached.year, 2022);
        assert_eq!(cached.month, 5);
        assert!(cached.is_exif_date);
        assert_eq!(cached.embedding, vec![1.0, 2.0]);
        assert_eq!(cached.thumbnail, None);

        let _ = std::fs::remove_file(&db_path);
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
        assert_eq!(cached.year, 2024);
        // Thumbnail should be gracefully set to None when buffer is truncated/mismatched
        assert_eq!(cached.thumbnail, None);
    }

    #[test]
    fn test_db_update_existing_cache() {
        let db = Database::init(":memory:").expect("init in-memory db");
        let hash = "update_test_hash";

        let initial_data = CachedPhotoData {
            year: 2020,
            month: 1,
            is_exif_date: false,
            embedding: vec![0.1],
            thumbnail: None,
        };
        db.insert_cache(hash, &initial_data).expect("insert");

        let updated_data = CachedPhotoData {
            year: 2021,
            month: 6,
            is_exif_date: true,
            embedding: vec![0.5, 0.9],
            thumbnail: Some(CachedThumbnail {
                width: 1,
                height: 1,
                rgba: vec![10, 20, 30, 40],
            }),
        };
        db.insert_cache(hash, &updated_data).expect("update");

        let retrieved = db.get_cached(hash).unwrap().unwrap();
        assert_eq!(retrieved.year, 2021);
        assert_eq!(retrieved.month, 6);
        assert!(retrieved.is_exif_date);
        assert_eq!(retrieved.embedding, vec![0.5, 0.9]);
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
                    year: 2000 + i as u32,
                    month: (i % 12 + 1) as u32,
                    is_exif_date: true,
                    embedding: vec![i as f32],
                    thumbnail: Some(CachedThumbnail {
                        width: 1,
                        height: 1,
                        rgba: vec![i as u8, 0, 0, 255],
                    }),
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
            assert_eq!(item.year, 2000 + i as u32);
        }
    }
}
