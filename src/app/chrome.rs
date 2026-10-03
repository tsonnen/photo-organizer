//! The chrome every modal in the app shares: a dimming backdrop over the whole
//! screen, a centred card sized against that screen, and the click-outside-to-
//! close rule.
//!
//! All three overlays are "stop and deal with one thing" dialogs, so they are
//! built the same way and differ only in what goes inside the card. Escape and
//! backdrop-click handling is deliberately *not* here: the inspection modal
//! also owns the arrow and space keys, so each caller decides what dismissal
//! means for it.

use eframe::egui;

/// What one modal frame wants the app to do, gathered before anything acts.
///
/// Clicks are collected during drawing and applied by the caller afterwards,
/// because the usual response to a press — re-classifying, re-scanning — must
/// not happen mid-draw.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct ModalFrame {
    /// The backdrop was clicked outside the card, or Escape was pressed.
    pub close: bool,
}

/// Draws a modal card of `card_size` centred on the screen and runs
/// `add_contents` inside it.
///
/// Returns whether the user dismissed the overlay. The contents are laid out
/// inside a window frame, so `card_size` is the *card*, not the space the
/// contents get: the frame spends part of its rect on margins and shadow.
pub(super) fn show_modal_card<R>(
    ctx: &egui::Context,
    area_id: &str,
    card_size: egui::Vec2,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> (R, ModalFrame) {
    let screen_rect = ctx.screen_rect();
    let card_rect = egui::Rect::from_center_size(screen_rect.center(), card_size);

    let mut frame = ModalFrame::default();

    let response = egui::Area::new(egui::Id::new(area_id))
        // Foreground, so the card is drawn over the panels rather than clipped
        // by whatever was laid out last.
        .order(egui::Order::Foreground)
        .fixed_pos(screen_rect.min)
        .show(ctx, |ui| {
            let (backdrop_rect, backdrop_resp) =
                ui.allocate_exact_size(screen_rect.size(), egui::Sense::click());
            ui.painter()
                .rect_filled(backdrop_rect, 0.0, egui::Color32::from_black_alpha(200));

            if backdrop_resp.clicked() {
                // Only a click *outside* the card dismisses: the card is drawn
                // inside the same clickable area, so a plain `clicked()` would
                // close the modal every time the user pressed a button in it.
                if let Some(click) = backdrop_resp.interact_pointer_pos() {
                    if !card_rect.contains(click) {
                        frame.close = true;
                    }
                }
            }

            // `scope_builder` and `Frame::show` both return an InnerResponse,
            // so the card's value is two `.inner`s down.
            ui.scope_builder(egui::UiBuilder::new().max_rect(card_rect), |ui| {
                // The card is sized on the ui *around* the frame, not inside
                // it: asking for the full card size from within the frame
                // overflows by the frame's margin and shadow.
                ui.set_min_size(card_size);
                ui.set_max_size(card_size);

                egui::Frame::window(&ctx.style())
                    .rounding(8.0)
                    .show(ui, add_contents)
            })
            .inner
            .inner
        })
        .inner;

    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        frame.close = true;
    }

    (response, frame)
}
