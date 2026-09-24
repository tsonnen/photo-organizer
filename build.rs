use std::fs;
use std::path::PathBuf;

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

    // Copy any models from project root `models/` or `src/models/`
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
                        let _ = fs::copy(&path, &dest_path);
                    }
                }
            }
        }
    }
}
