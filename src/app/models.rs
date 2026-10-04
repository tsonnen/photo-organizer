//! Plain data the UI renders, plus the one method that writes a classification
//! onto a photo.
//!
//! The records here flow between the scanner, the profile store and the widgets.
//! The write method is behaviour, and lives here because this is the record it
//! writes to: it is the only place a photo's category, confidence, source and
//! custom flag are ever changed by the pipeline, which is what keeps a scan's
//! `Update` and **Re-classify All** from disagreeing about what a manual pick
//! means.

use crate::classification::{
    Classification, ClassificationSource, FrameSize, PhotoDate, PhotoFacts, CLASSIFYING_LABEL,
};
use crate::profile_store::ProfileStore;
use eframe::egui;
use std::path::PathBuf;

/// One scanned photo staged for review, with its classification and thumbnail.
pub struct StagedItem {
    pub source_path: PathBuf,
    pub year: u32,
    pub month: u32,
    pub is_exif: bool,
    /// The photo's true frame size, carried from the scan that read the file.
    ///
    /// The grid only ever decodes a ≤200x140 thumbnail, and the rules key off
    /// resolution, so this is the field that stopped a re-classification being
    /// fed the thumbnail's dimensions instead.
    pub frame: Option<FrameSize>,
    pub category: String,
    pub confidence: f32,
    pub source: ClassificationSource,
    /// True while the photo is staged but not yet classified.
    ///
    /// Nothing preserves a pending photo: its category is a placeholder rather
    /// than a decision, so whatever the scan decides next replaces it.
    pub pending: bool,
    pub embedding: Vec<f32>,
    pub texture: egui::TextureHandle,
    pub selected: bool,
    pub is_custom: bool,
}

impl StagedItem {
    /// Stages a photo the scan has just delivered, selected and ready to review.
    pub(super) fn new(
        profiles: &ProfileStore,
        facts: PhotoFacts,
        classification: Classification,
        texture: egui::TextureHandle,
    ) -> Self {
        let mut item = Self {
            source_path: facts.path,
            year: facts.date.year,
            month: facts.date.month,
            is_exif: facts.is_exif,
            frame: facts.frame,
            category: String::new(),
            confidence: 0.0,
            source: ClassificationSource::UnsortedFallback,
            pending: false,
            embedding: facts.embedding,
            texture,
            selected: true,
            is_custom: false,
        };
        item.apply_decision(profiles, classification);
        item
    }

    /// Writes a classification onto this photo.
    ///
    /// The single write path a classification takes, shared by a scan's
    /// `Update` and by **Re-classify All**. It enforces three rules:
    ///
    /// - A **pending** photo is never preserved. Its category is a placeholder
    ///   shown while the model works, so the decision that follows always lands
    ///   — even if the user has already clicked over to the custom path, which
    ///   is what used to leave a photo filed as "Classifying..." for good.
    /// - A **manual** photo is never re-decided. The name is the user's call, so
    ///   a later classification leaves the category, confidence and source
    ///   alone. `is_custom` is still re-derived against the live profiles: a
    ///   profile can be deleted while the category stands, and the name then
    ///   has to become editable again or it could never be corrected.
    /// - The **facts** always refresh, manual or not. A photo is staged before
    ///   the model has touched it, so the embedding it was staged with is empty
    ///   and arrives with the decision; without this a photo the user
    ///   categorised mid-scan would train the next profile on nothing.
    ///   **Re-classify All** is the degenerate case — it hands back the facts it
    ///   just read off this item, so the write is a no-op.
    pub(super) fn apply_classification(
        &mut self,
        profiles: &ProfileStore,
        facts: &PhotoFacts,
        classification: Classification,
    ) {
        self.year = facts.date.year;
        self.month = facts.date.month;
        self.is_exif = facts.is_exif;
        self.frame = facts.frame;
        self.embedding = facts.embedding.clone();

        self.apply_decision(profiles, classification);
    }

    /// Writes just the decision, leaving the facts as they are.
    ///
    /// The half of `apply_classification` that staging a photo needs: a photo
    /// that has just arrived already carries its own facts, so only the
    /// classification is missing.
    fn apply_decision(&mut self, profiles: &ProfileStore, classification: Classification) {
        let Classification::Decided(decision) = classification else {
            // A pending photo shows a label and nothing else: `is_custom` is
            // false, so the custom name input does not render and there is
            // nothing on screen for a keystroke to turn into a manual category.
            self.category = CLASSIFYING_LABEL.to_string();
            self.confidence = 0.0;
            self.source = ClassificationSource::UnsortedFallback;
            self.is_custom = false;
            self.pending = true;
            return;
        };

        let keep_manual = self.source == ClassificationSource::Manual && !self.pending;
        if !keep_manual {
            self.category = decision.category.into_string();
            self.confidence = decision.confidence;
            self.source = decision.source;
            self.is_custom = decision.is_custom;
        } else if profiles.is_custom_category(&self.category) != self.is_custom {
            self.is_custom = !self.is_custom;
        }
        self.pending = false;
    }

    /// Marks this photo's category as the user's own call.
    ///
    /// The counterpart to `apply_classification`: the one write a photo gets
    /// that the pipeline does not make, so the Manual flag is set in one place
    /// rather than at each of the three widgets that can trigger it.
    ///
    /// It deliberately does not touch `pending`. A photo still waiting on the
    /// model has no category to have claimed — its `category` is a label — so a
    /// click that arrives in that window does not become a manual claim over
    /// one. What the model decides a moment later wins, which is the only way
    /// a photo can be sure not to end up filed as "Classifying...".
    pub(super) fn mark_manual(&mut self) {
        self.source = ClassificationSource::Manual;
    }

    /// The facts the classifier reads for this photo.
    ///
    /// Read back out of the fields the widgets already render rather than
    /// stored twice. The frame size is carried across the rescan by the scanner
    /// and by the cache, so a photo classified again is read from the same
    /// numbers as the scan that read the file.
    pub(super) fn facts(&self) -> PhotoFacts {
        PhotoFacts {
            path: self.source_path.clone(),
            date: PhotoDate::new(self.year, self.month),
            frame: self.frame,
            is_exif: self.is_exif,
            embedding: self.embedding.clone(),
        }
    }
}

/// State of the inspection modal: which item it shows and, once the background
/// thread has delivered it, the full-resolution texture.
pub struct ModalPreview {
    pub item_index: usize,
    pub high_res_texture: Option<egui::TextureHandle>,
    pub high_res_path: Option<PathBuf>,
    pub is_loading: bool,
}

/// Which control in the inspection modal's bottom row was pressed.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ModalActions {
    pub prev: bool,
    pub next: bool,
    pub train: bool,
    pub close: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classification::{Decision, PhotoFacts};

    /// A decided classification, built the way the store builds one.
    fn decided(category: &str, source: ClassificationSource, is_custom: bool) -> Classification {
        Classification::Decided(Decision {
            category: crate::category_name::CategoryName::from_user_input(category),
            confidence: 0.9,
            source,
            is_custom,
        })
    }

    /// The facts the scan holds once the model has finished with the photo.
    fn facts() -> PhotoFacts {
        PhotoFacts {
            path: PathBuf::from("/photos/img.png"),
            date: PhotoDate::new(2026, 9),
            frame: Some(FrameSize::new(1920, 1080)),
            is_exif: false,
            embedding: vec![0.1, 0.2, 0.3],
        }
    }

    /// A photo staged the way the scan stages one: the frame size is already
    /// known from decoding the file, but the embedding is still being computed.
    fn item(profiles: &ProfileStore) -> StagedItem {
        let ctx = egui::Context::default();
        StagedItem::new(
            profiles,
            PhotoFacts {
                embedding: Vec::new(),
                ..facts()
            },
            Classification::Pending,
            ctx.load_texture(
                "thumb",
                egui::ColorImage::new([4, 4], egui::Color32::GRAY),
                egui::TextureOptions::default(),
            ),
        )
    }

    /// What a scan's `Update` does, with the facts the scan would have read.
    fn update(
        item: &mut StagedItem,
        profiles: &ProfileStore,
        facts: &PhotoFacts,
        classification: Classification,
    ) {
        item.apply_classification(profiles, facts, classification);
    }

    #[test]
    fn a_pending_photo_carries_a_label_and_nothing_to_edit() {
        let profiles = ProfileStore::default();
        let item = item(&profiles);

        assert!(item.pending);
        assert_eq!(item.category, CLASSIFYING_LABEL);
        assert_eq!(item.confidence, 0.0);
        assert_eq!(item.source, ClassificationSource::UnsortedFallback);
        // The custom name input renders only on the custom path, so a
        // placeholder that is not on it cannot be typed into.
        assert!(
            !item.is_custom,
            "the placeholder must not be editable as a category name"
        );
    }

    #[test]
    fn the_decision_replaces_the_pending_placeholder() {
        let profiles = ProfileStore::default();
        let mut item = item(&profiles);

        update(
            &mut item,
            &profiles,
            &facts(),
            decided("Screenshots", ClassificationSource::Heuristic, false),
        );

        assert!(!item.pending);
        assert_eq!(item.category, "Screenshots");
        assert_eq!(item.source, ClassificationSource::Heuristic);
        assert!((item.confidence - 0.9).abs() < 1e-6);
        assert!(!item.is_custom);
    }

    #[test]
    fn the_embedding_catches_up_even_for_a_manual_pick() {
        // The scan stages a photo before the model has touched it, so the
        // embedding arrives with the decision. A photo the user categorised in
        // between has to get it anyway: keeping the empty one would train the
        // next profile on nothing.
        let profiles = ProfileStore::default();
        let mut item = item(&profiles);
        assert!(item.embedding.is_empty(), "staged with nothing to train on");

        update(
            &mut item,
            &profiles,
            &facts(),
            decided("Sunsets", ClassificationSource::Manual, false),
        );
        assert_eq!(item.embedding, vec![0.1, 0.2, 0.3]);

        let mut later = facts();
        later.embedding = vec![0.7, 0.2, 0.1];
        update(
            &mut item,
            &profiles,
            &later,
            decided("Documents", ClassificationSource::Heuristic, true),
        );

        assert_eq!(item.category, "Sunsets", "the user's pick is preserved");
        assert_eq!(
            item.embedding,
            vec![0.7, 0.2, 0.1],
            "a preserved pick must not freeze the embedding it was preserved with"
        );
    }

    #[test]
    fn clicking_other_on_a_pending_photo_claims_nothing() {
        // The direction the combo actually takes, and the one the fix turns on.
        // Clicking "Other" on a photo the model is still working on puts it on
        // the custom path with the placeholder as its name. That is not a
        // category the user chose, so it must not become a claim that outlives
        // the real answer — which is how a photo used to end up filed as
        // "Classifying..." for good.
        let profiles = ProfileStore::default();
        let mut item = item(&profiles);

        item.is_custom = true;
        item.mark_manual();
        assert!(item.pending, "a click during the scan is not a decision");

        update(
            &mut item,
            &profiles,
            &facts(),
            decided("Screenshots", ClassificationSource::Heuristic, false),
        );

        assert_eq!(item.category, "Screenshots");
        assert!(!item.pending);
    }

    #[test]
    fn a_pending_placeholder_is_never_preserved() {
        // The rule in one assertion, whatever route the Manual flag was set by.
        // Before `Pending` existed the placeholder was a real category with a
        // text input bound to it, so one keystroke locked it in and the scan's
        // real answer was skipped for good.
        let profiles = ProfileStore::default();
        let mut item = item(&profiles);
        assert_eq!(item.category, CLASSIFYING_LABEL);

        item.is_custom = true;
        item.mark_manual();

        update(
            &mut item,
            &profiles,
            &facts(),
            decided("Screenshots", ClassificationSource::Heuristic, false),
        );

        assert_eq!(
            item.category, "Screenshots",
            "nothing has been decided yet, so a later decision must land"
        );
        assert!(!item.pending);
        assert!(!item.is_custom);
    }

    #[test]
    fn a_manual_category_is_preserved_across_a_reclassification() {
        let mut profiles = ProfileStore::default();
        profiles.add_exemplar("Sunsets", &[1.0, 0.0, 0.0]);

        let mut item = item(&profiles);
        update(
            &mut item,
            &profiles,
            &facts(),
            decided("Sunsets", ClassificationSource::Manual, false),
        );
        assert_eq!(item.category, "Sunsets");

        update(
            &mut item,
            &profiles,
            &facts(),
            decided("Documents", ClassificationSource::Heuristic, true),
        );

        assert_eq!(item.category, "Sunsets", "the user's pick is the call");
        assert_eq!(item.source, ClassificationSource::Manual);
        assert_eq!(item.confidence, 0.9);
    }

    #[test]
    fn a_manual_category_becomes_editable_when_its_profile_is_deleted() {
        // Deleting a profile re-classifies, and a preserved manual name has to
        // follow it onto the custom path or it could never be corrected.
        let mut profiles = ProfileStore::default();
        profiles.add_exemplar("Sunsets", &[1.0, 0.0, 0.0]);

        let mut item = item(&profiles);
        update(
            &mut item,
            &profiles,
            &facts(),
            decided("Sunsets", ClassificationSource::Manual, false),
        );
        assert!(!item.is_custom);

        profiles.remove_category("Sunsets");
        update(
            &mut item,
            &profiles,
            &facts(),
            decided("Unsorted", ClassificationSource::UnsortedFallback, true),
        );

        assert_eq!(item.category, "Sunsets");
        assert!(
            item.is_custom,
            "a name that matches no profile any more belongs on the custom path"
        );
    }

    #[test]
    fn facts_round_trip_the_frame_size_a_rescan_read() {
        // **Re-classify All** rebuilds its facts from the staged item, so the
        // resolution the rules see has to be the photo's, not the thumbnail's.
        let profiles = ProfileStore::default();
        let item = item(&profiles);
        assert_eq!(item.facts().frame, Some(FrameSize::new(1920, 1080)));
        assert_ne!(
            item.facts().frame.map(|f| f.width),
            Some(200),
            "the frame size must not be the 200x140 thumbnail's"
        );
    }

    #[test]
    fn a_photo_with_no_recorded_frame_size_says_so() {
        let profiles = ProfileStore::default();
        let ctx = egui::Context::default();
        // A cache row written before the frame columns existed: the rescan has
        // no size to offer, and says so rather than inventing one.
        let unknown = PhotoFacts {
            path: PathBuf::from("/photos/legacy.png"),
            date: PhotoDate::new(2020, 1),
            frame: None,
            is_exif: false,
            embedding: vec![0.5],
        };
        let item = StagedItem::new(
            &profiles,
            unknown,
            Classification::Pending,
            ctx.load_texture(
                "thumb",
                egui::ColorImage::new([4, 4], egui::Color32::GRAY),
                egui::TextureOptions::default(),
            ),
        );
        assert_eq!(item.facts().frame, None);
    }
}
