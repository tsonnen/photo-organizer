//! The settings modal: the confidence threshold, the transfer destination and
//! where to find the CLIP checkpoint.
//!
//! All three are things the user configures once and then forgets, which is
//! why they live behind a modal rather than in the toolbar — the toolbar is for
//! the actions of the moment, and it had no room left for three more controls.
//!
//! The chrome comes from [`super::chrome`]. Like the other modals, this one
//! cannot be open at the same time as the profile or inspection modal: each
//! one's backdrop covers the control that opens the others.

use super::chrome;
use super::layout;
use super::PhotoOrganizerApp;
use crate::inference::find_model_path;
use crate::settings::{Settings, MAX_CONFIDENCE_THRESHOLD, MIN_CONFIDENCE_THRESHOLD};
use eframe::egui;

/// What the settings modal asked the app to do, gathered during the frame and
/// applied after it.
#[derive(Debug, Default)]
struct SettingsActions {
    close: bool,
    /// The confidence threshold moved, so the staged photos need re-classifying.
    threshold_changed: bool,
    /// The user picked a different checkpoint.
    model_changed: bool,
    /// A Browse button was pressed. Carried out once the card has finished
    /// laying out: `rfd`'s dialog is a blocking native call, and blocking
    /// inside a draw would stall egui mid-layout.
    pick_output: bool,
    pick_model: bool,
}

/// Green when a checkpoint is reachable, the app's warning orange when it is
/// not, matching the model status the categories panel shows.
fn model_status_color(available: bool) -> egui::Color32 {
    if available {
        egui::Color32::from_rgb(0, 200, 0)
    } else {
        egui::Color32::from_rgb(220, 150, 0)
    }
}

/// Whether the checkpoint the app will load is the one the user chose.
///
/// `find_model_path` prefers the chosen path but carries on to `models/` when it
/// isn't an existing file, so "something was found" and "what the user asked
/// for is what is running" are different questions. A green dot next to a
/// silently substituted checkpoint is the worse of the two: it reads as
/// confirmation that a stale or mistyped path is fine.
fn chosen_model_in_use(
    chosen: Option<&std::path::Path>,
    resolved: Option<&std::path::Path>,
) -> bool {
    match (chosen, resolved) {
        (Some(chosen), Some(resolved)) => chosen == resolved,
        _ => false,
    }
}

impl PhotoOrganizerApp {
    /// Draws the settings modal over the rest of the UI, if it is open.
    ///
    /// Every edit is written to `settings.json` as it is made, rather than on
    /// a Save button: the settings are independent of one another, so there is
    /// no set of edits to commit together, and a modal that silently discards
    /// them on backdrop-click would be a trap.
    pub(super) fn render_settings_modal(&mut self, ctx: &egui::Context) {
        if !self.show_settings_modal {
            return;
        }

        let card_size = layout::settings_modal_size(ctx.screen_rect().size());
        let mut actions = SettingsActions::default();
        let mut close_button = false;

        // Split out so the closure can take the settings and the derived model
        // path separately, without borrowing all of `self` at once.
        let mut settings = self.settings.clone();
        let resolved_model = find_model_path(settings.model_path.as_deref());

        // Split out so the row can see what the grid was last classified at
        // without borrowing `self` while the card is being drawn.
        let classified_threshold = self.classified_threshold;

        let (_, frame) = chrome::show_modal_card(ctx, "settings_modal_area", card_size, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("⚙ Settings").strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("✖").clicked() {
                        close_button = true;
                    }
                });
            });
            ui.separator();

            Self::render_threshold_row(ui, &mut settings, &mut actions, classified_threshold);
            ui.separator();

            Self::render_output_folder_row(ui, &mut settings, &mut actions);
            ui.separator();

            Self::render_model_row(ui, &mut settings, resolved_model.as_deref(), &mut actions);
        });

        actions.close = frame.close || close_button;

        // The pickers run here, once the card has finished laying out: `rfd`'s
        // dialog is a blocking native call, and blocking inside a draw would
        // stall egui mid-layout. A result wins over whatever the field held,
        // because the dialog is the more deliberate of the two.
        if actions.pick_output {
            if let Some(chosen) = rfd::FileDialog::new()
                .set_title("Choose the output folder")
                .pick_folder()
            {
                settings.output_folder = Some(chosen);
            }
        }

        if actions.pick_model {
            if let Some(chosen) = rfd::FileDialog::new()
                .set_title("Choose the CLIP vision checkpoint")
                .add_filter("SafeTensors weights", &["safetensors"])
                .pick_file()
            {
                settings.model_path = Some(chosen);
                actions.model_changed = true;
            }
        }

        self.apply_settings(settings, actions);
    }

    /// The confidence threshold, and what moving it will do.
    ///
    /// The re-classification fires once per *settled* move, not on every frame
    /// the value changes: dragging the slider across its range would otherwise
    /// re-run classification over the whole grid dozens of times a second and
    /// rewrite the status line each time.
    ///
    /// "Settled" is what makes this more than a `drag_stopped()` check. egui
    /// applies the pointer position to a slider on the press frame and on every
    /// frame the handle travels, so the value has already reached its resting
    /// place by the time the drag stops — the release frame carries no change at
    /// all, and would fire for nothing. And `drag_stopped` never fires at all
    /// for the arrow keys, which is the other way to move a focused slider. So
    /// the test is: no drag in flight, and the value differs from the one the
    /// grid was last classified at.
    fn render_threshold_row(
        ui: &mut egui::Ui,
        settings: &mut Settings,
        actions: &mut SettingsActions,
        classified_threshold: f32,
    ) {
        ui.label(egui::RichText::new("Classification").strong());
        ui.small(
            "How close a photo must be to a trained category before the model \
             is believed over the filename and EXIF rules. Lower it to sort \
             more aggressively; raise it if photos are landing in the wrong \
             category.",
        );
        ui.add_space(4.0);

        let response = ui.add(
            egui::Slider::new(
                &mut settings.confidence_threshold,
                MIN_CONFIDENCE_THRESHOLD..=MAX_CONFIDENCE_THRESHOLD,
            )
            .text("Confidence threshold")
            .step_by(0.01)
            .trailing_fill(true)
            .fixed_decimals(2),
        );

        let still_dragging = response.dragged() || response.drag_started();
        if !still_dragging && settings.confidence_threshold != classified_threshold {
            actions.threshold_changed = true;
        }
    }

    /// Where transfers file approved photos.
    ///
    /// Editable as well as browsable: a destination is a path a user often
    /// already has in their clipboard, and making them reach for a file
    /// dialog to paste it would be a needless step.
    fn render_output_folder_row(
        ui: &mut egui::Ui,
        settings: &mut Settings,
        actions: &mut SettingsActions,
    ) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Transfers").strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Browse…").clicked() {
                    actions.pick_output = true;
                }
            });
        });
        ui.small("Approved photos are filed into <output>/<Category>/<Year>/<Month>/.");
        ui.add_space(4.0);

        // The button rides on the header rather than beside the field. Sharing
        // one row would mean the field and the button negotiating for the same
        // width, and `with_layout` does not advance the parent's cursor — so
        // the field would end up laid out past the button and off the card.
        let mut current = settings
            .output_folder
            .as_deref()
            .map(|p| p.display().to_string())
            .unwrap_or_default();

        let response = ui.add(
            egui::TextEdit::singleline(&mut current)
                .desired_width(f32::INFINITY)
                .hint_text("No output folder chosen"),
        );

        if response.changed() {
            settings.output_folder = match current.trim() {
                "" => None,
                path => Some(std::path::PathBuf::from(path)),
            };
        }
    }

    /// Which CLIP checkpoint to load, and whether it was found.
    ///
    /// The picker is deliberately optional: with nothing chosen the app
    /// auto-detects, which is what every install relies on. The path is shown
    /// read-only because a model is chosen from disk, not typed.
    fn render_model_row(
        ui: &mut egui::Ui,
        settings: &mut Settings,
        resolved: Option<&std::path::Path>,
        actions: &mut SettingsActions,
    ) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Model").strong());
            ui.separator();
            if resolved.is_some() {
                ui.colored_label(model_status_color(true), "● Ready");
            } else {
                ui.colored_label(model_status_color(false), "▲ Missing (Rules Only)");
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if settings.model_path.is_some() && ui.button("Auto-detect").clicked() {
                    settings.model_path = None;
                    actions.model_changed = true;
                }
                if ui.button("Browse…").clicked() {
                    actions.pick_model = true;
                }
            });
        });
        ui.small(
            "The CLIP vision checkpoint. Left on auto-detect, the app looks in \
             models/ next to the binary.",
        );
        ui.add_space(4.0);

        let mut current = match settings.model_path.as_deref() {
            Some(path) => path.display().to_string(),
            None => match resolved {
                Some(path) => format!("Auto-detected: {}", path.display()),
                None => "Auto-detect (nothing found)".to_string(),
            },
        };

        ui.add(
            egui::TextEdit::singleline(&mut current)
                .desired_width(f32::INFINITY)
                .hint_text("No model found")
                // Read-only: a checkpoint is picked from disk, and a
                // half-typed path would only ever resolve to nothing.
                .interactive(false),
        );

        if settings.model_path.is_some()
            && !chosen_model_in_use(settings.model_path.as_deref(), resolved)
        {
            // Worth saying out loud, and worth saying whenever *something*
            // resolved: a path the user chose that the app quietly replaced —
            // or could not read, and so fell back to rules — looks the same as
            // a healthy install otherwise.
            ui.colored_label(
                egui::Color32::from_rgb(240, 180, 0),
                "⚠ That file can't be used — falling back to the standard locations.",
            );
        }
    }

    /// Writes back whatever the modal changed, and does the work that follows.
    fn apply_settings(&mut self, settings: Settings, actions: SettingsActions) {
        let model_changed = actions.model_changed;
        let threshold_changed = actions.threshold_changed;
        let settings_changed = settings != self.settings;
        self.settings = settings;

        if model_changed {
            // Cached embeddings were produced by whichever checkpoint was
            // loaded before. Similarities are only meaningful within one
            // model's embedding space, so reusing the old ones would classify
            // against a bar the new model never set — wrong answers with
            // nothing on screen to say so.
            self.invalidate_embedding_cache();
            self.model_available =
                crate::inference::is_model_available(self.settings.model_path.as_deref());
        }

        if settings_changed {
            self.save_settings();
        }

        if actions.close {
            self.show_settings_modal = false;
        }

        if threshold_changed {
            if self.is_processing {
                // The scan classifies with the threshold it started with, so
                // re-running it now would only cover the photos staged so far
                // and leave the rest sorted against a bar the grid no longer
                // shows. Hold the request until the scan finishes.
                self.pending_reclassify = true;
            } else {
                self.reclassify_all();
            }
        }
    }

    /// Deletes the embedding cache, leaving `profiles.json` alone.
    ///
    /// Only the cache goes: the trained centroids were derived from the old
    /// model's embeddings too, so they are stale as well — but they are the
    /// user's work, and silently discarding them would be far worse than
    /// leaving them for the user to retrain deliberately. The status line says
    /// so.
    fn invalidate_embedding_cache(&mut self) {
        let cache = std::path::Path::new("photo_cache.db");
        let removed = match std::fs::remove_file(cache) {
            Ok(()) => true,
            // Already gone is the state we wanted.
            Err(e) => e.kind() == std::io::ErrorKind::NotFound,
        };

        if removed {
            self.set_warning(
                "🔄 Model changed: cleared the photo cache so embeddings are recomputed. \
                 Re-scan the folder, and retrain any categories if the new model sorts \
                 them differently.",
            );
        } else {
            // The old embeddings stay on disk and the next scan will read them,
            // which is the mismatch this deletion exists to prevent — most
            // likely because a scan holds the database open (a sharing
            // violation on Windows). Staying silent would leave the swap
            // looking exactly as clean as the successful one.
            self.set_warning(
                "⚠ Model changed, but the photo cache could not be deleted, so the old \
                 model's embeddings are still in use. Stop any running scan and try \
                 again, or delete photo_cache.db by hand before re-scanning.",
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::chosen_model_in_use;
    use std::path::Path;

    const CHOSEN: &str = "/models/clip_vision.safetensors";
    const FALLBACK: &str = "/usr/share/photo-organizer/clip_vision.safetensors";

    #[test]
    fn a_chosen_path_is_in_use_only_when_it_is_what_resolves() {
        assert!(
            chosen_model_in_use(Some(Path::new(CHOSEN)), Some(Path::new(CHOSEN))),
            "the chosen path resolved, so it is what is loading"
        );
    }

    #[test]
    fn a_silently_substituted_checkpoint_is_not_the_chosen_one() {
        // The case the warning exists for. Something *was* found, so a green
        // dot reads as confirmation that the chosen path is fine — while the
        // app quietly runs a different checkpoint than the one the user picked.
        assert!(!chosen_model_in_use(
            Some(Path::new("/nonexistent/clip_vision.safetensors")),
            Some(Path::new(FALLBACK)),
        ));
    }

    #[test]
    fn a_chosen_path_that_resolves_to_nothing_is_not_in_use() {
        assert!(!chosen_model_in_use(Some(Path::new(CHOSEN)), None));
    }

    #[test]
    fn nothing_is_chosen_when_the_user_left_it_on_auto_detect() {
        assert!(!chosen_model_in_use(None, Some(Path::new(FALLBACK))));
        assert!(!chosen_model_in_use(None, None));
    }
}
