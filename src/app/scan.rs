//! Driving a folder scan and folding its results back into the staged items.
//!
//! The scanner runs on its own thread and streams `ScanMessage`s over a
//! channel; `drain_scan_messages` is what the frame loop calls to apply them
//! without blocking the UI.

use super::PhotoOrganizerApp;
use crate::app::models::StagedItem;
use crate::inference::find_model_path;
use crate::profile_store::ClassificationSource;
use crate::scanner::{scan_folder, ProcessedPayload, ScanConfig, ScanMessage};
use eframe::egui;
use std::path::PathBuf;

impl PhotoOrganizerApp {
    /// Kicks off a background scan of `folder`, discarding anything staged
    /// from a previous one.
    pub(super) fn start_scan(&mut self, ctx: egui::Context, folder: PathBuf) {
        self.items.clear();
        self.status_message = None;
        self.is_processing = true;
        // This scan classifies against the threshold in force now, so the grid
        // starts out agreeing with the slider. That also discharges whatever a
        // held re-classification was owed: it was owed for the photos just
        // discarded, and there is nothing left half-sorted for it to put right.
        self.classified_threshold = self.settings.confidence_threshold;
        self.pending_reclassify = false;
        let tx = self.tx.clone();
        let profiles = self.profiles.clone();

        // Resolved here, once, rather than inside the scan: every worker would
        // otherwise run the same model search to reach the same answer.
        let config = ScanConfig {
            threshold: self.settings.confidence_threshold,
            model_path: find_model_path(self.settings.model_path.as_deref()),
        };

        scan_folder(folder, profiles, config, tx, ctx);
    }

    /// Applies every scan message queued since the last frame.
    pub(super) fn drain_scan_messages(&mut self, ctx: &egui::Context) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                ScanMessage::Item(payload) => self.stage_scanned_item(ctx, payload),
                ScanMessage::Update {
                    source_path,
                    year,
                    month,
                    is_exif,
                    category,
                    confidence,
                    source,
                    embedding,
                } => self.apply_scan_update(
                    source_path,
                    year,
                    month,
                    is_exif,
                    category,
                    confidence,
                    source,
                    embedding,
                ),
                ScanMessage::Complete => {
                    self.is_processing = false;
                    // A threshold moved while this scan was running leaves the
                    // grid sorted against two different bars: the photos that
                    // arrived after the change were classified by the scan
                    // against the threshold it started with, and the ones
                    // before it by the new one. Settle on the value the user
                    // can see now.
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
            .source_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy();
        let texture = ctx.load_texture(filename, payload.image, egui::TextureOptions::LINEAR);
        let is_custom = Self::is_custom_category(&self.profiles, &payload.category);

        self.items.push(StagedItem {
            source_path: payload.source_path,
            year: payload.year,
            month: payload.month,
            is_exif: payload.is_exif,
            category: payload.category,
            confidence: payload.confidence,
            source: payload.source,
            embedding: payload.embedding,
            texture,
            selected: true,
            is_custom,
        });
    }

    /// Applies a re-classification for an already staged photo.
    ///
    /// A manual category is the user's call and keeps its name, but the
    /// embedding still updates so later training uses the current model.
    #[allow(clippy::too_many_arguments)]
    fn apply_scan_update(
        &mut self,
        source_path: PathBuf,
        year: u32,
        month: u32,
        is_exif: bool,
        category: String,
        confidence: f32,
        source: ClassificationSource,
        embedding: Vec<f32>,
    ) {
        if let Some(item) = self.items.iter_mut().find(|i| i.source_path == source_path) {
            if item.source != ClassificationSource::Manual {
                item.year = year;
                item.month = month;
                item.is_exif = is_exif;
                item.category = category.clone();
                item.confidence = confidence;
                item.source = source;
                item.embedding = embedding;
                item.is_custom = Self::is_custom_category(&self.profiles, &category);
            } else {
                item.embedding = embedding;
            }
        }
    }
}
