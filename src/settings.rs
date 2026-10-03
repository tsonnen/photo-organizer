//! User-configurable app settings, persisted to `settings.json`.
//!
//! Kept apart from [`crate::profile_store`], which holds trained *categories*:
//! these are the knobs that decide how the app behaves rather than what it has
//! learned. That split also means a `profiles.json` written by an older build
//! never carries a stale copy of a value this module owns.

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
}

fn default_confidence_threshold() -> f32 {
    DEFAULT_CONFIDENCE_THRESHOLD
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            confidence_threshold: DEFAULT_CONFIDENCE_THRESHOLD,
            output_folder: None,
            model_path: None,
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
        settings
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
    }

    #[test]
    fn round_trips_through_a_file() {
        let path = temp_path("roundtrip");

        let settings = Settings {
            confidence_threshold: 0.72,
            output_folder: Some(PathBuf::from("/photos/sorted")),
            model_path: Some(PathBuf::from("/models/clip_vision.safetensors")),
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
