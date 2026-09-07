use std::time::Duration;

use kirara::{ListenEndpoint, ServerOptions};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn options(dir: &tempfile::TempDir, listen: ListenEndpoint) -> ServerOptions {
    ServerOptions {
        base_dir: dir.path().into(),
        db_dir: dir.path().into(),
        listen,
    }
}

async fn get(endpoint: &ListenEndpoint, path: &str) -> String {
    let mut stream = endpoint.connect().await.unwrap();
    stream
        .write_all(
            format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .await
        .unwrap();
    let mut bytes = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), stream.read_to_end(&mut bytes))
        .await
        .unwrap()
        .unwrap();
    String::from_utf8(bytes).unwrap()
}

#[tokio::test]
async fn save_path_analysis_lists_all_torrents_without_self_use() {
    use axum::{
        Json, Router,
        routing::{get, post},
    };
    use serde_json::{Value, json};

    let qb = Router::new()
        .route(
            "/api/v2/auth/login",
            post(|| async { ([("set-cookie", "SID=test-session; path=/")], "Ok.") }),
        )
        .route(
            "/api/v2/torrents/info",
            get(|| async {
                Json(json!([
                    {"hash": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "name": "Complete",
                     "size": 100, "downloaded": 100, "progress": 1.0,
                     "state": "stalledUP", "save_path": "/downloads/movies", "added_on": 1},
                    {"hash": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", "name": "Partial",
                     "size": 200, "downloaded": 50, "progress": 0.25,
                     "state": "downloading", "save_path": "/downloads/tv", "added_on": 2}
                ]))
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let qb_address = listener.local_addr().unwrap();
    let qb_server = tokio::spawn(async move { axum::serve(listener, qb).await.unwrap() });
    let dir = tempfile::tempdir().unwrap();
    let server = kirara::start(options(
        &dir,
        ListenEndpoint::Tcp("127.0.0.1:0".parse().unwrap()),
    ))
    .await
    .unwrap();
    let ListenEndpoint::Tcp(address) = server.endpoint() else {
        panic!("expected TCP")
    };
    let base = format!("http://{address}");
    let client = reqwest::Client::new();
    let features: Value = client
        .get(format!("{base}/api/features"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        features["self_use"], false,
        "run this test with SELF_USE=false"
    );
    let created: Value = client.post(format!("{base}/api/downloaders"))
        .json(&json!({"name": "Test", "downloader_type": "qbittorrent",
                     "url": format!("http://{qb_address}"), "username": "admin", "password": "test"}))
        .send().await.unwrap().error_for_status().unwrap().json().await.unwrap();
    let url = format!("{base}/api/downloaders/{}/torrents", created["id"]);
    let torrents: Vec<Value> = client
        .get(format!("{url}?include_incomplete=true"))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(torrents.len(), 2);
    assert_eq!(torrents[0]["save_path"], "/downloads/tv");
    assert_eq!(torrents[0]["downloaded"], 50);
    assert_eq!(torrents[0]["size"], 200);
    assert_eq!(torrents[1]["save_path"], "/downloads/movies");
    let completed: Vec<Value> = client
        .get(&url)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(completed.len(), 1);
    assert_eq!(completed[0]["name"], "Complete");
    server.shutdown().await.unwrap();
    qb_server.abort();
}

#[tokio::test]
async fn tcp_start_respond_shutdown_and_restart_preserves_database() {
    let dir = tempfile::tempdir().unwrap();
    let server = kirara::start(options(
        &dir,
        ListenEndpoint::Tcp("127.0.0.1:0".parse().unwrap()),
    ))
    .await
    .unwrap();
    let endpoint = server.endpoint().clone();
    assert!(
        get(&endpoint, "/api/settings")
            .await
            .starts_with("HTTP/1.1 200")
    );
    let database = dir.path().join("kirara.db");
    let connection = rusqlite::Connection::open(&database).unwrap();
    connection
        .execute("CREATE TABLE lifecycle_marker (id INTEGER PRIMARY KEY)", [])
        .unwrap();
    connection
        .execute("INSERT INTO lifecycle_marker VALUES (42)", [])
        .unwrap();
    drop(connection);
    tokio::time::timeout(Duration::from_secs(15), server.shutdown())
        .await
        .unwrap()
        .unwrap();
    assert!(endpoint.connect().await.is_err());
    let restarted = kirara::start(options(&dir, endpoint)).await.unwrap();
    assert!(
        get(restarted.endpoint(), "/api/features")
            .await
            .starts_with("HTTP/1.1 200")
    );
    let connection = rusqlite::Connection::open(database).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT id FROM lifecycle_marker", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        42
    );
    drop(connection);
    restarted.shutdown().await.unwrap();
}

#[tokio::test]
async fn shutdown_finishes_with_an_open_log_stream() {
    let dir = tempfile::tempdir().unwrap();
    let server = kirara::start(options(
        &dir,
        ListenEndpoint::Tcp("127.0.0.1:0".parse().unwrap()),
    ))
    .await
    .unwrap();
    let mut stream = server.endpoint().connect().await.unwrap();
    stream
        .write_all(b"GET /api/system/logs/stream HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await
        .unwrap();
    let mut buffer = [0; 2048];
    let count = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buffer))
        .await
        .unwrap()
        .unwrap();
    assert!(String::from_utf8_lossy(&buffer[..count]).starts_with("HTTP/1.1 200"));
    tokio::time::timeout(Duration::from_secs(15), server.shutdown())
        .await
        .unwrap()
        .unwrap();
    let mut tail = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut tail))
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn shutdown_closes_an_incomplete_request_without_waiting_for_the_client() {
    let dir = tempfile::tempdir().unwrap();
    let server = kirara::start(options(
        &dir,
        ListenEndpoint::Tcp("127.0.0.1:0".parse().unwrap()),
    ))
    .await
    .unwrap();
    let endpoint = server.endpoint().clone();
    let mut stream = endpoint.connect().await.unwrap();
    stream
        .write_all(
            b"PUT /api/settings HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: 100\r\nExpect: 100-continue\r\n\r\n",
        )
        .await
        .unwrap();

    // The interim response proves the handler is already waiting for its body,
    // so shutdown cannot pass merely because the socket was not accepted yet.
    let mut interim = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), async {
        while !interim.ends_with(b"\r\n\r\n") {
            interim.push(stream.read_u8().await.unwrap());
            assert!(interim.len() <= 1024);
        }
    })
    .await
    .unwrap();
    assert!(String::from_utf8_lossy(&interim).starts_with("HTTP/1.1 100 Continue"));
    stream.write_all(b"{").await.unwrap();

    // Keep the client alive and its request unfinished throughout shutdown.
    tokio::time::timeout(Duration::from_secs(10), server.shutdown())
        .await
        .expect("shutdown waited indefinitely for an incomplete request")
        .unwrap();
    assert!(endpoint.connect().await.is_err());
    let mut remaining = [0; 1];
    let closed = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut remaining))
        .await
        .expect("shutdown left the accepted connection running");
    assert!(match closed {
        Ok(0) => true,
        Err(error) => matches!(
            error.kind(),
            std::io::ErrorKind::ConnectionReset
                | std::io::ErrorKind::ConnectionAborted
                | std::io::ErrorKind::BrokenPipe
        ),
        _ => false,
    });
}

#[cfg(unix)]
#[tokio::test]
async fn local_socket_serves_api_and_is_removed_after_shutdown() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.sock");
    let server = kirara::start(options(&dir, ListenEndpoint::Unix(path.clone())))
        .await
        .unwrap();
    assert!(path.exists());
    assert!(
        get(server.endpoint(), "/api/features")
            .await
            .starts_with("HTTP/1.1 200")
    );
    server.shutdown().await.unwrap();
    assert!(!path.exists());
}

#[tokio::test]
async fn occupied_listener_does_not_initialize_a_database() {
    let dir = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let result = kirara::start(options(
        &dir,
        ListenEndpoint::Tcp(listener.local_addr().unwrap()),
    ))
    .await;
    assert!(result.is_err());
    assert!(!dir.path().join("kirara.db").exists());
}
