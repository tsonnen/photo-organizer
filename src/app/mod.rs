//! The egui application: state, the frame loop, and the panels it draws.
//!
//! This module holds the app struct itself and wires the pieces together.
//! The work lives in submodules:
//!
//! - [`scan`] runs a folder scan and folds results into staged items
//! - [`grid`] the thumbnail grid
//! - [`modal`] the inspection modal
//! - [`profiles_modal`] the scrollable profile list and its two-step delete
//! - [`settings_modal`] the confidence threshold, output folder and model path
//! - [`chrome`] the backdrop and card every modal shares
//! - [`categories`] classification, training and the category widgets
//! - [`toolbar`] the top panel and its menu bar
//! - [`footer`] the bottom panel: selection count and transfer destination
//! - [`transfer`] move/copy and undo
//! - [`layout`] grid, control and modal sizing arithmetic
//! - [`models`] the staged-item record and the single write path onto it

mod categories;
mod chrome;
mod footer;
mod grid;
mod layout;
mod modal;
mod models;
mod profiles_modal;
mod scan;
mod settings_modal;
mod toolbar;
mod transfer;

#[cfg(test)]
mod layout_tests;

use crate::inference::is_model_available;
use crate::profile_store::ProfileStore;
use crate::scanner::ScanEvent;
use crate::settings::Settings;
use eframe::egui;
use models::{ModalPreview, StagedItem};
use profiles_modal::DeletePrompt;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};

pub struct PhotoOrganizerApp {
    input_folder: Option<PathBuf>,
    items: Vec<StagedItem>,
    is_processing: bool,
    profiles: ProfileStore,
    /// The user's configuration, from `settings.json`. The output folder and
    /// the model path live here rather than as their own fields, so there is
    /// one place a setting is read from and one place it is written to.
    settings: Settings,
    /// Cached from the settings' model path at startup and recomputed when the
    /// user points the app at a different checkpoint.
    model_available: bool,
    /// The threshold the staged photos were last classified at.
    ///
    /// The threshold slider applies a pointer position on the press frame and
    /// on every frame the handle travels, so by the time the drag *stops* the
    /// value has already settled and the release frame looks like no change at
    /// all. Committing therefore means "settled at a value other than this one",
    /// not "changed this frame". Kept in step by [`Self::reclassify_all`],
    /// which is every place the grid's classifications are rewritten.
    classified_threshold: f32,
    /// Set when the threshold moved mid-scan, so the photos still arriving —
    /// classified by the scan against the threshold it started with — are
    /// re-classified once it finishes instead of being left mixed.
    pending_reclassify: bool,
    show_categories_panel: bool,
    show_profiles_modal: bool,
    show_settings_modal: bool,
    delete_prompt: DeletePrompt,
    target_training_category: String,
    status_message: Option<(String, egui::Color32)>,
    tx: Sender<ScanEvent>,
    rx: Receiver<ScanEvent>,
    /// Which scan is current. Messages from any other are dropped on arrival:
    /// a scan that has been superseded cannot be unsent, and its answer was
    /// decided against the profile store as it was at the time.
    scan_id: u64,
    modal_preview: Option<ModalPreview>,
    high_res_tx: Sender<(PathBuf, egui::ColorImage)>,
    high_res_rx: Receiver<(PathBuf, egui::ColorImage)>,
}

impl Default for PhotoOrganizerApp {
    fn default() -> Self {
        Self::new()
    }
}

impl PhotoOrganizerApp {
    pub fn new() -> Self {
        let (tx, rx) = channel();
        let (high_res_tx, high_res_rx) = channel();
        let profiles = ProfileStore::load_from_file("profiles.json")
            .unwrap_or_else(|_| ProfileStore::default());

        // Settings first: the model path the user configured decides which
        // checkpoint the app looks for, so asking before loading them would
        // always answer with the auto-detect guess.
        let settings = Settings::load_from_file("settings.json");
        let model_available = is_model_available(settings.model_path.as_deref());
        let classified_threshold = settings.confidence_threshold;

        Self {
            input_folder: None,
            items: Vec::new(),
            is_processing: false,
            profiles,
            settings,
            model_available,
            classified_threshold,
            pending_reclassify: false,
            show_categories_panel: false,
            show_profiles_modal: false,
            show_settings_modal: false,
            delete_prompt: DeletePrompt::default(),
            target_training_category: String::new(),
            status_message: None,
            tx,
            rx,
            scan_id: 0,
            modal_preview: None,
            high_res_tx,
            high_res_rx,
        }
    }

    /// Selects or clears every staged photo.
    fn set_all_selected(&mut self, selected: bool) {
        for item in &mut self.items {
            item.selected = selected;
        }
    }

    /// Persists the profile store, ignoring a failed write: the store stays
    /// usable in memory and the user is not blocked by a read-only folder.
    fn save_profiles(&self) {
        let _ = self.profiles.save_to_file("profiles.json");
    }

    /// Persists the settings, ignoring a failed write: they stay in effect for
    /// this run and the user is not blocked by a read-only folder.
    fn save_settings(&self) {
        let _ = self.settings.save_to_file("settings.json");
    }

    /// Deletes a category profile and re-classifies the staged photos.
    fn remove_profile(&mut self, category: &str) {
        self.profiles.remove_category(category);
        self.save_profiles();
        self.reclassify_all();
    }

    /// Shows a message in the status line.
    fn set_status(&mut self, message: impl Into<String>, color: egui::Color32) {
        self.status_message = Some((message.into(), color));
    }

    /// Shows a message for something the user needs to fix before proceeding.
    fn set_warning(&mut self, message: impl Into<String>) {
        self.set_status(message, egui::Color32::from_rgb(240, 180, 0));
    }
}

impl eframe::App for PhotoOrganizerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Fold in background work first, so this frame renders the latest
        // classifications and any full-resolution image that just arrived.
        self.drain_high_res(ctx);
        self.drain_scan_messages(ctx);

        // Panel order matters: egui hands the central panel whatever the top and
        // bottom panels leave, so both have to be placed before the grid is
        // drawn or it would be laid out against the wrong height.
        self.render_toolbar(ctx);
        self.render_footer(ctx);
        self.render_grid(ctx);

        self.render_modal(ctx);
        self.render_profiles_modal(ctx);
        self.render_settings_modal(ctx);
    }
}
