//! CPU kernels for the vision models: a single-threaded `sgemm` that callers
//! parallelize, a tiled convolution with a fused epilogue, and the fused
//! transformer pieces. candle only loads weights and carries tensors.

use candle_core::Result;
use candle_nn::VarBuilder;

pub(crate) const D_MODEL: usize = 256;
pub(crate) const HEADS: usize = 8;

/// exp(x) for x <= 0 (softmax inputs after subtracting the row max), as a
/// polynomial the compiler vectorizes. Relative error is at most 1.2e-7 on
/// [-1, 0] and 1e-6 down to -20; below that the f32 range reduction lets it
/// grow to 4e-6, on terms under 2e-9 of the row maximum. Range reduction and
/// coefficients follow Cephes `exp2f` (public domain, https://www.netlib.org/cephes/).
#[inline(always)]
pub(crate) fn fast_exp(x: f32) -> f32 {
    let x = x.max(-87.0);
    // exp(x) = 2^i * 2^f with x*log2(e) = i + f, f in [-0.5, 0.5].
    let t = x * std::f32::consts::LOG2_E;
    let i = t.round();
    let f = t - i;
    let p = 1.0
        + f * (6.931_472e-1
            + f * (2.402_264_8e-1
                + f * (5.550_332_5e-2
                    + f * (9.618_437e-3 + f * (1.339_887_4e-3 + f * 1.535_336_2e-4)))));
    f32::from_bits(((i as i32 + 127) << 23) as u32) * p
}

fn f32_ref<'a>(
    storage: &'a candle_core::CpuStorage,
    layout: &candle_core::Layout,
) -> Result<&'a [f32]> {
    match storage {
        candle_core::CpuStorage::F32(v) => {
            let start = layout.start_offset();
            Ok(&v[start..start + layout.shape().elem_count()])
        }
        _ => candle_core::bail!("expected f32"),
    }
}

/// 3x3 max pool, stride 2, padding 1, one channel per task. Padding is
/// ignored rather than read as zero, so this is exact for any input.
pub(crate) struct MaxPool;

impl candle_core::CustomOp1 for MaxPool {
    fn name(&self) -> &'static str {
        "rait-maxpool"
    }

    fn cpu_fwd(
        &self,
        storage: &candle_core::CpuStorage,
        layout: &candle_core::Layout,
    ) -> Result<(candle_core::CpuStorage, candle_core::Shape)> {
        use rayon::prelude::*;
        let src = f32_ref(storage, layout)?;
        let (b, c, h, w) = layout.shape().dims4()?;
        let (ho, wo) = (h.div_ceil(2), w.div_ceil(2));
        let mut dst = vec![0f32; b * c * ho * wo];
        dst.par_chunks_mut(ho * wo)
            .zip(src.par_chunks(h * w))
            .for_each(|(out, plane)| {
                for oy in 0..ho {
                    let y0 = (2 * oy).saturating_sub(1);
                    let y1 = (2 * oy + 1).min(h - 1);
                    for ox in 0..wo {
                        let x0 = (2 * ox).saturating_sub(1);
                        let x1 = (2 * ox + 1).min(w - 1);
                        let mut m = f32::NEG_INFINITY;
                        for y in y0..=y1 {
                            for &v in &plane[y * w + x0..=y * w + x1] {
                                m = m.max(v);
                            }
                        }
                        out[oy * wo + ox] = m;
                    }
                }
            });
        Ok((
            candle_core::CpuStorage::F32(dst),
            candle_core::Shape::from((b, c, ho, wo)),
        ))
    }
}

/// C (m x n, row stride ldc) = A (m x k, row stride lda) * B, where B is
/// (k x n, row stride ldb), or its transpose stored as (n x k) when `bt`.
/// Single-threaded: callers parallelize over tiles, row blocks or heads.
#[cfg(target_os = "macos")]
#[allow(clippy::too_many_arguments)]
pub(crate) unsafe fn sgemm(
    m: usize,
    n: usize,
    k: usize,
    a: *const f32,
    lda: usize,
    b: *const f32,
    ldb: usize,
    bt: bool,
    c: *mut f32,
    ldc: usize,
) {
    #[link(name = "Accelerate", kind = "framework")]
    unsafe extern "C" {
        fn cblas_sgemm(
            order: i32,
            ta: i32,
            tb: i32,
            m: i32,
            n: i32,
            k: i32,
            alpha: f32,
            a: *const f32,
            lda: i32,
            b: *const f32,
            ldb: i32,
            beta: f32,
            c: *mut f32,
            ldc: i32,
        );
    }
    const ROW_MAJOR: i32 = 101;
    const NO_TRANS: i32 = 111;
    const TRANS: i32 = 112;
    let tb = if bt { TRANS } else { NO_TRANS };
    unsafe {
        cblas_sgemm(
            ROW_MAJOR, NO_TRANS, tb, m as i32, n as i32, k as i32, 1.0, a, lda as i32, b,
            ldb as i32, 0.0, c, ldc as i32,
        )
    }
}

#[cfg(not(target_os = "macos"))]
#[allow(clippy::too_many_arguments)]
pub(crate) unsafe fn sgemm(
    m: usize,
    n: usize,
    k: usize,
    a: *const f32,
    lda: usize,
    b: *const f32,
    ldb: usize,
    bt: bool,
    c: *mut f32,
    ldc: usize,
) {
    let (rhs_cs, rhs_rs) = if bt {
        (ldb as isize, 1)
    } else {
        (1, ldb as isize)
    };
    unsafe {
        gemm::gemm(
            m,
            n,
            k,
            c,
            1,
            ldc as isize,
            false,
            a,
            1,
            lda as isize,
            b,
            rhs_cs,
            rhs_rs,
            0.0,
            1.0,
            false,
            false,
            false,
            gemm::Parallelism::None,
        )
    }
}

/// A raw output pointer shared by tiles that write disjoint columns.
#[derive(Clone, Copy)]
pub(crate) struct Out(pub(crate) *mut f32);
unsafe impl Send for Out {}
unsafe impl Sync for Out {}

impl Out {
    /// The pointer `offset` elements in. A method, so closures capture the
    /// whole `Send` wrapper rather than the raw pointer field.
    ///
    /// # Safety
    /// `offset` must stay inside the buffer.
    pub(crate) unsafe fn at(self, offset: usize) -> *mut f32 {
        unsafe { self.0.add(offset) }
    }
}

/// Convolution with folded batch norm, bias, optional residual and ReLU, run as
/// tiles of output rows: each tile builds a small im2col block that stays in
/// cache, multiplies it straight into its output columns and applies the
/// epilogue while those columns are still in cache.
pub(crate) struct ConvOp {
    pub(crate) weight: std::sync::Arc<Vec<f32>>,
    pub(crate) bias: std::sync::Arc<Vec<f32>>,
    pub(crate) o: usize,
    pub(crate) k: usize,
    pub(crate) stride: usize,
    pub(crate) padding: usize,
    pub(crate) relu: bool,
}

/// Bytes of im2col per tile; small enough to stay in a core's cache.
const TILE_BYTES: usize = 1 << 20;

impl ConvOp {
    fn run(
        &self,
        src: &[f32],
        dims: (usize, usize, usize),
        skip: Option<&[f32]>,
    ) -> (Vec<f32>, usize, usize) {
        self.run_tiled(src, dims, skip, TILE_BYTES)
    }

    fn run_tiled(
        &self,
        src: &[f32],
        (c, h, w): (usize, usize, usize),
        skip: Option<&[f32]>,
        tile_bytes: usize,
    ) -> (Vec<f32>, usize, usize) {
        use rayon::prelude::*;
        let (k, s, p, o) = (self.k, self.stride, self.padding, self.o);
        let h_out = (h + 2 * p - k) / s + 1;
        let w_out = (w + 2 * p - k) / s + 1;
        let n = h_out * w_out;
        let kk = c * k * k;
        let direct = k == 1 && s == 1 && p == 0;
        let rows_per_tile = (tile_bytes / (4 * kk * w_out)).max(1);
        let mut dst = vec![0f32; o * n];
        let out = Out(dst.as_mut_ptr());
        let tiles: Vec<(usize, usize)> = (0..h_out)
            .step_by(rows_per_tile)
            .map(|r| (r, (r + rows_per_tile).min(h_out)))
            .collect();
        tiles
            .par_iter()
            .for_each_init(Vec::new, |cols: &mut Vec<f32>, &(r0, r1)| {
                let cols_n = (r1 - r0) * w_out;
                let col0 = r0 * w_out;
                let (b_ptr, ldb) = if direct {
                    (src[col0..].as_ptr(), n)
                } else {
                    cols.clear();
                    cols.resize(kk * cols_n, 0.0);
                    im2col_tile(src, (h, w), (k, s, p), (r0, r1, w_out), cols);
                    (cols.as_ptr(), cols_n)
                };
                unsafe {
                    sgemm(
                        o,
                        cols_n,
                        kk,
                        self.weight.as_ptr(),
                        kk,
                        b_ptr,
                        ldb,
                        false,
                        out.at(col0),
                        n,
                    )
                };
                for ch in 0..o {
                    let row =
                        unsafe { std::slice::from_raw_parts_mut(out.at(ch * n + col0), cols_n) };
                    let b = self.bias[ch];
                    match skip {
                        Some(skip) => {
                            let sk = &skip[ch * n + col0..ch * n + col0 + cols_n];
                            row.iter_mut()
                                .zip(sk)
                                .for_each(|(v, s)| *v = (*v + b + s).max(0.0));
                        }
                        None if self.relu => row.iter_mut().for_each(|v| *v = (*v + b).max(0.0)),
                        None => row.iter_mut().for_each(|v| *v += b),
                    }
                }
            });
        (dst, h_out, w_out)
    }
}

/// im2col for output rows r0..r1 into `cols` (C*k*k rows of (r1-r0)*w_out).
fn im2col_tile(
    src: &[f32],
    (h, w): (usize, usize),
    (k, s, p): (usize, usize, usize),
    (r0, r1, w_out): (usize, usize, usize),
    cols: &mut [f32],
) {
    let cols_n = (r1 - r0) * w_out;
    for (row, out) in cols.chunks_mut(cols_n).enumerate() {
        let (ch, ky, kx) = (row / (k * k), (row / k) % k, row % k);
        let plane = &src[ch * h * w..(ch + 1) * h * w];
        let first = p.saturating_sub(kx).div_ceil(s);
        let last = ((w + p - kx - 1) / s + 1).min(w_out);
        if first >= last {
            continue;
        }
        for oy in r0..r1 {
            let iy = (oy * s + ky) as isize - p as isize;
            if iy < 0 || iy >= h as isize {
                continue;
            }
            let line = &plane[iy as usize * w..(iy as usize + 1) * w];
            let base = (oy - r0) * w_out;
            let seg = &mut out[base + first..base + last];
            let start = first * s + kx - p;
            if s == 1 {
                seg.copy_from_slice(&line[start..start + seg.len()]);
            } else {
                for (o, &v) in seg.iter_mut().zip(line[start..].iter().step_by(s)) {
                    *o = v;
                }
            }
        }
    }
}

impl candle_core::CustomOp1 for ConvOp {
    fn name(&self) -> &'static str {
        "rait-conv"
    }

    fn cpu_fwd(
        &self,
        storage: &candle_core::CpuStorage,
        layout: &candle_core::Layout,
    ) -> Result<(candle_core::CpuStorage, candle_core::Shape)> {
        let (_, c, h, w) = layout.shape().dims4()?;
        let (dst, ho, wo) = self.run(f32_ref(storage, layout)?, (c, h, w), None);
        Ok((
            candle_core::CpuStorage::F32(dst),
            candle_core::Shape::from((1, self.o, ho, wo)),
        ))
    }
}

impl candle_core::CustomOp2 for ConvOp {
    fn name(&self) -> &'static str {
        "rait-conv-residual"
    }

    fn cpu_fwd(
        &self,
        s1: &candle_core::CpuStorage,
        l1: &candle_core::Layout,
        s2: &candle_core::CpuStorage,
        l2: &candle_core::Layout,
    ) -> Result<(candle_core::CpuStorage, candle_core::Shape)> {
        let (_, c, h, w) = l1.shape().dims4()?;
        let (dst, ho, wo) = self.run(f32_ref(s1, l1)?, (c, h, w), Some(f32_ref(s2, l2)?));
        Ok((
            candle_core::CpuStorage::F32(dst),
            candle_core::Shape::from((1, self.o, ho, wo)),
        ))
    }
}

/// Rows per parallel block for dense layers.
const ROW_BLOCK: usize = 32;

/// What to do with a dense layer's output after adding its bias.
#[derive(Clone, Copy)]
pub(crate) enum Act<'a> {
    None,
    Relu,
    Residual(&'a [f32]),
}

/// A linear layer, weights stored transposed (in x out) so y = x W + b is a
/// plain row-major product.
pub(crate) struct Dense {
    w: Vec<f32>,
    b: Vec<f32>,
    i: usize,
    o: usize,
}

impl Dense {
    pub(crate) fn load(vb: VarBuilder, i: usize, o: usize, scale: f32) -> Result<Self> {
        let w = (vb.get((o, i), "weight")?.t()?.contiguous()? * scale as f64)?;
        let b = (vb.get(o, "bias")? * scale as f64)?;
        Ok(Self {
            w: w.flatten_all()?.to_vec1()?,
            b: b.to_vec1()?,
            i,
            o,
        })
    }

    pub(crate) fn run(&self, x: &[f32], rows: usize, act: Act) -> Vec<f32> {
        use rayon::prelude::*;
        let (i, o) = (self.i, self.o);
        let mut y = vec![0f32; rows * o];
        y.par_chunks_mut(ROW_BLOCK * o)
            .enumerate()
            .for_each(|(blk, out)| {
                let r0 = blk * ROW_BLOCK;
                let n = out.len() / o;
                unsafe {
                    sgemm(
                        n,
                        o,
                        i,
                        x[r0 * i..].as_ptr(),
                        i,
                        self.w.as_ptr(),
                        o,
                        false,
                        out.as_mut_ptr(),
                        o,
                    )
                };
                for (r, row) in out.chunks_mut(o).enumerate() {
                    match act {
                        Act::None => row.iter_mut().zip(&self.b).for_each(|(v, b)| *v += b),
                        Act::Relu => row
                            .iter_mut()
                            .zip(&self.b)
                            .for_each(|(v, b)| *v = (*v + b).max(0.0)),
                        Act::Residual(res) => {
                            let res = &res[(r0 + r) * o..(r0 + r + 1) * o];
                            row.iter_mut()
                                .zip(&self.b)
                                .zip(res)
                                .for_each(|((v, b), s)| *v += b + s);
                        }
                    }
                }
            });
        y
    }
}

pub(crate) fn add(a: &[f32], b: &[f32]) -> Vec<f32> {
    a.iter().zip(b).map(|(x, y)| x + y).collect()
}

pub(crate) struct Norm {
    gamma: Vec<f32>,
    beta: Vec<f32>,
}

impl Norm {
    pub(crate) fn load(vb: VarBuilder) -> Result<Self> {
        Ok(Self {
            gamma: vb.get(D_MODEL, "weight")?.to_vec1()?,
            beta: vb.get(D_MODEL, "bias")?.to_vec1()?,
        })
    }

    pub(crate) fn run(&self, x: &[f32]) -> Vec<f32> {
        use rayon::prelude::*;
        let n = D_MODEL;
        let mut y = vec![0f32; x.len()];
        y.par_chunks_mut(n)
            .zip(x.par_chunks(n))
            .for_each(|(out, row)| {
                let mean = row.iter().sum::<f32>() / n as f32;
                let var = row.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / n as f32;
                let inv = 1.0 / (var + 1e-5).sqrt();
                for (((o, &v), &g), &b) in out.iter_mut().zip(row).zip(&self.gamma).zip(&self.beta)
                {
                    *o = (v - mean) * inv * g + b;
                }
            });
        y
    }
}

pub(crate) fn softmax_rows(scores: &mut [f32], n: usize) {
    for row in scores.chunks_mut(n) {
        let max = row.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let mut sum = 0.0;
        for v in row.iter_mut() {
            *v = fast_exp(*v - max);
            sum += *v;
        }
        let inv = 1.0 / sum;
        row.iter_mut().for_each(|v| *v *= inv);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use candle_core::{Device, Tensor};

    fn random(n: usize, seed: u64) -> Vec<f32> {
        let mut x = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (0..n)
            .map(|_| {
                x = x
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                ((x >> 40) as f32 / (1u64 << 24) as f32) * 2.0 - 1.0
            })
            .collect()
    }

    fn close(a: &[f32], b: &[f32]) {
        assert_eq!(a.len(), b.len());
        for (i, (x, y)) in a.iter().zip(b).enumerate() {
            assert!(
                (x - y).abs() <= 1e-4 * (1.0 + y.abs()),
                "index {i}: {x} vs {y}"
            );
        }
    }

    #[test]
    fn sgemm_matches_naive_with_strides_and_transpose() {
        let (m, n, k) = (7, 5, 9);
        let (lda, ldb, ldc) = (k + 3, n + 2, n + 4);
        let a = random(m * lda, 1);
        let b = random(k * ldb, 2);
        let bt = random(n * (k + 1), 3);
        for transposed in [false, true] {
            let mut c = vec![0f32; m * ldc];
            let (bp, bl) = if transposed { (&bt, k + 1) } else { (&b, ldb) };
            unsafe {
                sgemm(
                    m,
                    n,
                    k,
                    a.as_ptr(),
                    lda,
                    bp.as_ptr(),
                    bl,
                    transposed,
                    c.as_mut_ptr(),
                    ldc,
                )
            };
            for i in 0..m {
                for j in 0..n {
                    let want: f32 = (0..k)
                        .map(|t| {
                            a[i * lda + t]
                                * if transposed {
                                    bt[j * (k + 1) + t]
                                } else {
                                    b[t * ldb + j]
                                }
                        })
                        .sum();
                    assert!((c[i * ldc + j] - want).abs() < 1e-4, "{transposed} {i} {j}");
                }
            }
        }
    }

    fn naive_conv(
        x: &[f32],
        (c, h, w): (usize, usize, usize),
        wt: &[f32],
        bias: &[f32],
        o: usize,
        (k, s, p): (usize, usize, usize),
        (skip, relu): (Option<&[f32]>, bool),
    ) -> Vec<f32> {
        let ho = (h + 2 * p - k) / s + 1;
        let wo = (w + 2 * p - k) / s + 1;
        let mut out = vec![0f32; o * ho * wo];
        for oc in 0..o {
            for oy in 0..ho {
                for ox in 0..wo {
                    let mut v = bias[oc];
                    for ic in 0..c {
                        for ky in 0..k {
                            for kx in 0..k {
                                let iy = (oy * s + ky) as isize - p as isize;
                                let ix = (ox * s + kx) as isize - p as isize;
                                if iy >= 0 && ix >= 0 && (iy as usize) < h && (ix as usize) < w {
                                    v += wt[((oc * c + ic) * k + ky) * k + kx]
                                        * x[(ic * h + iy as usize) * w + ix as usize];
                                }
                            }
                        }
                    }
                    let idx = (oc * ho + oy) * wo + ox;
                    if let Some(skip) = skip {
                        v += skip[idx];
                    }
                    out[idx] = if relu || skip.is_some() {
                        v.max(0.0)
                    } else {
                        v
                    };
                }
            }
        }
        out
    }

    #[test]
    fn tiled_conv_matches_naive_for_every_shape_tatr_uses() {
        let (c, h, w, o) = (5, 13, 11, 4);
        let x = random(c * h * w, 4);
        for (k, s, p) in [(7, 2, 3), (3, 1, 1), (3, 2, 1), (1, 2, 0), (1, 1, 0)] {
            let wt = random(o * c * k * k, 5);
            let bias = random(o, 6);
            for tile_bytes in [1, 256, 1 << 20] {
                for relu in [false, true] {
                    let op = ConvOp {
                        weight: std::sync::Arc::new(wt.clone()),
                        bias: std::sync::Arc::new(bias.clone()),
                        o,
                        k,
                        stride: s,
                        padding: p,
                        relu,
                    };
                    let (got, ho, wo) = op.run_tiled(&x, (c, h, w), None, tile_bytes);
                    close(
                        &got,
                        &naive_conv(&x, (c, h, w), &wt, &bias, o, (k, s, p), (None, relu)),
                    );
                    let skip = random(o * ho * wo, 7);
                    let (got, _, _) = op.run_tiled(&x, (c, h, w), Some(&skip), tile_bytes);
                    close(
                        &got,
                        &naive_conv(&x, (c, h, w), &wt, &bias, o, (k, s, p), (Some(&skip), relu)),
                    );
                }
            }
        }
    }

    #[test]
    fn max_pool_matches_naive_including_negatives() {
        let (c, h, w) = (3, 7, 6);
        let x = random(c * h * w, 8);
        let t = Tensor::from_vec(x.clone(), (1, c, h, w), &Device::Cpu).unwrap();
        let got: Vec<f32> = t
            .apply_op1_no_bwd(&MaxPool)
            .unwrap()
            .flatten_all()
            .unwrap()
            .to_vec1()
            .unwrap();
        let (ho, wo) = (h.div_ceil(2), w.div_ceil(2));
        let mut want = vec![f32::NEG_INFINITY; c * ho * wo];
        for ch in 0..c {
            for oy in 0..ho {
                for ox in 0..wo {
                    for dy in 0..3 {
                        for dx in 0..3 {
                            let (iy, ix) = ((2 * oy + dy) as isize - 1, (2 * ox + dx) as isize - 1);
                            if iy >= 0 && ix >= 0 && (iy as usize) < h && (ix as usize) < w {
                                let i = (ch * ho + oy) * wo + ox;
                                want[i] = want[i].max(x[(ch * h + iy as usize) * w + ix as usize]);
                            }
                        }
                    }
                }
            }
        }
        close(&got, &want);
    }

    #[test]
    fn fast_exp_and_softmax_are_accurate() {
        for i in 0..20_000 {
            let x = -(i as f32) * 0.001;
            let (got, want) = (fast_exp(x) as f64, (x as f64).exp());
            let limit = if x >= -1.0 { 1.5e-7 } else { 1.1e-6 };
            assert!(((got - want) / want).abs() <= limit, "{x}: {got} vs {want}");
        }
        let mut rows = random(3 * 10, 9);
        let want: Vec<f32> = rows
            .chunks(10)
            .flat_map(|r| {
                let m = r.iter().copied().fold(f32::MIN, f32::max);
                let e: Vec<f32> = r.iter().map(|v| (v - m).exp()).collect();
                let sum: f32 = e.iter().sum();
                e.into_iter().map(move |v| v / sum)
            })
            .collect();
        softmax_rows(&mut rows, 10);
        close(&rows, &want);
    }
}
