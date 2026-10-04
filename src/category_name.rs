//! A category name that is safe to use as a single directory component.
//!
//! A category is a dropdown label *and* a directory under the output folder, and
//! only the second constrains what the string may contain. `CategoryName` is the
//! one place that decides which names are allowed; see
//! [`CategoryName::from_user_input`] for the rules.

use serde::{Deserialize, Serialize};
use std::fmt;

/// A category name, guaranteed to be usable as exactly one path component.
///
/// Serialises as a plain string, so the JSON on disk is unchanged.
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

    /// Sanitises user input into a name safe to use as one directory component.
    ///
    /// Separators become `-` rather than being dropped, so `Vacation / Japan`
    /// still reads as `Vacation - Japan` and the user can see what happened to
    /// their input. A name with nothing usable left falls back to
    /// [`CategoryName::unsorted`].
    pub fn from_user_input(raw: &str) -> Self {
        let replaced: String = raw
            .chars()
            .map(|c| match c {
                '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
                // Control characters, NUL included, are invalid in a path on
                // every platform.
                c if c.is_control() => ' ',
                c => c,
            })
            .collect();

        // Windows silently strips trailing dots and spaces when creating a
        // file, which would make `Sunsets.` and `Sunsets` collide on disk.
        let trimmed = replaced.trim().trim_end_matches(['.', ' ']);

        // `.` and `..` are the current and parent directory.
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
        // Separators become dashes rather than being dropped, so a name of
        // nothing but separators leaves one harmless component.
        assert_eq!(CategoryName::from_user_input("///").as_str(), "---");
    }

    #[test]
    fn test_trailing_dots_and_spaces_are_trimmed() {
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
        // `#[serde(transparent)]` has to stay, or this becomes `{"0": "Sunsets"}`
        // and every `profiles.json` on disk changes shape.
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
