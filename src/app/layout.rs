//! Layout arithmetic for the photo grid, the category controls inside it, and
//! the profile management modal.
//!
//! Kept apart from the renderers so the geometry can be asserted directly,
//! without a headless egui context, and so the numbers those renderers depend
//! on live in one place.

use eframe::egui;

/// Horizontal and vertical gap between grid cells, in points.
pub(crate) const SPACING: f32 = 14.0;

/// Width of the train button beside the category combo in a grid cell.
pub(crate) const TRAIN_BUTTON_SIZE: f32 = 20.0;

/// Narrowest control we will lay out inside a grid cell, in points.
pub(crate) const MIN_CONTROL_WIDTH: f32 = 60.0;

/// Width of the category combo in the inspection modal, in points.
pub(crate) const MODAL_COMBO_WIDTH: f32 = 120.0;

/// Points the profile modal's header, hint line and separators take, leaving the
/// rest of the card to the scrollable list.
const PROFILE_MODAL_CHROME: f32 = 92.0;

/// Floor for the profile list's height, so a degenerate card still shows a
/// couple of rows instead of collapsing the scroll area to nothing. Never bites
/// at production sizes: the card is at least 300pt tall, leaving 208pt.
const MIN_PROFILE_LIST_HEIGHT: f32 = 120.0;

/// Grid column count and item width for a given available width.
///
/// The column count is derived from `MIN_ITEM_WIDTH` first, then the
/// per-column width is clamped to `MAX_ITEM_WIDTH`; clamping only ever shrinks
/// the content, so the grid always fits the width it was given.
pub(crate) fn grid_column_layout(available_width: f32) -> (usize, f32) {
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

/// Width of the category combo in a grid cell: the cell minus the train
/// button and the gaps either side of it.
pub(crate) fn grid_cell_combo_width(item_width: f32) -> f32 {
    (item_width - TRAIN_BUTTON_SIZE - (SPACING * 2.0)).max(MIN_CONTROL_WIDTH)
}

/// Width of the custom category text input in a grid cell: the full cell
/// width, since it sits on its own line rather than sharing one.
pub(crate) fn grid_cell_input_width(item_width: f32) -> f32 {
    (item_width - (SPACING * 2.0)).max(MIN_CONTROL_WIDTH)
}

/// Width the inline custom category input may claim on the modal's control
/// row: whatever is left once the train and close buttons still have room.
///
/// Clamping to what is actually free keeps a narrow modal from overflowing
/// the row, while a wide one gives the input all the slack it wants.
pub(crate) fn modal_input_width(ui: &egui::Ui) -> f32 {
    const TRAILING_CONTROLS: f32 = 170.0;

    (ui.available_width() - TRAILING_CONTROLS).clamp(MIN_CONTROL_WIDTH, 200.0)
}

/// Size of the inspection modal's card, in points.
///
/// Takes most of the screen, because the thing it shows is a photograph and
/// the controls underneath it want a single row of their own. Clamped so it
/// never becomes larger than the screen it is drawn on.
pub(crate) fn inspection_modal_size(screen: egui::Vec2) -> egui::Vec2 {
    egui::vec2(
        (screen.x * 0.85).clamp(500.0, 1100.0),
        (screen.y * 0.85).clamp(400.0, 800.0),
    )
}

/// Size of the settings modal's card, in points.
///
/// Wider than it is tall by design: the paths it shows — a model checkpoint in
/// particular — are long, and the column is left wide enough for one to be
/// read rather than scrolled sideways. The height covers the three rows plus
/// the warning line the model row grows when a chosen path cannot be read.
pub(crate) fn settings_modal_size(screen: egui::Vec2) -> egui::Vec2 {
    egui::vec2(
        (screen.x * 0.45).clamp(420.0, 640.0),
        (screen.y * 0.42).clamp(340.0, 480.0),
    )
}

/// Size of the profile management modal's card, in points.
///
/// Derived from the screen the way the inspection modal's card is, and clamped
/// so a long list scrolls inside a card rather than stretching one. The height
/// is set to show roughly a dozen categories: taller than that and a typical
/// handful of profiles leaves the card mostly blank.
pub(crate) fn profiles_modal_size(screen: egui::Vec2) -> egui::Vec2 {
    egui::vec2(
        (screen.x * 0.40).clamp(360.0, 560.0),
        (screen.y * 0.45).clamp(300.0, 560.0),
    )
}

/// Height the scrollable profile list gets inside a card of `card` size.
///
/// The list is the thing that scrolls, so it is bounded here rather than left
/// to `available_height`, which the card's own chrome has already eaten into.
pub(crate) fn profiles_list_height(card: egui::Vec2) -> f32 {
    (card.y - PROFILE_MODAL_CHROME).max(MIN_PROFILE_LIST_HEIGHT)
}

#[cfg(test)]
mod tests {
    use super::*;

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
            let (columns, item_width) = grid_column_layout(available);
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
        let (columns, item_width) = grid_column_layout(100.0);
        assert_eq!(columns, 1);
        assert_eq!(item_width, 240.0);
    }

    #[test]
    fn column_count_grows_with_available_width() {
        // Monotonicity: a wider window should never produce fewer columns.
        let mut previous = 0;
        for available in [300.0, 500.0, 800.0, 1240.0, 1600.0, 2000.0, 2560.0] {
            let (columns, _) = grid_column_layout(available);
            assert!(
                columns >= previous,
                "column count dropped from {previous} to {columns} at width {available}"
            );
            previous = columns;
        }
    }

    #[test]
    fn grid_cell_controls_claim_distinct_widths() {
        // The combo shares its row with the train button, so it must be the
        // narrower of the two asks; the input sits alone below and gets the
        // whole cell. Both stay above the usable minimum.
        for item_width in [240.0, 300.0, 350.0] {
            let combo = grid_cell_combo_width(item_width);
            let input = grid_cell_input_width(item_width);
            assert!(
                combo < input,
                "combo {combo} should be narrower than input {input}"
            );
            assert!(combo >= MIN_CONTROL_WIDTH, "combo {combo} below minimum");
            assert!(input >= MIN_CONTROL_WIDTH, "input {input} below minimum");
        }
    }

    #[test]
    fn profile_card_fits_the_window_it_is_shown_on() {
        // Sized off the screen and clamped, so it stays a card rather than
        // becoming a sheet. The window enforces a 1240x900 minimum, which is
        // the smallest screen it can actually be drawn on.
        for screen in [
            egui::vec2(1240.0, 900.0),
            egui::vec2(1366.0, 768.0),
            egui::vec2(1600.0, 1200.0),
            egui::vec2(2560.0, 1440.0),
            egui::vec2(3840.0, 2160.0),
        ] {
            let card = profiles_modal_size(screen);
            assert!(
                card.x <= screen.x && card.y <= screen.y,
                "{card:?} card overflows a {screen:?} screen"
            );
            assert!(
                (360.0..=560.0).contains(&card.x),
                "card width {} outside 360..=560 at {screen:?}",
                card.x
            );
            assert!(
                (300.0..=560.0).contains(&card.y),
                "card height {} outside 300..=560 at {screen:?}",
                card.y
            );
        }
    }

    #[test]
    fn profile_list_keeps_a_usable_height_on_every_card() {
        // The list is bounded from the card, so the card's chrome can never
        // squeeze it out of existence, and it never gets more than the card.
        for screen in [egui::vec2(1240.0, 900.0), egui::vec2(3840.0, 2160.0)] {
            let card = profiles_modal_size(screen);
            let list = profiles_list_height(card);
            assert!(list >= MIN_PROFILE_LIST_HEIGHT, "list {list} too short");
            assert!(
                list + PROFILE_MODAL_CHROME <= card.y + 0.5,
                "list {list} plus chrome overflows a {card:?} card"
            );
        }
    }

    #[test]
    fn profile_list_floors_rather_than_collapsing() {
        // Documents the degenerate case rather than pretending it fits: a card
        // with no room left still gets a couple of rows, which overflows that
        // card. Production cards are at least 300pt tall, so it never happens.
        assert_eq!(profiles_list_height(egui::vec2(360.0, 0.0)), 120.0);
    }
}
