# AGENTS.md

Single-binary Rust desktop app (eframe/egui 0.30 + Candle CLIP on CPU) that scans a folder of
photos, classifies them, and files approved ones into `<output>/<Category>/<YYYY>/<MM>/`.
No server, no API keys, no network at runtime.

## Workflow

Every change lands through a **git worktree** — never edit in the main checkout, so `main` stays clean and
buildable while a branch is in flight. Worktrees are siblings of the repo, not children of it:

    /home/tobye/code/photo-organizer/                     # main checkout, stays on main
    /home/tobye/code/photo-organizer_worktrees/feat/foo/   # branch feat/foo

Branch from an up-to-date `origin/main`, run every `cargo` command from the worktree root:

```bash
git fetch origin
git worktree add ../photo-organizer_worktrees/feat/foo -b feat/foo origin/main
cd ../photo-organizer_worktrees/feat/foo
```

A cold debug worktree lands near 4 GB (~600 MB LFS weights in `models/`, ~600 MB for the copy
`build.rs` publishes into `target/<profile>/models`, ~3 GB compiled deps). Hardlink the weights to one
canonical copy (invisible to `git status`, unlike a symlink):

```bash
ln -f "$(git rev-parse --git-common-dir)/../models/clip_vision.safetensors" models/clip_vision.safetensors
```

`build.rs` then hardlinks the `target/` copy. Never point `target/<profile>/models` at that path yourself —
`fs::copy` truncates *through* hardlinks; that's what `publish_model` prevents. Don't share
`CARGO_TARGET_DIR` across worktrees: they invalidate each other's artifacts on every switch.

Reuse a worktree for follow-up commits on the same branch; drop it once the PR merges:

```bash
git worktree remove ../photo-organizer_worktrees/feat/foo
git branch -d feat/foo
```

### PR size and stacking

Aim for **~1500 changed lines per PR** against its own base (insertions + deletions, tests and docs
included, `Cargo.lock` excluded): `git diff --shortstat <base-branch>...HEAD`. Going over is a prompt to
look for a seam, not an automatic split. Stack only when there's a natural fault line and each slice is
independently reviewable/mergeable (usually a refactor the rest depends on). Don't stack to hit the number.

## Commands

CI runs these in order; match locally:

```bash
cargo fmt --check
cargo clippy -- -D warnings     # warnings are errors; tree is clean
cargo build
cargo test                      # 275 tests, ~5s once built
```

- One test/area: `cargo test media::tests::test_scan_preview_jpeg_scales_down_and_keeps_original_size`,
  `cargo test app::layout_tests`, `cargo test scanner`, `cargo test transfer`, `cargo test app::models::tests`,
  `cargo test app::view`, `cargo test app::calendar`.
- `cargo test -- --nocapture inference::tests::test_init_clip_session_real_file` watches the real CLIP
  load + forward pass. Silently returns if the model file is under 1024 bytes (an LFS pointer).
- Linux build deps: `libgtk-3-dev libxkbcommon-dev`. `cargo run` needs a display and must run from the
  worktree root so the relative `models/` lookup hits.

## Model + build.rs

- `models/clip_vision.safetensors` (~600 MB, OpenCLIP ViT-B/32) is **Git LFS**. `git lfs pull` before
  building; CI checks out with `lfs: true`. Missing model is supported: app runs rules-only, can't train.
- `inference.rs` hardcodes ViT-B/32 geometry (`EMBED_DIM 768`, 12 heads/layers, patch 32, 50 positional
  embeddings, optional 768→512 `proj`). Embeddings are always 512-d. A different checkpoint means editing
  those constants, not just swapping the file.
- `build.rs` publishes `models/` (and `src/models/`) into `target/<profile>/models`, hardlinking after
  the first build. `find_model_path()` tries CWD-relative paths first, then `<exe dir>/models`.

## State written at runtime

`profiles.json`, `settings.json`, `photo_cache.db`, `last_execution_manifest.json` are opened by **relative
path** (land in process CWD, not next to the binary). All four gitignored, safe to delete. SQLite cache
keyed by BLAKE3 of file contents; schema self-migrates via `CREATE TABLE IF NOT EXISTS` + best-effort
`ALTER TABLE`.

`settings.json` is deliberately *not* folded into `profiles.json`: profiles are learned data, settings are
knobs. `Settings::clamp_threshold` runs on load and save (serde will read `99.0`, classifying everything
as Unsorted).

`Cargo.lock` is committed even though `.gitignore` lists it. Don't commit a regenerated lock unless deps
changed.

## Execution flow

`src/main.rs` → `PhotoOrganizerApp::update`: drain high-res channel, drain scan messages, then render
toolbar → footer → grid → modals. Actions discovered during drawing collect into a `ModalActions`/
`SettingsActions` struct or local flag and apply *after* the frame (mid-draw would re-enter `open_modal`).
`rfd` pickers block for the same reason: open once the row/modal card finished laying out, never from a
button handler. Panel order is load-bearing — `render_footer` must show *before* `render_grid`.

All three modals share `src/app/chrome.rs` (`show_modal_card`): dimming backdrop, centred card,
backdrop-click-to-close. Two traps: card is sized on the ui *around* the window `Frame`, not inside it
(overflow by the frame's margin — `the_profile_modal_fits_its_card` guards); and
`ui.with_layout(Layout::right_to_left)` does **not** advance the parent ui's cursor, so a widget added
after one lands past it (settings modal puts Browse buttons on section headers for this reason).

- **Scan** (`src/scanner.rs`): one spawned thread walks the folder (non-recursive, files only) running a
  *dedicated* rayon pool sized `scan_thread_count()` = `(cores / 2).clamp(2, 12)`, overridable via
  `PHOTO_ORGANIZER_SCAN_THREADS`. Streams `ScanMessage::Item` (thumbnail, `Classification::Pending`) →
  `Update` (final call) → `Complete`. Every message tagged with the scan id; `drain_scan_messages` drops
  any but the current scan's.
- **Decode** (`src/media.rs`): scan never full-decodes. JPEG via `jpeg-decoder` DCT scaling; others
  full-decode then rescale. `ScanPreview::original_*` carries true frame size (heuristics key on it).
- **Classify** (`src/profile_store.rs`): `ProfileStore::classify(&PhotoFacts, threshold)` is the *only*
  entry point: CLIP centroid match above threshold → rules (screenshot/document/EXIF) → `Unsorted`.
  Threshold is a **parameter**, not a store field. Reaches scanner via `ScanConfig.threshold`; it's the
  only path into the rules tier.
- **Settings** (`src/settings.rs`, `src/app/settings_modal.rs`): threshold slider, output folder, model
  path; edits written on change. Slider re-classifies when it *comes to rest* at a value other than
  `classified_threshold` (via `reclassify_all`) — not on `changed()` or `drag_stopped()`. Same "rest, not
  movement" rule governs `settings.json` via `SettingsActions::worth_persisting`: a drag in flight
  (`threshold_settling`) writes nothing, commit frame writes. Output-folder and model rows are discrete
  edits and still hit disk immediately. Live threshold is written back to `self.settings` during a drag
  (else the handle jumps back); only the file write waits. A threshold moved mid-scan sets
  `pending_reclassify`, spent by `ScanMessage::Complete`. Switching model deletes `photo_cache.db` but
  keeps `profiles.json`. Chosen model path is preferred, not required — warn when what resolves isn't
  what was chosen.
- **Transfer** (`src/transfer/`): `plan_batch` → `execute_batch` → `TransferJournal` → `undo`. `_1`
  collision suffixes, `.xmp`/`.aae` sidecars travel with the photo. One module owns the on-disk layout:
  both directions go through one `place()` rule, journal records mode + per-file length/mtime
  `Fingerprint`, undo refuses to overwrite an occupied path or touch a changed file. Manifest records the
  whole batch; Undo reverses only the last batch. `execute_transfer` holds back `Pending` photos.
  `place` returns `Placed.warning` for cross-device Move whose original couldn't be unlinked (stays in
  `completed_ops`, warning becomes its own `failed_ops` entry); a batch landing nothing isn't journalled.
  `discover_sidecars` claims sidecars batch-wide. The batch is scoped to `visible_indices`, so a photo the
  grid's filters exclude is not transferred even if a tick on it outlived the filter.
- **Bulk move** (`src/app/bulk_move_modal.rs`): fourth route to a category, trains nothing; typed name
  becomes a `Manual` pick (survives re-classification). `assign_selected_to_category` labels items, then
  `execute_transfer_to` runs the same plan→execute→journal→report sequence with the name as
  `override_subject`. Toolbar button gated on the *selection*, not the destination. Remembered names live
  in `Settings::custom_categories` (not `profiles.json`); `remember_custom_category` is the only writer and
  keeps `Unsorted` out. Card closes only on a batch that landed something. Scoped to `visible_indices`
  like every other route that consumes the selection.
- **Grid view and order** (`src/app/view.rs`): the one answer to "is this photo in view" — grid, footer
  count, **All**/**None**, both training routes, the transfer batch and modal paging all read
  `visible_indices`. Runs over `Row`, a borrowing view of the fields that decide visibility, so ordering
  stays assertable without a live `Context`. Recomputed per frame (items change underneath). Two
  orderings aren't the plain comparison: `rank_confidence` scales a rule-assigned photo into the band
  *below* the threshold it failed; `category_sort_key` files the unsorted bucket last (under `ZZZZ`).
  Descending inverts the key (`Reverse`), never the run, so `sort_by_cached_key`'s tie-break survives.
  See `docs/classification_pipeline.md` → **Ordering and narrowing the grid**.
- **Grid must not reorder while a cell's category editor holds focus** (`holding_order`). Editing a
  category changes the Category sort key, and egui identifies a cell's widgets by position, so a
  reordering frame makes the focused field the *next* photo's. `render_custom_category_input` returns
  whether it has focus and `render_grid` records it at frame end; `render_grid_cell_controls` returns
  `(train_request, editing)`. Check focus via `ui.memory(|m| m.has_focus(response.id))`, not
  `Response::has_focus()` (the latter also requires OS window focus). The hold releases on sort change
  (`order_sort`), not only on focus leaving (clicking doesn't move keyboard focus in egui 0.30). The hold
  keeps **the order the grid last drew** (`drawn_order`), not the absence of one — `view::held_order`
  reapplies it to the current admitted set. Tests: `custom_sorted_grid` stages photos in Category-sort
  order, so `items` order equals sorted order — stage against the sort or the regression passes.
- **Sort and filters are one row** (`render_sort_and_filter_row`), not a sort row plus a filter panel
  behind a toggle. `horizontal_wrapped` so a narrow window folds it rather than running the category
  picker off the end. `Clear` appears only when something is narrowed.
- **Calendar** (`src/app/calendar.rs`): hand-rolled month-grid date picker (egui 0.30 has none usable).
  Whole weeks padding (`month_grid`) so height stays stable across months. `first_weekday` via chrono with
  `+1` (not `+6`). `range_after_click` state machine: one click = one day (sets both bounds); second
  widens, widening backwards swaps the bounds; third starts over. States told apart by whether bounds are
  the same day; bounds compare through `start()`/`end()`. Layout: weekday initials and days are rows of
  **one** grid; `min_col_width(DAY_SIZE)` (else 40pt default); horizontal spacing 0, vertical `ROW_SPACING`
  (abutting cells read as one bar, gap between weeks stops wrap-around looking continuous). Selected range
  is one continuous bar per week; `day_fill` decides `Starts`/`Middle`/`Ends`/`Only` by whether the day
  either side is selected **and in the same grid row** (column `(leading + day - 1) % 7`); `first_weekday`
  converts chrono with `+1`. Popup, not toolbar row (inline pushed the grid down 505pt —
  `opening_the_calendar_does_not_move_the_grid`); open/closed is egui popup memory, only the month cursor
  is `CalendarControl`'s; `CloseOnClickOutside`, not `CloseOnClick`. Heading is three explicit slots
  (`ARROW_SLOT`, title, `ARROW_SLOT`) in a band the width of the grid.

### Classification types

`src/classification.rs` holds two records:

- **`PhotoFacts`**: `path`, `date`, `frame: Option<FrameSize>`, `is_exif`, `embedding`. True frame size
  lives here. `frame` is `Option` because old `photo_cache` rows lack the columns; scanner backfills from
  the file header on next rescan, resolution rule declines to fire meanwhile.
- **`Classification`**: `Pending`, or `Decision { category: CategoryName, confidence, source }`. `Decision`
  carries no `is_custom`; that's asked once in `StagedItem::apply_classification` against the store in
  hand.

`Pending` is load-bearing in three places (nothing else gets there first):
- `apply_classification` never preserves one, so the decision always lands.
- Re-classify All and both training routes are reachable mid-scan, so `reclassify_all` steps over pending
  photos.
- `StagedItem::is_filable` is false for one, and `execute_transfer` filters on it.

The one write clearing `pending` is `StagedItem::mark_manual_over_pending`, used solely by bulk move.
`mark_manual` stays non-claiming. Both mark methods zero `confidence`. Callers write the category *before*
calling either; `apply_classification` stays reserved for pipeline decisions.

Copy clears the grid exactly as Move does (a copy is a completed filing).

- **Dates** (`PhotoDate`): `day` is `Option<u32>`, `photo_cache.day` nullable. A row written before the
  column has none; `Option` says "unknown" without inventing a day. `StagedItem` carries a whole
  `PhotoDate` so year/month/day can't drift. Range filtering is interval overlap over `PhotoDate::span()`,
  not tuple comparison — that's what lets "March 2021" keep a photo taken on the 14th, and a month-wide
  range keep a photo with no recorded day. Don't "simplify" to `Ord`. The missing day is repaired by the
  scanner's cached branch re-reading the file (beside the frame-size repair), because the reader reporting
  `None` is not a repair — it hands `PhotoFacts` a dateless photo and `insert_cache` writes that `None`
  back out. `is_exif_date` is rewritten alongside, since `extract_date` returns the pair together. If a day
  is ever added to `RawPhotoInput`, note the output layout `<Category>/<YYYY>/<MM>/` is month-granular by
  contract.

## Testing quirks

- `src/app/layout_tests.rs` uses `egui_kittest` for real geometry from production renderers. Grid cell
  stacks the custom-category input *below* the combo; modal puts it *inline* — both asserted, don't unify.
  Modal tests filter `placed_widgets` to inside the card rect. Bulk move harness stages photos on a
  `Cell<bool>`, not `items.is_empty()`.
- Settings modal driven by real pointer/key events: egui only hands `interact_pointer_pos` while a button
  is held or released *that* frame, so press+release in one frame cancel (use `press_at`/`drag_to`/
  `release_at`, one event per frame); clicking doesn't focus in egui 0.30, so keyboard tests need
  kittest's `Node::focus()`. Those tests commit a threshold change, rewriting `settings.json` in crate
  root (gitignored). Write *policy* is unit-tested in `settings_modal.rs`, not via the file.
- `categories.rs` source-text test (`include_str!`) requires exactly two call sites of
  `render_custom_category_input` (grid + modal).
- Closed egui `ComboBox` puts no label in the accesskit tree; find by role, stringify `accesskit::Role`
  the way `placed_widgets` does. Open it to assert contents.
- `scanner.rs` mutates process-global `PHOTO_ORGANIZER_SCAN_THREADS` in exactly one test. Don't split or
  add another env-mutating test.
- Tests create temp files as `temp_dir()/name_<pid>.<ext>`; `src/transfer/` builds a whole temp tree per
  test (`temp_dir()/transfer_<name>_<pid>`). No fixtures dir.
- Anything running a real transfer must point `app.journal_path` at a temp dir (`LAST_JOURNAL` is a
  relative path; default overwrites the crate-root journal Undo reads).
- `transfer::tests::test_undo_is_the_inverse_of_execute_for_both_modes`: `undo(execute(inputs, mode))`
  restores fs for both modes incl. sidecars. Keep passing; add to it rather than hand-built manifests.
- Cross-device Move fallback unreachable on one fs; `transfer::tests::set_dir_read_only` provokes its
  second half (read-only dir refuses `rename`/`unlink`). It asserts the read-only bit was honoured.
- `src/app/` split out of `app.rs` (commit 161b3e7); `mod.rs` documents the split. Keep module boundary
  and top-of-file `//!` orientation comments.
- `app_with`'s `tune` runs **every frame** — right for a standing condition, wrong for anything a test
  then tries to undo (`app_with_once` exists for that).
- `view.rs` is the single answer to "is this photo in view" — grid, footer, **All**/**None**, both
  training routes, the transfer batch and modal navigation all read `visible_indices`. Adding a second
  copy of the is-it-in-view predicate anywhere is the bug it exists to prevent.
- `holding_order` details above; the hold holds **the order the grid last drew**. `custom_sorted_grid`
  stages photos in Category-sort order, so `items` order equals sorted order — stage against the sort or
  the regression passes.

## Category names

A category is a dropdown label *and* a directory. `src/category_name.rs` owns the second: `CategoryName`
is one path component by construction; `from_user_input` is the only way to build one from typed input.
Separators and Windows-reserved chars become `-`, trailing dots/spaces trimmed, and a name that is empty,
`.`, `..`, or a reserved device name (`NUL`, `COM1`, `LPT1`, …) falls back to `Unsorted`.

`CategoryProfile::new` and `ProfileStore::load_from_file` are the two points where a stored profile name
becomes canonical. Profiles looked up *by* name, so sanitise only there.

`StagedItem.category` stays a plain `String` (sanitised at the transfer seam, `app/transfer.rs`).
`Decision.category` and `RawPhotoInput.subject` are `CategoryName`. Don't join a category into a path any
other way; `plan_batch` is the one place that does.

## Conventions

- Branch with a prefix (`feat/`, `fix/`, `refactor/`, `perf/`, `chore/`) in a worktree (see **Workflow**)
  and open a PR; `main` history is one squash commit per PR ending in `(#N)`, with a one-line message.
- Commit messages stay succinct: short headline, bullets fine, no essays. The diff and PR carry the "why".
  Less is more.
- Prefer simple solutions: smallest change that works; avoid new abstractions, indirection, or cleverness
  unless something concrete needs them. Less is more.
- When scanning, classification, or the transfer flow changes, update `README.md` **and**
  `docs/classification_pipeline.md` in the same change.
- Comments explain *why* and cite measurements. Match that density; clippy with `-D warnings` must stay
  clean.
- Tagging `v*` runs `.github/workflows/release.yaml`: Linux + Windows x86_64, binary packaged with
  `models/`. Update it if outputs or asset names change.
