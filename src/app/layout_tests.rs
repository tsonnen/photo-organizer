//! Layout assertions driven through `egui_kittest`.
//!
//! These render the real `render_category_selector`,
//! `render_custom_category_input` and `render_profile_list` into a headless egui
//! context and assert on the geometry egui actually computes. That matters
//! because the original column-overflow bug was invisible to source-text
//! checks: the code looked right, but egui placed the widgets on one line and
//! pushed the row wider than the grid column.

use super::layout::{self, MIN_CONTROL_WIDTH};
use super::models::StagedItem;
use super::profiles_modal::DeletePrompt;
use super::PhotoOrganizerApp;
use crate::classification::{ClassificationSource, FrameSize};
use crate::profile_store::{CategoryProfile, ProfileStore};
use eframe::egui;
use egui_kittest::kittest::{by, Queryable};
use egui_kittest::Harness;
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
        year: 2024,
        month: 5,
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
                PhotoOrganizerApp::render_grid_cell_controls(
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
