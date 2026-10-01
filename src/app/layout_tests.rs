//! Layout assertions driven through `egui_kittest`.
//!
//! These render the real `render_category_selector` and
//! `render_custom_category_input` into a headless egui context and assert on
//! the geometry egui actually computes. That matters because the original
//! column-overflow bug was invisible to source-text checks: the code looked
//! right, but egui placed the widgets on one line and pushed the row wider
//! than the grid column.

use super::layout::MIN_CONTROL_WIDTH;
use super::models::StagedItem;
use super::PhotoOrganizerApp;
use crate::profile_store::{ClassificationSource, ProfileStore};
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
        category: "Beach Trip".into(),
        confidence: 0.5,
        source: ClassificationSource::Manual,
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
fn placed_widgets(harness: &Harness<'_, Option<StagedItem>>) -> Vec<WidgetRect> {
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
