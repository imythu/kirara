pub mod scheduler;
pub mod signers;

use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
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
pub struct SignInResultRule {
    pub outcome: String,
    pub kind: String,
    #[serde(default)]
    pub selector: String,
    #[serde(default)]
    pub field: String,
    #[serde(default)]
    pub value: String,
    #[serde(default = "default_rule_value_type")]
    pub value_type: String,
}

fn default_rule_value_type() -> String {
    "string".into()
}

fn default_submit_method() -> String {
    "click".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserlessTaskConfig {
    #[serde(default = "default_browserless_selector")]
    pub selector: String,
    #[serde(default = "default_attendance_path")]
    pub attendance_path: String,
    #[serde(default)]
    pub captcha_selector: String,
    #[serde(default)]
    pub captcha_input_selector: String,
    #[serde(default)]
    pub already_keywords: String,
    #[serde(default = "default_submit_method")]
    pub submit_method: String,
    #[serde(default)]
    pub result_rules: Vec<SignInResultRule>,
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
            attendance_path: default_attendance_path(),
            captcha_selector: String::new(),
            captcha_input_selector: String::new(),
            already_keywords: String::new(),
            submit_method: default_submit_method(),
            result_rules: Vec::new(),
            cf_mode: default_browserless_cf_mode(),
            wait_ms: None,
            solve_timeout: None,
            action_timeout: None,
            post_click_wait_ms: None,
        }
    }
}

fn default_attendance_path() -> String {
    "/attendance.php".to_string()
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
    let signer = signers::resolve(
        &base_url,
        &task.browser,
        &task.sign_in_method,
        &task.browserless,
    );
    let output = signer
        .sign_in(
            base_url.clone(),
            cookie.clone(),
            &task.browserless,
            &settings,
        )
        .await;
    if is_qingwa_url(&base_url) {
        match qingwa_bonus_exchange(&site, &settings, &cookie).await {
            Ok(message) => tracing::info!("[签到][{}] 青蛙附加兑换: {}", task.name, message),
            Err(message) => tracing::warn!("[签到][{}] 青蛙附加兑换: {}", task.name, message),
        }
    }
    let output = output?;
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

fn is_qingwa_url(base_url: &str) -> bool {
    reqwest::Url::parse(base_url).ok().is_some_and(|url| {
        matches!(
            url.host_str()
                .unwrap_or_default()
                .trim_start_matches("www."),
            "qingwapt.com" | "qingwapt.org" | "qingwa.pro"
        )
    })
}

fn summarize_qingwa_exchange(value: &Value) -> String {
    let message = value
        .get("msg")
        .and_then(Value::as_str)
        .unwrap_or("响应缺少 msg");
    let status = if value.get("success").and_then(Value::as_bool) == Some(true) {
        "兑换成功"
    } else if value.get("success").and_then(Value::as_bool) == Some(false)
        && message.trim_end_matches(['。', '.']) == "超过限购数量"
    {
        "已达限购数量（重复调用）"
    } else {
        "兑换失败"
    };
    format!("{}: {}", status, compact_text(message).unwrap_or_default())
}

async fn qingwa_bonus_exchange(
    site: &SiteRecord,
    settings: &GlobalConfig,
    cookie: &str,
) -> Result<String, String> {
    let base_url = site.base_url.trim_end_matches('/');
    let mut builder = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none());
    if site.use_proxy {
        if let Some(proxy) = settings.proxy.as_deref().filter(|v| !v.trim().is_empty()) {
            builder = builder.proxy(reqwest::Proxy::all(proxy).map_err(|_| "代理配置无效")?);
        }
    }
    let headers = crate::site::site_request_header_map(&crate::site::parse_site_request_headers(
        &site.request_headers,
    )?)?;
    let response = builder.build().map_err(|_| "创建 HTTP 客户端失败")?
        .post(format!("{}/api/bonus-shop/exchange", base_url))
        .headers(headers)
        .header("accept", "*/*")
        .header("accept-language", "zh-CN,zh;q=0.9,en-US;q=0.8,en;q=0.7,zh-TW;q=0.6")
        .header("cookie", cookie)
        .header("origin", base_url)
        .header("referer", format!("{}/bonusshop.php", base_url))
        .header("dnt", "1")
        .header("priority", "u=1, i")
        .header("sec-ch-ua", "\"Chromium\";v=\"152\", \"Not?A_Brand\";v=\"24\", \"Google Chrome\";v=\"152\"")
        .header("sec-ch-ua-mobile", "?0")
        .header("sec-ch-ua-platform", "\"Windows\"")
        .header("sec-fetch-dest", "empty")
        .header("sec-fetch-mode", "cors")
        .header("sec-fetch-site", "same-origin")
        .header("user-agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/152.0.0.0 Safari/537.36")
        .multipart(reqwest::multipart::Form::new().text("id", "28").text("amount", "1"))
        .send().await.map_err(|_| "兑换 HTTP 请求失败或超时")?;
    let status = response.status();
    let value = response
        .json::<Value>()
        .await
        .map_err(|_| format!("兑换返回无效 JSON（HTTP {}）", status))?;
    Ok(format!(
        "HTTP {} {}",
        status,
        summarize_qingwa_exchange(&value)
    ))
}

fn browserless_sign_in_query(image_captcha: bool, script_submit: bool) -> String {
    let solve = if image_captcha {
        "solve: solveImageCaptcha(captchaSelector: $captchaSelector, inputSelector: $captchaInputSelector, timeout: $solveTimeout) { found solved time }"
    } else {
        "solve(type: cloudflare, timeout: $solveTimeout) { found solved time }"
    };
    let (submit_variables, submit_action) = if script_submit {
        (
            "$submitScript: String!",
            "submitForm: evaluate(content: $submitScript) { value }",
        )
    } else {
        (
            "$selector: String!",
            "click(selector: $selector, visible: true, timeout: $actionTimeout) { time }",
        )
    };
    let action_variables = if script_submit {
        ""
    } else {
        "$actionTimeout: Float!"
    };
    let image_variables = if image_captcha {
        "$captchaSelector: String! $captchaInputSelector: String!"
    } else {
        ""
    };
    format!(
        r#"
mutation CheckIn($cookies: [CookieInput!]! $url: String! {submit_variables}
 $waitMs: Float! $solveTimeout: Float! {action_variables} $postClickWaitMs: Float!
 $guard: String! $resultScript: String! $submitCondition: String! {image_variables}) {{
 cookies(cookies: $cookies) {{ cookies {{ name }} }}
 goto(url: $url, waitUntil: networkIdle) {{ status }}
 waitBefore: waitForTimeout(time: $waitMs) {{ time }}
 checkBefore: evaluate(content: $guard) {{ value }}
 beforeClick: html {{ html }}
 pending: ifnot(selector: "html[data-rflush-stop]") {{
  {solve}
 }}
 checkAfter: evaluate(content: $guard) {{ value }}
 afterSolve: html {{ html }}
 submit: if(selector: $submitCondition) {{
  {submit_action}
 }}
 waitAfter: waitForTimeout(time: $postClickWaitMs) {{ time }}
 html {{ html }}
 result: evaluate(content: $resultScript) {{ value }}
}}
"#
    )
}

fn browserless_submit_condition(selector: &str, image_captcha: bool) -> String {
    if image_captcha {
        "html:not([data-rflush-stop])[data-rflush-image-ready]".to_string()
    } else {
        format!("html:not([data-rflush-stop]) :is({selector})")
    }
}

fn form_submit_script(selector: &str, method: &str, timeout: u64) -> String {
    format!(
        r#"(async () => {{
 const target = document.querySelector({});
 const form = target instanceof HTMLFormElement ? target : target?.form;
 if (!(form instanceof HTMLFormElement)) throw new Error('未找到提交目标对应的表单');
 if ({} === 'form') {{ HTMLFormElement.prototype.submit.call(form); return 'submitted'; }}
 const action = new URL(form.action, location.href);
 if (action.origin !== location.origin) throw new Error('AJAX 提交仅支持本站表单');
 const data = new FormData(form);
 if (target?.name && !target.disabled) data.append(target.name, target.value);
 const method = form.method.toUpperCase();
 let body;
 if (method === 'GET') {{ for (const [key, value] of data) action.searchParams.append(key, String(value)); }}
 else if (form.enctype === 'multipart/form-data') body = data;
 else body = new URLSearchParams(data);
 const response = await fetch(action.href, {{ method, credentials: 'same-origin', body, signal: AbortSignal.timeout({}) }});
 const text = await response.text();
 window.__rflushResponse = {{status: response.status, text}};
 // Show the response as text; do not execute scripts returned by the endpoint.
 const pre = document.createElement('pre'); pre.textContent = text;
 document.body.replaceChildren(pre);
 return response.status;
}})()"#,
        json!(selector),
        json!(method),
        timeout
    )
}

fn result_rule_script(
    config: &BrowserlessTaskConfig,
    before: bool,
    image_input: Option<&str>,
) -> String {
    format!(
        r#"(() => {{
 const rules = {};
 const before = {};
 const keywords = {};
 const input = {};
 const ready = input && !!document.querySelector(input)?.value?.trim();
 document.documentElement.toggleAttribute('data-rflush-image-ready', !!ready);
 const visible = el => !!el && !!el.getClientRects().length && getComputedStyle(el).visibility !== 'hidden' && getComputedStyle(el).display !== 'none';
 const text = selector => Array.from(document.querySelectorAll(selector || 'body')).filter(visible).map(el => el.innerText || '').join('\n');
 const response = window.__rflushResponse;
 let payload;
 try {{ payload = JSON.parse(response ? response.text : document.body?.innerText || ''); }} catch {{}}
 const matches = rule => {{
  if (rule.kind === 'selector') return Array.from(document.querySelectorAll(rule.selector)).some(visible);
  if (rule.kind === 'text') return text(rule.selector).includes(rule.value);
  if (rule.kind === 'json') {{
   let current = payload;
   for (const key of rule.field.slice(1).split('/').map(k => k.replace(/~1/g, '/').replace(/~0/g, '~'))) {{
    if (current === null || typeof current !== 'object' || !Object.prototype.hasOwnProperty.call(current, key)) return false;
    current = current[key];
   }}
   const expected = rule.value_type === 'number' ? Number(rule.value) : rule.value_type === 'boolean' ? rule.value === 'true' : rule.value_type === 'null' ? null : rule.value;
   return current === expected;
  }}
  return false;
 }};
 let result = null;
 if (!before && response && (response.status < 200 || response.status >= 300)) result = {{status:'failed', message:'表单提交失败（HTTP ' + response.status + '）'}};
 for (const outcome of ['failed', 'already', 'success']) {{
  if (result || (before && outcome === 'success')) continue;
  if (rules.some(rule => rule.outcome === outcome && matches(rule))) result = {{status:outcome, message:{{failed:'命中签到失败规则',already:'命中已签到规则',success:'命中签到成功规则'}}[outcome]}};
 }}
 if (!result && keywords.some(word => text('').includes(word))) result = {{status:'already', message:'命中已签到提示'}};
 if (before) {{
  document.documentElement.toggleAttribute('data-rflush-stop', !!result);
  if (result) document.documentElement.setAttribute('data-rflush-outcome', JSON.stringify(result));
  else document.documentElement.removeAttribute('data-rflush-outcome');
 }} else {{
  const stopped = document.documentElement.getAttribute('data-rflush-outcome');
  if (stopped) result = JSON.parse(stopped);
 }}
 return JSON.stringify(result);
}})()"#,
        json!(config.result_rules),
        before,
        json!(
            config
                .already_keywords
                .lines()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
        ),
        json!(image_input)
    )
}

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
    sign_in_method: &str,
) -> Result<SignInOutput, String> {
    let timings = resolve_browserless_timings(&task_config)?;
    let image_captcha = sign_in_method == SIGN_IN_METHOD_OCR_CAPTCHA;
    let path = if image_captcha {
        task_config.attendance_path.as_str()
    } else {
        "/attendance.php"
    };
    let target_url = reqwest::Url::parse(&format!("{}{}", base_url, path))
        .map_err(|error| format!("签到地址无效: {}", error))?;
    let domain = target_url
        .host_str()
        .ok_or_else(|| "签到地址缺少域名".to_string())?;
    let script_submit = task_config.submit_method != "click";
    let cookies = browserless_cookies(&cookie_header, domain, target_url.as_str());
    if cookies.is_empty() {
        return Err("Cookie 不能为空".to_string());
    }

    // A generic submit selector otherwise clicks iloli's header search form.
    let selector = if !image_captcha
        && task_config.cf_mode == BROWSERLESS_CF_MODE_TURNSTILE
        && task_config.selector == DEFAULT_BROWSERLESS_SELECTOR
    {
        "form:has(.cf-turnstile) input[type='submit'], form[action*='attendance'] input[type='submit']"
    } else {
        &task_config.selector
    };
    let request_timeout_ms = timings
        .wait_ms
        .saturating_add(timings.solve_timeout)
        .saturating_add(timings.action_timeout.saturating_mul(2))
        .saturating_add(timings.post_click_wait_ms)
        .saturating_add(30_000);
    // Only top-level BQL mutations are ordered. In a nested conditional,
    // a sibling wait can finish before the conditional's click executes.
    // Keep submit and its wait at the top level, and include both guards in
    // the selector so stopped tasks and unsolved image captchas cannot submit.
    let submit_condition = browserless_submit_condition(selector, image_captcha);
    let result = post_browserless_bql(
        service_config,
        &browserless_sign_in_query(image_captcha, script_submit),
        "CheckIn",
        json!({
            "cookies": cookies,
            "guard": result_rule_script(&task_config, true, image_captcha.then_some(task_config.captcha_input_selector.as_str())),
            "submitScript": form_submit_script(selector, &task_config.submit_method, timings.action_timeout),
            "resultScript": result_rule_script(&task_config, false, None),
            "submitCondition": submit_condition,
            "captchaSelector": task_config.captcha_selector,
            "captchaInputSelector": task_config.captcha_input_selector,
            "url": target_url.as_str(),
            "selector": selector,
            "waitMs": timings.wait_ms,
            "solveTimeout": timings.solve_timeout,
            "actionTimeout": timings.action_timeout,
            "postClickWaitMs": timings.post_click_wait_ms,
        }),
        Duration::from_millis(request_timeout_ms),
    )
    .await?;

    summarize_configured_result(&result, !task_config.result_rules.is_empty())
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

fn classify_browserless_html(html: &str) -> Option<SignInOutput> {
    let document = Html::parse_document(html);
    let selector = Selector::parse("body").ok()?;
    let body = document.select(&selector).next()?;
    let text = body
        .descendants()
        .filter_map(|node| {
            let text = node.value().as_text()?;
            let hidden = node.ancestors().any(|ancestor| {
                ancestor
                    .value()
                    .as_element()
                    .is_some_and(|el| matches!(el.name(), "script" | "style" | "template"))
            });
            (!hidden).then_some(text.to_string())
        })
        .collect::<Vec<_>>()
        .join(" ");
    if text.contains("未登录") || text.contains("必须在登录后才能访问") {
        return None;
    }
    if document
        .root_element()
        .value()
        .attr("data-rflush-already")
        .is_some()
    {
        return Some(SignInOutput {
            status: "already".into(),
            message: "命中已签到提示，已跳过验证和点击".into(),
        });
    }
    if let Ok(value) = serde_json::from_str::<Value>(text.trim()) {
        if let Some(state) = value.get("state").and_then(Value::as_str) {
            if state == "success" {
                return Some(SignInOutput {
                    status: "success".into(),
                    message: "Browserless 签到成功".into(),
                });
            }
            if let Some(message) = value
                .get("msg")
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
            {
                let already = [
                    "已经签到",
                    "已經簽到",
                    "已签到",
                    "已簽到",
                    "重复签到",
                    "重複簽到",
                ]
                .iter()
                .any(|phrase| message.contains(phrase));
                return Some(SignInOutput {
                    status: if already { "already" } else { "failed" }.into(),
                    message: message.to_string(),
                });
            }
            return Some(SignInOutput {
                status: "failed".into(),
                message: format!("站点返回签到失败（state={}），未提供具体原因", state),
            });
        }
    }
    // A calendar legend can say 已签到 even after a successful submission.
    if ["签到成功", "簽到成功", "成功签到"]
        .iter()
        .any(|phrase| text.contains(phrase))
    {
        return Some(SignInOutput {
            status: "success".into(),
            message: "Browserless 签到成功".into(),
        });
    }
    // Calendar legends are not proof of completion while a captcha form remains.
    if document
        .select(&Selector::parse("input[name='imagestring']").ok()?)
        .next()
        .is_some()
    {
        return None;
    }
    let (status, message) = if [
        "已签到",
        "已经签到",
        "今日已签",
        "今天已签",
        "重复签到",
        "签到已得",
        "已簽到",
        "已經簽到",
    ]
    .iter()
    .any(|phrase| text.contains(phrase))
    {
        ("already", "今日已经签到")
    } else if ["签到成功", "簽到成功", "成功签到"]
        .iter()
        .any(|phrase| text.contains(phrase))
    {
        ("success", "Browserless 签到成功")
    } else {
        return None;
    };
    Some(SignInOutput {
        status: status.to_string(),
        message: message.to_string(),
    })
}

fn summarize_configured_result(result: &Value, configured: bool) -> Result<SignInOutput, String> {
    for path in [
        "/data/result/value",
        "/data/checkAfter/value",
        "/data/pending/checkAfter/value",
        "/data/checkBefore/value",
    ] {
        if let Some(value) = result.pointer(path).and_then(Value::as_str)
            && let Ok(value) = serde_json::from_str::<Value>(value)
            && let (Some(status), Some(message)) = (
                value.get("status").and_then(Value::as_str),
                value.get("message").and_then(Value::as_str),
            )
            && matches!(status, "success" | "already" | "failed")
        {
            return Ok(SignInOutput {
                status: status.into(),
                message: message.into(),
            });
        }
    }
    if configured {
        if let Some(error) = browserless_error_message(result) {
            return Err(error);
        }
        return Ok(SignInOutput {
            status: "failed".into(),
            message: "结果未知：未命中任何签到结果规则".into(),
        });
    }
    summarize_browserless_sign_in(result)
}

fn summarize_browserless_sign_in(result: &Value) -> Result<SignInOutput, String> {
    // A completed attendance page may have no button; BQL still returns HTML
    // after a selector timeout. Inspect visible outcome text before action errors.
    for path in [
        "/data/html/html",
        "/data/afterSolve/html",
        "/data/pending/afterSolve/html",
        "/data/beforeClick/html",
    ] {
        if let Some(html) = result.pointer(path).and_then(Value::as_str) {
            if let Some(output) = classify_browserless_html(html) {
                return Ok(output);
            }
        }
    }
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

    let solve_data = data.get("pending").unwrap_or(data);
    let solve_found = solve_data.pointer("/solve/found").and_then(Value::as_bool);
    let solve_solved = solve_data.pointer("/solve/solved").and_then(Value::as_bool);
    if solve_found == Some(true) && solve_solved == Some(false) {
        return Err("Browserless 找到验证码，但未能完成验证".to_string());
    }
    if data.get("click").is_none_or(Value::is_null)
        && data.pointer("/submit/click").is_none_or(Value::is_null)
        && data
            .pointer("/submit/submitForm/value")
            .is_none_or(Value::is_null)
        && data
            .pointer("/pending/pending/submit/click")
            .is_none_or(Value::is_null)
        && data
            .pointer("/pending/pending/submit/submitForm/value")
            .is_none_or(Value::is_null)
    {
        return Err("Browserless 未执行签到点击，请检查 selector".to_string());
    }

    let html = data
        .pointer("/html/html")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if html.contains("未登录") || html.contains("必须在登录后才能访问") {
        return Err("Cookie 无效或已过期".to_string());
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
const CDP_IO_POLL_INTERVAL: Duration = Duration::from_millis(10);
const CDP_SHUTDOWN_ERROR: &str = "服务正在退出，已取消浏览器操作";

struct AbortTask(tokio::task::JoinHandle<()>);

impl Drop for AbortTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

struct CdpConnectionState {
    socket: TcpStream,
    interrupted: AtomicBool,
}

impl CdpConnectionState {
    fn interrupt(&self) {
        self.interrupted.store(true, Ordering::Release);
        let _ = self.socket.shutdown(Shutdown::Both);
    }
}

struct InterruptSocket(Arc<CdpConnectionState>);

impl Drop for InterruptSocket {
    fn drop(&mut self) {
        self.0.interrupt();
    }
}

struct CdpConnectionGuard {
    socket: InterruptSocket,
    _watcher: AbortTask,
}

impl CdpConnectionGuard {
    fn new(stream: &TcpStream, shutdown: CancellationToken) -> Result<Self, String> {
        let socket = InterruptSocket(Arc::new(CdpConnectionState {
            socket: stream.try_clone().map_err(|error| error.to_string())?,
            interrupted: AtomicBool::new(false),
        }));
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
            socket.interrupt();
        }))
    }
}

// Present blocking Read/Write semantics to tungstenite and rustls while each
// actual socket operation remains nonblocking. On Windows, shutdown on a cloned
// handle does not reliably interrupt a recv already blocked in a handshake.
struct CdpStream {
    socket: TcpStream,
    connection: Arc<CdpConnectionState>,
    shutdown: CancellationToken,
}

impl CdpStream {
    fn wait_for_io<T>(
        &mut self,
        mut operation: impl FnMut(&mut TcpStream) -> io::Result<T>,
    ) -> io::Result<T> {
        let deadline = Instant::now() + CDP_IO_TIMEOUT;
        loop {
            if self.shutdown.is_cancelled() || self.connection.interrupted.load(Ordering::Acquire) {
                // Interrupted is retried by the TLS/HTTP handshake layers;
                // cancellation must instead terminate the current operation.
                return Err(io::Error::new(
                    io::ErrorKind::ConnectionAborted,
                    CDP_SHUTDOWN_ERROR,
                ));
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "浏览器 CDP 读写超时",
                ));
            }
            match operation(&mut self.socket) {
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(CDP_IO_POLL_INTERVAL.min(remaining));
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                result => return result,
            }
        }
    }
}

impl Read for CdpStream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.wait_for_io(|socket| socket.read(buffer))
    }
}

impl Write for CdpStream {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.wait_for_io(|socket| socket.write(buffer))
    }

    fn flush(&mut self) -> io::Result<()> {
        self.wait_for_io(TcpStream::flush)
    }
}

struct CdpClient {
    socket: tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<CdpStream>>,
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
                .set_nonblocking(true)
                .map_err(|error| error.to_string())?;
            stream
                .set_nodelay(true)
                .map_err(|error| error.to_string())?;
            let connection = CdpConnectionGuard::new(&stream, shutdown.clone())?;
            let handshake_deadline = connection.deadline(CDP_IO_TIMEOUT);
            let stream = CdpStream {
                socket: stream,
                connection: connection.socket.0.clone(),
                shutdown: shutdown.clone(),
            };
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
        if self.shutdown.is_cancelled()
            || self.connection.socket.0.interrupted.load(Ordering::Acquire)
        {
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
            if self.shutdown.is_cancelled()
                || self.connection.socket.0.interrupted.load(Ordering::Acquire)
            {
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
        self.connection.socket.0.interrupt();
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
    /// Optional live regression: the fixture stays in about:blank and never
    /// sends site cookies or submits to a real tracker. The JSON file contains
    /// {"browserless":{"address":"...","token":"..."}}.
    #[tokio::test]
    #[ignore = "requires BROWSERLESS_TEST_CONFIG pointing to a private JSON file"]
    async fn browserless_live_waits_after_conditional_submit() {
        let path = std::env::var("BROWSERLESS_TEST_CONFIG").expect("BROWSERLESS_TEST_CONFIG");
        let config: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let service: BrowserlessConfig =
            serde_json::from_value(config["browserless"].clone()).unwrap();
        // Keep the production query's control flow. Replace navigation and
        // CAPTCHA solving with a local fixture and a deterministic short wait.
        let query = browserless_sign_in_query(false, false)
            .replace(
                "goto(url: $url, waitUntil: networkIdle) { status }",
                "goto: evaluate(content: $url) { value }",
            )
            .replace(
                "solve(type: cloudflare, timeout: $solveTimeout) { found solved time }",
                "solve: waitForTimeout(time: $solveTimeout) { time }",
            );
        for (already, image_input) in [(false, None), (true, None), (false, Some("#answer"))] {
            let task = BrowserlessTaskConfig {
                already_keywords: "今日已经签到".into(),
                ..Default::default()
            };
            let setup = format!(
                r#"(() => {{
 document.documentElement.removeAttribute('data-rflush-stop');
 document.documentElement.removeAttribute('data-rflush-outcome');
 window.testClicks = 0;
 document.body.innerHTML = '<input type="submit" value="立即签到"><input id="answer" name="imagestring"><p id="outcome">{}</p>';
 document.querySelector('input').onclick = () => {{
  window.testClicks++;
  setTimeout(() => {{ document.querySelector('#outcome').textContent = '签到成功'; }}, 800);
 }};
 return 'ready';
}})()"#,
                if already {
                    "今日已经签到"
                } else {
                    "待签到"
                }
            );
            let result = post_browserless_bql(
                &service,
                &query.replace(
                    "result: evaluate(content: $resultScript) { value }",
                    "result: evaluate(content: $resultScript) { value }\n clicks: evaluate(content: \"String(window.testClicks)\") { value }",
                ),
                "CheckIn",
                json!({
                    "cookies": [], "url": setup,
                    "guard": result_rule_script(&task, true, image_input),
                    "resultScript": result_rule_script(&task, false, None),
                    "selector": task.selector,
                    "submitCondition": browserless_submit_condition(&task.selector, image_input.is_some()),
                    "waitMs": 1, "solveTimeout": 50,
                    "actionTimeout": 3000, "postClickWaitMs": 1500,
                }),
                Duration::from_secs(30),
            ).await.unwrap();
            assert!(
                result.get("errors").is_none(),
                "BQL fixture returned errors"
            );
            if image_input.is_none() {
                let output = summarize_configured_result(&result, false).unwrap();
                assert_eq!(output.status, if already { "already" } else { "success" });
            } else {
                assert!(result.pointer("/data/submit").unwrap().is_null());
            }
            assert_eq!(
                result.pointer("/data/clicks/value").and_then(Value::as_str),
                Some(if already || image_input.is_some() {
                    "0"
                } else {
                    "1"
                })
            );
        }
    }

    #[test]
    fn browserless_flat_results_preserve_guards_and_submission_errors() {
        let guarded = json!({"data": {
            "checkAfter": {"value": r#"{"status":"already","message":"guard"}"#},
            "submit": null
        }});
        assert_eq!(
            summarize_configured_result(&guarded, true).unwrap().status,
            "already"
        );
        let solved_page = json!({"data": {
            "afterSolve": {"html": "<body>签到成功</body>"}, "submit": null
        }});
        assert_eq!(
            summarize_browserless_sign_in(&solved_page).unwrap().status,
            "success"
        );
        for submit in [
            json!({"click": {"time": 10}}),
            json!({"submitForm": {"value": "200"}}),
        ] {
            let result = json!({"data": {
                "goto": {"status": 200}, "submit": submit,
                "html": {"html": "<body>立即签到</body>"}
            }});
            let output = summarize_browserless_sign_in(&result).unwrap();
            assert_eq!(output.status, "failed");
            assert!(output.message.contains("仍停留"));
        }
    }

    #[test]
    fn configured_results_require_a_match_and_preserve_outcomes() {
        for status in ["success", "already", "failed"] {
            let result = serde_json::json!({"data":{"result":{"value":serde_json::json!({"status":status,"message":"rule"}).to_string()}}});
            assert_eq!(
                super::summarize_configured_result(&result, true)
                    .unwrap()
                    .status,
                status
            );
        }
        let unknown = serde_json::json!({"data":{"html":{"html":"<body>签到成功</body>"},"result":{"value":"null"}}});
        let output = super::summarize_configured_result(&unknown, true).unwrap();
        assert_eq!(output.status, "failed");
        assert!(output.message.contains("结果未知"));
        assert_eq!(
            super::summarize_configured_result(&unknown, false)
                .unwrap()
                .status,
            "success"
        );
        let old: super::BrowserlessTaskConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(old.submit_method, "click");
        assert!(old.result_rules.is_empty());
    }

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
    fn opencd_json_rejections_report_the_server_result() {
        for (response, expected_status, expected_message) in [
            (r#"{"state":"false"}"#, "failed", "state=false"),
            (
                r#"{"state":"error","msg":"验证码错误"}"#,
                "failed",
                "验证码错误",
            ),
            (
                r#"{"state":"error","msg":"今天已经签到"}"#,
                "already",
                "今天已经签到",
            ),
        ] {
            let result = summarize_browserless_sign_in(&json!({"data": {
                "goto": {"status": 200},
                "pending": {"pending": {"submit": {"submitForm": {"value": "200"}}}},
                "html": {"html": format!("<body><pre>{}</pre></body>", response)}
            }}))
            .unwrap();
            assert_eq!(result.status, expected_status);
            assert!(result.message.contains(expected_message));
        }
    }

    #[test]
    fn browserless_image_result_and_already_guard_are_distinct() {
        let success = summarize_browserless_sign_in(&json!({"data": {
            "html": {"html": "<html><body><pre>{\"state\":\"success\",\"integral\":\"10\"}</pre></body></html>"}
        }})).unwrap();
        assert_eq!(success.status, "success");
        let calendar =
            classify_browserless_html("<body><h1>签到成功</h1><p>橙色日期代表已签到</p></body>")
                .unwrap();
        assert_eq!(calendar.status, "success");
        assert!(
            classify_browserless_html(
                "<body>橙色日期代表已签到<form><input name='imagestring'></form></body>"
            )
            .is_none()
        );
        let already = summarize_browserless_sign_in(&json!({"data": {
            "goto": {"status": 200}, "pending": null,
            "html": {"html": "<html data-rflush-already><body>您已领取今天的奖励</body></html>"}
        }}))
        .unwrap();
        assert_eq!(already.status, "already");
        let failed = summarize_browserless_sign_in(&json!({"data": {
            "goto": {"status": 200}, "pending": {"solve": {"found": true, "solved": false}},
            "html": {"html": "<body><form>验证码<input name='imagestring'></form></body>"}
        }}));
        assert!(failed.is_err());
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
    fn browserless_completed_page_survives_missing_button_errors() {
        for (html, status) in [
            ("<main>您今天已经签到，请勿重复签到。</main>", "already"),
            ("<main>签到成功</main>", "success"),
            ("<main>签到已得100</main>", "already"),
        ] {
            let output = summarize_browserless_sign_in(&json!({
                "errors": [{"message": "selector timeout"}],
                "data": {"goto": {"status": 200}, "click": null, "html": {"html": html}}
            }))
            .unwrap();
            assert_eq!(output.status, status);
        }
        assert!(
            classify_browserless_html(
                "<script>const message='签到成功';</script><main>立即签到</main>"
            )
            .is_none()
        );
        assert!(
            classify_browserless_html(&format!("<main>{}签到成功</main>", "站点导航 ".repeat(100)))
                .is_some()
        );
    }

    #[test]
    fn qingwa_exchange_classifies_limits_and_failures() {
        assert!(
            summarize_qingwa_exchange(&json!({"success":false,"msg":"超过限购数量。"}))
                .contains("重复调用")
        );
        assert!(
            summarize_qingwa_exchange(&json!({"success":true,"msg":"兑换成功"}))
                .starts_with("兑换成功")
        );
        assert!(
            summarize_qingwa_exchange(&json!({"success":false,"msg":"余额不足"}))
                .starts_with("兑换失败")
        );
        assert!(is_qingwa_url("https://www.qingwapt.com/"));
        assert!(!is_qingwa_url("https://www.qingwapt.com.example.org/"));
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
