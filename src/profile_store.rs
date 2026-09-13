use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CategoryProfile {
    pub name: String,
    pub centroid: Vec<f32>,
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
    pub fn classify(&self, embedding: &[f32]) -> (String, f32) {
        if embedding.is_empty() || self.profiles.is_empty() {
            return ("Unsorted".to_string(), 0.0);
        }

        let mut best_category = "Unsorted".to_string();
        let mut max_similarity = -1.0f32;

        for profile in &self.profiles {
            let sim = cosine_similarity(embedding, &profile.centroid);
            if sim > max_similarity {
                max_similarity = sim;
                if sim >= self.confidence_threshold {
                    best_category = profile.name.clone();
                }
            }
        }

        (best_category, max_similarity.max(0.0))
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
    fn test_classify_empty() {
        let store = ProfileStore::default();
        let (cat, conf) = store.classify(&[]);
        assert_eq!(cat, "Unsorted");
        assert_eq!(conf, 0.0);

        let (cat, conf) = store.classify(&[1.0, 0.0]);
        assert_eq!(cat, "Unsorted");
        assert_eq!(conf, 0.0);
    }

    #[test]
    fn test_classify_matching() {
        let store = ProfileStore {
            profiles: vec![
                CategoryProfile {
                    name: "Landscape".to_string(),
                    centroid: vec![1.0, 0.0, 0.0],
                },
                CategoryProfile {
                    name: "Portrait".to_string(),
                    centroid: vec![0.0, 1.0, 0.0],
                },
            ],
            confidence_threshold: 0.65,
        };

        let (cat, conf) = store.classify(&[0.9, 0.1, 0.0]);
        assert_eq!(cat, "Landscape");
        assert!(conf > 0.65);

        let (cat, conf) = store.classify(&[0.1, 0.9, 0.0]);
        assert_eq!(cat, "Portrait");
        assert!(conf > 0.65);
    }

    #[test]
    fn test_classify_below_threshold() {
        let store = ProfileStore {
            profiles: vec![CategoryProfile {
                name: "Landscape".to_string(),
                centroid: vec![1.0, 0.0, 0.0],
            }],
            confidence_threshold: 0.8,
        };

        // Similarity is 0.5 (below 0.8 threshold)
        let (cat, conf) = store.classify(&[0.5, 0.866, 0.0]);
        assert_eq!(cat, "Unsorted");
        assert!(conf < 0.8);
    }
}
