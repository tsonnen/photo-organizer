//! The top toolbar: folder pickers, selection, transfer actions and the
//! collapsible AI & categories panel.

use super::PhotoOrganizerApp;
use crate::execution_engine::TransferMode;
use eframe::egui;

impl PhotoOrganizerApp {
    pub(super) fn render_toolbar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            self.render_toolbar_actions(ui, ctx);
            self.render_status_line(ui);
            if self.show_categories_panel {
                self.render_categories_panel(ui);
            }
        });
    }

    fn render_toolbar_actions(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
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
                self.set_all_selected(true);
            }
            if ui.button("☐ None").clicked() {
                self.set_all_selected(false);
            }

            ui.separator();
            if ui.button("🚀 Move").clicked() {
                self.execute_transfer(TransferMode::Move);
            }
            if ui.button("📋 Copy").clicked() {
                self.execute_transfer(TransferMode::Copy);
            }
            if ui.button("↩ Undo").clicked() {
                self.undo_last_transfer();
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
    }

    /// The dismissible message left by training, scanning and transfers.
    fn render_status_line(&mut self, ui: &mut egui::Ui) {
        let Some((msg, color)) = self.status_message.clone() else {
            return;
        };

        ui.separator();
        let mut clear_status = false;
        ui.horizontal(|ui| {
            ui.colored_label(color, &msg);
            if ui.small_button("✖").clicked() {
                clear_status = true;
            }
        });
        if clear_status {
            self.status_message = None;
        }
    }

    /// Model status, the confidence threshold, the profile list and training
    /// from the selected photos.
    fn render_categories_panel(&mut self, ui: &mut egui::Ui) {
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
                self.save_profiles();
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
                self.remove_profile(&cat);
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
}
