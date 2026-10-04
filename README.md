# Photo Organizer

A small desktop app for sorting a folder of photos. You point it at a directory, it
scans everything in there, works out what each photo is, and then files the ones you
approve into `Category/Year/Month/` folders. Nothing leaves your machine.

It sorts visually rather than by folder structure, which is the whole point. A dump of
`IMG_4821.jpg` next to `receipt_march.png` and a couple of screenshots gets split into
something sensible, and you teach it your own categories as you go.

## What it does

- Scans a folder for JPEGs, PNGs, WebP, TIFF, GIF, BMP and camera RAW files
  (CR2, CR3, NEF, ARW, DNG, RAF, ORF, PEF).
- Embeds each photo with CLIP and sorts it by similarity to the categories you defined.
- Falls back to filename and EXIF rules when the model can't make a confident call, so
  screenshots and receipts still land somewhere sensible on their own.
- Lets you train a category from photos you pick, then re-sorts everything immediately.
- Moves or copies the approved photos into the output folder, and can undo the last run.
- Caches thumbnails and embeddings in SQLite, so a second scan of the same folder is
  almost instant.
- Remembers your confidence threshold, output folder and model location in a settings
  file, so they survive a restart.

It runs entirely on the CPU. There's no server, no API key, and no network calls at
runtime.

## Installing
[Latest release](https://github.com/tsonnen/photo-organizer/releases). For new installs, 
download the appropriate compressed file, this will include required models. For updating 
a release, just download the executable and replace it where ever the uncompressed file 
was placed. This will allow for maintaining training. 

## The CLIP model

Classification runs on `models/clip_vision.safetensors`. It's tracked with Git LFS, so
pull it down before building:

```bash
git lfs install
git lfs pull
```

That file is a full OpenCLIP ViT-B/32 checkpoint, about 600 MB. Only the `visual.*` half
of it is read, but the text tower comes along for the ride. Both Hugging Face and OpenCLIP
weight layouts are handled, so any `ViT-B/32` visual model will do.

If the file isn't there at all, the app still runs. It tells you the model is missing and
falls back to rules only, which handles screenshots, documents and camera photos fine but
can't learn your categories.

## Using it

Pick a **Source Folder** and the scan starts. Photos appear in the grid as thumbnails
while classification continues in the background, so you can start reviewing before it's
finished.

Every cell has a category dropdown showing your trained profiles with a confidence
percentage, plus a small 🎓 button to train that category from the one photo. Anything
that doesn't fit gets **Other**, which gives you a free-text field for a name of your
own.

Open **AI & Categories** in the toolbar to see the model status, re-sort everything, or
train a whole batch at once from the selected photos.

**Manage Profiles** opens a scrollable list of everything you've trained, with each
category's sample count and a delete button. Deleting is deliberately two clicks —
confirm and delete — because it throws away that category's training for good and
re-sorts the photos that were using it.

In the inspection modal (click any thumbnail):

| Key | Action |
| --- | --- |
| `←` / `→` | Previous / next photo |
| `Space` | Toggle selection |
| `Esc` | Close |

The full-resolution image loads in the background once the modal is open, so browsing a
folder of large files stays responsive.

Select what you want and hit **Move** or **Copy**. Files go to
`<output>/<Category>/<YYYY>/<MM>/`, and name collisions get a `_1`, `_2` suffix rather
than overwriting anything. `.xmp` and `.aae` sidecars travel with their photo. Nothing
is ever overwritten: if something already sits at the destination, that file is
reported and both are left alone.

**Undo** reverses whichever of those you did last, which makes it two different
actions rather than one:

| Last action | Undo does |
| --- | --- |
| Move | Puts the batch back where it came from, sidecars included. |
| Copy | Deletes the copies it made. Your originals are never touched. |

Undo checks before it destroys anything. It will not overwrite a path that is occupied
again — a photo you have put back yourself stays put — and it will not move or delete a
file in the output folder that has changed since the app wrote it. Those files are left
where they are and named in the status line: undo is for putting a batch back the way
it was, not for throwing away work. It also tidies up the category folders the transfer
emptied, stopping at the output folder itself, which is yours.

Every transfer says how many photos landed and what failed; the ones that failed stay
in the grid so a retry is one click away. Undo reports the same way, and it can only
undo the last batch — the journal it reads is rewritten by the next transfer that
lands anything, and a transfer that landed nothing leaves the previous batch's Undo
intact rather than replacing it.

One case is worth knowing about because the files are in both places afterwards: if
the original cannot be deleted — a read-only source folder, or a read-only file on
Windows — the photo is still filed, and the status line says so rather than calling
the transfer a failure. Undo can still reverse the filed copy; what it cannot do is
guess which of the two you want to keep.

The category is always exactly one folder. Slashes, backslashes and the characters
Windows reserves are replaced with `-`, so a category named `Vacation / Japan` files
under `Vacation - Japan` rather than inventing a subfolder. A name with nothing usable
left in it, one named `.` or `..`, and one Windows reserves for a device — `NUL`,
`COM1` and the rest — file under **Unsorted**.

## How sorting works

Every photo becomes a 512-dimension CLIP embedding. A category is a centroid, and a
photo is assigned to the category it sits closest to, as long as the similarity clears
the confidence threshold. Below that, the rules get a turn: filename keywords and aspect
ratios for screenshots, keywords like `receipt` or `invoice` for documents, EXIF
presence for camera photos. Failing all of that, the photo is marked **Unsorted**.

The threshold defaults to 0.65 and is a slider in **Settings**. Worth knowing why the
bar exists at all, though — it's what hands a photo that doesn't really resemble
anything you've trained to the filename and EXIF rules below. Turn it all the way down
and every screenshot gets filed under whatever your centroids happen to lean towards,
instead of **Screenshots**. Turn it up if confident-looking matches are landing in the
wrong category; the default sits where a mediocre match loses to the rules. Moving it
re-sorts what is already staged, and if a scan is still running that re-sort waits for it
to finish — so the whole grid always ends up judged by the same bar.

Two settings live alongside it. **Output folder** decides where transfers go, and the
current destination is shown in the status bar along the bottom of the window, so you
can always see where **Move** is about to put things. **Model** points at the CLIP
checkpoint, and is left on auto-detect — `models/next to the binary` — for the normal
case. Pointing it somewhere else makes sense if you keep the weights on another drive;
switching models clears the photo cache, because embeddings from one checkpoint mean
nothing in another's space.

Settings are written to `settings.json` as you change them, so they survive a restart. There is no
Save button, and nothing waits for one: the only thing held back is a slider drag that is still in
progress, so the file gets the value you let go at rather than the sixty-odd values the handle
passed through on the way.

Training a category folds the photo's embedding into the category centroid as a weighted
average, so a handful of representative examples gets you a usable profile. Adding more
examples shifts the centroid gradually rather than replacing it, which means a couple of
odd photos won't wreck a category you've already built up. Profiles live in
`profiles.json` next to the binary.

The grid shows where each call came from, colour-coded: green for the model, blue for a
rule, purple for something you set by hand, grey for unsorted. Worth knowing which is
which, because the model is confidently wrong sometimes and a green badge on a photo of
a dog sitting in a "Food" category is worth knowing about.

`docs/classification_pipeline.md` has the full flow if you want the detail.

## Development

```bash
cargo test      # unit tests across scanner, cache, classification and transfer
cargo fmt
cargo clippy -- -D warnings
```

CI runs all three plus a build on every push and PR.

The code is laid out like this:

| Path | What lives there |
| --- | --- |
| `src/main.rs` | Window setup |
| `src/app/` | The egui app, split by responsibility: grid, modal, profile modal, toolbar, categories, transfer |
| `src/scanner.rs` | Folder scan, threaded and parallel |
| `src/inference.rs` | CLIP model loading and embedding extraction |
| `src/profile_store.rs` | Category profiles, centroids, rules |
| `src/category_name.rs` | Category names, kept safe as one folder name |
| `src/db.rs` | SQLite cache, keyed by file hash |
| `src/media.rs` | Image decoding, EXIF dates, thumbnails |
| `src/transfer/` | The transfer layout: planning, placing files, the journal, and undo |

`photo_cache.db`, `profiles.json` and `last_execution_manifest.json` are opened by
relative path, so they land in the folder you started the app from rather than next to
the binary, and all three are safe to delete. The cache only means the next scan is
slower; the journal only matters until the next transfer overwrites it.
