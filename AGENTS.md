# AGENTS.md

Single-binary Rust desktop app (eframe/egui 0.30 + Candle CLIP on CPU) that scans a folder of
photos, classifies them, and files approved ones into `<output>/<Category>/<YYYY>/<MM>/`.
No server, no API keys, no network at runtime.

## Commands

CI (`.github/workflows/build-and-test.yaml`) runs these in order, so match it locally:

```bash
cargo fmt --check
cargo clippy -- -D warnings     # warnings are errors; the tree is currently clean
cargo build
cargo test                      # 59 tests, ~8s once built
```

- One test / one area: `cargo test media::tests::test_scan_preview_jpeg_scales_down_and_keeps_original_size`,
  `cargo test app::layout_tests`, `cargo test scanner`.
- `cargo test -- --nocapture inference::tests::test_init_clip_session_real_file` to watch the real
  CLIP load + forward pass. It **silently returns** if the model file is under 1024 bytes (an LFS pointer).
- Linux build deps: `libgtk-3-dev libxkbcommon-dev` (the release workflow installs these).
- `cargo run` needs a display and must be run from the repo root so the relative `models/` lookup hits.

## Model + build.rs

- `models/clip_vision.safetensors` (~600 MB, OpenCLIP ViT-B/32) is **Git LFS**. `git lfs pull` before
  building; CI checks out with `lfs: true`. Missing model is a supported state: the app runs rules-only
  and simply can't train categories.
- `inference.rs` hardcodes ViT-B/32 geometry (`EMBED_DIM 768`, 12 heads/layers, patch 32, 50 positional
  embeddings, optional 768→512 `proj`). Embeddings are always 512-d. A different checkpoint size means
  editing those constants, not just swapping the file.
- `build.rs` copies `models/` (and `src/models/`) into `target/<profile>/models` on every build.
  `find_model_path()` tries CWD-relative paths first, then `<exe dir>/models`.

## State written at runtime

`profiles.json`, `photo_cache.db`, `last_execution_manifest.json` are opened by **relative path**, so they
land in the process CWD (repo root under `cargo run`), not next to the binary as the README implies. All
three are gitignored and safe to delete. The SQLite cache is keyed by BLAKE3 of file contents; schema
self-migrates with `CREATE TABLE IF NOT EXISTS` + best-effort `ALTER TABLE`.

`Cargo.lock` is committed even though `.gitignore` lists it — the rule is inert for tracked files. Don't
commit a regenerated lock unless dependencies actually changed.

## Execution flow

`src/main.rs` → `PhotoOrganizerApp::update`: drain the high-res channel, drain scan messages, then
render toolbar → grid → modal. Actions discovered during drawing are collected into a `ModalActions`/
local flags and applied *after* the frame, because acting mid-draw would re-enter `open_modal`.

- **Scan** (`src/scanner.rs`): one spawned thread walks the folder (non-recursive, files only) and runs a
  *dedicated* rayon pool, not the global one — sized `scan_thread_count()` = `(cores / 2).clamp(2, 12)`,
  overridable with `PHOTO_ORGANIZER_SCAN_THREADS`. Bandwidth-bound CLIP, not core-bound. It streams
  `ScanMessage::Item` (thumbnail now, category `"Classifying..."`) → `ScanMessage::Update` (final call) →
  `Complete`, so the UI is usable while classification runs.
- **Decode** (`src/media.rs`): a scan never full-decodes. JPEG goes through `jpeg-decoder`'s DCT scaling;
  every other format full-decodes then rescales. `ScanPreview::original_*` carries the true frame size
  because the heuristics key off resolution.
- **Classify** (`src/profile_store.rs`): CLIP centroid match above `CONFIDENCE_THRESHOLD` (0.65, a
  constant, not a setting) → rules (screenshot/document/EXIF) → `Unsorted`. That threshold is the only
  path into the rules tier, so dropping it would make screenshot/document/EXIF detection unreachable
  for anyone with a trained profile. `ClassificationSource::Manual` items are never overwritten by
  `reclassify_all`, the **Re-classify All** button, or a scan `Update`; only their embedding refreshes.
- **Transfer** (`src/execution_engine.rs`, `src/undo_engine.rs`): `plan_batch` → `execute_batch`, `_1`
  collision suffixes instead of overwriting, `.xmp`/`.aae` sidecars travel with the photo. The manifest
  records the whole batch and Undo reverses **only the last batch**.

### Known divergence

`reclassify_all` (`src/app/categories.rs`) feeds `item.texture.size()` — the ≤200x140 thumbnail — into
`classify_with_heuristics`, while the scanner passes the real dimensions. The ratio-based screenshot rule
needs width ≥ 800, so a photo can classify one way at scan time and another after "Re-classify All".
Carry the original dimensions on `StagedItem` if you touch this.

## Testing quirks

- `src/app/layout_tests.rs` uses `egui_kittest` to assert **real** geometry from the production
  renderers. The grid cell deliberately stacks the custom-category input *below* the combo while the modal
  puts it *inline* — both directions are asserted. Don't "unify" those layouts.
- `categories.rs` has a source-text test (`include_str!`) requiring exactly two call sites of
  `render_custom_category_input` (grid + modal). Adding a third call site fails the build.
- `scanner.rs` mutates the process-global `PHOTO_ORGANIZER_SCAN_THREADS` in exactly one test on purpose
  (cargo runs tests on parallel threads). Don't split it or add another env-mutating test.
- Tests create temp files as `temp_dir()/name_<pid>.<ext>`; no fixtures directory exists.
- `src/app/` was split out of `app.rs` (commit 161b3e7); `mod.rs` documents the submodule split. Keep the
  module boundary and the top-of-file `//!` orientation comments that go with it.

## Conventions

- Branch with a prefix (`feat/`, `fix/`, `refactor/`, `perf/`) and open a PR; `main` history is one squash
  commit per PR ending in `(#N)`.
- When scanning, classification or the transfer flow changes, update `README.md` **and**
  `docs/classification_pipeline.md` in the same change.
- Comments explain *why* and cite measurements (e.g. why half the cores, why WAL). Match that density;
  clippy with `-D warnings` must stay clean.
- Tagging `v*` runs `.github/workflows/release.yaml`: Linux + Windows x86_64, binary packaged with
  `models/`. Update it if outputs or asset names change.