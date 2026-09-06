use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD};
use chrono::{DateTime, FixedOffset, NaiveDate, NaiveDateTime, TimeZone};
use regex::{Regex, escape};
use reqwest::header::{COOKIE, HeaderMap, HeaderValue};
use reqwest::{Client, Url};
use scraper::{Element, Html, Selector};
use serde_json::Value;
use tracing::{debug, warn};

use super::{
    SiteAdapter, SiteAuth, SiteTestResult, TorrentAttributes, UserStats, UserStatsDetails,
};
use std::error::Error as _;
use std::future::Future;
use std::pin::Pin;

pub struct NexusPhpAdapter {
    base_url: String,
    auth: SiteAuth,
    request_headers: HeaderMap,
    client: Client,
    cached_user_id: Option<String>,
}

impl NexusPhpAdapter {
    pub fn new(
        base_url: String,
        auth: SiteAuth,
        request_headers: HeaderMap,
        client: Client,
    ) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            auth,
            request_headers,
            client,
            cached_user_id: None,
        }
    }

    /// PT-Depiler keeps the stable NexusPHP user id from the previous successful refresh.
    /// Supplying it here lets subsequent refreshes go straight to the profile page instead of
    /// depending on every site's homepage layout and availability.
    pub fn with_cached_user_id(mut self, user_id: Option<&str>) -> Self {
        self.cached_user_id = user_id.and_then(normalize_user_id);
        self
    }

    fn cookie_value(&self) -> Option<&str> {
        match &self.auth {
            SiteAuth::Cookie { cookie } => Some(cookie.as_str()),
            SiteAuth::CookiePasskey { cookie, .. } => Some(cookie.as_str()),
            _ => None,
        }
    }

    fn build_headers(&self) -> HeaderMap {
        let mut headers = self.request_headers.clone();
        if let Some(cookie) = self.cookie_value()
            && let Ok(val) = HeaderValue::from_str(cookie)
        {
            headers.insert(COOKIE, val);
        }
        headers
    }

    async fn fetch_user_info_html(&self) -> Result<UserStats, String> {
        if self
            .cookie_value()
            .is_some_and(|cookie| HeaderValue::from_str(cookie).is_err())
        {
            return Err("Cookie 格式无效，请检查是否包含换行等非法字符".to_string());
        }
        if self
            .build_headers()
            .get(COOKIE)
            .and_then(|value| value.to_str().ok())
            .is_none_or(|value| value.trim().is_empty())
        {
            return Err(
                "NexusPHP 通用用户统计需要 Cookie，单独的 Passkey 或 API Key 不能用于此 HTML 流程"
                    .to_string(),
            );
        }
        // Match PT-Depiler's generic NexusPHP process: reuse the stable id first and only visit
        // /index.php when no id is known (or when the profile cannot supply complete totals). Cookie-derived
        // ids take precedence over cached ids so changing accounts cannot silently reuse stale data.
        let mut identity = self
            .cookie_value()
            .and_then(extract_current_user_from_cookie)
            .or_else(|| {
                self.cached_user_id
                    .as_deref()
                    .and_then(current_user_from_id)
            });
        let mut index_html = None;
        let mut detail_html = None;
        let mut page_errors = Vec::new();
        let mut attempted_detail_url = None;

        if let Some(current_user) = identity.as_ref() {
            let detail_url = self.resolve_same_origin_url(&current_user.href)?;
            attempted_detail_url = Some(detail_url.clone());
            match self
                .fetch_user_profile(&detail_url, &current_user.uid)
                .await
            {
                Ok(html) => {
                    if !has_transfer_totals(&html) {
                        page_errors.push("用户详情页没有完整的上传量和下载量".to_string());
                    }
                    detail_html = Some(html);
                }
                Err(error) => {
                    debug!(%error, "NexusPHP 用户详情页获取失败，回退首页");
                    page_errors.push(error);
                    // A cached/cookie id is a hint, not proof that this account is still active.
                    identity = None;
                }
            }
        }

        if detail_html
            .as_deref()
            .is_none_or(|html| !has_transfer_totals(html))
        {
            let index_url = format!("{}/index.php", self.base_url);
            debug!("NexusPHP HTML request: {}", index_url);
            let homepage = match self.fetch_html_page(&index_url, "首页").await {
                Ok(homepage) => homepage,
                Err(homepage_error) => {
                    page_errors.push(homepage_error);
                    return Err(page_errors.join("；"));
                }
            };
            let homepage_identity = extract_current_user(&homepage);
            if let Some(found) = homepage_identity {
                if identity
                    .as_ref()
                    .is_some_and(|known| known.uid != found.uid)
                {
                    // Never combine another member's profile with the logged-in user's overview.
                    detail_html = None;
                }
                identity = Some(found);
            }
            index_html = Some(homepage);

            if let Some(current_user) = identity.as_ref() {
                let detail_url = self.resolve_same_origin_url(&current_user.href)?;
                // Some installations use user.php for the same id. Compare URLs, not just ids,
                // and do not immediately retry the exact request that already failed.
                if attempted_detail_url.as_deref() != Some(detail_url.as_str()) {
                    match self
                        .fetch_user_profile(&detail_url, &current_user.uid)
                        .await
                    {
                        Ok(html) => detail_html = Some(html),
                        Err(error) => {
                            debug!(%error, "NexusPHP 用户详情页获取失败，使用首页统计");
                            page_errors.push(error);
                        }
                    }
                }
            }
        }

        let detail_page_loaded = detail_html.is_some();
        let homepage_loaded = index_html.is_some();
        let detail_html = detail_html
            .as_deref()
            .or(index_html.as_deref())
            .ok_or_else(|| "没有可解析的用户信息页面".to_string())?;
        // When the profile page was loaded directly it also contains the shared info block, so it
        // can provide homepage-only fields without another request.
        let index_html = index_html.as_deref().unwrap_or(detail_html);

        let detail_identity = extract_current_user(detail_html);
        let uid = identity
            .as_ref()
            .map(|value| value.uid.clone())
            .or_else(|| detail_identity.as_ref().map(|value| value.uid.clone()));
        let username = detail_identity
            .as_ref()
            .and_then(|value| value.username.clone())
            .or_else(|| identity.as_ref().and_then(|value| value.username.clone()))
            .or_else(|| extract_username(detail_html))
            .or_else(|| extract_username(index_html))
            .unwrap_or_else(|| uid.clone().unwrap_or_else(|| "unknown".to_string()));

        let detail_text = extract_visible_text(detail_html);
        let index_text = extract_visible_text(index_html);
        let uploaded = parse_labeled_size(&detail_text, &["上传量", "上傳量", "Uploaded"])
            .or_else(|| parse_labeled_size(&index_text, &["上传量", "上傳量", "Uploaded"]));
        let downloaded = parse_labeled_size(&detail_text, &["下载量", "下載量", "Downloaded"])
            .or_else(|| parse_labeled_size(&index_text, &["下载量", "下載量", "Downloaded"]));

        let page_label = match (detail_page_loaded, homepage_loaded) {
            (true, true) => "用户详情页和首页",
            (true, false) => "用户详情页",
            _ => "首页",
        };
        let missing_field = |field: &str| {
            let mut errors = page_errors.clone();
            errors.push(format!(
                "{page_label}没有找到{field}，站点页面结构可能需要单独适配"
            ));
            errors.join("；")
        };
        let uploaded = uploaded.ok_or_else(|| missing_field("上传量"))?;
        let downloaded = downloaded.ok_or_else(|| missing_field("下载量"))?;

        let ratio = ratio_from_totals(Some(uploaded), Some(downloaded));
        let bonus = parse_profile_number(
            detail_html,
            &[
                "魔力值",
                "Karma Points",
                "魅力值",
                "星焱",
                "沙粒",
                "魔力",
                "Bonus",
                "蝌蚪",
                "U币",
                "UBits Coin",
                "UCoin",
                "憨豆",
            ],
        );
        let mut seeding_count = parse_labeled_integer(
            &detail_text,
            &["当前做种", "當前做種", "做种数", "做種數", "Seeding"],
        );
        let leeching_count = parse_labeled_integer(
            &detail_text,
            &["当前下载", "當前下載", "下载数", "下載數", "Leeching"],
        );

        let (hnr_pre_warning, hnr_unsatisfied) = parse_hnr_counts(index_html);
        let mut details = UserStatsDetails {
            is_donor: detail_page_loaded.then(|| detect_donor(detail_html)),
            level_name: parse_profile_level(detail_html),
            join_time: parse_profile_value(
                detail_html,
                &["加入日期", "加入時間", "Join date", "Joined"],
            )
            .as_deref()
            .and_then(parse_user_datetime_millis),
            last_access_at: parse_profile_value(
                detail_html,
                &["最近动向", "最近動向", "Last Action", "Last access"],
            )
            .as_deref()
            .and_then(parse_user_datetime_millis),
            message_count: parse_message_count(index_html),
            invites: parse_labeled_u64(&detail_text, &["邀请", "邀請", "Invites", "Invitations"]),
            avatar: extract_avatar(detail_html)
                .and_then(|avatar| self.resolve_same_origin_url(&avatar).ok().or(Some(avatar))),
            true_downloaded: parse_labeled_size(
                &detail_text,
                &[
                    "实际下载量",
                    "真实下载量",
                    "實際下載量",
                    "真實下載量",
                    "Real Downloaded",
                    "Actual Downloaded",
                ],
            ),
            true_uploaded: parse_labeled_size(
                &detail_text,
                &[
                    "实际上傳量",
                    "实际上传量",
                    "真实上传量",
                    "實際上傳量",
                    "真實上傳量",
                    "Real Uploaded",
                    "Actual Uploaded",
                ],
            ),
            seeding_time: parse_labeled_duration_seconds(
                &detail_text,
                &["做种时间", "做種時間", "Seeding Time", "Seed Time"],
            ),
            average_seeding_time: parse_labeled_duration_seconds(
                &detail_text,
                &[
                    "平均做种时间",
                    "平均做種時間",
                    "Average Seeding Time",
                    "Average Seed Time",
                ],
            ),
            seeding_bonus: parse_profile_number(
                detail_html,
                &["做种积分", "做種積分", "Seeding Points", "保种积分"],
            ),
            uploads: parse_labeled_u64(
                &detail_text,
                &["发布数", "發佈數", "上传种子", "上傳種子", "Uploads"],
            ),
            snatches: parse_labeled_u64(
                &detail_text,
                &["完成数", "完成數", "下载完成", "下載完成", "Snatches"],
            ),
            posts: parse_labeled_u64(&detail_text, &["论坛发帖", "論壇發帖", "Posts"]),
            adoptions: parse_labeled_u64(&detail_text, &["认领种子", "認領種子", "Adoptions"]),
            hnr_unsatisfied,
            hnr_pre_warning,
            ..Default::default()
        };

        // PT-Depiler 的 NexusPHP 通用 schema 会用 AJAX 补充做种量和发布数。
        if let Some(user_id) = uid.as_deref() {
            if let Some((count, size)) = self.fetch_user_torrent_summary(user_id, "seeding").await {
                seeding_count = Some(count);
                details.seeding_size = size;
            }
            if let Some((count, _)) = self.fetch_user_torrent_summary(user_id, "uploaded").await {
                details.uploads = Some(count as u64);
            }
        }

        let ptd_site = Url::parse(&self.base_url)
            .ok()
            .and_then(|url| url.host_str().and_then(crate::ptd_sites::site_id_for_host));
        let is_u2 = ptd_site == Some("u2");
        details.level_id = details
            .level_name
            .as_deref()
            .and_then(|name| super::nexusphp_levels::level_id(ptd_site?, name))
            .or_else(|| {
                profile_class_level(detail_html)
                    .and_then(|name| super::nexusphp_levels::level_id(ptd_site?, &name))
            });
        if ptd_site == Some("ilolicon") {
            details.ptd_user_id = profile_uuid(detail_html);
        }
        let bonus_url = if is_u2 {
            format!(
                "{}/mprecent.php?user={}",
                self.base_url,
                uid.as_deref().unwrap_or("")
            )
        } else {
            format!("{}/mybonus.php", self.base_url)
        };
        match self.fetch_html_page(&bonus_url, "魔力值页面").await {
            Ok(bonus_html) => {
                let (bonus_per_hour, seeding_bonus_per_hour) =
                    parse_bonus_rates(&bonus_html, is_u2);
                details.bonus_per_hour = bonus_per_hour;
                details.seeding_bonus_per_hour = seeding_bonus_per_hour;
                if ptd_site == Some("hhanclub") {
                    if let (Some(user_id), Some(base)) = (uid.as_deref(), seeding_bonus_per_hour) {
                        // A missing settlement page must not masquerade as a complete rate.
                        details.seeding_bonus_per_hour = self
                            .fetch_rescue_daily_bonus(user_id)
                            .await
                            .map(|daily| base + daily / 24.0);
                    }
                }
            }
            Err(error) => debug!(%error, "NexusPHP 魔力值页面获取失败"),
        }

        let mut stats = UserStats {
            uid,
            username,
            uploaded,
            downloaded,
            ratio,
            bonus,
            seeding_count,
            leeching_count,
            details,
        };
        stats.fill_derived();
        Ok(stats)
    }

    async fn fetch_rescue_daily_bonus(&self, user_id: &str) -> Option<f64> {
        let url = format!("{}/rescuesettleinfo.php?id={user_id}", self.base_url);
        let mut html = self.fetch_html_page(&url, "保种结算记录").await.ok()?;
        let page = {
            let document = Html::parse_document(&html);
            let selector = Selector::parse("table + div b").ok()?;
            document
                .select(&selector)
                .next_back()
                .and_then(|element| first_integer(&element.text().collect::<String>()))
                .unwrap_or(1)
                .saturating_sub(1)
        };
        if page > 0 {
            html = self
                .fetch_html_page(&format!("{url}&page={page}"), "保种结算记录末页")
                .await
                .ok()?;
        }
        let document = Html::parse_document(&html);
        let selector = Selector::parse("table tbody tr:last-child > td:nth-of-type(6)").ok()?;
        document
            .select(&selector)
            .next()
            .and_then(|element| first_number(&element.text().collect::<String>()))
    }

    async fn fetch_user_profile(&self, url: &str, user_id: &str) -> Result<String, String> {
        let html = self.fetch_html_page(url, "用户详情页").await?;
        if extract_current_user(&html).is_some_and(|current| current.uid != user_id) {
            return Err("用户详情页的登录用户与请求的 UID 不一致，已拒绝使用该页统计".to_string());
        }
        Ok(html)
    }

    async fn fetch_html_page(&self, url: &str, label: &str) -> Result<String, String> {
        let response = self
            .client
            .get(url)
            .headers(self.build_headers())
            .send()
            .await
            .map_err(|error| format!("{label}请求失败: {}", describe_reqwest_error(error)))?;
        let status = response.status();
        let final_url = response.url().clone();
        let html = response
            .text()
            .await
            .map_err(|error| format!("读取{label}响应失败: {}", describe_reqwest_error(error)))?;

        if looks_like_cloudflare_challenge(&html) {
            return Err(format!("{label}被 Cloudflare 验证页拦截"));
        }
        if looks_like_login_page(&html, &final_url) {
            return Err("Cookie 无效或已过期，站点返回了登录页".to_string());
        }
        if !status.is_success() {
            return Err(format!("{label}返回 HTTP {status}"));
        }
        Ok(html)
    }

    fn resolve_same_origin_url(&self, href: &str) -> Result<String, String> {
        let base = Url::parse(&format!("{}/", self.base_url.trim_end_matches('/')))
            .map_err(|error| format!("站点地址无效: {error}"))?;
        let url = base
            .join(href)
            .map_err(|error| format!("用户详情链接无效: {error}"))?;
        if base.scheme() != url.scheme()
            || base.host_str() != url.host_str()
            || base.port_or_known_default() != url.port_or_known_default()
        {
            return Err("用户详情链接跳转到了其他站点，已拒绝请求".to_string());
        }
        Ok(url.to_string())
    }

    async fn fetch_user_torrent_summary(
        &self,
        user_id: &str,
        torrent_type: &str,
    ) -> Option<(u32, Option<u64>)> {
        let mut url = Url::parse(&format!("{}/getusertorrentlistajax.php", self.base_url)).ok()?;
        url.query_pairs_mut()
            .append_pair("userid", user_id)
            .append_pair("type", torrent_type);
        let html = self
            .fetch_html_page(url.as_str(), "用户种子列表")
            .await
            .ok()?;
        let summary = parse_user_torrent_ajax_summary(&html);
        if summary.is_none() {
            warn!(
                torrent_type,
                "NexusPHP 用户种子列表已返回，但无法解析记录数"
            );
        }
        summary
    }

    async fn fetch_torrent_detail_html(&self, detail_url: &str) -> Result<String, String> {
        debug!("NexusPHP detail request: {}", detail_url);

        let resp = self
            .client
            .get(detail_url)
            .headers(self.build_headers())
            .send()
            .await
            .map_err(|error| format!("请求失败: {}", describe_reqwest_error(error)))?;

        if !resp.status().is_success() {
            return Err(format!("HTTP {}", resp.status()));
        }

        let html = resp
            .text()
            .await
            .map_err(|error| format!("读取响应失败: {}", describe_reqwest_error(error)))?;
        if html.contains("login.php") && !html.contains("details") {
            return Err("Cookie 无效或已过期".to_string());
        }

        Ok(html)
    }

    fn detect_torrent_attributes(html: &str) -> TorrentAttributes {
        let document = Html::parse_document(html);
        let selectors = [
            "body",
            ".torrentname",
            ".embedded",
            ".sticky",
            ".pro_free",
            ".pro_free2up",
            ".free",
            ".twoupfree",
            ".twoup",
            ".hitandrun",
            ".hr",
            ".promotion-tag",
            ".torrent-promote",
            ".torrent-detail",
            ".torrent_info",
            "span",
            "a",
            "b",
            "strong",
            "font",
        ];
        let mut fragments = Vec::new();
        for selector_str in selectors {
            if let Ok(selector) = Selector::parse(selector_str) {
                for element in document.select(&selector) {
                    let text = element.text().collect::<Vec<_>>().join(" ");
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        fragments.push(trimmed.to_string());
                    }
                    if let Some(class) = element.value().attr("class") {
                        fragments.push(class.to_string());
                    }
                    if let Some(title) = element.value().attr("title") {
                        fragments.push(title.to_string());
                    }
                }
            }
        }
        let upper = fragments.join(" ").to_ascii_uppercase();

        let has_two_x_free = contains_any(
            &upper,
            &[
                "2XFREE",
                "2X FREE",
                "FREE 2XUP",
                "FREE,2XUP",
                "TWOUPFREE",
                "PRO_FREE2UP",
            ],
        );
        let has_free = has_two_x_free
            || contains_any(
                &upper,
                &[
                    "FREELEECH",
                    "FREE LEECH",
                    " FREE ",
                    "PRO_FREE",
                    " 免费 ",
                    " FREE<",
                    ">FREE ",
                ],
            );
        let hit_and_run = contains_any(
            &upper,
            &[
                "H&R",
                "HIT AND RUN",
                "HIT&RUN",
                "HR:",
                "HNR",
                "HITRUN",
                "HITANDRUN",
                " HR ",
            ],
        );

        let (download_volume_factor, upload_volume_factor) = if has_two_x_free {
            (Some(0.0), Some(2.0))
        } else if has_free {
            (Some(0.0), Some(1.0))
        } else {
            (
                detect_download_factor(&upper).or(Some(1.0)),
                detect_upload_factor(&upper).or(Some(1.0)),
            )
        };

        let free_end_timestamp = if has_free {
            detect_free_end_timestamp(html)
        } else {
            None
        };

        TorrentAttributes {
            free: has_free || download_volume_factor == Some(0.0),
            two_x_free: has_two_x_free,
            hit_and_run,
            seeder_count: None,
            leecher_count: None,
            free_end_timestamp,
            download_volume_factor,
            upload_volume_factor,
        }
    }
}

impl SiteAdapter for NexusPhpAdapter {
    fn test_connection(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<SiteTestResult, String>> + Send + '_>> {
        Box::pin(async move {
            match self.get_user_stats().await {
                Ok(stats) => Ok(SiteTestResult {
                    success: true,
                    message: format!("连接成功，用户: {}", stats.username),
                    user_stats: Some(stats),
                }),
                Err(e) => Ok(SiteTestResult {
                    success: false,
                    message: e,
                    user_stats: None,
                }),
            }
        })
    }

    fn get_user_stats(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<UserStats, String>> + Send + '_>> {
        Box::pin(self.fetch_user_info_html())
    }

    fn get_torrent_attributes(
        &self,
        detail_url: &str,
    ) -> Pin<Box<dyn Future<Output = Result<TorrentAttributes, String>> + Send + '_>> {
        let detail_url = detail_url.to_string();
        Box::pin(async move {
            let html = self.fetch_torrent_detail_html(&detail_url).await?;
            Ok(Self::detect_torrent_attributes(&html))
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
struct CurrentUser {
    uid: String,
    username: Option<String>,
    href: String,
}

fn describe_reqwest_error(error: reqwest::Error) -> String {
    // Request URLs can contain passkeys or signed query parameters. Keep the source chain for
    // TLS/DNS/connection diagnostics without echoing credentials into stored errors and backups.
    let error = error.without_url();
    let category = if error.is_timeout() {
        "请求超时"
    } else if error.is_connect() {
        "连接失败"
    } else if error.is_redirect() {
        "重定向失败"
    } else if error.is_body() || error.is_decode() {
        "响应传输失败"
    } else {
        "HTTP 请求失败"
    };
    let top_level = error.to_string();
    let mut causes = Vec::new();
    let mut source = error.source();
    while let Some(cause) = source {
        let message = cause.to_string();
        if !message.is_empty()
            && message != top_level
            && causes.last().is_none_or(|previous| previous != &message)
        {
            causes.push(message);
        }
        if causes.len() >= 4 {
            break;
        }
        source = cause.source();
    }

    if causes.is_empty() {
        format!("{category}: {top_level}")
    } else {
        format!("{category}: {top_level}；底层原因: {}", causes.join(" -> "))
    }
}

fn json_value_to_string(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::to_string)
        .or_else(|| value.as_u64().map(|n| n.to_string()))
        .or_else(|| value.as_i64().map(|n| n.to_string()))
}

fn ratio_from_totals(uploaded: Option<u64>, downloaded: Option<u64>) -> Option<f64> {
    match (uploaded, downloaded) {
        (Some(uploaded), Some(downloaded)) if downloaded > 0 => {
            Some(uploaded as f64 / downloaded as f64)
        }
        _ => None,
    }
}

fn extract_current_user(html: &str) -> Option<CurrentUser> {
    let document = Html::parse_document(html);
    let selectors = [
        "#info_block a[href*='userdetails.php'][href*='id=']",
        "#info_block a[href*='user.php'][href*='id=']",
        "#userinfo a[href*='userdetails.php'], #userinfo a[href*='user.php']",
        "#user_info a[href*='userdetails.php'], #user_info a[href*='user.php']",
        "#header a.User_Name[href*='userdetails.php'], #header a.username[href*='userdetails.php']",
    ];

    let mut fallback: Option<CurrentUser> = None;
    for selector in selectors {
        let Ok(selector) = Selector::parse(selector) else {
            continue;
        };
        for element in document.select(&selector) {
            let Some(href) = element.value().attr("href") else {
                continue;
            };
            let Some(uid) = extract_user_id_from_href(href) else {
                continue;
            };
            let username =
                normalize_text(element.text()).filter(|name| !name.eq_ignore_ascii_case("details"));
            let current = CurrentUser {
                uid,
                username,
                href: href.to_string(),
            };
            if fallback
                .as_ref()
                .is_some_and(|first| first.uid != current.uid)
            {
                continue;
            }
            if current.username.is_some() {
                return Some(current);
            }
            fallback = fallback.or(Some(current));
        }
    }
    fallback
}

fn extract_current_user_from_cookie(cookie: &str) -> Option<CurrentUser> {
    let uid = extract_user_id_from_cookie(cookie)?;
    current_user_from_id(&uid)
}

fn current_user_from_id(user_id: &str) -> Option<CurrentUser> {
    let uid = normalize_user_id(user_id)?;
    Some(CurrentUser {
        href: format!("userdetails.php?id={uid}"),
        uid,
        username: None,
    })
}

fn normalize_user_id(user_id: &str) -> Option<String> {
    let user_id = user_id.trim();
    (!user_id.is_empty() && user_id.chars().all(|ch| ch.is_ascii_digit()))
        .then(|| user_id.to_string())
}

fn extract_user_id_from_cookie(cookie: &str) -> Option<String> {
    let pairs = cookie.split(';').filter_map(|part| {
        let (name, value) = part.trim().split_once('=')?;
        let value = value.trim().trim_matches('"');
        let decoded = urlencoding::decode(value).ok()?.into_owned();
        Some((name.trim().to_ascii_lowercase(), decoded))
    });
    let pairs = pairs.collect::<Vec<_>>();

    // Older NexusPHP installations expose a dedicated uid cookie.
    for (name, value) in &pairs {
        if (name == "c_secure_uid" || name.ends_with("_secure_uid"))
            && let Some(uid) = decode_cookie_user_id(value, true)
        {
            return Some(uid);
        }
    }

    // Current NexusPHP stores {"user_id": ..., "expires": ...}.<signature> in
    // a base64 encoded c_secure_pass cookie.
    for (name, value) in &pairs {
        if (name == "c_secure_pass" || name.ends_with("_secure_pass"))
            && let Some(uid) = decode_cookie_user_id(value, false)
        {
            return Some(uid);
        }
    }
    None
}

fn decode_cookie_user_id(value: &str, allow_plain_uid: bool) -> Option<String> {
    let value = value.trim();
    if allow_plain_uid && let Some(uid) = normalize_user_id(value) {
        return Some(uid);
    }

    for engine in [&STANDARD, &STANDARD_NO_PAD, &URL_SAFE, &URL_SAFE_NO_PAD] {
        let Ok(decoded) = engine.decode(value) else {
            continue;
        };
        let Ok(decoded) = String::from_utf8(decoded) else {
            continue;
        };
        let payload = decoded
            .split_once('.')
            .map_or(decoded.as_str(), |(json, _)| json);
        if allow_plain_uid && let Some(uid) = normalize_user_id(payload) {
            return Some(uid);
        }
        let Ok(json) = serde_json::from_str::<Value>(payload) else {
            continue;
        };
        if let Some(uid) = ["user_id", "uid", "id"]
            .iter()
            .find_map(|key| json.get(key).and_then(json_value_to_string))
            .filter(|uid| !uid.is_empty() && uid.chars().all(|ch| ch.is_ascii_digit()))
        {
            return Some(uid);
        }
    }
    None
}

fn extract_username(html: &str) -> Option<String> {
    let document = Html::parse_document(html);
    for selector in [
        "#info_block .User_Name",
        "#info_block .username",
        ".User_Name",
        ".username",
    ] {
        let Ok(selector) = Selector::parse(selector) else {
            continue;
        };
        if let Some(username) = document
            .select(&selector)
            .find_map(|element| normalize_text(element.text()))
        {
            return Some(username);
        }
    }
    None
}

fn extract_user_id_from_href(href: &str) -> Option<String> {
    let base = Url::parse("https://tracker.invalid/").ok()?;
    let url = base.join(href).ok()?;
    url.query_pairs()
        .find(|(key, _)| key == "id" || key == "userid" || key == "user_id")
        .and_then(|(_, value)| normalize_user_id(&value))
}

fn normalize_text<'a>(parts: impl Iterator<Item = &'a str>) -> Option<String> {
    let text = parts
        .flat_map(str::split_whitespace)
        .collect::<Vec<_>>()
        .join(" ");
    (!text.is_empty()).then_some(text)
}

fn extract_visible_text(html: &str) -> String {
    let document = Html::parse_document(html);
    normalize_text(document.root_element().text()).unwrap_or_default()
}

fn has_transfer_totals(html: &str) -> bool {
    let text = extract_visible_text(html);
    parse_labeled_size(&text, &["上传量", "上傳量", "Uploaded"]).is_some()
        && parse_labeled_size(&text, &["下载量", "下載量", "Downloaded"]).is_some()
}

pub(super) fn parse_labeled_size(text: &str, labels: &[&str]) -> Option<u64> {
    labels.iter().find_map(|label| {
        let expression = format!(
            r"(?i){}\s*[^0-9]{{0,48}}([0-9][0-9,.]*)\s*(bytes?|[kmgtpez]i?b)",
            escape(label)
        );
        let captures = Regex::new(&expression).ok()?.captures(text)?;
        size_from_parts(captures.get(1)?.as_str(), captures.get(2)?.as_str())
    })
}

fn parse_labeled_number(text: &str, labels: &[&str]) -> Option<f64> {
    labels.iter().find_map(|label| {
        let expression = format!(r"(?i){}\s*[^0-9]{{0,48}}([0-9][0-9,.]*)", escape(label));
        let value = Regex::new(&expression)
            .ok()?
            .captures(text)?
            .get(1)?
            .as_str()
            .replace(',', "");
        value.parse::<f64>().ok()
    })
}

fn parse_labeled_integer(text: &str, labels: &[&str]) -> Option<u32> {
    // Counts must immediately follow their label. The generic number parser
    // also matches "Seeding Bonus Per Hour", and truncates fractional rates.
    labels.iter().find_map(|label| {
        let expression = format!(
            r"(?i){}\s*[:：]?\s*([0-9][0-9,]*)(?:$|[\s)）;；|])",
            escape(label)
        );
        Regex::new(&expression)
            .ok()?
            .captures(text)?
            .get(1)?
            .as_str()
            .replace(',', "")
            .parse()
            .ok()
    })
}

fn parse_labeled_u64(text: &str, labels: &[&str]) -> Option<u64> {
    parse_labeled_number(text, labels).and_then(|value| {
        if value.is_finite() && value >= 0.0 && value <= u64::MAX as f64 {
            Some(value as u64)
        } else {
            None
        }
    })
}

fn parse_labeled_duration_seconds(text: &str, labels: &[&str]) -> Option<u64> {
    labels.iter().find_map(|label| {
        let expression = format!(
            r"(?i){}\s*[^0-9]{{0,48}}([0-9][0-9,.]*)\s*(years?|months?|weeks?|days?|hours?|hrs?|minutes?|mins?|seconds?|secs?|年|月|周|週|天|日|小时|小時|时|時|分钟|分鐘|分|秒)",
            escape(label)
        );
        let captures = Regex::new(&expression).ok()?.captures(text)?;
        let value = captures.get(1)?.as_str().replace(',', "").parse::<f64>().ok()?;
        if !value.is_finite() || value < 0.0 {
            return None;
        }
        let unit = captures.get(2)?.as_str().to_ascii_lowercase();
        let multiplier = if matches!(unit.as_str(), "year" | "years" | "年") {
            365 * 24 * 60 * 60
        } else if matches!(unit.as_str(), "month" | "months" | "月") {
            30 * 24 * 60 * 60
        } else if matches!(unit.as_str(), "week" | "weeks" | "周" | "週") {
            7 * 24 * 60 * 60
        } else if matches!(unit.as_str(), "day" | "days" | "天" | "日") {
            24 * 60 * 60
        } else if matches!(unit.as_str(), "hour" | "hours" | "hr" | "hrs" | "小时" | "小時" | "时" | "時") {
            60 * 60
        } else if matches!(unit.as_str(), "minute" | "minutes" | "min" | "mins" | "分钟" | "分鐘" | "分") {
            60
        } else {
            1
        };
        Some((value * multiplier as f64).min(u64::MAX as f64) as u64)
    })
}

fn parse_table_labeled_value(html: &str, labels: &[&str]) -> Option<String> {
    let document = Html::parse_document(html);
    let row_selector = Selector::parse("tr").ok()?;
    let cell_selector = Selector::parse(":scope > th, :scope > td").ok()?;
    let image_selector = Selector::parse("img[title], img[alt]").ok()?;

    let mut fallback = None;
    for row in document.select(&row_selector) {
        let cells = row.select(&cell_selector).collect::<Vec<_>>();
        if cells.len() < 2 {
            continue;
        }
        let Some(label_text) = normalize_text(cells[0].text()) else {
            continue;
        };
        for label in labels {
            let exact = label_text.trim().eq_ignore_ascii_case(label);
            if !exact
                && !label_text
                    .to_ascii_lowercase()
                    .contains(&label.to_ascii_lowercase())
            {
                continue;
            }
            let value = cells[1]
                .select(&image_selector)
                .find_map(|image| {
                    image
                        .value()
                        .attr("title")
                        .or_else(|| image.value().attr("alt"))
                })
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .or_else(|| normalize_text(cells[1].text()));
            if exact {
                return value;
            }
            fallback = fallback.or(value);
        }
    }
    fallback
}

fn parse_profile_value(html: &str, labels: &[&str]) -> Option<String> {
    parse_table_labeled_value(html, labels).or_else(|| {
        let document = Html::parse_document(html);
        let selector = Selector::parse("span").ok()?;
        document.select(&selector).find_map(|element| {
            let label = normalize_text(element.text())?;
            if !labels.iter().any(|expected| {
                label
                    .trim_end_matches([':', '：'])
                    .trim()
                    .eq_ignore_ascii_case(expected)
            }) {
                return None;
            }
            normalize_text(element.next_sibling_element()?.text())
        })
    })
}

fn first_number(text: &str) -> Option<f64> {
    Regex::new(r"[0-9][0-9,.]*")
        .ok()?
        .find(text)?
        .as_str()
        .replace(',', "")
        .parse()
        .ok()
}

fn parse_profile_number(html: &str, labels: &[&str]) -> Option<f64> {
    // U2's rounded visible UCoin amount has its precise balance in the span title.
    let document = Html::parse_document(html);
    let selector = Selector::parse("td.rowhead").ok()?;
    for label in document.select(&selector) {
        if label.text().collect::<String>().contains("UCoin") && labels.contains(&"UCoin") {
            let title_selector = Selector::parse("span[title]").ok()?;
            if let Some(number) = label
                .next_sibling_element()?
                .select(&title_selector)
                .find_map(|span| span.value().attr("title").and_then(first_number))
            {
                return Some(number);
            }
        }
    }
    parse_profile_value(html, labels)
        .as_deref()
        .and_then(first_number)
}

fn parse_profile_level(html: &str) -> Option<String> {
    parse_table_labeled_value(html, &["等级", "等級", "Class"])
        .or_else(|| profile_class_level(html))
}

fn profile_class_level(html: &str) -> Option<String> {
    let document = Html::parse_document(html);
    let selector = Selector::parse("a[href*='userdetails.php'][class*='_Name']").ok()?;
    document.select(&selector).find_map(|element| {
        element
            .value()
            .classes()
            .find_map(|class| class.strip_suffix("_Name").map(str::to_string))
    })
}

fn profile_uuid(html: &str) -> Option<String> {
    let document = Html::parse_document(html);
    let selector = Selector::parse("a[href*='userdetails.php'][class*='Name']").ok()?;
    let link = document.select(&selector).next()?.value().attr("href")?;
    Url::parse("https://tracker.invalid/")
        .ok()?
        .join(link)
        .ok()?
        .query_pairs()
        .find(|(key, value)| key == "uuid" && !value.is_empty())
        .map(|(_, value)| value.into_owned())
}

fn parse_bonus_rates(html: &str, is_u2: bool) -> (Option<f64>, Option<f64>) {
    let document = Html::parse_document(html);
    let text = extract_visible_text(html);
    if is_u2 {
        let rate = Regex::new(r"UCoin([0-9][0-9,.]*)")
            .unwrap()
            .captures(&text)
            .and_then(|capture| first_number(&capture[1]))
            .map(|daily| daily / 24.0);
        return (rate, None);
    }
    // Qingwa publishes the final hourly amount and seeding points in a summary table.
    let heading_selector = Selector::parse("h1").unwrap();
    for heading in document.select(&heading_selector) {
        if heading
            .text()
            .collect::<String>()
            .contains("每小时获得的合计蝌蚪")
        {
            if let Some(container) = heading.next_sibling_element() {
                let rows = Selector::parse("tr").unwrap();
                let cells = Selector::parse("td").unwrap();
                if let Some(row) = container.select(&rows).last() {
                    let values: Vec<_> = row
                        .select(&cells)
                        .map(|cell| first_number(&cell.text().collect::<String>()))
                        .collect();
                    return (
                        values.last().copied().flatten(),
                        values.get(16).copied().flatten(),
                    );
                }
            }
        }
    }
    let hhan_selector = Selector::parse(".grid .row-span-4").unwrap();
    if let Some(total) = document
        .select(&hhan_selector)
        .next()
        .and_then(|element| first_number(&element.text().collect::<String>()))
    {
        let seed = parse_hourly_amounts(&text).first().copied();
        return (Some(total), seed);
    }
    let summary_selector = Selector::parse("#outer td[rowspan]").unwrap();
    if let Some(total) = document
        .select(&summary_selector)
        .next()
        .and_then(|element| first_number(&element.text().collect::<String>()))
    {
        return (Some(total), None);
    }
    // UB and HDHome can show multiple additive rewards. Exclude the separate
    // seeding-points paragraph, which is not spendable bonus.
    let selector = Selector::parse("div").unwrap();
    let mut amounts = Vec::new();
    for element in document.select(&selector) {
        if element
            .select(&selector)
            .any(|child| !parse_hourly_amounts(&child.text().collect::<String>()).is_empty())
        {
            continue;
        }
        let text = normalize_text(element.text()).unwrap_or_default();
        if text.contains("对于做种积分") {
            continue;
        }
        amounts.extend(parse_hourly_amounts(&text));
    }
    let total = if amounts.is_empty() {
        parse_hourly_amounts(&text).first().copied()
    } else {
        Some(amounts.into_iter().sum())
    };
    (total, None)
}

fn parse_hourly_amounts(text: &str) -> Vec<f64> {
    Regex::new(r"(?i)(?:你当前每小时能获取|你當前每小時能獲取|You are currently getting)\s*([0-9][0-9,.]*)")
        .unwrap().captures_iter(text).filter_map(|capture| first_number(&capture[1])).collect()
}

fn parse_user_datetime_millis(value: &str) -> Option<i64> {
    let value = value.trim();
    if let Ok(timestamp) = value.parse::<i64>() {
        return normalize_timestamp_millis(timestamp);
    }
    if let Ok(datetime) = DateTime::parse_from_rfc3339(value) {
        return Some(datetime.timestamp_millis());
    }
    if let Some(timestamp) = extract_datetime_to_utc8(value) {
        return timestamp.checked_mul(1000);
    }
    let date = NaiveDate::parse_from_str(value.split(" (").next()?, "%Y-%m-%d").ok()?;
    let timezone = FixedOffset::east_opt(8 * 3600)?;
    timezone
        .from_local_datetime(&date.and_hms_opt(0, 0, 0)?)
        .single()
        .map(|datetime| datetime.timestamp_millis())
}

fn normalize_timestamp_millis(timestamp: i64) -> Option<i64> {
    if timestamp.unsigned_abs() < 100_000_000_000 {
        timestamp.checked_mul(1000)
    } else {
        Some(timestamp)
    }
}

fn parse_message_count(html: &str) -> Option<u64> {
    let document = Html::parse_document(html);
    let selectors = [
        "td[style*='background: red'] a[href*='messages.php']",
        "td[style*='background:red'] a[href*='messages.php']",
        "div.relative:has(#display-message-alert) a.flex[href*='messages.php']",
    ];
    for selector in selectors {
        let Ok(selector) = Selector::parse(selector) else {
            continue;
        };
        for element in document.select(&selector) {
            let Some(text) = normalize_text(element.text()) else {
                continue;
            };
            // Assessment notifications also link to messages.php, and contain dates/targets.
            if text.contains("考核") || text.contains("指标") || text.contains("Assessment") {
                continue;
            }
            if let Some(value) = first_integer(&text) {
                return Some(value);
            }
        }
    }
    Some(0)
}

fn first_integer(value: &str) -> Option<u64> {
    Regex::new(r"[0-9][0-9,]*")
        .ok()?
        .find(value)?
        .as_str()
        .replace(',', "")
        .parse()
        .ok()
}

fn extract_avatar(html: &str) -> Option<String> {
    let document = Html::parse_document(html);
    let selectors = [
        "img.avatar",
        "img[class*='avatar']",
        "img[src*='/avatars/']",
        "img[src*='avatar']",
    ];
    selectors.iter().find_map(|selector| {
        let selector = Selector::parse(selector).ok()?;
        document
            .select(&selector)
            .find_map(|image| image.value().attr("src"))
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    })
}

fn detect_donor(html: &str) -> bool {
    let document = Html::parse_document(html);
    let Ok(selector) = Selector::parse("h1 img[alt], h1 img[title]") else {
        return false;
    };
    document.select(&selector).any(|image| {
        [image.value().attr("alt"), image.value().attr("title")]
            .into_iter()
            .flatten()
            .any(|value| {
                let value = value.to_ascii_lowercase();
                value.contains("donor") || value.contains("捐赠") || value.contains("捐贈")
            })
    })
}

fn parse_hnr_counts(html: &str) -> (Option<u64>, Option<u64>) {
    let document = Html::parse_document(html);
    let selector = Selector::parse("#info_block a[href*='myhr.php']").unwrap();
    let text = document
        .select(&selector)
        .next_back()
        .and_then(|element| normalize_text(element.text()))
        .unwrap_or_default();
    let expression = Regex::new(r"([0-9,]+)\s*/\s*([0-9,]+)(?:\s*/\s*[0-9,]+)?").unwrap();
    let (mut pre, mut unsatisfied) = (0u64, 0u64);
    for captures in expression.captures_iter(&text) {
        pre = pre.saturating_add(captures[1].replace(',', "").parse().unwrap_or(0));
        unsatisfied = unsatisfied.saturating_add(captures[2].replace(',', "").parse().unwrap_or(0));
    }
    (Some(pre), Some(unsatisfied))
}

fn parse_user_torrent_ajax_summary(html: &str) -> Option<(u32, Option<u64>)> {
    let text = extract_visible_text(html);
    let summary_expression =
        Regex::new(r"(?i)([0-9][0-9,]*)\s*(?:条记录|條記錄|records?)?\s*\|\s*(?:总大小|總大小|Total size)?\s*[:：]?\s*([0-9][0-9,.]*)\s*([kmgtpez]i?b|bytes?)").ok()?;
    if let Some(captures) = summary_expression.captures(&text) {
        let count = captures.get(1)?.as_str().replace(',', "").parse().ok()?;
        let size = size_from_parts(captures.get(2)?.as_str(), captures.get(3)?.as_str());
        return Some((count, size));
    }

    if Regex::new(r"(?i)no records?|没有记录|沒有記錄")
        .ok()?
        .is_match(&text)
    {
        return Some((0, Some(0)));
    }

    let record_expression = Regex::new(r"(?i)([0-9][0-9,]*)\s*(?:条记录|條記錄|records?)").ok()?;
    let mut record_count = None;
    if let Some(captures) = record_expression.captures(&text) {
        let count = captures.get(1)?.as_str().replace(',', "").parse().ok()?;
        record_count = Some(count);
        let size = parse_labeled_size(&text, &["总大小", "總大小", "大小", "Total size"]);
        if size.is_some() || count == 0 {
            return Some((count, size.or(Some(0))));
        }
        // A record count alone does not provide the total size; sum the table below.
    }

    let document = Html::parse_document(html);
    let table_selector = Selector::parse("table").ok()?;
    let row_selector = Selector::parse("tr").ok()?;
    let Some(table) = document.select(&table_selector).next_back() else {
        return record_count.map(|count| (count, None));
    };
    let rows = table.select(&row_selector).skip(1).collect::<Vec<_>>();
    if rows.is_empty() {
        return record_count.map(|count| (count, None));
    }
    let size_expression = Regex::new(r"(?i)^([0-9][0-9,.]*)\s*([kmgtpez]i?b|bytes?)$").ok()?;
    let cell_selector = Selector::parse(":scope > td").ok()?;
    let size_column = rows[0].select(&cell_selector).position(|cell| {
        size_expression.is_match(&normalize_text(cell.text()).unwrap_or_default())
    });
    let mut total_size = 0u64;
    let mut has_size = false;
    for row in &rows {
        let row_text = size_column
            .and_then(|column| row.select(&cell_selector).nth(column))
            .and_then(|cell| normalize_text(cell.text()))
            .unwrap_or_default();
        if let Some(captures) = size_expression.captures(&row_text)
            && let Some(size) =
                size_from_parts(captures.get(1)?.as_str(), captures.get(2)?.as_str())
        {
            total_size = total_size.saturating_add(size);
            has_size = true;
        }
    }
    Some((
        record_count.unwrap_or_else(|| u32::try_from(rows.len()).unwrap_or(u32::MAX)),
        (has_size && record_count.is_none_or(|count| count as usize == rows.len()))
            .then_some(total_size),
    ))
}

#[cfg(test)]
fn parse_seeding_ajax_count(html: &str) -> Option<u32> {
    parse_user_torrent_ajax_summary(html).map(|(count, _)| count)
}

fn looks_like_login_page(html: &str, final_url: &Url) -> bool {
    if final_url.path().to_ascii_lowercase().contains("login.php") {
        return true;
    }
    let lower = html.to_ascii_lowercase();
    (lower.contains("type=\"password\"") || lower.contains("type='password'"))
        && (lower.contains("action=\"login.php")
            || lower.contains("action='login.php")
            || lower.contains("action=\"takelogin.php")
            || lower.contains("action='takelogin.php")
            || lower.contains("id=\"form-login\"")
            || lower.contains("id='form-login'")
            || lower.contains("name=\"login\"")
            || lower.contains("name='login'"))
        && extract_current_user(html).is_none()
}

fn looks_like_cloudflare_challenge(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    lower.contains("cf-chl-")
        || lower.contains("challenge-platform")
        || lower.contains("just a moment...")
        || lower.contains("cloudflare ray id")
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

fn detect_download_factor(upper: &str) -> Option<f64> {
    if contains_any(
        upper,
        &[
            "50%DL",
            "50% DL",
            "0.5X",
            "DOWNLOAD 50%",
            "50%DOWN",
            "半价",
            "五折",
        ],
    ) {
        Some(0.5)
    } else if contains_any(
        upper,
        &["30%DL", "30% DL", "0.3X", "DOWNLOAD 30%", "30%DOWN", "七折"],
    ) {
        Some(0.3)
    } else {
        None
    }
}

fn detect_upload_factor(upper: &str) -> Option<f64> {
    if contains_any(
        upper,
        &[
            "2XUP",
            "2X UP",
            "2XUPLOAD",
            "UPLOAD 200%",
            "UP 200%",
            "双倍上传",
        ],
    ) {
        Some(2.0)
    } else if contains_any(upper, &["0XUP", "UPLOAD 0%", "UP 0%", "零上传", "不计上传"]) {
        Some(0.0)
    } else {
        None
    }
}

fn detect_free_end_timestamp(html: &str) -> Option<i64> {
    const KEYWORDS: &[&str] = &[
        "free结束",
        "free 到期",
        "free截止",
        "free until",
        "promotion until",
        "限时免费",
        "優惠到期",
        "促销结束",
    ];

    let lower = html.to_lowercase();
    for keyword in KEYWORDS {
        let keyword_lower = keyword.to_lowercase();
        if let Some(pos) = lower.find(&keyword_lower) {
            let end = (pos + 160).min(html.len());
            if let Some(timestamp) = extract_datetime_to_utc8(&html[pos..end]) {
                return Some(timestamp);
            }
        }
    }

    extract_datetime_to_utc8(html)
}

fn extract_datetime_to_utc8(text: &str) -> Option<i64> {
    let tz = FixedOffset::east_opt(8 * 3600)?;
    for window in text.as_bytes().windows(19) {
        // 窗口可能切在多字节 UTF-8 字符中间，此时跳过而不是中止整个扫描。
        let Ok(candidate) = std::str::from_utf8(window) else {
            continue;
        };
        if looks_like_datetime(candidate)
            && let Ok(naive) = NaiveDateTime::parse_from_str(candidate, "%Y-%m-%d %H:%M:%S")
            && let Some(datetime) = tz.from_local_datetime(&naive).single()
        {
            return Some(datetime.timestamp());
        }
    }
    None
}

fn looks_like_datetime(value: &str) -> bool {
    value.len() == 19
        && value.chars().enumerate().all(|(index, ch)| match index {
            4 | 7 => ch == '-',
            10 => ch == ' ',
            13 | 16 => ch == ':',
            _ => ch.is_ascii_digit(),
        })
}

fn size_from_parts(number: &str, unit: &str) -> Option<u64> {
    let number: f64 = number.replace(',', "").parse().ok()?;
    if !number.is_finite() || number < 0.0 {
        return None;
    }
    let normalized_unit = unit.trim().to_ascii_lowercase();
    let power = match normalized_unit.as_str() {
        value if value.starts_with("zb") || value.starts_with("zib") => 7,
        value if value.starts_with("eb") || value.starts_with("eib") => 6,
        value if value.starts_with("pb") || value.starts_with("pib") => 5,
        value if value.starts_with("tb") || value.starts_with("tib") => 4,
        value if value.starts_with("gb") || value.starts_with("gib") => 3,
        value if value.starts_with("mb") || value.starts_with("mib") => 2,
        value if value.starts_with("kb") || value.starts_with("kib") => 1,
        _ => 0,
    };
    let bytes = number * 1024_f64.powi(power);
    Some(bytes.min(u64::MAX as f64) as u64)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::Router;
    use axum::extract::Query;
    use axum::http::StatusCode;
    use axum::response::Html;
    use axum::routing::get;
    use base64::Engine as _;
    use reqwest::Client;
    use reqwest::header::{COOKIE, HeaderMap, HeaderValue};

    use super::{
        NexusPhpAdapter, SiteAuth, describe_reqwest_error, detect_donor, extract_avatar,
        extract_current_user, extract_user_id_from_cookie, extract_username, extract_visible_text,
        looks_like_login_page, parse_hnr_counts, parse_labeled_duration_seconds,
        parse_labeled_integer, parse_labeled_number, parse_labeled_size, parse_message_count,
        parse_seeding_ajax_count, parse_table_labeled_value, parse_user_datetime_millis,
        parse_user_torrent_ajax_summary,
    };
    use crate::site::SiteAdapter;

    async fn serve_fixture(app: Router) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{address}"), server)
    }

    fn fixture_adapter(base_url: String) -> NexusPhpAdapter {
        NexusPhpAdapter::new(
            base_url,
            SiteAuth::Cookie {
                cookie: "session=valid".to_string(),
            },
            HeaderMap::new(),
            Client::builder().no_proxy().build().unwrap(),
        )
    }

    #[test]
    fn custom_headers_are_applied_without_overriding_authentication() {
        let mut custom = HeaderMap::new();
        custom.insert("x-browser-profile", HeaderValue::from_static("desktop"));
        custom.insert(COOKIE, HeaderValue::from_static("stale=1"));
        let adapter = NexusPhpAdapter::new(
            "https://tracker.example".to_string(),
            SiteAuth::Cookie {
                cookie: "uid=1".to_string(),
            },
            custom,
            Client::new(),
        );

        let headers = adapter.build_headers();
        assert_eq!(headers["x-browser-profile"], "desktop");
        assert_eq!(headers[COOKIE], "uid=1");
    }

    #[test]
    fn extracts_user_id_from_legacy_and_current_nexusphp_cookies() {
        let legacy_uid = base64::engine::general_purpose::STANDARD.encode("42");
        assert_eq!(
            extract_user_id_from_cookie(&format!("c_secure_uid={legacy_uid}; session=ok"))
                .as_deref(),
            Some("42")
        );

        let token = base64::engine::general_purpose::STANDARD
            .encode(r#"{"user_id":314,"expires":1999999999}.signature"#);
        let token = urlencoding::encode(&token);
        assert_eq!(
            extract_user_id_from_cookie(&format!("c_secure_pass={token}; c_lang_folder=chs"))
                .as_deref(),
            Some("314")
        );
        // A legacy password/hash is not a uid, even if it happens to contain only digits.
        assert_eq!(
            extract_user_id_from_cookie("c_secure_pass=1234567890"),
            None
        );
        assert_eq!(
            extract_user_id_from_cookie(&format!("c_secure_pass={legacy_uid}")),
            None
        );
    }

    #[test]
    fn unrelated_member_links_are_not_treated_as_the_current_user() {
        assert!(extract_current_user(
            r#"<div id="news"><a class="User_Name" href="userdetails.php?id=99">Moderator</a></div>"#
        ).is_none());
        assert!(
            extract_current_user(
                r#"<div id="info_block"><a href="userdetails.php?id=invalid">Alice</a></div>"#
            )
            .is_none()
        );
    }

    #[tokio::test]
    async fn unsupported_or_malformed_credentials_fail_before_sending_requests() {
        let mut adapter = fixture_adapter("not-a-url".to_string());
        for auth in [
            SiteAuth::Passkey {
                passkey: "secret".to_string(),
            },
            SiteAuth::ApiKey {
                api_key: "secret".to_string(),
            },
        ] {
            adapter.auth = auth;
            let error = adapter.get_user_stats().await.unwrap_err();
            assert!(error.contains("需要 Cookie"), "{error}");
            assert!(!error.contains("secret"));
        }
        adapter.auth = SiteAuth::Cookie {
            cookie: "session=secret\ninvalid".to_string(),
        };
        let error = adapter.get_user_stats().await.unwrap_err();
        assert!(error.contains("Cookie 格式无效"), "{error}");
        assert!(!error.contains("secret"));
    }

    #[tokio::test]
    async fn incomplete_profile_falls_back_to_homepage_and_preserves_profile_fields() {
        let profile_hits = Arc::new(AtomicUsize::new(0));
        let hits = Arc::clone(&profile_hits);
        let (base_url, server) = serve_fixture(
            Router::new()
                .route(
                    "/userdetails.php",
                    get(move || async move {
                        hits.fetch_add(1, Ordering::SeqCst);
                        Html(
                            r#"<div id="info_block"><a href="userdetails.php?id=42">Alice</a></div>
                    <table><tr><td class="rowhead">等级</td><td>Elite User</td></tr></table>"#,
                        )
                    }),
                )
                .route(
                    "/index.php",
                    get(|| async {
                        Html(
                            r#"<div id="info_block"><a href="userdetails.php?id=42">Alice</a>
                    上传量 2 GiB 下载量 1 GiB</div>"#,
                        )
                    }),
                ),
        )
        .await;
        let stats = fixture_adapter(base_url)
            .with_cached_user_id(Some("42"))
            .get_user_stats()
            .await
            .unwrap();
        assert_eq!(stats.uid.as_deref(), Some("42"));
        assert_eq!(stats.uploaded, 2_147_483_648);
        assert_eq!(stats.downloaded, 1_073_741_824);
        assert_eq!(stats.details.level_name.as_deref(), Some("Elite User"));
        assert_eq!(profile_hits.load(Ordering::SeqCst), 1);
        server.abort();
    }

    #[tokio::test]
    async fn homepage_can_supply_an_alternative_profile_path_for_the_same_id() {
        let (base_url, server) = serve_fixture(
            Router::new()
                .route("/userdetails.php", get(|| async { StatusCode::NOT_FOUND }))
                .route(
                    "/index.php",
                    get(|| async {
                        Html(r#"<div id="info_block"><a href="user.php?id=42">Alice</a></div>"#)
                    }),
                )
                .route(
                    "/user.php",
                    get(|Query(query): Query<HashMap<String, String>>| async move {
                        assert_eq!(query.get("id").map(String::as_str), Some("42"));
                        Html(
                            r#"<div id="info_block"><a href="user.php?id=42">Alice</a></div>
                    上传量 2 GiB 下载量 1 GiB"#,
                        )
                    }),
                ),
        )
        .await;
        let stats = fixture_adapter(base_url)
            .with_cached_user_id(Some("42"))
            .get_user_stats()
            .await
            .unwrap();
        assert_eq!(stats.uid.as_deref(), Some("42"));
        assert_eq!(stats.uploaded, 2_147_483_648);
        server.abort();
    }

    #[tokio::test]
    async fn stale_profile_identity_is_rediscovered_without_mixing_accounts() {
        let profile_hits = Arc::new(AtomicUsize::new(0));
        let hits = Arc::clone(&profile_hits);
        let (base_url, server) = serve_fixture(Router::new()
            .route("/userdetails.php", get(move |Query(query): Query<HashMap<String, String>>| async move {
                hits.fetch_add(1, Ordering::SeqCst);
                let totals = match query.get("id").map(String::as_str) {
                    Some("42") => "上传量 99 GiB 下载量 99 GiB",
                    Some("7") => "上传量 2 GiB 下载量 1 GiB",
                    other => panic!("unexpected profile id: {other:?}"),
                };
                Html(format!(r#"<div id="info_block"><a href="userdetails.php?id=7">Bob</a></div>{totals}"#))
            }))
            .route("/index.php", get(|| async {
                Html(r#"<div id="info_block"><a href="userdetails.php?id=7">Bob</a></div>"#)
            }))).await;
        let stats = fixture_adapter(base_url)
            .with_cached_user_id(Some("42"))
            .get_user_stats()
            .await
            .unwrap();
        assert_eq!(stats.uid.as_deref(), Some("7"));
        assert_eq!(stats.username, "Bob");
        assert_eq!(stats.uploaded, 2_147_483_648);
        assert_eq!(stats.downloaded, 1_073_741_824);
        assert_eq!(profile_hits.load(Ordering::SeqCst), 2);
        server.abort();
    }

    #[tokio::test]
    async fn cookie_uid_overrides_cache_and_preserves_site_subdirectory() {
        let (base_url, server) = serve_fixture(Router::new().route(
            "/tracker/userdetails.php",
            get(
                |headers: HeaderMap, Query(query): Query<HashMap<String, String>>| async move {
                    assert_eq!(headers[COOKIE], "c_secure_uid=7; session=valid");
                    assert_eq!(headers["x-browser-profile"], "desktop");
                    assert_eq!(query.get("id").map(String::as_str), Some("7"));
                    Html(
                        r#"<div id="info_block"><a href="userdetails.php?id=7">Bob</a></div>
                    上传量 2 GiB 下载量 1 GiB"#,
                    )
                },
            ),
        ))
        .await;
        let mut adapter =
            fixture_adapter(format!("{base_url}/tracker/")).with_cached_user_id(Some("42"));
        adapter.auth = SiteAuth::Cookie {
            cookie: "c_secure_uid=7; session=valid".to_string(),
        };
        adapter
            .request_headers
            .insert("x-browser-profile", HeaderValue::from_static("desktop"));
        let stats = adapter.get_user_stats().await.unwrap();
        assert_eq!(stats.uid.as_deref(), Some("7"));
        assert_eq!(stats.uploaded, 2_147_483_648);
        server.abort();
    }

    #[tokio::test]
    async fn page_errors_distinguish_challenges_login_and_http_status() {
        let (base_url, server) = serve_fixture(Router::new()
            .route("/challenge", get(|| async {
                (StatusCode::FORBIDDEN, Html("<title>Just a moment...</title><script src='/cdn-cgi/challenge-platform'></script>"))
            }))
            .route("/login.php", get(|| async {
                Html(r#"<form id="form-login"><input type="password"></form>"#)
            }))
            .route("/unavailable", get(|| async { StatusCode::SERVICE_UNAVAILABLE }))).await;
        let adapter = fixture_adapter(base_url.clone());
        for (path, expected) in [
            ("challenge", "Cloudflare"),
            ("login.php", "登录页"),
            ("unavailable", "HTTP 503"),
        ] {
            let error = adapter
                .fetch_html_page(&format!("{base_url}/{path}"), "首页")
                .await
                .unwrap_err();
            assert!(error.contains(expected), "{error}");
        }
        let error = adapter
            .client
            .get(format!("{base_url}/unavailable?passkey=secret-token"))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap_err();
        let message = describe_reqwest_error(error);
        assert!(message.contains("503"));
        assert!(!message.contains("secret-token"));
        server.abort();
    }

    #[tokio::test]
    async fn failed_profile_context_is_retained_when_homepage_has_no_totals() {
        let (base_url, server) = serve_fixture(
            Router::new()
                .route("/userdetails.php", get(|| async { StatusCode::FORBIDDEN }))
                .route("/index.php", get(|| async { Html("<html>Welcome</html>") })),
        )
        .await;
        let error = fixture_adapter(base_url)
            .with_cached_user_id(Some("42"))
            .get_user_stats()
            .await
            .unwrap_err();
        assert!(error.contains("HTTP 403"), "{error}");
        assert!(error.contains("没有找到上传量"), "{error}");
        assert!(!error.contains("Cookie 无效"), "{error}");
        server.abort();
    }

    #[test]
    fn recognizes_qingwa_style_login_form() {
        let url = reqwest::Url::parse("https://www.qingwapt.com/index.php").unwrap();
        let html = r#"<form id="form-login" method="post" action="takelogin.php">
            <input name="password" type="password">
        </form>"#;
        assert!(looks_like_login_page(html, &url));
    }

    #[test]
    fn extracts_username_without_requiring_a_profile_anchor() {
        assert_eq!(
            extract_username(
                r#"<div id="info_block"><span class="User_Name"><b>Alice</b></span></div>"#
            )
            .as_deref(),
            Some("Alice")
        );
    }

    #[tokio::test]
    async fn html_stats_do_not_require_a_current_user_anchor_or_api_probe() {
        let api_hits = Arc::new(AtomicUsize::new(0));
        let api_hits_for_route = Arc::clone(&api_hits);
        let app = Router::new()
            .route(
                "/index.php",
                get(|| async {
                    Html(
                        r#"<html><body>
                            <div id="info_block">
                              <span class="User_Name">Alice</span>
                              上传量 1 GiB 下载量 512 MiB 分享率 2.0
                            </div>
                        </body></html>"#,
                    )
                }),
            )
            .route("/mybonus.php", get(|| async { Html("<html></html>") }))
            .route(
                "/api/user",
                get(move || async move {
                    api_hits_for_route.fetch_add(1, Ordering::SeqCst);
                    StatusCode::NOT_FOUND
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let adapter = NexusPhpAdapter::new(
            format!("http://{address}"),
            SiteAuth::Cookie {
                cookie: "session=valid".to_string(),
            },
            HeaderMap::new(),
            Client::new(),
        );
        let stats = adapter.get_user_stats().await.unwrap();

        assert_eq!(stats.username, "Alice");
        assert_eq!(stats.uploaded, 1_073_741_824);
        assert_eq!(stats.downloaded, 536_870_912);
        assert_eq!(api_hits.load(Ordering::SeqCst), 0);
        server.abort();
    }

    #[tokio::test]
    async fn cached_user_id_skips_homepage_and_nonstandard_api() {
        let homepage_hits = Arc::new(AtomicUsize::new(0));
        let homepage_hits_for_route = Arc::clone(&homepage_hits);
        let api_hits = Arc::new(AtomicUsize::new(0));
        let api_hits_for_route = Arc::clone(&api_hits);
        let app = Router::new()
            .route(
                "/index.php",
                get(move || async move {
                    homepage_hits_for_route.fetch_add(1, Ordering::SeqCst);
                    StatusCode::SERVICE_UNAVAILABLE
                }),
            )
            .route(
                "/userdetails.php",
                get(|| async {
                    Html(
                        r#"<html><body>
                            <div id="info_block">
                              <a class="User_Name" href="/userdetails.php?id=42">Alice</a>
                            </div>
                            <table><tr><td class="rowhead">传输</td>
                              <td>上传量 2 GiB 下载量 1 GiB 分享率 2.0</td>
                            </tr></table>
                        </body></html>"#,
                    )
                }),
            )
            .route(
                "/api/user",
                get(move || async move {
                    api_hits_for_route.fetch_add(1, Ordering::SeqCst);
                    StatusCode::NOT_FOUND
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let adapter = NexusPhpAdapter::new(
            format!("http://{address}"),
            SiteAuth::Cookie {
                cookie: "session=valid".to_string(),
            },
            HeaderMap::new(),
            Client::new(),
        )
        .with_cached_user_id(Some("42"));
        let stats = adapter.get_user_stats().await.unwrap();

        assert_eq!(stats.uid.as_deref(), Some("42"));
        assert_eq!(stats.username, "Alice");
        assert_eq!(stats.uploaded, 2_147_483_648);
        assert_eq!(stats.downloaded, 1_073_741_824);
        assert_eq!(homepage_hits.load(Ordering::SeqCst), 0);
        assert_eq!(api_hits.load(Ordering::SeqCst), 0);
        server.abort();
    }

    #[tokio::test]
    async fn homepage_failure_is_not_masked_by_nonstandard_api_probe() {
        let api_hits = Arc::new(AtomicUsize::new(0));
        let api_hits_for_route = Arc::clone(&api_hits);
        let app = Router::new()
            .route(
                "/index.php",
                get(|| async { StatusCode::SERVICE_UNAVAILABLE }),
            )
            .route(
                "/api/user",
                get(move || async move {
                    api_hits_for_route.fetch_add(1, Ordering::SeqCst);
                    StatusCode::NOT_FOUND
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let adapter = NexusPhpAdapter::new(
            format!("http://{address}"),
            SiteAuth::Cookie {
                cookie: "session=valid".to_string(),
            },
            HeaderMap::new(),
            Client::new(),
        );
        let error = adapter.get_user_stats().await.unwrap_err();

        assert!(error.contains("首页返回 HTTP 503 Service Unavailable"));
        assert!(!error.contains("/api/user"));
        assert_eq!(api_hits.load(Ordering::SeqCst), 0);
        server.abort();
    }

    #[test]
    fn detects_free_and_hr_from_detail_html() {
        let attrs = NexusPhpAdapter::detect_torrent_attributes(
            r#"<html><body><span>FREE</span><span>2XUP</span><span>H&amp;R</span></body></html>"#,
        );

        assert!(attrs.free);
        assert!(attrs.hit_and_run);
        assert_eq!(attrs.download_volume_factor, Some(0.0));
        assert_eq!(attrs.upload_volume_factor, Some(2.0));
    }

    #[test]
    fn detects_free_end_time_from_detail_html() {
        let attrs = NexusPhpAdapter::detect_torrent_attributes(
            r#"<html><body><span>FREE</span><span>Free到期：2026-04-16 12:30:00</span></body></html>"#,
        );

        assert!(attrs.free_end_timestamp.is_some());
    }

    #[test]
    fn parses_pt_depiler_style_nexusphp_user_pages() {
        let html = r#"
            <html><body>
              <div id="info_block">
                <a class="User_Name" href="/userdetails.php?id=42"><b>Alice</b></a>
              </div>
              <table>
                <tr><td class="rowhead">传输</td><td>上传量 1.5 TiB 下载量 256 GiB 分享率 6.0</td></tr>
                <tr><td class="rowhead">魔力值</td><td>12,345.67</td></tr>
                <tr><td class="rowhead">当前做种</td><td>88</td></tr>
              </table>
            </body></html>
        "#;
        let identity = extract_current_user(html).unwrap();
        assert_eq!(identity.uid, "42");
        assert_eq!(identity.username.as_deref(), Some("Alice"));
        assert_eq!(identity.href, "/userdetails.php?id=42");

        let text = extract_visible_text(html);
        assert_eq!(
            parse_labeled_size(&text, &["上传量"]),
            Some(1_649_267_441_664)
        );
        assert_eq!(
            parse_labeled_size(&text, &["下载量"]),
            Some(274_877_906_944)
        );
        assert_eq!(parse_labeled_number(&text, &["分享率"]), Some(6.0));
        assert_eq!(parse_labeled_number(&text, &["魔力值"]), Some(12_345.67));
        assert_eq!(parse_labeled_integer(&text, &["当前做种"]), Some(88));
    }

    #[test]
    fn parses_english_and_traditional_transfer_labels() {
        let text = "Transfers Uploaded: 2.25 TB Downloaded: 512.5 GB 分享率: 4.49";
        assert_eq!(
            parse_labeled_size(text, &["Uploaded"]),
            Some(2_473_901_162_496)
        );
        assert_eq!(
            parse_labeled_size(text, &["Downloaded"]),
            Some(550_292_684_800)
        );

        let traditional = "傳送 上傳量 10 GiB 下載量 2 GiB";
        assert_eq!(
            parse_labeled_size(traditional, &["上傳量"]),
            Some(10_737_418_240)
        );
        assert_eq!(
            parse_labeled_size(traditional, &["下載量"]),
            Some(2_147_483_648)
        );
    }

    #[test]
    fn peer_counts_do_not_read_bonus_rates_or_other_seeding_metrics() {
        for text in [
            "Seeding Bonus Per Hour: 64.892",
            "Seeding Points: 1234",
            "Seeding Time: 3600",
            "Seeding Size: 64 GiB",
            "当前做种 时魔: 64.892",
            "Seeding: 64.892",
        ] {
            assert_eq!(
                parse_labeled_integer(text, &["当前做种", "Seeding"]),
                None,
                "{text}"
            );
        }
        assert_eq!(
            parse_labeled_integer(
                "Seeding Bonus Per Hour: 64.892 Seeding: 69 Leeching: 1",
                &["Seeding"]
            ),
            Some(69)
        );
        assert_eq!(
            parse_labeled_integer("当前做种：1,234", &["当前做种"]),
            Some(1234)
        );
        assert_eq!(parse_labeled_integer("Seeding: 0", &["Seeding"]), Some(0));
    }

    #[test]
    fn parses_nexusphp_seeding_ajax_summaries_and_tables() {
        assert_eq!(
            parse_seeding_ajax_count("<div><b>1,234</b> 条记录</div>"),
            Some(1234)
        );
        assert_eq!(
            parse_seeding_ajax_count(
                "<div>56 | 1.2 TiB</div><table><tr><th>标题</th></tr></table>"
            ),
            Some(56)
        );
        assert_eq!(parse_seeding_ajax_count("<div>No records.</div>"), Some(0));
        assert_eq!(
            parse_seeding_ajax_count(
                "<table><tr><th>标题</th></tr><tr><td>A</td></tr><tr><td>B</td></tr></table>"
            ),
            Some(2)
        );
        assert_eq!(
            parse_user_torrent_ajax_summary(
                "<div>56 | 1.2 TiB</div><table><tr><th>标题</th></tr></table>"
            ),
            Some((56, Some(1_319_413_953_331)))
        );
        assert_eq!(
            parse_user_torrent_ajax_summary(
                "<table><tr><th>标题</th><th>大小</th></tr><tr><td>A</td><td>1 GiB</td></tr><tr><td>B</td><td>512 MiB</td></tr></table>"
            ),
            Some((2, Some(1_610_612_736)))
        );
    }

    #[test]
    fn profile_numbers_ignore_navigation_and_preserve_precise_ucoin() {
        let html = r#"<nav>魔力 100 邀请 4</nav><table>
          <tr><td class="rowhead">U币</td><td>102,825.1</td></tr>
          <tr><td class="rowhead">这是你的做种积分，多多做种，多多积分！</td><td>2,439.1 (2026-09-04)</td></tr>
          <tr><td class="rowhead">UCoin[详情]</td><td><span title="122,217.24">122217</span></td></tr>
        </table>"#;
        assert_eq!(
            super::parse_profile_number(html, &["魔力", "U币"]),
            Some(102825.1)
        );
        assert_eq!(
            super::parse_profile_number(html, &["做种积分"]),
            Some(2439.1)
        );
        assert_eq!(
            super::parse_profile_number(html, &["UCoin"]),
            Some(122217.24)
        );
        assert_eq!(super::parse_profile_number(html, &["蝌蚪"]), None);
        let hhan = "<span>憨豆：</span><div>23886.7</div><span>加入日期：</span><span>2026-08-20 10:16:35</span>";
        assert_eq!(super::parse_profile_number(hhan, &["憨豆"]), Some(23886.7));
        assert_eq!(
            super::parse_profile_value(hhan, &["加入日期"]).as_deref(),
            Some("2026-08-20 10:16:35")
        );
    }

    #[test]
    fn empty_profile_icon_does_not_hide_the_account_name() {
        let html = r#"<div id="info_block"><a href="userdetails.php?id=42"><img src="avatar.png"></a><a class="PowerUser_Name" href="userdetails.php?id=42">alice</a><a href="userdetails.php?id=99">bob</a></div>"#;
        let current = super::extract_current_user(html).unwrap();
        assert_eq!(current.uid, "42");
        assert_eq!(current.username.as_deref(), Some("alice"));
    }

    #[test]
    fn notices_and_neighboring_limits_are_not_message_or_hnr_counts() {
        let html = r#"<div id="info_block"><a href="myhr.php">5/0/20</a> 魔力 0/1000</div>
          <table><tr><td style="background: red"><a href="messages.php">新手考核 时间 2026-09-04 指标 500</a></td></tr></table>
          <a href="messages.php">站点公告 2025-09-01</a><img alt="Donor" src="legend.gif">"#;
        assert_eq!(parse_hnr_counts(html), (Some(5), Some(0)));
        assert_eq!(parse_message_count(html), Some(0));
        assert!(!detect_donor(html));
    }

    #[test]
    fn seeding_summary_includes_total_size_with_record_labels() {
        assert_eq!(
            super::parse_user_torrent_ajax_summary(
                "<div>13 条记录 | 总大小：379.45 GB | 官种数量: 3</div>"
            ),
            Some((13, Some((379.45 * 1073741824.0) as u64)))
        );
        assert_eq!(
            super::parse_user_torrent_ajax_summary("<b>9</b>条记录 大小 119.091 GiB"),
            Some((9, Some((119.091 * 1073741824.0) as u64)))
        );
    }

    #[test]
    fn hourly_bonus_sums_spendable_rewards_without_seeding_points() {
        let html = "<div><div>对于非官奖励，你当前每小时能获取20.565个U币</div><div>对于官种奖励，你当前每小时能获取17.755个U币</div><div>对于做种积分，你当前每小时能获取18.598</div></div>";
        let rates = super::parse_bonus_rates(html, false);
        assert!((rates.0.unwrap() - 38.32).abs() < 0.00001);
        assert_eq!(rates.1, None);
        let nested = "<div><table><tr><td><div>你當前每小時能獲取3.822個魔力值; 如果你有捐贈標誌，你每小時將能獲取7.644個魔力值</div></td></tr></table></div>";
        assert_eq!(super::parse_bonus_rates(nested, false), (Some(3.822), None));
        assert_eq!(
            super::parse_bonus_rates("最近24小时获得种子UCoin1296，计算次数82", true),
            (Some(54.0), None)
        );
        assert_eq!(
            super::parse_bonus_rates(
                r#"<div id="outer"><table><tr><td rowspan="2">3.516</td></tr></table><div>你当前每小时能获取0个魔力值</div></div>"#,
                false
            ),
            (Some(3.516), None)
        );
    }

    #[test]
    fn parses_extended_pt_depiler_nexusphp_fields() {
        let html = r#"
            <html><body>
              <div id="info_block">
                <table><tr><td style="background: red"><a href="messages.php">消息 (3)</a></td></tr></table>
                <a href="myhr.php">H&amp;R: 2/1/5</a>
              </div>
              <h1><img src="pic/flag/donor.gif" alt="Donor"></h1>
              <img class="avatar" src="avatars/42.png">
              <table>
                <tr><td class="rowhead">等级</td><td><img title="Elite User"></td></tr>
                <tr><td class="rowhead">加入日期</td><td>2024-01-02 03:04:05 (100 weeks ago)</td></tr>
                <tr><td class="rowhead">最近动向</td><td>2026-09-05 12:00:00 (now)</td></tr>
              </table>
            </body></html>
        "#;
        assert_eq!(
            parse_table_labeled_value(html, &["等级"]).as_deref(),
            Some("Elite User")
        );
        assert_eq!(parse_message_count(html), Some(3));
        assert_eq!(parse_hnr_counts(html), (Some(2), Some(1)));
        assert_eq!(
            parse_hnr_counts(
                r#"<div id="info_block"><a href="myhr.php">电影区 2/1/5 剧集区 3/4/10</a></div>"#
            ),
            (Some(5), Some(5))
        );
        assert!(detect_donor(html));
        assert_eq!(extract_avatar(html).as_deref(), Some("avatars/42.png"));
        assert_eq!(
            parse_user_datetime_millis("2026-09-05 12:00:00 (now)"),
            Some(1_788_580_800_000)
        );
        assert_eq!(
            parse_labeled_duration_seconds("平均做种时间：2.5 days", &["平均做种时间"]),
            Some(216_000)
        );
    }
}
