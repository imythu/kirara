use super::*;
use chrono::TimeZone;
fn config() -> HttpConfig {
    HttpConfig {
        send_via_browser: false,
        use_global_proxy: false,
        browser_use_global_proxy: false,
        method: "GET".into(),
        url: "http://127.0.0.1/secret-path?token=secret".into(),
        query: vec![],
        headers: vec![],
        bearer_token: "secret".into(),
        auth: None,
        form_fields: vec![],
        binary: None,
        content_type: String::new(),
        body: String::new(),
        body_type: "none".into(),
        timeout_seconds: 5,
        follow_redirects: false,
        expected_status: None,
    }
}
fn request() -> TaskRequest {
    TaskRequest {
        name: "Test".into(),
        task_type: "http".into(),
        enabled: true,
        timing: Timing::Interval { minutes: 5 },
        http: Some(config()),
    }
}
#[test]
fn interval_and_cron_validation_and_timezones() {
    let at = Utc.with_ymd_and_hms(2026, 9, 13, 0, 0, 0).unwrap();
    assert_eq!(
        Timing::Interval { minutes: 90 }.next(at).unwrap(),
        at + chrono::Duration::minutes(90)
    );
    assert!(Timing::Interval { minutes: 0 }.next(at).is_err());
    assert_eq!(
        Timing::Cron {
            expression: "0 9 * * *".into(),
            utc_offset_minutes: 480
        }
        .next(at)
        .unwrap(),
        at + chrono::Duration::hours(1)
    );
    assert!(
        Timing::Cron {
            expression: "wrong".into(),
            utc_offset_minutes: 0
        }
        .next(at)
        .is_err()
    );
    assert!(
        Timing::Cron {
            expression: "0 9 31 2 *".into(),
            utc_offset_minutes: 0
        }
        .next(at)
        .is_err()
    );
}
#[test]
fn validates_http_and_redacts_summary() {
    let mut http = config();
    http.validate().unwrap();
    assert_eq!(http.summary(), "GET · 127.0.0.1");
    http.url = "file:///etc/passwd".into();
    assert!(http.validate().is_err());
    http = config();
    http.headers.push(Pair {
        name: "Authorization".into(),
        value: "secret".into(),
    });
    assert!(http.validate().is_err());
    http = config();
    http.method = "POST".into();
    http.body_type = "json".into();
    http.body = "{".into();
    assert!(http.validate().is_err());
}
#[tokio::test]
async fn durable_claim_pause_recovery_and_secret_preservation() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(dir.path()).await.unwrap();
    let id = db.save_scheduled_task(None, request()).await.unwrap();
    assert!(db.claim_scheduled_task(id, false).await.is_err());
    let before = db.list_scheduled_tasks().await.unwrap().remove(0);
    assert!(!serde_json::to_string(&before).unwrap().contains("secret"));
    let (_, run) = db.claim_scheduled_task(id, true).await.unwrap();
    assert!(db.claim_scheduled_task(id, true).await.is_err());
    assert!(db.delete_scheduled_task(id).await.is_err());
    db.set_scheduled_enabled(id, false).await.unwrap();
    db.recover_scheduled_tasks().await.unwrap();
    assert_eq!(
        db.scheduled_runs(id).await.unwrap()[0].status,
        "interrupted"
    );
    let mut body = request();
    body.http = None;
    body.enabled = false;
    body.name = "Renamed".into();
    db.save_scheduled_task(Some(id), body).await.unwrap();
    let after = db.list_scheduled_tasks().await.unwrap().remove(0);
    assert!(after.next_run_at.is_none());
    assert_eq!(after.http.unwrap().bearer_token, "secret");
    assert_eq!(db.scheduled_runs(id).await.unwrap()[0].id, run.id);
    db.delete_scheduled_task(id).await.unwrap();
    assert!(db.list_scheduled_tasks().await.unwrap().is_empty());
}
#[tokio::test]
async fn sends_real_http_and_records_failure_without_response_secrets() {
    use axum::{Router, http::StatusCode, routing::post};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = Router::new().route(
        "/hook",
        post(|headers: axum::http::HeaderMap, body: String| async move {
            assert_eq!(headers["authorization"], "Bearer secret");
            assert_eq!(body, "{\"ok\":true}");
            (StatusCode::SERVICE_UNAVAILABLE, "secret-response")
        }),
    );
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(dir.path()).await.unwrap();
    let mut body = request();
    let http = body.http.as_mut().unwrap();
    http.url = format!("http://127.0.0.1:{port}/hook");
    http.method = "POST".into();
    http.body_type = "json".into();
    http.body = "{\"ok\":true}".into();
    let id = db.save_scheduled_task(None, body).await.unwrap();
    let scheduler = Scheduler::new(db.clone());
    scheduler.trigger(id, true).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let runs = db.scheduled_runs(id).await.unwrap();
            if runs[0].status != "running" {
                assert_eq!(runs[0].status, "failed");
                assert_eq!(runs[0].status_code, Some(503));
                assert!(!serde_json::to_string(&runs).unwrap().contains("secret"));
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    server.abort();
}

#[test]
fn cron_uses_standard_weekdays_and_day_or_semantics() {
    let at = Utc.with_ymd_and_hms(2026, 9, 13, 0, 0, 0).unwrap(); // Sunday
    let next = |expression: &str| {
        Timing::Cron {
            expression: expression.into(),
            utc_offset_minutes: 0,
        }
        .next(at)
        .unwrap()
    };
    assert_eq!(next("0 9 * * 0"), at + chrono::Duration::hours(9));
    assert_eq!(next("0 9 * * 7"), next("0 9 * * SUN"));
    assert_eq!(next("0 9 * * 1-5"), next("0 9 * * MON-FRI"));
    assert_eq!(next("0 9 * * 5-7"), next("0 9 * * SUN"));
    assert_eq!(next("0 9 14 * SUN"), next("0 9 * * SUN"));
}

#[tokio::test]
async fn expanded_auth_and_body_types_match_real_wire_requests() {
    use super::http::{Auth, FilePayload, FormField};
    use axum::{
        Router,
        body::Bytes,
        extract::RawQuery,
        http::{HeaderMap, StatusCode},
        routing::any,
    };
    use base64::{Engine, engine::general_purpose::STANDARD};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/capture", listener.local_addr().unwrap());
    let (sender, mut receiver) = tokio::sync::mpsc::channel(20);
    let app = Router::new().route(
        "/capture",
        any(
            move |headers: HeaderMap, RawQuery(query): RawQuery, body: Bytes| {
                let sender = sender.clone();
                async move {
                    sender.send((headers, query, body)).await.unwrap();
                    StatusCode::NO_CONTENT
                }
            },
        ),
    );
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let mut c = config();
    c.url = url;
    c.method = "POST".into();
    c.bearer_token.clear();
    let cases = [
        (Auth::None, None),
        (
            Auth::Bearer {
                token: "new-token".into(),
            },
            Some(("authorization", "Bearer new-token".to_string())),
        ),
        (
            Auth::Basic {
                username: "用户名".into(),
                password: "p:a:ss".into(),
            },
            Some((
                "authorization",
                format!("Basic {}", STANDARD.encode("用户名:p:a:ss")),
            )),
        ),
        (
            Auth::ApiKey {
                name: "X-API-Key".into(),
                value: "header-key".into(),
                location: "header".into(),
            },
            Some(("x-api-key", "header-key".into())),
        ),
        (
            Auth::ApiKey {
                name: "api_key".into(),
                value: "a+b &中".into(),
                location: "query".into(),
            },
            None,
        ),
        (
            Auth::Cookie {
                value: "session=abc; user=42".into(),
            },
            Some(("cookie", "session=abc; user=42".into())),
        ),
    ];
    for (auth, expected) in cases {
        c.auth = Some(auth);
        let preview = serde_json::to_value(request_preview(&c).unwrap()).unwrap();
        assert_eq!(execute(&c).await.unwrap(), 204);
        let (headers, query, body) = receiver.recv().await.unwrap();
        assert!(body.is_empty());
        if let Some((name, value)) = expected {
            assert_eq!(headers[name], value);
            assert!(
                preview["headers"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|p| p["name"] == name && p["value"] == value)
            );
        }
        if matches!(c.auth,Some(Auth::ApiKey{ref location,..}) if location=="query") {
            assert_eq!(query.unwrap(), "api_key=a%2Bb+%26%E4%B8%AD");
            assert!(
                preview["url"]
                    .as_str()
                    .unwrap()
                    .ends_with("api_key=a%2Bb+%26%E4%B8%AD")
            );
        }
    }
    c.auth = Some(Auth::None);
    for (kind, mime, text) in [
        ("json", "application/json", "{\"ok\":true}"),
        ("text", "text/plain; charset=utf-8", "hello"),
        ("xml", "application/xml", "<ok>true</ok>"),
        ("html", "text/html; charset=utf-8", "<p>Hello</p>"),
        ("raw", "application/graphql", "{ viewer { id } }"),
    ] {
        c.body_type = kind.into();
        c.body = text.into();
        c.content_type = "application/graphql".into();
        let preview = serde_json::to_value(request_preview(&c).unwrap()).unwrap();
        assert_eq!(execute(&c).await.unwrap(), 204);
        let (headers, _, body) = receiver.recv().await.unwrap();
        assert_eq!(headers["content-type"], mime);
        assert_eq!(body, text);
        assert_eq!(preview["body"], text);
    }
    c.body_type = "urlencoded".into();
    c.form_fields = vec![
        FormField {
            name: "tag".into(),
            value: "a+b &中".into(),
            file: None,
        },
        FormField {
            name: "tag".into(),
            value: "second".into(),
            file: None,
        },
    ];
    let preview = serde_json::to_value(request_preview(&c).unwrap()).unwrap();
    execute(&c).await.unwrap();
    let (headers, _, body) = receiver.recv().await.unwrap();
    assert_eq!(headers["content-type"], "application/x-www-form-urlencoded");
    assert_eq!(body, "tag=a%2Bb+%26%E4%B8%AD&tag=second");
    assert_eq!(preview["body"], String::from_utf8(body.to_vec()).unwrap());
    let file = FilePayload {
        name: "sample.bin".into(),
        content_type: "application/octet-stream".into(),
        data_base64: STANDARD.encode([0, 1, 127, 255]),
    };
    c.body_type = "binary".into();
    c.binary = Some(file.clone());
    execute(&c).await.unwrap();
    let (headers, _, body) = receiver.recv().await.unwrap();
    assert_eq!(headers["content-type"], "application/octet-stream");
    assert_eq!(body.as_ref(), [0, 1, 127, 255]);
    c.body_type = "multipart".into();
    c.form_fields.push(FormField {
        name: "upload".into(),
        value: String::new(),
        file: Some(file),
    });
    execute(&c).await.unwrap();
    let (headers, _, body) = receiver.recv().await.unwrap();
    let boundary = headers["content-type"]
        .to_str()
        .unwrap()
        .strip_prefix("multipart/form-data; boundary=")
        .unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains(&format!("--{boundary}")));
    assert_eq!(text.matches("name=\"tag\"").count(), 2);
    assert!(text.contains("name=\"upload\"; filename=\"sample.bin\""));
    assert!(body.windows(4).any(|b| b == [0, 1, 127, 255]));
    server.abort();
}

#[test]
fn rejects_conflicting_auth_and_invalid_files() {
    use super::http::{Auth, FilePayload};
    let mut c = config();
    c.auth = Some(Auth::Basic {
        username: "a:b".into(),
        password: "x".into(),
    });
    assert!(c.validate().is_err());
    c.auth = Some(Auth::ApiKey {
        name: "token".into(),
        value: "new".into(),
        location: "query".into(),
    });
    assert!(c.validate().is_err()); // original URL already has token
    c.auth = Some(Auth::Cookie {
        value: "session=x\r\nInjected: value".into(),
    });
    assert!(c.validate().is_err());
    c.auth = Some(Auth::None);
    c.method = "POST".into();
    c.body_type = "multipart".into();
    c.headers.push(Pair {
        name: "Content-Type".into(),
        value: "multipart/form-data".into(),
    });
    assert!(c.validate().is_err());
    c.headers.clear();
    c.body_type = "binary".into();
    assert!(c.validate().is_err());
    c.binary = Some(FilePayload {
        name: "f".into(),
        content_type: "application/octet-stream".into(),
        data_base64: "invalid!".into(),
    });
    assert!(c.validate().is_err());
    c.binary.as_mut().unwrap().data_base64 = "A".repeat(400000);
    assert!(c.validate().is_err());
}

#[tokio::test]
async fn explicit_config_disclosure_preserves_new_auth_and_files_but_lists_stay_redacted() {
    use super::http::{Auth, FilePayload};
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(dir.path()).await.unwrap();
    let mut body = request();
    let c = body.http.as_mut().unwrap();
    c.method = "POST".into();
    c.auth = Some(Auth::Basic {
        username: "saved-user".into(),
        password: "saved-password".into(),
    });
    c.body_type = "binary".into();
    c.binary = Some(FilePayload {
        name: "saved-file".into(),
        content_type: "application/octet-stream".into(),
        data_base64: "AAE=".into(),
    });
    let id = db.save_scheduled_task(None, body).await.unwrap();
    let disclosed = db.scheduled_http_config(id).await.unwrap();
    let json = serde_json::to_string(&disclosed).unwrap();
    assert!(json.contains("saved-password") && json.contains("AAE="));
    let tasks = serde_json::to_string(&db.list_scheduled_tasks().await.unwrap()).unwrap();
    assert!(!tasks.contains("saved-password") && !tasks.contains("AAE="));
    let mut update = request();
    update.http = Some(disclosed);
    update.name = "updated".into();
    db.save_scheduled_task(Some(id), update).await.unwrap();
    assert!(
        serde_json::to_string(&db.scheduled_http_config(id).await.unwrap())
            .unwrap()
            .contains("saved-password")
    );
}

#[tokio::test]
async fn independent_delivery_proxy_switches_route_only_the_selected_connection() {
    use axum::{Json, Router, http::StatusCode, routing::any};
    use std::sync::atomic::{AtomicUsize, Ordering};
    let hits = std::sync::Arc::new(AtomicUsize::new(0));
    let count = hits.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new().fallback(any(move || {
        let count = count.clone();
        async move {
            count.fetch_add(1, Ordering::SeqCst);
            (
                StatusCode::ACCEPTED,
                Json(serde_json::json!({"status_code":201})),
            )
        }
    }));
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let mut settings = crate::config::GlobalConfig::default();
    settings.proxy = Some(proxy.clone());
    settings.browserless = crate::config::BrowserlessConfig {
        address: Some("http://browser.invalid".into()),
        token: Some("test".into()),
    };
    let mut c = config();
    c.url = "http://target.invalid/hook".into();
    c.use_global_proxy = true;
    assert_eq!(execute_with_settings(&c, &settings).await.unwrap(), 202);
    c.send_via_browser = true;
    c.browser_use_global_proxy = true;
    c.use_global_proxy = false;
    assert_eq!(execute_with_settings(&c, &settings).await.unwrap(), 201);
    assert_eq!(hits.load(Ordering::SeqCst), 2);
    let target = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", target.local_addr().unwrap());
    let direct = tokio::spawn(async move {
        axum::serve(
            target,
            Router::new().fallback(any(|| async { (StatusCode::NO_CONTENT, "") })),
        )
        .await
        .unwrap()
    });
    c.send_via_browser = false;
    c.url = address;
    c.use_global_proxy = false;
    assert_eq!(execute_with_settings(&c, &settings).await.unwrap(), 204);
    assert_eq!(hits.load(Ordering::SeqCst), 2);
    c.send_via_browser = true;
    c.browser_use_global_proxy = false;
    settings.browserless.address = Some(proxy);
    assert_eq!(execute_with_settings(&c, &settings).await.unwrap(), 201);
    assert_eq!(hits.load(Ordering::SeqCst), 3);
    settings.proxy = None;
    c.browser_use_global_proxy = true;
    assert!(
        execute_with_settings(&c, &settings)
            .await
            .unwrap_err()
            .contains("全局代理")
    );
    direct.abort();
    server.abort();
}

#[tokio::test]
#[ignore = "requires KIRARA_TEST_BROWSERLESS_URL pointing to a local Browserless function harness"]
async fn live_browser_delivery_preserves_auth_files_methods_and_redirects() {
    use axum::{
        Router,
        body::Bytes,
        http::{HeaderMap, StatusCode},
        response::Redirect,
        routing::any,
    };
    use base64::{Engine, engine::general_purpose::STANDARD};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let target = format!("http://{}", listener.local_addr().unwrap());
    let (sender, mut receiver) = tokio::sync::mpsc::channel(16);
    let app = Router::new()
        .route(
            "/capture",
            any(
                move |headers: HeaderMap, method: axum::http::Method, body: Bytes| {
                    let sender = sender.clone();
                    async move {
                        sender.send((headers, method, body)).await.unwrap();
                        (
                            StatusCode::MULTI_STATUS,
                            "<script>fetch('/unexpected')</script><img src='/unexpected'>",
                        )
                    }
                },
            ),
        )
        .route(
            "/redirect",
            any(|| async { Redirect::temporary("/capture") }),
        );
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let mut settings = crate::config::GlobalConfig::default();
    settings.browserless = crate::config::BrowserlessConfig {
        address: Some(std::env::var("KIRARA_TEST_BROWSERLESS_URL").unwrap()),
        token: Some("test".into()),
    };
    let mut c = config();
    c.send_via_browser = true;
    c.url = format!("{target}/capture");
    c.timeout_seconds = 15;
    c.method = "POST".into();
    c.auth = Some(http::Auth::Cookie {
        value: "session=browser-test".into(),
    });
    c.body_type = "binary".into();
    c.binary = Some(http::FilePayload {
        name: "sample.bin".into(),
        content_type: "application/octet-stream".into(),
        data_base64: STANDARD.encode([0, 1, 127, 255]),
    });
    assert_eq!(execute_with_settings(&c, &settings).await.unwrap(), 207);
    let (headers, method, body) = receiver.recv().await.unwrap();
    assert_eq!(headers["cookie"], "session=browser-test");
    assert!(headers["user-agent"].to_str().unwrap().contains("Chrome"));
    assert_eq!(method, "POST");
    assert_eq!(body.as_ref(), [0, 1, 127, 255]);
    c.body_type = "urlencoded".into();
    c.form_fields = vec![http::FormField {
        name: "tag".into(),
        value: "a +中".into(),
        file: None,
    }];
    assert_eq!(execute_with_settings(&c, &settings).await.unwrap(), 207);
    let (_, _, body) = receiver.recv().await.unwrap();
    assert_eq!(body, "tag=a+%2B%E4%B8%AD");
    c.body_type = "multipart".into();
    c.form_fields.push(http::FormField {
        name: "file".into(),
        value: String::new(),
        file: c.binary.clone(),
    });
    assert_eq!(execute_with_settings(&c, &settings).await.unwrap(), 207);
    let (headers, _, body) = receiver.recv().await.unwrap();
    assert!(
        headers["content-type"]
            .to_str()
            .unwrap()
            .starts_with("multipart/form-data; boundary=")
    );
    assert!(body.windows(4).any(|b| b == [0, 1, 127, 255]));
    c.method = "GET".into();
    c.body_type = "none".into();
    c.url = format!("{target}/redirect");
    c.follow_redirects = false;
    assert_eq!(execute_with_settings(&c, &settings).await.unwrap(), 307);
    assert!(receiver.try_recv().is_err());
    c.follow_redirects = true;
    assert_eq!(execute_with_settings(&c, &settings).await.unwrap(), 207);
    receiver.recv().await.unwrap();
    c.method = "HEAD".into();
    c.url = format!("{target}/capture");
    assert_eq!(execute_with_settings(&c, &settings).await.unwrap(), 207);
    let (_, method, body) = receiver.recv().await.unwrap();
    assert_eq!(method, "HEAD");
    assert!(body.is_empty());
    server.abort();
}
