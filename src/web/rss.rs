use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde_json::json;

use crate::rss_download::{models::*, service::RssService};

pub(super) fn router(service: Arc<RssService>) -> Router {
    Router::new()
        .route("/summary", get(summary))
        .route("/feeds", get(feeds).post(create_feed))
        .route("/feeds/test", post(test_feed))
        .route(
            "/feeds/{id}",
            get(feed).put(update_feed).delete(archive_feed),
        )
        .route("/feeds/{id}/check", post(check_feed))
        .route("/feeds/{id}/{action}", post(feed_action))
        .route("/rules", get(rules).post(create_rule))
        .route("/rules/preview", post(preview))
        .route(
            "/rules/{id}",
            get(rule).put(update_rule).delete(archive_rule),
        )
        .route("/rules/{id}/{action}", post(rule_action))
        .route("/items", get(items))
        .route("/items/{id}", get(item))
        .route("/backfills", post(backfill))
        .route("/downloads", get(downloads))
        .route("/downloads/{id}", get(download))
        .route("/downloads/{id}/{action}", post(download_action))
        .route("/runs", get(runs))
        .route("/runs/{id}", get(run))
        .layer(DefaultBodyLimit::max(128 * 1024))
        .layer(axum::middleware::map_response(extractor_error))
        .with_state(service)
}

// Axum's JSON/query rejections otherwise return plain text, unlike the shared API contract.
async fn extractor_error(response: Response) -> Response {
    let status = response.status();
    let json = response
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|header| header.to_str().ok())
        .is_some_and(|value| value.starts_with("application/json"));
    if !status.is_client_error() || json {
        return response;
    }
    let message = match status {
        StatusCode::PAYLOAD_TOO_LARGE => "请求内容超过 128 KiB 上限",
        StatusCode::UNSUPPORTED_MEDIA_TYPE => "请使用 application/json 提交请求",
        StatusCode::NOT_FOUND => "对象或操作不存在",
        StatusCode::METHOD_NOT_ALLOWED => "此接口不支持该请求方法",
        _ => "请求参数格式无效，请检查 JSON、字段类型和分页参数",
    };
    (
        status,
        Json(json!({"error":message,"code":"invalid_config","retry_at":null})),
    )
        .into_response()
}

impl IntoResponse for RssError {
    fn into_response(self) -> Response {
        let (status, code, retry_at) = match &self {
            Self::Invalid(_) => (StatusCode::UNPROCESSABLE_ENTITY, "invalid_config", None),
            Self::NotFound(_) => (StatusCode::NOT_FOUND, "not_found", None),
            Self::Conflict(_) => (StatusCode::CONFLICT, "state_changed", None),
            Self::Authentication(_) => (StatusCode::BAD_GATEWAY, "authentication_expired", None),
            Self::Unavailable(_) => (StatusCode::BAD_GATEWAY, "upstream_unavailable", None),
            Self::RateLimited { retry_at, .. } => (
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limited",
                Some(retry_at.clone()),
            ),
            Self::Database(_) => (StatusCode::INTERNAL_SERVER_ERROR, "database_error", None),
        };
        (
            status,
            Json(json!({ "error": self.to_string(), "code": code, "retry_at": retry_at })),
        )
            .into_response()
    }
}

async fn summary(State(service): State<Arc<RssService>>) -> RssResult<Json<RssSummary>> {
    Ok(Json(service.db.rss_summary().await?))
}
async fn feeds(
    State(service): State<Arc<RssService>>,
    Query(query): Query<ListQuery>,
) -> RssResult<Json<Page<FeedRecord>>> {
    Ok(Json(service.db.rss_list_feeds(query.normalized()).await?))
}
async fn feed(
    State(service): State<Arc<RssService>>,
    Path(id): Path<i64>,
) -> RssResult<Json<FeedRecord>> {
    Ok(Json(service.db.rss_get_feed(id).await?.record))
}
async fn create_feed(
    State(service): State<Arc<RssService>>,
    Json(input): Json<FeedInput>,
) -> RssResult<(StatusCode, Json<FeedRecord>)> {
    let value = service.db.rss_save_feed(None, input).await?;
    service.wake.notify_waiters();
    Ok((StatusCode::CREATED, Json(value)))
}
async fn update_feed(
    State(service): State<Arc<RssService>>,
    Path(id): Path<i64>,
    Json(input): Json<FeedInput>,
) -> RssResult<Json<FeedRecord>> {
    let value = service.db.rss_save_feed(Some(id), input).await?;
    service.wake.notify_waiters();
    Ok(Json(value))
}
async fn test_feed(
    State(service): State<Arc<RssService>>,
    Json(request): Json<FeedTestRequest>,
) -> RssResult<Json<FeedTestResponse>> {
    Ok(Json(service.test_feed(request).await?))
}
async fn archive_feed(
    State(service): State<Arc<RssService>>,
    Path(id): Path<i64>,
    Json(request): Json<ActionRequest>,
) -> RssResult<StatusCode> {
    service.db.rss_archive_feed(id, request).await?;
    service.wake.notify_waiters();
    Ok(StatusCode::NO_CONTENT)
}
async fn check_feed(
    State(service): State<Arc<RssService>>,
    Path(id): Path<i64>,
    Json(request): Json<ActionRequest>,
) -> RssResult<(StatusCode, Json<RunRecord>)> {
    let value = service.db.rss_request_check(id, request).await?;
    service.wake.notify_waiters();
    Ok((StatusCode::ACCEPTED, Json(value)))
}
async fn feed_action(
    State(service): State<Arc<RssService>>,
    Path((id, action)): Path<(i64, String)>,
    Json(request): Json<ActionRequest>,
) -> RssResult<Json<FeedRecord>> {
    let enabled = parse_enabled(&action)?;
    let value = service
        .db
        .rss_set_feed_enabled(id, enabled, request)
        .await?;
    service.wake.notify_waiters();
    Ok(Json(value))
}
async fn rules(
    State(service): State<Arc<RssService>>,
    Query(query): Query<ListQuery>,
) -> RssResult<Json<Page<RuleRecord>>> {
    Ok(Json(service.db.rss_list_rules(query.normalized()).await?))
}
async fn rule(
    State(service): State<Arc<RssService>>,
    Path(id): Path<i64>,
) -> RssResult<Json<RuleRecord>> {
    Ok(Json(service.db.rss_get_rule(id).await?))
}
async fn create_rule(
    State(service): State<Arc<RssService>>,
    Json(input): Json<RuleInput>,
) -> RssResult<(StatusCode, Json<RuleRecord>)> {
    let value = service.db.rss_save_rule(None, input).await?;
    service.wake.notify_waiters();
    Ok((StatusCode::CREATED, Json(value)))
}
async fn update_rule(
    State(service): State<Arc<RssService>>,
    Path(id): Path<i64>,
    Json(input): Json<RuleInput>,
) -> RssResult<Json<RuleRecord>> {
    let value = service.db.rss_save_rule(Some(id), input).await?;
    service.wake.notify_waiters();
    Ok(Json(value))
}
async fn archive_rule(
    State(service): State<Arc<RssService>>,
    Path(id): Path<i64>,
    Json(request): Json<ActionRequest>,
) -> RssResult<StatusCode> {
    service.db.rss_archive_rule(id, request).await?;
    service.wake.notify_waiters();
    Ok(StatusCode::NO_CONTENT)
}
async fn rule_action(
    State(service): State<Arc<RssService>>,
    Path((id, action)): Path<(i64, String)>,
    Json(request): Json<ActionRequest>,
) -> RssResult<Json<RuleRecord>> {
    let enabled = parse_enabled(&action)?;
    let value = service
        .db
        .rss_set_rule_enabled(id, enabled, request)
        .await?;
    service.wake.notify_waiters();
    Ok(Json(value))
}
fn parse_enabled(action: &str) -> RssResult<bool> {
    match action {
        "pause" => Ok(false),
        "resume" => Ok(true),
        _ => Err(RssError::NotFound("操作不存在".into())),
    }
}
async fn preview(
    State(service): State<Arc<RssService>>,
    Json(request): Json<PreviewRequest>,
) -> RssResult<Json<PreviewResponse>> {
    Ok(Json(service.preview(request).await?))
}
async fn items(
    State(service): State<Arc<RssService>>,
    Query(query): Query<ListQuery>,
) -> RssResult<Json<Page<ItemRecord>>> {
    Ok(Json(service.db.rss_list_items(query.normalized()).await?))
}
async fn item(
    State(service): State<Arc<RssService>>,
    Path(id): Path<i64>,
) -> RssResult<Json<ItemRecord>> {
    Ok(Json(service.db.rss_get_item(id).await?))
}
async fn backfill(
    State(service): State<Arc<RssService>>,
    Json(request): Json<BackfillRequest>,
) -> RssResult<(StatusCode, Json<RunRecord>)> {
    let value = service.db.rss_backfill(request).await?;
    service.wake.notify_waiters();
    Ok((StatusCode::ACCEPTED, Json(value)))
}
async fn downloads(
    State(service): State<Arc<RssService>>,
    Query(query): Query<ListQuery>,
) -> RssResult<Json<Page<JobRecord>>> {
    let jobs = service.db.rss_list_jobs(query.normalized()).await?;
    Ok(Json(service.with_download_state(jobs).await))
}
async fn download(
    State(service): State<Arc<RssService>>,
    Path(id): Path<i64>,
) -> RssResult<Json<JobRecord>> {
    let mut job = service.db.rss_get_job(id).await?;
    service.decorate_job(&mut job).await;
    Ok(Json(job))
}
async fn download_action(
    State(service): State<Arc<RssService>>,
    Path((id, action)): Path<(i64, String)>,
    Json(request): Json<ActionRequest>,
) -> RssResult<Json<JobRecord>> {
    if !matches!(action.as_str(), "retry" | "reconcile" | "cancel") {
        return Err(RssError::NotFound("操作不存在".into()));
    }
    let job = service.db.rss_job_action(id, &action, request).await?;
    service.wake.notify_waiters();
    Ok(Json(job))
}
async fn runs(
    State(service): State<Arc<RssService>>,
    Query(query): Query<ListQuery>,
) -> RssResult<Json<Page<RunRecord>>> {
    Ok(Json(service.db.rss_list_runs(query.normalized()).await?))
}
async fn run(
    State(service): State<Arc<RssService>>,
    Path(id): Path<i64>,
) -> RssResult<Json<RunRecord>> {
    Ok(Json(service.db.rss_get_run(id).await?))
}
