mod app;
mod db;
mod execution_engine;
mod inference;
mod media;
mod profile_store;
mod scanner;
mod undo_engine;

use app::PhotoOrganizerApp;
use eframe::egui;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1240.0, 900.0])
            .with_min_inner_size([1240.0, 900.0])
            .with_title("Photo Organizer - Integrated Suite"),
        ..Default::default()
    };
    eframe::run_native(
        "Photo Organizer",
        options,
        Box::new(|_cc| Ok(Box::new(PhotoOrganizerApp::new()))),
    )
}
