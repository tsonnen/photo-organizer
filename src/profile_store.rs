use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClassificationSource {
    VisualModel,
    Heuristic,
    Manual,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClassificationResult {
    pub category: String,
    pub confidence: f32,
    pub source: ClassificationSource,
}

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
            name: name.into(),
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileStore {
    pub profiles: Vec<CategoryProfile>,
    pub confidence_threshold: f32,
}

impl Default for ProfileStore {
    fn default() -> Self {
        Self {
            profiles: Vec::new(),
            confidence_threshold: 0.65,
        }
    }
}

impl ProfileStore {
    pub fn load_from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let content = fs::read_to_string(path)?;
        let store: Self = serde_json::from_str(&content)?;
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

    pub fn add_exemplar(&mut self, category: &str, embedding: &[f32]) {
        if embedding.is_empty() {
            return;
        }
        if let Some(existing) = self
            .profiles
            .iter_mut()
            .find(|p| p.name.eq_ignore_ascii_case(category))
        {
            existing.add_sample(embedding);
        } else {
            self.profiles
                .push(CategoryProfile::new(category, embedding.to_vec()));
        }
    }

    pub fn classify(&self, embedding: &[f32]) -> ClassificationResult {
        if embedding.is_empty() || self.profiles.is_empty() {
            return ClassificationResult {
                category: "Unsorted".to_string(),
                confidence: 0.0,
                source: ClassificationSource::UnsortedFallback,
            };
        }

        let mut best_match: Option<(&CategoryProfile, f32)> = None;

        for profile in &self.profiles {
            let sim = cosine_similarity(embedding, &profile.centroid);
            match best_match {
                None => best_match = Some((profile, sim)),
                Some((_, max_sim)) if sim > max_sim => best_match = Some((profile, sim)),
                _ => {}
            }
        }

        if let Some((profile, sim)) = best_match {
            if sim >= self.confidence_threshold {
                return ClassificationResult {
                    category: profile.name.clone(),
                    confidence: sim.clamp(0.0, 1.0),
                    source: ClassificationSource::VisualModel,
                };
            } else {
                return ClassificationResult {
                    category: "Unsorted".to_string(),
                    confidence: sim.max(0.0),
                    source: ClassificationSource::UnsortedFallback,
                };
            }
        }

        ClassificationResult {
            category: "Unsorted".to_string(),
            confidence: 0.0,
            source: ClassificationSource::UnsortedFallback,
        }
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

    pub fn classify_with_heuristics(
        &self,
        embedding: &[f32],
        path: &Path,
        is_exif: bool,
        width: u32,
        height: u32,
    ) -> ClassificationResult {
        // 1. Try Visual CLIP classification first if embedding exists
        let visual_res = self.classify(embedding);
        if visual_res.source == ClassificationSource::VisualModel {
            return visual_res;
        }

        // 2. Try Rule-based / Metadata heuristics
        if let Some((cat, conf)) = Self::classify_heuristics(path, is_exif, width, height) {
            return ClassificationResult {
                category: cat,
                confidence: conf,
                source: ClassificationSource::Heuristic,
            };
        }

        // 3. Fallback to visual result (which is Unsorted)
        visual_res
    }

    pub fn classify_heuristics(
        path: &Path,
        is_exif: bool,
        width: u32,
        height: u32,
    ) -> Option<(String, f32)> {
        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_lowercase();
        let ext = path
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

        let aspect_ratio = if height > 0 {
            width as f32 / height as f32
        } else {
            1.0
        };
        // Check standard screen aspect ratios: 16:9 (~1.777), 16:10 (1.6), 19.5:9 (~2.166), 20:9 (~2.222), 21:9 (~2.333) and portrait inverses
        let is_screen_ratio = (aspect_ratio - 1.777).abs() < 0.03
            || (aspect_ratio - 1.6).abs() < 0.03
            || (aspect_ratio - 2.166).abs() < 0.04
            || (aspect_ratio - 0.5625).abs() < 0.03
            || (aspect_ratio - 0.625).abs() < 0.03
            || (aspect_ratio - 0.4615).abs() < 0.03;

        if has_screenshot_keyword {
            return Some(("Screenshots".to_string(), 0.95));
        }

        if !is_exif && ext == "png" && is_screen_ratio && width >= 800 {
            return Some(("Screenshots".to_string(), 0.85));
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
            return Some(("Documents".to_string(), 0.90));
        }

        // 3. Camera photos fallback if EXIF tags exist
        if is_exif {
            return Some(("Camera Photos".to_string(), 0.70));
        }

        None
    }
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

    #[test]
    fn test_classify_empty() {
        let store = ProfileStore::default();
        let res = store.classify(&[]);
        assert_eq!(res.category, "Unsorted");
        assert_eq!(res.confidence, 0.0);
        assert_eq!(res.source, ClassificationSource::UnsortedFallback);

        let res2 = store.classify(&[1.0, 0.0]);
        assert_eq!(res2.category, "Unsorted");
        assert_eq!(res2.confidence, 0.0);
        assert_eq!(res2.source, ClassificationSource::UnsortedFallback);
    }

    #[test]
    fn test_classify_matching() {
        let store = ProfileStore {
            profiles: vec![
                CategoryProfile::new("Landscape", vec![1.0, 0.0, 0.0]),
                CategoryProfile::new("Portrait", vec![0.0, 1.0, 0.0]),
            ],
            confidence_threshold: 0.65,
        };

        let res1 = store.classify(&[0.9, 0.1, 0.0]);
        assert_eq!(res1.category, "Landscape");
        assert!(res1.confidence > 0.65);
        assert_eq!(res1.source, ClassificationSource::VisualModel);

        let res2 = store.classify(&[0.1, 0.9, 0.0]);
        assert_eq!(res2.category, "Portrait");
        assert!(res2.confidence > 0.65);
        assert_eq!(res2.source, ClassificationSource::VisualModel);
    }

    #[test]
    fn test_classify_below_threshold() {
        let store = ProfileStore {
            profiles: vec![CategoryProfile::new("Landscape", vec![1.0, 0.0, 0.0])],
            confidence_threshold: 0.8,
        };

        // Similarity is 0.5 (below 0.8 threshold)
        let res = store.classify(&[0.5, 0.866, 0.0]);
        assert_eq!(res.category, "Unsorted");
        assert!(res.confidence < 0.8);
        assert_eq!(res.source, ClassificationSource::UnsortedFallback);
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
        store.confidence_threshold = 0.72;

        store.save_to_file(&file_path).expect("save profiles");
        let loaded = ProfileStore::load_from_file(&file_path).expect("load profiles");

        assert_eq!(loaded.profiles.len(), 1);
        assert_eq!(loaded.profiles[0].name, "Sunsets");
        assert_eq!(loaded.profiles[0].sample_count, 3);
        assert!((loaded.confidence_threshold - 0.72).abs() < 1e-6);

        let _ = fs::remove_file(&file_path);
    }

    #[test]
    fn test_classify_heuristics_screenshot() {
        let p1 = Path::new("/path/to/Screenshot_2026-09-14.png");
        let res = ProfileStore::classify_heuristics(p1, false, 1920, 1080);
        assert!(res.is_some());
        let (cat, conf) = res.unwrap();
        assert_eq!(cat, "Screenshots");
        assert!(conf >= 0.85);

        let p2 = Path::new("/path/to/Screen Shot 2026.jpg");
        let res2 = ProfileStore::classify_heuristics(p2, false, 2560, 1440);
        assert_eq!(res2.unwrap().0, "Screenshots");

        // Ratio matching without keyword
        let p3 = Path::new("/path/to/image_12345.png");
        let res3 = ProfileStore::classify_heuristics(p3, false, 1920, 1080);
        assert_eq!(res3.unwrap().0, "Screenshots");
    }

    #[test]
    fn test_classify_heuristics_documents() {
        let p = Path::new("/path/to/grocery_receipt_october.jpg");
        let res = ProfileStore::classify_heuristics(p, false, 800, 1200);
        assert!(res.is_some());
        let (cat, conf) = res.unwrap();
        assert_eq!(cat, "Documents");
        assert!(conf >= 0.90);
    }

    #[test]
    fn test_classify_heuristics_camera_fallback() {
        let p = Path::new("/path/to/IMG_4321.jpg");
        let res = ProfileStore::classify_heuristics(p, true, 4000, 3000);
        assert!(res.is_some());
        let (cat, _) = res.unwrap();
        assert_eq!(cat, "Camera Photos");
    }

    #[test]
    fn test_classify_with_heuristics_fallbacks() {
        let store = ProfileStore {
            profiles: vec![CategoryProfile::new("Landscape", vec![1.0, 0.0, 0.0])],
            confidence_threshold: 0.8,
        };

        // 1. Matches visual
        let res1 = store.classify_with_heuristics(
            &[0.99, 0.01, 0.0],
            Path::new("screenshot.png"),
            false,
            1920,
            1080,
        );
        assert_eq!(res1.category, "Landscape");
        assert_eq!(res1.source, ClassificationSource::VisualModel);

        // 2. Visual below threshold or missing, falls back to heuristic
        let res2 =
            store.classify_with_heuristics(&[], Path::new("my_screenshot.png"), false, 1920, 1080);
        assert_eq!(res2.category, "Screenshots");
        assert_eq!(res2.source, ClassificationSource::Heuristic);

        // 3. Neither matches -> Unsorted
        let res3 =
            store.classify_with_heuristics(&[], Path::new("unknown_file.xyz"), false, 500, 500);
        assert_eq!(res3.category, "Unsorted");
        assert_eq!(res3.source, ClassificationSource::UnsortedFallback);
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
            confidence_threshold: 0.65,
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
                CategoryProfile::new("Sunset", vec![0.7071, 0.7071, 0.0]),
            ],
            confidence_threshold: 0.65,
        };

        // Query vector is close to Landscape [1.0, 0.0, 0.0]
        // Cosine similarities:
        // Landscape: ~1.0
        // Sunset: ~0.7071
        // Portrait: 0.0
        let query = vec![0.98, 0.02, 0.0];
        let ranked = store.rank_profiles(&query);

        assert_eq!(ranked.len(), 3);
        assert_eq!(ranked[0].name, "Landscape");
        assert!(ranked[0].confidence > 0.95);
        assert_eq!(ranked[1].name, "Sunset");
        assert!((ranked[1].confidence - 0.7071).abs() < 0.05);
        assert_eq!(ranked[2].name, "Portrait");
        assert!(ranked[2].confidence < 0.05);
    }
}
