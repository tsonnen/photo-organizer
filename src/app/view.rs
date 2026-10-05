//! Which staged photos the grid is currently showing, and in what order.
//!
//! This is the one place that answers "is this photo in view". The grid draws
//! it, the footer counts it, the toolbar's select-all ticks it, the transfer
//! moves it and the inspection modal pages through it. Four copies of that
//! predicate would be four chances for the count in the footer to disagree with
//! the rows above it, and a transfer that quietly widens past what is on screen
//! is the kind of bug nobody reports until a photo is in the wrong folder.
//!
//! The logic runs over [`Row`] rather than over [`StagedItem`] itself. Nothing
//! here looks at a thumbnail, an embedding or a selection flag, so borrowing just
//! the five fields that decide visibility keeps the ordering rules assertable
//! without a live egui context — and keeps this module from growing a dependency
//! on the grid's renderer.
//!
//! Kept apart from the renderers so those rules can be asserted directly. Two
//! of them are not obvious:
//!
//! - **Confidence is ranked, not read.** A rule-assigned category carries a
//!   high number that measures the rule's certainty, not the model's, and
//!   ranking it beside a CLIP similarity files confidently mis-filed screenshots
//!   above genuine matches. See [`rank_confidence`].
//! - **"Unsorted" is not a category.** Ordered as `ZZZZ` so everything still
//!   unclassified gathers at one end instead of filing between "Travel" and
//!   "Vacation" as though it were a category the user chose. See
//!   [`category_sort_key`].

use super::models::{Filters, SortBy, SortDirection, StagedItem};
use super::PhotoOrganizerApp;
use crate::category_name::CategoryName;
use crate::classification::{ClassificationSource, PhotoDate};

/// Sorts after every real category name, in an ascending sort.
///
/// Lower-cased like the keys it has to outrank, which is the whole trick: `ZZZZ`
/// as written sorts *before* every lower-cased name, since `Z` is `0x5A` and
/// `a` is `0x61`. A category actually named "zzzz" would tie with it, which
/// costs nothing — the two rows would keep their existing order either way.
const UNCLASSIFIED_SORT_KEY: &str = "zzzz";

/// The fields of a staged photo that decide whether it is in view and where it
/// goes. Borrows rather than copies, so building one per photo per frame
/// allocates nothing — which matters, because the grid asks this question
/// several times a frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Row<'a> {
    date: PhotoDate,
    category: &'a str,
    confidence: f32,
    source: ClassificationSource,
}

impl<'a> From<&'a StagedItem> for Row<'a> {
    fn from(item: &'a StagedItem) -> Self {
        Self {
            date: item.date,
            category: &item.category,
            confidence: item.confidence,
            source: item.source,
        }
    }
}

/// The confidence a row is *ordered and filtered by*, which is not always the
/// number its cell displays.
///
/// A rule-assigned category — a screenshot caught by its filename, a camera
/// photo by its EXIF — reports a high confidence, but that number measures how
/// sure the *rule* is, not how sure the model is. Ranked as-is it files a
/// confidently mis-filed screenshot above a genuine match, which is backwards:
/// the rules only ran at all because the best centroid fell short of the user's
/// threshold.
///
/// So a rule's number is scaled into the band *below* the threshold rather than
/// pinned to its edge, which keeps two things true at once: every rule-assigned
/// row sorts under every accepted match, and two of them still order by how sure
/// their own rule was. Pinning them all to one value would satisfy the first and
/// throw the second away — every screenshot would tie with every receipt.
///
/// Unsorted rows keep their own number. It is a real centroid similarity, just a
/// low one, and a low one is exactly what should sink them.
///
/// Clamped at the bottom because a confidence is a cosine similarity, already
/// clamped on the way out of `classify`, and the "no floor" default of
/// `Filters::confidence_from` is zero: at a zero threshold the scaling would
/// otherwise put every rule-assigned row just below zero and drop it out of a
/// filter that was meant to admit everything.
pub(super) fn rank_confidence(row: &Row<'_>, threshold: f32) -> f32 {
    let ranked = match row.source {
        ClassificationSource::Heuristic => {
            // `min(next_down)` for the rule that reports a perfect 1.0: scaled
            // it would land exactly *on* the threshold and tie with a real match
            // that only just cleared it.
            (row.confidence * threshold).min(threshold.next_down())
        }
        _ => row.confidence,
    };
    ranked.clamp(0.0, 1.0)
}

/// The key a category name is ordered by.
///
/// Lower-cased, so `apples` and `Apples` are one bucket rather than two, and
/// [`UNCLASSIFIED_SORT_KEY`] for the two ways a photo can end up with no category
/// at all: the `Unsorted` fallback, and a custom name the user cleared back to
/// empty. Both would otherwise read as ordinary names — one filing under the
/// U's, the other at the very front, ahead of everything — when what they both
/// mean is "nobody has decided yet".
pub(super) fn category_sort_key(category: &str) -> String {
    let trimmed = category.trim();
    if is_unclassified(trimmed) {
        UNCLASSIFIED_SORT_KEY.to_string()
    } else {
        trimmed.to_lowercase()
    }
}

/// Whether `category` names the absence of a category: the unsorted fallback, or
/// a custom name the user cleared back to empty.
///
/// Asked of `CategoryName::unsorted()` rather than a literal copied out of it.
/// The sort key has to recognise exactly the name a `Decision` carries, and a
/// second copy of the string is free to drift from it without anything noticing.
fn is_unclassified(category: &str) -> bool {
    category.is_empty() || category.eq_ignore_ascii_case(CategoryName::unsorted().as_str())
}

/// Whether `row` survives every filter the user has set.
///
/// The three axes are independent and combine with AND, which is what makes
/// filters worth having: a photo has to be inside the date range *and* inside
/// the confidence range *and* carry the chosen category. Narrowing by one axis
/// would otherwise silently override the other two, leaving three controls that
/// all set the same single filter.
pub(super) fn admits(filters: &Filters, row: &Row<'_>, threshold: f32) -> bool {
    admits_date(filters, row)
        && admits_confidence(filters, row, threshold)
        && admits_category(filters, row)
}

/// Inclusive on both ends, so a range set to one exact day keeps that day.
///
/// Dates carry an *optional* day, so each covers a span rather than a point: a
/// date with a day is that day, a date without one is its whole month. A photo is
/// admitted when its span overlaps the range. That reads naturally at every
/// precision — "March 2021" keeps a photo taken on the 14th, and a month-wide
/// range keeps a photo whose day was never recorded, which dropping it would not.
fn admits_date(filters: &Filters, row: &Row<'_>) -> bool {
    let (photo_first, photo_last) = row.date.span();
    filters
        .date_from
        .is_none_or(|from| photo_last >= from.start())
        && filters.date_to.is_none_or(|to| photo_first <= to.end())
}

/// Compares against [`rank_confidence`] rather than the displayed number, so the
/// confidence filter and the confidence sort agree about which photos are
/// confident. Filtering on one number and sorting on another would let the grid
/// show a set its own ordering does not reflect.
fn admits_confidence(filters: &Filters, row: &Row<'_>, threshold: f32) -> bool {
    let confidence = rank_confidence(row, threshold);
    confidence >= filters.confidence_from && confidence <= filters.confidence_to
}

/// Matched without regard to case or surrounding space, the same way a category
/// is compared everywhere else it is compared, so a filter set from `beach trip`
/// still catches a photo a re-classification has since renamed `Beach Trip`.
fn admits_category(filters: &Filters, row: &Row<'_>) -> bool {
    match &filters.category {
        Some(wanted) => row.category.trim().eq_ignore_ascii_case(wanted.trim()),
        None => true,
    }
}

/// Orders `rows` into the sequence the grid should draw them.
pub(super) fn sort_rows(
    rows: &mut [(Row<'_>, usize)],
    sort: SortBy,
    direction: SortDirection,
    threshold: f32,
) {
    match sort {
        SortBy::DateTaken => sort_ordered(rows, direction, |row| row.date.sort_key()),
        SortBy::Confidence => {
            // Not `sort_ordered`: `f32` is not `Ord`, so there is no key to
            // cache — and none worth caching either, since the key is a field
            // read. `sort_by` rather than `sort_unstable_by` so that equal
            // confidences keep the order they arrived in.
            //
            // `total_cmp` rather than `<`: confidences are ordinary finite floats
            // today and it costs nothing, but a comparison built on `<` would
            // leave the ordering non-transitive if one ever were NaN, and a
            // non-transitive comparator makes `sort` panic rather than merely
            // misorder.
            rows.sort_by(|(a, _), (b, _)| {
                let (a, b) = (rank_confidence(a, threshold), rank_confidence(b, threshold));
                match direction {
                    SortDirection::Ascending => a.total_cmp(&b),
                    SortDirection::Descending => b.total_cmp(&a),
                }
            });
        }
        SortBy::Category => sort_ordered(rows, direction, |row| category_sort_key(row.category)),
    }
}

/// Sorts `rows` by a key derived from each one, in the given direction.
///
/// The key is computed once per row rather than once per comparison: a
/// comparison sort calls its comparator O(n log n) times, and the category key
/// allocates a fresh `String` every call, which on a folder of any size is the
/// most expensive thing in this frame.
///
/// Descending is expressed by inverting the *key*, never by reversing the run
/// afterwards. Rows that tie — two files from the same month, two at the same
/// confidence — have nothing to tell each other apart, and `reverse` would
/// shuffle them every time the user checked the other end of the order.
/// `sort_by_cached_key` breaks ties by the position each row arrived in, so
/// inverting the key leaves that tie-break untouched: the rows stay put and only
/// the groups between them move.
fn sort_ordered<K: Ord>(
    rows: &mut [(Row<'_>, usize)],
    direction: SortDirection,
    key: impl Fn(&Row<'_>) -> K,
) {
    match direction {
        SortDirection::Ascending => rows.sort_by_cached_key(|(row, _)| key(row)),
        SortDirection::Descending => {
            rows.sort_by_cached_key(|(row, _)| std::cmp::Reverse(key(row)))
        }
    }
}

impl PhotoOrganizerApp {
    /// Whether this frame should keep the order the grid already has, rather than
    /// re-sorting it.
    ///
    /// True while a cell's category editor holds focus and the sort itself has not
    /// changed. Editing a category is what moves it in a Category sort, so
    /// re-sorting mid-word pulls the row out from under the caret — and egui
    /// identifies a cell's widgets by position, so the field under the caret
    /// silently becomes another photo's and the characters land there instead.
    ///
    /// Derived rather than stored, so everything asking what is in view — the
    /// toolbar, the footer, the grid — gets the same answer without depending on
    /// where in the frame it happens to be worked out.
    pub(super) fn holding_order(&self) -> bool {
        self.editing_cell && (self.sort_by, self.sort_direction) == self.order_sort
    }

    pub(super) fn visible_indices(&self) -> Vec<usize> {
        let threshold = self.settings.confidence_threshold;
        let mut rows: Vec<(Row<'_>, usize)> = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| admits(&self.filters, &Row::from(*item), threshold))
            .map(|(index, item)| (Row::from(item), index))
            .collect();

        if !self.holding_order() {
            sort_rows(&mut rows, self.sort_by, self.sort_direction, threshold);
        }

        rows.into_iter().map(|(_, index)| index).collect()
    }

    /// How many photos are in view, and how many are staged in total.
    ///
    /// Both numbers rather than one, because the footer and the filter bar can
    /// only say "3 of 8 shown" if they know the 8 is 8 of something larger.
    pub(super) fn view_counts(&self) -> (usize, usize) {
        (self.visible_indices().len(), self.items.len())
    }
}

/// The category names worth offering in the filter picker: lower-cased and
/// deduped, for the combo box to show.
pub(super) fn category_choices<'a>(rows: impl IntoIterator<Item = Row<'a>>) -> Vec<String> {
    let mut names: Vec<String> = rows
        .into_iter()
        .map(|row| row.category.trim().to_lowercase())
        .filter(|name| !name.is_empty())
        .collect();
    names.sort();
    names.dedup();
    names
}

/// The newest `(year, month)` among the staged photos, or today's if nothing is
/// staged.
///
/// Where the date picker opens by default. A photo library is read forwards from
/// its most recent end far more often than backwards from 1970, so that is where
/// the calendar starts.
pub(super) fn newest_date<'a>(rows: impl IntoIterator<Item = Row<'a>>) -> (u32, u32) {
    rows.into_iter()
        .map(|row| (row.date.year, row.date.month))
        .max()
        .unwrap_or_else(|| {
            let today = chrono::Local::now().date_naive();
            use chrono::Datelike;
            (today.year() as u32, today.month())
        })
}

/// The direction button's text: which way the *current* sort runs, named in
/// terms of what it does to that sort, since "Ascending" means oldest-first for a
/// date, lowest-first for a confidence and first-letter-first for a category.
pub(super) fn direction_label(sort: SortBy, direction: SortDirection) -> &'static str {
    match (sort, direction) {
        (SortBy::DateTaken, SortDirection::Ascending) => "↑ Oldest first",
        (SortBy::DateTaken, SortDirection::Descending) => "↓ Newest first",
        (SortBy::Confidence, SortDirection::Ascending) => "↑ Least confident",
        (SortBy::Confidence, SortDirection::Descending) => "↓ Most confident",
        (SortBy::Category, SortDirection::Ascending) => "↑ A to Z",
        (SortBy::Category, SortDirection::Descending) => "↓ Z to A",
    }
}

impl std::fmt::Display for SortBy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::DateTaken => "Date Taken",
            Self::Confidence => "Confidence",
            Self::Category => "Category",
        })
    }
}

impl SortDirection {
    /// The other direction.
    pub(super) fn toggled(self) -> Self {
        match self {
            Self::Ascending => Self::Descending,
            Self::Descending => Self::Ascending,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::category_name::CategoryName;

    /// The unsorted name, as a literal so a `Row` can borrow it.
    ///
    /// Pinned to `CategoryName::unsorted()` by
    /// [`the_unsorted_name_the_sort_keys_for_is_the_one_decisions_carry`], since a
    /// copy of the string is exactly the thing that could drift.
    const UNSORTED: &str = "Unsorted";

    const THRESHOLD: f32 = 0.65;

    /// A row on a given day of a month.
    fn row(
        year: u32,
        month: u32,
        day: u32,
        category: &'static str,
        confidence: f32,
    ) -> Row<'static> {
        row_on(PhotoDate::new(year, month, Some(day)), category, confidence)
    }

    /// A date with no day, the way a photo cached before the day column existed
    /// reports itself.
    fn month(year: u32, month: u32) -> PhotoDate {
        PhotoDate::new(year, month, None)
    }

    /// A row reduced to the fields the view logic reads. Dates, names,
    /// confidences and sources are the whole vocabulary here, which is why these
    /// tests need neither a texture nor a live `Context`.
    fn row_on(date: PhotoDate, category: &'static str, confidence: f32) -> Row<'static> {
        Row {
            date,
            category,
            confidence,
            source: ClassificationSource::VisualModel,
        }
    }

    fn manual(row: Row<'_>) -> Row<'_> {
        Row {
            source: ClassificationSource::Manual,
            ..row
        }
    }

    fn by_rules(row: Row<'_>) -> Row<'_> {
        Row {
            source: ClassificationSource::Heuristic,
            ..row
        }
    }

    fn unsorted(row: Row<'_>) -> Row<'_> {
        Row {
            source: ClassificationSource::UnsortedFallback,
            ..row
        }
    }

    #[test]
    fn open_filters_admit_everything() {
        let filters = Filters::default();
        assert!(admits(
            &filters,
            &row(2019, 3, 1, "Beach Trip", 0.5),
            THRESHOLD
        ));
        assert!(admits(&filters, &row(2019, 3, 1, UNSORTED, 0.0), THRESHOLD));
        assert!(admits(&filters, &row(2019, 3, 1, "", 0.0), THRESHOLD));
    }

    #[test]
    fn a_date_range_keeps_only_what_is_inside_it() {
        let filters = Filters {
            date_from: Some(month(2020, 6)),
            date_to: Some(month(2021, 3)),
            ..Filters::default()
        };

        // Both ends inclusive, so the boundary months themselves survive.
        assert!(admits(&filters, &row(2020, 6, 1, "a", 0.5), THRESHOLD));
        assert!(admits(&filters, &row(2021, 3, 1, "a", 0.5), THRESHOLD));
        assert!(!admits(&filters, &row(2020, 5, 1, "a", 0.5), THRESHOLD));
        assert!(!admits(&filters, &row(2021, 4, 1, "a", 0.5), THRESHOLD));
    }

    #[test]
    fn an_open_ended_date_range_bounds_only_one_side() {
        let from = Filters {
            date_from: Some(month(2020, 1)),
            ..Filters::default()
        };
        assert!(admits(&from, &row(2030, 12, 1, "a", 0.5), THRESHOLD));
        assert!(!admits(&from, &row(2019, 12, 1, "a", 0.5), THRESHOLD));

        let to = Filters {
            date_to: Some(month(2020, 1)),
            ..Filters::default()
        };
        assert!(admits(&to, &row(1999, 1, 1, "a", 0.5), THRESHOLD));
        assert!(!admits(&to, &row(2020, 2, 1, "a", 0.5), THRESHOLD));
    }

    #[test]
    fn a_confidence_range_is_inclusive_at_both_ends() {
        let filters = Filters {
            confidence_from: 0.7,
            confidence_to: 0.9,
            ..Filters::default()
        };

        assert!(admits(&filters, &row(2024, 1, 1, "a", 0.7), THRESHOLD));
        assert!(admits(&filters, &row(2024, 1, 1, "a", 0.9), THRESHOLD));
        assert!(!admits(&filters, &row(2024, 1, 1, "a", 0.69), THRESHOLD));
        assert!(!admits(&filters, &row(2024, 1, 1, "a", 0.91), THRESHOLD));
    }

    #[test]
    fn a_confidence_floor_leaves_out_rule_assigned_photos() {
        // The floor is measured against the ranked confidence, so asking for
        // "the photos the model was sure about" gets the accepted matches and
        // not the rules that only ran because the model was unsure.
        let filters = Filters {
            confidence_from: 0.7,
            ..Filters::default()
        };

        assert!(admits(
            &filters,
            &row(2024, 1, 1, "Beach Trip", 0.70),
            THRESHOLD
        ));
        assert!(!admits(
            &filters,
            &by_rules(row(2024, 1, 1, "Screenshots", 0.92)),
            THRESHOLD
        ));
    }

    #[test]
    fn the_widest_confidence_range_admits_a_rule_assigned_photo() {
        // Zero and one are the "no floor, no ceiling" resting positions, so a
        // rule-assigned photo ranked just under the threshold has to land inside
        // them. Ranking it below zero instead would silently drop it out of a
        // filter the user had not touched.
        let filters = Filters::default();

        assert!(admits(
            &filters,
            &by_rules(row(2024, 1, 1, "Screenshots", 0.92)),
            THRESHOLD
        ));
        // Including at a threshold of zero, where `next_down` goes negative.
        assert!(admits(
            &filters,
            &by_rules(row(2024, 1, 1, "Screenshots", 0.92)),
            0.0
        ));
    }

    #[test]
    fn open_filters_are_reported_as_open() {
        // The footer and the toolbar both key off this to decide whether there is
        // a narrower set worth mentioning at all.
        assert!(Filters::default().is_open());
        assert!(!Filters {
            category: Some("Beach Trip".to_string()),
            ..Filters::default()
        }
        .is_open());
        assert!(!Filters {
            confidence_from: 0.5,
            ..Filters::default()
        }
        .is_open());
        assert!(!Filters {
            date_to: Some(month(2020, 1)),
            ..Filters::default()
        }
        .is_open());
    }

    #[test]
    fn a_category_filter_ignores_case_and_surrounding_space() {
        let filters = Filters {
            category: Some("beach trip".to_string()),
            ..Filters::default()
        };

        assert!(admits(
            &filters,
            &row(2024, 1, 1, "Beach Trip", 0.5),
            THRESHOLD
        ));
        assert!(admits(
            &filters,
            &row(2024, 1, 1, "  beach trip  ", 0.5),
            THRESHOLD
        ));
        assert!(!admits(
            &filters,
            &row(2024, 1, 1, "Documents", 0.5),
            THRESHOLD
        ));
    }

    #[test]
    fn the_three_filters_combine_rather_than_override_each_other() {
        // Each axis alone would admit this photo; together they must not. If any
        // one short-circuited to true, the filter panel would be three controls
        // all setting the same single filter.
        let filters = Filters {
            date_from: Some(month(2020, 1)),
            date_to: Some(month(2020, 12)),
            confidence_from: 0.8,
            confidence_to: 0.95,
            category: Some("Beach Trip".to_string()),
        };

        // Right date, right category, but not confident enough.
        assert!(!admits(
            &filters,
            &row(2020, 6, 1, "Beach Trip", 0.5),
            THRESHOLD
        ));
        // Confident enough and right category, but the wrong year.
        assert!(!admits(
            &filters,
            &row(2019, 6, 1, "Beach Trip", 0.9),
            THRESHOLD
        ));
        // In range and confident, but a different category.
        assert!(!admits(
            &filters,
            &row(2020, 6, 1, "Documents", 0.9),
            THRESHOLD
        ));
        // All three at once.
        assert!(admits(
            &filters,
            &row(2020, 6, 1, "Beach Trip", 0.9),
            THRESHOLD
        ));
    }

    #[test]
    fn a_rule_assigned_category_ranks_below_the_threshold_it_failed() {
        // The rules only ran because the best centroid fell short of the
        // threshold, so a rule's 90% is certainty about the *rule*. Ranking it
        // against a real 70% CLIP match would put the screenshot on top.
        let rules = by_rules(row(2024, 1, 1, "Screenshots", 0.90));
        let model = row(2024, 1, 1, "Beach Trip", 0.70);

        assert!(rank_confidence(&rules, THRESHOLD) < THRESHOLD);
        assert!(rank_confidence(&model, THRESHOLD) > THRESHOLD);
        assert!(
            rank_confidence(&model, THRESHOLD) > rank_confidence(&rules, THRESHOLD),
            "an accepted match must outrank a rule-assigned photo"
        );
    }

    #[test]
    fn a_rule_assigned_category_keeps_its_own_number_among_the_rules() {
        // Pinning every rule-assigned row to the same value would leave them in
        // whatever order the folder was walked, discarding the only signal the
        // rules actually produce.
        let sure = by_rules(row(2024, 1, 1, "Documents", 0.95));
        let unsure = by_rules(row(2024, 1, 1, "Screenshots", 0.70));

        assert!(rank_confidence(&sure, THRESHOLD) > rank_confidence(&unsure, THRESHOLD));
    }

    #[test]
    fn the_rank_follows_the_threshold_the_user_set() {
        // The bar is a setting, so a rules-assigned photo tracks wherever the
        // slider is rather than sitting at a hardcoded number.
        let rules = by_rules(row(2024, 1, 1, "Screenshots", 0.90));

        assert!(rank_confidence(&rules, 0.30) < 0.30);
        assert!(rank_confidence(&rules, 0.95) < 0.95);
    }

    #[test]
    fn an_unsorted_photo_keeps_its_own_similarity() {
        // Its number is a real centroid similarity, just a low one, and a low one
        // is what should sink it. Pinning it to the threshold too would float it
        // above the rule-assigned rows that beat it.
        let row = unsorted(row(2024, 1, 1, UNSORTED, 0.42));

        assert_eq!(rank_confidence(&row, THRESHOLD), 0.42);
    }

    #[test]
    fn a_manual_pick_keeps_its_own_number() {
        // The user chose this one; its confidence is whatever centroid it was
        // picked from, and nothing about it should be second-guessed here.
        let row = manual(row(2024, 1, 1, "Beach Trip", 0.88));

        assert_eq!(rank_confidence(&row, THRESHOLD), 0.88);
    }

    #[test]
    fn the_unsorted_name_the_sort_keys_for_is_the_one_decisions_carry() {
        // The sort key has to recognise the unsorted bucket by the name the
        // pipeline actually gives it. If `CategoryName::unsorted()` ever stops
        // spelling it this way, every unsorted photo files as an ordinary category
        // between "Travel" and "Vacation" again.
        assert_eq!(CategoryName::unsorted().as_str(), UNSORTED);
        assert!(is_unclassified(UNSORTED));
        assert!(is_unclassified("UNSORTED"));
    }

    #[test]
    fn unsorted_and_uncategorised_sort_after_every_real_name() {
        assert!(category_sort_key("apples") < category_sort_key("beaches"));
        assert!(category_sort_key(UNSORTED) > category_sort_key("beaches"));
        assert!(category_sort_key("") > category_sort_key("beaches"));
        assert!(category_sort_key("   ") > category_sort_key("beaches"));
        assert!(category_sort_key("unsorted") > category_sort_key("beaches"));
        // Case-blind, so two spellings of one category cannot split apart.
        assert_eq!(
            category_sort_key("Beach Trip"),
            category_sort_key("beach trip")
        );
    }

    /// Runs the real sort over the given rows and returns their category names in
    /// the order it produces, so an assertion reads as the ordering a user would
    /// see rather than as a list of indices.
    fn sorted_names<'a>(
        rows: Vec<Row<'a>>,
        sort: SortBy,
        direction: SortDirection,
    ) -> Vec<&'a str> {
        let mut rows: Vec<(Row<'_>, usize)> = rows
            .into_iter()
            .enumerate()
            .map(|(index, row)| (row, index))
            .collect();
        sort_rows(&mut rows, sort, direction, THRESHOLD);
        rows.into_iter().map(|(row, _)| row.category).collect()
    }

    #[test]
    fn dates_sort_oldest_first_and_newest_first() {
        let rows = vec![
            row(2021, 5, 1, "c", 0.5),
            row(2019, 12, 1, "a", 0.5),
            row(2020, 1, 1, "b", 0.5),
        ];

        assert_eq!(
            sorted_names(rows.clone(), SortBy::DateTaken, SortDirection::Ascending),
            vec!["a", "b", "c"]
        );
        assert_eq!(
            sorted_names(rows, SortBy::DateTaken, SortDirection::Descending),
            vec!["c", "b", "a"]
        );
    }

    #[test]
    fn a_month_sorts_inside_its_year() {
        // Year-then-month as one comparison rather than the two fields compared
        // independently, which is what the tuple key buys.
        let rows = vec![row(2020, 11, 1, "late", 0.5), row(2020, 2, 1, "early", 0.5)];

        assert_eq!(
            sorted_names(rows, SortBy::DateTaken, SortDirection::Ascending),
            vec!["early", "late"]
        );
    }

    #[test]
    fn rows_tied_on_the_sort_key_keep_a_stable_order() {
        // Three files from one month have nothing to distinguish them. Flipping
        // the direction must not reshuffle them, or the grid jumps about every
        // time the user checks the other end.
        let rows = vec![
            row(2020, 6, 1, "first", 0.5),
            row(2020, 6, 1, "second", 0.5),
            row(2020, 6, 1, "third", 0.5),
        ];

        assert_eq!(
            sorted_names(rows.clone(), SortBy::DateTaken, SortDirection::Ascending),
            vec!["first", "second", "third"]
        );
        assert_eq!(
            sorted_names(rows.clone(), SortBy::DateTaken, SortDirection::Descending),
            vec!["first", "second", "third"]
        );
        assert_eq!(
            sorted_names(rows.clone(), SortBy::Confidence, SortDirection::Descending),
            vec!["first", "second", "third"]
        );
    }

    #[test]
    fn confidence_sorts_model_matches_above_rule_assigned_photos() {
        let rows = vec![
            by_rules(row(2024, 1, 1, "rule", 0.92)),
            row(2024, 1, 1, "weak match", 0.70),
            row(2024, 1, 1, "strong match", 0.88),
        ];

        assert_eq!(
            sorted_names(rows, SortBy::Confidence, SortDirection::Descending),
            vec!["strong match", "weak match", "rule"]
        );
    }

    #[test]
    fn confidence_sorts_the_weakest_way_round_too() {
        let rows = vec![
            by_rules(row(2024, 1, 1, "rule", 0.92)),
            row(2024, 1, 1, "strong match", 0.88),
        ];

        assert_eq!(
            sorted_names(rows, SortBy::Confidence, SortDirection::Ascending),
            vec!["rule", "strong match"]
        );
    }

    #[test]
    fn categories_sort_alphabetically_with_the_unclassified_last() {
        let rows = vec![
            row(2024, 1, 1, "Travel", 0.5),
            row(2024, 1, 1, UNSORTED, 0.5),
            row(2024, 1, 1, "apples", 0.5),
            row(2024, 1, 1, "Beaches", 0.5),
            row(2024, 1, 1, "", 0.5),
        ];

        // Case-blind, so "apples" and "Beaches" interleave instead of filing all
        // the capitals first. Both ways of having no category sink to the end.
        assert_eq!(
            sorted_names(rows.clone(), SortBy::Category, SortDirection::Ascending),
            vec!["apples", "Beaches", "Travel", UNSORTED, ""]
        );
        // `Unsorted` and the empty name tie on the same key, so they keep the
        // order they arrived in going *down* as well as up — the groups between
        // them reverse, the pair does not.
        assert_eq!(
            sorted_names(rows, SortBy::Category, SortDirection::Descending),
            vec![UNSORTED, "", "Travel", "Beaches", "apples"]
        );
    }

    #[test]
    fn dates_sort_by_day_not_just_by_month() {
        // The reason this exists at all: three photos from one month used to be
        // indistinguishable, so a "Date Taken" sort silently fell back to whatever
        // order the folder was walked in.
        let rows = vec![
            row(2021, 3, 20, "late", 0.5),
            row(2021, 3, 4, "early", 0.5),
            row(2021, 3, 11, "middle", 0.5),
        ];

        assert_eq!(
            sorted_names(rows.clone(), SortBy::DateTaken, SortDirection::Ascending),
            vec!["early", "middle", "late"]
        );
        assert_eq!(
            sorted_names(rows, SortBy::DateTaken, SortDirection::Descending),
            vec!["late", "middle", "early"]
        );
    }

    #[test]
    fn a_day_orders_against_its_neighbouring_months() {
        // The day has to take part in the comparison, not merely break ties
        // within a month: 1 March is after any February and before any April.
        let rows = vec![
            row(2021, 4, 1, "april", 0.5),
            row(2021, 2, 28, "february", 0.5),
            row(2021, 3, 31, "march", 0.5),
        ];

        assert_eq!(
            sorted_names(rows, SortBy::DateTaken, SortDirection::Ascending),
            vec!["february", "march", "april"]
        );
    }

    #[test]
    fn photos_with_no_recorded_day_sort_before_the_rest_of_their_month() {
        // A row cached before the day column existed has none. It sorts first in
        // its month — arbitrary, but stable, so two undated photos do not trade
        // places on every frame.
        let rows = vec![
            row(2021, 3, 4, "known", 0.5),
            row_on(month(2021, 3), "undated", 0.5),
        ];

        assert_eq!(
            sorted_names(rows, SortBy::DateTaken, SortDirection::Ascending),
            vec!["undated", "known"]
        );
    }

    #[test]
    fn a_date_range_can_be_narrowed_to_a_single_day() {
        let filters = Filters {
            date_from: Some(PhotoDate::new(2021, 3, Some(4))),
            date_to: Some(PhotoDate::new(2021, 3, Some(6))),
            ..Filters::default()
        };

        assert!(admits(&filters, &row(2021, 3, 4, "a", 0.5), THRESHOLD));
        assert!(admits(&filters, &row(2021, 3, 6, "a", 0.5), THRESHOLD));
        assert!(!admits(&filters, &row(2021, 3, 3, "a", 0.5), THRESHOLD));
        assert!(!admits(&filters, &row(2021, 3, 7, "a", 0.5), THRESHOLD));
    }

    #[test]
    fn a_month_wide_bound_covers_a_photo_dated_to_the_day() {
        // "March 2021" must keep a photo taken on the 14th — the bound is as coarse
        // as it was written, not as coarse as it can be.
        let filters = Filters {
            date_from: Some(month(2021, 3)),
            date_to: Some(month(2021, 3)),
            ..Filters::default()
        };

        assert!(admits(&filters, &row(2021, 3, 1, "a", 0.5), THRESHOLD));
        assert!(admits(&filters, &row(2021, 3, 31, "a", 0.5), THRESHOLD));
        assert!(!admits(&filters, &row(2021, 2, 28, "a", 0.5), THRESHOLD));
        assert!(!admits(&filters, &row(2021, 4, 1, "a", 0.5), THRESHOLD));
    }

    #[test]
    fn a_photo_with_no_recorded_day_survives_a_month_wide_range() {
        // Dropping undated photos from any date-filtered view would hide them
        // silently, and a photo from a folder scanned before the day column existed
        // is exactly the one a user might be looking for.
        let filters = Filters {
            date_from: Some(month(2021, 3)),
            date_to: Some(month(2021, 3)),
            ..Filters::default()
        };

        assert!(admits(
            &filters,
            &row_on(month(2021, 3), "undated", 0.5),
            THRESHOLD
        ));
        assert!(!admits(
            &filters,
            &row_on(month(2021, 4), "elsewhere", 0.5),
            THRESHOLD
        ));
    }

    #[test]
    fn every_sort_survives_an_empty_and_a_single_photo_grid() {
        assert!(sorted_names(Vec::new(), SortBy::Category, SortDirection::Descending).is_empty());

        let one = vec![row(2020, 1, 1, "only", 0.5)];
        for sort in [SortBy::DateTaken, SortBy::Confidence, SortBy::Category] {
            for direction in [SortDirection::Ascending, SortDirection::Descending] {
                assert_eq!(sorted_names(one.clone(), sort, direction), vec!["only"]);
            }
        }
    }

    #[test]
    fn the_category_picker_offers_each_name_once() {
        // The picker builds from the staged photos, so a folder that arrived with
        // two spellings of one category must not offer them as two choices.
        let rows: Vec<Row<'_>> = ["Beach Trip", "beach trip", "Documents", "  ", ""]
            .into_iter()
            .map(|category| row(2024, 1, 1, category, 0.5))
            .collect();

        assert_eq!(category_choices(rows), vec!["beach trip", "documents"]);
    }
}
