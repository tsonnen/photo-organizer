//! The bottom status bar: how much of the grid is selected, and where a
//! transfer will put it.
//!
//! This is where the output folder lives now. It was a toolbar button, but a
//! destination is a piece of standing context rather than an action, and the
//! toolbar was already carrying more buttons than it could hold in one row —
//! the picker moving to the settings modal left the user with no way to see
//! where their photos were about to go.
//!
//! Every count here is over the photos currently in view, not over everything
//! staged, so that the number above the Move button describes the same set of
//! photos the grid is showing and the transfer is about to act on.

use super::PhotoOrganizerApp;
use eframe::egui;

/// The part of a path worth showing when there isn't room for all of it.
///
/// Keeps the tail: `/home/tobye/photos` and `~/Downloads/archive` are told
/// apart by their last segment, not their first, so a long path is elided at
/// front rather than back.
fn shorten_path(path: &std::path::Path) -> String {
    const MAX_CHARS: usize = 48;

    let full = path.display().to_string();
    if full.chars().count() <= MAX_CHARS {
        return full;
    }

    // Drop leading components until what is left fits, prefixing an ellipsis so
    // a shortened path is never mistaken for the whole one.
    let components: Vec<String> = path
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect();

    let mut tail: Vec<&str> = Vec::new();
    let mut length = 1; // the ellipsis
    for component in components.iter().rev() {
        // Counted in chars, not bytes, to match the check above: the budget is
        // about how wide the label is, and a component of multi-byte characters
        // would overshoot it on every count of its length.
        let component_chars = component.chars().count();
        if length + component_chars + 1 > MAX_CHARS {
            break;
        }
        length += component_chars + 1;
        tail.push(component.as_str());
    }
    // Collected back-to-front, so put it back the right way round.
    tail.reverse();

    if tail.is_empty() {
        // A single component longer than the budget: elide the middle rather
        // than show nothing at all.
        let tail_of: String = full
            .chars()
            .skip(full.chars().count().saturating_sub(MAX_CHARS - 1))
            .collect();
        return format!("…{tail_of}");
    }

    format!("…{}", tail.join(std::path::MAIN_SEPARATOR_STR))
}

impl PhotoOrganizerApp {
    /// The bottom panel: selection count on the left, transfer destination on
    /// the right.
    ///
    /// Rendered before the grid, because egui hands `CentralPanel` whatever the
    /// top and bottom panels leave it.
    pub(super) fn render_footer(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("footer").show(ctx, |ui| {
            ui.horizontal(|ui| {
                // Counted over what the grid is showing, not over everything
                // staged. A footer reading "3 of 400 selected" under a grid of
                // eight photos is a statement about photos the user cannot see,
                // and it invites them to read the transfer that follows as
                // covering all 400 — which, since the transfer is scoped the
                // same way, it does not.
                let visible = self.visible_indices();
                let staged = self.items.len();
                let selected = visible.iter().filter(|&&i| self.items[i].selected).count();

                ui.label(match (staged, visible.len()) {
                    (0, _) => "No photos staged".to_string(),
                    // Photos staged, none in view: the filters are the whole
                    // story, so say that rather than reporting a selection count
                    // of zero over an empty grid as though nothing were wrong.
                    (_, 0) => format!("No photos match the filters · {staged} staged"),
                    (_, shown) if shown < staged => {
                        format!("{selected} of {shown} selected · {staged} staged")
                    }
                    (_, shown) => format!("{selected} of {shown} selected"),
                });

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    match self.settings.output_folder.as_deref() {
                        Some(path) => {
                            let label = shorten_path(path);
                            ui.label(
                                egui::RichText::new(format!("📂 {label}"))
                                    .color(egui::Color32::LIGHT_GRAY),
                            )
                            .on_hover_text(path.display().to_string());
                        }
                        None => {
                            // Said plainly rather than left blank: with the
                            // picker moved into the settings modal, an
                            // unconfigured destination is one click from
                            // invisible. The toolbar greys Move and Copy for
                            // the same reason, but a greyed button explains
                            // itself only to whoever hovers it — this is the
                            // statement of it that's always on screen.
                            ui.colored_label(
                                egui::Color32::from_rgb(240, 180, 0),
                                "⚠ No output folder set — choose one in Settings",
                            );
                        }
                    }
                });
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::shorten_path;
    use std::path::Path;

    #[test]
    fn a_short_path_is_shown_whole() {
        let path = Path::new("/home/tobye/photos");
        assert_eq!(shorten_path(path), "/home/tobye/photos");
    }

    #[test]
    fn a_long_path_keeps_the_end_that_identifies_it() {
        // The tail is what tells two photo folders apart, so elision happens at
        // the front and the last segment always survives.
        let path = Path::new("/home/tobye/Pictures/2026/family holiday/raw scans");
        let shortened = shorten_path(path);

        assert!(
            shortened.ends_with("raw scans"),
            "the last segment must survive, got {shortened:?}"
        );
        assert!(
            shortened.starts_with('…'),
            "elision is marked, got {shortened:?}"
        );
    }

    #[test]
    fn shortening_never_exceeds_its_budget() {
        let path = Path::new("/a/very/long/path/with/many/components/that/keeps/going/and/going");
        let shortened = shorten_path(path);

        assert!(
            shortened.chars().count() <= 48,
            "shortened path is {} chars: {shortened:?}",
            shortened.chars().count()
        );
    }

    #[test]
    fn a_single_oversized_component_still_shows_something() {
        // A path whose last segment alone is over the budget must not collapse
        // to an empty string, which would leave the footer showing a bare
        // ellipsis.
        let long = format!("/{}/model.safetensors", "x".repeat(120));
        let shortened = shorten_path(Path::new(&long));

        assert!(shortened.len() > 1, "got {shortened:?}");
        assert!(
            shortened.contains("model.safetensors"),
            "the filename is the identifying part, got {shortened:?}"
        );
    }

    #[test]
    fn a_multi_byte_path_keeps_every_segment_that_fits() {
        // The budget is about how wide the label is, so it counts characters.
        // Counting bytes instead would run out after a third of them and elide
        // segments that had room to spare: five 9-character segments are 45
        // characters of label but 95 bytes.
        let path = Path::new("/æøåæøåæøå/æøåæøåæøå/æøåæøåæøå/æøåæøåæøå/æøåæøåæøå");

        assert_eq!(
            shorten_path(path),
            "…æøåæøåæøå/æøåæøåæøå/æøåæøåæøå/æøåæøåæøå"
        );
    }
}
