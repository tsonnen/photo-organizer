//! The top toolbar: folder pickers, selection, transfer actions, the sort and
//! filter controls, and the collapsible AI & categories panel.

use super::models::{Filters, SortBy};
use super::view;
use super::PhotoOrganizerApp;
use crate::transfer::TransferMode;
use eframe::egui;

/// Width of the sort combo, in points. Sized to its widest option, "Date Taken".
const SORT_COMBO_WIDTH: f32 = 110.0;

/// Width of a date picker in the filter panel, in points. A year is four digits
/// and a month is two, so both fit well inside this.
const DATE_PICKER_WIDTH: f32 = 64.0;

/// Width of the category combo in the filter panel, in points. Category names are
/// the longest labels the app renders anywhere, so this is the one picker that
/// gets extra room; it clips rather than widening the toolbar past the window.
const CATEGORY_PICKER_WIDTH: f32 = 160.0;

/// The pickers' "no bound" and "no filter" entries.
const ANY: &str = "Any";
const ALL: &str = "All categories";

impl PhotoOrganizerApp {
    pub(super) fn render_toolbar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            self.render_toolbar_actions(ui, ctx);
            self.render_status_line(ui);
            self.render_sort_row(ui);
            if self.show_filter_panel {
                self.render_filter_panel(ui);
            }
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
            // either: it moves the last moved batch home again, or deletes the
            // copies the last copied batch made.
            let undo = ui.button("↩ Undo").on_hover_text(
                "Reverse the last transfer: move a moved batch home again, or delete the copies a copied batch made. Files you have edited since are left alone.",
            );
            if undo.clicked() {
                self.undo_last_transfer();
            }

            ui.separator();

            // The panel is open, or something is narrowing the grid: either way
            // the button says so. An open panel on its own would go stale the
            // moment a filter was cleared from inside it, leaving a button that
            // reads as inactive while its controls are still on screen.
            let filters_active = !self.filters.is_open();
            let filter_text = match (self.show_filter_panel, filters_active) {
                (false, false) => "🔍 Filters ▼",
                (false, true) => "🔍 Filters ▼ ●",
                (true, _) => "🔍 Filters ▲",
            };
            if ui
                .button(filter_text)
                .on_hover_text(if filters_active {
                    "Narrow which photos the grid shows"
                } else {
                    "No filters are narrowing the grid"
                })
                .clicked()
            {
                self.show_filter_panel = !self.show_filter_panel;
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

    /// The order the grid is in, always visible.
    ///
    /// A permanent row rather than a menu item, because the current order is the
    /// thing a user checks most often — usually to confirm a sort did what they
    /// meant — and a menu hides the answer behind the click that changes it.
    fn render_sort_row(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Sort").strong());

            egui::ComboBox::from_id_salt("sort_by")
                .width(SORT_COMBO_WIDTH)
                .selected_text(self.sort_by.to_string())
                .show_ui(ui, |ui| {
                    for option in [SortBy::DateTaken, SortBy::Confidence, SortBy::Category] {
                        ui.selectable_value(&mut self.sort_by, option, option.to_string());
                    }
                });

            if ui
                .button(view::direction_label(self.sort_by, self.sort_direction))
                .on_hover_text("Reverse the order")
                .clicked()
            {
                self.sort_direction = self.sort_direction.toggled();
            }

            // Only while a filter is actually narrowing something: with the whole
            // folder in view, "400 of 400 shown" is noise.
            let (visible, staged) = self.view_counts();
            if visible < staged {
                ui.separator();
                ui.label(format!("{visible} of {staged} shown"));
            }
        });
    }

    /// The three narrowing controls, behind a toggle.
    ///
    /// Collapsed by default where the sort is not, because three pickers, two
    /// sliders and a clear button is a wall of controls for something a user sets
    /// once and then mostly leaves alone — but unlike the sort, nothing here is
    /// worth reading at a glance, so there is nothing to keep on screen.
    fn render_filter_panel(&mut self, ui: &mut egui::Ui) {
        ui.separator();

        // Read the folder's shape before the row is drawn: both pickers are
        // populated from the staged photos, and borrowing `self.items` while the
        // row mutates `self.filters` would be two borrows of one struct.
        let rows: Vec<view::Row<'_>> = self.items.iter().map(view::Row::from).collect();
        let years = view::year_choices(rows.iter().copied());
        let categories = view::category_choices(rows.iter().copied());

        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Filter").strong());

            Self::render_date_bound(ui, "From", &years, &mut self.filters.date_from, "from");
            Self::render_date_bound(ui, "to", &years, &mut self.filters.date_to, "to");

            ui.separator();
            Self::render_confidence_bounds(ui, &mut self.filters);

            ui.separator();
            Self::render_category_filter(ui, &categories, &mut self.filters);

            ui.separator();
            if ui
                .button("✖ Clear")
                .on_hover_text("Show every staged photo again")
                .clicked()
            {
                self.filters = Filters::default();
            }
        });

        ui.small(
            "Filters combine, and apply to what you can see: photos outside them \
             are not counted below, not ticked by All, and not moved.",
        );
    }

    /// One end of the date range: a year, and once there is one, a month.
    ///
    /// Two pickers rather than one free-text date because the app has no day to
    /// offer — `extract_date` reduces every timestamp to a year and a month — so
    /// a date field asking for more precision than it can store would be a lie.
    fn render_date_bound(
        ui: &mut egui::Ui,
        label: &str,
        years: &[u32],
        bound: &mut Option<(u32, u32)>,
        id: &str,
    ) {
        // Year 0 stands in for "no bound": no photo is dated year 0, so it cannot
        // collide with a real one, and it keeps the pickers free of `Option`
        // handling in the click paths below.
        let (year, month) = bound.unwrap_or((0, 0));

        ui.label(label);
        egui::ComboBox::from_id_salt((id, "year"))
            .width(DATE_PICKER_WIDTH)
            .selected_text(match year {
                0 => ANY.to_string(),
                year => year.to_string(),
            })
            .show_ui(ui, |ui| {
                if ui.selectable_label(year == 0, ANY).clicked() {
                    *bound = None;
                }
                for &candidate in years {
                    if ui
                        .selectable_label(year == candidate, candidate.to_string())
                        .clicked()
                    {
                        // Keeps whichever month was already chosen: only the
                        // year is being changed. The placeholder month resolves
                        // to January, since any month in any year beats none.
                        *bound = Some((candidate, if month == 0 { 1 } else { month }));
                    }
                }
            });

        if year == 0 {
            return;
        }
        egui::ComboBox::from_id_salt((id, "month"))
            .width(DATE_PICKER_WIDTH)
            .selected_text(format!("{month:02}"))
            .show_ui(ui, |ui| {
                for candidate in 1..=12 {
                    if ui
                        .selectable_label(month == candidate, format!("{candidate:02}"))
                        .clicked()
                    {
                        *bound = Some((year, candidate));
                    }
                }
            });
    }

    /// The confidence floor and ceiling, kept from crossing.
    ///
    /// The two bounds are one range, so neither is allowed past the other: a
    /// floor above the ceiling admits nothing at all, and leaves the user hunting
    /// for which of the two sliders to drag back.
    fn render_confidence_bounds(ui: &mut egui::Ui, filters: &mut Filters) {
        ui.label("Confidence");

        let floor = ui.add(
            egui::Slider::new(&mut filters.confidence_from, 0.0..=1.0)
                .trailing_fill(true)
                .fixed_decimals(2),
        );
        let _ceiling = ui.add(
            egui::Slider::new(&mut filters.confidence_to, 0.0..=1.0)
                .trailing_fill(true)
                .fixed_decimals(2),
        );

        // Snap rather than swap: swapping the two would make a handle jump
        // across the track to the other end every time they pass, and the user
        // would lose track of which slider they were holding.
        if filters.confidence_from > filters.confidence_to {
            if floor.changed() {
                filters.confidence_to = filters.confidence_from;
            } else {
                filters.confidence_from = filters.confidence_to;
            }
        }
    }

    /// The category picker, offering every category staged plus a way back to all
    /// of them.
    fn render_category_filter(ui: &mut egui::Ui, categories: &[String], filters: &mut Filters) {
        ui.label("Category");
        egui::ComboBox::from_id_salt("category_filter")
            .width(CATEGORY_PICKER_WIDTH)
            .selected_text(filters.category.as_deref().unwrap_or(ALL).to_string())
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(filters.category.is_none(), ALL)
                    .clicked()
                {
                    filters.category = None;
                }
                for name in categories {
                    if ui
                        .selectable_label(filters.category.as_deref() == Some(name.as_str()), name)
                        .clicked()
                    {
                        filters.category = Some(name.clone());
                    }
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
