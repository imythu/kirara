//! Numerical kernels: BitLinear forward + embedding lookup (feature-gated).
//!
//! `bitlinear_forward` is the load-bearing math — it MUST match
//! `training/pack/unpack.py::bitlinear_forward` within the per-format
//! tolerance documented in `docs/tern-inference-engine.md`. The Python
//! function is the reference; the Rust function below is the implementation.
//!
//! Embedding lookup is feature-gated: each build compiles in exactly one
//! `embedding_lookup` matching its embedding-format Cargo feature.
//!
//! Performance note: this is the v1 scalar implementation. SIMD acceleration
//! lives in `docs/tern-runtime-perf.md` → Phase B. Per-call allocations
//! (LN buffer, x_quant buffer) are acceptable at the current performance
//! target; the scratch-buffer-reuse rewrite is Phase C1 in the same doc.

// ═════════════════════════════════════════════════════════════════════════════
// BitLinear forward — NOT feature-gated (BitLinear weights are always ternary)
// ═════════════════════════════════════════════════════════════════════════════

const BITLINEAR_EPS: f32 = 1e-5;
const ACTIVATION_RANGE_MAX: f32 = 128.0; // max(|range|) for activation_range = (-128, 127)

/// Engine-equivalent BitLinear forward, mirroring `bitlinear==2.4.6`'s
/// `BitLinear.forward()` with the model's training-time defaults:
///   weight_range = (-1, 1), weight_measure = AbsMedian
///   activation_range = (-128, 127), activation_measure = AbsMax (per-token)
///   norm = parameterless LayerNorm (default eps = 1e-5)
///   strategy = round_clamp, lambda = 1 (inference)
///
/// Math reference: `training/pack/unpack.py::bitlinear_forward`.
///
/// Weights are pre-unpacked from 2-bit packed → `i8 {-1, 0, +1}` once at engine
/// init (see [Phase A3] in `docs/tern-runtime-perf.md`). The inner loop is then
/// a clean `i8 × i8 → i32` MAC with no per-element branching — both faster on
/// scalar (no unpredictable branch on the 33%/33%/33% ternary distribution)
/// and ready for the SIMD lane shape used by Phase B2 (`i16x8_extmul_low_i8x16`).
///
/// # Arguments
/// - `x`:           `[n_rows × in_features]` fp32 input rows
/// - `w_unpacked`:  `[out_features × in_features]` row-major `i8` ternary weights
///                  (values in `{-1, 0, +1}`)
/// - `w_scale`:     fp32 scale from the packer (one per matrix; AbsMedian-derived)
/// - `bias`:        optional fp32 bias vector of length `out_features`. None for Q/K/V.
/// - `n_rows`:      number of input rows to process
/// - `in_features`
/// - `out_features`
/// - `out`:         `[n_rows × out_features]` output buffer, fp32
pub fn bitlinear_forward(
    x: &[f32],
    w_unpacked: &[i8],
    w_scale: f32,
    bias: Option<&[f32]>,
    n_rows: usize,
    in_features: usize,
    out_features: usize,
    out: &mut [f32],
) {
    // x and out may be larger than n_rows * stride — callers (post Phase A1)
    // pass max-sized scratch buffers but only ask us to process the active
    // prefix. We only touch the first n_rows * stride elements either way.
    debug_assert!(x.len() >= n_rows * in_features);
    debug_assert!(out.len() >= n_rows * out_features);
    debug_assert_eq!(w_unpacked.len(), out_features * in_features);

    // Scratch buffers reused across output features within a row. Per-row
    // allocations are the v1 trade-off; revisit if per-query latency is tight.
    let mut x_norm = vec![0f32; in_features];
    let mut x_quant = vec![0i8; in_features];

    for row_idx in 0..n_rows {
        let x_row = &x[row_idx * in_features..(row_idx + 1) * in_features];
        let out_row = &mut out[row_idx * out_features..(row_idx + 1) * out_features];

        // 1) Parameterless LayerNorm over the last dim.
        layer_norm_(x_row, &mut x_norm);

        // 2) Per-token AbsMax → x_scale.
        let mut max_abs: f32 = BITLINEAR_EPS;
        for &v in x_norm.iter() {
            let av = v.abs();
            if av > max_abs {
                max_abs = av;
            }
        }
        let x_scale: f32 = ACTIVATION_RANGE_MAX / max_abs;

        // 3) Quantize activations to int8 (round + clamp; STE inactive at inference).
        for i in 0..in_features {
            let q = (x_norm[i] * x_scale).round().clamp(-128.0, 127.0);
            x_quant[i] = q as i8;
        }

        // 4) Matmul (i8 × i8 → i32) + optional bias (fp32) + rescale.
        //    Bias is added in pre-rescale space, matching F.linear semantics in the
        //    Python reference: y = (matmul + bias) / (w_scale * x_scale).
        let rescale = 1.0 / (w_scale * x_scale);

        for j in 0..out_features {
            let w_row = &w_unpacked[j * in_features..(j + 1) * in_features];
            let acc = bitlinear_inner_simd(&x_quant, w_row, in_features);
            let mut y = acc as f32;
            if let Some(b) = bias {
                y += b[j];
            }
            out_row[j] = y * rescale;
        }
    }
}

// ── SIMD inner dot product (Phase B2) ───────────────────────────────────────
//
// `i8 × i8 → i32` dot over `in_features` using WASM 128-bit SIMD. Replaces the
// scalar inner loop with 16 lanes of i8 multiply-accumulate per iteration:
//
//   v128_load                       — load 16 i8 lanes of x_quant and of w_row
//   i16x8_extmul_low_i8x16          — widening multiply, low 8 lanes  → i16x8
//   i16x8_extmul_high_i8x16         — widening multiply, high 8 lanes → i16x8
//   i32x4_extadd_pairwise_i16x8     — pairwise sum i16x8 → i32x4
//   i32x4_add                       — accumulate
//
// Accumulator overflow: max activation magnitude is 128 (post-quant clamp);
// max in_features in this model is 1024. Worst case |acc| ≤ 128 × 1024 = 131072,
// well inside i32 range and 24-bit fp32 mantissa range, so `acc as f32` is exact.
//
// Required size: `in_features % 16 == 0`. Our model has d_model=256, ffn_dim=1024
// — both multiples of 16. The scalar tail loop handles any leftover and is dead
// code at current config but kept for safety against future dim changes.

#[cfg(target_arch = "wasm32")]
#[inline]
fn bitlinear_inner_simd(x_quant: &[i8], w_row: &[i8], in_features: usize) -> i32 {
    use core::arch::wasm32::*;

    debug_assert!(x_quant.len() >= in_features);
    debug_assert!(w_row.len() >= in_features);

    let mut acc = i32x4_splat(0);
    let mut i = 0usize;

    while i + 16 <= in_features {
        // SAFETY: bounds checked above; v128_load reads 16 bytes from an i8
        // pointer with no alignment requirement on WASM.
        let x_vec: v128 = unsafe { v128_load(x_quant.as_ptr().add(i) as *const v128) };
        let w_vec: v128 = unsafe { v128_load(w_row.as_ptr().add(i) as *const v128) };

        let lo = i16x8_extmul_low_i8x16(x_vec, w_vec);
        let hi = i16x8_extmul_high_i8x16(x_vec, w_vec);
        acc = i32x4_add(acc, i32x4_extadd_pairwise_i16x8(lo));
        acc = i32x4_add(acc, i32x4_extadd_pairwise_i16x8(hi));
        i += 16;
    }

    // Horizontal sum: i32x4 → scalar
    let mut buf = [0i32; 4];
    // SAFETY: buf is 4 × 4 = 16 bytes, matching v128 width.
    unsafe { v128_store(buf.as_mut_ptr() as *mut v128, acc) };
    let mut total = buf[0] + buf[1] + buf[2] + buf[3];

    // Scalar tail (dead code at current model dims; guards against future shape changes)
    while i < in_features {
        total += (x_quant[i] as i32) * (w_row[i] as i32);
        i += 1;
    }
    total
}

// Native fallback so the rlib build (used by future tests) still compiles.
// Never on the ship path — wasm-pack always emits wasm32, where the SIMD
// path above is taken.
#[cfg(not(target_arch = "wasm32"))]
#[inline]
fn bitlinear_inner_simd(x_quant: &[i8], w_row: &[i8], in_features: usize) -> i32 {
    let mut acc: i32 = 0;
    for i in 0..in_features {
        acc += (x_quant[i] as i32) * (w_row[i] as i32);
    }
    acc
}

/// Parameterless LayerNorm: `(x - mean) / sqrt(var + eps)`, biased variance.
/// Matches `torch.layer_norm(x, [n])` with default eps=1e-5.
fn layer_norm_(input: &[f32], output: &mut [f32]) {
    let n = input.len();
    debug_assert!(n > 0);
    let inv_n = 1.0 / n as f32;
    let mean: f32 = input.iter().sum::<f32>() * inv_n;
    let var: f32 = input
        .iter()
        .map(|&x| {
            let d = x - mean;
            d * d
        })
        .sum::<f32>()
        * inv_n;
    let inv_std = 1.0 / (var + BITLINEAR_EPS).sqrt();
    for i in 0..n {
        output[i] = (input[i] - mean) * inv_std;
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// Embedding lookup — feature-gated, one variant per build
// ═════════════════════════════════════════════════════════════════════════════
//
// Signature is intentionally NOT uniform across formats — each has different
// input slices (fp32 table; or i8 weights + fp32 scales; or packed bytes +
// scales). Callers from inference.rs choose the right call shape via the
// active feature flag.

/// fp32 embedding lookup. Pure gather — read 4 × d_model bytes per token id
/// as little-endian f32 and copy into `out`.
#[cfg(feature = "emb_fp32")]
pub fn embedding_lookup(
    ids: &[u32],
    table: &[u8], // raw fp32 bytes, row-major
    d_model: usize,
    out: &mut [f32],
) {
    debug_assert_eq!(out.len(), ids.len() * d_model);
    let row_bytes = d_model * 4;
    for (i, &id) in ids.iter().enumerate() {
        let src = &table[(id as usize) * row_bytes..(id as usize + 1) * row_bytes];
        let dst = &mut out[i * d_model..(i + 1) * d_model];
        for (k, chunk) in src.chunks_exact(4).enumerate() {
            dst[k] = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        }
    }
}

/// int8 embedding lookup with per-row fp32 scale. `dst[k] = (row[k] as f32) * scale`.
#[cfg(feature = "emb_int8")]
pub fn embedding_lookup(
    ids: &[u32],
    weights: &[i8], // [vocab_size × d_model] row-major
    scales: &[f32], // [vocab_size]
    d_model: usize,
    out: &mut [f32],
) {
    debug_assert_eq!(out.len(), ids.len() * d_model);
    debug_assert_eq!(weights.len(), scales.len() * d_model);
    for (i, &id) in ids.iter().enumerate() {
        let id_u = id as usize;
        let row = &weights[id_u * d_model..(id_u + 1) * d_model];
        let scale = scales[id_u];
        let dst = &mut out[i * d_model..(i + 1) * d_model];
        for k in 0..d_model {
            dst[k] = (row[k] as f32) * scale;
        }
    }
}

/// Ternary embedding lookup with per-row fp32 scale.
/// Codes: 00=zero, 01=+1, 10=-1, 11=reserved/pad. Lower bits = lower index.
/// `bytes_per_row = d_model / 4` (2 bits per element, 4 per byte).
#[cfg(feature = "emb_ternary")]
pub fn embedding_lookup(
    ids: &[u32],
    packed: &[u8],  // [vocab_size × (d_model / 4)] row-major
    scales: &[f32], // [vocab_size]
    d_model: usize,
    out: &mut [f32],
) {
    debug_assert_eq!(out.len(), ids.len() * d_model);
    debug_assert_eq!(d_model % 4, 0);
    let bytes_per_row = d_model / 4;
    debug_assert_eq!(packed.len(), scales.len() * bytes_per_row);
    for (i, &id) in ids.iter().enumerate() {
        let id_u = id as usize;
        let row = &packed[id_u * bytes_per_row..(id_u + 1) * bytes_per_row];
        let scale = scales[id_u];
        let dst = &mut out[i * d_model..(i + 1) * d_model];
        for (byte_idx, &byte) in row.iter().enumerate() {
            let base = byte_idx * 4;
            for sub in 0..4 {
                let code = (byte >> (sub * 2)) & 0b11;
                dst[base + sub] = match code {
                    0b00 => 0.0,
                    0b01 => scale,
                    0b10 => -scale,
                    _ => 0.0, // reserved/pad — should not appear in valid `.bin`
                };
            }
        }
    }
}

/// int4 embedding lookup with per-row fp32 scale.
/// Two values per byte: lower nibble = element 2k, upper = 2k+1.
/// Signed symmetric range stored as [-7, +7] (-8 excluded per packer); sign-extend the nibble.
#[cfg(feature = "emb_int4")]
pub fn embedding_lookup(
    ids: &[u32],
    packed: &[u8],  // [vocab_size × (d_model / 2)] row-major
    scales: &[f32], // [vocab_size]
    d_model: usize,
    out: &mut [f32],
) {
    debug_assert_eq!(out.len(), ids.len() * d_model);
    debug_assert_eq!(d_model % 2, 0);
    let bytes_per_row = d_model / 2;
    debug_assert_eq!(packed.len(), scales.len() * bytes_per_row);
    for (i, &id) in ids.iter().enumerate() {
        let id_u = id as usize;
        let row = &packed[id_u * bytes_per_row..(id_u + 1) * bytes_per_row];
        let scale = scales[id_u];
        let dst = &mut out[i * d_model..(i + 1) * d_model];
        for (byte_idx, &byte) in row.iter().enumerate() {
            let low_n = (byte & 0x0F) as i8;
            let high_n = ((byte >> 4) & 0x0F) as i8;
            // Sign-extend from 4 bits: values 0..7 stay, values 8..15 map to -8..-1.
            // Packer only emits [-7, 7], so we never observe -8 in practice.
            let low_s = if low_n < 8 { low_n } else { low_n - 16 };
            let high_s = if high_n < 8 { high_n } else { high_n - 16 };
            dst[byte_idx * 2] = (low_s as f32) * scale;
            dst[byte_idx * 2 + 1] = (high_s as f32) * scale;
        }
    }
}
