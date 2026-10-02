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

Open **AI & Categories** in the toolbar to see the model status, move the confidence
threshold, delete profiles, or train a whole batch at once from the selected photos.
Changing the threshold re-sorts everything that hasn't been manually set.

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
than overwriting anything. `.xmp` and `.aae` sidecars travel with their photo. **Undo**
puts the last batch back where it came from.

## How sorting works

Every photo becomes a 512-dimension CLIP embedding. A category is a centroid, and a
photo is assigned to the category it sits closest to, as long as the similarity clears
the threshold (0.65 by default). Below that, the rules get a turn: filename keywords and
aspect ratios for screenshots, keywords like `receipt` or `invoice` for documents, EXIF
presence for camera photos. Failing all of that, the photo is marked **Unsorted**.

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
| `src/app/` | The egui app, split by responsibility: grid, modal, toolbar, categories, transfer |
| `src/scanner.rs` | Folder scan, threaded and parallel |
| `src/inference.rs` | CLIP model loading and embedding extraction |
| `src/profile_store.rs` | Category profiles, centroids, rules |
| `src/db.rs` | SQLite cache, keyed by file hash |
| `src/media.rs` | Image decoding, EXIF dates, thumbnails |
| `src/execution_engine.rs` | Planning and running file transfers |
| `src/undo_engine.rs` | Rolling a transfer back from its manifest |

`photo_cache.db`, `profiles.json` and `last_execution_manifest.json` are written next to
the binary and are safe to delete. The first one just means the next scan is slower.
