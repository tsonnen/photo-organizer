use anyhow::{Context, Result};
use candle_core::{DType, Device, Module, Tensor, D};
use candle_nn::{linear_no_bias, Linear, VarBuilder};
use candle_transformers::models::clip::vision_model::{ClipVisionConfig, ClipVisionTransformer};
use image::DynamicImage;
use std::path::{Path, PathBuf};

/// Wrapper holding the Candle CLIP vision model and optional visual projection.
pub struct ClipVisionSession {
    vision_model: ClipVisionTransformer,
    visual_projection: Option<Linear>,
    device: Device,
}

// Ensure ClipVisionSession is thread-safe for Rayon parallel scanning
unsafe impl Send for ClipVisionSession {}
unsafe impl Sync for ClipVisionSession {}

/// Searches for the CLIP visual model weights in multiple standard locations.
pub fn find_model_path() -> Option<PathBuf> {
    let candidate_paths = [
        PathBuf::from("models/clip_vision.safetensors"),
        PathBuf::from("models/model.safetensors"),
        PathBuf::from("../models/clip_vision.safetensors"),
        PathBuf::from("src/models/clip_vision.safetensors"),
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("models/clip_vision.safetensors")))
            .unwrap_or_default(),
    ];

    for candidate in &candidate_paths {
        if !candidate.as_os_str().is_empty() && candidate.exists() {
            return Some(candidate.clone());
        }
    }
    None
}

/// Checks if a valid CLIP visual model is reachable.
pub fn is_model_available() -> bool {
    find_model_path().is_some()
}

/// Initializes a Candle CLIP vision session from a SafeTensors model file.
pub fn init_clip_session<P: AsRef<Path>>(model_path: P) -> Result<ClipVisionSession> {
    let p = model_path.as_ref();
    if !p.exists() {
        return Err(anyhow::anyhow!("Model file does not exist: {:?}", p));
    }

    let device = Device::Cpu;
    let config = ClipVisionConfig::vit_base_patch32();

    let vb = unsafe {
        VarBuilder::from_mmaped_safetensors(&[p], DType::F32, &device)
            .context("Failed to map SafeTensors weights")?
    };

    // Support standard HF checkpoints where vision weights are under `vision_model`
    let vb_vision = if vb.contains_tensor("vision_model.embeddings.class_embedding") {
        vb.pp("vision_model")
    } else {
        vb.clone()
    };

    let vision_model = ClipVisionTransformer::new(vb_vision, &config)
        .context("Failed to initialize Candle CLIP vision transformer")?;

    let visual_projection = if vb.contains_tensor("visual_projection.weight") {
        Some(linear_no_bias(
            config.embed_dim,
            config.projection_dim,
            vb.pp("visual_projection"),
        )?)
    } else {
        None
    };

    Ok(ClipVisionSession {
        vision_model,
        visual_projection,
        device,
    })
}

/// Preprocesses the image (resizing to 224x224 and ImageNet normalization),
/// evaluates the CLIP visual model, and returns an L2-normalized embedding vector.
pub fn extract_embedding(session: &ClipVisionSession, img: &DynamicImage) -> Result<Vec<f32>> {
    const IMAGE_HEIGHT: usize = 224;
    const IMAGE_WIDTH: usize = 224;
    const NUM_CHANNELS: usize = 3;

    let resized = img
        .resize_exact(
            IMAGE_WIDTH as u32,
            IMAGE_HEIGHT as u32,
            image::imageops::FilterType::Triangle,
        )
        .to_rgb8();

    let mut data = Vec::with_capacity(NUM_CHANNELS * IMAGE_WIDTH * IMAGE_HEIGHT);
    const IMAGENET_MEAN: [f32; 3] = [0.485, 0.456, 0.406];
    const IMAGENET_STD: [f32; 3] = [0.229, 0.224, 0.225];

    // Dynamic image is RGB row-major; convert to NCHW format [1, 3, 224, 224]
    for channel in 0..NUM_CHANNELS {
        for y in 0..IMAGE_HEIGHT {
            for x in 0..IMAGE_WIDTH {
                let pixel = resized.get_pixel(x as u32, y as u32);
                let val = (pixel[channel] as f32 / 255.0 - IMAGENET_MEAN[channel])
                    / IMAGENET_STD[channel];
                data.push(val);
            }
        }
    }

    let input_tensor =
        Tensor::from_vec(data, (1, NUM_CHANNELS, IMAGE_HEIGHT, IMAGE_WIDTH), &session.device)?;

    let mut embedding = session.vision_model.forward(&input_tensor)?;
    if let Some(ref proj) = session.visual_projection {
        embedding = proj.forward(&embedding)?;
    }

    // Compute L2 norm: v / ||v||_2
    let l2_norm = embedding.sqr()?.sum_keepdim(D::Minus1)?.sqrt()?;
    let normalized = embedding.broadcast_div(&l2_norm)?;

    let flat_vec: Vec<f32> = normalized.flatten_all()?.to_vec1()?;
    Ok(flat_vec)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_model_path_nonexistent() {
        let _ = is_model_available();
    }

    #[test]
    fn test_init_clip_session_missing_error() {
        let res = init_clip_session("nonexistent_model_file_12345.safetensors");
        assert!(res.is_err());
    }
}
