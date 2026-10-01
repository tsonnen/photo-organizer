//! Moving and copying staged photos into the output folder, plus the undo
//! that reverses a transfer from its manifest.

use super::PhotoOrganizerApp;
use crate::execution_engine::{ExecutionEngine, RawPhotoInput, TransferMode};
use crate::undo_engine::UndoEngine;
use std::fs;

impl PhotoOrganizerApp {
    /// Transfers every selected photo, then drops them from the grid.
    ///
    /// Does nothing without an output folder: a transfer has nowhere to land.
    pub(super) fn execute_transfer(&mut self, mode: TransferMode) {
        let Some(out_dir) = self.output_folder.clone() else {
            return;
        };

        let inputs: Vec<RawPhotoInput> = self
            .items
            .iter()
            .filter(|i| i.selected)
            .map(|i| RawPhotoInput {
                source_path: i.source_path.clone(),
                subject: i.category.clone(),
                year: i.year,
                month: i.month,
            })
            .collect();

        let engine = ExecutionEngine::new(out_dir, mode);
        let plan = engine.plan_batch(&inputs);
        let manifest = engine.execute_batch(&plan, |curr, total, _| {
            println!("Executing {curr}/{total}");
        });

        let _ = fs::write(
            "last_execution_manifest.json",
            serde_json::to_string_pretty(&manifest).unwrap(),
        );
        self.items.retain(|i| !i.selected);
    }

    /// Rolls back the last transfer, if there is a manifest to read.
    pub(super) fn undo_last_transfer(&mut self) {
        let _ = UndoEngine::rollback_from_file("last_execution_manifest.json", |_, _, _| {});
    }
}
