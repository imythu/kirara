//! Strict parsing for the RSS downloader. The legacy brush parser intentionally
//! keeps its historical, more permissive behaviour in the parent module.

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use quick_xml::events::{BytesStart, Event};
use quick_xml::name::ResolveResult;
use quick_xml::reader::NsReader;
use reqwest::Url;
use sha2::{Digest, Sha256};

use crate::rss_download::models::{FetchedFeed, ItemAttributes, NormalizedItem};

const MAX_FEED_BYTES: usize = 8 * 1024 * 1024;
const MAX_ITEMS: usize = 1000;
const MAX_DEPTH: usize = 64;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Namespace {
    Rss,
    Metadata,
    Other,
}

struct Element {
    local: String,
    namespace: Namespace,
    text: String,
    base: Url,
    torrent_type: bool,
}

#[derive(Default)]
struct PartialItem {
    guid: Option<String>,
    title: Option<String>,
    description: Option<String>,
    detail_url: Option<String>,
    enclosure_url: Option<String>,
    torrent_link: Option<String>,
    published_at: Option<String>,
    categories: Vec<String>,
    attributes: ItemAttributes,
    freeleech: Option<bool>,
    hr: Option<bool>,
}

/// Accept only a complete RSS 2.0 document. Errors deliberately contain no XML
/// snippets or URLs: both can carry a user's passkey.
pub fn parse_download_feed(xml: &[u8], base_url: &str) -> Result<FetchedFeed, String> {
    if xml.len() > MAX_FEED_BYTES {
        return Err("RSS 解压后超过 8 MiB，整次检查已拒绝".into());
    }
    let xml = xml.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(xml);
    let xml = std::str::from_utf8(xml)
        .map_err(|_| "RSS 编码不受支持；目前仅支持 UTF-8 / UTF-8 BOM".to_string())?;
    if xml.chars().any(|c| !is_xml_char(c)) {
        return Err("RSS 包含 XML 不允许的字符".into());
    }
    let base = safe_url(base_url, None).ok_or("RSS 地址必须为不含用户信息的 HTTP(S) 地址")?;
    let mut reader = NsReader::from_str(xml);
    reader.config_mut().check_comments = true;
    reader.config_mut().expand_empty_elements = true;
    let mut stack: Vec<Element> = Vec::new();
    let mut feed = FetchedFeed::default();
    let mut item: Option<PartialItem> = None;
    let mut root_seen = false;
    let mut root_closed = false;
    let mut channel_seen = false;
    let mut channel_closed = false;
    let mut declaration_seen = false;
    let mut item_count = 0usize;
    let mut seen = HashSet::new();

    loop {
        let (namespace, event) = reader
            .read_resolved_event()
            .map_err(|_| "RSS XML 损坏或结构未完整结束".to_string())?;
        let namespace = match namespace {
            ResolveResult::Unbound => Namespace::Rss,
            ResolveResult::Bound(uri) => match uri.as_ref() {
                b"http://torznab.com/schemas/2015/feed"
                | b"https://torznab.com/schemas/2015/feed"
                | b"http://www.newznab.com/DTD/2010/feeds/attributes/"
                | b"https://www.newznab.com/DTD/2010/feeds/attributes/"
                | b"http://xmlns.ezrss.it/0.1/" => Namespace::Metadata,
                _ => Namespace::Other,
            },
            ResolveResult::Unknown(_) => return Err("RSS 使用了未声明的 XML 命名空间".into()),
        };
        match event {
            Event::Decl(declaration) => {
                if root_seen || declaration_seen || !stack.is_empty() {
                    return Err("RSS XML 声明位置无效".into());
                }
                declaration_seen = true;
                if declaration
                    .version()
                    .map_err(|_| "RSS XML 声明无效")?
                    .as_ref()
                    != b"1.0"
                {
                    return Err("RSS 目前仅支持 XML 1.0".into());
                }
                if let Some(encoding) = declaration.encoding() {
                    let encoding = encoding.map_err(|_| "RSS XML 编码声明无效")?;
                    if !encoding.eq_ignore_ascii_case(b"utf-8") {
                        return Err("RSS 编码不受支持；目前仅支持 UTF-8 / UTF-8 BOM".into());
                    }
                }
            }
            Event::DocType(_) => return Err("RSS 不允许 DTD 或外部实体声明".into()),
            Event::Start(start) => {
                if stack.len() >= MAX_DEPTH {
                    return Err("RSS XML 嵌套超过 64 层，整次检查已拒绝".into());
                }
                let attributes = read_attributes(&start)?;
                // Validate attribute namespaces too; quick-xml resolves element
                // prefixes automatically, but does not reject unknown attributes.
                for attribute in start.attributes() {
                    let attribute = attribute.map_err(|_| "RSS XML 属性无效")?;
                    if attribute.key.as_namespace_binding().is_none()
                        && matches!(
                            reader.resolve_attribute(attribute.key).0,
                            ResolveResult::Unknown(_)
                        )
                    {
                        return Err("RSS 使用了未声明的 XML 属性命名空间".into());
                    }
                }
                let local = std::str::from_utf8(start.local_name().as_ref())
                    .map_err(|_| "RSS XML 元素名称无效")?
                    .to_owned();
                match stack.len() {
                    0 => {
                        if root_seen || root_closed || namespace != Namespace::Rss || local != "rss"
                        {
                            return Err(
                                "响应不是完整 RSS 2.0；请检查是否为登录页或 Atom 订阅".into()
                            );
                        }
                        if attribute(&attributes, "version").is_some_and(|v| v != "2.0") {
                            return Err("RSS 版本不受支持；目前仅支持 RSS 2.0 / Torznab".into());
                        }
                        root_seen = true;
                    }
                    1 => {
                        if local == "channel" && namespace == Namespace::Rss {
                            if channel_seen {
                                return Err("RSS 必须且只能包含一个 channel".into());
                            }
                            channel_seen = true;
                        } else if namespace == Namespace::Rss {
                            return Err("RSS 根元素下缺少有效 channel 结构".into());
                        }
                    }
                    _ => {}
                }
                if namespace == Namespace::Rss && (local == "rss" || local == "channel") {
                    let expected_depth = if local == "rss" { 0 } else { 1 };
                    if stack.len() != expected_depth {
                        return Err("RSS 根元素或 channel 结构嵌套无效".into());
                    }
                }
                if namespace == Namespace::Rss && local == "item" {
                    if stack.len() != 2
                        || stack[1].local != "channel"
                        || stack[1].namespace != Namespace::Rss
                    {
                        return Err("RSS item 必须直接属于 channel".into());
                    }
                    item_count += 1;
                    if item_count > MAX_ITEMS {
                        return Err("RSS 超过 1000 个条目，整次检查已拒绝".into());
                    }
                    item = Some(PartialItem::default());
                }
                let inherited_base = stack.last().map(|element| &element.base).unwrap_or(&base);
                let element_base = match attribute(&attributes, "xml:base") {
                    Some(value) => safe_url(value, Some(inherited_base))
                        .ok_or("RSS xml:base 必须为不含用户信息的 HTTP(S) 地址")?,
                    None => inherited_base.clone(),
                };
                let torrent_type = attribute(&attributes, "type").is_some_and(is_torrent_type);
                if stack.len() == 3 {
                    if let Some(item) = item.as_mut() {
                        if local == "enclosure" && namespace == Namespace::Rss {
                            let allowed_type = attribute(&attributes, "type").is_none_or(|mime| {
                                mime.trim().is_empty()
                                    || is_torrent_type(mime)
                                    || mime
                                        .split(';')
                                        .next()
                                        .unwrap_or("")
                                        .trim()
                                        .eq_ignore_ascii_case("application/octet-stream")
                            });
                            if allowed_type {
                                if let Some(url) = attribute(&attributes, "url") {
                                    if let Some(url) = safe_url(url, Some(&element_base)) {
                                        item.enclosure_url.get_or_insert_with(|| url.to_string());
                                    } else {
                                        push_hint(
                                            &mut item.attributes,
                                            "种子附件地址无效或使用不支持的协议",
                                        );
                                    }
                                }
                                if let Some(length) = attribute(&attributes, "length") {
                                    fill_metadata(item, "size", length);
                                }
                            } else {
                                push_hint(&mut item.attributes, "附件类型不是种子文件");
                            }
                        } else if local == "attr" && namespace == Namespace::Metadata {
                            if let (Some(name), Some(value)) = (
                                attribute(&attributes, "name"),
                                attribute(&attributes, "value"),
                            ) {
                                fill_metadata(item, name, value);
                            }
                        }
                    }
                }
                stack.push(Element {
                    local,
                    namespace,
                    text: String::new(),
                    base: element_base,
                    torrent_type,
                });
            }
            Event::End(_) => {
                let element = stack.pop().ok_or("RSS XML 包含无对应开始的结束元素")?;
                if element.local == "item"
                    && element.namespace == Namespace::Rss
                    && stack.len() == 2
                {
                    let partial = item.take().ok_or("RSS item 结构无效")?;
                    let normalized = normalize_item(partial, &base, &mut feed.warnings, item_count);
                    if seen.insert(normalized.item_key.clone()) {
                        feed.items.push(normalized);
                    } else {
                        feed.warnings.push(format!(
                            "第 {item_count} 条与同批条目身份重复，已保留首次出现的条目"
                        ));
                    }
                } else if stack.len() == 3 {
                    if let Some(item) = item.as_mut() {
                        finish_item_field(item, &element);
                    }
                } else if stack.len() == 2
                    && stack[1].local == "channel"
                    && element.namespace == Namespace::Rss
                    && element.local == "title"
                {
                    feed.title = nonempty(element.text.clone());
                }
                if element.local == "channel"
                    && element.namespace == Namespace::Rss
                    && stack.len() == 1
                {
                    channel_closed = true;
                }
                if element.local == "rss" && stack.is_empty() {
                    root_closed = true;
                }
                // Descriptions can contain child markup. They remain inert text;
                // a nested tag can never become an item field or metadata source.
                if stack.len() >= 4 {
                    if let Some(parent) = stack.last_mut() {
                        parent.text.push_str(&element.text);
                    }
                }
            }
            Event::Text(text) => {
                let text = text.unescape().map_err(|_| "RSS XML 含无效实体或转义")?;
                append_text(&mut stack, &text)?;
            }
            Event::CData(text) => {
                if stack.is_empty() {
                    return Err("RSS XML 根元素外不能包含 CDATA".into());
                }
                let text = text.decode().map_err(|_| "RSS CDATA 编码无效")?;
                append_text(&mut stack, &text)?;
            }
            Event::Eof => {
                if !root_seen
                    || !root_closed
                    || !channel_seen
                    || !channel_closed
                    || !stack.is_empty()
                    || item.is_some()
                {
                    return Err("RSS 缺少完整的 rss/channel 结构；不会建立历史基线".into());
                }
                break;
            }
            Event::Comment(_) | Event::PI(_) => {}
            Event::Empty(_) => unreachable!("empty XML elements are expanded"),
        }
    }
    Ok(feed)
}

/// Expose only the origin; private trackers also embed tokens in path segments.
pub fn redact_url(raw: &str) -> String {
    match Url::parse(raw) {
        Ok(url) if matches!(url.scheme(), "http" | "https") && url.host().is_some() => {
            format!("{}/…", url.origin().ascii_serialization())
        }
        _ => "地址已隐藏".into(),
    }
}

fn is_xml_char(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r' | '\u{20}'..='\u{d7ff}' | '\u{e000}'..='\u{fffd}' | '\u{10000}'..='\u{10ffff}')
}

fn append_text(stack: &mut [Element], text: &str) -> Result<(), String> {
    if text.chars().any(|c| !is_xml_char(c)) {
        return Err("RSS 包含 XML 不允许的字符实体".into());
    }
    if let Some(element) = stack.last_mut() {
        element.text.push_str(text);
    } else if !text.trim().is_empty() {
        return Err("RSS 根元素外包含非空文本".into());
    }
    Ok(())
}

fn read_attributes(start: &BytesStart<'_>) -> Result<Vec<(String, String)>, String> {
    start
        .attributes()
        .map(|attribute| {
            let attribute = attribute.map_err(|_| "RSS XML 包含无效或重复属性")?;
            let name = std::str::from_utf8(attribute.key.as_ref())
                .map_err(|_| "RSS XML 属性名称无效")?
                .to_string();
            let value = attribute
                .unescape_value()
                .map_err(|_| "RSS XML 属性含无效实体或转义")?
                .into_owned();
            if value.chars().any(|c| !is_xml_char(c)) {
                return Err("RSS XML 属性包含不允许的字符".into());
            }
            Ok((name, value))
        })
        .collect()
}

fn attribute<'a>(attributes: &'a [(String, String)], key: &str) -> Option<&'a str> {
    attributes
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
}

fn safe_url(raw: &str, base: Option<&Url>) -> Option<Url> {
    let raw = raw.trim();
    if raw.is_empty() || raw.chars().any(|c| c.is_control()) {
        return None;
    }
    let mut url = match base {
        Some(base) => base.join(raw).ok()?,
        None => Url::parse(raw).ok()?,
    };
    if !matches!(url.scheme(), "http" | "https")
        || url.host().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    // A fragment is never sent in an HTTP request. Query pairs, including their
    // order and repetition, are deliberately retained in the locator identity.
    url.set_fragment(None);
    Some(url)
}

fn is_torrent_type(mime: &str) -> bool {
    matches!(
        mime.split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "application/x-bittorrent" | "application/bittorrent"
    )
}

fn nonempty(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}

fn finish_item_field(item: &mut PartialItem, element: &Element) {
    if element.namespace == Namespace::Rss {
        match element.local.as_str() {
            "guid" => item.guid = nonempty(element.text.trim().to_string()),
            "title" => item.title = nonempty(element.text.clone()),
            "description" => item.description = nonempty(element.text.clone()),
            "category" => {
                if let Some(category) = nonempty(element.text.clone()) {
                    item.categories.push(category);
                }
            }
            "pubDate" => item.published_at = parse_timestamp(&element.text),
            "link" => {
                if let Some(url) = safe_url(&element.text, Some(&element.base)) {
                    if element.torrent_type || url.path().to_ascii_lowercase().ends_with(".torrent")
                    {
                        item.torrent_link.get_or_insert_with(|| url.to_string());
                    } else {
                        item.detail_url.get_or_insert_with(|| url.to_string());
                    }
                } else if !element.text.trim().is_empty() {
                    push_hint(&mut item.attributes, "详情或种子链接无效或使用不支持的协议");
                }
            }
            _ => fill_metadata(item, &element.local, &element.text),
        }
    } else if element.namespace == Namespace::Metadata && element.local != "attr" {
        fill_metadata(item, &element.local, &element.text);
    }
}

fn fill_metadata(item: &mut PartialItem, name: &str, value: &str) {
    let name = name.trim().to_ascii_lowercase();
    let value = value.trim();
    let attributes = &mut item.attributes;
    match name.as_str() {
        "size" | "contentlength" | "filesize" => {
            if let Ok(value) = value.parse::<u64>() {
                attributes.size_bytes = Some(value);
            }
        }
        "seeders" | "seeds" | "seeder" => {
            if let Ok(value) = value.parse::<u32>() {
                attributes.seeders = Some(value);
            }
        }
        "leechers" | "leech" | "leecher" => {
            if let Ok(value) = value.parse::<u32>() {
                attributes.leechers = Some(value);
            }
        }
        // Torznab's peers is the total swarm, not the leecher count. Keep it
        // unknown rather than incorrectly reporting all peers as downloaders.
        "downloadvolumefactor" => {
            if let Some(value) = finite_nonnegative(value) {
                attributes.download_volume_factor = Some(
                    attributes
                        .download_volume_factor
                        .map_or(value, |old| old.max(value)),
                );
            }
        }
        "uploadvolumefactor" => {
            if let Some(value) = finite_nonnegative(value) {
                attributes.upload_volume_factor = Some(value);
            }
        }
        "freeleech" | "free" => {
            if let Some(value) = parse_bool(value) {
                item.freeleech = Some(item.freeleech.unwrap_or(true) && value);
            }
        }
        "minimumratio" | "minratio" => {
            if let Some(value) = finite_nonnegative(value) {
                attributes.minimum_ratio =
                    Some(attributes.minimum_ratio.map_or(value, |old| old.max(value)));
            }
        }
        "minimumseedtime" | "minseedtime" => {
            if let Ok(value) = value.parse::<u64>() {
                attributes.minimum_seed_time = Some(
                    attributes
                        .minimum_seed_time
                        .map_or(value, |old| old.max(value)),
                );
            }
        }
        "hr" | "hitandrun" | "hit_and_run" => {
            if let Some(value) = parse_bool(value) {
                item.hr = Some(item.hr.unwrap_or(false) || value);
            }
        }
        "freeuntil" | "free_until" | "freeleechend" | "freeendtime" | "free_end_timestamp" => {
            let timestamp = value
                .parse::<i64>()
                .ok()
                .filter(|value| *value >= 0)
                .or_else(|| parse_datetime(value).map(|datetime| datetime.timestamp()));
            if let Some(timestamp) = timestamp {
                attributes.free_until = Some(
                    attributes
                        .free_until
                        .map_or(timestamp, |old| old.min(timestamp)),
                );
            }
        }
        _ => {}
    }
}

fn finite_nonnegative(value: &str) -> Option<f64> {
    value
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite() && *value >= 0.0)
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "y" => Some(true),
        "false" | "0" | "no" | "n" => Some(false),
        _ => None,
    }
}

fn parse_datetime(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc2822(value.trim())
        .or_else(|_| DateTime::parse_from_rfc3339(value.trim()))
        .ok()
        .map(|datetime| datetime.with_timezone(&Utc))
}

fn parse_timestamp(value: &str) -> Option<String> {
    parse_datetime(value).map(|datetime| datetime.to_rfc3339())
}

fn push_hint(attributes: &mut ItemAttributes, hint: &str) {
    if !attributes.hints.iter().any(|existing| existing == hint) {
        attributes.hints.push(hint.to_string());
    }
}

fn normalize_item(
    mut item: PartialItem,
    base: &Url,
    warnings: &mut Vec<String>,
    index: usize,
) -> NormalizedItem {
    item.attributes.source = "rss".into();
    item.attributes.observed_at = Utc::now().to_rfc3339();
    if item.freeleech == Some(false) {
        // A contradictory explicit non-free flag must never turn into free.
        if item
            .attributes
            .download_volume_factor
            .is_none_or(|value| value == 0.0)
        {
            item.attributes.download_volume_factor = Some(1.0);
        }
    } else if item.freeleech == Some(true) {
        item.attributes.download_volume_factor.get_or_insert(0.0);
    }
    item.attributes.hr = if item.hr == Some(true)
        || item
            .attributes
            .minimum_ratio
            .is_some_and(|value| value > 0.0)
        || item
            .attributes
            .minimum_seed_time
            .is_some_and(|value| value > 0)
    {
        Some(true)
    } else if item.hr == Some(false)
        || (item.attributes.minimum_ratio == Some(0.0)
            && item.attributes.minimum_seed_time == Some(0))
    {
        Some(false)
    } else {
        None
    };
    let title = item.title.unwrap_or_else(|| "未命名资源".into());
    let textual = format!(
        "{} {} {}",
        title,
        item.description.as_deref().unwrap_or(""),
        item.categories.join(" ")
    )
    .to_ascii_lowercase();
    if ["[free]", "freeleech", "free leech", "免费", "零魔"]
        .iter()
        .any(|marker| textual.contains(marker))
    {
        push_hint(&mut item.attributes, "文本含免费线索，不能作为免费证据");
    }
    if ["h&r", "hit and run", "hitandrun"]
        .iter()
        .any(|marker| textual.contains(marker))
    {
        push_hint(
            &mut item.attributes,
            "文本含 H&R 线索，不能作为 H&R 判定证据",
        );
    }
    let mut download_url = item.enclosure_url.or(item.torrent_link);
    let site_torrent_id = item
        .detail_url
        .as_deref()
        .and_then(|url| nexus_torrent_id(url, base))
        .or_else(|| {
            download_url
                .as_deref()
                .and_then(|url| nexus_torrent_id(url, base))
        });
    let item_key = if let Some(guid) = item.guid.as_deref() {
        fingerprint("guid", &[guid])
    } else if let Some(id) = site_torrent_id.as_deref() {
        fingerprint("site", &[&base.origin().ascii_serialization(), id])
    } else if let Some(url) = item.detail_url.as_deref() {
        fingerprint("detail", &[url])
    } else if let Some(url) = download_url.as_deref() {
        fingerprint("download", &[url])
    } else {
        warnings.push(format!(
            "第 {index} 条缺少 GUID 和可靠定位，仅保留展示，不会自动处理"
        ));
        download_url = None;
        fingerprint(
            "unidentified",
            &[
                &title,
                item.description.as_deref().unwrap_or(""),
                item.published_at.as_deref().unwrap_or(""),
            ],
        )
    };
    if download_url.is_none() && site_torrent_id.is_none() {
        push_hint(&mut item.attributes, "缺少可验证的种子下载定位");
    }
    NormalizedItem {
        item_key,
        guid: item.guid,
        title,
        detail_url: item.detail_url,
        download_url,
        site_torrent_id,
        published_at: item.published_at,
        categories: item.categories,
        description: item.description,
        attributes: item.attributes,
    }
}

fn fingerprint(kind: &str, components: &[&str]) -> String {
    let mut digest = Sha256::new();
    for component in components {
        digest.update((component.len() as u64).to_be_bytes());
        digest.update(component.as_bytes());
    }
    format!("{kind}:{:x}", digest.finalize())
}

/// Only conventional NexusPHP locators with one unambiguous decimal ID are
/// recognized. Generic /detail paths (including arbitrary M-Team lookalikes)
/// remain opaque, and a different origin is never assigned this feed's site ID.
fn nexus_torrent_id(raw: &str, base: &Url) -> Option<String> {
    let url = safe_url(raw, None)?;
    if url.origin() != base.origin() {
        return None;
    }
    let filename = url.path().rsplit('/').next()?;
    if !matches!(filename, "details.php" | "download.php") {
        return None;
    }
    let mut ids = url.query_pairs().filter(|(name, _)| name == "id");
    let (_, id) = ids.next()?;
    if ids.next().is_some()
        || id.is_empty()
        || id.len() > 20
        || !id.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let id = id.parse::<u64>().ok()?;
    (id > 0).then(|| id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "https://tracker.test/rss.php?passkey=private";

    fn feed(body: &str) -> FetchedFeed {
        parse_download_feed(format!("<rss version=\"2.0\" xmlns:t=\"http://torznab.com/schemas/2015/feed\"><channel>{body}</channel></rss>").as_bytes(), BASE).unwrap_or_else(|error| panic!("{error}"))
    }

    #[test]
    fn requires_a_complete_rss_channel_even_when_empty() {
        for xml in [
            "",
            "<rss/>",
            "<rss><channel>",
            "<rss><channel><item/></channel>",
            "<html><body>login</body></html>",
            "<feed xmlns=\"http://www.w3.org/2005/Atom\"/>",
            "<rss><channel/></rss><rss><channel/></rss>",
            "<rss><channel/><channel/></rss>",
            "<rss><channel></rss>",
            "<rss><channel/></rss>trailing",
            "<rss><channel><x><item/></x></channel></rss>",
        ] {
            assert!(
                parse_download_feed(xml.as_bytes(), BASE).is_err(),
                "accepted invalid structure"
            );
        }
        assert!(feed("").items.is_empty());
        let bom = b"\xef\xbb\xbf<?xml version=\"1.0\" encoding=\"UTF-8\"?><rss><channel/></rss>";
        assert!(parse_download_feed(bom, BASE).is_ok());
    }

    #[test]
    fn rejects_unsafe_entities_other_encodings_and_excess_items_as_a_batch() {
        for xml in [
            "<?xml version=\"1.0\" encoding=\"GBK\"?><rss><channel/></rss>",
            "<!DOCTYPE rss SYSTEM \"file:///etc/passwd\"><rss><channel/></rss>",
            "<!DOCTYPE rss [<!ENTITY x \"private\">]><rss><channel/></rss>",
            "<rss><channel><title>&notdeclared;</title></channel></rss>",
            "<rss><channel><title>&#0;</title></channel></rss>",
            "<rss><channel><title>\u{0}</title></channel></rss>",
            "<rss><channel><item><a:attr name=\"hr\" value=\"false\"/></item></channel></rss>",
            "<rss><channel><item x:bad=\"value\"/></channel></rss>",
            "<rss><channel><item id=\"1\" id=\"2\"/></channel></rss>",
        ] {
            assert!(parse_download_feed(xml.as_bytes(), BASE).is_err());
        }
        let xml = format!(
            "<rss><channel>{}</channel></rss>",
            "<item><guid>same</guid></item>".repeat(1001)
        );
        assert!(
            parse_download_feed(xml.as_bytes(), BASE)
                .err()
                .unwrap()
                .contains("1000")
        );
        let xml = format!(
            "<rss><channel>{}</channel></rss>",
            "<item><guid>same</guid></item>".repeat(1000)
        );
        assert!(parse_download_feed(xml.as_bytes(), BASE).is_ok());
    }

    #[test]
    fn preserves_text_and_resolves_enclosures_without_exposing_credentials_in_keys() {
        let result = feed(
            r#"<title>Feed &amp; resources</title><item><guid>private-guid</guid><title>  ＷＥＢ Straße <![CDATA[<release>]]>  </title><description><![CDATA[<b>[FREE]</b> H&amp;R]]></description><category>Documentary</category><link>details.php?id=12&amp;passkey=private</link><enclosure url="download.php?id=12&amp;passkey=private" length="4096" type="application/x-bittorrent"/><pubDate>Tue, 08 Sep 2026 01:00:00 +0000</pubDate></item>"#,
        );
        assert_eq!(result.title.as_deref(), Some("Feed & resources"));
        let item = &result.items[0];
        assert_eq!(item.title, "  ＷＥＢ Straße <release>  ");
        assert_eq!(item.description.as_deref(), Some("<b>[FREE]</b> H&amp;R"));
        assert_eq!(item.categories, ["Documentary"]);
        assert_eq!(item.site_torrent_id.as_deref(), Some("12"));
        assert_eq!(
            item.download_url.as_deref(),
            Some("https://tracker.test/download.php?id=12&passkey=private")
        );
        assert_eq!(item.attributes.size_bytes, Some(4096));
        assert!(item.item_key.starts_with("guid:"));
        assert!(!item.item_key.contains("private"));
        assert_eq!(
            item.published_at.as_deref(),
            Some("2026-09-08T01:00:00+00:00")
        );
    }

    #[test]
    fn fallback_identity_preserves_queries_and_is_independent_of_title_and_date() {
        let first = feed(
            r#"<item><title>Old</title><link>/details.php?id=003&amp;passkey=one</link></item>"#,
        );
        let rotated = feed(
            r#"<item><title>Renamed</title><link>/details.php?id=3&amp;passkey=two</link><pubDate>Tue, 08 Sep 2026 02:00:00 +0000</pubDate></item>"#,
        );
        assert_eq!(first.items[0].item_key, rotated.items[0].item_key);
        let query = feed(
            r#"<item><link>/resource?id=1</link></item><item><link>/resource?id=2</link></item>"#,
        );
        assert_eq!(query.items.len(), 2);
        assert_ne!(query.items[0].item_key, query.items[1].item_key);
        let downloads = feed(
            r#"<item><enclosure url="/fetch?signature=a"/></item><item><enclosure url="/fetch?signature=b"/></item>"#,
        );
        assert_eq!(downloads.items.len(), 2);
        let anonymous = feed("<item><title>Unknown</title></item>");
        assert!(anonymous.items[0].item_key.starts_with("unidentified:"));
        assert!(anonymous.items[0].download_url.is_none());
        assert!(!anonymous.warnings.is_empty());
    }

    #[test]
    fn only_torrent_links_are_download_locations() {
        let result = feed(
            r#"<item><guid>1</guid><link>https://tracker.test/detail/123</link></item><item><guid>2</guid><link>/file.torrent?passkey=secret</link></item><item><guid>3</guid><link type="application/x-bittorrent">/fetch?token=secret</link></item><item><guid>4</guid><enclosure url="https://user:pass@tracker.test/fetch"/></item><item><guid>5</guid><enclosure url="javascript:alert(1)"/></item><item><guid>6</guid><enclosure url="/episode.mp3" type="audio/mpeg"/></item><item><guid>7</guid><link>https://other.test/details.php?id=77</link></item><item><guid>8</guid><link>/details.php?id=1&amp;id=2</link></item>"#,
        );
        assert!(result.items[0].download_url.is_none());
        assert!(result.items[0].site_torrent_id.is_none());
        assert!(result.items[1].download_url.is_some());
        assert!(result.items[2].download_url.is_some());
        for item in &result.items[3..] {
            assert!(item.download_url.is_none());
        }
        assert!(result.items[6].site_torrent_id.is_none());
        assert!(result.items[7].site_torrent_id.is_none());
    }

    #[test]
    fn namespace_metadata_is_evidence_and_text_is_only_a_hint() {
        let result = feed(
            r#"<item><guid>1</guid><title>[FREE] H&amp;R</title><t:attr name="seeders" value="12"/><t:attr name="freeleech" value="true"/><t:attr name="hr" value="false"/></item><item><guid>2</guid><title>[FREE]</title><description>freeleech and hit and run</description></item><item><guid>3</guid><t:attr name="minimumratio" value="0"/></item><item><guid>4</guid><t:attr name="minimumratio" value="0"/><t:attr name="minimumseedtime" value="0"/></item><item><guid>5</guid><t:attr name="hr" value="false"/><t:attr name="minimumseedtime" value="3600"/></item><item xmlns:fake="https://untrusted.test/schema"><guid>6</guid><fake:attr name="freeleech" value="true"/><fake:hr>false</fake:hr></item>"#,
        );
        assert_eq!(result.items[0].attributes.seeders, Some(12));
        assert_eq!(result.items[0].attributes.download_volume_factor, Some(0.0));
        assert_eq!(result.items[0].attributes.hr, Some(false));
        assert_eq!(result.items[1].attributes.download_volume_factor, None);
        assert_eq!(result.items[1].attributes.hr, None);
        assert_eq!(result.items[1].attributes.hints.len(), 3);
        assert_eq!(result.items[2].attributes.hr, None);
        assert_eq!(result.items[3].attributes.hr, Some(false));
        assert_eq!(result.items[4].attributes.hr, Some(true));
        assert_eq!(result.items[5].attributes.download_volume_factor, None);
        assert_eq!(result.items[5].attributes.hr, None);
    }

    #[test]
    fn false_or_conflicting_promotions_and_nonfinite_numbers_do_not_become_free() {
        let result = feed(
            r#"<item><guid>1</guid><t:attr name="freeleech" value="false"/></item><item><guid>2</guid><t:attr name="freeleech" value="true"/><t:attr name="downloadvolumefactor" value="0.5"/></item><item><guid>3</guid><t:attr name="freeleech" value="false"/><t:attr name="downloadvolumefactor" value="0"/></item><item><guid>4</guid><t:attr name="downloadvolumefactor" value="NaN"/><t:attr name="uploadvolumefactor" value="inf"/><t:attr name="seeders" value="-1"/><t:attr name="minimumratio" value="-1"/></item>"#,
        );
        assert_eq!(result.items[0].attributes.download_volume_factor, Some(1.0));
        assert_eq!(result.items[1].attributes.download_volume_factor, Some(0.5));
        assert_eq!(result.items[2].attributes.download_volume_factor, Some(1.0));
        assert_eq!(result.items[3].attributes.download_volume_factor, None);
        assert_eq!(result.items[3].attributes.upload_volume_factor, None);
        assert_eq!(result.items[3].attributes.seeders, None);
        assert_eq!(result.items[3].attributes.hr, None);
    }

    #[test]
    fn nested_description_markup_cannot_supply_metadata() {
        let result = feed(
            r#"<item><guid>1</guid><title>Example</title><description><b>Hi</b><t:attr name="hr" value="false"/><title>[FREE]</title></description></item>"#,
        );
        assert_eq!(result.items[0].title, "Example");
        assert_eq!(result.items[0].description.as_deref(), Some("Hi[FREE]"));
        assert_eq!(result.items[0].attributes.hr, None);
        assert_eq!(result.items[0].attributes.download_volume_factor, None);
    }

    #[test]
    fn redaction_hides_path_query_fragment_and_userinfo() {
        assert_eq!(
            redact_url(
                "https://user:password@tracker.test:8443/private/token/rss?key=secret#password"
            ),
            "https://tracker.test:8443/…"
        );
        assert_eq!(
            redact_url("https://[::1]:8443/private"),
            "https://[::1]:8443/…"
        );
        assert_eq!(redact_url("private-token"), "地址已隐藏");
    }
}
