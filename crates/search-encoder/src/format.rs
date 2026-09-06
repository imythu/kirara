//! `.bin` v1 wire format — header parsing, format-tag dispatch, sha256 verify.
//!
//! Mirrors `training/pack/format.py`. Constants here are the single source of
//! truth for the WASM side; if you change them, change the Python side too.
//! Canonical spec: `docs/tern-inference-engine.md`.

// ── Wire constants ──────────────────────────────────────────────────────────

pub const MAGIC: &[u8; 4] = b"TERN";
pub const FORMAT_VERSION: u16 = 1;

pub const EMB_FP32: u8 = 0;
pub const EMB_INT8: u8 = 1;
pub const EMB_TERNARY: u8 = 2;
pub const EMB_INT4: u8 = 3;

pub const WEIGHTS_TERNARY: u8 = 0;

pub const HEADER_SIZE: usize = 32;
pub const SHA256_SIZE: usize = 32;

// ── Compile-time format dispatch ────────────────────────────────────────────
//
// The feature flag picks ONE embedding format; this constant lets the rest
// of the code reference "the format this build targets" without `#[cfg]`
// littered everywhere.

#[cfg(feature = "emb_fp32")]
pub const BUILD_EMBEDDING_FORMAT: u8 = EMB_FP32;
#[cfg(feature = "emb_int8")]
pub const BUILD_EMBEDDING_FORMAT: u8 = EMB_INT8;
#[cfg(feature = "emb_ternary")]
pub const BUILD_EMBEDDING_FORMAT: u8 = EMB_TERNARY;
#[cfg(feature = "emb_int4")]
pub const BUILD_EMBEDDING_FORMAT: u8 = EMB_INT4;

// ── Header ──────────────────────────────────────────────────────────────────

/// Parsed `.bin` header — 32 bytes, little-endian.
///
/// Field layout matches `training/pack/format.py:_HEADER_STRUCT`. Adding fields
/// requires bumping `FORMAT_VERSION` on both sides.
#[derive(Debug, Clone, Copy)]
pub struct Header {
    pub format_version: u16,
    pub embedding_format: u8,
    pub weights_format: u8,
    pub vocab_size: u32,
    pub d_model: u16,
    pub n_layers: u8,
    pub n_heads: u8,
    pub ffn_dim: u16,
    pub output_dim: u16,
    pub max_seq_len: u16,
}

#[derive(Debug)]
pub enum FormatError {
    TooShort,
    BadMagic,
    UnsupportedVersion(u16),
    UnknownEmbeddingFormat(u8),
    EmbeddingFormatMismatch { expected: u8, got: u8 },
    UnsupportedWeightsFormat(u8),
    Sha256Mismatch,
}

/// Parse the 32-byte header from the start of a `.bin` buffer.
///
/// Validates: magic, format_version, embedding_format byte is known, and
/// (most importantly) the embedding_format matches the build's compile-time
/// feature. A `.bin` packed for one format CANNOT be loaded by an engine
/// built for another.
pub fn parse_header(buf: &[u8]) -> Result<Header, FormatError> {
    if buf.len() < HEADER_SIZE {
        return Err(FormatError::TooShort);
    }
    if &buf[0..4] != MAGIC {
        return Err(FormatError::BadMagic);
    }
    let format_version = u16::from_le_bytes([buf[4], buf[5]]);
    if format_version != FORMAT_VERSION {
        return Err(FormatError::UnsupportedVersion(format_version));
    }
    let embedding_format = buf[6];
    if embedding_format > EMB_INT4 {
        return Err(FormatError::UnknownEmbeddingFormat(embedding_format));
    }
    if embedding_format != BUILD_EMBEDDING_FORMAT {
        return Err(FormatError::EmbeddingFormatMismatch {
            expected: BUILD_EMBEDDING_FORMAT,
            got: embedding_format,
        });
    }
    let weights_format = buf[7];
    if weights_format != WEIGHTS_TERNARY {
        return Err(FormatError::UnsupportedWeightsFormat(weights_format));
    }
    let vocab_size = u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]);
    let d_model = u16::from_le_bytes([buf[12], buf[13]]);
    let n_layers = buf[14];
    let n_heads = buf[15];
    let ffn_dim = u16::from_le_bytes([buf[16], buf[17]]);
    let output_dim = u16::from_le_bytes([buf[18], buf[19]]);
    let max_seq_len = u16::from_le_bytes([buf[20], buf[21]]);
    // bytes 22..32 are reserved padding — ignored on read
    Ok(Header {
        format_version,
        embedding_format,
        weights_format,
        vocab_size,
        d_model,
        n_layers,
        n_heads,
        ffn_dim,
        output_dim,
        max_seq_len,
    })
}

/// Verify the trailing 32-byte SHA256 of a `.bin` buffer.
///
/// Returns the body slice (everything except the trailing hash) on success.
/// Call this once at engine init — the .bin lives in immutable WASM memory
/// after that, so re-verifying per query is pointless.
pub fn verify_sha256(buf: &[u8]) -> Result<&[u8], FormatError> {
    use sha2::{Digest, Sha256};
    if buf.len() < HEADER_SIZE + SHA256_SIZE {
        return Err(FormatError::TooShort);
    }
    let (body, tail) = buf.split_at(buf.len() - SHA256_SIZE);
    let mut hasher = Sha256::new();
    hasher.update(body);
    let actual = hasher.finalize();
    if actual.as_slice() != tail {
        return Err(FormatError::Sha256Mismatch);
    }
    Ok(body)
}
