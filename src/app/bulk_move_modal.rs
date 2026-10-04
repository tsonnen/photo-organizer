//! The bulk move modal: file a whole selection under one name the user typed.
//!
//! This is the answer to a question the app could not previously be asked. Every
//! other route to a category either trains one (**🎓 Train**, which builds a
//! centroid and therefore takes several photos to mean anything) or edits one
//! photo at a time (**Other**, a text box on a single grid cell). Neither suits
//! "these are all from the same weekend": filing 200 photos that way is 200
//! trips through a dropdown, and training a category for a single event puts a
//! profile in `profiles.json` that describes nothing reusable.
//!
//! So the name here is *just a name*. It becomes a
//! [`ClassificationSource::Manual`] pick, which no later re-classification
//! touches, and it never becomes a profile.
//!
//! Two things follow from that, and both are the point of the feature:
//!
//! - **The name is offered again next time.** [`Settings::custom_categories`]
//!   remembers the names used, newest first, so a second event is a click rather
//!   than a retype.
//! - **The user chooses the folder.** The default is the configured output
//!   folder, keeping the `<Category>/<YYYY>/<MM>/` layout, but a folder picked
//!   here overrides it for this batch alone — an event that straddles a year
//!   boundary otherwise splits across two year folders, which is right for the
//!   library and wrong for one trip.

use super::chrome;
use super::layout;
use super::models::StagedItem;
use super::PhotoOrganizerApp;
use crate::category_name::CategoryName;
use crate::transfer::TransferMode;
use eframe::egui;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// What the bulk move modal asked the app to do, gathered during the frame and
/// applied after it.
///
/// The same collect-then-act shape the other modals use: a move re-plans and
/// re-files the selection, which must not happen part-way through drawing the
/// card that asked for it.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct BulkMoveActions {
    close: bool,
    /// Label the selection and leave it staged.
    assign: bool,
    /// File the selection under the typed name.
    transfer: Option<TransferMode>,
    /// A folder was chosen for this batch. Carried out after the card has
    /// finished laying out: `rfd`'s dialog is a blocking native call, and
    /// blocking inside a draw would stall the UI mid-layout.
    pick_folder: bool,
    /// Go back to the configured output folder after a one-off override.
    use_output_folder: bool,
}

/// The width the category name field claims, in points.
///
/// Wide enough for the names this feature exists to type ("Beach Trip 2024"),
/// and narrower than the card so the row it shares has room for the recalled
/// names beside it.
const NAME_FIELD_WIDTH: f32 = 260.0;

/// A photo's filing folder, as the user should expect to find it.
///
/// `<base>/<name>/<year>/<month>/` — the layout the rest of the app promises,
/// with `name` already through [`CategoryName`], so it is one directory and
/// cannot escape `base`.
fn destination_for(base: &Path, name: &CategoryName, item: &StagedItem) -> PathBuf {
    base.join(name.as_str())
        .join(format!("{:04}", item.year))
        .join(format!("{:02}", item.month))
}

/// The distinct `<year>/<month>` folders a selection would land in, ordered.
///
/// Ordered rather than counted because a trip crossing midnight on the 1st files
/// into two folders, and the count alone would not say which. Deduped because
/// the usual case is 200 photos in one month, which is one folder.
fn month_folders(items: &[&StagedItem], name: &CategoryName, base: &Path) -> Vec<PathBuf> {
    items
        .iter()
        .map(|item| destination_for(base, name, item))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// The `Settings` half of the modal's state.
///
/// Taken by value and put back after the frame, the way the settings modal does
/// it, so the card can be drawn against a draft without borrowing all of `self`.
#[derive(Debug, Clone, Default)]
struct BulkMoveDraft {
    name: String,
    folder: Option<PathBuf>,
}

/// Why an action cannot run, or `None` when it can.
///
/// The two things a batch needs are not the same requirement: labelling a
/// selection is not a transfer, so it needs no destination, while Move and Copy
/// need both. Collapsing that into one "ready" flag is what made the disabled
/// tooltip able to name only one of the two things missing.
fn missing_requirement(
    has_name: bool,
    has_destination: bool,
    needs_destination: bool,
) -> Option<&'static str> {
    if !has_name {
        return Some("Type a category name first");
    }
    if needs_destination && !has_destination {
        return Some("Choose a destination folder first");
    }
    None
}

/// Plain grey for a destination that is set, the app's warning orange for one
/// that is not — the same orange the footer and the categories panel use, so
/// "nowhere to put these" looks the same everywhere it can happen.
fn destination_color(base: Option<&Path>) -> egui::Color32 {
    match base {
        Some(_) => egui::Color32::LIGHT_GRAY,
        None => egui::Color32::from_rgb(240, 180, 0),
    }
}

/// A button that is live or greyed, and says which when it is greyed.
///
/// `on_hover_text` is deliberately suppressed by egui on a non-interactable
/// widget, so the disabled case has to use `on_disabled_hover_text` — otherwise
/// the obvious spelling produces a dead button that explains nothing, which is
/// the exact dead end the greying exists to avoid.
fn action_button(
    ui: &mut egui::Ui,
    label: &str,
    blocked: Option<&'static str>,
    enabled_hint: &str,
) -> bool {
    let response = match blocked {
        Some(reason) => ui
            .add_enabled(false, egui::Button::new(label))
            .on_disabled_hover_text(reason),
        None => ui
            .add_enabled(true, egui::Button::new(label))
            .on_hover_text(enabled_hint),
    };
    response.clicked()
}

impl PhotoOrganizerApp {
    /// Draws the bulk move modal over the rest of the UI, if it is open.
    pub(super) fn render_bulk_move_modal(&mut self, ctx: &egui::Context) {
        if !self.show_bulk_move_modal {
            return;
        }

        let card_size = layout::bulk_move_modal_size(ctx.screen_rect().size());
        let mut actions = BulkMoveActions::default();
        let mut close_button = false;

        // Split out so the closure can read the selection and the remembered
        // names without borrowing all of `self` while the card is drawn.
        let selected: Vec<&StagedItem> = self.items.iter().filter(|i| i.selected).collect();
        let remembered: Vec<String> = self.settings.custom_categories.clone();
        let output_folder = self.settings.output_folder.clone();
        let mut draft = BulkMoveDraft {
            name: std::mem::take(&mut self.bulk_move_name),
            folder: self.bulk_move_folder.clone(),
        };

        // The name is sanitised once, here, so everything downstream agrees:
        // the folder the preview shows and the folder the engine builds are the
        // same one. `CategoryName` is the single place that decides what a
        // category may contain, and a name typed as `A/B` becomes `A-B` rather
        // than silently adding a directory level.
        let name = CategoryName::from_user_input(&draft.name);
        let has_name = !draft.name.trim().is_empty();
        // A chosen folder wins over the configured one for this batch only. The
        // alternative — offering a picker with no way back — would leave the
        // user having to remember which way round the override went.
        let base = draft.folder.clone().or(output_folder.clone());
        let has_destination = base.is_some();

        let (_, frame) = chrome::show_modal_card(ctx, "bulk_move_modal_area", card_size, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("🏷 Bulk Move").strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("✖").clicked() {
                        close_button = true;
                    }
                });
            });
            ui.separator();

            Self::render_name_row(ui, &mut draft.name, &remembered);
            ui.separator();

            Self::render_destination_row(
                ui,
                base.as_deref(),
                draft.folder.is_some(),
                output_folder.as_deref(),
                &mut actions,
            );
            ui.separator();

            Self::render_preview(ui, &selected, &name, has_name, base.as_deref());
            ui.separator();

            Self::render_action_row(ui, has_name, has_destination, &mut actions);
        });

        actions.close = frame.close || close_button;

        // Opened once the card has finished laying out: `rfd`'s dialog blocks,
        // and a blocking call inside a draw stalls egui mid-layout.
        if actions.pick_folder {
            if let Some(chosen) = rfd::FileDialog::new()
                .set_title("Choose where this batch should go")
                .pick_folder()
            {
                draft.folder = Some(chosen);
            }
        }

        self.bulk_move_name = draft.name;
        self.bulk_move_folder = draft.folder;

        if actions.use_output_folder {
            self.bulk_move_folder = None;
        }

        if actions.close {
            self.show_bulk_move_modal = false;
            // The folder is per-batch state and does not outlive the modal. The
            // name does: reopening after a mistyped move should not mean
            // retyping the one thing the user already got right.
            self.bulk_move_folder = None;
            return;
        }

        if actions.assign {
            // `assign_selected_to_category` says so itself when it refused, so
            // the modal stays open with the reason on screen rather than closing
            // over a warning.
            if self.assign_selected_to_category(&name, has_name).is_some() {
                self.close_bulk_move();
            }
            return;
        }

        if let Some(mode) = actions.transfer {
            self.bulk_transfer(mode, &name, base, has_name);
        }
    }

    /// The category name field, with the names used before offered beside it.
    fn render_name_row(ui: &mut egui::Ui, name: &mut String, remembered: &[String]) {
        ui.label(egui::RichText::new("Category name").strong());
        ui.small("A name of your own. Nothing is trained, and the photos keep it through any later re-classification.");
        ui.add_space(4.0);

        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(name)
                    .hint_text("Beach Trip 2024")
                    .desired_width(NAME_FIELD_WIDTH),
            );

            // The recall list is what makes a second batch cheap, so it sits
            // next to the field rather than behind a menu: it is at most
            // `MAX_REMEMBERED_CATEGORIES` entries and the recent ones are the
            // ones wanted.
            if !remembered.is_empty() {
                egui::ComboBox::from_id_salt("bulk_move_recall")
                    .selected_text("Recent names")
                    .show_ui(ui, |ui| {
                        for previous in remembered {
                            if ui
                                .selectable_label(
                                    previous.eq_ignore_ascii_case(name.trim()),
                                    previous,
                                )
                                .clicked()
                            {
                                *name = previous.clone();
                            }
                        }
                    });
            }
        });
    }

    /// Where the batch will go, and the one-off override.
    fn render_destination_row(
        ui: &mut egui::Ui,
        base: Option<&Path>,
        overridden: bool,
        output_folder: Option<&Path>,
        actions: &mut BulkMoveActions,
    ) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Destination").strong());

            match base {
                Some(path) => {
                    ui.colored_label(destination_color(Some(path)), path.display().to_string())
                        .on_hover_text(path.display().to_string());
                }
                None => {
                    // Said plainly rather than left blank: with the picker moved
                    // out of the toolbar, an unconfigured destination is one
                    // click from invisible.
                    ui.colored_label(
                        destination_color(None),
                        "⚠ No destination — choose one here or set an output folder in Settings",
                    );
                }
            }

            if ui.button("Choose Folder…").clicked() {
                actions.pick_folder = true;
            }

            // Only offered when there is an override to take back, so the row
            // does not carry a button that does nothing. With no output folder
            // configured there would be nothing to go back to.
            if overridden && output_folder.is_some() && ui.button("Use Output Folder").clicked() {
                actions.use_output_folder = true;
            }
        });
    }

    /// Exactly where the selected photos will land, before anything is filed.
    fn render_preview(
        ui: &mut egui::Ui,
        selected: &[&StagedItem],
        name: &CategoryName,
        has_name: bool,
        base: Option<&Path>,
    ) {
        if selected.is_empty() {
            ui.colored_label(
                egui::Color32::from_rgb(240, 180, 0),
                "⚠ Nothing is selected, so there is nothing to move",
            );
            return;
        }

        if !has_name {
            ui.small("Type a name to see where these will go.");
            return;
        }

        let Some(base) = base else {
            ui.small("Choose a destination to see where these will go.");
            return;
        };

        let folders = month_folders(selected, name, base);
        match folders.as_slice() {
            [] => {}
            // The usual case, and the only one that can be stated in one line.
            [only] => {
                ui.colored_label(
                    destination_color(Some(base)),
                    format!("{} photo(s) → {}", selected.len(), only.display()),
                );
            }
            // A selection spanning months files into more than one folder, and
            // saying so is the difference between a preview and a surprise.
            many => {
                let listed: Vec<String> = many
                    .iter()
                    .take(3)
                    .map(|p| p.display().to_string())
                    .collect();
                let rest = many.len().saturating_sub(listed.len());
                let suffix = if rest > 0 {
                    format!(" and {rest} more")
                } else {
                    String::new()
                };
                ui.colored_label(
                    destination_color(Some(base)),
                    format!(
                        "{} photo(s) → {} folders: {}{}",
                        selected.len(),
                        many.len(),
                        listed.join(", "),
                        suffix
                    ),
                );
            }
        }
    }

    /// The three ways out: label it, file it, or change your mind.
    fn render_action_row(
        ui: &mut egui::Ui,
        has_name: bool,
        has_destination: bool,
        actions: &mut BulkMoveActions,
    ) {
        ui.horizontal(|ui| {
            if action_button(
                ui,
                "🏷 Assign Only",
                // Assign is not a transfer, so a missing destination is nothing
                // to it: it only needs a name.
                missing_requirement(has_name, has_destination, false),
                "Label the selected photos and leave them staged",
            ) {
                actions.assign = true;
            }

            ui.separator();

            // Move and Copy need both, and each says which of the two it is
            // missing rather than just sitting greyed.
            let transfer_blocked = missing_requirement(has_name, has_destination, true);
            if action_button(
                ui,
                "🚀 Move Selected",
                transfer_blocked,
                "File the selected photos under this name, then drop them from the grid",
            ) {
                actions.transfer = Some(TransferMode::Move);
            }
            if action_button(
                ui,
                "📋 Copy Selected",
                transfer_blocked,
                "Copy the selected photos under this name, leaving the originals alone",
            ) {
                actions.transfer = Some(TransferMode::Copy);
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Cancel").clicked() {
                    actions.close = true;
                }
            });
        });
    }

    /// Closes the modal, leaving the typed name for next time.
    fn close_bulk_move(&mut self) {
        self.show_bulk_move_modal = false;
        self.bulk_move_folder = None;
    }

    /// Labels every selected photo `name`, training nothing.
    ///
    /// `None` when there is nothing to do — the name is blank — so the caller
    /// can tell "no photos carry this name" from "there was no name", which
    /// read identically in a status message otherwise.
    ///
    /// Manual is the point. A `Manual` pick is the one classification
    /// `apply_classification` will not overwrite, so these photos keep this
    /// name through **Re-classify All**, a threshold move, and any scan still
    /// running — which is what makes a batch decision stick.
    ///
    /// "Any scan still running" is the clause that needs
    /// [`StagedItem::mark_manual_over_pending`] rather than
    /// [`StagedItem::mark_manual`]. A photo the model has not reached is not yet a
    /// photo with a category, so `mark_manual` refuses to claim it and the model's
    /// answer lands on top of the name written here — silently, after the status
    /// line has already reported every photo as assigned. This is the one route
    /// that arrives with a name the user actually chose, so it claims the pending
    /// ones too, and the count below is then the number of photos that really do
    /// carry the name.
    fn assign_selected_to_category(
        &mut self,
        name: &CategoryName,
        has_name: bool,
    ) -> Option<usize> {
        if !has_name {
            self.set_warning("Type a category name first.");
            return None;
        }

        let is_custom = self.profiles.is_custom_category(name.as_str());
        let mut assigned = 0;
        for item in &mut self.items {
            if !item.selected {
                continue;
            }
            item.category = name.as_str().to_string();
            item.is_custom = is_custom;
            item.mark_manual_over_pending();
            assigned += 1;
        }

        if assigned == 0 {
            self.set_warning("No photos selected. Check at least one photo.");
            return None;
        }

        // Remembered so the next batch of the same kind is a click rather than a
        // retype. Sanitised name, not the raw text, so what the list offers next
        // time is exactly what will be used as a folder.
        let remembered = name.as_str().to_string();
        self.settings.remember_custom_category(&remembered);
        self.save_settings();

        self.set_status(
            format!("🏷 Assigned {assigned} photo(s) to '{remembered}'. Nothing was trained."),
            egui::Color32::from_rgb(180, 100, 220),
        );
        Some(assigned)
    }

    /// Files the selected photos under `name`, into `base`.
    ///
    /// Assigns first, so the transfer plans against the name just typed and the
    /// grid's labels agree with where the files went. A photo the move fails on
    /// keeps the name and stays selected, which is what makes the retry a single
    /// click rather than a re-type.
    fn bulk_transfer(
        &mut self,
        mode: TransferMode,
        name: &CategoryName,
        base: Option<PathBuf>,
        has_name: bool,
    ) {
        let Some(base) = base else {
            self.set_warning("Choose a destination folder first.");
            return;
        };
        if !has_name {
            self.set_warning("Type a category name first.");
            return;
        }
        if self.assign_selected_to_category(name, has_name).is_none() {
            return;
        }

        self.execute_transfer_to(mode, base, Some(name.clone()));
        self.close_bulk_move();
    }
}
