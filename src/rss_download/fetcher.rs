use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use reqwest::header::{
    AUTHORIZATION, COOKIE, ETAG, HeaderMap, HeaderValue, IF_MODIFIED_SINCE, IF_NONE_MATCH,
    LAST_MODIFIED,
};
use reqwest::{Client, Proxy, Response, StatusCode, Url};
use scraper::{Html, Selector};
use serde_json::Value;

use crate::db::{Database, rss::ItemLocator};
use crate::indexer::{
    IndexerError, IndexerPool, SearchResult, rate_limit_error_from_body,
    response_is_authentication_page,
};
use crate::rss::download::parse_download_feed;
use crate::site::{
    SiteAuth, SiteType, browser_request_header_map, default_site_request_headers,
    parse_site_request_headers, site_request_header_map,
};

use super::models::*;

const MAX_FEED_BYTES: usize = 8 * 1024 * 1024;
const MAX_TORRENT_BYTES: usize = 64 * 1024 * 1024;

/// Keep preview, persisted eligibility, and the actual adapter path aligned.
/// These are the credential forms accepted by the existing indexer constructors.
pub(crate) fn supports_torrent_resolution(site_type: &str, auth_config: &str) -> bool {
    let Ok(auth) = serde_json::from_str::<SiteAuth>(auth_config) else {
        return false;
    };
    let credential = match (SiteType::from_str(site_type.trim()), auth) {
        (Some(SiteType::NexusPhp | SiteType::MTeam), SiteAuth::ApiKey { api_key }) => api_key,
        (
            Some(SiteType::NexusPhp),
            SiteAuth::Cookie { cookie } | SiteAuth::CookiePasskey { cookie, .. },
        ) => cookie,
        _ => return false,
    };
    let credential = credential.trim();
    !credential.is_empty() && HeaderValue::from_str(credential).is_ok()
}

pub struct RssFetcher {
    db: Database,
    indexers: Arc<IndexerPool>,
}

struct RequestContext {
    client: Client,
    headers: HeaderMap,
}

impl RssFetcher {
    pub fn new(db: Database, indexers: Arc<IndexerPool>) -> Self {
        Self { db, indexers }
    }

    async fn context(&self, feed: &StoredFeed, destination: &Url) -> RssResult<RequestContext> {
        let site =
            match feed.record.site_id {
                Some(id) => Some(self.db.get_site(id).await?.ok_or_else(|| {
                    RssError::NotFound("关联站点已不存在，请重新选择站点".into())
                })?),
                None => None,
            };
        let settings = self.db.get_settings().await?;
        let use_proxy = feed
            .record
            .use_proxy
            .unwrap_or_else(|| site.as_ref().is_some_and(|site| site.use_proxy));
        let mut builder = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .no_proxy();
        if use_proxy {
            let proxy = settings
                .proxy
                .as_deref()
                .filter(|p| !p.trim().is_empty())
                .ok_or_else(|| {
                    RssError::Invalid("尚未配置全局代理，请在系统设置中配置或关闭此源的代理".into())
                })?;
            builder = builder.proxy(
                Proxy::all(proxy).map_err(|_| RssError::Invalid("全局代理地址无效".into()))?,
            );
        }
        let client = builder
            .build()
            .map_err(|_| RssError::Unavailable("无法创建 RSS 网络连接".into()))?;
        let mut headers = site_request_header_map(&default_site_request_headers())
            .map_err(|_| RssError::Invalid("默认请求头无效".into()))?;
        if let Some(site) = &site {
            let custom = parse_site_request_headers(&site.request_headers)
                .and_then(|h| site_request_header_map(&h))
                .map_err(|_| RssError::Invalid("关联站点请求头无效，请检查站点配置".into()))?;
            let base = checked_url(&site.base_url)?;
            if same_origin(&base, destination) {
                headers.extend(custom);
                let auth: SiteAuth = serde_json::from_str(&site.auth_config)
                    .map_err(|_| RssError::Invalid("关联站点认证配置无效".into()))?;
                match auth {
                    SiteAuth::Cookie { cookie } | SiteAuth::CookiePasskey { cookie, .. } => {
                        headers.insert(COOKIE, sensitive_header(&cookie)?);
                    }
                    SiteAuth::ApiKey { api_key }
                        if matches!(site.site_type.as_str(), "mteam" | "m_team") =>
                    {
                        headers.insert("x-api-key", sensitive_header(&api_key)?);
                    }
                    SiteAuth::ApiKey { api_key } => {
                        headers.insert(
                            AUTHORIZATION,
                            sensitive_header(&format!("Bearer {api_key}"))?,
                        );
                    }
                    SiteAuth::Passkey { .. } => {}
                }
            } else {
                headers.extend(browser_request_header_map(&custom));
            }
        }
        Ok(RequestContext { client, headers })
    }

    pub async fn fetch(&self, feed: &StoredFeed, conditional: bool) -> RssResult<FetchedFeed> {
        let url = checked_url(&feed.url)?;
        let context = self.context(feed, &url).await?;
        let gate = self.indexers.access_gate_for_url(&url).await;
        let _operation = gate.lock_operation().await;
        let mut request = context.client.get(url.clone()).headers(context.headers);
        if conditional && feed.record.initialized_at.is_some() {
            if let Some(value) = feed
                .etag
                .as_deref()
                .and_then(|v| HeaderValue::from_str(v).ok())
            {
                request = request.header(IF_NONE_MATCH, value);
            }
            if let Some(value) = feed
                .last_modified
                .as_deref()
                .and_then(|v| HeaderValue::from_str(v).ok())
            {
                request = request.header(IF_MODIFIED_SINCE, value);
            }
        }
        let response = gate
            .send_with_same_origin_redirects(&context.client, request, &url)
            .await
            .map_err(public_indexer_error)?;
        let etag = header_string(&response, ETAG);
        let last_modified = header_string(&response, LAST_MODIFIED);
        if response.status() == StatusCode::NOT_MODIFIED {
            if !conditional || feed.record.initialized_at.is_none() {
                return Err(RssError::Unavailable(
                    "首次抓取收到 304，未取得可用于初始化的 RSS 内容".into(),
                ));
            }
            return Ok(FetchedFeed {
                not_modified: true,
                etag,
                last_modified,
                ..Default::default()
            });
        }
        ensure_success(response.status())?;
        let final_url = response.url().clone();
        let body = bounded_body(response, MAX_FEED_BYTES).await?;
        let text = std::str::from_utf8(&body).unwrap_or("");
        if response_is_authentication_page(&final_url, text) {
            return Err(RssError::Authentication(
                "站点登录已失效或需要验证，请更新站点配置后重新检查".into(),
            ));
        }
        if let Some(IndexerError::RateLimited(limit)) = rate_limit_error_from_body(text) {
            return Err(public_indexer_error(IndexerError::RateLimited(
                gate.observe_rate_limit(limit).await,
            )));
        }
        let mut parsed =
            parse_download_feed(&body, final_url.as_str()).map_err(RssError::Invalid)?;
        parsed.etag = etag;
        parsed.last_modified = last_modified;
        Ok(parsed)
    }

    pub async fn torrent(
        &self,
        feed: &StoredFeed,
        item: &ItemRecord,
        locator: &ItemLocator,
    ) -> RssResult<Vec<u8>> {
        if let (Some(site_id), Some(torrent_id)) =
            (locator.site_id, locator.site_torrent_id.as_ref())
        {
            let mut site = self
                .db
                .get_site(site_id)
                .await?
                .ok_or_else(|| RssError::NotFound("关联站点已不存在".into()))?;
            if supports_torrent_resolution(&site.site_type, &site.auth_config) {
                let settings = self.db.get_settings().await?;
                site.use_proxy = feed.record.use_proxy.unwrap_or(site.use_proxy);
                if site.use_proxy
                    && settings
                        .proxy
                        .as_deref()
                        .is_none_or(|proxy| proxy.trim().is_empty())
                {
                    return Err(RssError::Invalid(
                        "尚未配置全局代理，请在系统设置中配置或关闭此源的代理".into(),
                    ));
                }
                let adapter = self
                    .indexers
                    .get_or_create(&site, settings.proxy.as_deref())
                    .await
                    .map_err(public_indexer_error)?;
                let result = SearchResult {
                    site_id,
                    source_site: site.name,
                    torrent_id: torrent_id.clone(),
                    title: item.title.clone(),
                    detail_url: locator.detail_url.clone(),
                    download_locator: Some(torrent_id.clone()),
                    magnet: None,
                    size: item.attributes.size_bytes.unwrap_or(0),
                    seeders: item.attributes.seeders.unwrap_or(0),
                    leechers: item.attributes.leechers.unwrap_or(0),
                    publish_time: item
                        .published_at
                        .as_deref()
                        .and_then(|v| DateTime::parse_from_rfc3339(v).ok())
                        .map(|v| v.with_timezone(&Utc)),
                };
                return adapter
                    .fetch_torrent(&result)
                    .await
                    .map_err(public_indexer_error);
            }
        }
        let raw = locator.download_url.as_deref().ok_or_else(|| {
            RssError::Invalid("条目没有可用的种子下载地址，关联站点也无法生成取种请求".into())
        })?;
        match self.direct_torrent(feed, raw).await {
            Ok(bytes) => Ok(bytes),
            Err(error @ (RssError::Authentication(_) | RssError::NotFound(_))) => {
                let latest = self.fetch(feed, false).await?;
                let replacement = latest
                    .items
                    .iter()
                    .find(|entry| entry.item_key == item.item_key);
                match replacement.and_then(|entry| entry.download_url.as_deref()) {
                    Some(url) if url != raw => self.direct_torrent(feed, url).await,
                    _ => Err(error),
                }
            }
            Err(error) => Err(error),
        }
    }

    async fn direct_torrent(&self, feed: &StoredFeed, raw: &str) -> RssResult<Vec<u8>> {
        let url = checked_url(raw)?;
        let context = self.context(feed, &url).await?;
        let gate = self.indexers.access_gate_for_url(&url).await;
        let _operation = gate.lock_operation().await;
        let response = gate
            .send_with_same_origin_redirects(
                &context.client,
                context.client.get(url.clone()).headers(context.headers),
                &url,
            )
            .await
            .map_err(public_indexer_error)?;
        ensure_success(response.status())?;
        let final_url = response.url().clone();
        let bytes = bounded_body(response, MAX_TORRENT_BYTES).await?;
        if response_is_authentication_page(&final_url, std::str::from_utf8(&bytes).unwrap_or("")) {
            return Err(RssError::Authentication(
                "种子下载需要重新登录或更新 RSS 地址".into(),
            ));
        }
        crate::media::torrent::torrent_infohash(&bytes).map_err(|_| {
            RssError::Invalid("下载响应不是有效的种子文件，请检查 RSS 地址或站点登录状态".into())
        })?;
        Ok(bytes)
    }

    /// Use fresh structured evidence only. Existing adapters expose booleans whose false value
    /// sometimes means "not detected", so they cannot establish a strict no-H&R decision.
    pub async fn enrich(
        &self,
        feed: &StoredFeed,
        item: &ItemRecord,
        locator: &ItemLocator,
    ) -> RssResult<ItemAttributes> {
        let Some(site_id) = locator.site_id else {
            return Ok(item.attributes.clone());
        };
        let Some(torrent_id) = locator.site_torrent_id.as_deref() else {
            return Ok(item.attributes.clone());
        };
        if torrent_id.is_empty() || !torrent_id.bytes().all(|b| b.is_ascii_digit()) {
            return Ok(item.attributes.clone());
        }
        let site = self
            .db
            .get_site(site_id)
            .await?
            .ok_or_else(|| RssError::NotFound("关联站点已不存在".into()))?;
        let base = checked_url(&site.base_url)?;
        let api_auth = matches!(
            serde_json::from_str::<SiteAuth>(&site.auth_config),
            Ok(SiteAuth::ApiKey { .. })
        );
        let (url, post, json_response) = match site.site_type.as_str() {
            "mteam" | "m_team" if api_auth => (base.join("/api/torrent/detail"), true, true),
            "nexusphp" | "nexus_php" | "nexus" if api_auth => (
                base.join(&format!("/api/v1/torrents/{torrent_id}")),
                false,
                true,
            ),
            "nexusphp" | "nexus_php" | "nexus" => (
                base.join(&format!("details.php?id={torrent_id}")),
                false,
                false,
            ),
            _ => return Ok(item.attributes.clone()),
        };
        let url = url.map_err(|_| RssError::Invalid("站点详情地址无法构建".into()))?;
        let context = self.context(feed, &url).await?;
        let gate = self.indexers.access_gate_for_url(&url).await;
        let _operation = gate.lock_operation().await;
        let request = if post {
            context.client.post(url.clone()).form(&[("id", torrent_id)])
        } else {
            context.client.get(url.clone())
        };
        let response = gate
            .send_with_same_origin_redirects(
                &context.client,
                request.headers(context.headers),
                &base,
            )
            .await
            .map_err(public_indexer_error)?;
        ensure_success(response.status())?;
        let final_url = response.url().clone();
        let bytes = bounded_body(response, MAX_FEED_BYTES).await?;
        let text = std::str::from_utf8(&bytes)
            .map_err(|_| RssError::Invalid("站点属性响应不是 UTF-8".into()))?;
        if response_is_authentication_page(&final_url, text) {
            return Err(RssError::Authentication(
                "站点登录已失效，无法确认种子属性".into(),
            ));
        }
        if let Some(IndexerError::RateLimited(limit)) = rate_limit_error_from_body(text) {
            return Err(public_indexer_error(IndexerError::RateLimited(
                gate.observe_rate_limit(limit).await,
            )));
        }
        let fresh = if json_response {
            let json: Value = serde_json::from_str(text)
                .map_err(|_| RssError::Invalid("站点属性响应不是有效 JSON".into()))?;
            attributes_from_json(&json, site.site_type.starts_with('m'))?
        } else {
            attributes_from_html(text, Some(torrent_id))
        };
        Ok(merge_attributes(&item.attributes, fresh))
    }

    pub async fn fresh_attributes(
        &self,
        feed: &StoredFeed,
        item: &ItemRecord,
        locator: &ItemLocator,
        filters: &RuleFilters,
    ) -> RssResult<ItemAttributes> {
        let time_sensitive = filters.free_only || filters.hr_policy == "require_clear";
        let stale = DateTime::parse_from_rfc3339(&item.attributes.observed_at)
            .ok()
            .is_none_or(|time| (Utc::now() - time.with_timezone(&Utc)).num_seconds() > 60);
        let mut current = item.clone();
        if time_sensitive && stale {
            let fetched = self.fetch(feed, false).await?;
            if let Some(entry) = fetched
                .items
                .into_iter()
                .find(|entry| entry.item_key == item.item_key)
            {
                current.attributes = entry.attributes;
            } else {
                // An old successful lookup is not current proof of freeleech or no H&R.
                current.attributes.download_volume_factor = None;
                current.attributes.hr = None;
                current.attributes.observed_at = Utc::now().to_rfc3339();
            }
        }
        let decision = super::matcher::evaluate(filters, &current);
        if decision.needs_attributes {
            self.enrich(feed, &current, locator).await
        } else {
            Ok(current.attributes)
        }
    }
}

pub fn checked_url(raw: &str) -> RssResult<Url> {
    if raw.len() > 8192 {
        return Err(RssError::Invalid("RSS 地址不能超过 8192 字节".into()));
    }
    let url = Url::parse(raw.trim())
        .map_err(|_| RssError::Invalid("请输入完整的 HTTP 或 HTTPS 地址".into()))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(RssError::Invalid(
            "仅支持 HTTP/HTTPS 地址，地址中不能包含用户名或密码".into(),
        ));
    }
    Ok(url)
}

fn same_origin(a: &Url, b: &Url) -> bool {
    a.origin() == b.origin()
}
fn sensitive_header(value: &str) -> RssResult<HeaderValue> {
    let mut header = HeaderValue::from_str(value)
        .map_err(|_| RssError::Invalid("站点凭据包含无效请求头字符".into()))?;
    header.set_sensitive(true);
    Ok(header)
}
fn header_string(response: &Response, name: reqwest::header::HeaderName) -> Option<String> {
    response
        .headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .filter(|v| v.len() <= 4096)
        .map(str::to_string)
}
fn ensure_success(status: StatusCode) -> RssResult<()> {
    match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Err(RssError::Authentication(
            "站点拒绝访问，请检查 RSS 地址和站点登录凭据".into(),
        )),
        StatusCode::NOT_FOUND | StatusCode::GONE => Err(RssError::NotFound(
            "订阅源或种子链接已失效，请检查来源地址".into(),
        )),
        status if status.is_success() => Ok(()),
        status if status.is_redirection() => Err(RssError::Invalid(
            "站点要求跳转到另一地址，未发送凭据，请检查配置".into(),
        )),
        status => Err(RssError::Unavailable(format!(
            "远端返回 HTTP {}，请稍后重新检查",
            status.as_u16()
        ))),
    }
}
pub fn public_indexer_error(error: IndexerError) -> RssError {
    match error {
        IndexerError::RateLimited(limit) => RssError::RateLimited {
            message: "站点正在限流，冷却结束后将继续检查".into(),
            retry_at: (Utc::now()
                + chrono::Duration::seconds(
                    limit.retry_after_secs().unwrap_or(60).min(86400) as i64
                ))
            .to_rfc3339(),
        },
        IndexerError::AuthenticationExpired(_) => {
            RssError::Authentication("站点登录已失效或需要验证，请更新站点配置".into())
        }
        IndexerError::UnsafeUrl(_) => {
            RssError::Invalid("远端链接或重定向不符合来源限制，请检查站点地址".into())
        }
        IndexerError::Configuration(_) => {
            RssError::Invalid("站点配置不支持此取种请求，请检查地址和认证方式".into())
        }
        IndexerError::InvalidTorrent(_) => RssError::Invalid("远端未返回有效的种子文件".into()),
        IndexerError::Parse(_) => {
            RssError::Invalid("站点响应无法解析，请检查登录状态或站点适配情况".into())
        }
        IndexerError::Http(_) | IndexerError::Api(_) => {
            RssError::Unavailable("站点请求失败，请检查连接与认证配置后重试".into())
        }
    }
}
async fn bounded_body(mut response: Response, limit: usize) -> RssResult<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(RssError::Invalid(format!(
            "响应超过 {} MiB 上限，未处理此响应",
            limit / 1024 / 1024
        )));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| RssError::Unavailable("读取远端响应失败，请稍后重试".into()))?
    {
        if bytes.len().saturating_add(chunk.len()) > limit {
            return Err(RssError::Invalid(format!(
                "响应超过 {} MiB 上限，未处理此响应",
                limit / 1024 / 1024
            )));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn number(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str()?.parse().ok())
        .filter(|v| v.is_finite() && *v >= 0.0)
}
fn count(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| value.as_str()?.parse().ok())
}
fn boolean(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(v) => Some(*v),
        Value::Number(v) if v.as_u64() == Some(0) => Some(false),
        Value::Number(v) if v.as_u64() == Some(1) => Some(true),
        Value::String(v) if matches!(v.to_ascii_lowercase().as_str(), "true" | "1" | "yes") => {
            Some(true)
        }
        Value::String(v) if matches!(v.to_ascii_lowercase().as_str(), "false" | "0" | "no") => {
            Some(false)
        }
        _ => None,
    }
}
fn first<'a>(objects: &[&'a Value], keys: &[&str]) -> Option<&'a Value> {
    objects.iter().find_map(|object| {
        keys.iter()
            .find_map(|key| object.get(*key).filter(|v| !v.is_null()))
    })
}

fn attributes_from_json(json: &Value, mteam: bool) -> RssResult<ItemAttributes> {
    if mteam
        && json
            .get("code")
            .is_some_and(|code| code.as_str() != Some("0") && code.as_i64() != Some(0))
    {
        return Err(RssError::Unavailable(
            "站点返回属性查询错误，请检查站点配置".into(),
        ));
    }
    let data = json.get("data").unwrap_or(json);
    let objects = [
        data.get("status").filter(|v| v.is_object()).unwrap_or(data),
        data.get("attributes").unwrap_or(data),
        data,
    ];
    let mut result = ItemAttributes {
        observed_at: Utc::now().to_rfc3339(),
        source: "site_api".into(),
        ..Default::default()
    };
    result.size_bytes = first(&objects, &["size", "size_bytes"]).and_then(count);
    result.seeders = first(&objects, &["seeders", "seeder_count"])
        .and_then(count)
        .and_then(|v| u32::try_from(v).ok());
    result.leechers = first(&objects, &["leechers", "leecher_count"])
        .and_then(count)
        .and_then(|v| u32::try_from(v).ok());
    result.download_volume_factor = first(
        &objects,
        &["download_volume_factor", "downloadvolumefactor"],
    )
    .and_then(number);
    result.upload_volume_factor =
        first(&objects, &["upload_volume_factor", "uploadvolumefactor"]).and_then(number);
    result.minimum_ratio = first(&objects, &["minimum_ratio", "minimumratio"]).and_then(number);
    result.minimum_seed_time =
        first(&objects, &["minimum_seed_time", "minimumseedtime"]).and_then(count);
    result.hr = first(&objects, &["hit_and_run", "hitAndRun", "hr"]).and_then(boolean);
    if result.minimum_ratio.is_some_and(|v| v > 0.0)
        || result.minimum_seed_time.is_some_and(|v| v > 0)
    {
        result.hr = Some(true);
    } else if result.minimum_ratio == Some(0.0)
        && result.minimum_seed_time == Some(0)
        && result.hr != Some(true)
    {
        result.hr = Some(false);
    }
    if result.download_volume_factor.is_none() {
        if let Some(free) = first(&objects, &["freeleech", "free"]).and_then(boolean) {
            result.download_volume_factor = Some(if free { 0.0 } else { 1.0 });
        }
    }
    if mteam {
        if let Some(discount) = first(&objects, &["discount"]).and_then(Value::as_str) {
            let factor = match discount {
                "FREE" | "_2X_FREE" | "FREE_2XUP" | "TWOFREE" => Some(0.0),
                "PERCENT_50" | "_2X_PERCENT_50" | "PERCENT_50_2XUP" => Some(0.5),
                "PERCENT_70" | "_2X_PERCENT_70" | "PERCENT_70_2XUP" => Some(0.3),
                "NORMAL" | "_2X" | "TWOUP" => Some(1.0),
                _ => None,
            };
            result.download_volume_factor = factor.or(result.download_volume_factor);
        }
    }
    result.free_until = first(&objects, &["free_end_timestamp", "free_until"])
        .and_then(count)
        .and_then(|v| i64::try_from(v).ok());
    if result.free_until.is_none() {
        result.free_until = first(&objects, &["discountEndTime"])
            .and_then(Value::as_str)
            .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
            .map(|date| date.timestamp());
    }
    Ok(result)
}

fn attributes_from_html(html: &str, torrent_id: Option<&str>) -> ItemAttributes {
    let document = Html::parse_document(html);
    let mut result = ItemAttributes {
        observed_at: Utc::now().to_rfc3339(),
        source: "site_detail".into(),
        ..Default::default()
    };
    // Only explicit, torrent-specific markup is evidence. Whole-page words and absence of a
    // badge are not proof: trackers frequently include freeleech/H&R help in their page chrome.
    let mut containers = vec![
        "#torrent-info".to_string(),
        ".torrent-detail:not([data-torrent-id])".to_string(),
    ];
    if let Some(id) =
        torrent_id.filter(|id| !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit()))
    {
        containers.push(format!("[data-torrent-id=\"{id}\"]"));
    }
    for (attribute, field) in [
        ("data-download-volume-factor", "free"),
        ("data-hr", "hr"),
        ("data-seeders", "seeders"),
    ] {
        let selector = containers
            .iter()
            .map(|container| format!("{container}[{attribute}]"))
            .collect::<Vec<_>>()
            .join(", ");
        if let Ok(selector) = Selector::parse(&selector) {
            for value in document
                .select(&selector)
                .filter_map(|node| node.value().attr(attribute))
            {
                match field {
                    "free" => {
                        if let Some(factor) = number(&Value::String(value.into())) {
                            result.download_volume_factor = Some(
                                result
                                    .download_volume_factor
                                    .map_or(factor, |old| old.max(factor)),
                            );
                        }
                    }
                    "hr" => {
                        if let Some(hr) = boolean(&Value::String(value.into())) {
                            result.hr = Some(result.hr.unwrap_or(false) || hr);
                        }
                    }
                    "seeders" => {
                        if let Ok(seeders) = value.parse::<u32>() {
                            result.seeders =
                                Some(result.seeders.map_or(seeders, |old| old.min(seeders)));
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    for (class, hr) in [(".hitandrun", true), (".no-hr", false)] {
        let selector = containers
            .iter()
            .map(|container| format!("{container} {class}"))
            .collect::<Vec<_>>()
            .join(", ");
        if Selector::parse(&selector)
            .ok()
            .is_some_and(|s| document.select(&s).next().is_some())
        {
            if result.hr != Some(true) {
                result.hr = Some(hr);
            }
        }
    }
    let selector = containers
        .iter()
        .flat_map(|container| {
            [
                format!("{container} .pro_free"),
                format!("{container} .pro_free2up"),
            ]
        })
        .collect::<Vec<_>>()
        .join(", ");
    if result.download_volume_factor.is_none()
        && Selector::parse(&selector)
            .ok()
            .is_some_and(|s| document.select(&s).next().is_some())
    {
        result.download_volume_factor = Some(0.0);
    }
    result
}

fn merge_attributes(old: &ItemAttributes, fresh: ItemAttributes) -> ItemAttributes {
    let retaining_dynamic_evidence = (fresh.hr.is_none() && old.hr.is_some())
        || (fresh.download_volume_factor.is_none() && old.download_volume_factor.is_some());
    ItemAttributes {
        size_bytes: fresh.size_bytes.or(old.size_bytes),
        seeders: fresh.seeders.or(old.seeders),
        leechers: fresh.leechers.or(old.leechers),
        download_volume_factor: fresh.download_volume_factor.or(old.download_volume_factor),
        upload_volume_factor: fresh.upload_volume_factor.or(old.upload_volume_factor),
        hr: fresh.hr.or(old.hr),
        minimum_ratio: fresh.minimum_ratio.or(old.minimum_ratio),
        minimum_seed_time: fresh.minimum_seed_time.or(old.minimum_seed_time),
        free_until: if fresh.download_volume_factor.is_some() {
            fresh.free_until
        } else {
            old.free_until
        },
        observed_at: if retaining_dynamic_evidence {
            old.observed_at.clone()
        } else {
            fresh.observed_at
        },
        source: if retaining_dynamic_evidence {
            let mut sources = old
                .source
                .split(" + ")
                .chain(fresh.source.split(" + "))
                .collect::<Vec<_>>();
            sources.sort_unstable();
            sources.dedup();
            sources.truncate(8);
            sources.join(" + ")
        } else {
            fresh.source
        },
        hints: old.hints.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, extract::State, http::Uri, routing::get};
    use std::sync::Mutex;

    const TEST_TORRENT: &[u8] = b"d4:infod6:lengthi12e4:name8:file.mkvee";
    const TEST_FEED: &str = r#"<rss version="2.0"><channel><title>Adapter test</title>
        <item><guid>stable-only</guid><title>Documentary 42</title><link>/details.php?id=42</link></item>
        <item><guid>with-enclosure</guid><title>Documentary 43</title><link>/details.php?id=43</link>
        <enclosure url="/file.torrent" type="application/x-bittorrent" length="12"/></item>
        </channel></rss>"#;
    type Requests = Arc<Mutex<Vec<String>>>;

    async fn serve_torrent(
        State(requests): State<Requests>,
        uri: Uri,
    ) -> ([(&'static str, &'static str); 1], &'static [u8]) {
        requests.lock().unwrap().push(uri.to_string());
        ([("content-type", "application/x-bittorrent")], TEST_TORRENT)
    }

    struct TorrentFixture {
        _directory: tempfile::TempDir,
        db: Database,
        fetcher: RssFetcher,
        feed: StoredFeed,
        requests: Requests,
        server: tokio::task::JoinHandle<()>,
    }

    impl Drop for TorrentFixture {
        fn drop(&mut self) {
            self.server.abort();
        }
    }

    impl TorrentFixture {
        async fn new(kind: &str, auth: &str, site_proxy: bool) -> Self {
            let requests: Requests = Arc::new(Mutex::new(Vec::new()));
            let app = Router::new()
                .route("/rss", get(|| async { TEST_FEED }))
                .route("/file.torrent", get(serve_torrent))
                .route("/download.php", get(serve_torrent))
                .with_state(requests.clone());
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let endpoint = format!("http://{}", listener.local_addr().unwrap());
            let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            let directory = tempfile::tempdir().unwrap();
            let db = Database::open(directory.path()).await.unwrap();
            let site = db
                .create_site(
                    "Tracker",
                    kind,
                    &endpoint,
                    auth,
                    &serde_json::to_string(&default_site_request_headers()).unwrap(),
                    site_proxy,
                )
                .await
                .unwrap();
            let record = db
                .rss_save_feed(
                    None,
                    FeedInput {
                        name: "Adapter feed".into(),
                        url: Some(format!("{endpoint}/rss")),
                        site_id: Some(site),
                        use_proxy: Some(false),
                        enabled: true,
                        interval_minutes: 15,
                        expected_version: None,
                        request_id: None,
                    },
                )
                .await
                .unwrap();
            let feed = db.rss_get_feed(record.id).await.unwrap();
            let fetcher = RssFetcher::new(db.clone(), IndexerPool::new());
            let response = fetcher.fetch(&feed, false).await.unwrap();
            let claim = db.rss_claim_feed("test", 120).await.unwrap().unwrap();
            db.rss_commit_scan(&claim, response).await.unwrap();
            Self {
                _directory: directory,
                db,
                fetcher,
                feed,
                requests,
                server,
            }
        }

        async fn item(&self, torrent_id: &str) -> ItemRecord {
            self.db
                .rss_list_items(ListQuery::default())
                .await
                .unwrap()
                .items
                .into_iter()
                .find(|item| item.site_torrent_id.as_deref() == Some(torrent_id))
                .unwrap()
        }
    }

    #[test]
    fn torrent_resolution_requires_the_credentials_accepted_by_the_adapter() {
        for (site_type, auth, expected) in [
            ("mteam", r#"{"auth_type":"api_key","api_key":"key"}"#, true),
            (
                "m_team",
                r#"{"auth_type":"api_key","api_key":" key "}"#,
                true,
            ),
            ("mteam", r#"{"auth_type":"api_key","api_key":" "}"#, false),
            (
                "mteam",
                r#"{"auth_type":"cookie","cookie":"session=1"}"#,
                false,
            ),
            (
                "mteam",
                r#"{"auth_type":"cookie_passkey","cookie":"session=1","passkey":"key"}"#,
                false,
            ),
            (
                "nexusphp",
                r#"{"auth_type":"cookie","cookie":"session=1"}"#,
                true,
            ),
            (
                "nexus_php",
                r#"{"auth_type":"cookie_passkey","cookie":"session=1","passkey":""}"#,
                true,
            ),
            (
                "nexusphp",
                r#"{"auth_type":"api_key","api_key":"key"}"#,
                true,
            ),
            ("nexusphp", r#"{"auth_type":"api_key","api_key":""}"#, false),
            ("nexusphp", r#"{"auth_type":"cookie","cookie":"  "}"#, false),
            (
                "nexusphp",
                r#"{"auth_type":"cookie_passkey","cookie":"","passkey":"key"}"#,
                false,
            ),
            (
                "nexusphp",
                r#"{"auth_type":"passkey","passkey":"key"}"#,
                false,
            ),
            (
                "nexusphp",
                r#"{"auth_type":"cookie","cookie":"session=1\nInjected: value"}"#,
                false,
            ),
            (
                "gazelle",
                r#"{"auth_type":"api_key","api_key":"key"}"#,
                false,
            ),
            (
                "unknown",
                r#"{"auth_type":"api_key","api_key":"key"}"#,
                false,
            ),
            ("nexusphp", "{}", false),
        ] {
            assert_eq!(
                supports_torrent_resolution(site_type, auth),
                expected,
                "site type {site_type}"
            );
        }
    }

    #[tokio::test]
    async fn unsupported_adapter_auth_keeps_preview_and_storage_honest_and_uses_enclosures() {
        let fixture = TorrentFixture::new(
            "mteam",
            r#"{"auth_type":"cookie","cookie":"session=1"}"#,
            false,
        )
        .await;
        let unsupported = fixture.item("42").await;
        let enclosure = fixture.item("43").await;
        assert!(!unsupported.downloadable);
        assert!(enclosure.downloadable);
        let filters = RuleFilters {
            match_all: true,
            hr_policy: "any".into(),
            ..Default::default()
        };
        assert!(!crate::rss_download::matcher::evaluate(&filters, &unsupported).matched);
        assert!(crate::rss_download::matcher::evaluate(&filters, &enclosure).matched);
        let downloader = fixture
            .db
            .create_downloader(
                "queue target",
                "qb",
                "http://127.0.0.1:1",
                "admin",
                "password",
            )
            .await
            .unwrap();
        let rule = fixture
            .db
            .rss_save_rule(
                None,
                RuleInput {
                    name: "All downloadable items".into(),
                    enabled: true,
                    priority: 100,
                    feed_ids: vec![fixture.feed.record.id],
                    filters,
                    downloader_id: Some(downloader),
                    options: DownloadOptions::default(),
                    expected_version: None,
                    request_id: None,
                },
            )
            .await
            .unwrap();
        fixture
            .db
            .rss_backfill(BackfillRequest {
                rule_id: rule.id,
                expected_version: rule.version,
                item_ids: vec![unsupported.id, enclosure.id],
                request_id: "adapter-eligibility".into(),
            })
            .await
            .unwrap();
        let mut queued = 0;
        while let Some(claim) = fixture.db.rss_claim_evaluation("test", 120).await.unwrap() {
            queued += fixture
                .db
                .rss_commit_evaluation(&claim, None)
                .await
                .unwrap();
        }
        assert_eq!(queued, 1);
        let jobs = fixture
            .db
            .rss_list_jobs(ListQuery::default())
            .await
            .unwrap();
        assert_eq!(jobs.items[0].item_id, enclosure.id);

        let pool = crate::downloader::DownloaderClientPool::new(fixture.db.clone());
        let collector = Arc::new(crate::collector::DownloaderSnapshotCollector::new(
            fixture.db.clone(),
            pool.clone(),
        ));
        let service = crate::rss_download::service::RssService::new(
            fixture.db.clone(),
            pool,
            IndexerPool::new(),
            collector,
        );
        let preview = service
            .test_feed(FeedTestRequest {
                feed_id: Some(fixture.feed.record.id),
                url: None,
                site_id: None,
                use_proxy: None,
            })
            .await
            .unwrap();
        assert!(
            !preview
                .items
                .iter()
                .find(|item| item.site_torrent_id.as_deref() == Some("42"))
                .unwrap()
                .downloadable
        );
        assert!(
            preview
                .items
                .iter()
                .find(|item| item.site_torrent_id.as_deref() == Some("43"))
                .unwrap()
                .downloadable
        );

        let locator = fixture
            .db
            .rss_get_item_locator(unsupported.id)
            .await
            .unwrap();
        assert!(matches!(
            fixture
                .fetcher
                .torrent(&fixture.feed, &unsupported, &locator)
                .await,
            Err(RssError::Invalid(_))
        ));
        let locator = fixture.db.rss_get_item_locator(enclosure.id).await.unwrap();
        let bytes = fixture
            .fetcher
            .torrent(&fixture.feed, &enclosure, &locator)
            .await
            .unwrap();
        assert_eq!(bytes, TEST_TORRENT);
        assert_eq!(*fixture.requests.lock().unwrap(), vec!["/file.torrent"]);
    }

    #[tokio::test]
    async fn stable_id_downloads_honor_proxy_overrides_and_reject_missing_proxy_configuration() {
        let mut fixture = TorrentFixture::new(
            "nexusphp",
            r#"{"auth_type":"cookie","cookie":"session=1"}"#,
            true,
        )
        .await;
        let item = fixture.item("42").await;
        assert!(item.downloadable);
        let locator = fixture.db.rss_get_item_locator(item.id).await.unwrap();
        fixture.feed.record.use_proxy = None;
        assert!(matches!(
            fixture
                .fetcher
                .torrent(&fixture.feed, &item, &locator)
                .await,
            Err(RssError::Invalid(_))
        ));
        let site = fixture
            .db
            .get_site(locator.site_id.unwrap())
            .await
            .unwrap()
            .unwrap();
        fixture
            .db
            .update_site(
                site.id,
                &site.name,
                &site.site_type,
                &site.base_url,
                &site.auth_config,
                &site.request_headers,
                false,
            )
            .await
            .unwrap();
        fixture.feed.record.use_proxy = Some(true);
        assert!(matches!(
            fixture
                .fetcher
                .torrent(&fixture.feed, &item, &locator)
                .await,
            Err(RssError::Invalid(_))
        ));

        let proxy_requests: Requests = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new()
            .fallback(get(serve_torrent))
            .with_state(proxy_requests.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy = format!("http://{}", listener.local_addr().unwrap());
        let proxy_server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let mut settings = fixture.db.get_settings().await.unwrap();
        settings.proxy = Some(proxy);
        fixture.db.update_settings(&settings).await.unwrap();
        assert_eq!(
            fixture
                .fetcher
                .torrent(&fixture.feed, &item, &locator)
                .await
                .unwrap(),
            TEST_TORRENT
        );
        assert_eq!(proxy_requests.lock().unwrap().len(), 1);
        assert!(
            fixture.requests.lock().unwrap().is_empty(),
            "the proxy handled the request"
        );
        fixture.feed.record.use_proxy = Some(false);
        assert_eq!(
            fixture
                .fetcher
                .torrent(&fixture.feed, &item, &locator)
                .await
                .unwrap(),
            TEST_TORRENT
        );
        assert_eq!(
            proxy_requests.lock().unwrap().len(),
            1,
            "explicit direct mode must skip the configured proxy"
        );
        assert_eq!(
            *fixture.requests.lock().unwrap(),
            vec!["/download.php?id=42"]
        );
        proxy_server.abort();
    }

    #[test]
    fn detail_absence_and_help_text_do_not_imply_free_or_clear_hr() {
        let result = attributes_from_html(
            "<html><body>FREELEECH 帮助 H&amp;R 规则</body></html>",
            Some("1"),
        );
        assert_eq!(result.hr, None);
        assert_eq!(result.download_volume_factor, None);
        let result = attributes_from_html(
            "<div id='torrent-info' data-hr='false' data-download-volume-factor='0'></div>",
            Some("1"),
        );
        assert_eq!(result.hr, Some(false));
        assert_eq!(result.download_volume_factor, Some(0.0));
    }

    #[test]
    fn sidebars_and_other_torrents_cannot_supply_free_or_clear_hr_evidence() {
        let result = attributes_from_html(
            "<div id='torrent-info' data-download-volume-factor='1'><img class='pro_free'></div><aside><img class='pro_free'><i class='no-hr'></i></aside><div class='torrent-detail' data-torrent-id='2' data-hr='false' data-download-volume-factor='0'></div>",
            Some("1"),
        );
        assert_eq!(result.download_volume_factor, Some(1.0));
        assert_eq!(result.hr, None);
    }

    #[test]
    fn api_missing_hr_stays_unknown_and_unknown_discount_is_not_free() {
        let attrs = attributes_from_json(
            &serde_json::json!({"code":"0","data":{"status":{"discount":"FREE","seeders":"4"}}}),
            true,
        )
        .unwrap();
        assert_eq!(attrs.hr, None);
        assert_eq!(attrs.seeders, Some(4));
        assert_eq!(attrs.download_volume_factor, Some(0.0));
        let attrs = attributes_from_json(
            &serde_json::json!({"data":{"status":{"discount":"new_promotion"}}}),
            true,
        )
        .unwrap();
        assert_eq!(attrs.download_volume_factor, None);
        let attrs = attributes_from_json(
            &serde_json::json!({"data":{"minimum_ratio":0,"minimum_seed_time":0,"hr":true}}),
            false,
        )
        .unwrap();
        assert_eq!(attrs.hr, Some(true));
    }

    #[test]
    fn merging_one_attribute_cannot_refresh_old_promotion_evidence() {
        let old = ItemAttributes {
            download_volume_factor: Some(0.0),
            observed_at: "2025-01-01T00:00:00Z".into(),
            ..Default::default()
        };
        let fresh = ItemAttributes {
            hr: Some(false),
            observed_at: Utc::now().to_rfc3339(),
            ..Default::default()
        };
        assert_eq!(merge_attributes(&old, fresh).observed_at, old.observed_at);
    }

    #[test]
    fn invalid_urls_do_not_echo_secrets() {
        let error = checked_url("file:///private/passkey-secret")
            .err()
            .unwrap()
            .to_string();
        assert!(!error.contains("passkey-secret"));
        assert!(checked_url("https://user:secret@example.test/rss").is_err());
        assert!(checked_url("http://127.0.0.1/rss?passkey=test").is_ok());
    }
}
