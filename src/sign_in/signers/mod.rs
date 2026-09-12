//! Site routing and signers. Browser transports stay in the parent module;
//! site-specific behavior belongs here, never in the settings UI.
mod baozi;
mod captcha;
mod cf_challenge;
mod cf_turnstile;
mod nexus;
mod opencd;
mod u2;

use super::*;
use std::future::Future;
use std::pin::Pin;

#[derive(Clone, Serialize)]
pub struct SignerProfile {
    pub id: &'static str,
    pub label: &'static str,
    pub browser: &'static str,
    pub sign_in_method: &'static str,
    pub browserless: BrowserlessTaskConfig,
}

pub(super) trait Signer: Send + Sync {
    fn profile(&self, config: &BrowserlessTaskConfig) -> SignerProfile;

    fn sign_in<'a>(
        &'a self,
        base_url: String,
        cookie: String,
        config: &'a BrowserlessTaskConfig,
        settings: &'a GlobalConfig,
    ) -> Pin<Box<dyn Future<Output = Result<SignInOutput, String>> + Send + 'a>> {
        Box::pin(async move {
            let profile = self.profile(config);
            if profile.browser == SIGN_IN_BROWSER_LIGHTPANDA {
                let endpoint = build_lightpanda_endpoint(
                    &settings.lightpanda,
                    settings.use_proxy_for_lightpanda,
                )?;
                run_cdp_sign_in(
                    endpoint,
                    base_url,
                    cookie,
                    profile.sign_in_method.into(),
                    settings
                        .effective_proxy(settings.use_global_proxy_for_lightpanda)
                        .map(str::to_owned),
                )
                .await
            } else {
                run_browserless_sign_in(
                    &settings.browserless,
                    settings.effective_proxy(settings.use_global_proxy_for_browserless),
                    base_url,
                    cookie,
                    profile.browserless,
                    profile.sign_in_method,
                )
                .await
            }
        })
    }
}

pub(super) fn known(base_url: &str) -> Option<Box<dyn Signer>> {
    let url = reqwest::Url::parse(base_url).ok()?;
    let host = url.host_str()?.trim_end_matches('.');
    if host == "open.cd" || host.ends_with(".open.cd") {
        return Some(Box::new(opencd::OpenCd));
    }
    match host.strip_prefix("www.").unwrap_or(host) {
        "u2.dmhy.org" => Some(Box::new(u2::U2)),
        "p.t-baozi.cc" => Some(Box::new(baozi::Baozi)),
        "dstudio.me" => Some(Box::new(cf_challenge::CfChallenge)),
        "mua.xloli.cc" | "share.ilolicon.com" => Some(Box::new(cf_turnstile::CfTurnstile)),
        _ => None,
    }
}

pub fn known_profile(base_url: &str) -> Option<SignerProfile> {
    known(base_url).map(|signer| signer.profile(&BrowserlessTaskConfig::default()))
}

pub(super) fn resolve(
    base_url: &str,
    browser: &str,
    method: &str,
    config: &BrowserlessTaskConfig,
) -> Box<dyn Signer> {
    known(base_url).unwrap_or_else(|| {
        if method == SIGN_IN_METHOD_OCR_CAPTCHA {
            Box::new(captcha::Captcha)
        } else if method == SIGN_IN_METHOD_CLOUDFLARE || browser == SIGN_IN_BROWSER_BROWSERLESS {
            if config.cf_mode == BROWSERLESS_CF_MODE_TURNSTILE {
                Box::new(cf_turnstile::CfTurnstile)
            } else {
                Box::new(cf_challenge::CfChallenge)
            }
        } else {
            Box::new(nexus::Nexus)
        }
    })
}

/// Apply at both API and execution boundaries, including tasks saved before 2.3.3.
pub fn normalize_request(base_url: &str, request: &mut SignInTaskRequest) {
    let config = request.browserless.clone().unwrap_or_default();
    let signer = resolve(
        base_url,
        request.browser.as_deref().unwrap_or("lightpanda"),
        request.sign_in_method.as_deref().unwrap_or("open_page"),
        &config,
    );
    let profile = signer.profile(&config);
    request.browser = Some(profile.browser.into());
    request.sign_in_method = Some(profile.sign_in_method.into());
    request.browserless = Some(profile.browserless);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn browserless_signers_use_their_expected_transport_and_results() {
        use axum::{Json, Router, routing::post};
        use std::sync::{Arc, Mutex};
        let captured = Arc::new(Mutex::new(Vec::<Value>::new()));
        let requests = captured.clone();
        let app = Router::new().route("/stealth/bql", post(move |Json(body): Json<Value>| {
            let requests = requests.clone();
            async move {
                let is_opencd = body["variables"]["url"].as_str().unwrap().contains("plugin_sign-in");
                requests.lock().unwrap().push(body);
                Json(if is_opencd {
                    json!({"data":{"result":{"value":r#"{"status":"success","message":"命中签到成功规则"}"#}}})
                } else {
                    json!({"data":{"goto":{"status":200},"html":{"html":"<body>签到成功</body>"}}})
                })
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let settings = GlobalConfig {
            browserless: BrowserlessConfig {
                address: Some(format!("http://{address}")),
                token: Some("test-token".into()),
            },
            ..Default::default()
        };
        for (url, image, ajax) in [
            ("https://check.open.cd", true, true),
            ("https://p.t-baozi.cc", true, false),
            ("https://dstudio.me", false, false),
            ("https://mua.xloli.cc", false, false),
            ("https://share.ilolicon.com", false, false),
        ] {
            let signer = known(url).unwrap();
            let output = signer
                .sign_in(
                    url.into(),
                    "uid=test".into(),
                    &BrowserlessTaskConfig::default(),
                    &settings,
                )
                .await
                .unwrap();
            assert_eq!(output.status, "success");
            let requests = captured.lock().unwrap();
            let request = requests.last().unwrap();
            let query = request["query"].as_str().unwrap();
            assert_eq!(query.contains("solveImageCaptcha"), image);
            assert_eq!(query.contains("submitForm: evaluate"), ajax);
            assert!(
                request["variables"]["url"]
                    .as_str()
                    .unwrap()
                    .ends_with(if ajax {
                        "/plugin_sign-in.php"
                    } else {
                        "/attendance.php"
                    })
            );
            if ajax {
                assert!(
                    request["variables"]["submitScript"]
                        .as_str()
                        .unwrap()
                        .contains("await fetch")
                );
            }
        }
        server.abort();
    }

    #[test]
    fn known_sites_override_stale_or_custom_configuration() {
        for (url, id, browser, mode) in [
            ("https://check.open.cd", "opencd", "browserless", "auto"),
            ("https://p.t-baozi.cc", "baozi", "browserless", "auto"),
            (
                "https://dstudio.me",
                "nexus_cf_challenge",
                "browserless",
                "page",
            ),
            (
                "https://mua.xloli.cc",
                "nexus_cf_turnstile",
                "browserless",
                "turnstile",
            ),
            (
                "https://share.ilolicon.com",
                "nexus_cf_turnstile",
                "browserless",
                "turnstile",
            ),
        ] {
            let stale = BrowserlessTaskConfig {
                selector: "#wrong".into(),
                submit_method: "form".into(),
                ..Default::default()
            };
            let profile = resolve(url, "lightpanda", "open_page", &stale).profile(&stale);
            assert_eq!(profile.id, id);
            assert_eq!(profile.browser, browser);
            assert_eq!(profile.browserless.cf_mode, mode);
            assert_ne!(profile.browserless.selector, "#wrong");
            assert_ne!(profile.browserless.submit_method, "form");
        }
        assert!(known_profile("https://open.cd.evil.example").is_none());
        assert!(known_profile("https://fakeopen.cd").is_none());
    }

    #[test]
    fn generic_captcha_keeps_only_the_five_fields() {
        let config = BrowserlessTaskConfig {
            attendance_path: "/check.php".into(),
            captcha_selector: "#image".into(),
            captcha_input_selector: "#answer".into(),
            selector: "#submit".into(),
            already_keywords: "完成".into(),
            submit_method: "ajax".into(),
            cf_mode: "turnstile".into(),
            ..Default::default()
        };
        let profile =
            resolve("https://example.org", "browserless", "ocr_captcha", &config).profile(&config);
        assert_eq!(profile.id, "captcha");
        assert_eq!(profile.browserless.attendance_path, "/check.php");
        assert_eq!(profile.browserless.captcha_selector, "#image");
        assert_eq!(profile.browserless.captcha_input_selector, "#answer");
        assert_eq!(profile.browserless.selector, "#submit");
        assert_eq!(profile.browserless.already_keywords, "完成");
        assert_eq!(profile.browserless.submit_method, "click");
        assert!(profile.browserless.result_rules.is_empty());
        let open =
            resolve("https://example.org", "lightpanda", "open_page", &config).profile(&config);
        assert_eq!(open.id, "nexus");
        assert_eq!(open.browser, "lightpanda");
    }

    #[test]
    fn opencd_extends_captcha_with_typed_response_rules() {
        let profile = known_profile("https://check.open.cd").unwrap();
        assert_eq!(profile.browserless.attendance_path, "/plugin_sign-in.php");
        assert_eq!(profile.browserless.submit_method, "ajax");
        assert!(
            profile
                .browserless
                .result_rules
                .iter()
                .any(|r| r.field == "/state"
                    && r.value == "false"
                    && r.value_type == "string"
                    && r.outcome == "success")
        );
        let baozi = known_profile("https://p.t-baozi.cc").unwrap();
        assert_eq!(baozi.browserless.already_keywords, "签到成功");
        assert_eq!(baozi.browserless.submit_method, "click");
    }
}
