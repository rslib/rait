mod kernels;
pub mod models;
mod tatr;

use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use rait_core::{ErrorCode, ErrorInfo};
use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;

pub use models::{DEPENDENCIES, MODELS, ModelSpec, TATR, TATR_STRUCTURE};

/// Longest side, in pixels, of the page image TATR expects.
pub const INPUT_SIZE: u32 = 800;
/// Longest side, in pixels, of the table image the structure model expects.
pub const STRUCTURE_SIZE: u32 = 800;
/// Structure elements below this confidence are dropped.
const MIN_STRUCTURE_SCORE: f32 = 0.5;
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
    /// Load TATR from its safetensors file.
    pub fn load(path: &Path) -> Result<Self, ErrorInfo> {
        Ok(Self {
            model: load_tatr(path, 15, 3)?,
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
        let (logits, boxes) = run(&self.model, rgb, width, height)?;
        Ok(regions(&logits, &boxes))
    }
}

/// What a structure element marks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Label {
    Table,
    Column,
    Row,
    ColumnHeader,
    ProjectedRowHeader,
    SpanningCell,
}

const LABELS: [Label; 6] = [
    Label::Table,
    Label::Column,
    Label::Row,
    Label::ColumnHeader,
    Label::ProjectedRowHeader,
    Label::SpanningCell,
];

/// A structure element as a fraction of the table image.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Element {
    pub label: Label,
    pub region: Region,
}

/// Table structure recognition with Table Transformer: rows, columns, header
/// and spanning cells inside one cropped table.
pub struct TableStructure {
    model: tatr::Tatr,
}

impl TableStructure {
    pub fn load(path: &Path) -> Result<Self, ErrorInfo> {
        Ok(Self {
            model: load_tatr(path, 125, 7)?,
        })
    }

    /// Elements of one table image: interleaved RGB, longest side about
    /// [`STRUCTURE_SIZE`] pixels.
    pub fn recognize(
        &self,
        rgb: &[u8],
        width: usize,
        height: usize,
    ) -> Result<Vec<Element>, ErrorInfo> {
        let (logits, boxes) = run(&self.model, rgb, width, height)?;
        Ok(elements(&logits, &boxes))
    }
}

/// Load TATR weights. The structure model v1.1 names its ResNet layers the
/// Hugging Face way; they are renamed to the timm names the loader uses.
fn load_tatr(path: &Path, queries: usize, classes: usize) -> Result<tatr::Tatr, ErrorInfo> {
    let tensors = candle_core::safetensors::load(path, &Device::Cpu).map_err(model_error)?;
    let tensors: HashMap<String, Tensor> = tensors
        .into_iter()
        .map(|(k, v)| (timm_name(&k), v))
        .collect();
    let vb = VarBuilder::from_tensors(tensors, DType::F32, &Device::Cpu);
    tatr::Tatr::new(vb, queries, classes).map_err(model_error)
}

fn timm_name(name: &str) -> String {
    let Some((prefix, rest)) = name.split_once("conv_encoder.model.") else {
        return name.to_string();
    };
    let part = |p: &str| match p {
        "convolution" => "conv",
        _ => "bn",
    };
    let parts: Vec<&str> = rest.split('.').collect();
    let renamed = match parts.as_slice() {
        ["embedder", "embedder", kind, param] => format!("{}1.{param}", part(kind)),
        ["encoder", "stages", s, "layers", l, "layer", i, kind, param] => {
            let (s, i): (usize, usize) = (s.parse().unwrap_or(0), i.parse().unwrap_or(0));
            format!("layer{}.{l}.{}{}.{param}", s + 1, part(kind), i + 1)
        }
        ["encoder", "stages", s, "layers", l, "shortcut", kind, param] => {
            let s: usize = s.parse().unwrap_or(0);
            let i = if *kind == "convolution" { 0 } else { 1 };
            format!("layer{}.{l}.downsample.{i}.{param}", s + 1)
        }
        _ => rest.to_string(),
    };
    format!("{prefix}conv_encoder.model.{renamed}")
}

fn run(
    model: &tatr::Tatr,
    rgb: &[u8],
    width: usize,
    height: usize,
) -> Result<(Vec<f32>, Vec<f32>), ErrorInfo> {
    if rgb.len() != width * height * 3 {
        return Err(ErrorInfo::new(
            ErrorCode::InternalError,
            "Page image has the wrong size.",
            None,
        ));
    }
    let plane = width * height;
    let mut pixels = vec![0f32; 3 * plane];
    for (i, px) in rgb.as_chunks::<3>().0.iter().enumerate() {
        for c in 0..3 {
            pixels[c * plane + i] = (px[c] as f32 / 255.0 - MEAN[c]) / STD[c];
        }
    }
    let input =
        Tensor::from_vec(pixels, (1, 3, height, width), &Device::Cpu).map_err(model_error)?;
    model.forward(&input).map_err(model_error)
}

fn corners(score: f32, b: &[f32]) -> Region {
    Region {
        score,
        x0: (b[0] - b[2] / 2.0).clamp(0.0, 1.0),
        y0: (b[1] - b[3] / 2.0).clamp(0.0, 1.0),
        x1: (b[0] + b[2] / 2.0).clamp(0.0, 1.0),
        y1: (b[1] + b[3] / 2.0).clamp(0.0, 1.0),
    }
}

fn softmax(logits: &[f32]) -> Vec<f32> {
    let max = logits.iter().copied().fold(f32::MIN, f32::max);
    let e: Vec<f32> = logits.iter().map(|v| (v - max).exp()).collect();
    let sum: f32 = e.iter().sum();
    e.into_iter().map(|v| v / sum).collect()
}

/// Each query's most likely class, as in TATR's own inference: dropped when
/// that is "no object" or below [`MIN_STRUCTURE_SCORE`].
fn elements(logits: &[f32], boxes: &[f32]) -> Vec<Element> {
    logits
        .as_chunks::<{ LABELS.len() + 1 }>()
        .0
        .iter()
        .zip(boxes.as_chunks::<4>().0)
        .filter_map(|(l, b)| {
            let p = softmax(l);
            let (best, &score) = p.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1))?;
            let label = *LABELS.get(best)?;
            (score >= MIN_STRUCTURE_SCORE).then(|| Element {
                label,
                region: corners(score, b),
            })
        })
        .collect()
}

/// Keep queries whose "table" or "table rotated" probability passes
/// [`MIN_SCORE`]; boxes are (cx, cy, w, h) as fractions of the image.
fn regions(logits: &[f32], boxes: &[f32]) -> Vec<Region> {
    logits
        .as_chunks::<3>()
        .0
        .iter()
        .zip(boxes.as_chunks::<4>().0)
        .filter_map(|(l, b)| {
            let p = softmax(l);
            let score = p[0].max(p[1]);
            (score >= MIN_SCORE).then(|| corners(score, b))
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

    #[test]
    fn renames_hugging_face_resnet_to_timm() {
        let p = "model.backbone.conv_encoder.model.";
        for (hf, timm) in [
            ("embedder.embedder.convolution.weight", "conv1.weight"),
            (
                "embedder.embedder.normalization.running_var",
                "bn1.running_var",
            ),
            (
                "encoder.stages.0.layers.1.layer.1.normalization.bias",
                "layer1.1.bn2.bias",
            ),
            (
                "encoder.stages.2.layers.0.layer.0.convolution.weight",
                "layer3.0.conv1.weight",
            ),
            (
                "encoder.stages.3.layers.0.shortcut.convolution.weight",
                "layer4.0.downsample.0.weight",
            ),
            (
                "encoder.stages.3.layers.0.shortcut.normalization.weight",
                "layer4.0.downsample.1.weight",
            ),
            ("layer2.0.conv1.weight", "layer2.0.conv1.weight"),
        ] {
            assert_eq!(timm_name(&format!("{p}{hf}")), format!("{p}{timm}"));
        }
        assert_eq!(
            timm_name("class_labels_classifier.bias"),
            "class_labels_classifier.bias"
        );
    }

    #[test]
    fn structure_keeps_argmax_labels_above_threshold() {
        // Column (index 1) wins; second query is "no object" (index 6).
        let mut logits = vec![0.0; 14];
        logits[1] = 5.0;
        logits[7 + 6] = 5.0;
        let boxes = [0.5, 0.5, 0.2, 1.0, 0.5, 0.5, 1.0, 0.1];
        let found = elements(&logits, &boxes);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].label, Label::Column);
    }
}
