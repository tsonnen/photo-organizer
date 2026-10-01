//! The egui application: state, the frame loop, and the panels it draws.
//!
//! This module holds the app struct itself and wires the pieces together.
//! The work lives in submodules:
//!
//! - [`scan`] runs a folder scan and folds results into staged items
//! - [`grid`] the thumbnail grid
//! - [`modal`] the inspection modal
//! - [`categories`] classification, training and the category widgets
//! - [`toolbar`] the top panel
//! - [`transfer`] move/copy and undo
//! - [`layout`] grid and control sizing arithmetic
//! - [`models`] the plain data records the widgets render

mod categories;
mod grid;
mod layout;
mod modal;
mod models;
mod scan;
mod toolbar;
mod transfer;

#[cfg(test)]
mod layout_tests;

use crate::inference::is_model_available;
use crate::profile_store::ProfileStore;
use crate::scanner::ScanMessage;
use eframe::egui;
use models::{ModalPreview, StagedItem};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};

pub struct PhotoOrganizerApp {
    input_folder: Option<PathBuf>,
    output_folder: Option<PathBuf>,
    items: Vec<StagedItem>,
    is_processing: bool,
    profiles: ProfileStore,
    model_available: bool,
    show_categories_panel: bool,
    target_training_category: String,
    status_message: Option<(String, egui::Color32)>,
    tx: Sender<ScanMessage>,
    rx: Receiver<ScanMessage>,
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
        let model_available = is_model_available();

        Self {
            input_folder: None,
            output_folder: None,
            items: Vec::new(),
            is_processing: false,
            profiles,
            model_available,
            show_categories_panel: false,
            target_training_category: String::new(),
            status_message: None,
            tx,
            rx,
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

        self.render_toolbar(ctx);
        self.render_grid(ctx);
        self.render_modal(ctx);
    }
}
