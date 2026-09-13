use super::{ApiError, AppState};
use crate::scheduled_task::{Run, Task, TaskRequest, Timing};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use serde::Deserialize;

pub(super) async fn list(State(s): State<AppState>) -> Result<Json<Vec<Task>>, ApiError> {
    Ok(Json(s.db.list_scheduled_tasks().await?))
}
pub(super) async fn create(
    State(s): State<AppState>,
    Json(body): Json<TaskRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if let Some(http) = &body.http {
        crate::scheduled_task::validate_delivery(http, &s.db.get_settings().await?)
            .map_err(ApiError::bad_request)?;
    }
    let id = s.db.save_scheduled_task(None, body).await?;
    Ok(Json(serde_json::json!({"id":id})))
}
pub(super) async fn update(
    State(s): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<TaskRequest>,
) -> Result<StatusCode, ApiError> {
    if let Some(http) = &body.http {
        crate::scheduled_task::validate_delivery(http, &s.db.get_settings().await?)
            .map_err(ApiError::bad_request)?;
    }
    s.db.save_scheduled_task(Some(id), body).await?;
    Ok(StatusCode::NO_CONTENT)
}
pub(super) async fn delete(
    State(s): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    s.db.delete_scheduled_task(id).await?;
    Ok(StatusCode::NO_CONTENT)
}
#[derive(Deserialize)]
pub(super) struct Enabled {
    enabled: bool,
}
pub(super) async fn enabled(
    State(s): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<Enabled>,
) -> Result<StatusCode, ApiError> {
    s.db.set_scheduled_enabled(id, body.enabled).await?;
    Ok(StatusCode::NO_CONTENT)
}
pub(super) async fn run(
    State(s): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    s.scheduled_task_scheduler
        .trigger(id, true)
        .await
        .map_err(ApiError::conflict)?;
    Ok(StatusCode::NO_CONTENT)
}
pub(super) async fn records(
    State(s): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<Vec<Run>>, ApiError> {
    Ok(Json(s.db.scheduled_runs(id).await?))
}
pub(super) async fn preview(Json(timing): Json<Timing>) -> Result<Json<Vec<String>>, ApiError> {
    let mut at = chrono::Utc::now();
    let mut dates = Vec::new();
    for _ in 0..3 {
        at = timing.next(at).map_err(ApiError::bad_request)?;
        dates.push(at.to_rfc3339());
    }
    Ok(Json(dates))
}

// Only explicit disclosure endpoints return credentials, never list/history or caches.
pub(super) async fn http_config(
    State(s): State<AppState>,
    Path(id): Path<i64>,
) -> Result<impl axum::response::IntoResponse, ApiError> {
    let config = s.db.scheduled_http_config(id).await?;
    Ok((
        [(axum::http::header::CACHE_CONTROL, "no-store")],
        Json(config),
    ))
}
pub(super) async fn request_preview(
    Json(config): Json<crate::scheduled_task::HttpConfig>,
) -> Result<impl axum::response::IntoResponse, ApiError> {
    let preview = crate::scheduled_task::request_preview(&config).map_err(ApiError::bad_request)?;
    Ok((
        [(axum::http::header::CACHE_CONTROL, "no-store")],
        Json(preview),
    ))
}
