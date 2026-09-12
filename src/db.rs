use anyhow::Result;
use rusqlite::{params, Connection};
use std::fs::File;
use std::io::Read;
use std::path::Path;

pub struct Database {
    conn: Connection,
}

pub struct CachedPhotoData {
    pub year: u32,
    pub month: u32,
    pub is_exif_date: bool,
    pub embedding: Vec<f32>,
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
                embedding BLOB NOT NULL
            )",
            [],
        )?;
        Ok(Self { conn })
    }

    pub fn compute_file_hash<P: AsRef<Path>>(path: P) -> Result<String> {
        let mut file = File::open(path)?;
        let mut hasher = blake3::Hasher::new();
        let mut buffer = [0u8; 65536];
        loop {
            let count = file.read(&mut buffer)?;
            if count == 0 { break; }
            hasher.update(&buffer[..count]);
        }
        Ok(hasher.finalize().to_hex().to_string())
    }

    pub fn get_cached(&self, hash: &str) -> Result<Option<CachedPhotoData>> {
        let mut stmt = self.conn.prepare(
            "SELECT year, month, is_exif_date, embedding FROM photo_cache WHERE hash = ?1",
        )?;
        let mut rows = stmt.query(params![hash])?;

        if let Some(row) = rows.next()? {
            let year: u32 = row.get(0)?;
            let month: u32 = row.get(1)?;
            let is_exif: i32 = row.get(2)?;
            let blob: Vec<u8> = row.get(3)?;
            let embedding: Vec<f32> = blob
                .chunks_exact(4)
                .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
                .collect();

            Ok(Some(CachedPhotoData {
                year,
                month,
                is_exif_date: is_exif != 0,
                embedding,
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

        self.conn.execute(
            "INSERT OR REPLACE INTO photo_cache (hash, year, month, is_exif_date, embedding)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![hash, data.year, data.month, data.is_exif_date as i32, blob],
        )?;
        Ok(())
    }
}
