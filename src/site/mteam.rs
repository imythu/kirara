use chrono::{DateTime, FixedOffset, NaiveDateTime, TimeZone};
use reqwest::Client;
use reqwest::header::{HeaderMap, HeaderValue};
use serde_json::Value;
use tracing::{debug, trace, warn};

use super::{SiteAdapter, SiteAuth, TorrentAttributes, UserStats, UserStatsDetails};
use std::future::Future;
use std::pin::Pin;

const MTEAM_DEFAULT_API: &str = "https://api.m-team.cc";
/// M-Team 详情请求间最小间隔（毫秒），防止 API 限流
const MTEAM_REQUEST_INTERVAL_MS: u64 = 4000;

pub struct MTeamAdapter {
    base_url: String,
    api_key: String,
    request_headers: HeaderMap,
    client: Client,
}

impl MTeamAdapter {
    pub fn new(
        base_url: String,
        auth: SiteAuth,
        request_headers: HeaderMap,
        client: Client,
    ) -> Self {
        let api_key = match &auth {
            SiteAuth::ApiKey { api_key } => api_key.clone(),
            _ => String::new(),
        };
        let url = if base_url.is_empty() {
            MTEAM_DEFAULT_API.to_string()
        } else {
            base_url.trim_end_matches('/').to_string()
        };
        Self {
            base_url: url,
            api_key,
            request_headers,
            client,
        }
    }

    fn build_headers(&self) -> HeaderMap {
        let mut headers = self.request_headers.clone();
        if let Ok(val) = HeaderValue::from_str(&self.api_key) {
            headers.insert("x-api-key", val);
        }
        headers
    }

    async fn api_post(&self, path: &str, body: Option<&Value>) -> Result<Value, String> {
        let url = format!("{}{}", self.base_url, path);
        debug!("M-Team API POST: {}", url);

        let mut req = self.client.post(&url).headers(self.build_headers());
        if let Some(body) = body {
            req = req.json(body);
        }

        let resp = req.send().await.map_err(|e| format!("请求失败: {}", e))?;

        if !resp.status().is_success() {
            return Err(format!("HTTP {}", resp.status()));
        }

        let text = resp
            .text()
            .await
            .map_err(|e| format!("读取响应失败: {}", e))?;

        let json: Value =
            serde_json::from_str(&text).map_err(|_| "响应不是有效JSON".to_string())?;

        let code = json.get("code");
        if !api_response_succeeded(&json) {
            let msg = json
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error");
            return Err(format!(
                "API错误 code={}: {}",
                code.map(Value::to_string)
                    .unwrap_or_else(|| "missing".to_string()),
                msg
            ));
        }

        Ok(json)
    }

    /// 使用 form data 发送 POST 请求（M-Team 部分接口需要）
    async fn api_post_form(&self, path: &str, form: &[(&str, &str)]) -> Result<Value, String> {
        let url = format!("{}{}", self.base_url, path);
        debug!("M-Team API POST form: {} {:?}", url, form);

        let resp = self
            .client
            .post(&url)
            .headers(self.build_headers())
            .form(form)
            .send()
            .await
            .map_err(|e| format!("请求失败: {}", e))?;

        if !resp.status().is_success() {
            return Err(format!("HTTP {}", resp.status()));
        }

        let text = resp
            .text()
            .await
            .map_err(|e| format!("读取响应失败: {}", e))?;

        if path == "/api/torrent/detail" {
            trace!("M-Team torrent detail response: {}", text);
        }

        let json: Value =
            serde_json::from_str(&text).map_err(|_| "响应不是有效JSON".to_string())?;

        let code = json.get("code");
        if !api_response_succeeded(&json) {
            let msg = json
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error");
            return Err(format!(
                "API错误 code={}: {}",
                code.map(Value::to_string)
                    .unwrap_or_else(|| "missing".to_string()),
                msg
            ));
        }

        Ok(json)
    }

    fn extract_torrent_id(detail_url: &str) -> Option<String> {
        let mut current = String::new();
        let mut last = None;
        for ch in detail_url.chars() {
            if ch.is_ascii_digit() {
                current.push(ch);
            } else if !current.is_empty() {
                last = Some(current.clone());
                current.clear();
            }
        }
        if !current.is_empty() {
            last = Some(current);
        }
        last
    }

    fn parse_discount(discount: Option<&str>) -> (Option<f64>, Option<f64>) {
        // 取值对应 M-Team API 的 discount 枚举（见 /api/v3/api-docs）：
        // NORMAL, PERCENT_70, PERCENT_50, FREE, _2X_FREE, _2X, _2X_PERCENT_50
        // 返回 (下载系数, 上传系数)。
        match discount.unwrap_or_default() {
            "FREE" => (Some(0.0), Some(1.0)),
            "_2X_FREE" | "FREE_2XUP" | "TWOFREE" => (Some(0.0), Some(2.0)),
            "_2X" | "TWOUP" => (Some(1.0), Some(2.0)),
            "PERCENT_50" => (Some(0.5), Some(1.0)),
            "_2X_PERCENT_50" | "PERCENT_50_2XUP" => (Some(0.5), Some(2.0)),
            "PERCENT_70" => (Some(0.3), Some(1.0)),
            "_2X_PERCENT_70" | "PERCENT_70_2XUP" => (Some(0.3), Some(2.0)),
            "NORMAL" | "" => (Some(1.0), Some(1.0)),
            other => {
                debug!("未识别的 M-Team 促销类型: {:?}", other);
                (Some(1.0), Some(1.0))
            }
        }
    }
}

impl SiteAdapter for MTeamAdapter {
    fn get_user_stats(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<UserStats, String>> + Send + '_>> {
        Box::pin(async move {
            let json = self.api_post("/api/member/profile", None).await?;
            let data = json
                .get("data")
                .ok_or_else(|| "响应缺少 data 字段".to_string())?;

            let username = data
                .get("username")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty() && !value.eq_ignore_ascii_case("unknown"))
                .ok_or_else(|| "响应缺少用户名".to_string())?
                .to_string();

            let uid = data
                .get("id")
                .or_else(|| data.get("uid"))
                .or_else(|| data.get("userId"))
                .or_else(|| data.get("memberId"))
                .and_then(json_value_to_string);

            let member_count = data.get("memberCount").unwrap_or(data);

            let uploaded = member_count
                .get("uploaded")
                .and_then(json_value_to_u64)
                .ok_or_else(|| "响应缺少上传量".to_string())?;

            let downloaded = member_count
                .get("downloaded")
                .and_then(json_value_to_u64)
                .ok_or_else(|| "响应缺少下载量".to_string())?;

            let ratio = member_count
                .get("shareRate")
                .or_else(|| member_count.get("ratio"))
                .and_then(json_value_to_f64)
                .or_else(|| (downloaded > 0).then(|| uploaded as f64 / downloaded as f64));

            // PT-Depiler 当前定义使用 data.memberCount.bonus；旧响应也可能放在 data.bonus。
            let bonus = member_count
                .get("bonus")
                .or_else(|| data.get("bonus"))
                .and_then(json_value_to_f64);

            let mut details = UserStatsDetails {
                level_id: data.get("role").and_then(json_value_to_i64),
                level_name: data.get("role").and_then(mteam_level_name),
                join_time: data
                    .get("createdDate")
                    .and_then(json_value_to_timestamp_millis),
                last_access_at: data
                    .get("memberStatus")
                    .and_then(|status| status.get("lastBrowse"))
                    .and_then(json_value_to_timestamp_millis),
                ..Default::default()
            };

            let mut seeding_count = member_count
                .get("seeding")
                .or_else(|| member_count.get("seederCount"))
                .and_then(json_value_to_u32);

            let mut leeching_count = member_count
                .get("leeching")
                .or_else(|| member_count.get("leecherCount"))
                .and_then(json_value_to_u32);

            // M-Team 将活动做种、时魔和未读消息拆到了独立接口。它们不是登录校验
            // 的必要条件，因此单个附加接口失败时保留其余统计结果，等待下次定时同步。
            match self.api_post("/api/tracker/myPeerStatistics", None).await {
                Ok(peer_json) => {
                    if let Some(peer_data) = peer_json.get("data") {
                        seeding_count = peer_data
                            .get("seederCount")
                            .and_then(json_value_to_u32)
                            .or(seeding_count);
                        leeching_count = peer_data
                            .get("leecherCount")
                            .and_then(json_value_to_u32)
                            .or(leeching_count);
                        details.seeding_size =
                            peer_data.get("seederSize").and_then(json_value_to_u64);
                        details.uploads = peer_data.get("uploadCount").and_then(json_value_to_u64);
                    }
                }
                Err(error) => warn!(%error, "M-Team 做种统计接口获取失败"),
            }

            match self.api_post("/api/tracker/mybonus", None).await {
                Ok(bonus_json) => {
                    details.bonus_per_hour = bonus_json
                        .get("data")
                        .and_then(|bonus_data| bonus_data.get("formulaParams"))
                        .and_then(|params| params.get("finalBs"))
                        .and_then(json_value_to_f64);
                }
                Err(error) => warn!(%error, "M-Team 时魔接口获取失败"),
            }

            match self.api_post("/api/msg/notify/statistic", None).await {
                Ok(message_json) => {
                    details.message_count = message_json
                        .get("data")
                        .and_then(|message_data| message_data.get("unMake"))
                        .and_then(json_value_to_u64);
                }
                Err(error) => warn!(%error, "M-Team 消息统计接口获取失败"),
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
        })
    }

    fn get_torrent_attributes(
        &self,
        detail_url: &str,
    ) -> Pin<Box<dyn Future<Output = Result<TorrentAttributes, String>> + Send + '_>> {
        let detail_url = detail_url.to_string();
        Box::pin(async move {
            let torrent_id = Self::extract_torrent_id(&detail_url)
                .ok_or_else(|| format!("无法从链接提取 M-Team 种子 ID: {detail_url}"))?;

            // 首次请求前加延迟，防止并发触发限流
            let delay_ms = MTEAM_REQUEST_INTERVAL_MS + simple_random_ms(4000);
            tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;

            let form = [("id", torrent_id.as_str())];
            match self.api_post_form("/api/torrent/detail", &form).await {
                Ok(json) => {
                    let data = json
                        .get("data")
                        .ok_or_else(|| "响应缺少 data 字段".to_string())?;
                    let status = data.get("status").unwrap_or(data);
                    let discount = status.get("discount").and_then(|v| v.as_str());
                    let (download_volume_factor, upload_volume_factor) =
                        Self::parse_discount(discount);
                    let seeder_count = status.get("seeders").and_then(|v| {
                        v.as_str()
                            .and_then(|s| s.parse::<i32>().ok())
                            .or_else(|| v.as_i64().map(|n| n as i32))
                    });
                    // M-Team status 中下载人数字段为 leechers（字符串或整数）。
                    let leecher_count = status
                        .get("leechers")
                        .or_else(|| status.get("leecher"))
                        .and_then(|v| {
                            v.as_str()
                                .and_then(|s| s.parse::<i32>().ok())
                                .or_else(|| v.as_i64().map(|n| n as i32))
                        });
                    let free_end_timestamp = status
                        .get("discountEndTime")
                        .or_else(|| data.get("discountEndTime"))
                        .and_then(|v| v.as_str())
                        .and_then(parse_mteam_datetime);

                    Ok(TorrentAttributes {
                        free: download_volume_factor == Some(0.0),
                        two_x_free: download_volume_factor == Some(0.0)
                            && upload_volume_factor.is_some_and(|factor| factor >= 2.0),
                        hit_and_run: false,
                        seeder_count,
                        leecher_count,
                        free_end_timestamp,
                        download_volume_factor,
                        upload_volume_factor,
                    })
                }
                Err(e) => {
                    if e.contains("頻繁") {
                        warn!("M-Team 限流，跳过当前条目: {} {}", &detail_url, e);
                        return Err(e);
                    }
                    Err(e)
                }
            }
        })
    }
}

/// 简单的伪随机数生成，基于当前时间纳秒，返回 [0, max_ms) 范围内的毫秒数。
/// 仅用于请求抖动，不需要密码学强度的随机性。
fn simple_random_ms(max_ms: u64) -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    // 折叠高低位增加低位熵
    let seed = nanos ^ (nanos >> 32);
    if max_ms == 0 { 0 } else { seed % max_ms }
}

fn json_value_to_string(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::to_string)
        .or_else(|| value.as_u64().map(|n| n.to_string()))
        .or_else(|| value.as_i64().map(|n| n.to_string()))
}

fn api_response_succeeded(value: &Value) -> bool {
    value.get("code").is_some_and(|code| {
        code.as_i64() == Some(0)
            || code
                .as_str()
                .is_some_and(|code| code == "0" || code.eq_ignore_ascii_case("SUCCESS"))
    })
}

fn json_value_to_u64(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_i64().and_then(|number| u64::try_from(number).ok()))
        .or_else(|| {
            let value = value.as_str()?.trim().replace(',', "");
            value.parse::<u64>().ok().or_else(|| {
                value
                    .parse::<f64>()
                    .ok()
                    .filter(|number| number.is_finite() && *number >= 0.0)
                    .map(|number| number.min(u64::MAX as f64) as u64)
            })
        })
}

fn json_value_to_i64(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|number| i64::try_from(number).ok()))
        .or_else(|| value.as_str()?.trim().replace(',', "").parse::<i64>().ok())
}

fn json_value_to_u32(value: &Value) -> Option<u32> {
    json_value_to_u64(value).and_then(|number| u32::try_from(number).ok())
}

fn json_value_to_f64(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str()?.trim().replace(',', "").parse::<f64>().ok())
}

fn json_value_to_timestamp_millis(value: &Value) -> Option<i64> {
    if let Some(timestamp) = json_value_to_i64(value) {
        return normalize_timestamp_millis(timestamp);
    }
    let value = value.as_str()?.trim();
    DateTime::parse_from_rfc3339(value)
        .map(|datetime| datetime.timestamp_millis())
        .ok()
        .or_else(|| {
            NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S")
                .ok()
                .and_then(|naive| {
                    FixedOffset::east_opt(8 * 3600)?
                        .from_local_datetime(&naive)
                        .single()
                        .map(|datetime| datetime.timestamp_millis())
                })
        })
}

fn normalize_timestamp_millis(timestamp: i64) -> Option<i64> {
    if timestamp.unsigned_abs() < 100_000_000_000 {
        timestamp.checked_mul(1000)
    } else {
        Some(timestamp)
    }
}

fn mteam_level_name(value: &Value) -> Option<String> {
    let level_id = json_value_to_i64(value)?;
    let name = match level_id {
        1 => "User",
        2 => "Power User",
        3 => "Elite User",
        4 => "Crazy User",
        5 => "Insane User",
        6 => "Veteran User",
        7 => "Extreme User",
        8 => "Ultimate User",
        9 => "mTorrent Master",
        10 => "VIP",
        _ => return Some(value.to_string().trim_matches('"').to_string()),
    };
    Some(name.to_string())
}

fn parse_mteam_datetime(value: &str) -> Option<i64> {
    let naive = NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S").ok()?;
    let tz = FixedOffset::east_opt(8 * 3600)?;
    tz.from_local_datetime(&naive)
        .single()
        .map(|dt| dt.timestamp())
}

#[cfg(test)]
mod tests {
    use reqwest::Client;
    use reqwest::header::{HeaderMap, HeaderValue};
    use serde_json::json;

    use super::{
        MTeamAdapter, SiteAuth, api_response_succeeded, json_value_to_timestamp_millis,
        mteam_level_name,
    };

    #[test]
    fn custom_headers_are_applied_without_overriding_api_key() {
        let mut custom = HeaderMap::new();
        custom.insert("x-browser-profile", HeaderValue::from_static("desktop"));
        custom.insert("x-api-key", HeaderValue::from_static("stale"));
        let adapter = MTeamAdapter::new(
            "https://api.m-team.cc".to_string(),
            SiteAuth::ApiKey {
                api_key: "current".to_string(),
            },
            custom,
            Client::new(),
        );

        let headers = adapter.build_headers();
        assert_eq!(headers["x-browser-profile"], "desktop");
        assert_eq!(headers["x-api-key"], "current");
    }

    #[test]
    fn parse_discount_maps_current_api_enum() {
        // 对应 swagger 枚举：NORMAL, PERCENT_70, PERCENT_50, FREE, _2X_FREE, _2X, _2X_PERCENT_50
        assert_eq!(
            MTeamAdapter::parse_discount(Some("NORMAL")),
            (Some(1.0), Some(1.0))
        );
        assert_eq!(
            MTeamAdapter::parse_discount(Some("FREE")),
            (Some(0.0), Some(1.0))
        );
        assert_eq!(
            MTeamAdapter::parse_discount(Some("_2X_FREE")),
            (Some(0.0), Some(2.0))
        );
        assert_eq!(
            MTeamAdapter::parse_discount(Some("_2X")),
            (Some(1.0), Some(2.0))
        );
        assert_eq!(
            MTeamAdapter::parse_discount(Some("PERCENT_50")),
            (Some(0.5), Some(1.0))
        );
        assert_eq!(
            MTeamAdapter::parse_discount(Some("_2X_PERCENT_50")),
            (Some(0.5), Some(2.0))
        );
        assert_eq!(
            MTeamAdapter::parse_discount(Some("PERCENT_70")),
            (Some(0.3), Some(1.0))
        );
        // 缺省与未知都按原价处理
        assert_eq!(MTeamAdapter::parse_discount(None), (Some(1.0), Some(1.0)));
        assert_eq!(
            MTeamAdapter::parse_discount(Some("WHATEVER")),
            (Some(1.0), Some(1.0))
        );
    }

    #[test]
    fn extract_torrent_id_takes_last_number() {
        assert_eq!(
            MTeamAdapter::extract_torrent_id("https://kp.m-team.cc/detail/1165802"),
            Some("1165802".to_string())
        );
    }

    #[test]
    fn accepts_string_numeric_and_named_success_codes() {
        assert!(api_response_succeeded(&json!({ "code": "0" })));
        assert!(api_response_succeeded(&json!({ "code": 0 })));
        assert!(api_response_succeeded(&json!({ "code": "SUCCESS" })));
        assert!(!api_response_succeeded(&json!({ "code": "1" })));
        assert!(!api_response_succeeded(&json!({ "data": {} })));
    }

    #[test]
    fn parses_pt_depiler_mteam_profile_fields() {
        assert_eq!(
            mteam_level_name(&json!(9)).as_deref(),
            Some("mTorrent Master")
        );
        assert_eq!(mteam_level_name(&json!(10)).as_deref(), Some("VIP"));
        assert_eq!(
            json_value_to_timestamp_millis(&json!("2026-09-05T12:00:00+08:00")),
            Some(1_788_580_800_000)
        );
        assert_eq!(
            json_value_to_timestamp_millis(&json!(1_788_580_800)),
            Some(1_788_580_800_000)
        );
    }
}
