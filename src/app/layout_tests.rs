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

/// A staged photo with `selected` set, for the bulk move's selection handling.
fn staged_item_selected(ctx: &egui::Context, selected: bool, year: u32, month: u32) -> StagedItem {
    StagedItem {
        year,
        month,
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
    bulk_move_harness_with(screen, settings, name, staged)
}

/// [`bulk_move_harness`] with real source paths, for the tests that move files.
fn bulk_move_harness_with(
    screen: egui::Vec2,
    settings: crate::settings::Settings,
    name: &str,
    staged: Vec<(PathBuf, u32, u32)>,
) -> Harness<'static, PhotoOrganizerApp> {
    let mut app = PhotoOrganizerApp::new();
    app.settings = settings;
    app.show_bulk_move_modal = true;
    app.bulk_move_name = name.to_string();

    // Staged exactly once, and deliberately not on `items.is_empty()`: a move
    // empties the grid, so that condition would put the photos straight back and
    // hide whether they were dropped at all. `Cell` because the harness closure
    // is `Fn` and re-staging has to be observable from inside it.
    let staged_once = std::cell::Cell::new(false);
    let mut harness = Harness::new_ui_state(
        move |ui, app: &mut PhotoOrganizerApp| {
            if !staged_once.get() {
                staged_once.set(true);
                app.items = staged
                    .iter()
                    .map(|(path, year, month)| StagedItem {
                        source_path: path.clone(),
                        year: *year,
                        month: *month,
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
}

#[test]
fn bulk_copy_leaves_the_originals_and_keeps_the_photos_staged() {
    // The same batch as a copy: the files are duplicated under the typed name
    // and the grid keeps them, so the selection can be reviewed or re-filed.
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
    assert_eq!(harness.state().items.len(), 1, "copied photos stay staged");
}

#[test]
fn a_name_typed_with_separators_cannot_create_a_directory_level() {
    // The traversal guard, at the point the feature makes it reachable. Filing
    // is free text over a whole selection here, so a name typed as `A/B` would
    // otherwise add a level and `..` would leave the output folder. This leans
    // on `CategoryName` doing its one job; the point of the assertion is that
    // the bulk move routes through it rather than joining the raw string.
    let scratch = Scratch::new("name_is_one_component");
    let photo = scratch.write("incoming/a.jpg", "bytes");

    let screen = egui::vec2(1240.0, 900.0);
    let settings = crate::settings::Settings {
        output_folder: Some(scratch.join("sorted")),
        ..Default::default()
    };
    let mut harness =
        bulk_move_harness_with(screen, settings, "../../escaped", vec![(photo, 2024, 7)]);

    harness
        .query_all_by_label_contains("Move Selected")
        .next()
        .expect("expected a Move Selected button")
        .click();
    harness.run();

    assert!(
        !scratch.exists("escaped"),
        "a traversal attempt must not resolve outside the output folder"
    );
    let filed = scratch.exists("sorted/Unsorted/2024/07/a.jpg")
        || scratch.exists("sorted/..-..-escaped/2024/07/a.jpg");
    assert!(
        filed,
        "the photo still has to be filed somewhere, under the sanitised name"
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
