//! Exercise the actual HTTP clients against local RSS and qBittorrent servers.
use std::sync::{Arc, Mutex};

use axum::{
    Json, Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde_json::{Value, json};

use super::{models::*, service::RssService};
use crate::{
    collector::DownloaderSnapshotCollector,
    db::Database,
    downloader::{DownloaderClient, DownloaderClientPool, qbittorrent::QBittorrentClient},
    indexer::IndexerPool,
    media::torrent::torrent_infohash,
};

const TORRENT: &[u8] = b"d4:infod6:lengthi12e4:name8:file.mkvee";

struct Remote {
    feed: String,
    feed_status: StatusCode,
    requests: Vec<HeaderMap>,
    present: bool,
    adds: usize,
    add_body: Vec<u8>,
    lose_response: bool,
    unreachable: bool,
    free_space: Option<u64>,
}
type Shared = Arc<Mutex<Remote>>;

struct Harness {
    directory: tempfile::TempDir,
    service: Arc<RssService>,
    state: Shared,
    endpoint: String,
    server: tokio::task::JoinHandle<()>,
    downloader: i64,
}

impl Harness {
    async fn new() -> Self {
        let state = Arc::new(Mutex::new(Remote {
            feed: xml(""),
            feed_status: StatusCode::OK,
            requests: Vec::new(),
            present: false,
            adds: 0,
            add_body: Vec::new(),
            lose_response: false,
            unreachable: false,
            free_space: Some(1_000_000),
        }));
        let app = Router::new()
            .route("/rss", get(remote_feed))
            .route(
                "/file.torrent",
                get(|| async { ([("content-type", "application/x-bittorrent")], TORRENT) }),
            )
            .route("/expired.torrent", get(|| async { StatusCode::FORBIDDEN }))
            .route(
                "/api/v2/auth/login",
                post(|| async { ([("set-cookie", "SID=rss-test; path=/")], "Ok.") }),
            )
            .route("/api/v2/torrents/info", get(remote_torrents))
            .route("/api/v2/torrents/add", post(remote_add))
            .route("/api/v2/sync/maindata", get(remote_space))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let directory = tempfile::tempdir().unwrap();
        let db = Database::open(directory.path()).await.unwrap();
        let downloader = db
            .create_downloader("RSS mock", "qbittorrent", &endpoint, "test", "private")
            .await
            .unwrap();
        let service = make_service(db);
        Self {
            directory,
            service,
            state,
            endpoint,
            server,
            downloader,
        }
    }

    async fn feed(&self) -> FeedRecord {
        self.service
            .db
            .rss_save_feed(
                None,
                FeedInput {
                    name: "Feed".into(),
                    url: Some(format!("{}/rss?passkey=private-token", self.endpoint)),
                    site_id: None,
                    use_proxy: None,
                    enabled: true,
                    interval_minutes: 15,
                    expected_version: None,
                    request_id: None,
                },
            )
            .await
            .unwrap()
    }

    fn rule_input(&self, feed_id: i64) -> RuleInput {
        RuleInput {
            name: "Documentary".into(),
            enabled: true,
            priority: 100,
            feed_ids: vec![feed_id],
            filters: RuleFilters {
                include: vec!["Documentary".into()],
                hr_policy: "any".into(),
                ..Default::default()
            },
            downloader_id: Some(self.downloader),
            options: DownloadOptions {
                category: Some("documentary".into()),
                tags: vec!["rss".into()],
                save_path: Some("/downloads/documentary".into()),
                ..Default::default()
            },
            expected_version: None,
            request_id: None,
        }
    }

    async fn scan(&self, id: i64) {
        let feed = self.service.db.rss_get_feed(id).await.unwrap();
        self.service
            .db
            .rss_request_check(
                id,
                ActionRequest {
                    expected_version: Some(feed.record.version),
                    request_id: None,
                },
            )
            .await
            .unwrap();
        let claim = self
            .service
            .db
            .rss_claim_feed("scan", 120)
            .await
            .unwrap()
            .unwrap();
        self.service.scan(claim).await.unwrap();
    }

    async fn queued(&self) -> JobRecord {
        let feed = self.feed().await;
        self.scan(feed.id).await;
        self.service
            .db
            .rss_save_rule(None, self.rule_input(feed.id))
            .await
            .unwrap();
        self.state.lock().unwrap().feed = xml(&entry("new", "/file.torrent", ""));
        self.scan(feed.id).await;
        let claim = self
            .service
            .db
            .rss_claim_evaluation("evaluate", 120)
            .await
            .unwrap()
            .unwrap();
        self.service.evaluate(claim).await.unwrap();
        self.service
            .db
            .rss_list_jobs(ListQuery::default().normalized())
            .await
            .unwrap()
            .items
            .remove(0)
    }

    fn due_reconciliation(&self) {
        let conn = rusqlite::Connection::open(self.directory.path().join("kirara.db")).unwrap();
        conn.execute("UPDATE rss_download_jobs SET next_attempt_at='2000-01-01T00:00:00+00:00', submission_started_at='2000-01-01T00:00:00+00:00' WHERE status='reconciling'", []).unwrap();
    }
}
impl Drop for Harness {
    fn drop(&mut self) {
        self.server.abort();
    }
}

fn make_service(db: Database) -> Arc<RssService> {
    let pool = DownloaderClientPool::new(db.clone());
    let collector = Arc::new(DownloaderSnapshotCollector::new(db.clone(), pool.clone()));
    RssService::new(db, pool, IndexerPool::new(), collector)
}

fn xml(body: &str) -> String {
    format!(
        "<rss version=\"2.0\" xmlns:t=\"http://torznab.com/schemas/2015/feed\"><channel><title>Test feed</title>{body}</channel></rss>"
    )
}
fn entry(id: &str, location: &str, attributes: &str) -> String {
    format!(
        "<item><guid>{id}</guid><title>Documentary {id}</title><enclosure url=\"{location}\" type=\"application/x-bittorrent\" length=\"12\"/>{attributes}</item>"
    )
}
async fn remote_feed(State(state): State<Shared>, headers: HeaderMap) -> Response {
    let mut remote = state.lock().unwrap();
    remote.requests.push(headers);
    (
        remote.feed_status,
        [("etag", "\"sample\""), ("retry-after", "120")],
        remote.feed.clone(),
    )
        .into_response()
}
async fn remote_torrents(State(state): State<Shared>) -> Response {
    let remote = state.lock().unwrap();
    if remote.unreachable {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    Json(if remote.present {
        json!([{"hash": torrent_infohash(TORRENT).unwrap(), "name": "Documentary",
        "size": 12, "amount_left": 12, "state": "downloading", "progress": 0.0}])
    } else {
        json!([])
    })
    .into_response()
}
async fn remote_add(State(state): State<Shared>, body: Bytes) -> Response {
    let mut remote = state.lock().unwrap();
    remote.adds += 1;
    remote.add_body = body.to_vec();
    remote.present = true;
    if remote.lose_response {
        remote.unreachable = true;
        StatusCode::BAD_GATEWAY.into_response()
    } else {
        "Ok.".into_response()
    }
}
async fn remote_space(State(state): State<Shared>) -> Json<Value> {
    let remote = state.lock().unwrap();
    Json(match remote.free_space {
        Some(space) => json!({"rid": 1, "server_state": {"free_space_on_disk": space}}),
        None => json!({"rid": 1, "server_state": {}}),
    })
}

#[tokio::test]
async fn test_and_both_preview_modes_have_no_persistent_side_effects() {
    let harness = Harness::new().await;
    harness.state.lock().unwrap().feed = xml(&entry("old", "/file.torrent", ""));
    let feed = harness.feed().await;
    let response = harness
        .service
        .test_feed(serde_json::from_value(json!({"feed_id":feed.id})).unwrap())
        .await
        .unwrap();
    assert_eq!(response.item_count, 1);
    assert!(
        !serde_json::to_string(&feed)
            .unwrap()
            .contains("private-token")
    );
    for (refresh_samples, expected) in [(false, 0), (true, 1)] {
        let preview = harness
            .service
            .preview(PreviewRequest {
                rule: harness.rule_input(feed.id),
                item_ids: vec![],
                refresh_samples,
            })
            .await
            .unwrap();
        assert_eq!(preview.total, expected);
    }
    let unchanged = harness.service.db.rss_get_feed(feed.id).await.unwrap();
    assert_eq!(unchanged.record.last_sequence, 0);
    assert!(unchanged.record.initialized_at.is_none());
    assert_eq!(
        harness
            .service
            .db
            .rss_list_items(ListQuery::default().normalized())
            .await
            .unwrap()
            .total,
        0
    );
    assert_eq!(harness.service.db.rss_summary().await.unwrap().queued, 0);
    assert_eq!(
        harness
            .service
            .db
            .rss_list_rules(ListQuery::default().normalized())
            .await
            .unwrap()
            .total,
        0
    );
}

#[tokio::test]
async fn test_draft_can_clear_credentials_and_cross_origin_never_receives_them() {
    let harness = Harness::new().await;
    let remote = Harness::new().await;
    let site = harness
        .service
        .db
        .create_site(
            "Private",
            "nexusphp",
            &harness.endpoint,
            r#"{"auth_type":"cookie","cookie":"private-cookie"}"#,
            "[]",
            false,
        )
        .await
        .unwrap();
    let feed = harness.feed().await;
    let saved = harness
        .service
        .db
        .rss_save_feed(
            Some(feed.id),
            FeedInput {
                name: feed.name,
                url: None,
                site_id: Some(site),
                use_proxy: None,
                enabled: true,
                interval_minutes: 15,
                expected_version: Some(feed.version),
                request_id: None,
            },
        )
        .await
        .unwrap();
    for request in [
        json!({"feed_id":saved.id}),
        json!({"feed_id":saved.id,"site_id":null,"use_proxy":null}),
        json!({"feed_id":saved.id,"url":format!("{}/rss",remote.endpoint)}),
    ] {
        harness
            .service
            .test_feed(serde_json::from_value(request).unwrap())
            .await
            .unwrap();
    }
    let requests = &harness.state.lock().unwrap().requests;
    assert_eq!(requests[0].get("cookie").unwrap(), "private-cookie");
    assert!(requests[1].get("cookie").is_none());
    assert!(
        remote.state.lock().unwrap().requests[0]
            .get("cookie")
            .is_none()
    );
    assert_eq!(
        harness
            .service
            .db
            .rss_get_feed(saved.id)
            .await
            .unwrap()
            .record
            .site_id,
        Some(site)
    );
}

#[tokio::test]
async fn separate_rss_and_site_api_origins_support_saved_feeds_enrichment_and_torrents() {
    type ApiRequests = Arc<Mutex<Vec<(&'static str, HeaderMap)>>>;
    async fn detail(State(requests): State<ApiRequests>, headers: HeaderMap) -> Json<Value> {
        requests.lock().unwrap().push(("detail", headers));
        Json(json!({"code":"0","data":{"size":12,"status":{"discount":"FREE","hr":false}}}))
    }
    async fn token(State(requests): State<ApiRequests>, headers: HeaderMap) -> Json<Value> {
        requests.lock().unwrap().push(("token", headers));
        Json(json!({"code":"0","data":"/file.torrent"}))
    }
    async fn torrent(State(requests): State<ApiRequests>, headers: HeaderMap) -> Response {
        requests.lock().unwrap().push(("torrent", headers));
        ([("content-type", "application/x-bittorrent")], TORRENT).into_response()
    }
    let harness = Harness::new().await;
    let requests: ApiRequests = Arc::new(Mutex::new(Vec::new()));
    let api = Router::new()
        .route("/api/torrent/detail", post(detail))
        .route("/api/torrent/genDlToken", post(token))
        .route("/file.torrent", get(torrent))
        .with_state(requests.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api_url = format!("http://localhost:{}", listener.local_addr().unwrap().port());
    let server = tokio::spawn(async move { axum::serve(listener, api).await.unwrap() });
    let site = harness.service.db.create_site(
        "Separate API", "mteam", &api_url,
        r#"{"auth_type":"api_key","api_key":"api-private-key"}"#,
        r#"[{"name":"Authorization","value":"Bearer custom-secret"},{"name":"Cookie","value":"session=private-cookie"},{"name":"User-Agent","value":"RSS-Test"}]"#,
        false,
    ).await.unwrap();
    harness.state.lock().unwrap().feed = xml(&format!(
        "<item><guid>42</guid><title>Documentary 42</title><link>{}/details.php?id=42</link></item>",
        harness.endpoint,
    ));
    let input = FeedInput {
        name: "RSS with separate API".into(),
        url: Some(format!("{}/rss?passkey=rss-own-token", harness.endpoint)),
        site_id: Some(site),
        use_proxy: Some(false),
        enabled: true,
        interval_minutes: 15,
        expected_version: None,
        request_id: None,
    };
    let preview = harness
        .service
        .test_feed(FeedTestRequest {
            feed_id: None,
            url: input.url.clone(),
            site_id: Some(Some(site)),
            use_proxy: Some(Some(false)),
        })
        .await
        .unwrap();
    assert_eq!(preview.item_count, 1);
    assert!(preview.items[0].downloadable);
    let feed = harness
        .service
        .db
        .rss_save_feed(None, input.clone())
        .await
        .unwrap();
    // Editing unrelated settings also works when the linked API has another origin.
    let feed = harness
        .service
        .db
        .rss_save_feed(
            Some(feed.id),
            FeedInput {
                url: None,
                interval_minutes: 30,
                expected_version: Some(feed.version),
                ..input
            },
        )
        .await
        .unwrap();
    assert_eq!(feed.site_id, Some(site));
    harness.scan(feed.id).await;
    let stored = harness.service.db.rss_get_feed(feed.id).await.unwrap();
    let item = harness
        .service
        .db
        .rss_list_items(ListQuery::default())
        .await
        .unwrap()
        .items
        .remove(0);
    assert_eq!(item.site_torrent_id.as_deref(), Some("42"));
    let locator = harness
        .service
        .db
        .rss_get_item_locator(item.id)
        .await
        .unwrap();
    let fetcher = super::fetcher::RssFetcher::new(harness.service.db.clone(), IndexerPool::new());
    let attributes = fetcher.enrich(&stored, &item, &locator).await.unwrap();
    assert_eq!(attributes.hr, Some(false));
    assert_eq!(attributes.download_volume_factor, Some(0.0));
    assert_eq!(
        fetcher.torrent(&stored, &item, &locator).await.unwrap(),
        TORRENT
    );
    {
        let rss = harness.state.lock().unwrap();
        assert_eq!(rss.requests.len(), 2);
        for headers in &rss.requests {
            assert!(!headers.contains_key("x-api-key"));
            assert!(!headers.contains_key("authorization"));
            assert!(!headers.contains_key("cookie"));
            assert_eq!(headers.get("user-agent").unwrap(), "RSS-Test");
        }
    }
    {
        let api = requests.lock().unwrap();
        assert_eq!(
            api.iter().map(|(kind, _)| *kind).collect::<Vec<_>>(),
            ["detail", "token", "torrent"]
        );
        for (_, headers) in api.iter() {
            assert_eq!(headers.get("x-api-key").unwrap(), "api-private-key");
            assert_eq!(
                headers.get("authorization").unwrap(),
                "Bearer custom-secret"
            );
            assert_eq!(headers.get("cookie").unwrap(), "session=private-cookie");
        }
    }
    server.abort();
}

#[tokio::test]
async fn baseline_then_new_item_is_delivered_once_with_saved_options() {
    let harness = Harness::new().await;
    let job = harness.queued().await;
    assert_eq!(job.status, "queued");
    let claim = harness
        .service
        .db
        .rss_claim_job("worker", 300)
        .await
        .unwrap()
        .unwrap();
    harness.service.process_job(claim).await.unwrap();
    let job = harness.service.db.rss_get_job(job.id).await.unwrap();
    assert_eq!(job.status, "submitted");
    assert_eq!(
        job.infohash.as_deref(),
        Some(torrent_infohash(TORRENT).unwrap().as_str())
    );
    assert_eq!(job.reserved_bytes, 12);
    let body = String::from_utf8_lossy(&harness.state.lock().unwrap().add_body).into_owned();
    assert!(body.contains("/downloads/documentary"));
    assert!(body.contains("documentary"));
    harness.scan(job.feed_id).await;
    assert!(
        harness
            .service
            .db
            .rss_claim_evaluation("again", 120)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(harness.state.lock().unwrap().adds, 1);
    assert_eq!(
        harness
            .service
            .db
            .rss_list_jobs(ListQuery::default().normalized())
            .await
            .unwrap()
            .total,
        1
    );
    harness
        .service
        .db
        .rss_release_observed_reservations(
            harness.downloader,
            vec![job.infohash.unwrap()],
            &chrono::Utc::now().to_rfc3339(),
        )
        .await
        .unwrap();
    assert_eq!(
        harness
            .service
            .db
            .rss_reserved_bytes(harness.downloader, -1)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn lost_response_and_restart_reconcile_without_a_second_add() {
    let harness = Harness::new().await;
    let job = harness.queued().await;
    harness.state.lock().unwrap().lose_response = true;
    let claim = harness
        .service
        .db
        .rss_claim_job("old-process", 300)
        .await
        .unwrap()
        .unwrap();
    harness.service.process_job(claim).await.unwrap();
    assert_eq!(
        harness.service.db.rss_get_job(job.id).await.unwrap().status,
        "reconciling"
    );
    harness
        .service
        .db
        .rss_release_owner("old-process")
        .await
        .unwrap();
    harness.due_reconciliation();
    let restarted = make_service(Database::open(harness.directory.path()).await.unwrap());
    let claim = restarted
        .db
        .rss_claim_job("new-process", 300)
        .await
        .unwrap()
        .unwrap();
    restarted.process_job(claim).await.unwrap();
    let unknown = restarted.db.rss_get_job(job.id).await.unwrap();
    assert_eq!(
        unknown.status, "reconciling",
        "unreachable is never proof of absence"
    );
    assert_eq!(unknown.reserved_bytes, 12);
    assert_eq!(harness.state.lock().unwrap().adds, 1);
    harness.state.lock().unwrap().unreachable = false;
    harness.due_reconciliation();
    let claim = restarted
        .db
        .rss_claim_job("new-process", 300)
        .await
        .unwrap()
        .unwrap();
    restarted.process_job(claim).await.unwrap();
    assert_eq!(
        restarted.db.rss_get_job(job.id).await.unwrap().status,
        "submitted"
    );
    assert_eq!(harness.state.lock().unwrap().adds, 1);
}

#[tokio::test]
async fn expired_enclosure_refreshes_by_stable_identity() {
    let harness = Harness::new().await;
    let feed = harness.feed().await;
    harness.scan(feed.id).await;
    harness
        .service
        .db
        .rss_save_rule(None, harness.rule_input(feed.id))
        .await
        .unwrap();
    harness.state.lock().unwrap().feed = xml(&entry("signed", "/expired.torrent", ""));
    harness.scan(feed.id).await;
    harness
        .service
        .evaluate(
            harness
                .service
                .db
                .rss_claim_evaluation("match", 120)
                .await
                .unwrap()
                .unwrap(),
        )
        .await
        .unwrap();
    harness.state.lock().unwrap().feed = xml(&entry("signed", "/file.torrent", ""));
    harness
        .service
        .process_job(
            harness
                .service
                .db
                .rss_claim_job("worker", 300)
                .await
                .unwrap()
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(harness.state.lock().unwrap().adds, 1);
    assert_eq!(
        harness
            .service
            .db
            .rss_list_jobs(ListQuery::default().normalized())
            .await
            .unwrap()
            .items[0]
            .status,
        "submitted"
    );
}

#[tokio::test]
async fn expired_free_evidence_is_refetched_and_blocks_submission() {
    let harness = Harness::new().await;
    let feed = harness.feed().await;
    harness.scan(feed.id).await;
    let mut rule = harness.rule_input(feed.id);
    rule.filters.free_only = true;
    harness.service.db.rss_save_rule(None, rule).await.unwrap();
    harness.state.lock().unwrap().feed = xml(&entry(
        "free",
        "/file.torrent",
        "<t:attr name=\"freeleech\" value=\"true\"/>",
    ));
    harness.scan(feed.id).await;
    harness
        .service
        .evaluate(
            harness
                .service
                .db
                .rss_claim_evaluation("match", 120)
                .await
                .unwrap()
                .unwrap(),
        )
        .await
        .unwrap();
    let conn = rusqlite::Connection::open(harness.directory.path().join("kirara.db")).unwrap();
    conn.execute("UPDATE rss_items SET attributes_json=json_set(attributes_json,'$.observed_at','2000-01-01T00:00:00Z')", []).unwrap();
    harness.state.lock().unwrap().feed = xml(&entry(
        "free",
        "/file.torrent",
        "<t:attr name=\"freeleech\" value=\"false\"/>",
    ));
    harness
        .service
        .process_job(
            harness
                .service
                .db
                .rss_claim_job("worker", 300)
                .await
                .unwrap()
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(harness.state.lock().unwrap().adds, 0);
    let job = harness
        .service
        .db
        .rss_list_jobs(ListQuery::default().normalized())
        .await
        .unwrap()
        .items
        .remove(0);
    assert_eq!(job.status, "waiting");
    assert!(job.last_error.unwrap().contains("免费"));
}

#[tokio::test]
async fn invalid_and_authenticated_error_responses_never_establish_baselines() {
    for (status, body) in [
        (StatusCode::UNAUTHORIZED, "secret-from-server"),
        (StatusCode::OK, "<rss><channel>"),
        (
            StatusCode::OK,
            "<html><form action='/login'>Login</form></html>",
        ),
    ] {
        let harness = Harness::new().await;
        let feed = harness.feed().await;
        {
            let mut state = harness.state.lock().unwrap();
            state.feed_status = status;
            state.feed = body.into();
        }
        harness.scan(feed.id).await;
        let failed = harness.service.db.rss_get_feed(feed.id).await.unwrap();
        assert!(failed.record.initialized_at.is_none());
        assert!(failed.etag.is_none());
        assert_eq!(failed.record.last_status, "error");
        assert!(
            !failed
                .record
                .last_error
                .unwrap()
                .contains("secret-from-server")
        );
    }
}

#[tokio::test]
async fn rate_limit_cooldown_is_shared_with_manual_test_requests() {
    let harness = Harness::new().await;
    let feed = harness.feed().await;
    harness.state.lock().unwrap().feed_status = StatusCode::TOO_MANY_REQUESTS;
    harness.scan(feed.id).await;
    let error = harness
        .service
        .test_feed(serde_json::from_value(json!({"feed_id":feed.id})).unwrap())
        .await
        .err()
        .unwrap();
    assert!(matches!(error, RssError::RateLimited { .. }));
    assert_eq!(
        harness.state.lock().unwrap().requests.len(),
        1,
        "manual checks obey the same origin gate"
    );
}

#[tokio::test]
async fn missing_space_is_unknown_and_low_space_wait_does_not_consume_attempts() {
    let harness = Harness::new().await;
    let client =
        QBittorrentClient::new(harness.endpoint.clone(), "test".into(), "test".into(), None)
            .unwrap();
    harness.state.lock().unwrap().free_space = None;
    assert!(client.get_free_space(None).await.is_err());
    harness.state.lock().unwrap().free_space = Some(0);
    assert_eq!(client.get_free_space(None).await.unwrap(), 0);
    let job = harness.queued().await;
    harness
        .service
        .process_job(
            harness
                .service
                .db
                .rss_claim_job("worker", 300)
                .await
                .unwrap()
                .unwrap(),
        )
        .await
        .unwrap();
    let waiting = harness.service.db.rss_get_job(job.id).await.unwrap();
    assert_eq!(waiting.status, "waiting");
    assert_eq!(waiting.attempts, 0);
    assert_eq!(harness.state.lock().unwrap().adds, 0);
}
