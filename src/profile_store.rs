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
