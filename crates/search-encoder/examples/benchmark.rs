use search_encoder::{expand_semantic_text, Encoder};
use std::{hint::black_box, time::Instant};
fn main() {
    let start = Instant::now();
    let encoder = Encoder::new().unwrap();
    let load = start.elapsed().as_secs_f64() * 1000.;
    let queries = [
        "无损音乐",
        "适合新手的动漫站",
        "rare arthouse cinema",
        "收藏冷门老电影",
        "有声书和学习资料",
    ];
    let start = Instant::now();
    black_box(encoder.embed(&expand_semantic_text(queries[0])).unwrap());
    let first = start.elapsed().as_secs_f64() * 1000.;
    let mut times = Vec::new();
    for _ in 0..20 {
        for query in queries {
            let start = Instant::now();
            black_box(encoder.embed(&expand_semantic_text(query)).unwrap());
            times.push(start.elapsed().as_secs_f64() * 1000.);
        }
    }
    times.sort_by(f64::total_cmp);
    println!(
        "{}",
        serde_json::json!({"load_ms":load,"first_encode_ms":first,"samples":times.len(),"hot_p50_ms":times[times.len()/2],"hot_p95_ms":times[times.len()*95/100],"hot_max_ms":times.last()})
    );
}
