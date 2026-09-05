pub mod scheduler;

use std::net::{Shutdown, TcpStream};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Utc;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use tungstenite::client::IntoClientRequest;
use tungstenite::{HandshakeError, Message, client_tls};

use crate::config::{BrowserlessConfig, GlobalConfig, LightpandaConfig};
use crate::site::{SiteAuth, SiteRecord};

pub const SIGN_IN_BROWSER_LIGHTPANDA: &str = "lightpanda";
pub const SIGN_IN_BROWSER_BROWSERLESS: &str = "browserless";

pub const BROWSERLESS_CF_MODE_AUTO: &str = "auto";
pub const BROWSERLESS_CF_MODE_PAGE: &str = "page";
pub const BROWSERLESS_CF_MODE_TURNSTILE: &str = "turnstile";
pub const DEFAULT_BROWSERLESS_SELECTOR: &str = "input[type='submit']";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserlessTaskConfig {
    #[serde(default = "default_browserless_selector")]
    pub selector: String,
    #[serde(default = "default_browserless_cf_mode")]
    pub cf_mode: String,
    #[serde(default)]
    pub wait_ms: Option<u64>,
    #[serde(default)]
    pub solve_timeout: Option<u64>,
    #[serde(default)]
    pub action_timeout: Option<u64>,
    #[serde(default)]
    pub post_click_wait_ms: Option<u64>,
}

impl Default for BrowserlessTaskConfig {
    fn default() -> Self {
        Self {
            selector: default_browserless_selector(),
            cf_mode: default_browserless_cf_mode(),
            wait_ms: None,
            solve_timeout: None,
            action_timeout: None,
            post_click_wait_ms: None,
        }
    }
}

fn default_browserless_selector() -> String {
    DEFAULT_BROWSERLESS_SELECTOR.to_string()
}

fn default_browserless_cf_mode() -> String {
    BROWSERLESS_CF_MODE_AUTO.to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignInTaskRecord {
    pub id: i64,
    pub name: String,
    pub site_id: i64,
    pub cron_expression: String,
    pub browser: String,
    pub sign_in_method: String,
    pub browserless: BrowserlessTaskConfig,
    pub enabled: bool,
    pub last_status: Option<String>,
    pub last_message: Option<String>,
    pub last_run_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignInTaskRequest {
    pub name: String,
    pub site_id: i64,
    pub cron_expression: String,
    pub browser: Option<String>,
    pub sign_in_method: Option<String>,
    #[serde(default)]
    pub browserless: Option<BrowserlessTaskConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignInRecord {
    pub id: i64,
    pub task_id: i64,
    pub site_id: i64,
    pub site_name: String,
    pub started_at: String,
    pub finished_at: String,
    pub status: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignInResult {
    pub status: String,
    pub message: String,
    pub started_at: String,
    pub finished_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserProbeResult {
    pub success: bool,
    pub url: String,
    pub message: String,
    pub title: Option<String>,
}

pub async fn probe_browser_1_1_1_1(
    browser: String,
    settings: GlobalConfig,
) -> Result<BrowserProbeResult, String> {
    match browser.as_str() {
        SIGN_IN_BROWSER_LIGHTPANDA => {
            let endpoint =
                build_lightpanda_endpoint(&settings.lightpanda, settings.use_proxy_for_lightpanda)?;
            tokio::task::spawn_blocking(move || {
                run_cdp_probe(endpoint, "https://1.1.1.1", "Lightpanda")
            })
            .await
            .map_err(|e| format!("Lightpanda 探测任务 join 失败: {}", e))?
        }
        SIGN_IN_BROWSER_BROWSERLESS => run_browserless_probe(&settings.browserless).await,
        _ => Err(format!("未知签到浏览器: {}", browser)),
    }
}

pub async fn execute_task(
    _base_dir: std::path::PathBuf,
    task: SignInTaskRecord,
    site: SiteRecord,
    settings: GlobalConfig,
) -> Result<SignInResult, String> {
    if site.site_type != "nexusphp" && site.site_type != "nexus_php" {
        return Err("自动签到目前仅支持 NexusPHP 站点".to_string());
    }

    let auth = serde_json::from_str::<SiteAuth>(&site.auth_config)
        .map_err(|e| format!("认证配置解析失败: {}", e))?;
    let cookie = match auth {
        SiteAuth::Cookie { cookie } | SiteAuth::CookiePasskey { cookie, .. } => cookie,
        _ => return Err("NexusPHP 自动签到需要 Cookie 认证".to_string()),
    };
    if cookie.trim().is_empty() {
        return Err("Cookie 不能为空".to_string());
    }

    let base_url = site.base_url.trim_end_matches('/').to_string();
    let started_at = Utc::now().to_rfc3339();
    let output = match task.browser.as_str() {
        SIGN_IN_BROWSER_LIGHTPANDA => {
            let endpoint =
                build_lightpanda_endpoint(&settings.lightpanda, settings.use_proxy_for_lightpanda)?;
            run_cdp_sign_in(
                endpoint,
                base_url,
                cookie,
                task.sign_in_method,
                settings.ocr_api_key,
            )
            .await?
        }
        SIGN_IN_BROWSER_BROWSERLESS => {
            run_browserless_sign_in(&settings.browserless, base_url, cookie, task.browserless)
                .await?
        }
        _ => return Err(format!("未知签到浏览器: {}", task.browser)),
    };
    let finished_at = Utc::now().to_rfc3339();

    Ok(SignInResult {
        status: output.status,
        message: output.message,
        started_at,
        finished_at,
    })
}

fn run_cdp_probe(
    endpoint: String,
    url: &str,
    browser_name: &str,
) -> Result<BrowserProbeResult, String> {
    let mut client = CdpClient::connect(endpoint)?;
    let session_id = create_target_session(&mut client)?;

    let _ = client.call("Page.enable", json!({}), Some(&session_id));
    let navigate = client.call("Page.navigate", json!({ "url": url }), Some(&session_id));
    if let Err(error) = navigate {
        return Ok(BrowserProbeResult {
            success: false,
            url: url.to_string(),
            message: format!("Page.navigate 失败: {}", error),
            title: None,
        });
    }

    client.wait(Duration::from_secs(2))?;
    let title = client
        .call(
            "Runtime.evaluate",
            json!({
                "expression": "document.title",
                "returnByValue": true
            }),
            Some(&session_id),
        )
        .ok()
        .and_then(|value| {
            value
                .get("result")
                .and_then(|v| v.get("value"))
                .and_then(Value::as_str)
                .map(str::to_string)
        });

    Ok(BrowserProbeResult {
        success: true,
        url: url.to_string(),
        message: format!("{} 已成功导航到 1.1.1.1", browser_name),
        title,
    })
}

const BROWSERLESS_SIGN_IN_QUERY: &str = r#"
mutation CheckIn(
  $cookies: [CookieInput!]!
  $url: String!
  $selector: String!
  $waitMs: Float!
  $solveTimeout: Float!
  $actionTimeout: Float!
  $postClickWaitMs: Float!
) {
  setCookie: cookies(cookies: $cookies) {
    cookies {
      name
      domain
      path
      secure
    }
  }

  goto(url: $url, waitUntil: networkIdle) {
    status
  }

  waitBeforeSolve: waitForTimeout(time: $waitMs) {
    time
  }

  solve(type: cloudflare, timeout: $solveTimeout) {
    found
    solved
    time
  }

  waitForSelector(
    selector: $selector
    visible: true
    timeout: $actionTimeout
  ) {
    time
  }

  click(
    selector: $selector
    visible: true
    timeout: $actionTimeout
  ) {
    time
  }

  waitAfterClick: waitForTimeout(time: $postClickWaitMs) {
    time
  }

  html {
    html
  }
}
"#;

const BROWSERLESS_PROBE_QUERY: &str = r#"
mutation Probe($url: String!) {
  goto(url: $url, waitUntil: networkIdle) {
    status
  }
  html {
    html
  }
}
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BrowserlessTimings {
    wait_ms: u64,
    solve_timeout: u64,
    action_timeout: u64,
    post_click_wait_ms: u64,
}

fn browserless_mode_defaults(cf_mode: &str) -> Option<BrowserlessTimings> {
    match normalize_browserless_cf_mode(cf_mode)? {
        BROWSERLESS_CF_MODE_AUTO | BROWSERLESS_CF_MODE_TURNSTILE => Some(BrowserlessTimings {
            wait_ms: 5_000,
            solve_timeout: 60_000,
            action_timeout: 30_000,
            post_click_wait_ms: 5_000,
        }),
        BROWSERLESS_CF_MODE_PAGE => Some(BrowserlessTimings {
            wait_ms: 1_000,
            solve_timeout: 30_000,
            action_timeout: 30_000,
            post_click_wait_ms: 3_000,
        }),
        _ => None,
    }
}

fn resolve_browserless_timings(
    config: &BrowserlessTaskConfig,
) -> Result<BrowserlessTimings, String> {
    let defaults = browserless_mode_defaults(&config.cf_mode)
        .ok_or_else(|| format!("未知 Browserless CF 模式: {}", config.cf_mode))?;
    Ok(BrowserlessTimings {
        wait_ms: config.wait_ms.unwrap_or(defaults.wait_ms),
        solve_timeout: config.solve_timeout.unwrap_or(defaults.solve_timeout),
        action_timeout: config.action_timeout.unwrap_or(defaults.action_timeout),
        post_click_wait_ms: config
            .post_click_wait_ms
            .unwrap_or(defaults.post_click_wait_ms),
    })
}

async fn run_browserless_probe(config: &BrowserlessConfig) -> Result<BrowserProbeResult, String> {
    const PROBE_URL: &str = "https://1.1.1.1";
    let result = post_browserless_bql(
        config,
        BROWSERLESS_PROBE_QUERY,
        "Probe",
        json!({ "url": PROBE_URL }),
        Duration::from_secs(45),
    )
    .await?;

    if let Some(message) = browserless_error_message(&result) {
        return Ok(BrowserProbeResult {
            success: false,
            url: PROBE_URL.to_string(),
            message,
            title: None,
        });
    }

    let status = result.pointer("/data/goto/status").and_then(Value::as_u64);
    let success = status.is_some_and(|status| (200..400).contains(&status));
    let html = result
        .pointer("/data/html/html")
        .and_then(Value::as_str)
        .unwrap_or_default();
    Ok(BrowserProbeResult {
        success,
        url: PROBE_URL.to_string(),
        message: match status {
            Some(status) if success => {
                format!("Browserless 已成功导航到 1.1.1.1（HTTP {}）", status)
            }
            Some(status) => format!("Browserless 导航失败（HTTP {}）", status),
            None => "Browserless 未返回导航状态".to_string(),
        },
        title: html_title(html),
    })
}

async fn run_browserless_sign_in(
    service_config: &BrowserlessConfig,
    base_url: String,
    cookie_header: String,
    task_config: BrowserlessTaskConfig,
) -> Result<SignInOutput, String> {
    let timings = resolve_browserless_timings(&task_config)?;
    let target_url = reqwest::Url::parse(&format!("{}/attendance.php", base_url))
        .map_err(|error| format!("签到地址无效: {}", error))?;
    let domain = target_url
        .host_str()
        .ok_or_else(|| "签到地址缺少域名".to_string())?;
    let cookies = browserless_cookies(&cookie_header, domain, target_url.as_str());
    if cookies.is_empty() {
        return Err("Cookie 不能为空".to_string());
    }

    let request_timeout_ms = timings
        .wait_ms
        .saturating_add(timings.solve_timeout)
        .saturating_add(timings.action_timeout.saturating_mul(2))
        .saturating_add(timings.post_click_wait_ms)
        .saturating_add(30_000);
    let result = post_browserless_bql(
        service_config,
        BROWSERLESS_SIGN_IN_QUERY,
        "CheckIn",
        json!({
            "cookies": cookies,
            "url": target_url.as_str(),
            "selector": task_config.selector,
            "waitMs": timings.wait_ms,
            "solveTimeout": timings.solve_timeout,
            "actionTimeout": timings.action_timeout,
            "postClickWaitMs": timings.post_click_wait_ms,
        }),
        Duration::from_millis(request_timeout_ms),
    )
    .await?;

    summarize_browserless_sign_in(&result)
}

async fn post_browserless_bql(
    config: &BrowserlessConfig,
    query: &str,
    operation_name: &str,
    variables: Value,
    timeout: Duration,
) -> Result<Value, String> {
    let endpoint = build_browserless_bql_url(config)?;
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| format!("创建 Browserless 客户端失败: {}", error))?;
    let response = client
        .post(endpoint)
        .json(&json!({
            "query": query,
            "operationName": operation_name,
            "variables": variables,
        }))
        .send()
        .await
        .map_err(|error| {
            if error.is_timeout() {
                "Browserless 请求超时".to_string()
            } else {
                "Browserless 请求失败，请检查地址、Token 和网络连接".to_string()
            }
        })?;
    let status = response.status();
    let result = response
        .json::<Value>()
        .await
        .map_err(|_| format!("Browserless 返回了无效的 JSON（HTTP {}）", status.as_u16()))?;
    if !status.is_success() {
        return Err(browserless_error_message(&result)
            .unwrap_or_else(|| format!("Browserless 请求失败（HTTP {}）", status.as_u16())));
    }
    Ok(result)
}

fn build_browserless_bql_url(config: &BrowserlessConfig) -> Result<reqwest::Url, String> {
    let address = config.address.as_deref().unwrap_or_default().trim();
    if address.is_empty() {
        return Err("Browserless 地址不能为空".to_string());
    }
    let token = config.token.as_deref().unwrap_or_default().trim();
    if token.is_empty() {
        return Err("Browserless Token 不能为空".to_string());
    }

    let mut endpoint =
        reqwest::Url::parse(address).map_err(|error| format!("Browserless 地址无效: {}", error))?;
    if !matches!(endpoint.scheme(), "http" | "https") {
        return Err("Browserless 地址必须以 http:// 或 https:// 开头".to_string());
    }
    let current_path = endpoint.path().trim_end_matches('/');
    let bql_path = if current_path.ends_with("/bql") {
        current_path.to_string()
    } else if current_path.ends_with("/stealth") {
        format!("{}/bql", current_path)
    } else if current_path.is_empty() {
        "/stealth/bql".to_string()
    } else {
        format!("{}/stealth/bql", current_path)
    };
    endpoint.set_path(&bql_path);
    endpoint.set_fragment(None);

    let existing_pairs = endpoint
        .query_pairs()
        .filter(|(key, _)| key != "token")
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect::<Vec<_>>();
    endpoint.set_query(None);
    {
        let mut query = endpoint.query_pairs_mut();
        for (key, value) in existing_pairs {
            query.append_pair(&key, &value);
        }
        query.append_pair("token", token);
    }
    Ok(endpoint)
}

fn browserless_cookies(cookie_header: &str, domain: &str, url: &str) -> Vec<Value> {
    parse_cookie_pairs(cookie_header)
        .into_iter()
        .map(|(name, value)| {
            json!({
                "name": name,
                "value": value,
                "domain": domain,
                "path": "/",
                "secure": url.starts_with("https://"),
                "url": url,
            })
        })
        .collect()
}

fn summarize_browserless_sign_in(result: &Value) -> Result<SignInOutput, String> {
    if let Some(message) = browserless_error_message(result) {
        return Err(message);
    }

    let data = result.get("data").unwrap_or(&Value::Null);
    let goto_status = data.pointer("/goto/status").and_then(Value::as_u64);
    if goto_status != Some(200) {
        return Err(match goto_status {
            Some(status) => format!("Browserless 打开签到页失败（HTTP {}）", status),
            None => "Browserless 未执行页面导航".to_string(),
        });
    }

    let solve_found = data.pointer("/solve/found").and_then(Value::as_bool);
    let solve_solved = data.pointer("/solve/solved").and_then(Value::as_bool);
    if solve_found == Some(true) && solve_solved == Some(false) {
        return Err("Browserless 找到 Cloudflare 验证，但未能完成验证".to_string());
    }
    if data.get("click").is_none_or(Value::is_null) {
        return Err("Browserless 未执行签到点击，请检查 selector".to_string());
    }

    let html = data
        .pointer("/html/html")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if html.contains("未登录") || html.contains("必须在登录后才能访问") {
        return Err("Cookie 无效或已过期".to_string());
    }
    if html.contains("签到成功") {
        return Ok(SignInOutput {
            status: "success".to_string(),
            message: "Browserless 签到成功".to_string(),
        });
    }
    if html.contains("已经签到") || html.contains("今日已签到") || html.contains("今天已经签到")
    {
        return Ok(SignInOutput {
            status: "already".to_string(),
            message: "今日已经签到".to_string(),
        });
    }
    if html.contains("cf-turnstile") || html.contains("立即签到") {
        return Ok(SignInOutput {
            status: "failed".to_string(),
            message: "Browserless 已点击签到入口，但页面仍停留在验证或签到状态".to_string(),
        });
    }

    Ok(SignInOutput {
        status: "failed".to_string(),
        message: "Browserless 已完成点击，但未识别到签到成功结果".to_string(),
    })
}

fn browserless_error_message(result: &Value) -> Option<String> {
    let messages = result
        .get("errors")?
        .as_array()?
        .iter()
        .filter_map(|error| error.get("message").and_then(Value::as_str))
        .take(3)
        .collect::<Vec<_>>();
    (!messages.is_empty()).then(|| format!("Browserless BQL 执行失败: {}", messages.join("; ")))
}

fn html_title(html: &str) -> Option<String> {
    let document = Html::parse_document(html);
    let selector = Selector::parse("title").ok()?;
    let title = document
        .select(&selector)
        .next()?
        .text()
        .collect::<Vec<_>>()
        .join(" ");
    let title = title.trim();
    (!title.is_empty()).then(|| title.to_string())
}

fn create_target_session(client: &mut CdpClient) -> Result<String, String> {
    let target = client.call(
        "Target.createTarget",
        json!({
            "url": "about:blank",
            "newWindow": false,
            "background": false
        }),
        None,
    )?;
    let target_id = target
        .get("targetId")
        .and_then(Value::as_str)
        .ok_or_else(|| "CDP 未返回 targetId".to_string())?
        .to_string();
    let attached = client.call(
        "Target.attachToTarget",
        json!({
            "targetId": target_id,
            "flatten": true
        }),
        None,
    )?;
    let session_id = attached
        .get("sessionId")
        .and_then(Value::as_str)
        .ok_or_else(|| "CDP 未返回 sessionId".to_string())?
        .to_string();
    Ok(session_id)
}

const CDP_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const CDP_IO_TIMEOUT: Duration = Duration::from_secs(30);
const CDP_CALL_TIMEOUT: Duration = Duration::from_secs(60);
const CDP_SHUTDOWN_ERROR: &str = "服务正在退出，已取消浏览器操作";

struct AbortTask(tokio::task::JoinHandle<()>);

impl Drop for AbortTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

struct InterruptSocket(Arc<TcpStream>);

impl Drop for InterruptSocket {
    fn drop(&mut self) {
        let _ = self.0.shutdown(Shutdown::Both);
    }
}

struct CdpConnectionGuard {
    socket: InterruptSocket,
    _watcher: AbortTask,
}

impl CdpConnectionGuard {
    fn new(stream: &TcpStream, shutdown: CancellationToken) -> Result<Self, String> {
        let socket = InterruptSocket(Arc::new(
            stream.try_clone().map_err(|error| error.to_string())?,
        ));
        let on_shutdown = InterruptSocket(socket.0.clone());
        let watcher = tokio::spawn(async move {
            // This guard also interrupts blocking IO if the runtime drops the
            // watcher before it has a chance to observe cancellation.
            let _on_shutdown = on_shutdown;
            shutdown.cancelled().await;
        });
        Ok(Self {
            socket,
            _watcher: AbortTask(watcher),
        })
    }

    fn deadline(&self, duration: Duration) -> AbortTask {
        let socket = self.socket.0.clone();
        AbortTask(tokio::spawn(async move {
            tokio::time::sleep(duration).await;
            let _ = socket.shutdown(Shutdown::Both);
        }))
    }
}

struct CdpClient {
    socket: tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<TcpStream>>,
    connection: CdpConnectionGuard,
    shutdown: CancellationToken,
    next_id: u64,
}

fn cdp_block_on<T>(
    shutdown: &CancellationToken,
    future: impl std::future::Future<Output = Result<T, String>>,
) -> Result<T, String> {
    let handle = tokio::runtime::Handle::try_current()
        .map_err(|error| format!("获取 tokio handle 失败: {error}"))?;
    handle.block_on(async {
        tokio::select! {
            biased;
            _ = shutdown.cancelled() => Err(CDP_SHUTDOWN_ERROR.to_string()),
            result = future => result,
        }
    })
}

impl CdpClient {
    fn connect(endpoint: String) -> Result<Self, String> {
        Self::connect_with_shutdown(endpoint, crate::runtime_shutdown_token())
    }

    fn connect_with_shutdown(
        mut endpoint: String,
        shutdown: CancellationToken,
    ) -> Result<Self, String> {
        // Retain tungstenite::connect's limit of three redirects.
        for attempt in 0..=3 {
            let request = endpoint
                .as_str()
                .into_client_request()
                .map_err(|error| format!("浏览器 CDP 地址无效: {error}"))?;
            let port = request
                .uri()
                .port_u16()
                .unwrap_or(match request.uri().scheme_str() {
                    Some("ws") => 80,
                    Some("wss") => 443,
                    _ => return Err("浏览器 CDP 地址必须使用 ws 或 wss".to_string()),
                });
            let host = request
                .uri()
                .host()
                .ok_or_else(|| "浏览器 CDP 地址缺少主机名".to_string())?
                .trim_start_matches('[')
                .trim_end_matches(']');
            let stream = cdp_block_on(&shutdown, async {
                tokio::time::timeout(
                    CDP_CONNECT_TIMEOUT,
                    tokio::net::TcpStream::connect((host, port)),
                )
                .await
                .map_err(|_| "连接浏览器 CDP 超时".to_string())?
                .map_err(|error| format!("连接浏览器 CDP 失败: {error}"))?
                .into_std()
                .map_err(|error| error.to_string())
            })?;
            stream
                .set_nonblocking(false)
                .map_err(|error| error.to_string())?;
            stream
                .set_nodelay(true)
                .map_err(|error| error.to_string())?;
            stream
                .set_read_timeout(Some(CDP_IO_TIMEOUT))
                .map_err(|error| error.to_string())?;
            stream
                .set_write_timeout(Some(CDP_IO_TIMEOUT))
                .map_err(|error| error.to_string())?;
            let connection = CdpConnectionGuard::new(&stream, shutdown.clone())?;
            let handshake_deadline = connection.deadline(CDP_IO_TIMEOUT);
            match client_tls(request, stream) {
                Ok((socket, _)) => {
                    drop(handshake_deadline);
                    return Ok(Self {
                        socket,
                        connection,
                        shutdown,
                        next_id: 0,
                    });
                }
                Err(HandshakeError::Failure(tungstenite::Error::Http(response)))
                    if response.status().is_redirection() && attempt < 3 =>
                {
                    endpoint = response
                        .headers()
                        .get("Location")
                        .and_then(|location| location.to_str().ok())
                        .ok_or_else(|| "浏览器 CDP 重定向缺少有效地址".to_string())?
                        .to_string();
                }
                Err(error) => return Err(format!("连接浏览器 CDP 失败: {error}")),
            }
        }
        Err("浏览器 CDP 重定向次数过多".to_string())
    }

    fn wait(&self, duration: Duration) -> Result<(), String> {
        cdp_block_on(&self.shutdown, async {
            tokio::time::sleep(duration).await;
            Ok(())
        })
    }

    fn call(
        &mut self,
        method: &str,
        params: Value,
        session_id: Option<&str>,
    ) -> Result<Value, String> {
        if self.shutdown.is_cancelled() {
            return Err(CDP_SHUTDOWN_ERROR.to_string());
        }
        // A stream of unrelated browser events must not extend the operation
        // forever. The deadline also interrupts an incomplete WebSocket frame.
        let _deadline = self.connection.deadline(CDP_CALL_TIMEOUT);
        self.next_id += 1;
        let id = self.next_id;
        let mut request = json!({
            "id": id,
            "method": method,
            "params": params,
        });
        if let Some(session_id) = session_id {
            request["sessionId"] = json!(session_id);
        }

        self.socket
            .send(Message::Text(request.to_string().into()))
            .map_err(|e| format!("发送 CDP 指令失败: {}", e))?;

        loop {
            if self.shutdown.is_cancelled() {
                return Err(CDP_SHUTDOWN_ERROR.to_string());
            }
            let message = self
                .socket
                .read()
                .map_err(|e| format!("读取 CDP 响应失败: {}", e))?;
            let Message::Text(text) = message else {
                continue;
            };
            let value: Value =
                serde_json::from_str(&text).map_err(|e| format!("解析 CDP 响应失败: {}", e))?;
            if value.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = value.get("error") {
                return Err(error.to_string());
            }
            return Ok(value.get("result").cloned().unwrap_or_else(|| json!({})));
        }
    }
}

impl Drop for CdpClient {
    fn drop(&mut self) {
        // A WebSocket close frame can itself block on a stalled peer. Close the
        // underlying connection directly when releasing this blocking client.
        let _ = self.connection.socket.0.shutdown(Shutdown::Both);
    }
}

async fn run_cdp_sign_in(
    endpoint: String,
    base_url: String,
    cookie: String,
    sign_in_method: String,
    ocr_api_key: Option<String>,
) -> Result<SignInOutput, String> {
    tokio::task::spawn_blocking(move || {
        run_cdp_sign_in_blocking(endpoint, base_url, cookie, sign_in_method, ocr_api_key)
    })
    .await
    .map_err(|e| format!("签到任务 join 失败: {}", e))?
}

fn run_cdp_sign_in_blocking(
    endpoint: String,
    base_url: String,
    cookie: String,
    sign_in_method: String,
    ocr_api_key: Option<String>,
) -> Result<SignInOutput, String> {
    let mut client = CdpClient::connect(endpoint)?;
    let session_id = create_target_session(&mut client)?;

    client.call("Page.enable", json!({}), Some(&session_id))?;
    client.call("Runtime.enable", json!({}), Some(&session_id))?;
    client.call("Network.enable", json!({}), Some(&session_id))?;
    set_cookies_via_cdp(&mut client, &session_id, &base_url, &cookie)?;

    let index_url = format!("{}/index.php", base_url);
    navigate_via_cdp(&mut client, &session_id, &index_url, "打开首页失败")?;
    wait_for_cloudflare(&mut client, &session_id)?;
    let text = page_text_via_cdp(&mut client, &session_id)?;
    if looks_logged_out(&text) {
        return Err("Cookie 无效或已过期".to_string());
    }

    let attendance_url = format!("{}/attendance.php", base_url);
    navigate_via_cdp(&mut client, &session_id, &attendance_url, "打开签到页失败")?;
    wait_for_cloudflare(&mut client, &session_id)?;

    match sign_in_method.as_str() {
        SIGN_IN_METHOD_OPEN_PAGE => run_open_page_sign_in(&mut client, &session_id),
        SIGN_IN_METHOD_CLOUDFLARE => run_cloudflare_sign_in(&mut client, &session_id, &base_url),
        SIGN_IN_METHOD_OCR_CAPTCHA => {
            run_ocr_captcha_sign_in(&mut client, &session_id, &base_url, ocr_api_key.as_deref())
        }
        other => Err(format!("未知签到方式: {}", other)),
    }
}

fn run_open_page_sign_in(client: &mut CdpClient, session_id: &str) -> Result<SignInOutput, String> {
    client.wait(Duration::from_secs(7))?;
    let text = page_text_via_cdp(client, session_id)?;
    if let Some(result) = classify_sign_in_text(&text) {
        return Ok(result);
    }
    Ok(SignInOutput {
        status: "success".to_string(),
        message: compact_text(&text).unwrap_or_else(|| "已访问签到页".to_string()),
    })
}

fn run_cloudflare_sign_in(
    client: &mut CdpClient,
    session_id: &str,
    base_url: &str,
) -> Result<SignInOutput, String> {
    let text = page_text_via_cdp(client, session_id)?;
    if let Some(result) = classify_sign_in_text(&text) {
        return Ok(result);
    }

    let clicked = evaluate_bool_via_cdp(client, session_id, CLICK_SIGN_IN_SCRIPT)?;
    if clicked {
        client.wait(Duration::from_millis(2500))?;
    } else {
        for url in [
            format!("{}/attendance.php?action=sign", base_url),
            format!("{}/attendance.php?do=sign", base_url),
            format!("{}/attendance.php?sign=1", base_url),
        ] {
            if navigate_via_cdp(client, session_id, &url, "尝试备用签到地址失败").is_ok()
                && wait_for_cloudflare(client, session_id).is_ok()
            {
                let text = page_text_via_cdp(client, session_id)?;
                if let Some(result) = classify_sign_in_text(&text) {
                    return Ok(result);
                }
            }
        }
    }

    let text = page_text_via_cdp(client, session_id)?;
    if let Some(result) = classify_sign_in_text(&text) {
        return Ok(result);
    }

    Ok(SignInOutput {
        status: if clicked { "success" } else { "failed" }.to_string(),
        message: if clicked {
            compact_text(&text).unwrap_or_else(|| "已尝试点击签到按钮".to_string())
        } else {
            "未找到 NexusPHP 签到入口".to_string()
        },
    })
}

fn run_ocr_captcha_sign_in(
    client: &mut CdpClient,
    session_id: &str,
    base_url: &str,
    ocr_api_key: Option<&str>,
) -> Result<SignInOutput, String> {
    let text = page_text_via_cdp(client, session_id)?;
    if let Some(result) = classify_sign_in_text(&text) {
        return Ok(result);
    }

    let clicked = evaluate_bool_via_cdp(client, session_id, CLICK_SIGN_IN_SCRIPT)?;
    if clicked {
        client.wait(Duration::from_millis(2500))?;
        if handle_captcha_if_present(client, session_id, ocr_api_key)? {
            client.wait(Duration::from_millis(2500))?;
        }
    } else {
        for url in [
            format!("{}/attendance.php?action=sign", base_url),
            format!("{}/attendance.php?do=sign", base_url),
            format!("{}/attendance.php?sign=1", base_url),
        ] {
            if navigate_via_cdp(client, session_id, &url, "尝试备用签到地址失败").is_ok()
                && wait_for_cloudflare(client, session_id).is_ok()
            {
                let text = page_text_via_cdp(client, session_id)?;
                if let Some(result) = classify_sign_in_text(&text) {
                    return Ok(result);
                }
                if handle_captcha_if_present(client, session_id, ocr_api_key)? {
                    client.wait(Duration::from_millis(2500))?;
                }
            }
        }
    }

    let text = page_text_via_cdp(client, session_id)?;
    if let Some(result) = classify_sign_in_text(&text) {
        return Ok(result);
    }

    Ok(SignInOutput {
        status: if clicked { "success" } else { "failed" }.to_string(),
        message: if clicked {
            compact_text(&text).unwrap_or_else(|| "已尝试点击签到按钮".to_string())
        } else {
            "未找到 NexusPHP 签到入口".to_string()
        },
    })
}

fn set_cookies_via_cdp(
    client: &mut CdpClient,
    session_id: &str,
    base_url: &str,
    cookie: &str,
) -> Result<(), String> {
    let cookies = parse_cookie_pairs(cookie)
        .into_iter()
        .map(|(name, value)| {
            json!({
                "name": name,
                "value": value,
                "url": base_url,
                "path": "/",
                "secure": base_url.starts_with("https://")
            })
        })
        .collect::<Vec<_>>();
    if cookies.is_empty() {
        return Err("Cookie 不能为空".to_string());
    }
    client.call(
        "Network.setCookies",
        json!({ "cookies": cookies }),
        Some(session_id),
    )?;
    Ok(())
}

fn navigate_via_cdp(
    client: &mut CdpClient,
    session_id: &str,
    url: &str,
    context: &str,
) -> Result<(), String> {
    let result = client.call("Page.navigate", json!({ "url": url }), Some(session_id));
    match result {
        Ok(value) => {
            if let Some(error_text) = value.get("errorText").and_then(Value::as_str) {
                return Err(format!("{context}: {error_text}"));
            }
            client.wait(Duration::from_secs(2))?;
            Ok(())
        }
        Err(error) => Err(format!("{context}: {error}")),
    }
}

fn page_text_via_cdp(client: &mut CdpClient, session_id: &str) -> Result<String, String> {
    evaluate_string_via_cdp(
        client,
        session_id,
        "document.body ? document.body.innerText : document.documentElement.innerText",
    )
}

fn wait_for_cloudflare(client: &mut CdpClient, session_id: &str) -> Result<(), String> {
    const MAX_WAIT: Duration = Duration::from_secs(10);
    const POLL_INTERVAL: Duration = Duration::from_secs(2);

    let started = Instant::now();
    loop {
        let title =
            evaluate_string_via_cdp(client, session_id, "document.title").unwrap_or_default();
        let body = page_text_via_cdp(client, session_id).unwrap_or_default();
        if !is_cloudflare_challenge(&title, &body) {
            return Ok(());
        }

        try_click_turnstile(client, session_id);

        if started.elapsed() >= MAX_WAIT {
            return Err("Cloudflare 挑战未通过，请检查 cf_clearance cookie 或代理".to_string());
        }
        client.wait(POLL_INTERVAL)?;
    }
}

fn is_cloudflare_challenge(title: &str, body: &str) -> bool {
    let lower_title = title.to_ascii_lowercase();
    let lower_body = body.to_ascii_lowercase();
    lower_title.contains("just a moment")
        || lower_body.contains("cf-challenge")
        || lower_body.contains("checking your browser")
        || lower_body.contains("checking if the site connection is secure")
        || lower_body.contains("cf_chl_opt")
        || lower_body.contains("turnstile")
}

fn try_click_turnstile(client: &mut CdpClient, session_id: &str) {
    let coords = evaluate_string_via_cdp(
        client,
        session_id,
        r#"(() => {
            const iframe = document.querySelector('iframe[src*="challenges.cloudflare.com"], iframe[id^="cf-chl-widget"], iframe[title*="Cloudflare"]');
            if (!iframe) return '';
            const r = iframe.getBoundingClientRect();
            if (r.width === 0 || r.height === 0) return '';
            return JSON.stringify({ x: r.x + 28, y: r.y + r.height / 2 });
        })()"#,
    )
    .unwrap_or_default();

    if coords.is_empty() {
        return;
    }
    let Ok(value) = serde_json::from_str::<Value>(&coords) else {
        return;
    };
    let Some(x) = value.get("x").and_then(Value::as_f64) else {
        return;
    };
    let Some(y) = value.get("y").and_then(Value::as_f64) else {
        return;
    };

    let _ = client.call(
        "Input.dispatchMouseEvent",
        json!({ "type": "mousePressed", "x": x, "y": y, "button": "left", "clickCount": 1 }),
        Some(session_id),
    );
    let _ = client.call(
        "Input.dispatchMouseEvent",
        json!({ "type": "mouseReleased", "x": x, "y": y, "button": "left", "clickCount": 1 }),
        Some(session_id),
    );
}

fn handle_captcha_if_present(
    client: &mut CdpClient,
    session_id: &str,
    ocr_api_key: Option<&str>,
) -> Result<bool, String> {
    let data_url = evaluate_string_await_via_cdp(
        client,
        session_id,
        EXTRACT_CAPTCHA_IMAGE_SCRIPT,
        Duration::from_secs(5),
    )?;
    if data_url.trim().is_empty() {
        return Ok(false);
    }

    let api_key = match ocr_api_key.map(str::trim).filter(|v| !v.is_empty()) {
        Some(key) => key.to_string(),
        None => return Err("页面要求图片验证码，但未配置 OCR API key".to_string()),
    };

    let code = ocr_space_recognize(&api_key, &data_url)?;
    let code = code.trim().to_string();
    if code.is_empty() {
        return Err("OCR 识别结果为空".to_string());
    }

    let filled = evaluate_bool_via_cdp(client, session_id, &fill_captcha_script(&code))?;
    if !filled {
        return Err("验证码识别成功但未找到输入框".to_string());
    }
    Ok(true)
}

fn ocr_space_recognize(api_key: &str, data_url: &str) -> Result<String, String> {
    cdp_block_on(&crate::runtime_shutdown_token(), async {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|e| format!("构建 OCR HTTP 客户端失败: {}", e))?;
        let form = reqwest::multipart::Form::new()
            .text("apikey", api_key.to_string())
            .text("language", "auto".to_string())
            .text("scale", "true".to_string())
            .text("base64Image", data_url.to_string());
        let resp = client
            .post("https://api.ocr.space/parse/image")
            .multipart(form)
            .send()
            .await
            .map_err(|e| format!("OCR 请求失败: {}", e))?;
        let value: Value = resp
            .json()
            .await
            .map_err(|e| format!("解析 OCR 响应失败: {}", e))?;
        if value
            .get("IsErroredOnProcessing")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            let msg = value
                .get("ErrorMessage")
                .and_then(Value::as_str)
                .unwrap_or("未知错误");
            return Err(format!("OCR 处理出错: {}", msg));
        }
        let text = value
            .get("ParsedResults")
            .and_then(|v| v.get(0))
            .and_then(|v| v.get("ParsedText"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        Ok(text)
    })
    .map_err(|e| format!("OCR 任务失败: {}", e))
}

fn evaluate_string_await_via_cdp(
    client: &mut CdpClient,
    session_id: &str,
    expression: &str,
    _timeout: Duration,
) -> Result<String, String> {
    let value = client.call(
        "Runtime.evaluate",
        json!({
            "expression": expression,
            "returnByValue": true,
            "awaitPromise": true,
        }),
        Some(session_id),
    )?;
    Ok(value
        .get("result")
        .and_then(|v| v.get("value"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string())
}

fn fill_captcha_script(code: &str) -> String {
    let escaped = code
        .replace('\\', "\\\\")
        .replace('\'', "\\'")
        .replace('\n', "\\n");
    format!(
        r#"
(() => {{
  const code = '{}';
  const visible = (el) => {{
    const r = el.getBoundingClientRect();
    return r.width > 0 && r.height > 0 && el.offsetParent !== null;
  }};
  const inputs = Array.from(document.querySelectorAll('input[type="text"], input:not([type])'))
    .filter(visible)
    .filter((el) => !/search|搜索|username|user|password|email/i.test(el.name + ' ' + (el.placeholder || '') + ' ' + (el.id || '')));
  if (inputs.length === 0) return false;
  const input = inputs[0];
  const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value')?.set;
  if (setter) setter.call(input, code); else input.value = code;
  input.dispatchEvent(new Event('input', {{ bubbles: true }}));
  input.dispatchEvent(new Event('change', {{ bubbles: true }}));
  const form = input.form;
  const submitBtn = form
    ? form.querySelector('input[type="submit"], button[type="submit"], button')
    : null;
  if (submitBtn) {{
    submitBtn.click();
  }} else if (form) {{
    form.submit();
  }} else {{
    const btns = Array.from(document.querySelectorAll('button, input[type="button"], a'))
      .filter(visible)
      .filter((el) => /签到|簽到|打卡|确认|確認|submit|ok|verify/i.test([el.innerText, el.value, el.title].filter(Boolean).join(' ')));
    if (btns.length > 0) btns[0].click(); else return false;
  }}
  return true;
}})()
"#,
        escaped
    )
}

const EXTRACT_CAPTCHA_IMAGE_SCRIPT: &str = r#"
(async () => {
  const visible = (el) => {
    const r = el.getBoundingClientRect();
    return r.width > 0 && r.height > 0 && el.offsetParent !== null;
  };
  const selectors = [
    'img[src*="code" i]', 'img[src*="captcha" i]', 'img[src*="verify" i]',
    'img[src*="image_code" i]', 'img[id*="code" i]', 'img[alt*="captcha" i]',
    'img[alt*="code" i]', 'img[alt*="verify" i]'
  ];
  const imgs = Array.from(document.querySelectorAll(selectors.join(','))).filter(visible);
  if (imgs.length === 0) return '';
  const img = imgs[0];
  if (!img.src) return '';
  try {
    const resp = await fetch(img.src, { credentials: 'include' });
    const blob = await resp.blob();
    const dataUrl = await new Promise((resolve, reject) => {
      const reader = new FileReader();
      reader.onloadend = () => resolve(reader.result);
      reader.onerror = reject;
      reader.readAsDataURL(blob);
    });
    return dataUrl || '';
  } catch (e) {
    return '';
  }
})()
"#;

fn evaluate_string_via_cdp(
    client: &mut CdpClient,
    session_id: &str,
    expression: &str,
) -> Result<String, String> {
    let value = client.call(
        "Runtime.evaluate",
        json!({
            "expression": expression,
            "returnByValue": true
        }),
        Some(session_id),
    )?;
    Ok(value
        .get("result")
        .and_then(|v| v.get("value"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string())
}

fn evaluate_bool_via_cdp(
    client: &mut CdpClient,
    session_id: &str,
    expression: &str,
) -> Result<bool, String> {
    let value = client.call(
        "Runtime.evaluate",
        json!({
            "expression": expression,
            "returnByValue": true
        }),
        Some(session_id),
    )?;
    Ok(value
        .get("result")
        .and_then(|v| v.get("value"))
        .and_then(Value::as_bool)
        .unwrap_or(false))
}

fn build_lightpanda_endpoint(
    config: &LightpandaConfig,
    use_proxy_for_lightpanda: bool,
) -> Result<String, String> {
    if let Some(endpoint) = config
        .endpoint
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Ok(endpoint.to_string());
    }

    let token = config.token.as_deref().unwrap_or_default().trim();
    if token.is_empty() {
        return Err("Lightpanda Token 不能为空".to_string());
    }

    let mut endpoint = format!(
        "wss://{}.cloud.lightpanda.io/ws?token={}",
        normalize_region(&config.region),
        urlencoding::encode(token)
    );
    if !config.browser.trim().is_empty() {
        endpoint.push_str("&browser=");
        endpoint.push_str(&urlencoding::encode(config.browser.trim()));
    }
    if let Some(proxy) = use_proxy_for_lightpanda
        .then_some(config.proxy.as_deref())
        .flatten()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        endpoint.push_str("&proxy=");
        endpoint.push_str(&urlencoding::encode(proxy));
    }
    if let Some(country) = config
        .country
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        endpoint.push_str("&country=");
        endpoint.push_str(&urlencoding::encode(country));
    }
    Ok(endpoint)
}

fn normalize_region(region: &str) -> &str {
    match region.trim() {
        "uswest" => "uswest",
        _ => "euwest",
    }
}

pub fn normalize_sign_in_browser(browser: &str) -> Option<&'static str> {
    match browser.trim().to_ascii_lowercase().as_str() {
        SIGN_IN_BROWSER_LIGHTPANDA => Some(SIGN_IN_BROWSER_LIGHTPANDA),
        SIGN_IN_BROWSER_BROWSERLESS => Some(SIGN_IN_BROWSER_BROWSERLESS),
        _ => None,
    }
}

pub fn normalize_browserless_cf_mode(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        BROWSERLESS_CF_MODE_AUTO => Some(BROWSERLESS_CF_MODE_AUTO),
        BROWSERLESS_CF_MODE_PAGE => Some(BROWSERLESS_CF_MODE_PAGE),
        BROWSERLESS_CF_MODE_TURNSTILE => Some(BROWSERLESS_CF_MODE_TURNSTILE),
        _ => None,
    }
}

#[derive(Debug)]
struct SignInOutput {
    status: String,
    message: String,
}

fn parse_cookie_pairs(cookie: &str) -> Vec<(String, String)> {
    cookie
        .split(';')
        .filter_map(|part| {
            let (name, value) = part.trim().split_once('=')?;
            let name = name.trim();
            if name.is_empty() {
                None
            } else {
                Some((name.to_string(), value.trim().to_string()))
            }
        })
        .collect()
}

fn looks_logged_out(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    (lower.contains("login") || text.contains("用户登录") || text.contains("會員登入"))
        && !(lower.contains("logout") || text.contains("登出") || text.contains("退出"))
}

fn classify_sign_in_text(text: &str) -> Option<SignInOutput> {
    let compact = compact_text(text)?;
    let lower = compact.to_ascii_lowercase();
    if compact.contains("已签到")
        || compact.contains("已经签到")
        || compact.contains("今日已签")
        || compact.contains("今天已签")
        || compact.contains("已打卡")
        || lower.contains("already")
    {
        return Some(SignInOutput {
            status: "already".to_string(),
            message: compact,
        });
    }
    if compact.contains("签到成功")
        || compact.contains("簽到成功")
        || compact.contains("打卡成功")
        || compact.contains("成功签到")
        || lower.contains("success")
    {
        return Some(SignInOutput {
            status: "success".to_string(),
            message: compact,
        });
    }
    None
}

fn compact_text(text: &str) -> Option<String> {
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.is_empty() {
        None
    } else {
        Some(compact.chars().take(240).collect())
    }
}

const CLICK_SIGN_IN_SCRIPT: &str = r#"
(() => {
  const candidates = Array.from(document.querySelectorAll('button,input[type="button"],input[type="submit"],a'));
  const target = candidates.find((node) => {
    const label = [node.innerText, node.value, node.title, node.getAttribute('aria-label')]
      .filter(Boolean)
      .join(' ');
    return /签到|簽到|打卡|attendance|sign/i.test(label);
  });
  if (!target) return false;
  target.click();
  return true;
})()
"#;

pub const SIGN_IN_METHOD_OPEN_PAGE: &str = "open_page";
pub const SIGN_IN_METHOD_CLOUDFLARE: &str = "cloudflare";
pub const SIGN_IN_METHOD_OCR_CAPTCHA: &str = "ocr_captcha";

pub const SIGN_IN_METHODS: &[&str] = &[
    SIGN_IN_METHOD_OPEN_PAGE,
    SIGN_IN_METHOD_CLOUDFLARE,
    SIGN_IN_METHOD_OCR_CAPTCHA,
];

pub fn normalize_sign_in_method(value: &str) -> String {
    let trimmed = value.trim();
    if SIGN_IN_METHODS.contains(&trimmed) {
        return trimmed.to_string();
    }
    SIGN_IN_METHOD_OPEN_PAGE.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use tokio::io::AsyncReadExt;

    async fn assert_cdp_handshake_is_cancellable(scheme: &str) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("{scheme}://{}/", listener.local_addr().unwrap());
        let shutdown = CancellationToken::new();
        let client_shutdown = shutdown.clone();
        let client = tokio::task::spawn_blocking(move || {
            CdpClient::connect_with_shutdown(endpoint, client_shutdown).map(|_| ())
        });
        let (mut peer, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
            .await
            .unwrap()
            .unwrap();
        let mut handshake = [0; 4096];
        assert!(
            tokio::time::timeout(Duration::from_secs(5), peer.read(&mut handshake))
                .await
                .unwrap()
                .unwrap()
                > 0
        );
        // Keep the peer open without replying to its TLS/WebSocket handshake.
        shutdown.cancel();
        let result = tokio::time::timeout(Duration::from_secs(2), client)
            .await
            .expect("CDP handshake did not stop after cancellation")
            .unwrap();
        assert!(result.is_err());
        drop(peer);
    }

    #[tokio::test]
    async fn cdp_shutdown_interrupts_a_stalled_websocket_handshake() {
        assert_cdp_handshake_is_cancellable("ws").await;
    }

    #[tokio::test]
    async fn cdp_shutdown_interrupts_a_stalled_tls_handshake() {
        assert_cdp_handshake_is_cancellable("wss").await;
    }

    async fn stalled_cdp_peer(
        listener: tokio::net::TcpListener,
        received: tokio::sync::oneshot::Sender<()>,
    ) {
        let (peer, _) = listener.accept().await.unwrap();
        let peer = peer.into_std().unwrap();
        peer.set_nonblocking(false).unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        peer.set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        tokio::task::spawn_blocking(move || {
            let mut socket = tungstenite::accept(peer).unwrap();
            assert!(socket.read().unwrap().is_text());
            received.send(()).unwrap();
            // Never reply to the command; wait for the client to close its socket.
            assert!(socket.read().is_err());
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn cdp_shutdown_interrupts_a_stalled_command_response() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("ws://{}/", listener.local_addr().unwrap());
        let shutdown = CancellationToken::new();
        let client_shutdown = shutdown.clone();
        let (received, ready) = tokio::sync::oneshot::channel();
        let peer = tokio::spawn(stalled_cdp_peer(listener, received));
        let client = tokio::task::spawn_blocking(move || {
            let mut client = CdpClient::connect_with_shutdown(endpoint, client_shutdown)?;
            client.call("Browser.getVersion", json!({}), None)
        });
        tokio::time::timeout(Duration::from_secs(5), ready)
            .await
            .unwrap()
            .unwrap();
        shutdown.cancel();
        assert!(
            tokio::time::timeout(Duration::from_secs(2), client)
                .await
                .expect("CDP read did not stop after cancellation")
                .unwrap()
                .is_err()
        );
        peer.await.unwrap();
    }

    #[tokio::test]
    async fn cdp_runtime_drop_interrupts_a_blocking_command() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("ws://{}/", listener.local_addr().unwrap());
        let (received, ready) = tokio::sync::oneshot::channel();
        let peer = tokio::spawn(stalled_cdp_peer(listener, received));
        let (finished, done) = tokio::sync::oneshot::channel();
        let thread = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async move {
                tokio::task::spawn_blocking(move || {
                    let mut client =
                        CdpClient::connect_with_shutdown(endpoint, CancellationToken::new())
                            .unwrap();
                    assert!(client.call("Browser.getVersion", json!({}), None).is_err());
                });
                ready.await.unwrap();
            });
            // No explicit cancellation: dropping the watcher must still close
            // the socket so runtime teardown can join its blocking thread.
            drop(runtime);
            let _ = finished.send(());
        });
        tokio::time::timeout(Duration::from_secs(2), done)
            .await
            .expect("runtime drop waited indefinitely for blocking CDP IO")
            .unwrap();
        thread.join().unwrap();
        peer.await.unwrap();
    }

    #[test]
    fn lightpanda_endpoint_uses_global_browser_configuration() {
        let config = LightpandaConfig {
            endpoint: None,
            token: Some("token with space".to_string()),
            region: "uswest".to_string(),
            browser: "chrome".to_string(),
            proxy: Some("datacenter".to_string()),
            country: Some("DE".to_string()),
        };

        assert_eq!(
            build_lightpanda_endpoint(&config, true).unwrap(),
            "wss://uswest.cloud.lightpanda.io/ws?token=token%20with%20space&browser=chrome&proxy=datacenter&country=DE"
        );
        assert_eq!(
            build_lightpanda_endpoint(&config, false).unwrap(),
            "wss://uswest.cloud.lightpanda.io/ws?token=token%20with%20space&browser=chrome&country=DE"
        );
    }

    #[test]
    fn sign_in_browser_only_accepts_supported_providers() {
        assert_eq!(
            normalize_sign_in_browser("lightpanda"),
            Some(SIGN_IN_BROWSER_LIGHTPANDA)
        );
        assert_eq!(
            normalize_sign_in_browser(" Browserless "),
            Some(SIGN_IN_BROWSER_BROWSERLESS)
        );
        assert_eq!(normalize_sign_in_browser("chrome"), None);
    }

    #[test]
    fn browserless_mode_defaults_and_overrides_match_reference_script() {
        assert_eq!(
            resolve_browserless_timings(&BrowserlessTaskConfig::default()).unwrap(),
            BrowserlessTimings {
                wait_ms: 5_000,
                solve_timeout: 60_000,
                action_timeout: 30_000,
                post_click_wait_ms: 5_000,
            }
        );

        let page = BrowserlessTaskConfig {
            cf_mode: BROWSERLESS_CF_MODE_PAGE.to_string(),
            action_timeout: Some(12_345),
            ..BrowserlessTaskConfig::default()
        };
        assert_eq!(
            resolve_browserless_timings(&page).unwrap(),
            BrowserlessTimings {
                wait_ms: 1_000,
                solve_timeout: 30_000,
                action_timeout: 12_345,
                post_click_wait_ms: 3_000,
            }
        );
    }

    #[test]
    fn browserless_endpoint_accepts_service_address_and_replaces_token() {
        let endpoint = build_browserless_bql_url(&BrowserlessConfig {
            address: Some("https://production-sfo.browserless.io?region=us&token=old".to_string()),
            token: Some("token with space".to_string()),
        })
        .unwrap();

        assert_eq!(endpoint.path(), "/stealth/bql");
        let pairs = endpoint.query_pairs().collect::<Vec<_>>();
        assert!(
            pairs
                .iter()
                .any(|(key, value)| key == "region" && value == "us")
        );
        assert_eq!(
            pairs
                .iter()
                .filter(|(key, _)| key == "token")
                .map(|(_, value)| value.as_ref())
                .collect::<Vec<_>>(),
            vec!["token with space"]
        );
    }

    #[test]
    fn browserless_result_distinguishes_success_already_and_failure() {
        let success = summarize_browserless_sign_in(&json!({
            "data": {
                "goto": { "status": 200 },
                "solve": { "found": true, "solved": true },
                "click": { "time": 25 },
                "html": { "html": "<main>签到成功</main>" }
            }
        }))
        .unwrap();
        assert_eq!(success.status, "success");

        let already = summarize_browserless_sign_in(&json!({
            "data": {
                "goto": { "status": 200 },
                "solve": { "found": false, "solved": false },
                "click": { "time": 10 },
                "html": { "html": "<main>今日已签到</main>" }
            }
        }))
        .unwrap();
        assert_eq!(already.status, "already");

        let error = summarize_browserless_sign_in(&json!({
            "data": {
                "goto": { "status": 200 },
                "solve": { "found": true, "solved": false },
                "click": null,
                "html": { "html": "" }
            }
        }))
        .unwrap_err();
        assert!(error.contains("未能完成验证"));
    }

    #[test]
    fn cron_0_8_hour_schedule_next() {
        let expr = "0 0 0/8 * * *";
        let schedule: cron::Schedule = expr.parse().expect("cron parse");
        let now = chrono::Utc.with_ymd_and_hms(2026, 6, 27, 7, 59, 0).unwrap();
        let next = schedule.after(&now).next();
        assert_eq!(
            next.map(|t| t.format("%H:%M:%S").to_string()),
            Some("08:00:00".to_string()),
            "next after 07:59 should be 08:00"
        );

        let now2 = chrono::Utc.with_ymd_and_hms(2026, 6, 27, 8, 0, 1).unwrap();
        let next2 = schedule.after(&now2).next();
        assert_eq!(
            next2.map(|t| t.format("%H:%M:%S").to_string()),
            Some("16:00:00".to_string()),
            "next after 08:00:01 should be 16:00"
        );

        let now3 = chrono::Utc
            .with_ymd_and_hms(2026, 6, 27, 7, 59, 30)
            .unwrap();
        let next3 = schedule.after(&now3).next();
        let diff = (next3.unwrap() - now3).num_seconds();
        assert_eq!(diff, 30, "diff at 07:59:30 should be 30s");
    }
}
