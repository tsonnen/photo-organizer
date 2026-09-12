use crate::execution_engine::{ExecutionManifest, FileOperation};
use anyhow::{Result};
use std::fs;
use std::path::{Path, PathBuf};

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
                if let Some(p) = side_src.parent() { let _ = fs::create_dir_all(p); }
                if fs::rename(side_dst, side_src).is_err() && fs::copy(side_dst, side_src).is_ok() {
                    let _ = fs::remove_file(side_dst);
                }
            }
        }

        UndoStatus::Restored(op.source.clone())
    }
}
