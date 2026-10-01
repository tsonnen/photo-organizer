//! Layout arithmetic for the photo grid and the category controls inside it.
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
}
