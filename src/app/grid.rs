//! The photo grid: the scrolling central panel of thumbnails.

use super::PhotoOrganizerApp;
use crate::app::layout;
use crate::app::models::StagedItem;
use crate::classification::ClassificationSource;
use eframe::egui;

impl PhotoOrganizerApp {
    /// The central panel: a column-aware grid of photo cells, then any modal
    /// or training request the cells raised this frame.
    ///
    /// Draws [`view::visible_indices`] rather than every staged photo, so the
    /// filters and the sort apply here without either reaching into the grid's
    /// layout. An index into that list is still an index into `self.items`,
    /// which is what the modal and the training request downstream need.
    pub(super) fn render_grid(&mut self, ctx: &egui::Context) {
        let panel_frame = egui::Frame::central_panel(&ctx.style()).inner_margin(egui::Margin {
            left: 8.0,
            right: 0.0,
            top: 8.0,
            bottom: 8.0,
        });
        egui::CentralPanel::default()
            .frame(panel_frame)
            .show(ctx, |ui| {
                let visible = self.visible_indices();

                egui::ScrollArea::vertical()
                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
                    .show(ui, |ui| {
                        if visible.is_empty() {
                            self.render_empty_grid(ui);
                            return;
                        }

                        let mut train_request = None;
                        let mut open_modal_idx = None;
                        let (columns, item_width) =
                            layout::grid_column_layout(ui.available_width());

                        egui::Grid::new("grid")
                            .num_columns(columns)
                            .spacing([layout::SPACING, layout::SPACING])
                            .show(ui, |ui| {
                                // `position`, not the index into `items`: the row
                                // ends every `columns` *drawn* cells, and an
                                // index into the underlying vector counts the
                                // filtered-out photos too. Using it would leave
                                // the last row of a filtered grid short and wrap
                                // early.
                                for (position, &idx) in visible.iter().enumerate() {
                                    ui.vertical(|ui| {
                                        let item = &mut self.items[idx];
                                        if render_photo_cell(ui, item, item_width) {
                                            open_modal_idx = Some(idx);
                                        }
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
                                            train_request = Some((idx, category));
                                        }
                                    });
                                    if (position + 1) % columns == 0 {
                                        ui.end_row();
                                    }
                                }
                            });

                        if let Some(idx) = open_modal_idx {
                            self.open_modal(idx, ctx);
                        }

                        if let Some((idx, category)) = train_request {
                            self.train_single_item(idx, &category);
                        }
                    });
            });
    }

    /// What the grid says when it has no cells to draw.
    ///
    /// The two empty states need different words. Nothing staged means the user
    /// has not picked a folder yet, and "no photos match your filters" would send
    /// them looking for a filter they never set. Photos staged but none in view
    /// means the filters are doing exactly what was asked, so the way out is
    /// named rather than left for them to infer.
    fn render_empty_grid(&self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(40.0);
            if self.items.is_empty() {
                ui.label(egui::RichText::new("No photos staged").weak());
                ui.label("Pick a 📁 Source Folder to scan one.");
                return;
            }
            ui.label(
                egui::RichText::new(format!(
                    "None of the {} staged photos match the filters",
                    self.items.len()
                ))
                .weak(),
            );
            ui.label("Widen the 🔍 Filters to see the rest.");
        });
    }
}

/// Renders one cell's thumbnail, hover card, checkbox and source badge.
/// Returns true when the photo was clicked and should open the modal.
fn render_photo_cell(ui: &mut egui::Ui, item: &mut StagedItem, item_width: f32) -> bool {
    let tex_size = item.texture.size_vec2();
    let aspect = tex_size.y / tex_size.x;
    let scaled_height = item_width * aspect;

    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(item_width, scaled_height), egui::Sense::click());

    ui.painter().image(
        item.texture.id(),
        rect,
        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        egui::Color32::WHITE,
    );

    if response.hovered() {
        ui.painter().rect_stroke(
            rect,
            0.0,
            egui::Stroke::new(2.0_f32, egui::Color32::from_rgb(0, 180, 255)),
        );
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
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
            egui::RichText::new("🔍 Click to inspect photo in modal")
                .strong()
                .color(egui::Color32::from_rgb(0, 180, 255)),
        );
        let hover_w = 380.0;
        let hover_h = hover_w * aspect;
        ui.image(egui::load::SizedTexture::new(tex_id, [hover_w, hover_h]));
        ui.label(format!("File: {filename}"));
        ui.label(format!("Category: {category} ({:.0}%)", confidence * 100.0));
    });

    ui.checkbox(&mut item.selected, &filename);

    ui.horizontal(|ui| {
        ui.label(format!("{}/{:02}", item.year, item.month));
        ui.colored_label(
            source_badge_color(item.source),
            format!("{:.0}% [{}]", item.confidence * 100.0, item.source),
        );
    });

    is_clicked
}

/// Badge colour for a classification source, so a grid cell says at a glance
/// whether the category came from the model, the rules, or the user.
fn source_badge_color(source: ClassificationSource) -> egui::Color32 {
    match source {
        ClassificationSource::VisualModel => egui::Color32::from_rgb(0, 180, 0),
        ClassificationSource::Heuristic => egui::Color32::from_rgb(0, 150, 220),
        ClassificationSource::Manual => egui::Color32::from_rgb(180, 100, 220),
        ClassificationSource::UnsortedFallback => egui::Color32::GRAY,
    }
}
