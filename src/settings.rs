//! User-configurable app settings, persisted to `settings.json`.
//!
//! Kept apart from [`crate::profile_store`], which holds trained *categories*:
//! these are the knobs that decide how the app behaves rather than what it has
//! learned. That split also means a `profiles.json` written by an older build
//! never carries a stale copy of a value this module owns.

use crate::category_name::CategoryName;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// Similarity a centroid match must reach before it is believed over the rules.
///
/// The default for [`Settings::confidence_threshold`]. Bounded away from 0.0
/// deliberately: at a threshold of zero every photo matches its nearest
/// centroid, which makes the rule tier — and with it screenshot, document and
/// EXIF detection — unreachable for anyone with a trained profile.
pub const DEFAULT_CONFIDENCE_THRESHOLD: f32 = 0.65;

/// Narrowest threshold the slider offers. Below this a centroid match is
/// believed too readily, and the rules rarely get a turn.
pub const MIN_CONFIDENCE_THRESHOLD: f32 = 0.30;

/// Widest threshold the slider offers. At 1.0 only a pixel-identical match
/// could clear the bar, so the cap keeps a usable range at both ends.
pub const MAX_CONFIDENCE_THRESHOLD: f32 = 0.95;

/// What the user has configured, independent of what the app has learned.
///
/// Every field defaults, so a `settings.json` written by a build that predates
/// a field still loads rather than falling back to the whole-default struct and
/// silently discarding the user's other choices.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    /// Cosine similarity a centroid match must reach to beat the rules.
    #[serde(default = "default_confidence_threshold")]
    pub confidence_threshold: f32,

    /// Where transfers file approved photos. `None` until the user picks one.
    #[serde(default)]
    pub output_folder: Option<PathBuf>,

    /// An explicit CLIP checkpoint. `None` means auto-detect: fall back to the
    /// standard locations [`crate::inference::find_model_path`] searches.
    #[serde(default)]
    pub model_path: Option<PathBuf>,

    /// Custom category names the user has used, newest first, so the bulk move
    /// can offer them again.
    ///
    /// Not profiles, and deliberately not in `profiles.json`: a name here carries
    /// no centroid and is never matched against, it is only a label the user
    /// chose not to retype. That is the whole point of the bulk move — filing an
    /// event's photos without training a category for it — so storing these as
    /// exemplars would be the profile churn the feature exists to avoid.
    ///
    /// Kept to [`MAX_REMEMBERED_CATEGORIES`] and sanitised by
    /// [`CategoryName`] before it is stored, so what the dropdown offers is exactly
    /// what will be used as a folder name. [`Self::remember_custom_category`] is the
    /// only writer, and it keeps the fallback bucket out of the list.
    #[serde(default)]
    pub custom_categories: Vec<String>,
}

/// How many custom names the bulk move remembers.
///
/// A dropdown is only useful while it can be scanned, and the names are offered
/// in the order they were used, so the most recent ones are the ones worth
/// keeping. Past this the oldest go: an unbounded list would grow into a menu
/// nobody reads through, and a name that old can be retyped.
pub const MAX_REMEMBERED_CATEGORIES: usize = 24;

fn default_confidence_threshold() -> f32 {
    DEFAULT_CONFIDENCE_THRESHOLD
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            confidence_threshold: DEFAULT_CONFIDENCE_THRESHOLD,
            output_folder: None,
            model_path: None,
            custom_categories: Vec::new(),
        }
    }
}

impl Settings {
    /// Reads settings from `path`, falling back to the defaults when the file
    /// is missing or unreadable.
    ///
    /// A corrupt or partial file is not worth refusing to start over: the
    /// settings are all recoverable from the UI, and the trained profiles the
    /// app cares about live in a different file entirely.
    pub fn load_from_file<P: AsRef<Path>>(path: P) -> Self {
        let Ok(content) = fs::read_to_string(path) else {
            return Self::default();
        };
        Self::from_json(&content)
    }

    /// Parses settings from JSON text, clamping anything out of range.
    ///
    /// The clamp is what makes a hand-edited file safe: the threshold is the
    /// one field with a meaningful range, and serde will happily load `99.0`
    /// or a negative number, either of which would classify every photo as
    /// Unsorted.
    pub fn from_json(content: &str) -> Self {
        let mut settings: Self = serde_json::from_str(content).unwrap_or_default();
        settings.confidence_threshold = Self::clamp_threshold(settings.confidence_threshold);
        settings.normalize_custom_categories();
        settings
    }

    /// Puts the remembered category names into the shape the dropdown relies
    /// on: trimmed, free of duplicates that differ only in case, and no longer
    /// than [`MAX_REMEMBERED_CATEGORIES`].
    ///
    /// Runs on load for the same reason the threshold is clamped on load: the
    /// file is editable by hand, and a blank entry would show a dropdown row
    /// that files nothing. Newest-first order is preserved, and a dropped
    /// duplicate takes with it the entry that was already there, so a name the
    /// user retyped under different casing keeps one row in the position it
    /// already had.
    fn normalize_custom_categories(&mut self) {
        let mut kept: Vec<String> = Vec::with_capacity(self.custom_categories.len());
        for name in self.custom_categories.drain(..) {
            let name = name.trim().to_string();
            let already_kept = kept.iter().any(|k| k.eq_ignore_ascii_case(&name));
            if name.is_empty() || already_kept {
                continue;
            }
            kept.push(name);
            if kept.len() == MAX_REMEMBERED_CATEGORIES {
                break;
            }
        }
        self.custom_categories = kept;
    }

    /// Remembers a custom category name, moving it to the front if it was
    /// already known.
    ///
    /// A no-op for an empty name, and case-insensitive about the duplicate: a
    /// name the user has merely retyped with different casing is the same name,
    /// and offering both would have them choose between two spellings of one
    /// folder. Newest-first is what makes the dropdown worth reading at all.
    ///
    /// The fallback bucket is never remembered, and the guard is here rather than
    /// at the caller because the invariant is about what the dropdown may offer:
    /// [`CategoryName::unsorted`] is a directory every unmatched photo already
    /// lands in, not a name anyone chose, so listing it would put a row in the
    /// recall list that files photos nowhere the user did not ask for.
    pub fn remember_custom_category(&mut self, name: &str) {
        let name = name.trim();
        if name.is_empty() || name.eq_ignore_ascii_case(CategoryName::unsorted().as_str()) {
            return;
        }
        self.custom_categories
            .retain(|n| !n.eq_ignore_ascii_case(name));
        self.custom_categories.insert(0, name.to_string());
        self.custom_categories.truncate(MAX_REMEMBERED_CATEGORIES);
    }

    /// Forces a threshold into the range the slider offers.
    pub fn clamp_threshold(value: f32) -> f32 {
        if value.is_nan() {
            return DEFAULT_CONFIDENCE_THRESHOLD;
        }
        value.clamp(MIN_CONFIDENCE_THRESHOLD, MAX_CONFIDENCE_THRESHOLD)
    }

    /// Serialises to JSON, clamping the threshold on the way out so a value
    /// set programmatically cannot escape the range either.
    pub fn to_json(&self) -> String {
        let mut settings = self.clone();
        settings.confidence_threshold = Self::clamp_threshold(settings.confidence_threshold);
        serde_json::to_string_pretty(&settings).unwrap_or_default()
    }

    /// Persists to `path`. Errors are the caller's to surface: settings are
    /// re-enterable, and a read-only folder should not stop the app running.
    pub fn save_to_file<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        if let Some(parent) = path.as_ref().parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }
        fs::write(path, self.to_json())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A settings file path unique to this process, matching the convention the
    /// rest of the test suite uses for temp files.
    fn temp_path(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("test_settings_{tag}_{}.json", std::process::id()))
    }

    #[test]
    fn defaults_are_usable_with_no_file() {
        let path = temp_path("defaults");
        let _ = fs::remove_file(&path);

        let loaded = Settings::load_from_file(&path);
        assert_eq!(loaded, Settings::default());
        assert!((loaded.confidence_threshold - 0.65).abs() < 1e-6);
        assert_eq!(loaded.output_folder, None);
        assert_eq!(loaded.model_path, None);
        assert!(loaded.custom_categories.is_empty());
    }

    #[test]
    fn round_trips_through_a_file() {
        let path = temp_path("roundtrip");

        let settings = Settings {
            confidence_threshold: 0.72,
            output_folder: Some(PathBuf::from("/photos/sorted")),
            model_path: Some(PathBuf::from("/models/clip_vision.safetensors")),
            custom_categories: vec!["Beach Trip".to_string()],
        };
        settings.save_to_file(&path).expect("save settings");

        assert_eq!(Settings::load_from_file(&path), settings);

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn a_file_missing_new_fields_still_loads() {
        // A `settings.json` written before a field existed must keep the other
        // values the user chose, rather than failing the whole parse and
        // resetting everything to defaults.
        let settings = Settings::from_json(r#"{ "confidence_threshold": 0.80 }"#);

        assert!((settings.confidence_threshold - 0.80).abs() < 1e-6);
        assert_eq!(settings.output_folder, None);
        assert_eq!(settings.model_path, None);
        // The remembered names are a list that grows during use, so a file
        // written before the first bulk move has to load without the field.
        assert!(settings.custom_categories.is_empty());
    }

    #[test]
    fn remembering_a_name_puts_it_at_the_front_and_takes_no_duplicates() {
        // The point of the list is that the name is offered again rather than
        // retyped, so it has to be findable: newest first, and one row per name
        // however the user cased it.
        let mut settings = Settings::default();
        settings.remember_custom_category("Beach Trip");
        settings.remember_custom_category("Ski 2024");
        settings.remember_custom_category("Beach Trip");

        assert_eq!(
            settings.custom_categories,
            vec!["Beach Trip".to_string(), "Ski 2024".to_string()]
        );

        settings.remember_custom_category("beach trip");
        assert_eq!(
            settings.custom_categories,
            vec!["beach trip".to_string(), "Ski 2024".to_string()],
            "a re-typed spelling is the same name and moves to the front"
        );
    }

    #[test]
    fn remembering_trims_and_ignores_an_empty_name() {
        let mut settings = Settings::default();
        settings.remember_custom_category("   ");
        assert!(settings.custom_categories.is_empty());

        settings.remember_custom_category("  Beach Trip  ");
        assert_eq!(settings.custom_categories, vec!["Beach Trip"]);
    }

    #[test]
    fn the_fallback_bucket_is_never_remembered() {
        // A name the sanitiser cannot use as a directory becomes `Unsorted`, and
        // that is a directory every unmatched photo already lands in rather than
        // a name anyone chose. Offering it back would put a row in the dropdown
        // that files photos where they were going anyway, while looking like a
        // remembered choice.
        let mut settings = Settings::default();
        for fallback in ["Unsorted", "unsorted", " UNSORTED "] {
            settings.remember_custom_category(fallback);
        }
        assert!(
            settings.custom_categories.is_empty(),
            "the fallback bucket is not a name the user chose, got {:?}",
            settings.custom_categories
        );

        // An ordinary name is still remembered, so the guard is not over-broad.
        settings.remember_custom_category("Beach Trip");
        assert_eq!(settings.custom_categories, vec!["Beach Trip"]);
    }

    #[test]
    fn the_remembered_list_stops_growing_and_keeps_the_newest() {
        // A dropdown is only worth reading while it can be scanned, so it is
        // capped rather than left to grow one event at a time.
        let over = MAX_REMEMBERED_CATEGORIES + 8;
        let mut settings = Settings::default();
        for i in 0..over {
            settings.remember_custom_category(&format!("Event {i:02}"));
        }

        assert_eq!(settings.custom_categories.len(), MAX_REMEMBERED_CATEGORIES);
        assert_eq!(
            settings.custom_categories[0],
            format!("Event {:02}", over - 1),
            "the most recently used name is the first one offered"
        );
        assert_eq!(
            settings.custom_categories.last().unwrap(),
            &format!("Event {:02}", over - MAX_REMEMBERED_CATEGORIES),
            "the oldest names are the ones dropped"
        );
    }

    #[test]
    fn a_hand_edited_name_list_is_normalized_on_load() {
        // Same reasoning as the threshold clamp: the file is editable by hand,
        // and a blank or duplicated entry would put a row in the dropdown that
        // files nothing, or the same name twice.
        let settings = Settings::from_json(
            r#"{
                "confidence_threshold": 0.65,
                "custom_categories": ["Beach Trip", "  ", "beach trip", " Ski 2024 ", "Ski 2024"]
            }"#,
        );

        assert_eq!(
            settings.custom_categories,
            vec!["Beach Trip".to_string(), "Ski 2024".to_string()]
        );
    }

    #[test]
    fn an_oversized_name_list_is_trimmed_to_the_cap_on_load() {
        let names: Vec<String> = (0..MAX_REMEMBERED_CATEGORIES + 5)
            .map(|i| format!("\"Event {i:02}\""))
            .collect();
        let settings = Settings::from_json(&format!(
            r#"{{ "custom_categories": [{}] }}"#,
            names.join(", ")
        ));

        assert_eq!(settings.custom_categories.len(), MAX_REMEMBERED_CATEGORIES);
        assert_eq!(settings.custom_categories[0], "Event 00");
    }

    #[test]
    fn an_out_of_range_threshold_is_clamped_on_load() {
        // The threshold is the one field with a meaningful range, and a
        // hand-edited value outside it would classify every photo as Unsorted.
        assert!(
            (Settings::from_json(r#"{"confidence_threshold": 99.0}"#).confidence_threshold
                - MAX_CONFIDENCE_THRESHOLD)
                .abs()
                < 1e-6
        );
        assert!(
            (Settings::from_json(r#"{"confidence_threshold": -5.0}"#).confidence_threshold
                - MIN_CONFIDENCE_THRESHOLD)
                .abs()
                < 1e-6
        );
    }

    #[test]
    fn a_corrupt_file_falls_back_to_defaults() {
        // Settings are all recoverable from the UI, so refusing to start over a
        // bad file would cost the user more than starting fresh does.
        let settings = Settings::from_json("{ not json at all");

        assert!((settings.confidence_threshold - DEFAULT_CONFIDENCE_THRESHOLD).abs() < 1e-6);
        assert_eq!(settings.output_folder, None);
    }

    #[test]
    fn a_nan_threshold_falls_back_to_the_default() {
        // `f32::clamp` propagates NaN, so an explicit guard is needed: a NaN
        // threshold would make every comparison false and silently classify
        // nothing.
        let nan = f32::NAN;
        assert_eq!(Settings::clamp_threshold(nan), DEFAULT_CONFIDENCE_THRESHOLD);
    }

    #[test]
    fn clamping_leaves_in_range_values_alone() {
        assert_eq!(Settings::clamp_threshold(0.65), 0.65);
        assert_eq!(
            Settings::clamp_threshold(MIN_CONFIDENCE_THRESHOLD),
            MIN_CONFIDENCE_THRESHOLD
        );
        assert_eq!(
            Settings::clamp_threshold(MAX_CONFIDENCE_THRESHOLD),
            MAX_CONFIDENCE_THRESHOLD
        );
    }

    #[test]
    fn serialising_clamps_too() {
        // A value set programmatically must not escape the range on its way to
        // disk any more than a hand-edited one does on the way in.
        let settings = Settings {
            confidence_threshold: 42.0,
            ..Default::default()
        };

        let reloaded = Settings::from_json(&settings.to_json());
        assert!((reloaded.confidence_threshold - MAX_CONFIDENCE_THRESHOLD).abs() < 1e-6);
    }
}
