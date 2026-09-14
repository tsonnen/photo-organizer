use crate::execution_engine::{ExecutionEngine, RawPhotoInput, TransferMode};
use crate::profile_store::ProfileStore;
use crate::scanner::{scan_folder, ScanMessage};
use crate::undo_engine::UndoEngine;
use eframe::egui;
use std::fs;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};

pub struct StagedItem {
    pub source_path: PathBuf,
    pub year: u32,
    pub month: u32,
    #[allow(dead_code)]
    pub is_exif: bool,
    pub category: String,
    #[allow(dead_code)]
    pub confidence: f32,
    pub texture: egui::TextureHandle,
    pub selected: bool,
}

pub struct PhotoOrganizerApp {
    input_folder: Option<PathBuf>,
    output_folder: Option<PathBuf>,
    items: Vec<StagedItem>,
    is_processing: bool,
    profiles: ProfileStore,
    tx: Sender<ScanMessage>,
    rx: Receiver<ScanMessage>,
}

impl Default for PhotoOrganizerApp {
    fn default() -> Self {
        Self::new()
    }
}

impl PhotoOrganizerApp {
    pub fn new() -> Self {
        let (tx, rx) = channel();
        Self {
            input_folder: None,
            output_folder: None,
            items: Vec::new(),
            is_processing: false,
            profiles: ProfileStore::default(),
            tx,
            rx,
        }
    }

    fn start_scan(&mut self, ctx: egui::Context, folder: PathBuf) {
        self.items.clear();
        self.is_processing = true;
        let tx = self.tx.clone();
        let profiles = self.profiles.clone();
        scan_folder(folder, profiles, tx, ctx);
    }

    fn execute_transfer(&mut self, mode: TransferMode) {
        let out_dir = match &self.output_folder {
            Some(p) => p.clone(),
            None => return,
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
            println!("Executing {}/{}", curr, total);
        });

        let _ = fs::write(
            "last_execution_manifest.json",
            serde_json::to_string_pretty(&manifest).unwrap(),
        );
        self.items.retain(|i| !i.selected);
    }
}

impl eframe::App for PhotoOrganizerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                ScanMessage::Item(payload) => {
                    let filename = payload
                        .source_path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy();
                    let texture =
                        ctx.load_texture(filename, payload.image, egui::TextureOptions::LINEAR);

                    self.items.push(StagedItem {
                        source_path: payload.source_path,
                        year: payload.year,
                        month: payload.month,
                        is_exif: payload.is_exif,
                        category: payload.category,
                        confidence: payload.confidence,
                        texture,
                        selected: true,
                    });
                }
                ScanMessage::Update {
                    source_path,
                    year,
                    month,
                    is_exif,
                    category,
                    confidence,
                } => {
                    if let Some(item) = self.items.iter_mut().find(|i| i.source_path == source_path)
                    {
                        item.year = year;
                        item.month = month;
                        item.is_exif = is_exif;
                        item.category = category;
                        item.confidence = confidence;
                    }
                }
                ScanMessage::Complete => {
                    self.is_processing = false;
                }
            }
        }

        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Select Source Folder").clicked() {
                    if let Some(p) = rfd::FileDialog::new().pick_folder() {
                        self.input_folder = Some(p.clone());
                        self.start_scan(ctx.clone(), p);
                    }
                }
                if ui.button("Select Output Folder").clicked() {
                    self.output_folder = rfd::FileDialog::new().pick_folder();
                }

                ui.separator();
                if ui.button("Execute Move").clicked() {
                    self.execute_transfer(TransferMode::Move);
                }
                if ui.button("Execute Copy").clicked() {
                    self.execute_transfer(TransferMode::Copy);
                }
                if ui.button("Undo Last Run").clicked() {
                    let _ = UndoEngine::rollback_from_file(
                        "last_execution_manifest.json",
                        |_, _, _| {},
                    );
                }

                if self.is_processing {
                    ui.separator();
                    ui.spinner();
                    ui.label("Processing photos in parallel...");
                }
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                egui::Grid::new("grid")
                    .num_columns(4)
                    .spacing([12.0, 12.0])
                    .show(ui, |ui| {
                        for (idx, item) in self.items.iter_mut().enumerate() {
                            ui.vertical(|ui| {
                                ui.image(&item.texture);
                                ui.checkbox(
                                    &mut item.selected,
                                    item.source_path.file_name().unwrap().to_str().unwrap(),
                                );

                                ui.horizontal(|ui| {
                                    ui.label(format!("{}/{:02}", item.year, item.month));
                                    ui.text_edit_singleline(&mut item.category);
                                });
                            });
                            if (idx + 1) % 4 == 0 {
                                ui.end_row();
                            }
                        }
                    });
            });
        });
    }
}
