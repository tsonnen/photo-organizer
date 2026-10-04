//! The on-disk layout of a photo transfer, and the one rule both directions go
//! through.
//!
//! Four steps, all agreeing on `<output>/<Category>/<YYYY>/<MM>/<name>` with a
//! `_1`/`_2` suffix for a name that is taken: [`ExecutionEngine::plan_batch`],
//! [`ExecutionEngine::execute_batch`], [`TransferJournal`] to record it, and
//! [`TransferJournal::undo`] to put it back. They live in one module because the
//! reverse direction used to re-derive this layout by hand, and two
//! implementations of one layout drift — see
//! `tests::test_undo_is_the_inverse_of_execute_for_both_modes`.

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
/// do in reverse: a move goes back, a copy is removed again.
///
/// Recorded in the journal rather than inferred at undo time, because the two
/// need opposite actions and guessing wrong is not a cosmetic mistake.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransferMode {
    Move,
    Copy,
}

/// A file that travels with its photo: the `.xmp` Lightroom writes beside an
/// image, or the `.aae` roll iOS writes.
///
/// Named fields rather than a tuple, so the direction is in the type and
/// swapping the halves does not compile.
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

/// Size and modification time of a file as it was when the journal was written.
///
/// Deliberately not a content hash: hashing the batch again costs a full read of
/// every photo on a Move, where the transfer itself is a rename. Rewriting a
/// photo changes its length, its mtime, or both — the pair git's index caches
/// for the same job. The residual gap is an edit that preserves the length *and*
/// lands inside one filesystem timestamp tick, which needs FAT's 2-second
/// granularity to matter.
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

/// A file the user has to know did not go the way they asked, and why.
///
/// At file granularity rather than photo granularity, because a sidecar can fail
/// on its own while the photo beside it has already moved, and because a file
/// can land and still need saying something about (see [`Placed::warning`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailedOp {
    pub operation: FileOperation,
    pub error: String,
}

impl FailedOp {
    /// A report about `path`, named by the file itself so the status line can
    /// say which one it was.
    ///
    /// The operation carries no paths of its own: this is a note about one file,
    /// not a reversal waiting to happen, and putting the photo's own source and
    /// destination in it would suggest it was.
    fn at(path: &Path, error: String) -> Self {
        Self {
            operation: FileOperation {
                source: path.to_path_buf(),
                destination: path.to_path_buf(),
                filed: None,
                sidecars: Vec::new(),
            },
            error,
        }
    }

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
/// did not, and whether the batch was a move or a copy. This is the whole of
/// what undo knows.
///
/// Written once, after the batch, so a transfer interrupted by a crash leaves no
/// journal and nothing claims otherwise.
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
    /// Staged and renamed into place, because a journal is only useful if it is
    /// complete: `fs::write` truncates first, so a full disk would otherwise
    /// leave half a JSON document that undo cannot parse.
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

    /// [`TransferJournal::save_to`], with the single-slot rule applied.
    ///
    /// Undo reads whatever is at `path` as "the last batch", so a journal that
    /// cannot be written must not leave the previous one there to be read as
    /// that: it goes, and the batch is reported as one that cannot be undone.
    /// Either way the failure is returned.
    pub fn save_as_last_batch(&self, path: &Path) -> Result<()> {
        let result = self.save_to(path);
        if result.is_err() {
            let _ = fs::remove_file(path);
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
    /// A **Move** is undone with another move, through the same [`place`] call
    /// with the two paths swapped. A **Copy** is undone by deleting the copies:
    /// nothing left the source folder, so leaving them behind would make the
    /// button a lie for half the toolbar.
    ///
    /// Either way it refuses to destroy work — see [`undo_file`] for the two
    /// checks.
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
    /// The reversal hit something the user has to deal with. Nothing was lost:
    /// the message says what was left behind, and the file is still where it
    /// was found.
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
    /// Pure planning so the journal records exactly the paths undo later
    /// reverses, rather than undo re-deriving them.
    pub fn plan_batch(&self, inputs: &[RawPhotoInput]) -> Vec<FileOperation> {
        let mut planned_ops = Vec::new();
        let mut reserved_paths: HashSet<PathBuf> = HashSet::new();
        // Batch-wide, not per photo: see `discover_sidecars`.
        let mut claimed_sidecars: HashSet<PathBuf> = HashSet::new();

        for input in inputs {
            // Exactly three levels, because `subject` is a `CategoryName` and so
            // is one path component by construction.
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

            let sidecars = discover_sidecars(
                &input.source_path,
                &target_dir,
                &final_stem,
                &mut claimed_sidecars,
            );

            planned_ops.push(FileOperation {
                source: input.source_path.clone(),
                destination,
                filed: None,
                sidecars,
            });
        }

        planned_ops
    }

    /// Runs every planned operation, recording what landed and what did not.
    ///
    /// One failure does not stop the batch, and every failure is kept rather
    /// than logged, so the app can say which photos did not make it instead of
    /// dropping them from the grid as if they had.
    pub fn execute_batch<F>(&self, operations: &[FileOperation], progress: F) -> TransferJournal
    where
        F: Fn(usize, usize, &FileOperation),
    {
        let mut journal = TransferJournal::for_batch(self.mode, self.base_output_dir.clone());
        let total = operations.len();

        for (idx, op) in operations.iter().enumerate() {
            progress(idx + 1, total, op);
            let mut failures = Vec::new();
            match self.execute_single(op, &mut failures) {
                Ok(filed) => journal.completed_ops.push(filed),
                Err(e) => journal.failed_ops.push(FailedOp {
                    operation: op.clone(),
                    error: e.to_string(),
                }),
            }
            journal.failed_ops.append(&mut failures);
        }
        journal
    }

    /// Transfers one photo and its sidecars, or reports why it did not happen.
    ///
    /// A sidecar that fails is recorded separately and does not fail the photo,
    /// which is already in place by then. Neither does a file that landed with a
    /// warning: it goes into `failures` as a report, and the operation itself is
    /// still journalled so undo can reverse it.
    fn execute_single(
        &self,
        op: &FileOperation,
        failures: &mut Vec<FailedOp>,
    ) -> Result<FileOperation> {
        let placed = place(&op.source, &op.destination, self.mode)?;
        if let Some(warning) = placed.warning {
            failures.push(FailedOp::at(&op.source, warning));
        }
        let mut sidecars = Vec::with_capacity(op.sidecars.len());

        for sidecar in &op.sidecars {
            match place(&sidecar.source, &sidecar.destination, self.mode) {
                Ok(placed) => {
                    if let Some(warning) = placed.warning {
                        failures.push(FailedOp::at(&sidecar.source, format!("sidecar: {warning}")));
                    }
                    sidecars.push(Sidecar {
                        source: sidecar.source.clone(),
                        destination: sidecar.destination.clone(),
                        filed: Some(placed.fingerprint),
                    })
                }
                Err(e) => failures.push(FailedOp::at(&sidecar.source, format!("sidecar: {e}"))),
            }
        }

        Ok(FileOperation {
            source: op.source.clone(),
            destination: op.destination.clone(),
            filed: Some(placed.fingerprint),
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

/// A file that is safely at its destination, plus anything the user has to be
/// told about the way it got there.
#[derive(Debug)]
struct Placed {
    /// Of the file at the destination: what the journal records, and what undo
    /// later compares against.
    fingerprint: Fingerprint,
    /// The transfer happened; the step that should have followed it did not. A
    /// cross-device Move whose original could not be unlinked leaves the file in
    /// both places, which is not a failed transfer — the copy is there, it is
    /// fingerprinted, and undo can still reverse it — but it is not the state the
    /// user asked for either, so it is reported rather than folded into `Ok`.
    warning: Option<String>,
}

/// The one rule for putting a file at `to`, called by both directions: forward
/// as `place(source, destination, mode)`, undo as
/// `place(destination, source, mode)`.
///
/// Refuses to overwrite, never removes `from` until the copy at `to` is verified
/// complete, and leaves nothing half-written behind. Returns `to`'s fingerprint,
/// which is what the journal records.
fn place(from: &Path, to: &Path, mode: TransferMode) -> Result<Placed> {
    if !from.exists() {
        return Err(anyhow!("{} does not exist", from.display()));
    }
    // Also what stops the degenerate case where the user picks the input folder
    // as the output folder: copying a file onto itself truncates it, and the
    // length check below would compare zero against zero and pass.
    if to.exists() {
        return Err(anyhow!("refusing to overwrite {}", to.display()));
    }
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)?;
    }

    let warning = match mode {
        TransferMode::Copy => {
            copy_verified(from, to)?;
            None
        }
        TransferMode::Move => {
            // A rename within one filesystem cannot truncate, so it is
            // preferred. It fails across devices, which is the common case here.
            if fs::rename(from, to).is_err() {
                copy_verified(from, to)?;
                // Verified complete before this runs, so the original can be left
                // behind but never lost. A failure here leaves the file in both
                // places: recorded as a warning on a transfer that did happen,
                // because dropping it would put a copy in the output folder that
                // no journal mentions and no undo can reach.
                fs::remove_file(from).err().map(|e| {
                    format!(
                        "{} is a copy, and the original at {} could not be removed: {e}; \
                         that file is now in both places",
                        to.display(),
                        from.display()
                    )
                })
            } else {
                None
            }
        }
    };

    Ok(Placed {
        fingerprint: Fingerprint::read(to)?,
        warning,
    })
}

/// Copies `from` to `to` and refuses to leave the copy behind unless it is
/// complete.
///
/// `fs::copy` returns how many bytes it wrote, so a copy that stopped early — a
/// full disk, a cable pulled out — is caught without re-reading either file.
fn copy_verified(from: &Path, to: &Path) -> Result<()> {
    let written = fs::copy(from, to)?;
    // A source that cannot be measured after it was copied is a source that has
    // gone or become unreadable; the copy it left is not a usable file, and the
    // same "nothing else is at `to`" invariant as below makes removing it safe.
    let expected = fs::metadata(from).map(|m| m.len()).map_err(|e| {
        let _ = fs::remove_file(to);
        anyhow!("reading {}: {e}", from.display())
    })?;
    let landed = fs::metadata(to).map(|m| m.len()).unwrap_or(u64::MAX);
    reject_short_copy(from, to, written, landed, expected)
}

/// The decision [`copy_verified`] makes, split out because a short write cannot
/// be provoked on demand: a full disk and a pulled cable both leave exactly this
/// state, and neither can be arranged from a test.
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
        // Nothing recorded what this file looked like, so there is no way to
        // tell this app's copy from one the user has since edited. Leave it.
        return UndoStatus::SkippedChanged(from.to_path_buf());
    };
    if !filed.still_matches(from) {
        return UndoStatus::SkippedChanged(from.to_path_buf());
    }

    match mode {
        // The only deletion undo performs, and the fingerprint above is what
        // makes it safe.
        TransferMode::Copy => match fs::remove_file(from) {
            Ok(()) => {
                prune_empty_dirs(from, output_dir);
                UndoStatus::Removed(from.to_path_buf())
            }
            Err(e) => UndoStatus::Failed(from.to_path_buf(), e.to_string()),
        },
        TransferMode::Move => {
            // `place` refuses to overwrite, so an occupied original path is a
            // refusal rather than a failure. Deciding it here keeps the filed
            // photo in the output folder instead of treating it as damage.
            if to.exists() {
                return UndoStatus::SkippedOccupied(from.to_path_buf());
            }
            match place(from, to, TransferMode::Move) {
                // A restored photo that carries a warning is in both places, so
                // reporting a plain `Restored` would leave the user to find the
                // leftover copy themselves.
                Ok(placed) => match placed.warning {
                    Some(warning) => {
                        prune_empty_dirs(from, output_dir);
                        UndoStatus::Failed(from.to_path_buf(), warning)
                    }
                    None => {
                        prune_empty_dirs(from, output_dir);
                        UndoStatus::Restored(to.to_path_buf())
                    }
                },
                // `place` never removes `from` until the copy at `to` is verified
                // complete, so a failure leaves the file exactly as it was.
                Err(e) => UndoStatus::Failed(from.to_path_buf(), e.to_string()),
            }
        }
    }
}

/// Removes the category folders a reversal emptied, stopping at the first one
/// that is not empty.
///
/// Bounded twice over so undo cannot delete anything of the user's, including
/// the output folder itself: it never walks above `output_dir`, and only removes
/// directories with nothing in them.
fn prune_empty_dirs(from: &Path, output_dir: &Path) {
    // Every path starts with an empty one — `Path::starts_with("")` is true for
    // anything — so without this the bound below is no bound at all and the walk
    // climbs to the filesystem root. A journal can only get an empty
    // `output_dir` by being written by something other than this app, but the
    // cost of finding out is the user's empty directories.
    if output_dir.as_os_str().is_empty() {
        return;
    }
    let Some(mut dir) = from.parent().map(Path::to_path_buf) else {
        return;
    };
    while dir != output_dir && dir.starts_with(output_dir) {
        // Anything left in a folder, or one that cannot be read, is the edge of
        // what this reversal emptied.
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
/// Checks the disk *and* the batch's own reservations, because the plan and the
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
///
/// `claimed` is the whole batch's, not this photo's: one sidecar file can only
/// travel once, and two photos can want the same one. `photo.jpg` and
/// `photo.jpeg` beside each other both resolve to `photo.xmp`, and their
/// destinations collide too, since the `_1` suffix only ever lands on the photo
/// itself. Letting both claim it made the second attempt fail with "does not
/// exist" (or "refusing to overwrite", for a Copy) and the status line report a
/// sidecar as broken when it had in fact travelled. So the first photo to claim
/// a sidecar gets it, and the rest get none.
///
/// The destinations need no collision check: they are built from the photo's own
/// stem, which `resolve_destination` already made unique among the photos.
fn discover_sidecars(
    source: &Path,
    target_dir: &Path,
    final_stem: &str,
    claimed: &mut HashSet<PathBuf>,
) -> Vec<Sidecar> {
    // Both cases are listed because a `.XMP` beside a `.jpg` is a real thing on a
    // case-sensitive filesystem. On a case-insensitive one (macOS, Windows) both
    // lookups find the *same* file, and the batch-wide claim set is what dedupes
    // them, along with the second pair.
    let mut sidecars = Vec::new();

    for sidecar_ext in ["xmp", "XMP", "aae", "AAE"] {
        let sidecar_src = source.with_extension(sidecar_ext);
        if sidecar_src.exists() && claimed.insert(sidecar_src.clone()) {
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

    /// `save_as_last_batch` is the app's policy: the journal is one slot that
    /// undo reads as "the last batch", so a journal describing a newer batch
    /// must not sit next to an older one waiting to be read as it.
    ///
    /// A failed write therefore takes the stale journal with it, and says so by
    /// returning the error rather than leaving a usable-looking file behind.
    #[test]
    fn test_a_failed_journal_write_removes_the_stale_journal() {
        let root = temp_root("journal_stale_removed");
        let path = root.join(LAST_JOURNAL);
        TransferJournal::for_batch(TransferMode::Move, root.clone())
            .save_to(&path)
            .unwrap();
        assert!(path.exists(), "the earlier batch left a journal");

        // A directory where the staging file goes, which is the one write in
        // `save_to` that can fail while the directory itself stays writable — so
        // the removal of the stale journal can also be observed.
        fs::create_dir_all(path.with_extension("json.staging")).unwrap();

        let journal = TransferJournal::for_batch(TransferMode::Copy, root.clone());
        assert!(journal.save_as_last_batch(&path).is_err());
        assert!(
            !path.exists(),
            "a journal describing the previous batch must not survive a failed write, \
             or undo reverses it while the user is looking at this one"
        );

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

    /// Two photos whose stems match want the same sidecar file and the same
    /// sidecar destination, because the `_1` suffix only ever lands on the photo
    /// itself. `photo.jpg` and `photo.jpeg` beside each other is the realistic
    /// version.
    ///
    /// Before the batch-wide claim, both planned `photo.xmp` as their sidecar
    /// and the second attempt to place it failed — "does not exist" for a Move,
    /// having been carried off by the first, or "refusing to overwrite" for a
    /// Copy. The status line then named a sidecar as broken when it had in fact
    /// travelled with the first photo.
    #[test]
    fn test_one_sidecar_travels_with_one_photo_and_both_still_land() {
        for mode in [TransferMode::Move, TransferMode::Copy] {
            let root = temp_root(&format!("shared_sidecar_{mode:?}").to_lowercase());
            let src = root.join("photos");
            let out = root.join("library");
            fs::create_dir_all(&src).unwrap();
            write_file(&src.join("photo.jpg"), b"the original");
            write_file(&src.join("photo.jpeg"), b"the same photo, other extension");
            write_file(&src.join("photo.xmp"), b"<xmp/>");

            let engine = ExecutionEngine::new(out.clone(), mode);
            let inputs = vec![
                nature_input(src.join("photo.jpg")),
                nature_input(src.join("photo.jpeg")),
            ];
            let plan = engine.plan_batch(&inputs);
            let claimed: Vec<&Path> = plan
                .iter()
                .flat_map(|op| op.sidecars.iter().map(|s| s.source.as_path()))
                .collect();
            assert_eq!(
                claimed,
                vec![src.join("photo.xmp").as_path()],
                "{mode:?}: the sidecar is claimed once"
            );

            let journal = engine.execute_batch(&plan, |_, _, _| {});
            assert_eq!(journal.completed_ops.len(), 2, "{mode:?}");
            assert!(
                journal.failed_ops.is_empty(),
                "{mode:?}: nothing failed, so {:?}",
                journal.failed_ops
            );
            assert!(
                out.join("Nature/2024/05/photo.xmp").exists(),
                "{mode:?}: the sidecar travelled with the photo that claimed it"
            );

            let _ = fs::remove_dir_all(&root);
        }
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
        assert_eq!(
            journal.completed_ops[0].filed,
            journal.completed_ops[0]
                .filed
                .filter(|f| f.still_matches(&journal.completed_ops[0].destination)),
            "the journalled fingerprint must be the one the photo landed with"
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

    /// The cross-device Move path cannot be reached end to end on one
    /// filesystem, but the step that makes a `Move` a *move* can be: renaming
    /// out of a read-only directory fails while reading the file does not, so
    /// `place` falls through to copy-then-remove exactly as it does across
    /// devices, and the unlink is refused.
    ///
    /// The file is in both places afterwards, and the caller has to be told —
    /// the whole point being that the transfer is not reported as a failure,
    /// because the copy is there and undo still has to be able to reverse it.
    #[test]
    fn test_place_warns_when_a_move_cannot_remove_the_original() {
        let root = temp_root("place_move_unremovable");
        let src = root.join("photos");
        let to = root.join("library/Nature/2024/05/photo.jpg");
        fs::create_dir_all(&src).unwrap();
        write_file(&src.join("photo.jpg"), b"image data");

        let photo = src.join("photo.jpg");
        set_dir_read_only(&src, true);
        let placed = place(&photo, &to, TransferMode::Move);
        set_read_only_bit(&src, false);

        let placed = placed.expect("the copy landed, so the transfer happened");
        assert!(placed.warning.is_some(), "{placed:?}");
        let warning = placed.warning.unwrap();
        assert!(warning.contains("both places"), "{warning}");
        assert!(photo.exists(), "the original could not be unlinked");
        assert_eq!(fs::read(&to).unwrap(), b"image data", "the copy is there");

        // The fingerprint is of the copy, which is what the journal records.
        assert!(placed.fingerprint.still_matches(&to));

        let _ = fs::remove_dir_all(&root);
    }

    /// Makes a directory read-only, which refuses `rename` and `unlink` into and
    /// out of it while leaving reads alone — the permission shape the test above
    /// needs.
    ///
    /// Whether the bit was honoured is confirmed rather than assumed: as root it
    /// is advisory, and a test that quietly stopped testing anything would be
    /// worse than one that fails.
    fn set_dir_read_only(dir: &Path, read_only: bool) {
        let probe = dir.join("probe");
        // Written before the bit is set: a read-only directory refuses creation
        // as well as unlinking, and this is a probe of the unlink.
        fs::write(&probe, b"probe").unwrap();
        set_read_only_bit(dir, read_only);
        let refused = fs::remove_file(&probe).is_err();
        assert_eq!(
            refused,
            read_only,
            "{} does not honour its read-only bit; this filesystem cannot provoke the \
             unlink failure this test is about",
            dir.display()
        );
    }

    fn set_read_only_bit(dir: &Path, read_only: bool) {
        let mut perms = fs::metadata(dir).unwrap().permissions();
        perms.set_readonly(read_only);
        fs::set_permissions(dir, perms).unwrap();
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

    /// Every path starts with an empty one — `Path::starts_with("")` is true for
    /// anything — so an empty `output_dir` is not a bound at all: without the
    /// guard the walk climbed `Nature/2024/05` → `2024` → `Nature` → `library` →
    /// `out` and kept going for as far as the folders stayed empty.
    ///
    /// Only empty directories go, so this was never a data-loss bug, but it is
    /// the user's empty folder tree, and it stopped at the filesystem root rather
    /// than anywhere in particular. A journal can only get an empty `output_dir`
    /// from something other than this app; the cost of finding that out was not
    /// worth it.
    #[test]
    fn test_prune_empty_dirs_stops_at_an_empty_output_folder() {
        let root = temp_root("prune_no_output_dir");
        let filed = root.join("library/Nature/2024/05/photo.jpg");
        write_file(&filed, b"image data");
        // Also an empty sibling, to show the walk is not merely stopping early
        // because it hit something non-empty.
        fs::create_dir_all(root.join("elsewhere/2024")).unwrap();

        fs::remove_file(&filed).unwrap();
        prune_empty_dirs(&filed, Path::new(""));

        assert!(
            root.join("library/Nature/2024/05").is_dir(),
            "nothing pruned"
        );
        assert!(root.join("elsewhere/2024").is_dir(), "nothing above pruned");

        let _ = fs::remove_dir_all(&root);
    }
}
