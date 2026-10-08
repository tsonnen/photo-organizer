//! Layout assertions driven through `egui_kittest`.
//!
//! These render the real `render_category_selector`,
//! `render_custom_category_input` and `render_profile_list` into a headless egui
//! context and assert on the geometry egui actually computes. That matters
//! because the original column-overflow bug was invisible to source-text
//! checks: the code looked right, but egui placed the widgets on one line and
//! pushed the row wider than the grid column.

use super::layout::{self, MIN_CONTROL_WIDTH};
use super::models::{SortBy, SortDirection, StagedItem};
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
        date: PhotoDate::new(2024, 5, Some(5)),
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
                name: (*name).to_string(),
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
            name: name.clone(),
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

/// A photo dated to the 1st of its month, which is enough to order a grid by
/// month and leaves the day free for the tests that care about it.
fn photo(name: &'static str, year: u32, month: u32, category: &'static str) -> Photo {
    photo_on(name, PhotoDate::new(year, month, Some(1)), category)
}

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
            tune(app);

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
            app.filters.date_from = Some(PhotoDate::new(2030, 1, Some(1)));
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
fn the_filters_button_says_when_something_is_narrowing_the_grid() {
    // A closed panel whose filter is active is the case that has to read as
    // active: the user cleared a filter from inside it, the panel closed, and the
    // grid is still not showing everything.
    let mut harness = app_with(
        vec![
            photo("receipt", 2021, 5, "Documents"),
            photo("beach", 2021, 5, "Beach Trip"),
        ],
        |app| {
            app.filters.category = Some("Documents".to_string());
        },
    );
    harness.run();

    let text = all_labels(&harness);

    assert!(
        text.iter()
            .any(|t| t.contains("Filters") && t.contains('●')),
        "the filters button should report an active filter, got {text:#?}"
    );
    assert!(
        text.iter().any(|t| t.contains("1 of 2 shown")),
        "the sort row should report how much is showing, got {text:#?}"
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
                        date: PhotoDate::new(2021, 5, Some(12)),
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

/// The filter panel open, with the date picker showing.
///
/// Rendered through the real toolbar, so the calendar is reached the way a user
/// reaches it: press **Filters**, then press the calendar button.
fn with_filter_panel() -> Harness<'static, PhotoOrganizerApp> {
    let mut harness = app_with(
        vec![
            photo("jan", 2021, 1, "Beach Trip"),
            photo("mar", 2021, 3, "Beach Trip"),
            photo("jun", 2021, 6, "Beach Trip"),
        ],
        |app| {
            app.show_filter_panel = true;
        },
    );
    harness.run();
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

#[test]
fn the_calendar_picks_a_range_of_days() {
    // Driven with real clicks through the real toolbar, because the parts that
    // matter are all wiring: the button that opens the picker, the day cells it
    // draws, and the filter the click lands in.
    let mut harness = with_filter_panel();
    assert!(
        find_label(&harness, "Label", "Any"),
        "the date row should read as unbounded to begin with"
    );

    click_button(&mut harness, "📅");
    // June 2021, where `newest_date` leaves the picker for this folder.
    assert!(
        find_label(&harness, "Label", "06/2021"),
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
fn picking_backwards_through_the_calendar_still_gives_a_usable_range() {
    // Clicking the dates in the order a person thinks of them — the 20th, then
    // the 10th — has to leave a range. Storing it inverted reads as empty and
    // silently filters the whole folder away.
    let mut harness = with_filter_panel();
    click_button(&mut harness, "📅");
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
    let mut harness = with_filter_panel();
    click_button(&mut harness, "📅");
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
    let mut harness = with_filter_panel();
    click_button(&mut harness, "📅");
    assert!(find_label(&harness, "Label", "06/2021"));

    click_button(&mut harness, "◀");
    assert!(find_label(&harness, "Label", "05/2021"), "back one month");

    click_button(&mut harness, "▶");
    click_button(&mut harness, "▶");
    assert!(
        find_label(&harness, "Label", "07/2021"),
        "forward two months"
    );

    click_button(&mut harness, "»");
    assert!(find_label(&harness, "Label", "07/2022"), "forward a year");

    click_button(&mut harness, "«");
    assert!(find_label(&harness, "Label", "07/2021"), "back a year");
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
    use egui_kittest::Harness;
    let mut harness = Harness::new_ui_state(
        |ui, _: &mut ()| {
            let mut from = None;
            let mut to = None;
            // 1 June 2021 was a Tuesday, so "1" leads column 1 and column 0 is a
            // spacer.
            let mut cal = super::calendar::Calendar::new((2021, 6));
            cal.show(ui, &mut from, &mut to);
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
    // default. A 26pt date therefore sat in a 40pt column with a 14pt gap beside
    // it that was never the grid's spacing — which is why setting `DAY_SPACING`
    // to zero did nothing. The grid asks for its own minimum instead.
    //
    // Asserted as the calendar's whole width: seven columns and no gaps is exactly
    // seven cells.
    use egui_kittest::Harness;
    let mut harness = Harness::new_ui_state(
        |ui, width: &mut Option<f32>| {
            let mut from = None;
            let mut to = None;
            let mut cal = super::calendar::Calendar::new((2021, 6));
            cal.show(ui, &mut from, &mut to);
            *width = Some(ui.min_rect().width());
        },
        None,
    );
    harness.set_size(egui::vec2(1240.0, 900.0));
    harness.run();

    let day = super::calendar::DAY_SIZE;
    assert_eq!(
        *harness.state(),
        Some(day * 7.0),
        "seven columns with no gap between them should be exactly seven cells wide"
    );
}
