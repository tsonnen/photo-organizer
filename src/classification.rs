//! What a photo is ([`PhotoFacts`]) and what was decided about it
//! ([`Classification`]).
//!
//! These were one pile of loosely typed fields, assembled at three call sites
//! that disagreed about what `width` and `height` meant: the scanner passed the
//! photo's true frame size, while a cached rescan and **Re-classify All** passed
//! the ≤200x140 thumbnail's. The screenshot rule needs `width >= 800`, so a
//! 1920x1080 PNG was **Screenshots** on the scan that read the file and
//! **Unsorted** on every scan after.
//!
//! A decision is made in one place, [`ProfileStore::classify`], and written to a
//! staged photo in one place, [`StagedItem::apply_classification`].

use crate::category_name::CategoryName;
use std::path::PathBuf;

/// What a photo is called before the pipeline has decided: a label, not a
/// category, so nothing may be edited into it or filed under it.
pub const CLASSIFYING_LABEL: &str = "Classifying...";

/// Which tier produced a category, which is also what the grid's badge reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassificationSource {
    /// A trained centroid matched, above the user's confidence threshold.
    VisualModel,
    /// A filename, resolution or EXIF rule matched.
    Heuristic,
    /// The user picked it. Never replaced by a later classification.
    Manual,
    /// Nothing claimed the photo.
    UnsortedFallback,
}

impl std::fmt::Display for ClassificationSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::VisualModel => write!(f, "AI"),
            Self::Heuristic => write!(f, "Rule"),
            Self::Manual => write!(f, "Manual"),
            Self::UnsortedFallback => write!(f, "None"),
        }
    }
}

/// A photo's true pixel dimensions: a fact about the file, not about whatever size
/// happened to be decoded for the grid. Cached next to the thumbnail so every
/// classification of a photo sees the same numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameSize {
    pub width: u32,
    pub height: u32,
}

impl FrameSize {
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    /// Width over height. A frame with no rows reads as square rather than
    /// infinite, since the ratio rules look for screen shapes.
    pub fn aspect_ratio(self) -> f32 {
        if self.height == 0 {
            1.0
        } else {
            self.width as f32 / self.height as f32
        }
    }
}

/// The year and month a photo is filed under: from EXIF when it carries a date,
/// from the filesystem otherwise (`media::extract_date`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhotoDate {
    pub year: u32,
    pub month: u32,
}

impl PhotoDate {
    pub const fn new(year: u32, month: u32) -> Self {
        Self { year, month }
    }
}

/// Everything the classifier is allowed to read about a photo. One argument
/// rather than five, so a caller cannot assemble a photo's facts from the wrong
/// pieces.
#[derive(Debug, Clone, PartialEq)]
pub struct PhotoFacts {
    pub path: PathBuf,
    pub date: PhotoDate,
    /// `None` only for a photo cached by a build that did not record its
    /// dimensions. The rule tier skips its resolution rules in that case rather
    /// than substituting the thumbnail.
    pub frame: Option<FrameSize>,
    pub is_exif: bool,
    /// Empty when the CLIP model is missing, or while a scan is still working on
    /// this photo. Both mean "no visual tier", not "no embedding".
    pub embedding: Vec<f32>,
}

/// What the pipeline decided about a photo.
#[derive(Debug, Clone, PartialEq)]
pub enum Classification {
    /// Staged and on screen, but no category has been decided yet.
    ///
    /// This used to be the string `"Classifying..."` stored as a real category,
    /// which read as a custom name: the grid rendered a text box bound to it, and
    /// one keystroke marked the photo manual, so the real decision arriving
    /// moments later was skipped and the photo stayed filed under
    /// "Classifying..." for good.
    Pending,
    /// A category was decided.
    Decided(Decision),
}

/// A decided classification.
///
/// Deliberately carries no "is this name custom" flag. That question is not about
/// the photo, it is about whether a *trained profile* exists whose name matches,
/// and the store a scan decides against is a clone taken when the scan started —
/// minutes before a profile may have been trained. So it is asked at the one place
/// that writes the name onto a staged photo, against the store in hand. See
/// [`crate::app::models::StagedItem::apply_classification`].
#[derive(Debug, Clone, PartialEq)]
pub struct Decision {
    pub category: CategoryName,
    pub confidence: f32,
    pub source: ClassificationSource,
}

impl Classification {
    /// The decision, or `None` while pending. Readers that only need to tell the
    /// two states apart use this rather than the variant.
    pub fn decided(&self) -> Option<&Decision> {
        match self {
            Self::Pending => None,
            Self::Decided(decision) => Some(decision),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pending_classification_has_no_decision() {
        assert_eq!(Classification::Pending.decided(), None);

        let decided = Classification::Decided(Decision {
            category: CategoryName::from_user_input("Sunsets"),
            confidence: 0.9,
            source: ClassificationSource::VisualModel,
        });
        assert_eq!(
            decided.decided().map(|d| d.source),
            Some(ClassificationSource::VisualModel)
        );
    }

    #[test]
    fn aspect_ratio_of_a_real_frame() {
        assert!((FrameSize::new(1920, 1080).aspect_ratio() - 1.777_777_8).abs() < 1e-6);
        assert!((FrameSize::new(140, 200).aspect_ratio() - 0.7).abs() < 1e-6);
        // A frame with no rows is not an infinitely wide screen grab.
        assert_eq!(FrameSize::new(1920, 0).aspect_ratio(), 1.0);
    }

    #[test]
    fn an_unrecorded_frame_size_cannot_satisfy_the_resolution_rule() {
        // What a cache row written before the dimensions were persisted looks
        // like. `frame` is an `Option` so the rules can decline to guess rather
        // than reading a size off the thumbnail.
        let facts = PhotoFacts {
            path: PathBuf::from("/photos/plain.png"),
            date: PhotoDate::new(2026, 3),
            frame: None,
            is_exif: false,
            embedding: Vec::new(),
        };
        assert_eq!(facts.frame, None);
        assert!(!facts.frame.is_some_and(|frame| frame.width >= 800));
    }
}
