//! Driving a folder scan and folding its results back into the staged items.
//!
//! The scanner runs on its own thread and streams `ScanEvent`s over a channel;
//! `drain_scan_messages` is what the frame loop calls to apply them without
//! blocking the UI.

use super::PhotoOrganizerApp;
use crate::app::models::StagedItem;
use crate::classification::{Classification, PhotoFacts};
use crate::inference::find_model_path;
use crate::scanner::{scan_folder, ProcessedPayload, ScanConfig, ScanMessage};
use eframe::egui;
use std::path::PathBuf;

impl PhotoOrganizerApp {
    /// Kicks off a background scan of `folder`, discarding anything staged from
    /// a previous one.
    ///
    /// Bumps the scan generation first. A previous scan's messages are already on
    /// the shared channel and cannot be recalled, and were decided against the
    /// profile store as it was then; the tag keeps one scan's late answer from
    /// overwriting another's photo.
    pub(super) fn start_scan(&mut self, ctx: egui::Context, folder: PathBuf) {
        self.items.clear();
        self.status_message = None;
        self.is_processing = true;

        // The filters go, the sort stays. A filter was chosen against the photos
        // that were on screen, and there are none now: carried over, "only 2020
        // receipts above 80% confidence" would silently hide an entire new folder
        // behind a question the user answered about a different one, with nothing
        // on screen to say why. The sort is a standing preference about how to
        // read a grid rather than a statement about its contents, so it survives.

        self.filters = crate::app::models::Filters::default();
        // This scan classifies against the threshold in force now, so the grid
        // starts out agreeing with the slider. That also discharges whatever a
        // held re-classification was owed: it was owed for the photos just
        // discarded, and there is nothing left half-sorted for it to fix.
        self.classified_threshold = self.settings.confidence_threshold;
        self.pending_reclassify = false;
        self.scan_id += 1;
        let tx = self.tx.clone();
        let profiles = self.profiles.clone();

        // Resolved once here rather than inside the scan: every worker would
        // otherwise run the same model search to reach the same answer.
        let config = ScanConfig {
            threshold: self.settings.confidence_threshold,
            model_path: find_model_path(self.settings.model_path.as_deref()),
        };

        scan_folder(folder, profiles, config, tx, ctx, self.scan_id);
    }

    /// Applies every scan message queued since the last frame, ignoring any a
    /// superseded scan sent.
    pub(super) fn drain_scan_messages(&mut self, ctx: &egui::Context) {
        while let Ok(event) = self.rx.try_recv() {
            if event.scan_id != self.scan_id {
                continue;
            }
            match event.message {
                ScanMessage::Item(payload) => self.stage_scanned_item(ctx, payload),
                ScanMessage::Update {
                    facts,
                    classification,
                } => self.apply_scan_update(facts, classification),
                ScanMessage::Complete => {
                    self.is_processing = false;
                    // A threshold moved mid-scan leaves the grid sorted against
                    // two bars: the photos that arrived after the change were
                    // classified against the one the scan started with. Settle on
                    // the value the user can see now.
                    if std::mem::take(&mut self.pending_reclassify) {
                        self.reclassify_all();
                    }
                }
            }
        }
    }

    /// Appends a newly scanned photo, selected and ready to review.
    fn stage_scanned_item(&mut self, ctx: &egui::Context, payload: ProcessedPayload) {
        let filename = payload
            .facts
            .path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy();
        let texture = ctx.load_texture(filename, payload.image, egui::TextureOptions::LINEAR);

        self.items.push(StagedItem::new(
            &self.profiles,
            payload.facts,
            payload.classification,
            texture,
        ));
    }

    /// Applies a scan's decision to the photo it is about, if it is still staged.
    ///
    /// Only finds the item: a scan's answer is keyed by path, the grid holds
    /// photos by position, and the write is
    /// [`StagedItem::apply_classification`](crate::app::models::StagedItem::apply_classification).
    fn apply_scan_update(&mut self, facts: PhotoFacts, classification: Classification) {
        let Some(item) = self.items.iter_mut().find(|i| i.source_path == facts.path) else {
            return;
        };
        item.apply_classification(&self.profiles, &facts, classification);
    }
}
