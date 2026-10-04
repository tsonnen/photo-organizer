use crate::category_name::CategoryName;
use crate::classification::{Classification, Decision, PhotoFacts};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

// Re-exported for callers that think of the source enum as part of the store:
// it labels the store's output and was declared here for most of the app's life.
pub use crate::classification::ClassificationSource;

/// Narrowest frame the resolution rule will call a screenshot.
///
/// Below this a 16:9 PNG is a saved thumbnail of a screen or a small graphic,
/// which is not what the rule is for.
const SCREENSHOT_MIN_WIDTH: u32 = 800;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RankedProfile {
    pub name: String,
    pub confidence: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CategoryProfile {
    pub name: String,
    pub centroid: Vec<f32>,
    #[serde(default = "default_sample_count")]
    pub sample_count: usize,
}

fn default_sample_count() -> usize {
    1
}

impl CategoryProfile {
    pub fn new(name: impl Into<String>, centroid: Vec<f32>) -> Self {
        let norm_centroid = normalize_vector(&centroid);
        Self {
            // Canonical from the moment it exists. Everything downstream looks a
            // profile up by this name — `add_exemplar`, `is_custom_category`,
            // `rank_profiles`, the dropdown — and files it under it, so a name
            // only sanitised at the point of use would stop identifying its own
            // profile the moment it was shown next to the stored spelling.
            name: CategoryName::from_user_input(&name.into()).into_string(),
            centroid: norm_centroid,
            sample_count: 1,
        }
    }

    pub fn add_sample(&mut self, embedding: &[f32]) {
        if embedding.is_empty() {
            return;
        }
        if self.centroid.is_empty() || self.sample_count == 0 {
            self.centroid = normalize_vector(embedding);
            self.sample_count = 1;
            return;
        }

        if self.centroid.len() != embedding.len() {
            return;
        }

        // Weighted moving average: new_centroid = (old_centroid * count + new_sample) / (count + 1)
        let k = self.sample_count as f32;
        let updated: Vec<f32> = self
            .centroid
            .iter()
            .zip(embedding.iter())
            .map(|(&c, &e)| c * k + e)
            .collect();

        self.centroid = normalize_vector(&updated);
        self.sample_count += 1;
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProfileStore {
    pub profiles: Vec<CategoryProfile>,
}

impl ProfileStore {
    pub fn load_from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let content = fs::read_to_string(path)?;
        let mut store: Self = serde_json::from_str(&content)?;

        // A `profiles.json` written before `CategoryName` existed can hold a
        // name that is not one path component — `A/B`, `Sunsets.`, `NUL` — and
        // serde will not stop it. Canonicalising on load keeps each profile's
        // name, the label the dropdown shows and the directory it files into
        // from disagreeing; the rewrite lands on disk at the next save, the way
        // the retired threshold field does. Doing it here rather than at the
        // point of use is what stops a sanitised name from looking like a
        // different, custom category to the dropdown.
        for profile in &mut store.profiles {
            profile.name = CategoryName::from_user_input(&profile.name).into_string();
        }

        Ok(store)
    }

    pub fn save_to_file<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        if let Some(parent) = path.as_ref().parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }
        fs::write(path, json)?;
        Ok(())
    }

    pub fn remove_category(&mut self, name: &str) -> bool {
        let initial_len = self.profiles.len();
        self.profiles.retain(|p| !p.name.eq_ignore_ascii_case(name));
        self.profiles.len() < initial_len
    }

    /// Folds an embedding into the profile for `category`, creating it if this
    /// is the first exemplar.
    ///
    /// The name is sanitised here rather than at the two training call sites, so
    /// that what gets trained is exactly what will later be filed: a category
    /// typed as `A/B` is stored as `A-B` and shows that in the dropdown, instead
    /// of reading as `A/B` in the grid and creating two folders on transfer.
    pub fn add_exemplar(&mut self, category: &str, embedding: &[f32]) {
        if embedding.is_empty() {
            return;
        }
        let category = CategoryName::from_user_input(category);
        if let Some(existing) = self
            .profiles
            .iter_mut()
            .find(|p| p.name.eq_ignore_ascii_case(category.as_str()))
        {
            existing.add_sample(embedding);
        } else {
            self.profiles.push(CategoryProfile::new(
                category.into_string(),
                embedding.to_vec(),
            ));
        }
    }

    /// The one entry point for deciding what a photo is: the visual tier, then
    /// the rules, then `Unsorted`.
    ///
    /// Takes the photo's facts rather than a loose `&[f32]` plus a path, an
    /// EXIF flag and two dimensions, because the rules key off resolution and
    /// the frame size is the input callers used to get wrong. Two of the three
    /// callers passed the ≤200x140 thumbnail's dimensions — a cached rescan,
    /// because the cache stored nothing else, and **Re-classify All**, because
    /// a staged item carried nothing else — which ruled a 1920x1080 PNG out of
    /// Screenshots on every scan after the one that read the file.
    ///
    /// `threshold` is the user's bar from
    /// [`crate::settings::Settings::confidence_threshold`], passed in rather than
    /// read from the store: profiles are learned data, the bar is a setting, and
    /// keeping them apart means a `profiles.json` never carries a stale copy of a
    /// knob the UI owns. The gate cannot be dropped, whatever it is set to — it is
    /// the only path from the visual tier into the rules.
    pub fn classify(&self, facts: &PhotoFacts, threshold: f32) -> Classification {
        let best_match = self.best_centroid_match(&facts.embedding);
        let best_similarity = best_match.as_ref().map(|(_, similarity)| *similarity);

        if let Some((category, confidence)) = best_match {
            if confidence >= threshold {
                return self.decide(category, confidence, ClassificationSource::VisualModel);
            }
        }

        // Too weak to believe, but still the closest thing there is. The rules
        // outrank a low-confidence centroid match, which is the whole reason the
        // threshold gate exists at all.
        if let Some((category, confidence)) = classify_by_rules(facts) {
            return self.decide(category, confidence, ClassificationSource::Heuristic);
        }

        // Nothing claimed it. The similarity is still worth reporting, so the
        // dropdown can show how near the closest profile came.
        self.decide(
            CategoryName::unsorted(),
            best_similarity.unwrap_or(0.0).max(0.0),
            ClassificationSource::UnsortedFallback,
        )
    }

    /// Builds the one kind of decided classification there is, settling
    /// `is_custom` here rather than leaving each caller to re-ask.
    fn decide(
        &self,
        category: CategoryName,
        confidence: f32,
        source: ClassificationSource,
    ) -> Classification {
        Classification::Decided(Decision {
            is_custom: self.is_custom_category(category.as_str()),
            category,
            confidence,
            source,
        })
    }

    /// A category is "custom" when it matches no profile name, so the custom
    /// name input applies to it. Matching ignores case.
    ///
    /// Asked in exactly two places: `decide`, for every classification the
    /// pipeline makes, and
    /// [`StagedItem::apply_classification`](crate::app::models::StagedItem::apply_classification),
    /// which has to re-ask for a manual category because the profile it matched
    /// can be deleted while the category stands.
    pub fn is_custom_category(&self, category: &str) -> bool {
        !self
            .profiles
            .iter()
            .any(|p| p.name.eq_ignore_ascii_case(category))
    }

    /// The closest trained centroid to `embedding`, with its similarity, or
    /// `None` when there is nothing to compare against.
    fn best_centroid_match(&self, embedding: &[f32]) -> Option<(CategoryName, f32)> {
        if embedding.is_empty() || self.profiles.is_empty() {
            return None;
        }

        let mut best: Option<(&CategoryProfile, f32)> = None;
        for profile in &self.profiles {
            let similarity = cosine_similarity(embedding, &profile.centroid);
            // Strictly greater, so a tie keeps the earlier profile: two
            // categories trained on the same exemplar sort by the order they
            // were trained in.
            if best.is_none_or(|(_, top)| similarity > top) {
                best = Some((profile, similarity));
            }
        }

let (profile, similarity) = best?;
        Some((
            // A backstop, not the sanitisation point: `CategoryProfile::new`
            // and `load_from_file` both canonicalise the name, so this is
            // already a no-op and the result still identifies the profile
            // it came from. It stays because the field is public and a
            // `CategoryProfile` can be built by struct literal.
            CategoryName::from_user_input(&profile.name),
            similarity.clamp(0.0, 1.0),
        ))
    }

    /// Computes similarity of the given embedding against all profile centroids,
    /// returning them sorted descending by confidence (and alphabetically by name for ties).
    pub fn rank_profiles(&self, embedding: &[f32]) -> Vec<RankedProfile> {
        let mut list: Vec<RankedProfile> = self
            .profiles
            .iter()
            .map(|p| {
                let conf = if !embedding.is_empty() && !p.centroid.is_empty() {
                    cosine_similarity(embedding, &p.centroid).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                RankedProfile {
                    name: p.name.clone(),
                    confidence: conf,
                }
            })
            .collect();

        list.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.name.cmp(&b.name))
        });

        list
    }
}

/// The rule tier: filename, resolution and metadata patterns for photos no
/// centroid could claim.
///
/// Takes the whole [`PhotoFacts`] rather than four arguments because the rules
/// are only ever about what the photo is — and because `width`/`height` in
/// particular is the input that two of the three callers used to fill with the
/// thumbnail's size.
fn classify_by_rules(facts: &PhotoFacts) -> Option<(CategoryName, f32)> {
    let filename = facts
        .path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_lowercase();
    let ext = facts
        .path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    // 1. Screenshot detection
    let screenshot_names = [
        "screenshot",
        "screen_shot",
        "screen shot",
        "screencap",
        "capture",
        "snip",
        "bildupplösning",
        "bildschirmfoto",
        "captura",
    ];
    let has_screenshot_keyword = screenshot_names.iter().any(|&k| filename.contains(k));

    if has_screenshot_keyword {
        return Some((CategoryName::screenshots(), 0.95));
    }

    // The ratio rule needs the photo's true frame size, and a photo whose size
    // was never recorded — a cache row written before the dimensions were
    // persisted — has nothing to key off. Skipping the rule is the honest
    // answer; substituting the thumbnail is what ruled real screenshots out of
    // Screenshots on every rescan.
    if let Some(frame) = facts.frame {
        // Check standard screen aspect ratios: 16:9 (~1.777), 16:10 (1.6), 19.5:9 (~2.166), 20:9 (~2.222), 21:9 (~2.333) and portrait inverses
        let aspect_ratio = frame.aspect_ratio();
        let is_screen_ratio = (aspect_ratio - 1.777).abs() < 0.03
            || (aspect_ratio - 1.6).abs() < 0.03
            || (aspect_ratio - 2.166).abs() < 0.04
            || (aspect_ratio - 0.5625).abs() < 0.03
            || (aspect_ratio - 0.625).abs() < 0.03
            || (aspect_ratio - 0.4615).abs() < 0.03;

        if !facts.is_exif && ext == "png" && is_screen_ratio && frame.width >= SCREENSHOT_MIN_WIDTH
        {
            return Some((CategoryName::screenshots(), 0.85));
        }
    }

    // 2. Document / Receipt detection by filename keywords
    let doc_keywords = [
        "receipt",
        "invoice",
        "document",
        "scan",
        "statement",
        "bill",
        "tax",
    ];
    if doc_keywords.iter().any(|&k| filename.contains(k)) {
        return Some((CategoryName::documents(), 0.90));
    }

    // 3. Camera photos fallback if EXIF tags exist
    if facts.is_exif {
        return Some((CategoryName::camera_photos(), 0.70));
    }

    None
}

pub fn normalize_vector(v: &[f32]) -> Vec<f32> {
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm == 0.0 {
        v.to_vec()
    } else {
        v.iter().map(|x| x / norm).collect()
    }
}

pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

    if norm_a == 0.0 || norm_b == 0.0 {
        0.0
    } else {
        dot / (norm_a * norm_b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classification::{FrameSize, PhotoDate};
    // The bar is a setting, so every test that is not about the bar itself
    // decides against the shipped default.
    use crate::settings::DEFAULT_CONFIDENCE_THRESHOLD;
    use std::f32::consts::FRAC_1_SQRT_2;
    use std::path::PathBuf;

    #[test]
    fn test_cosine_similarity() {
        let v1 = vec![1.0, 0.0, 0.0];
        let v2 = vec![1.0, 0.0, 0.0];
        let v3 = vec![0.0, 1.0, 0.0];
        let v4 = vec![-1.0, 0.0, 0.0];

        assert!((cosine_similarity(&v1, &v2) - 1.0).abs() < 1e-6);
        assert!((cosine_similarity(&v1, &v3) - 0.0).abs() < 1e-6);
        assert!((cosine_similarity(&v1, &v4) - (-1.0)).abs() < 1e-6);
        assert_eq!(cosine_similarity(&[], &[]), 0.0);
        assert_eq!(cosine_similarity(&v1, &[]), 0.0);
    }

    #[test]
    fn test_normalize_vector() {
        let v = vec![3.0, 4.0];
        let norm_v = normalize_vector(&v);
        assert!((norm_v[0] - 0.6).abs() < 1e-6);
        assert!((norm_v[1] - 0.8).abs() < 1e-6);
        assert_eq!(normalize_vector(&[]), Vec::<f32>::new());
    }

    /// Facts for a photo that trips no rule: no keyword in the name, no EXIF,
    /// and no frame size the ratio rules could read.
    fn plain_facts(embedding: Vec<f32>) -> PhotoFacts {
        PhotoFacts {
            path: PathBuf::from("/photos/unremarkable_file.xyz"),
            date: PhotoDate::new(2026, 1),
            frame: None,
            is_exif: false,
            embedding,
        }
    }

    fn facts_with(path: &str, is_exif: bool, frame: Option<(u32, u32)>) -> PhotoFacts {
        PhotoFacts {
            path: PathBuf::from(path),
            date: PhotoDate::new(2026, 1),
            frame: frame.map(|(w, h)| FrameSize::new(w, h)),
            is_exif,
            embedding: Vec::new(),
        }
    }

    /// The `(category, confidence, source)` a classification decided, panicking
    /// on a `Pending` — every test here classifies.
    fn assert_decided(
        classification: &Classification,
        category: &CategoryName,
        source: ClassificationSource,
    ) {
        let decision = classification
            .decided()
            .unwrap_or_else(|| panic!("expected a decision, got {classification:?}"));
        assert_eq!(&decision.category, category);
        assert_eq!(decision.source, source);
    }

    #[test]
    fn test_classify_empty() {
        let store = ProfileStore::default();
        assert_decided(
            &store.classify(&plain_facts(Vec::new()), DEFAULT_CONFIDENCE_THRESHOLD),
            &CategoryName::unsorted(),
            ClassificationSource::UnsortedFallback,
        );

        // An embedding with nothing to compare it against is still nothing.
        assert_decided(
            &store.classify(&plain_facts(vec![1.0, 0.0]), DEFAULT_CONFIDENCE_THRESHOLD),
            &CategoryName::unsorted(),
            ClassificationSource::UnsortedFallback,
        );
    }

    #[test]
    fn test_classify_matching() {
        let store = ProfileStore {
            profiles: vec![
                CategoryProfile::new("Landscape", vec![1.0, 0.0, 0.0]),
                CategoryProfile::new("Portrait", vec![0.0, 1.0, 0.0]),
            ],
        };

        let res1 = store.classify(
            &plain_facts(vec![0.9, 0.1, 0.0]),
            DEFAULT_CONFIDENCE_THRESHOLD,
        );
        assert_decided(
            &res1,
            &CategoryName::from_user_input("Landscape"),
            ClassificationSource::VisualModel,
        );
        assert!(res1.decided().unwrap().confidence > DEFAULT_CONFIDENCE_THRESHOLD);
        // A trained profile's own name is not on the custom path.
        assert!(!res1.decided().unwrap().is_custom);

        let res2 = store.classify(
            &plain_facts(vec![0.1, 0.9, 0.0]),
            DEFAULT_CONFIDENCE_THRESHOLD,
        );
        assert_decided(
            &res2,
            &CategoryName::from_user_input("Portrait"),
            ClassificationSource::VisualModel,
        );
        assert!(res2.decided().unwrap().confidence > DEFAULT_CONFIDENCE_THRESHOLD);
    }

    #[test]
    fn test_classify_below_threshold() {
        let store = ProfileStore {
            profiles: vec![CategoryProfile::new("Landscape", vec![1.0, 0.0, 0.0])],
        };

        // Similarity is 0.5, under the default threshold: the closest centroid
        // still cannot claim the photo, so it is handed to the rules.
        let res = store.classify(
            &plain_facts(vec![0.5, 0.866, 0.0]),
            DEFAULT_CONFIDENCE_THRESHOLD,
        );
        assert_decided(
            &res,
            &CategoryName::unsorted(),
            ClassificationSource::UnsortedFallback,
        );
        assert!(res.decided().unwrap().confidence < DEFAULT_CONFIDENCE_THRESHOLD);
    }

    #[test]
    fn an_unknown_category_is_decided_as_custom() {
        // `is_custom` is settled here, in the one place a decision is built, so
        // no caller has to re-ask which profiles existed at the time.
        let store = ProfileStore {
            profiles: vec![CategoryProfile::new("Sunsets", vec![1.0, 0.0, 0.0])],
        };

        // Nothing matches this photo, so it lands on Unsorted — which matches no
        // profile, hence the custom path.
        assert!(
            store
                .classify(&plain_facts(Vec::new()), DEFAULT_CONFIDENCE_THRESHOLD)
                .decided()
                .unwrap()
                .is_custom
        );

        // A rule category is not a trained profile either.
        assert!(
            store
                .classify(
                    &facts_with("/photos/my_screenshot.png", false, None),
                    DEFAULT_CONFIDENCE_THRESHOLD
                )
                .decided()
                .unwrap()
                .is_custom
        );

        // A trained profile's name is not custom.
        assert!(
            !store
                .classify(
                    &plain_facts(vec![1.0, 0.0, 0.0]),
                    DEFAULT_CONFIDENCE_THRESHOLD
                )
                .decided()
                .unwrap()
                .is_custom
        );
    }

    #[test]
    fn test_classify_honours_the_given_threshold() {
        // The bar is the user's setting, so the same photo has to be classifiable
        // or not purely by which threshold it is classified against.
        let store = ProfileStore {
            profiles: vec![CategoryProfile::new("Landscape", vec![1.0, 0.0, 0.0])],
        };
        // A unit vector at 0.6 to the profile axis, so the similarity is exactly
        // 0.6 rather than a rounded approximation of it.
        let facts = plain_facts(vec![0.6, 0.8, 0.0]);

        let lenient = store.classify(&facts, 0.30);
        assert_decided(
            &lenient,
            &CategoryName::from_user_input("Landscape"),
            ClassificationSource::VisualModel,
        );
        assert!((lenient.decided().unwrap().confidence - 0.6).abs() < 1e-5);

        let strict = store.classify(&facts, 0.80);
        assert_decided(
            &strict,
            &CategoryName::unsorted(),
            ClassificationSource::UnsortedFallback,
        );

        // The rejected match still reports the similarity it found, so the UI
        // can show the user how close it came.
        assert!((strict.decided().unwrap().confidence - 0.6).abs() < 1e-5);
    }

    #[test]
    fn test_category_profile_multi_sample_centroid() {
        let mut profile = CategoryProfile::new("Test", vec![1.0, 0.0]);
        assert_eq!(profile.sample_count, 1);
        assert!((profile.centroid[0] - 1.0).abs() < 1e-6);

        // Add second orthogonal vector [0.0, 1.0] -> avg [0.5, 0.5] -> normalized [0.7071, 0.7071]
        profile.add_sample(&[0.0, 1.0]);
        assert_eq!(profile.sample_count, 2);
        assert!((profile.centroid[0] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-4);
        assert!((profile.centroid[1] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-4);
    }

    #[test]
    fn test_profile_store_json_save_load() {
        let temp_dir = std::env::temp_dir();
        let file_path = temp_dir.join(format!("test_profiles_{}.json", std::process::id()));

        let mut store = ProfileStore::default();
        store.profiles.push(CategoryProfile {
            name: "Sunsets".to_string(),
            centroid: vec![0.8, 0.6],
            sample_count: 3,
        });

        store.save_to_file(&file_path).expect("save profiles");
        let loaded = ProfileStore::load_from_file(&file_path).expect("load profiles");

        assert_eq!(loaded.profiles.len(), 1);
        assert_eq!(loaded.profiles[0].name, "Sunsets");
        assert_eq!(loaded.profiles[0].sample_count, 3);

        let _ = fs::remove_file(&file_path);
    }

    #[test]
    fn test_load_ignores_a_saved_threshold() {
        // The confidence threshold is a setting in `settings.json`, not profile
        // data, so a `profiles.json` that still carries one is stale from a
        // build that predates the split. Serde ignores unknown fields, so those
        // files have to keep loading and the stale value has to be dropped on
        // the next save rather than resurfacing as a phantom setting.
        let temp_dir = std::env::temp_dir();
        let file_path = temp_dir.join(format!("test_legacy_profiles_{}.json", std::process::id()));

        fs::write(
            &file_path,
            r#"{
  "profiles": [
    {
      "name": "Sunsets",
      "centroid": [0.8, 0.6],
      "sample_count": 3
    }
  ],
  "confidence_threshold": 0.72
}"#,
        )
        .expect("write legacy profiles");

        let loaded = ProfileStore::load_from_file(&file_path).expect("load legacy profiles");
        assert_eq!(loaded.profiles.len(), 1);
        assert_eq!(loaded.profiles[0].name, "Sunsets");

        loaded.save_to_file(&file_path).expect("re-save");
        let rewritten = fs::read_to_string(&file_path).expect("read re-saved");
        assert!(
            !rewritten.contains("confidence_threshold"),
            "re-saving should drop the retired field, got:\n{rewritten}"
        );

        let _ = fs::remove_file(&file_path);
    }

    #[test]
fn test_load_canonicalises_a_name_that_is_not_one_path_component() {
        // The train box took any text before `CategoryName` existed, so a saved
        // profile can hold a name that is not a legal directory. Canonicalising
        // it here — rather than at the point of use — is what keeps the profile
        // findable: sanitise only the returned name and `is_custom_category`
        // compares `A-B` against a stored `A/B`, finds nothing, and reports a
        // trained category as custom.
        let temp_dir = std::env::temp_dir();
        let file_path = temp_dir.join(format!("test_legacy_names_{}.json", std::process::id()));

        fs::write(
            &file_path,
            r#"{
  "profiles": [
    { "name": "Beach/Trip", "centroid": [0.8, 0.6], "sample_count": 3 },
    { "name": "Sunsets.", "centroid": [0.0, 1.0], "sample_count": 1 },
    { "name": "NUL", "centroid": [0.6, 0.8], "sample_count": 1 }
  ]
}"#,
        )
        .expect("write legacy names");

        let mut loaded = ProfileStore::load_from_file(&file_path).expect("load legacy names");
        let names: Vec<&str> = loaded.profiles.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Beach-Trip", "Sunsets", "Unsorted"]);

        // The canonical name is what the rest of the app matches on, so training
        // under it has to fold into the existing profile. Canonicalise only the
        // name `classify` returns and the dropdown would show `A-B`, retrain it
        // as `A-B` and fork a second profile alongside the stored `A/B`.
        loaded.add_exemplar("Beach-Trip", &[0.8, 0.6]);
        assert_eq!(
            loaded.profiles.len(),
            3,
            "training under the shown name must not fork a duplicate profile"
        );

        // And the rewrite is persisted, so it only has to happen once.
        loaded.save_to_file(&file_path).expect("re-save");
        let reloaded = ProfileStore::load_from_file(&file_path).expect("reload re-saved");
        assert_eq!(reloaded.profiles[0].name, "Beach-Trip");

        let _ = fs::remove_file(&file_path);
    }

    #[test]
    fn test_classify_rules_screenshot() {
        let store = ProfileStore::default();
        let res = store.classify(
            &facts_with(
                "/path/to/Screenshot_2026-09-14.png",
                false,
                Some((1920, 1080)),
            ),
            DEFAULT_CONFIDENCE_THRESHOLD,
        );
        assert_decided(
            &res,
            &CategoryName::screenshots(),
            ClassificationSource::Heuristic,
        );
        assert!(res.decided().unwrap().confidence >= 0.85);

        let res2 = store.classify(
            &facts_with("/path/to/Screen Shot 2026.jpg", false, Some((2560, 1440))),
            DEFAULT_CONFIDENCE_THRESHOLD,
        );
        assert_decided(
            &res2,
            &CategoryName::screenshots(),
            ClassificationSource::Heuristic,
        );

        // Ratio matching without a keyword, on a non-EXIF PNG at screen size.
        let res3 = store.classify(
            &facts_with("/path/to/image_12345.png", false, Some((1920, 1080))),
            DEFAULT_CONFIDENCE_THRESHOLD,
        );
        assert_decided(
            &res3,
            &CategoryName::screenshots(),
            ClassificationSource::Heuristic,
        );
    }

    #[test]
    fn test_classify_rules_documents() {
        let store = ProfileStore::default();
        let res = store.classify(
            &facts_with(
                "/path/to/grocery_receipt_october.jpg",
                false,
                Some((800, 1200)),
            ),
            DEFAULT_CONFIDENCE_THRESHOLD,
        );
        assert_decided(
            &res,
            &CategoryName::documents(),
            ClassificationSource::Heuristic,
        );
        assert!(res.decided().unwrap().confidence >= 0.90);
    }

    #[test]
    fn test_classify_rules_camera_fallback() {
        let store = ProfileStore::default();
        let res = store.classify(
            &facts_with("/path/to/IMG_4321.jpg", true, Some((4000, 3000))),
            DEFAULT_CONFIDENCE_THRESHOLD,
        );
        assert_decided(
            &res,
            &CategoryName::camera_photos(),
            ClassificationSource::Heuristic,
        );
    }

    #[test]
    fn the_resolution_rule_reads_the_frame_size_it_is_given() {
        // The rule that made the divergence a bug: a 16:9 PNG at 1920x1080 is a
        // screenshot, and the same photo's 200x140 thumbnail is not. Since
        // `frame` is a field of the facts rather than a loose argument, the two
        // callers that used to pass the thumbnail's size have no way to.
        let store = ProfileStore::default();

        let full = store.classify(
            &facts_with("/p/plain.png", false, Some((1920, 1080))),
            DEFAULT_CONFIDENCE_THRESHOLD,
        );
        assert_decided(
            &full,
            &CategoryName::screenshots(),
            ClassificationSource::Heuristic,
        );

        let thumbnail = store.classify(
            &facts_with("/p/plain.png", false, Some((200, 140))),
            DEFAULT_CONFIDENCE_THRESHOLD,
        );
        assert_decided(
            &thumbnail,
            &CategoryName::unsorted(),
            ClassificationSource::UnsortedFallback,
        );

        // And with no frame size recorded at all, the rule declines rather than
        // guessing from whatever is to hand.
        let unrecorded = store.classify(
            &facts_with("/p/plain.png", false, None),
            DEFAULT_CONFIDENCE_THRESHOLD,
        );
        assert_decided(
            &unrecorded,
            &CategoryName::unsorted(),
            ClassificationSource::UnsortedFallback,
        );
    }

    #[test]
    fn test_classify_tier_fallbacks() {
        let store = ProfileStore {
            profiles: vec![CategoryProfile::new("Landscape", vec![1.0, 0.0, 0.0])],
        };

        // 1. The centroid claims it, even over a rule that would also match.
        let res1 = store.classify(
            &PhotoFacts {
                path: PathBuf::from("screenshot.png"),
                embedding: vec![0.99, 0.01, 0.0],
                ..facts_with("screenshot.png", false, Some((1920, 1080)))
            },
            DEFAULT_CONFIDENCE_THRESHOLD,
        );
        assert_decided(
            &res1,
            &CategoryName::from_user_input("Landscape"),
            ClassificationSource::VisualModel,
        );

        // 2. Visual below threshold or missing, falls back to the rules.
        let res2 = store.classify(
            &facts_with("my_screenshot.png", false, Some((1920, 1080))),
            DEFAULT_CONFIDENCE_THRESHOLD,
        );
        assert_decided(
            &res2,
            &CategoryName::screenshots(),
            ClassificationSource::Heuristic,
        );

        // 2b. A real embedding that merely resembles the profile still loses to
        // the rules, which is the whole reason the threshold gate exists.
        let res2b = store.classify(
            &PhotoFacts {
                path: PathBuf::from("receipt_from_the_shop.jpg"),
                embedding: vec![0.5, 0.866, 0.0],
                ..facts_with("receipt_from_the_shop.jpg", false, Some((800, 1200)))
            },
            DEFAULT_CONFIDENCE_THRESHOLD,
        );
        assert_decided(
            &res2b,
            &CategoryName::documents(),
            ClassificationSource::Heuristic,
        );

        // 3. Neither matches -> Unsorted, still reporting how near the closest
        // profile came.
        let res3 = store.classify(
            &facts_with("unknown_file.xyz", false, Some((500, 500))),
            DEFAULT_CONFIDENCE_THRESHOLD,
        );
        assert_decided(
            &res3,
            &CategoryName::unsorted(),
            ClassificationSource::UnsortedFallback,
        );
        assert!(res3.decided().unwrap().confidence <= 1.0);
    }

    #[test]
    fn test_rank_profiles_empty_store() {
        let store = ProfileStore::default();
        let ranked = store.rank_profiles(&[1.0, 0.0]);
        assert!(ranked.is_empty());
    }

    #[test]
    fn test_rank_profiles_empty_embedding() {
        let store = ProfileStore {
            profiles: vec![
                CategoryProfile::new("Landscape", vec![1.0, 0.0]),
                CategoryProfile::new("Portrait", vec![0.0, 1.0]),
            ],
        };
        let ranked = store.rank_profiles(&[]);
        assert_eq!(ranked.len(), 2);
        assert_eq!(ranked[0].name, "Landscape");
        assert_eq!(ranked[0].confidence, 0.0);
        assert_eq!(ranked[1].name, "Portrait");
        assert_eq!(ranked[1].confidence, 0.0);
    }

    #[test]
    fn test_rank_profiles_sorted_order() {
        let store = ProfileStore {
            profiles: vec![
                CategoryProfile::new("Portrait", vec![0.0, 1.0, 0.0]),
                CategoryProfile::new("Landscape", vec![1.0, 0.0, 0.0]),
                CategoryProfile::new("Sunset", vec![FRAC_1_SQRT_2, FRAC_1_SQRT_2, 0.0]),
            ],
        };

        // Query vector is close to Landscape [1.0, 0.0, 0.0]
        // Cosine similarities:
        // Landscape: ~1.0
        // Sunset: ~0.7071 (FRAC_1_SQRT_2)
        // Portrait: 0.0
        let query = vec![0.98, 0.02, 0.0];
        let ranked = store.rank_profiles(&query);

        assert_eq!(ranked.len(), 3);
        assert_eq!(ranked[0].name, "Landscape");
        assert!(ranked[0].confidence > 0.95);
        assert_eq!(ranked[1].name, "Sunset");
        assert!((ranked[1].confidence - FRAC_1_SQRT_2).abs() < 0.05);
        assert_eq!(ranked[2].name, "Portrait");
        assert!(ranked[2].confidence < 0.05);
    }
}
