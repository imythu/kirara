//! Public API, persistent queue and runtime lifecycle against a local tracker and qBit.
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use kirara::{ListenEndpoint, ServerOptions};
use serde_json::{Value, json};
use sha1::{Digest, Sha1};

const TORRENT: &[u8] = b"d4:infod6:lengthi12e4:name8:file.mkvee";
const INFO: &[u8] = b"d6:lengthi12e4:name8:file.mkve";

#[derive(Default)]
struct Remote {
    entries: AtomicUsize,
    adds: AtomicUsize,
}

async fn rss(State(state): State<Arc<Remote>>, headers: HeaderMap) -> Response {
    let count = state.entries.load(Ordering::SeqCst);
    let etag = format!("\"{count}\"");
    if headers
        .get("if-none-match")
        .is_some_and(|value| value == etag.as_str())
    {
        return StatusCode::NOT_MODIFIED.into_response();
    }
    let entries = (1..=count).map(|id| format!("<item><guid>{id}</guid><title>Documentary {id}</title><enclosure url='/file.torrent' type='application/x-bittorrent' length='12'/></item>")).collect::<String>();
    (
        [("etag", etag)],
        format!(
            "<rss version='2.0'><channel><title>Local tracker</title>{entries}</channel></rss>"
        ),
    )
        .into_response()
}

async fn torrents(State(state): State<Arc<Remote>>) -> Json<Value> {
    Json(if state.adds.load(Ordering::SeqCst) > 0 {
        json!([{"hash":format!("{:x}",Sha1::digest(INFO)),"size":12,"amount_left":12,"state":"downloading","progress":0.0}])
    } else {
        json!([])
    })
}

async fn read(client: &reqwest::Client, base: &str, path: &str) -> Value {
    client
        .get(format!("{base}{path}"))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap()
}

async fn wait_for(
    client: &reqwest::Client,
    base: &str,
    path: &str,
    ready: impl Fn(&Value) -> bool,
) -> Value {
    tokio::time::timeout(Duration::from_secs(40), async {
        loop {
            let value = read(client, base, path).await;
            if ready(&value) {
                return value;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("RSS workflow did not settle at {path}"))
}

#[tokio::test]
async fn rss_public_api_baseline_delivery_idempotency_and_restart() {
    let remote = Arc::new(Remote::default());
    remote.entries.store(1, Ordering::SeqCst);
    let mock = Router::new()
        .route("/rss", get(rss))
        .route("/file.torrent", get(|| async { TORRENT }))
        .route(
            "/api/v2/auth/login",
            post(|| async { ([("set-cookie", "SID=integration; path=/")], "Ok.") }),
        )
        .route("/api/v2/torrents/info", get(torrents))
        .route(
            "/api/v2/sync/maindata",
            get(|| async {
                Json(json!({"rid":1,"server_state":{"free_space_on_disk":1_000_000}}))
            }),
        )
        .route(
            "/api/v2/torrents/add",
            post(|State(state): State<Arc<Remote>>| async move {
                state.adds.fetch_add(1, Ordering::SeqCst);
                "Ok."
            }),
        )
        .with_state(remote.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream = format!("http://{}", listener.local_addr().unwrap());
    let mock_server = tokio::spawn(async move { axum::serve(listener, mock).await.unwrap() });
    let dir = tempfile::tempdir().unwrap();
    let options = || ServerOptions {
        base_dir: dir.path().into(),
        db_dir: dir.path().into(),
        listen: ListenEndpoint::Tcp("127.0.0.1:0".parse().unwrap()),
    };
    let server = kirara::start(options()).await.unwrap();
    let ListenEndpoint::Tcp(address) = server.endpoint() else {
        panic!("expected TCP")
    };
    let base = format!("http://{address}");
    let client = reqwest::Client::new();
    let malformed = client
        .post(format!("{base}/api/rss/feeds/test"))
        .header("content-type", "application/json")
        .body("{invalid-private-token")
        .send()
        .await
        .unwrap();
    assert_eq!(malformed.status(), StatusCode::BAD_REQUEST);
    let error: Value = malformed.json().await.unwrap();
    assert_eq!(error["code"], "invalid_config");
    assert!(!error.to_string().contains("private-token"));
    let downloader: Value = client.post(format!("{base}/api/downloaders"))
        .json(&json!({"name":"RSS test","downloader_type":"qbittorrent","url":upstream,"username":"test","password":"test"}))
        .send().await.unwrap().error_for_status().unwrap().json().await.unwrap();
    let input = json!({"name":"RSS local","url":format!("{upstream}/rss?passkey=private-test"),"request_id":"create-source-once"});
    let test: Value = client
        .post(format!("{base}/api/rss/feeds/test"))
        .json(&input)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(test["item_count"], 1);
    assert_eq!(read(&client, &base, "/api/rss/feeds").await["total"], 0);
    let mut id = 0;
    for _ in 0..2 {
        let response = client
            .post(format!("{base}/api/rss/feeds"))
            .json(&input)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let feed: Value = response.json().await.unwrap();
        assert!(!feed.to_string().contains("private-test"));
        if id == 0 {
            id = feed["id"].as_i64().unwrap();
        } else {
            assert_eq!(feed["id"], id);
        }
    }
    let feed_path = format!("/api/rss/feeds/{id}");
    let feed = wait_for(&client, &base, &feed_path, |value| {
        value["initialized_at"].is_string()
    })
    .await;
    assert_eq!(
        remote.adds.load(Ordering::SeqCst),
        0,
        "first response only establishes a baseline"
    );
    assert_eq!(read(&client, &base, "/api/rss/downloads").await["total"], 0);
    let rule = json!({"name":"Documentary","feed_ids":[id],"downloader_id":downloader["id"],
        "filters":{"include":["Documentary"],"hr_policy":"any"},"options":{"tags":["rss"],"category":"docs"},"request_id":"create-rule-once"});
    let preview: Value = client
        .post(format!("{base}/api/rss/rules/preview"))
        .json(&json!({"rule":rule,"refresh_samples":false}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(preview["matched"], 1);
    assert_eq!(read(&client, &base, "/api/rss/downloads").await["total"], 0);
    client
        .post(format!("{base}/api/rss/rules"))
        .json(&rule)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    remote.entries.store(2, Ordering::SeqCst);
    let check = json!({"expected_version":feed["version"],"request_id":"check-once"});
    let response = client
        .post(format!("{base}{feed_path}/check"))
        .json(&check)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let run: Value = response.json().await.unwrap();
    assert!(run["id"].is_i64());
    let jobs = wait_for(&client, &base, "/api/rss/downloads", |value| {
        value["items"][0]["status"] == "submitted"
    })
    .await;
    assert_eq!(jobs["total"], 1);
    assert_eq!(remote.adds.load(Ordering::SeqCst), 1);
    assert_eq!(jobs["items"][0]["options_snapshot"]["category"], "docs");
    let replay: Value = client
        .post(format!("{base}{feed_path}/check"))
        .json(&check)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(replay["id"], run["id"]);
    let current = read(&client, &base, &feed_path).await;
    let response = client
        .post(format!("{base}{feed_path}/pause"))
        .json(&json!({"expected_version":-1}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        response.json::<Value>().await.unwrap()["code"],
        "state_changed"
    );
    let paused: Value = client
        .post(format!("{base}{feed_path}/pause"))
        .json(&json!({"expected_version":current["version"]}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        client
            .post(format!("{base}{feed_path}/check"))
            .json(&json!({"expected_version":paused["version"]}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    server.shutdown().await.unwrap();
    let restarted = kirara::start(options()).await.unwrap();
    let ListenEndpoint::Tcp(address) = restarted.endpoint() else {
        panic!("expected TCP")
    };
    let base = format!("http://{address}");
    assert_eq!(
        read(&client, &base, "/api/rss/downloads").await["items"][0]["status"],
        "submitted"
    );
    assert_eq!(read(&client, &base, &feed_path).await["enabled"], false);
    assert_eq!(remote.adds.load(Ordering::SeqCst), 1);
    restarted.shutdown().await.unwrap();
    mock_server.abort();
}
