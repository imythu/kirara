//! BERT tokenizer — HuggingFace `tokenizers` crate with `unstable_wasm` feature.
//!
//! Vocab is embedded at compile time via `include_bytes!` and lazy-initialized
//! into a `OnceLock<Tokenizer>` on first call. No runtime file I/O.
//!
//! Crate version + features are pinned in `Cargo.toml`. The Python training
//! pipeline uses the same underlying Rust crate (via `transformers`), so the
//! engine and training share tokenization implementation, not just compatibility.

use std::sync::OnceLock;
use tokenizers::Tokenizer;

/// BERT-base-uncased vocab, committed at `engine/assets/tokenizer.json`.
/// Updating this asset requires rebuilding all WASM artifacts.
pub(crate) static TOKENIZER_BYTES: &[u8] =
    include_bytes!("../../../assets/search/models/mini/tokenizer.json");

static TOKENIZER: OnceLock<Tokenizer> = OnceLock::new();

/// Pad token id for BERT-base-uncased ([PAD]). Used to right-pad sequences
/// shorter than max_seq_len.
pub const PAD_TOKEN_ID: u32 = 0;

/// Maximum sequence length. Matches the value the model was trained at.
/// Inputs longer than this are truncated.
pub const MAX_SEQ_LEN: usize = 128;

fn get_tokenizer() -> &'static Tokenizer {
    TOKENIZER.get_or_init(|| {
        Tokenizer::from_bytes(TOKENIZER_BYTES)
            .expect("embedded tokenizer.json is invalid — rebuild engine with a fresh asset")
    })
}

/// Tokenize a string into token IDs.
///
/// - Adds [CLS] / [SEP] special tokens (BERT convention)
/// - Truncates to MAX_SEQ_LEN
/// - Pads with PAD_TOKEN_ID to MAX_SEQ_LEN
///
/// Returns a Vec<u32> of length MAX_SEQ_LEN.
pub fn tokenize(text: &str) -> Vec<u32> {
    let tk = get_tokenizer();
    let encoding = tk.encode(text, true).expect("tokenizer.encode failed");
    let mut ids: Vec<u32> = encoding
        .get_ids()
        .iter()
        .take(MAX_SEQ_LEN)
        .copied()
        .collect();
    while ids.len() < MAX_SEQ_LEN {
        ids.push(PAD_TOKEN_ID);
    }
    ids
}

/// Attention mask companion to `tokenize()`. Returns 1s for real tokens,
/// 0s for padding. Same length as the output of `tokenize()`.
pub fn attention_mask(token_ids: &[u32]) -> Vec<u8> {
    token_ids
        .iter()
        .map(|&id| if id == PAD_TOKEN_ID { 0 } else { 1 })
        .collect()
}
