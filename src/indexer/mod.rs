pub(crate) mod access;
pub mod mteam;
pub mod nexusphp;
pub mod pool;

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use reqwest::header::RETRY_AFTER;
use reqwest::{Response, StatusCode, Url};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize, Serializer};
use thiserror::Error;

use crate::site::{
    SiteAuth, SiteRecord, SiteType, parse_site_request_headers, site_request_header_map,
};

#[allow(unused_imports)]
pub use pool::{AggregateSearchResult, IndexerAggregator, IndexerPool, SiteSearchError};

const DEFAULT_PAGE: u32 = 1;
const DEFAULT_PAGE_SIZE: u32 = 30;
const MAX_PAGE_SIZE: u32 = 100;
const MAX_QUERY_LEN: usize = 512;
const MAX_TORRENT_BYTES: usize = 64 * 1024 * 1024;

fn default_page() -> u32 {
    DEFAULT_PAGE
}

fn default_page_size() -> u32 {
    DEFAULT_PAGE_SIZE
}

/// A normalized query sent to an indexer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchRequest {
    pub query: String,
    #[serde(default = "default_page")]
    pub page: u32,
    #[serde(default = "default_page_size")]
    pub page_size: u32,
}

impl SearchRequest {
    #[allow(dead_code)]
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            page: DEFAULT_PAGE,
            page_size: DEFAULT_PAGE_SIZE,
        }
    }

    pub fn normalized(&self) -> Result<Self, IndexerError> {
        let query = self.query.trim();
        if query.is_empty() {
            return Err(IndexerError::Configuration(
                "search query cannot be empty".to_string(),
            ));
        }
        if query.len() > MAX_QUERY_LEN {
            return Err(IndexerError::Configuration(format!(
                "search query exceeds {MAX_QUERY_LEN} bytes"
            )));
        }

        Ok(Self {
            query: query.to_string(),
            page: self.page.max(1),
            page_size: self.page_size.clamp(1, MAX_PAGE_SIZE),
        })
    }
}

/// Credential-free result shared by manual search, subscriptions and the download outbox.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SearchResult {
    pub site_id: i64,
    pub source_site: String,
    pub torrent_id: String,
    pub title: String,
    pub detail_url: Option<String>,
    pub download_locator: Option<String>,
    pub magnet: Option<String>,
    pub size: u64,
    pub seeders: u32,
    pub leechers: u32,
    pub publish_time: Option<DateTime<Utc>>,
}

// Search results may be persisted in the download outbox. Serialize defensively so a caller
// cannot accidentally persist a signed download URL supplied by a tracker.
impl Serialize for SearchResult {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let detail_url = self
            .detail_url
            .as_deref()
            .and_then(sanitize_public_url_unscoped);
        let download_locator = self
            .download_locator
            .as_ref()
            .map(|_| self.torrent_id.clone());
        let magnet = self.magnet.as_deref().and_then(sanitize_magnet);

        let mut state = serializer.serialize_struct("SearchResult", 11)?;
        state.serialize_field("site_id", &self.site_id)?;
        state.serialize_field("source_site", &self.source_site)?;
        state.serialize_field("torrent_id", &self.torrent_id)?;
        state.serialize_field("title", &self.title)?;
        state.serialize_field("detail_url", &detail_url)?;
        state.serialize_field("download_locator", &download_locator)?;
        state.serialize_field("magnet", &magnet)?;
        state.serialize_field("size", &self.size)?;
        state.serialize_field("seeders", &self.seeders)?;
        state.serialize_field("leechers", &self.leechers)?;
        state.serialize_field("publish_time", &self.publish_time)?;
        state.end()
    }
}

impl SearchResult {
    pub(crate) fn sanitized_for_base(mut self, base_url: &Url) -> Result<Self, IndexerError> {
        self.torrent_id = normalize_torrent_id(&self.torrent_id)?;
        self.source_site = self.source_site.trim().to_string();
        self.title = self.title.trim().to_string();
        if self.title.is_empty() {
            return Err(IndexerError::Parse("torrent title is empty".to_string()));
        }

        self.detail_url = self
            .detail_url
            .as_deref()
            .and_then(|raw| resolve_same_origin_url(base_url, raw).ok())
            .map(sanitize_public_url)
            .transpose()?
            .map(|url| url.to_string());
        self.download_locator = self
            .download_locator
            .as_ref()
            .map(|_| self.torrent_id.clone());
        self.magnet = self.magnet.as_deref().and_then(sanitize_magnet);
        Ok(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct IndexerCapabilities {
    pub search: bool,
    pub fetch_torrent: bool,
    pub api_search: bool,
    pub html_search: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum IndexerError {
    #[error("configuration error: {0}")]
    Configuration(String),
    #[error("authentication expired: {0}")]
    AuthenticationExpired(String),
    #[error("HTTP error: {0}")]
    Http(String),
    #[error("tracker API error: {0}")]
    Api(String),
    #[error("tracker rate limited the request{0}")]
    RateLimited(IndexerRateLimit),
    #[error("response parse error: {0}")]
    Parse(String),
    #[error("unsafe URL: {0}")]
    UnsafeUrl(String),
    #[error("invalid torrent response: {0}")]
    InvalidTorrent(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexerRateLimit {
    retry_after_secs: Option<u64>,
}

impl IndexerRateLimit {
    pub(crate) const fn new(retry_after_secs: Option<u64>) -> Self {
        Self { retry_after_secs }
    }

    pub(crate) const fn retry_after_secs(self) -> Option<u64> {
        self.retry_after_secs
    }
}

impl fmt::Display for IndexerRateLimit {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.retry_after_secs {
            Some(seconds) => write!(formatter, "; retry after {seconds} seconds"),
            None => Ok(()),
        }
    }
}

impl IndexerError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Configuration(_) => "configuration_error",
            Self::AuthenticationExpired(_) => "authentication_expired",
            Self::Http(_) => "http_error",
            Self::Api(_) => "api_error",
            Self::RateLimited(_) => "rate_limited",
            Self::Parse(_) => "parse_error",
            Self::UnsafeUrl(_) => "unsafe_url",
            Self::InvalidTorrent(_) => "invalid_torrent",
        }
    }
}

pub type IndexerFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, IndexerError>> + Send + 'a>>;

pub trait IndexerAdapter: Send + Sync {
    fn site_id(&self) -> i64;
    fn site_name(&self) -> &str;
    #[allow(dead_code)]
    fn capabilities(&self) -> IndexerCapabilities;

    fn access_key(&self) -> Option<&str> {
        None
    }

    fn search<'a>(&'a self, request: &'a SearchRequest) -> IndexerFuture<'a, Vec<SearchResult>>;

    fn fetch_torrent<'a>(&'a self, result: &'a SearchResult) -> IndexerFuture<'a, Vec<u8>>;
}

/// Construct the inner adapter used by `IndexerPool` with its shared origin gate.
pub(crate) fn create_indexer(
    record: &SiteRecord,
    client: reqwest::Client,
    access_gate: Arc<access::OriginAccessGate>,
) -> Result<Arc<dyn IndexerAdapter>, IndexerError> {
    let site_type = SiteType::from_str(record.site_type.trim()).ok_or_else(|| {
        IndexerError::Configuration(format!("unsupported site type: {}", record.site_type))
    })?;
    let auth: SiteAuth = serde_json::from_str(&record.auth_config).map_err(|error| {
        IndexerError::Configuration(format!("invalid site authentication config: {error}"))
    })?;
    let request_headers = parse_site_request_headers(&record.request_headers)
        .and_then(|headers| site_request_header_map(&headers))
        .map_err(IndexerError::Configuration)?;

    match site_type {
        SiteType::Gazelle => Err(IndexerError::Configuration(
            "Gazelle 当前仅支持用户统计，尚未支持种子搜索".to_string(),
        )),
        SiteType::NexusPhp => Ok(Arc::new(nexusphp::NexusPhpIndexer::new(
            record.id,
            record.name.clone(),
            &record.base_url,
            auth,
            request_headers,
            client,
            Arc::clone(&access_gate),
        )?)),
        SiteType::MTeam => Ok(Arc::new(mteam::MTeamIndexer::new(
            record.id,
            record.name.clone(),
            &record.base_url,
            auth,
            request_headers,
            client,
            access_gate,
        )?)),
    }
}

/// Parse and normalize a base URL. Credentials, query strings and fragments are rejected.
pub fn normalize_base_url(raw: &str) -> Result<Url, IndexerError> {
    let mut url = Url::parse(raw.trim())
        .map_err(|_| IndexerError::Configuration("site base URL is invalid".to_string()))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(IndexerError::Configuration(
            "site base URL must use HTTP(S) and include a host".to_string(),
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(IndexerError::Configuration(
            "site base URL must not contain credentials".to_string(),
        ));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(IndexerError::Configuration(
            "site base URL must not contain a query or fragment".to_string(),
        ));
    }

    let normalized_path = url.path().trim_end_matches('/').to_string();
    url.set_path(if normalized_path.is_empty() {
        "/"
    } else {
        &normalized_path
    });
    Ok(url)
}

/// Resolve a candidate and reject scheme, host or port changes.
#[allow(dead_code)]
pub fn validate_same_origin_url(base_url: &str, candidate: &str) -> Result<String, IndexerError> {
    let base = normalize_base_url(base_url)?;
    resolve_same_origin_url(&base, candidate).map(|url| url.to_string())
}

pub(crate) fn endpoint_url(base: &Url, path: &str) -> Result<Url, IndexerError> {
    let mut url = base.clone();
    let base_path = base.path().trim_end_matches('/');
    let suffix = path.trim_start_matches('/');
    let full_path = if base_path.is_empty() {
        format!("/{suffix}")
    } else {
        format!("{base_path}/{suffix}")
    };
    url.set_path(&full_path);
    url.set_query(None);
    url.set_fragment(None);
    Ok(url)
}

pub(crate) fn resolve_same_origin_url(
    base_url: &Url,
    candidate: &str,
) -> Result<Url, IndexerError> {
    let candidate = candidate.trim().trim_start_matches("##");
    if candidate.is_empty() {
        return Err(IndexerError::UnsafeUrl("URL is empty".to_string()));
    }
    let url = match Url::parse(candidate) {
        Ok(url) => url,
        Err(_) => {
            let mut resolution_base = base_url.clone();
            let base_path = resolution_base.path().trim_end_matches('/');
            let directory_path = format!("{base_path}/");
            resolution_base.set_path(&directory_path);
            resolution_base.set_query(None);
            resolution_base.set_fragment(None);
            resolution_base
                .join(candidate)
                .map_err(|_| IndexerError::UnsafeUrl("URL is invalid".to_string()))?
        }
    };
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || !same_origin(base_url, &url)
    {
        return Err(IndexerError::UnsafeUrl(
            "URL must remain on the configured site origin".to_string(),
        ));
    }
    Ok(url)
}

pub(crate) fn same_origin(left: &Url, right: &Url) -> bool {
    left.scheme().eq_ignore_ascii_case(right.scheme())
        && left
            .host_str()
            .zip(right.host_str())
            .is_some_and(|(a, b)| a.eq_ignore_ascii_case(b))
        && left.port_or_known_default() == right.port_or_known_default()
}

pub(crate) fn sanitize_public_url(mut url: Url) -> Result<Url, IndexerError> {
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(IndexerError::UnsafeUrl(
            "public URL contains credentials or an unsupported scheme".to_string(),
        ));
    }

    let retained: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(key, _)| !is_sensitive_query_key(key))
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    url.set_query(None);
    if !retained.is_empty() {
        url.query_pairs_mut().extend_pairs(retained);
    }
    url.set_fragment(None);
    Ok(url)
}

fn sanitize_public_url_unscoped(raw: &str) -> Option<String> {
    let url = Url::parse(raw.trim()).ok()?;
    sanitize_public_url(url).ok().map(|url| url.to_string())
}

fn is_sensitive_query_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase().replace(['-', '_'], "");
    matches!(
        key.as_str(),
        "apikey"
            | "auth"
            | "authorization"
            | "cookie"
            | "downhash"
            | "jwt"
            | "key"
            | "passkey"
            | "secret"
            | "sign"
            | "signature"
            | "token"
    ) || key.contains("token")
        || key.contains("passkey")
        || key.contains("apikey")
        || key.contains("secret")
        || key.contains("signature")
        || key.contains("downhash")
}

pub(crate) fn sanitize_magnet(raw: &str) -> Option<String> {
    let url = Url::parse(raw.trim()).ok()?;
    if url.scheme() != "magnet" {
        return None;
    }
    let retained: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(key, value)| {
            matches!(key.as_ref(), "xt" | "dn" | "xl")
                && (key != "xt" || value.to_ascii_lowercase().starts_with("urn:btih:"))
        })
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    if !retained.iter().any(|(key, _)| key == "xt") {
        return None;
    }
    let mut safe = Url::parse("magnet:?").ok()?;
    safe.query_pairs_mut().extend_pairs(retained);
    Some(safe.to_string())
}

pub(crate) fn normalize_torrent_id(raw: &str) -> Result<String, IndexerError> {
    let id = raw.trim();
    if id.is_empty()
        || id.len() > 256
        || !id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | ':'))
    {
        return Err(IndexerError::Parse(
            "invalid torrent identifier".to_string(),
        ));
    }
    Ok(id.to_string())
}

pub(crate) fn http_error(error: reqwest::Error) -> IndexerError {
    IndexerError::Http(error.without_url().to_string())
}

pub(crate) fn rate_limit_error(response: &Response) -> IndexerError {
    let retry_after = response
        .headers()
        .get(RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| parse_retry_after(value, Utc::now()));
    IndexerError::RateLimited(IndexerRateLimit::new(retry_after))
}

pub(crate) fn rate_limit_error_from_json(json: &serde_json::Value) -> Option<IndexerError> {
    let code_is_rate_limited = ["code", "ret", "status"]
        .into_iter()
        .filter_map(|key| json.get(key))
        .chain(
            ["/error/code", "/error/status", "/data/code", "/data/status"]
                .into_iter()
                .filter_map(|pointer| json.pointer(pointer)),
        )
        .any(json_value_is_rate_limit_code);
    let message = json
        .get("message")
        .and_then(serde_json::Value::as_str)
        .or_else(|| json.get("msg").and_then(serde_json::Value::as_str))
        .or_else(|| json.get("error").and_then(serde_json::Value::as_str))
        .or_else(|| {
            json.pointer("/error/message")
                .and_then(serde_json::Value::as_str)
        })
        .or_else(|| {
            json.pointer("/data/message")
                .and_then(serde_json::Value::as_str)
        });
    if !code_is_rate_limited && !message.is_some_and(text_indicates_rate_limit) {
        return None;
    }

    let retry_after_secs = ["retry_after", "retryAfter", "retry_after_secs"]
        .into_iter()
        .filter_map(|key| json.get(key))
        .find_map(json_value_u64)
        .or_else(|| json.pointer("/data/retry_after").and_then(json_value_u64))
        .or_else(|| json.pointer("/data/retryAfter").and_then(json_value_u64));
    Some(IndexerError::RateLimited(IndexerRateLimit::new(
        retry_after_secs,
    )))
}

pub(crate) fn text_indicates_rate_limit(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("too many requests")
        || lower.contains("rate limit exceeded")
        || lower.contains("request rate limit")
        || lower.contains("requests are too frequent")
        || lower.contains("request is too frequent")
        || lower.contains("you are being rate limited")
        || text.contains("请求过于频繁")
        || text.contains("請求過於頻繁")
        || text.contains("请求太频繁")
        || text.contains("請求太頻繁")
}

pub(crate) fn rate_limit_error_from_body(body: &str) -> Option<IndexerError> {
    match serde_json::from_str(body) {
        Ok(json) => rate_limit_error_from_json(&json),
        Err(_) => text_indicates_rate_limit(body)
            .then(|| IndexerError::RateLimited(IndexerRateLimit::new(None))),
    }
}

pub(crate) fn parse_json_or_rate_limit(
    body: &str,
    invalid_message: &'static str,
) -> Result<serde_json::Value, IndexerError> {
    serde_json::from_str(body).map_err(|_| {
        rate_limit_error_from_body(body)
            .unwrap_or_else(|| IndexerError::Parse(invalid_message.to_string()))
    })
}

fn json_value_is_rate_limit_code(value: &serde_json::Value) -> bool {
    value.as_u64() == Some(StatusCode::TOO_MANY_REQUESTS.as_u16().into())
        || value.as_i64() == Some(StatusCode::TOO_MANY_REQUESTS.as_u16().into())
        || value
            .as_str()
            .is_some_and(|value| value.trim().parse::<u16>().ok() == Some(429))
}

fn json_value_u64(value: &serde_json::Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|value| value.trim().parse().ok()))
}

fn parse_retry_after(value: &str, now: DateTime<Utc>) -> Option<u64> {
    value.parse::<u64>().ok().or_else(|| {
        let retry_at = DateTime::parse_from_rfc2822(value)
            .ok()?
            .with_timezone(&Utc);
        let milliseconds = retry_at
            .signed_duration_since(now)
            .num_milliseconds()
            .max(0) as u64;
        Some(milliseconds.saturating_add(999) / 1_000)
    })
}

pub(crate) fn response_is_authentication_page(final_url: &Url, body: &str) -> bool {
    let path = final_url.path().to_ascii_lowercase();
    if path.contains("login") || path.contains("signin") || path.contains("verify") {
        return true;
    }

    let lower = body.to_ascii_lowercase();
    (lower.contains("login.php")
        || lower.contains("name=\"password\"")
        || lower.contains("name='password'"))
        && !lower.contains("details.php?id=")
}

pub(crate) async fn read_torrent_response(
    mut response: Response,
    base_url: &Url,
) -> Result<Vec<u8>, IndexerError> {
    let status = response.status();
    if status == StatusCode::TOO_MANY_REQUESTS {
        return Err(rate_limit_error(&response));
    }
    resolve_same_origin_url(base_url, response.url().as_str())?;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_TORRENT_BYTES as u64)
    {
        if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
            return Err(IndexerError::AuthenticationExpired(format!(
                "tracker returned HTTP {status}"
            )));
        }
        if !status.is_success() {
            return Err(IndexerError::Http(format!(
                "tracker returned HTTP {status}"
            )));
        }
        return Err(IndexerError::InvalidTorrent(
            "torrent response is too large".to_string(),
        ));
    }

    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(http_error)? {
        if body.len().saturating_add(chunk.len()) > MAX_TORRENT_BYTES {
            return Err(IndexerError::InvalidTorrent(
                "torrent response is too large".to_string(),
            ));
        }
        body.extend_from_slice(&chunk);
    }

    let preview = String::from_utf8_lossy(&body[..body.len().min(16 * 1024)]);
    let json = if body.first() != Some(&b'd') {
        serde_json::from_slice::<serde_json::Value>(&body).ok()
    } else {
        None
    };
    if let Some(error) = json.as_ref().and_then(rate_limit_error_from_json) {
        return Err(error);
    }
    if text_indicates_rate_limit(&preview) {
        return Err(IndexerError::RateLimited(IndexerRateLimit::new(None)));
    }
    if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
        return Err(IndexerError::AuthenticationExpired(format!(
            "tracker returned HTTP {status}"
        )));
    }
    if !status.is_success() {
        return Err(IndexerError::Http(format!(
            "tracker returned HTTP {status}"
        )));
    }
    if response_is_authentication_page(response.url(), &preview) {
        return Err(IndexerError::AuthenticationExpired(
            "tracker returned a login or verification page".to_string(),
        ));
    }
    if body.first() != Some(&b'd') {
        if let Some(json) = json {
            let message = json
                .get("message")
                .or_else(|| json.get("msg"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("tracker returned JSON instead of a torrent")
                .chars()
                .take(512)
                .collect();
            return Err(IndexerError::Api(message));
        }
        return Err(IndexerError::InvalidTorrent(
            "response is not a bencoded torrent dictionary".to_string(),
        ));
    }
    Ok(body)
}

pub(crate) fn ensure_result_site(
    result: &SearchResult,
    expected_site_id: i64,
) -> Result<String, IndexerError> {
    if result.site_id != expected_site_id {
        return Err(IndexerError::Configuration(
            "search result belongs to a different site".to_string(),
        ));
    }
    normalize_torrent_id(&result.torrent_id)
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::{
        IndexerError, IndexerRateLimit, SearchResult, normalize_base_url, parse_json_or_rate_limit,
        parse_retry_after, rate_limit_error_from_body, rate_limit_error_from_json,
        read_torrent_response, text_indicates_rate_limit, validate_same_origin_url,
    };

    #[test]
    fn rate_limit_metadata_preserves_retry_after_without_exposing_other_headers() {
        assert_eq!(IndexerRateLimit::new(None).to_string(), "");
        assert_eq!(
            IndexerRateLimit::new(Some(90)).to_string(),
            "; retry after 90 seconds"
        );
        assert_eq!(IndexerRateLimit::new(Some(90)).retry_after_secs(), Some(90));
    }

    #[test]
    fn retry_after_accepts_seconds_and_http_dates() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-07-29T10:00:00.500Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(parse_retry_after("90", now), Some(90));
        assert_eq!(
            parse_retry_after("Wed, 29 Jul 2026 10:01:30 GMT", now),
            Some(90)
        );
        assert_eq!(
            parse_retry_after("Wed, 29 Jul 2026 09:59:00 GMT", now),
            Some(0)
        );
        assert_eq!(parse_retry_after("invalid", now), None);
    }

    #[test]
    fn recognizes_structured_and_text_rate_limit_responses() {
        for json in [
            serde_json::json!({ "code": 429, "message": "try later" }),
            serde_json::json!({ "ret": "429", "msg": "try later" }),
            serde_json::json!({ "code": 1, "message": "請求過於頻繁" }),
            serde_json::json!({ "status": 1, "error": { "message": "Too Many Requests" } }),
            serde_json::json!({ "code": 1, "message": null, "msg": "Too Many Requests" }),
            serde_json::json!({ "error": { "code": 429, "message": "try later" } }),
        ] {
            assert!(matches!(
                rate_limit_error_from_json(&json),
                Some(super::IndexerError::RateLimited(_))
            ));
        }
        assert!(text_indicates_rate_limit(
            "Error 1015: You are being rate limited"
        ));
        assert!(
            rate_limit_error_from_json(&serde_json::json!({
                "code": 1,
                "message": "torrent is unavailable"
            }))
            .is_none()
        );
        assert!(matches!(
            parse_json_or_rate_limit("Too Many Requests", "invalid JSON"),
            Err(super::IndexerError::RateLimited(_))
        ));
        assert!(matches!(
            rate_limit_error_from_body(r#"{"error":{"code":429}}"#),
            Some(super::IndexerError::RateLimited(_))
        ));
        assert!(
            rate_limit_error_from_body(r#"{"ret":0,"data":[{"name":"Too Many Requests"}]}"#)
                .is_none()
        );
        assert!(matches!(
            parse_json_or_rate_limit("<html>temporary failure</html>", "invalid JSON"),
            Err(super::IndexerError::Parse(_))
        ));
    }

    #[tokio::test]
    async fn torrent_rate_limit_body_wins_over_forbidden_status() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request).await.unwrap();
            let body = "Too Many Requests";
            let response = format!(
                "HTTP/1.1 403 Forbidden\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        });
        let base_url = reqwest::Url::parse(&format!("http://{address}")).unwrap();
        let response = reqwest::Client::new()
            .get(base_url.clone())
            .send()
            .await
            .unwrap();

        assert!(matches!(
            read_torrent_response(response, &base_url).await,
            Err(IndexerError::RateLimited(_))
        ));
        server.await.unwrap();
    }

    #[test]
    fn same_origin_validation_rejects_host_scheme_and_port_changes() {
        let base = "https://tracker.example:443/sub";
        assert!(validate_same_origin_url(base, "/download.php?id=1").is_ok());
        assert!(
            validate_same_origin_url(base, "https://tracker.example/download.php?id=1").is_ok()
        );
        assert!(validate_same_origin_url(base, "https://evil.example/download.php?id=1").is_err());
        assert!(
            validate_same_origin_url(base, "http://tracker.example/download.php?id=1").is_err()
        );
        assert!(
            validate_same_origin_url(base, "https://tracker.example:444/download.php?id=1")
                .is_err()
        );
        assert!(validate_same_origin_url(base, "//evil.example/download.php?id=1").is_err());
    }

    #[test]
    fn relative_url_preserves_the_site_prefix_and_query() {
        assert_eq!(
            validate_same_origin_url("https://tracker.example/sub", "details.php?id=456&hit=1",)
                .unwrap(),
            "https://tracker.example/sub/details.php?id=456&hit=1"
        );
    }

    #[test]
    fn base_url_rejects_embedded_credentials() {
        assert!(normalize_base_url("https://user:password@tracker.example").is_err());
        assert!(normalize_base_url("file:///tmp/tracker").is_err());
    }

    #[test]
    fn serialized_result_removes_signed_urls_and_private_magnet_trackers() {
        let result = SearchResult {
            site_id: 1,
            source_site: "Example".to_string(),
            torrent_id: "42".to_string(),
            title: "Release".to_string(),
            detail_url: Some(
                "https://tracker.example/details.php?id=42&passkey=very-secret".to_string(),
            ),
            download_locator: Some(
                "https://tracker.example/download.php?id=42&token=very-secret".to_string(),
            ),
            magnet: Some(
                "magnet:?xt=urn:btih:ABCDEF&dn=Release&tr=https%3A%2F%2Ftracker.example%2Fvery-secret"
                    .to_string(),
            ),
            size: 10,
            seeders: 2,
            leechers: 1,
            publish_time: Some(Utc::now()),
        };

        let json = serde_json::to_string(&result).unwrap();
        assert!(!json.contains("very-secret"));
        assert!(!json.contains("passkey"));
        assert!(!json.contains("token"));
        assert!(!json.contains("tracker.example%2Fvery-secret"));
        assert!(json.contains("\"download_locator\":\"42\""));
    }
}
