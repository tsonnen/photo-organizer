//! Plain data the UI renders. No behaviour lives here, just the records that
//! flow between the scanner, the profile store and the widgets.

use crate::profile_store::ClassificationSource;
use eframe::egui;
use std::path::PathBuf;

/// One scanned photo staged for review, with its classification and thumbnail.
pub struct StagedItem {
    pub source_path: PathBuf,
    pub year: u32,
    pub month: u32,
    pub is_exif: bool,
    pub category: String,
    pub confidence: f32,
    pub source: ClassificationSource,
    pub embedding: Vec<f32>,
    pub texture: egui::TextureHandle,
    pub selected: bool,
    pub is_custom: bool,
}

/// State of the inspection modal: which item it shows and, once the background
/// thread has delivered it, the full-resolution texture.
pub struct ModalPreview {
    pub item_index: usize,
    pub high_res_texture: Option<egui::TextureHandle>,
    pub high_res_path: Option<PathBuf>,
    pub is_loading: bool,
}

/// Which control in the inspection modal's bottom row was pressed.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ModalActions {
    pub prev: bool,
    pub next: bool,
    pub train: bool,
    pub close: bool,
}
