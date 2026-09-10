use std::collections::HashMap;

use quick_xml::Reader;
use quick_xml::escape::unescape;
use quick_xml::events::{BytesStart, Event};
use serde::Serialize;
use tracing::warn;

pub mod download;

#[derive(Debug, thiserror::Error)]
pub enum RssParseError {
    #[error("xml error: {0}")]
    Xml(#[from] quick_xml::Error),
    #[error("unexpected EOF while parsing RSS")]
    UnexpectedEof,
}

#[derive(Debug, Clone, Serialize)]
pub struct TorrentItem {
    pub rss_name: String,
    pub guid: String,
    pub title: String,
    pub link: Option<String>,
    pub pub_date: Option<String>,
    pub download_url: String,
    pub size_bytes: Option<u64>,
    pub seeders: Option<i32>,
    pub leechers: Option<i32>,
    pub free_end_timestamp: Option<i64>,
    pub free_elapsed_seconds: Option<u64>,
    pub download_volume_factor: Option<f64>,
    pub upload_volume_factor: Option<f64>,
    pub minimum_ratio: Option<f64>,
    pub minimum_seed_time: Option<u64>,
}

impl TorrentItem {
    pub fn is_hr(&self) -> bool {
        self.minimum_seed_time.is_some_and(|secs| secs > 0)
            || self.minimum_ratio.is_some_and(|ratio| ratio > 0.0)
    }

    #[allow(dead_code)]
    pub fn is_free(&self) -> bool {
        self.download_volume_factor
            .is_some_and(|v| v < f64::EPSILON)
    }

    #[allow(dead_code)]
    pub fn is_promoted(&self) -> bool {
        self.is_free()
            || self
                .download_volume_factor
                .is_some_and(|v| (v - 1.0).abs() > f64::EPSILON)
            || self
                .upload_volume_factor
                .is_some_and(|v| (v - 1.0).abs() > f64::EPSILON)
    }
}

#[derive(Debug, Clone)]
pub struct FeedSnapshot {
    pub items: HashMap<String, TorrentItem>,
}

#[derive(Debug, Default)]
struct PartialItem {
    guid: Option<String>,
    title: Option<String>,
    description: Option<String>,
    link: Option<String>,
    pub_date: Option<String>,
    download_url: Option<String>,
    size_bytes: Option<u64>,
    seeders: Option<i32>,
    leechers: Option<i32>,
    categories: Vec<String>,
    download_volume_factor: Option<f64>,
    upload_volume_factor: Option<f64>,
    minimum_ratio: Option<f64>,
    minimum_seed_time: Option<u64>,
}

#[derive(Debug)]
pub struct ParsedFeed {
    items: Vec<PartialItem>,
}

impl ParsedFeed {
    pub fn into_snapshot(self, rss_name: String) -> FeedSnapshot {
        let mut items = HashMap::new();

        for item in self.items {
            let Some(guid) = item.guid else {
                warn!("skipped RSS item without guid");
                continue;
            };
            let Some(download_url) = item.download_url else {
                warn!("skipped RSS item without enclosure url, guid={guid}");
                continue;
            };

            let title = item.title.unwrap_or_else(|| guid.clone());
            let mut torrent = TorrentItem {
                rss_name: rss_name.clone(),
                guid: guid.clone(),
                title,
                link: item.link,
                pub_date: item.pub_date,
                download_url,
                size_bytes: item.size_bytes,
                seeders: item.seeders,
                leechers: item.leechers,
                free_end_timestamp: None,
                free_elapsed_seconds: None,
                download_volume_factor: item.download_volume_factor,
                upload_volume_factor: item.upload_volume_factor,
                minimum_ratio: item.minimum_ratio,
                minimum_seed_time: item.minimum_seed_time,
            };
            apply_textual_markers(&mut torrent, item.description.as_deref(), &item.categories);
            items.insert(guid.clone(), torrent);
        }

        FeedSnapshot { items }
    }
}

pub fn parse_feed(xml: &str) -> Result<ParsedFeed, RssParseError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut items = Vec::new();
    let mut current_item: Option<PartialItem> = None;
    let mut current_tag: Option<Vec<u8>> = None;

    loop {
        match reader.read_event()? {
            Event::Start(event) => {
                let name = event.name().as_ref().to_vec();
                if name.as_slice() == b"item" {
                    current_item = Some(PartialItem::default());
                } else if let Some(item) = current_item.as_mut() {
                    let local = name.split(|&b| b == b':').last().unwrap_or(&name);
                    if local == b"enclosure" {
                        fill_enclosure(item, &event);
                    } else if local == b"attr" {
                        fill_attr(item, &event);
                    }
                }
                current_tag = Some(name);
            }
            Event::Empty(event) => {
                let name = event.name().as_ref().to_vec();
                let local = name.split(|&b| b == b':').last().unwrap_or(&name);
                if local == b"enclosure" {
                    if let Some(item) = current_item.as_mut() {
                        fill_enclosure(item, &event);
                    }
                } else if local == b"attr" {
                    if let Some(item) = current_item.as_mut() {
                        fill_attr(item, &event);
                    }
                }
            }
            Event::Text(event) => {
                let raw = String::from_utf8_lossy(event.as_ref());
                let text = unescape(&raw)
                    .map(|value| value.into_owned())
                    .unwrap_or_else(|_| raw.into_owned());
                if let Some(item) = current_item.as_mut() {
                    match current_tag.as_deref() {
                        Some(b"guid") => item.guid = Some(text),
                        Some(b"title") => item.title = Some(text),
                        Some(b"description") | Some(b"comments") => item.description = Some(text),
                        Some(b"link") => item.link = Some(text),
                        Some(b"category") => item.categories.push(text),
                        Some(b"pubDate") => item.pub_date = Some(text),
                        Some(tag) => {
                            let local = tag.split(|&b| b == b':').last().unwrap_or(tag);
                            match local {
                                b"description" | b"comments" => {
                                    item.description = Some(text);
                                }
                                b"category" => {
                                    item.categories.push(text);
                                }
                                b"seeders" | b"seeds" | b"seeder" => {
                                    item.seeders = text.trim().parse().ok();
                                }
                                b"peers" | b"leechers" | b"leecher" => {
                                    item.leechers = text.trim().parse().ok();
                                }
                                b"size" | b"contentLength" | b"filesize" => {
                                    item.size_bytes = text.trim().parse().ok();
                                }
                                b"downloadvolumefactor" => {
                                    item.download_volume_factor = text.trim().parse().ok();
                                }
                                b"uploadvolumefactor" => {
                                    item.upload_volume_factor = text.trim().parse().ok();
                                }
                                b"minimumratio" => {
                                    item.minimum_ratio = text.trim().parse().ok();
                                }
                                b"minimumseedtime" => {
                                    item.minimum_seed_time = text.trim().parse().ok();
                                }
                                _ => {}
                            }
                        }
                        None => {}
                    }
                }
            }
            Event::CData(event) => {
                let text = String::from_utf8_lossy(event.as_ref()).to_string();
                if let Some(item) = current_item.as_mut() {
                    match current_tag.as_deref() {
                        Some(b"title") => item.title = Some(text),
                        Some(b"description") | Some(b"comments") => item.description = Some(text),
                        Some(b"category") => item.categories.push(text),
                        Some(tag) => {
                            let local = tag.split(|&b| b == b':').last().unwrap_or(tag);
                            match local {
                                b"description" | b"comments" => item.description = Some(text),
                                b"category" => item.categories.push(text),
                                _ => {}
                            }
                        }
                        None => {}
                    }
                }
            }
            Event::End(event) => {
                if event.name().as_ref() == b"item" {
                    let Some(item) = current_item.take() else {
                        return Err(RssParseError::UnexpectedEof);
                    };
                    items.push(item);
                }
                current_tag = None;
            }
            Event::Eof => break,
            _ => {}
        }
    }

    Ok(ParsedFeed { items })
}

fn fill_enclosure(item: &mut PartialItem, event: &BytesStart<'_>) {
    for attribute in event.attributes().flatten() {
        match attribute.key.as_ref() {
            b"url" => {
                let raw = String::from_utf8_lossy(attribute.value.as_ref());
                let value = unescape(&raw)
                    .map(|value| value.into_owned())
                    .unwrap_or_else(|_| raw.into_owned());
                item.download_url = Some(value);
            }
            b"length" => {
                item.size_bytes = String::from_utf8_lossy(attribute.value.as_ref())
                    .trim()
                    .parse()
                    .ok();
            }
            _ => {}
        }
    }
}

fn fill_attr(item: &mut PartialItem, event: &BytesStart<'_>) {
    let mut attr_name: Option<String> = None;
    let mut attr_value: Option<String> = None;

    for attribute in event.attributes().flatten() {
        let raw = String::from_utf8_lossy(attribute.value.as_ref());
        let value = unescape(&raw)
            .map(|value| value.into_owned())
            .unwrap_or_else(|_| raw.into_owned());
        match attribute.key.as_ref() {
            b"name" => attr_name = Some(value.to_ascii_lowercase()),
            b"value" => attr_value = Some(value),
            _ => {}
        }
    }

    let (Some(name), Some(value)) = (attr_name.as_deref(), attr_value.as_deref()) else {
        return;
    };

    match name {
        "seeders" | "seeds" | "seed" => {
            item.seeders = value.trim().parse().ok();
        }
        "peers" | "leechers" | "leech" => {
            // torznab 中 peers 即下载人数（leechers）。
            item.leechers = value.trim().parse().ok();
        }
        "size" => {
            item.size_bytes = value.trim().parse().ok();
        }
        "downloadvolumefactor" => {
            item.download_volume_factor = value.trim().parse().ok();
        }
        "uploadvolumefactor" => {
            item.upload_volume_factor = value.trim().parse().ok();
        }
        "minimumratio" => {
            item.minimum_ratio = value.trim().parse().ok();
        }
        "minimumseedtime" => {
            item.minimum_seed_time = value.trim().parse().ok();
        }
        "freeleech" => {
            if is_truthy(value) {
                item.download_volume_factor = Some(0.0);
            }
        }
        _ => {}
    }
}

fn is_truthy(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "y"
    )
}

fn apply_textual_markers(item: &mut TorrentItem, description: Option<&str>, categories: &[String]) {
    let mut text_parts = Vec::new();
    text_parts.push(item.title.as_str());
    if let Some(description) = description {
        text_parts.push(description);
    }
    for category in categories {
        text_parts.push(category.as_str());
    }

    let joined = text_parts.join(" ");
    if joined.is_empty() {
        return;
    }

    let upper = joined.to_ascii_uppercase();

    if item.download_volume_factor.is_none() {
        if contains_any(
            &upper,
            &["2XFREE", "2X FL", "2XFREE", "FREE,2XUP", "FREE 2XUP"],
        ) {
            item.download_volume_factor = Some(0.0);
            item.upload_volume_factor.get_or_insert(2.0);
        } else if contains_any(
            &upper,
            &[
                "FREELEECH",
                "FREE LEECH",
                "FREE",
                "0X",
                "0.0X",
                "DOWN 0%",
                "DOWNLOAD 0%",
                "零魔",
                "免费",
            ],
        ) {
            item.download_volume_factor = Some(0.0);
        } else if contains_any(
            &upper,
            &[
                "30%DL",
                "30% DL",
                "0.3X",
                "30%DOWN",
                "DOWNLOAD 30%",
                "七折",
                "3成下载",
            ],
        ) {
            item.download_volume_factor = Some(0.3);
        } else if contains_any(
            &upper,
            &[
                "50%DL",
                "50% DL",
                "0.5X",
                "50%DOWN",
                "DOWNLOAD 50%",
                "半价",
                "五折",
            ],
        ) {
            item.download_volume_factor = Some(0.5);
        }
    }

    if item.upload_volume_factor.is_none() {
        if contains_any(
            &upper,
            &[
                "2XUP",
                "2X UP",
                "2XUPLOAD",
                "UPLOAD 200%",
                "UP 200%",
                "双倍上传",
            ],
        ) {
            item.upload_volume_factor = Some(2.0);
        } else if contains_any(
            &upper,
            &["0XUP", "UPLOAD 0%", "UP 0%", "零上传", "不计上传"],
        ) {
            item.upload_volume_factor = Some(0.0);
        }
    }
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::parse_feed;

    #[test]
    fn parses_torznab_promotion_and_hr_attrs() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0" xmlns:torznab="http://torznab.com/schemas/2015/feed">
  <channel>
    <item>
      <title>Example</title>
      <guid>abc</guid>
      <enclosure url="https://example.test/download" length="1024" />
      <torznab:attr name="seeders" value="12" />
      <torznab:attr name="peers" value="5" />
      <torznab:attr name="downloadvolumefactor" value="0" />
      <torznab:attr name="uploadvolumefactor" value="2" />
      <torznab:attr name="minimumratio" value="1.5" />
      <torznab:attr name="minimumseedtime" value="86400" />
    </item>
  </channel>
</rss>"#;

        let parsed = parse_feed(xml).expect("rss should parse");
        let snapshot = parsed.into_snapshot("test".to_string());
        let item = snapshot.items.get("abc").expect("item should exist");

        assert_eq!(item.seeders, Some(12));
        assert_eq!(item.leechers, Some(5));
        assert_eq!(item.size_bytes, Some(1024));
        assert_eq!(item.download_volume_factor, Some(0.0));
        assert_eq!(item.upload_volume_factor, Some(2.0));
        assert_eq!(item.minimum_ratio, Some(1.5));
        assert_eq!(item.minimum_seed_time, Some(86400));
        assert!(item.is_free());
        assert!(item.is_hr());
        assert!(item.is_promoted());
    }

    #[test]
    fn parses_nonstandard_promotion_markers_from_title_and_description() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0">
  <channel>
    <item>
      <title>[FREE][2XUP][H&R] Example</title>
      <description><![CDATA[This torrent is freeleech and hit and run applies.]]></description>
      <guid>def</guid>
      <enclosure url="https://example.test/download2" length="2048" />
    </item>
  </channel>
</rss>"#;

        let parsed = parse_feed(xml).expect("rss should parse");
        let snapshot = parsed.into_snapshot("test".to_string());
        let item = snapshot.items.get("def").expect("item should exist");

        assert_eq!(item.download_volume_factor, Some(0.0));
        assert_eq!(item.upload_volume_factor, Some(2.0));
        assert!(item.is_free());
        assert!(item.is_promoted());
    }

    #[test]
    fn parses_discount_markers_from_category() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0">
  <channel>
    <item>
      <title>Example</title>
      <category>50%DL</category>
      <guid>ghi</guid>
      <enclosure url="https://example.test/download3" length="4096" />
    </item>
  </channel>
</rss>"#;

        let parsed = parse_feed(xml).expect("rss should parse");
        let snapshot = parsed.into_snapshot("test".to_string());
        let item = snapshot.items.get("ghi").expect("item should exist");

        assert_eq!(item.download_volume_factor, Some(0.5));
        assert!(item.is_promoted());
        assert!(!item.is_free());
    }
}
