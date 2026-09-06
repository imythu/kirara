fn main() {
    let start = std::time::Instant::now();
    let e = search_encoder::Encoder::new().unwrap();
    eprintln!("load {:?} {}", start.elapsed(), e.info());
    for text in [
        "lossless music",
        "anime for beginners",
        "无损音乐",
        "适合新手的动漫站",
        "An archive of high quality films.",
    ] {
        let start = std::time::Instant::now();
        let vector = e.embed(text).unwrap();
        eprintln!("{text}: {:?}", start.elapsed());
        println!(
            "{}",
            serde_json::json!({"text":text,"tokens":e.tokenize(text),"vector":vector})
        );
    }
}
