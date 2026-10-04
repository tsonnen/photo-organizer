//! Moving and copying staged photos into the output folder, and the undo that
//! reverses a transfer from its journal.
//!
//! [`crate::transfer`] owns the filesystem; this module owns what the user is told
//! about it. The journal write and the undo run both report their failures,
//! because a transfer that quietly fails to record itself leaves Undo pointing at
//! the wrong batch.

use super::PhotoOrganizerApp;
use crate::category_name::CategoryName;
use crate::transfer::{
    ExecutionEngine, RawPhotoInput, TransferJournal, TransferMode, UndoStatus, LAST_JOURNAL,
};
use eframe::egui;
use std::collections::HashSet;
use std::path::Path;

impl PhotoOrganizerApp {
    /// Transfers every selected photo, then drops the ones that landed from the
    /// grid.
    pub(super) fn execute_transfer(&mut self, mode: TransferMode) {
        let Some(out_dir) = self.settings.output_folder.clone() else {
            // A backstop, not the feedback path: the toolbar already greys Move
            // and Copy without a destination, so reaching here means some
            // caller has not asked whether it was allowed to. Warn rather than
            // return quietly — a transfer that does nothing must still say so.
            self.set_warning(
                "⚠ No output folder is set. Choose one in Settings before transferring.",
            );
            return;
        };
        if !self.items.iter().any(|i| i.selected) {
            self.set_warning("Select at least one photo to transfer.");
            return;
        }

        // A photo the model has not reached yet holds `CLASSIFYING_LABEL` where
        // its category goes, and `is_filable` is false for exactly that reason: see
        // `StagedItem::is_filable`. Move and Copy are not gated on the scan
        // finishing, so this is reachable by pressing either one mid-scan. What is
        // held stays in the grid, still selected, for the retry.
        let held = self
            .items
            .iter()
            .filter(|i| i.selected && !i.is_filable())
            .count();
        let inputs: Vec<RawPhotoInput> = self
            .items
            .iter()
            .filter(|i| i.selected && i.is_filable())
            .map(|i| RawPhotoInput {
                source_path: i.source_path.clone(),
                // The item's category is a free-text display string the user can
                // retype per photo, so it is sanitised here rather than trusted:
                // this is the last point before it becomes a directory name.
                subject: CategoryName::from_user_input(&i.category),
                year: i.year,
                month: i.month,
            })
            .collect();

        if inputs.is_empty() {
            self.set_warning(
                "⚠ Every selected photo is still being classified. Wait for the scan to finish, \
                 then press again.",
            );
            return;
        }

        let engine = ExecutionEngine::new(out_dir.clone(), mode);
        let plan = engine.plan_batch(&inputs);
        let journal = engine.execute_batch(&plan, |curr, total, _| {
            println!("Transferring {curr}/{total}");
        });

        // Only photos that landed leave the grid. A failed one stays selected in
        // place, so the reason is on screen and the retry is one click away.
        let landed: HashSet<&Path> = journal
            .completed_ops
            .iter()
            .map(|op| op.source.as_path())
            .collect();
        self.items
            .retain(|item| !(item.selected && landed.contains(item.source_path.as_path())));

        let verb = match mode {
            TransferMode::Move => "Moved",
            TransferMode::Copy => "Copied",
        };
        let mut message = match (journal.completed_ops.len(), journal.failed_ops.len()) {
            (0, 0) => "Nothing was transferred.".to_string(),
            (0, _) => format!("Nothing was transferred. {}", first_failure(&journal)),
            (placed, 0) => format!("{verb} {placed} photo(s) into {}.", out_dir.display()),
            (placed, _) => format!(
                "{verb} {placed} of {} photo(s). {}",
                inputs.len(),
                first_failure(&journal)
            ),
        };
        if let Some(note) = Self::persist_journal(&journal) {
            message = format!("{message} {note}");
        }
        if held > 0 {
            message = format!(
                "{message} {held} photo(s) are still being classified and were left in the grid."
            );
        }

        if journal.failed_ops.is_empty() {
            self.set_status(message, egui::Color32::from_rgb(40, 200, 40));
        } else {
            self.set_warning(message);
        }
    }

    /// Rolls back the last transfer, if there is a journal to read.
    pub(super) fn undo_last_transfer(&mut self) {
        let path = Path::new(LAST_JOURNAL);
        if !path.exists() {
            self.set_warning("Nothing to undo: no transfer has been recorded yet.");
            return;
        }

        let journal = match TransferJournal::load_from(path) {
            Ok(journal) => journal,
            Err(e) => {
                self.set_warning(format!("Could not read the undo journal: {e}"));
                return;
            }
        };
        let statuses = match journal.undo(|curr, total, op| {
            println!("Undoing {curr}/{total}: {}", op.destination.display());
        }) {
            Ok(statuses) => statuses,
            Err(e) => {
                self.set_warning(format!("{e}."));
                return;
            }
        };

        for status in &statuses {
            println!("{}", status.describe());
        }
        let message = Self::undo_summary(&statuses);
        if statuses.iter().any(UndoStatus::needs_attention) {
            self.set_warning(message);
        } else {
            self.set_status(message, egui::Color32::from_rgb(40, 200, 40));
        }
    }

    /// Writes the journal, and takes a stale one away if the write fails.
    ///
    /// The journal is a single slot that undo reads as "the last batch", so
    /// leaving the previous one in place would make Undo reverse an older batch
    /// while the user is looking at the newest. The alternative is a confusing
    /// undo; this is a visible one.
    ///
    /// A batch that landed nothing is not written at all, for the same reason
    /// from the other side: there is no new batch to record, and overwriting
    /// would throw away the only record of the last one that did move anything.
    /// The note says which batch Undo is still holding, so it is not a surprise.
    fn persist_journal(journal: &TransferJournal) -> Option<String> {
        Self::persist_journal_to(journal, Path::new(LAST_JOURNAL))
    }

    /// [`PhotoOrganizerApp::persist_journal`] against a given path, so the policy
    /// can be tested without writing into the process working directory — the
    /// reason the settings write policy is unit-tested rather than read back off
    /// the file too.
    fn persist_journal_to(journal: &TransferJournal, path: &Path) -> Option<String> {
        if journal.completed_ops.is_empty() {
            return Some("Undo still refers to the batch before this one.".to_string());
        }
        match journal.save_as_last_batch(path) {
            Ok(()) => None,
            Err(e) => Some(format!(
                "The undo journal could not be written ({e}), so this batch cannot be undone."
            )),
        }
    }

    /// One line describing what undo did, with the first thing it left for the user
    /// spelled out.
    fn undo_summary(statuses: &[UndoStatus]) -> String {
        let restored = statuses
            .iter()
            .filter(|s| matches!(s, UndoStatus::Restored(_)))
            .count();
        let removed = statuses
            .iter()
            .filter(|s| matches!(s, UndoStatus::Removed(_)))
            .count();
        let already_gone = statuses
            .iter()
            .filter(|s| matches!(s, UndoStatus::SkippedMissing(_)))
            .count();
        let attention = statuses.iter().filter(|s| s.needs_attention()).count();

        let mut parts = Vec::new();
        if restored > 0 {
            parts.push(format!("{restored} moved back"));
        }
        if removed > 0 {
            parts.push(format!("{removed} copies removed"));
        }
        if already_gone > 0 {
            parts.push(format!("{already_gone} already gone"));
        }

        let mut message = if parts.is_empty() {
            "Undo: nothing to reverse.".to_string()
        } else {
            format!("Undo: {}.", parts.join(", "))
        };
        if restored > 0 {
            // A restored photo is on disk but no longer in the grid; re-staging it
            // means a rescan.
            message.push_str(" Re-scan the source folder to see them again.");
        }
        match statuses.iter().find(|s| s.needs_attention()) {
            Some(first) if attention > 1 => message.push_str(&format!(
                " {attention} files need a hand, e.g. {}",
                first.describe()
            )),
            Some(first) => message.push_str(&format!(" {}", first.describe())),
            None => {}
        }
        message
    }
}

/// The first thing that went wrong, in a sentence.
fn first_failure(journal: &TransferJournal) -> String {
    match journal.failed_ops.first() {
        Some(failure) => failure.describe(),
        None => "Nothing failed.".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transfer::FileOperation;

    /// A journal with nothing in it, which is what a batch that landed no
    /// photos produces.
    fn empty_journal() -> TransferJournal {
        TransferJournal {
            mode: Some(TransferMode::Copy),
            ..Default::default()
        }
    }

    /// A journal with one landed photo, so it is a batch worth recording.
    fn journal_with_one_photo() -> TransferJournal {
        let mut journal = empty_journal();
        journal.completed_ops.push(FileOperation {
            source: Path::new("/photos/photo.jpg").into(),
            destination: Path::new("/library/Nature/2024/05/photo.jpg").into(),
            filed: None,
            sidecars: Vec::new(),
        });
        journal
    }

    /// Undo reads whatever is at `LAST_JOURNAL` as "the last batch", so the slot
    /// is worth more than the file it holds: a batch that landed nothing must not
    /// overwrite the record of the batch that did, or the photos the user moved
    /// an hour ago become unundoable because an unrelated transfer failed.
    ///
    /// The note is not decoration either — without it, "Undo still refers to the
    /// batch before this one" is a surprise rather than an explanation.
    #[test]
    fn test_a_batch_that_landed_nothing_keeps_the_previous_journal() {
        let dir = std::env::temp_dir().join(format!("app_journal_empty_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(crate::transfer::LAST_JOURNAL);

        // The earlier batch's journal, as `execute_transfer` would have left it.
        journal_with_one_photo().save_to(&path).unwrap();
        let before = std::fs::read(&path).unwrap();

        let note = PhotoOrganizerApp::persist_journal_to(&empty_journal(), &path)
            .expect("the user is told which batch Undo still holds");
        assert!(
            note.contains("batch before"),
            "the note has to say what Undo now refers to: {note}"
        );
        assert_eq!(
            std::fs::read(&path).unwrap(),
            before,
            "the previous batch's journal survives a batch that landed nothing"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The other half of the same policy: a batch that did land something is
    /// recorded, and a write that fails says so rather than passing for success.
    #[test]
    fn test_a_batch_that_landed_photos_is_recorded() {
        let dir = std::env::temp_dir().join(format!("app_journal_written_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(crate::transfer::LAST_JOURNAL);

        assert_eq!(
            PhotoOrganizerApp::persist_journal_to(&journal_with_one_photo(), &path),
            None
        );
        let written = TransferJournal::load_from(&path).expect("the journal is readable");
        assert_eq!(written.completed_ops.len(), 1);

        // A write that cannot land is reported, and the stale journal goes with
        // it, because a journal describing an older batch is worse than none. A
        // directory where the staging file goes is the one failure that leaves
        // the folder writable, so the removal is observable rather than refused
        // for the same reason as the write.
        std::fs::create_dir_all(path.with_extension("json.staging")).unwrap();
        let note = PhotoOrganizerApp::persist_journal_to(&journal_with_one_photo(), &path)
            .expect("a failed write is not silent");
        assert!(note.contains("cannot be undone"), "{note}");
        assert!(
            !path.exists(),
            "the stale journal is taken with the failure"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The status line is the only feedback a transfer gives, and it is built
    /// from the journal's counts. A caveat now shares `failed_ops` with a real
    /// failure, so "N of M" must stay true when a photo landed with a warning on
    /// it: the counts come from the two lists independently.
    #[test]
    fn test_a_warning_counts_as_a_landed_photo_that_also_failed() {
        let mut journal = journal_with_one_photo();
        journal.failed_ops.push(crate::transfer::FailedOp {
            operation: FileOperation {
                source: Path::new("/photos/photo.jpg").into(),
                destination: Path::new("/photos/photo.jpg").into(),
                filed: None,
                sidecars: Vec::new(),
            },
            error: "the original could not be removed; that file is now in both places".to_string(),
        });

        let message = format!(
            "Moved {} of {} photo(s). {}",
            journal.completed_ops.len(),
            1,
            first_failure(&journal)
        );
        assert!(
            message.starts_with("Moved 1 of 1 photo(s)."),
            "the photo landed, so it is counted as landed: {message}"
        );
        assert!(
            message.contains("both places"),
            "and the user is still told what happened to it: {message}"
        );
    }
}
