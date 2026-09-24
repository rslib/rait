mod kernels;
pub mod models;
mod tatr;

use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use rait_core::{ErrorCode, ErrorInfo};
use serde::Serialize;
use std::path::Path;

pub use models::{DEPENDENCIES, MODELS, ModelSpec, TATR};

/// Longest side, in pixels, of the page image TATR expects.
pub const INPUT_SIZE: u32 = 800;
/// Detections below this confidence are dropped.
const MIN_SCORE: f32 = 0.9;
const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const STD: [f32; 3] = [0.229, 0.224, 0.225];

/// A detected table as a fraction of the page: 0..1 from the top-left corner.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Region {
    pub score: f32,
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

/// Table detection with Table Transformer. Load once; `detect` is safe to call
/// from many threads.
pub struct TableDetector {
    model: tatr::Tatr,
}

impl TableDetector {
    /// Load TATR from its safetensors file, memory-mapped.
    pub fn load(path: &Path) -> Result<Self, ErrorInfo> {
        // SAFETY: the file is not modified while mapped; rait writes models only
        // through a temporary file and a rename.
        let vb = unsafe { VarBuilder::from_mmaped_safetensors(&[path], DType::F32, &Device::Cpu) }
            .map_err(model_error)?;
        Ok(Self {
            model: tatr::Tatr::new(vb).map_err(model_error)?,
        })
    }

    /// Tables on one page image: interleaved RGB, longest side about
    /// [`INPUT_SIZE`] pixels.
    pub fn detect(
        &self,
        rgb: &[u8],
        width: usize,
        height: usize,
    ) -> Result<Vec<Region>, ErrorInfo> {
        if rgb.len() != width * height * 3 {
            return Err(ErrorInfo::new(
                ErrorCode::InternalError,
                "Page image has the wrong size.",
                None,
            ));
        }
        let plane = width * height;
        let mut pixels = vec![0f32; 3 * plane];
        for (i, px) in rgb.chunks_exact(3).enumerate() {
            for c in 0..3 {
                pixels[c * plane + i] = (px[c] as f32 / 255.0 - MEAN[c]) / STD[c];
            }
        }
        let input =
            Tensor::from_vec(pixels, (1, 3, height, width), &Device::Cpu).map_err(model_error)?;
        let (logits, boxes) = self.model.forward(&input).map_err(model_error)?;
        Ok(regions(&logits, &boxes))
    }
}

/// Keep queries whose "table" or "table rotated" probability passes
/// [`MIN_SCORE`]; boxes are (cx, cy, w, h) as fractions of the image.
fn regions(logits: &[f32], boxes: &[f32]) -> Vec<Region> {
    logits
        .chunks_exact(3)
        .zip(boxes.chunks_exact(4))
        .filter_map(|(l, b)| {
            let max = l.iter().copied().fold(f32::MIN, f32::max);
            let e: Vec<f32> = l.iter().map(|v| (v - max).exp()).collect();
            let score = e[0].max(e[1]) / e.iter().sum::<f32>();
            (score >= MIN_SCORE).then(|| Region {
                score,
                x0: (b[0] - b[2] / 2.0).clamp(0.0, 1.0),
                y0: (b[1] - b[3] / 2.0).clamp(0.0, 1.0),
                x1: (b[0] + b[2] / 2.0).clamp(0.0, 1.0),
                y1: (b[1] + b[3] / 2.0).clamp(0.0, 1.0),
            })
        })
        .collect()
}

fn model_error(err: candle_core::Error) -> ErrorInfo {
    ErrorInfo::new(
        ErrorCode::DependencyFailed,
        "The table detection model could not run.",
        "Run `rait models fetch` to reinstall it.".to_string(),
    )
    .with_detail(err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_confident_tables_as_corner_boxes() {
        let logits = [5.0, -9.0, -3.0, -3.0, -3.0, 4.0, -9.0, 6.0, -3.0];
        let boxes = [0.5, 0.5, 0.2, 0.4, 0.1, 0.1, 0.1, 0.1, 0.95, 0.5, 0.2, 0.2];
        let found = regions(&logits, &boxes);
        assert_eq!(found.len(), 2);
        assert!((found[0].x0 - 0.4).abs() < 1e-6 && (found[0].y1 - 0.7).abs() < 1e-6);
        assert_eq!(found[1].x1, 1.0);
    }
}
