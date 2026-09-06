//! Native CPU adapter for the pinned MIT ternlight mini 0.1.1 engine.
#[allow(dead_code, unexpected_cfgs)]
mod format;
#[allow(unexpected_cfgs)]
mod inference;
#[allow(unexpected_cfgs)]
mod kernels;
mod model;
#[allow(dead_code)]
mod tokenizer;
#[cfg(not(feature = "emb_int4"))]
compile_error!("this pinned mini model requires only emb_int4");
pub const DIMENSION: usize = 384;
pub const ENGINE_REVISION: &str = "c6d2c0a35d14c574ed2898b3dbf95977bca07208";
pub const MODEL_SHA256: &str = "07d8cfdba5773ad69a3fe6164b6c964e87b2368cc3ad6c2bdaf8566f2e5b6c98";
pub const TOKENIZER_SHA256: &str =
    "d241a60d5e8f04cc1b2b3e9ef7a4921b27bf526d9f6050ab90f9267a1f9e5c66";
#[derive(Debug, Clone)]
pub struct EncoderError(pub &'static str);
impl std::fmt::Display for EncoderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for EncoderError {}
#[derive(Clone, Copy, Debug)]
pub struct Encoder {
    _private: (),
}
impl Encoder {
    pub fn new() -> Result<Self, EncoderError> {
        static INITIALIZED: std::sync::OnceLock<Result<(), EncoderError>> =
            std::sync::OnceLock::new();
        INITIALIZED
            .get_or_init(|| {
                if sha256(model::MODEL_BYTES) != MODEL_SHA256
                    || sha256(tokenizer::TOKENIZER_BYTES) != TOKENIZER_SHA256
                {
                    return Err(EncoderError(
                        "embedded model/tokenizer fingerprint mismatch",
                    ));
                }
                std::panic::catch_unwind(|| {
                    model::get();
                    tokenizer::tokenize("warmup");
                })
                .map_err(|_| EncoderError("embedded model or tokenizer initialization failed"))
            })
            .clone()?;
        Ok(Self { _private: () })
    }
    pub fn embed(&self, text: &str) -> Result<Vec<f32>, EncoderError> {
        if text.len() > 16384 {
            return Err(EncoderError("encoder input exceeds 16384 bytes"));
        }
        let vector = std::panic::catch_unwind(|| inference::embed(text))
            .map_err(|_| EncoderError("native embedding failed"))?;
        if vector.len() != DIMENSION || vector.iter().any(|v| !v.is_finite()) {
            return Err(EncoderError("invalid embedding output"));
        }
        Ok(vector)
    }
    pub fn tokenize(&self, text: &str) -> Vec<u32> {
        tokenizer::tokenize(text)
    }
    pub fn info(&self) -> String {
        model::config_summary()
    }
}

pub const TEMPLATE_VERSION: u32 = 1;
pub fn sha256(bytes: &[u8]) -> String {
    use sha2::Digest;
    format!("{:x}", sha2::Sha256::digest(bytes))
}
/// Match Latin terms at word boundaries; CJK terms use literal substring matching.
pub fn concept_matches(text: &str, term: &str) -> bool {
    if !term.is_ascii() {
        return text.contains(term);
    }
    text.match_indices(term).any(|(start, _)| {
        let end = start + term.len();
        !text[..start]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_ascii_alphanumeric())
            && !text[end..]
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphanumeric())
    })
}
fn concepts() -> &'static serde_json::Value {
    static CONCEPTS: std::sync::OnceLock<serde_json::Value> = std::sync::OnceLock::new();
    CONCEPTS.get_or_init(|| {
        serde_json::from_str(include_str!("../../../assets/search/concepts.json"))
            .expect("embedded concepts")
    })
}
pub fn semantic_concepts(text: &str) -> Vec<String> {
    let text = text.to_lowercase();
    concepts()["concepts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| {
            c["terms"]
                .as_array()
                .unwrap()
                .iter()
                .any(|t| concept_matches(&text, t.as_str().unwrap()))
        })
        .map(|c| c["expansion"].as_str().unwrap().to_owned())
        .collect()
}
/// The same expansion is used for stored documents and runtime queries.
pub fn expand_semantic_text(text: &str) -> String {
    let terms = semantic_concepts(text);
    if terms.is_empty() {
        text.trim().to_owned()
    } else {
        format!("{}. {}", terms.join(" "), text.trim())
    }
}
