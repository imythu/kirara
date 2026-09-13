//! User-defined HTTP automations. Lists and execution records omit credentials;
//! saved request configuration is disclosed only through an explicit no-store endpoint.
use crate::db::Database;
use chrono::{DateTime, FixedOffset, Utc};
use serde::{Deserialize, Serialize};
use std::{str::FromStr, sync::Arc, time::Duration};

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum Timing {
    Interval {
        minutes: u32,
    },
    Cron {
        expression: String,
        utc_offset_minutes: i32,
    },
}
impl Timing {
    pub fn next(&self, after: DateTime<Utc>) -> Result<DateTime<Utc>, String> {
        match self {
            Self::Interval { minutes } => {
                if !(1..=525600).contains(minutes) {
                    return Err("间隔应为 1 分钟至 365 天".into());
                }
                Ok(after + chrono::Duration::minutes(i64::from(*minutes)))
            }
            Self::Cron {
                expression,
                utc_offset_minutes,
            } => {
                if !(-720..=840).contains(utc_offset_minutes) {
                    return Err("无效的时区偏移".into());
                }
                let mut fields = expression
                    .split_whitespace()
                    .map(str::to_string)
                    .collect::<Vec<_>>();
                if fields.len() != 5 {
                    return Err("Cron 需要 5 个字段：分 时 日 月 星期".into());
                }
                let offset = FixedOffset::east_opt(utc_offset_minutes * 60).ok_or("无效的时区")?;
                let both_days = !fields[2].starts_with('*') && !fields[4].starts_with('*');
                fields[4] = normalize_weekdays(&fields[4])?;
                let next = |parts: &[String]| -> Result<Option<DateTime<Utc>>, String> {
                    let schedule = cron::Schedule::from_str(&format!("0 {}", parts.join(" ")))
                        .map_err(|_| "Cron 表达式无效，请检查各字段范围".to_string())?;
                    Ok(schedule
                        .after(&after.with_timezone(&offset))
                        .next()
                        .map(|v| v.with_timezone(&Utc)))
                };
                // Traditional five-field cron uses OR when both day fields are restricted.
                let result = if both_days {
                    let mut by_day = fields.clone();
                    by_day[4] = "*".into();
                    let mut by_weekday = fields;
                    by_weekday[2] = "*".into();
                    next(&by_day)?.into_iter().chain(next(&by_weekday)?).min()
                } else {
                    next(&fields)?
                };
                result.ok_or("此 Cron 没有未来执行时间".into())
            }
        }
    }
}
// Adapt standard cron weekdays (0/7 = Sunday) to the crate (1 = Sunday).
fn normalize_weekdays(value: &str) -> Result<String, String> {
    let error =
        || "Cron 星期字段无效，支持 0–7（0/7 为周日）、MON–SUN、范围、列表和步长".to_string();
    let number = |s: &str| -> Result<u32, String> {
        match s.to_uppercase().as_str() {
            "SUN" => Ok(0),
            "MON" => Ok(1),
            "TUE" => Ok(2),
            "WED" => Ok(3),
            "THU" => Ok(4),
            "FRI" => Ok(5),
            "SAT" => Ok(6),
            _ => s.parse::<u32>().ok().filter(|v| *v <= 7).ok_or_else(error),
        }
    };
    let mut days = std::collections::BTreeSet::new();
    for item in value.split(',') {
        let (range, step) = match item.split_once('/') {
            Some((range, step)) => (
                range,
                step.parse::<usize>()
                    .ok()
                    .filter(|s| *s > 0 && *s <= 7)
                    .ok_or_else(error)?,
            ),
            None => (item, 1),
        };
        let (start, end) = if range == "*" {
            (0, 6)
        } else if let Some((a, b)) = range.split_once('-') {
            (number(a)?, number(b)?)
        } else {
            let n = number(range)?;
            (n, if item.contains('/') { 7 } else { n })
        };
        if start > end {
            return Err(error());
        }
        for day in (start..=end).step_by(step) {
            days.insert(day % 7 + 1);
        }
    }
    Ok(days
        .into_iter()
        .map(|d| d.to_string())
        .collect::<Vec<_>>()
        .join(","))
}
mod browser;
mod http;
pub use http::HttpConfig;
#[cfg(test)]
use http::Pair;
pub use http::request_preview;
#[derive(Clone, Deserialize)]
pub struct TaskRequest {
    pub name: String,
    pub task_type: String,
    pub enabled: bool,
    pub timing: Timing,
    pub http: Option<HttpConfig>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: i64,
    pub name: String,
    pub task_type: String,
    pub enabled: bool,
    pub timing: Timing,
    pub request_summary: String,
    pub next_run_at: Option<String>,
    pub running: bool,
    pub last_run: Option<Run>,
    #[serde(skip)]
    pub http: Option<HttpConfig>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Run {
    pub id: i64,
    pub task_id: i64,
    pub trigger: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub status: String,
    pub status_code: Option<u16>,
    pub duration_ms: Option<u64>,
    pub message: String,
}
#[derive(Clone)]
pub struct Scheduler {
    pub db: Database,
    slots: Arc<tokio::sync::Semaphore>,
}
impl Scheduler {
    pub fn new(db: Database) -> Arc<Self> {
        Arc::new(Self {
            db,
            slots: Arc::new(tokio::sync::Semaphore::new(8)),
        })
    }
    pub async fn start(self: Arc<Self>, shutdown: tokio_util::sync::CancellationToken) {
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        loop {
            tokio::select! { _ = shutdown.cancelled() => break, _ = tick.tick() => {} }
            match self.db.list_scheduled_tasks().await {
                Ok(tasks) => {
                    for task in tasks {
                        if task.enabled
                            && !task.running
                            && task
                                .next_run_at
                                .as_deref()
                                .is_some_and(|v| v <= Utc::now().to_rfc3339().as_str())
                        {
                            if let Err(e) = self.trigger(task.id, false).await {
                                tracing::debug!("scheduled task claim: {e}");
                            }
                        }
                    }
                }
                Err(e) => tracing::error!("scheduled task scan: {e}"),
            }
        }
    }
    pub async fn trigger(self: &Arc<Self>, id: i64, manual: bool) -> Result<(), String> {
        let permit = self
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| "已有 8 个任务执行中，请稍后重试".to_string())?;
        let (task, run) = self
            .db
            .claim_scheduled_task(id, manual)
            .await
            .map_err(|e| e.to_string())?;
        let db = self.db.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let start = std::time::Instant::now();
            let config = task.http.expect("stored HTTP config");
            let result = match db.get_settings().await {
                Ok(settings) => execute_with_settings(&config, &settings).await,
                Err(_) => Err("无法读取全局代理和浏览器配置".into()),
            };
            let (status_code, status, message) = match result {
                Ok(code) => {
                    let ok = config
                        .expected_status
                        .map_or((200..300).contains(&code), |expected| code == expected);
                    (
                        Some(code),
                        if ok { "success" } else { "failed" },
                        if ok {
                            format!("HTTP {code} · 符合预期")
                        } else {
                            format!("HTTP {code} · 未达到预期状态码")
                        },
                    )
                }
                Err(message) => (None, "failed", message),
            };
            if let Err(error) = db
                .finish_scheduled_run(
                    run.id,
                    task.id,
                    status.into(),
                    status_code,
                    start.elapsed().as_millis() as u64,
                    message,
                )
                .await
            {
                tracing::error!("scheduled task record: {error}");
            }
        });
        Ok(())
    }
}
pub fn validate_delivery(
    config: &HttpConfig,
    settings: &crate::config::GlobalConfig,
) -> Result<(), String> {
    let uses_proxy = if config.send_via_browser {
        config.browser_use_global_proxy
    } else {
        config.use_global_proxy
    };
    if uses_proxy && settings.effective_proxy(true).is_none() {
        return Err("已启用全局代理，请先在系统设置中填写代理地址".into());
    }
    if uses_proxy {
        reqwest::Proxy::all(settings.effective_proxy(true).unwrap())
            .map_err(|_| "全局代理地址无效".to_string())?;
    }
    if config.send_via_browser {
        browser::endpoint(&settings.browserless)?;
    }
    Ok(())
}
#[cfg(test)]
async fn execute(config: &HttpConfig) -> Result<u16, String> {
    execute_with_settings(config, &crate::config::GlobalConfig::default()).await
}
async fn execute_with_settings(
    config: &HttpConfig,
    settings: &crate::config::GlobalConfig,
) -> Result<u16, String> {
    validate_delivery(config, settings)?;
    if config.send_via_browser {
        return browser::execute(config, settings).await;
    }
    let mut builder = reqwest::Client::builder().no_proxy();
    if let Some(proxy) = settings.effective_proxy(config.use_global_proxy) {
        builder =
            builder.proxy(reqwest::Proxy::all(proxy).map_err(|_| "全局代理地址无效".to_string())?);
    }
    let client = builder
        .timeout(Duration::from_secs(config.timeout_seconds))
        .redirect(if config.follow_redirects {
            reqwest::redirect::Policy::limited(5)
        } else {
            reqwest::redirect::Policy::none()
        })
        .build()
        .map_err(|_| "HTTP 客户端初始化失败".to_string())?;
    let request = config.build_request(&client)?;
    // Never log URLs, response bodies, or reqwest errors, which may expose credentials.
    let response = client.execute(request).await.map_err(|e| {
        if e.is_timeout() {
            "请求超时，请检查服务或增加超时时间"
        } else if e.is_connect() {
            "连接失败，请检查地址、网络和 TLS 证书"
        } else if e.is_redirect() {
            "重定向次数超过 5 次"
        } else {
            "请求失败，请检查网络与请求配置"
        }
        .to_string()
    })?;
    Ok(response.status().as_u16())
}

#[cfg(test)]
mod tests;
