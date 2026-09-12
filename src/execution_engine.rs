use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferMode {
    Move,
    Copy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileOperation {
    pub source: PathBuf,
    pub destination: PathBuf,
    pub sidecars: Vec<(PathBuf, PathBuf)>, // (source_sidecar, dest_sidecar)
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ExecutionManifest {
    pub completed_ops: Vec<FileOperation>,
    pub failed_ops: Vec<(FileOperation, String)>,
}

pub struct ExecutionEngine {
    base_output_dir: PathBuf,
    mode: TransferMode,
}

impl ExecutionEngine {
    pub fn new(base_output_dir: PathBuf, mode: TransferMode) -> Self {
        Self { base_output_dir, mode }
    }

    pub fn plan_batch(&self, inputs: &[RawPhotoInput]) -> Vec<FileOperation> {
        let mut planned_ops = Vec::new();
        let mut reserved_paths: HashSet<PathBuf> = HashSet::new();

        for input in inputs {
            let rel_dir = PathBuf::from(&input.subject)
                .join(format!("{:04}", input.year))
                .join(format!("{:02}", input.month));

            let target_dir = self.base_output_dir.join(rel_dir);
            let stem = input.source_path.file_stem().and_then(|s| s.to_str()).unwrap_or("file");
            let ext = input.source_path.extension().and_then(|e| e.to_str()).unwrap_or("");

            let (destination, final_stem) = resolve_destination(&target_dir, stem, ext, &reserved_paths);
            reserved_paths.insert(destination.clone());

            // Detect matching sidecar files (.xmp, .aae)
            let mut sidecars = Vec::new();
            for sidecar_ext in &["xmp", "XMP", "aae", "AAE"] {
                let sidecar_src = input.source_path.with_extension(sidecar_ext);
                if sidecar_src.exists() {
                    let sidecar_dst = target_dir.join(format!("{}.{}", final_stem, sidecar_ext));
                    reserved_paths.insert(sidecar_dst.clone());
                    sidecars.push((sidecar_src, sidecar_dst));
                }
            }

            planned_ops.push(FileOperation {
                source: input.source_path.clone(),
                destination,
                sidecars,
            });
        }

        planned_ops
    }

    pub fn execute_batch<F>(&self, operations: &[FileOperation], progress: F) -> ExecutionManifest
    where
        F: Fn(usize, usize, &FileOperation),
    {
        let mut manifest = ExecutionManifest::default();
        let total = operations.len();

        for (idx, op) in operations.iter().enumerate() {
            progress(idx + 1, total, op);
            match self.execute_single(op) {
                Ok(_) => manifest.completed_ops.push(op.clone()),
                Err(e) => manifest.failed_ops.push((op.clone(), e.to_string())),
            }
        }
        manifest
    }

    fn execute_single(&self, op: &FileOperation) -> Result<()> {
        transfer_file(&op.source, &op.destination, self.mode)?;
        for (src_side, dst_side) in &op.sidecars {
            let _ = transfer_file(src_side, dst_side, self.mode);
        }
        Ok(())
    }
}

pub struct RawPhotoInput {
    pub source_path: PathBuf,
    pub subject: String,
    pub year: u32,
    pub month: u32,
}

fn transfer_file(src: &Path, dst: &Path, mode: TransferMode) -> Result<()> {
    if !src.exists() {
        return Err(anyhow!("Source file does not exist: {:?}", src));
    }
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }

    match mode {
        TransferMode::Copy => { fs::copy(src, dst)?; },
        TransferMode::Move => {
            if fs::rename(src, dst).is_err() {
                fs::copy(src, dst)?;
                if fs::metadata(src)?.len() == fs::metadata(dst)?.len() {
                    fs::remove_file(src)?;
                } else {
                    let _ = fs::remove_file(dst);
                    return Err(anyhow!("Size mismatch during cross-device move"));
                }
            }
        }
    }
    Ok(())
}

fn resolve_destination(target_dir: &Path, stem: &str, ext: &str, reserved: &HashSet<PathBuf>) -> (PathBuf, String) {
    let mut counter = 0;
    loop {
        let cur_stem = if counter == 0 { stem.to_string() } else { format!("{}_{}", stem, counter) };
        let filename = if ext.is_empty() { cur_stem.clone() } else { format!("{}.{}", cur_stem, ext) };
        let candidate = target_dir.join(&filename);

        if !candidate.exists() && !reserved.contains(&candidate) {
            return (candidate, cur_stem);
        }
        counter += 1;
    }
}
