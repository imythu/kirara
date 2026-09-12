use super::*;
use crate::db::search::{SearchBinding, SearchSnapshot};
use crate::search::{ParsedFilter, SearchDocument, SearchFilters};

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum SiteSort {
    #[default]
    JoinTime,
    CreatedAt,
    Name,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SearchQuery {
    #[serde(default)]
    q: String,
    sort: Option<SiteSort>,
    page: Option<usize>,
    page_size: Option<usize>,
    site_id: Option<i64>,
    task_id: Option<i64>,
    enabled: Option<bool>,
    result: Option<String>,
    health: Option<String>,
    #[serde(rename = "type")]
    site_type: Option<String>,
}
#[derive(Serialize)]
pub(super) struct SearchItem {
    record: serde_json::Value,
    matched_by: Vec<String>,
}
#[derive(Serialize)]
pub(super) struct SearchResponse {
    items: Vec<SearchItem>,
    total: usize,
    page: usize,
    page_size: usize,
    parsed_filters: Vec<ParsedFilter>,
    semantic_status: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BindingRequest {
    mode: String,
    catalog_id: Option<String>,
}

pub(super) async fn get_binding(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<SearchBinding>, ApiError> {
    state
        .db
        .search_binding(id, None)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::not_found("site not found"))
}
pub(super) async fn put_binding(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(input): Json<BindingRequest>,
) -> Result<Json<SearchBinding>, ApiError> {
    match input.mode.as_str() {
        "manual"
            if input
                .catalog_id
                .as_ref()
                .is_some_and(|id| crate::search::catalog().iter().any(|entry| &entry.id == id)) => {
        }
        "auto" | "none" if input.catalog_id.is_none() => {}
        _ => {
            return Err(ApiError::bad_request(
                "binding requires auto/none without catalog_id, or manual with a valid catalog_id",
            ));
        }
    }
    state
        .db
        .search_binding(id, Some((input.mode, input.catalog_id)))
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::not_found("site not found"))
}

fn validate(query: &SearchQuery, scope: &str) -> Result<(), ApiError> {
    if query.q.chars().count() > 256
        || query.page == Some(0)
        || query.page_size.is_some_and(|v| !(1..=100).contains(&v))
    {
        return Err(ApiError::bad_request(
            "q must be at most 256 characters; page must be positive; page_size must be 1..100",
        ));
    }
    if query.site_id.is_some_and(|id| id <= 0) || query.task_id.is_some_and(|id| id <= 0) {
        return Err(ApiError::bad_request("IDs must be positive"));
    }
    if query
        .result
        .as_deref()
        .is_some_and(|v| !matches!(v, "success" | "failed" | "unknown"))
    {
        return Err(ApiError::bad_request(
            "result must be success, failed, or unknown",
        ));
    }
    if query
        .health
        .as_deref()
        .is_some_and(|v| !matches!(v, "healthy" | "failed" | "pending"))
    {
        return Err(ApiError::bad_request(
            "health must be healthy, failed, or pending",
        ));
    }
    if query
        .site_type
        .as_deref()
        .is_some_and(|v| v.trim().is_empty())
    {
        return Err(ApiError::bad_request("type must not be empty"));
    }
    if (scope != "sites"
        && (query.health.is_some() || query.site_type.is_some() || query.sort.is_some()))
        || (scope != "records" && query.task_id.is_some())
        || (matches!(scope, "sites" | "catalog")
            && (query.enabled.is_some() || query.result.is_some() || query.site_id.is_some()))
        || (scope == "records" && query.enabled.is_some())
    {
        return Err(ApiError::bad_request(
            "filter is not supported in this search scope",
        ));
    }
    Ok(())
}
fn result_status(status: Option<&str>) -> String {
    match status {
        Some("success" | "already") => "success",
        Some("failed") => "failed",
        _ => "unknown",
    }
    .to_owned()
}
fn site_document(site: &SiteWithStats, snapshot: &SearchSnapshot) -> SearchDocument {
    let mut fields = vec![site.site_type.clone()];
    // Preserve the legacy PTD-identifier text search without using protocol guesses as identity.
    fields.extend(crate::ptd_backup::ptd_site_id(site).map(str::to_owned));
    fields.extend(crate::search::normalize_host(&site.base_url));
    if let Some(stats) = &site.stats {
        fields.extend(stats.username.clone());
        fields.extend(stats.uid.clone());
    }
    let health = match &site.stats {
        Some(stats) if stats.last_error.as_ref().is_some_and(|s| !s.is_empty()) => "failed",
        Some(stats) if stats.uploaded.is_some() && stats.downloaded.is_some() => "healthy",
        _ => "pending",
    };
    SearchDocument {
        id: site.id,
        names: vec![site.name.clone()],
        site_names: vec![site.name.clone()],
        site_fields: crate::search::normalize_host(&site.base_url)
            .into_iter()
            .collect(),
        fields,
        catalog_id: snapshot
            .bindings
            .get(&site.id)
            .and_then(|b| b.catalog_id.clone()),
        site_id: Some(site.id),
        health: Some(health.to_owned()),
        result: Some(
            match health {
                "failed" => "failed",
                "healthy" => "success",
                _ => "unknown",
            }
            .to_owned(),
        ),
        site_type: Some(site.site_type.clone()),
        ..Default::default()
    }
}
fn task_document(
    id: i64,
    name: &str,
    site_id: Option<i64>,
    snapshot: &SearchSnapshot,
) -> SearchDocument {
    let mut doc = site_id
        .and_then(|id| snapshot.sites.iter().find(|s| s.id == id))
        .map(|s| site_document(s, snapshot))
        .unwrap_or_default();
    doc.id = id;
    doc.site_id = site_id;
    doc.names.insert(0, name.to_owned());
    doc.health = None;
    doc.site_type = None;
    doc
}
fn value<T: Serialize>(record: T) -> Result<serde_json::Value, ApiError> {
    serde_json::to_value(record)
        .map_err(|_| ApiError::bad_request("cannot serialize search record"))
}

const SEARCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
static SEARCH_ADMISSION: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);
static HISTORY_ADMISSION: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

fn admit_search<'a>(
    global: &'a tokio::sync::Semaphore,
    history: &'a tokio::sync::Semaphore,
    is_history: bool,
) -> Result<
    (
        tokio::sync::SemaphorePermit<'a>,
        Option<tokio::sync::SemaphorePermit<'a>>,
    ),
    ApiError,
> {
    // Reject excess history before touching a global slot, reserving list capacity.
    let history_permit = if is_history {
        Some(history.try_acquire().map_err(|_| busy_error())?)
    } else {
        None
    };
    let global_permit = global.try_acquire().map_err(|_| busy_error())?;
    Ok((global_permit, history_permit))
}
fn busy_error() -> ApiError {
    ApiError {
        status: StatusCode::SERVICE_UNAVAILABLE,
        message: "search is busy; retry shortly".into(),
    }
}
fn timeout_error() -> ApiError {
    ApiError {
        status: StatusCode::GATEWAY_TIMEOUT,
        message: "search exceeded its time limit; narrow the query or scope".into(),
    }
}
fn check_deadline(deadline: std::time::Instant) -> Result<(), ApiError> {
    if std::time::Instant::now() >= deadline {
        Err(timeout_error())
    } else {
        Ok(())
    }
}

async fn execute(
    state: AppState,
    query: SearchQuery,
    scope: &'static str,
) -> Result<Json<SearchResponse>, ApiError> {
    validate(&query, scope)?;
    let permit = admit_search(&SEARCH_ADMISSION, &HISTORY_ADMISSION, scope == "records")?;
    let deadline = std::time::Instant::now() + SEARCH_TIMEOUT;
    let worker = tokio::task::spawn_blocking(move || {
        // Admission lives in the worker, even if the requesting client disconnects.
        let _permit = permit;
        check_deadline(deadline)?;
        let response = if scope == "records" {
            search_history_results(&state.db, query, deadline)
        } else {
            let snapshot = state.db.search_snapshot_blocking(scope)?;
            check_deadline(deadline)?;
            search_snapshot_results(snapshot, query, scope)
        }?;
        check_deadline(deadline)?;
        Ok(response)
    });
    tokio::time::timeout(SEARCH_TIMEOUT, worker)
        .await
        .map_err(|_| timeout_error())?
        .map_err(|_| ApiError::internal("search worker failed"))?
}

fn record_document(
    record: &crate::sign_in::SignInRecord,
    snapshot: &SearchSnapshot,
) -> SearchDocument {
    let mut doc = task_document(record.id, &record.site_name, Some(record.site_id), snapshot);
    doc.result = Some(result_status(Some(&record.status)));
    doc.fields.push(record.message.clone());
    doc.fields.push(record.task_id.to_string());
    if let Some(task) = snapshot
        .sign_tasks
        .iter()
        .find(|task| task.id == record.task_id)
    {
        doc.names.push(task.name.clone());
    }
    doc
}

fn search_history_results(
    db: &Database,
    query: SearchQuery,
    deadline: std::time::Instant,
) -> Result<Json<SearchResponse>, ApiError> {
    let result = db.with_history_search(query.task_id, query.site_id, deadline, |session| {
        let mut context = crate::search::SearchContext::default();
        let mut before = None;
        while !crate::search::normalize(&query.q).is_empty() {
            check_deadline(deadline)?;
            let batch = session.batch(before, true)?;
            if batch.is_empty() {
                break;
            }
            before = batch.last().map(|record| record.id);
            let docs: Vec<_> = batch
                .iter()
                .map(|record| record_document(record, &session.snapshot))
                .collect();
            crate::search::context_observe(&mut context, &docs, &query.q)
                .map_err(ApiError::bad_request)?;
        }
        let filters = SearchFilters {
            site_id: query.site_id,
            result: query.result.clone(),
            ..Default::default()
        };
        let plan = crate::search::prepare_context(&query.q, &filters, context)
            .map_err(ApiError::bad_request)?;
        // The prepared query shares one inference result across all batches.
        let explanation =
            crate::search::search_prepared(&[], &plan).map_err(ApiError::bad_request)?;
        validate_parsed_scope(&explanation.parsed_filters, "records")?;
        check_deadline(deadline)?;
        if crate::search::normalize(&query.q).is_empty() && query.result.is_none() {
            // No textual or result constraints: SQLite can copy exact scoped IDs directly.
            session.store_all_scoped_hits()?;
        } else {
            before = None;
            loop {
                check_deadline(deadline)?;
                let batch = session.batch(before, false)?;
                if batch.is_empty() {
                    break;
                }
                before = batch.last().map(|record| record.id);
                let docs: Vec<_> = batch
                    .iter()
                    .map(|record| record_document(record, &session.snapshot))
                    .collect();
                let output =
                    crate::search::search_prepared(&docs, &plan).map_err(ApiError::bad_request)?;
                session.store_hits(&output.hits)?;
            }
        }
        check_deadline(deadline)?;
        let page_size = query.page_size.unwrap_or(20);
        let (total, page, records) = session.page(
            query.page.unwrap_or(1),
            page_size,
            crate::search::normalize(&query.q).is_empty(),
        )?;
        check_deadline(deadline)?;
        let items = records
            .into_iter()
            .map(|(record, matched_by)| {
                Ok(SearchItem {
                    record: value(record)?,
                    matched_by,
                })
            })
            .collect::<Result<Vec<_>, ApiError>>()?;
        Ok(Json(SearchResponse {
            items,
            total,
            page,
            page_size,
            parsed_filters: explanation.parsed_filters,
            semantic_status: explanation.semantic_status,
        }))
    });
    // SQLite's progress hook aborts scans/sorts too; expose the same HTTP timeout.
    check_deadline(deadline)?;
    result
}

fn validate_parsed_scope(filters: &[ParsedFilter], scope: &str) -> Result<(), ApiError> {
    if filters.iter().any(|filter| {
        (scope != "sites" && matches!(filter.field.as_str(), "health" | "type"))
            || (matches!(scope, "sites" | "catalog" | "records") && filter.field == "enabled")
            || (scope == "catalog" && matches!(filter.field.as_str(), "site" | "result"))
    }) {
        Err(ApiError::bad_request(
            "query field is not supported in this search scope",
        ))
    } else {
        Ok(())
    }
}

fn search_snapshot_results(
    snapshot: SearchSnapshot,
    query: SearchQuery,
    scope: &str,
) -> Result<Json<SearchResponse>, ApiError> {
    let mut docs = Vec::new();
    let mut records = HashMap::new();
    match scope {
        "sites" => {
            for site in &snapshot.sites {
                docs.push(site_document(site, &snapshot));
                records.insert(site.id, value(SiteResponse::from(site.clone()))?);
            }
        }
        "sign" => {
            for task in &snapshot.sign_tasks {
                let mut doc = task_document(task.id, &task.name, Some(task.site_id), &snapshot);
                doc.enabled = Some(task.enabled);
                doc.result = Some(result_status(task.last_status.as_deref()));
                doc.fields.extend(task.last_message.clone());
                docs.push(doc);
                records.insert(task.id, value(task)?);
            }
        }
        "brush" => {
            for task in &snapshot.brush_tasks {
                let mut doc = task_document(task.id, &task.name, task.site_id, &snapshot);
                doc.enabled = Some(task.enabled);
                let info = task
                    .last_run_info
                    .as_deref()
                    .and_then(|v| serde_json::from_str::<serde_json::Value>(v).ok());
                doc.result = Some(result_status(
                    info.as_ref()
                        .and_then(|v| v.get("status"))
                        .and_then(|v| v.as_str()),
                ));
                doc.fields.push(task.tag.clone());
                doc.fields
                    .extend(crate::search::normalize_host(&task.rss_url));
                if let Some(info) = &info {
                    for key in ["error", "message"] {
                        if let Some(message) = info.get(key).and_then(|v| v.as_str()) {
                            doc.fields.push(message.to_owned());
                        }
                    }
                }
                docs.push(doc);
                records.insert(task.id, value(task)?);
            }
        }
        "records" => {
            for record in &snapshot.records {
                let doc = record_document(record, &snapshot);
                docs.push(doc);
                records.insert(record.id, value(record)?);
            }
        }
        "catalog" => {
            for (index, entry) in crate::search::catalog().iter().enumerate() {
                let id = index as i64 + 1;
                docs.push(SearchDocument {
                    id,
                    names: vec![entry.name.clone()],
                    fields: vec![entry.id.clone()],
                    catalog_id: Some(entry.id.clone()),
                    ..Default::default()
                });
                records.insert(id, value(entry)?);
            }
        }
        _ => unreachable!(),
    }
    let filters = SearchFilters {
        site_id: query.site_id,
        enabled: query.enabled,
        result: query.result,
        health: query.health,
        site_type: query.site_type,
        ..Default::default()
    };
    let mut output =
        crate::search::search(&docs, &query.q, &filters).map_err(ApiError::bad_request)?;
    if scope == "sites" {
        let sites: HashMap<_, _> = snapshot.sites.iter().map(|site| (site.id, site)).collect();
        output.hits.sort_by(|a, b| {
            let a = sites[&a.id];
            let b = sites[&b.id];
            match query.sort.unwrap_or_default() {
                SiteSort::JoinTime => b
                    .stats
                    .as_ref()
                    .and_then(|s| s.details.join_time)
                    .cmp(&a.stats.as_ref().and_then(|s| s.details.join_time)),
                SiteSort::CreatedAt => b.created_at.cmp(&a.created_at),
                SiteSort::Name => {
                    crate::search::normalize(&a.name).cmp(&crate::search::normalize(&b.name))
                }
            }
            .then_with(|| a.id.cmp(&b.id))
        });
    }
    validate_parsed_scope(&output.parsed_filters, scope)?;
    let total = output.hits.len();
    let page_size = query.page_size.unwrap_or(20);
    let page = query
        .page
        .unwrap_or(1)
        .min(total.div_ceil(page_size).max(1));
    let items = output
        .hits
        .into_iter()
        .skip((page - 1) * page_size)
        .take(page_size)
        .filter_map(|hit| {
            records.remove(&hit.id).map(|record| SearchItem {
                record,
                matched_by: hit.matched_by,
            })
        })
        .collect();
    Ok(Json(SearchResponse {
        items,
        total,
        page,
        page_size,
        parsed_filters: output.parsed_filters,
        semantic_status: output.semantic_status,
    }))
}

macro_rules! endpoint {
    ($name:ident,$scope:literal) => {
        pub(super) async fn $name(
            State(state): State<AppState>,
            Query(query): Query<SearchQuery>,
        ) -> Result<Json<SearchResponse>, ApiError> {
            execute(state, query, $scope).await
        }
    };
}
endpoint!(sites, "sites");
endpoint!(sign_tasks, "sign");
endpoint!(brush_tasks, "brush");
endpoint!(records, "records");
endpoint!(catalog, "catalog");

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn site_sorting_precedes_pagination_and_puts_unknown_join_times_last() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(dir.path()).await.unwrap();
        let conn = rusqlite::Connection::open(dir.path().join("kirara.db")).unwrap();
        for (id, name, created) in [
            (1, "Charlie", "2026-01-01"),
            (2, "Alpha", "2026-03-01"),
            (3, "Bravo", "2026-02-01"),
        ] {
            conn.execute("INSERT INTO sites(id,name,site_type,base_url,auth_config,created_at,updated_at) VALUES(?1,?2,'nexusphp','https://unknown.invalid','{}',?3,?3)", rusqlite::params![id, name, created]).unwrap();
        }
        for (sort, expected) in [
            (None, vec![3, 2, 1]),
            (Some(SiteSort::CreatedAt), vec![2, 3, 1]),
            (Some(SiteSort::Name), vec![2, 3, 1]),
        ] {
            for (index, expected_id) in expected.into_iter().enumerate() {
                let mut snapshot = db.search_snapshot("sites", None, None).await.unwrap();
                for site in &mut snapshot.sites {
                    if site.id != 1 {
                        let mut stats = crate::site::SiteStatsRecord::default();
                        stats.details.join_time = Some(site.id * 1000);
                        site.stats = Some(stats);
                    }
                }
                let response = search_snapshot_results(
                    snapshot,
                    SearchQuery {
                        sort,
                        page: Some(index + 1),
                        page_size: Some(1),
                        q: "type:nexusphp".into(),
                        ..Default::default()
                    },
                    "sites",
                )
                .unwrap()
                .0;
                assert_eq!(response.total, 3);
                assert_eq!(response.items[0].record["id"], expected_id);
            }
        }
        assert!(
            validate(
                &SearchQuery {
                    sort: Some(SiteSort::Name),
                    ..Default::default()
                },
                "sign"
            )
            .is_err()
        );
        assert!(
            serde_json::from_value::<SearchQuery>(serde_json::json!({"sort": "unsupported"}))
                .is_err()
        );
    }

    #[test]
    fn params_and_real_statuses_are_conservative() {
        assert!(
            validate(
                &SearchQuery {
                    page: Some(0),
                    ..Default::default()
                },
                "sites"
            )
            .is_err()
        );
        assert!(
            validate(
                &SearchQuery {
                    enabled: Some(true),
                    ..Default::default()
                },
                "records"
            )
            .is_err()
        );
        assert!(
            validate(
                &SearchQuery {
                    health: Some("failed".into()),
                    ..Default::default()
                },
                "sites"
            )
            .is_ok()
        );
        assert_eq!(result_status(Some("already")), "success");
        assert_eq!(result_status(Some("skipped")), "unknown");
        assert_eq!(result_status(None), "unknown");
    }
    #[tokio::test]
    async fn database_search_filters_before_paging_and_returns_safe_sites() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(dir.path()).await.unwrap();
        let conn = rusqlite::Connection::open(dir.path().join("kirara.db")).unwrap();
        for id in 1..=25 {
            conn.execute("INSERT INTO sites(id,name,site_type,base_url,auth_config,created_at,updated_at) VALUES(?1,?2,'nexusphp','https://unknown.invalid',?3,'now','now')",rusqlite::params![id,format!("local {id}"),r#"{"auth_type":"cookie","cookie":"DO_NOT_SEARCH_SECRET"}"#]).unwrap();
        }
        conn.execute("INSERT INTO site_stats(site_id,last_checked_at,last_error) VALUES(25,'now','network failed')",[]).unwrap();
        let result = search_snapshot_results(
            db.search_snapshot("sites", None, None).await.unwrap(),
            SearchQuery {
                health: Some("failed".into()),
                page: Some(90),
                ..Default::default()
            },
            "sites",
        )
        .unwrap()
        .0;
        assert_eq!(result.total, 1);
        assert_eq!(result.page, 1);
        assert_eq!(result.items[0].record["id"], 25);
        assert!(result.items[0].record.get("auth_config").is_none());
        let result = search_snapshot_results(
            db.search_snapshot("sites", None, None).await.unwrap(),
            SearchQuery {
                q: "DO_NOT_SEARCH_SECRET".into(),
                ..Default::default()
            },
            "sites",
        )
        .unwrap()
        .0;
        assert_eq!(result.total, 0);
        let result = search_snapshot_results(
            db.search_snapshot("sites", None, None).await.unwrap(),
            SearchQuery {
                page: Some(2),
                ..Default::default()
            },
            "sites",
        )
        .unwrap()
        .0;
        assert_eq!(result.total, 25);
        assert_eq!(result.items.len(), 5);
        assert_eq!(result.items[0].record["id"], 21);
        // This PTD ID is absent from both custom name and hostname; none still disables catalog identity.
        conn.execute("UPDATE sites SET base_url='https://hdts.ru' WHERE id=1", [])
            .unwrap();
        db.search_binding(1, Some(("none".into(), None)))
            .await
            .unwrap();
        let result = search_snapshot_results(
            db.search_snapshot("sites", None, None).await.unwrap(),
            SearchQuery {
                q: "hdtorrents".into(),
                ..Default::default()
            },
            "sites",
        )
        .unwrap()
        .0;
        assert_eq!(result.total, 1);
        assert_eq!(result.items[0].record["id"], 1);
        assert!(result.items[0].record.get("auth_config").is_none());
        assert!(
            db.search_binding(1, None)
                .await
                .unwrap()
                .unwrap()
                .catalog_id
                .is_none()
        );
    }
    #[tokio::test]
    async fn old_history_is_searchable_and_brush_status_requires_valid_json() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(dir.path()).await.unwrap();
        let conn = rusqlite::Connection::open(dir.path().join("kirara.db")).unwrap();
        conn.execute("INSERT INTO sites(id,name,site_type,base_url,auth_config,created_at,updated_at) VALUES(1,'Present','nexusphp','https://unknown.invalid','{}','now','now')",[]).unwrap();
        conn.execute("INSERT INTO sign_in_tasks(id,name,site_id,cron_expression,lightpanda_token,enabled,last_status,created_at,updated_at) VALUES(1,'Task',1,'0 * * * * *','',1,'success','now','now')",[]).unwrap();
        for id in 1..=510 {
            conn.execute("INSERT INTO sign_in_records(task_id,site_id,site_name,started_at,finished_at,status,message) VALUES(1,1,'Past','now','now','failed',?1)",[if id==1 {"oldneedle"}else{"recent"}]).unwrap();
        }
        let result = search_snapshot_results(
            db.search_snapshot("records", None, None).await.unwrap(),
            SearchQuery {
                q: "oldneedle".into(),
                ..Default::default()
            },
            "records",
        )
        .unwrap()
        .0;
        assert_eq!(result.total, 1);
        assert_eq!(result.items[0].record["id"], 1);
        for (id, status) in [
            (1, Some(r#"{"status":"failed"}"#)),
            (2, Some("malformed")),
            (3, None),
        ] {
            conn.execute("INSERT INTO brush_tasks(id,name,cron_expression,site_id,downloader_ids,tag,rss_url,enabled,last_run_info,created_at,updated_at) VALUES(?1,?2,'0 * * * * *',1,'[]','','https://rss.invalid/?secret=HIDDEN',1,?3,'now','now')",rusqlite::params![id,format!("Brush {id}"),status]).unwrap();
        }
        let result = search_snapshot_results(
            db.search_snapshot("brush", None, None).await.unwrap(),
            SearchQuery {
                result: Some("failed".into()),
                enabled: Some(true),
                ..Default::default()
            },
            "brush",
        )
        .unwrap()
        .0;
        assert_eq!(result.total, 1);
        assert_eq!(result.items[0].record["id"], 1);
        let result = search_snapshot_results(
            db.search_snapshot("brush", None, None).await.unwrap(),
            SearchQuery {
                result: Some("unknown".into()),
                ..Default::default()
            },
            "brush",
        )
        .unwrap()
        .0;
        assert_eq!(result.total, 2);
    }
    #[tokio::test]
    async fn streamed_history_equals_global_search_across_batches_and_deep_pages() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(dir.path()).await.unwrap();
        let conn = rusqlite::Connection::open(dir.path().join("kirara.db")).unwrap();
        conn.execute("INSERT INTO sites(id,name,site_type,base_url,auth_config,created_at,updated_at) VALUES(1,'CurrentSite','nexusphp','https://unknown.invalid','{}','now','now')",[]).unwrap();
        conn.execute("INSERT INTO sign_in_tasks(id,name,site_id,cron_expression,lightpanda_token,enabled,last_status,created_at,updated_at) VALUES(1,'CurrentTask',1,'0 * * * * *','',1,'success','now','now')",[]).unwrap();
        for id in 1..=620 {
            conn.execute("INSERT INTO sign_in_records(task_id,site_id,site_name,started_at,finished_at,status,message) VALUES(1,1,?1,'now','now',?2,?3)",rusqlite::params![if id==1 {"失败"}else{"Historical"},if id%3==0 {"success"}else{"failed"},format!("message {id}")]).unwrap();
        }
        for (q, page, result) in [
            ("", 25, None),
            ("", 9999, None),
            ("失败", 1, None),
            ("Historical", 2, None),
            ("CurrentTask", 30, Some("failed")),
            ("result:success", 3, None),
            ("message 619", 1, None),
        ] {
            let query = SearchQuery {
                q: q.into(),
                page: Some(page),
                result: result.map(str::to_owned),
                ..Default::default()
            };
            let eager = search_snapshot_results(
                db.search_snapshot("records", None, None).await.unwrap(),
                query,
                "records",
            )
            .unwrap()
            .0;
            let query = SearchQuery {
                q: q.into(),
                page: Some(page),
                result: result.map(str::to_owned),
                ..Default::default()
            };
            let streamed =
                search_history_results(&db, query, std::time::Instant::now() + SEARCH_TIMEOUT)
                    .unwrap()
                    .0;
            assert_eq!(
                serde_json::to_value(streamed).unwrap(),
                serde_json::to_value(eager).unwrap(),
                "query {q} page {page}"
            );
        }
        let error = search_history_results(
            &db,
            SearchQuery::default(),
            std::time::Instant::now() - std::time::Duration::from_secs(1),
        )
        .err()
        .unwrap();
        assert_eq!(error.status, StatusCode::GATEWAY_TIMEOUT);
        assert_eq!(
            db.search_snapshot("records", None, None)
                .await
                .unwrap()
                .records
                .len(),
            620
        );
    }
    #[test]
    fn history_admission_reserves_list_capacity_and_recovers_on_error() {
        let global = tokio::sync::Semaphore::new(4);
        let history = tokio::sync::Semaphore::new(1);
        let first = admit_search(&global, &history, true).unwrap();
        assert_eq!(
            admit_search(&global, &history, true).unwrap_err().status,
            StatusCode::SERVICE_UNAVAILABLE
        );
        let lists: Vec<_> = (0..3)
            .map(|_| admit_search(&global, &history, false).unwrap())
            .collect();
        assert!(admit_search(&global, &history, false).is_err());
        drop(first);
        let completed = admit_search(&global, &history, true).unwrap();
        drop(completed);
        let failed: Result<(), ApiError> = (|| {
            let _permit = admit_search(&global, &history, true)?;
            Err(timeout_error())
        })();
        assert_eq!(failed.unwrap_err().status, StatusCode::GATEWAY_TIMEOUT);
        assert!(admit_search(&global, &history, true).is_ok());
        drop(lists);
        assert_eq!(global.available_permits(), 4);
        assert_eq!(history.available_permits(), 1);
    }
    #[tokio::test]
    async fn historical_semantic_snapshot_measurement() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(dir.path()).await.unwrap();
        let entry = crate::search::catalog()
            .iter()
            .find(|entry| crate::search::resolve_catalog(&entry.url, "auto", None).is_some())
            .unwrap();
        let site_id = db
            .create_site("LocalAccount", "nexusphp", &entry.url, "{}", "[]", false)
            .await
            .unwrap();
        let conn = rusqlite::Connection::open(dir.path().join("kirara.db")).unwrap();
        conn.execute("INSERT INTO sign_in_tasks(id,name,site_id,cron_expression,lightpanda_token,enabled,last_status,created_at,updated_at) VALUES(1,'RegularTask',?1,'0 * * * * *','',1,'success','now','now')",[site_id]).unwrap();
        conn.execute_batch("BEGIN").unwrap();
        for _ in 0..620 {
            conn.execute("INSERT INTO sign_in_records(task_id,site_id,site_name,started_at,finished_at,status,message) VALUES(1,?1,'HistoricalAccount','now','now','success','done')",[site_id]).unwrap();
        }
        conn.execute_batch("COMMIT").unwrap();
        let q = "适合新手的动漫站";
        let started = std::time::Instant::now();
        let streamed = search_history_results(
            &db,
            SearchQuery {
                q: q.into(),
                ..Default::default()
            },
            started + SEARCH_TIMEOUT,
        )
        .unwrap()
        .0;
        eprintln!(
            "history semantic snapshot: 620 records, elapsed_ms={}, status={}",
            started.elapsed().as_millis(),
            streamed.semantic_status
        );
        // The next eager query is the oracle on identical data and the same public model.
        let eager = search_snapshot_results(
            db.search_snapshot("records", None, None).await.unwrap(),
            SearchQuery {
                q: q.into(),
                ..Default::default()
            },
            "records",
        )
        .unwrap()
        .0;
        if streamed.semantic_status == eager.semantic_status {
            assert_eq!(streamed.total, eager.total);
            assert_eq!(
                serde_json::to_value(streamed.items).unwrap(),
                serde_json::to_value(eager.items).unwrap()
            );
        } else {
            // Other parallel tests may temporarily own the single inference worker.
            assert!(streamed.semantic_status == "busy" || eager.semantic_status == "busy");
        }
    }
}
