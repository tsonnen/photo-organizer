use crate::execution_engine::{ExecutionEngine, RawPhotoInput, TransferMode};
use crate::inference::is_model_available;
use crate::profile_store::{ClassificationSource, ProfileStore};
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
    pub is_exif: bool,
    pub category: String,
    pub confidence: f32,
    pub source: ClassificationSource,
    pub embedding: Vec<f32>,
    pub texture: egui::TextureHandle,
    pub selected: bool,
}

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
}

impl Default for PhotoOrganizerApp {
    fn default() -> Self {
        Self::new()
    }
}

impl PhotoOrganizerApp {
    pub fn new() -> Self {
        let (tx, rx) = channel();
        let profiles =
            ProfileStore::load_from_file("profiles.json").unwrap_or_else(|_| ProfileStore::default());
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
        }
    }

    fn start_scan(&mut self, ctx: egui::Context, folder: PathBuf) {
        self.items.clear();
        self.status_message = None;
        self.is_processing = true;
        let tx = self.tx.clone();
        let profiles = self.profiles.clone();
        scan_folder(folder, profiles, tx, ctx);
    }

    pub fn reclassify_all(&mut self) {
        for item in &mut self.items {
            let width = item.texture.size()[0] as u32;
            let height = item.texture.size()[1] as u32;
            let res = self.profiles.classify_with_heuristics(
                &item.embedding,
                &item.source_path,
                item.is_exif,
                width,
                height,
            );
            item.category = res.category;
            item.confidence = res.confidence;
            item.source = res.source;
        }
    }

    pub fn train_selected_as_category(&mut self, category: &str) {
        let category = category.trim();
        if category.is_empty() {
            self.status_message = Some((
                "Please enter a category name to train.".to_string(),
                egui::Color32::from_rgb(240, 180, 0),
            ));
            return;
        }

        let selected_count = self.items.iter().filter(|i| i.selected).count();
        if selected_count == 0 {
            self.status_message = Some((
                "No photos selected to train. Check at least one photo.".to_string(),
                egui::Color32::from_rgb(240, 180, 0),
            ));
            return;
        }

        let mut trained_count = 0;
        for item in &self.items {
            if item.selected && !item.embedding.is_empty() {
                self.profiles.add_exemplar(category, &item.embedding);
                trained_count += 1;
            }
        }

        if trained_count == 0 {
            self.status_message = Some((
                format!(
                    "⚠️ Could not train '{}': Selected photo(s) have no visual embeddings (CLIP model missing). Place clip_visual.onnx in models/",
                    category
                ),
                egui::Color32::from_rgb(240, 70, 70),
            ));
        } else {
            let _ = self.profiles.save_to_file("profiles.json");
            self.reclassify_all();
            self.status_message = Some((
                format!(
                    "✅ Successfully trained category '{}' from {} photo(s)!",
                    category, trained_count
                ),
                egui::Color32::from_rgb(40, 200, 40),
            ));
        }
    }

    pub fn train_single_item(&mut self, item_index: usize, category: &str) {
        let category = category.trim();
        if category.is_empty() {
            self.status_message = Some((
                "Category name cannot be empty.".to_string(),
                egui::Color32::from_rgb(240, 180, 0),
            ));
            return;
        }

        if let Some(item) = self.items.get(item_index) {
            if item.embedding.is_empty() {
                self.status_message = Some((
                    format!(
                        "⚠️ Cannot train '{}': Photo has no visual embedding (CLIP model missing). Place clip_visual.onnx in models/",
                        category
                    ),
                    egui::Color32::from_rgb(240, 70, 70),
                ));
                return;
            }
            let emb = item.embedding.clone();
            self.profiles.add_exemplar(category, &emb);
            let _ = self.profiles.save_to_file("profiles.json");
            self.reclassify_all();
            self.status_message = Some((
                format!("✅ Successfully trained category '{}' from this photo!", category),
                egui::Color32::from_rgb(40, 200, 40),
            ));
        }
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
                        source: payload.source,
                        embedding: payload.embedding,
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
                    source,
                    embedding,
                } => {
                    if let Some(item) = self.items.iter_mut().find(|i| i.source_path == source_path)
                    {
                        item.year = year;
                        item.month = month;
                        item.is_exif = is_exif;
                        item.category = category;
                        item.confidence = confidence;
                        item.source = source;
                        item.embedding = embedding;
                    }
                }
                ScanMessage::Complete => {
                    self.is_processing = false;
                }
            }
        }

        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("📁 Source Folder").clicked() {
                    if let Some(p) = rfd::FileDialog::new().pick_folder() {
                        self.input_folder = Some(p.clone());
                        self.start_scan(ctx.clone(), p);
                    }
                }
                if ui.button("📂 Output Folder").clicked() {
                    self.output_folder = rfd::FileDialog::new().pick_folder();
                }

                ui.separator();
                if ui.button("☑ All").clicked() {
                    for item in &mut self.items {
                        item.selected = true;
                    }
                }
                if ui.button("☐ None").clicked() {
                    for item in &mut self.items {
                        item.selected = false;
                    }
                }

                ui.separator();
                if ui.button("🚀 Move").clicked() {
                    self.execute_transfer(TransferMode::Move);
                }
                if ui.button("📋 Copy").clicked() {
                    self.execute_transfer(TransferMode::Copy);
                }
                if ui.button("↩ Undo").clicked() {
                    let _ = UndoEngine::rollback_from_file(
                        "last_execution_manifest.json",
                        |_, _, _| {},
                    );
                }

                ui.separator();
                let toggle_text = if self.show_categories_panel {
                    "⚙ AI & Categories ▲"
                } else {
                    "⚙ AI & Categories ▼"
                };
                if ui.button(toggle_text).clicked() {
                    self.show_categories_panel = !self.show_categories_panel;
                }

                if self.is_processing {
                    ui.separator();
                    ui.spinner();
                    ui.label("Processing...");
                }
            });

            let mut clear_status = false;
            if let Some((ref msg, color)) = self.status_message {
                let msg = msg.clone();
                ui.separator();
                ui.horizontal(|ui| {
                    ui.colored_label(color, &msg);
                    if ui.small_button("✖").clicked() {
                        clear_status = true;
                    }
                });
            }
            if clear_status {
                self.status_message = None;
            }

            if self.show_categories_panel {
                ui.separator();
                ui.horizontal(|ui| {
                    if self.model_available {
                        ui.colored_label(egui::Color32::from_rgb(0, 200, 0), "● CLIP Model: Ready");
                    } else {
                        ui.colored_label(
                            egui::Color32::from_rgb(220, 150, 0),
                            "▲ CLIP Model: Missing (Rules Only)",
                        );
                    }

                    ui.separator();
                    let threshold_changed = ui
                        .add(
                            egui::Slider::new(&mut self.profiles.confidence_threshold, 0.30..=0.95)
                                .text("Confidence Threshold")
                                .step_by(0.01),
                        )
                        .changed();

                    if threshold_changed {
                        let _ = self.profiles.save_to_file("profiles.json");
                    }

                    if ui.button("⚡ Re-classify All").clicked() {
                        self.reclassify_all();
                    }
                });

                ui.horizontal_wrapped(|ui| {
                    ui.label(format!("Profiles ({}):", self.profiles.profiles.len()));
                    let mut category_to_delete = None;
                    for p in &self.profiles.profiles {
                        ui.horizontal(|ui| {
                            ui.label(format!("🏷 {} ({})", p.name, p.sample_count));
                            if ui.small_button("❌").clicked() {
                                category_to_delete = Some(p.name.clone());
                            }
                        });
                    }
                    if let Some(cat) = category_to_delete {
                        self.profiles.remove_category(&cat);
                        let _ = self.profiles.save_to_file("profiles.json");
                        self.reclassify_all();
                    }
                });

                ui.horizontal(|ui| {
                    ui.label("Train Category from Selected Photos:");
                    ui.text_edit_singleline(&mut self.target_training_category);
                    if ui.button("🎓 Train Selected").clicked() {
                        let cat = self.target_training_category.clone();
                        self.train_selected_as_category(&cat);
                    }
                });
            }
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                let mut single_train_request = None;

                egui::Grid::new("grid")
                    .num_columns(4)
                    .spacing([14.0, 14.0])
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
                                    let badge_color = match item.source {
                                        ClassificationSource::VisualModel => {
                                            egui::Color32::from_rgb(0, 180, 0)
                                        }
                                        ClassificationSource::Heuristic => {
                                            egui::Color32::from_rgb(0, 150, 220)
                                        }
                                        ClassificationSource::Manual => {
                                            egui::Color32::from_rgb(180, 100, 220)
                                        }
                                        ClassificationSource::UnsortedFallback => {
                                            egui::Color32::GRAY
                                        }
                                    };
                                    ui.colored_label(
                                        badge_color,
                                        format!("{:.0}% [{}]", item.confidence * 100.0, item.source),
                                    );
                                });

                                ui.horizontal(|ui| {
                                    let changed =
                                        ui.text_edit_singleline(&mut item.category).changed();
                                    if changed {
                                        item.source = ClassificationSource::Manual;
                                    }
                                    if ui
                                        .button("🎓")
                                        .on_hover_text("Train category from this photo")
                                        .clicked()
                                    {
                                        single_train_request =
                                            Some((idx, item.category.clone()));
                                    }
                                });
                            });
                            if (idx + 1) % 4 == 0 {
                                ui.end_row();
                            }
                        }
                    });

                if let Some((idx, category)) = single_train_request {
                    self.train_single_item(idx, &category);
                }
            });
        });
    }
}
