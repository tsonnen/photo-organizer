//! The profile management modal: a scrollable list of the trained category
//! profiles, and the two-step delete that removes one.
//!
//! The chrome deliberately mirrors the inspection modal in [`super::modal`] —
//! full-screen backdrop, centred card, Escape and backdrop-click to close —
//! because both are "stop and deal with one thing" overlays. The two can never
//! be open at once: each one's backdrop covers the control that opens the
//! other, so there is no ordering between them to get wrong.

use super::layout;
use super::PhotoOrganizerApp;
use crate::profile_store::{CategoryProfile, ProfileStore};
use eframe::egui;

/// Tint of an armed delete row: the app's warning orange, washed out for the
/// row's background and used at full strength for its text.
fn armed_color() -> egui::Color32 {
    egui::Color32::from_rgb(240, 180, 0)
}

/// Which profile's delete is armed, if any.
///
/// Deleting a profile throws its trained centroid away for good, so a click on
/// ❌ only arms the row; the deletion happens on a second, explicit click. Held
/// here rather than in the render closure so the state machine can be tested
/// without an egui context.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct DeletePrompt {
    armed: Option<String>,
}

impl DeletePrompt {
    /// Arms a row, replacing any previously armed one: only one confirmation is
    /// ever outstanding, so it is never ambiguous which profile is about to go.
    pub(super) fn arm(&mut self, name: &str) {
        self.armed = Some(name.to_string());
    }

    pub(super) fn is_armed(&self, name: &str) -> bool {
        self.armed.as_deref() == Some(name)
    }

    /// Yields the armed profile once and disarms, so a second confirm cannot
    /// delete a profile that has since been re-armed elsewhere.
    pub(super) fn confirm(&mut self) -> Option<String> {
        self.armed.take()
    }

    pub(super) fn cancel(&mut self) {
        self.armed = None;
    }
}

impl PhotoOrganizerApp {
    /// Draws the profile modal over the rest of the UI, if it is open.
    ///
    /// Presses are collected first and acted on after the frame, since
    /// confirming a delete re-classifies every staged photo and that must not
    /// happen mid-draw.
    pub(super) fn render_profiles_modal(&mut self, ctx: &egui::Context) {
        if !self.show_profiles_modal {
            return;
        }

        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.close_profiles_modal();
            return;
        }

        let screen_rect = ctx.screen_rect();
        let card_size = layout::profiles_modal_size(screen_rect.size());
        let card_rect = egui::Rect::from_center_size(screen_rect.center(), card_size);

        let mut close = false;
        let mut delete_request = None;

        // Split out so the frame below borrows the two fields separately.
        let profiles = &self.profiles;
        let prompt = &mut self.delete_prompt;

        egui::Area::new(egui::Id::new("profiles_modal_area"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen_rect.min)
            .show(ctx, |ui| {
                let (backdrop_rect, backdrop_resp) =
                    ui.allocate_exact_size(screen_rect.size(), egui::Sense::click());
                ui.painter()
                    .rect_filled(backdrop_rect, 0.0, egui::Color32::from_black_alpha(200));

                if backdrop_resp.clicked() {
                    if let Some(click) = backdrop_resp.interact_pointer_pos() {
                        if !card_rect.contains(click) {
                            close = true;
                        }
                    }
                }

                ui.scope_builder(egui::UiBuilder::new().max_rect(card_rect), |ui| {
                    // The card is sized on the ui *around* the frame, not
                    // inside it: a window frame already spends part of its
                    // rect on margins and shadow, so asking for the full card
                    // size within it overflows by that much.
                    ui.set_min_size(card_size);
                    ui.set_max_size(card_size);

                    egui::Frame::window(&ctx.style())
                        .rounding(8.0)
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new(format!(
                                        "🏷 Profiles ({})",
                                        profiles.profiles.len()
                                    ))
                                    .strong(),
                                );
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if ui.button("✖").clicked() {
                                            close = true;
                                        }
                                    },
                                );
                            });
                            ui.separator();
                            ui.small(
                                "Deleting a profile discards its trained centroid and \
                                 re-classifies the staged photos.",
                            );
                            ui.separator();

                            delete_request = Self::render_profile_list(
                                profiles,
                                ui,
                                prompt,
                                layout::profiles_list_height(card_size),
                            );
                        });
                });
            });

        if close {
            self.close_profiles_modal();
        } else if let Some(name) = delete_request {
            // The row's Confirm button has already consumed the arm, so all
            // that is left is to delete the profile it named.
            self.remove_profile(&name);
        }
    }

    fn close_profiles_modal(&mut self) {
        self.show_profiles_modal = false;
        self.delete_prompt.cancel();
    }

    /// The scrollable profile rows, returning the profile to delete when a
    /// confirmation button is clicked.
    ///
    /// Split from the modal chrome so the list's geometry can be laid out and
    /// asserted on its own, at any card size.
    pub(super) fn render_profile_list(
        profiles: &ProfileStore,
        ui: &mut egui::Ui,
        prompt: &mut DeletePrompt,
        list_height: f32,
    ) -> Option<String> {
        if profiles.profiles.is_empty() {
            ui.vertical_centered(|ui| {
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new(
                        "No profiles yet.\nClose this and hit 🎓 Train Selected to \
                         build one.",
                    )
                    .color(egui::Color32::GRAY),
                );
                ui.add_space(8.0);
            });
            return None;
        }

        let mut delete_request = None;

        // The scroll area, not the card, is what gives: `max_height` keeps it
        // inside the card, and `auto_shrink` off stops a short list from
        // stretching out to fill the card's whole height.
        egui::ScrollArea::vertical()
            .id_salt("profiles_list")
            .max_height(list_height)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for profile in &profiles.profiles {
                    if let Some(deleted) = render_profile_row(ui, profile, prompt) {
                        delete_request = Some(deleted);
                    }
                }
            });

        delete_request
    }
}

/// One profile row: its name and sample count, and either the delete button or
/// the confirmation that button arms. Returns the profile the row confirmed
/// deleting, taken from the prompt itself so the two can't disagree.
fn render_profile_row(
    ui: &mut egui::Ui,
    profile: &CategoryProfile,
    prompt: &mut DeletePrompt,
) -> Option<String> {
    let armed = prompt.is_armed(&profile.name);
    let mut confirmed = None;

    egui::Frame::none()
        .fill(if armed {
            armed_color().gamma_multiply(0.25)
        } else {
            egui::Color32::TRANSPARENT
        })
        .inner_margin(egui::Margin::symmetric(4.0, 3.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                if armed {
                    ui.colored_label(armed_color(), format!("⚠ Delete \"{}\"?", profile.name));
                } else {
                    ui.label(format!("🏷 {} ({})", profile.name, profile.sample_count));
                }

                // Right-aligned, as in the inspection modal's header.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if armed {
                        if ui
                            .button("✔ Confirm")
                            .on_hover_text("Discard this profile and re-classify")
                            .clicked()
                        {
                            confirmed = prompt.confirm();
                        }
                        if ui.button("✖ Cancel").clicked() {
                            prompt.cancel();
                        }
                    } else if ui
                        .button("❌")
                        .on_hover_text(format!("Delete the {} profile", profile.name))
                        .clicked()
                    {
                        prompt.arm(&profile.name);
                    }
                });
            });
        });

    confirmed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_armed_to_begin_with() {
        let mut prompt = DeletePrompt::default();
        assert!(!prompt.is_armed("Sunsets"));
        assert_eq!(prompt.confirm(), None);
    }

    #[test]
    fn arming_does_not_delete() {
        // The whole point of the two-step: one click is not a deletion.
        let mut prompt = DeletePrompt::default();
        prompt.arm("Sunsets");

        assert!(prompt.is_armed("Sunsets"));
        assert_eq!(prompt.confirm(), Some("Sunsets".to_string()));
    }

    #[test]
    fn only_one_profile_is_armed_at_a_time() {
        let mut prompt = DeletePrompt::default();
        prompt.arm("Sunsets");
        prompt.arm("Receipts");

        assert!(!prompt.is_armed("Sunsets"));
        assert!(prompt.is_armed("Receipts"));
        assert_eq!(prompt.confirm(), Some("Receipts".to_string()));
    }

    #[test]
    fn confirm_yields_the_profile_exactly_once() {
        // A stale confirm click must not delete a profile that has since been
        // armed somewhere else.
        let mut prompt = DeletePrompt::default();
        prompt.arm("Sunsets");

        assert_eq!(prompt.confirm(), Some("Sunsets".to_string()));
        assert!(!prompt.is_armed("Sunsets"));
        assert_eq!(prompt.confirm(), None);
    }

    #[test]
    fn cancelling_disarms() {
        let mut prompt = DeletePrompt::default();
        prompt.arm("Sunsets");
        prompt.cancel();

        assert!(!prompt.is_armed("Sunsets"));
        assert_eq!(prompt.confirm(), None);
    }

    #[test]
    fn arming_matches_the_row_own_name() {
        // `ProfileStore::remove_category` matches names case-insensitively, so
        // the arm is held against the row's own spelling of the name.
        let mut prompt = DeletePrompt::default();
        prompt.arm("Sunsets");

        assert!(prompt.is_armed("Sunsets"));
        assert!(!prompt.is_armed("sunsets"));
    }
}
