use anyhow::{Context, Result};
use candle_core::{DType, Device, IndexOp, Module, Tensor, D};
use candle_nn::{
    conv2d_no_bias, layer_norm, linear, linear_no_bias, ops::softmax_last_dim, Conv2dConfig,
    LayerNorm, Linear, VarBuilder,
};
use candle_transformers::models::clip::vision_model::{ClipVisionConfig, ClipVisionTransformer};
use image::DynamicImage;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// OpenCLIP Visual Transformer Implementation
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct OpenClipAttention {
    q_proj: Linear,
    k_proj: Linear,
    v_proj: Linear,
    out_proj: Linear,
    head_dim: usize,
    scale: f64,
    num_attention_heads: usize,
}

impl OpenClipAttention {
    fn new(vs: VarBuilder, embed_dim: usize, num_attention_heads: usize) -> Result<Self> {
        let in_proj_weights = vs
            .get((embed_dim * 3, embed_dim), "in_proj_weight")?
            .chunk(3, 0)?;
        let (q_w, k_w, v_w) = (&in_proj_weights[0], &in_proj_weights[1], &in_proj_weights[2]);

        let (q_b, k_b, v_b) = if vs.contains_tensor("in_proj_bias") {
            let in_proj_biases = vs.get(embed_dim * 3, "in_proj_bias")?.chunk(3, 0)?;
            (
                Some(in_proj_biases[0].clone()),
                Some(in_proj_biases[1].clone()),
                Some(in_proj_biases[2].clone()),
            )
        } else {
            (None, None, None)
        };

        let q_proj = Linear::new(q_w.clone(), q_b);
        let k_proj = Linear::new(k_w.clone(), k_b);
        let v_proj = Linear::new(v_w.clone(), v_b);
        let out_proj = linear(embed_dim, embed_dim, vs.pp("out_proj"))?;
        let head_dim = embed_dim / num_attention_heads;
        let scale = (head_dim as f64).powf(-0.5);

        Ok(Self {
            q_proj,
            k_proj,
            v_proj,
            out_proj,
            head_dim,
            scale,
            num_attention_heads,
        })
    }

    fn shape_multihead(&self, xs: &Tensor, bsz: usize, seq_len: usize) -> Result<Tensor> {
        let res = xs
            .reshape((bsz, seq_len, self.num_attention_heads, self.head_dim))?
            .transpose(1, 2)?
            .contiguous()?
            .to_dtype(DType::F32)?;
        Ok(res)
    }

    fn forward(&self, xs: &Tensor) -> Result<Tensor> {
        let in_dtype = xs.dtype();
        let (bsz, seq_len, embed_dim) = xs.dims3()?;

        let q = self.shape_multihead(&self.q_proj.forward(xs)?, bsz, seq_len)?;
        let k = self.shape_multihead(&self.k_proj.forward(xs)?, bsz, seq_len)?;
        let v = self.shape_multihead(&self.v_proj.forward(xs)?, bsz, seq_len)?;
        let q = (q * self.scale)?;

        let attn_weights = q.matmul(&k.transpose(D::Minus1, D::Minus2)?)?;
        let attn_weights = softmax_last_dim(&attn_weights)?;
        let attn_output = attn_weights.matmul(&v)?.to_dtype(in_dtype)?;
        let attn_output = attn_output
            .transpose(1, 2)?
            .contiguous()?
            .reshape((bsz, seq_len, embed_dim))?;
        let out = self.out_proj.forward(&attn_output)?;
        Ok(out)
    }
}

#[derive(Clone, Debug)]
struct OpenClipMlp {
    c_fc: Linear,
    c_proj: Linear,
}

impl OpenClipMlp {
    fn new(vs: VarBuilder, embed_dim: usize, intermediate_size: usize) -> Result<Self> {
        let c_fc = linear(embed_dim, intermediate_size, vs.pp("c_fc"))?;
        let c_proj = linear(intermediate_size, embed_dim, vs.pp("c_proj"))?;
        Ok(Self { c_fc, c_proj })
    }

    fn forward(&self, xs: &Tensor) -> Result<Tensor> {
        let xs = self.c_fc.forward(xs)?;
        // QuickGELU approximation x * sigmoid(1.702 * x) or GELU erf
        let gelu = (xs.clone() * candle_nn::ops::sigmoid(&(xs * 1.702)?)?)?;
        let out = self.c_proj.forward(&gelu)?;
        Ok(out)
    }
}

#[derive(Clone, Debug)]
struct OpenClipResBlock {
    ln_1: LayerNorm,
    attn: OpenClipAttention,
    ln_2: LayerNorm,
    mlp: OpenClipMlp,
}

impl OpenClipResBlock {
    fn new(
        vs: VarBuilder,
        embed_dim: usize,
        num_attention_heads: usize,
        intermediate_size: usize,
    ) -> Result<Self> {
        let ln_1 = layer_norm(embed_dim, 1e-5, vs.pp("ln_1"))?;
        let attn = OpenClipAttention::new(vs.pp("attn"), embed_dim, num_attention_heads)?;
        let ln_2 = layer_norm(embed_dim, 1e-5, vs.pp("ln_2"))?;
        let mlp = OpenClipMlp::new(vs.pp("mlp"), embed_dim, intermediate_size)?;
        Ok(Self {
            ln_1,
            attn,
            ln_2,
            mlp,
        })
    }

    fn forward(&self, xs: &Tensor) -> Result<Tensor> {
        let residual = xs;
        let xs = self.ln_1.forward(xs)?;
        let xs = self.attn.forward(&xs)?;
        let xs = (xs + residual)?;

        let residual = &xs;
        let xs = self.ln_2.forward(&xs)?;
        let xs = self.mlp.forward(&xs)?;
        let out = (xs + residual)?;
        Ok(out)
    }
}

#[derive(Clone, Debug)]
pub struct OpenClipVisionTransformer {
    conv1: candle_nn::Conv2d,
    class_embedding: Tensor,
    positional_embedding: Tensor,
    ln_pre: LayerNorm,
    resblocks: Vec<OpenClipResBlock>,
    ln_post: LayerNorm,
    proj: Option<Tensor>,
}

impl OpenClipVisionTransformer {
    pub fn new(vs: VarBuilder) -> Result<Self> {
        const EMBED_DIM: usize = 768;
        const NUM_HEADS: usize = 12;
        const INTERMEDIATE_SIZE: usize = 3072;
        const NUM_LAYERS: usize = 12;
        const PATCH_SIZE: usize = 32;

        let conv2d_cfg = Conv2dConfig {
            stride: PATCH_SIZE,
            ..Default::default()
        };
        let conv1 = conv2d_no_bias(3, EMBED_DIM, PATCH_SIZE, conv2d_cfg, vs.pp("conv1"))?;
        let class_embedding = vs.get(EMBED_DIM, "class_embedding")?;
        let positional_embedding = vs.get((50, EMBED_DIM), "positional_embedding")?;
        let ln_pre = layer_norm(EMBED_DIM, 1e-5, vs.pp("ln_pre"))?;

        let resblocks_vb = vs.pp("transformer").pp("resblocks");
        let mut resblocks = Vec::with_capacity(NUM_LAYERS);
        for i in 0..NUM_LAYERS {
            let block = OpenClipResBlock::new(
                resblocks_vb.pp(i.to_string()),
                EMBED_DIM,
                NUM_HEADS,
                INTERMEDIATE_SIZE,
            )?;
            resblocks.push(block);
        }

        let ln_post = layer_norm(EMBED_DIM, 1e-5, vs.pp("ln_post"))?;
        let proj = if vs.contains_tensor("proj") {
            Some(vs.get((EMBED_DIM, 512), "proj")?)
        } else {
            None
        };

        Ok(Self {
            conv1,
            class_embedding,
            positional_embedding,
            ln_pre,
            resblocks,
            ln_post,
            proj,
        })
    }

    pub fn forward(&self, pixel_values: &Tensor) -> Result<Tensor> {
        let (bsz, _channels, _h, _w) = pixel_values.dims4()?;
        let x = self.conv1.forward(pixel_values)?;
        let x = x.flatten_from(2)?.transpose(1, 2)?; // [bsz, 49, 768]

        let class_emb = self
            .class_embedding
            .reshape((1, 1, 768))?
            .expand((bsz, 1, 768))?;
        let x = Tensor::cat(&[&class_emb, &x], 1)?; // [bsz, 50, 768]
        let x = x.broadcast_add(&self.positional_embedding)?;
        let mut x = self.ln_pre.forward(&x)?;

        for block in &self.resblocks {
            x = block.forward(&x)?;
        }

        let x = self.ln_post.forward(&x)?;
        let cls_token = x.i((.., 0, ..))?; // [bsz, 768]

        if let Some(ref proj) = self.proj {
            let res = cls_token.matmul(proj)?;
            Ok(res)
        } else {
            Ok(cls_token)
        }
    }
}

// ---------------------------------------------------------------------------
// Unified Session
// ---------------------------------------------------------------------------

enum ModelBackend {
    HuggingFace {
        vision_model: ClipVisionTransformer,
        visual_projection: Option<Linear>,
    },
    OpenClip(OpenClipVisionTransformer),
}

/// Wrapper holding the Candle CLIP vision model and execution device.
pub struct ClipVisionSession {
    backend: ModelBackend,
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
        PathBuf::from("src/models/model.safetensors"),
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

/// Initializes a Candle CLIP vision session from a SafeTensors model file (supporting HF and OpenCLIP).
pub fn init_clip_session<P: AsRef<Path>>(model_path: P) -> Result<ClipVisionSession> {
    let p = model_path.as_ref();
    if !p.exists() {
        return Err(anyhow::anyhow!("Model file does not exist: {:?}", p));
    }

    let device = Device::Cpu;

    let vb = unsafe {
        VarBuilder::from_mmaped_safetensors(&[p], DType::F32, &device)
            .context("Failed to map SafeTensors weights")?
    };

    // Check if OpenCLIP format (e.g. `visual.conv1.weight`)
    if vb.contains_tensor("visual.conv1.weight") || vb.contains_tensor("conv1.weight") {
        let vb_visual = if vb.contains_tensor("visual.conv1.weight") {
            vb.pp("visual")
        } else {
            vb.clone()
        };
        let open_clip = OpenClipVisionTransformer::new(vb_visual)
            .context("Failed to initialize OpenCLIP Vision Transformer")?;
        return Ok(ClipVisionSession {
            backend: ModelBackend::OpenClip(open_clip),
            device,
        });
    }

    // Standard Hugging Face format
    let config = ClipVisionConfig::vit_base_patch32();
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
        backend: ModelBackend::HuggingFace {
            vision_model,
            visual_projection,
        },
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

    let embedding = match &session.backend {
        ModelBackend::OpenClip(model) => model.forward(&input_tensor)?,
        ModelBackend::HuggingFace {
            vision_model,
            visual_projection,
        } => {
            let mut emb = vision_model.forward(&input_tensor)?;
            if let Some(ref proj) = visual_projection {
                emb = proj.forward(&emb)?;
            }
            emb
        }
    };

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

    #[test]
    fn test_init_clip_session_real_file() {
        if let Some(path) = find_model_path() {
            println!("Testing real model load from {:?}", path);
            let session = init_clip_session(&path).expect("Failed to initialize clip session");
            let dummy_img = DynamicImage::new_rgb8(100, 100);
            let emb = extract_embedding(&session, &dummy_img).expect("Failed to extract embedding");
            assert_eq!(emb.len(), 512);
            let norm: f32 = emb.iter().map(|x| x * x).sum::<f32>().sqrt();
            println!("Embedding extracted with length {} and norm {:.6}", emb.len(), norm);
            assert!((norm - 1.0).abs() < 1e-4);
        }
    }
}
