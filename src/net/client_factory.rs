use std::sync::Once;
use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderValue, USER_AGENT};
use reqwest::redirect::Policy;
use reqwest::{Client, Proxy, Url};
use serde::Serialize;
use tracing::warn;

const BROWSER_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/145.0.0.0 Safari/537.36";
static MISSING_PROXY_WARNING: Once = Once::new();

/// 代理测试结果
#[derive(Debug, Clone, Serialize)]
pub struct ProxyTestResult {
    pub success: bool,
    pub status_code: Option<u16>,
    pub elapsed_ms: u64,
    pub message: String,
}

/// 构建一个配置了浏览器 UA 和超时的 reqwest Client Builder 基础模板。
fn base_builder() -> reqwest::ClientBuilder {
    let mut headers = HeaderMap::new();
    headers.insert(USER_AGENT, HeaderValue::from_static(BROWSER_USER_AGENT));
    Client::builder()
        .default_headers(headers)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
}

fn site_builder() -> reqwest::ClientBuilder {
    Client::builder()
        .redirect(Policy::custom(|attempt| {
            if attempt.previous().len() >= 10 {
                return attempt.error("站点重定向次数过多");
            }
            if attempt
                .previous()
                .first()
                .is_some_and(|origin| same_origin(origin, attempt.url()))
            {
                attempt.follow()
            } else {
                attempt.stop()
            }
        }))
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
}

fn same_origin(left: &Url, right: &Url) -> bool {
    left.scheme() == right.scheme()
        && left.host_str() == right.host_str()
        && left.port_or_known_default() == right.port_or_known_default()
}

/// 构建一个 reqwest::Client，可选带代理。
pub fn build_client(proxy: Option<&str>) -> Result<Client, reqwest::Error> {
    let mut builder = base_builder();
    if let Some(proxy_url) = proxy.map(str::trim).filter(|v| !v.is_empty()) {
        builder = builder.proxy(Proxy::all(proxy_url)?);
    }
    builder.build()
}

fn build_site_client(proxy: Option<&str>) -> Result<Client, reqwest::Error> {
    let builder = site_builder();
    let builder = match proxy.map(str::trim).filter(|value| !value.is_empty()) {
        Some(proxy_url) => builder.proxy(Proxy::all(proxy_url)?),
        // Disabling the per-site proxy must also disable reqwest's automatic environment proxy.
        None => builder.no_proxy(),
    };
    builder.build()
}

/// 构建站点专用客户端。请求头由站点适配器逐次添加，删除已保存的默认头后不会被客户端补回。
pub fn resolve_site_client(proxy: Option<&str>, use_proxy: bool) -> Result<Client, reqwest::Error> {
    let effective_proxy = proxy.map(str::trim).filter(|v| !v.is_empty());
    if use_proxy {
        if let Some(url) = effective_proxy {
            return build_site_client(Some(url));
        }
        MISSING_PROXY_WARNING.call_once(|| {
            warn!("站点标记了使用代理，但全局代理地址未配置，将直连访问");
        });
    }
    build_site_client(None)
}

/// 代理连通性测试：用指定代理 GET 一个 URL，返回结果。
pub async fn test_proxy(proxy: &str, test_url: &str) -> ProxyTestResult {
    let proxy = proxy.trim();
    if proxy.is_empty() {
        return ProxyTestResult {
            success: false,
            status_code: None,
            elapsed_ms: 0,
            message: "代理地址不能为空".to_string(),
        };
    }

    let client = match build_client(Some(proxy)) {
        Ok(c) => c,
        Err(e) => {
            return ProxyTestResult {
                success: false,
                status_code: None,
                elapsed_ms: 0,
                message: format!("创建代理客户端失败: {}", e),
            };
        }
    };

    let start = std::time::Instant::now();
    match client.get(test_url).send().await {
        Ok(resp) => {
            let elapsed = start.elapsed().as_millis() as u64;
            let status = resp.status();
            ProxyTestResult {
                success: status.is_success() || status.is_redirection(),
                status_code: Some(status.as_u16()),
                elapsed_ms: elapsed,
                message: format!("HTTP {}", status),
            }
        }
        Err(e) => {
            let elapsed = start.elapsed().as_millis() as u64;
            ProxyTestResult {
                success: false,
                status_code: None,
                elapsed_ms: elapsed,
                message: format!("请求失败: {}", e),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use reqwest::Url;

    use super::{resolve_site_client, same_origin};

    #[test]
    fn site_redirect_origin_requires_matching_scheme_host_and_effective_port() {
        let base = Url::parse("https://tracker.example/path").unwrap();
        assert!(same_origin(
            &base,
            &Url::parse("https://tracker.example:443/next").unwrap()
        ));
        assert!(!same_origin(
            &base,
            &Url::parse("http://tracker.example/next").unwrap()
        ));
        assert!(!same_origin(
            &base,
            &Url::parse("https://cdn.example/next").unwrap()
        ));
    }

    #[tokio::test]
    async fn site_proxy_usage_is_controlled_only_by_the_application_setting() {
        const ORIGIN_ENV: &str = "KIRARA_SITE_PROXY_TEST_ORIGIN";
        if let Ok(origin) = std::env::var(ORIGIN_ENV) {
            let proxy = std::env::var("HTTP_PROXY").unwrap();
            for (setting, enabled, expected) in [
                (Some(proxy.as_str()), false, "direct"),
                (None, false, "direct"),
                (Some(proxy.as_str()), true, "proxy"),
            ] {
                let response = resolve_site_client(setting, enabled)
                    .unwrap()
                    .get(&origin)
                    .send()
                    .await
                    .unwrap()
                    .text()
                    .await
                    .unwrap();
                assert_eq!(response, expected);
            }
            return;
        }

        let origin_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", origin_listener.local_addr().unwrap());
        let proxy = format!("http://{}", proxy_listener.local_addr().unwrap());
        let origin_server = tokio::spawn(async move {
            axum::serve(
                origin_listener,
                axum::Router::new().fallback(|| async { "direct" }),
            )
            .await
            .unwrap();
        });
        let proxy_server = tokio::spawn(async move {
            axum::serve(
                proxy_listener,
                axum::Router::new().fallback(|| async { "proxy" }),
            )
            .await
            .unwrap();
        });
        // Isolate environment overrides in a child test process; changing the current process
        // environment would race every other parallel HTTP test (and is unsafe in Rust 2024).
        let output = tokio::task::spawn_blocking(move || {
            std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "net::client_factory::tests::site_proxy_usage_is_controlled_only_by_the_application_setting", "--nocapture"])
                .env(ORIGIN_ENV, origin)
                .env("HTTP_PROXY", &proxy).env("http_proxy", &proxy)
                .env("HTTPS_PROXY", &proxy).env("https_proxy", &proxy)
                .env("ALL_PROXY", &proxy).env("all_proxy", &proxy)
                .env("NO_PROXY", "").env("no_proxy", "")
                .output().unwrap()
        }).await.unwrap();
        origin_server.abort();
        proxy_server.abort();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
