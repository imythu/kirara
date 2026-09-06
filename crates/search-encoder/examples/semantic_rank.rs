//! Diagnostic only: rank all public vectors without the service query classifier.
use std::{
    io::{self, Read},
    path::PathBuf,
};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/search");
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("manifest.json"))?)?;
    let bytes = std::fs::read(root.join("catalog-vectors.bin"))?;
    let vectors: Vec<f32> = bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
        .collect();
    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;
    let queries: Vec<String> = serde_json::from_str(&input)?;
    let encoder = search_encoder::Encoder::new()?;
    let mut output = Vec::new();
    for query in queries {
        let q = encoder.embed(&search_encoder::expand_semantic_text(&query))?;
        let mut ranked: Vec<_> = vectors
            .chunks_exact(768)
            .enumerate()
            .map(|(i, row)| {
                let score = row
                    .chunks_exact(384)
                    .map(|v| v.iter().zip(&q).map(|(a, b)| a * b).sum::<f32>())
                    .fold(f32::NEG_INFINITY, f32::max);
                (i, score)
            })
            .filter(|(_, score)| *score >= 0.65)
            .collect();
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        output.push(serde_json::json!({"query":query,"ids":ranked.iter().map(|(i,_)|manifest["row_ids"][*i].clone()).collect::<Vec<_>>(),"scores":ranked.iter().map(|(_,s)|s).collect::<Vec<_>>()}));
    }
    println!("{}", serde_json::to_string(&output)?);
    Ok(())
}
