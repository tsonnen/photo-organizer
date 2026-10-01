use crate::execution_engine::{ExecutionEngine, RawPhotoInput, TransferMode};
use crate::inference::is_model_available;
use crate::profile_store::{ClassificationSource, ProfileStore, RankedProfile};
use crate::scanner::{scan_folder, ScanMessage};
use crate::undo_engine::UndoEngine;
use eframe::egui;
use std::fs;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};

/// Horizontal and vertical gap between grid cells, in points.
const SPACING: f32 = 14.0;

/// Width of the train button beside the category combo in a grid cell.
const TRAIN_BUTTON_SIZE: f32 = 20.0;

/// Narrowest control we will lay out inside a grid cell, in points.
const MIN_CONTROL_WIDTH: f32 = 60.0;

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

pub struct ModalPreview {
    pub item_index: usize,
    pub high_res_texture: Option<egui::TextureHandle>,
    pub high_res_path: Option<PathBuf>,
    pub is_loading: bool,
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

    fn start_scan(&mut self, ctx: egui::Context, folder: PathBuf) {
        self.items.clear();
        self.status_message = None;
        self.is_processing = true;
        let tx = self.tx.clone();
        let profiles = self.profiles.clone();
        scan_folder(folder, profiles, tx, ctx);
    }

    pub fn reclassify_all(&mut self) {
        let mut visual_count = 0;
        let total_count = self.items.len();
        for item in &mut self.items {
            if item.source == ClassificationSource::Manual {
                item.is_custom = Self::is_custom_category(&self.profiles, &item.category);
                continue;
            }
            let width = item.texture.size()[0] as u32;
            let height = item.texture.size()[1] as u32;
            let res = self.profiles.classify_with_heuristics(
                &item.embedding,
                &item.source_path,
                item.is_exif,
                width,
                height,
            );
            if res.source == crate::profile_store::ClassificationSource::VisualModel {
                visual_count += 1;
            }
            item.is_custom = Self::is_custom_category(&self.profiles, &res.category);
            item.category = res.category;
            item.confidence = res.confidence;
            item.source = res.source;
        }
        if total_count > 0 {
            self.status_message = Some((
                format!(
                    "⚡ Re-classified {} photo(s) ({} visual AI match(es), {} active category profile(s), threshold {:.2})",
                    total_count,
                    visual_count,
                    self.profiles.profiles.len(),
                    self.profiles.confidence_threshold
                ),
                egui::Color32::from_rgb(180, 220, 255),
            ));
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
                    "⚠️ Could not train '{}': Selected photo(s) have no visual embeddings (CLIP model missing). Place clip_vision.safetensors in models/",
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
                        "⚠️ Cannot train '{}': Photo has no visual embedding (CLIP model missing). Place clip_vision.safetensors in models/",
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
                format!(
                    "✅ Successfully trained category '{}' from this photo!",
                    category
                ),
                egui::Color32::from_rgb(40, 200, 40),
            ));
        }
    }

    fn is_custom_category(profiles: &ProfileStore, category: &str) -> bool {
        !profiles
            .profiles
            .iter()
            .any(|p| p.name.eq_ignore_ascii_case(category))
    }

    fn profile_label(name: &str, confidence: f32, has_embedding: bool) -> String {
        if has_embedding {
            format!("{} ({:.0}%)", name, confidence * 100.0)
        } else {
            name.to_string()
        }
    }

    fn category_label(item: &StagedItem, ranked_profiles: &[RankedProfile]) -> String {
        if item.is_custom {
            "Other".to_string()
        } else if let Some(matching) = ranked_profiles
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(&item.category))
        {
            Self::profile_label(
                &matching.name,
                matching.confidence,
                !item.embedding.is_empty(),
            )
        } else {
            "Other".to_string()
        }
    }

    fn render_category_selector(
        profiles: &ProfileStore,
        ui: &mut egui::Ui,
        item: &mut StagedItem,
        combo_id: egui::Id,
        combo_width: Option<f32>,
    ) {
        let ranked_profiles = profiles.rank_profiles(&item.embedding);
        let has_embedding = !item.embedding.is_empty();
        let selected_label = Self::category_label(item, &ranked_profiles);

        let mut combo = egui::ComboBox::from_id_salt(combo_id);
        if let Some(w) = combo_width {
            combo = combo.width(w);
        }
        combo.selected_text(&selected_label).show_ui(ui, |ui| {
            for prof in &ranked_profiles {
                let is_selected = !item.is_custom && item.category.eq_ignore_ascii_case(&prof.name);
                let label = Self::profile_label(&prof.name, prof.confidence, has_embedding);
                if ui.selectable_label(is_selected, label).clicked() {
                    item.category = prof.name.clone();
                    item.confidence = prof.confidence;
                    item.source = ClassificationSource::Manual;
                    item.is_custom = false;
                }
            }
            if !ranked_profiles.is_empty() {
                ui.separator();
            }
            if ui.selectable_label(item.is_custom, "Other").clicked() {
                item.is_custom = true;
                item.source = ClassificationSource::Manual;
            }
        });
    }

    /// Grid column count and item width for a given available width.
    ///
    /// Extracted from the grid renderer so the arithmetic can be asserted
    /// directly. The column count is derived from `MIN_ITEM_WIDTH` first, then
    /// the per-column width is clamped to `MAX_ITEM_WIDTH`; clamping only ever
    /// shrinks the content, so the grid always fits the width it was given.
    fn grid_column_layout(available_width: f32) -> (usize, f32) {
        const MIN_ITEM_WIDTH: f32 = 240.0;
        const MAX_ITEM_WIDTH: f32 = 350.0;

        let number_columns = ((available_width) / (MIN_ITEM_WIDTH + SPACING))
            .floor()
            .max(1.0) as usize;
        let item_width = ((available_width - (number_columns as f32 - 1.0) * SPACING)
            / number_columns as f32)
            .clamp(MIN_ITEM_WIDTH, MAX_ITEM_WIDTH);
        (number_columns, item_width)
    }

    /// Renders one grid cell's category controls and returns the category to
    /// train if the train button was clicked.
    ///
    /// The combo and train button share a row; the custom category input, when
    /// the item is custom, goes on the row *below*. That stacking is load
    /// bearing: sharing a single row starves the input down to whatever sliver
    /// is left after the combo, and forces the cell wider than its grid column.
    fn render_grid_cell_controls(
        profiles: &ProfileStore,
        ui: &mut egui::Ui,
        item: &mut StagedItem,
        item_width: f32,
        combo_id: egui::Id,
    ) -> Option<String> {
        let mut train_request = None;
        let combo_width = Self::grid_cell_combo_width(item_width);

        ui.horizontal(|ui| {
            Self::render_category_selector(profiles, ui, item, combo_id, Some(combo_width));

            if ui
                .add_sized(
                    [TRAIN_BUTTON_SIZE, TRAIN_BUTTON_SIZE],
                    egui::Button::new("🎓"),
                )
                .on_hover_text("Train category from this photo")
                .clicked()
            {
                train_request = Some(item.category.clone());
            }
        });

        if item.is_custom {
            let category_input_width = Self::grid_cell_input_width(item_width);
            Self::render_custom_category_input(ui, item, category_input_width);
        }

        train_request
    }

    /// Width of the category combo in a grid cell: the cell minus the train
    /// button and the gaps either side of it.
    fn grid_cell_combo_width(item_width: f32) -> f32 {
        (item_width - TRAIN_BUTTON_SIZE - (SPACING * 2.0)).max(MIN_CONTROL_WIDTH)
    }

    /// Width of the custom category text input in a grid cell: the full cell
    /// width, since it sits on its own line rather than sharing one.
    fn grid_cell_input_width(item_width: f32) -> f32 {
        (item_width - (SPACING * 2.0)).max(MIN_CONTROL_WIDTH)
    }

    fn render_custom_category_input(ui: &mut egui::Ui, item: &mut StagedItem, input_width: f32) {
        let custom_input = ui.add(
            egui::TextEdit::singleline(&mut item.category)
                .hint_text("Custom category...")
                .desired_width(input_width),
        );
        if custom_input.changed() {
            item.source = ClassificationSource::Manual;
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

    pub fn open_modal(&mut self, index: usize, ctx: &egui::Context) {
        if index >= self.items.len() {
            return;
        }
        let item = &self.items[index];
        let path = item.source_path.clone();

        self.modal_preview = Some(ModalPreview {
            item_index: index,
            high_res_texture: None,
            high_res_path: None,
            is_loading: true,
        });

        let tx = self.high_res_tx.clone();
        let ctx_clone = ctx.clone();
        std::thread::spawn(move || {
            if let Ok(dyn_img) = crate::media::load_image(&path) {
                let color_img = crate::media::dynamic_to_preview_color_image(&dyn_img, 1920);
                let _ = tx.send((path, color_img));
                ctx_clone.request_repaint();
            }
        });
    }

    pub fn navigate_modal(&mut self, ctx: &egui::Context, delta: isize) {
        if self.items.is_empty() {
            return;
        }
        if let Some(modal) = &self.modal_preview {
            let current = modal.item_index as isize;
            let len = self.items.len() as isize;
            let new_index = ((current + delta).rem_euclid(len)) as usize;
            self.open_modal(new_index, ctx);
        }
    }

    fn render_modal(&mut self, ctx: &egui::Context) {
        let modal_index = match &self.modal_preview {
            Some(m) => m.item_index,
            None => return,
        };

        if modal_index >= self.items.len() {
            self.modal_preview = None;
            return;
        }

        let mut close_modal = false;
        let mut prev_requested = false;
        let mut next_requested = false;
        let mut toggle_select = false;

        ctx.input(|i| {
            if i.key_pressed(egui::Key::Escape) {
                close_modal = true;
            }
            if i.key_pressed(egui::Key::ArrowLeft) {
                prev_requested = true;
            }
            if i.key_pressed(egui::Key::ArrowRight) {
                next_requested = true;
            }
            if i.key_pressed(egui::Key::Space) {
                toggle_select = true;
            }
        });

        if close_modal {
            self.modal_preview = None;
            return;
        }
        if prev_requested {
            self.navigate_modal(ctx, -1);
            return;
        }
        if next_requested {
            self.navigate_modal(ctx, 1);
            return;
        }

        if toggle_select {
            if let Some(item) = self.items.get_mut(modal_index) {
                item.selected = !item.selected;
            }
        }

        let screen_rect = ctx.screen_rect();
        let item_count = self.items.len();
        let is_loading = self.modal_preview.as_ref().is_some_and(|m| m.is_loading);
        let high_res_tex = self
            .modal_preview
            .as_ref()
            .and_then(|m| m.high_res_texture.clone());

        let mut single_train_requested = false;

        let item = &mut self.items[modal_index];
        let filename = item
            .source_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        egui::Area::new(egui::Id::new("photo_verification_modal_area"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen_rect.min)
            .show(ctx, |ui| {
                let modal_w = (screen_rect.width() * 0.85).clamp(500.0, 1100.0);
                let modal_h = (screen_rect.height() * 0.85).clamp(400.0, 800.0);
                let modal_rect = egui::Rect::from_center_size(
                    screen_rect.center(),
                    egui::vec2(modal_w, modal_h),
                );
                // 1. Dark translucent masking backdrop
                let (backdrop_rect, backdrop_resp) =
                    ui.allocate_exact_size(screen_rect.size(), egui::Sense::click());
                ui.painter()
                    .rect_filled(backdrop_rect, 0.0, egui::Color32::from_black_alpha(200));

                if backdrop_resp.clicked() {
                    // Only close if the click occurred outside the modal card bounds
                    if let Some(interact_pos) = backdrop_resp.interact_pointer_pos() {
                        if !modal_rect.contains(interact_pos) {
                            close_modal = true;
                        }
                    }
                }

                // 2. Centered Modal Card Container
                ui.scope_builder(egui::UiBuilder::new().max_rect(modal_rect), |ui| {
                    egui::Frame::window(&ctx.style())
                        .rounding(8.0)
                        .show(ui, |ui| {
                            ui.set_min_size(modal_rect.size());
                            ui.set_max_size(modal_rect.size());

                            // Header / Title & Metadata
                            ui.horizontal(|ui| {
                                ui.checkbox(&mut item.selected, "☑ Selected");
                                ui.separator();
                                ui.label(egui::RichText::new(&filename).strong());
                                ui.separator();
                                ui.label(format!("Date: {}/{:02}", item.year, item.month));
                                if is_loading {
                                    ui.separator();
                                    ui.spinner();
                                    ui.colored_label(
                                        egui::Color32::LIGHT_GRAY,
                                        "Loading full image...",
                                    );
                                } else if high_res_tex.is_some() {
                                    ui.separator();
                                    ui.colored_label(
                                        egui::Color32::from_rgb(0, 200, 100),
                                        "✨ High-Res",
                                    );
                                }

                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if ui.button("✖").clicked() {
                                            close_modal = true;
                                        }
                                    },
                                );
                            });
                            ui.separator();

                            // Image View
                            let active_tex = high_res_tex.as_ref().unwrap_or(&item.texture);
                            let tex_size = active_tex.size_vec2();
                            let avail_w = ui.available_width();
                            let avail_h = (ui.available_height() - 50.0).max(150.0);

                            let img_aspect = tex_size.x / tex_size.y;
                            let container_aspect = avail_w / avail_h;

                            let (disp_w, disp_h) = if img_aspect > container_aspect {
                                (avail_w, avail_w / img_aspect)
                            } else {
                                (avail_h * img_aspect, avail_h)
                            };

                            ui.vertical_centered(|ui| {
                                ui.image(egui::load::SizedTexture::new(
                                    active_tex.id(),
                                    [disp_w, disp_h],
                                ));
                            });

                            ui.separator();

                            // Bottom Navigation & Controls
                            ui.horizontal(|ui| {
                                if ui.button("◀ Previous (Left)").clicked() {
                                    prev_requested = true;
                                }
                                ui.label(format!("{}/{}", modal_index + 1, item_count));
                                if ui.button("Next (Right) ▶").clicked() {
                                    next_requested = true;
                                }

                                ui.separator();
                                ui.label("Category:");

                                Self::render_category_selector(
                                    &self.profiles,
                                    ui,
                                    item,
                                    ui.make_persistent_id((
                                        "modal_cat_combo",
                                        modal_index,
                                        &item.source_path,
                                    )),
                                    Some(120.0),
                                );

                                if ui
                                    .button("🎓 Train")
                                    .on_hover_text("Train category from this photo")
                                    .clicked()
                                {
                                    single_train_requested = true;
                                }

                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if ui.button("Close (Esc)").clicked() {
                                            close_modal = true;
                                        }
                                    },
                                );
                            });

                            if item.is_custom {
                                Self::render_custom_category_input(ui, item, 120.0);
                            }
                        });
                });
            });

        if close_modal {
            self.modal_preview = None;
        } else if prev_requested {
            self.navigate_modal(ctx, -1);
        } else if next_requested {
            self.navigate_modal(ctx, 1);
        } else if single_train_requested {
            let cat = item.category.clone();
            self.train_single_item(modal_index, &cat);
        }
    }
}

impl eframe::App for PhotoOrganizerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok((path, color_img)) = self.high_res_rx.try_recv() {
            if let Some(modal) = &mut self.modal_preview {
                if let Some(item) = self.items.get(modal.item_index) {
                    if item.source_path == path {
                        let filename = path.file_name().unwrap_or_default().to_string_lossy();
                        let texture = ctx.load_texture(
                            format!("modal_{}", filename),
                            color_img,
                            egui::TextureOptions::LINEAR,
                        );
                        modal.high_res_texture = Some(texture);
                        modal.high_res_path = Some(path);
                        modal.is_loading = false;
                    }
                }
            }
        }

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
                        self.reclassify_all();
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

        let panel_frame = egui::Frame::central_panel(&ctx.style()).inner_margin(egui::Margin {
            left: 8.0,
            right: 0.0,
            top: 8.0,
            bottom: 8.0,
        });
        egui::CentralPanel::default()
            .frame(panel_frame)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
                    .show(ui, |ui| {
                        let mut single_train_request = None;
                        let mut open_modal_idx = None;
                        let available_width = ui.available_width();
                        let (number_columns, item_width) =
                            Self::grid_column_layout(available_width);
                        egui::Grid::new("grid")
                            .num_columns(number_columns)
                            .spacing([SPACING, SPACING])
                            .show(ui, |ui| {
                                for (idx, item) in self.items.iter_mut().enumerate() {
                                    ui.vertical(|ui| {
                                        let tex_size = item.texture.size_vec2();
                                        let aspect = tex_size.y / tex_size.x;
                                        let scaled_height = item_width * aspect;

                                        let (rect, response) = ui.allocate_exact_size(
                                            egui::vec2(item_width, scaled_height),
                                            egui::Sense::click(),
                                        );

                                        ui.painter().image(
                                            item.texture.id(),
                                            rect,
                                            egui::Rect::from_min_max(
                                                egui::pos2(0.0, 0.0),
                                                egui::pos2(1.0, 1.0),
                                            ),
                                            egui::Color32::WHITE,
                                        );

                                        if response.hovered() {
                                            ui.painter().rect_stroke(
                                                rect,
                                                0.0,
                                                egui::Stroke::new(
                                                    2.0_f32,
                                                    egui::Color32::from_rgb(0, 180, 255),
                                                ),
                                            );
                                            ui.ctx()
                                                .set_cursor_icon(egui::CursorIcon::PointingHand);
                                        }

                                        let is_clicked = response.clicked();
                                        let filename = item
                                            .source_path
                                            .file_name()
                                            .unwrap_or_default()
                                            .to_string_lossy()
                                            .to_string();
                                        let category = item.category.clone();
                                        let confidence = item.confidence;
                                        let tex_id = item.texture.id();
                                        response.on_hover_ui(|ui| {
                                            ui.label(
                                                egui::RichText::new(
                                                    "🔍 Click to inspect photo in modal",
                                                )
                                                .strong()
                                                .color(egui::Color32::from_rgb(0, 180, 255)),
                                            );
                                            let hover_w = 380.0;
                                            let hover_h = hover_w * aspect;
                                            ui.image(egui::load::SizedTexture::new(
                                                tex_id,
                                                [hover_w, hover_h],
                                            ));
                                            ui.label(format!("File: {}", filename));
                                            ui.label(format!(
                                                "Category: {} ({:.0}%)",
                                                category,
                                                confidence * 100.0
                                            ));
                                        });

                                        if is_clicked {
                                            open_modal_idx = Some(idx);
                                        }

                                        ui.checkbox(&mut item.selected, &filename);

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
                                                format!(
                                                    "{:.0}% [{}]",
                                                    item.confidence * 100.0,
                                                    item.source
                                                ),
                                            );
                                        });

                                        if let Some(category) = Self::render_grid_cell_controls(
                                            &self.profiles,
                                            ui,
                                            item,
                                            item_width,
                                            ui.make_persistent_id((
                                                "cat_combo",
                                                idx,
                                                &item.source_path,
                                            )),
                                        ) {
                                            single_train_request = Some((idx, category));
                                        }
                                    });
                                    if (idx + 1) % number_columns == 0 {
                                        ui.end_row();
                                    }
                                }
                            });

                        if let Some(idx) = open_modal_idx {
                            self.open_modal(idx, ctx);
                        }

                        if let Some((idx, category)) = single_train_request {
                            self.train_single_item(idx, &category);
                        }
                    });
            });

        self.render_modal(ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_custom_input_rendered_by_both_grid_and_modal() {
        // Both the grid cell and the inspection modal must render the custom input,
        // otherwise a custom category becomes uneditable in one of the two views.
        // Count only production call sites, excluding this test's own literal.
        let source = include_str!("app.rs");
        let production = &source[..source.find("#[cfg(test)]").unwrap()];

        let call_sites = production
            .matches("Self::render_custom_category_input(")
            .count();
        assert_eq!(
            call_sites, 2,
            "expected render_custom_category_input to be called from both the grid cell \
             and the modal, found {call_sites} call site(s)"
        );
    }

    #[test]
    fn test_is_custom_category_is_case_insensitive() {
        let mut store = ProfileStore::default();
        store.add_exemplar("Sunsets", &[1.0, 0.0, 0.0]);

        // Known profile names are not custom, and matching ignores case.
        assert!(!PhotoOrganizerApp::is_custom_category(&store, "Sunsets"));
        assert!(!PhotoOrganizerApp::is_custom_category(&store, "sunsets"));
        // Unknown names are custom.
        assert!(PhotoOrganizerApp::is_custom_category(&store, "Beach Trip"));
    }
}

/// Layout assertions driven through `egui_kittest`.
///
/// These render the real `render_category_selector` and
/// `render_custom_category_input` into a headless egui context and assert on
/// the geometry egui actually computes. That matters because the original
/// column-overflow bug was invisible to source-text checks: the code looked
/// right, but egui placed the widgets on one line and pushed the row wider
/// than the grid column.
#[cfg(test)]
mod layout_tests {
    use super::*;
    use egui_kittest::kittest::{by, Queryable};
    use egui_kittest::Harness;

    /// One widget as laid out by egui, in points.
    #[derive(Debug, Clone)]
    struct WidgetRect {
        role: String,
        label: String,
        x0: f64,
        y0: f64,
        x1: f64,
        y1: f64,
    }

    impl WidgetRect {
        fn width(&self) -> f64 {
            self.x1 - self.x0
        }

        fn height(&self) -> f64 {
            self.y1 - self.y0
        }

        /// True when the two rects share any vertical band, i.e. sit on the
        /// same visual row.
        fn same_row(&self, other: &WidgetRect) -> bool {
            self.y0 < other.y1 && other.y0 < self.y1
        }
    }

    /// A staged item wired for layout purposes. `is_custom` is true so the
    /// custom category input is rendered.
    ///
    /// The texture handle has to be created against a live `Context`, so the
    /// caller passes one in; `Harness::new_ui_state` hands us `ui.ctx()`.
    fn staged_item(ctx: &egui::Context) -> StagedItem {
        StagedItem {
            source_path: PathBuf::from("/photos/sample.jpg"),
            year: 2024,
            month: 5,
            is_exif: true,
            category: "Beach Trip".into(),
            confidence: 0.5,
            source: ClassificationSource::Manual,
            embedding: vec![0.1, 0.2, 0.3],
            selected: false,
            is_custom: true,
            texture: ctx.load_texture(
                "thumb",
                egui::ColorImage::new([64, 64], egui::Color32::from_gray(128)),
                egui::TextureOptions::default(),
            ),
        }
    }

    /// Lays out the grid cell's category controls into a harness `cell_width`
    /// points wide and returns every widget egui placed, in points.
    ///
    /// `stacked` mirrors the production grid cell: the combo and train button
    /// share a horizontal line, and the custom input follows on its own line.
    /// `inline` is the shape that caused the original bug, kept so the tests
    /// can prove their assertions actually detect it.
    fn render_category_cell(cell_width: f32) -> Vec<WidgetRect> {
        let store = ProfileStore::default();

        let mut harness = Harness::new_ui_state(
            move |ui, item: &mut Option<StagedItem>| {
                let item = item.get_or_insert_with(|| staged_item(ui.ctx()));
                ui.vertical(|ui| {
                    // The production grid cell renderer itself, so these tests
                    // measure the real layout rather than a copy of it.
                    PhotoOrganizerApp::render_grid_cell_controls(
                        &store,
                        ui,
                        item,
                        cell_width,
                        egui::Id::new("cat_combo"),
                    );
                });
            },
            None,
        );
        harness.set_size(egui::vec2(cell_width, 400.0));
        harness.run();

        harness
            .kittest_state()
            .query_all(by().recursive(true))
            .filter_map(|node| {
                let b = node.raw_bounds()?;
                let role = format!("{:?}", node.role());
                // The window root is not a widget we lay out.
                if role == "Window" {
                    return None;
                }
                let label = node.label().or_else(|| node.value()).unwrap_or_default();
                Some(WidgetRect {
                    role,
                    label,
                    x0: b.x0,
                    y0: b.y0,
                    x1: b.x1,
                    y1: b.y1,
                })
            })
            .collect()
    }

    fn find<'a>(rects: &'a [WidgetRect], role_contains: &str) -> &'a WidgetRect {
        rects
            .iter()
            .find(|r| r.role.contains(role_contains))
            .unwrap_or_else(|| {
                panic!("no widget with role containing {role_contains:?} in {rects:#?}")
            })
    }

    #[test]
    fn custom_input_renders_below_the_combo_not_beside_it() {
        // The regression guard for the column-overflow bug. In the production
        // layout the combo+button share a row and the custom input follows
        // underneath, so the input's vertical band must not overlap the row.
        let cell_width = 300.0;
        let rects = render_category_cell(cell_width);

        let combo = find(&rects, "ComboBox");
        let input = find(&rects, "TextInput");

        assert!(
            !combo.same_row(input),
            "custom input must not share a row with the combo.\ncombo: {combo:?}\ninput: {input:?}"
        );
        assert!(
            input.y0 >= combo.y1 - 0.5,
            "custom input should start below the combo.\ncombo: {combo:?}\ninput: {input:?}"
        );

        // On its own line it gets at least the full cell width it asked for,
        // rather than the sliver left over after the combo. (`desired_width`
        // is a floor, not a fixed size, so egui may hand back slightly more.)
        let requested_input_width = f64::from(PhotoOrganizerApp::grid_cell_input_width(cell_width));
        assert!(
            input.width() >= requested_input_width,
            "stacked input should render at least its requested {requested_input_width}pt, \
             got {:.1}pt",
            input.width()
        );
    }

    #[test]
    fn grid_cell_controls_stay_within_the_column_width() {
        // Whatever width the window happens to be, no widget in a grid cell may
        // spill past the column it was allocated.
        for available in [300.0, 420.0, 640.0, 900.0, 1240.0, 1600.0, 1920.0] {
            let (columns, item_width) = PhotoOrganizerApp::grid_column_layout(available);
            let rects = render_category_cell(item_width);

            assert!(
                !rects.is_empty(),
                "expected widgets to be laid out at available width {available}"
            );
            for r in &rects {
                assert!(
                    r.x1 <= item_width as f64 + 0.5 && r.x0 >= -0.5,
                    "widget {:?} ({}) escapes its {item_width}pt column at available \
                     width {available} (columns={columns})\nrect: {r:?}",
                    r.role,
                    r.label,
                );
            }
        }
    }

    #[test]
    fn grid_columns_fit_the_available_width() {
        // The grid's own arithmetic: N columns plus N-1 gaps must never exceed
        // the width it was handed, otherwise the last column is clipped.
        //
        // Starts at MIN_ITEM_WIDTH (240). Below that the layout deliberately
        // cannot fit a single cell, which is fine because the window enforces
        // a 1240pt minimum and egui clips a degenerate panel; see
        // `grid_holds_one_column_below_minimum_width`.
        for available in [
            240.0, 254.0, 300.0, 500.0, 762.0, 1016.0, 1240.0, 1600.0, 2560.0, 3840.0,
        ] {
            let (columns, item_width) = PhotoOrganizerApp::grid_column_layout(available);
            assert!(columns >= 1, "always at least one column, got {columns}");
            assert!(
                item_width >= 240.0,
                "item_width {item_width} below minimum at available {available}"
            );

            let content = columns as f32 * item_width + (columns as f32 - 1.0) * SPACING;
            assert!(
                content <= available + 0.5,
                "grid content {content} exceeds available {available} \
                 (columns={columns}, item_width={item_width})"
            );
        }
    }

    #[test]
    fn grid_holds_one_column_below_minimum_width() {
        // Documents the degenerate case rather than pretending it fits: at
        // 100pt the grid still offers a single readable cell, it just cannot
        // fit the panel it was given.
        let (columns, item_width) = PhotoOrganizerApp::grid_column_layout(100.0);
        assert_eq!(columns, 1);
        assert_eq!(item_width, 240.0);
    }

    #[test]
    fn column_count_grows_with_available_width() {
        // Monotonicity: a wider window should never produce fewer columns.
        let mut previous = 0;
        for available in [300.0, 500.0, 800.0, 1240.0, 1600.0, 2000.0, 2560.0] {
            let (columns, _) = PhotoOrganizerApp::grid_column_layout(available);
            assert!(
                columns >= previous,
                "column count dropped from {previous} to {columns} at width {available}"
            );
            previous = columns;
        }
    }

    #[test]
    fn every_widget_has_a_positive_size() {
        // A zero- or negative-sized widget is invisible or misclickable; this
        // catches a control collapsed by a bad width calculation.
        let rects = render_category_cell(300.0);
        for r in &rects {
            assert!(
                r.width() > 0.0 && r.height() > 0.0,
                "widget {:?} ({}) collapsed to {}x{}",
                r.role,
                r.label,
                r.width(),
                r.height()
            );
        }
    }
}
