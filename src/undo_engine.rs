use crate::execution_engine::{ExecutionManifest, FileOperation};
use anyhow::Result;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, PartialEq, Eq)]
pub enum UndoStatus {
    Restored(PathBuf),
    SkippedMissing(PathBuf),
    Failed(PathBuf, String),
}

pub struct UndoEngine;

impl UndoEngine {
    pub fn rollback_from_file<P, F>(manifest_path: P, progress: F) -> Result<Vec<UndoStatus>>
    where
        P: AsRef<Path>,
        F: Fn(usize, usize, &UndoStatus),
    {
        let contents = fs::read_to_string(&manifest_path)?;
        let manifest: ExecutionManifest = serde_json::from_str(&contents)?;

        let mut results = Vec::new();
        let ops_reversed: Vec<&FileOperation> = manifest.completed_ops.iter().rev().collect();
        let total = ops_reversed.len();

        for (idx, op) in ops_reversed.iter().enumerate() {
            let status = Self::undo_op(op);
            progress(idx + 1, total, &status);
            results.push(status);
        }
        Ok(results)
    }

    fn undo_op(op: &FileOperation) -> UndoStatus {
        if !op.destination.exists() {
            return UndoStatus::SkippedMissing(op.destination.clone());
        }

        if let Some(parent) = op.source.parent() {
            let _ = fs::create_dir_all(parent);
        }

        if fs::rename(&op.destination, &op.source).is_err() {
            if fs::copy(&op.destination, &op.source).is_ok() {
                let _ = fs::remove_file(&op.destination);
            } else {
                return UndoStatus::Failed(op.destination.clone(), "Failed back-copy".to_string());
            }
        }

        // Restore sidecars
        for (side_src, side_dst) in &op.sidecars {
            if side_dst.exists() {
                if let Some(p) = side_src.parent() {
                    let _ = fs::create_dir_all(p);
                }
                if fs::rename(side_dst, side_src).is_err() && fs::copy(side_dst, side_src).is_ok() {
                    let _ = fs::remove_file(side_dst);
                }
            }
        }

        UndoStatus::Restored(op.source.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;

    #[test]
    fn test_rollback_from_file_restores_moved_file() {
        let temp_dir =
            std::env::temp_dir().join(format!("test_undo_engine_{}", std::process::id()));
        let src_path = temp_dir.join("original/photo.jpg");
        let dst_path = temp_dir.join("destination/photo.jpg");
        fs::create_dir_all(dst_path.parent().unwrap()).unwrap();

        {
            let mut f = File::create(&dst_path).unwrap();
            f.write_all(b"moved image data").unwrap();
        }

        let manifest = ExecutionManifest {
            completed_ops: vec![FileOperation {
                source: src_path.clone(),
                destination: dst_path.clone(),
                sidecars: Vec::new(),
            }],
            failed_ops: Vec::new(),
        };

        let manifest_path = temp_dir.join("manifest.json");
        fs::write(&manifest_path, serde_json::to_string(&manifest).unwrap()).unwrap();

        let statuses = UndoEngine::rollback_from_file(&manifest_path, |_, _, _| {}).unwrap();
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0], UndoStatus::Restored(src_path.clone()));
        assert!(src_path.exists());
        assert!(!dst_path.exists());

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_rollback_skipped_missing() {
        let temp_dir =
            std::env::temp_dir().join(format!("test_undo_missing_{}", std::process::id()));
        fs::create_dir_all(&temp_dir).unwrap();

        let src_path = temp_dir.join("src.jpg");
        let dst_path = temp_dir.join("nonexistent_dst.jpg");

        let manifest = ExecutionManifest {
            completed_ops: vec![FileOperation {
                source: src_path,
                destination: dst_path.clone(),
                sidecars: Vec::new(),
            }],
            failed_ops: Vec::new(),
        };

        let manifest_path = temp_dir.join("manifest.json");
        fs::write(&manifest_path, serde_json::to_string(&manifest).unwrap()).unwrap();

        let statuses = UndoEngine::rollback_from_file(&manifest_path, |_, _, _| {}).unwrap();
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0], UndoStatus::SkippedMissing(dst_path));

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
