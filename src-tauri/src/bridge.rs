use std::{collections::HashMap, sync::Mutex, time::Duration};

use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::{Method, Request, Response, Uri, body::Incoming, client::conn::http1};
use hyper_util::rt::TokioIo;
use kirara::{ListenEndpoint, ServerHandle};
use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::sse;

const LOGS_PATH: &str = "/api/system/logs/stream";
const MAX_REQUEST_BYTES: usize = 16 * 1024 * 1024;
const MAX_RESPONSE_BYTES: usize = 64 * 1024 * 1024;

pub struct Backend {
    pub endpoint: Option<ListenEndpoint>,
    pub startup_error: Option<String>,
    pub runtime: tokio::sync::Mutex<Option<BackendRuntime>>,
    pub stopping: CancellationToken,
    streams: Mutex<HashMap<String, CancellationToken>>,
}

pub struct BackendRuntime {
    pub server: ServerHandle,
    // Keep the private directory alive until the server has closed its listener.
    pub socket_dir: Option<tempfile::TempDir>,
}

impl Backend {
    pub fn new(result: Result<BackendRuntime, String>) -> Self {
        let (endpoint, startup_error, runtime) = match result {
            Ok(runtime) => (Some(runtime.server.endpoint().clone()), None, Some(runtime)),
            Err(error) => (None, Some(error), None),
        };
        Self {
            endpoint,
            startup_error,
            runtime: tokio::sync::Mutex::new(runtime),
            stopping: CancellationToken::new(),
            streams: Mutex::new(HashMap::new()),
        }
    }

    fn endpoint(&self) -> Result<&ListenEndpoint, String> {
        if self.stopping.is_cancelled() {
            return Err("应用正在退出".into());
        }
        self.endpoint.as_ref().ok_or_else(|| {
            self.startup_error
                .clone()
                .unwrap_or_else(|| "服务尚未启动".into())
        })
    }

    pub fn close_streams(&self) {
        for (_, token) in self.streams.lock().unwrap().drain() {
            token.cancel();
        }
    }

    pub async fn shutdown(&self) -> Result<(), String> {
        self.stopping.cancel();
        self.close_streams();
        if let Some(runtime) = self.runtime.lock().await.take() {
            let result = runtime
                .server
                .shutdown()
                .await
                .map_err(|error| error.to_string());
            drop(runtime.socket_dir);
            result?;
        }
        Ok(())
    }
}

#[derive(Deserialize)]
pub struct ApiRequest {
    path: String,
    method: String,
    body: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiResponse {
    status: u16,
    status_text: String,
    body: String,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum LogEvent {
    Open,
    Data { data: String },
    Error { message: String },
    End,
}

// Abort the HTTP driver as soon as a request/stream is cancelled or completed.
// Dropping a Tokio JoinHandle alone would detach it and keep the connection open.
struct Connection(tokio::task::JoinHandle<()>);

impl Drop for Connection {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn api_uri(path: &str) -> Result<Uri, String> {
    let uri: Uri = path.parse().map_err(|_| "无效的 API 路径")?;
    if uri.scheme().is_some()
        || uri.authority().is_some()
        || !uri.path().starts_with("/api/")
        || path.contains(['\\', '#'])
        || uri
            .path()
            .split('/')
            .any(|part| part == "." || part == "..")
    {
        return Err("仅允许访问应用 API".into());
    }
    Ok(uri)
}

async fn send(
    endpoint: &ListenEndpoint,
    request: Request<Full<Bytes>>,
) -> Result<(Response<Incoming>, Connection), String> {
    let (mut sender, connection) = tokio::time::timeout(Duration::from_secs(30), async {
        let stream = endpoint
            .connect()
            .await
            .map_err(|error| error.to_string())?;
        http1::handshake(TokioIo::new(stream))
            .await
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|_| "连接应用服务超时".to_owned())??;
    let connection = Connection(tokio::spawn(async move {
        let _ = connection.await;
    }));
    let response = sender
        .send_request(request)
        .await
        .map_err(|error| error.to_string())?;
    Ok((response, connection))
}

#[tauri::command]
pub async fn api_request(
    state: tauri::State<'_, std::sync::Arc<Backend>>,
    request: ApiRequest,
) -> Result<ApiResponse, String> {
    let endpoint = state.endpoint()?;
    let uri = api_uri(&request.path)?;
    if uri.path() == LOGS_PATH {
        return Err("日志流必须使用专用订阅接口".into());
    }
    let method = match request.method.as_str() {
        "GET" => Method::GET,
        "POST" => Method::POST,
        "PUT" => Method::PUT,
        "PATCH" => Method::PATCH,
        "DELETE" => Method::DELETE,
        _ => return Err("不支持的 API 请求方法".into()),
    };
    let body = request.body.unwrap_or_default();
    if body.len() > MAX_REQUEST_BYTES {
        return Err("请求超过大小限制".into());
    }
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header("Host", "localhost")
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .body(Full::new(Bytes::from(body)))
        .map_err(|error| error.to_string())?;
    tokio::select! {
        _ = state.stopping.cancelled() => Err("应用正在退出".into()),
        result = tokio::time::timeout(Duration::from_secs(600), async {
            let (response, _connection) = send(endpoint, request).await?;
            let status = response.status();
            let body = Limited::new(response.into_body(), MAX_RESPONSE_BYTES)
                .collect().await.map_err(|error| error.to_string())?.to_bytes();
            Ok(ApiResponse {
                status: status.as_u16(),
                status_text: status.canonical_reason().unwrap_or("").into(),
                body: String::from_utf8(body.to_vec()).map_err(|error| error.to_string())?,
            })
        }) => result.map_err(|_| "应用请求超时".to_owned())?,
    }
}

#[tauri::command]
pub fn logs_open(
    state: tauri::State<'_, std::sync::Arc<Backend>>,
    on_event: Channel<LogEvent>,
) -> Result<String, String> {
    let endpoint = state.endpoint()?.clone();
    let id = Uuid::new_v4().to_string();
    let cancel = state.stopping.child_token();
    {
        let mut streams = state.streams.lock().unwrap();
        if streams.len() >= 4 {
            return Err("日志订阅数量已达上限".into());
        }
        streams.insert(id.clone(), cancel.clone());
    }
    let state = state.inner().clone();
    let task_id = id.clone();
    tauri::async_runtime::spawn(async move {
        let result = tokio::select! {
            _ = cancel.cancelled() => Ok(()),
            result = forward_logs(&endpoint, &on_event) => result,
        };
        if !cancel.is_cancelled() {
            let event = match result {
                Ok(()) => LogEvent::End,
                Err(message) => LogEvent::Error { message },
            };
            let _ = on_event.send(event);
        }
        state.streams.lock().unwrap().remove(&task_id);
    });
    Ok(id)
}

#[tauri::command]
pub fn logs_close(state: tauri::State<'_, std::sync::Arc<Backend>>, id: String) {
    if let Some(token) = state.streams.lock().unwrap().remove(&id) {
        token.cancel();
    }
}

async fn forward_logs(
    endpoint: &ListenEndpoint,
    channel: &Channel<LogEvent>,
) -> Result<(), String> {
    let request = Request::builder()
        .uri(LOGS_PATH)
        .header("Host", "localhost")
        .header("Accept", "text/event-stream")
        .body(Full::new(Bytes::new()))
        .map_err(|error| error.to_string())?;
    let (response, _connection) =
        tokio::time::timeout(Duration::from_secs(30), send(endpoint, request))
            .await
            .map_err(|_| "日志订阅超时".to_owned())??;
    if !response.status().is_success() {
        return Err(format!("日志订阅失败：HTTP {}", response.status()));
    }
    channel
        .send(LogEvent::Open)
        .map_err(|error| error.to_string())?;
    let mut decoder = sse::Decoder::default();
    let mut body = response.into_body();
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|error| error.to_string())?;
        if let Ok(data) = frame.into_data() {
            for data in decoder.push(&data)? {
                channel
                    .send(LogEvent::Data { data })
                    .map_err(|error| error.to_string())?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::serve::Listener;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn test_endpoint(directory: &tempfile::TempDir) -> ListenEndpoint {
        #[cfg(unix)]
        {
            ListenEndpoint::Unix(directory.path().join("bridge.sock"))
        }
        #[cfg(windows)]
        {
            let _ = directory;
            ListenEndpoint::NamedPipe(format!(r"\\.\pipe\kirara-test-{}", Uuid::new_v4()))
        }
    }

    fn request() -> Request<Full<Bytes>> {
        Request::builder()
            .uri("/api/features")
            .header("Host", "localhost")
            .body(Full::new(Bytes::new()))
            .unwrap()
    }

    #[test]
    fn proxy_only_accepts_local_api_paths() {
        for invalid in [
            "https://example.com/api/settings",
            "//example.com/api/settings",
            "/",
            "/api",
            "/api/../index.html",
            "/api/settings#other",
            "/api\\settings",
        ] {
            assert!(api_uri(invalid).is_err(), "accepted {invalid}");
        }
        assert_eq!(
            api_uri("/api/sites?q=a%20b&limit=3").unwrap().path(),
            "/api/sites"
        );
    }

    #[tokio::test]
    async fn local_http_preserves_success_and_error_responses() {
        let directory = tempfile::tempdir().unwrap();
        let endpoint = test_endpoint(&directory);
        let mut listener = endpoint.bind().await.unwrap();
        let server = tokio::spawn(async move {
            for status in [200, 422] {
                let (mut stream, _) = listener.accept().await;
                let mut request = [0; 1024];
                stream.read(&mut request).await.unwrap();
                let body = if status == 200 {
                    r#"{"ok":true}"#
                } else {
                    r#"{"error":"invalid input"}"#
                };
                let response = format!(
                    "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            }
        });
        for (status, body) in [
            (200, r#"{"ok":true}"#),
            (422, r#"{"error":"invalid input"}"#),
        ] {
            let (response, _connection) = send(&endpoint, request()).await.unwrap();
            assert_eq!(response.status().as_u16(), status);
            assert_eq!(
                response.into_body().collect().await.unwrap().to_bytes(),
                body
            );
        }
        server.await.unwrap();
    }

    #[tokio::test]
    async fn log_body_is_incremental_and_dropping_driver_disconnects() {
        let directory = tempfile::tempdir().unwrap();
        let endpoint = test_endpoint(&directory);
        let mut listener = endpoint.bind().await.unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await;
            let mut request = [0; 1024];
            stream.read(&mut request).await.unwrap();
            let event = "event: log\ndata: first\n\n";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{event}\r\n",
                event.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            // The server deliberately never ends the response; the client must
            // receive this event and be able to close the transport explicitly.
            let closed = stream.read(&mut request).await;
            assert!(matches!(closed, Ok(0)) || closed.is_err());
        });
        let (response, connection) = send(&endpoint, request()).await.unwrap();
        let mut body = response.into_body();
        let frame = tokio::time::timeout(Duration::from_secs(2), body.frame())
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .into_data()
            .unwrap();
        assert_eq!(sse::Decoder::default().push(&frame).unwrap(), ["first"]);
        drop(body);
        drop(connection);
        tokio::time::timeout(Duration::from_secs(2), server)
            .await
            .unwrap()
            .unwrap();
    }
}
