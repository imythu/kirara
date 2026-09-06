use search_encoder::{sha256, Encoder, DIMENSION, MODEL_SHA256, TOKENIZER_SHA256};
#[test]
fn pinned_model_and_tokenizer_hashes() {
    assert_eq!(
        sha256(include_bytes!(
            "../../../assets/search/models/mini/model.bin"
        )),
        MODEL_SHA256
    );
    assert_eq!(
        sha256(include_bytes!(
            "../../../assets/search/models/mini/tokenizer.json"
        )),
        TOKENIZER_SHA256
    );
}
#[test]
fn published_mini_tokenizer_and_vector_parity() {
    let fixtures: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/mini-reference.json")).unwrap();
    let encoder = Encoder::new().unwrap();
    for row in fixtures["rows"].as_array().unwrap() {
        let text = row["text"].as_str().unwrap();
        let expected_tokens = row["tokens"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_u64().unwrap() as u32)
            .collect::<Vec<_>>();
        assert_eq!(
            encoder.tokenize(text),
            expected_tokens,
            "tokenizer mismatch {text}"
        );
        let actual = encoder.embed(text).unwrap();
        assert_eq!(actual.len(), DIMENSION);
        let expected = row["vector"].as_array().unwrap();
        let max_diff = actual
            .iter()
            .zip(expected)
            .map(|(a, b)| (a - b.as_f64().unwrap() as f32).abs())
            .fold(0f32, f32::max);
        assert!(
            max_diff <= 0.00002,
            "native/reference mismatch {text}: {max_diff}"
        );
        let norm = actual.iter().map(|x| x * x).sum::<f32>();
        assert!((norm - 1.0).abs() < 0.0001);
    }
}
#[test]
fn concept_expansion_preserves_original_and_word_boundaries() {
    assert!(search_encoder::expand_semantic_text("无损音乐").starts_with("music audio lossless"));
    assert!(search_encoder::expand_semantic_text("无损音乐").ends_with("无损音乐"));
    assert!(search_encoder::semantic_concepts("gamedomain.example").is_empty());
    assert!(Encoder::new().unwrap().embed(&"x".repeat(16385)).is_err());
}
