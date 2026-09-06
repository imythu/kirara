//! cargo run -p search-encoder --release --example assets -- build|verify|eval
use pinyin::ToPinyin;
use search_encoder::{sha256, Encoder, DIMENSION, ENGINE_REVISION, MODEL_SHA256, TOKENIZER_SHA256};
use serde_json::{json, Value};
use std::{fs, path::PathBuf};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/search");
    let command = std::env::args().nth(1).unwrap_or_else(|| "verify".into());
    let mut catalog: Value = serde_json::from_slice(&fs::read(root.join("catalog.json"))?)?;
    if command == "build" {
        for row in catalog["sites"].as_array_mut().ok_or("sites array")? {
            let names = std::iter::once(row["name"].as_str().unwrap().to_owned())
                .chain(
                    row["aka"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|x| x.as_str().unwrap().to_owned()),
                )
                .collect::<Vec<_>>();
            let (mut full, mut initials) = (Vec::new(), Vec::new());
            for name in names {
                let (mut f, mut i) = (String::new(), String::new());
                let mut remaining = name.as_str();
                while !remaining.is_empty() {
                    let correction = [
                        ("重庆", ["chong", "qing"]),
                        ("重邮", ["chong", "you"]),
                        ("音乐", ["yin", "yue"]),
                        ("快乐", ["kuai", "le"]),
                    ]
                    .into_iter()
                    .find(|(word, _)| remaining.starts_with(word));
                    if let Some((word, syllables)) = correction {
                        for syllable in syllables {
                            f.push_str(syllable);
                            i.push(syllable.chars().next().unwrap());
                        }
                        remaining = &remaining[word.len()..];
                        continue;
                    }
                    let c = remaining.chars().next().unwrap();
                    remaining = &remaining[c.len_utf8()..];
                    if let Some(p) = c.to_pinyin() {
                        f.push_str(p.plain());
                        i.push(p.plain().chars().next().unwrap());
                    } else if c.is_alphanumeric() {
                        let c = c.to_lowercase().next().unwrap();
                        f.push(c);
                        i.push(c);
                    }
                }
                full.push(f);
                initials.push(i);
            }
            full.sort();
            full.dedup();
            initials.sort();
            initials.dedup();
            row["pinyin"] = json!(full);
            row["initials"] = json!(initials);
        }
        fs::write(
            root.join("catalog.json"),
            serde_json::to_string_pretty(&catalog)? + "\n",
        )?;
        let encoder = Encoder::new()?;
        let mut bytes = Vec::new();
        for (n, site) in catalog["sites"].as_array().unwrap().iter().enumerate() {
            for field in ["resource_text", "feature_text"] {
                let input =
                    search_encoder::expand_semantic_text(site[field].as_str().unwrap_or(""));
                for v in encoder.embed(&input)? {
                    bytes.extend_from_slice(&v.to_le_bytes());
                }
            }
            if n % 50 == 0 {
                eprintln!(
                    "embedded {}/{}",
                    n + 1,
                    catalog["sites"].as_array().unwrap().len()
                );
            }
        }
        fs::write(root.join("catalog-vectors.bin"), &bytes)?;
        let mut manifest = json!({"format_version":1,"dimension":DIMENSION,"vectors_per_site":2,"byte_order":"little","normalization":"l2","template_version":1,"concepts_version":1,
            "engine_revision":ENGINE_REVISION,"model_sha256":MODEL_SHA256,"tokenizer_sha256":TOKENIZER_SHA256,
            "source_revision":catalog["source_revision"],"row_ids":catalog["sites"].as_array().unwrap().iter().map(|s|s["id"].clone()).collect::<Vec<_>>()});
        for (key, file) in [
            ("catalog_sha256", "catalog.json"),
            ("concepts_sha256", "concepts.json"),
            ("vectors_sha256", "catalog-vectors.bin"),
            ("crosswalk_sha256", "ptd-crosswalk.json"),
            ("overrides_sha256", "overrides.json"),
        ] {
            manifest[key] = json!(sha256(&fs::read(root.join(file))?));
        }
        let fingerprint = format!(
            "{}:{}:{}:{}:{}:l2:384:two-documents-v1",
            MODEL_SHA256,
            TOKENIZER_SHA256,
            ENGINE_REVISION,
            manifest["concepts_sha256"].as_str().unwrap(),
            manifest["catalog_sha256"].as_str().unwrap()
        );
        manifest["fingerprint"] = json!(sha256(fingerprint.as_bytes()));
        fs::write(
            root.join("manifest.json"),
            serde_json::to_string_pretty(&manifest)? + "\n",
        )?;
    }
    let manifest: Value = serde_json::from_slice(&fs::read(root.join("manifest.json"))?)?;
    for (key, file) in [
        ("catalog_sha256", "catalog.json"),
        ("concepts_sha256", "concepts.json"),
        ("vectors_sha256", "catalog-vectors.bin"),
        ("crosswalk_sha256", "ptd-crosswalk.json"),
        ("overrides_sha256", "overrides.json"),
        ("model_sha256", "models/mini/model.bin"),
        ("tokenizer_sha256", "models/mini/tokenizer.json"),
    ] {
        if manifest[key].as_str() != Some(&sha256(&fs::read(root.join(file))?)) {
            return Err(format!("hash mismatch: {file}").into());
        }
    }
    if manifest["model_sha256"] != MODEL_SHA256
        || manifest["tokenizer_sha256"] != TOKENIZER_SHA256
        || manifest["engine_revision"] != ENGINE_REVISION
        || manifest["format_version"] != 1
        || manifest["dimension"] != DIMENSION
        || manifest["vectors_per_site"] != 2
        || manifest["byte_order"] != "little"
        || manifest["normalization"] != "l2"
        || manifest["template_version"] != 1
        || manifest["concepts_version"] != 1
    {
        return Err("incompatible manifest".into());
    }
    let ids = catalog["sites"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].clone())
        .collect::<Vec<_>>();
    if manifest["row_ids"] != json!(ids) {
        return Err("row mapping mismatch".into());
    }
    let bytes = fs::read(root.join("catalog-vectors.bin"))?;
    if bytes.len() != ids.len() * 2 * DIMENSION * 4 {
        return Err("vector length mismatch".into());
    }
    let vectors = bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
        .collect::<Vec<_>>();
    for v in vectors.chunks_exact(DIMENSION) {
        let norm = v.iter().map(|v| v * v).sum::<f32>();
        if v.iter().any(|v| !v.is_finite()) || !(0.998..=1.002).contains(&norm) {
            return Err("invalid normalized vector".into());
        }
    }
    if command == "eval" {
        let encoder = Encoder::new()?;
        for query in [
            "无损音乐",
            "适合新手的动漫站",
            "lossless music",
            "anime for beginners",
            "rare arthouse cinema",
            "xqzvjk qprst nonsense",
            "天气预报明天下雨",
            "learn organic chemistry",
            "儿童动画",
            "足球赛车直播",
            "收藏冷门老电影",
        ] {
            let embedding = encoder.embed(&search_encoder::expand_semantic_text(query))?;
            let mut ranks = vectors
                .chunks_exact(2 * DIMENSION)
                .enumerate()
                .map(|(i, vs)| {
                    let score = vs
                        .chunks_exact(DIMENSION)
                        .map(|v| v.iter().zip(&embedding).map(|(a, b)| a * b).sum::<f32>())
                        .fold(f32::NEG_INFINITY, f32::max);
                    (i, score)
                })
                .collect::<Vec<_>>();
            ranks.sort_by(|a, b| b.1.total_cmp(&a.1));
            println!(
                "{}",
                json!({"query":query,"top5":ranks.iter().take(5).map(|(i,score)|json!({"id":ids[*i],"score":score})).collect::<Vec<_>>() })
            );
        }
    }
    eprintln!(
        "Verified {} rows, {} normalized vectors, {} bytes",
        ids.len(),
        ids.len() * 2,
        bytes.len()
    );
    Ok(())
}
