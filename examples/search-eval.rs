//! Maintainer helper: echo '["无损音乐"]' | cargo run --release --example search-eval
#[path = "../src/search/mod.rs"]
mod search;
use search::{SearchDocument, SearchFilters, catalog, search};
use std::io::{self, Read};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;
    let queries: Vec<String> = serde_json::from_str(&input)?;
    let docs: Vec<_> = catalog()
        .iter()
        .enumerate()
        .map(|(id, e)| SearchDocument {
            id: id as i64,
            names: vec![e.name.clone()],
            catalog_id: Some(e.id.clone()),
            ..Default::default()
        })
        .collect();
    let mut output = Vec::new();
    for query in queries {
        let start = std::time::Instant::now();
        let result = search(&docs, &query, &SearchFilters::default()).map_err(io::Error::other)?;
        output.push(serde_json::json!({"query":query,"ids":result.hits.iter().map(|h|&catalog()[h.id as usize].id).collect::<Vec<_>>(),"semantic_status":result.semantic_status,"elapsed_ms":start.elapsed().as_secs_f64()*1000.}));
    }
    println!("{}", serde_json::to_string(&output)?);
    Ok(())
}
