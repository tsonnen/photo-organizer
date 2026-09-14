use anyhow::Result;
use image::DynamicImage;
use ort::session::Session;
use ort::value::Tensor;
use std::path::Path;

/// Initializes an ONNX runtime session from a model file.
pub fn init_clip_session<P: AsRef<Path>>(model_path: P) -> Result<Session> {
    let session = Session::builder()?.commit_from_file(model_path)?;
    Ok(session)
}

/// Preprocesses the image (resizing to 224x224 and ImageNet normalization),
/// evaluates the CLIP visual model, and returns an L2-normalized embedding vector.
pub fn extract_embedding(session: &mut Session, img: &DynamicImage) -> Result<Vec<f32>> {
    const IMAGE_HEIGHT: usize = 224;
    const IMAGE_WIDTH: usize = 224;
    const NUM_CHANNELS: usize = 3;

    let resized = img
        .resize_exact(
            IMAGE_WIDTH.try_into().unwrap(),
            IMAGE_HEIGHT.try_into().unwrap(),
            image::imageops::FilterType::Triangle,
        )
        .to_rgb8();

    let mut data = Vec::with_capacity(NUM_CHANNELS * IMAGE_WIDTH * IMAGE_HEIGHT);
    // Dynamic image is RGB row-major; convert to NCHW format
    for channel in 0..NUM_CHANNELS {
        for y in 0..IMAGE_HEIGHT {
            for x in 0..IMAGE_WIDTH {
                const IMAGENET_MEAN: [f32; 3] = [0.485, 0.456, 0.406];
                const IMAGENET_STD: [f32; 3] = [0.229, 0.224, 0.225];

                let pixel = resized.get_pixel(x.try_into().unwrap(), y.try_into().unwrap());
                let val =
                    pixel[channel] as f32 / 255.0 - IMAGENET_MEAN[channel] / IMAGENET_STD[channel];

                data.push(val);
            }
        }
    }

    // Using shape tuple directly avoids ndarray trait version mismatches
    let input_tensor =
        Tensor::from_array((vec![1usize, NUM_CHANNELS, IMAGE_WIDTH, IMAGE_HEIGHT], data))?;
    let outputs = session.run(ort::inputs!["input" => input_tensor])?;

    // try_extract_tensor returns `(&Shape, &[f32])`. Access raw slice via `.1`
    let output_ref = outputs.get("output").unwrap().try_extract_tensor::<f32>()?;
    let slice = output_ref.1;

    let norm: f32 = slice.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm == 0.0 {
        Ok(slice.to_vec())
    } else {
        Ok(slice.iter().map(|x| x / norm).collect())
    }
}
