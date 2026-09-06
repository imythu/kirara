use std::future::Future;
use std::pin::Pin;

use chrono::{DateTime, FixedOffset, NaiveDateTime, TimeZone};

use reqwest::header::{COOKIE, HeaderMap, HeaderValue};
use reqwest::{Client, Url};
use scraper::{Html, Selector};
use serde_json::Value;

use super::{SiteAdapter, SiteAuth, TorrentAttributes, UserStats, UserStatsDetails};

/// Gazelle JSON API user statistics, including GPW's profile-only true download total.
pub struct GazelleAdapter {
    base_url: String,
    auth: SiteAuth,
    headers: HeaderMap,
    client: Client,
}

impl GazelleAdapter {
    pub fn new(base_url: String, auth: SiteAuth, headers: HeaderMap, client: Client) -> Self {
        Self {
            base_url,
            auth,
            headers,
            client,
        }
    }

    async fn request(&self, path: &str, query: &[(&str, &str)]) -> Result<String, String> {
        let cookie = match &self.auth {
            SiteAuth::Cookie { cookie } | SiteAuth::CookiePasskey { cookie, .. } => cookie,
            _ => return Err("Gazelle 用户统计需要 Cookie".to_string()),
        };
        let mut headers = self.headers.clone();
        headers.insert(
            COOKIE,
            HeaderValue::from_str(cookie).map_err(|_| "Cookie 格式无效")?,
        );
        let base = Url::parse(&self.base_url).map_err(|_| "站点地址无效")?;
        let url = base.join(path).map_err(|_| "站点接口地址无效")?;
        let response = self
            .client
            .get(url)
            .query(query)
            .headers(headers)
            .send()
            .await
            .map_err(|_| "Gazelle 请求失败，请检查网络或代理".to_string())?;
        if !response.status().is_success() {
            return Err(format!("Gazelle 返回 HTTP {}", response.status()));
        }
        response
            .text()
            .await
            .map_err(|_| "读取 Gazelle 响应失败".to_string())
    }

    async fn api(&self, action: &str, id: Option<&str>) -> Result<Value, String> {
        let mut query = vec![("action", action)];
        if let Some(id) = id {
            query.push(("id", id));
        }
        let text = self.request("/ajax.php", &query).await?;
        let result: Value = serde_json::from_str(&text)
            .map_err(|_| "Gazelle 未返回 JSON，请检查 Cookie 或站点验证页".to_string())?;
        if result["status"] != "success" || !result["response"].is_object() {
            return Err("Gazelle API 获取用户信息失败，请检查 Cookie".to_string());
        }
        Ok(result["response"].clone())
    }

    async fn fetch_stats(&self) -> Result<UserStats, String> {
        let index = self.api("index", None).await?;
        let id = numeric_id(&index["id"]).ok_or("Gazelle 用户 ID 缺失")?;
        let user = self.api("user", Some(&id)).await?;
        let ptd_id = Url::parse(&self.base_url)
            .ok()
            .and_then(|url| url.host_str().and_then(crate::ptd_sites::site_id_for_host));
        let offset = if matches!(ptd_id, Some("greatposterwall" | "dicmusic")) {
            8
        } else {
            0
        };
        let mut stats = parse_stats(&index, &user, offset)?;
        if ptd_id == Some("greatposterwall") {
            stats.details.level_id = stats
                .details
                .level_name
                .as_deref()
                .and_then(|name| super::nexusphp_levels::level_id("greatposterwall", name));
            let html = self.request("/user.php", &[("id", &id)]).await?;
            stats.details.true_downloaded = parse_true_downloaded(&html);
        }
        stats.fill_derived();
        Ok(stats)
    }
}

fn numeric_id(value: &Value) -> Option<String> {
    value.as_u64().map(|id| id.to_string()).or_else(|| {
        value
            .as_str()
            .filter(|id| !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()))
            .map(str::to_string)
    })
}

fn number(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str()?.parse().ok())
        .filter(|n| n.is_finite())
}

fn integer(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| value.as_str()?.parse().ok())
}

fn parse_stats(index: &Value, user: &Value, offset_hours: i32) -> Result<UserStats, String> {
    let uid = numeric_id(&index["id"]).ok_or("Gazelle 用户 ID 缺失")?;
    if user
        .get("id")
        .is_some_and(|id| numeric_id(id).as_deref() != Some(&uid))
    {
        return Err("Gazelle 用户信息 ID 不一致".to_string());
    }
    let username = index["username"]
        .as_str()
        .filter(|name| !name.trim().is_empty())
        .ok_or("Gazelle 用户名缺失")?
        .to_string();
    if user
        .get("username")
        .and_then(Value::as_str)
        .is_some_and(|name| name != username)
    {
        return Err("Gazelle 用户信息账号不一致".to_string());
    }
    let totals = &index["userstats"];
    let community = &user["community"];
    let mut details = UserStatsDetails {
        level_name: totals["class"].as_str().map(str::to_string),
        join_time: user["stats"]["joinedDate"]
            .as_str()
            .and_then(|value| parse_time(value, offset_hours)),
        last_access_at: user["stats"]["lastAccess"]
            .as_str()
            .and_then(|value| parse_time(value, offset_hours)),
        message_count: integer(&index["notifications"]["messages"]),
        seeding_size: integer(&totals["seedingSize"]),
        bonus_per_hour: number(&totals["bonusPointsPerHour"])
            .or_else(|| number(&totals["seedingBonusPointsPerHour"])),
        uploads: integer(&community["uploaded"]),
        ..Default::default()
    };
    for field in ["groups", "invited", "perfectFlacs"] {
        if let Some(value) = integer(&community[field]) {
            details.extra.insert(field.to_string(), value.into());
        }
    }
    Ok(UserStats {
        uid: Some(uid),
        username,
        uploaded: integer(&totals["uploaded"]).ok_or("Gazelle 上传量缺失")?,
        downloaded: integer(&totals["downloaded"]).ok_or("Gazelle 下载量缺失")?,
        ratio: number(&totals["ratio"]),
        bonus: number(&totals["bonusPoints"]),
        seeding_count: integer(&community["seeding"]).and_then(|v| u32::try_from(v).ok()),
        leeching_count: integer(&community["leeching"]).and_then(|v| u32::try_from(v).ok()),
        details,
    })
}

fn parse_time(value: &str, offset_hours: i32) -> Option<i64> {
    if let Ok(time) = DateTime::parse_from_rfc3339(value) {
        return Some(time.timestamp_millis());
    }
    let time = NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S").ok()?;
    FixedOffset::east_opt(offset_hours.checked_mul(3600)?)?
        .from_local_datetime(&time)
        .single()
        .map(|time| time.timestamp_millis())
}

fn parse_true_downloaded(html: &str) -> Option<u64> {
    let document = Html::parse_document(html);
    let selector = Selector::parse("#downloaded-value span[data-tooltip]").ok()?;
    let tooltip = document
        .select(&selector)
        .next()?
        .value()
        .attr("data-tooltip")?;
    let value = tooltip.split(',').nth(1)?;
    super::nexusphp::parse_labeled_size(&format!("size: {value}"), &["size"])
}

impl SiteAdapter for GazelleAdapter {
    fn get_user_stats(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<UserStats, String>> + Send + '_>> {
        Box::pin(self.fetch_stats())
    }
    fn get_torrent_attributes(
        &self,
        _detail_url: &str,
    ) -> Pin<Box<dyn Future<Output = Result<TorrentAttributes, String>> + Send + '_>> {
        Box::pin(async {
            Err("Gazelle 当前仅支持用户统计，尚未支持种子属性获取".to_string())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn gazelle_preserves_bytes_infinite_ratio_and_extended_stats() {
        let index = json!({"id":42,"username":"alice","notifications":{"messages":0},"userstats":{
            "uploaded":59388701716u64,"downloaded":0,"ratio":-1,"class":"User",
            "bonusPoints":6388.65,"seedingSize":15396889333u64,"seedingBonusPointsPerHour":0.7445}});
        let user = json!({"username":"alice","stats":{"joinedDate":"2026-06-25 22:44:23"},
            "community":{"seeding":1,"uploaded":0,"groups":0,"invited":0}});
        let stats = parse_stats(&index, &user, 8).unwrap();
        assert_eq!(stats.uploaded, 59388701716);
        assert_eq!(stats.ratio, Some(-1.0));
        assert_eq!(stats.details.join_time, Some(1782398663000));
        assert_eq!(stats.details.bonus_per_hour, Some(0.7445));
        assert_eq!(stats.details.extra["groups"], 0);
        let mut invalid = index.clone();
        invalid["userstats"]
            .as_object_mut()
            .unwrap()
            .remove("uploaded");
        assert!(parse_stats(&invalid, &user, 8).is_err());
        let mut wrong_user = user.clone();
        wrong_user["username"] = json!("bob");
        assert!(parse_stats(&index, &wrong_user, 8).is_err());
    }

    #[test]
    fn gpw_true_download_comes_from_the_profile_tooltip() {
        let html = r#"<span id="downloaded-value"><span data-tooltip="总计 0 B,实际下载 25.18 GiB">0 B</span></span>"#;
        assert_eq!(
            parse_true_downloaded(html),
            Some((25.18 * 1073741824.0) as u64)
        );
        assert_eq!(parse_true_downloaded("<html>Login</html>"), None);
    }
}
