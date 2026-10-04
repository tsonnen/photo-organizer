//! The on-disk layout of a photo transfer, and the one rule both directions go
//! through.
//!
//! A transfer is four steps: plan where each photo lands
//! ([`ExecutionEngine::plan_batch`]), put it there
//! ([`ExecutionEngine::execute_batch`]), record what happened
//! ([`TransferJournal`]), and — when the user changes their mind — put it back
//! ([`TransferJournal::undo`]). All four agree on a single layout:
//! `<output>/<Category>/<YYYY>/<MM>/<name>`, a `_1`/`_2` suffix for collisions,
//! `.xmp`/`.aae` beside the photo. That is why they live in one module.
//!
//! They used to be two, `execution_engine.rs` and `undo_engine.rs`, with the
//! undo side re-deriving the layout by hand. Two implementations of one layout
//! drift, and these drifted in four ways that all cost data:
//!
//! - The journal recorded paths but not whether the batch was a
//!   [`TransferMode::Move`] or a [`TransferMode::Copy`], so undo had to guess,
//!   and it guessed Move: undoing a batch of twenty copies deleted twenty copies
//!   out of the output folder.
//! - The forward direction checked a copy's length before deleting the source;
//!   the reverse direction deleted after any back-copy that returned `Ok`, so a
//!   truncated restore took the original with it.
//! - A sidecar was a `(PathBuf, PathBuf)` whose direction lived in a trailing
//!   comment, so swapping the two halves compiled and moved every sidecar the
//!   wrong way.
//! - A sidecar that failed to travel was dropped without a word.
//!
//! The fix is structural rather than a patch on each: one mutation rule
//! ([`place`]) that both directions call, one record of what a transfer actually
//! did — mode included — and direction carried in field names
//! ([`Sidecar::source`]) instead of in comments.
//!
//! The test that says all of this is
//! `tests::test_undo_is_the_inverse_of_execute_for_both_modes`, which cannot be
//! written unless the forward and reverse directions share a module.

use crate::category_name::CategoryName;
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// Where the journal of the last transfer lives.
///
/// Relative on purpose, like `profiles.json` and `photo_cache.db`: under
/// `cargo run` those land in the process working directory rather than next to
/// the binary.
pub const LAST_JOURNAL: &str = "last_execution_manifest.json";

/// What a transfer does to the source folder, and therefore what undo has to
/// do in reverse.
///
/// This is recorded in the journal, not inferred at undo time. The two modes
/// need opposite actions — a move has to be put back, a copy has to be taken
/// away again — so guessing wrong is not a cosmetic mistake.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransferMode {
    Move,
    Copy,
}

/// A file that travels with its photo: the `.xmp` Lightroom writes beside an
/// image, or the `.aae` roll iOS writes.
///
/// Named fields, not a tuple. The old `(PathBuf, PathBuf)` said which half was
/// which only in a comment, so swapping them was a change that compiled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sidecar {
    /// Where the sidecar was before the transfer.
    pub source: PathBuf,
    /// Where it landed, or where it goes back to on undo.
    pub destination: PathBuf,
    /// The sidecar as it was once placed, or `None` between planning and the
    /// transfer. Only the transfer can know it: a file's modification time is
    /// not fixed until the file exists.
    #[serde(default)]
    pub filed: Option<Fingerprint>,
}

/// Size and modification time of a file as they were when the journal was
/// written: enough to tell, later, whether the file at that path is still the
/// one this app put there.
///
/// Deliberately not a content hash. Hashing the batch again costs a full read
/// of every photo on a Move — where the transfer itself is a rename and costs
/// almost nothing — and a second pass over a batch that just left the page
/// cache on a Copy. Rewriting a photo changes its length, its modification time
/// or both, which is the pair git's index caches for exactly the same job. The
/// residual gap is an edit that preserves the length *and* lands inside one
/// filesystem timestamp tick, which needs FAT's 2-second granularity to matter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fingerprint {
    pub bytes: u64,
    /// Seconds since the Unix epoch. Negative for a file stamped before 1970,
    /// which `SystemTime` reports and `duration_since(UNIX_EPOCH)` refuses to.
    pub modified_secs: i64,
    pub modified_nanos: u32,
}

impl Fingerprint {
    /// Reads the fingerprint of a file that exists.
    fn read(path: &Path) -> Result<Self> {
        let meta = fs::metadata(path)?;
        let modified = match meta.modified() {
            Ok(t) => t,
            // No usable timestamp. Recorded as the epoch so the comparison
            // below fails and the file is treated as changed, which is the
            // direction that keeps it.
            Err(_) => UNIX_EPOCH,
        };
        let (modified_secs, modified_nanos) = match modified.duration_since(UNIX_EPOCH) {
            Ok(d) => (d.as_secs() as i64, d.subsec_nanos()),
            Err(e) => {
                let d = e.duration();
                (-(d.as_secs() as i64), 0)
            }
        };
        Ok(Self {
            bytes: meta.len(),
            modified_secs,
            modified_nanos,
        })
    }

    /// Whether `path` still holds the file this fingerprint was taken from.
    ///
    /// A path that cannot be read answers `false`, so a file that has been
    /// deleted, unmounted or made unreadable is never treated as unchanged.
    fn still_matches(&self, path: &Path) -> bool {
        Fingerprint::read(path).is_ok_and(|now| now == *self)
    }
}

/// One photo's transfer: where it came from, where it landed, and enough to
/// reverse it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileOperation {
    pub source: PathBuf,
    pub destination: PathBuf,
    /// The photo as it was once placed. `None` only between planning and the
    /// transfer; every entry in a journal has one.
    #[serde(default)]
    pub filed: Option<Fingerprint>,
    #[serde(default)]
    pub sidecars: Vec<Sidecar>,
}

/// A planned file that did not make it, and why.
///
/// Recorded at file granularity rather than photo granularity, because a sidecar
/// can fail on its own: the photo beside it has already moved, so failing the
/// whole operation would leave that photo in the output folder with no record
/// to undo it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailedOp {
    pub operation: FileOperation,
    pub error: String,
}

impl FailedOp {
    /// One clause naming the file and what went wrong, for the status line.
    pub fn describe(&self) -> String {
        format!(
            "{} failed: {}",
            file_name(&self.operation.destination),
            self.error
        )
    }
}

/// What one finished transfer leaves behind: which photos landed where, which
/// did not, and whether the batch was a move or a copy.
///
/// This is the whole of what undo knows. It is written once, after the batch,
/// so a transfer interrupted by a crash leaves no journal and nothing claims
/// otherwise.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct TransferJournal {
    /// The mode of the batch. `None` only in a journal written before the mode
    /// was recorded; undo refuses those rather than assuming, because the two
    /// modes need opposite actions.
    #[serde(default)]
    pub mode: Option<TransferMode>,
    /// The output folder the batch went into, so undo can tell which folders it
    /// emptied and stop there.
    #[serde(default)]
    pub output_dir: PathBuf,
    #[serde(default)]
    pub completed_ops: Vec<FileOperation>,
    #[serde(default)]
    pub failed_ops: Vec<FailedOp>,
}

impl TransferJournal {
    fn for_batch(mode: TransferMode, output_dir: PathBuf) -> Self {
        Self {
            mode: Some(mode),
            output_dir,
            ..Default::default()
        }
    }

    /// Writes the journal so undo can find it.
    ///
    /// Written beside the target and renamed into place: a journal is only
    /// useful if it is complete, and `fs::write` truncates first, so a full disk
    /// would otherwise leave half a JSON document that undo cannot parse.
    pub fn save_to(&self, path: &Path) -> Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        let staging = path.with_extension("json.staging");
        let result = fs::write(&staging, json)
            .with_context(|| format!("writing {}", staging.display()))
            .and_then(|()| {
                fs::rename(&staging, path)
                    .with_context(|| format!("renaming into {}", path.display()))
            });
        if result.is_err() {
            // Never leave the half-written staging file next to the real one:
            // the next attempt would read as a stale journal to whoever finds it.
            let _ = fs::remove_file(&staging);
        }
        result
    }

    /// Reads back a journal written by [`TransferJournal::save_to`].
    pub fn load_from(path: &Path) -> Result<Self> {
        let contents =
            fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        serde_json::from_str(&contents).with_context(|| format!("parsing {}", path.display()))
    }

    /// Reverses the batch, newest operation first, reporting one status per
    /// file.
    ///
    /// The two modes need opposite actions, which is what the recorded mode is
    /// for:
    ///
    /// - **Move**: the file goes back where it came from, through the same
    ///   [`place`] call the forward direction made with the two paths swapped.
    /// - **Copy**: nothing ever left the source folder, so the inverse is to
    ///   remove the copies this app made in the output folder.
    ///
    /// In both directions undo refuses to destroy work: it will not overwrite a
    /// path that is occupied again, and it will not move or delete a file whose
    /// fingerprint no longer matches the journal's — that file has been edited
    /// or replaced, so it is left where it is and reported.
    pub fn undo<F>(&self, progress: F) -> Result<Vec<UndoStatus>>
    where
        F: Fn(usize, usize, &FileOperation),
    {
        let Some(mode) = self.mode else {
            return Err(anyhow!(
                "this journal was written before transfers recorded their mode, so \
                 undo cannot tell whether to move files back or delete the copies; \
                 nothing was changed"
            ));
        };

        let mut results = Vec::new();
        let ops_reversed: Vec<&FileOperation> = self.completed_ops.iter().rev().collect();
        let total = ops_reversed.len();

        for (idx, op) in ops_reversed.iter().enumerate() {
            progress(idx + 1, total, op);
            for status in undo_op(op, mode, &self.output_dir) {
                results.push(status);
            }
        }
        Ok(results)
    }
}

/// What undo did with one file. The path is the file this status is about: the
/// copy or filed photo in the output folder, except for [`UndoStatus::Restored`],
/// where it is the file's original location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UndoStatus {
    /// Put the file back where it came from. Reverses a Move.
    Restored(PathBuf),
    /// Deleted the copy this app made. Reverses a Copy.
    Removed(PathBuf),
    /// Nothing at the path in the output folder: this batch is already undone,
    /// or the user moved the file on.
    SkippedMissing(PathBuf),
    /// The file in the output folder is not the one the journal placed, so undo
    /// left it there rather than moving or deleting the user's changes.
    SkippedChanged(PathBuf),
    /// Something already occupies the path the file would go back to, and undo
    /// does not overwrite.
    SkippedOccupied(PathBuf),
    /// The reversal failed. Nothing was removed.
    Failed(PathBuf, String),
}

impl UndoStatus {
    /// Whether undo left this file for the user to deal with by hand.
    ///
    /// A missing file is not one of them: undo has nothing to do about a batch
    /// it has already reversed.
    pub fn needs_attention(&self) -> bool {
        matches!(
            self,
            UndoStatus::SkippedChanged(_)
                | UndoStatus::SkippedOccupied(_)
                | UndoStatus::Failed(_, _)
        )
    }

    /// One clause describing what happened, for the status line.
    pub fn describe(&self) -> String {
        match self {
            UndoStatus::Restored(path) => {
                format!("{} is back where it came from", file_name(path))
            }
            UndoStatus::Removed(path) => format!("removed the copy {}", file_name(path)),
            UndoStatus::SkippedMissing(path) => format!("{} was already gone", file_name(path)),
            UndoStatus::SkippedChanged(path) => format!(
                "left {} alone, it changed after it was filed",
                file_name(path)
            ),
            UndoStatus::SkippedOccupied(path) => format!(
                "left {} alone, something already sits at the path it would go back to",
                file_name(path)
            ),
            UndoStatus::Failed(path, error) => format!("{}: {}", file_name(path), error),
        }
    }
}

/// Plans and runs a batch of transfers into one output folder.
pub struct ExecutionEngine {
    base_output_dir: PathBuf,
    mode: TransferMode,
}

impl ExecutionEngine {
    pub fn new(base_output_dir: PathBuf, mode: TransferMode) -> Self {
        Self {
            base_output_dir,
            mode,
        }
    }

    /// Works out where each input lands, without touching the filesystem.
    ///
    /// Pure planning is deliberate: `execute_batch` can then be handed exactly
    /// the paths undo will later reverse, rather than re-deriving them.
    pub fn plan_batch(&self, inputs: &[RawPhotoInput]) -> Vec<FileOperation> {
        let mut planned_ops = Vec::new();
        let mut reserved_paths: HashSet<PathBuf> = HashSet::new();

        for input in inputs {
            // Safe because `subject` is a `CategoryName`: it is one component by
            // construction, so this joins exactly three levels. It used to be a
            // display `String`, which meant a category typed as `A/B` silently
            // created two directories and `..` walked out of the output folder.
            let rel_dir = Path::new(input.subject.as_str())
                .join(format!("{:04}", input.year))
                .join(format!("{:02}", input.month));

            let target_dir = self.base_output_dir.join(rel_dir);
            let stem = input
                .source_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("file");
            let ext = input
                .source_path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("");

            let (destination, final_stem) =
                resolve_destination(&target_dir, stem, ext, &reserved_paths);
            reserved_paths.insert(destination.clone());

            planned_ops.push(FileOperation {
                source: input.source_path.clone(),
                destination,
                filed: None,
                sidecars: discover_sidecars(&input.source_path, &target_dir, &final_stem),
            });
        }

        planned_ops
    }

    /// Runs every planned operation, recording what landed and what did not.
    ///
    /// One failure does not stop the batch: a photo that cannot be written
    /// should not cost the user the other nineteen. Every failure is kept in
    /// `failed_ops` rather than logged, so the app can say which photos did not
    /// make it instead of dropping them from the grid as if they had.
    pub fn execute_batch<F>(&self, operations: &[FileOperation], progress: F) -> TransferJournal
    where
        F: Fn(usize, usize, &FileOperation),
    {
        let mut journal = TransferJournal::for_batch(self.mode, self.base_output_dir.clone());
        let total = operations.len();

        for (idx, op) in operations.iter().enumerate() {
            progress(idx + 1, total, op);
            let mut sidecar_failures = Vec::new();
            let outcome = self.execute_single(op, &mut sidecar_failures);
            journal.failed_ops.append(&mut sidecar_failures);
            match outcome {
                Ok(filed) => journal.completed_ops.push(filed),
                Err(e) => journal.failed_ops.push(FailedOp {
                    operation: op.clone(),
                    error: e.to_string(),
                }),
            }
        }
        journal
    }

    /// Transfers one photo and its sidecars, or reports why it did not happen.
    ///
    /// A sidecar that fails is recorded separately and does not fail the photo:
    /// the photo is already in place by then, and recording the operation as
    /// failed would leave it in the output folder with no journal entry to move
    /// it back.
    fn execute_single(
        &self,
        op: &FileOperation,
        sidecar_failures: &mut Vec<FailedOp>,
    ) -> Result<FileOperation> {
        let filed = place(&op.source, &op.destination, self.mode)?;
        let mut sidecars = Vec::with_capacity(op.sidecars.len());

        for sidecar in &op.sidecars {
            match place(&sidecar.source, &sidecar.destination, self.mode) {
                Ok(filed) => sidecars.push(Sidecar {
                    source: sidecar.source.clone(),
                    destination: sidecar.destination.clone(),
                    filed: Some(filed),
                }),
                Err(e) => sidecar_failures.push(FailedOp {
                    operation: FileOperation {
                        source: sidecar.source.clone(),
                        destination: sidecar.destination.clone(),
                        filed: None,
                        sidecars: Vec::new(),
                    },
                    error: format!("sidecar: {e}"),
                }),
            }
        }

        Ok(FileOperation {
            source: op.source.clone(),
            destination: op.destination.clone(),
            filed: Some(filed),
            sidecars,
        })
    }
}

/// A photo to file, and the folder it belongs in.
pub struct RawPhotoInput {
    pub source_path: PathBuf,
    /// The category, as a name already known to be one directory component.
    pub subject: CategoryName,
    pub year: u32,
    pub month: u32,
}

/// The one rule for putting a file at `to`.
///
/// Both directions call it: the forward direction as
/// `place(source, destination, mode)`, undo as
/// `place(destination, source, mode)`. That is what makes undo the inverse of
/// execute rather than a second implementation of it.
///
/// It refuses to overwrite whatever is already at `to`, never removes `from`
/// until the copy at `to` is verified complete, and leaves nothing
/// half-written behind. Returns the fingerprint of `to` as it now is, which is
/// what the journal records.
fn place(from: &Path, to: &Path, mode: TransferMode) -> Result<Fingerprint> {
    if !from.exists() {
        return Err(anyhow!("{} does not exist", from.display()));
    }
    // Also what stops the degenerate case where the user picks the input folder
    // as the output folder: copying a file onto itself truncates it, and the
    // length check below would then compare zero against zero and pass.
    if to.exists() {
        return Err(anyhow!("refusing to overwrite {}", to.display()));
    }
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)?;
    }

    match mode {
        TransferMode::Copy => copy_verified(from, to)?,
        TransferMode::Move => {
            // A rename within one filesystem cannot truncate, so it is
            // preferred. It fails across devices, which is the common case
            // here: a source folder on an internal disk and a library on an
            // external one.
            if fs::rename(from, to).is_err() {
                copy_verified(from, to)?;
                // The copy is verified complete before this runs, so the
                // original can be left behind but never lost. A failure here
                // means the file is now in both places, which the message has to
                // say rather than reporting as a plain refusal.
                fs::remove_file(from).map_err(|e| {
                    anyhow!(
                        "{} was copied but the original at {} could not be removed: {e}",
                        to.display(),
                        from.display()
                    )
                })?;
            }
        }
    }

    Fingerprint::read(to)
}

/// Copies `from` to `to` and refuses to leave the copy behind unless it is
/// complete.
///
/// `fs::copy` returns how many bytes it wrote, so a copy that stopped early —
/// a full disk, a cable pulled out — is caught here without re-reading either
/// file. The forward direction did this and the reverse direction did not, which
/// is how a truncated restore could take the original with it.
fn copy_verified(from: &Path, to: &Path) -> Result<()> {
    let written = fs::copy(from, to)?;
    let expected = fs::metadata(from)?.len();
    let landed = fs::metadata(to).map(|m| m.len()).unwrap_or(u64::MAX);
    reject_short_copy(from, to, written, landed, expected)
}

/// Decides whether a copy that reported `written` bytes and left `landed` behind
/// is complete enough to keep.
///
/// Split out of [`copy_verified`] because a short write cannot be provoked on
/// demand: a full disk and a pulled cable both leave behind exactly this state,
/// and neither can be arranged from a test.
fn reject_short_copy(
    from: &Path,
    to: &Path,
    written: u64,
    landed: u64,
    expected: u64,
) -> Result<()> {
    if written == expected && landed == expected {
        return Ok(());
    }
    // `to` did not exist when this call started, so removing it removes the
    // partial copy and nothing else. That invariant is what makes the same
    // cleanup safe here whether this is the forward direction or undo.
    let _ = fs::remove_file(to);
    Err(anyhow!(
        "copy of {} stopped at {landed} of {expected} bytes",
        from.display()
    ))
}

/// Reverses one operation: the photo, then the sidecars that travelled with it.
fn undo_op(op: &FileOperation, mode: TransferMode, output_dir: &Path) -> Vec<UndoStatus> {
    let mut statuses = vec![undo_file(
        &op.destination,
        &op.source,
        op.filed,
        mode,
        output_dir,
    )];
    for sidecar in &op.sidecars {
        statuses.push(undo_file(
            &sidecar.destination,
            &sidecar.source,
            sidecar.filed,
            mode,
            output_dir,
        ));
    }
    statuses
}

/// Reverses one file, in whichever direction `mode` says.
fn undo_file(
    from: &Path,
    to: &Path,
    filed: Option<Fingerprint>,
    mode: TransferMode,
    output_dir: &Path,
) -> UndoStatus {
    if !from.exists() {
        return UndoStatus::SkippedMissing(from.to_path_buf());
    }

    let Some(filed) = filed else {
        // No fingerprint means nothing recorded what this file looked like, so
        // there is no way to tell this app's copy from a file the user has
        // since edited. Leave it.
        return UndoStatus::SkippedChanged(from.to_path_buf());
    };
    if !filed.still_matches(from) {
        return UndoStatus::SkippedChanged(from.to_path_buf());
    }

    match mode {
        // Nothing left the source folder, so the inverse of a copy is that the
        // copy is gone. This is the only deletion undo performs, and the
        // fingerprint above is what makes it safe.
        TransferMode::Copy => match fs::remove_file(from) {
            Ok(()) => {
                prune_empty_dirs(from, output_dir);
                UndoStatus::Removed(from.to_path_buf())
            }
            Err(e) => UndoStatus::Failed(from.to_path_buf(), e.to_string()),
        },
        // A move is undone with another move: the file comes back the same way it
        // went out, through the same rule, so the two directions cannot disagree
        // about renames, cross-device copies or a truncated write.
        TransferMode::Move => {
            // `place` refuses to overwrite, so an occupied original path is a
            // refusal rather than a failure. Deciding it here keeps the filed
            // photo in the output folder instead of treating it as damage.
            if to.exists() {
                return UndoStatus::SkippedOccupied(from.to_path_buf());
            }
            match place(from, to, TransferMode::Move) {
                Ok(_) => {
                    prune_empty_dirs(from, output_dir);
                    UndoStatus::Restored(to.to_path_buf())
                }
                // `place` never removes `from` until the copy at `to` is
                // verified complete, so a failure here leaves the file in the
                // output folder exactly as it was.
                Err(e) => UndoStatus::Failed(from.to_path_buf(), e.to_string()),
            }
        }
    }
}

/// Removes the category folders a reversal emptied, stopping at the first one
/// that is not empty.
///
/// Bounded twice over: it never walks above `output_dir`, and it only removes
/// directories with nothing in them, so undo cannot delete anything of the
/// user's — including the output folder itself, which is theirs by name. Both
/// bounds also fail safe: a journal whose `output_dir` no longer lines up with
/// the paths in it leaves the folders alone.
fn prune_empty_dirs(from: &Path, output_dir: &Path) {
    let Some(mut dir) = from.parent().map(Path::to_path_buf) else {
        return;
    };
    while dir != output_dir && dir.starts_with(output_dir) {
        // Anything left in a folder, or a folder that cannot be read, means we
        // have reached the edge of what this reversal emptied.
        let is_empty = fs::read_dir(&dir)
            .map(|mut entries| entries.next().is_none())
            .unwrap_or(false);
        if !is_empty || fs::remove_dir(&dir).is_err() {
            return;
        }
        match dir.parent() {
            Some(parent) => dir = parent.to_path_buf(),
            None => return,
        }
    }
}

/// Picks a free filename in `target_dir`, appending `_1`, `_2` and so on.
///
/// Checks the disk and the batch's own reservations, because the plan and the
/// transfer can be minutes apart for a large batch.
fn resolve_destination(
    target_dir: &Path,
    stem: &str,
    ext: &str,
    reserved: &HashSet<PathBuf>,
) -> (PathBuf, String) {
    let mut counter = 0;
    loop {
        let cur_stem = if counter == 0 {
            stem.to_string()
        } else {
            format!("{}_{}", stem, counter)
        };
        let filename = if ext.is_empty() {
            cur_stem.clone()
        } else {
            format!("{}.{}", cur_stem, ext)
        };
        let candidate = target_dir.join(&filename);

        if !candidate.exists() && !reserved.contains(&candidate) {
            return (candidate, cur_stem);
        }
        counter += 1;
    }
}

/// Finds the sidecars that travel with `source`, each with its direction in
/// field names rather than as a tuple.
fn discover_sidecars(source: &Path, target_dir: &Path, final_stem: &str) -> Vec<Sidecar> {
    // The extension list carries both cases because a `.XMP` next to a `.jpg` is
    // a real thing on a case-sensitive filesystem, and both are then a real
    // sidecar of the photo. On a case-insensitive one (macOS, Windows) both
    // lookups find the *same* file, so the sources are deduplicated: without
    // that, half of these sidecars would be transferred twice and the second
    // attempt reported as a failure.
    //
    // The destinations need no collision check: they are built from the photo's
    // own resolved stem, which `resolve_destination` has already made unique
    // across the batch.
    let mut seen_sources: HashSet<PathBuf> = HashSet::new();
    let mut sidecars = Vec::new();

    for sidecar_ext in ["xmp", "XMP", "aae", "AAE"] {
        let sidecar_src = source.with_extension(sidecar_ext);
        if sidecar_src.exists() && seen_sources.insert(sidecar_src.clone()) {
            sidecars.push(Sidecar {
                source: sidecar_src,
                destination: target_dir.join(format!("{}.{}", final_stem, sidecar_ext)),
                filed: None,
            });
        }
    }
    sidecars
}

/// The last component of a path, for a message that has to fit in a status line.
fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Pushes a file's modification time forward, so a test can prove that an edit
/// is noticed even when it leaves the file exactly the same length.
#[cfg(test)]
fn bump_mtime(path: &Path, by: std::time::Duration) {
    let file = fs::OpenOptions::new().write(true).open(path).unwrap();
    let later = std::time::SystemTime::now() + by;
    file.set_times(fs::FileTimes::new().set_modified(later))
        .unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;
    use std::time::Duration;

    /// A directory of this process's own, named for the test, removed by the
    /// caller. Unique per test because cargo runs them on parallel threads.
    fn temp_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("transfer_{name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn write_file(path: &Path, contents: &[u8]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let mut file = File::create(path).unwrap();
        file.write_all(contents).unwrap();
    }

    /// Every file under `root` as sorted `(relative path, byte length)` pairs.
    ///
    /// Comparing two of these is how these tests say "the filesystem is back
    /// where it started" without depending on directory timestamps, inodes or
    /// which empty folders happen to remain.
    fn snapshot(root: &Path) -> Vec<(PathBuf, u64)> {
        fn walk(root: &Path, dir: &Path, files: &mut Vec<(PathBuf, u64)>) {
            let Ok(entries) = fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(root, &path, files);
                } else {
                    let bytes = fs::metadata(&path).map(|m| m.len()).unwrap_or(u64::MAX);
                    files.push((
                        path.strip_prefix(root).unwrap_or(&path).to_path_buf(),
                        bytes,
                    ));
                }
            }
        }

        let mut files = Vec::new();
        walk(root, root, &mut files);
        files.sort();
        files
    }

    fn nature_input(source_path: PathBuf) -> RawPhotoInput {
        RawPhotoInput {
            source_path,
            subject: CategoryName::from_user_input("Nature"),
            year: 2024,
            month: 5,
        }
    }

    // --- The inverse property ------------------------------------------------

    /// The test that could not be written before: for either mode, undoing a
    /// transfer has to leave the filesystem exactly as it started, sidecars and
    /// all.
    #[test]
    fn test_undo_is_the_inverse_of_execute_for_both_modes() {
        for mode in [TransferMode::Move, TransferMode::Copy] {
            let root = temp_root(&format!("inverse_{mode:?}").to_lowercase());
            let src = root.join("photos");
            let out = root.join("library");
            fs::create_dir_all(&src).unwrap();
            write_file(&src.join("beach.jpg"), b"beach jpeg bytes");
            write_file(&src.join("beach.xmp"), b"<xmp/>");
            write_file(&src.join("receipt.png"), b"receipt png bytes");

            let starting_state = snapshot(&root);

            let inputs = vec![
                nature_input(src.join("beach.jpg")),
                nature_input(src.join("receipt.png")),
            ];
            let engine = ExecutionEngine::new(out.clone(), mode);
            let plan = engine.plan_batch(&inputs);
            assert_eq!(plan.len(), 2);
            assert_eq!(plan[0].sidecars.len(), 1, "{mode:?} should find the xmp");

            let journal = engine.execute_batch(&plan, |_, _, _| {});
            assert_eq!(journal.completed_ops.len(), 2, "{mode:?}");
            assert!(
                journal.failed_ops.is_empty(),
                "{mode:?}: {:?}",
                journal.failed_ops
            );
            assert!(
                out.join("Nature/2024/05/beach.xmp").exists(),
                "{mode:?}: the sidecar should travel with the photo"
            );

            let statuses = journal.undo(|_, _, _| {}).unwrap();
            assert!(
                statuses.iter().all(|s| !s.needs_attention()),
                "{mode:?}: {statuses:?}"
            );
            assert_eq!(
                snapshot(&root),
                starting_state,
                "{mode:?} did not put the filesystem back"
            );
            assert!(
                !out.join("Nature/2024/05").exists(),
                "{mode:?} left the emptied category folder behind"
            );

            let _ = fs::remove_dir_all(&root);
        }
    }

    #[test]
    fn test_undo_of_a_copy_removes_the_copies_and_keeps_the_originals() {
        let root = temp_root("undo_copy");
        let src = root.join("photos");
        let out = root.join("library");
        fs::create_dir_all(&src).unwrap();
        write_file(&src.join("photo.jpg"), b"the original");

        let engine = ExecutionEngine::new(out.clone(), TransferMode::Copy);
        let journal = {
            let plan = engine.plan_batch(&[nature_input(src.join("photo.jpg"))]);
            engine.execute_batch(&plan, |_, _, _| {})
        };

        let filed = out.join("Nature/2024/05/photo.jpg");
        assert!(
            src.join("photo.jpg").exists(),
            "a copy leaves the original alone"
        );
        assert!(filed.exists());

        let statuses = journal.undo(|_, _, _| {}).unwrap();
        assert_eq!(statuses, vec![UndoStatus::Removed(filed.clone())]);
        assert!(!filed.exists(), "undoing a copy takes the copy away");
        assert_eq!(
            fs::read(src.join("photo.jpg")).unwrap(),
            b"the original",
            "undoing a copy must never touch the original"
        );

        let _ = fs::remove_dir_all(&root);
    }

    /// The defect this whole change is about, in the form it takes today:
    /// twenty copies, three clicks, twenty files gone.
    #[test]
    fn test_undo_of_a_copy_leaves_a_file_the_user_edited_alone() {
        let root = temp_root("undo_copy_edited");
        let src = root.join("photos");
        let out = root.join("library");
        fs::create_dir_all(&src).unwrap();
        write_file(&src.join("photo.jpg"), b"the original");

        let engine = ExecutionEngine::new(out.clone(), TransferMode::Copy);
        let journal = {
            let plan = engine.plan_batch(&[nature_input(src.join("photo.jpg"))]);
            engine.execute_batch(&plan, |_, _, _| {})
        };

        let filed = out.join("Nature/2024/05/photo.jpg");
        // The user crops the copy in the output folder. The new file is longer,
        // so even a length check would catch it.
        write_file(&filed, b"the original, cropped, retouched and longer");
        let statuses = journal.undo(|_, _, _| {}).unwrap();
        assert_eq!(statuses, vec![UndoStatus::SkippedChanged(filed.clone())]);
        assert!(filed.exists(), "an edited file must survive undo");
        assert!(statuses[0].needs_attention());

        let _ = fs::remove_dir_all(&root);
    }

    /// The same protection for an edit that keeps the file the same length,
    /// which is the case a length check alone would wave through.
    #[test]
    fn test_undo_detects_an_edit_that_keeps_the_file_length() {
        let root = temp_root("undo_same_length_edit");
        let src = root.join("photos");
        let out = root.join("library");
        fs::create_dir_all(&src).unwrap();
        write_file(&src.join("photo.jpg"), b"before");

        let engine = ExecutionEngine::new(out.clone(), TransferMode::Copy);
        let journal = {
            let plan = engine.plan_batch(&[nature_input(src.join("photo.jpg"))]);
            engine.execute_batch(&plan, |_, _, _| {})
        };

        let filed = out.join("Nature/2024/05/photo.jpg");
        write_file(&filed, b"after!");
        assert_eq!(
            fs::metadata(&filed).unwrap().len(),
            6,
            "same length as `before`"
        );
        // Make the timestamp difference unambiguous, whatever the filesystem's
        // resolution happens to be.
        bump_mtime(&filed, Duration::from_secs(60));

        let statuses = journal.undo(|_, _, _| {}).unwrap();
        assert_eq!(statuses, vec![UndoStatus::SkippedChanged(filed.clone())]);
        assert_eq!(fs::read(&filed).unwrap(), b"after!");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_undo_does_not_overwrite_a_file_that_is_back_at_its_original_path() {
        let root = temp_root("undo_occupied");
        let src = root.join("photos");
        let out = root.join("library");
        fs::create_dir_all(&src).unwrap();
        write_file(&src.join("photo.jpg"), b"the photo that got filed");

        let engine = ExecutionEngine::new(out.clone(), TransferMode::Move);
        let journal = {
            let plan = engine.plan_batch(&[nature_input(src.join("photo.jpg"))]);
            engine.execute_batch(&plan, |_, _, _| {})
        };
        let filed = out.join("Nature/2024/05/photo.jpg");
        assert!(!src.join("photo.jpg").exists());

        // The user has put a different photo back where the original was.
        write_file(
            &src.join("photo.jpg"),
            b"an unrelated photo with the same name",
        );
        let statuses = journal.undo(|_, _, _| {}).unwrap();
        assert_eq!(statuses, vec![UndoStatus::SkippedOccupied(filed.clone())]);
        assert_eq!(
            fs::read(src.join("photo.jpg")).unwrap(),
            b"an unrelated photo with the same name",
            "undo must not clobber what is already at the original path"
        );
        assert!(
            filed.exists(),
            "the filed photo stays in the output folder rather than being lost"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_undo_does_not_move_back_a_photo_edited_in_the_output_folder() {
        let root = temp_root("undo_move_edited");
        let src = root.join("photos");
        let out = root.join("library");
        fs::create_dir_all(&src).unwrap();
        write_file(&src.join("photo.jpg"), b"the original");

        let engine = ExecutionEngine::new(out.clone(), TransferMode::Move);
        let journal = {
            let plan = engine.plan_batch(&[nature_input(src.join("photo.jpg"))]);
            engine.execute_batch(&plan, |_, _, _| {})
        };

        let filed = out.join("Nature/2024/05/photo.jpg");
        write_file(&filed, b"the original, edited in the output folder");
        let statuses = journal.undo(|_, _, _| {}).unwrap();
        assert_eq!(statuses, vec![UndoStatus::SkippedChanged(filed.clone())]);
        assert!(!src.join("photo.jpg").exists(), "nothing is moved back");
        assert_eq!(
            fs::read(&filed).unwrap(),
            b"the original, edited in the output folder"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_undo_is_idempotent() {
        let root = temp_root("undo_twice");
        let src = root.join("photos");
        let out = root.join("library");
        fs::create_dir_all(&src).unwrap();
        write_file(&src.join("photo.jpg"), b"the original");

        let engine = ExecutionEngine::new(out.clone(), TransferMode::Copy);
        let journal = {
            let plan = engine.plan_batch(&[nature_input(src.join("photo.jpg"))]);
            engine.execute_batch(&plan, |_, _, _| {})
        };

        journal.undo(|_, _, _| {}).unwrap();
        let second = journal.undo(|_, _, _| {}).unwrap();
        assert_eq!(
            second,
            vec![UndoStatus::SkippedMissing(
                out.join("Nature/2024/05/photo.jpg")
            )],
            "a second undo has nothing to do"
        );
        assert!(src.join("photo.jpg").exists());

        let _ = fs::remove_dir_all(&root);
    }

    // --- The journal ---------------------------------------------------------

    #[test]
    fn test_journal_records_the_mode_and_survives_a_round_trip() {
        let root = temp_root("journal_round_trip");
        let src = root.join("photos");
        let out = root.join("library");
        fs::create_dir_all(&src).unwrap();
        write_file(&src.join("photo.jpg"), b"the original");
        write_file(&src.join("photo.xmp"), b"<xmp/>");

        let engine = ExecutionEngine::new(out.clone(), TransferMode::Move);
        let journal = {
            let plan = engine.plan_batch(&[nature_input(src.join("photo.jpg"))]);
            engine.execute_batch(&plan, |_, _, _| {})
        };

        let path = root.join(LAST_JOURNAL);
        journal.save_to(&path).unwrap();
        let read_back = TransferJournal::load_from(&path).unwrap();
        assert_eq!(read_back.mode, Some(TransferMode::Move));
        assert_eq!(read_back.output_dir, out);
        assert_eq!(read_back.completed_ops, journal.completed_ops);
        assert!(!root.join("last_execution_manifest.json.staging").exists());

        let _ = fs::remove_dir_all(&root);
    }

    /// A journal from before the mode was recorded cannot say whether the batch
    /// moved or copied files, and those need opposite actions. Guessing is what
    /// deleted twenty copies, so undo refuses instead.
    #[test]
    fn test_undo_refuses_a_journal_with_no_recorded_mode() {
        let root = temp_root("journal_legacy");
        let path = root.join(LAST_JOURNAL);
        // Exactly what the old writer produced, including the shape of an
        // operation with no fingerprint.
        fs::write(
            &path,
            r#"{"completed_ops":[{"source":"a/photo.jpg","destination":"b/photo.jpg","sidecars":[]}],"failed_ops":[]}"#,
        )
        .unwrap();

        let journal = TransferJournal::load_from(&path).unwrap();
        assert_eq!(journal.mode, None);
        let err = journal.undo(|_, _, _| {}).unwrap_err();
        assert!(
            err.to_string()
                .contains("before transfers recorded their mode"),
            "unexpected error: {err}"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_load_from_a_missing_journal_is_an_error() {
        let root = temp_root("journal_missing");
        let err = TransferJournal::load_from(&root.join("nothing-here.json")).unwrap_err();
        assert!(
            err.to_string().contains("nothing-here.json"),
            "the error should name the file it wanted: {err}"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_load_from_malformed_json_is_an_error() {
        let root = temp_root("journal_malformed");
        let path = root.join("bad_manifest.json");
        fs::write(&path, b"{ not valid json }").unwrap();

        let err = TransferJournal::load_from(&path).unwrap_err();
        assert!(err.to_string().contains("parsing"), "{err}");

        let _ = fs::remove_dir_all(&root);
    }

    /// A journal is only useful if it is complete, so a failed write has to
    /// fail loudly and leave nothing half-finished behind.
    #[test]
    fn test_save_to_reports_failure_without_leaving_a_staging_file() {
        let root = temp_root("journal_save_fails");
        let path = root.join("no-such-folder").join(LAST_JOURNAL);

        let journal = TransferJournal::for_batch(TransferMode::Copy, root.clone());
        assert!(journal.save_to(&path).is_err());
        assert!(!path.with_extension("json.staging").exists());

        let _ = fs::remove_dir_all(&root);
    }

    // --- Planning ------------------------------------------------------------

    #[test]
    fn test_plan_batch_basic_and_collision() {
        let base = PathBuf::from("/test/output");
        let engine = ExecutionEngine::new(base.clone(), TransferMode::Copy);

        let inputs = vec![
            nature_input(PathBuf::from("/photos/pic.jpg")),
            nature_input(PathBuf::from("/other/pic.jpg")),
        ];

        let ops = engine.plan_batch(&inputs);
        assert_eq!(ops.len(), 2);
        assert_eq!(ops[0].destination, base.join("Nature/2024/05/pic.jpg"));
        assert_eq!(ops[1].destination, base.join("Nature/2024/05/pic_1.jpg"));
    }

    #[test]
    fn test_plan_batch_multi_collision_chain() {
        let base = PathBuf::from("/test/output");
        let engine = ExecutionEngine::new(base.clone(), TransferMode::Copy);

        let inputs = (0..5)
            .map(|i| nature_input(PathBuf::from(format!("/src_{}/sample.png", i))))
            .collect::<Vec<_>>();

        let ops = engine.plan_batch(&inputs);
        assert_eq!(ops.len(), 5);
        assert_eq!(ops[0].destination, base.join("Nature/2024/05/sample.png"));
        assert_eq!(ops[1].destination, base.join("Nature/2024/05/sample_1.png"));
        assert_eq!(ops[2].destination, base.join("Nature/2024/05/sample_2.png"));
        assert_eq!(ops[3].destination, base.join("Nature/2024/05/sample_3.png"));
        assert_eq!(ops[4].destination, base.join("Nature/2024/05/sample_4.png"));
    }

    #[test]
    fn test_plan_batch_special_characters_in_filename() {
        let base = PathBuf::from("/test/output");
        let engine = ExecutionEngine::new(base.clone(), TransferMode::Copy);

        // Spaces, ampersands and brackets are legal in a filename on every
        // platform, so a photo's own name is passed through untouched. Only the
        // *category* is sanitised, because only the category is user-typed.
        let inputs = vec![RawPhotoInput {
            source_path: PathBuf::from("/photos/Summer Party & Fireworks (2024).jpg"),
            subject: CategoryName::from_user_input("Vacation Japan 2024"),
            year: 2024,
            month: 7,
        }];

        let ops = engine.plan_batch(&inputs);
        assert_eq!(ops.len(), 1);
        assert_eq!(
            ops[0].destination,
            base.join("Vacation Japan 2024/2024/07/Summer Party & Fireworks (2024).jpg")
        );
    }

    #[test]
    fn test_plan_batch_category_with_separator_stays_one_directory() {
        let base = PathBuf::from("/test/output");
        let engine = ExecutionEngine::new(base.clone(), TransferMode::Copy);

        // This used to assert the opposite: `Vacation / Japan 2024` joined
        // straight into the path and produced three levels of directory where
        // the README promises one.
        let inputs = vec![RawPhotoInput {
            source_path: PathBuf::from("/photos/pic.jpg"),
            subject: CategoryName::from_user_input("Vacation / Japan 2024"),
            year: 2024,
            month: 7,
        }];

        let ops = engine.plan_batch(&inputs);
        assert_eq!(
            ops[0].destination,
            base.join("Vacation - Japan 2024/2024/07/pic.jpg")
        );

        // The category contributes exactly one component, so the whole relative
        // path is category + year + month + filename and nothing more.
        let rel = ops[0]
            .destination
            .strip_prefix(&base)
            .expect("destination stays under the output dir");
        assert_eq!(rel.components().count(), 4, "got {rel:?}");
    }

    #[test]
    fn test_plan_batch_dot_category_stays_inside_the_output_dir() {
        let base = PathBuf::from("/test/output");
        let engine = ExecutionEngine::new(base.clone(), TransferMode::Copy);

        // A category named `..` or `.` would otherwise resolve to the parent or
        // the output folder itself, writing the photo outside `<Category>`.
        for hostile in ["..", "."] {
            let inputs = vec![RawPhotoInput {
                source_path: PathBuf::from("/photos/pic.jpg"),
                subject: CategoryName::from_user_input(hostile),
                year: 2024,
                month: 7,
            }];

            let ops = engine.plan_batch(&inputs);
            assert_eq!(
                ops[0].destination,
                base.join("Unsorted/2024/07/pic.jpg"),
                "{hostile:?} should fall back to Unsorted"
            );
        }

        // A traversal with separators in it cannot survive either: the
        // separators become dashes, so what is left is an odd but entirely
        // harmless single directory.
        let inputs = vec![RawPhotoInput {
            source_path: PathBuf::from("/photos/pic.jpg"),
            subject: CategoryName::from_user_input("../.."),
            year: 2024,
            month: 7,
        }];
        let ops = engine.plan_batch(&inputs);
        assert!(
            ops[0].destination.starts_with(&base),
            "escaped the output dir: {:?}",
            ops[0].destination
        );
        assert_eq!(
            ops[0]
                .destination
                .strip_prefix(&base)
                .unwrap()
                .components()
                .count(),
            4,
            "category + year + month + filename, and nothing more"
        );
    }

    #[test]
    fn test_plan_batch_finds_both_cases_of_a_sidecar_extension() {
        let root = temp_root("sidecar_cases");
        let src = root.join("photos");
        fs::create_dir_all(&src).unwrap();
        // Two genuinely different files on a case-sensitive filesystem; both are
        // a real sidecar of the photo and both travel.
        write_file(&src.join("photo.jpg"), b"the original");
        write_file(&src.join("photo.xmp"), b"lower");
        write_file(&src.join("photo.XMP"), b"upper");

        let engine = ExecutionEngine::new(root.join("library"), TransferMode::Copy);
        let ops = engine.plan_batch(&[nature_input(src.join("photo.jpg"))]);
        let destinations: Vec<String> = ops[0]
            .sidecars
            .iter()
            .map(|s| s.destination.to_string_lossy().into_owned())
            .collect();
        assert_eq!(destinations.len(), 2, "got {destinations:?}");
        assert!(
            destinations[0].ends_with("photo.xmp"),
            "got {destinations:?}"
        );
        assert!(
            destinations[1].ends_with("photo.XMP"),
            "got {destinations:?}"
        );

        let _ = fs::remove_dir_all(&root);
    }

    // --- Executing -----------------------------------------------------------

    #[test]
    fn test_execute_batch_copies_the_bytes_and_keeps_the_source() {
        let root = temp_root("exec_copy");
        let src = root.join("photos");
        let out = root.join("library");
        fs::create_dir_all(&src).unwrap();
        let photo = src.join("photo.jpg");
        write_file(&photo, b"image data, byte for byte");

        let engine = ExecutionEngine::new(out.clone(), TransferMode::Copy);
        let plan = engine.plan_batch(&[nature_input(photo.clone())]);
        let journal = engine.execute_batch(&plan, |_, _, _| {});

        assert_eq!(journal.completed_ops.len(), 1);
        assert!(journal.failed_ops.is_empty());
        assert_eq!(
            fs::read(&plan[0].destination).unwrap(),
            fs::read(&photo).unwrap()
        );
        assert!(photo.exists(), "a copy leaves the source in place");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_execute_batch_moves_the_bytes_and_leaves_the_source_gone() {
        let root = temp_root("exec_move");
        let src = root.join("photos");
        let out = root.join("library");
        fs::create_dir_all(&src).unwrap();
        let photo = src.join("photo.jpg");
        write_file(&photo, b"image data, byte for byte");

        let engine = ExecutionEngine::new(out.clone(), TransferMode::Move);
        let plan = engine.plan_batch(&[nature_input(photo.clone())]);
        let journal = engine.execute_batch(&plan, |_, _, _| {});

        assert_eq!(journal.completed_ops.len(), 1);
        assert!(!photo.exists(), "a move takes the source with it");
        assert_eq!(
            fs::read(&plan[0].destination).unwrap(),
            b"image data, byte for byte"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_execute_batch_onto_its_own_source_folder_does_not_destroy_anything() {
        let root = temp_root("exec_same_folder");
        let src = root.join("photos");
        fs::create_dir_all(&src).unwrap();
        let photo = src.join("photo.jpg");
        write_file(&photo, b"image data");

        // The output folder is the input folder. Copying a file onto itself
        // truncates it, and the length check afterwards would compare zero with
        // zero and call that a success.
        let engine = ExecutionEngine::new(src.clone(), TransferMode::Copy);
        let plan = engine.plan_batch(&[nature_input(photo.clone())]);
        engine.execute_batch(&plan, |_, _, _| {});

        assert_eq!(fs::read(&photo).unwrap(), b"image data");
        assert_eq!(plan[0].destination, src.join("Nature/2024/05/photo.jpg"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_execute_batch_refuses_to_overwrite_an_existing_destination() {
        let root = temp_root("exec_no_overwrite");
        let src = root.join("photos");
        let out = root.join("library");
        fs::create_dir_all(&src).unwrap();
        write_file(&src.join("photo.jpg"), b"incoming");

        // A directory sitting where the sidecar will go. The planner does not
        // resolve sidecar names, so this is where the no-overwrite rule has to
        // hold — and the refusal has to be reported rather than swallowed.
        let blocked = out.join("Nature/2024/05/photo.xmp");
        fs::create_dir_all(&blocked).unwrap();
        write_file(&src.join("photo.jpg"), b"incoming");
        write_file(&src.join("photo.xmp"), b"<xmp/>");

        let engine = ExecutionEngine::new(out.clone(), TransferMode::Copy);
        let plan = engine.plan_batch(&[nature_input(src.join("photo.jpg"))]);
        let journal = engine.execute_batch(&plan, |_, _, _| {});

        assert_eq!(journal.completed_ops.len(), 1, "the photo still lands");
        assert_eq!(
            journal.completed_ops[0].sidecars.len(),
            0,
            "the sidecar that did not travel must not be journalled as if it had"
        );
        assert_eq!(journal.failed_ops.len(), 1, "{:?}", journal.failed_ops);
        assert_eq!(
            journal.failed_ops[0].operation.source,
            src.join("photo.xmp")
        );
        assert!(
            journal.failed_ops[0].error.starts_with("sidecar:"),
            "{:?}",
            journal.failed_ops[0].error
        );
        assert!(blocked.is_dir(), "the blocker is untouched");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_execute_batch_missing_source_failure() {
        let root = temp_root("exec_missing_source");
        let out = root.join("library");

        let engine = ExecutionEngine::new(out.clone(), TransferMode::Copy);
        let plan = engine.plan_batch(&[nature_input(PathBuf::from(
            "/nonexistent/file/does_not_exist_12345.jpg",
        ))]);
        let journal = engine.execute_batch(&plan, |_, _, _| {});

        assert!(journal.completed_ops.is_empty());
        assert_eq!(journal.failed_ops.len(), 1);
        assert!(
            journal.failed_ops[0].error.contains("does not exist"),
            "{:?}",
            journal.failed_ops[0].error
        );
        assert!(
            !out.exists(),
            "a batch that transferred nothing creates nothing"
        );

        let _ = fs::remove_dir_all(&root);
    }

    // --- The rule both directions share --------------------------------------

    #[test]
    fn test_place_refuses_to_overwrite_and_reports_the_path() {
        let root = temp_root("place_no_overwrite");
        let from = root.join("from.jpg");
        let to = root.join("to.jpg");
        write_file(&from, b"new");
        write_file(&to, b"existing");

        let err = place(&from, &to, TransferMode::Move).unwrap_err();
        assert!(err.to_string().contains("refusing to overwrite"), "{err}");
        assert_eq!(fs::read(&to).unwrap(), b"existing");
        assert!(from.exists(), "a refused transfer moves nothing");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_place_does_not_copy_a_file_onto_itself() {
        let root = temp_root("place_onto_self");
        let path = root.join("photo.jpg");
        write_file(&path, b"image data");

        assert!(place(&path, &path, TransferMode::Copy).is_err());
        assert_eq!(
            fs::read(&path).unwrap(),
            b"image data",
            "the file is still there after a refused self-copy"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_a_short_copy_is_rejected_and_not_left_behind() {
        let root = temp_root("short_copy");
        let from = root.join("from.jpg");
        let to = root.join("to.jpg");
        write_file(&from, b"0123456789");

        // What a full disk leaves behind: three of the ten bytes landed.
        write_file(&to, b"012");
        let err = reject_short_copy(&from, &to, 3, 3, 10).unwrap_err();
        assert!(err.to_string().contains("of 10 bytes"), "{err}");
        assert!(!to.exists(), "the partial copy is not left behind");

        // A complete copy is kept, and a copy that claims the right count but
        // landed short on disk is rejected too.
        write_file(&to, b"0123456789");
        assert!(reject_short_copy(&from, &to, 10, 10, 10).is_ok());
        assert!(reject_short_copy(&from, &to, 10, 3, 10).is_err());
        assert!(!to.exists());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_fingerprint_notices_a_rewritten_file() {
        let root = temp_root("fingerprint");
        let path = root.join("photo.jpg");
        write_file(&path, b"before");

        let before = Fingerprint::read(&path).unwrap();
        assert!(before.still_matches(&path));

        write_file(&path, b"after, and a different length entirely");
        assert!(!before.still_matches(&path));

        // Gone entirely reads as "changed", never as "safe to remove".
        fs::remove_file(&path).unwrap();
        assert!(!before.still_matches(&path));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_prune_empty_dirs_stops_at_the_output_folder() {
        let root = temp_root("prune");
        let out = root.join("library");
        let filed = out.join("Nature/2024/05/photo.jpg");
        let sibling = out.join("Nature/2024/05/other.jpg");
        write_file(&filed, b"image data");
        write_file(&sibling, b"something else");

        // Undo calls this once the file it just removed is gone, so that is the
        // state to reproduce. Nothing is pruned while the folder still has the
        // user's other file in it.
        fs::remove_file(&filed).unwrap();
        prune_empty_dirs(&filed, &out);
        assert!(out.join("Nature/2024/05").is_dir());

        // With the last file gone the whole chain goes, up to but not including
        // the output folder: that one is the user's, chosen by name.
        fs::remove_file(&sibling).unwrap();
        prune_empty_dirs(&filed, &out);
        assert!(out.is_dir(), "the output folder itself is never removed");
        assert!(!out.join("Nature").exists());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_prune_empty_dirs_stops_outside_the_output_folder() {
        let root = temp_root("prune_outside");
        let out = root.join("library");
        let filed = out.join("Nature/2024/05/photo.jpg");
        write_file(&filed, b"image data");

        // A journal whose output folder no longer lines up with the paths in it
        // must not walk out of it, even when every folder it meets is empty.
        prune_empty_dirs(&filed, &root.join("somewhere-else"));
        assert!(out.join("Nature").is_dir());

        let _ = fs::remove_dir_all(&root);
    }
}
