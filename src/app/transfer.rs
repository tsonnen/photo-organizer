//! Moving and copying staged photos into the output folder, and the undo that
//! reverses a transfer from its journal.
//!
//! Everything that touches the filesystem here goes through
//! [`crate::transfer`], which owns the on-disk layout. This module owns what the
//! user is told about it: both the journal write and the undo run report their
//! failures, because a transfer that quietly fails to record itself leaves Undo
//! pointing at the wrong batch.

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
        let Some(out_dir) = self.output_folder.clone() else {
            self.set_warning("Pick an output folder first — a transfer has nowhere to land.");
            return;
        };
        if !self.items.iter().any(|i| i.selected) {
            self.set_warning("Select at least one photo to transfer.");
            return;
        }

        let inputs: Vec<RawPhotoInput> = self
            .items
            .iter()
            .filter(|i| i.selected)
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

        let engine = ExecutionEngine::new(out_dir.clone(), mode);
        let plan = engine.plan_batch(&inputs);
        let journal = engine.execute_batch(&plan, |curr, total, _| {
            println!("Transferring {curr}/{total}");
        });

        // Only the photos that actually landed leave the grid. A photo that
        // failed stays selected in place, so the reason is still on screen to be
        // read and the retry is one click away.
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
    /// The journal is a single slot that undo reads as "the last batch". If this
    /// batch's journal cannot be written, leaving the previous one in place would
    /// make Undo reverse an older batch while the user is looking at the newest
    /// one — so the stale journal goes and the user is told that this batch
    /// cannot be undone. The alternative is a confusing undo; this is a visible
    /// one.
    fn persist_journal(journal: &TransferJournal) -> Option<String> {
        let path = Path::new(LAST_JOURNAL);
        match journal.save_to(path) {
            Ok(()) => None,
            Err(e) => {
                let _ = std::fs::remove_file(path);
                Some(format!(
                    "The undo journal could not be written ({e}), so this batch cannot be undone."
                ))
            }
        }
    }

    /// One line describing what undo did, with the first thing it left for the
    /// user spelled out.
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
            // A restored photo is on disk in the source folder but no longer in
            // the grid, and re-staging it means a rescan.
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
