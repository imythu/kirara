//! `.bin` parsing → `WeightLayout` + `RuntimeWeights`, both precomputed once via `OnceLock`.
//!
//! The model bytes are embedded into the WASM at compile time. At engine init we:
//!   1. Verify trailing SHA256
//!   2. Parse the header
//!   3. Walk the wire format to compute byte offsets (`WeightLayout`)
//!   4. Pre-decode the small fp32 sections (LN params, scales, biases, projection)
//!      into typed `Vec<f32>` (`RuntimeWeights`) so the hot path can use them
//!      without re-parsing bytes on every query.
//!   5. Pre-unpack the BitLinear ternary weights from 2-bit packed → `i8` so
//!      the matmul inner loop is branchless (see [Phase A3] in
//!      `docs/tern-runtime-perf.md`). +4× memory (~384 KB → ~1.5 MB total for
//!      our 2-layer config), comfortably L2-resident on all targets.
//!
//! The embedding table stays as byte offsets and is read on demand inside
//! `kernels::embedding_lookup` — too large to keep duplicated alongside the
//! `include_bytes!` copy, and the lookup itself is essentially a gather, not
//! a hot inner loop.
//!
//! Layout walk mirrors the section order documented in
//! `docs/tern-inference-engine.md` and produced by `training/pack/pack.py`.

use std::sync::OnceLock;

use crate::format::{
    self, Header, EMB_FP32, EMB_INT4, EMB_INT8, EMB_TERNARY, HEADER_SIZE, SHA256_SIZE,
};

/// The packed `.bin`, embedded at compile time. NOT committed to the repo —
/// the build pipeline writes it into `engine/assets/model.bin` before the WASM
/// build, per the format the build target's Cargo feature requires.
///
/// During scaffolding, this is empty — guard accesses with `is_empty()` and
/// don't try to parse.
pub(crate) static MODEL_BYTES: &[u8] =
    include_bytes!("../../../assets/search/models/mini/model.bin");

static MODEL: OnceLock<LoadedModel> = OnceLock::new();

// ── Byte-offset layout (computed once from the header) ──────────────────────

/// Byte offsets for one packed-ternary BitLinear matrix.
///
/// Storage shape (sequential): `[packed_weights | w_scale (f32) | bias (f32) if present]`.
/// Weights are 2-bit packed at 4 weights per byte, row-major over (out_features, in_features).
#[derive(Debug, Clone, Copy)]
pub struct BitLinearLayout {
    pub weights_offset: usize,
    pub weights_bytes: usize, // out_features × (in_features / 4)
    pub scale_offset: usize,
    pub bias_offset: Option<usize>,
    pub in_features: usize,
    pub out_features: usize,
}

/// Byte offsets for one transformer layer.
#[derive(Debug, Clone, Copy)]
pub struct LayerLayout {
    pub ln1_offset: usize,
    pub w_q: BitLinearLayout,
    pub w_k: BitLinearLayout,
    pub w_v: BitLinearLayout,
    pub w_out: BitLinearLayout,
    pub ln2_offset: usize,
    pub fc1: BitLinearLayout,
    pub fc2: BitLinearLayout,
}

/// Precomputed byte offsets for every weight section in the `.bin`.
#[derive(Debug, Clone)]
pub struct WeightLayout {
    pub header: Header,

    // Embedding section
    pub embedding_weights_offset: usize,
    pub embedding_weights_bytes: usize,
    pub embedding_scales_offset: usize, // 0 if format is fp32 (no scales)
    pub embedding_scales_bytes: usize,

    // Per-layer offsets
    pub layers: Vec<LayerLayout>,

    // Final layernorm + output projection (fp32, NOT ternary)
    pub ln_final_offset: usize,
    pub projection_weights_offset: usize,
    pub projection_bias_offset: usize,
}

// ── Pre-decoded fp32 sections (the small stuff used in the hot path) ────────

/// Pre-decoded per-layer weights and biases.
///
/// fp32 sections (LN params, scales, biases) are small (~10 KB per layer).
/// BitLinear weights are pre-unpacked from 2-bit packed → `i8 {-1, 0, +1}`
/// once at engine init (Phase A3). Per-layer i8 weight totals for our config:
/// `wq + wk + wv + wout = 4 × 64 KB = 256 KB`, plus `fc1 + fc2 = 512 KB`,
/// for ~768 KB per layer × 2 layers ≈ 1.5 MB total — comfortably L2-resident.
///
/// Layout convention: `[out_features × in_features]` row-major. Row `j`
/// occupies indices `j * in_features .. (j+1) * in_features`. Matches the
/// shape `kernels::bitlinear_forward` expects.
#[derive(Debug)]
pub struct LayerWeights {
    pub ln1_w: Vec<f32>,
    pub ln1_b: Vec<f32>,
    pub wq: Vec<i8>, // [d_model × d_model]
    pub wq_scale: f32,
    pub wk: Vec<i8>, // [d_model × d_model]
    pub wk_scale: f32,
    pub wv: Vec<i8>, // [d_model × d_model]
    pub wv_scale: f32,
    pub wout: Vec<i8>, // [d_model × d_model]
    pub wout_scale: f32,
    pub wout_bias: Vec<f32>,
    pub ln2_w: Vec<f32>,
    pub ln2_b: Vec<f32>,
    pub fc1: Vec<i8>, // [ffn_dim × d_model]
    pub fc1_scale: f32,
    pub fc1_bias: Vec<f32>,
    pub fc2: Vec<i8>, // [d_model × ffn_dim]
    pub fc2_scale: f32,
    pub fc2_bias: Vec<f32>,
}

/// All pre-decoded weight sections. The embedding table is the only big
/// section that stays as a byte offset on `LoadedModel::body` — it's an
/// O(1) gather per token id, not a hot inner loop.
#[derive(Debug)]
pub struct RuntimeWeights {
    /// Per-row scales for the embedding table. Empty for `emb_fp32` builds
    /// (no scales section in that format).
    pub embedding_scales: Vec<f32>,
    pub layers: Vec<LayerWeights>,
    pub ln_final_w: Vec<f32>,
    pub ln_final_b: Vec<f32>,
    /// Output projection weight: `[output_dim × d_model]` row-major fp32.
    pub projection_w: Vec<f32>,
    pub projection_b: Vec<f32>,
}

#[derive(Debug)]
pub struct LoadedModel {
    pub layout: WeightLayout,
    pub weights: RuntimeWeights,
    pub body: &'static [u8], // MODEL_BYTES minus the trailing sha256
}

// ── Lazy init ───────────────────────────────────────────────────────────────

/// Lazy-init: parse header + sha256 verify + compute layout + decode runtime
/// weights on first access. Panics on any format error — the engine cannot
/// operate with a malformed `.bin`.
pub fn get() -> &'static LoadedModel {
    MODEL.get_or_init(|| {
        assert!(
            !MODEL_BYTES.is_empty(),
            "model.bin is empty — the build pipeline must populate engine/assets/model.bin \
             with a packed .bin matching the build's embedding-format feature"
        );
        let body =
            format::verify_sha256(MODEL_BYTES).expect("sha256 mismatch — corrupted model.bin");
        let header = format::parse_header(body).expect("invalid .bin header");
        let layout = compute_layout(&header, body);
        let weights = decode_runtime_weights(&layout, body);
        LoadedModel {
            layout,
            weights,
            body,
        }
    })
}

// ── Layout walk ─────────────────────────────────────────────────────────────

/// Walk the wire format. Single pass, computes every section's byte offset.
fn compute_layout(header: &Header, body: &[u8]) -> WeightLayout {
    let d_model = header.d_model as usize;
    let n_layers = header.n_layers as usize;
    let ffn_dim = header.ffn_dim as usize;
    let output_dim = header.output_dim as usize;
    let vocab_size = header.vocab_size as usize;

    let mut off = HEADER_SIZE;

    // Embedding
    let embedding_weights_offset = off;
    let (embedding_weights_bytes, embedding_scales_bytes) = match header.embedding_format {
        EMB_FP32 => (vocab_size * d_model * 4, 0),
        EMB_INT8 => (vocab_size * d_model, vocab_size * 4),
        EMB_TERNARY => (vocab_size * (d_model / 4), vocab_size * 4), // 2 bits/elem → 4 per byte
        EMB_INT4 => (vocab_size * (d_model / 2), vocab_size * 4),    // 4 bits/elem → 2 per byte
        _ => panic!("compute_layout: unknown embedding_format byte"),
    };
    off += embedding_weights_bytes;
    let embedding_scales_offset = off;
    off += embedding_scales_bytes;

    // Per-layer transformer blocks
    let ln_bytes = d_model * 4 * 2; // weight (d_model fp32) + bias (d_model fp32)

    let mut layers = Vec::with_capacity(n_layers);
    for _ in 0..n_layers {
        let ln1_offset = off;
        off += ln_bytes;

        // Q/K/V: d_model × d_model, NO bias
        let w_q = take_bitlinear(&mut off, d_model, d_model, false);
        let w_k = take_bitlinear(&mut off, d_model, d_model, false);
        let w_v = take_bitlinear(&mut off, d_model, d_model, false);
        // W_out: d_model × d_model, WITH bias
        let w_out = take_bitlinear(&mut off, d_model, d_model, true);

        let ln2_offset = off;
        off += ln_bytes;

        // fc1: d_model → ffn_dim, WITH bias
        let fc1 = take_bitlinear(&mut off, d_model, ffn_dim, true);
        // fc2: ffn_dim → d_model, WITH bias
        let fc2 = take_bitlinear(&mut off, ffn_dim, d_model, true);

        layers.push(LayerLayout {
            ln1_offset,
            w_q,
            w_k,
            w_v,
            w_out,
            ln2_offset,
            fc1,
            fc2,
        });
    }

    // Final LayerNorm
    let ln_final_offset = off;
    off += ln_bytes;

    // Output projection (fp32, NOT ternary)
    let projection_weights_offset = off;
    off += d_model * output_dim * 4;
    let projection_bias_offset = off;
    off += output_dim * 4;

    debug_assert_eq!(
        off, body.len(),
        "compute_layout consumed {} bytes, body has {} (wire-format drift between packer and engine)",
        off, body.len(),
    );
    let _ = SHA256_SIZE; // referenced for symmetry with verify_sha256 callers

    WeightLayout {
        header: *header,
        embedding_weights_offset,
        embedding_weights_bytes,
        embedding_scales_offset,
        embedding_scales_bytes,
        layers,
        ln_final_offset,
        projection_weights_offset,
        projection_bias_offset,
    }
}

fn take_bitlinear(
    off: &mut usize,
    in_features: usize,
    out_features: usize,
    has_bias: bool,
) -> BitLinearLayout {
    let weights_bytes = out_features * (in_features / 4);
    let weights_offset = *off;
    *off += weights_bytes;
    let scale_offset = *off;
    *off += 4;
    let bias_offset = if has_bias {
        let o = *off;
        *off += out_features * 4;
        Some(o)
    } else {
        None
    };
    BitLinearLayout {
        weights_offset,
        weights_bytes,
        scale_offset,
        bias_offset,
        in_features,
        out_features,
    }
}

// ── Decode the small fp32 sections once ─────────────────────────────────────

fn decode_runtime_weights(layout: &WeightLayout, body: &[u8]) -> RuntimeWeights {
    let h = &layout.header;
    let d_model = h.d_model as usize;
    let ffn_dim = h.ffn_dim as usize;
    let output_dim = h.output_dim as usize;

    let embedding_scales = if layout.embedding_scales_bytes == 0 {
        Vec::new()
    } else {
        read_f32_vec(body, layout.embedding_scales_offset, h.vocab_size as usize)
    };

    let mut layers = Vec::with_capacity(layout.layers.len());
    for l in &layout.layers {
        layers.push(LayerWeights {
            ln1_w: read_f32_vec(body, l.ln1_offset, d_model),
            ln1_b: read_f32_vec(body, l.ln1_offset + d_model * 4, d_model),
            wq: unpack_bitlinear(body, &l.w_q),
            wq_scale: read_f32(body, l.w_q.scale_offset),
            wk: unpack_bitlinear(body, &l.w_k),
            wk_scale: read_f32(body, l.w_k.scale_offset),
            wv: unpack_bitlinear(body, &l.w_v),
            wv_scale: read_f32(body, l.w_v.scale_offset),
            wout: unpack_bitlinear(body, &l.w_out),
            wout_scale: read_f32(body, l.w_out.scale_offset),
            wout_bias: read_f32_vec(body, l.w_out.bias_offset.expect("W_out has bias"), d_model),
            ln2_w: read_f32_vec(body, l.ln2_offset, d_model),
            ln2_b: read_f32_vec(body, l.ln2_offset + d_model * 4, d_model),
            fc1: unpack_bitlinear(body, &l.fc1),
            fc1_scale: read_f32(body, l.fc1.scale_offset),
            fc1_bias: read_f32_vec(body, l.fc1.bias_offset.expect("fc1 has bias"), ffn_dim),
            fc2: unpack_bitlinear(body, &l.fc2),
            fc2_scale: read_f32(body, l.fc2.scale_offset),
            fc2_bias: read_f32_vec(body, l.fc2.bias_offset.expect("fc2 has bias"), d_model),
        });
    }

    let ln_final_w = read_f32_vec(body, layout.ln_final_offset, d_model);
    let ln_final_b = read_f32_vec(body, layout.ln_final_offset + d_model * 4, d_model);

    let projection_w = read_f32_vec(body, layout.projection_weights_offset, output_dim * d_model);
    let projection_b = read_f32_vec(body, layout.projection_bias_offset, output_dim);

    RuntimeWeights {
        embedding_scales,
        layers,
        ln_final_w,
        ln_final_b,
        projection_w,
        projection_b,
    }
}

// ── BitLinear unpack (2-bit packed → `i8 {-1, 0, +1}`) ──────────────────────
//
// Codes: 00 = 0, 01 = +1, 10 = -1, 11 = reserved/pad. Lower bits = lower
// index (matches the packer in `training/pack/pack.py`). Run once at engine
// init — see [Phase A3] in `docs/tern-runtime-perf.md`.

fn unpack_bitlinear(body: &[u8], layout: &BitLinearLayout) -> Vec<i8> {
    let packed = &body[layout.weights_offset..layout.weights_offset + layout.weights_bytes];
    let n_elems = layout.in_features * layout.out_features;
    let mut out = Vec::with_capacity(n_elems);
    for &byte in packed {
        for sub in 0..4 {
            let code = (byte >> (sub * 2)) & 0b11;
            out.push(match code {
                0b00 => 0i8,
                0b01 => 1i8,
                0b10 => -1i8,
                _ => 0i8, // reserved — should not appear in valid `.bin`
            });
        }
    }
    debug_assert_eq!(out.len(), n_elems);
    out
}

// ── Byte → fp32 helpers (little-endian, no alignment assumptions) ───────────

#[inline]
pub(crate) fn read_f32(body: &[u8], offset: usize) -> f32 {
    f32::from_le_bytes([
        body[offset],
        body[offset + 1],
        body[offset + 2],
        body[offset + 3],
    ])
}

pub(crate) fn read_f32_vec(body: &[u8], offset: usize, n: usize) -> Vec<f32> {
    (0..n).map(|k| read_f32(body, offset + k * 4)).collect()
}

// ── Debug surface ───────────────────────────────────────────────────────────

pub fn config_summary() -> String {
    if MODEL_BYTES.is_empty() {
        return "model.bin not yet provisioned — engine scaffolding only".into();
    }
    let m = get();
    let h = &m.layout.header;
    format!(
        "tern-engine v{} | embedding_format={} | vocab={} d_model={} n_layers={} n_heads={} ffn_dim={} output_dim={} max_seq_len={}",
        h.format_version,
        match h.embedding_format {
            EMB_FP32 => "fp32", EMB_INT8 => "int8", EMB_TERNARY => "ternary", EMB_INT4 => "int4",
            _ => "?",
        },
        h.vocab_size, h.d_model, h.n_layers, h.n_heads, h.ffn_dim, h.output_dim, h.max_seq_len,
    )
}
