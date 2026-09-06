use super::{
    import,
    store::{self, sql},
};
use crate::db::Database;
use axum::{
    Router,
    body::{Body, to_bytes},
    extract::{Request, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use rusqlite::{OptionalExtension, params};
use std::{sync::Arc, time::Duration};
use subtle::ConstantTimeEq;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
struct DavState {
    db: Database,
    stop: CancellationToken,
    uploads: Arc<Semaphore>,
}
pub fn router(db: Database, stop: CancellationToken) -> Router {
    Router::new()
        .route("/dav/ptd", axum::routing::any(handle))
        .route("/dav/ptd/", axum::routing::any(handle))
        .route("/dav/ptd/{*path}", axum::routing::any(handle))
        .with_state(DavState {
            db,
            stop,
            uploads: Arc::new(Semaphore::new(4)),
        })
}
fn response(status: StatusCode, message: &str) -> Response {
    (
        status,
        [(header::CACHE_CONTROL, "no-store")],
        message.to_string(),
    )
        .into_response()
}
fn xml(status: StatusCode, body: String) -> Response {
    (
        status,
        [
            (header::CONTENT_TYPE, "application/xml; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        body,
    )
        .into_response()
}
fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
fn path_name(path: &str) -> Option<String> {
    if matches!(path, "/dav/ptd" | "/dav/ptd/") {
        return Some(String::new());
    }
    let name = path.strip_prefix("/dav/ptd/")?;
    if name.len() > 128
        || name.is_empty()
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        || name.starts_with('.')
    {
        return None;
    }
    if !name.ends_with(".zip") && !name.ends_with(".json") {
        return None;
    }
    Some(name.into())
}

async fn handle(State(state): State<DavState>, request: Request) -> Response {
    let Some(name) = path_name(request.uri().path()) else {
        return response(StatusCode::NOT_FOUND, "路径不存在");
    };
    let cfg = match state.db.dav(|conn| store::config(conn)).await {
        Ok(v) => v,
        Err(_) => return response(StatusCode::SERVICE_UNAVAILABLE, "接收服务暂时不可用"),
    };
    if !cfg.enabled || state.stop.is_cancelled() {
        return response(StatusCode::SERVICE_UNAVAILABLE, "接收服务已关闭");
    }
    let credentials = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .filter(|v| v.len() < 1024)
        .and_then(|v| v.split_once(' '))
        .filter(|(kind, _)| kind.eq_ignore_ascii_case("Basic"))
        .and_then(|(_, v)| STANDARD.decode(v).ok())
        .and_then(|v| String::from_utf8(v).ok());
    let authorized = credentials
        .as_deref()
        .and_then(|s| s.split_once(':'))
        .is_some_and(|(user, password)| {
            let hash = store::digest(password.as_bytes());
            user == cfg.username
                && !cfg.password_hash.is_empty()
                && bool::from(hash.as_bytes().ct_eq(cfg.password_hash.as_bytes()))
        });
    if !authorized {
        return (
            StatusCode::UNAUTHORIZED,
            [
                (
                    header::WWW_AUTHENTICATE,
                    "Basic realm=\"Kirara PTD\", charset=\"UTF-8\"",
                ),
                (header::CACHE_CONTROL, "no-store"),
            ],
            "需要 WebDAV 账号和密码",
        )
            .into_response();
    }
    let method = request.method().as_str().to_string();
    if method == "OPTIONS" {
        return (
            StatusCode::NO_CONTENT,
            [
                (header::ALLOW, "OPTIONS, PROPFIND, PUT, GET, HEAD, DELETE"),
                (header::CACHE_CONTROL, "no-store"),
            ],
        )
            .into_response();
    }
    if method == "PUT" {
        if name.is_empty() {
            return response(StatusCode::METHOD_NOT_ALLOWED, "不能覆盖接收目录");
        }
        if request.headers().contains_key(header::CONTENT_ENCODING) {
            return response(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "不支持 Content-Encoding",
            );
        }
        let Ok(_permit) = state.uploads.clone().try_acquire_owned() else {
            return response(StatusCode::SERVICE_UNAVAILABLE, "上传并发已满，请稍后重试");
        };
        let body = tokio::select! {
            _=state.stop.cancelled()=>return response(StatusCode::SERVICE_UNAVAILABLE,"服务正在关闭"),
            result=tokio::time::timeout(Duration::from_secs(30),to_bytes(request.into_body(),import::MAX_BODY))=>result,
        };
        let bytes = match body {
            Ok(Ok(b)) => b.to_vec(),
            Ok(Err(_)) => return response(StatusCode::PAYLOAD_TOO_LARGE, "上传未完成或超过 8 MiB"),
            Err(_) => return response(StatusCode::REQUEST_TIMEOUT, "上传超时"),
        };
        let parsed = tokio::task::spawn_blocking(move || {
            let metadata = import::parse(&bytes)?;
            Ok::<_, String>((bytes, metadata.generated_at))
        })
        .await;
        let (bytes, time) = match parsed {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => return response(StatusCode::UNPROCESSABLE_ENTITY, &e),
            Err(_) => return response(StatusCode::INTERNAL_SERVER_ERROR, "校验任务中断"),
        };
        let hash = cfg.password_hash;
        return match state
            .db
            .dav(move |conn| store::receive(conn, name, bytes, time, hash))
            .await
        {
            Ok(created) => response(
                if created {
                    StatusCode::CREATED
                } else {
                    StatusCode::NO_CONTENT
                },
                "",
            ),
            Err(e) if e.starts_with("接收存储已满") => {
                response(StatusCode::INSUFFICIENT_STORAGE, &e)
            }
            Err(e) if e.starts_with("接收服务已关闭") => {
                response(StatusCode::SERVICE_UNAVAILABLE, &e)
            }
            Err(_) => response(StatusCode::SERVICE_UNAVAILABLE, "保存失败，请稍后重试"),
        };
    }
    if method == "PROPFIND" {
        let depth = request
            .headers()
            .get("Depth")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("infinity")
            .to_string();
        if !matches!(depth.as_str(), "0" | "1") {
            return xml(
                StatusCode::FORBIDDEN,
                "<D:error xmlns:D=\"DAV:\"><D:propfind-finite-depth/></D:error>".into(),
            );
        }
        let body = match tokio::time::timeout(
            Duration::from_secs(5),
            to_bytes(request.into_body(), 64 * 1024),
        )
        .await
        {
            Ok(Ok(v)) => v,
            _ => return response(StatusCode::BAD_REQUEST, "属性查询数据无效"),
        };
        if !body.is_empty() {
            let mut reader = quick_xml::Reader::from_reader(body.as_ref());
            loop {
                match reader.read_event() {
                    Ok(quick_xml::events::Event::Eof) => break,
                    Ok(quick_xml::events::Event::DocType(_)) | Err(_) => {
                        return response(StatusCode::BAD_REQUEST, "属性查询 XML 无效");
                    }
                    _ => {}
                }
            }
        }
        let rows=state.db.dav(move|conn| {
            let mut stmt=conn.prepare("SELECT r.name,length(j.body),j.digest,j.received_at FROM webdav_sync_resources r JOIN webdav_sync_jobs j ON j.id=r.job_id WHERE ?='' OR r.name=? ORDER BY r.name").map_err(sql)?;
            let rows=stmt.query_map(params![name,name],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,r.get::<_,String>(2)?,r.get::<_,i64>(3)?))).map_err(sql)?.collect::<Result<Vec<_>,_>>().map_err(sql)?;
            Ok((name,rows))
        }).await;
        let (name, rows) = match rows {
            Ok(v) => v,
            Err(_) => return response(StatusCode::SERVICE_UNAVAILABLE, "读取目录失败"),
        };
        if !name.is_empty() && rows.is_empty() {
            return response(StatusCode::NOT_FOUND, "文件不存在");
        }
        let mut output = String::from(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?><D:multistatus xmlns:D=\"DAV:\">",
        );
        if name.is_empty() {
            output.push_str("<D:response><D:href>/dav/ptd/</D:href><D:propstat><D:prop><D:displayname>PTD</D:displayname><D:resourcetype><D:collection/></D:resourcetype></D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response>");
        }
        if depth == "1" || !name.is_empty() {
            for (file, size, hash, time) in rows {
                let date = chrono::DateTime::from_timestamp_millis(time)
                    .unwrap_or_default()
                    .format("%a, %d %b %Y %H:%M:%S GMT")
                    .to_string();
                output.push_str(&format!("<D:response><D:href>/dav/ptd/{file}</D:href><D:propstat><D:prop><D:displayname>{}</D:displayname><D:resourcetype/><D:getcontentlength>{size}</D:getcontentlength><D:getcontenttype>application/octet-stream</D:getcontenttype><D:getetag>&quot;{hash}&quot;</D:getetag><D:getlastmodified>{date}</D:getlastmodified></D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response>",escape(&file)));
            }
        }
        output.push_str("</D:multistatus>");
        return xml(StatusCode::MULTI_STATUS, output);
    }
    if matches!(method.as_str(), "GET" | "HEAD") {
        if name.is_empty() {
            return response(
                StatusCode::OK,
                if method == "HEAD" {
                    ""
                } else {
                    "Kirara PTD WebDAV"
                },
            );
        }
        let resource=state.db.dav(move|conn| {
            conn.query_row("SELECT j.body,j.digest FROM webdav_sync_resources r JOIN webdav_sync_jobs j ON j.id=r.job_id WHERE r.name=?",[name],|r|Ok((r.get::<_,Vec<u8>>(0)?,r.get::<_,String>(1)?))).optional().map_err(sql)
        }).await;
        return match resource {
            Ok(Some((bytes, hash))) => Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "application/octet-stream")
                .header(header::CONTENT_LENGTH, bytes.len())
                .header(header::ETAG, format!("\"{hash}\""))
                .header(header::CACHE_CONTROL, "no-store")
                .body(if method == "HEAD" {
                    Body::empty()
                } else {
                    Body::from(bytes)
                })
                .unwrap(),
            Ok(None) => response(StatusCode::NOT_FOUND, "文件不存在"),
            Err(_) => response(StatusCode::SERVICE_UNAVAILABLE, "读取文件失败"),
        };
    }
    if method == "DELETE" {
        if name.is_empty() {
            return response(StatusCode::FORBIDDEN, "不能删除接收目录");
        }
        return match state
            .db
            .dav(move |conn| {
                let tx = conn.transaction().map_err(sql)?;
                let count = tx
                    .execute("DELETE FROM webdav_sync_resources WHERE name=?", [name])
                    .map_err(sql)?;
                store::prune(&tx)?;
                tx.commit().map_err(sql)?;
                Ok(count)
            })
            .await
        {
            Ok(0) => response(StatusCode::NOT_FOUND, "文件不存在"),
            Ok(_) => response(StatusCode::NO_CONTENT, ""),
            Err(_) => response(StatusCode::SERVICE_UNAVAILABLE, "删除文件失败"),
        };
    }
    (
        StatusCode::METHOD_NOT_ALLOWED,
        [(header::ALLOW, "OPTIONS, PROPFIND, PUT, GET, HEAD, DELETE")],
    )
        .into_response()
}
