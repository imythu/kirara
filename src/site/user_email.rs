use reqwest::header::{COOKIE, HeaderMap, HeaderValue};
use reqwest::{Client, Url};
use scraper::{Html, Selector};

use super::default_site_request_headers;
use super::nexusphp::{
    extract_user_id_from_href, extract_username, extract_visible_text, parse_labeled_integer,
    parse_labeled_size, parse_profile_value, parse_user_datetime_millis, ratio_from_totals,
};
use super::site_request_header_map;

/// 用户资料查询失败类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserProfileFailureKind {
    /// 网络、HTTP 状态或读取响应失败。
    RequestFailed,
    /// 被 Cloudflare、登录页等拦截。
    Intercepted,
    /// URL/UID 非法，或响应无法按规则解析。
    ParseFailed,
}

impl UserProfileFailureKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::RequestFailed => "request_failed",
            Self::Intercepted => "intercepted",
            Self::ParseFailed => "parse_failed",
        }
    }

    pub fn label_zh(&self) -> &'static str {
        match self {
            Self::RequestFailed => "请求失败",
            Self::Intercepted => "请求被拦截",
            Self::ParseFailed => "解析失败",
        }
    }
}

/// 从用户资料页解析出的公开信息。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UserProfileInfo {
    pub uid: Option<String>,
    pub username: Option<String>,
    pub email: Option<String>,
    pub uploaded: Option<u64>,
    pub downloaded: Option<u64>,
    /// 本地按 uploaded / downloaded 计算，不直接采信页面分享率。
    pub ratio: Option<f64>,
    /// 入站时间（Unix 毫秒）。
    pub join_time: Option<i64>,
    /// 当前做种数。
    pub seeding_count: Option<u32>,
    /// 当前做种体积（字节）。
    pub seeding_size: Option<u64>,
}

/// 用户资料查询结果：`profile` 尽力填充；失败时 `failure` 有值。
#[derive(Debug, Clone, PartialEq)]
pub struct UserProfileLookup {
    pub profile: UserProfileInfo,
    pub failure: Option<UserProfileFailureKind>,
    pub message: String,
}

impl UserProfileLookup {
    pub fn ok(profile: UserProfileInfo) -> Self {
        Self {
            profile,
            failure: None,
            message: String::new(),
        }
    }

    pub fn failed(kind: UserProfileFailureKind, message: impl Into<String>) -> Self {
        Self {
            profile: UserProfileInfo::default(),
            failure: Some(kind),
            message: message.into(),
        }
    }

    pub fn is_ok(&self) -> bool {
        self.failure.is_none()
    }

    pub fn email(&self) -> &str {
        self.profile.email.as_deref().unwrap_or_default()
    }
}

/// 低层公共方法：对完整用户资料 URL 携带 Cookie/浏览器头请求，并解析用户信息。
///
/// 成功且页面未公开某字段时对应字段为 `None`，`failure` 为 `None`。
pub async fn fetch_user_profile_from_url(
    client: &Client,
    url: &str,
    cookie: &str,
    request_headers: Option<&HeaderMap>,
) -> UserProfileLookup {
    let parsed = match Url::parse(url) {
        Ok(parsed) => parsed,
        Err(error) => {
            return UserProfileLookup::failed(
                UserProfileFailureKind::ParseFailed,
                format!("URL 无效: {error}"),
            );
        }
    };
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return UserProfileLookup::failed(
            UserProfileFailureKind::ParseFailed,
            "URL 必须使用 http/https",
        );
    }
    let requested_uid = parsed
        .query_pairs()
        .find(|(key, _)| key == "id")
        .map(|(_, value)| value.into_owned());

    let mut headers = match site_request_header_map(default_site_request_headers().as_slice()) {
        Ok(headers) => headers,
        Err(error) => {
            return UserProfileLookup::failed(
                UserProfileFailureKind::ParseFailed,
                format!("默认请求头无效: {error}"),
            );
        }
    };
    if let Some(extra) = request_headers {
        for (name, value) in extra.iter() {
            headers.insert(name.clone(), value.clone());
        }
    }
    let cookie = cookie.trim();
    if !cookie.is_empty() {
        match HeaderValue::from_str(cookie) {
            Ok(value) => {
                headers.insert(COOKIE, value);
            }
            Err(_) => {
                return UserProfileLookup::failed(
                    UserProfileFailureKind::ParseFailed,
                    "Cookie 格式无效，请检查是否包含换行等非法字符",
                );
            }
        }
    }

    let response = match client.get(parsed).headers(headers).send().await {
        Ok(response) => response,
        Err(error) => {
            return UserProfileLookup::failed(
                UserProfileFailureKind::RequestFailed,
                format!("请求失败: {}", describe_reqwest_error(error)),
            );
        }
    };

    let status = response.status();
    let final_url = response.url().clone();
    let body = match response.text().await {
        Ok(body) => body,
        Err(error) => {
            return UserProfileLookup::failed(
                UserProfileFailureKind::RequestFailed,
                format!("读取响应失败: {}", describe_reqwest_error(error)),
            );
        }
    };

    if looks_like_cloudflare_challenge(&body) {
        return UserProfileLookup::failed(
            UserProfileFailureKind::Intercepted,
            "请求被 Cloudflare 验证页拦截",
        );
    }
    if looks_like_login_page(&body, &final_url) {
        return UserProfileLookup::failed(
            UserProfileFailureKind::Intercepted,
            "Cookie 无效或已过期，站点返回了登录页",
        );
    }
    if !status.is_success() {
        return UserProfileLookup::failed(
            UserProfileFailureKind::RequestFailed,
            format!("HTTP {status}"),
        );
    }
    if body.trim().is_empty() {
        return UserProfileLookup::failed(
            UserProfileFailureKind::ParseFailed,
            "响应正文为空，无法解析用户信息",
        );
    }

    UserProfileLookup::ok(parse_user_profile_html(
        &body,
        requested_uid.as_deref(),
    ))
}

/// NexusPHP：站点地址 + Cookie + UID → `/userdetails.php?id={uid}`
pub async fn fetch_nexusphp_user_profile(
    client: &Client,
    site_url: &str,
    cookie: &str,
    user_id: &str,
    request_headers: Option<&HeaderMap>,
) -> UserProfileLookup {
    let Some(url) = join_user_url(site_url, "userdetails.php", user_id) else {
        return UserProfileLookup::failed(
            UserProfileFailureKind::ParseFailed,
            "UID 无效，必须为纯数字",
        );
    };
    fetch_user_profile_from_url(client, &url, cookie, request_headers).await
}

/// Gazelle：站点地址 + Cookie + UID → `/user.php?id={uid}`
pub async fn fetch_gazelle_user_profile(
    client: &Client,
    site_url: &str,
    cookie: &str,
    user_id: &str,
    request_headers: Option<&HeaderMap>,
) -> UserProfileLookup {
    let Some(url) = join_user_url(site_url, "user.php", user_id) else {
        return UserProfileLookup::failed(
            UserProfileFailureKind::ParseFailed,
            "UID 无效，必须为纯数字",
        );
    };
    fetch_user_profile_from_url(client, &url, cookie, request_headers).await
}

fn join_user_url(site_url: &str, path: &str, user_id: &str) -> Option<String> {
    let uid = user_id.trim();
    if uid.is_empty() || !uid.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    let base = site_url.trim().trim_end_matches('/');
    if base.is_empty() {
        return None;
    }
    Some(format!("{base}/{path}?id={uid}"))
}

/// 解析用户资料页 HTML：用户名、UID、邮箱、上传/下载、入站时间、做种，分享率本地计算。
pub(crate) fn parse_user_profile_html(html: &str, requested_uid: Option<&str>) -> UserProfileInfo {
    let text = extract_visible_text(html);
    let uploaded = parse_labeled_size(&text, &["上传量", "上傳量", "Uploaded"]);
    let downloaded = parse_labeled_size(&text, &["下载量", "下載量", "Downloaded"]);
    let ratio = ratio_from_totals(uploaded, downloaded);
    let seeding_count = parse_labeled_integer(
        &text,
        &["当前做种", "當前做種", "做种数", "做種數", "正在做种", "Seeding"],
    );
    let seeding_size = parse_labeled_size(
        &text,
        &["做种体积", "做種體積", "做种量", "做種量", "Seeding Size", "Seeding size"],
    );

    let uid = requested_uid
        .map(str::trim)
        .filter(|uid| !uid.is_empty() && uid.chars().all(|ch| ch.is_ascii_digit()))
        .map(str::to_string)
        .or_else(|| extract_uid_from_page(html));

    let username = extract_username(html).or_else(|| extract_username_from_title(html));

    let join_time = parse_profile_value(
        html,
        &["加入日期", "加入時間", "Join date", "Joined", "注册时间", "註冊時間"],
    )
    .as_deref()
    .and_then(parse_user_datetime_millis);

    UserProfileInfo {
        uid,
        username,
        email: extract_user_email(html),
        uploaded,
        downloaded,
        ratio,
        join_time,
        seeding_count,
        seeding_size,
    }
}

fn extract_uid_from_page(html: &str) -> Option<String> {
    let document = Html::parse_document(html);
    let selectors = [
        "a[href*='userdetails.php'][href*='id=']",
        "a[href*='user.php'][href*='id=']",
    ];
    for selector in selectors {
        let Ok(selector) = Selector::parse(selector) else {
            continue;
        };
        for element in document.select(&selector) {
            if let Some(uid) = element
                .value()
                .attr("href")
                .and_then(extract_user_id_from_href)
            {
                return Some(uid);
            }
        }
    }
    None
}

fn extract_username_from_title(html: &str) -> Option<String> {
    let document = Html::parse_document(html);
    let selector = Selector::parse("title").ok()?;
    let title = document
        .select(&selector)
        .next()
        .and_then(|element| element.text().next())
        .map(str::trim)
        .filter(|value| !value.is_empty())?;

    // 常见格式：「站点名 :: 用户详情 - username - Powered by NexusPHP」
    // 不要取最后一段，避免把 Powered by … 当成用户名。
    const NOISE: &[&str] = &[
        "details",
        "user details",
        "用户详情",
        "用戶詳情",
        "用户资料",
        "user profile",
        "profile",
        "powered by nexusphp",
        "powered by gazelle",
        "powered by unit3d",
        "powered by",
    ];
    let is_noise = |value: &str| {
        let lower = value.trim().to_ascii_lowercase();
        lower.is_empty()
            || NOISE.iter().any(|noise| lower == *noise || lower.starts_with(noise))
    };

    for separator in [" - ", " – ", " — "] {
        let parts: Vec<&str> = title.split(separator).collect();
        // 从后往前找第一个像用户名的段
        for part in parts.iter().rev() {
            let name = part.trim();
            if is_noise(name) {
                continue;
            }
            // 站点名常见含空格/中文标题描述，用户名一般不含连续多个词
            if name.contains("::") || name.contains(" - ") {
                continue;
            }
            return Some(name.to_string());
        }
    }
    // 「username :: 站点」或整段 title
    if let Some((head, _)) = title.split_once("::") {
        let name = head.trim();
        if !is_noise(name) && !name.contains(' ') {
            return Some(name.to_string());
        }
    }
    None
}

/// 从用户详情页提取公开邮箱。优先 `mailto:`，其次 Cloudflare `data-cfemail`。
pub(crate) fn extract_user_email(html: &str) -> Option<String> {
    let document = Html::parse_document(html);
    if let Ok(selector) = Selector::parse("a[href]") {
        for element in document.select(&selector) {
            if let Some(email) = element
                .value()
                .attr("href")
                .and_then(email_from_mailto_href)
            {
                return Some(email);
            }
        }
    }
    if let Ok(selector) = Selector::parse("[data-cfemail]") {
        for element in document.select(&selector) {
            if let Some(email) = element
                .value()
                .attr("data-cfemail")
                .and_then(cf_decode_email)
            {
                return Some(email);
            }
        }
    }
    None
}

pub(crate) fn email_from_mailto_href(href: &str) -> Option<String> {
    let trimmed = href.trim();
    if trimmed.len() < 7 || !trimmed[..7].eq_ignore_ascii_case("mailto:") {
        return None;
    }
    let rest = &trimmed[7..];
    let email = rest.split(['?', '#']).next().unwrap_or("").trim();
    if email.is_empty() {
        return None;
    }
    let decoded = urlencoding::decode(email)
        .map(|value| value.into_owned())
        .unwrap_or_else(|_| email.to_string());
    let decoded = decoded.trim();
    decoded.contains('@').then(|| decoded.to_string())
}

pub(crate) fn cf_decode_email(encoded: &str) -> Option<String> {
    let encoded = encoded.trim();
    if encoded.len() < 4
        || encoded.len() % 2 != 0
        || !encoded.chars().all(|ch| ch.is_ascii_hexdigit())
    {
        return None;
    }
    let key = u8::from_str_radix(&encoded[..2], 16).ok()?;
    let mut bytes = Vec::with_capacity((encoded.len() - 2) / 2);
    let mut index = 2;
    while index < encoded.len() {
        let byte = u8::from_str_radix(&encoded[index..index + 2], 16).ok()? ^ key;
        bytes.push(byte);
        index += 2;
    }
    let email = String::from_utf8(bytes).ok()?;
    email.contains('@').then_some(email)
}

fn describe_reqwest_error(error: reqwest::Error) -> String {
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
    format!("{category}: {error}")
}

fn looks_like_cloudflare_challenge(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    lower.contains("cf-chl-")
        || lower.contains("challenge-platform")
        || lower.contains("just a moment...")
        || lower.contains("cloudflare ray id")
}

fn looks_like_login_page(html: &str, final_url: &Url) -> bool {
    if final_url.path().to_ascii_lowercase().contains("login.php") {
        return true;
    }
    let lower = html.to_ascii_lowercase();
    let has_password = lower.contains("type=\"password\"") || lower.contains("type='password'");
    let has_login_action = lower.contains("action=\"login.php")
        || lower.contains("action='login.php")
        || lower.contains("action=\"takelogin.php")
        || lower.contains("action='takelogin.php")
        || lower.contains("id=\"form-login\"")
        || lower.contains("id='form-login'")
        || lower.contains("name=\"login\"")
        || lower.contains("name='login'");
    has_password && has_login_action
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_email_from_mailto_and_cloudflare() {
        let mailto = r#"
            <html><body>
              <tr><td>邮箱</td>
              <td><a href="mailto:2275974893@qq.com">2275974893@qq.com</a></td></tr>
            </body></html>
        "#;
        assert_eq!(
            extract_user_email(mailto).as_deref(),
            Some("2275974893@qq.com")
        );

        let with_query = r#"<a href="mailto:user%40example.com?subject=hi">x</a>"#;
        assert_eq!(
            extract_user_email(with_query).as_deref(),
            Some("user@example.com")
        );

        let upper = r#"<a href="MAILTO:Admin@Example.COM">Admin</a>"#;
        assert_eq!(
            extract_user_email(upper).as_deref(),
            Some("Admin@Example.COM")
        );

        let encoded = "2b5f4e585f6b4e534a465b474e05484446";
        let cf = format!(
            r#"<html><body><a href="/cdn-cgi/l/email-protection#{encoded}" class="__cf_email__" data-cfemail="{encoded}">[email protected]</a></body></html>"#
        );
        assert_eq!(extract_user_email(&cf).as_deref(), Some("test@example.com"));
        assert_eq!(cf_decode_email(encoded).as_deref(), Some("test@example.com"));
    }

    #[test]
    fn absent_email_returns_none() {
        assert_eq!(
            extract_user_email("<html><body>no email row</body></html>"),
            None
        );
        assert_eq!(extract_user_email(r#"<a href="mailto:"></a>"#), None);
        assert_eq!(cf_decode_email("zz"), None);
        assert_eq!(cf_decode_email("2b"), None);
    }

    #[test]
    fn join_user_url_validates_uid() {
        assert_eq!(
            join_user_url("https://pt.example/", "user.php", "33037").as_deref(),
            Some("https://pt.example/user.php?id=33037")
        );
        assert_eq!(join_user_url("https://pt.example", "user.php", "abc"), None);
        assert_eq!(join_user_url("", "user.php", "1"), None);
    }

    #[test]
    fn failure_kind_labels() {
        assert_eq!(
            UserProfileFailureKind::RequestFailed.as_str(),
            "request_failed"
        );
        assert_eq!(UserProfileFailureKind::Intercepted.as_str(), "intercepted");
        assert_eq!(UserProfileFailureKind::ParseFailed.as_str(), "parse_failed");
        assert_eq!(UserProfileFailureKind::RequestFailed.label_zh(), "请求失败");
        assert_eq!(UserProfileFailureKind::Intercepted.label_zh(), "请求被拦截");
        assert_eq!(UserProfileFailureKind::ParseFailed.label_zh(), "解析失败");
    }

    #[test]
    fn parses_profile_fields_and_computes_ratio_locally() {
        let html = r#"
            <html>
              <head><title>青蛙 :: 用户详情 - sun2008050</title></head>
              <body>
                <div id="info_block">
                  <a class="User_Name" href="/userdetails.php?id=708227"><b>sun2008050</b></a>
                </div>
                <table>
                  <tr><td>用户ID</td><td>708227</td></tr>
                  <tr><td>上传量</td><td>4 GiB</td></tr>
                  <tr><td>下载量</td><td>2 GiB</td></tr>
                  <tr><td>分享率</td><td>99.0</td></tr>
                  <tr><td>当前做种</td><td>12</td></tr>
                  <tr><td>做种体积</td><td>3.5 TiB</td></tr>
                  <tr><td>加入日期</td><td>2024-01-02 03:04:05 (100 weeks ago)</td></tr>
                  <tr><td>邮箱</td><td><a href="mailto:2275974893@qq.com">2275974893@qq.com</a></td></tr>
                </table>
              </body>
            </html>
        "#;
        let profile = parse_user_profile_html(html, Some("708227"));
        assert_eq!(profile.uid.as_deref(), Some("708227"));
        assert_eq!(profile.username.as_deref(), Some("sun2008050"));
        assert_eq!(profile.email.as_deref(), Some("2275974893@qq.com"));
        assert_eq!(profile.uploaded, Some(4 * 1024 * 1024 * 1024));
        assert_eq!(profile.downloaded, Some(2 * 1024 * 1024 * 1024));
        // 本地计算：4/2 = 2.0，忽略页面上的 99.0
        assert_eq!(profile.ratio, Some(2.0));
        assert!(profile.join_time.is_some());
        assert_eq!(profile.seeding_count, Some(12));
        assert_eq!(
            profile.seeding_size,
            Some((3.5 * 1024.0 * 1024.0 * 1024.0 * 1024.0) as u64)
        );
    }

    #[test]
    fn title_username_skips_powered_by_footer() {
        let html = r#"
            <html>
              <head><title>青蛙 :: 用户详情 - sun2008050 - Powered by NexusPHP</title></head>
              <body>
                <a href="/userdetails.php?id=708227">sun2008050</a>
              </body>
            </html>
        "#;
        let profile = parse_user_profile_html(html, Some("708227"));
        assert_eq!(profile.username.as_deref(), Some("sun2008050"));
    }
}
