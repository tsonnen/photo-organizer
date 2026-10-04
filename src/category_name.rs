//! A category name that is safe to use as a single directory component.
//!
//! A category is three things at once: the label in the grid's dropdown, the
//! value a [`Decision`](crate::classification::Decision) carries, and a directory
//! under the output folder. Only the third constrains what the string may
//! contain, and nothing in the type system said so — the transfer engine joined a
//! display string straight into a path, so a category typed as `A/B` silently
//! created an extra directory level and one typed as `..` escaped the output
//! folder entirely.
//!
//! This module is the one place that decides which names are allowed. The name
//! it returns is always a single component, so a caller joining it into a path
//! gets one directory and not two.

use serde::{Deserialize, Serialize};
use std::fmt;

/// A category name, guaranteed to be usable as exactly one path component.
///
/// Serialises as a plain string, so `Decision` and `RankedProfile` keep the
/// JSON shape they have always had on disk.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CategoryName(String);

impl CategoryName {
    /// Where a photo lands when nothing else matched.
    pub fn unsorted() -> Self {
        Self("Unsorted".to_string())
    }

    /// The rule-tier category for screenshots.
    pub fn screenshots() -> Self {
        Self("Screenshots".to_string())
    }

    /// The rule-tier category for documents and receipts.
    pub fn documents() -> Self {
        Self("Documents".to_string())
    }

    /// The rule-tier category for photos carrying EXIF camera metadata.
    pub fn camera_photos() -> Self {
        Self("Camera Photos".to_string())
    }

    /// Sanitises user input into a name that is safe to use as one directory
    /// component.
    ///
    /// Everything the user can reach — the training box, the per-photo custom
    /// name input, a hand-edited `profiles.json` — goes through here, so there
    /// is no path by which an unsanitised name reaches the filesystem.
    ///
    /// Separators become `-` rather than being dropped, so `Vacation / Japan`
    /// still reads as `Vacation - Japan` and the user can tell what happened to
    /// their input. A name with nothing usable left falls back to
    /// [`CategoryName::unsorted`].
    pub fn from_user_input(raw: &str) -> Self {
        let replaced: String = raw
            .chars()
            .map(|c| match c {
                // Path separators. This is the case that actually bites: the
                // transfer engine joins the category into `<Category>/<YYYY>/`,
                // so an embedded separator silently adds directory levels.
                '/' | '\\' => '-',
                // Characters Windows rejects outright. The release workflow
                // ships a Windows binary, so a category named `what?` would fail
                // every transfer rather than merely look odd.
                ':' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
                // Control characters, NUL included, are invalid in a path on
                // every platform.
                c if c.is_control() => ' ',
                c => c,
            })
            .collect();

        // Windows silently strips trailing dots and spaces when creating a
        // file, which would make `Sunsets.` and `Sunsets` collide on disk.
        let trimmed = replaced.trim().trim_end_matches(['.', ' ']);

        // `.` and `..` are the current and parent directory. Rejecting them is
        // what stops a category from resolving outside the output folder.
        if trimmed.is_empty() || trimmed == "." || trimmed == ".." {
            return Self::unsorted();
        }

        Self(trimmed.to_string())
    }

    /// The name as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consumes the name, yielding the string behind it.
    pub fn into_string(self) -> String {
        self.0
    }
}

impl fmt::Display for CategoryName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_separators_do_not_add_directory_levels() {
        // The defect this type exists to prevent: `A/B` used to become two
        // directories instead of one.
        assert_eq!(CategoryName::from_user_input("A/B").as_str(), "A-B");
        assert_eq!(CategoryName::from_user_input("A\\B").as_str(), "A-B");

        let name = CategoryName::from_user_input("Vacation / Japan 2024");
        assert_eq!(name.as_str(), "Vacation - Japan 2024");
        assert_eq!(
            Path::new(name.as_str()).components().count(),
            1,
            "a category name must stay a single path component"
        );
    }

    #[test]
    fn test_dot_names_cannot_escape_the_output_folder() {
        assert_eq!(CategoryName::from_user_input(".").as_str(), "Unsorted");
        assert_eq!(CategoryName::from_user_input("..").as_str(), "Unsorted");
        // A name that merely starts with dots is still an ordinary directory.
        assert_eq!(
            CategoryName::from_user_input("..hidden").as_str(),
            "..hidden"
        );
    }

    #[test]
    fn test_empty_and_whitespace_fall_back_to_unsorted() {
        assert_eq!(CategoryName::from_user_input("").as_str(), "Unsorted");
        assert_eq!(CategoryName::from_user_input("   ").as_str(), "Unsorted");
        // Every separator becomes a dash, so a name of nothing but separators
        // still leaves a single, harmless component behind rather than being
        // silently emptied.
        assert_eq!(CategoryName::from_user_input("///").as_str(), "---");
    }

    #[test]
    fn test_trailing_dots_and_spaces_are_trimmed() {
        // Windows drops these when creating a file, so leaving them would make
        // two different categories collide on disk.
        assert_eq!(
            CategoryName::from_user_input("Sunsets.").as_str(),
            "Sunsets"
        );
        assert_eq!(
            CategoryName::from_user_input("Sunsets  ").as_str(),
            "Sunsets"
        );
        assert_eq!(
            CategoryName::from_user_input("  Sunsets.  ").as_str(),
            "Sunsets"
        );
    }

    #[test]
    fn test_windows_reserved_characters_are_replaced() {
        assert_eq!(CategoryName::from_user_input("what?").as_str(), "what-");
        assert_eq!(CategoryName::from_user_input("a:b*c").as_str(), "a-b-c");
        assert_eq!(
            CategoryName::from_user_input("say\"hi\"").as_str(),
            "say-hi-"
        );
    }

    #[test]
    fn test_control_characters_are_dropped() {
        assert_eq!(
            CategoryName::from_user_input("Suns\0ets").as_str(),
            "Suns ets",
            "NUL is invalid in a path on every platform"
        );
    }

    #[test]
    fn test_ordinary_names_pass_through_unchanged() {
        for name in ["Sunsets", "Camera Photos", "Beach Trip 2024"] {
            assert_eq!(CategoryName::from_user_input(name).as_str(), name);
        }
    }

    #[test]
    fn test_display_and_serde_round_trip_as_a_plain_string() {
        let name = CategoryName::from_user_input("Sunsets");
        assert_eq!(name.to_string(), "Sunsets");
        // A decided category is written into the grid's state and read back in
        // tests, so it has to keep serialising as the bare string it always was
        // rather than as `{"0": "Sunsets"}`.
        assert_eq!(serde_json::to_string(&name).unwrap(), "\"Sunsets\"");
        let back: CategoryName = serde_json::from_str("\"Sunsets\"").unwrap();
        assert_eq!(back, name);
    }

    #[test]
    fn test_builtin_names_are_all_single_components() {
        for name in [
            CategoryName::unsorted(),
            CategoryName::screenshots(),
            CategoryName::documents(),
            CategoryName::camera_photos(),
        ] {
            assert_eq!(Path::new(name.as_str()).components().count(), 1);
            assert!(!name.as_str().is_empty());
        }
    }
}
