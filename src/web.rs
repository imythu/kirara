mod search;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use crate::listener::{ListenEndpoint, Listener};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use chrono::Utc;
use futures::stream;
use rust_embed::RustEmbed;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing::{info, info_span, warn};

use crate::brush::BrushTaskRequest;
use crate::brush::scheduler::BrushScheduler;
use crate::collector::DownloaderSnapshotCollector;
use crate::config::GlobalConfig;
use crate::db::{
    Database, ManualMediaRelocationTarget, MediaRelocationJob, OpenListConfig, OpenListPathMapping,
    OpenListTargetDirectory,
};
use crate::downloader::DownloaderSpaceStats;
use crate::downloader::{DownloaderClient, DownloaderClientPool};
use crate::error::AppError;
use crate::indexer::IndexerError;
use crate::media::domain::{MediaTarget, ReleaseInfo, ReleaseParser};
use crate::media::models::{
    MediaDownloadDeletion, MediaDownloadRecord, MediaSettings, QualityProfileRecord,
    QualityProfileRequest, SubscriptionRecord, UpdateSubscription, media_download_category,
};
use crate::media::scheduler::MediaScheduler;
use crate::media::service::{
    CreateSubscriptionRequest, FailedDownloadReconciliation, MediaService, MediaServiceError,
    QueueDownloadRequest, ResourceSearchRequest, ResourceSearchResponse, SubscriptionRunResult,
    SubscriptionRunSnapshot,
};
use crate::media::tmdb::{TmdbDetails, TmdbError, TmdbMedia, TmdbMediaType, TmdbSeason};
use crate::monitor::{SystemMonitor, SystemSnapshot, SystemSnapshotRecord};
use crate::net::client_factory;
use crate::openlist::{OpenListClient, OpenListTask, openlist_identity_key};
use crate::ptd_backup::{PtdBackupRunResult, ptd_site_id};
use crate::relocation::{
    RelocationScheduler, archive_relative_directory, is_path_prefix, join_path, normalize_path,
    torrent_is_complete, validate_torrent_files_complete,
};
use crate::sign_in::scheduler::SignInScheduler;
use crate::site::factory as site_factory;
use crate::site::{
    SiteAuth, SiteRequestHeader, SiteStatsRecord, SiteType, SiteWithStats,
    default_site_request_headers, normalize_site_request_headers, parse_site_request_headers,
};
use crate::site_stats::SiteStatsRefresher;
use crate::tag_rule::scheduler::TagRuleScheduler;

#[derive(Clone)]
pub struct AppState {
    db: Database,
    scheduler: Arc<BrushScheduler>,
    sign_in_scheduler: Arc<SignInScheduler>,
    site_stats_refresher: Arc<SiteStatsRefresher>,
    collector: Arc<DownloaderSnapshotCollector>,
    pool: Arc<DownloaderClientPool>,
    media: Arc<MediaService>,
    media_scheduler: Arc<MediaScheduler>,
    monitor: Arc<SystemMonitor>,
    tag_rule_scheduler: Arc<TagRuleScheduler>,
    relocation_scheduler: Arc<RelocationScheduler>,
    self_use: bool,
    shutdown: CancellationToken,
}

#[derive(Debug, Deserialize)]
struct BrushTorrentsQuery {
    page: Option<usize>,
    page_size: Option<usize>,
    keyword: Option<String>,
}

#[derive(Debug, Serialize)]
struct BrushTaskTorrentsResponse {
    task: crate::brush::BrushTaskRecord,
    page: usize,
    page_size: usize,
    total_records: usize,
    records: Vec<crate::brush::BrushTorrentRecord>,
}

#[derive(RustEmbed)]
#[folder = "frontend/dist"]
struct FrontendAssets;

impl AppState {
    pub fn new(
        db: Database,
        scheduler: Arc<BrushScheduler>,
        sign_in_scheduler: Arc<SignInScheduler>,
        site_stats_refresher: Arc<SiteStatsRefresher>,
        collector: Arc<DownloaderSnapshotCollector>,
        pool: Arc<DownloaderClientPool>,
        media: Arc<MediaService>,
        media_scheduler: Arc<MediaScheduler>,
        monitor: Arc<SystemMonitor>,
        tag_rule_scheduler: Arc<TagRuleScheduler>,
        relocation_scheduler: Arc<RelocationScheduler>,
        self_use: bool,
    ) -> Self {
        Self {
            db,
            scheduler,
            sign_in_scheduler,
            site_stats_refresher,
            collector,
            pool,
            media,
            media_scheduler,
            monitor,
            tag_rule_scheduler,
            relocation_scheduler,
            self_use,
            shutdown: CancellationToken::new(),
        }
    }
}

pub async fn serve(
    listener: Listener,
    db: Database,
    scheduler: Arc<BrushScheduler>,
    sign_in_scheduler: Arc<SignInScheduler>,
    site_stats_refresher: Arc<SiteStatsRefresher>,
    collector: Arc<DownloaderSnapshotCollector>,
    pool: Arc<DownloaderClientPool>,
    media: Arc<MediaService>,
    media_scheduler: Arc<MediaScheduler>,
    monitor: Arc<SystemMonitor>,
    tag_rule_scheduler: Arc<TagRuleScheduler>,
    relocation_scheduler: Arc<RelocationScheduler>,
    self_use: bool,
    shutdown: CancellationToken,
) -> Result<(), AppError> {
    let addr = listener.endpoint().map_err(|error| AppError::Server {
        message: format!("failed to read bound web server address: {error}"),
    })?;
    let mut state = AppState::new(
        db,
        scheduler,
        sign_in_scheduler,
        site_stats_refresher,
        collector,
        pool,
        media,
        media_scheduler,
        monitor,
        tag_rule_scheduler,
        Arc::clone(&relocation_scheduler),
        self_use,
    );
    state.shutdown = shutdown.clone();
    let app = app_router(state, relocation_scheduler);
    if matches!(&addr, ListenEndpoint::Tcp(addr) if !addr.ip().is_loopback()) {
        warn!(
            "web server is listening on a non-loopback address; place kirara behind an authenticated reverse proxy and restrict network access"
        );
    }
    info!("web server listening on {}", addr);
    let server = std::future::IntoFuture::into_future(
        axum::serve(listener, app).with_graceful_shutdown(shutdown.clone().cancelled_owned()),
    );
    tokio::pin!(server);
    let result = tokio::select! {
        result = &mut server => result,
        _ = shutdown.cancelled() => {
            match tokio::time::timeout(std::time::Duration::from_secs(5), &mut server).await {
                Ok(result) => result,
                Err(_) => {
                    warn!("HTTP shutdown grace period elapsed; closing remaining requests");
                    Ok(())
                }
            }
        }
    };
    result.map_err(|e| AppError::Server {
        message: format!("server exited: {}", e),
    })
}

fn app_router(state: AppState, relocation_scheduler: Arc<RelocationScheduler>) -> Router {
    let media = Arc::clone(&state.media);
    let media_scheduler = Arc::clone(&state.media_scheduler);
    let self_use = state.self_use;
    Router::new()
        .route("/api/sites/search", get(search::sites))
        .route("/api/sign-in-tasks/search", get(search::sign_tasks))
        .route("/api/brush-tasks/search", get(search::brush_tasks))
        .route("/api/sign-in-records/search", get(search::records))
        .route("/api/site-catalog/search", get(search::catalog))
        .route(
            "/api/sites/{id}/search-binding",
            get(search::get_binding).put(search::put_binding),
        )
        .route("/api/features", get(get_features))
        .route("/api/settings", get(get_settings).put(update_settings))
        // 站点管理
        .route("/api/sites", get(list_sites).post(create_site))
        .route("/api/sites/catalog", get(list_site_presets))
        .route(
            "/api/sites/ptd-backup",
            get(get_ptd_backup_config).put(update_ptd_backup_config),
        )
        .route("/api/sites/ptd-backup/test", post(test_ptd_backup))
        .route("/api/sites/ptd-backup/run", post(run_ptd_backup))
        .route("/api/sites/stats-overview", get(get_sites_stats_overview))
        .route(
            "/api/sites/refresh-all",
            get(get_sites_stats_refresh_status).post(start_sites_stats_refresh),
        )
        .route("/api/sites/{id}", put(update_site).delete(delete_site))
        .route("/api/sites/{id}/credentials", get(get_site_credentials))
        .route(
            "/api/sites/{id}/request-headers",
            get(get_site_request_headers),
        )
        .route("/api/sites/{id}/test", post(test_site))
        .route("/api/sites/{id}/stats", get(get_site_stats))
        .route("/api/proxy/test", post(test_proxy))
        // 自动签到
        .route(
            "/api/sign-in-tasks",
            get(list_sign_in_tasks).post(create_sign_in_task),
        )
        .route(
            "/api/sign-in-tasks/{id}",
            put(update_sign_in_task).delete(delete_sign_in_task),
        )
        .route("/api/sign-in-profiles", get(list_sign_in_profiles))
        .route("/api/sign-in-tasks/{id}/start", post(start_sign_in_task))
        .route("/api/sign-in-tasks/{id}/stop", post(stop_sign_in_task))
        .route("/api/sign-in-tasks/{id}/run", post(run_sign_in_task_once))
        .route(
            "/api/sign-in-tasks/{id}/probe-1-1-1-1",
            post(probe_sign_in_task_1_1_1_1),
        )
        .route(
            "/api/sign-in-probe-1-1-1-1",
            post(probe_sign_in_form_1_1_1_1),
        )
        .route("/api/sign-in-records", get(list_sign_in_records))
        // 下载器管理
        .route(
            "/api/downloaders",
            get(list_downloaders).post(create_downloader),
        )
        .route(
            "/api/downloaders/{id}",
            put(update_downloader).delete(delete_downloader),
        )
        .route("/api/downloaders/{id}/test", post(test_downloader))
        .route(
            "/api/downloaders/{id}/space",
            get(get_downloader_space_stats),
        )
        .route(
            "/api/downloaders/{id}/default-path",
            get(get_downloader_default_path),
        )
        .route(
            "/api/downloaders/{id}/torrents",
            get(list_downloader_torrents),
        )
        .route(
            "/api/downloaders/{id}/openlist-transfer",
            post(create_openlist_transfer),
        )
        .route(
            "/api/downloaders/{id}/openlist-transfer/preview",
            post(preview_openlist_transfer),
        )
        // 刷流任务
        .route(
            "/api/brush-tasks",
            get(list_brush_tasks).post(create_brush_task),
        )
        .route(
            "/api/brush-tasks/{id}",
            get(get_brush_task)
                .put(update_brush_task)
                .delete(delete_brush_task),
        )
        .route("/api/brush-tasks/{id}/start", post(start_brush_task))
        .route("/api/brush-tasks/{id}/stop", post(stop_brush_task))
        .route("/api/brush-tasks/{id}/run", post(run_brush_task_once))
        .route(
            "/api/brush-tasks/{id}/torrents",
            get(list_brush_task_torrents),
        )
        .route("/api/system/logs/stream", get(stream_logs))
        // 统计
        .route("/api/stats/overview", get(stats_overview))
        .route("/api/stats/trend", get(stats_trend))
        .route(
            "/api/stats/downloader-speed-trend",
            get(downloader_speed_trend),
        )
        .route("/api/stats/daily-transfer", get(daily_transfer))
        // 系统监控
        .route("/api/system/stats", get(get_system_stats))
        .route("/api/system/stats/history", get(get_system_stats_history))
        // 标签规则
        .route("/api/tag-rules", get(list_tag_rules).post(create_tag_rule))
        .route("/api/tag-rules/trackers", get(list_tag_rule_trackers))
        .route(
            "/api/tag-rules/{id}",
            get(get_tag_rule)
                .put(update_tag_rule)
                .delete(delete_tag_rule),
        )
        .route("/api/tag-rules/scan", post(scan_tag_rules))
        .route("/", get(index))
        .route("/{*path}", get(static_asset))
        .with_state(state)
        .nest(
            "/api/media",
            media_router(media, media_scheduler, relocation_scheduler, self_use),
        )
        .layer(
            TraceLayer::new_for_http().make_span_with(|request: &axum::http::Request<_>| {
                info_span!(
                    "http",
                    method = %request.method(),
                    path = %request.uri().path(),
                )
            }),
        )
        .layer(cors_layer())
}

const VITE_DEV_ORIGINS: [&str; 3] = [
    "http://localhost:5173",
    "http://127.0.0.1:5173",
    "http://[::1]:5173",
];

fn cors_layer() -> CorsLayer {
    let origins = VITE_DEV_ORIGINS.map(HeaderValue::from_static);
    CorsLayer::new()
        .allow_origin(origins)
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
        .allow_headers([header::ACCEPT, header::CONTENT_TYPE])
}

#[derive(Clone)]
struct MediaApiState {
    service: Arc<MediaService>,
    scheduler: Arc<MediaScheduler>,
    relocation_scheduler: Arc<RelocationScheduler>,
}

fn media_router(
    service: Arc<MediaService>,
    scheduler: Arc<MediaScheduler>,
    relocation_scheduler: Arc<RelocationScheduler>,
    self_use: bool,
) -> Router {
    let router = Router::new()
        .route(
            "/settings",
            get(get_media_settings).put(update_media_settings),
        )
        .route("/tmdb/search", get(search_tmdb_media))
        .route("/tmdb/details", get(get_tmdb_details_query))
        .route("/tmdb/season", get(get_tmdb_season_query))
        .route("/tmdb/{media_type}/{id}", get(get_tmdb_details_path))
        .route("/tmdb/tv/{id}/season/{season}", get(get_tmdb_season_path))
        .route(
            "/quality-profiles",
            get(list_quality_profiles).post(create_quality_profile),
        )
        .route("/quality-profiles/reset", post(reset_quality_profiles))
        .route(
            "/quality-profiles/{id}",
            get(get_quality_profile)
                .put(update_quality_profile)
                .delete(delete_quality_profile),
        )
        .route(
            "/subscriptions",
            get(list_media_subscriptions).post(create_media_subscription),
        )
        .route(
            "/subscriptions/{id}",
            get(get_media_subscription)
                .put(update_media_subscription)
                .delete(delete_media_subscription),
        )
        .route("/subscriptions/{id}/run", post(run_media_subscription))
        .route(
            "/subscriptions/{id}/last-run",
            get(get_media_subscription_last_run),
        )
        .route("/subscriptions/{id}/pause", post(pause_media_subscription))
        .route(
            "/subscriptions/{id}/resume",
            post(resume_media_subscription),
        )
        .route(
            "/subscriptions/{id}/downloads",
            get(list_subscription_downloads),
        )
        .route("/resources/search", post(search_media_resources))
        .route(
            "/downloads",
            get(list_media_downloads).post(queue_media_download),
        )
        .route(
            "/downloads/{id}",
            get(get_media_download).delete(delete_media_download),
        )
        .route(
            "/downloads/{id}/reconcile-failed",
            post(reconcile_failed_media_download),
        )
        .route("/downloads/{id}/redeliver", post(redeliver_media_download));
    let router = if self_use {
        router
            .route(
                "/openlist/settings",
                get(get_openlist_config).put(update_openlist_config),
            )
            .route("/openlist/jobs", get(list_openlist_jobs))
            .route("/openlist/manual-jobs", get(list_manual_openlist_jobs))
            .route("/openlist/jobs/clear-all", post(clear_openlist_jobs))
            .route(
                "/openlist/jobs/{id}/resolve-copy",
                post(resolve_openlist_copy),
            )
            .route(
                "/openlist/jobs/{id}/resolve-migration",
                post(resolve_openlist_migration),
            )
            .route("/openlist/scan", post(scan_openlist_jobs))
    } else {
        router
    };
    router.with_state(MediaApiState {
        service,
        scheduler,
        relocation_scheduler,
    })
}

#[derive(Debug, Serialize)]
struct OpenListConfigResponse {
    address: String,
    api_key: Option<String>,
    api_key_configured: bool,
    enabled: bool,
    target_directory_id: Option<i64>,
    selected_target_index: Option<usize>,
    scan_interval_mins: u64,
    updated_at: String,
    source_mappings: Vec<OpenListPathMapping>,
    target_directories: Vec<OpenListTargetDirectory>,
}

#[derive(Debug, Serialize)]
struct OpenListJobResponse {
    id: i64,
    media_download_id: Option<i64>,
    downloader_id: Option<i64>,
    infohash: String,
    torrent_name: String,
    stage: String,
    workflow: &'static str,
    version: i64,
    source_qb_path: String,
    source_openlist_path: String,
    target_openlist_path: String,
    target_qb_path: String,
    attempts: u32,
    openlist_task_ids: Vec<String>,
    copy_checkpoint: Option<OpenListCheckpointSummary>,
    manual_resolution_allowed: bool,
    copy_resolution_actions: Vec<&'static str>,
    migration_resolution_allowed: bool,
    copy_lock_acquired: bool,
    manifest_cursor: usize,
    next_attempt_at: Option<String>,
    last_error: Option<String>,
    created_at: String,
    updated_at: String,
    stage_started_at: String,
    completed_at: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct OpenListCheckpointSummary {
    path: String,
    size: i64,
    #[serde(default = "default_copy_checkpoint_operation")]
    operation: String,
    phase: String,
    submitted_at: Option<String>,
    #[serde(default)]
    terminal_failure_verified: bool,
}

fn default_copy_checkpoint_operation() -> String {
    "copy_file".to_string()
}

impl From<MediaRelocationJob> for OpenListJobResponse {
    fn from(job: MediaRelocationJob) -> Self {
        let openlist_task_ids = decode_openlist_job_task_ids(job.openlist_task_id.as_deref());
        let copy_checkpoint = job
            .copy_checkpoint_json
            .as_deref()
            .and_then(|value| serde_json::from_str(value).ok());
        let copy_resolution_actions = copy_resolution_actions(&job);
        let manual_resolution_allowed = !copy_resolution_actions.is_empty();
        let workflow = if job.media_download_id.is_none() {
            "qb_migration"
        } else {
            "auto_copy"
        };
        let migration_resolution_allowed = workflow == "qb_migration"
            && matches!(
                job.stage.as_str(),
                "qb_manual_review" | "source_remove_manual_review"
            );
        Self {
            id: job.id,
            media_download_id: job.media_download_id,
            downloader_id: job.downloader_id,
            infohash: job.infohash,
            torrent_name: job.torrent_name,
            stage: job.stage,
            workflow,
            version: job.version,
            source_qb_path: job.source_qb_path,
            source_openlist_path: job.source_openlist_path,
            target_openlist_path: job.target_openlist_path,
            target_qb_path: job.target_qb_path,
            attempts: job.attempts,
            openlist_task_ids,
            copy_checkpoint,
            manual_resolution_allowed,
            copy_resolution_actions,
            migration_resolution_allowed,
            copy_lock_acquired: job.copy_lock_acquired,
            manifest_cursor: job.manifest_cursor,
            next_attempt_at: job.next_attempt_at,
            last_error: job.last_error,
            created_at: job.created_at,
            updated_at: job.updated_at,
            stage_started_at: job.stage_started_at,
            completed_at: job.completed_at,
        }
    }
}

fn copy_resolution_actions(job: &MediaRelocationJob) -> Vec<&'static str> {
    match job.stage.as_str() {
        "planning_manual_review"
            if job.media_download_id.is_some()
                && job.openlist_task_id.is_none()
                && job.copy_checkpoint_json.is_none()
                && !job.copy_lock_acquired =>
        {
            vec!["recheck", "cancel"]
        }
        "copy_manual_review" => vec!["recheck", "cancel"],
        "copying" if job.copy_checkpoint_json.is_none() => {
            vec!["recheck", "cancel"]
        }
        "manifest_required" if job.media_download_id.is_none() => vec!["recheck", "cancel"],
        "manifest_required" | "auto_copy_paused" => vec!["cancel"],
        _ => Vec::new(),
    }
}

#[derive(Debug, Deserialize)]
struct OpenListJobsQuery {
    page: Option<usize>,
    page_size: Option<usize>,
}

#[derive(Debug, Serialize)]
struct OpenListJobsResponse {
    page: usize,
    page_size: usize,
    total: usize,
    records: Vec<OpenListJobResponse>,
}

#[derive(Debug, Serialize)]
struct ClearOpenListJobsResponse {
    cleared: usize,
}

async fn list_openlist_jobs(
    State(state): State<MediaApiState>,
    Query(query): Query<OpenListJobsQuery>,
) -> Result<Json<OpenListJobsResponse>, ApiError> {
    let page = query.page.unwrap_or(1).max(1);
    let page_size = query.page_size.unwrap_or(20).clamp(1, 100);
    let (records, total) = state
        .service
        .database()
        .list_automatic_media_relocation_jobs(page, page_size)
        .await
        .map_err(media_app_error)?;
    Ok(Json(OpenListJobsResponse {
        page,
        page_size,
        total,
        records: records.into_iter().map(Into::into).collect(),
    }))
}

async fn clear_openlist_jobs(
    State(state): State<MediaApiState>,
) -> Result<Json<ClearOpenListJobsResponse>, ApiError> {
    let cleared = state
        .service
        .database()
        .stop_and_clear_automatic_media_relocation_jobs()
        .await
        .map_err(media_app_error)?;
    state.relocation_scheduler.request_scan();
    Ok(Json(ClearOpenListJobsResponse { cleared }))
}

#[derive(Debug, Deserialize)]
struct ManualOpenListJobsQuery {
    page: Option<usize>,
    page_size: Option<usize>,
}

#[derive(Debug, Serialize)]
struct ManualOpenListJobsResponse {
    page: usize,
    page_size: usize,
    total: usize,
    records: Vec<OpenListJobResponse>,
}

async fn list_manual_openlist_jobs(
    State(state): State<MediaApiState>,
    Query(query): Query<ManualOpenListJobsQuery>,
) -> Result<Json<ManualOpenListJobsResponse>, ApiError> {
    let page = query.page.unwrap_or(1).max(1);
    let page_size = query.page_size.unwrap_or(20).clamp(1, 100);
    let (records, total) = state
        .service
        .database()
        .list_manual_media_relocation_jobs(page, page_size)
        .await
        .map_err(media_app_error)?;
    Ok(Json(ManualOpenListJobsResponse {
        page,
        page_size,
        total,
        records: records.into_iter().map(OpenListJobResponse::from).collect(),
    }))
}

#[derive(Debug, Deserialize)]
struct ResolveOpenListCopyRequest {
    resolution: String,
    expected_version: i64,
    #[serde(default)]
    confirm_task_terminated: bool,
}

async fn resolve_openlist_copy(
    State(state): State<MediaApiState>,
    Path(id): Path<i64>,
    Json(payload): Json<ResolveOpenListCopyRequest>,
) -> Result<Json<OpenListJobResponse>, ApiError> {
    let resolution = payload.resolution.trim();
    if !matches!(resolution, "recheck" | "cancel") {
        return Err(ApiError::bad_request(
            "resolution must be recheck or cancel",
        ));
    }
    let db = state.service.database();
    let current = db
        .get_media_relocation_job(id)
        .await
        .map_err(media_app_error)?
        .ok_or_else(|| ApiError::not_found("OpenList relocation job not found"))?;
    if current.version != payload.expected_version {
        return Err(ApiError::conflict(
            "OpenList relocation job version changed; reload before resolving",
        ));
    }
    if !copy_resolution_actions(&current).contains(&resolution) {
        return Err(ApiError::conflict(
            "OpenList relocation job is not awaiting this manual resolution",
        ));
    }
    let planning_resolution = current.stage == "planning_manual_review";
    if resolution == "cancel" && !planning_resolution && !payload.confirm_task_terminated {
        return Err(ApiError::bad_request(
            "copy-stage cancel requires confirm_task_terminated=true",
        ));
    }
    if resolution == "cancel" && !planning_resolution {
        require_safe_openlist_cancel(verify_openlist_tasks_for_cancel(db, &current).await?)?;
    }
    let changed = if current.stage == "manifest_required" && resolution == "recheck" {
        db.recheck_media_relocation_manifest(id, payload.expected_version)
            .await
    } else {
        db.resolve_media_relocation_copy(
            id,
            resolution,
            payload.expected_version,
            payload.confirm_task_terminated,
        )
        .await
    }
    .map_err(media_app_error)?;
    if !changed {
        return Err(ApiError::conflict(
            "OpenList relocation job changed or cannot apply this resolution",
        ));
    }
    state.relocation_scheduler.request_scan();
    let updated = db
        .get_media_relocation_job(id)
        .await
        .map_err(media_app_error)?
        .ok_or_else(|| ApiError::not_found("OpenList relocation job not found"))?;
    Ok(Json(updated.into()))
}

#[derive(Debug, Deserialize)]
struct ResolveOpenListMigrationRequest {
    expected_version: i64,
    resolution: String,
}

async fn resolve_openlist_migration(
    State(state): State<MediaApiState>,
    Path(id): Path<i64>,
    Json(payload): Json<ResolveOpenListMigrationRequest>,
) -> Result<Json<OpenListJobResponse>, ApiError> {
    let db = state.service.database();
    let current = db
        .get_media_relocation_job(id)
        .await
        .map_err(media_app_error)?
        .ok_or_else(|| ApiError::not_found("OpenList migration job not found"))?;
    if current.version != payload.expected_version {
        return Err(ApiError::conflict(
            "OpenList migration job version changed; reload before retrying",
        ));
    }
    if !matches!(
        current.stage.as_str(),
        "qb_manual_review" | "source_remove_manual_review"
    ) || current.media_download_id.is_some()
    {
        return Err(ApiError::conflict(
            "OpenList migration job is not awaiting a migration retry",
        ));
    }
    let changed = match payload.resolution.as_str() {
        "retry" => {
            db.retry_media_relocation_migration(id, payload.expected_version)
                .await
        }
        "abandon" => {
            db.abandon_media_relocation_migration(id, payload.expected_version)
                .await
        }
        _ => return Err(ApiError::bad_request("resolution must be retry or abandon")),
    }
    .map_err(media_app_error)?;
    if !changed {
        return Err(ApiError::conflict(
            "OpenList migration job changed or cannot be retried",
        ));
    }
    state.relocation_scheduler.request_scan();
    let updated = db
        .get_media_relocation_job(id)
        .await
        .map_err(media_app_error)?
        .ok_or_else(|| ApiError::not_found("OpenList migration job not found"))?;
    Ok(Json(updated.into()))
}

fn decode_openlist_job_task_ids(value: Option<&str>) -> Vec<String> {
    let Some(value) = value else {
        return Vec::new();
    };
    serde_json::from_str::<Vec<String>>(value)
        .unwrap_or_else(|_| vec![value.to_string()])
        .into_iter()
        .filter(|id| !id.trim().is_empty())
        .collect()
}

#[derive(Debug, PartialEq, Eq)]
enum OpenListCancelVerification {
    ProvenSafe,
    Active(Vec<String>),
    Unknown(Vec<String>),
}

fn require_safe_openlist_cancel(verification: OpenListCancelVerification) -> Result<(), ApiError> {
    match verification {
        OpenListCancelVerification::ProvenSafe => Ok(()),
        OpenListCancelVerification::Active(task_ids) => Err(ApiError::conflict(format!(
            "OpenList 任务仍在运行，不能释放复制锁；请先在 OpenList 停止任务后重新检查: {}",
            task_ids.join(", ")
        ))),
        OpenListCancelVerification::Unknown(reasons) => Err(ApiError::conflict(format!(
            "OpenList 任务状态无法证明已经终止，已保留复制锁；请恢复 OpenList 连接并重新检查: {}",
            reasons.join("; ")
        ))),
    }
}

async fn verify_openlist_tasks_for_cancel(
    db: &Database,
    job: &MediaRelocationJob,
) -> Result<OpenListCancelVerification, ApiError> {
    let task_ids = decode_openlist_job_task_ids(job.openlist_task_id.as_deref());
    if task_ids.is_empty() {
        if job.stage == "auto_copy_paused"
            || (job.stage == "manifest_required" && job.copy_checkpoint_json.is_none())
        {
            return Ok(OpenListCancelVerification::ProvenSafe);
        }
        let checkpoint = job
            .copy_checkpoint_json
            .as_deref()
            .and_then(|value| serde_json::from_str::<serde_json::Value>(value).ok())
            .map(|value| {
                let phase = value
                    .get("phase")
                    .and_then(|phase| phase.as_str())
                    .unwrap_or_default()
                    .to_string();
                let operation = value
                    .get("operation")
                    .and_then(|operation| operation.as_str())
                    .unwrap_or("copy_file")
                    .to_string();
                (phase, operation)
            });
        let safe_without_task = checkpoint
            .is_some_and(|(phase, operation)| phase == "prepared" && operation != "remove_file");
        return if safe_without_task {
            Ok(OpenListCancelVerification::ProvenSafe)
        } else {
            Ok(OpenListCancelVerification::Unknown(vec![
                "任务 ID 缺失或提交结果不确定".to_string(),
            ]))
        };
    }
    let config = db.get_openlist_config().await.map_err(media_app_error)?;
    let client = match manual_resolution_openlist_client(&config.base_url, &config.api_key) {
        Ok(client) => client,
        Err(error) => {
            warn!(job_id = job.id, message = %error.message, "could not construct OpenList client before manual resolution");
            return Ok(OpenListCancelVerification::Unknown(vec![error.message]));
        }
    };
    let mut observations = Vec::with_capacity(task_ids.len());
    for task_id in task_ids {
        let result = client.task_info_if_exists(&task_id).await;
        if let Err(error) = &result {
            warn!(
                job_id = job.id,
                task_id,
                %error,
                "OpenList task state is uncertain during confirmed manual resolution"
            );
        }
        observations.push((task_id, result));
    }
    Ok(summarize_openlist_cancel_observations(observations))
}

fn manual_resolution_openlist_client(
    base_url: &str,
    api_key: &str,
) -> Result<OpenListClient, ApiError> {
    OpenListClient::new(base_url, api_key).map_err(|_| {
        ApiError::conflict("OpenList client is unavailable; task termination could not be verified")
    })
}

fn summarize_openlist_cancel_observations(
    observations: Vec<(String, Result<Option<OpenListTask>, String>)>,
) -> OpenListCancelVerification {
    let mut active = Vec::new();
    let mut unknown = Vec::new();
    for (task_id, result) in observations {
        match result {
            Ok(Some(task)) if task.succeeded() || task.terminal_failure() => {}
            Ok(Some(_)) => active.push(task_id),
            Ok(None) => {
                // This confirmed manual-cancel path accepts only the client's exact
                // OpenList task-not-found response; every ambiguous lookup remains Err.
            }
            Err(error) => unknown.push(format!("{task_id}: {error}")),
        }
    }
    if !active.is_empty() {
        OpenListCancelVerification::Active(active)
    } else if !unknown.is_empty() {
        OpenListCancelVerification::Unknown(unknown)
    } else {
        OpenListCancelVerification::ProvenSafe
    }
}

#[derive(Debug, Serialize)]
struct OpenListScanResponse {
    accepted: bool,
    discovered: usize,
    processing_enabled: bool,
}

async fn scan_openlist_jobs(
    State(state): State<MediaApiState>,
) -> Result<Json<OpenListScanResponse>, ApiError> {
    let db = state.service.database();
    let config = db.get_openlist_config().await.map_err(media_app_error)?;
    let discovered = db
        .enqueue_submitted_media_relocation_jobs(config.enabled)
        .await
        .map_err(media_app_error)?;
    state.relocation_scheduler.request_scan();
    Ok(Json(OpenListScanResponse {
        accepted: true,
        discovered,
        processing_enabled: config.enabled,
    }))
}

impl From<OpenListConfig> for OpenListConfigResponse {
    fn from(config: OpenListConfig) -> Self {
        Self {
            address: config.base_url,
            api_key: None,
            api_key_configured: !config.api_key.trim().is_empty(),
            enabled: config.enabled,
            target_directory_id: config.target_directory_id,
            selected_target_index: config.selected_target_index,
            scan_interval_mins: config.scan_interval_secs.div_ceil(60),
            updated_at: config.updated_at,
            source_mappings: config.path_mappings,
            target_directories: config.target_directories,
        }
    }
}

#[derive(Debug, Deserialize)]
struct UpdateOpenListConfigRequest {
    address: String,
    api_key: Option<String>,
    updated_at: String,
    #[serde(default)]
    clear_api_key: bool,
    enabled: bool,
    target_directory_id: Option<i64>,
    selected_target_index: Option<usize>,
    scan_interval_mins: u64,
    #[serde(default)]
    source_mappings: Vec<OpenListPathMapping>,
    #[serde(default)]
    target_directories: Vec<OpenListTargetDirectory>,
}

async fn get_openlist_config(
    State(state): State<MediaApiState>,
) -> Result<Json<OpenListConfigResponse>, ApiError> {
    let config = state
        .service
        .database()
        .get_openlist_config()
        .await
        .map_err(media_app_error)?;
    Ok(Json(config.into()))
}

async fn update_openlist_config(
    State(state): State<MediaApiState>,
    Json(mut payload): Json<UpdateOpenListConfigRequest>,
) -> Result<Json<OpenListConfigResponse>, ApiError> {
    normalize_and_validate_openlist_config(&mut payload)?;
    let db = state.service.database();
    let qb_ids = db
        .list_downloaders()
        .await
        .map_err(media_app_error)?
        .into_iter()
        .filter(|downloader| matches!(downloader.downloader_type.as_str(), "qbittorrent" | "qb"))
        .map(|downloader| downloader.id)
        .collect::<std::collections::HashSet<_>>();
    if payload
        .source_mappings
        .iter()
        .any(|mapping| !qb_ids.contains(&mapping.downloader_id))
        || payload
            .target_directories
            .iter()
            .any(|target| !qb_ids.contains(&target.downloader_id))
    {
        return Err(ApiError::bad_request(
            "all OpenList mappings must reference qBittorrent downloaders",
        ));
    }
    let current = db.get_openlist_config().await.map_err(media_app_error)?;
    require_current_openlist_config_version(&payload.updated_at, &current.updated_at)?;
    if payload.clear_api_key
        && payload
            .api_key
            .as_deref()
            .is_some_and(|key| !key.trim().is_empty())
    {
        return Err(ApiError::bad_request(
            "api_key and clear_api_key cannot be submitted together",
        ));
    }
    let supplied_key = payload
        .api_key
        .take()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let api_key = if payload.clear_api_key {
        String::new()
    } else {
        supplied_key.unwrap_or_else(|| current.api_key.clone())
    };
    if payload.enabled && api_key.is_empty() {
        return Err(ApiError::bad_request(
            "api_key is required when OpenList automation is enabled",
        ));
    }
    if openlist_connection_change_requires_idle(
        &current.base_url,
        &current.api_key,
        &payload.address,
        &api_key,
    ) && db
        .has_in_flight_openlist_operations()
        .await
        .map_err(media_app_error)?
    {
        return Err(ApiError::conflict(
            "OpenList address and API key cannot change while relocation jobs are active",
        ));
    }
    let updated = db
        .update_openlist_config(&OpenListConfig {
            base_url: payload.address,
            api_key,
            enabled: payload.enabled,
            target_directory_id: payload.target_directory_id,
            selected_target_index: payload.selected_target_index,
            scan_interval_secs: payload.scan_interval_mins.saturating_mul(60),
            updated_at: current.updated_at,
            path_mappings: payload.source_mappings,
            target_directories: payload.target_directories,
        })
        .await
        .map_err(media_app_error)?;
    state.relocation_scheduler.request_scan();
    Ok(Json(updated.into()))
}

fn require_current_openlist_config_version(
    expected_updated_at: &str,
    current_updated_at: &str,
) -> Result<(), ApiError> {
    if expected_updated_at != current_updated_at {
        return Err(ApiError::conflict(
            "OpenList settings changed; reload before saving",
        ));
    }
    Ok(())
}

fn openlist_connection_change_requires_idle(
    current_address: &str,
    current_api_key: &str,
    next_address: &str,
    next_api_key: &str,
) -> bool {
    current_address != next_address || current_api_key != next_api_key
}

fn normalize_and_validate_openlist_config(
    payload: &mut UpdateOpenListConfigRequest,
) -> Result<(), ApiError> {
    payload.address = payload.address.trim().trim_end_matches('/').to_string();
    if !payload.address.is_empty() {
        let url = reqwest::Url::parse(&payload.address)
            .map_err(|_| ApiError::bad_request("openlist base_url is invalid"))?;
        if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
            return Err(ApiError::bad_request(
                "openlist base_url must be an absolute HTTP(S) URL",
            ));
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(ApiError::bad_request(
                "openlist address must not contain credentials",
            ));
        }
        if url.query().is_some() || url.fragment().is_some() {
            return Err(ApiError::bad_request(
                "openlist base_url must not contain a query or fragment",
            ));
        }
    }
    if payload.scan_interval_mins == 0 || payload.scan_interval_mins > 1_440 {
        return Err(ApiError::bad_request(
            "scan_interval_mins must be between 1 and 1440",
        ));
    }
    for mapping in &mut payload.source_mappings {
        if mapping.downloader_id <= 0 {
            return Err(ApiError::bad_request(
                "mapping downloader_id must be positive",
            ));
        }
        mapping.qb_path = normalize_absolute_mapping_path(&mapping.qb_path)?;
        mapping.openlist_path = normalize_absolute_mapping_path(&mapping.openlist_path)?;
    }
    for target in &mut payload.target_directories {
        target.name = target.name.trim().to_string();
        if target.name.is_empty() || target.name.len() > 100 {
            return Err(ApiError::bad_request(
                "target directory name must contain between 1 and 100 bytes",
            ));
        }
        if target.downloader_id <= 0 {
            return Err(ApiError::bad_request(
                "target directory downloader_id must be positive",
            ));
        }
        target.openlist_path = normalize_absolute_mapping_path(&target.openlist_path)?;
        target.qb_path = normalize_absolute_mapping_path(&target.qb_path)?;
    }
    validate_non_overlapping_mapping_paths(&payload.source_mappings)?;
    validate_non_overlapping_target_paths(&payload.target_directories)?;
    for mapping in &payload.source_mappings {
        for target in &payload.target_directories {
            if openlist_paths_overlap(&mapping.openlist_path, &target.openlist_path) {
                return Err(ApiError::bad_request(
                    "OpenList source mappings and target directories must not overlap",
                ));
            }
        }
    }
    if let Some(index) = payload.selected_target_index {
        if index >= payload.target_directories.len() {
            return Err(ApiError::bad_request(
                "selected_target_index is out of range",
            ));
        }
    } else if let Some(id) = payload.target_directory_id
        && !payload
            .target_directories
            .iter()
            .any(|target| target.id == Some(id))
    {
        return Err(ApiError::bad_request(
            "target_directory_id does not reference a submitted target",
        ));
    }
    if payload.enabled {
        if payload.address.is_empty() {
            return Err(ApiError::bad_request("address is required when enabled"));
        }
        let has_key = payload
            .api_key
            .as_deref()
            .is_some_and(|key| !key.trim().is_empty());
        if payload.clear_api_key && has_key {
            return Err(ApiError::bad_request(
                "api_key and clear_api_key cannot be submitted together",
            ));
        }
        if payload.selected_target_index.is_none() && payload.target_directory_id.is_none() {
            return Err(ApiError::bad_request(
                "a target directory must be selected when enabled",
            ));
        }
        if payload.source_mappings.is_empty() || payload.target_directories.is_empty() {
            return Err(ApiError::bad_request(
                "path mappings and target directories are required when enabled",
            ));
        }
    }
    Ok(())
}

fn normalize_absolute_mapping_path(value: &str) -> Result<String, ApiError> {
    let replaced = value.trim().replace('\\', "/");
    if !replaced.starts_with('/') {
        return Err(ApiError::bad_request("mapping paths must be absolute"));
    }
    let mut segments = Vec::new();
    for segment in replaced.split('/') {
        match segment {
            "" | "." => {}
            ".." => return Err(ApiError::bad_request("mapping paths must not contain '..'")),
            value => segments.push(value),
        }
    }
    Ok(if segments.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", segments.join("/"))
    })
}

fn paths_overlap(left: &str, right: &str) -> bool {
    left == right
        || left == "/"
        || right == "/"
        || left
            .strip_prefix(right)
            .is_some_and(|suffix| suffix.starts_with('/'))
        || right
            .strip_prefix(left)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

fn openlist_paths_overlap(left: &str, right: &str) -> bool {
    paths_overlap(&openlist_identity_key(left), &openlist_identity_key(right))
}

fn validate_non_overlapping_mapping_paths(
    mappings: &[OpenListPathMapping],
) -> Result<(), ApiError> {
    for (index, left) in mappings.iter().enumerate() {
        for right in &mappings[index + 1..] {
            if (left.downloader_id == right.downloader_id
                && paths_overlap(&left.qb_path, &right.qb_path))
                || openlist_paths_overlap(&left.openlist_path, &right.openlist_path)
            {
                return Err(ApiError::bad_request(
                    "path mappings for one downloader must not overlap",
                ));
            }
        }
    }
    Ok(())
}

fn validate_non_overlapping_target_paths(
    targets: &[OpenListTargetDirectory],
) -> Result<(), ApiError> {
    for (index, left) in targets.iter().enumerate() {
        for right in &targets[index + 1..] {
            if left.name == right.name {
                return Err(ApiError::bad_request(
                    "target directory names must be unique",
                ));
            }
            if (left.downloader_id == right.downloader_id
                && paths_overlap(&left.qb_path, &right.qb_path))
                || openlist_paths_overlap(&left.openlist_path, &right.openlist_path)
            {
                return Err(ApiError::bad_request(
                    "target directories for one downloader must not overlap",
                ));
            }
        }
    }
    Ok(())
}

#[derive(Debug, Serialize)]
struct MediaSettingsResponse {
    // Never return the configured token. `tmdb_token_configured` is sufficient for the UI.
    tmdb_token: Option<String>,
    tmdb_token_configured: bool,
    tmdb_language: String,
    scan_interval_mins: u64,
    max_search_queries: usize,
    search_concurrency: usize,
    updated_at: String,
}

impl MediaSettingsResponse {
    fn from_settings(settings: MediaSettings) -> Self {
        let tmdb_token_configured = settings
            .tmdb_token
            .as_deref()
            .is_some_and(|token| !token.trim().is_empty());
        Self {
            tmdb_token: None,
            tmdb_token_configured,
            tmdb_language: settings.tmdb_language,
            scan_interval_mins: settings.scan_interval_mins,
            max_search_queries: settings.max_search_queries,
            search_concurrency: settings.search_concurrency,
            updated_at: settings.updated_at,
        }
    }
}

#[derive(Debug, Deserialize)]
struct UpdateMediaSettingsRequest {
    tmdb_token: Option<String>,
    #[serde(default)]
    clear_tmdb_token: bool,
    tmdb_language: String,
    scan_interval_mins: u64,
    max_search_queries: usize,
    search_concurrency: usize,
}

#[derive(Debug, Deserialize)]
struct TmdbSearchQuery {
    query: String,
    media_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TmdbDetailsQuery {
    tmdb_id: i64,
    media_type: String,
}

#[derive(Debug, Deserialize)]
struct TmdbSeasonQuery {
    tmdb_id: i64,
    season: u32,
}

#[derive(Debug, Default, Deserialize)]
struct MediaDownloadsQuery {
    subscription_id: Option<i64>,
    status: Option<String>,
    page: Option<usize>,
    page_size: Option<usize>,
    before_id: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
struct SubscriptionDownloadsQuery {
    status: Option<String>,
    page: Option<usize>,
    page_size: Option<usize>,
    before_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct DeleteMediaDownloadQuery {
    version: i64,
}

async fn get_media_settings(
    State(state): State<MediaApiState>,
) -> Result<Json<MediaSettingsResponse>, ApiError> {
    let settings = state
        .service
        .database()
        .get_media_settings()
        .await
        .map_err(media_app_error)?;
    Ok(Json(MediaSettingsResponse::from_settings(settings)))
}

async fn update_media_settings(
    State(state): State<MediaApiState>,
    Json(payload): Json<UpdateMediaSettingsRequest>,
) -> Result<Json<MediaSettingsResponse>, ApiError> {
    validate_media_settings(&payload)?;
    let current = state
        .service
        .database()
        .get_media_settings()
        .await
        .map_err(media_app_error)?;
    let supplied_token = payload
        .tmdb_token
        .as_deref()
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(str::to_string);
    let tmdb_token = if payload.clear_tmdb_token {
        None
    } else {
        supplied_token.or(current.tmdb_token)
    };
    let updated = state
        .service
        .database()
        .update_media_settings(&MediaSettings {
            tmdb_token,
            tmdb_language: payload.tmdb_language.trim().to_string(),
            scan_interval_mins: payload.scan_interval_mins,
            max_search_queries: payload.max_search_queries,
            search_concurrency: payload.search_concurrency,
            updated_at: current.updated_at,
        })
        .await
        .map_err(media_app_error)?;
    Ok(Json(MediaSettingsResponse::from_settings(updated)))
}

async fn search_tmdb_media(
    State(state): State<MediaApiState>,
    Query(query): Query<TmdbSearchQuery>,
) -> Result<Json<Vec<TmdbMedia>>, ApiError> {
    let text = query.query.trim();
    if text.is_empty() {
        return Err(ApiError::bad_request("query is required"));
    }
    if text.len() > 200 {
        return Err(ApiError::bad_request("query must not exceed 200 bytes"));
    }
    let media_type = normalize_tmdb_search_type(query.media_type.as_deref())?;
    Ok(Json(
        state
            .service
            .tmdb_search(text, media_type)
            .await
            .map_err(ApiError::from)?,
    ))
}

async fn get_tmdb_details_query(
    State(state): State<MediaApiState>,
    Query(query): Query<TmdbDetailsQuery>,
) -> Result<Json<TmdbDetails>, ApiError> {
    tmdb_details(&state, query.tmdb_id, &query.media_type).await
}

async fn get_tmdb_details_path(
    State(state): State<MediaApiState>,
    Path((media_type, id)): Path<(String, i64)>,
) -> Result<Json<TmdbDetails>, ApiError> {
    tmdb_details(&state, id, &media_type).await
}

async fn tmdb_details(
    state: &MediaApiState,
    tmdb_id: i64,
    media_type: &str,
) -> Result<Json<TmdbDetails>, ApiError> {
    validate_positive_id(tmdb_id, "tmdb_id")?;
    let media_type = parse_tmdb_media_type(media_type)?;
    Ok(Json(
        state
            .service
            .tmdb_details(tmdb_id, media_type)
            .await
            .map_err(ApiError::from)?,
    ))
}

async fn get_tmdb_season_query(
    State(state): State<MediaApiState>,
    Query(query): Query<TmdbSeasonQuery>,
) -> Result<Json<TmdbSeason>, ApiError> {
    tmdb_season(&state, query.tmdb_id, query.season).await
}

async fn get_tmdb_season_path(
    State(state): State<MediaApiState>,
    Path((id, season)): Path<(i64, u32)>,
) -> Result<Json<TmdbSeason>, ApiError> {
    tmdb_season(&state, id, season).await
}

async fn tmdb_season(
    state: &MediaApiState,
    tmdb_id: i64,
    season: u32,
) -> Result<Json<TmdbSeason>, ApiError> {
    validate_positive_id(tmdb_id, "tmdb_id")?;
    if season > 1_000 {
        return Err(ApiError::bad_request("season must not exceed 1000"));
    }
    Ok(Json(
        state
            .service
            .tmdb_season(tmdb_id, season)
            .await
            .map_err(ApiError::from)?,
    ))
}

async fn list_quality_profiles(
    State(state): State<MediaApiState>,
) -> Result<Json<Vec<QualityProfileRecord>>, ApiError> {
    Ok(Json(
        state
            .service
            .database()
            .list_quality_profiles()
            .await
            .map_err(media_app_error)?,
    ))
}

async fn get_quality_profile(
    State(state): State<MediaApiState>,
    Path(id): Path<i64>,
) -> Result<Json<QualityProfileRecord>, ApiError> {
    validate_positive_id(id, "quality profile id")?;
    let profile = state
        .service
        .database()
        .get_quality_profile(id)
        .await
        .map_err(media_app_error)?
        .ok_or_else(|| ApiError::not_found("quality profile not found"))?;
    Ok(Json(profile))
}

async fn create_quality_profile(
    State(state): State<MediaApiState>,
    Json(mut payload): Json<QualityProfileRequest>,
) -> Result<(StatusCode, Json<QualityProfileRecord>), ApiError> {
    normalize_and_validate_quality_profile(&mut payload)?;
    let profile = state
        .service
        .database()
        .create_quality_profile(&payload)
        .await
        .map_err(media_app_error)?;
    Ok((StatusCode::CREATED, Json(profile)))
}

async fn reset_quality_profiles(
    State(state): State<MediaApiState>,
) -> Result<Json<Vec<QualityProfileRecord>>, ApiError> {
    Ok(Json(
        state
            .service
            .database()
            .reset_quality_profiles()
            .await
            .map_err(media_app_error)?,
    ))
}

async fn update_quality_profile(
    State(state): State<MediaApiState>,
    Path(id): Path<i64>,
    Json(mut payload): Json<QualityProfileRequest>,
) -> Result<Json<QualityProfileRecord>, ApiError> {
    validate_positive_id(id, "quality profile id")?;
    normalize_and_validate_quality_profile(&mut payload)?;
    let profile = state
        .service
        .database()
        .update_quality_profile(id, &payload)
        .await
        .map_err(media_app_error)?
        .ok_or_else(|| ApiError::not_found("quality profile not found"))?;
    Ok(Json(profile))
}

async fn delete_quality_profile(
    State(state): State<MediaApiState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    validate_positive_id(id, "quality profile id")?;
    if state
        .service
        .database()
        .delete_quality_profile(id)
        .await
        .map_err(media_app_error)?
    {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found("quality profile not found"))
    }
}

async fn list_media_subscriptions(
    State(state): State<MediaApiState>,
) -> Result<Json<Vec<SubscriptionRecord>>, ApiError> {
    Ok(Json(
        state
            .service
            .database()
            .list_subscriptions()
            .await
            .map_err(media_app_error)?,
    ))
}

async fn get_media_subscription(
    State(state): State<MediaApiState>,
    Path(id): Path<i64>,
) -> Result<Json<SubscriptionRecord>, ApiError> {
    Ok(Json(load_media_subscription(&state, id).await?))
}

async fn create_media_subscription(
    State(state): State<MediaApiState>,
    Json(mut payload): Json<CreateSubscriptionRequest>,
) -> Result<(StatusCode, Json<SubscriptionRecord>), ApiError> {
    normalize_and_validate_new_subscription(&mut payload)?;
    let subscription = state
        .service
        .create_subscription(&payload)
        .await
        .map_err(ApiError::from)?;
    if subscription.enabled {
        state.scheduler.wake();
    }
    Ok((StatusCode::CREATED, Json(subscription)))
}

async fn update_media_subscription(
    State(state): State<MediaApiState>,
    Path(id): Path<i64>,
    Json(mut payload): Json<UpdateSubscription>,
) -> Result<Json<SubscriptionRecord>, ApiError> {
    let current = load_media_subscription(&state, id).await?;
    if subscription_is_leased(&current) {
        return Err(ApiError::conflict(
            "subscription is currently being scanned",
        ));
    }
    normalize_and_validate_subscription_update(&current, &mut payload)?;
    ensure_media_references(
        &state,
        payload.quality_profile_id,
        payload.downloader_id,
        &payload.site_ids,
    )
    .await?;
    let updated = state
        .service
        .database()
        .update_subscription(id, current.version, &payload)
        .await
        .map_err(media_app_error)?
        .ok_or_else(|| ApiError::conflict("subscription changed or is currently being scanned"))?;
    if updated.enabled {
        state.scheduler.wake();
    }
    Ok(Json(updated))
}

async fn delete_media_subscription(
    State(state): State<MediaApiState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    let current = load_media_subscription(&state, id).await?;
    if state
        .service
        .database()
        .delete_subscription(id, current.version)
        .await
        .map_err(media_app_error)?
    {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::conflict(
            "subscription changed while being deleted",
        ))
    }
}

async fn run_media_subscription(
    State(state): State<MediaApiState>,
    Path(id): Path<i64>,
) -> Result<Json<SubscriptionRunResult>, ApiError> {
    let subscription = load_media_subscription(&state, id).await?;
    reject_completed_subscription(&subscription, "run")?;
    let result = state
        .service
        .run_subscription(id)
        .await
        .map_err(ApiError::from)?;
    info!(
        subscription_id = result.subscription_id,
        target_key = %result.target_key,
        queries = result.query_count,
        candidates = result.candidate_count,
        accepted = result.accepted_count,
        queued = result.download.is_some(),
        site_errors = result.site_errors.len(),
        "manual media subscription scan completed"
    );
    state.scheduler.wake();
    Ok(Json(result))
}

async fn get_media_subscription_last_run(
    State(state): State<MediaApiState>,
    Path(id): Path<i64>,
) -> Result<Json<SubscriptionRunSnapshot>, ApiError> {
    load_media_subscription(&state, id).await?;
    state
        .service
        .get_subscription_last_run(id)
        .await
        .map_err(ApiError::from)?
        .map(Json)
        .ok_or_else(|| ApiError::not_found("subscription has no recorded run details"))
}

async fn pause_media_subscription(
    State(state): State<MediaApiState>,
    Path(id): Path<i64>,
) -> Result<Json<SubscriptionRecord>, ApiError> {
    set_media_subscription_enabled(&state, id, false).await
}

async fn resume_media_subscription(
    State(state): State<MediaApiState>,
    Path(id): Path<i64>,
) -> Result<Json<SubscriptionRecord>, ApiError> {
    set_media_subscription_enabled(&state, id, true).await
}

async fn set_media_subscription_enabled(
    state: &MediaApiState,
    id: i64,
    enabled: bool,
) -> Result<Json<SubscriptionRecord>, ApiError> {
    let current = load_media_subscription(state, id).await?;
    if enabled {
        reject_completed_subscription(&current, "resume")?;
    }
    if subscription_is_leased(&current) {
        return Err(ApiError::conflict(
            "subscription is currently being scanned",
        ));
    }
    if !state
        .service
        .database()
        .set_subscription_enabled(id, current.version, enabled)
        .await
        .map_err(media_app_error)?
    {
        return Err(ApiError::conflict(
            "subscription changed or is currently being scanned",
        ));
    }
    let subscription = load_media_subscription(state, id).await?;
    if enabled {
        state.scheduler.wake();
    }
    Ok(Json(subscription))
}

async fn search_media_resources(
    State(state): State<MediaApiState>,
    Json(payload): Json<ResourceSearchRequest>,
) -> Result<Json<ResourceSearchResponse>, ApiError> {
    validate_resource_search(&payload)?;
    Ok(Json(
        state
            .service
            .search_resources(&payload)
            .await
            .map_err(ApiError::from)?,
    ))
}

async fn queue_media_download(
    State(state): State<MediaApiState>,
    Json(mut payload): Json<QueueDownloadRequest>,
) -> Result<(StatusCode, Json<MediaDownloadResponse>), ApiError> {
    normalize_and_validate_download(&state, &mut payload).await?;
    let download = state
        .service
        .queue_download(&payload)
        .await
        .map_err(ApiError::from)?;
    state.scheduler.wake();
    Ok((
        StatusCode::CREATED,
        Json(MediaDownloadResponse::from(download)),
    ))
}

async fn list_media_downloads(
    State(state): State<MediaApiState>,
    Query(query): Query<MediaDownloadsQuery>,
) -> Result<Json<Vec<MediaDownloadResponse>>, ApiError> {
    let (status, limit, offset) =
        validate_download_query(query.status.as_deref(), query.page, query.page_size)?;
    let before_id = validate_download_cursor(query.before_id, query.page)?;
    if let Some(subscription_id) = query.subscription_id {
        load_media_subscription(&state, subscription_id).await?;
    }
    let db = state.service.database();
    let downloads = if let Some(before_id) = before_id {
        db.list_media_downloads_before(query.subscription_id, status.as_deref(), limit, before_id)
            .await
    } else {
        db.list_media_downloads(query.subscription_id, status.as_deref(), limit, offset)
            .await
    }
    .map_err(media_app_error)?;
    Ok(Json(
        media_download_responses(state.service.database(), downloads).await?,
    ))
}

async fn list_subscription_downloads(
    State(state): State<MediaApiState>,
    Path(id): Path<i64>,
    Query(query): Query<SubscriptionDownloadsQuery>,
) -> Result<Json<Vec<MediaDownloadResponse>>, ApiError> {
    load_media_subscription(&state, id).await?;
    let (status, limit, offset) =
        validate_download_query(query.status.as_deref(), query.page, query.page_size)?;
    let before_id = validate_download_cursor(query.before_id, query.page)?;
    let db = state.service.database();
    let downloads = if let Some(before_id) = before_id {
        db.list_media_downloads_before(Some(id), status.as_deref(), limit, before_id)
            .await
    } else {
        db.list_media_downloads(Some(id), status.as_deref(), limit, offset)
            .await
    }
    .map_err(media_app_error)?;
    Ok(Json(
        media_download_responses(state.service.database(), downloads).await?,
    ))
}

async fn get_media_download(
    State(state): State<MediaApiState>,
    Path(id): Path<i64>,
) -> Result<Json<MediaDownloadResponse>, ApiError> {
    validate_positive_id(id, "download id")?;
    let download = state
        .service
        .database()
        .get_media_download(id)
        .await
        .map_err(media_app_error)?
        .ok_or_else(|| ApiError::not_found("media download not found"))?;
    Ok(Json(
        MediaDownloadResponse::from_database(state.service.database(), download).await?,
    ))
}

#[derive(Debug, Serialize)]
struct DeleteMediaDownloadResponse {
    #[serde(flatten)]
    deletion: MediaDownloadDeletion,
    qb_torrent_deleted: bool,
    openlist_data_deleted: bool,
}

async fn delete_media_download(
    State(state): State<MediaApiState>,
    Path(id): Path<i64>,
    Query(query): Query<DeleteMediaDownloadQuery>,
) -> Result<Json<DeleteMediaDownloadResponse>, ApiError> {
    validate_positive_id(id, "download id")?;
    if query.version < 0 {
        return Err(ApiError::bad_request(
            "download version must be greater than or equal to zero",
        ));
    }
    let deletion = state
        .service
        .delete_download_record(id, query.version)
        .await
        .map_err(ApiError::from)?;
    state.scheduler.wake();
    Ok(Json(DeleteMediaDownloadResponse {
        deletion,
        qb_torrent_deleted: false,
        openlist_data_deleted: false,
    }))
}

#[derive(Debug, Serialize)]
struct ReconcileFailedMediaDownloadResponse {
    resolution: String,
    #[serde(flatten)]
    download: MediaDownloadResponse,
}

impl From<FailedDownloadReconciliation> for ReconcileFailedMediaDownloadResponse {
    fn from(value: FailedDownloadReconciliation) -> Self {
        Self {
            resolution: value.resolution,
            download: MediaDownloadResponse::from(value.download),
        }
    }
}

async fn reconcile_failed_media_download(
    State(state): State<MediaApiState>,
    Path(id): Path<i64>,
    Query(query): Query<DeleteMediaDownloadQuery>,
) -> Result<Json<ReconcileFailedMediaDownloadResponse>, ApiError> {
    validate_positive_id(id, "download id")?;
    if query.version < 0 {
        return Err(ApiError::bad_request(
            "download version must be greater than or equal to zero",
        ));
    }
    let result = state
        .service
        .reconcile_failed_download(id, query.version)
        .await
        .map_err(ApiError::from)?;
    state.scheduler.wake();
    Ok(Json(ReconcileFailedMediaDownloadResponse::from(result)))
}

async fn redeliver_media_download(
    State(state): State<MediaApiState>,
    Path(id): Path<i64>,
) -> Result<Json<MediaDownloadResponse>, ApiError> {
    validate_positive_id(id, "download id")?;
    let download = state
        .service
        .redeliver_download(id)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(MediaDownloadResponse::from(download)))
}

#[derive(Debug, Serialize)]
struct MediaDownloadResponse {
    #[serde(flatten)]
    download: MediaDownloadRecord,
    parsed_release: Option<ReleaseInfo>,
    failed_reconciliation_allowed: bool,
}

impl From<MediaDownloadRecord> for MediaDownloadResponse {
    fn from(download: MediaDownloadRecord) -> Self {
        let parsed_release = ReleaseParser::default().parse(&download.title).ok();
        Self {
            download,
            parsed_release,
            failed_reconciliation_allowed: false,
        }
    }
}

impl MediaDownloadResponse {
    async fn from_database(db: &Database, download: MediaDownloadRecord) -> Result<Self, ApiError> {
        let failed_reconciliation_allowed = if download.status == "failed"
            && download.subscription_id.is_some()
            && download.infohash.is_some()
        {
            db.media_download_failed_reconciliation_allowed(download.id, download.version)
                .await
                .map_err(media_app_error)?
        } else {
            false
        };
        let mut response = Self::from(download);
        response.failed_reconciliation_allowed = failed_reconciliation_allowed;
        Ok(response)
    }
}

async fn media_download_responses(
    db: &Database,
    downloads: Vec<MediaDownloadRecord>,
) -> Result<Vec<MediaDownloadResponse>, ApiError> {
    let mut responses = Vec::with_capacity(downloads.len());
    for download in downloads {
        responses.push(MediaDownloadResponse::from_database(db, download).await?);
    }
    Ok(responses)
}

async fn load_media_subscription(
    state: &MediaApiState,
    id: i64,
) -> Result<SubscriptionRecord, ApiError> {
    validate_positive_id(id, "subscription id")?;
    state
        .service
        .database()
        .get_subscription(id)
        .await
        .map_err(media_app_error)?
        .ok_or_else(|| ApiError::not_found("subscription not found"))
}

fn validate_media_settings(payload: &UpdateMediaSettingsRequest) -> Result<(), ApiError> {
    let language = payload.tmdb_language.trim();
    if language.is_empty() || language.len() > 32 {
        return Err(ApiError::bad_request(
            "tmdb_language must contain between 1 and 32 bytes",
        ));
    }
    if payload.scan_interval_mins == 0 || payload.scan_interval_mins > 10_080 {
        return Err(ApiError::bad_request(
            "scan_interval_mins must be between 1 and 10080",
        ));
    }
    if !(2..=32).contains(&payload.max_search_queries) {
        return Err(ApiError::bad_request(
            "max_search_queries must be between 2 and 32",
        ));
    }
    if !(1..=16).contains(&payload.search_concurrency) {
        return Err(ApiError::bad_request(
            "search_concurrency must be between 1 and 16",
        ));
    }
    if payload
        .tmdb_token
        .as_deref()
        .is_some_and(|token| token.trim().len() > 4_096)
    {
        return Err(ApiError::bad_request(
            "tmdb_token must not exceed 4096 bytes",
        ));
    }
    if payload.clear_tmdb_token
        && payload
            .tmdb_token
            .as_deref()
            .is_some_and(|token| !token.trim().is_empty())
    {
        return Err(ApiError::bad_request(
            "tmdb_token and clear_tmdb_token cannot both be set",
        ));
    }
    Ok(())
}

fn normalize_tmdb_search_type(media_type: Option<&str>) -> Result<Option<&str>, ApiError> {
    match media_type
        .unwrap_or("multi")
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "multi" | "" => Ok(None),
        "tv" => Ok(Some("tv")),
        "movie" => Ok(Some("movie")),
        _ => Err(ApiError::bad_request(
            "media_type must be multi, tv, or movie",
        )),
    }
}

fn parse_tmdb_media_type(value: &str) -> Result<TmdbMediaType, ApiError> {
    TmdbMediaType::parse(value).map_err(|_| ApiError::bad_request("media_type must be tv or movie"))
}

fn validate_positive_id(id: i64, field: &str) -> Result<(), ApiError> {
    if id <= 0 {
        Err(ApiError::bad_request(format!("{field} must be positive")))
    } else {
        Ok(())
    }
}

fn normalize_and_validate_quality_profile(
    payload: &mut QualityProfileRequest,
) -> Result<(), ApiError> {
    payload.name = payload.name.trim().to_string();
    if payload.name.is_empty() || payload.name.len() > 100 {
        return Err(ApiError::bad_request(
            "quality profile name must contain between 1 and 100 bytes",
        ));
    }
    if !(0..=100).contains(&payload.minimum_score) {
        return Err(ApiError::bad_request(
            "minimum_score must be between 0 and 100",
        ));
    }
    if payload.min_seeders > 1_000_000 {
        return Err(ApiError::bad_request("min_seeders must not exceed 1000000"));
    }
    for (field, values) in [
        ("resolution_order", &mut payload.resolution_order),
        ("allowed_resolutions", &mut payload.allowed_resolutions),
        ("blocked_resolutions", &mut payload.blocked_resolutions),
        ("source_order", &mut payload.source_order),
        ("allowed_sources", &mut payload.allowed_sources),
        ("codec_order", &mut payload.codec_order),
        ("blocked_codecs", &mut payload.blocked_codecs),
    ] {
        normalize_string_list(field, values)?;
    }
    Ok(())
}

fn normalize_string_list(field: &str, values: &mut Vec<String>) -> Result<(), ApiError> {
    if values.len() > 64 {
        return Err(ApiError::bad_request(format!(
            "{field} must not contain more than 64 values"
        )));
    }
    let mut seen = std::collections::HashSet::new();
    for value in values.iter_mut() {
        *value = value.trim().to_string();
        if value.is_empty() || value.len() > 64 {
            return Err(ApiError::bad_request(format!(
                "{field} values must contain between 1 and 64 bytes"
            )));
        }
        if !seen.insert(value.to_ascii_lowercase()) {
            return Err(ApiError::bad_request(format!(
                "{field} must not contain duplicate values"
            )));
        }
    }
    Ok(())
}

fn normalize_and_validate_new_subscription(
    payload: &mut CreateSubscriptionRequest,
) -> Result<(), ApiError> {
    validate_positive_id(payload.tmdb_id, "tmdb_id")?;
    validate_positive_id(payload.quality_profile_id, "quality_profile_id")?;
    validate_positive_id(payload.downloader_id, "downloader_id")?;
    payload.media_type = payload.media_type.trim().to_ascii_lowercase();
    if !matches!(payload.media_type.as_str(), "tv" | "movie") {
        return Err(ApiError::bad_request("media_type must be tv or movie"));
    }
    validate_site_ids(&payload.site_ids)?;
    normalize_save_path(&mut payload.save_path)?;
    if payload.start_episode == Some(0) || payload.absolute_episode == Some(0) {
        return Err(ApiError::bad_request(
            "episode numbers must be greater than zero",
        ));
    }
    if payload.media_type == "tv" && payload.season.is_none() {
        return Err(ApiError::bad_request(
            "season is required for a TV subscription",
        ));
    }
    if payload.media_type == "movie"
        && (payload.season.is_some()
            || payload.start_episode.is_some()
            || payload.absolute_episode.is_some())
    {
        return Err(ApiError::bad_request(
            "movie subscriptions cannot contain episode fields",
        ));
    }
    Ok(())
}

fn normalize_and_validate_subscription_update(
    current: &SubscriptionRecord,
    payload: &mut UpdateSubscription,
) -> Result<(), ApiError> {
    validate_positive_id(payload.quality_profile_id, "quality_profile_id")?;
    validate_positive_id(payload.downloader_id, "downloader_id")?;
    validate_site_ids(&payload.site_ids)?;
    normalize_save_path(&mut payload.save_path)?;
    if payload.next_episode == Some(0) || payload.absolute_episode == Some(0) {
        return Err(ApiError::bad_request(
            "episode numbers must be greater than zero",
        ));
    }
    if current.media_type == "movie" {
        if payload.season.is_some()
            || payload.next_episode.is_some()
            || payload.absolute_episode.is_some()
        {
            return Err(ApiError::bad_request(
                "movie subscriptions cannot contain episode fields",
            ));
        }
    } else {
        if payload.season.is_none() {
            return Err(ApiError::bad_request(
                "season is required for a TV subscription",
            ));
        }
        if payload.next_episode.is_none() && payload.absolute_episode.is_some() {
            payload.next_episode = current.next_episode.or(current.start_episode);
        }
        if payload.next_episode.is_none() {
            return Err(ApiError::bad_request(
                "next_episode is required for a TV subscription",
            ));
        }
    }
    Ok(())
}

fn validate_site_ids(site_ids: &[i64]) -> Result<(), ApiError> {
    if site_ids.is_empty() {
        return Err(ApiError::bad_request("at least one site is required"));
    }
    if site_ids.len() > 100 {
        return Err(ApiError::bad_request(
            "no more than 100 sites may be selected",
        ));
    }
    let mut unique = std::collections::HashSet::new();
    for site_id in site_ids {
        validate_positive_id(*site_id, "site_id")?;
        if !unique.insert(*site_id) {
            return Err(ApiError::bad_request(
                "site_ids must not contain duplicates",
            ));
        }
    }
    Ok(())
}

fn normalize_save_path(save_path: &mut Option<String>) -> Result<(), ApiError> {
    let normalized = save_path
        .take()
        .map(|path| path.trim().to_string())
        .filter(|path| !path.is_empty());
    if normalized.as_deref().is_some_and(|path| path.len() > 1_024) {
        return Err(ApiError::bad_request(
            "save_path must not exceed 1024 bytes",
        ));
    }
    *save_path = normalized;
    Ok(())
}

async fn ensure_media_references(
    state: &MediaApiState,
    quality_profile_id: i64,
    downloader_id: i64,
    site_ids: &[i64],
) -> Result<(), ApiError> {
    let db = state.service.database();
    if db
        .get_quality_profile(quality_profile_id)
        .await
        .map_err(media_app_error)?
        .is_none()
    {
        return Err(ApiError::not_found("quality profile not found"));
    }
    if db
        .get_downloader(downloader_id)
        .await
        .map_err(media_app_error)?
        .is_none()
    {
        return Err(ApiError::not_found("downloader not found"));
    }
    let configured: std::collections::HashSet<_> = db
        .list_sites()
        .await
        .map_err(media_app_error)?
        .into_iter()
        .map(|site| site.id)
        .collect();
    if site_ids.iter().any(|site_id| !configured.contains(site_id)) {
        return Err(ApiError::not_found(
            "one or more selected PT sites do not exist",
        ));
    }
    Ok(())
}

fn subscription_is_leased(subscription: &SubscriptionRecord) -> bool {
    subscription.lease_owner.is_some()
        && subscription
            .lease_until
            .as_deref()
            .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
            .is_some_and(|until| until > Utc::now())
}

fn validate_resource_search(payload: &ResourceSearchRequest) -> Result<(), ApiError> {
    if payload
        .query
        .as_deref()
        .is_none_or(|query| query.trim().is_empty())
        && payload.target.is_none()
    {
        return Err(ApiError::bad_request("query or target is required"));
    }
    if payload
        .query
        .as_deref()
        .is_some_and(|query| query.trim().len() > 512)
    {
        return Err(ApiError::bad_request("query must not exceed 512 bytes"));
    }
    if payload
        .page_size
        .is_some_and(|size| !(1..=100).contains(&size))
    {
        return Err(ApiError::bad_request("page_size must be between 1 and 100"));
    }
    if let Some(profile_id) = payload.quality_profile_id {
        validate_positive_id(profile_id, "quality_profile_id")?;
    }
    if !payload.site_ids.is_empty() {
        validate_site_ids(&payload.site_ids)?;
    }
    if let Some(target) = &payload.target {
        validate_media_target(target)?;
    }
    Ok(())
}

fn validate_media_target(target: &MediaTarget) -> Result<(), ApiError> {
    validate_positive_id(target.tmdb_id(), "target.tmdb_id")?;
    if target.titles().is_empty() || target.titles().len() > 32 {
        return Err(ApiError::bad_request(
            "target.titles must contain between 1 and 32 values",
        ));
    }
    if target
        .titles()
        .iter()
        .any(|title| title.trim().is_empty() || title.len() > 512)
    {
        return Err(ApiError::bad_request(
            "target titles must contain between 1 and 512 bytes",
        ));
    }
    match target {
        MediaTarget::Episode { episode: 0, .. }
        | MediaTarget::Anime {
            absolute_episode: 0,
            ..
        } => Err(ApiError::bad_request(
            "target episode numbers must be greater than zero",
        )),
        _ => Ok(()),
    }
}

async fn normalize_and_validate_download(
    _state: &MediaApiState,
    payload: &mut QueueDownloadRequest,
) -> Result<(), ApiError> {
    validate_positive_id(payload.quality_profile_id, "quality_profile_id")?;
    validate_positive_id(payload.downloader_id, "downloader_id")?;
    payload.candidate_id = payload.candidate_id.trim().to_string();
    let token = payload
        .candidate_id
        .strip_prefix("cand_")
        .ok_or_else(|| ApiError::bad_request("candidate_id is invalid"))?;
    if token.len() != 48 || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ApiError::bad_request("candidate_id is invalid"));
    }

    if let Some(reason) = payload.override_reason.take() {
        let reason = reason.trim().to_string();
        if reason.len() > 1_000 {
            return Err(ApiError::bad_request(
                "override_reason must not exceed 1000 bytes",
            ));
        }
        payload.override_reason = (!reason.is_empty()).then_some(reason);
    }

    Ok(())
}

fn reject_completed_subscription(
    subscription: &SubscriptionRecord,
    action: &str,
) -> Result<(), ApiError> {
    if subscription.last_status.as_deref() == Some("completed") {
        Err(ApiError::conflict(format!(
            "completed subscription cannot {action}; edit it with a new cursor first"
        )))
    } else {
        Ok(())
    }
}

fn validate_download_query(
    status: Option<&str>,
    page: Option<usize>,
    page_size: Option<usize>,
) -> Result<(Option<String>, usize, usize), ApiError> {
    let page = page.unwrap_or(1);
    let page_size = page_size.unwrap_or(100);
    if page == 0 {
        return Err(ApiError::bad_request("page must be greater than zero"));
    }
    if !(1..=200).contains(&page_size) {
        return Err(ApiError::bad_request("page_size must be between 1 and 200"));
    }
    let offset = page
        .checked_sub(1)
        .and_then(|value| value.checked_mul(page_size))
        .ok_or_else(|| ApiError::bad_request("pagination values are too large"))?;
    i64::try_from(offset).map_err(|_| ApiError::bad_request("pagination values are too large"))?;
    let status = status
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase);
    if status.as_deref().is_some_and(|value| {
        !matches!(
            value,
            "queued"
                | "fetching"
                | "submitting"
                | "reconciling"
                | "retry_wait"
                | "submitted"
                | "failed"
                | "cancelled"
        )
    }) {
        return Err(ApiError::bad_request("invalid download status"));
    }
    Ok((status, page_size, offset))
}

fn validate_download_cursor(
    before_id: Option<i64>,
    page: Option<usize>,
) -> Result<Option<i64>, ApiError> {
    if before_id.is_some() && page.is_some() {
        return Err(ApiError::bad_request(
            "before_id cannot be combined with page",
        ));
    }
    if before_id.is_some_and(|value| value <= 0) {
        return Err(ApiError::bad_request("before_id must be greater than zero"));
    }
    Ok(before_id)
}

async fn get_features(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "self_use": state.self_use }))
}

async fn get_settings(State(state): State<AppState>) -> Result<Json<GlobalConfig>, ApiError> {
    Ok(Json(state.db.get_settings().await?))
}

async fn update_settings(
    State(state): State<AppState>,
    Json(mut settings): Json<GlobalConfig>,
) -> Result<Json<GlobalConfig>, ApiError> {
    normalize_sign_in_settings(&mut settings);
    validate_settings(&settings)?;
    state.db.update_settings(&settings).await?;
    let saved = state.db.get_settings().await?;
    crate::logging::update_log_filter(saved.log_level.as_deref())?;
    Ok(Json(saved))
}

async fn index() -> impl IntoResponse {
    serve_asset("index.html")
}

async fn static_asset(Path(path): Path<String>) -> impl IntoResponse {
    if path.starts_with("api/") {
        return StatusCode::NOT_FOUND.into_response();
    }
    serve_asset(&path)
}

fn serve_asset(path: &str) -> Response {
    let requested = if path.is_empty() { "index.html" } else { path };
    let asset = FrontendAssets::get(requested).or_else(|| FrontendAssets::get("index.html"));
    let Some(asset) = asset else {
        return Html("<h1>Frontend not built</h1><p>Run the frontend build first.</p>")
            .into_response();
    };
    let mime = mime_guess::from_path(requested).first_or_octet_stream();
    let mut response = asset.data.into_owned().into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(mime.as_ref())
            .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
    );
    response
}

// ========== Sites API ==========

#[derive(Debug, Deserialize)]
struct CreateSiteRequest {
    name: String,
    site_type: String,
    base_url: String,
    auth_config: serde_json::Value,
    #[serde(default = "default_site_request_headers")]
    request_headers: Vec<SiteRequestHeader>,
    #[serde(default = "default_true")]
    use_proxy: bool,
}

#[derive(Debug, Deserialize)]
struct UpdateSiteRequest {
    name: String,
    site_type: String,
    base_url: String,
    auth_config: Option<serde_json::Value>,
    request_headers: Option<Vec<SiteRequestHeader>>,
    #[serde(default)]
    clear_auth_config: bool,
    #[serde(default = "default_true")]
    use_proxy: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SiteAuthInput {
    auth_type: String,
    cookie: Option<String>,
    passkey: Option<String>,
    api_key: Option<String>,
}

#[derive(Debug, Serialize)]
struct SiteResponse {
    id: i64,
    name: String,
    site_type: String,
    base_url: String,
    auth_type: Option<&'static str>,
    auth_configured: bool,
    use_proxy: bool,
    created_at: String,
    updated_at: String,
    stats: Option<SiteStatsRecord>,
}

#[derive(Debug, Serialize)]
struct SiteCredentialsResponse {
    auth_type: &'static str,
    cookie: Option<String>,
    passkey: Option<String>,
    api_key: Option<String>,
}

#[derive(Debug, Serialize)]
struct SiteStatsRefreshStartResponse {
    started: bool,
    refreshing: bool,
}

#[derive(Debug, Serialize)]
struct SiteStatsRefreshStatusResponse {
    refreshing: bool,
}

#[derive(Debug, Serialize)]
struct PtdBackupConfigResponse {
    enabled: bool,
    webdav_url: String,
    username: String,
    password_configured: bool,
    use_proxy: bool,
    backup_interval_hours: u64,
    site_identifiers: BTreeMap<i64, Option<String>>,
    configured: bool,
    last_backup_at: Option<String>,
    last_backup_filename: Option<String>,
    last_error: Option<String>,
    updated_at: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdatePtdBackupConfigRequest {
    #[serde(default)]
    enabled: bool,
    webdav_url: String,
    #[serde(default)]
    username: String,
    password: Option<String>,
    #[serde(default)]
    clear_password: bool,
    #[serde(default)]
    use_proxy: bool,
    backup_interval_hours: u64,
}

#[derive(Debug, Serialize)]
struct PtdBackupTestResponse {
    success: bool,
    message: String,
}

fn default_true() -> bool {
    true
}

impl From<SiteWithStats> for SiteResponse {
    fn from(site: SiteWithStats) -> Self {
        let auth = serde_json::from_str::<SiteAuth>(&site.auth_config).ok();
        Self {
            id: site.id,
            name: site.name,
            site_type: site.site_type,
            base_url: site.base_url,
            auth_type: auth.as_ref().map(site_auth_type),
            auth_configured: auth.as_ref().is_some_and(site_auth_is_configured),
            use_proxy: site.use_proxy,
            created_at: site.created_at,
            updated_at: site.updated_at,
            stats: site.stats,
        }
    }
}

impl PtdBackupConfigResponse {
    fn new(config: crate::db::PtdBackupConfig, sites: &[SiteWithStats]) -> Self {
        let site_identifiers = sites
            .iter()
            .map(|site| (site.id, ptd_site_id(site).map(str::to_string)))
            .collect();
        Self {
            enabled: config.enabled,
            configured: !config.webdav_url.trim().is_empty(),
            webdav_url: config.webdav_url,
            username: config.username,
            password_configured: !config.password.is_empty(),
            use_proxy: config.use_proxy,
            backup_interval_hours: config.backup_interval_hours,
            site_identifiers,
            last_backup_at: config.last_backup_at,
            last_backup_filename: config.last_backup_filename,
            last_error: config.last_error,
            updated_at: config.updated_at,
        }
    }
}

impl From<SiteAuth> for SiteCredentialsResponse {
    fn from(auth: SiteAuth) -> Self {
        let auth_type = site_auth_type(&auth);
        match auth {
            SiteAuth::Cookie { cookie } => Self {
                auth_type,
                cookie: Some(cookie),
                passkey: None,
                api_key: None,
            },
            SiteAuth::Passkey { passkey } => Self {
                auth_type,
                cookie: None,
                passkey: Some(passkey),
                api_key: None,
            },
            SiteAuth::CookiePasskey { cookie, passkey } => Self {
                auth_type,
                cookie: Some(cookie),
                passkey: Some(passkey),
                api_key: None,
            },
            SiteAuth::ApiKey { api_key } => Self {
                auth_type,
                cookie: None,
                passkey: None,
                api_key: Some(api_key),
            },
        }
    }
}

fn site_auth_type(auth: &SiteAuth) -> &'static str {
    match auth {
        SiteAuth::Cookie { .. } => "cookie",
        SiteAuth::Passkey { .. } => "passkey",
        SiteAuth::CookiePasskey { .. } => "cookie_passkey",
        SiteAuth::ApiKey { .. } => "api_key",
    }
}

fn site_auth_is_configured(auth: &SiteAuth) -> bool {
    match auth {
        SiteAuth::Cookie { cookie } => !cookie.trim().is_empty(),
        SiteAuth::Passkey { passkey } => !passkey.trim().is_empty(),
        SiteAuth::CookiePasskey { cookie, passkey } => {
            !cookie.trim().is_empty() && !passkey.trim().is_empty()
        }
        SiteAuth::ApiKey { api_key } => !api_key.trim().is_empty(),
    }
}

fn site_auth_has_secret(auth: &SiteAuth) -> bool {
    match auth {
        SiteAuth::Cookie { cookie } => !cookie.is_empty(),
        SiteAuth::Passkey { passkey } => !passkey.is_empty(),
        SiteAuth::CookiePasskey { cookie, passkey } => !cookie.is_empty() || !passkey.is_empty(),
        SiteAuth::ApiKey { api_key } => !api_key.is_empty(),
    }
}

fn parse_site_auth_input(value: serde_json::Value) -> Result<SiteAuth, ApiError> {
    let input: SiteAuthInput =
        serde_json::from_value(value).map_err(|_| ApiError::bad_request("认证配置格式无效"))?;
    match input.auth_type.as_str() {
        "cookie" => Ok(SiteAuth::Cookie {
            cookie: input.cookie.unwrap_or_default(),
        }),
        "passkey" => Ok(SiteAuth::Passkey {
            passkey: input.passkey.unwrap_or_default(),
        }),
        "cookie_passkey" => Ok(SiteAuth::CookiePasskey {
            cookie: input.cookie.unwrap_or_default(),
            passkey: input.passkey.unwrap_or_default(),
        }),
        "api_key" => Ok(SiteAuth::ApiKey {
            api_key: input.api_key.unwrap_or_default(),
        }),
        _ => Err(ApiError::bad_request("不支持的认证类型")),
    }
}

fn serialize_site_request_headers(headers: Vec<SiteRequestHeader>) -> Result<String, ApiError> {
    let headers = normalize_site_request_headers(headers).map_err(ApiError::bad_request)?;
    serde_json::to_string(&headers)
        .map_err(|error| ApiError::bad_request(format!("请求头配置序列化失败: {error}")))
}

fn parse_site_type(value: &str) -> Result<SiteType, ApiError> {
    SiteType::from_str(value.trim()).ok_or_else(|| ApiError::bad_request("不支持的站点类型"))
}

fn validate_site_auth_type(site_type: SiteType, auth: &SiteAuth) -> Result<(), ApiError> {
    if site_type == SiteType::MTeam && !matches!(auth, SiteAuth::ApiKey { .. }) {
        return Err(ApiError::bad_request("M-Team 站点必须使用 API Key 认证"));
    }
    if site_type == SiteType::Gazelle
        && !matches!(
            auth,
            SiteAuth::Cookie { .. } | SiteAuth::CookiePasskey { .. }
        )
    {
        return Err(ApiError::bad_request("Gazelle 账户统计需要 Cookie 认证"));
    }
    Ok(())
}

fn require_configured_site_auth(auth: &SiteAuth) -> Result<(), ApiError> {
    if site_auth_is_configured(auth) {
        Ok(())
    } else {
        Err(ApiError::bad_request("认证凭据不能为空"))
    }
}

fn empty_site_auth(auth: &SiteAuth) -> SiteAuth {
    match auth {
        SiteAuth::Cookie { .. } => SiteAuth::Cookie {
            cookie: String::new(),
        },
        SiteAuth::Passkey { .. } => SiteAuth::Passkey {
            passkey: String::new(),
        },
        SiteAuth::CookiePasskey { .. } => SiteAuth::CookiePasskey {
            cookie: String::new(),
            passkey: String::new(),
        },
        SiteAuth::ApiKey { .. } => SiteAuth::ApiKey {
            api_key: String::new(),
        },
    }
}

fn merge_site_auth(existing: SiteAuth, incoming: SiteAuth) -> Result<SiteAuth, ApiError> {
    match (existing, incoming) {
        (SiteAuth::Cookie { cookie }, SiteAuth::Cookie { cookie: next }) => Ok(SiteAuth::Cookie {
            cookie: if next.is_empty() { cookie } else { next },
        }),
        (SiteAuth::Passkey { passkey }, SiteAuth::Passkey { passkey: next }) => {
            Ok(SiteAuth::Passkey {
                passkey: if next.is_empty() { passkey } else { next },
            })
        }
        (
            SiteAuth::CookiePasskey { cookie, passkey },
            SiteAuth::CookiePasskey {
                cookie: next_cookie,
                passkey: next_passkey,
            },
        ) => Ok(SiteAuth::CookiePasskey {
            cookie: if next_cookie.is_empty() {
                cookie
            } else {
                next_cookie
            },
            passkey: if next_passkey.is_empty() {
                passkey
            } else {
                next_passkey
            },
        }),
        (SiteAuth::ApiKey { api_key }, SiteAuth::ApiKey { api_key: next }) => {
            Ok(SiteAuth::ApiKey {
                api_key: if next.is_empty() { api_key } else { next },
            })
        }
        _ => Err(ApiError::bad_request("切换认证类型时必须提交完整的新凭据")),
    }
}

fn resolve_site_auth_update(
    existing: &crate::site::SiteRecord,
    site_type: &str,
    incoming: Option<serde_json::Value>,
    clear_auth_config: bool,
) -> Result<SiteAuth, ApiError> {
    let old_site_type = SiteType::from_str(existing.site_type.trim())
        .ok_or_else(|| ApiError::internal("现有站点类型无效"))?;
    let new_site_type = parse_site_type(site_type)?;
    let existing_auth: SiteAuth = serde_json::from_str(&existing.auth_config)
        .map_err(|_| ApiError::internal("现有认证配置无效"))?;
    let incoming = incoming.map(parse_site_auth_input).transpose()?;

    if let Some(auth) = incoming.as_ref() {
        validate_site_auth_type(new_site_type, auth)?;
    }

    let site_type_changed = old_site_type != new_site_type;
    let auth_type_changed = incoming
        .as_ref()
        .is_some_and(|auth| site_auth_type(auth) != site_auth_type(&existing_auth));

    if site_type_changed || auth_type_changed {
        if clear_auth_config {
            return Err(ApiError::bad_request(
                "切换站点或认证类型时不能同时清除凭据",
            ));
        }
        let auth = incoming
            .ok_or_else(|| ApiError::bad_request("切换站点或认证类型时必须提交完整的新凭据"))?;
        require_configured_site_auth(&auth)?;
        return Ok(auth);
    }

    validate_site_auth_type(new_site_type, &existing_auth)?;
    if clear_auth_config {
        if incoming.as_ref().is_some_and(site_auth_has_secret) {
            return Err(ApiError::bad_request(
                "认证凭据和 clear_auth_config 不能同时提交",
            ));
        }
        return Ok(empty_site_auth(&existing_auth));
    }

    match incoming {
        Some(auth) => merge_site_auth(existing_auth, auth),
        None => Ok(existing_auth),
    }
}

fn resolve_ptd_backup_config_update(
    current: crate::db::PtdBackupConfig,
    body: UpdatePtdBackupConfigRequest,
) -> Result<crate::db::PtdBackupConfig, ApiError> {
    if body.clear_password
        && body
            .password
            .as_deref()
            .is_some_and(|password| !password.is_empty())
    {
        return Err(ApiError::bad_request(
            "WebDAV 密码和 clear_password 不能同时提交",
        ));
    }
    let password = if body.clear_password {
        String::new()
    } else {
        body.password
            .filter(|password| !password.is_empty())
            .unwrap_or(current.password)
    };
    Ok(crate::db::PtdBackupConfig {
        enabled: body.enabled,
        webdav_url: body.webdav_url,
        username: body.username,
        password,
        use_proxy: body.use_proxy,
        backup_interval_hours: body.backup_interval_hours,
        last_backup_at: current.last_backup_at,
        last_backup_filename: current.last_backup_filename,
        last_error: current.last_error,
        updated_at: current.updated_at,
    })
}

async fn get_ptd_backup_config(
    State(state): State<AppState>,
) -> Result<Json<PtdBackupConfigResponse>, ApiError> {
    let config = state.db.get_ptd_backup_config().await?;
    let sites = state.db.list_sites_with_stats().await?;
    Ok(Json(PtdBackupConfigResponse::new(config, &sites)))
}

async fn update_ptd_backup_config(
    State(state): State<AppState>,
    Json(body): Json<UpdatePtdBackupConfigRequest>,
) -> Result<Json<PtdBackupConfigResponse>, ApiError> {
    let current = state.db.get_ptd_backup_config().await?;
    let mut config = resolve_ptd_backup_config_update(current, body)?;
    let sites = state.db.list_sites_with_stats().await?;
    crate::ptd_backup::validate_config(&mut config).map_err(ApiError::bad_request)?;
    state.db.update_ptd_backup_config(&config).await?;
    let saved = state.db.get_ptd_backup_config().await?;
    Ok(Json(PtdBackupConfigResponse::new(saved, &sites)))
}

async fn test_ptd_backup(
    State(state): State<AppState>,
    Json(body): Json<UpdatePtdBackupConfigRequest>,
) -> Result<Json<PtdBackupTestResponse>, ApiError> {
    let current = state.db.get_ptd_backup_config().await?;
    let mut config = resolve_ptd_backup_config_update(current, body)?;
    if let Err(message) = crate::ptd_backup::validate_config(&mut config) {
        return Ok(Json(PtdBackupTestResponse {
            success: false,
            message,
        }));
    }
    if config.webdav_url.is_empty() {
        return Ok(Json(PtdBackupTestResponse {
            success: false,
            message: "请填写 WebDAV 地址".to_string(),
        }));
    }
    let settings = state.db.get_settings().await?;
    let proxy = config
        .use_proxy
        .then_some(settings.proxy.as_deref())
        .flatten();
    match crate::ptd_backup::test_webdav(&config, proxy).await {
        Ok(()) => Ok(Json(PtdBackupTestResponse {
            success: true,
            message: "WebDAV 认证成功，目标目录可以访问".to_string(),
        })),
        Err(message) => Ok(Json(PtdBackupTestResponse {
            success: false,
            message,
        })),
    }
}

async fn run_ptd_backup(
    State(state): State<AppState>,
) -> Result<Json<PtdBackupRunResult>, ApiError> {
    crate::ptd_backup::backup_now(&state.db)
        .await
        .map(Json)
        .map_err(ApiError::bad_gateway)
}

async fn list_sites(State(state): State<AppState>) -> Result<Json<Vec<SiteResponse>>, ApiError> {
    Ok(Json(
        state
            .db
            .list_sites_with_stats()
            .await?
            .into_iter()
            .map(SiteResponse::from)
            .collect(),
    ))
}

async fn list_site_presets() -> Json<&'static [crate::ptd_site_catalog::PtdSitePreset]> {
    Json(crate::ptd_site_catalog::SITE_PRESETS)
}

async fn get_site_credentials(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Response, ApiError> {
    let site = state
        .db
        .get_site(id)
        .await?
        .ok_or_else(|| ApiError::not_found("站点不存在"))?;
    let auth = serde_json::from_str::<SiteAuth>(&site.auth_config)
        .map_err(|_| ApiError::internal("现有认证配置无效"))?;

    Ok((
        [(header::CACHE_CONTROL, HeaderValue::from_static("no-store"))],
        Json(SiteCredentialsResponse::from(auth)),
    )
        .into_response())
}

async fn get_site_request_headers(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Response, ApiError> {
    let site = state
        .db
        .get_site(id)
        .await?
        .ok_or_else(|| ApiError::not_found("站点不存在"))?;
    let request_headers = parse_site_request_headers(&site.request_headers)
        .map_err(|_| ApiError::internal("现有请求头配置无效"))?;

    Ok((
        [(header::CACHE_CONTROL, HeaderValue::from_static("no-store"))],
        Json(request_headers),
    )
        .into_response())
}

async fn create_site(
    State(state): State<AppState>,
    Json(body): Json<CreateSiteRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if body.name.is_empty() || body.site_type.is_empty() || body.base_url.is_empty() {
        return Err(ApiError::bad_request("名称、站点类型和基础URL不能为空"));
    }
    let site_type = parse_site_type(&body.site_type)?;
    let auth = parse_site_auth_input(body.auth_config)?;
    validate_site_auth_type(site_type, &auth)?;
    require_configured_site_auth(&auth)?;
    let auth_str = serde_json::to_string(&auth)
        .map_err(|e| ApiError::bad_request(format!("认证配置序列化失败: {}", e)))?;
    let request_headers = serialize_site_request_headers(body.request_headers)?;
    let id = state
        .db
        .create_site(
            &body.name,
            &body.site_type,
            &body.base_url,
            &auth_str,
            &request_headers,
            body.use_proxy,
        )
        .await?;
    Ok(Json(serde_json::json!({ "id": id })))
}

async fn update_site(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<UpdateSiteRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if body.name.is_empty() || body.site_type.is_empty() || body.base_url.is_empty() {
        return Err(ApiError::bad_request("名称、站点类型和基础URL不能为空"));
    }
    let existing = state
        .db
        .get_site(id)
        .await?
        .ok_or_else(|| ApiError::not_found("站点不存在"))?;
    let auth = resolve_site_auth_update(
        &existing,
        &body.site_type,
        body.auth_config,
        body.clear_auth_config,
    )?;
    let auth_str = serde_json::to_string(&auth)
        .map_err(|e| ApiError::bad_request(format!("认证配置序列化失败: {}", e)))?;
    let request_headers = match body.request_headers {
        Some(headers) => serialize_site_request_headers(headers)?,
        None => existing.request_headers,
    };
    state
        .db
        .update_site(
            id,
            &body.name,
            &body.site_type,
            &body.base_url,
            &auth_str,
            &request_headers,
            body.use_proxy,
        )
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn delete_site(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state.db.delete_site(id).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn test_site(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<crate::site::SiteTestResult>, ApiError> {
    let site_with_stats = state
        .db
        .get_site_with_stats(id)
        .await?
        .ok_or_else(|| ApiError::not_found("站点不存在"))?;
    let cached_user_id = site_with_stats.reusable_user_id();
    let site = site_with_stats.site_record();
    let settings = state.db.get_settings().await?;
    let client = client_factory::resolve_site_client(settings.proxy.as_deref(), site.use_proxy)
        .map_err(|e| ApiError::internal(format!("创建 HTTP 客户端失败: {}", e)))?;
    let adapter = site_factory::create_adapter_with_cached_user_id(&site, client, cached_user_id)
        .map_err(ApiError::bad_request)?;
    let result = adapter
        .test_connection()
        .await
        .map_err(|e| ApiError::internal(e))?;
    Ok(Json(result))
}

async fn get_site_stats(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<crate::site::UserStats>, ApiError> {
    let site_with_stats = state
        .db
        .get_site_with_stats(id)
        .await?
        .ok_or_else(|| ApiError::not_found("站点不存在"))?;
    let cached_user_id = site_with_stats.reusable_user_id();
    let site = site_with_stats.site_record();
    let settings = state.db.get_settings().await?;
    let client = client_factory::resolve_site_client(settings.proxy.as_deref(), site.use_proxy)
        .map_err(|e| ApiError::internal(format!("创建 HTTP 客户端失败: {}", e)))?;
    let adapter = site_factory::create_adapter_with_cached_user_id(&site, client, cached_user_id)
        .map_err(ApiError::bad_request)?;
    let stats = adapter
        .get_user_stats()
        .await
        .map_err(|e| ApiError::internal(e))?;
    Ok(Json(stats))
}

async fn get_sites_stats_overview(
    State(state): State<AppState>,
) -> Result<Json<Vec<SiteResponse>>, ApiError> {
    Ok(Json(
        state
            .db
            .list_sites_with_stats()
            .await?
            .into_iter()
            .map(SiteResponse::from)
            .collect(),
    ))
}

async fn start_sites_stats_refresh(
    State(state): State<AppState>,
) -> (StatusCode, Json<SiteStatsRefreshStartResponse>) {
    let started = state.site_stats_refresher.refresh_all_in_background();
    let refreshing = state.site_stats_refresher.is_refreshing();
    (
        StatusCode::ACCEPTED,
        Json(SiteStatsRefreshStartResponse {
            started,
            refreshing,
        }),
    )
}

async fn get_sites_stats_refresh_status(State(state): State<AppState>) -> Response {
    (
        [(header::CACHE_CONTROL, HeaderValue::from_static("no-store"))],
        Json(SiteStatsRefreshStatusResponse {
            refreshing: state.site_stats_refresher.is_refreshing(),
        }),
    )
        .into_response()
}

// ========== Proxy Test API ==========

#[derive(Debug, Deserialize)]
struct ProxyTestRequest {
    proxy: String,
    test_url: String,
}

async fn test_proxy(
    Json(body): Json<ProxyTestRequest>,
) -> Result<Json<client_factory::ProxyTestResult>, ApiError> {
    if body.proxy.trim().is_empty() {
        return Err(ApiError::bad_request("代理地址不能为空"));
    }
    if body.test_url.trim().is_empty() {
        return Err(ApiError::bad_request("测试URL不能为空"));
    }
    let result = client_factory::test_proxy(&body.proxy, &body.test_url).await;
    Ok(Json(result))
}

// ========== Sign-in API ==========

#[derive(Debug, Deserialize)]
struct SignInRecordsQuery {
    task_id: Option<i64>,
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct SignInBrowserProbeRequest {
    browser: String,
}

async fn list_sign_in_profiles(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let sites = state.db.list_sites().await?;
    let profiles: Vec<_> = sites
        .iter()
        .filter_map(|site| {
            crate::sign_in::signers::known_profile(&site.base_url)
                .map(|profile| serde_json::json!({ "site_id": site.id, "profile": profile }))
        })
        .collect();
    Ok(Json(serde_json::json!(profiles)))
}

async fn list_sign_in_tasks(
    State(state): State<AppState>,
) -> Result<Json<Vec<crate::sign_in::SignInTaskRecord>>, ApiError> {
    Ok(Json(state.db.list_sign_in_tasks().await?))
}

async fn create_sign_in_task(
    State(state): State<AppState>,
    Json(mut body): Json<crate::sign_in::SignInTaskRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    validate_sign_in_task(&state, &mut body).await?;
    let id = state.db.create_sign_in_task(&body).await?;
    Ok(Json(serde_json::json!({ "id": id })))
}

async fn update_sign_in_task(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(mut body): Json<crate::sign_in::SignInTaskRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    validate_sign_in_task(&state, &mut body).await?;
    state.db.update_sign_in_task(id, &body).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn delete_sign_in_task(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state.sign_in_scheduler.stop_task(id).await;
    state.db.delete_sign_in_task(id).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn start_sign_in_task(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state.db.set_sign_in_task_enabled(id, true).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn stop_sign_in_task(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state.db.set_sign_in_task_enabled(id, false).await?;
    state.sign_in_scheduler.stop_task(id).await;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn run_sign_in_task_once(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state
        .sign_in_scheduler
        .trigger_task(id)
        .await
        .map_err(map_sign_in_trigger_error)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn probe_sign_in_task_1_1_1_1(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<crate::sign_in::BrowserProbeResult>, ApiError> {
    let task = state
        .db
        .get_sign_in_task(id)
        .await?
        .ok_or_else(|| ApiError::not_found("签到任务不存在"))?;
    let site = state
        .db
        .get_site(task.site_id)
        .await?
        .ok_or_else(|| ApiError::not_found("所选站点不存在"))?;
    let browser = crate::sign_in::signers::known_profile(&site.base_url)
        .map(|profile| profile.browser.to_string())
        .unwrap_or(task.browser);
    let settings = state.db.get_settings().await?;
    validate_sign_in_browser_config(&browser, &settings)?;
    let result = crate::sign_in::probe_browser_1_1_1_1(browser, settings)
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(result))
}

async fn probe_sign_in_form_1_1_1_1(
    State(state): State<AppState>,
    Json(body): Json<SignInBrowserProbeRequest>,
) -> Result<Json<crate::sign_in::BrowserProbeResult>, ApiError> {
    let browser = crate::sign_in::normalize_sign_in_browser(&body.browser)
        .ok_or_else(|| ApiError::bad_request("browser 必须是 lightpanda 或 browserless"))?
        .to_string();
    let settings = state.db.get_settings().await?;
    validate_sign_in_browser_config(&browser, &settings)?;
    let result = crate::sign_in::probe_browser_1_1_1_1(browser, settings)
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(result))
}

async fn list_sign_in_records(
    State(state): State<AppState>,
    Query(query): Query<SignInRecordsQuery>,
) -> Result<Json<Vec<crate::sign_in::SignInRecord>>, ApiError> {
    Ok(Json(
        state
            .db
            .list_sign_in_records(query.task_id, query.limit.unwrap_or(100))
            .await?,
    ))
}

async fn validate_sign_in_task(
    state: &AppState,
    body: &mut crate::sign_in::SignInTaskRequest,
) -> Result<(), ApiError> {
    let site = state
        .db
        .get_site(body.site_id)
        .await?
        .ok_or_else(|| ApiError::bad_request("所选站点不存在"))?;
    crate::sign_in::signers::normalize_request(&site.base_url, body);
    body.name = body.name.trim().to_string();
    body.cron_expression = normalize_cron(&body.cron_expression);
    let browser = crate::sign_in::normalize_sign_in_browser(
        body.browser
            .as_deref()
            .unwrap_or(crate::sign_in::SIGN_IN_BROWSER_LIGHTPANDA),
    )
    .ok_or_else(|| ApiError::bad_request("browser 必须是 lightpanda 或 browserless"))?;
    body.browser = Some(browser.to_string());
    body.sign_in_method = Some(crate::sign_in::normalize_sign_in_method(
        body.sign_in_method
            .as_deref()
            .unwrap_or(crate::sign_in::SIGN_IN_METHOD_OPEN_PAGE),
    ));
    let mut browserless = body.browserless.take().unwrap_or_default();
    browserless.selector = browserless.selector.trim().to_string();
    if browserless.selector.is_empty() {
        return Err(ApiError::bad_request("Browserless selector 不能为空"));
    }
    if browserless.selector.chars().count() > 2_048 {
        return Err(ApiError::bad_request(
            "Browserless selector 不能超过 2048 个字符",
        ));
    }
    if !matches!(
        browserless.submit_method.as_str(),
        "click" | "form" | "ajax"
    ) {
        return Err(ApiError::bad_request("提交方式必须是 click、form 或 ajax"));
    }
    if browserless.result_rules.len() > 30 {
        return Err(ApiError::bad_request("结果识别规则最多 30 条"));
    }
    for rule in &mut browserless.result_rules {
        rule.selector = rule.selector.trim().to_string();
        rule.field = rule.field.trim().to_string();
        if !matches!(rule.outcome.as_str(), "already" | "success" | "failed")
            || !matches!(rule.kind.as_str(), "text" | "selector" | "json")
            || !matches!(
                rule.value_type.as_str(),
                "string" | "boolean" | "number" | "null"
            )
        {
            return Err(ApiError::bad_request("结果识别规则类型无效"));
        }
        if rule.selector.len() > 2048 || rule.field.len() > 2048 || rule.value.len() > 4096 {
            return Err(ApiError::bad_request("结果识别规则内容过长"));
        }
        if rule.kind != "json"
            && ((!rule.selector.is_empty() && scraper::Selector::parse(&rule.selector).is_err())
                || (rule.kind == "selector" && rule.selector.is_empty()))
        {
            return Err(ApiError::bad_request("结果规则需要有效的 CSS Selector"));
        }
        if rule.kind == "text" && rule.value.trim().is_empty() {
            return Err(ApiError::bad_request("文字规则的提示内容不能为空"));
        }
        if rule.kind == "json" {
            if !rule.field.starts_with('/') {
                return Err(ApiError::bad_request(
                    "JSON 字段路径必须以 / 开头，例如 /state 或 /data/status",
                ));
            }
            if rule.value_type == "boolean" && !matches!(rule.value.as_str(), "true" | "false") {
                return Err(ApiError::bad_request("布尔值必须填写 true 或 false"));
            }
            if rule.value_type == "number" && !rule.value.parse::<f64>().is_ok_and(f64::is_finite) {
                return Err(ApiError::bad_request("数字规则需要有效数字"));
            }
        }
    }
    browserless.attendance_path = browserless.attendance_path.trim().to_string();
    browserless.captcha_selector = browserless.captcha_selector.trim().to_string();
    browserless.captcha_input_selector = browserless.captcha_input_selector.trim().to_string();
    browserless.already_keywords = browserless.already_keywords.trim().to_string();
    if browserless.already_keywords.chars().count() > 4096 {
        return Err(ApiError::bad_request("已签到提示文字不能超过 4096 个字符"));
    }
    if body.sign_in_method.as_deref() == Some(crate::sign_in::SIGN_IN_METHOD_OCR_CAPTCHA) {
        if browser != crate::sign_in::SIGN_IN_BROWSER_BROWSERLESS {
            return Err(ApiError::bad_request("图片验证码签到请使用 Browserless"));
        }
        let path = &browserless.attendance_path;
        if !path.starts_with('/')
            || path.starts_with("//")
            || path.contains('\\')
            || path.contains('#')
            || path.chars().any(char::is_control)
            || path.len() > 2048
        {
            return Err(ApiError::bad_request(
                "签到路径必须是以 / 开头的站内路径，不能包含域名、反斜杠或片段",
            ));
        }
        for selector in [
            &browserless.captcha_selector,
            &browserless.captcha_input_selector,
        ] {
            if selector.is_empty()
                || selector.chars().count() > 2048
                || scraper::Selector::parse(selector).is_err()
            {
                return Err(ApiError::bad_request(
                    "请填写有效的验证码图片和输入框 CSS Selector（最多 2048 字符）",
                ));
            }
        }
    }
    browserless.cf_mode = crate::sign_in::normalize_browserless_cf_mode(&browserless.cf_mode)
        .ok_or_else(|| ApiError::bad_request("Browserless cf_mode 必须是 auto、page 或 turnstile"))?
        .to_string();
    validate_browserless_timing("wait_ms", browserless.wait_ms, true)?;
    validate_browserless_timing("solve_timeout", browserless.solve_timeout, false)?;
    validate_browserless_timing("action_timeout", browserless.action_timeout, false)?;
    validate_browserless_timing("post_click_wait_ms", browserless.post_click_wait_ms, true)?;
    body.browserless = Some(browserless);

    if body.name.is_empty() {
        return Err(ApiError::bad_request("名称不能为空"));
    }
    body.cron_expression
        .parse::<cron::Schedule>()
        .map_err(|e| ApiError::bad_request(format!("无效的cron表达式: {}", e)))?;
    if site.site_type != "nexusphp" && site.site_type != "nexus_php" {
        return Err(ApiError::bad_request("自动签到目前仅支持 NexusPHP 站点"));
    }
    let settings = state.db.get_settings().await?;
    validate_sign_in_browser_config(browser, &settings)?;
    Ok(())
}

fn validate_sign_in_browser_config(browser: &str, settings: &GlobalConfig) -> Result<(), ApiError> {
    match browser {
        crate::sign_in::SIGN_IN_BROWSER_LIGHTPANDA => {
            let endpoint_configured = settings
                .lightpanda
                .endpoint
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty());
            let token_configured = settings
                .lightpanda
                .token
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty());
            if !endpoint_configured && !token_configured {
                return Err(ApiError::bad_request(
                    "请先配置公共 Lightpanda endpoint 或 token",
                ));
            }
        }
        crate::sign_in::SIGN_IN_BROWSER_BROWSERLESS => {
            let address_configured = settings
                .browserless
                .address
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty());
            let token_configured = settings
                .browserless
                .token
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty());
            if !address_configured || !token_configured {
                return Err(ApiError::bad_request(
                    "请先配置公共 Browserless 地址和 Token",
                ));
            }
        }
        _ => return Err(ApiError::bad_request("未知签到浏览器")),
    }
    Ok(())
}

fn validate_browserless_timing(
    field: &str,
    value: Option<u64>,
    allow_zero: bool,
) -> Result<(), ApiError> {
    const MAX_BROWSERLESS_TIMING_MS: u64 = 3_600_000;
    let Some(value) = value else {
        return Ok(());
    };
    if (!allow_zero && value == 0) || value > MAX_BROWSERLESS_TIMING_MS {
        let range = if allow_zero {
            "0..=3600000"
        } else {
            "1..=3600000"
        };
        return Err(ApiError::bad_request(format!(
            "Browserless {} 必须在 {} 毫秒范围内",
            field, range
        )));
    }
    Ok(())
}

// ========== Downloaders API ==========

#[derive(Debug, Deserialize)]
struct CreateDownloaderRequest {
    name: String,
    downloader_type: String,
    url: String,
    username: Option<String>,
    password: Option<String>,
    #[serde(default)]
    copy_from_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct UpdateDownloaderRequest {
    name: String,
    downloader_type: String,
    url: String,
    username: Option<String>,
    password: Option<String>,
    #[serde(default)]
    clear_password: bool,
}

#[derive(Debug, Serialize)]
struct DownloaderResponse {
    id: i64,
    name: String,
    downloader_type: String,
    url: String,
    username: String,
    password_configured: bool,
    created_at: String,
    updated_at: String,
}

impl From<crate::downloader::DownloaderRecord> for DownloaderResponse {
    fn from(downloader: crate::downloader::DownloaderRecord) -> Self {
        Self {
            id: downloader.id,
            name: downloader.name,
            downloader_type: downloader.downloader_type,
            url: downloader.url,
            username: downloader.username,
            password_configured: !downloader.password.is_empty(),
            created_at: downloader.created_at,
            updated_at: downloader.updated_at,
        }
    }
}

fn resolve_downloader_password(
    existing: &str,
    incoming: Option<String>,
    clear_password: bool,
) -> Result<String, ApiError> {
    if clear_password {
        if incoming
            .as_ref()
            .is_some_and(|password| !password.is_empty())
        {
            return Err(ApiError::bad_request(
                "password and clear_password cannot both be set",
            ));
        }
        return Ok(String::new());
    }

    Ok(incoming
        .filter(|password| !password.is_empty())
        .unwrap_or_else(|| existing.to_string()))
}

fn resolve_created_downloader_password(
    copied_password: String,
    incoming: Option<String>,
) -> String {
    incoming
        .filter(|password| !password.is_empty())
        .unwrap_or(copied_password)
}

fn downloader_connection_identity_changed(
    current_type: &str,
    current_url: &str,
    next_type: &str,
    next_url: &str,
) -> bool {
    let current_is_qb = current_type.trim().eq_ignore_ascii_case("qb")
        || current_type.trim().eq_ignore_ascii_case("qbittorrent");
    let next_is_qb = next_type.trim().eq_ignore_ascii_case("qb")
        || next_type.trim().eq_ignore_ascii_case("qbittorrent");
    let same_type =
        (current_is_qb && next_is_qb) || current_type.trim().eq_ignore_ascii_case(next_type.trim());
    let same_url =
        current_url.trim().trim_end_matches('/') == next_url.trim().trim_end_matches('/');
    !same_type || !same_url
}

async fn list_downloaders(
    State(state): State<AppState>,
) -> Result<Json<Vec<DownloaderResponse>>, ApiError> {
    Ok(Json(
        state
            .db
            .list_downloaders()
            .await?
            .into_iter()
            .map(DownloaderResponse::from)
            .collect(),
    ))
}

async fn create_downloader(
    State(state): State<AppState>,
    Json(body): Json<CreateDownloaderRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if body.name.is_empty() || body.url.is_empty() {
        return Err(ApiError::bad_request("名称和URL不能为空"));
    }
    let copied_password = if let Some(source_id) = body.copy_from_id {
        if source_id <= 0 {
            return Err(ApiError::bad_request("copy_from_id must be positive"));
        }
        state
            .db
            .get_downloader(source_id)
            .await?
            .ok_or_else(|| ApiError::not_found("复制来源下载器不存在"))?
            .password
    } else {
        String::new()
    };
    let password = resolve_created_downloader_password(copied_password, body.password);
    let id = state
        .db
        .create_downloader(
            &body.name,
            &body.downloader_type,
            &body.url,
            body.username.as_deref().unwrap_or(""),
            &password,
        )
        .await?;
    Ok(Json(serde_json::json!({ "id": id })))
}

async fn update_downloader(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<UpdateDownloaderRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if body.name.is_empty() || body.url.is_empty() {
        return Err(ApiError::bad_request("名称和URL不能为空"));
    }
    let existing = state
        .db
        .get_downloader(id)
        .await?
        .ok_or_else(|| ApiError::not_found("下载器不存在"))?;
    let username = body
        .username
        .as_deref()
        .unwrap_or(&existing.username)
        .to_string();
    let password =
        resolve_downloader_password(&existing.password, body.password, body.clear_password)?;
    if state
        .db
        .has_active_relocation_for_downloader(id)
        .await
        .map_err(media_app_error)?
    {
        if downloader_connection_identity_changed(
            &existing.downloader_type,
            &existing.url,
            &body.downloader_type,
            &body.url,
        ) {
            return Err(ApiError::conflict(
                "活动迁移任务引用此下载器，不能修改下载器类型或 URL",
            ));
        }
        let credentials_changed = existing.username != username || existing.password != password;
        if credentials_changed && (username.trim().is_empty() || password.is_empty()) {
            return Err(ApiError::conflict(
                "活动迁移任务引用此下载器，只允许轮换为非空凭据",
            ));
        }
    }
    state
        .db
        .update_downloader(
            id,
            &body.name,
            &body.downloader_type,
            &body.url,
            &username,
            &password,
        )
        .await
        .map_err(media_app_error)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn delete_downloader(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if state
        .db
        .has_active_relocation_for_downloader(id)
        .await
        .map_err(media_app_error)?
    {
        return Err(ApiError::conflict("活动迁移任务引用此下载器，不能删除"));
    }
    state
        .db
        .delete_downloader(id)
        .await
        .map_err(media_app_error)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn test_downloader(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<crate::downloader::DownloaderTestResult>, ApiError> {
    let dl = state
        .db
        .get_downloader(id)
        .await?
        .ok_or_else(|| ApiError::not_found("下载器不存在"))?;
    let client = state.pool.get(&dl).await.map_err(ApiError::bad_request)?;
    let result = client
        .test_connection()
        .await
        .map_err(|e| ApiError::internal(e))?;
    Ok(Json(result))
}

async fn get_downloader_space_stats(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<DownloaderSpaceStats>, ApiError> {
    let dl = state
        .db
        .get_downloader(id)
        .await?
        .ok_or_else(|| ApiError::not_found("下载器不存在"))?;
    let client = state.pool.get(&dl).await.map_err(ApiError::bad_request)?;
    let torrents = state
        .collector
        .get_all_torrents(&dl)
        .await
        .map_err(ApiError::internal)?;
    let stats = client
        .get_effective_free_space(None, &torrents)
        .await
        .map_err(ApiError::internal)?;

    Ok(Json(stats))
}

async fn get_downloader_default_path(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let dl = state
        .db
        .get_downloader(id)
        .await?
        .ok_or_else(|| ApiError::not_found("下载器不存在"))?;
    let client = state.pool.get(&dl).await.map_err(ApiError::bad_request)?;
    let path = client
        .get_default_save_path()
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(serde_json::json!({ "path": path })))
}

#[derive(Debug, Deserialize)]
struct DownloaderTorrentQuery {
    keyword: Option<String>,
    #[serde(default)]
    include_incomplete: bool,
}

#[derive(Debug, Serialize)]
struct TransferableTorrentResponse {
    hash: String,
    name: String,
    size: i64,
    downloaded: i64,
    save_path: String,
    category: String,
    tags: String,
    added_on: i64,
    progress: f64,
    state: String,
}

async fn list_downloader_torrents(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(query): Query<DownloaderTorrentQuery>,
) -> Result<Json<Vec<TransferableTorrentResponse>>, ApiError> {
    if !state.self_use {
        return Err(ApiError::not_found("功能不可用"));
    }
    let downloader = state
        .db
        .get_downloader(id)
        .await?
        .ok_or_else(|| ApiError::not_found("下载器不存在"))?;
    if !matches!(downloader.downloader_type.as_str(), "qbittorrent" | "qb") {
        return Err(ApiError::bad_request("仅支持 qBittorrent 下载器"));
    }
    let client = state
        .pool
        .get(&downloader)
        .await
        .map_err(ApiError::bad_request)?;
    let keyword = query.keyword.unwrap_or_default().trim().to_lowercase();
    let mut torrents = client
        .list_torrents(None)
        .await
        .map_err(ApiError::bad_gateway)?
        .into_iter()
        .filter(|torrent| {
            query.include_incomplete
                || torrent_is_complete(
                    torrent.completion_on,
                    torrent.downloaded,
                    torrent.size,
                    torrent.progress,
                    &torrent.state,
                )
        })
        .filter(|torrent| {
            keyword.is_empty()
                || torrent.name.to_lowercase().contains(&keyword)
                || torrent.hash.to_lowercase().contains(&keyword)
        })
        .map(|torrent| TransferableTorrentResponse {
            hash: torrent.hash,
            name: torrent.name,
            size: torrent.size,
            downloaded: torrent.downloaded,
            save_path: torrent.save_path,
            category: torrent.category,
            tags: torrent.tags,
            added_on: torrent.added_on,
            progress: torrent.progress,
            state: torrent.state,
        })
        .collect::<Vec<_>>();
    torrents.sort_by(|left, right| right.added_on.cmp(&left.added_on));
    Ok(Json(torrents))
}

#[derive(Debug, Deserialize)]
struct CreateOpenListTransferRequest {
    hashes: Vec<String>,
    target_directory_id: i64,
    expected_config_updated_at: String,
    #[serde(default)]
    target_mode: Option<String>,
    #[serde(default)]
    target_downloader_id: Option<i64>,
    #[serde(default)]
    plan_confirmed: bool,
    #[serde(default)]
    planned_targets: Vec<OpenListTransferPlannedTarget>,
    #[serde(default)]
    expected_source_downloader_updated_at: Option<String>,
    #[serde(default)]
    expected_target_downloader_updated_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PreviewOpenListTransferRequest {
    hashes: Vec<String>,
    target_directory_id: i64,
    expected_config_updated_at: String,
    #[serde(default)]
    target_downloader_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OpenListTransferPlannedTarget {
    hash: String,
    target_openlist_path: String,
    target_qb_path: String,
}

#[derive(Debug, Clone, Serialize)]
struct OpenListTransferClassification {
    tmdb_id: Option<i64>,
    media_type: Option<String>,
    title: String,
    year: Option<u32>,
    category: String,
    genre: String,
    matched: bool,
    source: String,
}

#[derive(Debug, Clone, Serialize)]
struct OpenListTransferPlanItem {
    hash: String,
    torrent_name: String,
    target_openlist_path: String,
    target_qb_path: String,
    classification: OpenListTransferClassification,
}

#[derive(Debug, Serialize)]
struct OpenListTransferPlanResponse {
    mode: &'static str,
    target_directory_id: i64,
    target_downloader_id: i64,
    openlist_root: String,
    qb_root: String,
    directories: Vec<String>,
    items: Vec<OpenListTransferPlanItem>,
    warnings: Vec<String>,
    expected_config_updated_at: String,
    source_downloader_updated_at: String,
    target_downloader_updated_at: String,
}

#[derive(Debug, Serialize)]
struct CreateOpenListTransferResponse {
    created: usize,
    skipped: usize,
}

async fn create_openlist_transfer(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(payload): Json<CreateOpenListTransferRequest>,
) -> Result<Json<CreateOpenListTransferResponse>, ApiError> {
    if !state.self_use {
        return Err(ApiError::not_found("功能不可用"));
    }
    let mode = normalize_transfer_mode(payload.target_mode.as_deref())?;
    let (config, downloader, target, target_downloader, client) = load_openlist_transfer_context(
        &state,
        id,
        payload.target_directory_id,
        payload.target_downloader_id,
        &payload.expected_config_updated_at,
    )
    .await?;
    if payload
        .expected_source_downloader_updated_at
        .as_deref()
        .is_some_and(|expected| expected != downloader.updated_at)
        || payload
            .expected_target_downloader_updated_at
            .as_deref()
            .is_some_and(|expected| expected != target_downloader.updated_at)
    {
        return Err(ApiError::conflict(
            "qBittorrent 配置已变化，请重新生成 TMDB 分类规划",
        ));
    }
    let hashes = normalize_transfer_hashes(payload.hashes)?;
    let torrents = load_transfer_torrents(&client, &config, id, &hashes).await?;
    let selected = torrents
        .iter()
        .map(|torrent| (torrent.hash.to_ascii_lowercase(), torrent.name.clone()))
        .collect::<Vec<_>>();

    let (created, skipped) = if mode == "tmdb" {
        if !payload.plan_confirmed {
            return Err(ApiError::bad_request(
                "TMDB 分类目录必须先生成规划并完成二次确认",
            ));
        }
        let planned =
            validate_planned_transfer_targets(&hashes, &payload.planned_targets, &target)?;
        let openlist = OpenListClient::new(&config.base_url, &config.api_key)
            .map_err(ApiError::bad_gateway)?;
        let mut checked_directories = HashSet::new();
        for (target_openlist_path, _) in planned.values() {
            if !checked_directories.insert(target_openlist_path.clone()) {
                continue;
            }
            match openlist
                .stat_if_exists(target_openlist_path)
                .await
                .map_err(ApiError::bad_gateway)?
            {
                Some(object) if object.is_dir => {}
                Some(_) => {
                    return Err(ApiError::bad_request(format!(
                        "TMDB 分类目录不是目录，无法确认: {target_openlist_path}"
                    )));
                }
                None => {
                    return Err(ApiError::conflict(
                        "TMDB 分类目录已不存在，请重新生成目录后再确认",
                    ));
                }
            }
        }
        let targets = torrents
            .iter()
            .map(|torrent| {
                let hash = torrent.hash.to_ascii_lowercase();
                let (target_openlist_path, target_qb_path) = planned
                    .get(&hash)
                    .cloned()
                    .ok_or_else(|| ApiError::bad_request("TMDB 分类规划缺少种子目标路径"))?;
                Ok(ManualMediaRelocationTarget {
                    infohash: hash,
                    torrent_name: torrent.name.clone(),
                    target_openlist_path,
                    target_qb_path,
                })
            })
            .collect::<Result<Vec<_>, ApiError>>()?;
        state
            .db
            .enqueue_manual_media_relocation_jobs_with_targets_if_config_current(
                id,
                target.downloader_id,
                &targets,
                &payload.expected_config_updated_at,
                &downloader.updated_at,
                &target_downloader.updated_at,
            )
            .await
            .map_err(media_app_error)?
    } else {
        state
            .db
            .enqueue_manual_media_relocation_jobs_if_config_current(
                id,
                target.downloader_id,
                &target.openlist_path,
                &target.qb_path,
                &selected,
                &payload.expected_config_updated_at,
                &downloader.updated_at,
                &target_downloader.updated_at,
            )
            .await
            .map_err(media_app_error)?
    };
    state.relocation_scheduler.request_scan();
    Ok(Json(CreateOpenListTransferResponse { created, skipped }))
}

async fn preview_openlist_transfer(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(payload): Json<PreviewOpenListTransferRequest>,
) -> Result<Json<OpenListTransferPlanResponse>, ApiError> {
    if !state.self_use {
        return Err(ApiError::not_found("功能不可用"));
    }
    let (config, downloader, target, target_downloader, client) = load_openlist_transfer_context(
        &state,
        id,
        payload.target_directory_id,
        payload.target_downloader_id,
        &payload.expected_config_updated_at,
    )
    .await?;
    let hashes = normalize_transfer_hashes(payload.hashes)?;
    let torrents = load_transfer_torrents(&client, &config, id, &hashes).await?;
    let target_openlist_root =
        normalize_path(&target.openlist_path).map_err(ApiError::bad_request)?;
    let target_qb_root = normalize_path(&target.qb_path).map_err(ApiError::bad_request)?;
    let tmdb_configured = state
        .db
        .get_media_settings()
        .await
        .map_err(media_app_error)?
        .tmdb_token
        .is_some_and(|token| !token.trim().is_empty());
    let mut tmdb_cache = HashMap::<String, Option<TmdbMedia>>::new();
    let mut items = Vec::with_capacity(torrents.len());
    let mut warnings = Vec::new();
    let mut directories = HashSet::new();
    for torrent in torrents {
        let classification =
            classify_transfer_torrent(&state, id, &torrent, tmdb_configured, &mut tmdb_cache)
                .await?;
        let relative = archive_relative_directory(
            &classification.category,
            &classification.genre,
            classification.year,
        );
        let target_openlist_path =
            join_path(&target_openlist_root, &relative).map_err(ApiError::bad_request)?;
        let target_qb_path =
            join_path(&target_qb_root, &relative).map_err(ApiError::bad_request)?;
        if !classification.matched {
            warnings.push(format!(
                "{} 未匹配到 TMDB，已使用 {} / {} / {}",
                torrent.name,
                classification.category,
                classification.genre,
                classification
                    .year
                    .map(|year| year.to_string())
                    .unwrap_or_else(|| "年份未知".to_string())
            ));
        }
        directories.insert(target_openlist_path.clone());
        items.push(OpenListTransferPlanItem {
            hash: torrent.hash.to_ascii_lowercase(),
            torrent_name: torrent.name,
            target_openlist_path,
            target_qb_path,
            classification,
        });
    }

    let openlist =
        OpenListClient::new(&config.base_url, &config.api_key).map_err(ApiError::bad_gateway)?;
    let mut directories = directories.into_iter().collect::<Vec<_>>();
    directories.sort();
    for directory in &directories {
        openlist
            .create_directory_tree_if_missing(directory)
            .await
            .map_err(ApiError::bad_gateway)?;
    }
    Ok(Json(OpenListTransferPlanResponse {
        mode: "tmdb",
        target_directory_id: payload.target_directory_id,
        target_downloader_id: target.downloader_id,
        openlist_root: target_openlist_root,
        qb_root: target_qb_root,
        directories,
        items,
        warnings,
        expected_config_updated_at: config.updated_at,
        source_downloader_updated_at: downloader.updated_at,
        target_downloader_updated_at: target_downloader.updated_at,
    }))
}

fn normalize_transfer_mode(value: Option<&str>) -> Result<&'static str, ApiError> {
    match value
        .unwrap_or("fixed")
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "fixed" | "directory" => Ok("fixed"),
        "tmdb" | "auto" | "auto_tmdb" => Ok("tmdb"),
        _ => Err(ApiError::bad_request("target_mode must be fixed or tmdb")),
    }
}

fn normalize_transfer_hashes(hashes: Vec<String>) -> Result<Vec<String>, ApiError> {
    if hashes.is_empty() || hashes.len() > 100 {
        return Err(ApiError::bad_request("请选择 1 到 100 个种子"));
    }
    let mut hashes = hashes
        .into_iter()
        .map(|hash| hash.trim().to_ascii_lowercase())
        .collect::<Vec<_>>();
    hashes.sort();
    hashes.dedup();
    if hashes
        .iter()
        .any(|hash| !matches!(hash.len(), 40 | 64) || !hash.bytes().all(|c| c.is_ascii_hexdigit()))
    {
        return Err(ApiError::bad_request("种子 hash 格式无效"));
    }
    Ok(hashes)
}

async fn load_openlist_transfer_context(
    state: &AppState,
    source_id: i64,
    target_directory_id: i64,
    requested_target_downloader_id: Option<i64>,
    expected_config_updated_at: &str,
) -> Result<
    (
        OpenListConfig,
        crate::downloader::DownloaderRecord,
        OpenListTargetDirectory,
        crate::downloader::DownloaderRecord,
        std::sync::Arc<dyn DownloaderClient>,
    ),
    ApiError,
> {
    let downloader = state
        .db
        .get_downloader(source_id)
        .await?
        .ok_or_else(|| ApiError::not_found("下载器不存在"))?;
    if !matches!(downloader.downloader_type.as_str(), "qbittorrent" | "qb") {
        return Err(ApiError::bad_request("仅支持 qBittorrent 下载器"));
    }
    let config = state
        .db
        .get_openlist_config()
        .await
        .map_err(media_app_error)?;
    if expected_config_updated_at != config.updated_at {
        return Err(ApiError::conflict(
            "OpenList settings changed; reload migration targets before creating tasks",
        ));
    }
    if config.base_url.trim().is_empty() || config.api_key.trim().is_empty() {
        return Err(ApiError::bad_request("请先配置 OpenList 地址和 API Key"));
    }
    if !config
        .path_mappings
        .iter()
        .any(|mapping| mapping.downloader_id == source_id)
    {
        return Err(ApiError::bad_request(
            "该下载器尚未配置 OpenList 来源路径映射",
        ));
    }
    let target = config
        .target_directories
        .iter()
        .find(|target| target.id == Some(target_directory_id))
        .cloned()
        .ok_or_else(|| ApiError::bad_request("所选迁移目标目录不存在，请刷新后重试"))?;
    if requested_target_downloader_id.is_some_and(|requested| requested != target.downloader_id) {
        return Err(ApiError::conflict(
            "目标下载器与所选目录不匹配，请刷新后重试",
        ));
    }
    let target_downloader = state
        .db
        .get_downloader(target.downloader_id)
        .await?
        .ok_or_else(|| ApiError::conflict("目标下载器已不存在，请刷新后重试"))?;
    if !matches!(
        target_downloader.downloader_type.as_str(),
        "qbittorrent" | "qb"
    ) {
        return Err(ApiError::conflict(
            "目标下载器已不再是 qBittorrent，请刷新后重试",
        ));
    }
    let client = state
        .pool
        .get(&downloader)
        .await
        .map_err(ApiError::bad_request)?;
    Ok((config, downloader, target, target_downloader, client))
}

async fn load_transfer_torrents(
    client: &std::sync::Arc<dyn DownloaderClient>,
    config: &OpenListConfig,
    source_id: i64,
    hashes: &[String],
) -> Result<Vec<crate::downloader::TorrentInfo>, ApiError> {
    let torrents = client
        .list_torrents_by_hashes(hashes)
        .await
        .map_err(ApiError::bad_gateway)?;
    let expected_hashes = hashes.iter().cloned().collect::<HashSet<_>>();
    let returned_hashes = torrents
        .iter()
        .map(|torrent| torrent.hash.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    if torrents.len() != expected_hashes.len() || returned_hashes != expected_hashes {
        return Err(ApiError::bad_request("部分种子已不存在，请刷新后重试"));
    }
    if torrents.iter().any(|torrent| {
        !torrent_is_complete(
            torrent.completion_on,
            torrent.downloaded,
            torrent.size,
            torrent.progress,
            &torrent.state,
        )
    }) {
        return Err(ApiError::bad_request("只能转移已下载完成的种子"));
    }
    for torrent in &torrents {
        let save_path = normalize_path(&torrent.save_path).map_err(ApiError::bad_request)?;
        let mapped = config
            .path_mappings
            .iter()
            .filter(|mapping| mapping.downloader_id == source_id)
            .any(|mapping| {
                normalize_path(&mapping.qb_path)
                    .is_ok_and(|mapping_root| is_path_prefix(&mapping_root, &save_path))
            });
        if !mapped {
            return Err(ApiError::bad_request(format!(
                "种子 {} 的保存路径 {} 没有 OpenList 来源映射",
                torrent.name, save_path
            )));
        }
        let files = client
            .get_torrent_files(&torrent.hash)
            .await
            .map_err(ApiError::bad_gateway)?;
        validate_torrent_files_complete(&files).map_err(|error| {
            ApiError::bad_request(format!(
                "种子 {:?} 包含未完整下载或已跳过的文件: {error}",
                torrent.name
            ))
        })?;
    }
    Ok(torrents)
}

async fn classify_transfer_torrent(
    state: &AppState,
    source_id: i64,
    torrent: &crate::downloader::TorrentInfo,
    tmdb_configured: bool,
    tmdb_cache: &mut HashMap<String, Option<TmdbMedia>>,
) -> Result<OpenListTransferClassification, ApiError> {
    let parsed = ReleaseParser::default().parse(&torrent.name).ok();
    let linked_download = state
        .db
        .get_media_download_by_infohash(source_id, &torrent.hash)
        .await
        .map_err(media_app_error)?;
    if let Some(download) = linked_download {
        let subscription = match download.subscription_id {
            Some(id) => state
                .db
                .get_subscription(id)
                .await
                .map_err(media_app_error)?,
            None => None,
        };
        let category = media_download_category(
            &download.target_key,
            subscription
                .as_ref()
                .is_some_and(|item| item.tmdb_is_animation),
        );
        let year = subscription
            .as_ref()
            .and_then(|item| item.year)
            .or_else(|| release_year(&download.release_json))
            .or_else(|| parsed.as_ref().and_then(|release| release.year));
        let genre = subscription
            .as_ref()
            .and_then(|item| item.tmdb_genres.first())
            .map(|genre| safe_transfer_component(&genre.name, "其他"))
            .unwrap_or_else(|| "其他".to_string());
        return Ok(OpenListTransferClassification {
            tmdb_id: subscription.as_ref().map(|item| item.tmdb_id),
            media_type: subscription.as_ref().map(|item| item.media_type.clone()),
            title: subscription
                .as_ref()
                .map(|item| item.title.clone())
                .unwrap_or_else(|| download.title.clone()),
            year,
            category: category.to_string(),
            genre,
            matched: subscription.is_some(),
            source: if subscription.is_some() {
                "subscription".to_string()
            } else {
                "download_record".to_string()
            },
        });
    }

    let query = parsed
        .as_ref()
        .map(|release| release.title.trim().to_string())
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| torrent.name.trim().to_string());
    let year = parsed.as_ref().and_then(|release| release.year);
    let media_type_hint = parsed.as_ref().and_then(|release| {
        if release.season.is_some() || release.full_season || !release.absolute_episodes.is_empty()
        {
            Some("tv")
        } else if release.matched_rule == "movie" {
            Some("movie")
        } else {
            None
        }
    });
    if tmdb_configured {
        let cache_key = format!(
            "{}:{}",
            media_type_hint.unwrap_or("multi"),
            query.to_ascii_lowercase()
        );
        if !tmdb_cache.contains_key(&cache_key) {
            let result = state
                .media
                .tmdb_search(&query, media_type_hint)
                .await
                .map_err(ApiError::from)?
                .into_iter()
                .next();
            tmdb_cache.insert(cache_key.clone(), result);
        }
        if let Some(Some(media)) = tmdb_cache.get(&cache_key) {
            let category = match media.media_type {
                TmdbMediaType::Movie => "电影",
                TmdbMediaType::Tv if media.is_animation => "动漫",
                TmdbMediaType::Tv => "电视剧",
            };
            let genre = media
                .genres
                .first()
                .map(|genre| safe_transfer_component(&genre.name, "其他"))
                .unwrap_or_else(|| "其他".to_string());
            return Ok(OpenListTransferClassification {
                tmdb_id: Some(media.tmdb_id),
                media_type: Some(media.media_type.as_str().to_string()),
                title: media.title.clone(),
                year: media.year.or(year),
                category: category.to_string(),
                genre,
                matched: true,
                source: "tmdb".to_string(),
            });
        }
    }
    Ok(OpenListTransferClassification {
        tmdb_id: None,
        media_type: None,
        title: query,
        year,
        category: fallback_transfer_category(parsed.as_ref(), media_type_hint).to_string(),
        genre: "其他".to_string(),
        matched: false,
        source: "fallback".to_string(),
    })
}

fn fallback_transfer_category(
    parsed: Option<&ReleaseInfo>,
    media_type_hint: Option<&str>,
) -> &'static str {
    if media_type_hint == Some("movie") {
        "电影"
    } else if parsed.is_some_and(|release| !release.absolute_episodes.is_empty()) {
        "动漫"
    } else {
        "电视剧"
    }
}

fn release_year(value: &str) -> Option<u32> {
    serde_json::from_str::<serde_json::Value>(value)
        .ok()
        .and_then(|value| value.get("year").and_then(serde_json::Value::as_u64))
        .and_then(|year| u32::try_from(year).ok())
}

fn safe_transfer_component(value: &str, fallback: &str) -> String {
    let value = value
        .trim()
        .chars()
        .filter(|character| !character.is_control())
        .map(|character| {
            if character == '/' || character == '\\' {
                '_'
            } else {
                character
            }
        })
        .collect::<String>();
    let value = value
        .trim_matches(['.', ' '])
        .chars()
        .take(80)
        .collect::<String>();
    if value.is_empty() || value == "." || value == ".." {
        fallback.to_string()
    } else {
        value
    }
}

fn validate_planned_transfer_targets(
    hashes: &[String],
    planned: &[OpenListTransferPlannedTarget],
    target: &OpenListTargetDirectory,
) -> Result<HashMap<String, (String, String)>, ApiError> {
    if planned.len() != hashes.len() {
        return Err(ApiError::bad_request(
            "TMDB 分类规划与选中的种子数量不一致，请重新生成目录",
        ));
    }
    let openlist_root = normalize_path(&target.openlist_path).map_err(ApiError::bad_request)?;
    let qb_root = normalize_path(&target.qb_path).map_err(ApiError::bad_request)?;
    let expected = hashes.iter().cloned().collect::<HashSet<_>>();
    let mut result = HashMap::new();
    for item in planned {
        let hash = item.hash.trim().to_ascii_lowercase();
        if !expected.contains(&hash) || result.contains_key(&hash) {
            return Err(ApiError::bad_request("TMDB 分类规划包含未知或重复的种子"));
        }
        let openlist_path =
            normalize_path(&item.target_openlist_path).map_err(ApiError::bad_request)?;
        let qb_path = normalize_path(&item.target_qb_path).map_err(ApiError::bad_request)?;
        if !is_path_prefix(&openlist_root, &openlist_path) || !is_path_prefix(&qb_root, &qb_path) {
            return Err(ApiError::bad_request(
                "TMDB 分类目标路径必须位于所选根目录下",
            ));
        }
        let openlist_suffix = transfer_relative_suffix(&openlist_root, &openlist_path)?;
        let qb_suffix = transfer_relative_suffix(&qb_root, &qb_path)?;
        if openlist_suffix != qb_suffix || !is_generated_transfer_suffix(&openlist_suffix) {
            return Err(ApiError::bad_request(
                "TMDB 分类目标路径必须使用生成的 云母/类型/主类型/年份 目录",
            ));
        }
        result.insert(hash, (openlist_path, qb_path));
    }
    if result.len() != expected.len() {
        return Err(ApiError::bad_request("TMDB 分类规划缺少种子目标路径"));
    }
    Ok(result)
}

fn transfer_relative_suffix(root: &str, path: &str) -> Result<String, ApiError> {
    if root == path {
        return Ok(String::new());
    }
    if root == "/" {
        return Ok(path.trim_start_matches('/').to_string());
    }
    path.strip_prefix(root)
        .map(|suffix| suffix.trim_start_matches('/').to_string())
        .ok_or_else(|| ApiError::bad_request("目标路径不在根目录下"))
}

fn is_generated_transfer_suffix(value: &str) -> bool {
    let mut components = value.split('/');
    let Some(prefix) = components.next() else {
        return false;
    };
    let Some(category) = components.next() else {
        return false;
    };
    let Some(genre) = components.next() else {
        return false;
    };
    let Some(year) = components.next() else {
        return false;
    };
    if components.next().is_some()
        || prefix != "云母"
        || !matches!(category, "电影" | "电视剧" | "动漫")
        || genre.is_empty()
        || genre == "."
        || genre == ".."
        || genre.chars().count() > 80
        || genre.chars().any(|character| character.is_control())
    {
        return false;
    }
    year == "年份未知"
        || (!year.is_empty()
            && year.bytes().all(|byte| byte.is_ascii_digit())
            && year.parse::<u32>().is_ok())
}

// ========== Brush Tasks API ==========

async fn list_brush_tasks(
    State(state): State<AppState>,
) -> Result<Json<Vec<crate::brush::BrushTaskRecord>>, ApiError> {
    Ok(Json(state.db.list_brush_tasks().await?))
}

async fn get_brush_task(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<crate::brush::BrushTaskRecord>, ApiError> {
    let task = state
        .db
        .get_brush_task(id)
        .await?
        .ok_or_else(|| ApiError::not_found("刷流任务不存在"))?;
    Ok(Json(task))
}

/// 规范化 cron 表达式：标准5字段自动补秒字段
fn normalize_cron(expr: &str) -> String {
    let fields: Vec<&str> = expr.trim().split_whitespace().collect();
    if fields.len() == 5 {
        format!("0 {}", expr.trim())
    } else {
        expr.trim().to_string()
    }
}

async fn create_brush_task(
    State(state): State<AppState>,
    Json(mut body): Json<BrushTaskRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if body.name.is_empty()
        || body.cron_expression.is_empty()
        || body.tag.is_empty()
        || body.rss_url.is_empty()
    {
        return Err(ApiError::bad_request(
            "名称、cron表达式、标签和RSS地址不能为空",
        ));
    }
    let site_id = body
        .site_id
        .ok_or_else(|| ApiError::bad_request("必须选择一个具体站点"))?;
    state
        .db
        .get_site(site_id)
        .await?
        .ok_or_else(|| ApiError::bad_request("所选站点不存在"))?;
    body.cron_expression = normalize_cron(&body.cron_expression);
    body.cron_expression
        .parse::<cron::Schedule>()
        .map_err(|e| ApiError::bad_request(format!("无效的cron表达式: {}", e)))?;
    let id = state.db.create_brush_task(&body).await?;
    Ok(Json(serde_json::json!({ "id": id })))
}

async fn update_brush_task(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(mut body): Json<BrushTaskRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let site_id = body
        .site_id
        .ok_or_else(|| ApiError::bad_request("必须选择一个具体站点"))?;
    state
        .db
        .get_site(site_id)
        .await?
        .ok_or_else(|| ApiError::bad_request("所选站点不存在"))?;
    body.cron_expression = normalize_cron(&body.cron_expression);
    body.cron_expression
        .parse::<cron::Schedule>()
        .map_err(|e| ApiError::bad_request(format!("无效的cron表达式: {}", e)))?;
    state.db.update_brush_task(id, &body).await?;
    state
        .scheduler
        .refresh_task_config(id)
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn delete_brush_task(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state.scheduler.stop_task(id).await;
    state.db.delete_brush_task(id).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn start_brush_task(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state.db.set_brush_task_enabled(id, true).await?;
    state
        .scheduler
        .trigger_task(id)
        .await
        .map_err(map_brush_trigger_error)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn stop_brush_task(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state.db.set_brush_task_enabled(id, false).await?;
    state.scheduler.stop_task(id).await;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn run_brush_task_once(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state
        .scheduler
        .trigger_task(id)
        .await
        .map_err(map_brush_trigger_error)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn list_brush_task_torrents(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(query): Query<BrushTorrentsQuery>,
) -> Result<Json<BrushTaskTorrentsResponse>, ApiError> {
    let task = state
        .db
        .get_brush_task(id)
        .await?
        .ok_or_else(|| ApiError::not_found("刷流任务不存在"))?;
    let page = query.page.unwrap_or(1);
    let page_size = query.page_size.unwrap_or(20);
    let mut torrents = state
        .db
        .list_brush_task_torrents(id, page, page_size, query.keyword.as_deref())
        .await?;

    // 按 downloader_id 分组，分别从对应下载器拉取实时种子信息
    let mut dl_live_cache: std::collections::HashMap<i64, Vec<crate::downloader::TorrentInfo>> =
        std::collections::HashMap::new();
    for record in &torrents.records {
        let dl_id = match record.downloader_id {
            Some(id) => id,
            None => continue,
        };
        if dl_live_cache.contains_key(&dl_id) {
            continue;
        }
        if let Ok(Some(downloader)) = state.db.get_downloader(dl_id).await {
            if let Ok(live_torrents) = state
                .collector
                .get_tagged_torrents(&downloader, &task.tag)
                .await
            {
                dl_live_cache.insert(dl_id, live_torrents);
            }
        }
    }

    for record in &mut torrents.records {
        if let Some(dl_id) = record.downloader_id {
            if let Some(live_torrents) = dl_live_cache.get(&dl_id) {
                if let Some(live) = find_live_brush_torrent(record, live_torrents) {
                    apply_live_torrent(record, live);
                }
            }
        }
    }

    for record in &mut torrents.records {
        if record.torrent_id.is_none() && !looks_like_info_hash(&record.torrent_hash) {
            record.torrent_id = Some(record.torrent_hash.clone());
            record.torrent_hash.clear();
        }
    }

    Ok(Json(BrushTaskTorrentsResponse {
        task,
        page: torrents.page,
        page_size: torrents.page_size,
        total_records: torrents.total_records,
        records: torrents.records,
    }))
}

async fn stream_logs(
    State(state): State<AppState>,
) -> Sse<impl futures::Stream<Item = Result<Event, std::convert::Infallible>>> {
    let receiver = crate::logging::subscribe_logs();
    let stream = stream::unfold(
        (receiver, state.shutdown),
        |(mut receiver, shutdown)| async move {
            loop {
                let received = tokio::select! {
                    biased;
                    _ = shutdown.cancelled() => return None,
                    result = receiver.recv() => result,
                };
                match received {
                    Ok(line) => {
                        let payload = serde_json::json!({
                            "encoded_line": urlencoding::encode(&line).into_owned()
                        })
                        .to_string();
                        let event = Event::default().event("log").data(payload);
                        return Some((Ok(event), (receiver, shutdown)));
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        continue;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        return None;
                    }
                }
            }
        },
    );

    Sse::new(stream).keep_alive(KeepAlive::default())
}

// ========== Stats API ==========

#[derive(Debug, Deserialize)]
struct StatsQuery {
    task_id: Option<i64>,
    since: Option<String>,
    until: Option<String>,
    hours: Option<i64>,
}

#[derive(Debug, Serialize)]
struct StatsOverview {
    tasks: Vec<TaskOverview>,
}

#[derive(Debug, Serialize)]
struct TaskOverview {
    task_id: i64,
    task_name: String,
    total_uploaded: i64,
    total_downloaded: i64,
    torrent_count: i64,
    enabled: bool,
}

async fn stats_overview(State(state): State<AppState>) -> Result<Json<StatsOverview>, ApiError> {
    let tasks = state.db.list_brush_tasks().await?;
    let mut overviews = Vec::new();
    for task in &tasks {
        // 获取最新的统计快照
        let now = Utc::now().to_rfc3339();
        let since = (Utc::now() - chrono::Duration::seconds(120)).to_rfc3339();
        let snapshots = state
            .db
            .get_task_stats_snapshots(Some(task.id), &since, &now)
            .await?;
        let latest = snapshots.last();
        let (total_uploaded, total_downloaded, historical_torrent_count) =
            state.db.get_brush_task_transfer_totals(task.id).await?;
        overviews.push(TaskOverview {
            task_id: task.id,
            task_name: task.name.clone(),
            total_uploaded,
            total_downloaded,
            torrent_count: latest
                .map(|s| s.torrent_count)
                .unwrap_or(historical_torrent_count),
            enabled: task.enabled,
        });
    }
    Ok(Json(StatsOverview { tasks: overviews }))
}

async fn stats_trend(
    State(state): State<AppState>,
    Query(q): Query<StatsQuery>,
) -> Result<Json<Vec<crate::stats::TaskStatsSnapshot>>, ApiError> {
    let hours = q.hours.unwrap_or(24);
    let until = q.until.unwrap_or_else(|| Utc::now().to_rfc3339());
    let since = q.since.unwrap_or_else(|| {
        // Stats are sampled periodically, so add a small grace window to avoid
        // dropping the latest bucket on exact boundary cuts like "last 1h".
        (Utc::now() - chrono::Duration::hours(hours) - chrono::Duration::minutes(2)).to_rfc3339()
    });
    let data = state
        .db
        .get_task_stats_snapshots(q.task_id, &since, &until)
        .await?;
    Ok(Json(data))
}

#[derive(Debug, Deserialize)]
struct DownloaderStatsQuery {
    downloader_id: Option<i64>,
    since: Option<String>,
    until: Option<String>,
    hours: Option<i64>,
}

async fn downloader_speed_trend(
    State(state): State<AppState>,
    Query(q): Query<DownloaderStatsQuery>,
) -> Result<Json<Vec<crate::stats::DownloaderSpeedSnapshot>>, ApiError> {
    let hours = q.hours.unwrap_or(24);
    let until = q.until.unwrap_or_else(|| Utc::now().to_rfc3339());
    let since = q.since.unwrap_or_else(|| {
        (Utc::now() - chrono::Duration::hours(hours) - chrono::Duration::minutes(2)).to_rfc3339()
    });
    // Choose aggregation bucket based on time range to avoid returning excessive raw data:
    //   ≤ 1h  → raw 10s data (max ~360 pts/downloader)
    //   ≤ 6h  → 1-minute buckets (max ~360 pts)
    //   ≤ 24h → 5-minute buckets (max ~288 pts)
    //   > 24h → 1-hour buckets   (max ~168 pts for 7d)
    let bucket_secs = if hours <= 1 {
        None
    } else if hours <= 6 {
        Some(60)
    } else if hours <= 24 {
        Some(300)
    } else {
        Some(3600)
    };
    let data = state
        .db
        .get_downloader_speed_snapshots(q.downloader_id, &since, &until, bucket_secs)
        .await?;
    Ok(Json(data))
}

#[derive(Debug, Serialize)]
struct DailyTransferItem {
    date: String,
    uploaded: i64,
    downloaded: i64,
}

async fn daily_transfer(
    State(state): State<AppState>,
    Query(q): Query<StatsQuery>,
) -> Result<Json<Vec<DailyTransferItem>>, ApiError> {
    let until = q.until.unwrap_or_else(|| Utc::now().to_rfc3339());
    let since = q
        .since
        .unwrap_or_else(|| (Utc::now() - chrono::Duration::days(30)).to_rfc3339());
    let data = state
        .db
        .get_daily_transfer_totals(q.task_id, &since, &until)
        .await?;
    Ok(Json(
        data.into_iter()
            .map(|(date, uploaded, downloaded)| DailyTransferItem {
                date,
                uploaded,
                downloaded,
            })
            .collect(),
    ))
}

async fn get_system_stats(
    State(state): State<AppState>,
) -> Result<Json<SystemSnapshot>, StatusCode> {
    match state.monitor.latest().await {
        Some(snapshot) => Ok(Json(snapshot)),
        None => Err(StatusCode::SERVICE_UNAVAILABLE),
    }
}

#[derive(Debug, Deserialize)]
struct SystemStatsQuery {
    since: Option<String>,
    until: Option<String>,
    hours: Option<i64>,
}

async fn get_system_stats_history(
    State(state): State<AppState>,
    Query(q): Query<SystemStatsQuery>,
) -> Result<Json<Vec<SystemSnapshotRecord>>, ApiError> {
    let hours = q.hours.unwrap_or(24);
    let until = q.until.unwrap_or_else(|| Utc::now().to_rfc3339());
    let since = q.since.unwrap_or_else(|| {
        (Utc::now() - chrono::Duration::hours(hours) - chrono::Duration::minutes(2)).to_rfc3339()
    });
    let bucket_secs = if hours <= 1 {
        None
    } else if hours <= 6 {
        Some(60)
    } else if hours <= 24 {
        Some(300)
    } else {
        Some(3600)
    };
    let data = state
        .db
        .get_system_snapshots(&since, &until, bucket_secs)
        .await?;
    Ok(Json(data))
}

fn normalize_sign_in_settings(settings: &mut GlobalConfig) {
    for value in [
        &mut settings.lightpanda.endpoint,
        &mut settings.lightpanda.token,
        &mut settings.lightpanda.proxy,
        &mut settings.lightpanda.country,
        &mut settings.browserless.address,
        &mut settings.browserless.token,
    ] {
        *value = value
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
    }
    settings.lightpanda.region = settings.lightpanda.region.trim().to_ascii_lowercase();
    settings.lightpanda.browser = settings.lightpanda.browser.trim().to_string();
    if settings.lightpanda.browser.is_empty() {
        settings.lightpanda.browser = "lightpanda".to_string();
    }
}

fn validate_settings(settings: &GlobalConfig) -> Result<(), ApiError> {
    const ALLOWED_LOG_LEVELS: &[&str] = &["trace", "debug", "info", "warn", "error"];

    if let Some(log_level) = settings.log_level.as_deref() {
        if !ALLOWED_LOG_LEVELS.contains(&log_level) {
            return Err(ApiError::bad_request(
                "log_level must be one of: trace, debug, info, warn, error",
            ));
        }
    }
    if let Some(proxy) = settings.proxy.as_deref() {
        validate_proxy_scheme(proxy, "proxy")?;
    }
    if !matches!(settings.lightpanda.region.as_str(), "euwest" | "uswest") {
        return Err(ApiError::bad_request(
            "lightpanda.region 必须是 euwest 或 uswest",
        ));
    }
    if let Some(endpoint) = settings.lightpanda.endpoint.as_deref() {
        if !endpoint.starts_with("ws://") && !endpoint.starts_with("wss://") {
            return Err(ApiError::bad_request(
                "lightpanda.endpoint 必须以 ws:// 或 wss:// 开头",
            ));
        }
    }
    if let Some(address) = settings.browserless.address.as_deref() {
        let parsed = reqwest::Url::parse(address)
            .map_err(|_| ApiError::bad_request("browserless.address 不是有效地址"))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(ApiError::bad_request(
                "browserless.address 必须以 http:// 或 https:// 开头",
            ));
        }
    }
    Ok(())
}

fn validate_proxy_scheme(proxy: &str, field: &str) -> Result<(), ApiError> {
    let proxy = proxy.trim();
    if proxy.is_empty()
        || proxy.starts_with("http://")
        || proxy.starts_with("https://")
        || proxy.starts_with("socks5://")
        || proxy.starts_with("socks5h://")
    {
        return Ok(());
    }
    Err(ApiError::bad_request(format!(
        "{} must start with http://, https://, socks5://, or socks5h://",
        field
    )))
}

fn find_live_brush_torrent<'a>(
    record: &crate::brush::BrushTorrentRecord,
    live_torrents: &'a [crate::downloader::TorrentInfo],
) -> Option<&'a crate::downloader::TorrentInfo> {
    live_torrents
        .iter()
        .find(|torrent| torrent.hash.eq_ignore_ascii_case(&record.torrent_hash))
        .or_else(|| {
            live_torrents
                .iter()
                .find(|torrent| torrent.name == record.torrent_name)
        })
}

fn apply_live_torrent(
    record: &mut crate::brush::BrushTorrentRecord,
    live: &crate::downloader::TorrentInfo,
) {
    record.status = live.state.clone();
    record.remove_reason = None;
    record.removed_at = None;
    record.torrent_hash = live.hash.clone();
    record.uploaded_bytes = live.uploaded;
    record.downloaded_bytes = live.downloaded;
    record.download_duration_secs = live.time_active.max(0);
    record.avg_upload_speed = crate::brush::average_upload_speed(live.uploaded, live.time_active);
    record.ratio = crate::brush::calculate_ratio(live.uploaded, live.downloaded, live.ratio);
}

fn looks_like_info_hash(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn map_brush_trigger_error(message: String) -> ApiError {
    match message.as_str() {
        "任务不存在" => ApiError::not_found(message),
        "任务正在运行中" => ApiError::conflict(message),
        _ => ApiError::internal(message),
    }
}

fn map_sign_in_trigger_error(message: String) -> ApiError {
    match message.as_str() {
        "签到任务不存在" => ApiError::not_found(message),
        "签到任务正在运行中" => ApiError::conflict(message),
        _ => ApiError::internal(message),
    }
}

// ========== Tag Rules API ==========

async fn list_tag_rules(
    State(state): State<AppState>,
) -> Result<Json<Vec<crate::tag_rule::TagRuleRecord>>, ApiError> {
    Ok(Json(state.db.list_tag_rules().await?))
}

async fn list_tag_rule_trackers(
    State(state): State<AppState>,
) -> Result<Json<crate::tag_rule::TagRuleTrackerDiscovery>, ApiError> {
    state
        .tag_rule_scheduler
        .discover_trackers()
        .await
        .map(Json)
        .map_err(ApiError::internal)
}

async fn get_tag_rule(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<crate::tag_rule::TagRuleRecord>, ApiError> {
    state
        .db
        .get_tag_rule(id)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::not_found("标签规则不存在"))
}

async fn create_tag_rule(
    State(state): State<AppState>,
    Json(body): Json<crate::tag_rule::TagRuleRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    validate_tag_rule(&body)?;
    let id = state.db.create_tag_rule(&body).await?;
    Ok(Json(serde_json::json!({ "id": id })))
}

async fn update_tag_rule(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<crate::tag_rule::TagRuleRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    validate_tag_rule(&body)?;
    state.db.update_tag_rule(id, &body).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn delete_tag_rule(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state.db.delete_tag_rule(id).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn scan_tag_rules(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state
        .tag_rule_scheduler
        .run_once()
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

fn validate_tag_rule(req: &crate::tag_rule::TagRuleRequest) -> Result<(), ApiError> {
    if req.name.trim().is_empty() {
        return Err(ApiError::bad_request("名称不能为空"));
    }
    if req.tag_name.trim().is_empty() {
        return Err(ApiError::bad_request("标签名不能为空"));
    }
    if req.match_rules.is_empty() {
        return Err(ApiError::bad_request("匹配规则不能为空"));
    }
    for (i, rule) in req.match_rules.iter().enumerate() {
        match rule.match_type.as_str() {
            "prefix" | "suffix" | "contains" | "exact" | "regex" => {}
            other => {
                return Err(ApiError::bad_request(format!(
                    "第{}条规则的匹配类型无效: {}，支持: prefix, suffix, contains, exact, regex",
                    i + 1,
                    other
                )));
            }
        }
        if rule.pattern.is_empty() {
            return Err(ApiError::bad_request(format!(
                "第{}条规则的匹配模式不能为空",
                i + 1
            )));
        }
    }
    Ok(())
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: message.into(),
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: message.into(),
        }
    }

    fn conflict(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            message: message.into(),
        }
    }

    fn bad_gateway(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            message: message.into(),
        }
    }
}

impl From<AppError> for ApiError {
    fn from(value: AppError) -> Self {
        match value {
            AppError::InvalidConfig { message } | AppError::Database { message } => Self {
                status: StatusCode::BAD_REQUEST,
                message,
            },
            other => Self {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                message: other.to_string(),
            },
        }
    }
}

impl From<MediaServiceError> for ApiError {
    fn from(value: MediaServiceError) -> Self {
        match value {
            MediaServiceError::App(error) => media_app_error(error),
            MediaServiceError::Tmdb(error) => match error {
                TmdbError::MissingToken => Self::bad_request("TMDB token is not configured"),
                TmdbError::InvalidMediaType(_) => {
                    Self::bad_request("media_type must be multi, tv, or movie")
                }
                TmdbError::Http { status, .. } if status.as_u16() == 404 => {
                    Self::not_found("TMDB media not found")
                }
                TmdbError::Http { status, .. } if status.as_u16() == 400 => {
                    Self::bad_request("TMDB rejected the request")
                }
                TmdbError::Http { status, .. } if matches!(status.as_u16(), 401 | 403) => {
                    Self::bad_gateway("TMDB authentication failed")
                }
                TmdbError::Http { status, .. } => {
                    Self::bad_gateway(format!("TMDB request failed with HTTP {}", status.as_u16()))
                }
                TmdbError::Transport(_) | TmdbError::Parse(_) => {
                    Self::bad_gateway("TMDB request failed")
                }
            },
            MediaServiceError::Progression(error) => Self::bad_request(error.to_string()),
            MediaServiceError::Indexer(error) => match error {
                IndexerError::Configuration(_) => {
                    Self::bad_gateway("PT indexer configuration is invalid")
                }
                IndexerError::AuthenticationExpired(_) => {
                    Self::bad_gateway("PT site authentication failed")
                }
                IndexerError::RateLimited(_) => Self::bad_gateway("PT site rate limit reached"),
                IndexerError::Http(_)
                | IndexerError::Api(_)
                | IndexerError::Parse(_)
                | IndexerError::UnsafeUrl(_)
                | IndexerError::InvalidTorrent(_) => Self::bad_gateway("PT site request failed"),
            },
            MediaServiceError::Torrent(_) => {
                Self::bad_gateway("PT site returned an invalid torrent")
            }
            MediaServiceError::NotFound(message) => Self::not_found(message),
            MediaServiceError::Conflict(message) => Self::conflict(message),
            MediaServiceError::Invalid(message) => {
                if message.starts_with("failed to create HTTP client") {
                    Self::bad_request("HTTP client configuration is invalid")
                } else {
                    Self::bad_request(message)
                }
            }
            MediaServiceError::Serialization(_) => Self::internal("failed to serialize media data"),
            MediaServiceError::Downloader(_) => Self::bad_gateway("downloader request failed"),
        }
    }
}

fn media_app_error(error: AppError) -> ApiError {
    match error {
        AppError::InvalidConfig { message }
            if message.contains("changed; reload")
                || message.contains("while relocation jobs are active")
                || message.contains("while download work is active")
                || message.contains("reset_download_history") =>
        {
            ApiError::conflict(message)
        }
        AppError::InvalidConfig { message } => ApiError::bad_request(message),
        AppError::Database { message } if message.contains("UNIQUE constraint failed") => {
            ApiError::conflict("resource already exists")
        }
        AppError::Database { message } if message.contains("FOREIGN KEY constraint failed") => {
            ApiError::conflict("resource is still in use or references missing data")
        }
        AppError::Database { message }
            if message.contains("CHECK constraint failed")
                || message.contains("NOT NULL constraint failed") =>
        {
            ApiError::bad_request("request violates a data constraint")
        }
        AppError::Database { message }
            if message.contains("database is locked") || message.contains("database is busy") =>
        {
            ApiError::conflict("database is busy; retry the request")
        }
        AppError::Database { .. } => ApiError::internal("media database operation failed"),
        _ => ApiError::internal("media operation failed"),
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(serde_json::json!({ "error": self.message })),
        )
            .into_response()
    }
}

#[cfg(test)]
mod media_api_tests {
    use super::*;

    #[test]
    fn tmdb_transfer_plans_require_matching_paths_under_the_selected_roots() {
        let target = OpenListTargetDirectory {
            id: Some(7),
            name: "archive".to_string(),
            downloader_id: 2,
            openlist_path: "/media".to_string(),
            qb_path: "/data/media".to_string(),
        };
        let hashes = vec!["0123456789abcdef0123456789abcdef01234567".to_string()];
        let valid = vec![OpenListTransferPlannedTarget {
            hash: hashes[0].clone(),
            target_openlist_path: "/media/云母/电影/剧情/2024".to_string(),
            target_qb_path: "/data/media/云母/电影/剧情/2024".to_string(),
        }];
        assert!(validate_planned_transfer_targets(&hashes, &valid, &target).is_ok());

        let escaped = vec![OpenListTransferPlannedTarget {
            hash: hashes[0].clone(),
            target_openlist_path: "/other/云母/电影/剧情/2024".to_string(),
            target_qb_path: "/data/media/云母/电影/剧情/2024".to_string(),
        }];
        assert!(validate_planned_transfer_targets(&hashes, &escaped, &target).is_err());

        let mismatched = vec![OpenListTransferPlannedTarget {
            hash: hashes[0].clone(),
            target_openlist_path: "/media/云母/电影/剧情/2024".to_string(),
            target_qb_path: "/data/media/other".to_string(),
        }];
        assert!(validate_planned_transfer_targets(&hashes, &mismatched, &target).is_err());
    }

    #[test]
    fn tmdb_transfer_directory_components_are_safe_and_bounded() {
        assert_eq!(
            safe_transfer_component("  剧情/../动作  ", "其他"),
            "剧情_.._动作"
        );
        assert_eq!(safe_transfer_component("...", "其他"), "其他");
        assert!(
            safe_transfer_component(&"a".repeat(100), "其他")
                .chars()
                .count()
                <= 80
        );
        assert!(is_generated_transfer_suffix("云母/电影/剧情/2024"));
        assert!(is_generated_transfer_suffix("云母/电视剧/其他/年份未知"));
        assert!(!is_generated_transfer_suffix("云母/电影/剧情/2024/extra"));
        assert!(!is_generated_transfer_suffix("云母/未知/剧情/2024"));
        assert!(!is_generated_transfer_suffix(&format!(
            "云母/电影/{}/2024",
            "a".repeat(81)
        )));
        let movie = ReleaseParser::default()
            .parse("Example.Movie.2024.1080p.WEB-DL")
            .unwrap();
        assert_eq!(
            fallback_transfer_category(Some(&movie), Some("movie")),
            "电影"
        );
        assert_eq!(normalize_transfer_mode(None).unwrap(), "fixed");
        assert_eq!(normalize_transfer_mode(Some("auto_tmdb")).unwrap(), "tmdb");
        assert!(normalize_transfer_mode(Some("unknown")).is_err());
    }

    #[test]
    fn openlist_task_ids_accept_current_and_legacy_formats() {
        assert_eq!(
            decode_openlist_job_task_ids(Some("[\"task-a\",\"task-b\"]")),
            vec!["task-a".to_string(), "task-b".to_string()]
        );
        assert_eq!(
            decode_openlist_job_task_ids(Some("legacy-task")),
            vec!["legacy-task".to_string()]
        );
    }

    #[test]
    fn openlist_connection_guard_requires_idle_for_any_credential_change() {
        assert!(!openlist_connection_change_requires_idle(
            "https://openlist.example",
            "old-key",
            "https://openlist.example",
            "old-key",
        ));
        assert!(openlist_connection_change_requires_idle(
            "https://openlist.example",
            "old-key",
            "https://openlist.example",
            "new-key",
        ));
        assert!(openlist_connection_change_requires_idle(
            "https://openlist.example",
            "old-key",
            "https://openlist.example",
            "",
        ));
        assert!(openlist_connection_change_requires_idle(
            "https://openlist.example",
            "old-key",
            "https://other.example",
            "old-key",
        ));
    }

    #[test]
    fn openlist_config_rejects_case_only_remote_path_overlap() {
        let mut payload = UpdateOpenListConfigRequest {
            address: "https://openlist.example".to_string(),
            api_key: Some("test-key".to_string()),
            updated_at: String::new(),
            clear_api_key: false,
            enabled: true,
            target_directory_id: None,
            selected_target_index: Some(0),
            scan_interval_mins: 1,
            source_mappings: vec![OpenListPathMapping {
                id: None,
                downloader_id: 1,
                qb_path: "/downloads/source".to_string(),
                openlist_path: "/Media".to_string(),
            }],
            target_directories: vec![OpenListTargetDirectory {
                id: None,
                name: "archive".to_string(),
                downloader_id: 1,
                openlist_path: "/media/archive".to_string(),
                qb_path: "/downloads/archive".to_string(),
            }],
        };

        assert!(normalize_and_validate_openlist_config(&mut payload).is_err());
        assert!(openlist_paths_overlap("/Archive", "/archive/show"));
        assert!(openlist_paths_overlap("/Ärchive", "/ärchive/show"));
        assert!(openlist_paths_overlap("/Café", "/Cafe\u{301}/show"));
        assert!(
            !paths_overlap("/Downloads/Archive", "/downloads/archive"),
            "qB paths keep their existing case-sensitive semantics"
        );
    }

    #[tokio::test]
    async fn openlist_settings_reject_a_stale_or_missing_client_version() {
        let (_temp, state) = test_media_state().await;
        let current = state
            .service
            .database()
            .get_openlist_config()
            .await
            .unwrap();
        let request = |updated_at: String, scan_interval_mins| UpdateOpenListConfigRequest {
            address: String::new(),
            api_key: None,
            updated_at,
            clear_api_key: false,
            enabled: false,
            target_directory_id: None,
            selected_target_index: None,
            scan_interval_mins,
            source_mappings: Vec::new(),
            target_directories: Vec::new(),
        };
        let first = update_openlist_config(
            State(state.clone()),
            Json(request(current.updated_at.clone(), 2)),
        )
        .await
        .unwrap()
        .0;
        assert_ne!(first.updated_at, current.updated_at);

        let stale =
            update_openlist_config(State(state.clone()), Json(request(current.updated_at, 3)))
                .await
                .unwrap_err();
        assert_eq!(stale.status, StatusCode::CONFLICT);

        let missing = update_openlist_config(State(state), Json(request(String::new(), 4)))
            .await
            .unwrap_err();
        assert_eq!(missing.status, StatusCode::CONFLICT);
    }

    #[test]
    fn openlist_cancel_verification_prioritizes_any_proven_active_task() {
        assert!(manual_resolution_openlist_client("", "").is_err());

        let mut task = OpenListTask {
            id: "task-a".to_string(),
            name: String::new(),
            state: 0,
            status: String::new(),
            progress: 0.0,
            total_bytes: 0,
            error: String::new(),
        };
        assert_eq!(
            summarize_openlist_cancel_observations(vec![
                ("task-a".to_string(), Err("request timed out".to_string()),),
                ("task-b".to_string(), Ok(Some(task.clone()))),
            ]),
            OpenListCancelVerification::Active(vec!["task-b".to_string()]),
            "an earlier unknown result must not hide a later active task"
        );

        assert!(
            require_safe_openlist_cancel(OpenListCancelVerification::Unknown(vec![
                "task-a: request timed out".to_string(),
            ]))
            .is_err(),
            "explicit confirmation must never release a lock while task state is unknown"
        );
        assert!(
            require_safe_openlist_cancel(OpenListCancelVerification::Active(vec![
                "task-a".to_string(),
            ]))
            .is_err()
        );
        assert!(require_safe_openlist_cancel(OpenListCancelVerification::ProvenSafe).is_ok());

        task.state = 2;
        assert_eq!(
            summarize_openlist_cancel_observations(vec![(
                "task-a".to_string(),
                Ok(Some(task.clone())),
            )]),
            OpenListCancelVerification::ProvenSafe
        );
        task.state = 5;
        task.error = "temporary failure before retry".to_string();
        assert_eq!(
            summarize_openlist_cancel_observations(vec![(
                "task-a".to_string(),
                Ok(Some(task.clone())),
            )]),
            OpenListCancelVerification::Active(vec!["task-a".to_string()])
        );
        task.state = 7;
        assert_eq!(
            summarize_openlist_cancel_observations(vec![("task-a".to_string(), Ok(Some(task)),)]),
            OpenListCancelVerification::ProvenSafe
        );
    }

    #[test]
    fn openlist_cancel_verification_accepts_tasks_proven_missing() {
        let mut terminal_task = OpenListTask {
            id: "task-terminal".to_string(),
            name: String::new(),
            state: 2,
            status: String::new(),
            progress: 0.0,
            total_bytes: 0,
            error: String::new(),
        };

        assert_eq!(
            summarize_openlist_cancel_observations(vec![("task-missing".to_string(), Ok(None))]),
            OpenListCancelVerification::ProvenSafe
        );
        assert_eq!(
            summarize_openlist_cancel_observations(vec![
                ("task-missing".to_string(), Ok(None)),
                ("task-terminal".to_string(), Ok(Some(terminal_task.clone())),),
            ]),
            OpenListCancelVerification::ProvenSafe
        );
        assert_eq!(
            summarize_openlist_cancel_observations(vec![
                ("task-missing".to_string(), Ok(None)),
                (
                    "task-unknown".to_string(),
                    Err("request timed out".to_string())
                ),
            ]),
            OpenListCancelVerification::Unknown(vec![
                "task-unknown: request timed out".to_string()
            ])
        );

        terminal_task.state = 0;
        assert_eq!(
            summarize_openlist_cancel_observations(vec![
                ("task-missing".to_string(), Ok(None)),
                ("task-active".to_string(), Ok(Some(terminal_task))),
            ]),
            OpenListCancelVerification::Active(vec!["task-active".to_string()])
        );
    }

    fn subscription_with_status(status: Option<&str>) -> SubscriptionRecord {
        SubscriptionRecord {
            id: 1,
            tmdb_id: 42,
            media_type: "tv".to_string(),
            tmdb_is_animation: false,
            tmdb_genres: Vec::new(),
            title: "Example Show".to_string(),
            original_title: None,
            aliases: Vec::new(),
            year: Some(2026),
            poster_path: None,
            season: Some(1),
            next_episode: Some(3),
            start_episode: Some(3),
            absolute_episode: None,
            quality_profile_id: 1,
            downloader_id: 1,
            site_ids: vec![1],
            save_path: None,
            enabled: true,
            next_run_at: String::new(),
            lease_owner: None,
            lease_until: None,
            version: 0,
            last_status: status.map(str::to_string),
            last_error: None,
            last_run_at: None,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    async fn test_media_state() -> (tempfile::TempDir, MediaApiState) {
        let temp = tempfile::tempdir().unwrap();
        let db = Database::open(temp.path()).await.unwrap();
        let downloaders = DownloaderClientPool::new(db.clone());
        let relocation_scheduler = RelocationScheduler::new(db.clone(), downloaders.clone(), false);
        let service = MediaService::new(db, downloaders);
        let scheduler = MediaScheduler::new(service.clone());
        (
            temp,
            MediaApiState {
                service,
                scheduler,
                relocation_scheduler,
            },
        )
    }

    #[tokio::test]
    async fn confirmed_cancel_releases_lock_when_openlist_task_is_proven_missing() {
        let app = Router::new().route(
            "/api/task/copy/info",
            post(|| async {
                Json(serde_json::json!({
                    "code": 404,
                    "message": "task not found",
                    "data": null
                }))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let (temp, state) = test_media_state().await;
        let db = state.service.database();
        let mut config = db.get_openlist_config().await.unwrap();
        config.base_url = format!("http://{address}");
        config.api_key = "test-key".to_string();
        db.update_openlist_config(&config).await.unwrap();

        let conn = rusqlite::Connection::open(temp.path().join("kirara.db")).unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        let checkpoint = r#"{"path":"episode.mkv","size":10,"operation":"copy_file","phase":"uncertain","submitted_at":"2026-01-01T00:00:00Z"}"#;
        conn.execute(
            "INSERT INTO media_relocation_jobs
             (infohash, source_qb_path, source_openlist_path, target_openlist_path,
              target_qb_path, torrent_name, stage, openlist_task_id,
              copy_checkpoint_json, copy_lock_acquired, manifest_cursor, last_error,
              stage_started_at, created_at, updated_at)
             VALUES ('0123456789abcdef0123456789abcdef01234567', '/source', '/source',
                     '/target', '/target', 'missing task', 'copy_manual_review',
                     'missing-task', ?, 1, 4, 'ambiguous response', ?, ?, ?)",
            rusqlite::params![checkpoint, now, now, now],
        )
        .unwrap();
        let job_id = conn.last_insert_rowid();
        drop(conn);

        let current = db.get_media_relocation_job(job_id).await.unwrap().unwrap();
        let unconfirmed = resolve_openlist_copy(
            State(state.clone()),
            Path(job_id),
            Json(ResolveOpenListCopyRequest {
                resolution: "cancel".to_string(),
                expected_version: current.version,
                confirm_task_terminated: false,
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(unconfirmed.status, StatusCode::BAD_REQUEST);
        let unchanged = db.get_media_relocation_job(job_id).await.unwrap().unwrap();
        assert_eq!(unchanged.version, current.version);
        assert!(unchanged.copy_lock_acquired);
        assert_eq!(unchanged.openlist_task_id.as_deref(), Some("missing-task"));
        assert_eq!(unchanged.copy_checkpoint_json.as_deref(), Some(checkpoint));

        let Json(response) = resolve_openlist_copy(
            State(state.clone()),
            Path(job_id),
            Json(ResolveOpenListCopyRequest {
                resolution: "cancel".to_string(),
                expected_version: unchanged.version,
                confirm_task_terminated: true,
            }),
        )
        .await
        .unwrap();
        assert_eq!(response.stage, "cancelled");
        assert!(!response.copy_lock_acquired);
        assert!(response.openlist_task_ids.is_empty());
        assert!(response.copy_checkpoint.is_none());

        let cancelled = db.get_media_relocation_job(job_id).await.unwrap().unwrap();
        assert_eq!(cancelled.stage, "cancelled");
        assert_eq!(cancelled.openlist_task_id, None);
        assert_eq!(cancelled.copy_checkpoint_json, None);
        assert!(!cancelled.copy_lock_acquired);
        assert_eq!(cancelled.manifest_cursor, 0);
        assert_eq!(cancelled.last_error, None);
        server.abort();
    }

    #[tokio::test]
    async fn automatic_openlist_jobs_handler_reaches_records_past_legacy_200_limit() {
        let (temp, state) = test_media_state().await;
        let conn = rusqlite::Connection::open(temp.path().join("kirara.db")).unwrap();
        conn.execute_batch("PRAGMA foreign_keys=OFF;").unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        let mut oldest_review_id = 0;
        for media_download_id in 1..=205_i64 {
            conn.execute(
                "INSERT INTO media_relocation_jobs
                 (media_download_id, infohash, source_qb_path, source_openlist_path,
                  target_openlist_path, target_qb_path, torrent_name, stage,
                  stage_started_at, created_at, updated_at)
                 VALUES (?, ?, '', '', '', '', ?, 'copy_manual_review', ?, ?, ?)",
                rusqlite::params![
                    media_download_id,
                    format!("{media_download_id:040x}"),
                    format!("automatic-review-{media_download_id}"),
                    now,
                    now,
                    now,
                ],
            )
            .unwrap();
            if media_download_id == 1 {
                oldest_review_id = conn.last_insert_rowid();
            }
        }
        drop(conn);

        let Json(response) = list_openlist_jobs(
            State(state),
            Query(OpenListJobsQuery {
                page: Some(3),
                page_size: Some(100),
            }),
        )
        .await
        .unwrap();
        assert_eq!(response.page, 3);
        assert_eq!(response.page_size, 100);
        assert_eq!(response.total, 205);
        assert_eq!(response.records.len(), 5);
        assert!(
            response
                .records
                .iter()
                .any(|record| record.id == oldest_review_id)
        );
    }

    #[tokio::test]
    async fn settings_handler_never_returns_or_accidentally_clears_token() {
        let (_temp, state) = test_media_state().await;
        let current = state.service.database().get_media_settings().await.unwrap();
        state
            .service
            .database()
            .update_media_settings(&MediaSettings {
                tmdb_token: Some("super-secret-token".to_string()),
                ..current
            })
            .await
            .unwrap();

        let Json(response) = get_media_settings(State(state.clone())).await.unwrap();
        assert!(response.tmdb_token.is_none());
        assert!(response.tmdb_token_configured);
        assert!(
            !serde_json::to_string(&response)
                .unwrap()
                .contains("super-secret-token")
        );

        let Json(saved) = update_media_settings(
            State(state.clone()),
            Json(UpdateMediaSettingsRequest {
                tmdb_token: None,
                clear_tmdb_token: false,
                tmdb_language: "zh-CN".to_string(),
                scan_interval_mins: 45,
                max_search_queries: 6,
                search_concurrency: 3,
            }),
        )
        .await
        .unwrap();
        assert!(saved.tmdb_token.is_none());
        assert!(saved.tmdb_token_configured);
        assert_eq!(
            state
                .service
                .database()
                .get_media_settings()
                .await
                .unwrap()
                .tmdb_token
                .as_deref(),
            Some("super-secret-token")
        );
    }

    #[test]
    fn media_service_errors_map_to_stable_http_classes_without_secrets() {
        let not_found = ApiError::from(MediaServiceError::NotFound("download 9".to_string()));
        assert_eq!(not_found.status, StatusCode::NOT_FOUND);

        let conflict = ApiError::from(MediaServiceError::Conflict("leased".to_string()));
        assert_eq!(conflict.status, StatusCode::CONFLICT);

        let upstream = ApiError::from(MediaServiceError::Indexer(
            IndexerError::AuthenticationExpired("cookie=secret".to_string()),
        ));
        assert_eq!(upstream.status, StatusCode::BAD_GATEWAY);
        assert!(!upstream.message.contains("secret"));

        let client = ApiError::from(MediaServiceError::Invalid(
            "failed to create HTTP client: proxy http://user:secret@example.test".to_string(),
        ));
        assert_eq!(client.status, StatusCode::BAD_REQUEST);
        assert!(!client.message.contains("secret"));

        let database = media_app_error(AppError::Database {
            message: "unexpected database detail at /private/path".to_string(),
        });
        assert_eq!(database.status, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(!database.message.contains("private"));
    }

    #[test]
    fn download_filters_validate_status_and_pagination_bounds() {
        assert_eq!(
            validate_download_query(Some(" CANCELLED "), Some(2), Some(50)).unwrap(),
            (Some("cancelled".to_string()), 50, 50)
        );
        assert!(validate_download_query(Some("unknown"), None, None).is_err());
        assert!(validate_download_query(None, Some(0), Some(10)).is_err());
        assert!(validate_download_query(None, Some(1), Some(201)).is_err());
        assert!(validate_download_query(None, Some(usize::MAX), Some(200)).is_err());
        assert_eq!(validate_download_cursor(Some(42), None).unwrap(), Some(42));
        assert!(validate_download_cursor(Some(0), None).is_err());
        assert!(validate_download_cursor(Some(42), Some(1)).is_err());
    }

    #[tokio::test]
    async fn download_delete_endpoint_is_versioned_and_reports_the_external_data_boundary() {
        let (_temp, state) = test_media_state().await;
        let queued = state
            .service
            .database()
            .enqueue_media_download(&crate::media::models::NewMediaDownload {
                subscription_id: None,
                target_key: "manual:test:delete".to_string(),
                dedupe_key: "web-delete-history".to_string(),
                site_id: None,
                downloader_id: None,
                source_site: "test".to_string(),
                downloader_name: "removed downloader".to_string(),
                torrent_id: "delete-history".to_string(),
                title: "Delete.History.Test.1080p.WEB-DL".to_string(),
                size: 1,
                release_json: "{}".to_string(),
                decision_json: "{}".to_string(),
                profile_snapshot_json: "{}".to_string(),
            })
            .await
            .unwrap();
        let claimed = state
            .service
            .database()
            .claim_due_media_downloads("web-delete-worker", 60, 1)
            .await
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(claimed.id, queued.id);
        assert!(
            state
                .service
                .database()
                .transition_media_download(
                    claimed.id,
                    claimed.version,
                    "web-delete-worker",
                    "fetching",
                    "cancelled",
                    None,
                    Some("cancelled for test"),
                    None,
                )
                .await
                .unwrap()
        );
        let cancelled = state
            .service
            .database()
            .get_media_download(queued.id)
            .await
            .unwrap()
            .unwrap();

        let stale = delete_media_download(
            State(state.clone()),
            Path(cancelled.id),
            Query(DeleteMediaDownloadQuery {
                version: cancelled.version - 1,
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(stale.status, StatusCode::CONFLICT);

        let Json(response) = delete_media_download(
            State(state.clone()),
            Path(cancelled.id),
            Query(DeleteMediaDownloadQuery {
                version: cancelled.version,
            }),
        )
        .await
        .unwrap();
        assert_eq!(response.deletion.deleted_id, cancelled.id);
        assert!(!response.deletion.target_reopened);
        assert!(!response.qb_torrent_deleted);
        assert!(!response.openlist_data_deleted);
        assert!(
            state
                .service
                .database()
                .get_media_download(cancelled.id)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn failed_download_reconciliation_rejects_an_invalid_version_before_qb_access() {
        let (_temp, state) = test_media_state().await;

        let error = reconcile_failed_media_download(
            State(state),
            Path(1),
            Query(DeleteMediaDownloadQuery { version: -1 }),
        )
        .await
        .unwrap_err();

        assert_eq!(error.status, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn completed_subscriptions_reject_run_and_resume() {
        let completed = subscription_with_status(Some("completed"));
        let run = reject_completed_subscription(&completed, "run").unwrap_err();
        let resume = reject_completed_subscription(&completed, "resume").unwrap_err();
        assert_eq!(run.status, StatusCode::CONFLICT);
        assert_eq!(resume.status, StatusCode::CONFLICT);
        assert!(run.message.contains("new cursor"));

        assert!(
            reject_completed_subscription(&subscription_with_status(Some("waiting")), "run")
                .is_ok()
        );
    }

    #[tokio::test]
    async fn download_queue_accepts_only_well_formed_candidate_ids() {
        let (_temp, state) = test_media_state().await;
        let mut valid = QueueDownloadRequest {
            candidate_id: format!("cand_{}", "a".repeat(48)),
            quality_profile_id: 1,
            downloader_id: 1,
            subscription_id: None,
            override_reason: Some(" operator checked ".to_string()),
        };
        normalize_and_validate_download(&state, &mut valid)
            .await
            .unwrap();
        assert_eq!(valid.override_reason.as_deref(), Some("operator checked"));

        valid.candidate_id = "cand_predictable".to_string();
        assert!(
            normalize_and_validate_download(&state, &mut valid)
                .await
                .is_err()
        );
    }

    #[test]
    fn media_router_builds_with_all_route_groups() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let (_temp, state) = runtime.block_on(test_media_state());
        let _router = media_router(
            state.service,
            state.scheduler,
            state.relocation_scheduler,
            false,
        );
    }
}

#[cfg(test)]
mod security_boundary_tests {
    use super::*;

    #[test]
    fn site_and_downloader_responses_never_serialize_credentials() {
        let site = SiteResponse::from(SiteWithStats {
            id: 1,
            name: "example".to_string(),
            site_type: "nexusphp".to_string(),
            base_url: "https://tracker.example".to_string(),
            auth_config: r#"{"auth_type":"cookie","cookie":"dummy-site-secret"}"#.to_string(),
            request_headers: r#"[{"name":"X-Private-Token","value":"dummy-header-secret"}]"#
                .to_string(),
            use_proxy: true,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            stats: None,
        });
        let site_json = serde_json::to_value(site).unwrap();
        assert_eq!(site_json["auth_type"], "cookie");
        assert_eq!(site_json["auth_configured"], true);
        assert!(site_json.get("auth_config").is_none());
        assert!(site_json.get("request_headers").is_none());
        assert!(!site_json.to_string().contains("dummy-site-secret"));
        assert!(!site_json.to_string().contains("dummy-header-secret"));

        let downloader = DownloaderResponse::from(crate::downloader::DownloaderRecord {
            id: 2,
            name: "qBittorrent".to_string(),
            downloader_type: "qbittorrent".to_string(),
            url: "http://127.0.0.1:8080".to_string(),
            username: "operator".to_string(),
            password: "dummy-downloader-secret".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
        });
        let downloader_json = serde_json::to_value(downloader).unwrap();
        assert_eq!(downloader_json["password_configured"], true);
        assert!(downloader_json.get("password").is_none());
        assert!(
            !downloader_json
                .to_string()
                .contains("dummy-downloader-secret")
        );

        let ptd = PtdBackupConfigResponse::new(
            crate::db::PtdBackupConfig {
                enabled: true,
                webdav_url: "https://dav.example/ptd/".to_string(),
                username: "operator".to_string(),
                password: "dummy-webdav-secret".to_string(),
                use_proxy: false,
                backup_interval_hours: 24,
                last_backup_at: None,
                last_backup_filename: None,
                last_error: None,
                updated_at: "2026-01-01T00:00:00Z".to_string(),
            },
            &[],
        );
        let ptd_json = serde_json::to_value(ptd).unwrap();
        assert_eq!(ptd_json["password_configured"], true);
        assert!(ptd_json.get("password").is_none());
        assert!(!ptd_json.to_string().contains("dummy-webdav-secret"));
    }

    #[test]
    fn site_credentials_response_only_includes_matching_auth_fields() {
        let credentials = SiteCredentialsResponse::from(SiteAuth::CookiePasskey {
            cookie: "dummy-cookie".to_string(),
            passkey: "dummy-passkey".to_string(),
        });
        let json = serde_json::to_value(credentials).unwrap();

        assert_eq!(json["auth_type"], "cookie_passkey");
        assert_eq!(json["cookie"], "dummy-cookie");
        assert_eq!(json["passkey"], "dummy-passkey");
        assert!(json["api_key"].is_null());
    }

    #[test]
    fn site_update_preserves_blank_credentials_and_requires_explicit_clear() {
        let existing = crate::site::SiteRecord {
            id: 1,
            name: "example".to_string(),
            site_type: "nexusphp".to_string(),
            base_url: "https://tracker.example".to_string(),
            auth_config: r#"{"auth_type":"cookie","cookie":"dummy-site-secret"}"#.to_string(),
            request_headers: "[]".to_string(),
            use_proxy: true,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
        };

        let preserved = resolve_site_auth_update(
            &existing,
            "nexusphp",
            Some(serde_json::json!({ "auth_type": "cookie", "cookie": "" })),
            false,
        )
        .unwrap();
        assert!(matches!(
            preserved,
            SiteAuth::Cookie { ref cookie } if cookie == "dummy-site-secret"
        ));

        let cleared = resolve_site_auth_update(
            &existing,
            "nexusphp",
            Some(serde_json::json!({ "auth_type": "cookie", "cookie": "" })),
            true,
        )
        .unwrap();
        assert!(matches!(cleared, SiteAuth::Cookie { ref cookie } if cookie.is_empty()));

        assert!(
            resolve_site_auth_update(
                &existing,
                "nexusphp",
                Some(serde_json::json!({ "auth_type": "api_key", "api_key": "" })),
                false,
            )
            .is_err()
        );
        assert!(
            resolve_site_auth_update(
                &existing,
                "mteam",
                Some(serde_json::json!({
                    "auth_type": "api_key",
                    "api_key": "dummy-new-secret"
                })),
                false,
            )
            .is_ok()
        );
    }

    #[test]
    fn ptd_backup_update_preserves_or_explicitly_clears_password() {
        fn current() -> crate::db::PtdBackupConfig {
            crate::db::PtdBackupConfig {
                enabled: false,
                webdav_url: "https://dav.example/ptd/".to_string(),
                username: "alice".to_string(),
                password: "saved-secret".to_string(),
                use_proxy: false,
                backup_interval_hours: 24,
                last_backup_at: None,
                last_backup_filename: None,
                last_error: None,
                updated_at: String::new(),
            }
        }

        let request = |clear_password| UpdatePtdBackupConfigRequest {
            enabled: false,
            webdav_url: "https://dav.example/ptd/".to_string(),
            username: "alice".to_string(),
            password: Some(String::new()),
            clear_password,
            use_proxy: false,
            backup_interval_hours: 24,
        };
        assert_eq!(
            resolve_ptd_backup_config_update(current(), request(false))
                .unwrap()
                .password,
            "saved-secret"
        );
        assert!(
            resolve_ptd_backup_config_update(current(), request(true))
                .unwrap()
                .password
                .is_empty()
        );
    }

    #[test]
    fn ptd_backup_rejects_user_supplied_site_identifiers() {
        let request = serde_json::json!({
            "enabled": false,
            "webdav_url": "https://dav.example/ptd/",
            "username": "alice",
            "backup_interval_hours": 24,
            "site_mappings": { "1": "madeup" }
        });
        assert!(serde_json::from_value::<UpdatePtdBackupConfigRequest>(request).is_err());
    }

    #[test]
    fn downloader_copy_reuses_password_unless_a_replacement_is_supplied() {
        assert_eq!(
            resolve_created_downloader_password(
                "dummy-downloader-secret".to_string(),
                Some(String::new()),
            ),
            "dummy-downloader-secret"
        );
        assert_eq!(
            resolve_created_downloader_password(
                "dummy-downloader-secret".to_string(),
                Some("dummy-new-secret".to_string()),
            ),
            "dummy-new-secret"
        );
        assert_eq!(resolve_created_downloader_password(String::new(), None), "");
    }

    #[tokio::test]
    async fn downloader_update_keeps_password_until_clear_is_explicit() {
        let temp = tempfile::tempdir().unwrap();
        let db = Database::open(temp.path()).await.unwrap();
        let id = db
            .create_downloader(
                "qBittorrent",
                "qbittorrent",
                "http://127.0.0.1:8080",
                "operator",
                "dummy-downloader-secret",
            )
            .await
            .unwrap();
        let existing = db.get_downloader(id).await.unwrap().unwrap();

        let preserved =
            resolve_downloader_password(&existing.password, Some(String::new()), false).unwrap();
        db.update_downloader(
            id,
            &existing.name,
            &existing.downloader_type,
            &existing.url,
            &existing.username,
            &preserved,
        )
        .await
        .unwrap();
        assert_eq!(
            db.get_downloader(id).await.unwrap().unwrap().password,
            "dummy-downloader-secret"
        );

        let cleared = resolve_downloader_password(&preserved, None, true).unwrap();
        db.update_downloader(
            id,
            &existing.name,
            &existing.downloader_type,
            &existing.url,
            &existing.username,
            &cleared,
        )
        .await
        .unwrap();
        assert!(
            db.get_downloader(id)
                .await
                .unwrap()
                .unwrap()
                .password
                .is_empty()
        );
    }

    #[tokio::test]
    async fn manual_review_responses_expose_only_safe_resolution_actions() {
        let temp = tempfile::tempdir().unwrap();
        let db = Database::open(temp.path()).await.unwrap();
        let downloader_id = db
            .create_downloader("qB", "qbittorrent", "http://127.0.0.1:8080", "", "")
            .await
            .unwrap();
        db.enqueue_manual_media_relocation_jobs(
            downloader_id,
            downloader_id,
            "/archive",
            "/archive",
            &[(
                "0123456789012345678901234567890123456789".to_string(),
                "planning review".to_string(),
            )],
        )
        .await
        .unwrap();
        let (mut jobs, _) = db.list_manual_media_relocation_jobs(1, 10).await.unwrap();
        let mut job = jobs.pop().unwrap();
        job.stage = "planning_manual_review".to_string();
        job.media_download_id = Some(1);
        job.manual_requested_at = None;
        let response = OpenListJobResponse::from(job.clone());

        assert!(response.manual_resolution_allowed);
        assert_eq!(response.copy_resolution_actions, ["recheck", "cancel"]);
        assert!(!response.copy_resolution_actions.contains(&"force_retry"));

        job.stage = "copy_manual_review".to_string();
        job.copy_checkpoint_json = Some(
            r#"{"path":"episode.mkv","size":10,"operation":"copy_file","phase":"uncertain","submitted_at":"2026-01-01T00:00:00Z","terminal_failure_verified":true}"#
                .to_string(),
        );
        job.openlist_task_id = Some("task-failed".to_string());
        assert_eq!(copy_resolution_actions(&job), ["recheck", "cancel"]);

        job.stage = "manifest_required".to_string();
        job.media_download_id = None;
        job.copy_checkpoint_json = None;
        job.openlist_task_id = None;
        assert_eq!(copy_resolution_actions(&job), ["recheck", "cancel"]);
        assert_eq!(
            verify_openlist_tasks_for_cancel(&db, &job).await.unwrap(),
            OpenListCancelVerification::ProvenSafe
        );

        job.copy_checkpoint_json = Some(
            r#"{"path":"episode.mkv","size":10,"operation":"remove_file","phase":"uncertain","submitted_at":"2026-01-01T00:00:00Z"}"#
                .to_string(),
        );
        assert!(matches!(
            verify_openlist_tasks_for_cancel(&db, &job).await.unwrap(),
            OpenListCancelVerification::Unknown(_)
        ));

        job.media_download_id = Some(1);
        assert_eq!(copy_resolution_actions(&job), ["cancel"]);
    }

    #[test]
    fn downloader_identity_guard_treats_qb_alias_and_trailing_slash_as_same_endpoint() {
        assert!(!downloader_connection_identity_changed(
            "qbittorrent",
            "http://127.0.0.1:8080",
            "qb",
            "http://127.0.0.1:8080/",
        ));
        assert!(downloader_connection_identity_changed(
            "qbittorrent",
            "http://127.0.0.1:8080",
            "qbittorrent",
            "http://127.0.0.1:9090",
        ));
        assert!(downloader_connection_identity_changed(
            "qbittorrent",
            "http://127.0.0.1:8080",
            "other-client",
            "http://127.0.0.1:8080",
        ));
    }

    #[test]
    fn cors_is_limited_to_local_vite_origins() {
        assert_eq!(
            VITE_DEV_ORIGINS,
            [
                "http://localhost:5173",
                "http://127.0.0.1:5173",
                "http://[::1]:5173",
            ]
        );
        assert!(!VITE_DEV_ORIGINS.contains(&"https://third-party.example"));
        let _layer = cors_layer();
    }
}
