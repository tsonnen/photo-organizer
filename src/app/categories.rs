//! Category classification, labels and the widgets that edit them.
//!
//! Everything here is about "which category does this photo belong to":
//! the heuristic pass, training from exemplars, and the combo box plus custom
//! name input the user edits in both views.

use crate::app::layout::{grid_cell_combo_width, grid_cell_input_width, MODAL_COMBO_WIDTH};
use crate::app::models::{ModalActions, StagedItem};
use crate::classification::ClassificationSource;
use crate::profile_store::{ProfileStore, RankedProfile};
use eframe::egui;

impl super::PhotoOrganizerApp {
    /// Re-runs classification over every staged photo, leaving manual picks
    /// alone, and reports what changed in the status line.
    pub fn reclassify_all(&mut self) {
        let mut visual_count = 0;
        let total_count = self.items.len();
        for item in &mut self.items {
            let facts = item.facts();
            let classification = self
                .profiles
                .classify(&facts, self.settings.confidence_threshold);
            if classification
                .decided()
                .is_some_and(|d| d.source == ClassificationSource::VisualModel)
            {
                visual_count += 1;
            }
            // The same write a scan's `Update` makes, so a manual pick is
            // preserved here exactly as it is there. The facts come off the
            // staged item, which carries the frame size the scan read — not the
            // thumbnail's, which is what used to rule a screenshot back out of
            // Screenshots the moment the user hit this button.
            item.apply_classification(&self.profiles, &facts, classification);
        }

        // The grid now agrees with the threshold, so the slider's next move has
        // something to compare its settled value against.
        self.classified_threshold = self.settings.confidence_threshold;

        if total_count > 0 {
            self.status_message = Some((
                format!(
                    "⚡ Re-classified {} photo(s) ({} visual AI match(es), {} active category profile(s), threshold {:.2})",
                    total_count,
                    visual_count,
                    self.profiles.profiles.len(),
                    self.settings.confidence_threshold,
                ),
                egui::Color32::from_rgb(180, 220, 255),
            ));
        }
    }

    /// Adds every selected photo with an embedding as an exemplar for
    /// `category`, then re-classifies so the new profile takes effect.
    pub fn train_selected_as_category(&mut self, category: &str) {
        let category = category.trim();
        if category.is_empty() {
            self.set_warning("Please enter a category name to train.");
            return;
        }

        let selected_count = self.items.iter().filter(|i| i.selected).count();
        if selected_count == 0 {
            self.set_warning("No photos selected to train. Check at least one photo.");
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
                    "⚠️ Could not train '{category}': Selected photo(s) have no visual embeddings. Choose a CLIP checkpoint in Settings."
                ),
                egui::Color32::from_rgb(240, 70, 70),
            ));
        } else {
            self.save_profiles();
            self.reclassify_all();
            self.status_message = Some((
                format!(
                    "✅ Successfully trained category '{category}' from {trained_count} photo(s)!"
                ),
                egui::Color32::from_rgb(40, 200, 40),
            ));
        }
    }

    /// Trains a single photo as an exemplar for `category`.
    pub fn train_single_item(&mut self, item_index: usize, category: &str) {
        let category = category.trim();
        if category.is_empty() {
            self.set_warning("Category name cannot be empty.");
            return;
        }

        if let Some(item) = self.items.get(item_index) {
            if item.embedding.is_empty() {
                self.status_message = Some((
                    format!(
                        "⚠️ Cannot train '{category}': Photo has no visual embedding. Choose a CLIP checkpoint in Settings."
                    ),
                    egui::Color32::from_rgb(240, 70, 70),
                ));
                return;
            }
            let emb = item.embedding.clone();
            self.profiles.add_exemplar(category, &emb);
            self.save_profiles();
            self.reclassify_all();
            self.status_message = Some((
                format!("✅ Successfully trained category '{category}' from this photo!"),
                egui::Color32::from_rgb(40, 200, 40),
            ));
        }
    }

    /// A profile name plus its confidence, when the photo has an embedding to
    /// be scored against.
    fn profile_label(name: &str, confidence: f32, has_embedding: bool) -> String {
        if has_embedding {
            format!("{} ({:.0}%)", name, confidence * 100.0)
        } else {
            name.to_string()
        }
    }

    /// Text shown in a closed category combo box.
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

    /// The category combo box, shared by the grid cell and the modal.
    ///
    /// Choosing a ranked profile copies its confidence and marks the item
    /// manual, so a later re-classify will not undo the pick. "Other" flips
    /// to the custom path without clearing the current name.
    pub(super) fn render_category_selector(
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
                    item.is_custom = false;
                    item.mark_manual();
                }
            }
            if !ranked_profiles.is_empty() {
                ui.separator();
            }
            if ui.selectable_label(item.is_custom, "Other").clicked() {
                item.is_custom = true;
                item.mark_manual();
            }
        });
    }

    /// The free-text category name input, shown only on the custom path.
    ///
    /// Both the grid cell and the modal render this, so a custom category is
    /// editable in either view.
    pub(super) fn render_custom_category_input(
        ui: &mut egui::Ui,
        item: &mut StagedItem,
        input_width: f32,
    ) {
        let custom_input = ui.add(
            egui::TextEdit::singleline(&mut item.category)
                .hint_text("Custom category...")
                .desired_width(input_width),
        );
        if custom_input.changed() {
            item.mark_manual();
        }
    }

    /// Renders one grid cell's category controls and returns the category to
    /// train if the train button was clicked.
    ///
    /// The combo and train button share a row; the custom category input, when
    /// the item is custom, goes on the row *below*. That stacking is load
    /// bearing: sharing a single row starves the input down to whatever sliver
    /// is left after the combo, and forces the cell wider than its grid column.
    pub(super) fn render_grid_cell_controls(
        profiles: &ProfileStore,
        ui: &mut egui::Ui,
        item: &mut StagedItem,
        item_width: f32,
        combo_id: egui::Id,
    ) -> Option<String> {
        let mut train_request = None;
        let combo_width = grid_cell_combo_width(item_width);

        ui.horizontal(|ui| {
            Self::render_category_selector(profiles, ui, item, combo_id, Some(combo_width));

            if ui
                .add_sized(
                    [
                        super::layout::TRAIN_BUTTON_SIZE,
                        super::layout::TRAIN_BUTTON_SIZE,
                    ],
                    egui::Button::new("🎓"),
                )
                .on_hover_text("Train category from this photo")
                .clicked()
            {
                train_request = Some(item.category.clone());
            }
        });

        if item.is_custom {
            let category_input_width = grid_cell_input_width(item_width);
            Self::render_custom_category_input(ui, item, category_input_width);
        }

        train_request
    }

    /// Renders the inspection modal's bottom control row and reports which
    /// control was pressed.
    ///
    /// The custom category input shares this line with the combo, immediately
    /// after it, because the modal has a whole window's worth of width to spend
    /// on one row. A grid cell cannot: its line is only a column wide, so
    /// `render_grid_cell_controls` keeps the same input on the row below.
    pub(super) fn render_modal_controls(
        profiles: &ProfileStore,
        ui: &mut egui::Ui,
        item: &mut StagedItem,
        modal_index: usize,
        item_count: usize,
    ) -> ModalActions {
        let mut actions = ModalActions::default();

        ui.horizontal(|ui| {
            if ui.button("◀ Previous (Left)").clicked() {
                actions.prev = true;
            }
            ui.label(format!("{}/{}", modal_index + 1, item_count));
            if ui.button("Next (Right) ▶").clicked() {
                actions.next = true;
            }

            ui.separator();
            ui.label("Category:");

            Self::render_category_selector(
                profiles,
                ui,
                item,
                ui.make_persistent_id(("modal_cat_combo", modal_index, &item.source_path)),
                Some(MODAL_COMBO_WIDTH),
            );

            if item.is_custom {
                Self::render_custom_category_input(ui, item, super::layout::modal_input_width(ui));
            }

            if ui
                .button("🎓 Train")
                .on_hover_text("Train category from this photo")
                .clicked()
            {
                actions.train = true;
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Close (Esc)").clicked() {
                    actions.close = true;
                }
            });
        });

        actions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_input_rendered_by_both_grid_and_modal() {
        // Both the grid cell and the inspection modal must render the custom input,
        // otherwise a custom category becomes uneditable in one of the two views.
        // Both call sites live in this file: one per view.
        let source = include_str!("categories.rs");
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
    fn is_custom_category_is_case_insensitive() {
        let mut store = ProfileStore::default();
        store.add_exemplar("Sunsets", &[1.0, 0.0, 0.0]);

        // Known profile names are not custom, and matching ignores case.
        assert!(!store.is_custom_category("Sunsets"));
        assert!(!store.is_custom_category("sunsets"));
        // Unknown names are custom.
        assert!(store.is_custom_category("Beach Trip"));
    }
}
