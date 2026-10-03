use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=models");
    println!("cargo:rerun-if-changed=src/models");

    let manifest_dir = match std::env::var("CARGO_MANIFEST_DIR") {
        Ok(dir) => PathBuf::from(dir),
        Err(_) => return,
    };

    let out_dir = match std::env::var("OUT_DIR") {
        Ok(dir) => PathBuf::from(dir),
        Err(_) => return,
    };

    // OUT_DIR is typically `target/<profile>/build/<crate>/out`
    // Going 3 ancestors up gives `target/<profile>/` where the executable binary is emitted.
    let target_dir = match out_dir.ancestors().nth(3) {
        Some(dir) => dir.to_path_buf(),
        None => return,
    };

    let target_models_dir = target_dir.join("models");

    // Publish any models from project root `models/` or `src/models/`
    let source_dirs = [
        manifest_dir.join("models"),
        manifest_dir.join("src").join("models"),
    ];

    for source_dir in &source_dirs {
        if source_dir.exists() && source_dir.is_dir() {
            if let Ok(entries) = fs::read_dir(source_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_file() {
                        let file_name = match path.file_name() {
                            Some(name) => name,
                            None => continue,
                        };
                        let dest_path = target_models_dir.join(file_name);

                        let _ = fs::create_dir_all(&target_models_dir);
                        publish_model(&path, &dest_path);
                    }
                }
            }
        }
    }
}

// Publishes one model file into `target/<profile>/models`, hardlinking instead of copying.
//
// The models dir is ~600 MB, and every profile and every worktree pays for it again. A hardlink makes the
// published file a second name for the same inode: no space, and no 600 MB memcpy per build.
//
// `fs::copy` must never be allowed to write to a path that resolves back to the source. It opens the
// destination with O_TRUNC, which follows hardlinks and symlinks, so the copy empties the *source*:
// measured on 1.98, copying a file onto a hardlink of itself returns Ok(0) and leaves 0 bytes behind.
// Hence the unlink first, and the same-inode short-circuit that avoids the write altogether.
fn publish_model(src: &Path, dest: &Path) {
    if already_linked(src, dest) {
        return;
    }

    // Unlink drops the old name without touching the shared inode.
    let _ = fs::remove_file(dest);

    // Hardlinks fail across devices, and on filesystems that lack them (FAT, some network mounts).
    if fs::hard_link(src, dest).is_err() {
        let _ = fs::copy(src, dest);
    }
}

// Unix-only: `Metadata` exposes no stable inode on Windows, so there it falls through and relinks on
// every build. That is still much cheaper than the copy it replaces.
#[cfg(unix)]
fn already_linked(src: &Path, dest: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;

    match (fs::metadata(src), fs::metadata(dest)) {
        (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
        _ => false,
    }
}

#[cfg(not(unix))]
fn already_linked(_src: &Path, _dest: &Path) -> bool {
    false
}
