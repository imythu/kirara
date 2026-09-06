mod import;
mod protocol;
mod store;
#[cfg(test)]
mod tests;

use crate::db::Database;
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use store::sql;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Config {
    pub enabled: bool,
    pub username: String,
    pub password_hash: String,
    pub existing_policy: String,
    pub auto_create: bool,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: false,
            username: "ptd".into(),
            password_hash: String::new(),
            existing_policy: "update".into(),
            auto_create: true,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigInput {
    enabled: bool,
    username: String,
    existing_policy: String,
    auto_create: bool,
    #[serde(default)]
    rotate_password: bool,
}

fn json_response(value: serde_json::Value) -> Response {
    ([(header::CACHE_CONTROL, "no-store")], Json(value)).into_response()
}
fn failure(status: StatusCode, message: &str) -> Response {
    (
        status,
        [(header::CACHE_CONTROL, "no-store")],
        Json(serde_json::json!({"error":message})),
    )
        .into_response()
}

pub fn management_router(db: Database) -> Router {
    Router::new()
        .route(
            "/api/sites/ptd-import",
            post(manual_import).layer(DefaultBodyLimit::max(12 * 1024 * 1024)),
        )
        .route("/api/sites/webdav-sync", get(get_config).put(save_config))
        .route("/api/sites/webdav-sync/runs", get(list_runs))
        .route("/api/sites/webdav-sync/runs/{id}/retry", post(retry))
        .with_state(db)
}

fn config_view(conn: &rusqlite::Connection) -> Result<serde_json::Value, String> {
    let cfg = store::config(conn)?;
    let last_received: Option<i64> = conn
        .query_row("SELECT MAX(received_at) FROM webdav_sync_jobs", [], |r| {
            r.get(0)
        })
        .map_err(sql)?;
    Ok(
        serde_json::json!({"enabled":cfg.enabled,"username":cfg.username,"password_configured":!cfg.password_hash.is_empty(),"existing_policy":cfg.existing_policy,"auto_create":cfg.auto_create,"running":cfg.enabled,"runtime_error":null,"last_received_at":last_received,"path":"/dav/ptd/"}),
    )
}

async fn get_config(State(db): State<Database>) -> Response {
    match db.dav(|conn| config_view(conn)).await {
        Ok(value) => json_response(value),
        Err(_) => failure(StatusCode::INTERNAL_SERVER_ERROR, "读取 WebDAV 配置失败"),
    }
}

async fn save_config(State(db): State<Database>, Json(input): Json<ConfigInput>) -> Response {
    if input.username.trim().is_empty()
        || input.username.len() > 128
        || input.username.contains(':')
        || input.username.chars().any(char::is_control)
    {
        return failure(
            StatusCode::BAD_REQUEST,
            "用户名不能为空，且不能包含冒号或控制字符",
        );
    }
    if !matches!(input.existing_policy.as_str(), "update" | "skip") {
        return failure(StatusCode::BAD_REQUEST, "已有站点处理方式无效");
    }
    let new_password = if input.rotate_password {
        let mut bytes = [0u8; 32];
        if getrandom::fill(&mut bytes).is_err() {
            return failure(StatusCode::INTERNAL_SERVER_ERROR, "生成密码失败，请重试");
        }
        Some(bytes.iter().map(|b| format!("{b:02x}")).collect::<String>())
    } else {
        None
    };
    let result = db
        .dav(move |conn| {
            let tx = conn
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(sql)?;
            let old = store::config(&tx)?;
            let hash = new_password
                .as_ref()
                .map(|p| store::digest(p.as_bytes()))
                .unwrap_or(old.password_hash);
            if input.enabled && hash.is_empty() {
                return Err("请先生成接收密码".into());
            }
            let cfg = Config {
                enabled: input.enabled,
                username: input.username.trim().into(),
                password_hash: hash,
                existing_policy: input.existing_policy,
                auto_create: input.auto_create,
            };
            tx.execute(
                "UPDATE webdav_sync_settings SET config=?,running=0,runtime_error=NULL WHERE id=1",
                params![serde_json::to_string(&cfg).map_err(|e| e.to_string())?],
            )
            .map_err(sql)?;
            let mut view = config_view(&tx)?;
            if let Some(password) = new_password {
                view["new_password"] = password.into();
            }
            tx.commit().map_err(sql)?;
            Ok(view)
        })
        .await;
    match result {
        Ok(view) => json_response(view),
        Err(message) if message == "请先生成接收密码" => {
            failure(StatusCode::BAD_REQUEST, &message)
        }
        Err(_) => failure(StatusCode::INTERNAL_SERVER_ERROR, "保存 WebDAV 配置失败"),
    }
}

async fn list_runs(State(db): State<Database>) -> Response {
    match db.dav(|conn| {
        let mut stmt = conn.prepare("SELECT id,name,received_at,status,attempts,result,error,body IS NOT NULL FROM webdav_sync_jobs ORDER BY id DESC LIMIT 100").map_err(sql)?;
        let rows = stmt.query_map([],|r| {
            let result:Option<String> = r.get(5)?;
            Ok(serde_json::json!({"id":r.get::<_,i64>(0)?,"name":r.get::<_,String>(1)?,"received_at":r.get::<_,i64>(2)?,"status":r.get::<_,String>(3)?,"attempts":r.get::<_,i64>(4)?,"result":result.and_then(|v|serde_json::from_str::<serde_json::Value>(&v).ok()),"error":r.get::<_,Option<String>>(6)?,"can_retry":r.get::<_,bool>(7)?}))
        }).map_err(sql)?.collect::<Result<Vec<_>,_>>().map_err(sql)?;
        Ok(serde_json::json!(rows))
    }).await { Ok(value)=>json_response(value),Err(_)=>failure(StatusCode::INTERNAL_SERVER_ERROR,"读取同步记录失败") }
}

async fn retry(State(db): State<Database>, Path(id): Path<i64>) -> Response {
    match db.dav(move|conn| {
        conn.execute("UPDATE webdav_sync_jobs SET status='pending',attempts=0,next_retry=0,error=NULL WHERE id=? AND status='failed' AND body IS NOT NULL",[id]).map_err(sql)
    }).await { Ok(1)=>json_response(serde_json::json!({"ok":true})),Ok(_)=>failure(StatusCode::CONFLICT,"此任务无法重试，可能数据已清理，请从 PTD 重新推送"),Err(_)=>failure(StatusCode::INTERNAL_SERVER_ERROR,"重试失败") }
}

/// Routes share the main HTTP listener; only this worker has a separate task.
pub fn receiver_router(db: Database, stop: CancellationToken) -> Router {
    protocol::router(db, stop)
}

pub async fn run(db: Database, stop: CancellationToken) {
    loop {
        if stop.is_cancelled() {
            break;
        }
        match db.dav(|conn| store::config(conn)).await {
            Ok(cfg) => {
                if cfg.enabled {
                    if db.dav(store::process_next).await.is_err() {
                        // No partial site changes survive a failed transaction.
                        let _ = db.dav(|conn| {
                            let job:Option<(i64,i64)> = conn.query_row("SELECT id,attempts FROM webdav_sync_jobs WHERE status IN ('pending','retry') AND next_retry<=? ORDER BY id LIMIT 1",[chrono::Utc::now().timestamp_millis()],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(sql)?;
                            if let Some((id,attempts))=job {
                                let delay = [60_000,300_000,900_000][(attempts as usize).min(2)];
                                conn.execute("UPDATE webdav_sync_jobs SET attempts=attempts+1,status=?,next_retry=?,error='数据库处理失败，已回滚本次站点变更' WHERE id=?",params![if attempts>=3 {"failed"} else {"retry"},chrono::Utc::now().timestamp_millis()+delay,id]).map_err(sql)?;
                            }
                            Ok(())
                        }).await;
                    }
                }
            }
            Err(_) => tracing::warn!("could not read WebDAV receiver configuration"),
        }
        tokio::select! { _=stop.cancelled()=>break, _=tokio::time::sleep(Duration::from_secs(1))=>{} }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ManualImportInput {
    content_base64: String,
    existing_policy: String,
    auto_create: bool,
}

async fn manual_import(
    State(db): State<Database>,
    Json(input): Json<ManualImportInput>,
) -> Response {
    use base64::{Engine, engine::general_purpose::STANDARD};
    if !matches!(input.existing_policy.as_str(), "update" | "skip") {
        return failure(StatusCode::BAD_REQUEST, "已有站点处理方式无效");
    }
    let parsed = tokio::task::spawn_blocking(move || {
        if input.content_base64.len() > import::MAX_BODY.div_ceil(3) * 4 {
            return Err("文件超过 8 MiB".to_string());
        }
        let bytes = STANDARD
            .decode(&input.content_base64)
            .map_err(|_| "文件编码无效".to_string())?;
        let data = import::parse(&bytes)?;
        Ok((data, input.existing_policy, input.auto_create))
    })
    .await;
    let (data, policy, auto_create) = match parsed {
        Ok(Ok(value)) => value,
        Ok(Err(error)) => return failure(StatusCode::UNPROCESSABLE_ENTITY, &error),
        Err(_) => return failure(StatusCode::INTERNAL_SERVER_ERROR, "读取文件失败，请重试"),
    };
    match db
        .dav(move |conn| store::import_manual(conn, data, policy, auto_create))
        .await
    {
        Ok(report) => json_response(serde_json::to_value(report).unwrap()),
        Err(_) => failure(
            StatusCode::INTERNAL_SERVER_ERROR,
            "导入失败，本次站点变更已回滚，请重试",
        ),
    }
}
