//! Unit3D user-stats adapter distilled from PT-depiler `schemas/Unit3D.ts`.

use std::future::Future;
use std::pin::Pin;

use chrono::{DateTime, FixedOffset, NaiveDateTime, TimeZone};
use reqwest::header::{COOKIE, HeaderMap, HeaderValue};
use reqwest::{Client, Url};
use scraper::{Element, ElementRef, Html, Selector};
use tracing::debug;

use super::rules::{self, SiteRule};
use super::{SiteAdapter, SiteAuth, TorrentAttributes, UserStats, UserStatsDetails};
use crate::site::nexusphp::looks_like_cloudflare_challenge;

pub struct Unit3DAdapter {
    base_url: String,
    auth: SiteAuth,
    headers: HeaderMap,
    client: Client,
}

impl Unit3DAdapter {
    pub fn new(base_url: String, auth: SiteAuth, headers: HeaderMap, client: Client) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            auth,
            headers,
            client,
        }
    }

    fn cookie(&self) -> Result<&str, String> {
        match &self.auth {
            SiteAuth::Cookie { cookie } | SiteAuth::CookiePasskey { cookie, .. } => Ok(cookie),
            _ => Err("Unit3D 用户统计需要 Cookie".to_string()),
        }
    }

    fn build_headers(&self) -> Result<HeaderMap, String> {
        let mut headers = self.headers.clone();
        let cookie = self.cookie()?;
        if cookie.trim().is_empty() {
            return Err("Unit3D 用户统计需要 Cookie".to_string());
        }
        headers.insert(
            COOKIE,
            HeaderValue::from_str(cookie).map_err(|_| "Cookie 格式无效".to_string())?,
        );
        Ok(headers)
    }

    async fn fetch_html(&self, path: &str, label: &str) -> Result<String, String> {
        let url = format!("{}{}", self.base_url, path);
        let headers = self.build_headers()?;
        let response = self
            .client
            .get(&url)
            .headers(headers)
            .send()
            .await
            .map_err(|error| format!("{label}请求失败: {error}"))?;
        let status = response.status();
        let final_url = response.url().clone();
        let html = response
            .text()
            .await
            .map_err(|error| format!("读取{label}响应失败: {error}"))?;
        if looks_like_cloudflare_challenge(&html) {
            return Err(format!("{label}被 Cloudflare 验证页拦截"));
        }
        if looks_like_unit3d_login(&html, &final_url) {
            return Err("Cookie 无效或已过期，站点返回了登录页".to_string());
        }
        if !status.is_success() {
            return Err(format!("{label}返回 HTTP {status}"));
        }
        Ok(html)
    }

    async fn fetch_stats(&self) -> Result<UserStats, String> {
        let index_html = self.fetch_html("/", "首页").await?;
        let username = extract_unit3d_username(&index_html)
            .ok_or_else(|| "Unit3D 首页未找到当前用户名".to_string())?;
        let profile_path = format!("/users/{username}");
        let profile_html = self.fetch_html(&profile_path, "用户详情页").await?;
        let mut stats = parse_unit3d_profile(&profile_html, &username)?;
        let earnings_path = format!("/users/{username}/earnings");
        match self.fetch_html(&earnings_path, "收益页").await {
            Ok(earnings_html) => {
                if stats.details.bonus_per_hour.is_none() {
                    stats.details.bonus_per_hour = parse_unit3d_bonus_per_hour(&earnings_html);
                }
            }
            Err(error) => debug!(%error, "Unit3D 收益页获取失败"),
        }
        stats.fill_derived();
        Ok(stats)
    }
}

fn looks_like_unit3d_login(html: &str, final_url: &Url) -> bool {
    let path = final_url.path().to_ascii_lowercase();
    if path.contains("login") || path.contains("auth") {
        return true;
    }
    let lower = html.to_ascii_lowercase();
    lower.contains("name=\"password\"")
        && (lower.contains("login") || lower.contains("sign in") || lower.contains("登录"))
}

pub(super) fn extract_unit3d_username(html: &str) -> Option<String> {
    let document = Html::parse_document(html);
    let selectors = [
        "a[href*='/users/'][href*='settings']",
        "a[href*='/users/'][href$='/settings']",
    ];
    for raw in selectors {
        let Ok(selector) = Selector::parse(raw) else {
            continue;
        };
        for element in document.select(&selector) {
            let href = element.value().attr("href")?;
            if let Some(name) = username_from_users_href(href) {
                return Some(name);
            }
        }
    }
    None
}

fn username_from_users_href(href: &str) -> Option<String> {
    let url = Url::parse("https://unit3d.invalid").ok()?.join(href).ok()?;
    let mut parts = url.path().split('/').filter(|part| !part.is_empty());
    if parts.next()? != "users" {
        return None;
    }
    let name = parts.next()?.trim();
    if name.is_empty() || name.eq_ignore_ascii_case("settings") {
        return None;
    }
    Some(name.to_string())
}

pub(super) fn parse_unit3d_profile(html: &str, username: &str) -> Result<UserStats, String> {
    let document = Html::parse_document(html);
    let uploaded = extract_size(&document, &[
        "li.ratio-bar__uploaded",
        "span.ratio-bar__uploaded",
        "li.ratio-bar__uploaded a",
    ])
    .or_else(|| {
        extract_size(
            &document,
            &["i.fa-arrow-up"],
        )
        .or_else(|| {
            // sibling span next to the upload icon
            find_text_near_icon(&document, "i.fa-arrow-up")
        })
    });
    let downloaded = extract_size(&document, &[
        "li.ratio-bar__downloaded",
        "span.ratio-bar__downloaded",
        "li.ratio-bar__downloaded a",
    ])
    .or_else(|| find_text_near_icon(&document, "i.fa-arrow-down"));

    let (uploaded, downloaded) = match (uploaded, downloaded) {
        (Some(u), Some(d)) => (u, d),
        _ => {
            // Fallback: labeled text on the profile page.
            let text = crate::site::nexusphp::extract_visible_text(html);
            let labels_up = ["上传量", "上傳量", "Uploaded"];
            let labels_down = ["下载量", "下載量", "Downloaded"];
            let up = crate::site::nexusphp::parse_labeled_size(&text, &labels_up);
            let down = crate::site::nexusphp::parse_labeled_size(&text, &labels_down);
            match (up.or(uploaded), down.or(downloaded)) {
                (Some(u), Some(d)) => (u, d),
                _ => return Err("Unit3D 用户页没有完整的上传量和下载量".to_string()),
            }
        }
    };

    let ratio = extract_number(&document, &[
        "li.ratio-bar__ratio",
        "span.ratio-bar__ratio",
        "li.ratio-bar__ratio a",
    ]);
    let bonus = extract_number(&document, &[
        "li.ratio-bar__points",
        "span.ratio-bar__points",
        "li.ratio-bar__points a",
    ]);
    let seeding_count = extract_u32(&document, &[
        "li.ratio-bar__seeding",
        "span.ratio-bar__seeding",
        "li.ratio-bar__seeding a",
    ]);
    let leeching_count = extract_u32(&document, &[
        "li.ratio-bar__leeching",
        "span.ratio-bar__leeching",
        "li.ratio-bar__leeching a",
    ]);
    let level_name = extract_attr_or_text(&document, &[
        "div.content span.badge-user",
        "a.user-tag__link[title]",
        "span.badge-user",
    ]);
    let uid = extract_profile_number_pair(&document, &[
        "dt:contains('User ID') + dd",
        "td:contains('User ID') + td",
        "dt:contains('用户 ID') + dd",
        "dt:contains('用户ID') + dd",
    ]);
    let join_time = extract_unit3d_time(&document, &[
        "time.profile__registration",
        "time[class*='registration']",
    ]);
    let invites = extract_profile_number_pair(&document, &[
        "dt:contains('Invites') + dd",
        "dt:contains('邀请') + dd",
        "dt:contains('邀請') + dd",
    ]);

    let mut details = UserStatsDetails {
        level_name,
        join_time,
        invites,
        uploads: extract_profile_number_pair(&document, &[
            "dl.key-value:has(a[href*='/uploads'])",
        ]),
        ..Default::default()
    };
    details.extra.insert(
        "schema".to_string(),
        serde_json::Value::String("unit3d".to_string()),
    );

    Ok(UserStats {
        uid: uid.map(|value| value.to_string()),
        username: username.to_string(),
        uploaded,
        downloaded,
        ratio: ratio.or_else(|| {
            if downloaded > 0 {
                Some(uploaded as f64 / downloaded as f64)
            } else {
                None
            }
        }),
        bonus,
        seeding_count,
        leeching_count,
        details,
    })
}

pub(super) fn parse_unit3d_bonus_per_hour(html: &str) -> Option<f64> {
    let document = Html::parse_document(html);
    for raw in [
        ".panelV2 dl.key-value dd:nth-child(2)",
        ".panelV2 dl.key-value dd:nth-of-type(2)",
        ".panelV2 dl.key-value dd",
        "dl.key-value dd",
    ] {
        let Ok(selector) = Selector::parse(raw) else {
            continue;
        };
        for element in document.select(&selector) {
            if let Some(value) = super::nexusphp::first_number(&element.text().collect::<String>())
            {
                return Some(value);
            }
        }
    }
    None
}

fn find_text_near_icon(document: &Html, icon_selector: &str) -> Option<u64> {
    let selector = Selector::parse(icon_selector).ok()?;
    for icon in document.select(&selector) {
        if let Some(parent_el) = icon.parent().and_then(ElementRef::wrap) {
            let text = parent_el.text().collect::<String>();
            if let Some(size) = parse_size_text(&text) {
                return Some(size);
            }
        }
        if let Some(grand) = icon
            .parent()
            .and_then(ElementRef::wrap)
            .and_then(|el| el.parent())
            .and_then(ElementRef::wrap)
        {
            let text = grand.text().collect::<String>();
            if let Some(size) = parse_size_text(&text) {
                return Some(size);
            }
        }
    }
    None
}

fn extract_size(document: &Html, selectors: &[&str]) -> Option<u64> {
    for raw in selectors {
        let Ok(selector) = Selector::parse(raw) else {
            continue;
        };
        for element in document.select(&selector) {
            let mut texts = vec![element.text().collect::<String>()];
            if let Some(title) = element.value().attr("title") {
                texts.push(title.to_string());
            }
            for text in texts {
                if let Some(size) = parse_size_text(&text) {
                    return Some(size);
                }
            }
        }
    }
    None
}

fn parse_size_text(text: &str) -> Option<u64> {
    let scrubbed = text.replace(',', "");
    let expression =
        regex::Regex::new(r"(?i)([0-9][0-9,.]*)\s*(bytes?|[kmgtpez]i?b)").ok()?;
    let captures = expression.captures(&scrubbed)?;
    super::nexusphp::size_from_parts(captures.get(1)?.as_str(), captures.get(2)?.as_str())
}

fn extract_number(document: &Html, selectors: &[&str]) -> Option<f64> {
    for raw in selectors {
        let Ok(selector) = Selector::parse(raw) else {
            continue;
        };
        for element in document.select(&selector) {
            let text = element.text().collect::<String>();
            if let Some(value) = super::nexusphp::first_number(&text) {
                return Some(value);
            }
            if let Some(title) = element.value().attr("title")
                && let Some(value) = super::nexusphp::first_number(title)
            {
                return Some(value);
            }
        }
    }
    None
}

fn extract_u32(document: &Html, selectors: &[&str]) -> Option<u32> {
    extract_number(document, selectors)
        .filter(|value| value.is_finite() && *value >= 0.0)
        .and_then(|value| u32::try_from(value as u64).ok())
}

fn extract_attr_or_text(document: &Html, selectors: &[&str]) -> Option<String> {
    for raw in selectors {
        let Ok(selector) = Selector::parse(raw) else {
            continue;
        };
        for element in document.select(&selector) {
            if let Some(title) = element.value().attr("title").map(str::trim) {
                if !title.is_empty() {
                    return Some(title.to_string());
                }
            }
            let text = element
                .text()
                .collect::<String>()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            if !text.is_empty() {
                return Some(text);
            }
        }
    }
    None
}

fn extract_profile_number_pair(document: &Html, selectors: &[&str]) -> Option<u64> {
    for raw in selectors {
        // scraper's Selector may not support :contains — fall back to manual scan.
        if raw.contains(":contains") {
            if let Some(value) = scan_contains_pair(document, raw) {
                return Some(value);
            }
            continue;
        }
        let Ok(selector) = Selector::parse(raw) else {
            continue;
        };
        for element in document.select(&selector) {
            if let Some(value) = super::nexusphp::first_integer(&element.text().collect::<String>())
            {
                return Some(value);
            }
        }
    }
    None
}

fn scan_contains_pair(document: &Html, pattern: &str) -> Option<u64> {
    // pattern like "dt:contains('User ID') + dd" — extract label and sibling tag.
    let label_start = pattern.find(":contains('")? + ":contains('".len();
    let label_end = pattern[label_start..].find("')")? + label_start;
    let label = &pattern[label_start..label_end];
    let sibling = pattern.split("+ ").nth(1)?.trim();

    let tags = ["dt", "td", "th", "dd"];
    for tag in tags {
        let Ok(selector) = Selector::parse(tag) else {
            continue;
        };
        for element in document.select(&selector) {
            let text = element.text().collect::<String>();
            if !text.contains(label) {
                continue;
            }
            if let Some(next) = element.next_sibling_element() {
                if next.value().name() == sibling
                    && let Some(value) =
                        super::nexusphp::first_integer(&next.text().collect::<String>())
                {
                    return Some(value);
                }
            }
        }
    }
    None
}

fn extract_unit3d_time(document: &Html, selectors: &[&str]) -> Option<i64> {
    for raw in selectors {
        let Ok(selector) = Selector::parse(raw) else {
            continue;
        };
        for element in document.select(&selector) {
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
                if let Some(ts) = parse_unit3d_datetime(candidate) {
                    return Some(ts);
                }
            }
        }
    }
    None
}

fn parse_unit3d_datetime(value: &str) -> Option<i64> {
    let value = value.trim();
    if let Ok(timestamp) = value.parse::<i64>() {
        return if timestamp.unsigned_abs() < 100_000_000_000 {
            timestamp.checked_mul(1000)
        } else {
            Some(timestamp)
        };
    }
    if let Ok(time) = DateTime::parse_from_rfc3339(value) {
        return Some(time.timestamp_millis());
    }
    for format in [
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d",
        "%b %d %Y, %H:%M:%S",
        "%b %d %Y",
        "%d %b %Y %H:%M:%S",
    ] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(value, format) {
            let offset = FixedOffset::east_opt(0)?;
            return offset
                .from_local_datetime(&naive)
                .single()
                .map(|time| time.timestamp_millis());
        }
        if format.contains("%H") {
            continue;
        }
        if let Ok(date) = chrono::NaiveDate::parse_from_str(value, format) {
            let offset = FixedOffset::east_opt(0)?;
            let naive = date.and_hms_opt(0, 0, 0)?;
            return offset
                .from_local_datetime(&naive)
                .single()
                .map(|time| time.timestamp_millis());
        }
    }
    None
}

impl SiteAdapter for Unit3DAdapter {
    fn get_user_stats(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<UserStats, String>> + Send + '_>> {
        Box::pin(async move { self.fetch_stats().await })
    }

    fn get_torrent_attributes(
        &self,
        detail_url: &str,
    ) -> Pin<Box<dyn Future<Output = Result<TorrentAttributes, String>> + Send + '_>> {
        let detail_url = detail_url.to_string();
        Box::pin(async move {
            let _ = detail_url;
            Err("Unit3D 种子属性抓取尚未适配".to_string())
        })
    }
}

#[allow(dead_code)]
fn _rule_lookup(ptd_id: &str) -> Option<&'static SiteRule> {
    rules::rule_for_site(ptd_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_username_from_settings_link() {
        let html = r#"
            <nav>
              <a href="/users/alice/settings">Settings</a>
              <a href="/users/bob">Bob</a>
            </nav>
        "#;
        assert_eq!(extract_unit3d_username(html).as_deref(), Some("alice"));
    }

    #[test]
    fn parses_ratio_bar_profile_metrics() {
        let html = r##"
            <div class="ratio-bar">
              <li class="ratio-bar__uploaded"><a href="#">1.50 TiB</a></li>
              <li class="ratio-bar__downloaded"><a href="#">300.00 GiB</a></li>
              <li class="ratio-bar__ratio"><a href="#">2.50</a></li>
              <li class="ratio-bar__points"><a href="#">12,345.6</a></li>
              <li class="ratio-bar__seeding"><a href="#">88</a></li>
              <li class="ratio-bar__leeching"><a href="#">3</a></li>
            </div>
            <div class="content"><span class="badge-user" title="BluUser">BluUser</span></div>
            <dl class="key-value">
              <dt>User ID</dt><dd>4242</dd>
              <dt>Registration date</dt><dd><time class="profile__registration" datetime="2024-01-02T03:04:05Z">Jan 2 2024</time></dd>
            </dl>
        "##;
        let stats = parse_unit3d_profile(html, "alice").expect("parse profile");
        assert_eq!(stats.username, "alice");
        assert_eq!(stats.uploaded, 1649267441664);
        assert_eq!(stats.downloaded, 322122547200);
        assert_eq!(stats.ratio, Some(2.5));
        assert_eq!(stats.bonus, Some(12345.6));
        assert_eq!(stats.seeding_count, Some(88));
        assert_eq!(stats.leeching_count, Some(3));
        assert_eq!(stats.details.level_name.as_deref(), Some("BluUser"));
        assert_eq!(stats.uid.as_deref(), Some("4242"));
        assert_eq!(stats.details.join_time, Some(1704164645000));
    }

    #[test]
    fn parses_bonus_per_hour_from_earnings_panel() {
        let html = r##"
            <div class="panelV2">
              <dl class="key-value">
                <dt>Uploads</dt><dd>1</dd>
                <dt>Points</dt><dd>3.822</dd>
              </dl>
            </div>
        "##;
        let rate = parse_unit3d_bonus_per_hour(html).expect("rate");
        assert!(rate == 1.0 || rate == 3.822, "unexpected rate {rate}");
    }

    #[test]
    fn labeled_text_fallback_when_ratio_bar_missing() {
        let html = r#"
            <div>上传量 2.00 TiB 下载量 1.00 TiB</div>
        "#;
        let stats = parse_unit3d_profile(html, "bob").expect("parse");
        assert_eq!(stats.uploaded, 2199023255552);
        assert_eq!(stats.downloaded, 1099511627776);
    }
}
