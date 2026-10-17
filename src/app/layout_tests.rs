//! Layout assertions driven through `egui_kittest`.
//!
//! These render the real `render_category_selector`,
//! `render_custom_category_input` and `render_profile_list` into a headless egui
//! context and assert on the geometry egui actually computes. That matters
//! because the original column-overflow bug was invisible to source-text
//! checks: the code looked right, but egui placed the widgets on one line and
//! pushed the row wider than the grid column.

use super::layout::{self, MIN_CONTROL_WIDTH};
use super::models::{Filters, SortBy, SortDirection, StagedItem};
use super::profiles_modal::DeletePrompt;
use super::PhotoOrganizerApp;
use crate::classification::{ClassificationSource, FrameSize, PhotoDate};
use crate::profile_store::{CategoryProfile, ProfileStore};
use eframe::egui;
use egui_kittest::kittest::{by, Node, Queryable};
use egui_kittest::Harness;
use std::fs;
use std::path::PathBuf;

/// One widget as laid out by egui, in points.
#[derive(Debug, Clone)]
struct WidgetRect {
    role: String,
    label: String,
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
}

impl WidgetRect {
    fn width(&self) -> f64 {
        self.x1 - self.x0
    }

    fn height(&self) -> f64 {
        self.y1 - self.y0
    }

    /// True when the two rects share any vertical band, i.e. sit on the
    /// same visual row.
    fn same_row(&self, other: &WidgetRect) -> bool {
        self.y0 < other.y1 && other.y0 < self.y1
    }
}

/// A staged item wired for layout purposes. `is_custom` is true so the
/// custom category input is rendered.
///
/// The texture handle has to be created against a live `Context`, so the
/// caller passes one in; `Harness::new_ui_state` hands us `ui.ctx()`.
fn staged_item(ctx: &egui::Context) -> StagedItem {
    StagedItem {
        source_path: PathBuf::from("/photos/sample.jpg"),
        date: PhotoDate::new(2024, 5, None),
        is_exif: true,
        frame: Some(FrameSize::new(3000, 2000)),
        category: "Beach Trip".into(),
        confidence: 0.5,
        source: ClassificationSource::Manual,
        pending: false,
        embedding: vec![0.1, 0.2, 0.3],
        selected: false,
        is_custom: true,
        texture: ctx.load_texture(
            "thumb",
            egui::ColorImage::new([64, 64], egui::Color32::from_gray(128)),
            egui::TextureOptions::default(),
        ),
    }
}

/// Lays out the grid cell's category controls into a harness `cell_width`
/// points wide and returns every widget egui placed, in points.
fn render_category_cell(cell_width: f32) -> Vec<WidgetRect> {
    let store = ProfileStore::default();

    let mut harness = Harness::new_ui_state(
        move |ui, item: &mut Option<StagedItem>| {
            let item = item.get_or_insert_with(|| staged_item(ui.ctx()));
            ui.vertical(|ui| {
                // The production grid cell renderer itself, so these tests
                // measure the real layout rather than a copy of it.
                // The focus flag is the grid's business; this test only cares
                // about geometry.
                let _ = PhotoOrganizerApp::render_grid_cell_controls(
                    &store,
                    ui,
                    item,
                    cell_width,
                    egui::Id::new("cat_combo"),
                );
            });
        },
        None,
    );
    harness.set_size(egui::vec2(cell_width, 400.0));
    harness.run();

    placed_widgets(&harness)
}

/// Lays out the inspection modal's bottom control row into a harness
/// `modal_width` points wide and returns every widget egui placed, in
/// points.
fn render_modal_row(modal_width: f32) -> Vec<WidgetRect> {
    render_modal_row_with(modal_width, |_| {})
}

/// `render_modal_row`, with `tweak` applied to the item before it is laid
/// out, so tests can vary the item's state.
fn render_modal_row_with(
    modal_width: f32,
    tweak: impl Fn(&mut StagedItem) + Send + 'static,
) -> Vec<WidgetRect> {
    let store = ProfileStore::default();

    let mut harness = Harness::new_ui_state(
        move |ui, item: &mut Option<StagedItem>| {
            let item = item.get_or_insert_with(|| staged_item(ui.ctx()));
            tweak(item);
            // The row sits inside the modal's window frame, which eats into
            // the width available to it, so lay it out inside a frame too.
            egui::Frame::window(ui.style()).show(ui, |ui| {
                PhotoOrganizerApp::render_modal_controls(&store, ui, item, 0, 12);
            });
        },
        None,
    );
    harness.set_size(egui::vec2(modal_width, 400.0));
    harness.run();

    placed_widgets(&harness)
}

/// Every widget egui placed in a harness, in points.
///
/// Generic over the harness state so both the category controls and the profile
/// list can share it.
fn placed_widgets<S>(harness: &Harness<'_, S>) -> Vec<WidgetRect> {
    harness
        .kittest_state()
        .query_all(by().recursive(true))
        .filter_map(|node| {
            let b = node.raw_bounds()?;
            let role = format!("{:?}", node.role());
            // The window root is not a widget we lay out.
            if role == "Window" {
                return None;
            }
            let label = node.label().or_else(|| node.value()).unwrap_or_default();
            Some(WidgetRect {
                role,
                label,
                x0: b.x0,
                y0: b.y0,
                x1: b.x1,
                y1: b.y1,
            })
        })
        .collect()
}

/// A profile store holding `names`, each with `samples` exemplars.
fn store_with(names: &[&str], samples: usize) -> ProfileStore {
    ProfileStore {
        profiles: names
            .iter()
            .map(|name| CategoryProfile {
                name: crate::category_name::CategoryName::from_user_input(name),
                centroid: vec![1.0, 0.0],
                sample_count: samples,
            })
            .collect(),
    }
}

/// Names long enough to prove a row fills a card rather than being clipped at
/// a width that only just fits short ones.
fn long_profile_names(count: usize) -> Vec<String> {
    (0..count)
        .map(|i| format!("Category Number {i:02} With An Unusually Long Name"))
        .collect()
}

/// Lays the profile list into a card `card` points big and returns every
/// widget egui placed, in points.
///
/// Nothing is armed, so every row is in its ordinary state. The card's window
/// frame is reproduced because it eats into the width available to the list,
/// which is exactly what would clip a long profile name.
fn render_profile_list_card(card: egui::Vec2, names: &[String]) -> Vec<WidgetRect> {
    render_profile_list_card_with(card, names, &mut DeletePrompt::default())
}

/// `render_profile_list_card` with `prompt` pre-armed, for the geometry of the
/// confirmation state.
fn render_profile_list_card_with(
    card: egui::Vec2,
    names: &[String],
    prompt: &mut DeletePrompt,
) -> Vec<WidgetRect> {
    let profiles = store_with(&names.iter().map(String::as_str).collect::<Vec<_>>(), 3);

    let mut harness = Harness::new_ui_state(
        move |ui, _state: &mut ()| {
            egui::Frame::window(ui.style()).show(ui, |ui| {
                // The production list renderer itself, so these tests measure
                // the real layout rather than a copy of it.
                let _ = PhotoOrganizerApp::render_profile_list(
                    &profiles,
                    ui,
                    prompt,
                    layout::profiles_list_height(card),
                );
            });
        },
        (),
    );
    harness.set_size(card);
    harness.run();

    placed_widgets(&harness)
}

/// The profile rows, ordered the way egui laid them out: by the y of their name
/// label.
///
/// The modal's `🏷 Profiles (n)` header carries the same icon as a row, so it is
/// excluded explicitly.
fn profile_rows(rects: &[WidgetRect]) -> Vec<&WidgetRect> {
    let mut rows: Vec<&WidgetRect> = rects
        .iter()
        .filter(|r| {
            r.role.contains("Label")
                && r.label.starts_with('🏷')
                && !r.label.starts_with("🏷 Profiles (")
        })
        .collect();
    rows.sort_by(|a, b| a.y0.total_cmp(&b.y0));
    rows
}

fn find<'a>(rects: &'a [WidgetRect], role_contains: &str) -> &'a WidgetRect {
    rects
        .iter()
        .find(|r| r.role.contains(role_contains))
        .unwrap_or_else(|| panic!("no widget with role containing {role_contains:?} in {rects:#?}"))
}

fn find_labelled<'a>(rects: &'a [WidgetRect], label_contains: &str) -> &'a WidgetRect {
    rects
        .iter()
        .find(|r| r.label.contains(label_contains))
        .unwrap_or_else(|| panic!("no widget labelled {label_contains:?} in {rects:#?}"))
}

#[test]
fn custom_input_renders_below_the_combo_not_beside_it() {
    // The regression guard for the column-overflow bug. In the production
    // layout the combo+button share a row and the custom input follows
    // underneath, so the input's vertical band must not overlap the row.
    let cell_width = 300.0;
    let rects = render_category_cell(cell_width);

    let combo = find(&rects, "ComboBox");
    let input = find(&rects, "TextInput");

    assert!(
        !combo.same_row(input),
        "custom input must not share a row with the combo.\ncombo: {combo:?}\ninput: {input:?}"
    );
    assert!(
        input.y0 >= combo.y1 - 0.5,
        "custom input should start below the combo.\ncombo: {combo:?}\ninput: {input:?}"
    );

    // On its own line it gets at least the full cell width it asked for,
    // rather than the sliver left over after the combo. (`desired_width`
    // is a floor, not a fixed size, so egui may hand back slightly more.)
    let requested_input_width = f64::from(super::layout::grid_cell_input_width(cell_width));
    assert!(
        input.width() >= requested_input_width,
        "stacked input should render at least its requested {requested_input_width}pt, \
         got {:.1}pt",
        input.width()
    );
}

#[test]
fn grid_cell_controls_stay_within_the_column_width() {
    // Whatever width the window happens to be, no widget in a grid cell may
    // spill past the column it was allocated.
    for available in [300.0, 420.0, 640.0, 900.0, 1240.0, 1600.0, 1920.0] {
        let (columns, item_width) = super::layout::grid_column_layout(available);
        let rects = render_category_cell(item_width);

        assert!(
            !rects.is_empty(),
            "expected widgets to be laid out at available width {available}"
        );
        for r in &rects {
            assert!(
                r.x1 <= item_width as f64 + 0.5 && r.x0 >= -0.5,
                "widget {:?} ({}) escapes its {item_width}pt column at available \
                 width {available} (columns={columns})\nrect: {r:?}",
                r.role,
                r.label,
            );
        }
    }
}

#[test]
fn modal_input_shares_the_row_with_the_combo() {
    // The modal's control row has room to spare, so the custom input sits
    // inline: same row as the combo, immediately after it. This is the
    // deliberate opposite of `custom_input_renders_below_the_combo_not_beside_it`,
    // which guards the grid cell, where a column simply has no room.
    let rects = render_modal_row(1100.0);

    let combo = find(&rects, "ComboBox");
    let input = find(&rects, "TextInput");
    let train = find_labelled(&rects, "Train");

    assert!(
        combo.same_row(input),
        "modal custom input must share the combo's row.\ncombo: {combo:?}\ninput: {input:?}"
    );
    assert!(
        input.x0 >= combo.x1 - 0.5 && input.x1 > combo.x1,
        "modal custom input should start right after the combo.\ncombo: {combo:?}\n\
         input: {input:?}"
    );
    assert!(
        combo.same_row(train),
        "the train button stays on the combo's row alongside the input.\n\
         combo: {combo:?}\ntrain: {train:?}"
    );

    // Sharing the row must not squeeze the input down to nothing, and it
    // must not grow into the gap that keeps it clear of the train button.
    assert!(
        input.width() >= f64::from(MIN_CONTROL_WIDTH),
        "inline modal input should stay usable, got {:.1}pt\ninput: {input:?}",
        input.width()
    );
    assert!(
        input.x1 <= train.x0,
        "inline modal input must not overlap the train button.\ninput: {input:?}\n\
         train: {train:?}"
    );
}

#[test]
fn modal_input_hides_for_known_categories() {
    // The input is a control for the custom path only; showing it for a
    // profile category would let the name drift away from the profile.
    let rects = render_modal_row_with(1100.0, |item| item.is_custom = false);

    assert!(
        !rects.iter().any(|r| r.role.contains("TextInput")),
        "no custom input expected for a known category, got {rects:#?}"
    );
}

#[test]
fn modal_controls_stay_within_the_modal() {
    // The modal is sized from the screen, so the row has to fit whatever
    // width it is given. Navigation plus the category controls need ~530pt
    // before the inline input, and the input takes only what the row has
    // left, so every width the app can actually produce - the window alone
    // insists on 1240pt - lays out without clipping.
    for modal_width in [800.0, 900.0, 1000.0, 1100.0] {
        let rects = render_modal_row(modal_width);

        assert!(
            !rects.is_empty(),
            "expected widgets to be laid out at modal width {modal_width}"
        );
        for r in &rects {
            assert!(
                r.x1 <= modal_width as f64 + 0.5 && r.x0 >= -0.5,
                "widget {:?} ({}) escapes the {modal_width}pt modal\nrect: {r:?}",
                r.role,
                r.label,
            );
        }
    }
}

#[test]
fn every_widget_has_a_positive_size() {
    // A zero- or negative-sized widget is invisible or misclickable; this
    // catches a control collapsed by a bad width calculation.
    let rects = render_category_cell(300.0);
    for r in &rects {
        assert!(
            r.width() > 0.0 && r.height() > 0.0,
            "widget {:?} ({}) collapsed to {}x{}",
            r.role,
            r.label,
            r.width(),
            r.height()
        );
    }
}

/// What a run of the profile list accumulated across frames.
#[derive(Default)]
struct ListRun {
    prompt: DeletePrompt,
    /// What `render_profile_list` asked to delete, one entry per frame.
    requests: Vec<Option<String>>,
}

/// Runs the profile list through several frames, clicking the buttons named by
/// `clicks` in order, and returns what each frame asked to delete.
///
/// This drives the real renderer with real clicks, so it covers the whole
/// arm-then-confirm sequence rather than just the state machine underneath it.
fn click_through_profile_list(card: egui::Vec2, names: &[String], clicks: &[&str]) -> ListRun {
    let profiles = store_with(&names.iter().map(String::as_str).collect::<Vec<_>>(), 3);

    let mut harness = Harness::new_ui_state(
        move |ui, run: &mut ListRun| {
            egui::Frame::window(ui.style()).show(ui, |ui| {
                let request = PhotoOrganizerApp::render_profile_list(
                    &profiles,
                    ui,
                    &mut run.prompt,
                    layout::profiles_list_height(card),
                );
                run.requests.push(request);
            });
        },
        ListRun::default(),
    );
    harness.set_size(card);
    harness.run();

    for label in clicks {
        // Every row carries the same delete button, so the first match is the
        // top row — the one this test is about.
        let button = harness
            .query_all_by_label_contains(label)
            .next()
            .unwrap_or_else(|| panic!("no button labelled {label:?}"));
        button.click();
        harness.run();
    }

    std::mem::take(harness.state_mut())
}

#[test]
fn clicking_delete_asks_for_nothing_until_it_is_confirmed() {
    // The end-to-end guard for the two-step delete, driven with real clicks:
    // one press of a row's ❌ must arm the row and nothing more. Only the
    // confirmation asks for a deletion, and it names the armed profile.
    let card = layout::profiles_modal_size(egui::vec2(1240.0, 900.0));
    let names = long_profile_names(3);

    let armed = click_through_profile_list(card, &names, &["❌"]);
    assert!(
        armed.requests.iter().all(Option::is_none),
        "a single delete click must not ask for a deletion, got {:#?}",
        armed.requests,
    );
    assert!(
        armed.prompt.is_armed(&names[0]),
        "the first delete click should arm the first row, got {:?}",
        armed.prompt,
    );

    // Confirm it, and exactly one frame asks for the profile that was armed.
    let confirmed = click_through_profile_list(card, &names, &["❌", "✔ Confirm"]);
    let asked: Vec<&str> = confirmed
        .requests
        .iter()
        .flatten()
        .map(String::as_str)
        .collect();
    assert_eq!(
        asked,
        vec![names[0].as_str()],
        "confirming should ask for exactly the armed profile, exactly once, \
         got {:#?}",
        confirmed.requests,
    );
    assert!(
        !confirmed.prompt.is_armed(&names[0]),
        "confirming should disarm the row, got {:?}",
        confirmed.prompt,
    );
}

#[test]
fn clicking_cancel_asks_for_nothing() {
    let card = layout::profiles_modal_size(egui::vec2(1240.0, 900.0));
    let names = long_profile_names(3);

    let cancelled = click_through_profile_list(card, &names, &["❌", "Cancel"]);
    assert!(
        cancelled.requests.iter().all(Option::is_none),
        "cancelling must not ask for a deletion, got {:#?}",
        cancelled.requests,
    );
    assert!(
        !cancelled.prompt.is_armed(&names[0]),
        "cancelling should disarm the row, got {:?}",
        cancelled.prompt,
    );
}

#[test]
fn profile_rows_stack_vertically_rather_than_running_on_one_line() {
    // The regression guard for the reason this list became a modal: rendered
    // inline in the toolbar it was one horizontal run of names that grew past
    // the panel. Inside the card each profile gets its own row, top to bottom.
    let card = layout::profiles_modal_size(egui::vec2(1240.0, 900.0));
    let names = long_profile_names(4);
    let rects = render_profile_list_card(card, &names);

    let rows = profile_rows(&rects);
    assert_eq!(
        rows.len(),
        names.len(),
        "every profile should render exactly one row label, got {rects:#?}"
    );

    let mut previous: Option<&WidgetRect> = None;
    for row in &rows {
        if let Some(prev) = previous {
            assert!(
                !prev.same_row(row),
                "profile rows must not share a row.\nprevious: {prev:?}\nrow: {row:?}"
            );
            assert!(
                row.y0 >= prev.y1 - 0.5,
                "a profile row should start below the one above it.\n\
                 previous: {prev:?}\nrow: {row:?}"
            );
        }
        previous = Some(row);
    }

    // The order matches the store, so the list is not shuffled.
    for (row, name) in rows.iter().zip(&names) {
        assert!(
            row.label.contains(name.as_str()),
            "expected a row for {name:?}, got {row:?}"
        );
    }
}

#[test]
fn profile_rows_stay_within_the_card() {
    // Whatever width the card is sized to, a profile name and its delete button
    // have to fit inside it. Long names are used so a card too narrow for them
    // would clip rather than quietly shrink.
    let tall_card = layout::profiles_modal_size(egui::vec2(1240.0, 900.0));
    let names = long_profile_names(4);

    for card_width in [360.0, 400.0, 480.0, 560.0] {
        let card = egui::vec2(card_width, tall_card.y);
        let rects = render_profile_list_card(card, &names);

        assert!(
            !rects.is_empty(),
            "expected widgets to be laid out at card width {card_width}"
        );
        for r in &rects {
            assert!(
                r.x1 <= card_width as f64 + 0.5 && r.x0 >= -0.5,
                "widget {:?} ({}) escapes the {card_width}pt card\nrect: {r:?}",
                r.role,
                r.label,
            );
        }

        // The name label has to survive intact: a truncated row would hide which
        // profile a delete button belongs to.
        for name in &names {
            let row = find_labelled(&rects, name.as_str());
            assert!(
                row.x1 <= card_width as f64 + 0.5,
                "the label for {name:?} is clipped by the {card_width}pt card\nrect: {row:?}"
            );
        }
    }
}

#[test]
fn armed_profile_row_offers_a_confirmation_instead_of_deleting() {
    // The armed row swaps its single delete button for a confirmation, and
    // stays inside the card while it does.
    let card = layout::profiles_modal_size(egui::vec2(1240.0, 900.0));
    let names = long_profile_names(3);
    let mut prompt = DeletePrompt::default();
    prompt.arm(&names[1]);

    let rects = render_profile_list_card_with(card, &names, &mut prompt);

    assert!(
        find_labelled(&rects, "✔ Confirm").width() > 0.0,
        "the armed row should offer a confirm button, got {rects:#?}"
    );
    assert!(
        rects.iter().any(|r| r.label.contains("Cancel")),
        "the armed row should offer a way out, got {rects:#?}"
    );
    assert!(
        find_labelled(&rects, &names[1]).label.contains("Delete"),
        "the armed row should say what it is about to do, got {rects:#?}"
    );

    for r in &rects {
        assert!(
            r.x1 <= card.x as f64 + 0.5 && r.x0 >= -0.5,
            "widget {:?} ({}) escapes the {:?} card\nrect: {r:?}",
            r.role,
            r.label,
            card,
        );
    }
}

#[test]
fn an_oversized_profile_list_scrolls_inside_the_card() {
    // egui lays a scroll area's contents out in full and clips them, so all
    // `count` rows exist in the tree whether or not they are on screen. What
    // the card must not do is grow to fit them: the rows run past its bottom
    // edge, so the tail is clipped and has to be scrolled to.
    let card = layout::profiles_modal_size(egui::vec2(1240.0, 900.0));
    let count = 60;
    let names = long_profile_names(count);
    let rects = render_profile_list_card(card, &names);

    let rows = profile_rows(&rects);
    assert_eq!(
        rows.len(),
        count,
        "every profile should be laid out, scrolling or not"
    );
    assert!(
        rows.last().is_some_and(|r| r.y1 > card.y as f64),
        "{count} rows should overflow the {:?} card rather than stretching it, \
         last row {:?}",
        card,
        rows.last(),
    );

    // More rows than fit, which is the whole reason to scroll.
    let visible = rows.iter().filter(|r| r.y1 <= card.y as f64).count();
    assert!(
        visible > 0 && visible < count,
        "expected some of {count} rows to be off-screen in a {:?} card, \
         but {visible} of them fit",
        card,
    );

    // egui registers a scroll area's bar as a node with no role of its own, and
    // only draws it when the content overflows, so this narrow tall node is the
    // scrollbar the user is given to reach the tail.
    assert!(
        rects
            .iter()
            .any(|r| r.role == "Unknown" && r.width() < 8.0 && r.height() > card.y as f64 * 0.3),
        "expected a vertical scrollbar for {count} rows in a {:?} card, got {rects:#?}",
        card,
    );
}

/// Opens the whole profile modal on a real app, with a profile per entry in
/// `names`, and returns the widgets egui placed plus the card's size.
fn open_profiles_modal(screen: egui::Vec2, names: &[String]) -> (Vec<WidgetRect>, egui::Vec2) {
    let mut app = PhotoOrganizerApp::new();
    app.profiles.profiles = names
        .iter()
        .map(|name| crate::profile_store::CategoryProfile {
            name: crate::category_name::CategoryName::from_user_input(name),
            centroid: vec![1.0, 0.0],
            sample_count: 3,
        })
        .collect();
    app.show_profiles_modal = true;

    let card = layout::profiles_modal_size(screen);
    let mut harness = Harness::new(move |ctx| app.render_profiles_modal(ctx));
    harness.set_size(screen);
    harness.run();

    // The backdrop covers the screen by design, so only the card's own contents
    // are of interest here.
    let rects: Vec<WidgetRect> = placed_widgets(&harness)
        .into_iter()
        .filter(|r| matches!(r.role.as_str(), "Label" | "Button"))
        .collect();

    (rects, card)
}

#[test]
fn the_profile_modal_fits_its_card() {
    // The whole modal, chrome and all, on a real app. The card is pinned to a
    // fixed size and its frame already spends part of that on margins and
    // shadow, so asking for the full card inside the frame pushes the content
    // past the card's edge — this is the guard against that.
    let screen = egui::vec2(1240.0, 900.0);
    let names = long_profile_names(8);
    let (rects, card) = open_profiles_modal(screen, &names);

    assert!(
        !rects.is_empty(),
        "expected the modal to render its contents, got nothing"
    );

    let card_rect = egui::Rect::from_center_size(screen.to_pos2() / 2.0, card);
    let (card_x0, card_y0) = (f64::from(card_rect.min.x), f64::from(card_rect.min.y));
    let (card_x1, card_y1) = (f64::from(card_rect.max.x), f64::from(card_rect.max.y));
    for r in &rects {
        assert!(
            r.x0 >= card_x0 - 0.5 && r.x1 <= card_x1 + 0.5,
            "widget {:?} ({}) escapes the {:?} card\nrect: {r:?}\ncard: {card_rect:?}",
            r.role,
            r.label,
            card,
        );
        assert!(
            r.y0 >= card_y0 - 0.5 && r.y1 <= card_y1 + 0.5,
            "widget {:?} ({}) escapes the {:?} card vertically\nrect: {r:?}\n\
             card: {card_rect:?}",
            r.role,
            r.label,
            card,
        );
    }
}

#[test]
fn the_profile_modal_lists_every_profile_under_a_count() {
    let screen = egui::vec2(1240.0, 900.0);
    let names = long_profile_names(8);
    let (rects, _) = open_profiles_modal(screen, &names);

    assert!(
        find_labelled(&rects, "Profiles (8)").width() > 0.0,
        "the modal should say how many profiles there are, got {rects:#?}"
    );
    assert!(
        find_labelled(&rects, "trained centroid").width() > 0.0,
        "the modal should warn that deleting is permanent, got {rects:#?}"
    );
    assert!(
        find_labelled(&rects, "❌").width() > 0.0,
        "every profile needs a delete button, got {rects:#?}"
    );
    assert_eq!(
        profile_rows(&rects).len(),
        names.len(),
        "every profile should get a row, got {rects:#?}"
    );
}

/// A staged photo with `selected` set, for the bulk move's selection handling.
fn staged_item_selected(ctx: &egui::Context, selected: bool, year: u32, month: u32) -> StagedItem {
    StagedItem {
        date: PhotoDate::new(year, month, None),
        selected,
        ..staged_item(ctx)
    }
}

/// A scratch directory that cleans itself up, named for the test that made it.
///
/// Suffixed with the pid because cargo runs tests on parallel threads and the
/// other filesystem tests in this crate use the same `temp_dir()` root.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("bulk_move_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("could not create the scratch directory");
        Scratch(dir)
    }

    fn join(&self, rest: &str) -> PathBuf {
        self.0.join(rest)
    }

    /// Writes a file, creating its parent directories, so a test can assert on a
    /// layout that only exists if the engine made the directories.
    fn write(&self, rest: &str, contents: &str) -> PathBuf {
        let path = self.join(rest);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, contents).unwrap();
        path
    }

    fn exists(&self, rest: &str) -> bool {
        self.join(rest).exists()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The bulk move card on a real app holding one selected photo per `months`
/// entry, already run one frame.
///
/// The app is the harness state so a test can hover a disabled button and read
/// its tooltip, or press one and see what happened. The settings are pinned
/// rather than read: `PhotoOrganizerApp::new` loads whatever `settings.json` is
/// in the crate root, and an output folder left over from a manual run would
/// decide whether the destination row warns.
fn bulk_move_harness(
    screen: egui::Vec2,
    settings: crate::settings::Settings,
    name: &str,
    months: &'static [(u32, u32)],
) -> Harness<'static, PhotoOrganizerApp> {
    let staged = months
        .iter()
        .enumerate()
        .map(|(i, (year, month))| {
            (
                PathBuf::from(format!("/photos/sample{i}.jpg")),
                *year,
                *month,
            )
        })
        .collect();
    bulk_move_harness_with(screen, settings, name, staged, Filters::default())
}

/// [`bulk_move_harness`] with real source paths, for the tests that move files,
/// and the filters the grid is narrowed to.
fn bulk_move_harness_with(
    screen: egui::Vec2,
    settings: crate::settings::Settings,
    name: &str,
    staged: Vec<(PathBuf, u32, u32)>,
    filters: Filters,
) -> Harness<'static, PhotoOrganizerApp> {
    let mut app = PhotoOrganizerApp::new();
    app.settings = settings;
    app.filters = filters;
    app.show_bulk_move_modal = true;
    app.bulk_move_name = name.to_string();
    // The undo journal's slot is a field precisely so this can point a real
    // transfer at a scratch directory. Left on its default it would overwrite the
    // journal in the crate root — the one the running app's Undo reads — every
    // time the suite ran.
    app.journal_path = std::env::temp_dir()
        .join(format!("bulk_move_journal_{}", std::process::id()))
        .join("last_execution_manifest.json");

    // Staged exactly once, and deliberately not on `items.is_empty()`: both a move
    // and a copy empty the grid once their photos land, so that condition would
    // put the photos straight back and hide whether they were dropped at all.
    // `Cell` because the harness closure is `Fn` and re-staging has to be
    // observable from inside it.
    let staged_once = std::cell::Cell::new(false);
    let mut harness = Harness::new_ui_state(
        move |ui, app: &mut PhotoOrganizerApp| {
            if !staged_once.get() {
                staged_once.set(true);
                app.items = staged
                    .iter()
                    .map(|(path, year, month)| StagedItem {
                        source_path: path.clone(),
                        date: PhotoDate::new(*year, *month, None),
                        selected: true,
                        // Deliberately *not* the name being bulk-filed: the
                        // point of the feature is that the per-photo category is
                        // irrelevant once a name is typed over the selection.
                        category: "Unsorted".into(),
                        ..staged_item(ui.ctx())
                    })
                    .collect();
            }
            app.render_bulk_move_modal(ui.ctx());
        },
        app,
    );
    harness.set_size(screen);
    harness.run();
    harness
}

/// The widgets the bulk move card placed, filtered to what is inside the card.
///
/// The backdrop covers the whole screen, so an unfiltered read is mostly
/// backdrop — the same filter `open_settings_modal` applies, plus `ComboBox`,
/// which is the role egui gives the closed recalled-names list.
fn bulk_move_widgets(
    screen: egui::Vec2,
    harness: &Harness<'_, PhotoOrganizerApp>,
) -> Vec<WidgetRect> {
    let card = layout::bulk_move_modal_size(screen);
    let card_rect = egui::Rect::from_center_size(screen.to_pos2() / 2.0, card);

    placed_widgets(harness)
        .into_iter()
        .filter(|r| {
            matches!(
                r.role.as_str(),
                "Label" | "Button" | "Slider" | "TextInput" | "ComboBox"
            )
        })
        .filter(|r| {
            f64::from(card_rect.min.x) - 0.5 <= r.x0
                && r.x1 <= f64::from(card_rect.max.x) + 0.5
                && f64::from(card_rect.min.y) - 0.5 <= r.y0
                && r.y1 <= f64::from(card_rect.max.y) + 0.5
        })
        .collect()
}

/// Opens the bulk move card at `screen` and returns the widgets inside it.
fn open_bulk_move_modal(
    screen: egui::Vec2,
    settings: crate::settings::Settings,
    name: &str,
    months: &'static [(u32, u32)],
) -> (Vec<WidgetRect>, egui::Vec2) {
    let harness = bulk_move_harness(screen, settings, name, months);
    let card = layout::bulk_move_modal_size(screen);
    (bulk_move_widgets(screen, &harness), card)
}

#[test]
fn the_bulk_move_modal_offers_a_name_a_destination_and_three_ways_out() {
    // The whole feature in one assertion: a name field to type into, somewhere to
    // put the result, and the three actions. Lose any of them and the feature
    // has no route through the UI at all.
    let screen = egui::vec2(1240.0, 900.0);
    let settings = crate::settings::Settings {
        output_folder: Some(std::path::PathBuf::from("/photos/sorted")),
        custom_categories: vec!["Ski 2024".to_string()],
        ..Default::default()
    };
    let (rects, _) = open_bulk_move_modal(screen, settings, "Beach Trip", &[(2024, 7)]);

    assert!(
        !rects.is_empty(),
        "expected the modal to render its contents, got nothing"
    );
    assert!(
        rects.iter().any(|r| r.role.contains("TextInput")),
        "the name has to be typeable, got {rects:#?}"
    );
    assert!(
        rects.iter().any(|r| r.label.contains("/photos/sorted")),
        "the destination has to be on screen before anything is filed, got {rects:#?}"
    );

    for action in ["Assign", "Move Selected", "Copy Selected"] {
        assert!(
            rects.iter().any(|r| r.label.contains(action)),
            "expected a {action:?} action, got {rects:#?}"
        );
    }

    // The recall list is what makes a second batch of the same kind cheap
    // rather than a retype, so a settings list that never reached the card would
    // quietly remove the feature's main convenience. Asserted by role, because
    // a closed egui combo exposes no label — which list is in it is asserted
    // properly in `picking_a_recalled_name_fills_the_field`.
    assert!(
        rects.iter().any(|r| r.role == "ComboBox"),
        "the recalled names should be offered as a list, got {rects:#?}"
    );
}

#[test]
fn picking_a_recalled_name_fills_the_field() {
    // The recall list has to actually recall: clicking an entry fills the name
    // field, which is the whole point of remembering names rather than
    // retyping them. Also the reason the names are stored already sanitised —
    // what the list offers is exactly what will be used as a folder.
    let screen = egui::vec2(1240.0, 900.0);
    let settings = crate::settings::Settings {
        output_folder: Some(std::path::PathBuf::from("/photos/sorted")),
        custom_categories: vec!["Ski 2024".to_string(), "Beach Trip 2023".to_string()],
        ..Default::default()
    };
    let mut harness = bulk_move_harness(screen, settings, "", &[(2024, 7)]);

    // By role rather than label, for the same reason `placed_widgets` stringifies
    // roles: `accesskit` is not a dependency of this crate, and a closed combo
    // exposes no label to search for anyway.
    harness
        .kittest_state()
        .query_all(by().recursive(true))
        .find(|node| format!("{:?}", node.role()) == "ComboBox")
        .expect("expected the recalled-names combo")
        .click();
    harness.run();

    let recalled: Vec<String> = harness
        .query_all_by_label_contains("Ski 2024")
        .map(|n| n.label().unwrap_or_default())
        .collect();
    assert!(
        !recalled.is_empty(),
        "the names from settings should be in the opened list"
    );

    harness
        .query_all_by_label_contains("Beach Trip 2023")
        .next()
        .expect("expected the older name in the list too")
        .click();
    harness.run();

    assert_eq!(
        harness.state().bulk_move_name,
        "Beach Trip 2023",
        "choosing a recalled name has to fill the field it is typed into"
    );
}

#[test]
fn the_bulk_move_preview_says_exactly_where_the_photos_land() {
    // The promise the rest of the app makes, checked before anything moves: one
    // folder, `<Category>/<YYYY>/<MM>/`, under the base the user was shown.
    let screen = egui::vec2(1240.0, 900.0);
    let settings = crate::settings::Settings {
        output_folder: Some(std::path::PathBuf::from("/photos/sorted")),
        ..Default::default()
    };
    let (rects, _) = open_bulk_move_modal(screen, settings, "Beach Trip", &[(2024, 7), (2024, 7)]);

    let preview: Vec<&WidgetRect> = rects.iter().filter(|r| r.label.contains("→")).collect();
    assert_eq!(
        preview.len(),
        1,
        "expected one preview line, got {rects:#?}"
    );
    assert!(
        preview[0].label.contains("2 photo(s)")
            && preview[0]
                .label
                .contains("/photos/sorted/Beach Trip/2024/07"),
        "two photos from one month are one folder, got {:?}",
        preview[0].label
    );
}

#[test]
fn a_bulk_move_spanning_months_says_how_many_folders_it_will_make() {
    // A trip crossing a month boundary files into more than one folder, and the
    // count alone would not say which. Saying so is what makes this a preview
    // rather than a surprise.
    let screen = egui::vec2(1240.0, 900.0);
    let settings = crate::settings::Settings {
        output_folder: Some(std::path::PathBuf::from("/photos/sorted")),
        ..Default::default()
    };
    let (rects, _) = open_bulk_move_modal(
        screen,
        settings,
        "Beach Trip",
        &[(2023, 12), (2024, 1), (2024, 7)],
    );

    let preview = rects
        .iter()
        .find(|r| r.label.contains("folders"))
        .unwrap_or_else(|| panic!("expected a multi-folder preview, got {rects:#?}"));
    assert!(
        preview.label.contains("3 folders"),
        "three distinct months are three folders, got {:?}",
        preview.label
    );
    assert!(
        preview.label.contains("2023/12") && preview.label.contains("2024/07"),
        "the preview should name them, got {:?}",
        preview.label
    );
}

#[test]
fn the_bulk_move_buttons_wait_for_a_name_and_a_destination() {
    // Both are needed, and each is said where it is missing rather than left to a
    // greyed button that explains nothing.
    let screen = egui::vec2(1240.0, 900.0);

    // No name typed, with the destination already set: the name is what is
    // missing, and that is what the disabled Move has to say.
    let mut nameless = bulk_move_harness(
        screen,
        crate::settings::Settings {
            output_folder: Some(std::path::PathBuf::from("/photos/sorted")),
            ..Default::default()
        },
        "   ",
        &[(2024, 7)],
    );
    let move_button = nameless
        .query_all_by_label_contains("Move Selected")
        .next()
        .expect("expected a Move Selected button");
    assert!(move_button.is_disabled(), "a blank name can move nothing");
    move_button.hover();
    nameless.run();
    assert!(
        placed_widgets(&nameless)
            .iter()
            .any(|r| r.label.contains("Type a category name first")),
        "the missing name should be named where the button is pressed"
    );

    // A name but nowhere to put it. The card has to offer its own picker rather
    // than only greying everything and leaving the user stuck.
    let mut homeless = bulk_move_harness(
        screen,
        crate::settings::Settings::default(),
        "Beach Trip",
        &[(2024, 7)],
    );
    assert!(
        placed_widgets(&homeless)
            .iter()
            .any(|r| r.label.contains("Choose Folder")),
        "the card must be able to pick its own destination"
    );
    let move_button = homeless
        .query_all_by_label_contains("Move Selected")
        .next()
        .expect("expected a Move Selected button");
    assert!(
        move_button.is_disabled(),
        "with no destination there is nowhere to move to"
    );
    move_button.hover();
    homeless.run();
    assert!(
        placed_widgets(&homeless)
            .iter()
            .any(|r| r.label.contains("Choose a destination folder first")),
        "the missing destination should be named where the button is pressed"
    );

    // Assign Only is the odd one out and deliberately so: labelling a selection
    // is not a transfer, so a missing folder must not block it.
    let assign = homeless
        .query_all_by_label_contains("Assign Only")
        .next()
        .expect("expected an Assign Only button");
    assert!(
        !assign.is_disabled(),
        "assigning a name needs no destination — nothing is being filed"
    );
}

#[test]
fn assigning_a_name_labels_the_selection_and_leaves_it_staged() {
    // The half of the feature that touches no disk. Manual is the whole point:
    // it is the one source `apply_classification` will not overwrite, so these
    // photos keep this name through a re-classification and a threshold move.
    // If it came out as `VisualModel` the label would evaporate on the next
    // scan, which is the failure this asserts against.
    let screen = egui::vec2(1240.0, 900.0);
    let settings = crate::settings::Settings {
        output_folder: Some(std::path::PathBuf::from("/photos/sorted")),
        ..Default::default()
    };
    let mut harness = bulk_move_harness(screen, settings, "Beach Trip", &[(2024, 7)]);

    harness
        .query_all_by_label_contains("Assign Only")
        .next()
        .expect("expected an Assign Only button")
        .click();
    harness.run();

    let app = harness.state();
    assert!(
        !app.show_bulk_move_modal,
        "the card closes once the label is on the photos"
    );
    assert_eq!(
        app.items.len(),
        1,
        "assigning files nothing, so no photo leaves the grid"
    );
    assert_eq!(app.items[0].category, "Beach Trip");
    assert_eq!(
        app.items[0].source,
        ClassificationSource::Manual,
        "a manual pick survives re-classification; anything else does not"
    );
    assert!(
        app.items[0].is_custom,
        "'Beach Trip' is not a trained profile, so the cell must offer to edit it"
    );
    // Assign is not a transfer, so it must not consume the selection the way a
    // move does: the user is labelling these, and is very likely to review or
    // re-file them next.
    assert!(
        app.items[0].selected,
        "labelling a photo does not deselect it"
    );
    assert!(
        app.settings
            .custom_categories
            .iter()
            .any(|n| n == "Beach Trip"),
        "a name used once has to be offered again, got {:?}",
        app.settings.custom_categories
    );
}

#[test]
fn assigning_a_name_over_a_scan_in_progress_claims_the_photos_it_has_not_reached() {
    // Bulk Move is reachable the moment the grid has rows in it, which during a
    // scan means some of those rows are `Classifying...`. Those photos must come
    // out carrying the typed name like every other one: `mark_manual` alone leaves
    // `pending` set, so `keep_manual` stays false and the scan's answer lands on
    // top of the name — silently, after the status line has already counted them.
    let screen = egui::vec2(1240.0, 900.0);
    let settings = crate::settings::Settings {
        output_folder: Some(std::path::PathBuf::from("/photos/sorted")),
        ..Default::default()
    };
    let mut harness = bulk_move_harness_with(
        screen,
        settings,
        "Beach Trip",
        vec![
            (PathBuf::from("/photos/decided.jpg"), 2024, 7),
            (PathBuf::from("/photos/still_going.jpg"), 2024, 7),
        ],
        Filters::default(),
    );

    // The one photo the model has not reached: a placeholder, not a decision.
    let pending = &mut harness.state_mut().items[1];
    pending.pending = true;
    pending.category = crate::classification::CLASSIFYING_LABEL.into();
    pending.source = ClassificationSource::UnsortedFallback;
    pending.is_custom = false;

    harness
        .query_all_by_label_contains("Assign Only")
        .next()
        .expect("expected an Assign Only button")
        .click();
    harness.run();

    let app = harness.state();
    for (i, item) in app.items.iter().enumerate() {
        assert_eq!(
            item.category, "Beach Trip",
            "photo {i} was selected, so it carries the typed name"
        );
        assert_eq!(
            item.source,
            ClassificationSource::Manual,
            "photo {i} keeps the name only if the pick is the user's"
        );
        assert!(
            !item.pending,
            "photo {i} holds a name now, so it is no longer pending — this is what \
             stops the scan's answer overwriting it"
        );
        assert!(
            item.is_filable(),
            "photo {i} is filable, or Move and Copy would hold it back for a retry \
             against a name the user already gave it"
        );
    }

    // And the number reported is the number that really do: both photos were
    // selected, so both are counted — the pending one included, not quietly
    // dropped from the tally.
    let assigned = &app
        .status_message
        .as_ref()
        .expect("expected a status line")
        .0;
    assert!(
        assigned.contains("2"),
        "both selected photos were assigned, so the count says 2: {assigned:?}"
    );
}

#[test]
fn bulk_move_puts_the_whole_selection_under_the_typed_name() {
    // The end-to-end claim, checked against the filesystem rather than the
    // status line: every selected photo lands under the one name typed, in the
    // `<Category>/<YYYY>/<MM>/` layout the rest of the app promises, no matter
    // what category each photo carried before. Two different months, because a
    // batch that spans a month boundary has to still honour the layout.
    let scratch = Scratch::new("files_under_typed_name");
    let july = scratch.write("incoming/july.jpg", "july bytes");
    let december = scratch.write("incoming/december.jpg", "december bytes");

    let screen = egui::vec2(1240.0, 900.0);
    let settings = crate::settings::Settings {
        output_folder: Some(scratch.join("sorted")),
        ..Default::default()
    };
    let mut harness = bulk_move_harness_with(
        screen,
        settings,
        "Beach Trip",
        vec![(july, 2024, 7), (december, 2023, 12)],
        Filters::default(),
    );

    harness
        .query_all_by_label_contains("Move Selected")
        .next()
        .expect("expected a Move Selected button")
        .click();
    harness.run();

    assert!(
        scratch.exists("sorted/Beach Trip/2024/07/july.jpg"),
        "the July photo belongs in its own month folder"
    );
    assert!(
        scratch.exists("sorted/Beach Trip/2023/12/december.jpg"),
        "and the December one in its own, both under the typed name"
    );
    assert!(
        !scratch.exists("incoming/july.jpg"),
        "a move takes the original, unlike a copy"
    );
    assert!(
        harness.state().items.is_empty(),
        "moved photos leave the grid, as the toolbar's Move already does"
    );
    assert!(
        !harness.state().show_bulk_move_modal,
        "a batch that landed something closes the card, the same as the other \
         two ways out"
    );
}

#[test]
fn a_bulk_move_that_files_nothing_keeps_the_card_and_what_was_typed() {
    // The other half of the close rule. The destination here cannot exist — it
    // sits under a regular file, so `create_dir_all` fails with ENOTDIR for every
    // photo in the batch — which is the ordinary way a bulk move lands nothing:
    // a folder that was deleted, a drive that is not mounted, a path that is
    // pointing at the wrong thing. Closing the card there would throw away the
    // name and the folder the user had already answered, so the retry would be
    // reopening the card and answering both questions again.
    let scratch = Scratch::new("nothing_lands");
    let photo = scratch.write("incoming/july.jpg", "july bytes");
    scratch.write("blocker", "not a directory");

    let screen = egui::vec2(1240.0, 900.0);
    let settings = crate::settings::Settings {
        output_folder: Some(scratch.join("blocker/under")),
        ..Default::default()
    };
    let mut harness = bulk_move_harness_with(
        screen,
        settings,
        "Beach Trip",
        vec![(photo, 2024, 7)],
        Filters::default(),
    );

    harness
        .query_all_by_label_contains("Move Selected")
        .next()
        .expect("expected a Move Selected button")
        .click();
    harness.run();

    assert!(
        !scratch.exists("blocker/under/Beach Trip"),
        "the batch should have filed nothing at all"
    );
    // Confirm the batch actually ran and failed, rather than never starting — an
    // assertion about the card staying open would otherwise also pass if the
    // button had done nothing at all.
    let reported = &harness
        .state()
        .status_message
        .as_ref()
        .expect("a failed batch still says so in the status line")
        .0;
    assert!(
        reported.contains("Nothing was transferred"),
        "the status line should report the failure, got {reported:?}"
    );
    assert!(
        harness.state().show_bulk_move_modal,
        "nothing landed, so the card stays up — its name and folder are part of \
         the retry"
    );
    assert_eq!(
        harness.state().bulk_move_name,
        "Beach Trip",
        "the typed name survives the failed attempt"
    );
    let items = &harness.state().items;
    assert_eq!(items.len(), 1, "the photo is still staged");
    assert!(
        items[0].selected,
        "and still selected, so the retry is the same press of the same button"
    );
}

#[test]
fn a_bulk_move_leaves_the_photos_the_filters_hide_alone() {
    // The bulk move is the fourth route to a category and it was the one the
    // scoping missed: it *labelled* every selected photo while the transfer it
    // runs in front of filed only the visible ones. The result was photos the
    // user could not see, renamed to a name that says where they are, and not
    // moved — so the next Move with the filter cleared filed them there.
    //
    // Both halves are checked: the hidden photo keeps its own category, and it
    // keeps its file.
    let scratch = Scratch::new("bulk_move_respects_the_filters");
    let july = scratch.write("incoming/july.jpg", "july bytes");
    let december = scratch.write("incoming/december.jpg", "december bytes");

    let screen = egui::vec2(1240.0, 900.0);
    let settings = crate::settings::Settings {
        output_folder: Some(scratch.join("sorted")),
        ..Default::default()
    };
    let mut harness = bulk_move_harness_with(
        screen,
        settings,
        "Beach Trip",
        vec![(july, 2024, 7), (december, 2023, 12)],
        Filters {
            date_from: Some(PhotoDate::new(2024, 1, None)),
            date_to: Some(PhotoDate::new(2024, 12, None)),
            ..Filters::default()
        },
    );

    harness
        .query_all_by_label_contains("Move Selected")
        .next()
        .expect("expected a Move Selected button")
        .click();
    harness.run();

    assert!(
        scratch.exists("sorted/Beach Trip/2024/07/july.jpg"),
        "the photo in view is filed under the typed name"
    );
    assert!(
        !scratch.exists("sorted/Beach Trip/2023/12/december.jpg"),
        "and the one the filters hide is not filed at all"
    );
    assert!(
        scratch.exists("incoming/december.jpg"),
        "so it still has its original, where a Move otherwise would have taken it"
    );
    assert_eq!(
        harness.state().items.len(),
        1,
        "and one photo is left staged: the one that was not in the batch"
    );
    assert_eq!(
        harness.state().items[0].category,
        "Unsorted",
        "which must keep the category it had, rather than claiming one the move \
         never gave it"
    );
}

#[test]
fn bulk_copy_leaves_the_originals_and_clears_the_grid() {
    // The same batch as a copy: the files are duplicated under the typed name and
    // the originals stay where they were, which is the whole difference between
    // the two modes. The grid clears either way — a copy is a completed filing,
    // so leaving the row behind would offer the same photo again, still selected
    // and looking untouched, and a second press would file it a second time.
    let scratch = Scratch::new("copy_keeps_originals");
    let july = scratch.write("incoming/july.jpg", "july bytes");

    let screen = egui::vec2(1240.0, 900.0);
    let settings = crate::settings::Settings {
        output_folder: Some(scratch.join("sorted")),
        ..Default::default()
    };
    let mut harness = bulk_move_harness_with(
        screen,
        settings,
        "Beach Trip",
        vec![(july.clone(), 2024, 7)],
        Filters::default(),
    );

    harness
        .query_all_by_label_contains("Copy Selected")
        .next()
        .expect("expected a Copy Selected button")
        .click();
    harness.run();

    assert!(
        scratch.exists("incoming/july.jpg"),
        "a copy leaves the original"
    );
    assert!(scratch.exists("sorted/Beach Trip/2024/07/july.jpg"));
    assert!(
        harness.state().items.is_empty(),
        "a copied photo leaves the grid like a moved one, so the same photo \
         cannot be filed a second time by pressing the button again"
    );
}

#[test]
fn a_name_typed_with_separators_cannot_create_a_directory_level() {
    // The traversal guard, at the point the feature makes it reachable. Filing
    // is free text over a whole selection here, so a name typed as `A/B` would
    // otherwise add a level and `..` would leave the output folder. This leans
    // on `CategoryName` doing its one job; the point of the assertion is that
    // the bulk move routes through it rather than joining the raw string.
    //
    // `../../escaped` has exactly one sanitised outcome: each `/` becomes a `-`,
    // and the result is neither `.` nor `..`, so it stays a legal single
    // component. Asserting that one path rather than accepting either of two is
    // what would catch the sanitiser changing underneath this feature.
    let scratch = Scratch::new("name_is_one_component");
    let photo = scratch.write("incoming/a.jpg", "bytes");

    let screen = egui::vec2(1240.0, 900.0);
    let settings = crate::settings::Settings {
        output_folder: Some(scratch.join("sorted")),
        ..Default::default()
    };
    let mut harness = bulk_move_harness_with(
        screen,
        settings,
        "../../escaped",
        vec![(photo, 2024, 7)],
        Filters::default(),
    );

    harness
        .query_all_by_label_contains("Move Selected")
        .next()
        .expect("expected a Move Selected button")
        .click();
    harness.run();

    // Two levels above the scratch directory is where an unsanitised `../../escaped`
    // would land, so that is the path to check rather than one inside the scratch
    // — a `..` that worked would never have produced a directory here to find.
    let escaped = scratch.join("../../escaped");
    assert!(
        !escaped.exists(),
        "a traversal attempt must not resolve outside the output folder, \
         found {escaped:?}"
    );
    assert!(
        scratch.exists("sorted/..-..-escaped/2024/07/a.jpg"),
        "the photo still has to be filed somewhere, under the one component the \
         sanitiser produced"
    );
}

#[test]
fn a_bulk_move_with_nothing_selected_says_so() {
    // Reachable through the toolbar button being disabled, but the card also has
    // to cope: "nothing selected" is the one state where the preview cannot say
    // anything at all.
    let screen = egui::vec2(1240.0, 900.0);
    let settings = crate::settings::Settings {
        output_folder: Some(std::path::PathBuf::from("/photos/sorted")),
        ..Default::default()
    };

    let mut app = PhotoOrganizerApp::new();
    app.settings = settings;
    app.show_bulk_move_modal = true;
    app.bulk_move_name = "Beach Trip".to_string();

    let mut harness = Harness::new_ui_state(
        move |ui, app: &mut PhotoOrganizerApp| {
            if app.items.is_empty() {
                app.items = vec![staged_item_selected(ui.ctx(), false, 2024, 7)];
            }
            app.render_bulk_move_modal(ui.ctx());
        },
        app,
    );
    harness.set_size(screen);
    harness.run();

    let card = layout::bulk_move_modal_size(screen);
    let card_rect = egui::Rect::from_center_size(screen.to_pos2() / 2.0, card);
    let warned = placed_widgets(&harness).into_iter().any(|r| {
        r.label.contains("Nothing is selected")
            && r.x0 >= f64::from(card_rect.min.x) - 0.5
            && r.x1 <= f64::from(card_rect.max.x) + 0.5
    });
    assert!(
        warned,
        "an empty selection must be stated rather than previewing nothing"
    );
}

#[test]
fn the_bulk_move_modal_fits_its_card() {
    // The same guard the other two cards get. This one is the widest, so it is
    // the one where a long category name and a long destination path would push
    // past the edge.
    for screen in [egui::vec2(1240.0, 900.0), egui::vec2(1920.0, 1080.0)] {
        let settings = crate::settings::Settings {
            output_folder: Some(std::path::PathBuf::from(
                "/home/tobye/Pictures/2026/family holiday/raw scans",
            )),
            custom_categories: vec![
                "Beach Trip".to_string(),
                "Ski 2024".to_string(),
                "Category Number 07 With An Unusually Long Name".to_string(),
            ],
            ..Default::default()
        };
        let (rects, card) = open_bulk_move_modal(screen, settings, "Beach Trip 2024", &[(2024, 7)]);

        assert!(
            !rects.is_empty(),
            "expected the modal to render its contents at {screen:?}, got nothing"
        );

        let card_rect = egui::Rect::from_center_size(screen.to_pos2() / 2.0, card);
        for r in &rects {
            assert!(
                r.x0 >= f64::from(card_rect.min.x) - 0.5
                    && r.x1 <= f64::from(card_rect.max.x) + 0.5,
                "widget {:?} ({}) escapes the {card:?} card at {screen:?}\nrect: {r:?}",
                r.role,
                r.label,
            );
            assert!(
                r.y0 >= f64::from(card_rect.min.y) - 0.5
                    && r.y1 <= f64::from(card_rect.max.y) + 0.5,
                "widget {:?} ({}) escapes the {card:?} card vertically at {screen:?}\nrect: {r:?}",
                r.role,
                r.label,
            );
        }
    }
}

/// The toolbar with `selected` photos staged and no output folder configured,
/// already run one frame. The app is the harness state so a test can press the
/// button and look at what it opened.
fn toolbar_with_selection(selected: bool) -> Harness<'static, PhotoOrganizerApp> {
    let mut app = PhotoOrganizerApp::new();
    app.settings = crate::settings::Settings::default();

    let mut harness = Harness::new_ui_state(
        move |ui, app: &mut PhotoOrganizerApp| {
            if app.items.is_empty() {
                app.items = vec![staged_item_selected(ui.ctx(), selected, 2024, 7)];
            }
            app.render_toolbar(ui.ctx());
        },
        app,
    );
    harness.set_size(egui::vec2(1240.0, 900.0));
    harness.run();
    harness
}

#[test]
fn bulk_move_is_disabled_until_something_is_selected() {
    // The toolbar gates it on the selection rather than the destination, which
    // is the opposite of Move and Copy: the card picks its own folder, so an
    // unconfigured output folder must not disable it. Both halves are asserted
    // because the button would still come out live by accident if only one of
    // them held.
    let mut empty = toolbar_with_selection(false);
    let button = empty
        .query_all_by_label_contains("Bulk Move")
        .next()
        .expect("expected a Bulk Move button");
    assert!(
        button.is_disabled(),
        "with nothing selected there is nothing for the bulk move to act on"
    );
    button.hover();
    empty.run();
    let tooltips: Vec<String> = placed_widgets(&empty)
        .into_iter()
        .map(|r| r.label)
        .filter(|t| t.contains("Select at least one"))
        .collect();
    assert!(
        !tooltips.is_empty(),
        "a disabled Bulk Move must say what would enable it, got {tooltips:#?}"
    );

    // A selection is enough: no output folder is configured here either, and
    // the card picks its own destination.
    let mut staged = toolbar_with_selection(true);
    let button = staged
        .query_all_by_label_contains("Bulk Move")
        .next()
        .expect("expected a Bulk Move button");
    assert!(
        !button.is_disabled(),
        "a selection is enough — the card picks its own destination folder"
    );

    button.click();
    staged.run();
    assert!(
        staged.state().show_bulk_move_modal,
        "the button's whole job is to open the card"
    );
}

/// Opens the settings modal on a real app and returns the widgets egui placed.
fn open_settings_modal(
    screen: egui::Vec2,
    settings: crate::settings::Settings,
) -> (Vec<WidgetRect>, egui::Vec2) {
    let mut app = PhotoOrganizerApp::new();
    app.settings = settings;
    app.show_settings_modal = true;

    let card = layout::settings_modal_size(screen);
    let mut harness = Harness::new(move |ctx| app.render_settings_modal(ctx));
    harness.set_size(screen);
    harness.run();

    // The footer draws across the whole width and the backdrop covers the
    // screen, so only the card's own contents are of interest here.
    let rects: Vec<WidgetRect> = placed_widgets(&harness)
        .into_iter()
        .filter(|r| matches!(r.role.as_str(), "Label" | "Button" | "Slider" | "TextInput"))
        .filter(|r| {
            let card_rect = egui::Rect::from_center_size(screen.to_pos2() / 2.0, card);
            f64::from(card_rect.min.x) - 0.5 <= r.x0 && r.x1 <= f64::from(card_rect.max.x) + 0.5
        })
        .collect();

    (rects, card)
}

#[test]
fn the_settings_modal_offers_all_three_settings() {
    // The regression guard for the relocation: the output folder picker left the
    // toolbar for this modal, so if any of the three goes missing the user has
    // no way to configure it at all.
    let screen = egui::vec2(1240.0, 900.0);
    let (rects, _) = open_settings_modal(screen, crate::settings::Settings::default());

    assert!(
        !rects.is_empty(),
        "expected the settings modal to render its contents, got nothing"
    );
    assert!(
        rects.iter().any(|r| r.role.contains("Slider")),
        "the confidence threshold needs a slider, got {rects:#?}"
    );
    assert!(
        rects.iter().any(|r| r.label.contains("Confidence")),
        "the slider needs a label saying what it is, got {rects:#?}"
    );

    // The folder picker, and the model picker beside it.
    let browse: Vec<&WidgetRect> = rects
        .iter()
        .filter(|r| r.label.contains("Browse"))
        .collect();
    assert_eq!(
        browse.len(),
        2,
        "both the output folder and the model need a browse button, got {browse:#?}"
    );
    assert!(
        find_labelled(&rects, "Transfers").width() > 0.0,
        "expected the transfer destination row, got {rects:#?}"
    );
}

#[test]
fn the_settings_modal_fits_its_card() {
    // The same guard `the_profile_modal_fits_its_card` gives, now covering a
    // card whose contents are wider: a model path is a long string, and the
    // card has to hold it rather than push past its own edge.
    for screen in [egui::vec2(1240.0, 900.0), egui::vec2(1920.0, 1080.0)] {
        let settings = crate::settings::Settings {
            model_path: Some(std::path::PathBuf::from(
                "/home/tobye/Pictures/2026/family holiday/raw scans/clip_vision.safetensors",
            )),
            ..Default::default()
        };
        let (rects, card) = open_settings_modal(screen, settings);

        assert!(
            !rects.is_empty(),
            "expected the modal to render its contents at {screen:?}, got nothing"
        );

        let card_rect = egui::Rect::from_center_size(screen.to_pos2() / 2.0, card);
        for r in &rects {
            assert!(
                r.x0 >= f64::from(card_rect.min.x) - 0.5
                    && r.x1 <= f64::from(card_rect.max.x) + 0.5,
                "widget {:?} ({}) escapes the {card:?} card at {screen:?}\nrect: {r:?}",
                r.role,
                r.label,
            );
            assert!(
                r.y0 >= f64::from(card_rect.min.y) - 0.5
                    && r.y1 <= f64::from(card_rect.max.y) + 0.5,
                "widget {:?} ({}) escapes the {card:?} card vertically at {screen:?}\nrect: {r:?}",
                r.role,
                r.label,
            );
        }
    }
}

/// Opens the settings modal on a real app holding `staged` photos, kept as
/// harness state so a test can read the status line back afterwards.
///
/// The settings are pinned to their defaults: `PhotoOrganizerApp::new` reads
/// whatever `settings.json` happens to be in the crate root, and a threshold
/// left over from a manual run would move the slider's starting point.
fn settings_modal_with_items(staged: usize) -> Harness<'static, PhotoOrganizerApp> {
    let mut app = PhotoOrganizerApp::new();
    app.settings = crate::settings::Settings::default();
    app.classified_threshold = app.settings.confidence_threshold;
    app.show_settings_modal = true;

    let mut harness = Harness::new_ui_state(
        move |ui, app: &mut PhotoOrganizerApp| {
            if app.items.is_empty() {
                app.items = (0..staged).map(|_| staged_item(ui.ctx())).collect();
            }
            app.render_settings_modal(ui.ctx());
        },
        app,
    );
    harness.set_size(egui::vec2(1240.0, 900.0));
    harness
}

/// The threshold slider's rail, as egui laid it out.
fn slider_rail(harness: &Harness<'_, PhotoOrganizerApp>) -> egui::Rect {
    let b = harness
        .kittest_state()
        .query_all(by().recursive(true))
        .find(|node| format!("{:?}", node.role()).contains("Slider"))
        .and_then(|node| node.raw_bounds())
        .expect("the settings modal to draw a threshold slider");

    egui::Rect::from_min_max(
        egui::pos2(b.x0 as f32, b.y0 as f32),
        egui::pos2(b.x1 as f32, b.y1 as f32),
    )
}

/// A point on the rail, `across` of its width from the left end.
fn on_the_rail(rail: egui::Rect, across: f32) -> egui::Pos2 {
    egui::pos2(rail.left() + rail.width() * across, rail.center().y)
}

/// A left mouse button press, on its own frame.
///
/// egui only hands a widget an `interact_pointer_pos` while something is held
/// or was released this frame, so a press and a release queued into one frame
/// cancel out and the widget never sees the pointer at all.
fn press_at(harness: &mut Harness<'_, PhotoOrganizerApp>, at: egui::Pos2) {
    harness
        .input_mut()
        .events
        .push(egui::Event::PointerMoved(at));
    harness.input_mut().events.push(egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: Default::default(),
    });
    harness.step();
}

/// The pointer travelling to a new position with the button still down.
fn drag_to(harness: &mut Harness<'_, PhotoOrganizerApp>, at: egui::Pos2) {
    harness
        .input_mut()
        .events
        .push(egui::Event::PointerMoved(at));
    harness.step();
}

fn release_at(harness: &mut Harness<'_, PhotoOrganizerApp>, at: egui::Pos2) {
    harness.input_mut().events.push(egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: Default::default(),
    });
    harness.step();
}

fn status_text(harness: &Harness<'_, PhotoOrganizerApp>) -> Option<String> {
    harness
        .state()
        .status_message
        .as_ref()
        .map(|(msg, _)| msg.clone())
}

#[test]
fn a_reclassification_leaves_a_still_pending_photo_pending() {
    // **Re-classify All** and both training routes are reachable while a scan is
    // running — which also reaches `reclassify_all`, since training ends in it.
    // Deciding a photo the model has not reached yet would clear its `pending`
    // flag, and that flag is the only thing stopping the fresh write from
    // outranking the decision still on its way: an `Unsorted` with an editable
    // input in front of the user, one keystroke from beating the model for good.
    let ctx = egui::Context::default();
    let mut app = PhotoOrganizerApp::new();
    app.settings = crate::settings::Settings::default();

    let mut decided = staged_item(&ctx);
    decided.selected = true;
    decided.pending = false;
    decided.category = "Sunsets".into();
    decided.source = ClassificationSource::Heuristic;
    decided.is_custom = false;

    // What `StagedItem::new` builds from `Classification::Pending`.
    let mut pending = staged_item(&ctx);
    pending.selected = true;
    pending.pending = true;
    pending.category = crate::classification::CLASSIFYING_LABEL.into();
    pending.confidence = 0.0;
    pending.source = ClassificationSource::UnsortedFallback;
    pending.is_custom = false;

    app.items = vec![decided, pending];
    app.reclassify_all();

    assert!(
        app.items[1].pending,
        "nothing has been decided about this photo, so nothing may decide it"
    );
    assert_eq!(
        app.items[1].category,
        crate::classification::CLASSIFYING_LABEL
    );
    assert!(
        !app.items[1].is_custom,
        "so no editable input is put in front of the placeholder either"
    );

    let status = app
        .status_message
        .clone()
        .map(|(msg, _)| msg)
        .unwrap_or_default();
    assert!(
        status.contains("Re-classified 1 photo(s)"),
        "only the decided photo was re-classified, status line was {status:?}"
    );
}

#[test]
fn the_threshold_is_applied_when_the_handle_is_released() {
    // egui puts the slider where the pointer is on the *press* frame and on
    // every frame the handle travels, so the value has already settled by the
    // time the drag stops: on the release frame there is nothing left to
    // detect. Gating on a per-frame change therefore loses the user's decision
    // entirely, and the grid keeps the classifications from the old threshold.
    let mut harness = settings_modal_with_items(3);
    harness.run();

    let rail = slider_rail(&harness);
    let start = on_the_rail(rail, 0.95);
    let lower = on_the_rail(rail, 0.1);

    press_at(&mut harness, start);
    drag_to(&mut harness, lower);
    assert_eq!(
        status_text(&harness),
        None,
        "the frames a drag travels over must not re-classify the whole grid"
    );

    release_at(&mut harness, lower);
    let status = status_text(&harness).unwrap_or_default();
    assert!(
        status.contains("Re-classified"),
        "releasing the handle should re-classify the grid against the new \
         threshold, status line was {status:?}"
    );
    assert_eq!(
        harness.state().classified_threshold,
        harness.state().settings.confidence_threshold,
        "the grid should now be classified at the threshold on screen"
    );
}

#[test]
fn the_threshold_is_applied_when_nudged_with_the_keyboard() {
    // The other way to move a focused slider. No drag ever starts, so nothing
    // about a pointer release can pick this up.
    let mut harness = settings_modal_with_items(3);
    harness.run();

    let before = harness.state().settings.confidence_threshold;
    harness
        .kittest_state()
        .query_all(by().recursive(true))
        .find(|node| format!("{:?}", node.role()).contains("Slider"))
        .expect("the settings modal to draw a threshold slider")
        .focus();
    harness.run();

    harness.press_key(egui::Key::ArrowRight);
    harness.run();

    assert!(
        harness.state().settings.confidence_threshold > before,
        "the arrow key should move the slider, {before} -> {}",
        harness.state().settings.confidence_threshold
    );
    let status = status_text(&harness).unwrap_or_default();
    assert!(
        status.contains("Re-classified"),
        "an arrow key should re-classify the grid just as a drag does, \
         status line was {status:?}"
    );
}

#[test]
fn a_threshold_change_during_a_scan_waits_for_the_scan() {
    // A scan classifies against the threshold it started with. Re-classifying
    // the moment the slider moves would leave the photos already staged on the
    // old bar and the photos still arriving on the new one, with nothing on
    // screen to explain the split — so the request waits for the scan to end.
    let mut harness = settings_modal_with_items(3);
    harness.run();
    harness.state_mut().is_processing = true;

    let rail = slider_rail(&harness);
    press_at(&mut harness, on_the_rail(rail, 0.95));
    release_at(&mut harness, on_the_rail(rail, 0.1));

    assert_eq!(
        status_text(&harness),
        None,
        "a scan in flight is classifying with its own threshold"
    );
    assert!(
        harness.state().pending_reclassify,
        "the re-classification should be held for the end of the scan, not dropped"
    );

    // The current generation, or the app would drop it as a superseded scan's
    // message and never spend the held re-classification.
    let scan_id = harness.state().scan_id;
    harness
        .state_mut()
        .tx
        .send(crate::scanner::ScanEvent {
            scan_id,
            message: crate::scanner::ScanMessage::Complete,
        })
        .expect("the app's own receiver to still be open");
    let ctx = egui::Context::default();
    harness.state_mut().drain_scan_messages(&ctx);

    let status = status_text(&harness).unwrap_or_default();
    assert!(
        status.contains("Re-classified"),
        "the held re-classification should run once the scan finishes, \
         status line was {status:?}"
    );
    assert!(
        !harness.state().pending_reclassify,
        "the held request is spent once it has run"
    );
}

#[test]
fn a_chosen_model_the_app_cannot_use_is_called_out() {
    // The chosen path is preferred but not required: `find_model_path` carries
    // on to `models/` when it isn't an existing file, so a green dot next to a
    // substituted checkpoint would read as confirmation that a stale or
    // mistyped path is fine.
    let screen = egui::vec2(1240.0, 900.0);
    let settings = crate::settings::Settings {
        model_path: Some(PathBuf::from("/nonexistent/clip_vision.safetensors")),
        ..Default::default()
    };

    let (rects, _) = open_settings_modal(screen, settings);
    assert!(
        rects.iter().any(|r| r.label.contains("can't be used")),
        "a model path the app fell back from should be called out, got {rects:#?}"
    );
}

/// Renders the footer on an app holding `staged` photos of which `selected`
/// are ticked, and returns the widgets egui placed.
///
/// The app is rebuilt each frame rather than kept across the harness, because
/// staging an item needs a live `Context` to create its texture handle.
fn render_footer_with(
    settings: crate::settings::Settings,
    staged: usize,
    selected: usize,
) -> Vec<WidgetRect> {
    let mut harness = Harness::new(move |ctx| {
        let mut app = PhotoOrganizerApp::new();
        app.settings = settings.clone();
        app.items = (0..staged)
            .map(|i| StagedItem {
                selected: i < selected,
                ..staged_item(ctx)
            })
            .collect();
        app.render_footer(ctx);
    });
    harness.set_size(egui::vec2(1240.0, 900.0));
    harness.run();
    placed_widgets(&harness)
}

#[test]
fn the_footer_reports_selection_and_destination() {
    // The destination moved out of the toolbar, so the footer is the only place
    // the user can see where a transfer will land. If it stops rendering the
    // path, Move becomes a button that silently does the wrong thing.
    let settings = crate::settings::Settings {
        output_folder: Some(PathBuf::from("/home/tobye/photos")),
        ..Default::default()
    };

    let rects = render_footer_with(settings, 12, 4);
    let text: Vec<&str> = rects.iter().map(|r| r.label.as_str()).collect();

    assert!(
        text.iter().any(|t| t.contains("4 of 12 selected")),
        "the footer should say how many photos are selected, got {text:#?}"
    );
    assert!(
        text.iter().any(|t| t.contains("/home/tobye/photos")),
        "the footer should show where transfers will land, got {text:#?}"
    );
}

#[test]
fn the_footer_elides_a_long_destination_but_keeps_its_tail() {
    // The path is the only record of the destination, and paths in real photo
    // libraries run long. Eliding from the front keeps the part that identifies
    // the folder; eliding from the back would hide it and leave every
    // destination looking alike.
    let settings = crate::settings::Settings {
        output_folder: Some(PathBuf::from(
            "/home/tobye/Pictures/2026/family holiday/raw scans",
        )),
        ..Default::default()
    };

    let rects = render_footer_with(settings, 3, 1);
    let text: Vec<&str> = rects.iter().map(|r| r.label.as_str()).collect();

    assert!(
        text.iter().any(|t| t.contains("raw scans")),
        "the identifying tail must survive, got {text:#?}"
    );
    assert!(
        text.iter().all(|t| !t.contains("/home/tobye/Pictures")),
        "the elided prefix should be gone, got {text:#?}"
    );
}

#[test]
fn the_footer_warns_when_no_output_folder_is_set() {
    // An unconfigured destination has to be said out loud, and in the place
    // nobody has to hover to see. The toolbar greying Move and Copy is a hint
    // aimed at whoever reaches for them; this is the always-on statement.
    let rects = render_footer_with(crate::settings::Settings::default(), 0, 0);

    let text: Vec<String> = rects.into_iter().map(|r| r.label).collect();

    assert!(
        text.iter().any(|t| t.contains("No output folder")),
        "expected a warning about the missing output folder, got {text:#?}"
    );
}

#[test]
fn the_file_menu_opens_the_settings_modal() {
    // Driven with real clicks, because this is the single wiring point that
    // makes the whole feature reachable. Every other settings test opens the
    // modal by setting the flag directly, so if the menu item were left
    // unwired — or the button removed and nothing put in its place — all of
    // them would still pass while the app had no way into its own settings.
    let screen = egui::vec2(1240.0, 900.0);

    let mut harness = Harness::new_ui_state(
        move |ui, app: &mut PhotoOrganizerApp| {
            let ctx = ui.ctx().clone();
            app.render_toolbar(&ctx);
            app.render_settings_modal(&ctx);
        },
        PhotoOrganizerApp::new(),
    );
    harness.set_size(screen);

    // Nothing should be open yet, and the toolbar must not still be offering
    // the output picker this PR relocated.
    harness.run();
    assert!(
        !harness.state().show_settings_modal,
        "the settings modal should start closed"
    );
    assert!(
        harness
            .query_all_by_label_contains("Output Folder")
            .next()
            .is_none(),
        "the output folder picker moved to the settings modal and must not \
         still be a toolbar button"
    );
    assert!(
        harness
            .query_all_by_label_contains("Settings")
            .next()
            .is_none(),
        "a closed menu renders no items, so Settings… should not be found yet"
    );

    // Open the menu, then take the item.
    harness
        .query_all_by_label_contains("File")
        .next()
        .expect("the File menu button should be in the toolbar")
        .click();
    harness.run();

    let item = harness
        .query_all_by_label_contains("Settings")
        .next()
        .expect("File menu should offer Settings…");
    item.click();
    harness.run();

    assert!(
        harness.state().show_settings_modal,
        "clicking File > Settings… must open the modal"
    );

    // And it must actually draw its card, not just flip the flag.
    let card = layout::settings_modal_size(screen);
    let card_rect = egui::Rect::from_center_size(screen.to_pos2() / 2.0, card);
    let inside = placed_widgets(&harness).into_iter().any(|r| {
        matches!(r.role.as_str(), "Label" | "Button" | "Slider" | "TextInput")
            && r.x0 >= f64::from(card_rect.min.x) - 0.5
            && r.x1 <= f64::from(card_rect.max.x) + 0.5
    });
    assert!(
        inside,
        "the opened modal should render its card, got nothing"
    );
}

#[test]
fn the_footer_and_the_settings_modal_agree_on_the_destination() {
    // The picker moved between the two, so a value set in one has to be the
    // value the other shows. They read the same `Settings`, but each derives
    // its display from that field independently — one shortens the path for a
    // narrow status bar, the other shows it in full — and that is exactly the
    // kind of duplication where they can drift apart.
    let destination = PathBuf::from("/home/tobye/photos/sorted");
    let screen = egui::vec2(1240.0, 900.0);
    let settings = crate::settings::Settings {
        output_folder: Some(destination.clone()),
        ..Default::default()
    };

    let mut harness = Harness::new(move |ctx| {
        let mut app = PhotoOrganizerApp::new();
        app.settings = settings.clone();
        app.render_footer(ctx);
        app.show_settings_modal = true;
        app.render_settings_modal(ctx);
    });
    harness.set_size(screen);
    harness.run();

    let text: Vec<String> = placed_widgets(&harness)
        .into_iter()
        .map(|r| r.label)
        .collect();

    let occurrences = text
        .iter()
        .filter(|t| t.contains(&destination.display().to_string()))
        .count();
    assert!(
        occurrences >= 2,
        "both the footer and the settings field should name the destination \
         {destination:?}, got {text:#?}"
    );
}

/// The toolbar with `destination` configured, already run one frame.
fn toolbar_with_destination(destination: Option<PathBuf>) -> Harness<'static, ()> {
    let mut harness = Harness::new(move |ctx| {
        let mut app = PhotoOrganizerApp::new();
        app.settings.output_folder = destination.clone();
        app.render_toolbar(ctx);
    });
    harness.set_size(egui::vec2(1240.0, 900.0));
    harness.run();
    harness
}

#[test]
fn move_and_copy_are_disabled_until_a_destination_is_set() {
    // The picker moved into the settings modal, so an unconfigured destination
    // is easy to reach by pressing Move. Greying the button is better than
    // letting it press and refuse — but the greying is only half the request:
    // a greyed button that says nothing is a dead end, so this asserts the
    // tooltip too.
    //
    // That tooltip is the non-obvious part. `on_hover_text` is silently
    // suppressed on a non-interactable widget, so the obvious spelling
    // produces a disabled button that explains nothing. Only
    // `on_disabled_hover_text` reaches it.
    let mut unconfigured = toolbar_with_destination(None);

    for label in ["Move", "Copy"] {
        let button = unconfigured
            .query_all_by_label_contains(label)
            .next()
            .unwrap_or_else(|| panic!("expected a {label} button"));

        assert!(
            button.is_disabled(),
            "{label} should be disabled with no output folder set"
        );

        button.hover();
        unconfigured.run();

        let tooltips: Vec<String> = placed_widgets(&unconfigured)
            .into_iter()
            .map(|r| r.label)
            .filter(|t| t.contains("output folder"))
            .collect();
        assert!(
            tooltips.iter().any(|t| t.contains("Settings")),
            "hovering a disabled {label} should point at the settings modal, \
             got {tooltips:#?}"
        );
    }
}

#[test]
fn move_and_copy_come_back_once_a_destination_is_set() {
    // The other half of the pair: a destination must actually re-enable them,
    // or the greying is a one-way trap. The hint also changes, since pointing
    // at Settings once it's configured would be noise.
    let mut configured = toolbar_with_destination(Some(PathBuf::from("/home/tobye/photos")));

    for label in ["Move", "Copy"] {
        let button = configured
            .query_all_by_label_contains(label)
            .next()
            .unwrap_or_else(|| panic!("expected a {label} button"));

        assert!(
            !button.is_disabled(),
            "{label} should be live once a destination is configured"
        );

        button.hover();
        configured.run();

        let tooltips: Vec<String> = placed_widgets(&configured)
            .into_iter()
            .map(|r| r.label)
            .filter(|t| t.contains("output folder"))
            .collect();
        assert!(
            tooltips.is_empty(),
            "a configured destination needs no pointer at Settings, got {tooltips:#?}"
        );
    }
}

#[test]
fn the_file_menu_does_not_duplicate_the_source_picker() {
    // The toolbar button is the route to scanning. A second route in the menu
    // is a second thing to keep in sync, and two controls that do the same
    // thing is worse than either one alone.
    let mut harness = toolbar_with_destination(Some(PathBuf::from("/home/tobye/photos")));

    harness
        .query_all_by_label_contains("File")
        .next()
        .expect("the File menu button should be in the toolbar")
        .click();
    harness.run();

    let items: Vec<String> = placed_widgets(&harness)
        .into_iter()
        .map(|r| r.label)
        .filter(|t| t.contains("Source Folder"))
        .collect();

    assert!(
        !items.iter().any(|t| t.contains('…')),
        "the menu should hold only Settings… and Quit, got {items:#?}"
    );
    assert!(
        items.iter().any(|t| t.contains("Source Folder")),
        "the toolbar button should still be there, got {items:#?}"
    );
}

/// A photo to stage, described without a `Context` so a test can list its
/// photos outside the harness closure — which is the only place one exists, and
/// a `StagedItem` cannot be built without one for its thumbnail.
struct Photo {
    name: &'static str,
    date: PhotoDate,
    category: &'static str,
}

fn photo(name: &'static str, year: u32, month: u32, category: &'static str) -> Photo {
    photo_on(name, PhotoDate::new(year, month, None), category)
}

/// A photo taken on a given day, for the calendar's day-level tests.
fn photo_on(name: &'static str, date: PhotoDate, category: &'static str) -> Photo {
    Photo {
        name,
        date,
        category,
    }
}

/// An app holding `photos`, drawn through the same panel sequence `update` uses.
///
/// The three panels are called one after another in the order `update` calls
/// them, rather than `update` itself: `eframe::Frame` is opaque and cannot be
/// built outside eframe, so a test cannot invoke the real entry point. The order
/// is the part that matters — egui hands the central panel whatever the top and
/// bottom panels leave it — and reproducing it here is what keeps a footer's
/// height and a toolbar's width honest against the grid between them.
///
/// `tune` runs every frame rather than once, so a filter set by a test survives
/// a later frame rewriting the classifications underneath it.
fn app_with(
    photos: Vec<Photo>,
    tune: fn(&mut PhotoOrganizerApp),
) -> Harness<'static, PhotoOrganizerApp> {
    app_with_timing(photos, tune, Timing::EveryFrame)
}

/// Whether an `app_with` setup is re-applied every frame or only once.
#[derive(Clone, Copy)]
enum Timing {
    EveryFrame,
    FirstFrameOnly,
}

/// As [`app_with`], but the setup runs once rather than on every frame.
///
/// `app_with` re-applies its setup each frame, which is what a test wants when it
/// is establishing a standing condition and cannot be defeated by the thing under
/// test. It is the wrong tool for anything a test then tries to *undo*: the frame
/// after the click that cleared the filter puts the filter straight back, and the
/// click becomes unobservable. Hence two entry points rather than one with a flag.
fn app_with_once(
    photos: Vec<Photo>,
    tune: impl FnMut(&mut PhotoOrganizerApp) + 'static,
) -> Harness<'static, PhotoOrganizerApp> {
    app_with_timing(photos, tune, Timing::FirstFrameOnly)
}

fn app_with_timing(
    photos: Vec<Photo>,
    mut tune: impl FnMut(&mut PhotoOrganizerApp) + 'static,
    timing: Timing,
) -> Harness<'static, PhotoOrganizerApp> {
    let mut applied = false;
    let mut harness = Harness::new_ui_state(
        move |ui, app: &mut PhotoOrganizerApp| {
            if app.items.is_empty() {
                app.items = photos
                    .iter()
                    .map(|p| StagedItem {
                        source_path: PathBuf::from(format!("/photos/{}.jpg", p.name)),
                        date: p.date,
                        category: p.category.to_string(),
                        selected: true,
                        ..staged_item(ui.ctx())
                    })
                    .collect();
            }
            match timing {
                Timing::EveryFrame => tune(app),
                Timing::FirstFrameOnly if applied => {}
                Timing::FirstFrameOnly => {
                    tune(app);
                    applied = true;
                }
            }

            let ctx = ui.ctx().clone();
            app.render_toolbar(&ctx);
            app.render_footer(&ctx);
            app.render_grid(&ctx);
        },
        PhotoOrganizerApp::new(),
    );
    harness.set_size(egui::vec2(1240.0, 900.0));
    harness
}

/// The filenames of the photos the grid drew, top to bottom then left to right.
///
/// Checkbox labels are the filenames, so this is the grid's own account of the
/// order it drew things in — geometry, not the sort function's return value.
fn drawn_photo_order(harness: &Harness<'_, PhotoOrganizerApp>) -> Vec<String> {
    let mut cells: Vec<(f64, f64, String)> = placed_widgets(harness)
        .into_iter()
        .filter(|r| r.role.contains("Check"))
        .map(|r| (r.y0, r.x0, r.label))
        .collect();
    cells.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    cells.into_iter().map(|(_, _, label)| label).collect()
}

/// Every label the frame laid out, for asserting on what the app said.
fn all_labels(harness: &Harness<'_, PhotoOrganizerApp>) -> Vec<String> {
    placed_widgets(harness)
        .into_iter()
        .map(|r| r.label)
        .collect()
}

#[test]
fn the_grid_draws_the_photos_in_the_sorted_order() {
    let mut harness = app_with(
        vec![
            photo("third", 2022, 3, "Travel"),
            photo("first", 2019, 7, "Travel"),
            photo("second", 2020, 11, "Travel"),
        ],
        |app| {
            app.sort_by = SortBy::DateTaken;
            app.sort_direction = SortDirection::Ascending;
        },
    );
    harness.run();

    assert_eq!(
        drawn_photo_order(&harness),
        vec!["first.jpg", "second.jpg", "third.jpg"],
        "oldest first"
    );
}

#[test]
fn reversing_the_sort_reverses_the_grid() {
    let mut harness = app_with(
        vec![
            photo("third", 2022, 3, "Travel"),
            photo("first", 2019, 7, "Travel"),
            photo("second", 2020, 11, "Travel"),
        ],
        |app| {
            app.sort_by = SortBy::DateTaken;
            app.sort_direction = SortDirection::Descending;
        },
    );
    harness.run();

    assert_eq!(
        drawn_photo_order(&harness),
        vec!["third.jpg", "second.jpg", "first.jpg"],
        "newest first"
    );
}

#[test]
fn the_grid_leaves_out_the_photos_a_filter_excludes() {
    let mut harness = app_with(
        vec![
            photo("receipt", 2021, 5, "Documents"),
            photo("beach", 2021, 5, "Beach Trip"),
            photo("old_receipt", 2018, 2, "Documents"),
        ],
        |app| {
            app.filters.category = Some("documents".to_string());
        },
    );
    harness.run();

    assert_eq!(
        drawn_photo_order(&harness),
        vec!["old_receipt.jpg", "receipt.jpg"],
        "only the filtered category, oldest first as the default sort has it"
    );
}

#[test]
fn the_footer_counts_the_photos_in_view_and_says_how_many_are_hidden() {
    // The number above the Move button has to describe the same set of photos the
    // grid is showing, or the user reads the count as covering the whole folder.
    let mut harness = app_with(
        vec![
            photo("receipt", 2021, 5, "Documents"),
            photo("beach", 2021, 5, "Beach Trip"),
            photo("beach2", 2021, 6, "Beach Trip"),
            photo("beach3", 2021, 7, "Beach Trip"),
        ],
        |app| {
            app.filters.category = Some("Beach Trip".to_string());
        },
    );
    harness.run();

    let text = all_labels(&harness);

    assert!(
        text.iter()
            .any(|t| t.contains("3 of 3 selected") && t.contains("4 staged")),
        "the footer should count the 3 visible and admit 4 are staged, got {text:#?}"
    );
}

#[test]
fn the_footer_says_so_when_a_filter_leaves_nothing_to_show() {
    // An empty grid with a selection count of zero reads as "nothing is selected"
    // rather than "your filter excluded everything", which sends the user hunting
    // for a tick they never unticked.
    let mut harness = app_with(
        vec![
            photo("receipt", 2021, 5, "Documents"),
            photo("beach", 2021, 5, "Beach Trip"),
        ],
        |app| {
            app.filters.category = Some("Documents".to_string());
            app.filters.date_from = Some(PhotoDate::new(2030, 1, None));
        },
    );
    harness.run();

    let text = all_labels(&harness);

    assert!(
        text.iter()
            .any(|t| t.contains("No photos match the filters")),
        "the footer should explain the empty grid, got {text:#?}"
    );
    assert!(
        text.iter()
            .any(|t| t.contains("None of the 2 staged photos")),
        "the grid should say what it excluded, got {text:#?}"
    );
}

#[test]
fn select_all_only_ticks_the_photos_in_view() {
    // The whole point of a filter: "All" means all of what is on screen. Ticking
    // the hidden ones too would leave the app one click from transferring photos
    // the user cannot see.
    let mut harness = app_with(
        vec![
            photo("receipt", 2021, 5, "Documents"),
            photo("beach", 2021, 5, "Beach Trip"),
            photo("beach2", 2021, 6, "Beach Trip"),
        ],
        |app| {
            for item in &mut app.items {
                item.selected = false;
            }
            app.filters.category = Some("Beach Trip".to_string());
        },
    );
    harness.run();

    harness.state_mut().set_all_selected(true);

    let ticked: Vec<String> = harness
        .state()
        .items
        .iter()
        .filter(|i| i.selected)
        .map(|i| i.source_path.display().to_string())
        .collect();

    assert_eq!(
        ticked,
        vec!["/photos/beach.jpg", "/photos/beach2.jpg"],
        "the filtered-out photo must stay unticked"
    );
}

#[test]
fn a_narrowed_grid_says_so_on_the_row_rather_than_behind_a_toggle() {
    // This used to be about a "Filters ●" button reporting a filter that had been
    // set and then hidden behind a collapsed panel — a state the user could not
    // otherwise see. The panel is gone: the filters sit on the sort row, so a
    // narrowed grid states the filter outright and offers the way to undo it.
    //
    // Set up with `app_with_once`, not `app_with`: the filter has to survive being
    // *cleared* below, and a setup re-applied every frame would put it straight
    // back and make the click unobservable.
    let mut harness = app_with_once(
        vec![
            photo("receipt", 2021, 5, "Documents"),
            photo("beach", 2021, 5, "Beach Trip"),
        ],
        |app| app.filters.category = Some("Documents".to_string()),
    );
    harness.run();

    let text = all_labels(&harness);

    assert!(
        text.iter().any(|t| t == "Documents"),
        "the active category should be readable on the row, got {text:#?}"
    );
    assert!(
        text.iter().any(|t| t.contains("1 of 2 shown")),
        "the sort row should report how much is showing, got {text:#?}"
    );
    assert!(
        text.iter().any(|t| t.contains("Clear")),
        "and offer a way to undo the narrowing, got {text:#?}"
    );

    // Undo it from the row, and the whole folder comes back.
    click_button(&mut harness, "Clear");
    assert_eq!(
        drawn_photo_order(&harness),
        vec!["receipt.jpg", "beach.jpg"],
        "clearing the filter should put the whole folder back"
    );
}

#[test]
fn a_transfer_moves_only_what_is_in_view() {
    // Checked against the filesystem rather than against the batch list: a
    // filtered-out photo must still be on disk *and* still on the grid. The grid
    // used to drop every ticked photo regardless of the filters, so a photo the
    // user could not see vanished from the view having never been transferred at
    // all — it was simply gone.
    let temp = std::env::temp_dir().join(format!("test_filtered_transfer_{}", std::process::id()));
    let _ = fs::remove_dir_all(&temp);
    let source = temp.join("source");
    let output = temp.join("output");
    fs::create_dir_all(&source).unwrap();
    fs::create_dir_all(&output).unwrap();

    for name in ["receipt", "beach"] {
        image::RgbImage::new(8, 8)
            .save(source.join(format!("{name}.jpg")))
            .unwrap();
    }

    // Cloned for the closure so the originals survive it for the assertions.
    let staged_from = source.clone();
    let staged_into = output.clone();

    let mut harness = Harness::new_ui_state(
        move |ctx, app: &mut PhotoOrganizerApp| {
            if app.items.is_empty() {
                app.settings.output_folder = Some(staged_into.clone());
                app.items = ["receipt", "beach"]
                    .into_iter()
                    .map(|name| StagedItem {
                        // The real file, so a transfer has something to act on.
                        source_path: staged_from.join(format!("{name}.jpg")),
                        date: PhotoDate::new(2021, 5, None),
                        category: if name == "receipt" {
                            "Documents".into()
                        } else {
                            "Beach Trip".into()
                        },
                        selected: true,
                        ..staged_item(ctx.ctx())
                    })
                    .collect();
            }
            app.filters.category = Some("Documents".to_string());

            app.execute_transfer(crate::transfer::TransferMode::Copy);
        },
        PhotoOrganizerApp::new(),
    );
    harness.set_size(egui::vec2(1240.0, 900.0));
    harness.run();

    // The photo in view was copied into the output folder...
    let copied = output.join("Documents").join("2021").join("05");
    assert!(
        copied.join("receipt.jpg").exists(),
        "the photo in view should have been transferred"
    );
    assert!(
        !copied.join("beach.jpg").exists(),
        "the filtered-out photo must not be transferred"
    );

    // ...and the filtered-out one is untouched in every respect: still on disk,
    // and still staged so the user can widen the filter and see it.
    assert!(
        source.join("beach.jpg").exists(),
        "a copy leaves the original alone either way"
    );
    let remaining: Vec<String> = harness
        .state()
        .items
        .iter()
        .map(|i| {
            i.source_path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(
        remaining,
        vec!["beach.jpg"],
        "only the transferred photo should leave the grid"
    );

    // Second phase, in this same test and not another: `last_execution_manifest`
    // is one process-global file at a fixed relative path, so a second test
    // calling `execute_transfer` would race this one over it and cargo runs tests
    // in parallel. What it costs is one extra `execute_transfer` call rather than
    // a whole duplicated fixture.
    //
    // The photo left on the grid is still ticked, but the filter excludes it —
    // so this press is exactly the "nothing to move" case a filter makes easy to
    // reach, and the manifest written a moment ago has to survive it. An empty
    // batch used to overwrite the last real manifest, leaving the transfer the
    // user actually cares about un-undoable; losing an undo is worse than a button
    // that did nothing.
    let manifest_after_first = fs::read_to_string("last_execution_manifest.json").unwrap();
    assert!(
        manifest_after_first.contains("receipt.jpg"),
        "the first transfer should have recorded itself, got {manifest_after_first}"
    );

    harness
        .state_mut()
        .execute_transfer(crate::transfer::TransferMode::Copy);

    assert!(
        harness
            .state()
            .status_message
            .as_ref()
            .is_some_and(|(msg, _)| msg.contains("Nothing to transfer")),
        "an empty transfer should say so, got {:?}",
        harness.state().status_message
    );
    assert_eq!(
        fs::read_to_string("last_execution_manifest.json").unwrap_or_default(),
        manifest_after_first,
        "the previous manifest must survive so its transfer stays undoable"
    );

    let _ = fs::remove_dir_all(&temp);
}

/// Focuses the custom-category editor showing `value`, the way a click would.
///
/// Found by matching the editor's *value*, since the editors are laid out in grid
/// order and the photos can be in any order under a sort.
fn focus_editor(harness: &mut Harness<'static, PhotoOrganizerApp>, value: &str) {
    harness
        .kittest_state()
        .query_all(by().recursive(true))
        .find(|node| {
            format!("{:?}", node.role()).contains("TextInput")
                && node.value().is_some_and(|v| v == value)
        })
        .unwrap_or_else(|| panic!("no category editor showing {value:?}"))
        .focus();
    harness.run();
}

/// One character into the focused editor, on its own frame.
///
/// One event per frame because that is how a real keyboard arrives, and because
/// batching would hide the bug this exists for: focus and the caret survive one
/// frame at a time or not at all.
fn type_text(harness: &mut Harness<'static, PhotoOrganizerApp>, text: &str) {
    harness
        .input_mut()
        .events
        .push(egui::Event::Text(text.to_string()));
    harness.run();
}

/// A grid of custom-category photos sorted by category, from `(name, category)`
/// pairs.
fn custom_sorted_grid(
    pairs: &[(&'static str, &'static str)],
) -> Harness<'static, PhotoOrganizerApp> {
    app_with(
        pairs
            .iter()
            .enumerate()
            .map(|(i, (name, category))| photo(name, 2021, i as u32 + 1, category))
            .collect(),
        |app| {
            if !app.items.iter().all(|item| item.is_custom) {
                for item in &mut app.items {
                    item.is_custom = true;
                }
            }
            app.sort_by = SortBy::Category;
        },
    )
}

#[test]
fn typing_a_category_keeps_the_row_and_the_caret_where_they_are() {
    // The bug this pins down. A Category sort reorders the grid on the very
    // keystroke that edits a category, and egui identifies a cell's widgets by the
    // position they occupy — so the field under the caret became the *next*
    // photo's field and the characters landed there, at that photo's old caret,
    // which is also what made the rows behind it look scrambled. Typing "A" into a
    // photo whose category was empty used to leave the intended photo holding "A"
    // and put "bA", then "bAA", into its neighbour.
    let mut harness = custom_sorted_grid(&[("aaa", "aaa"), ("bbb", "bbb"), ("zzz", "")]);

    harness.run();
    assert_eq!(
        drawn_photo_order(&harness),
        vec!["aaa.jpg", "bbb.jpg", "zzz.jpg"]
    );

    focus_editor(&mut harness, "");

    // "A" sorts before both "aaa" and "bbb", so without the hold this row leaps
    // from last to first on the first keystroke.
    for expected in ["A", "AA", "AAA"] {
        type_text(&mut harness, "A");
        assert_eq!(
            harness.state().items[2].category,
            expected,
            "the photo being edited should receive the character"
        );
        assert_eq!(
            harness.state().items[1].category,
            "bbb",
            "an unrelated photo must not be edited"
        );
        assert_eq!(
            drawn_photo_order(&harness),
            vec!["aaa.jpg", "bbb.jpg", "zzz.jpg"],
            "the grid must not move while a cell is being typed into"
        );
    }
}

#[test]
fn the_held_order_is_the_sorted_one_not_the_scan_one() {
    // The hold was implemented as "skip the sort", which substituted `items`
    // order — the scan's arrival order — for the sorted one. So the frame after
    // an editor took focus the whole grid reordered into directory-walk order and
    // stayed there for as long as the field had focus: the characters still landed
    // in the right photo, but the rows the user was reading moved anyway.
    //
    // `custom_sorted_grid` cannot see this, because it stages photos in the order
    // a Category sort produces, so its `items` order *is* its sorted order and the
    // two are indistinguishable. Staged deliberately against the sort here.
    let mut harness = app_with(
        vec![
            photo("alpha", 2021, 1, "mmm"),
            photo("beta", 2021, 2, "aaa"),
            photo("gamma", 2021, 3, "zzz"),
        ],
        |app| {
            for item in &mut app.items {
                item.is_custom = true;
            }
            app.sort_by = SortBy::Category;
            app.sort_direction = SortDirection::Ascending;
        },
    );

    harness.run();
    assert_eq!(
        drawn_photo_order(&harness),
        vec!["beta.jpg", "alpha.jpg", "gamma.jpg"],
        "the sorted order is not the order the photos were staged in"
    );

    focus_editor(&mut harness, "zzz");
    type_text(&mut harness, "!");

    assert_eq!(
        harness.state().items[2].category,
        "zzz!",
        "the photo being edited should receive the character"
    );
    assert_eq!(
        drawn_photo_order(&harness),
        vec!["beta.jpg", "alpha.jpg", "gamma.jpg"],
        "and the grid must stay in the sorted order while the editor holds focus"
    );
}

#[test]
fn the_grid_asserts_its_sort_once_the_editor_lets_go() {
    // Holding the order is a pause, not a cancellation: the rename has to take
    // effect once the user is done, or a category sort would quietly stop sorting.
    let mut harness = custom_sorted_grid(&[("aaa", "aaa"), ("bbb", "bbb"), ("zzz", "")]);

    harness.run();
    focus_editor(&mut harness, "");
    type_text(&mut harness, "A");
    assert_eq!(
        drawn_photo_order(&harness),
        vec!["aaa.jpg", "bbb.jpg", "zzz.jpg"]
    );

    // Take focus off the editor the way clicking away would.
    harness
        .kittest_state()
        .query_all(by().recursive(true))
        .find(|node| format!("{:?}", node.role()).contains("Button"))
        .expect("a button to take focus")
        .focus();
    harness.run();

    assert_eq!(
        drawn_photo_order(&harness),
        vec!["zzz.jpg", "aaa.jpg", "bbb.jpg"],
        "the sort should reassert itself once nothing is being edited"
    );
}

#[test]
fn changing_the_sort_while_typing_still_takes_effect() {
    // The hold is on the *order*, not on the sort. Reversing mid-edit must still
    // reverse the grid — clicking a button does not move keyboard focus in egui
    // 0.30, so an editor the user clicked into keeps the focus, and a plain
    // "editing, therefore frozen" rule would swallow this.
    let mut harness = custom_sorted_grid(&[("aaa", "aaa"), ("bbb", "bbb")]);

    harness.run();
    focus_editor(&mut harness, "aaa");
    assert!(
        harness.state().holding_order(),
        "an editor with focus should hold the order"
    );

    harness.state_mut().sort_direction = SortDirection::Descending;
    harness.run();

    assert!(
        !harness.state().holding_order(),
        "changing the sort releases the hold"
    );
    assert_eq!(
        drawn_photo_order(&harness),
        vec!["bbb.jpg", "aaa.jpg"],
        "the reversed sort should apply immediately"
    );
}

/// Rendered through the real toolbar and driven by a real click, so the calendar is
/// reached the way a user reaches it. There is no filter panel to open any more —
/// the filters are on the sort row and the calendar is a popup over the grid.
fn with_date_picker() -> Harness<'static, PhotoOrganizerApp> {
    let mut harness = app_with(
        vec![
            photo("jan", 2021, 1, "Beach Trip"),
            photo("mar", 2021, 3, "Beach Trip"),
            photo("jun", 2021, 6, "Beach Trip"),
        ],
        |_| {},
    );
    harness.run();
    click_button(&mut harness, "📅");
    harness
}

/// A widget's text, wherever egui put it.
///
/// `label().or_else(value())`, the same fallback `placed_widgets` uses: a plain
/// label lands in `value` and only an input's contents land in `label`, so
/// reading one field alone misses half the widgets on screen.
fn node_text(node: &Node) -> String {
    node.label().or_else(|| node.value()).unwrap_or_default()
}

/// Clicks the button whose text contains `text`.
///
/// A full `run` rather than a `step`: a click is a press *and* a release, and one
/// step can catch only half of it — the same trap the settings-modal tests
/// document.
fn click_button(harness: &mut Harness<'static, PhotoOrganizerApp>, text: &str) {
    harness
        .kittest_state()
        .query_all(by().recursive(true))
        .find(|node| {
            format!("{:?}", node.role()).contains("Button") && node_text(node).contains(text)
        })
        .unwrap_or_else(|| panic!("no button labelled {text:?}"))
        .click();
    harness.run();
}

/// Clicks a day cell in the calendar.
///
/// Matched exactly rather than by substring, because the day labels run 1..=31
/// against a toolbar full of other numbers: a substring search for "1" finds the
/// confidence slider's "1.00" first and drags it instead of picking a day.
fn click_day(harness: &mut Harness<'static, PhotoOrganizerApp>, day: u32) {
    let wanted = day.to_string();
    harness
        .kittest_state()
        .query_all(by().recursive(true))
        .find(|node| format!("{:?}", node.role()).contains("Button") && node_text(node) == wanted)
        .unwrap_or_else(|| panic!("no day cell labelled {day}"))
        .click();
    harness.run();
}

/// Whether a widget of the given role is on screen with that text.
fn find_label(harness: &Harness<'static, PhotoOrganizerApp>, role_part: &str, text: &str) -> bool {
    harness
        .kittest_state()
        .query_all(by().recursive(true))
        .any(|node| {
            format!("{:?}", node.role()).contains(role_part) && node_text(&node).contains(text)
        })
}

/// A node's top edge, by exact label.
fn top_of(harness: &Harness<'static, PhotoOrganizerApp>, text: &str) -> f64 {
    harness
        .kittest_state()
        .query_all(by().recursive(true))
        .find_map(|n| {
            let l = node_text(&n);
            (l == text).then(|| n.raw_bounds().unwrap().y0)
        })
        .unwrap_or_else(|| panic!("no label {text:?}"))
}

#[test]
fn the_calendar_picks_a_range_of_days() {
    // Driven with real clicks through the real toolbar, because the parts that
    // matter are all wiring: the button that opens the picker, the day cells it
    // draws, and the filter the click lands in.
    let mut harness = with_date_picker();
    assert!(
        find_label(&harness, "Label", "Any"),
        "the date row should read as unbounded to begin with"
    );

    // June 2021, where `newest_date` leaves the picker for this folder.
    assert!(
        find_label(&harness, "Label", "June") && find_label(&harness, "Label", "2021"),
        "the picker should open on the newest month in the folder"
    );

    // The 7th, then the 9th: a three-day range.
    click_day(&mut harness, 7);
    click_day(&mut harness, 9);

    assert_eq!(
        harness.state().filters.date_from,
        Some(PhotoDate::new(2021, 6, Some(7))),
        "the first click should open the range"
    );
    assert_eq!(
        harness.state().filters.date_to,
        Some(PhotoDate::new(2021, 6, Some(9))),
        "the second click should close it"
    );
    assert!(
        find_label(&harness, "Label", "2021-06-07 → 2021-06-09"),
        "the filter row should spell out the range it now holds"
    );
}

#[test]
fn one_click_on_a_day_filters_to_that_day_alone() {
    // The end-to-end claim behind the rework. One click used to set a start with no
    // end, so it filtered to every photo from that day onwards — while the grid
    // drew it as a bar running to the end of the month, so the picker looked like
    // it had understood and the contents disagreed.
    let mut harness = app_with(
        vec![
            photo_on("early", PhotoDate::new(2021, 6, Some(7)), "Beach Trip"),
            photo_on("middle", PhotoDate::new(2021, 6, Some(9)), "Beach Trip"),
            photo_on("late", PhotoDate::new(2021, 6, Some(20)), "Beach Trip"),
        ],
        |_| {},
    );
    harness.run();
    click_button(&mut harness, "📅");

    // One click, on the 9th.
    click_day(&mut harness, 9);

    assert_eq!(
        drawn_photo_order(&harness),
        vec!["middle.jpg"],
        "a single click should show that day alone, not everything after it"
    );

    // And it reads as one date rather than as a range one day wide.
    let text = all_labels(&harness);
    assert!(
        text.iter().any(|t| t == "2021-06-09"),
        "the row should show a single date, got {text:#?}"
    );
    assert!(
        !text.iter().any(|t| t.contains('→')),
        "a one-day selection is not a range, got {text:#?}"
    );

    // A second click turns it into a range, without discarding the first day.
    click_day(&mut harness, 20);
    assert_eq!(
        drawn_photo_order(&harness),
        vec!["middle.jpg", "late.jpg"],
        "widening should keep the day that was already selected"
    );

    // A third starts over on that day — "not that, this instead". Without it the
    // range would silently absorb every later click and a fortnight could become a
    // year with no way back.
    click_day(&mut harness, 7);
    assert_eq!(
        drawn_photo_order(&harness),
        vec!["early.jpg"],
        "a third click should start over on the day it was given"
    );
}

#[test]
fn picking_backwards_through_the_calendar_still_gives_a_usable_range() {
    // Clicking the dates in the order a person thinks of them — the 20th, then
    // the 10th — has to leave a range. Storing it inverted reads as empty and
    // silently filters the whole folder away.
    let mut harness = with_date_picker();
    click_day(&mut harness, 20);
    click_day(&mut harness, 10);

    let (from, to) = (
        harness.state().filters.date_from.unwrap(),
        harness.state().filters.date_to.unwrap(),
    );
    assert!(
        from.start() <= to.end(),
        "the range should come out ordered, got {from:?}..{to:?}"
    );
}

#[test]
fn the_calendar_only_keeps_the_photos_it_names() {
    // The end-to-end claim: a date filter drawn from the calendar narrows the grid,
    // and everything scoped to the view follows from it.
    let mut harness = with_date_picker();
    click_day(&mut harness, 1); // 1 June
    click_day(&mut harness, 30); // 30 June

    harness.run();
    assert_eq!(
        drawn_photo_order(&harness),
        vec!["jun.jpg"],
        "only June should be left in view"
    );
    assert!(
        find_label(&harness, "Label", "1 of 1 selected") || find_label(&harness, "Label", "1 of 1"),
        "the footer should count the visible photo, not the staged three"
    );
}

#[test]
fn paging_the_calendar_reaches_other_months_and_years() {
    // The arrows are the only way to reach a date the folder does not contain, so
    // a folder of one month still has to be filterable by any month.
    let mut harness = with_date_picker();
    assert!(find_label(&harness, "Label", "June"));

    click_button(&mut harness, "‹");
    assert!(find_label(&harness, "Label", "May"), "back one month");

    click_button(&mut harness, "›");
    click_button(&mut harness, "›");
    assert!(find_label(&harness, "Label", "July"), "forward two months");

    click_button(&mut harness, "»");
    assert!(
        find_label(&harness, "Label", "July") && find_label(&harness, "Label", "2022"),
        "forward a year"
    );

    click_button(&mut harness, "«");
    assert!(
        find_label(&harness, "Label", "July") && find_label(&harness, "Label", "2021"),
        "back a year"
    );
}

#[test]
fn the_calendar_days_line_up_in_one_set_of_columns() {
    // The weekday initials and the days are rows of a *single* grid. They used to
    // be two grids, and nothing made their columns agree — the pitch turned out to
    // be `DAY_SIZE + spacing + a further 14pt` that neither grid controlled, so
    // every header sat to the right of the day it named. One grid cannot drift.
    //
    // Checked structurally: every column's left edge is the same distance from the
    // first column's, for both rows.
    let mut harness = Harness::new_ui_state(
        |ui, _: &mut ()| {
            let (mut from, mut to) = (None, None);
            // 1 June 2021 was a Tuesday, so "1" leads column 1 and column 0 is a
            // spacer.
            let mut cal = super::calendar::Calendar::new((2021, 6));
            cal.show(ui, &mut from, &mut to, PhotoDate::new(2021, 6, Some(11)));
        },
        (),
    );
    harness.set_size(egui::vec2(1240.0, 900.0));
    harness.run();

    let left_of = |label: &str| {
        harness
            .kittest_state()
            .query_all(by().recursive(true))
            .find(|node| node.label().or_else(|| node.value()).as_deref() == Some(label))
            .and_then(|node| node.raw_bounds())
            .map(|b| b.x0)
    };

    let mut pitches = Vec::new();
    for (a, b) in [("2", "3"), ("3", "4"), ("9", "10")] {
        let (a, b) = (left_of(a).unwrap(), left_of(b).unwrap());
        pitches.push(b - a);
    }
    assert!(
        pitches.windows(2).all(|w| (w[0] - w[1]).abs() < 0.01),
        "columns should be evenly pitched, got {pitches:?}"
    );
}

#[test]
fn the_calendar_has_no_gap_between_its_dates() {
    // `egui::Grid` forces every cell to `spacing.interact_size`, 40pt wide by
    // default. A day smaller than that therefore sat in a wider column with the
    // difference reading as a gap beside it — which was never the grid's spacing,
    // and is why zeroing the spacing did nothing. The grid asks for its own
    // minimum instead, so consecutive days are exactly one cell apart.
    let mut harness = Harness::new_ui_state(
        |ui, _: &mut ()| {
            let (mut from, mut to) = (None, None);
            let mut cal = super::calendar::Calendar::new((2021, 6));
            cal.show(ui, &mut from, &mut to, PhotoDate::new(2021, 6, Some(11)));
        },
        (),
    );
    harness.set_size(egui::vec2(1240.0, 900.0));
    harness.run();

    let left_of = |label: &str| {
        harness
            .kittest_state()
            .query_all(by().recursive(true))
            .find(|node| {
                format!("{:?}", node.role()).contains("Button")
                    && node.label().or_else(|| node.value()).as_deref() == Some(label)
            })
            .and_then(|node| node.raw_bounds())
            .map(|b| b.x0)
            .unwrap_or_else(|| panic!("no day cell {label}"))
    };

    let day = super::calendar::DAY_SIZE as f64;
    assert_eq!(
        left_of("3") - left_of("2"),
        day,
        "adjacent days should be exactly one cell apart"
    );
}

#[test]
fn the_heading_is_centred_over_the_grid_and_fits_inside_it() {
    // Three bugs in one row, all of which look like styling and are not.
    //
    // A bare `ui.horizontal` greedily fills the panel, so in a 400pt filter panel
    // the arrows spread to the panel edges and `«`/`»` land *outside* it, clipped.
    // Stacking the month and the year into a `vertical_centered` puts the title on
    // two lines and leaves `‹` and `›` at different heights. And a plain `label`
    // left-aligns inside whatever slot it is given, which is not the same as
    // centring it.

    let heading = |month: u32| {
        let mut harness = Harness::new_ui_state(
            |ui, _: &mut ()| {
                let (mut from, mut to) = (None, None);
                let mut cal = super::calendar::Calendar::new((2026, month));
                cal.show(
                    ui,
                    &mut from,
                    &mut to,
                    PhotoDate::new(2026, month, Some(11)),
                );
            },
            (),
        );
        // Wider than seven 38pt days on purpose: that is the panel this is drawn
        // in, and the width the arrows used to escape.
        harness.set_size(egui::vec2(400.0, 700.0));
        harness.run();

        let bounds = |label: &str| {
            harness
                .kittest_state()
                .query_all(by().recursive(true))
                .find_map(|node| {
                    let l = node.label().or_else(|| node.value())?;
                    (l == label && format!("{:?}", node.role()).contains("Button"))
                        .then(|| node.raw_bounds().unwrap())
                })
                .unwrap_or_else(|| panic!("no heading button {label}"))
        };

        // The grid the heading is meant to sit over, read from a day cell.
        let grid = harness
            .kittest_state()
            .query_all(by().recursive(true))
            .filter_map(|n| {
                let l = n.label().or_else(|| n.value())?;
                (l.parse::<u32>().is_ok()).then(|| n.raw_bounds().unwrap())
            })
            .fold(None::<(f64, f64)>, |acc: Option<(f64, f64)>, b| {
                Some(match acc {
                    None => (b.x0, b.x1),
                    Some((lo, hi)) => (lo.min(b.x0), hi.max(b.x1)),
                })
            })
            .expect("no day cells");
        let title = harness
            .kittest_state()
            .query_all(by().recursive(true))
            .find_map(|n| {
                let l = n.label().or_else(|| n.value())?;
                (l.ends_with("2026")).then(|| n.raw_bounds().unwrap())
            })
            .expect("no month title");

        (
            bounds("«"),
            bounds("»"),
            title,
            grid,
            (grid.0 + grid.1) / 2.0,
        )
    };

    for month in [1u32, 9, 10, 12] {
        let (prev_year, next_year, title, grid, grid_centre) = heading(month);
        let title_centre = (title.x0 + title.x1) / 2.0;

        assert!(
            title_centre - grid_centre < 2.0 && grid_centre - title_centre < 2.0,
            "month {month}: title centred at {title_centre} against a grid centre of {grid_centre}"
        );
        // The long month names are the ones that would break it: a wider title
        // widens the slot it is in unless the arrows reserve their own.
        assert!(
            title.x1 < next_year.x0,
            "month {}: title (ends {:?}) overlaps the next-year arrow (starts {:?})",
            month,
            title.x1,
            next_year.x0
        );
        assert!(
            next_year.x1 <= grid.1 + 2.0,
            "month {}: the year arrows (end {:?}) escape the grid (ends {:?})",
            month,
            next_year.x1,
            grid.1
        );
        assert!(prev_year.x0 >= grid.0 - 2.0, "month {month}: escapes left");
    }
}

#[test]
fn the_sort_and_the_filters_share_one_line() {
    // The filter row used to live below the sort row, behind a toggle. Both facts
    // were wrong on their own: opening it pushed the whole grid down a line, and a
    // filter set and then hidden was a filter you could not see.
    let mut harness = app_with(
        vec![
            photo("receipt", 2021, 5, "Documents"),
            photo("beach", 2021, 5, "Beach Trip"),
        ],
        |_| {},
    );
    harness.run();

    assert_eq!(
        top_of(&harness, "Sort"),
        top_of(&harness, "Filter"),
        "Sort and Filter should be on the same line, not stacked"
    );
}

#[test]
fn opening_the_calendar_does_not_move_the_grid() {
    // The whole reason the picker is a popup rather than a row in the toolbar: a
    // 266pt month grid inline shoved everything below it down, so picking a date
    // moved the photos the date was about.
    let mut harness = app_with(
        vec![
            photo("jan", 2021, 1, "Beach Trip"),
            photo("mar", 2021, 3, "Beach Trip"),
        ],
        |_| {},
    );
    harness.run();

    let photos_on_screen = |harness: &Harness<'_, PhotoOrganizerApp>| {
        harness
            .kittest_state()
            .query_all(by().recursive(true))
            .filter_map(|n| {
                let l = node_text(&n);
                l.ends_with(".jpg").then(|| (l, n.raw_bounds().unwrap().y0))
            })
            .collect::<Vec<_>>()
    };
    let before = photos_on_screen(&harness);
    assert!(!before.is_empty(), "the grid should have photos in it");

    click_button(&mut harness, "📅");

    assert_eq!(
        before,
        photos_on_screen(&harness),
        "opening the calendar must not move the grid a single point"
    );
}
