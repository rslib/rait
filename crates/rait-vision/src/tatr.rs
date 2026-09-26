//! Table Transformer (TATR) table detection: DETR with a ResNet-18 backbone,
//! as published at microsoft/table-transformer-detection.

use crate::kernels::{
    Act, ConvOp, D_MODEL, Dense, HEADS, MaxPool, Norm, Out, add, sgemm, softmax_rows,
};
use candle_core::{Result, Tensor};
use candle_nn::VarBuilder;

const LAYERS: usize = 6;

/// A convolution with its batch norm folded in, run as im2col + one matrix
/// multiply: (O, C*k*k) x (C*k*k, Hout*Wout) is already NCHW.
struct FastConv {
    weight: std::sync::Arc<Vec<f32>>,
    bias: std::sync::Arc<Vec<f32>>,
    o: usize,
    k: usize,
    stride: usize,
    padding: usize,
}

impl FastConv {
    /// Convolution then `relu(out + bias [+ skip])`.
    fn forward(&self, x: &Tensor, relu: bool, skip: Option<&Tensor>) -> Result<Tensor> {
        let op = ConvOp {
            weight: self.weight.clone(),
            bias: self.bias.clone(),
            o: self.o,
            k: self.k,
            stride: self.stride,
            padding: self.padding,
            relu,
        };
        let x = x.contiguous()?;
        match skip {
            Some(skip) => x.apply_op2_no_bwd(&skip.contiguous()?, &op),
            None => x.apply_op1_no_bwd(&op),
        }
    }
}

impl FastConv {
    /// A 1x1 convolution with its own bias, no batch norm.
    fn projection(vb: VarBuilder, i: usize, o: usize) -> Result<Self> {
        Ok(Self {
            weight: std::sync::Arc::new(vb.get((o, i, 1, 1), "weight")?.flatten_all()?.to_vec1()?),
            bias: std::sync::Arc::new(vb.get(o, "bias")?.to_vec1()?),
            o,
            k: 1,
            stride: 1,
            padding: 0,
        })
    }
}

/// A convolution followed by a frozen batch norm, folded into one convolution
/// with bias at load time: w' = w * scale, b' = shift.
fn conv_bn(
    vb: VarBuilder,
    (conv_name, bn_name): (&str, &str),
    (i, o): (usize, usize),
    (k, stride, padding): (usize, usize, usize),
) -> Result<FastConv> {
    let w = vb.get((o, i, k, k), &format!("{conv_name}.weight"))?;
    let bn = vb.pp(bn_name);
    let (gamma, beta) = (bn.get(o, "weight")?, bn.get(o, "bias")?);
    let (mean, var) = (bn.get(o, "running_mean")?, bn.get(o, "running_var")?);
    let scale = (gamma / (var + 1e-5)?.sqrt()?)?;
    let shift = (beta - (&mean * &scale)?)?;
    let w = w.broadcast_mul(&scale.reshape((o, 1, 1, 1))?)?;
    Ok(FastConv {
        weight: std::sync::Arc::new(w.flatten_all()?.to_vec1()?),
        bias: std::sync::Arc::new(shift.to_vec1()?),
        o,
        k,
        stride,
        padding,
    })
}

struct Block {
    conv1: FastConv,
    conv2: FastConv,
    down: Option<FastConv>,
}

impl Block {
    fn new(vb: VarBuilder, i: usize, o: usize, stride: usize) -> Result<Self> {
        let down = if stride != 1 || i != o {
            Some(conv_bn(
                vb.clone(),
                ("downsample.0", "downsample.1"),
                (i, o),
                (1, stride, 0),
            )?)
        } else {
            None
        };
        Ok(Self {
            conv1: conv_bn(vb.clone(), ("conv1", "bn1"), (i, o), (3, stride, 1))?,
            conv2: conv_bn(vb.clone(), ("conv2", "bn2"), (o, o), (3, 1, 1))?,
            down,
        })
    }
    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        let y = self.conv1.forward(x, true, None)?;
        let skip = match &self.down {
            Some(c) => c.forward(x, false, None)?,
            None => x.clone(),
        };
        self.conv2.forward(&y, true, Some(&skip))
    }
}

struct Backbone {
    conv1: FastConv,
    layers: Vec<Block>,
}

impl Backbone {
    fn new(vb: VarBuilder) -> Result<Self> {
        let mut layers = vec![];
        let mut i = 64;
        for (n, o) in [(1, 64), (2, 128), (3, 256), (4, 512)] {
            let stride = if n == 1 { 1 } else { 2 };
            layers.push(Block::new(vb.pp(format!("layer{n}.0")), i, o, stride)?);
            layers.push(Block::new(vb.pp(format!("layer{n}.1")), o, o, 1)?);
            i = o;
        }
        Ok(Self {
            conv1: conv_bn(vb, ("conv1", "bn1"), (3, 64), (7, 2, 3))?,
            layers,
        })
    }
    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        let x = self
            .conv1
            .forward(x, true, None)?
            .apply_op1_no_bwd(&MaxPool)?;
        self.layers.iter().try_fold(x, |x, layer| layer.forward(&x))
    }
}

struct Attention {
    q: Dense,
    k: Dense,
    v: Dense,
    out: Dense,
}

impl Attention {
    fn load(vb: VarBuilder) -> Result<Self> {
        let hd = (D_MODEL / HEADS) as f32;
        Ok(Self {
            q: Dense::load(vb.pp("q_proj"), D_MODEL, D_MODEL, hd.powf(-0.5))?,
            k: Dense::load(vb.pp("k_proj"), D_MODEL, D_MODEL, 1.0)?,
            v: Dense::load(vb.pp("v_proj"), D_MODEL, D_MODEL, 1.0)?,
            out: Dense::load(vb.pp("out_proj"), D_MODEL, D_MODEL, 1.0)?,
        })
    }

    /// `query` and `key` carry position embeddings; `value` does not. Returns
    /// `residual + attention`.
    fn run(
        &self,
        query: &[f32],
        lq: usize,
        key: &[f32],
        value: &[f32],
        lk: usize,
        residual: &[f32],
    ) -> Vec<f32> {
        use rayon::prelude::*;
        let hd = D_MODEL / HEADS;
        let q = self.q.run(query, lq, Act::None);
        let k = self.k.run(key, lk, Act::None);
        let v = self.v.run(value, lk, Act::None);
        let mut ctx = vec![0f32; lq * D_MODEL];
        let out = Out(ctx.as_mut_ptr());
        (0..HEADS)
            .into_par_iter()
            .for_each_init(Vec::new, |scores: &mut Vec<f32>, h| {
                scores.clear();
                scores.resize(lq * lk, 0.0);
                let off = h * hd;
                unsafe {
                    sgemm(
                        lq,
                        lk,
                        hd,
                        q[off..].as_ptr(),
                        D_MODEL,
                        k[off..].as_ptr(),
                        D_MODEL,
                        true,
                        scores.as_mut_ptr(),
                        lk,
                    );
                }
                softmax_rows(scores, lk);
                unsafe {
                    sgemm(
                        lq,
                        hd,
                        lk,
                        scores.as_ptr(),
                        lk,
                        v[off..].as_ptr(),
                        D_MODEL,
                        false,
                        out.at(off),
                        D_MODEL,
                    )
                };
            });
        self.out.run(&ctx, lq, Act::Residual(residual))
    }
}

struct Ffn {
    fc1: Dense,
    fc2: Dense,
    norm: Norm,
}

impl Ffn {
    fn load(vb: &VarBuilder) -> Result<Self> {
        Ok(Self {
            fc1: Dense::load(vb.pp("fc1"), D_MODEL, 2048, 1.0)?,
            fc2: Dense::load(vb.pp("fc2"), 2048, D_MODEL, 1.0)?,
            norm: Norm::load(vb.pp("final_layer_norm"))?,
        })
    }

    fn run(&self, x: &[f32], rows: usize) -> Vec<f32> {
        let hidden = self.fc1.run(&self.norm.run(x), rows, Act::Relu);
        self.fc2.run(&hidden, rows, Act::Residual(x))
    }
}

struct EncoderLayer {
    attn: Attention,
    norm: Norm,
    ffn: Ffn,
}

struct DecoderLayer {
    self_attn: Attention,
    self_norm: Norm,
    cross_attn: Attention,
    cross_norm: Norm,
    ffn: Ffn,
}

pub(crate) struct Tatr {
    backbone: Backbone,
    projection: FastConv,
    encoder: Vec<EncoderLayer>,
    encoder_norm: Norm,
    decoder: Vec<DecoderLayer>,
    decoder_norm: Norm,
    /// Number of object queries: 15 for detection, 125 for structure.
    n_queries: usize,
    queries: Vec<f32>,
    classifier: Dense,
    bbox: Vec<Dense>,
}

impl Tatr {
    /// `classes` counts the labels plus "no object".
    pub(crate) fn new(vb: VarBuilder, n_queries: usize, classes: usize) -> Result<Self> {
        let m = vb.pp("model");
        let encoder = (0..LAYERS)
            .map(|i| {
                let l = m.pp(format!("encoder.layers.{i}"));
                Ok(EncoderLayer {
                    attn: Attention::load(l.pp("self_attn"))?,
                    norm: Norm::load(l.pp("self_attn_layer_norm"))?,
                    ffn: Ffn::load(&l)?,
                })
            })
            .collect::<Result<_>>()?;
        let decoder = (0..LAYERS)
            .map(|i| {
                let l = m.pp(format!("decoder.layers.{i}"));
                Ok(DecoderLayer {
                    self_attn: Attention::load(l.pp("self_attn"))?,
                    self_norm: Norm::load(l.pp("self_attn_layer_norm"))?,
                    cross_attn: Attention::load(l.pp("encoder_attn"))?,
                    cross_norm: Norm::load(l.pp("encoder_attn_layer_norm"))?,
                    ffn: Ffn::load(&l)?,
                })
            })
            .collect::<Result<_>>()?;
        Ok(Self {
            backbone: Backbone::new(m.pp("backbone.conv_encoder.model"))?,
            projection: FastConv::projection(m.pp("input_projection"), 512, D_MODEL)?,
            encoder,
            encoder_norm: Norm::load(m.pp("encoder.layernorm"))?,
            decoder,
            decoder_norm: Norm::load(m.pp("decoder.layernorm"))?,
            queries: m
                .get((n_queries, D_MODEL), "query_position_embeddings.weight")?
                .flatten_all()?
                .to_vec1()?,
            n_queries,
            classifier: Dense::load(vb.pp("class_labels_classifier"), D_MODEL, classes, 1.0)?,
            bbox: (0..3)
                .map(|i| {
                    Dense::load(
                        vb.pp(format!("bbox_predictor.layers.{i}")),
                        D_MODEL,
                        if i == 2 { 4 } else { D_MODEL },
                        1.0,
                    )
                })
                .collect::<Result<_>>()?,
        })
    }

    /// `pixels`: (1, 3, H, W), normalized. Returns logits (queries x classes)
    /// and boxes (queries x 4, cx cy w h relative to the image).
    pub(crate) fn forward(&self, pixels: &Tensor) -> Result<(Vec<f32>, Vec<f32>)> {
        let features = self.backbone.forward(pixels)?;
        let features = self.projection.forward(&features, false, None)?;
        let (_, _, h, w) = features.dims4()?;
        let len = h * w;
        // (256, H*W) -> (H*W, 256)
        let memory: Vec<f32> = features
            .reshape((D_MODEL, len))?
            .t()?
            .contiguous()?
            .flatten_all()?
            .to_vec1()?;
        let pos = sine_positions(h, w);
        let mut x = memory;
        for layer in &self.encoder {
            let hdn = layer.norm.run(&x);
            let qk = add(&hdn, &pos);
            x = layer.attn.run(&qk, len, &qk, &hdn, len, &x);
            x = layer.ffn.run(&x, len);
        }
        let memory = self.encoder_norm.run(&x);
        let keys = add(&memory, &pos);
        let n = self.n_queries;
        let mut y = vec![0f32; n * D_MODEL];
        for layer in &self.decoder {
            let hdn = layer.self_norm.run(&y);
            let qk = add(&hdn, &self.queries);
            y = layer.self_attn.run(&qk, n, &qk, &hdn, n, &y);
            let hdn = layer.cross_norm.run(&y);
            let q = add(&hdn, &self.queries);
            y = layer.cross_attn.run(&q, n, &keys, &memory, len, &y);
            y = layer.ffn.run(&y, n);
        }
        let y = self.decoder_norm.run(&y);
        let logits = self.classifier.run(&y, n, Act::None);
        let b = self.bbox[0].run(&y, n, Act::Relu);
        let b = self.bbox[1].run(&b, n, Act::Relu);
        let boxes = self.bbox[2]
            .run(&b, n, Act::None)
            .into_iter()
            .map(|v| 1.0 / (1.0 + (-v).exp()))
            .collect();
        Ok((logits, boxes))
    }
}

/// DETR's normalized sine position embedding for an all-valid mask: (H*W, 256),
/// y features first, then x, each as interleaved sin/cos pairs.
fn sine_positions(h: usize, w: usize) -> Vec<f32> {
    let half = D_MODEL / 2;
    let scale = 2.0 * std::f64::consts::PI;
    let dim_t: Vec<f64> = (0..half)
        .map(|i| 10000f64.powf(2.0 * (i / 2) as f64 / half as f64))
        .collect();
    let embed = |v: f64| -> Vec<f32> {
        (0..half)
            .map(|i| {
                let a = v / dim_t[i];
                if i % 2 == 0 {
                    a.sin() as f32
                } else {
                    a.cos() as f32
                }
            })
            .collect()
    };
    let mut data = Vec::with_capacity(h * w * D_MODEL);
    for y in 0..h {
        let ye = embed((y + 1) as f64 / (h as f64 + 1e-6) * scale);
        for x in 0..w {
            data.extend_from_slice(&ye);
            data.extend(embed((x + 1) as f64 / (w as f64 + 1e-6) * scale));
        }
    }
    data
}
