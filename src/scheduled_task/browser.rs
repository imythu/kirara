use super::HttpConfig;
use crate::config::{BrowserlessConfig, GlobalConfig};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::json;
use std::time::Duration;

pub(super) fn endpoint(config: &BrowserlessConfig) -> Result<reqwest::Url, String> {
    let address = config.address.as_deref().unwrap_or_default().trim();
    let token = config.token.as_deref().unwrap_or_default().trim();
    if address.is_empty() || token.is_empty() {
        return Err("请先在自动签到的工具配置中设置 Browserless 地址和 Token".into());
    }
    let mut url = reqwest::Url::parse(address).map_err(|_| "Browserless 地址无效".to_string())?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err("Browserless 地址应为不含用户名密码的 HTTP / HTTPS 地址".into());
    }
    let path = url.path().trim_end_matches('/');
    let prefix = path
        .strip_suffix("/stealth/bql")
        .or_else(|| path.strip_suffix("/bql"))
        .or_else(|| path.strip_suffix("/stealth"))
        .unwrap_or(path);
    let path = if prefix.ends_with("/function") {
        prefix.to_string()
    } else {
        format!("{prefix}/function")
    };
    url.set_path(&path);
    url.set_fragment(None);
    let pairs = url
        .query_pairs()
        .filter(|(k, _)| k != "token")
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect::<Vec<_>>();
    url.set_query(None);
    url.query_pairs_mut()
        .extend_pairs(pairs)
        .append_pair("token", token);
    Ok(url)
}

// Browserless executes this fixed program, never user-provided JavaScript. CDP
// interception preserves binary bodies and suppresses scripts and subresources.
pub(super) const SCRIPT: &str = include_str!("browser-request.js");
pub(super) async fn execute(config: &HttpConfig, settings: &GlobalConfig) -> Result<u16, String> {
    let endpoint = endpoint(&settings.browserless)?;
    let mut request = config.build_request(&reqwest::Client::new())?;
    let body = request
        .body_mut()
        .take()
        .unwrap_or_else(|| Vec::<u8>::new().into());
    let body = reqwest::Response::from(axum::http::Response::new(body))
        .bytes()
        .await
        .map_err(|_| "无法读取浏览器请求体".to_string())?;
    let headers=request.headers().iter().filter(|(name,_)|name.as_str()!="content-length").map(|(name,value)|json!({"name":name.as_str(),"value":value.to_str().unwrap_or_default()})).collect::<Vec<_>>();
    let context = json!({"url":request.url().as_str(),"method":request.method().as_str(),"headers":headers,"body":STANDARD.encode(body),"hasBody":config.body_type!="none","timeoutMs":config.timeout_seconds*1000,"followRedirects":config.follow_redirects});
    let mut client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(config.timeout_seconds));
    if let Some(proxy) = settings.effective_proxy(config.browser_use_global_proxy) {
        client =
            client.proxy(reqwest::Proxy::all(proxy).map_err(|_| "全局代理地址无效".to_string())?);
    }
    let response = client
        .build()
        .map_err(|_| "无法创建浏览器连接".to_string())?
        .post(endpoint)
        .json(&json!({"code":SCRIPT,"context":context}))
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                "连接或浏览器请求超时"
            } else {
                "无法连接 Browserless，请检查服务地址、Token 和连接代理"
            }
            .to_string()
        })?;
    if !response.status().is_success() {
        return Err("Browserless 执行失败，请检查 Token、服务状态及 /function API 支持情况".into());
    }
    let data: serde_json::Value = response
        .json()
        .await
        .map_err(|_| "Browserless 未返回有效的执行结果".to_string())?;
    data.get("status_code")
        .and_then(|v| v.as_u64())
        .filter(|code| (100..=599).contains(code))
        .map(|code| code as u16)
        .ok_or("浏览器请求失败或未返回状态码".into())
}
