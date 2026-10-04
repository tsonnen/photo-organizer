# AGENTS.md

Single-binary Rust desktop app (eframe/egui 0.30 + Candle CLIP on CPU) that scans a folder of
photos, classifies them, and files approved ones into `<output>/<Category>/<YYYY>/<MM>/`.
No server, no API keys, no network at runtime.

## Workflow

Every change lands through a **git worktree** — never edit in the main checkout, so `main` stays clean and
buildable while a branch is in flight. Worktrees are siblings of the repo, not children of it, so a stale
worktree can't turn up in `git status`:

    /home/tobye/code/photo-organizer/                     # main checkout, stays on main
    /home/tobye/code/photo-organizer_worktrees/feat/foo/   # branch feat/foo

The directory is the branch name verbatim, prefix included, so `git worktree list` output pastes straight into
`cd`. Branch from an up-to-date `origin/main`, and run every `cargo` command from the worktree root:

```bash
git fetch origin
git worktree add ../photo-organizer_worktrees/feat/foo -b feat/foo origin/main
cd ../photo-organizer_worktrees/feat/foo
```

A cold debug worktree lands near 4 GB: ~600 MB of LFS weights in `models/`, another ~600 MB for the copy
`build.rs` publishes into `target/<profile>/models`, and ~3 GB of compiled deps (`deps` alone is 3.1 GB —
eframe/wgpu/candle). LFS smudges the weights out of the shared `.git/lfs` object, so it's a copy rather than
a re-download, but it's still a copy per worktree. Hardlink it to one canonical copy:

```bash
ln -f "$(git rev-parse --git-common-dir)/../models/clip_vision.safetensors" models/clip_vision.safetensors
```

`ln`, not `ln -s`: a symlink reads as a typechange in `git status`, while a hardlink to identical bytes is
invisible. `build.rs` then hardlinks the `target/` copy, which brings a worktree to ~3 GB. Never point
`target/<profile>/models` at that path yourself — `fs::copy` truncates *through* hardlinks, which is what
`publish_model` in `build.rs` exists to prevent.

A shared `CARGO_TARGET_DIR` would delete the remaining ~3 GB, but two worktrees on different branches
invalidate each other's artifacts on every switch — exactly the workflow this convention creates. Don't.

Reuse a worktree for follow-up commits on the same branch rather than opening a new one, and drop it once
the PR merges:

```bash
git worktree remove ../photo-organizer_worktrees/feat/foo
git branch -d feat/foo
```

### PR size and stacking

Aim for **~1500 changed lines per PR**, measured against that PR's own base branch — insertions and
deletions, tests and docs included, `Cargo.lock` excluded:

```bash
git diff --shortstat <base-branch>...HEAD
```

Going over is a prompt to look for a seam, not an automatic split. Stack when the work has a natural fault
line and each slice is independently reviewable and mergeable on its own — usually a refactor that the rest
depends on:

- PR1 `refactor/extract-scan-service` → `main`: the scanner's pool and message plumbing behind one interface.
- PR2 `feat/scan-progress` → `refactor/extract-scan-service`: the feature that needed it.

In a stack, each PR targets the branch below it, the PR bodies list the stack bottom-up, and the PRs land on
`main` one squash commit at a time. Don't stack to hit the number: one cohesive 2000-line change beats two
arbitrary halves, while a 1200-line PR that mixes a shared-service refactor with every new caller of it is
still two PRs. Split when the change spans unrelated concerns, when a reviewer would need two mental models,
or when the first slice is safe to merge on its own.

## Commands

CI (`.github/workflows/build-and-test.yaml`) runs these in order, so match it locally:

```bash
cargo fmt --check
cargo clippy -- -D warnings     # warnings are errors; the tree is currently clean
cargo build
cargo test                      # 106 tests, ~7s once built
```

- One test / one area: `cargo test media::tests::test_scan_preview_jpeg_scales_down_and_keeps_original_size`,
  `cargo test app::layout_tests`, `cargo test scanner`, `cargo test transfer`.
- `cargo test -- --nocapture inference::tests::test_init_clip_session_real_file` to watch the real
  CLIP load + forward pass. It **silently returns** if the model file is under 1024 bytes (an LFS pointer).
- Linux build deps: `libgtk-3-dev libxkbcommon-dev` (the release workflow installs these).
- `cargo run` needs a display and must be run from the worktree root so the relative `models/` lookup hits.

## Model + build.rs

- `models/clip_vision.safetensors` (~600 MB, OpenCLIP ViT-B/32) is **Git LFS**. `git lfs pull` before
  building; CI checks out with `lfs: true`. Missing model is a supported state: the app runs rules-only
  and simply can't train categories.
- `inference.rs` hardcodes ViT-B/32 geometry (`EMBED_DIM 768`, 12 heads/layers, patch 32, 50 positional
  embeddings, optional 768→512 `proj`). Embeddings are always 512-d. A different checkpoint size means
  editing those constants, not just swapping the file.
- `build.rs` publishes `models/` (and `src/models/`) into `target/<profile>/models`, hardlinking after the
  first build instead of re-copying ~600 MB. `find_model_path()` tries CWD-relative paths first, then
  `<exe dir>/models`.

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
- **Transfer** (`src/transfer/`): `plan_batch` → `execute_batch` → `TransferJournal` →
  `undo`, `_1` collision suffixes instead of overwriting, `.xmp`/`.aae` sidecars travelling
  with the photo. One module owns the on-disk layout on purpose: with the reverse
  direction in `undo_engine.rs` it became a second implementation of the same layout, and
  the journal recorded no `TransferMode`, so Undo assumed every batch was a move and
  deleting a batch of twenty *copies* deleted twenty copies. Both directions now go
  through one `place()` rule, the journal records the mode plus a length/mtime
  `Fingerprint` per file, and undo refuses to overwrite an occupied path or touch a file
  that changed since it was filed. The manifest records the whole batch and Undo reverses
  **only the last batch**.

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
- Tests create temp files as `temp_dir()/name_<pid>.<ext>`, and `src/transfer/` builds a whole
  temp tree per test (`temp_dir()/transfer_<name>_<pid>`). No fixtures directory exists.
- `transfer::tests::test_undo_is_the_inverse_of_execute_for_both_modes` asserts that
  `undo(execute(inputs, mode))` puts the filesystem back exactly as it started, for both
  modes, sidecars included. It is the reason the two directions share a module — keep it
  passing, and add to it rather than to a hand-built manifest when you touch either side.
- `src/app/` was split out of `app.rs` (commit 161b3e7); `mod.rs` documents the submodule split. Keep the
  module boundary and the top-of-file `//!` orientation comments that go with it.

## Category names

A category is a dropdown label *and* a directory under the output folder. `src/category_name.rs`
owns the second job: `CategoryName` is one path component by construction, and
`from_user_input` is the only way to build one from something a person typed. Separators
and Windows-reserved characters become `-`, trailing dots and spaces are trimmed, and
`.`/`..` fall back to `Unsorted`.

`StagedItem.category` stays a plain `String` — it is display text the user retypes per
photo, so it is sanitised at the transfer seam (`app/transfer.rs`) rather than on the way
in. `ClassificationResult.category` and `RawPhotoInput.subject` are `CategoryName`. Don't
join a category into a path any other way; `plan_batch` is the one place that does it.

## Conventions

- Branch with a prefix (`feat/`, `fix/`, `refactor/`, `perf/`, `chore/`) in a worktree (see **Workflow**)
  and open a PR; `main` history is one squash commit per PR ending in `(#N)`.
- When scanning, classification or the transfer flow changes, update `README.md` **and**
  `docs/classification_pipeline.md` in the same change.
- Comments explain *why* and cite measurements (e.g. why half the cores, why WAL). Match that density;
  clippy with `-D warnings` must stay clean.
- Tagging `v*` runs `.github/workflows/release.yaml`: Linux + Windows x86_64, binary packaged with
  `models/`. Update it if outputs or asset names change.