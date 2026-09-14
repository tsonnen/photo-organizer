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
        let mut stmt = self.conn.prepare(
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

        self.conn.execute(
            "INSERT OR REPLACE INTO photo_cache (hash, year, month, is_exif_date, embedding, thumb_width, thumb_height, thumbnail)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![hash, data.year, data.month, data.is_exif_date as i32, blob, tw, th, trgba],
        )?;
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
}
