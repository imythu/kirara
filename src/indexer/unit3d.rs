//! Unit3D HTML search indexer distilled from PT-depiler `schemas/Unit3D.ts`.
//!
//! Cookie-authenticated sites only. Keyword search hits `/torrents/?name=...`.
//! Download uses `/torrents/download/{id}` on the same origin.
//! Site adaptation notes: `doc/ptd-site-rules.md`.

use std::collections::HashSet;
use std::sync::Arc;

use chrono::{DateTime, FixedOffset, NaiveDateTime, TimeZone, Utc};
use reqwest::header::{COOKIE, HeaderMap, HeaderValue};
use reqwest::{Client, StatusCode, Url};
use scraper::{ElementRef, Html, Selector};

use crate::site::SiteAuth;

use super::access::OriginAccessGate;
use super::{
    IndexerAdapter, IndexerCapabilities, IndexerError, IndexerFuture, SearchRequest, SearchResult,
    endpoint_url, ensure_result_site, http_error, normalize_base_url, rate_limit_error_from_body,
    read_torrent_response, resolve_same_origin_url, response_is_authentication_page, same_origin,
};

pub struct Unit3DIndexer {
    site_id: i64,
    site_name: String,
    base_url: Url,
    cookie: HeaderValue,
    request_headers: HeaderMap,
    client: Client,
    access_gate: Arc<OriginAccessGate>,
}

impl Unit3DIndexer {
    pub(crate) fn new(
        site_id: i64,
        site_name: String,
        base_url: &str,
        auth: SiteAuth,
        request_headers: HeaderMap,
        client: Client,
        access_gate: Arc<OriginAccessGate>,
    ) -> Result<Self, IndexerError> {
        let base_url = normalize_base_url(base_url)?;
        let cookie = match auth {
            SiteAuth::Cookie { cookie } | SiteAuth::CookiePasskey { cookie, .. } => cookie,
            _ => {
                return Err(IndexerError::Configuration(
                    "Unit3D search requires cookie authentication".to_string(),
                ));
            }
        };
        let cookie = cookie.trim();
        if cookie.is_empty() {
            return Err(IndexerError::Configuration(
                "Unit3D cookie cannot be empty".to_string(),
            ));
        }
        let mut cookie = HeaderValue::from_str(cookie).map_err(|_| {
            IndexerError::Configuration(
                "Unit3D cookie contains invalid header characters".to_string(),
            )
        })?;
        cookie.set_sensitive(true);

        Ok(Self {
            site_id,
            site_name: site_name.trim().to_string(),
            base_url,
            cookie,
            request_headers,
            client,
            access_gate,
        })
    }

    fn headers(&self) -> HeaderMap {
        let mut headers = self.request_headers.clone();
        headers.insert(COOKIE, self.cookie.clone());
        headers
    }

    async fn search_html(&self, request: &SearchRequest) -> Result<Vec<SearchResult>, IndexerError> {
        let mut url = endpoint_url(&self.base_url, "/torrents/")?;
        {
            let mut pairs = url.query_pairs_mut();
            pairs.append_pair("name", &request.query);
            pairs.append_pair("perPage", &request.page_size.to_string());
            // Unit3D pages are 1-based.
            pairs.append_pair("page", &request.page.max(1).to_string());
        }
        let http_request = self.client.get(url).headers(self.headers());
        let response = self
            .access_gate
            .send_with_same_origin_redirects(&self.client, http_request, &self.base_url)
            .await?;
        let status = response.status();
        let final_url = response.url().clone();
        if !same_origin(&self.base_url, &final_url) {
            return Err(IndexerError::UnsafeUrl(
                "Unit3D search redirected to another origin".to_string(),
            ));
        }
        let body = response.text().await.map_err(http_error)?;
        if let Some(error) = rate_limit_error_from_body(&body) {
            return Err(error);
        }
        if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
            return Err(IndexerError::AuthenticationExpired(format!(
                "Unit3D returned HTTP {status}"
            )));
        }
        if !status.is_success() {
            return Err(IndexerError::Http(format!(
                "Unit3D returned HTTP {status}"
            )));
        }
        if response_is_authentication_page(&final_url, &body) {
            return Err(IndexerError::AuthenticationExpired(
                "Unit3D cookie is invalid or expired".to_string(),
            ));
        }
        parse_html_results(&body, self.site_id, &self.site_name, &self.base_url)
    }

    async fn fetch(&self, result: &SearchResult) -> Result<Vec<u8>, IndexerError> {
        let torrent_id = ensure_result_site(result, self.site_id)?;
        let url = endpoint_url(&self.base_url, &format!("/torrents/download/{torrent_id}"))?;
        let request = self.client.get(url).headers(self.headers());
        let response = self
            .access_gate
            .send_with_same_origin_redirects(&self.client, request, &self.base_url)
            .await?;
        read_torrent_response(response, &self.base_url).await
    }
}

impl IndexerAdapter for Unit3DIndexer {
    fn site_id(&self) -> i64 {
        self.site_id
    }

    fn site_name(&self) -> &str {
        &self.site_name
    }

    fn capabilities(&self) -> IndexerCapabilities {
        IndexerCapabilities {
            search: true,
            fetch_torrent: true,
            api_search: false,
            html_search: true,
        }
    }

    fn search<'a>(&'a self, request: &'a SearchRequest) -> IndexerFuture<'a, Vec<SearchResult>> {
        Box::pin(async move {
            let request = request.normalized()?;
            self.search_html(&request).await
        })
    }

    fn fetch_torrent<'a>(&'a self, result: &'a SearchResult) -> IndexerFuture<'a, Vec<u8>> {
        Box::pin(async move { self.fetch(result).await })
    }
}

pub(super) fn parse_html_results(
    html: &str,
    site_id: i64,
    site_name: &str,
    base_url: &Url,
) -> Result<Vec<SearchResult>, IndexerError> {
    let document = Html::parse_document(html);
    let row_selectors = [
        "div.torrent-search--list__results > table:first-of-type > tbody > tr",
        "div.torrent-search--list__results table > tbody > tr",
        "div.table-responsive table > tbody > tr",
        "table > tbody > tr",
    ];
    let mut rows: Vec<ElementRef<'_>> = Vec::new();
    for raw in row_selectors {
        if let Ok(selector) = Selector::parse(raw) {
            rows = document.select(&selector).collect();
            if !rows.is_empty() {
                break;
            }
        }
    }

    let title_selectors = [
        "a.torrent-search--list__name",
        "a.view-torrent",
        "a[href*='/torrents/']",
    ];
    let download_selectors = [
        "a[href*='/torrents/download/']",
        "a[href*='/download_check/']",
        "a[href*='/download/']",
    ];
    let mut results = Vec::new();
    let mut seen = HashSet::new();

    for row in rows {
        let mut selected: Option<(String, String, String)> = None;
        for raw in title_selectors {
            let Ok(selector) = Selector::parse(raw) else {
                continue;
            };
            for link in row.select(&selector) {
                let Some(href) = link.value().attr("href") else {
                    continue;
                };
                if href.contains("/download") {
                    continue;
                }
                if href.contains("://")
                    && let Ok(absolute) = Url::parse(href)
                    && !same_origin(base_url, &absolute)
                {
                    continue;
                }
                let Some(torrent_id) = extract_unit3d_torrent_id(href) else {
                    continue;
                };
                let title = normalize_text(link.text());
                if title.is_empty() {
                    continue;
                }
                selected = Some((torrent_id, title, href.to_string()));
                break;
            }
            if selected.is_some() {
                break;
            }
        }
        let Some((torrent_id, title, detail_href)) = selected else {
            continue;
        };
        if !seen.insert(torrent_id.clone()) {
            continue;
        }

        let text = normalize_text(row.text());
        let size = extract_size_from_row(&row).or_else(|| extract_size_from_text(&text));
        let seeders = extract_count_near_icon(&row, &["fa-arrow-up", "seeders", "text-green"])
            .or_else(|| extract_trailing_number(&text, &["seeders", "做种"]))
            .unwrap_or(0);
        let leechers = extract_count_near_icon(&row, &["fa-arrow-down", "leechers", "text-red"])
            .or_else(|| extract_trailing_number(&text, &["leechers", "下载"]))
            .unwrap_or(0);
        let publish_time = extract_publish_time(&row);

        let result = SearchResult {
            site_id,
            source_site: site_name.to_string(),
            torrent_id: torrent_id.clone(),
            title,
            detail_url: Some(detail_href),
            download_locator: Some(torrent_id),
            magnet: None,
            size: size.unwrap_or(0),
            seeders,
            leechers,
            publish_time,
        }
        .sanitized_for_base(base_url)?;

        // Confirm a download link exists when the row exposes one.
        let mut has_download = false;
        for raw in download_selectors {
            if let Ok(selector) = Selector::parse(raw)
                && row.select(&selector).next().is_some()
            {
                has_download = true;
                break;
            }
        }
        if !has_download {
            // Still accept: some themes hide the download control until hover.
            let _ = resolve_same_origin_url(
                base_url,
                &format!("/torrents/download/{}", result.torrent_id),
            )?;
        }
        results.push(result);
    }

    Ok(results)
}

fn extract_unit3d_torrent_id(href: &str) -> Option<String> {
    let url = Url::parse("https://unit3d.invalid").ok()?.join(href).ok()?;
    let mut parts = url.path().split('/').filter(|part| !part.is_empty());
    if parts.next()? != "torrents" {
        return None;
    }
    let id = parts.next()?.trim();
    if id.eq_ignore_ascii_case("download") || id.eq_ignore_ascii_case("download_check") {
        return None;
    }
    if id.is_empty() || !id.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    Some(id.to_string())
}

fn normalize_text(parts: impl Iterator<Item = impl AsRef<str>>) -> String {
    parts
        .flat_map(|part| {
            part.as_ref()
                .split_whitespace()
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn extract_size_from_row(row: &ElementRef<'_>) -> Option<u64> {
    for raw in [
        "td.torrent-search--list__size",
        "span.text-blue",
        "td[class*='size']",
    ] {
        let Ok(selector) = Selector::parse(raw) else {
            continue;
        };
        for element in row.select(&selector) {
            if let Some(size) = extract_size_from_text(&element.text().collect::<String>()) {
                return Some(size);
            }
        }
    }
    None
}

fn extract_size_from_text(text: &str) -> Option<u64> {
    let scrubbed = text.replace(',', "");
    let expression = regex::Regex::new(r"(?i)([0-9][0-9,.]*)\s*(bytes?|[kmgtpez]i?b)").ok()?;
    let captures = expression.captures(&scrubbed)?;
    crate::site::nexusphp::size_from_parts(captures.get(1)?.as_str(), captures.get(2)?.as_str())
}

fn extract_count_near_icon(row: &ElementRef<'_>, needles: &[&str]) -> Option<u32> {
    for needle in needles {
        for raw in [
            format!("td[class*='{needle}']"),
            format!("span[class*='{needle}']"),
            format!("a[href*='/peers'] span[class*='{needle}']"),
        ] {
            let Ok(selector) = Selector::parse(&raw) else {
                continue;
            };
            for element in row.select(&selector) {
                if let Some(value) = first_u32(&element.text().collect::<String>()) {
                    return Some(value);
                }
            }
        }
    }
    // Unit3D often groups peer counts in /peers links.
    if let Ok(selector) = Selector::parse("a[href*='/peers']") {
        let mut values = Vec::new();
        for element in row.select(&selector) {
            if let Some(value) = first_u32(&element.text().collect::<String>()) {
                values.push(value);
            }
        }
        // Prefer green (seeders) then red (leechers) by order when both exist.
        if needles.iter().any(|n| n.contains("arrow-up") || n.contains("seed") || *n == "text-green")
        {
            return values.first().copied();
        }
        if needles
            .iter()
            .any(|n| n.contains("arrow-down") || n.contains("leech") || *n == "text-red")
        {
            return values.get(1).copied().or_else(|| values.first().copied());
        }
    }
    None
}

fn extract_trailing_number(text: &str, labels: &[&str]) -> Option<u32> {
    for label in labels {
        let expression = regex::Regex::new(&format!(r"(?i){label}\s*[:：]?\s*([0-9][0-9,]*)")).ok()?;
        if let Some(captures) = expression.captures(text) {
            return captures
                .get(1)?
                .as_str()
                .replace(',', "")
                .parse()
                .ok();
        }
    }
    None
}

fn first_u32(text: &str) -> Option<u32> {
    regex::Regex::new(r"[0-9][0-9,]*")
        .ok()?
        .find(text)?
        .as_str()
        .replace(',', "")
        .parse()
        .ok()
}

fn extract_publish_time(row: &ElementRef<'_>) -> Option<DateTime<Utc>> {
    if let Ok(selector) = Selector::parse("time")
        && let Some(element) = row.select(&selector).next()
    {
        for candidate in [
            element.value().attr("datetime"),
            element.value().attr("title"),
            Some(element.text().collect::<String>().as_str()),
        ]
        .into_iter()
        .flatten()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        {
            if let Some(time) = parse_unit3d_datetime(candidate) {
                return Some(time);
            }
        }
    }
    None
}

fn parse_unit3d_datetime(raw: &str) -> Option<DateTime<Utc>> {
    let raw = raw.trim();
    if let Ok(value) = DateTime::parse_from_rfc3339(raw) {
        return Some(value.with_timezone(&Utc));
    }
    if let Ok(timestamp) = raw.parse::<i64>() {
        let seconds = if timestamp.unsigned_abs() < 100_000_000_000 {
            timestamp
        } else {
            timestamp / 1000
        };
        return DateTime::from_timestamp(seconds, 0);
    }
    let timezone = FixedOffset::east_opt(0)?;
    for format in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%d"] {
        if let Ok(value) = NaiveDateTime::parse_from_str(raw, format) {
            return timezone
                .from_local_datetime(&value)
                .single()
                .map(|value| value.with_timezone(&Utc));
        }
        if format == "%Y-%m-%d"
            && let Ok(date) = chrono::NaiveDate::parse_from_str(raw, format)
            && let Some(value) = date.and_hms_opt(0, 0, 0)
        {
            return timezone
                .from_local_datetime(&value)
                .single()
                .map(|value| value.with_timezone(&Utc));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use reqwest::header::{COOKIE, HeaderMap, HeaderValue};
    use reqwest::{Client, Url};

    use crate::indexer::access::{OriginAccessGate, default_indexer_access_policy};
    use crate::site::SiteAuth;

    use super::{Unit3DIndexer, parse_html_results};

    #[test]
    fn headers_include_custom_values_and_protect_cookie() {
        let mut custom = HeaderMap::new();
        custom.insert("x-browser-profile", HeaderValue::from_static("desktop"));
        custom.insert(COOKIE, HeaderValue::from_static("stale=1"));
        let indexer = Unit3DIndexer::new(
            3,
            "Blutopia".to_string(),
            "https://blutopia.cc",
            SiteAuth::Cookie {
                cookie: "remember_web=abc".to_string(),
            },
            custom,
            Client::new(),
            Arc::new(OriginAccessGate::new(default_indexer_access_policy())),
        )
        .unwrap();
        let headers = indexer.headers();
        assert_eq!(headers["x-browser-profile"], "desktop");
        assert_eq!(headers[COOKIE], "remember_web=abc");
    }

    #[test]
    fn parses_unit3d_search_list_fixture() {
        let html = r##"
            <div class="torrent-search--list__results">
              <table>
                <tbody>
                  <tr>
                    <td>Movie</td>
                    <td><a class="torrent-search--list__name" href="/torrents/901">Show.S01.2160p.WEB-DL</a></td>
                    <td class="torrent-search--list__size">12.5 GiB</td>
                    <td class="torrent-search--list__seeders">31</td>
                    <td class="torrent-search--list__leechers">6</td>
                    <td><time datetime="2026-07-14T08:10:00Z">1 day</time></td>
                    <td><a href="/torrents/download/901">DL</a></td>
                  </tr>
                </tbody>
              </table>
            </div>
        "##;
        let base = Url::parse("https://blutopia.cc").unwrap();
        let results = parse_html_results(html, 5, "BLU", &base).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].torrent_id, "901");
        assert_eq!(results[0].title, "Show.S01.2160p.WEB-DL");
        assert_eq!(results[0].size, 13_421_772_800);
        assert_eq!(results[0].seeders, 31);
        assert_eq!(results[0].leechers, 6);
        assert_eq!(results[0].download_locator.as_deref(), Some("901"));
        assert!(results[0].detail_url.as_deref().unwrap().contains("/torrents/901"));
        assert!(results[0].publish_time.is_some());
    }

    #[test]
    fn ignores_cross_origin_and_download_links_as_titles() {
        let html = r##"
            <table><tbody>
              <tr>
                <td><a href="https://evil.example/torrents/1">Bad</a></td>
                <td><a href="/torrents/download/2">Download</a></td>
              </tr>
            </tbody></table>
        "##;
        let base = Url::parse("https://blutopia.cc").unwrap();
        let results = parse_html_results(html, 5, "BLU", &base).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn rejects_api_key_auth() {
        let result = Unit3DIndexer::new(
            1,
            "BLU".to_string(),
            "https://blutopia.cc",
            SiteAuth::ApiKey {
                api_key: "k".to_string(),
            },
            HeaderMap::new(),
            Client::new(),
            Arc::new(OriginAccessGate::new(default_indexer_access_policy())),
        );
        assert!(result.is_err());
        let message = result.err().unwrap().to_string();
        assert!(message.contains("cookie"));
    }
}
