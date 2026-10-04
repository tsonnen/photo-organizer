//! The top toolbar: folder pickers, selection, transfer actions and the
//! collapsible AI & categories panel.

use super::PhotoOrganizerApp;
use crate::transfer::TransferMode;
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
        let mut pick_source = false;

        self.render_menu_bar(ui);

        ui.horizontal(|ui| {
            if ui.button("📁 Source Folder").clicked() {
                pick_source = true;
            }

            ui.separator();
            if ui.button("☑ All").clicked() {
                self.set_all_selected(true);
            }
            if ui.button("☐ None").clicked() {
                self.set_all_selected(false);
            }

            ui.separator();

            // Move and Copy need somewhere to put things. Rather than letting
            // them press and then refuse, they sit disabled until a destination
            // is configured.
            //
            // `on_disabled_hover_text`, not `on_hover_text`: egui deliberately
            // suppresses the latter on a non-interactable widget, so the obvious
            // spelling gives a greyed button that explains nothing — exactly the
            // dead end the greying was meant to avoid.
            let has_destination = self.settings.output_folder.is_some();
            let destination_hint = if has_destination {
                "Files the selected photos into the folder in the status bar"
            } else {
                "Set an output folder in File ▸ Settings… before transferring"
            };

            if ui
                .add_enabled(has_destination, egui::Button::new("🚀 Move"))
                .on_disabled_hover_text(destination_hint)
                .on_hover_text(destination_hint)
                .clicked()
            {
                self.execute_transfer(TransferMode::Move);
            }
            if ui
                .add_enabled(has_destination, egui::Button::new("📋 Copy"))
                .on_disabled_hover_text(destination_hint)
                .on_hover_text(destination_hint)
                .clicked()
            {
                self.execute_transfer(TransferMode::Copy);
            }
            // The two buttons above are opposites, so undo is not a single gesture
            // either: it moves the last moved batch home again, and it deletes
            // the copies the last copied batch made. Both directions leave
            // anything edited since alone, which is worth saying where the
            // button is rather than only in the manual.
            let undo = ui.button("↩ Undo").on_hover_text(
                "Reverse the last transfer: move a moved batch home again, or delete the copies a copied batch made. Files you have edited since are left alone.",
            );
            if undo.clicked() {
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

        // Opened once the action row has finished laying out, not from inside
        // the button handler: `rfd`'s dialog is a blocking native call, and
        // blocking inside a panel closure would stall the UI thread mid-layout.
        if pick_source {
            self.pick_source_folder(ctx);
        }
    }

    /// Asks for a folder and starts scanning it, if one was chosen.
    fn pick_source_folder(&mut self, ctx: &egui::Context) {
        if let Some(picked) = rfd::FileDialog::new()
            .set_title("Choose the folder of photos to scan")
            .pick_folder()
        {
            self.input_folder = Some(picked.clone());
            self.start_scan(ctx.clone(), picked);
        }
    }

    /// The menu bar above the action row.
    ///
    /// Chrome only: where the app is configured and how it is exited. Scanning
    /// is deliberately *not* here — it has a toolbar button already, and a
    /// second route to it would be a second thing to keep in sync for no gain.
    /// The actions of the moment stay as buttons below for the same reason
    /// they're buttons: Move and Copy are what the toolbar is for, and burying
    /// them one click deep to tidy the strip up would cost more than the
    /// tidiness is worth.
    fn render_menu_bar(&mut self, ui: &mut egui::Ui) {
        egui::menu::bar(ui, |ui| {
            ui.menu_button("📁 File", |ui| {
                if ui.button("Settings…").clicked() {
                    // A plain flag, safe to set mid-frame: `render_settings_modal`
                    // runs later in this same frame.
                    self.show_settings_modal = true;
                    ui.close_menu();
                }
                ui.separator();
                if ui.button("Quit").clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                    ui.close_menu();
                }
            });
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

    /// Model status, the trigger for the profile modal and training from the
    /// selected photos.
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

            if ui.button("⚡ Re-classify All").clicked() {
                self.reclassify_all();
            }
        });
        ui.horizontal(|ui| {
            // The profile list itself lives in its own modal: rendered inline it was
            // one horizontal run of names, which ran off the panel as soon as a
            // handful of categories existed.
            if ui
                .button(format!(
                    "🏷 Manage Profiles ({})",
                    self.profiles.profiles.len()
                ))
                .on_hover_text("Review and delete the trained category profiles")
                .clicked()
            {
                // A plain flag, safe to set mid-frame: `render_profiles_modal` runs
                // later in this same frame, and nothing re-enters the toolbar.
                self.show_profiles_modal = true;
            }
            ui.separator();

            ui.label("Train Category from Selected Photos:");
            ui.text_edit_singleline(&mut self.target_training_category);
            if ui.button("🎓 Train Selected").clicked() {
                let cat = self.target_training_category.clone();
                self.train_selected_as_category(&cat);
            }
        });
    }
}
