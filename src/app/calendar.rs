//! A month-grid calendar for choosing the date range the grid is filtered to.
//!
//! egui 0.30 has no date picker, and the obvious alternative — a crate — is either
//! unmaintained or not in the registry this project builds against. So this is a
//! month grid: a weekday header, the days of one month, and arrows to move between
//! months. Small enough to read in one sitting, and small enough to test.
//!
//! The arithmetic is kept in free functions above the widget so the awkward parts
//! — which weekday a month starts on, how many blanks a leap February needs, which
//! end of a range a given cell rounds — are assertable without an egui context.
//! `month_grid` and `day_fill` are the two that have to be right: an off-by-one in
//! the first shifts a whole month by a column, and every wrong day then reads as a
//! real date the user could pick, while the second is what stops the selected range
//! looking like a row of loose squares instead of one bar.
//!
//! Weeks start on Sunday.

use crate::classification::{days_in_month, PhotoDate};
use eframe::egui::{Rounding, Stroke};

/// Weekday initials, Sunday first, matching [`month_grid`]'s layout.
///
/// Single letters because that is what seven columns affords. The cost is that
/// `S`/`S` and `T`/`T` are ambiguous, so each column carries the full name as a
/// hover instead of on screen.
const WEEKDAYS: [(&str, &str); 7] = [
    ("S", "Sunday"),
    ("M", "Monday"),
    ("T", "Tuesday"),
    ("W", "Wednesday"),
    ("T", "Thursday"),
    ("F", "Friday"),
    ("S", "Saturday"),
];

/// A day cell's edge, in points.
///
/// The *cell* is square — it is the hit target, and a month of squares reads as a
/// grid. Corner rounding belongs to the selected bar drawn inside it, not to the
/// cell: an unselected day has no visible shape to round.
pub(super) const DAY_SIZE: f32 = 38.0;

/// Horizontal gap between day cells, in points. Zero: the dates sit edge to edge,
/// and the only reason to see that is the selected range reading as one bar.
const DAY_SPACING_X: f32 = 0.0;

/// Vertical gap between weeks, in points.
///
/// Separate from [`DAY_SPACING_X`] because the two axes want opposite things. A
/// month grid with no vertical gap is a dense block where a range spilling into the
/// next week looks continuous with itself; a small gap makes each week read as its
/// own bar, which is what makes a range that wraps legible at a glance.
const ROW_SPACING: f32 = 6.0;

/// How far the ends of a selected range round, in points. Half a cell, which makes
/// a lone selected day a circle and a run of them a stadium.
const BAR_RADIUS: f32 = DAY_SIZE / 2.0;

/// Width reserved at each end of the heading for the two arrows, in points.
///
/// Fixed rather than measured, so the title between them is always centred: the
/// two ends have to be the same width, and they are the same two glyph pairs.
const ARROW_SLOT: f32 = 40.0;

/// The earliest year the arrows will page to, so they stop somewhere rather than
/// running off into a year before any camera existed.
const MIN_YEAR: i32 = 1900;

/// The fill behind one day, which says where the selected range begins and ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DayFill {
    /// Outside the range. A bare number — no cell behind it at all, so an
    /// unselected month is just numbers on the panel.
    None,
    /// The only day of the range, or the only one on its row: a full circle.
    Only,
    /// The bar starts at this cell, so its left corners round.
    Starts,
    /// The bar runs through this cell, so no corner rounds.
    Middle,
    /// The bar ends at this cell, so its right corners round.
    Ends,
}

impl DayFill {
    /// The corner radii for a cell with this fill.
    ///
    /// Rounding only the corners the bar actually turns at is what makes a
    /// selected run read as one shape: round all four on every day and the range
    /// looks like a row of separate pills; round none and it is a plain block with
    /// two square ends.
    pub(super) fn rounding(self) -> Rounding {
        let (left, right) = match self {
            Self::None | Self::Middle => (0.0, 0.0),
            Self::Only => (BAR_RADIUS, BAR_RADIUS),
            Self::Starts => (BAR_RADIUS, 0.0),
            Self::Ends => (0.0, BAR_RADIUS),
        };
        Rounding {
            nw: left,
            sw: left,
            ne: right,
            se: right,
        }
    }

    /// Whether anything is painted behind this day's number.
    pub(super) fn is_selected(self) -> bool {
        !matches!(self, Self::None)
    }
}

/// How one day is filled, given the range and where the day sits in its week.
///
/// `leading` is [`first_weekday`] for the month, which is what makes a range that
/// wraps to the next week start a fresh bar rather than running off the left edge
/// of a row it is not in.
pub(super) fn day_fill(
    year: u32,
    month: u32,
    day: u32,
    leading: usize,
    from: PhotoDate,
    to: PhotoDate,
) -> DayFill {
    let date = PhotoDate::new(year, month, Some(day));
    if !in_range(date, from, to) {
        return DayFill::None;
    }

    // The column within its own week, which is what decides whether the day
    // either side of it is a *neighbour* or merely the next entry in the month.
    // Without the wrap, a day in the second week looks as though it had a day to
    // its left, and a range runs off the end of its row instead of stopping.
    let column = (leading + day as usize - 1) % WEEKDAYS.len();
    let last_day = days_in_month(year, month);

    // Only neighbours in the same row of this grid continue the bar. The day
    // before the 1st, or after the last, is on another row — or another month —
    // and the bar has to start and stop at the edge of the row it is drawn on.
    let before =
        column > 0 && day > 1 && in_range(PhotoDate::new(year, month, Some(day - 1)), from, to);
    let after = column + 1 < WEEKDAYS.len()
        && day < last_day
        && in_range(PhotoDate::new(year, month, Some(day + 1)), from, to);

    match (before, after) {
        (false, false) => DayFill::Only,
        (false, true) => DayFill::Starts,
        (true, false) => DayFill::Ends,
        (true, true) => DayFill::Middle,
    }
}

/// Whether `date` falls inside the range, comparing the spans so a bound that
/// names no day covers its whole month.
fn in_range(date: PhotoDate, from: PhotoDate, to: PhotoDate) -> bool {
    date.end() >= from.start() && date.start() <= to.end()
}

/// The fill for one day, given the range as it stands.
///
/// An open range — a first click, so one bound and no other — fills from that
/// bound to the end of its month, so the bar visibly runs off towards the edge the
/// user has not chosen yet. That is also what makes a single click read as "from
/// here" rather than as one already-closed day.
pub(super) fn fill_for(
    from: Option<PhotoDate>,
    to: Option<PhotoDate>,
    year: u32,
    month: u32,
    day: u32,
    leading: usize,
) -> DayFill {
    match (from, to) {
        (Some(start), Some(end)) => day_fill(year, month, day, leading, start, end),
        (Some(start), None) => day_fill(
            year,
            month,
            day,
            leading,
            start,
            PhotoDate::new(start.year, start.month, None),
        ),
        (None, Some(end)) => day_fill(
            year,
            month,
            day,
            leading,
            PhotoDate::new(end.year, end.month, None),
            end,
        ),
        (None, None) => DayFill::None,
    }
}

/// The weekday the 1st of a month falls on, Sunday being 0, which is the blanks
/// before it in [`month_grid`]. Goes through `chrono` rather than a hand-rolled day
/// count: getting this wrong shifts the whole month by a column, and every wrong
/// day then reads as a real date the user could pick.
///
/// `+ 1`, not `+ 6`: chrono counts Monday as 0 and this counts Sunday as 0.
fn first_weekday(year: u32, month: u32) -> usize {
    use chrono::Datelike;
    chrono::NaiveDate::from_ymd_opt(year as i32, month, 1)
        .map(|date| ((date.weekday().num_days_from_monday() + 1) % 7) as usize)
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

/// What a click on `clicked` does to a range that currently reads `from..to`.
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

    /// Draws the month, editing `from..to` in place. `today` is passed in so the
    /// caller decides what "today" means — and so a test can.
    pub(super) fn show(
        &mut self,
        ui: &mut egui::Ui,
        from: &mut Option<PhotoDate>,
        to: &mut Option<PhotoDate>,
        today: PhotoDate,
    ) {
        let (year, month) = self.view;

        // The heading: two arrows, a centred title, two more arrows, laid out in
        // the grid's own width so it lines up with the days below.
        //
        // The title slot is fixed rather than "whatever is left over". A nested
        // `with_layout` ui takes *all* the remaining width, which pushed the right
        // arrows out past the end of the band entirely; and a plain `label`
        // left-aligns instead, which put "October 2026" at x=84 rather than x=141.
        // Reserving the same width at each end is what centres it: the arrows are
        // symmetric, so the middle sits centred on the band, and a long month name
        // cannot widen one side and shove the heading off axis.
        //
        // Constrained to the grid's width because a bare `ui.horizontal` greedily
        // fills the panel, and a filter panel is much wider than seven 38pt days:
        // the arrows spread out to the panel edges and the year buttons fall off
        // the right of it entirely.
        let width = DAY_SIZE * WEEKDAYS.len() as f32;
        ui.allocate_ui(egui::vec2(width, ui.spacing().interact_size.y), |ui| {
            // Three explicit slots rather than a row of widgets. A nested
            // `with_layout` ui claims *all* the remaining width, which left the
            // right arrows nothing and pushed them past the end of the band; and a
            // plain `label` left-aligns, which put "October 2026" at x=84 rather
            // than x=141. Reserving equal slots at each end is what centres the
            // title — and it stays centred for "September 2026", whose wider label
            // would otherwise push the whole heading off axis.
            ui.horizontal(|ui| {
                let mut arrows = |ui: &mut egui::Ui, steps: [(&str, &str, i32); 2]| {
                    for (arrow, tooltip, by) in steps {
                        if ui.button(arrow).on_hover_text(tooltip).clicked() {
                            self.view = shift_month(year, month, by);
                        }
                    }
                };
                ui.allocate_ui(egui::vec2(ARROW_SLOT, ui.spacing().interact_size.y), |ui| {
                    arrows(
                        ui,
                        [("«", "Previous year", -12), ("‹", "Previous month", -1)],
                    );
                });
                ui.allocate_ui_with_layout(
                    egui::vec2(width - ARROW_SLOT * 2.0, ui.spacing().interact_size.y),
                    egui::Layout::top_down(egui::Align::Center),
                    |ui| {
                        ui.label(
                            egui::RichText::new(format!("{} {}", month_name(month), year))
                                .text_style(egui::TextStyle::Heading)
                                .strong(),
                        );
                    },
                );
                ui.allocate_ui(egui::vec2(ARROW_SLOT, ui.spacing().interact_size.y), |ui| {
                    arrows(ui, [("›", "Next month", 1), ("»", "Next year", 12)]);
                });
            });
        });

        // The weekday initials are painted into the same seven columns as the days
        // rather than laid out as a row of labels, so they cannot drift out of
        // alignment with the dates they head. They are text, not widgets: at seven
        // columns there is no room for real ones, and a header of hit targets
        // would be a row of things that look clickable and are not.
        let leading = first_weekday(year, month);
        egui::Grid::new("calendar")
            .num_columns(WEEKDAYS.len())
            .spacing([DAY_SPACING_X, ROW_SPACING])
            // Without this every cell is forced to `spacing.interact_size`, which
            // is 40pt wide by default: a 38pt date would sit in a 40pt column and
            // the 2pt difference would read as a gap that no grid spacing could
            // close, because it was never the spacing.
            .min_col_width(DAY_SIZE)
            .min_row_height(DAY_SIZE)
            .show(ui, |ui| {
                for (initial, full) in WEEKDAYS {
                    let (rect, response) = ui
                        .allocate_exact_size(egui::vec2(DAY_SIZE, DAY_SIZE), egui::Sense::hover());
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        initial,
                        egui::FontId::proportional(
                            ui.text_style_height(&egui::TextStyle::Button) * 0.85,
                        ),
                        ui.visuals().weak_text_color(),
                    );
                    // The single-letter header cannot say which Tuesday this is,
                    // so the full name is a hover away rather than on screen.
                    response.on_hover_ui(|ui| {
                        ui.label(full.to_string());
                    });
                }
                ui.end_row();

                let mut clicked = None;
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
                        let fill = fill_for(*from, *to, year, month, day, leading);
                        let is_today =
                            today.day == Some(day) && today.month == month && today.year == year;

                        if day_button(ui, day, fill, is_today) {
                            clicked = Some(PhotoDate::new(year, month, Some(day)));
                        }
                    }
                    ui.end_row();
                }

                if let Some(clicked) = clicked {
                    let (new_from, new_to) = range_after_click(*from, *to, clicked);
                    *from = new_from;
                    *to = new_to;
                }
            });
    }
}

/// A month's name, for the heading.
///
/// Written out rather than looked up: `chrono`'s locale data is behind a feature
/// flag this project does not enable, and a picker that says "October" in English
/// is what every other picker on the desktop does.
fn month_name(month: u32) -> &'static str {
    const NAMES: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    NAMES
        .get(month.saturating_sub(1) as usize)
        .copied()
        .unwrap_or("?")
}

/// One day: a real button, carrying the fill its place in the range implies.
///
/// Kept a `Button` rather than painted text so the calendar is keyboard-reachable
/// and the layout tests can find a day by its label — both are worth more than the
/// alternative, which was tried and closes the same gap.
///
/// `add_sized` because a default `Button` claims more column width than it draws,
/// which puts a gap beside every date that no grid spacing can close.
fn day_button(ui: &mut egui::Ui, day: u32, fill: DayFill, is_today: bool) -> bool {
    let text = if fill.is_selected() {
        egui::Color32::WHITE
    } else if is_today {
        egui::Color32::from_rgb(120, 180, 255)
    } else {
        ui.visuals().text_color()
    };
    let label = egui::RichText::new(day.to_string()).color(text);

    // The end points a shade stronger than the days between them, so a range reads
    // as "from here to here" rather than as one undifferentiated run.
    let background = match fill {
        DayFill::None => egui::Color32::TRANSPARENT,
        DayFill::Only | DayFill::Starts | DayFill::Ends => egui::Color32::from_rgb(0, 140, 215),
        DayFill::Middle => egui::Color32::from_rgb(38, 62, 82),
    };

    let mut button = egui::Button::new(label)
        .rounding(fill.rounding())
        .fill(background);

    // Today, when it is not part of the range, is an outline rather than a fill:
    // a filled circle would read as selected, and the one thing the ring has to say
    // is "not selected, but this is now".
    if is_today && !fill.is_selected() {
        button = button.stroke(Stroke::new(1.5_f32, text));
    }
    if fill.is_selected() {
        button = button.selected(true);
    }

    ui.add_sized([DAY_SIZE, DAY_SIZE], button).clicked()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The days a month view offers, blanks dropped.
    fn days_shown(year: u32, month: u32) -> Vec<u32> {
        month_grid(year, month).into_iter().flatten().collect()
    }

    fn range(from: (u32, u32, u32), to: (u32, u32, u32)) -> (PhotoDate, PhotoDate) {
        (
            PhotoDate::new(from.0, from.1, Some(from.2)),
            PhotoDate::new(to.0, to.1, Some(to.2)),
        )
    }

    #[test]
    fn a_month_offers_every_day_exactly_once() {
        for (year, month, expected) in [
            (2021, 1, 31),
            (2021, 2, 28),
            (2020, 2, 29), // leap
            (1900, 2, 28), // century, not a leap year
            (2000, 2, 29), // 400-divisible, so a leap year
            (2021, 4, 30),
        ] {
            let days = days_shown(year, month);
            assert_eq!(
                days,
                (1..=expected).collect::<Vec<u32>>(),
                "{year}-{month:02} should offer 1..={expected}"
            );
        }
    }

    #[test]
    fn a_month_is_a_whole_number_of_weeks() {
        // The whole point: without the trailing blanks a February would lay out
        // as four rows and a May as five, and the picker would change height as
        // the user paged.
        for month in 1..=12 {
            let cells = month_grid(2021, month).len();
            assert_eq!(cells % 7, 0, "month {month} is not a whole number of weeks");
            assert!(
                (28..=42).contains(&cells),
                "month {month} laid out as {cells} cells"
            );
        }
    }

    #[test]
    fn weeks_start_on_sunday() {
        // 1 November 2026 was a Sunday, so it gets no leading blanks.
        assert_eq!(first_weekday(2026, 11), 0);
        // 1 June 2026 is a Monday, so one blank precedes it.
        assert_eq!(first_weekday(2026, 6), 1);
        // 1 October 2026 — the month in the reference — is a Thursday.
        assert_eq!(first_weekday(2026, 10), 4);

        assert_eq!(month_grid(2026, 11)[0], Some(1));
        assert_eq!(month_grid(2026, 11)[6], Some(7));
        assert_eq!(month_grid(2026, 6)[0], None);
        assert_eq!(month_grid(2026, 6)[1], Some(1));
    }

    #[test]
    fn stepping_months_carries_across_the_year() {
        assert_eq!(shift_month(2021, 12, 1), (2022, 1));
        assert_eq!(shift_month(2021, 1, -1), (2020, 12));
        assert_eq!(shift_month(2021, 6, 12), (2022, 6));
        assert_eq!(shift_month(2021, 6, -12), (2020, 6));
        // Far enough back and it stops at the floor rather than wrapping to a year
        // no camera was made in.
        assert_eq!(shift_month(1900, 1, -12).0, 1900);
    }

    #[test]
    fn a_click_starts_a_range_and_the_next_closes_it() {
        let first = PhotoDate::new(2021, 3, Some(4));
        let second = PhotoDate::new(2021, 3, Some(9));

        let (from, to) = range_after_click(None, None, first);
        assert_eq!(from, Some(first));
        assert_eq!(
            to, None,
            "the first click opens the range, it does not close it"
        );

        let (from, to) = range_after_click(from, to, second);
        assert_eq!((from, to), (Some(first), Some(second)));
    }

    #[test]
    fn closing_a_range_backwards_swaps_it_rather_than_inverting_it() {
        // Clicking the dates in the order a person thinks about them — end, then
        // start — has to leave a usable range. Storing it inverted reads as empty
        // and silently filters everything away.
        let early = PhotoDate::new(2021, 3, Some(4));
        let late = PhotoDate::new(2021, 3, Some(9));

        let (from, to) = range_after_click(Some(late), None, early);
        assert_eq!((from, to), (Some(early), Some(late)));
        assert!(from.unwrap().start() <= to.unwrap().end());
    }

    #[test]
    fn a_third_click_starts_the_range_over() {
        let first = PhotoDate::new(2021, 3, Some(4));
        let last = PhotoDate::new(2021, 3, Some(9));
        let elsewhere = PhotoDate::new(2021, 4, Some(2));

        let (from, to) = range_after_click(Some(first), Some(last), elsewhere);
        assert_eq!(from, Some(elsewhere));
        assert_eq!(to, None);
    }

    #[test]
    fn the_picker_opens_on_a_bound_before_a_default() {
        let bound = PhotoDate::new(2019, 7, Some(2));
        let fallback = (2021, 3);

        // A bound the user set beats the folder's own dates: a range whose start
        // is off screen is worse than no filter at all.
        assert_eq!(opening_month(Some(bound), None, fallback), (2019, 7));
        assert_eq!(opening_month(None, Some(bound), fallback), (2019, 7));
        assert_eq!(opening_month(None, None, fallback), fallback);
    }

    #[test]
    fn only_the_ends_of_a_range_round() {
        // The difference between "a range" and "a row of pills": a run of selected
        // days is one shape, so only the two cells where the bar begins and ends
        // have rounded corners.
        let leading = first_weekday(2026, 10);
        let (from, to) = range((2026, 10, 4), (2026, 10, 10));

        // Spelled out by day rather than as a literal list, so the expectation
        // cannot drift out of step with the range it describes.
        let fills: Vec<DayFill> = (1..=14)
            .map(|day| day_fill(2026, 10, day, leading, from, to))
            .collect();

        let expected: Vec<DayFill> = (1..=14u32)
            .map(|day| match day {
                4 => DayFill::Starts,
                5..=9 => DayFill::Middle,
                10 => DayFill::Ends,
                _ => DayFill::None,
            })
            .collect();

        assert_eq!(fills, expected, "only 4 and 10 are ends; 5..9 run through");
    }

    #[test]
    fn an_open_range_runs_off_the_edge_of_its_month() {
        // The state after a single click: one bound, no other. The bar has to look
        // unfinished, which is what tells the user a second click is expected — and
        // it is why the fill cannot simply treat a missing bound as "nothing
        // selected" and show a bare number on a day the user just clicked.
        let leading = first_weekday(2026, 11);
        let clicked = PhotoDate::new(2026, 11, Some(20));

        let fills: Vec<DayFill> = (1..=30)
            .map(|day| fill_for(Some(clicked), None, 2026, 11, day, leading))
            .collect();

        assert!(
            fills[..19].iter().all(|f| !f.is_selected()),
            "before the click"
        );
        assert_eq!(fills[19], DayFill::Starts, "the clicked day opens the bar");
        assert!(
            fills[20..].iter().all(|f| f.is_selected()),
            "the bar runs to the end of the month, got {:?}",
            &fills[20..]
        );
        assert_eq!(
            fills[29],
            DayFill::Ends,
            "and stops at the month's edge rather than running past it"
        );
    }

    #[test]
    fn a_lone_selected_day_is_a_circle() {
        let leading = first_weekday(2026, 10);
        let (from, to) = range((2026, 10, 7), (2026, 10, 7));

        assert_eq!(day_fill(2026, 10, 7, leading, from, to), DayFill::Only);
        assert_eq!(
            day_fill(2026, 10, 6, leading, from, to),
            DayFill::None,
            "the day before a lone one is not selected"
        );
    }

    #[test]
    fn a_range_wrapping_to_the_next_week_starts_a_fresh_bar() {
        // The case that decides whether contiguity is about the calendar or about
        // the grid. 8 and 15 November 2026 are consecutive days but sit in
        // different rows, so the bar has to stop at the row edge and start again
        // rather than running off the left of the second week.
        let leading = first_weekday(2026, 11);
        assert_eq!(
            leading, 0,
            "1 November 2026 is a Sunday, so no leading blanks"
        );

        let (from, to) = range((2026, 11, 8), (2026, 11, 15));

        // Day 8 opens the range in the second week and runs to that week's edge on
        // the 14th, then the 15th opens a fresh bar in the third.
        assert_eq!(day_fill(2026, 11, 7, leading, from, to), DayFill::None);
        assert_eq!(day_fill(2026, 11, 8, leading, from, to), DayFill::Starts);
        assert_eq!(day_fill(2026, 11, 13, leading, from, to), DayFill::Middle);
        assert_eq!(
            day_fill(2026, 11, 14, leading, from, to),
            DayFill::Ends,
            "the bar must stop at the row edge even though the range continues"
        );
        assert_eq!(day_fill(2026, 11, 15, leading, from, to), DayFill::Only);
    }

    #[test]
    fn a_range_covering_a_whole_row_is_one_bar() {
        let leading = first_weekday(2026, 11);
        let (from, to) = range((2026, 11, 8), (2026, 11, 14));

        let fills: Vec<DayFill> = (8..=14)
            .map(|day| day_fill(2026, 11, day, leading, from, to))
            .collect();
        assert_eq!(fills[0], DayFill::Starts);
        assert!(fills[1..6].iter().all(|f| *f == DayFill::Middle));
        assert_eq!(fills[6], DayFill::Ends);
    }

    #[test]
    fn a_month_wide_bound_still_selects_every_day_in_it() {
        // An open range drawn from the calendar's first click: the bound names no
        // day, so every day of it is selected and the bar runs to the edge.
        let leading = first_weekday(2026, 11);
        let from = PhotoDate::new(2026, 11, None);
        let to = PhotoDate::new(2026, 11, Some(31));

        let fills: Vec<DayFill> = (1..=10)
            .map(|day| day_fill(2026, 11, day, leading, from, to))
            .collect();
        assert!(
            fills.iter().all(|f| f.is_selected()),
            "a whole-month range selects the whole month, got {fills:?}"
        );
        assert_eq!(fills[0], DayFill::Starts);
    }

    #[test]
    fn only_the_corners_the_bar_turns_at_are_rounded() {
        assert_eq!(DayFill::Middle.rounding(), Rounding::ZERO);
        assert_eq!(DayFill::None.rounding(), Rounding::ZERO);

        let only = DayFill::Only.rounding();
        assert_eq!([only.nw, only.ne, only.sw, only.se], [BAR_RADIUS; 4]);

        let starts = DayFill::Starts.rounding();
        assert_eq!([starts.nw, starts.sw], [BAR_RADIUS; 2], "left corners");
        assert_eq!(
            [starts.ne, starts.se],
            [0.0; 2],
            "right corners stay square"
        );

        let ends = DayFill::Ends.rounding();
        assert_eq!([ends.nw, ends.sw], [0.0; 2]);
        assert_eq!([ends.ne, ends.se], [BAR_RADIUS; 2]);
    }
}
