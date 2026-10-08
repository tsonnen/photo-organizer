//! A month-grid calendar for choosing the date range the grid is filtered to.
//!
//! egui 0.30 has no date picker, and the obvious alternative — a crate — is either
//! unmaintained or not in the registry this project builds against. So this is a
//! month grid: a weekday header, the days of one month, and arrows to move between
//! months. Small enough to read in one sitting, and small enough to test.
//!
//! The arithmetic is kept in free functions above the widget so the awkward parts
//! — which weekday a month starts on, how many blanks a leap February needs — are
//! assertable without an egui context. `month_grid` is the one that has to be
//! right: an off-by-one there is a calendar that silently shifts every date.
//!
//! Weeks start on Monday, matching `chrono::Weekday`.

use crate::classification::{days_in_month, PhotoDate};
use eframe::egui;

/// Weekday initials, Monday first, matching [`month_grid`]'s layout.
const WEEKDAYS: [&str; 7] = ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"];

/// A day cell's edge, in points. Square so a month reads as a grid.
///
/// `pub(super)` because the layout test asserts on the calendar's rendered width,
/// which is seven of these and nothing else.
pub(super) const DAY_SIZE: f32 = 26.0;

/// The gap between day cells, in points. Shared by the header and the days so the
/// two grids cannot drift out of alignment.
const DAY_SPACING: f32 = 0.0;

/// How many days earlier than the 1st a month may start, so the arrows have
/// somewhere to go before the first photo was taken.
const MIN_YEAR: i32 = 1900;

/// The 1-based weekday the 1st of a month falls on, Monday being 0.
///
/// Blank cells before it in [`month_grid`]. Goes through `chrono` rather than a
/// hand-rolled day count: getting this wrong shifts the whole month by a column,
/// and every wrong day then reads as a real date the user could pick.
fn first_weekday(year: u32, month: u32) -> usize {
    use chrono::Datelike;
    chrono::NaiveDate::from_ymd_opt(year as i32, month, 1)
        .map(|date| date.weekday().num_days_from_monday() as usize)
        .unwrap_or(0)
}

/// The cells a month view draws: leading blanks, the days themselves, then blanks
/// out to a whole final week.
///
/// Every month is a whole number of weeks, so the trailing blanks are not
/// decoration — without them a 28-day February would lay out as four rows and a
/// 31-day one as five, and the grid would change height as the user paged.
fn month_grid(year: u32, month: u32) -> Vec<Option<u32>> {
    let leading = first_weekday(year, month);
    let total = days_in_month(year, month);
    let cells = leading + total as usize;
    let padded = cells.div_ceil(WEEKDAYS.len()) * WEEKDAYS.len();

    (0..padded)
        .map(|cell| match cell {
            c if c < leading => None,
            c if c < cells => Some((c - leading + 1) as u32),
            _ => None,
        })
        .collect()
}

/// Steps a `(year, month)` pair by `by` months, carrying across year boundaries.
fn shift_month(year: u32, month: u32, by: i32) -> (u32, u32) {
    // Counted in signed months from year 0 so a step back from January does not
    // need a special case for the borrow.
    let total = year as i32 * 12 + (month as i32 - 1) + by;
    let year = total.div_euclid(12).max(MIN_YEAR);
    let month = total.rem_euclid(12) + 1;
    (year as u32, month as u32)
}

/// The month a picker should open on: an existing bound first, then whatever the
/// caller suggests.
///
/// A filter set to a range the user cannot see the start of is worse than no
/// filter, so a bound always wins over a default.
pub(super) fn opening_month(
    from: Option<PhotoDate>,
    to: Option<PhotoDate>,
    fallback: (u32, u32),
) -> (u32, u32) {
    from.or(to)
        .map(|date| (date.year, date.month))
        .unwrap_or(fallback)
}

/// What a click on `clicked` does to a range that currently reads
/// `from..to`.
///
/// First click starts a range, second closes it, and a third starts over. Closing
/// on a click *before* the start swaps the two rather than storing an inverted
/// range: a range whose end precedes its start reads as empty, so clicking the
/// dates backwards — the natural way to drag out a selection in one's head — would
/// quietly filter everything away.
fn range_after_click(
    from: Option<PhotoDate>,
    to: Option<PhotoDate>,
    clicked: PhotoDate,
) -> (Option<PhotoDate>, Option<PhotoDate>) {
    match (from, to) {
        (None, _) | (Some(_), Some(_)) => (Some(clicked), None),
        (Some(start), None) if clicked.sort_key() < start.sort_key() => {
            (Some(clicked), Some(start))
        }
        (Some(start), None) => (Some(start), Some(clicked)),
    }
}

/// The months on show, and whether the user is choosing a range or a single day.
///
/// The cursor is per-visit state rather than app state: reopening the picker
/// should land on the bound being edited, not wherever the user last paged to.
pub(super) struct Calendar {
    view: (u32, u32),
}

impl Calendar {
    /// Opens on `month`.
    pub(super) fn new(month: (u32, u32)) -> Self {
        Self { view: month }
    }

    /// The month on show, so a caller can keep it across frames.
    pub(super) fn month(&self) -> (u32, u32) {
        self.view
    }

    /// Draws the month, editing `from..to` in place.
    pub(super) fn show(
        &mut self,
        ui: &mut egui::Ui,
        from: &mut Option<PhotoDate>,
        to: &mut Option<PhotoDate>,
    ) {
        let (year, month) = self.view;

        ui.horizontal(|ui| {
            // Month and year get their own controls rather than the month label
            // doubling as a button: a label that silently jumps a year when
            // clicked is not a thing anyone would try. `«`/`»` for the year is the
            // convention every other date picker uses.
            if ui.button("◀").on_hover_text("Previous month").clicked() {
                self.view = shift_month(year, month, -1);
            }
            if ui.button("▶").on_hover_text("Next month").clicked() {
                self.view = shift_month(year, month, 1);
            }
            if ui.button("«").on_hover_text("Previous year").clicked() {
                self.view = shift_month(year, month, -12);
            }
            if ui.button("»").on_hover_text("Next year").clicked() {
                self.view = shift_month(year, month, 12);
            }
            ui.label(egui::RichText::new(format!("{month:02}/{year}")).strong());
        });

        // One grid for the weekday initials *and* the days, rather than a grid each.
        //
        // Two grids means two independent sets of columns, and nothing makes them agree:
        // the pitch turned out to be `DAY_SIZE + spacing + a further 14pt` that neither
        // grid controls and that moves with the surrounding style. Aligning them by
        // computing the same numbers twice would have been a coincidence waiting to break.
        //
        // As rows of one grid the columns line up by construction, which is the only
        // version of this that cannot drift.
        //
        // The header is painted rather than laid out for a second reason: it is the first
        // row, so a real label there would size the columns to two letters rather than to
        // the day buttons.
        let mut clicked = None;
        egui::Grid::new("calendar")
            .num_columns(WEEKDAYS.len())
            .spacing([DAY_SPACING, DAY_SPACING])
            .min_col_width(DAY_SIZE)
            // Without this every cell is forced to `spacing.interact_size`, which
            // is 40pt wide by default: a 26pt date sat in a 40pt column and the
            // 14pt difference was a gap that no grid spacing could close, because
            // it was never the grid's spacing. This is the whole fix for
            // "leave a space between the dates".
            .min_row_height(DAY_SIZE)
            .show(ui, |ui| {
                for name in WEEKDAYS {
                    let (rect, _) = ui
                        .allocate_exact_size(egui::vec2(DAY_SIZE, DAY_SIZE), egui::Sense::hover());
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        name,
                        egui::FontId::proportional(
                            ui.text_style_height(&egui::TextStyle::Button) * 0.8,
                        ),
                        ui.visuals().weak_text_color(),
                    );
                }
                ui.end_row();

                for week in month_grid(year, month).chunks(WEEKDAYS.len()) {
                    for cell in week {
                        let Some(day) = *cell else {
                            // A spacer rather than an empty cell, so the days stay
                            // square and the columns line up with the header.
                            ui.allocate_exact_size(
                                egui::vec2(DAY_SIZE, DAY_SIZE),
                                egui::Sense::hover(),
                            );
                            continue;
                        };
                        if day_button(ui, year, month, day, *from, *to) {
                            clicked = Some(PhotoDate::new(year, month, Some(day)));
                        }
                    }
                    ui.end_row();
                }
            });

        if let Some(clicked) = clicked {
            let (new_from, new_to) = range_after_click(*from, *to, clicked);
            *from = new_from;
            *to = new_to;
        }
    }
}

/// One day cell: a button that knows whether it is the range's start, its end, or
/// inside it.
///
/// `add_sized` rather than `Button::min_size` because the two differ in exactly
/// the way that matters here: a default `Button` claims more column width than it
/// draws — about 40pt for a 26pt cell — so every date sat with a 14pt gap to its
/// right that no amount of grid spacing could close. `add_sized` forces the size,
/// and the gap becomes the grid spacing, which is none.
///
/// It stays a real `Button` rather than painted text: a calendar of 31 unlabelled
/// rectangles is neither keyboard-reachable nor assertable, and both matter.
fn day_button(
    ui: &mut egui::Ui,
    year: u32,
    month: u32,
    day: u32,
    from: Option<PhotoDate>,
    to: Option<PhotoDate>,
) -> bool {
    let date = PhotoDate::new(year, month, Some(day));
    let is_start = from.is_some_and(|bound| bound.start() == date.start());
    let is_end = to.is_some_and(|bound| bound.end() == date.end());
    let in_range = match (from, to) {
        (Some(start), Some(end)) => date.start() >= start.start() && date.end() <= end.end(),
        _ => false,
    };
    let selected = is_start || is_end || in_range;

    let (fill, text) = if is_start || is_end {
        (
            Some(egui::Color32::from_rgb(0, 150, 230)),
            egui::Color32::WHITE,
        )
    } else if in_range {
        (
            Some(egui::Color32::from_rgb(0, 90, 150)),
            egui::Color32::WHITE,
        )
    } else {
        (None, egui::Color32::GRAY)
    };

    // `RichText` for the label rather than a text colour on the button, which
    // egui 0.30's `Button` has no builder for: a filled button would keep the
    // default label colour and render dark-on-dark.
    let label = egui::RichText::new(day.to_string()).color(text);
    // Square, like the spacing being zero: with no gap between dates, rounded
    // corners leave four small notches at every cell junction, which reads as a
    // grid of separate chips rather than as one continuous field of dates.
    let mut button = egui::Button::new(label).rounding(0.0);
    if let Some(fill) = fill {
        button = button.fill(fill);
    }
    if selected {
        button = button.selected(true);
    }

    ui.add_sized([DAY_SIZE, DAY_SIZE], button).clicked()
}
