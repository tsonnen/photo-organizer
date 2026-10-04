//! Driving a folder scan and folding its results back into the staged items.
//!
//! The scanner runs on its own thread and streams `ScanMessage`s over a
//! channel; `drain_scan_messages` is what the frame loop calls to apply them
//! without blocking the UI.

use super::PhotoOrganizerApp;
use crate::app::models::StagedItem;
use crate::classification::{Classification, PhotoFacts};
use crate::scanner::{scan_folder, ProcessedPayload, ScanMessage};
use eframe::egui;
use std::path::PathBuf;

impl PhotoOrganizerApp {
    /// Kicks off a background scan of `folder`, discarding anything staged
    /// from a previous one.
    ///
    /// Bumps the scan generation first. A previous scan's messages are already
    /// on the shared channel and cannot be recalled, and they were decided
    /// against the profile store as it was then; the tag is what keeps one
    /// scan's late answer from overwriting another's photo.
    pub(super) fn start_scan(&mut self, ctx: egui::Context, folder: PathBuf) {
        self.items.clear();
        self.status_message = None;
        self.is_processing = true;
        self.scan_id += 1;
        let tx = self.tx.clone();
        let profiles = self.profiles.clone();
        scan_folder(folder, profiles, tx, ctx, self.scan_id);
    }

    /// Applies every scan message queued since the last frame, ignoring any that
    /// a superseded scan sent.
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
                ScanMessage::Complete => self.is_processing = false,
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
    /// The write itself is `StagedItem::apply_classification`, shared with
    /// **Re-classify All**; this only finds the item, because a scan's answer
    /// is keyed by path and the grid holds photos by position.
    fn apply_scan_update(&mut self, facts: PhotoFacts, classification: Classification) {
        let Some(item) = self.items.iter_mut().find(|i| i.source_path == facts.path) else {
            return;
        };
        item.apply_classification(&self.profiles, &facts, classification);
    }
}
