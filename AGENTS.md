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
cargo test                      # 162 tests, ~5s once built
```

- One test / one area: `cargo test media::tests::test_scan_preview_jpeg_scales_down_and_keeps_original_size`,
  `cargo test app::layout_tests`, `cargo test scanner`, `cargo test transfer`, `cargo test app::models::tests`.
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

`profiles.json`, `settings.json`, `photo_cache.db`, `last_execution_manifest.json` are opened by **relative
path**, so they land in the process CWD (repo root under `cargo run`), not next to the binary as the README
implies. All four are gitignored and safe to delete. The SQLite cache is keyed by BLAKE3 of file contents;
schema self-migrates with `CREATE TABLE IF NOT EXISTS` + best-effort `ALTER TABLE`.

`settings.json` is deliberately *not* folded into `profiles.json`: profiles are learned data, settings are
knobs, and keeping them apart is what stops an older `profiles.json` carrying a stale copy of a value the
UI owns. `Settings::clamp_threshold` runs on both load and save — serde will happily read `99.0`, which would
classify every photo as Unsorted.

`Cargo.lock` is committed even though `.gitignore` lists it — the rule is inert for tracked files. Don't
commit a regenerated lock unless dependencies actually changed.

## Execution flow

`src/main.rs` → `PhotoOrganizerApp::update`: drain the high-res channel, drain scan messages, then
render toolbar → footer → grid → modals. Actions discovered during drawing are collected into a
`ModalActions`/`SettingsActions` struct or a local flag and applied *after* the frame, because acting
mid-draw would re-enter `open_modal`. `rfd`'s pickers block for the same reason, so they are opened once the
row (or the modal card) has finished laying out, never from inside the button handler. Panel order is
load-bearing — egui hands `CentralPanel` whatever the top and bottom panels leave, so `render_footer` has to
be shown *before* `render_grid`.

All three modals share `src/app/chrome.rs` (`show_modal_card`): dimming backdrop, centred card,
backdrop-click-to-close. Two traps live in there — the card is sized on the ui *around* the window
`Frame`, not inside it (asking for the full card from within the frame overflows by the frame's margin,
which is what `the_profile_modal_fits_its_card` guards), and `ui.with_layout(Layout::right_to_left)` does
**not** advance the parent ui's cursor, so a widget added after one lands past it. The settings modal
puts its Browse buttons on section headers for that reason.

- **Scan** (`src/scanner.rs`): one spawned thread walks the folder (non-recursive, files only) and runs a
  *dedicated* rayon pool, not the global one — sized `scan_thread_count()` = `(cores / 2).clamp(2, 12)`,
  overridable with `PHOTO_ORGANIZER_SCAN_THREADS`. Bandwidth-bound CLIP, not core-bound. It streams
  `ScanMessage::Item` (thumbnail now, `Classification::Pending`) → `ScanMessage::Update` (final call) →
  `Complete`, so the UI is usable while classification runs. Every message is tagged with the id of the
  scan that sent it and `drain_scan_messages` drops any but the current scan's: starting a scan clears
  `items` but cannot un-send what a superseded scan already put on the shared channel, and that scan's
  `Update` was decided against the profile store as it was at the time.
- **Decode** (`src/media.rs`): a scan never full-decodes. JPEG goes through `jpeg-decoder`'s DCT scaling;
  every other format full-decodes then rescales. `ScanPreview::original_*` carries the true frame size
  because the heuristics key off resolution.
- **Classify** (`src/profile_store.rs`): `ProfileStore::classify(&PhotoFacts, threshold) -> Classification` is
  the *only* entry point. CLIP centroid match above the user's confidence threshold (default
  `settings::DEFAULT_CONFIDENCE_THRESHOLD` = 0.65, slider range 0.30..=0.95) → rules
  (screenshot/document/EXIF) → `Unsorted`. The threshold is a **parameter**, not a store field: profiles
  are learned data, the bar is a setting, and keeping them apart means a `profiles.json` never carries a
  stale copy of a knob the UI owns. It reaches the scanner through `ScanConfig.threshold`. That threshold
  is the only path into the rules tier, so dropping it would make screenshot/document/EXIF detection
  unreachable for anyone with a trained profile.
- **Settings** (`src/settings.rs`, `src/app/settings_modal.rs`): threshold slider, output folder, model
  path. Edits are written on change rather than on a Save button. The slider re-classifies when it comes to
  rest at a value other than `classified_threshold` (kept in step by `reclassify_all`, the only place the
  grid's classifications are rewritten) — *not* on `changed()` and *not* on `Response::drag_stopped()`.
  Neither of those works: egui puts a slider where the pointer is on the press frame and on every frame the
  handle travels, so the release frame carries no change at all and a per-frame test misses the decision;
  and the arrow keys never start a drag. Per-frame firing would re-run `reclassify_all` ~60×/sec. The same
  "rest, not movement" rule governs `settings.json`, via `SettingsActions::worth_persisting`: a drag in
  flight (`threshold_settling`) writes nothing, the commit frame writes. The output-folder and model rows
  are discrete edits and still go to disk as they happen — a deferred write there would lose a path. Note
  the live threshold *is* written back to `self.settings` during a drag, or the handle would jump back on
  every frame; only the file write waits.
  A threshold moved mid-scan sets `pending_reclassify` instead, which `ScanMessage::Complete` spends: the scan
  classifies against the threshold it started with, so re-running it there would only fix half the grid.
  Switching model deletes `photo_cache.db` (embeddings from one checkpoint are meaningless in another's
  space) but keeps `profiles.json`: the centroids are stale too, but they are the user's work. The chosen
  path is preferred but not required — the row warns when what resolves isn't what was chosen, because a
  green dot next to a silently substituted checkpoint reads as confirmation.
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

### Classification types

`src/classification.rs` holds the two records the whole pipeline speaks in:

- **`PhotoFacts`** — what a photo *is*: `path`, `date`, `frame: Option<FrameSize>`, `is_exif`, `embedding`.
  The true frame size lives here, which is what stopped three call sites reconstructing it (two of them
  from the ≤200x140 thumbnail). `frame` is `Option` because a `photo_cache` row written before the
  `original_width`/`original_height` columns existed has none; the scanner backfills one from the file
  header on the next rescan, and the resolution rule declines to fire meanwhile.
- **`Classification`** — `Pending`, or a `Decision { category: CategoryName, confidence, source,
  is_custom }`. `is_custom` is derived once in `ProfileStore::decide`, never re-asked. `Pending` replaced
  the `"Classifying..."` string that was stored as a real category: it read as a custom name, so the grid
  rendered a `TextEdit` bound to it and one keystroke set `source = Manual`, after which the real
  `Update` was skipped and the photo stayed filed as "Classifying..." forever.

`ProfileStore::classify` / `classify_with_heuristics` / `classify_heuristics` were collapsed into the one
entry point — which is also where `classify_with_heuristics`'s `threshold` parameter went, so the
`CONFIDENCE_THRESHOLD` constant is gone rather than reintroduced here. `ClassificationSource` is
re-exported from `profile_store` so existing `use` paths and the grid's badge keep working.

## Testing quirks

- `src/app/layout_tests.rs` uses `egui_kittest` to assert **real** geometry from the production
  renderers. The grid cell deliberately stacks the custom-category input *below* the combo while the modal
  puts it *inline* — both directions are asserted. Don't "unify" those layouts. Modal tests filter
  `placed_widgets` down to what lies inside the card rect, because the backdrop covers the whole screen.
- The same file drives the settings modal with **real** pointer and key events, which has two traps: egui
  only hands a widget an `interact_pointer_pos` while a button is held or was released *that* frame, so a
  press and a release queued into one frame cancel out (hence `press_at`/`drag_to`/`release_at`, one event
  per frame); and clicking a widget does *not* focus it in egui 0.30, so keyboard tests need kittest's
  `Node::focus()`, which sends the accesskit Focus action. Those tests commit a threshold change, so they
  rewrite `settings.json` in the crate root — gitignored, and safe to delete, but expect your threshold to
  have moved after a test run. The write *policy* is unit-tested in `settings_modal.rs` rather than through
  the file: a test reading `settings.json` would race the other tests' writes.
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