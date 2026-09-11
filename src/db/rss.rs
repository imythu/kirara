//! RSS storage, transactional admission, and durable leases.
//!
//! Network work never runs inside these transactions. Claims carry a fencing
//! version; committing stale work is an ordinary conflict, not a best effort.
use std::collections::HashSet;

use chrono::{Duration, Utc};
use rusqlite::{Connection, OptionalExtension, Row, TransactionBehavior, params};
use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};

use super::{Database, join_error, open_connection, sql_error};
use crate::error::AppError;
use crate::rss_download::{fetcher::supports_torrent_resolution, matcher, models::*};

#[derive(Clone)]
pub struct FeedClaim {
    pub feed: StoredFeed,
    pub run_id: i64,
    pub owner: String,
    pub version: i64,
}

#[derive(Clone)]
pub struct EvaluationClaim {
    pub item: ItemRecord,
    pub feed: StoredFeed,
    pub rules: Vec<RuleRecord>,
    pub owner: String,
    pub version: i64,
}

// Locators can contain passkeys. Do not derive Debug or Serialize.
#[derive(Clone)]
pub struct ItemLocator {
    pub download_url: Option<String>,
    pub detail_url: Option<String>,
    pub site_torrent_id: Option<String>,
    pub site_id: Option<i64>,
}

#[derive(Clone)]
pub struct JobClaim {
    pub job: JobRecord,
    pub item: ItemRecord,
    pub feed: StoredFeed,
    pub locator: ItemLocator,
    pub owner: String,
    pub submission_started_at: Option<String>,
}

#[derive(Clone, Default)]
pub struct JobTransition {
    pub status: String,
    pub infohash: Option<String>,
    pub last_error: Option<String>,
    pub next_attempt_at: Option<String>,
    pub download_state: Option<String>,
    pub progress: Option<f64>,
    pub sampled_at: Option<String>,
    pub release_reservation: bool,
}

const FEED_SELECT: &str = "SELECT f.id,f.name,f.url_display,f.site_id,s.name,f.use_proxy,f.enabled,
    f.interval_minutes,f.generation,f.version,f.initialized_at,f.last_sequence,f.last_checked_at,
    f.next_run_at,f.last_status,f.last_error,
    (SELECT count(*) FROM rss_items i WHERE i.feed_id=f.id AND i.generation=f.generation),
    (SELECT count(DISTINCT d.item_id) FROM rss_decisions d JOIN rss_items i ON i.id=d.item_id
     WHERE i.feed_id=f.id AND d.status IN ('pending','ready','attribute_unknown','priority_wait')),
    f.created_at,f.updated_at FROM rss_feeds f LEFT JOIN sites s ON s.id=f.site_id";
const RULE_SELECT: &str = "SELECT r.id,r.name,r.enabled,r.priority,r.filters_json,r.downloader_id,
    d.name,r.options_json,r.match_revision,r.version,r.last_error,r.matched_count,r.created_at,
    r.updated_at FROM rss_rules r LEFT JOIN downloaders d ON d.id=r.downloader_id";
const ITEM_SELECT: &str = "SELECT i.id,i.feed_id,f.name,i.generation,i.item_key,i.sequence,i.title,
    i.detail_display,i.site_torrent_id,i.published_at,i.categories_json,i.attributes_json,
    i.downloadable,i.content_revision,i.first_seen_at,i.last_seen_at,i.status
    FROM rss_items i JOIN rss_feeds f ON f.id=i.feed_id";
const JOB_SELECT: &str = "SELECT j.id,COALESCE(j.item_id,j.item_identity),j.feed_id,j.feed_generation,
    j.feed_name,j.rule_id,j.rule_name,COALESCE(j.downloader_id,j.downloader_identity),j.downloader_name,
    j.title,j.size_bytes,j.filters_json,j.options_json,j.decision_json,j.status,j.infohash,
    j.reserved_bytes,j.attempts,j.next_attempt_at,j.version,j.last_error,j.created_at,j.updated_at,
    j.submitted_at,j.download_state,j.progress,j.sampled_at FROM rss_download_jobs j";
const RUN_SELECT: &str =
    "SELECT id,feed_id,kind,status,item_count,new_count,queued_count,pending_count,
    message,started_at,finished_at FROM rss_runs";

impl Database {
    pub(super) async fn init_rss(&self) -> Result<(), AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS rss_feeds (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,name TEXT NOT NULL,url_display TEXT NOT NULL,
                    site_id INTEGER REFERENCES sites(id) ON DELETE SET NULL,use_proxy INTEGER,
                    enabled INTEGER NOT NULL DEFAULT 1,interval_minutes INTEGER NOT NULL DEFAULT 15,
                    generation INTEGER NOT NULL DEFAULT 1,version INTEGER NOT NULL DEFAULT 1,
                    initialized_at TEXT,last_sequence INTEGER NOT NULL DEFAULT 0,resume_baseline INTEGER NOT NULL DEFAULT 0,
                    etag TEXT,last_modified TEXT,last_checked_at TEXT,next_run_at TEXT,
                    last_status TEXT NOT NULL DEFAULT 'pending',last_error TEXT,
                    lease_owner TEXT,lease_until TEXT,lease_version INTEGER NOT NULL DEFAULT 0,current_run_id INTEGER,
                    quota_used INTEGER NOT NULL DEFAULT 0,cycle_id INTEGER NOT NULL DEFAULT 0,
                    failure_count INTEGER NOT NULL DEFAULT 0,requires_action INTEGER NOT NULL DEFAULT 0,
                    cooldown_until TEXT,created_at TEXT NOT NULL,updated_at TEXT NOT NULL,archived_at TEXT
                );
                CREATE TABLE IF NOT EXISTS rss_feed_secrets (
                    feed_id INTEGER NOT NULL REFERENCES rss_feeds(id),generation INTEGER NOT NULL,
                    url TEXT NOT NULL,url_digest TEXT NOT NULL,PRIMARY KEY(feed_id,generation)
                );
                CREATE TABLE IF NOT EXISTS rss_rules (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,name TEXT NOT NULL,enabled INTEGER NOT NULL DEFAULT 1,
                    priority INTEGER NOT NULL DEFAULT 100,filters_json TEXT NOT NULL,
                    downloader_id INTEGER REFERENCES downloaders(id) ON DELETE SET NULL,options_json TEXT NOT NULL,
                    match_revision INTEGER NOT NULL DEFAULT 1,version INTEGER NOT NULL DEFAULT 1,last_error TEXT,
                    matched_count INTEGER NOT NULL DEFAULT 0,created_at TEXT NOT NULL,updated_at TEXT NOT NULL,archived_at TEXT
                );
                CREATE TABLE IF NOT EXISTS rss_rule_feeds (
                    rule_id INTEGER NOT NULL REFERENCES rss_rules(id),feed_id INTEGER NOT NULL REFERENCES rss_feeds(id),
                    generation INTEGER NOT NULL,activation_sequence INTEGER NOT NULL,baseline_pending INTEGER NOT NULL,
                    PRIMARY KEY(rule_id,feed_id)
                );
                CREATE TABLE IF NOT EXISTS rss_seen_keys (
                    feed_id INTEGER NOT NULL REFERENCES rss_feeds(id),generation INTEGER NOT NULL,item_key TEXT NOT NULL,
                    first_sequence INTEGER NOT NULL,first_seen_at TEXT NOT NULL,
                    PRIMARY KEY(feed_id,generation,item_key),UNIQUE(feed_id,generation,first_sequence)
                );
                CREATE TABLE IF NOT EXISTS rss_items (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,feed_id INTEGER NOT NULL REFERENCES rss_feeds(id),
                    generation INTEGER NOT NULL,item_key TEXT NOT NULL,sequence INTEGER NOT NULL,title TEXT NOT NULL,
                    detail_display TEXT,site_torrent_id TEXT,published_at TEXT,categories_json TEXT NOT NULL,
                    attributes_json TEXT NOT NULL,downloadable INTEGER NOT NULL,content_revision INTEGER NOT NULL DEFAULT 1,
                    first_seen_at TEXT NOT NULL,last_seen_at TEXT NOT NULL,status TEXT NOT NULL DEFAULT 'baseline',
                    evaluation_owner TEXT,evaluation_until TEXT,evaluation_version INTEGER NOT NULL DEFAULT 0,
                    UNIQUE(feed_id,generation,item_key),UNIQUE(feed_id,generation,sequence)
                );
                CREATE TABLE IF NOT EXISTS rss_item_locators (
                    item_id INTEGER PRIMARY KEY REFERENCES rss_items(id) ON DELETE CASCADE,
                    feed_generation INTEGER NOT NULL,download_url TEXT,detail_url TEXT
                );
                CREATE TABLE IF NOT EXISTS rss_decisions (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,item_id INTEGER NOT NULL REFERENCES rss_items(id) ON DELETE CASCADE,
                    rule_id INTEGER NOT NULL REFERENCES rss_rules(id),match_revision INTEGER NOT NULL,item_revision INTEGER NOT NULL,
                    status TEXT NOT NULL,evaluation_json TEXT NOT NULL DEFAULT '{\"matched\":false,\"needs_attributes\":false,\"reasons\":[]}',checked_at TEXT,next_evaluate_at TEXT,
                    job_id INTEGER REFERENCES rss_download_jobs(id) ON DELETE SET NULL,manual INTEGER NOT NULL DEFAULT 0,version INTEGER NOT NULL DEFAULT 1,
                    UNIQUE(item_id,rule_id,match_revision)
                );
                CREATE TABLE IF NOT EXISTS rss_download_jobs (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,item_id INTEGER REFERENCES rss_items(id) ON DELETE SET NULL,
                    item_identity INTEGER NOT NULL,feed_id INTEGER NOT NULL REFERENCES rss_feeds(id),feed_generation INTEGER NOT NULL,
                    feed_name TEXT NOT NULL,rule_id INTEGER NOT NULL REFERENCES rss_rules(id),rule_name TEXT NOT NULL,
                    downloader_id INTEGER REFERENCES downloaders(id) ON DELETE SET NULL,downloader_identity INTEGER NOT NULL,
                    downloader_name TEXT NOT NULL,title TEXT NOT NULL,size_bytes INTEGER,
                    filters_json TEXT NOT NULL,options_json TEXT NOT NULL,decision_json TEXT NOT NULL,
                    status TEXT NOT NULL DEFAULT 'queued',infohash TEXT,reserved_bytes INTEGER NOT NULL DEFAULT 0,
                    attempts INTEGER NOT NULL DEFAULT 0,next_attempt_at TEXT,lease_owner TEXT,lease_until TEXT,
                    version INTEGER NOT NULL DEFAULT 1,last_error TEXT,created_at TEXT NOT NULL,updated_at TEXT NOT NULL,
                    submission_started_at TEXT,submitted_at TEXT,download_state TEXT,progress REAL,sampled_at TEXT
                );
                CREATE TABLE IF NOT EXISTS rss_delivery_keys (
                    downloader_id INTEGER NOT NULL,key_kind TEXT NOT NULL,key_digest TEXT NOT NULL,
                    job_id INTEGER REFERENCES rss_download_jobs(id) ON DELETE SET NULL,
                    outcome TEXT NOT NULL,created_at TEXT NOT NULL,updated_at TEXT NOT NULL,
                    PRIMARY KEY(downloader_id,key_kind,key_digest)
                );
                CREATE TABLE IF NOT EXISTS rss_downloader_slots (
                    downloader_id INTEGER PRIMARY KEY REFERENCES downloaders(id) ON DELETE CASCADE,
                    job_id INTEGER NOT NULL REFERENCES rss_download_jobs(id),owner TEXT NOT NULL,lease_until TEXT NOT NULL,
                    version INTEGER NOT NULL DEFAULT 1
                );
                CREATE TABLE IF NOT EXISTS rss_runs (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,feed_id INTEGER REFERENCES rss_feeds(id),kind TEXT NOT NULL,
                    status TEXT NOT NULL,item_count INTEGER NOT NULL DEFAULT 0,new_count INTEGER NOT NULL DEFAULT 0,
                    queued_count INTEGER NOT NULL DEFAULT 0,pending_count INTEGER NOT NULL DEFAULT 0,message TEXT,
                    started_at TEXT NOT NULL,finished_at TEXT,operation TEXT,request_id TEXT,request_digest TEXT,result_ref INTEGER,
                    UNIQUE(operation,request_id)
                );
                CREATE TABLE IF NOT EXISTS rss_run_items (
                    run_id INTEGER NOT NULL REFERENCES rss_runs(id) ON DELETE CASCADE,
                    item_id INTEGER NOT NULL REFERENCES rss_items(id) ON DELETE CASCADE,
                    rule_id INTEGER NOT NULL REFERENCES rss_rules(id),match_revision INTEGER NOT NULL,
                    PRIMARY KEY(run_id,item_id)
                );
                CREATE INDEX IF NOT EXISTS idx_rss_feeds_due ON rss_feeds(enabled,archived_at,next_run_at,lease_until);
                CREATE INDEX IF NOT EXISTS idx_rss_items_feed ON rss_items(feed_id,generation,sequence);
                CREATE INDEX IF NOT EXISTS idx_rss_decisions_due ON rss_decisions(status,next_evaluate_at,item_id);
                CREATE INDEX IF NOT EXISTS idx_rss_jobs_due ON rss_download_jobs(status,next_attempt_at,lease_until);
                CREATE INDEX IF NOT EXISTS idx_rss_jobs_downloader ON rss_download_jobs(downloader_id,status,infohash);
                CREATE INDEX IF NOT EXISTS idx_rss_jobs_rule ON rss_download_jobs(rule_id,status);
                CREATE INDEX IF NOT EXISTS idx_rss_runs_feed ON rss_runs(feed_id,started_at,id);",
            ).map_err(sql_error)?;
            super::ensure_column(&conn,"rss_download_jobs","submission_started_at","ALTER TABLE rss_download_jobs ADD COLUMN submission_started_at TEXT")?;
            conn.execute("UPDATE rss_decisions SET evaluation_json=? WHERE evaluation_json='{}'",[serde_json::to_string(&MatchEvaluation::default()).expect("serializable empty evaluation")]).map_err(sql_error)?;
            Ok(())
        }).await.map_err(join_error)?
    }

    async fn rss_read<T: Send + 'static>(
        &self,
        op: impl FnOnce(&mut Connection) -> RssResult<T> + Send + 'static,
    ) -> RssResult<T> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = open_connection(&path)?;
            op(&mut conn)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn rss_get_feed(&self, id: i64) -> RssResult<StoredFeed> {
        self.rss_read(move |conn| get_feed(conn, id)).await
    }

    pub async fn rss_get_rule(&self, id: i64) -> RssResult<RuleRecord> {
        self.rss_read(move |conn| get_rule(conn, id)).await
    }

    pub async fn rss_get_item(&self, id: i64) -> RssResult<ItemRecord> {
        self.rss_read(move |conn| get_item(conn, id)).await
    }

    pub async fn rss_get_item_locator(&self, id: i64) -> RssResult<ItemLocator> {
        self.rss_read(move |conn| get_locator(conn, id)).await
    }

    pub async fn rss_get_job(&self, id: i64) -> RssResult<JobRecord> {
        self.rss_read(move |conn| get_job(conn, id)).await
    }

    pub async fn rss_get_run(&self, id: i64) -> RssResult<RunRecord> {
        self.rss_read(move |conn| get_run(conn, id)).await
    }

    pub async fn rss_list_feeds(&self, query: ListQuery) -> RssResult<Page<FeedRecord>> {
        let query = query.normalized();
        self.rss_read(move |conn| {
            let mut where_sql = String::from("f.archived_at IS NULL");
            let mut values = Vec::new();
            keyword_filter(&mut where_sql, &mut values, "f.name", &query.keyword);
            id_filter(&mut where_sql, &mut values, "f.site_id", query.site_id);
            match query.status.as_deref() {
                Some("running" | "enabled") => where_sql.push_str(" AND f.enabled=1"),
                Some("paused") => where_sql.push_str(" AND f.enabled=0"),
                Some("needs_attention" | "error") => {
                    where_sql.push_str(" AND f.last_error IS NOT NULL")
                }
                _ => {}
            }
            query_page(
                conn,
                &format!("{FEED_SELECT} WHERE {where_sql}"),
                "f.created_at DESC,f.id DESC",
                values,
                &query,
                map_feed,
            )
        })
        .await
    }

    pub async fn rss_list_rules(&self, query: ListQuery) -> RssResult<Page<RuleRecord>> {
        let query = query.normalized();
        self.rss_read(move |conn| {
            let mut filter = String::from("r.archived_at IS NULL");
            let mut values = Vec::new();
            keyword_filter(&mut filter, &mut values, "r.name", &query.keyword);
            id_filter(&mut filter, &mut values, "r.downloader_id", query.downloader_id);
            if let Some(feed_id) = query.feed_id {
                filter.push_str(" AND EXISTS(SELECT 1 FROM rss_rule_feeds rf WHERE rf.rule_id=r.id AND rf.feed_id=?)");
                values.push(feed_id.into());
            }
            match query.status.as_deref() {
                Some("running" | "enabled") => filter.push_str(" AND r.enabled=1"),
                Some("paused") => filter.push_str(" AND r.enabled=0"),
                Some("needs_attention" | "error") => filter.push_str(" AND r.last_error IS NOT NULL"),
                _ => {}
            }
            let mut page = query_page(conn, &format!("{RULE_SELECT} WHERE {filter}"),
                "r.priority,r.id", values, &query, map_rule)?;
            for rule in &mut page.items { rule.feed_ids = rule_feed_ids(conn, rule.id)?; }
            Ok(page)
        }).await
    }

    pub async fn rss_list_items(&self, query: ListQuery) -> RssResult<Page<ItemRecord>> {
        let query = query.normalized();
        self.rss_read(move |conn| {
            let mut filter = String::from("f.archived_at IS NULL AND i.generation=f.generation");
            let mut values = Vec::new();
            keyword_filter(&mut filter, &mut values, "i.title", &query.keyword);
            id_filter(&mut filter, &mut values, "i.feed_id", query.feed_id);
            id_filter(&mut filter, &mut values, "f.site_id", query.site_id);
            if let Some(rule_id) = query.rule_id {
                filter.push_str(" AND EXISTS(SELECT 1 FROM rss_rule_feeds rf WHERE rf.feed_id=i.feed_id AND rf.rule_id=?)");
                values.push(rule_id.into());
            }
            if let Some(status) = query.status.as_deref().filter(|s| !s.is_empty() && *s != "all") {
                if status == "needs_attention" {
                    filter.push_str(" AND i.status IN ('attribute_unknown','failed','unavailable')");
                } else if status == "pending" {
                    filter.push_str(" AND i.status IN ('pending','ready','priority_wait')");
                } else {
                    filter.push_str(" AND i.status=?"); values.push(status.to_owned().into());
                }
            }
            let mut page = query_page(conn, &format!("{ITEM_SELECT} WHERE {filter}"),
                "i.first_seen_at DESC,i.id DESC", values, &query, map_item)?;
            for item in &mut page.items { item.decisions = get_decisions(conn, item.id)?; }
            Ok(page)
        }).await
    }

    pub async fn rss_list_jobs(&self, query: ListQuery) -> RssResult<Page<JobRecord>> {
        let query = query.normalized();
        self.rss_read(move |conn| {
            let mut filter = String::from("1=1");
            let mut values = Vec::new();
            keyword_filter(&mut filter, &mut values, "j.title", &query.keyword);
            id_filter(&mut filter, &mut values, "j.feed_id", query.feed_id);
            id_filter(&mut filter, &mut values, "j.rule_id", query.rule_id);
            id_filter(
                &mut filter,
                &mut values,
                "j.downloader_identity",
                query.downloader_id,
            );
            if let Some(site_id) = query.site_id {
                filter.push_str(
                    " AND EXISTS(SELECT 1 FROM rss_feeds f WHERE f.id=j.feed_id AND f.site_id=?)",
                );
                values.push(site_id.into());
            }
            if let Some(status) = query
                .status
                .as_deref()
                .filter(|s| !s.is_empty() && *s != "all")
            {
                if status == "needs_attention" {
                    filter.push_str(" AND j.status IN ('failed','waiting','reconciling')");
                } else {
                    filter.push_str(" AND j.status=?");
                    values.push(status.to_owned().into());
                }
            }
            query_page(
                conn,
                &format!("{JOB_SELECT} WHERE {filter}"),
                "j.created_at DESC,j.id DESC",
                values,
                &query,
                map_job,
            )
        })
        .await
    }

    pub async fn rss_list_runs(&self, query: ListQuery) -> RssResult<Page<RunRecord>> {
        let query = query.normalized();
        self.rss_read(move |conn| {
            let mut filter = String::from("1=1");
            let mut values = Vec::new();
            id_filter(&mut filter, &mut values, "feed_id", query.feed_id);
            if let Some(status) = query
                .status
                .as_deref()
                .filter(|s| !s.is_empty() && *s != "all")
            {
                filter.push_str(" AND status=?");
                values.push(status.to_owned().into());
            }
            query_page(
                conn,
                &format!("{RUN_SELECT} WHERE {filter}"),
                "started_at DESC,id DESC",
                values,
                &query,
                map_run,
            )
        })
        .await
    }

    pub async fn rss_summary(&self) -> RssResult<RssSummary> {
        self.rss_read(move |conn| {
            let (feeds_total,running,paused,feed_errors): (u64,u64,u64,u64) = conn.query_row(
                "SELECT count(*),COALESCE(sum(enabled=1),0),COALESCE(sum(enabled=0),0),COALESCE(sum(last_error IS NOT NULL),0)
                 FROM rss_feeds WHERE archived_at IS NULL", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).map_err(sql_error)?;
            let rules_enabled = conn.query_row("SELECT count(*) FROM rss_rules WHERE archived_at IS NULL AND enabled=1", [], |r| r.get(0)).map_err(sql_error)?;
            let (queued,submitted,failed): (u64,u64,u64) = conn.query_row(
                "SELECT COALESCE(sum(status IN ('queued','fetching','submitting','reconciling','retry_wait','waiting','held')),0),
                        COALESCE(sum(status IN ('submitted','already_present')),0),COALESCE(sum(status='failed'),0) FROM rss_download_jobs", [],
                |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(sql_error)?;
            let rule_errors: u64 = conn.query_row("SELECT count(*) FROM rss_rules WHERE archived_at IS NULL AND last_error IS NOT NULL", [], |r| r.get(0)).map_err(sql_error)?;
            let unsettled: u64 = conn.query_row("SELECT count(*) FROM rss_download_jobs WHERE status IN ('waiting','reconciling')", [], |r| r.get(0)).map_err(sql_error)?;
            let unknown: u64 = conn.query_row("SELECT count(*) FROM rss_items i JOIN rss_feeds f ON f.id=i.feed_id WHERE i.status='attribute_unknown' AND i.generation=f.generation AND f.archived_at IS NULL", [], |r| r.get(0)).map_err(sql_error)?;
            Ok(RssSummary { feeds_total,running,paused,needs_attention:feed_errors+failed+rule_errors+unsettled+unknown,rules_enabled,queued,submitted,failed })
        }).await
    }

    pub async fn rss_save_feed(&self, id: Option<i64>, input: FeedInput) -> RssResult<FeedRecord> {
        self.rss_read(move |conn| {
            if input.name.trim().is_empty() || input.name.chars().count()>100 || !(5..=1440).contains(&input.interval_minutes) {
                return Err(RssError::Invalid("请输入名称（不超过 100 字）和 5–1440 分钟的检查间隔".into()));
            }
            let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(sql_error)?;
            let operation=id.map(|v|format!("feed:update:{v}")).unwrap_or_else(||"feed:create".into());
            let request_digest=digest(&json(&input)?);
            if let Some(existing)=request_lookup(&tx,&operation,input.request_id.as_deref(),&request_digest)? { return Ok(get_feed(&tx,existing)?.record); }
            if let Some(site_id)=input.site_id { require_reference(&tx,"sites",site_id,"关联站点")?; }
            let current=id.map(|id|get_feed(&tx,id)).transpose()?;
            if let Some(current)=&current { check_version(current.record.version,input.expected_version)?; }
            let url=input.url.as_deref().map(str::trim).filter(|s|!s.is_empty()).map(str::to_owned)
                .or_else(||current.as_ref().map(|c|c.url.clone())).ok_or_else(||RssError::Invalid("请输入 RSS 地址".into()))?;
            if current.as_ref().is_some_and(|feed| feed.url != url) {
                return Err(RssError::Invalid("RSS 地址保存后不可修改，请添加新的订阅源".into()));
            }
            let parsed=reqwest::Url::parse(&url).map_err(|_|RssError::Invalid("RSS 地址格式无效".into()))?;
            if !matches!(parsed.scheme(),"http"|"https") || parsed.host_str().is_none() || !parsed.username().is_empty() || parsed.password().is_some() || url.len()>8192 || url.contains('•') {
                return Err(RssError::Invalid("RSS 地址必须是有效的 HTTP/HTTPS 地址，不能包含用户信息或脱敏占位符".into()));
            }
            // A site's API may use a different origin from its RSS feed.
            // The fetcher scopes credentials to the configured site origin per request.
            let time=now();
            let feed_id=if let Some(current)=current {
                let resume=!current.record.enabled && input.enabled;
                tx.execute("UPDATE rss_feeds SET name=?,site_id=?,use_proxy=?,enabled=?,interval_minutes=?,
                    version=version+1,resume_baseline=CASE WHEN ? THEN 1 ELSE resume_baseline END,
                    lease_owner=NULL,lease_until=NULL,lease_version=lease_version+1,current_run_id=NULL,
                    requires_action=0,last_error=NULL,next_run_at=?,updated_at=? WHERE id=?",
                    params![input.name.trim(),input.site_id,input.use_proxy,input.enabled,input.interval_minutes,
                        resume,time,time,current.record.id]).map_err(sql_error)?;
                if let Some(run_id)=tx.query_row("SELECT id FROM rss_runs WHERE feed_id=? AND kind='check' AND status IN ('queued','running') ORDER BY id DESC LIMIT 1",[current.record.id],|r|r.get::<_,i64>(0)).optional().map_err(sql_error)? {
                    tx.execute("UPDATE rss_runs SET status='superseded',message='源配置已更新，旧响应已丢弃',finished_at=? WHERE id=?",params![time,run_id]).map_err(sql_error)?;
                }
                synchronize_jobs(&tx,&time)?;
                current.record.id
            } else {
                tx.execute("INSERT INTO rss_feeds(name,url_display,site_id,use_proxy,enabled,interval_minutes,next_run_at,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?)",
                    params![input.name.trim(),masked_url(&url),input.site_id,input.use_proxy,input.enabled,input.interval_minutes,time,time,time]).map_err(sql_error)?;
                let id=tx.last_insert_rowid();
                tx.execute("INSERT INTO rss_feed_secrets(feed_id,generation,url,url_digest) VALUES(?,1,?,?)",params![id,url,digest(&url)]).map_err(sql_error)?; id
            };
            record_request(&tx,&operation,input.request_id.as_deref(),&request_digest,feed_id,Some(feed_id),"feed_save",&time)?;
            let result=get_feed(&tx,feed_id)?.record;
            tx.commit().map_err(sql_error)?; Ok(result)
        }).await
    }

    pub async fn rss_set_feed_enabled(
        &self,
        id: i64,
        enabled: bool,
        request: ActionRequest,
    ) -> RssResult<FeedRecord> {
        self.rss_read(move |conn| {
            let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(sql_error)?;
            let operation=format!("feed:{}:{id}",if enabled {"resume"} else {"pause"}); let request_digest=digest(&json(&request)?);
            if let Some(id)=request_lookup(&tx,&operation,request.request_id.as_deref(),&request_digest)? { return Ok(get_feed(&tx,id)?.record); }
            let feed=get_feed(&tx,id)?; check_version(feed.record.version,request.expected_version)?;
            let time=now();
            if feed.record.enabled!=enabled {
                tx.execute("UPDATE rss_feeds SET enabled=?,resume_baseline=CASE WHEN ? THEN 1 ELSE resume_baseline END,
                    version=version+1,lease_owner=NULL,lease_until=NULL,lease_version=lease_version+1,next_run_at=?,
                    current_run_id=NULL,requires_action=0,updated_at=? WHERE id=?",params![enabled,enabled,time,time,id]).map_err(sql_error)?;
                tx.execute("UPDATE rss_runs SET status='superseded',message='订阅源启停状态已变化',finished_at=? WHERE feed_id=? AND kind='check' AND status IN ('queued','running')",params![time,id]).map_err(sql_error)?;
                synchronize_jobs(&tx,&time)?;
            }
            record_request(&tx,&operation,request.request_id.as_deref(),&request_digest,id,Some(id),"feed_state",&time)?;
            let result=get_feed(&tx,id)?.record;tx.commit().map_err(sql_error)?;Ok(result)
        }).await
    }

    pub async fn rss_archive_feed(&self, id: i64, request: ActionRequest) -> RssResult<()> {
        self.rss_read(move |conn| {
            let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(sql_error)?;
            let operation=format!("feed:archive:{id}");let request_digest=digest(&json(&request)?);
            if request_lookup(&tx,&operation,request.request_id.as_deref(),&request_digest)?.is_some() {return Ok(());}
            let feed=get_feed(&tx,id)?;check_version(feed.record.version,request.expected_version)?;
            let time=now();
            tx.execute("UPDATE rss_feeds SET archived_at=?,enabled=0,version=version+1,updated_at=?,lease_owner=NULL,lease_until=NULL,lease_version=lease_version+1 WHERE id=?",params![time,time,id]).map_err(sql_error)?;
            tx.execute("UPDATE rss_runs SET status='cancelled',message='订阅源已归档',finished_at=? WHERE feed_id=? AND status IN ('queued','running','processing')",params![time,id]).map_err(sql_error)?;
            tx.execute("UPDATE rss_rules SET enabled=0,last_error='规则没有有效来源，请重新选择订阅源',version=version+1,updated_at=?
                WHERE archived_at IS NULL AND EXISTS(SELECT 1 FROM rss_rule_feeds rf WHERE rf.rule_id=rss_rules.id AND rf.feed_id=?)
                AND NOT EXISTS(SELECT 1 FROM rss_rule_feeds rf JOIN rss_feeds f ON f.id=rf.feed_id WHERE rf.rule_id=rss_rules.id AND f.archived_at IS NULL)",params![time,id]).map_err(sql_error)?;
            tx.execute("UPDATE rss_decisions SET status='cancelled',next_evaluate_at=NULL,version=version+1 WHERE job_id IS NULL AND item_id IN(SELECT id FROM rss_items WHERE feed_id=?)",[id]).map_err(sql_error)?;
            synchronize_jobs(&tx,&time)?;
            record_request(&tx,&operation,request.request_id.as_deref(),&request_digest,id,Some(id),"feed_archive",&time)?;
            tx.commit().map_err(sql_error)?;Ok(())
        }).await
    }

    pub async fn rss_save_rule(
        &self,
        id: Option<i64>,
        mut input: RuleInput,
    ) -> RssResult<RuleRecord> {
        self.rss_read(move |conn| {
            matcher::validate_filters(&input.filters).map_err(RssError::Invalid)?;
            input.feed_ids.sort_unstable();input.feed_ids.dedup();
            if input.name.trim().is_empty() || input.name.chars().count()>100 || input.feed_ids.is_empty() || input.feed_ids.len()>200 {
                return Err(RssError::Invalid("请填写规则名称并选择 1–200 个订阅源".into()));
            }
            let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(sql_error)?;
            let operation=id.map(|v|format!("rule:update:{v}")).unwrap_or_else(||"rule:create".into());let request_digest=digest(&json(&input)?);
            if let Some(id)=request_lookup(&tx,&operation,input.request_id.as_deref(),&request_digest)? {return get_rule(&tx,id);}
            for feed_id in &input.feed_ids {get_feed(&tx,*feed_id)?;}
            validate_rule_target(&tx,input.enabled,input.downloader_id)?;
            let time=now();
            let rule_id=if let Some(id)=id {
                let current=get_rule(&tx,id)?;check_version(current.version,input.expected_version)?;
                let new_revision=current.filters!=input.filters || current.feed_ids!=input.feed_ids || current.downloader_id!=input.downloader_id;
                let activate=new_revision || (!current.enabled && input.enabled);
                tx.execute("UPDATE rss_rules SET name=?,enabled=?,priority=?,filters_json=?,downloader_id=?,options_json=?,
                    match_revision=match_revision+?,version=version+1,last_error=NULL,updated_at=? WHERE id=?",
                    params![input.name.trim(),input.enabled,input.priority,json(&input.filters)?,input.downloader_id,json(&input.options)?,new_revision,time,id]).map_err(sql_error)?;
                if activate { reset_rule_boundaries(&tx,id,&input.feed_ids)?; }
                if new_revision {
                    tx.execute("UPDATE rss_decisions SET status='superseded',next_evaluate_at=NULL,version=version+1 WHERE rule_id=? AND job_id IS NULL AND match_revision=?",params![id,current.match_revision]).map_err(sql_error)?;
                }
                synchronize_jobs(&tx,&time)?;id
            } else {
                tx.execute("INSERT INTO rss_rules(name,enabled,priority,filters_json,downloader_id,options_json,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?)",
                    params![input.name.trim(),input.enabled,input.priority,json(&input.filters)?,input.downloader_id,json(&input.options)?,time,time]).map_err(sql_error)?;
                let id=tx.last_insert_rowid();reset_rule_boundaries(&tx,id,&input.feed_ids)?;id
            };
            record_request(&tx,&operation,input.request_id.as_deref(),&request_digest,rule_id,None,"rule_save",&time)?;
            let result=get_rule(&tx,rule_id)?;tx.commit().map_err(sql_error)?;Ok(result)
        }).await
    }

    pub async fn rss_set_rule_enabled(
        &self,
        id: i64,
        enabled: bool,
        request: ActionRequest,
    ) -> RssResult<RuleRecord> {
        self.rss_read(move |conn| {
            let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(sql_error)?;
            let operation=format!("rule:{}:{id}",if enabled {"resume"} else {"pause"});let request_digest=digest(&json(&request)?);
            if let Some(id)=request_lookup(&tx,&operation,request.request_id.as_deref(),&request_digest)? {return get_rule(&tx,id);}
            let rule=get_rule(&tx,id)?;check_version(rule.version,request.expected_version)?;
            validate_rule_target(&tx,enabled,rule.downloader_id)?;
            if enabled {
                let valid_sources:i64=tx.query_row("SELECT count(*) FROM rss_rule_feeds rf JOIN rss_feeds f ON f.id=rf.feed_id WHERE rf.rule_id=? AND f.archived_at IS NULL",[id],|r|r.get(0)).map_err(sql_error)?;
                if valid_sources==0 {return Err(RssError::Invalid("规则没有有效来源，请重新选择订阅源".into()));}
            }
            let time=now();
            if rule.enabled!=enabled {
                tx.execute("UPDATE rss_rules SET enabled=?,version=version+1,updated_at=?,last_error=NULL WHERE id=?",params![enabled,time,id]).map_err(sql_error)?;
                if enabled {
                    reset_rule_boundaries(&tx,id,&rule.feed_ids)?;
                    tx.execute("UPDATE rss_decisions SET status='skipped',next_evaluate_at=NULL,version=version+1 WHERE rule_id=? AND job_id IS NULL AND manual=0",[id]).map_err(sql_error)?;
                }
                synchronize_jobs(&tx,&time)?;
            }
            record_request(&tx,&operation,request.request_id.as_deref(),&request_digest,id,None,"rule_state",&time)?;
            let result=get_rule(&tx,id)?;tx.commit().map_err(sql_error)?;Ok(result)
        }).await
    }

    pub async fn rss_archive_rule(&self, id: i64, request: ActionRequest) -> RssResult<()> {
        self.rss_read(move |conn| {
            let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(sql_error)?;
            let operation=format!("rule:archive:{id}");let request_digest=digest(&json(&request)?);
            if request_lookup(&tx,&operation,request.request_id.as_deref(),&request_digest)?.is_some() {return Ok(());}
            let rule=get_rule(&tx,id)?;check_version(rule.version,request.expected_version)?;let time=now();
            tx.execute("UPDATE rss_rules SET archived_at=?,enabled=0,version=version+1,updated_at=? WHERE id=?",params![time,time,id]).map_err(sql_error)?;
            tx.execute("UPDATE rss_decisions SET status='cancelled',next_evaluate_at=NULL,version=version+1 WHERE rule_id=? AND job_id IS NULL",[id]).map_err(sql_error)?;
            synchronize_jobs(&tx,&time)?;
            record_request(&tx,&operation,request.request_id.as_deref(),&request_digest,id,None,"rule_archive",&time)?;
            tx.commit().map_err(sql_error)?;Ok(())
        }).await
    }

    pub async fn rss_request_check(&self, id: i64, request: ActionRequest) -> RssResult<RunRecord> {
        self.rss_read(move |conn| {
            let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(sql_error)?;
            let operation=format!("feed:check:{id}");let request_digest=digest(&json(&request)?);
            if let Some(run_id)=request_lookup(&tx,&operation,request.request_id.as_deref(),&request_digest)? {return get_run(&tx,run_id);}
            let feed=get_feed(&tx,id)?;
            if !feed.record.enabled {return Err(RssError::Conflict("订阅源已暂停，请恢复后再检查".into()));}
            if let Some(version)=request.expected_version {check_version(feed.record.version,Some(version))?;}
            let time=now();
            let (cooldown,active):(Option<String>,Option<i64>)=tx.query_row("SELECT cooldown_until,current_run_id FROM rss_feeds WHERE id=?",[id],|r|Ok((r.get(0)?,r.get(1)?))).map_err(sql_error)?;
            if let Some(retry_at)=cooldown.filter(|t|t>&time) {return Err(RssError::RateLimited {message:"来源正在冷却，请稍后重试".into(),retry_at});}
            let active=active.filter(|run_id|get_run(&tx,*run_id).is_ok_and(|r|matches!(r.status.as_str(),"queued"|"running")));
            let run_id=if let Some(active)=active {active} else {
                tx.execute("INSERT INTO rss_runs(feed_id,kind,status,started_at) VALUES(?,'check','queued',?)",params![id,time]).map_err(sql_error)?;
                let run_id=tx.last_insert_rowid();
                tx.execute("UPDATE rss_feeds SET current_run_id=?,next_run_at=?,requires_action=0 WHERE id=?",params![run_id,time,id]).map_err(sql_error)?;run_id
            };
            record_request(&tx,&operation,request.request_id.as_deref(),&request_digest,run_id,Some(id),"check_request",&time)?;
            let result=get_run(&tx,run_id)?;tx.commit().map_err(sql_error)?;Ok(result)
        }).await
    }

    pub async fn rss_claim_feed(
        &self,
        owner: &str,
        lease_secs: u64,
    ) -> RssResult<Option<FeedClaim>> {
        let owner = owner.to_owned();
        self.rss_read(move |conn| {
            let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(sql_error)?;let time=now();
            let id:Option<i64>=tx.query_row("SELECT id FROM rss_feeds WHERE enabled=1 AND archived_at IS NULL AND requires_action=0
                AND (next_run_at IS NULL OR next_run_at<=?1) AND (cooldown_until IS NULL OR cooldown_until<=?1)
                AND (lease_until IS NULL OR lease_until<?1) ORDER BY next_run_at,id LIMIT 1",[&time],|r|r.get(0)).optional().map_err(sql_error)?;
            let Some(id)=id else {return Ok(None)};
            let existing:Option<i64>=tx.query_row("SELECT current_run_id FROM rss_feeds WHERE id=?",[id],|r|r.get(0)).map_err(sql_error)?;
            let run_id=if let Some(id)=existing.filter(|id|get_run(&tx,*id).is_ok_and(|r|matches!(r.status.as_str(),"queued"|"running"))) {
                tx.execute("UPDATE rss_runs SET status='running' WHERE id=?",[id]).map_err(sql_error)?;id
            } else {
                tx.execute("INSERT INTO rss_runs(feed_id,kind,status,started_at) VALUES(?,'check','running',?)",params![id,time]).map_err(sql_error)?;tx.last_insert_rowid()
            };
            tx.execute("UPDATE rss_feeds SET lease_owner=?,lease_until=?,lease_version=lease_version+1,current_run_id=? WHERE id=?",params![owner,after(lease_secs),run_id,id]).map_err(sql_error)?;
            let version=tx.query_row("SELECT lease_version FROM rss_feeds WHERE id=?",[id],|r|r.get(0)).map_err(sql_error)?;
            let feed=get_feed(&tx,id)?;tx.commit().map_err(sql_error)?;
            Ok(Some(FeedClaim {feed,run_id,owner,version}))
        }).await
    }

    pub async fn rss_renew_feed(&self, claim: &FeedClaim, lease_secs: u64) -> RssResult<bool> {
        let claim = claim.clone();
        self.rss_read(move |conn| {
            Ok(conn.execute("UPDATE rss_feeds SET lease_until=? WHERE id=? AND lease_owner=? AND lease_version=?
                AND version=? AND generation=? AND enabled=1 AND archived_at IS NULL AND lease_until>=?",
                params![after(lease_secs),claim.feed.record.id,claim.owner,claim.version,claim.feed.record.version,claim.feed.record.generation,now()]).map_err(sql_error)?==1)
        }).await
    }

    pub async fn rss_commit_scan(
        &self,
        claim: &FeedClaim,
        fetched: FetchedFeed,
    ) -> RssResult<RunRecord> {
        let claim = claim.clone();
        self.rss_read(move |conn| {
            let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(sql_error)?;let time=now();
            validate_feed_claim(&tx,&claim,&time)?;
            let feed=&claim.feed.record;
            let (initialized,resume,mut sequence):(Option<String>,bool,i64)=tx.query_row("SELECT initialized_at,resume_baseline,last_sequence FROM rss_feeds WHERE id=?",[feed.id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(sql_error)?;
            if fetched.not_modified && initialized.is_none() {return Err(RssError::Invalid("尚未建立本地基线，不能接受 304 响应".into()));}
            // Deferred decisions belong to durable items. Finish the previous
            // check's log when a new cycle takes over, so historical run logs
            // cannot remain active forever merely because a feed has backlog.
            tx.execute("UPDATE rss_runs SET status='completed',finished_at=?,
                message=COALESCE(message || '；','') || '剩余待判定条目已转入后续检查周期'
                WHERE feed_id=? AND kind='check' AND status='processing'",params![time,feed.id]).map_err(sql_error)?;
            // A resume request does not change the existing snapshot. A 304
            // proves no additions during the pause and therefore closes it.
            let baseline=initialized.is_none() || resume;
            let site_can_resolve=if let Some(site_id)=feed.site_id {
                tx.query_row("SELECT site_type,auth_config FROM sites WHERE id=?",[site_id],|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?)))
                    .optional().map_err(sql_error)?.is_some_and(|(kind,auth)|supports_torrent_resolution(&kind,&auth))
            } else {false};
            let mut new_count=0u64;
            for normalized in &fetched.items {
                if normalized.item_key.is_empty() {continue;}
                let seen:Option<(i64,String)>=tx.query_row("SELECT first_sequence,first_seen_at FROM rss_seen_keys WHERE feed_id=? AND generation=? AND item_key=?",
                    params![feed.id,feed.generation,normalized.item_key],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(sql_error)?;
                let (item_sequence,first_seen)=if let Some(seen)=seen {seen} else {
                    sequence+=1;new_count+=1;
                    tx.execute("INSERT INTO rss_seen_keys(feed_id,generation,item_key,first_sequence,first_seen_at) VALUES(?,?,?,?,?)",params![feed.id,feed.generation,normalized.item_key,sequence,time]).map_err(sql_error)?;
                    (sequence,time.clone())
                };
                let current:Option<(i64,String,String,bool)>=tx.query_row("SELECT id,title,attributes_json,downloadable FROM rss_items WHERE feed_id=? AND generation=? AND item_key=?",
                    params![feed.id,feed.generation,normalized.item_key],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional().map_err(sql_error)?;
                let downloadable=normalized.download_url.is_some() || (site_can_resolve && normalized.site_torrent_id.is_some());
                let attributes_json=json(&normalized.attributes)?;
                let item_id=if let Some((id,title,old_attributes,old_downloadable))=current {
                    let changed=title!=normalized.title || old_downloadable!=downloadable || significant_attributes(&old_attributes)!=significant_attributes(&attributes_json);
                    tx.execute("UPDATE rss_items SET title=?,detail_display=?,site_torrent_id=?,published_at=?,categories_json=?,attributes_json=?,
                        downloadable=?,content_revision=content_revision+?,last_seen_at=? WHERE id=?",
                        params![normalized.title,normalized.detail_url.as_deref().map(masked_url),normalized.site_torrent_id,normalized.published_at,json(&normalized.categories)?,attributes_json,downloadable,changed,time,id]).map_err(sql_error)?;id
                } else {
                    tx.execute("INSERT INTO rss_items(feed_id,generation,item_key,sequence,title,detail_display,site_torrent_id,published_at,categories_json,attributes_json,downloadable,first_seen_at,last_seen_at,status)
                        VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?)",params![feed.id,feed.generation,normalized.item_key,item_sequence,normalized.title,normalized.detail_url.as_deref().map(masked_url),normalized.site_torrent_id,normalized.published_at,json(&normalized.categories)?,attributes_json,downloadable,first_seen,time,if baseline {"baseline"} else {"observed"}]).map_err(sql_error)?;
                    tx.last_insert_rowid()
                };
                tx.execute("INSERT INTO rss_item_locators(item_id,feed_generation,download_url,detail_url) VALUES(?,?,?,?)
                    ON CONFLICT(item_id) DO UPDATE SET download_url=excluded.download_url,detail_url=excluded.detail_url",
                    params![item_id,feed.generation,normalized.download_url,normalized.detail_url]).map_err(sql_error)?;
                if !baseline {create_item_decisions(&tx,item_id,&time)?;}
            }
            if baseline {
                tx.execute("UPDATE rss_rule_feeds SET generation=?,activation_sequence=?,baseline_pending=0 WHERE feed_id=?",params![feed.generation,sequence,feed.id]).map_err(sql_error)?;
            }
            tx.execute("UPDATE rss_feeds SET initialized_at=COALESCE(initialized_at,?),resume_baseline=0,last_sequence=?,
                etag=CASE WHEN ? THEN etag ELSE ? END,last_modified=CASE WHEN ? THEN last_modified ELSE ? END,
                last_checked_at=?,next_run_at=?,last_status=?,last_error=NULL,requires_action=0,failure_count=0,cooldown_until=NULL,
                quota_used=0,cycle_id=?,lease_owner=NULL,lease_until=NULL,current_run_id=NULL,updated_at=? WHERE id=?",
                params![time,sequence,fetched.not_modified,fetched.etag,fetched.not_modified,fetched.last_modified,time,
                    after(u64::from(feed.interval_minutes)*60),if baseline {"baseline"} else if fetched.not_modified {"not_modified"} else {"success"},claim.run_id,time,feed.id]).map_err(sql_error)?;
            let pending:i64=tx.query_row("SELECT count(DISTINCT d.item_id) FROM rss_decisions d JOIN rss_items i ON i.id=d.item_id WHERE i.feed_id=? AND i.generation=? AND d.status IN ('pending','ready','attribute_unknown','priority_wait')",params![feed.id,feed.generation],|r|r.get(0)).map_err(sql_error)?;
            tx.execute("UPDATE rss_runs SET status=?,item_count=?,new_count=?,pending_count=?,message=?,finished_at=? WHERE id=?",
                params![if pending>0 {"processing"} else {"completed"},fetched.items.len() as i64,new_count as i64,pending,
                    if baseline {Some("已建立历史基线，后续新增条目才自动匹配".to_string())} else if fetched.warnings.is_empty() {None} else {Some(fetched.warnings.join("；"))},time,claim.run_id]).map_err(sql_error)?;
            let run=get_run(&tx,claim.run_id)?;tx.commit().map_err(sql_error)?;Ok(run)
        }).await
    }

    pub async fn rss_fail_scan(
        &self,
        claim: &FeedClaim,
        message: &str,
        retry_at: Option<String>,
        requires_action: bool,
    ) -> RssResult<()> {
        let claim = claim.clone();
        let message = message.to_owned();
        self.rss_read(move |conn| {
            let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(sql_error)?;let time=now();
            validate_feed_claim(&tx,&claim,&time)?;
            let failures:u32=tx.query_row("SELECT failure_count FROM rss_feeds WHERE id=?",[claim.feed.record.id],|r|r.get(0)).map_err(sql_error)?;
            let next=retry_at.clone().unwrap_or_else(||after([60,300,900,3600][(failures as usize).min(3)]));
            tx.execute("UPDATE rss_feeds SET last_checked_at=?,next_run_at=?,last_status='error',last_error=?,requires_action=?,
                failure_count=failure_count+1,cooldown_until=?,lease_owner=NULL,lease_until=NULL,current_run_id=NULL,updated_at=? WHERE id=?",
                params![time,next,message,requires_action,retry_at,time,claim.feed.record.id]).map_err(sql_error)?;
            tx.execute("UPDATE rss_runs SET status='failed',message=?,finished_at=? WHERE id=?",params![message,time,claim.run_id]).map_err(sql_error)?;
            tx.commit().map_err(sql_error)?;Ok(())
        }).await
    }

    pub async fn rss_claim_evaluation(
        &self,
        owner: &str,
        lease_secs: u64,
    ) -> RssResult<Option<EvaluationClaim>> {
        let owner = owner.to_owned();
        self.rss_read(move |conn| {
            let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(sql_error)?;let time=now();
            let id:Option<i64>=tx.query_row("SELECT i.id FROM rss_items i JOIN rss_feeds f ON f.id=i.feed_id
                WHERE f.enabled=1 AND f.archived_at IS NULL AND i.generation=f.generation
                AND (i.evaluation_until IS NULL OR i.evaluation_until<?1)
                AND EXISTS(SELECT 1 FROM rss_decisions d JOIN rss_rules r ON r.id=d.rule_id
                    WHERE d.item_id=i.id AND d.match_revision=r.match_revision AND r.enabled=1 AND r.archived_at IS NULL
                    AND d.status IN ('pending','ready','attribute_unknown','priority_wait')
                    AND (d.next_evaluate_at IS NULL OR d.next_evaluate_at<=?1)
                    AND (d.status!='ready' OR f.quota_used<20))
                ORDER BY i.first_seen_at,i.id LIMIT 1",[&time],|r|r.get(0)).optional().map_err(sql_error)?;
            let Some(id)=id else {return Ok(None)};
            tx.execute("UPDATE rss_items SET evaluation_owner=?,evaluation_until=?,evaluation_version=evaluation_version+1 WHERE id=?",params![owner,after(lease_secs),id]).map_err(sql_error)?;
            let version=tx.query_row("SELECT evaluation_version FROM rss_items WHERE id=?",[id],|r|r.get(0)).map_err(sql_error)?;
            let item=get_item(&tx,id)?;let feed=get_feed(&tx,item.feed_id)?;
            let rules=evaluation_rules(&tx,id)?;
            tx.commit().map_err(sql_error)?;Ok(Some(EvaluationClaim {item,feed,rules,owner,version}))
        }).await
    }

    pub async fn rss_renew_evaluation(
        &self,
        claim: &EvaluationClaim,
        lease_secs: u64,
    ) -> RssResult<bool> {
        let claim = claim.clone();
        self.rss_read(move |conn| {
            Ok(conn.execute("UPDATE rss_items SET evaluation_until=? WHERE id=? AND evaluation_owner=? AND evaluation_version=?
                AND content_revision=? AND evaluation_until>=?",params![after(lease_secs),claim.item.id,claim.owner,claim.version,claim.item.content_revision,now()]).map_err(sql_error)?==1)
        }).await
    }

    pub async fn rss_commit_evaluation(
        &self,
        claim: &EvaluationClaim,
        attributes: Option<ItemAttributes>,
    ) -> RssResult<u64> {
        let claim = claim.clone();
        self.rss_read(move |conn| {
            let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(sql_error)?;let time=now();
            let valid:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM rss_items WHERE id=? AND evaluation_owner=? AND evaluation_version=? AND content_revision=? AND evaluation_until>=?)",
                params![claim.item.id,claim.owner,claim.version,claim.item.content_revision,time],|r|r.get(0)).map_err(sql_error)?;
            if !valid {return Err(conflict());}
            let feed=get_feed(&tx,claim.item.feed_id)?;
            if !feed.record.enabled || feed.record.generation!=claim.item.generation || feed.record.version!=claim.feed.record.version {return Err(conflict());}
            // Compare every rule before committing any choice, preserving stable
            // priority even when a slow attribute lookup overlaps an edit.
            for rule in &claim.rules {
                let current=get_rule(&tx,rule.id)?;
                if !current.enabled || current.version!=rule.version {return Err(conflict());}
            }
            let mut item=get_item(&tx,claim.item.id)?;
            if let Some(attributes)=attributes {
                item.attributes=attributes;
                item.content_revision=store_item_attributes(&tx,item.id,&item.attributes,&time)?;
            }
            // Enrichment can make an earlier static rejection eligible. Include
            // those newly pending rules before selecting a lower-priority one.
            let rules=evaluation_rules(&tx,item.id)?;
            let mut chosen=HashSet::new();let mut deferred=HashSet::new();let mut queued=0u64;
            let mut quota:u32=tx.query_row("SELECT quota_used FROM rss_feeds WHERE id=?",[feed.record.id],|r|r.get(0)).map_err(sql_error)?;
            for rule in &rules {
                let Some(downloader)=rule.downloader_id else {continue};
                let existing:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM rss_download_jobs WHERE item_identity=? AND downloader_identity=?)",params![item.id,downloader],|r|r.get(0)).map_err(sql_error)?;
                if existing {chosen.insert(downloader);}
                let evaluation=matcher::evaluate(&rule.filters,&item);
                let (status,evidence,next,job_id)=if chosen.contains(&downloader) {
                    ("lower_priority",reason("lower_priority","同一下载器已由优先级更高的规则决定"),None,None)
                } else if deferred.contains(&downloader) {
                    ("priority_wait",reason("higher_priority_pending","优先级更高的规则正在等待属性确认"),Some(after(60)),None)
                } else if evaluation.needs_attributes {
                    deferred.insert(downloader);("attribute_unknown",evaluation,Some(after(60)),None)
                } else if !evaluation.matched {
                    ("skipped",evaluation,None,None)
                } else if quota>=20 {
                    // A ready decision has already selected this rule. Prevent
                    // lower priority rules from overtaking it in this cycle.
                    deferred.insert(downloader);("ready",evaluation,None,None)
                } else {
                    let (job_id,created,successful)=enqueue_job(&tx,&feed,&item,rule,&evaluation,&time)?;
                    chosen.insert(downloader);
                    if created {quota+=1;queued+=1;}
                    (if successful {"already_present"} else {"queued"},evaluation,None,job_id)
                };
                tx.execute("UPDATE rss_decisions SET status=?,evaluation_json=?,checked_at=?,next_evaluate_at=?,job_id=?,item_revision=?,version=version+1
                    WHERE item_id=? AND rule_id=? AND match_revision=?",params![status,json(&evidence)?,time,next,job_id,item.content_revision,item.id,rule.id,rule.match_revision]).map_err(sql_error)?;
            }
            tx.execute("UPDATE rss_items SET evaluation_owner=NULL,evaluation_until=NULL WHERE id=?",[item.id]).map_err(sql_error)?;
            tx.execute("UPDATE rss_feeds SET quota_used=? WHERE id=?",params![quota,feed.record.id]).map_err(sql_error)?;
            refresh_item_status(&tx,item.id)?;
            refresh_run_counts(&tx,feed.record.id,queued,&time)?;
            tx.commit().map_err(sql_error)?;Ok(queued)
        }).await
    }

    pub async fn rss_backfill(&self, mut request: BackfillRequest) -> RssResult<RunRecord> {
        request.item_ids.sort_unstable();
        request.item_ids.dedup();
        self.rss_read(move |conn| {
            if request.item_ids.is_empty() || request.item_ids.len()>200 {return Err(RssError::Invalid("每次补下请选择 1–200 个已保存条目".into()));}
            let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(sql_error)?;let time=now();
            let operation="backfill";
            let request_digest=digest(&format!("{}:{}:{}",request.rule_id,request.expected_version,json(&request.item_ids)?));
            if let Some(id)=request_lookup(&tx,operation,Some(&request.request_id),&request_digest)? {return get_run(&tx,id);}
            let rule=get_rule(&tx,request.rule_id)?;check_version(rule.version,Some(request.expected_version))?;
            if !rule.enabled {return Err(RssError::Conflict("规则已暂停，请恢复后再补下".into()));}
            validate_rule_target(&tx,true,rule.downloader_id)?;
            for id in &request.item_ids {
                let item=get_item(&tx,*id)?;let feed=get_feed(&tx,item.feed_id)?;
                if !feed.record.enabled || feed.record.generation!=item.generation || !rule.feed_ids.contains(&item.feed_id) {
                    return Err(RssError::Conflict("所选条目不在当前启用的规则来源中".into()));
                }
                tx.execute("INSERT INTO rss_decisions(item_id,rule_id,match_revision,item_revision,status,manual,next_evaluate_at,evaluation_json)
                    VALUES(?,?,?,?,'pending',1,?,?) ON CONFLICT(item_id,rule_id,match_revision) DO UPDATE SET
                    status=CASE WHEN rss_decisions.job_id IS NULL AND rss_decisions.status NOT IN ('lower_priority','already_present') THEN 'pending' ELSE rss_decisions.status END,
                    manual=1,next_evaluate_at=CASE WHEN rss_decisions.job_id IS NULL THEN excluded.next_evaluate_at ELSE NULL END,
                    version=rss_decisions.version+1",params![id,rule.id,rule.match_revision,item.content_revision,time,json(&MatchEvaluation::default())?]).map_err(sql_error)?;
                refresh_item_status(&tx,*id)?;
            }
            tx.execute("INSERT INTO rss_runs(kind,status,item_count,pending_count,started_at,operation,request_id,request_digest)
                VALUES('backfill','processing',?,?,?,?,?,?)",params![request.item_ids.len() as i64,request.item_ids.len() as i64,time,operation,request.request_id,request_digest]).map_err(sql_error)?;
            let run_id=tx.last_insert_rowid();tx.execute("UPDATE rss_runs SET result_ref=? WHERE id=?",params![run_id,run_id]).map_err(sql_error)?;
            for item_id in &request.item_ids {
                tx.execute("INSERT INTO rss_run_items(run_id,item_id,rule_id,match_revision) VALUES(?,?,?,?)",params![run_id,item_id,rule.id,rule.match_revision]).map_err(sql_error)?;
            }
            refresh_backfill_runs(&tx,&time)?;
            let run=get_run(&tx,run_id)?;tx.commit().map_err(sql_error)?;Ok(run)
        }).await
    }

    pub async fn rss_claim_job(&self, owner: &str, lease_secs: u64) -> RssResult<Option<JobClaim>> {
        let owner = owner.to_owned();
        self.rss_read(move |conn| {
            let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(sql_error)?;let time=now();
            recover_expired(&tx,&time)?;
            synchronize_jobs(&tx,&time)?;
            let mut stmt=tx.prepare("SELECT id,downloader_id,infohash,status FROM rss_download_jobs j
                WHERE downloader_id IS NOT NULL AND status IN ('queued','waiting','retry_wait','reconciling')
                AND (next_attempt_at IS NULL OR next_attempt_at<=?1) AND (lease_until IS NULL OR lease_until<?1)
                AND NOT EXISTS(SELECT 1 FROM rss_downloader_slots s WHERE s.downloader_id=j.downloader_id AND s.job_id!=j.id)
                ORDER BY CASE WHEN status='reconciling' THEN 0 ELSE 1 END,next_attempt_at,id LIMIT 100").map_err(sql_error)?;
            let candidates=stmt.query_map([&time],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?,r.get::<_,Option<String>>(2)?,r.get::<_,String>(3)?))).map_err(sql_error)?.collect::<Result<Vec<_>,_>>().map_err(sql_error)?;drop(stmt);
            for (id,downloader,hash,status) in candidates {
                if status!="reconciling" && relocation_conflict(&tx,downloader,hash.as_deref(),&time)? {continue;}
                let until=after(lease_secs);
                tx.execute("UPDATE rss_download_jobs SET status=CASE WHEN status='reconciling' THEN status ELSE 'fetching' END,
                    lease_owner=?,lease_until=?,version=version+1,updated_at=? WHERE id=?",params![owner,until,time,id]).map_err(sql_error)?;
                tx.execute("INSERT INTO rss_downloader_slots(downloader_id,job_id,owner,lease_until) VALUES(?,?,?,?)
                    ON CONFLICT(downloader_id) DO UPDATE SET owner=excluded.owner,lease_until=excluded.lease_until,version=rss_downloader_slots.version+1
                    WHERE rss_downloader_slots.job_id=excluded.job_id",params![downloader,id,owner,until]).map_err(sql_error)?;
                let claim=job_claim(&tx,id,&owner)?;tx.commit().map_err(sql_error)?;return Ok(Some(claim));
            }
            tx.commit().map_err(sql_error)?;Ok(None)
        }).await
    }

    pub async fn rss_renew_job(&self, claim: &JobClaim, lease_secs: u64) -> RssResult<bool> {
        let claim = claim.clone();
        self.rss_read(move |conn| {
            let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(sql_error)?;let time=now();let until=after(lease_secs);
            let changed=tx.execute("UPDATE rss_download_jobs SET lease_until=? WHERE id=? AND version=? AND lease_owner=? AND lease_until>=?
                AND status IN ('fetching','submitting','reconciling')",params![until,claim.job.id,claim.job.version,claim.owner,time]).map_err(sql_error)?==1;
            if changed {
                tx.execute("UPDATE rss_downloader_slots SET lease_until=? WHERE downloader_id=? AND job_id=? AND owner=?",params![until,claim.job.downloader_id,claim.job.id,claim.owner]).map_err(sql_error)?;
            }
            tx.commit().map_err(sql_error)?;Ok(changed)
        }).await
    }

    pub async fn rss_reserved_bytes(
        &self,
        downloader_id: i64,
        exclude_job_id: i64,
    ) -> RssResult<u64> {
        self.rss_read(move |conn| {
            Ok(conn.query_row("SELECT COALESCE(sum(reserved_bytes),0) FROM rss_download_jobs WHERE downloader_id=? AND id!=?",params![downloader_id,exclude_job_id],|r|r.get(0)).map_err(sql_error)?)
        }).await
    }

    pub async fn rss_release_observed_reservations(
        &self,
        downloader_id: i64,
        hashes: Vec<String>,
        sampled_at: &str,
    ) -> RssResult<u64> {
        let sampled_at = sampled_at.to_owned();
        self.rss_read(move |conn| {
            if chrono::DateTime::parse_from_rfc3339(&sampled_at).is_err() {return Err(RssError::Invalid("下载器采集时间无效".into()));}
            let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(sql_error)?;let mut released=0;
            for hash in hashes {
                released+=tx.execute("UPDATE rss_download_jobs SET reserved_bytes=0,sampled_at=? WHERE downloader_id=? AND infohash=?
                    AND status='submitted' AND submitted_at<=? AND reserved_bytes>0",params![sampled_at,downloader_id,hash.to_ascii_lowercase(),sampled_at]).map_err(sql_error)? as u64;
            }
            tx.commit().map_err(sql_error)?;Ok(released)
        }).await
    }

    pub async fn rss_prepare_submission(
        &self,
        claim: &JobClaim,
        infohash: &str,
        reserved_bytes: u64,
        attributes: Option<ItemAttributes>,
    ) -> RssResult<Option<JobClaim>> {
        let claim = claim.clone();
        let infohash = normalize_hash(infohash)?;
        self.rss_read(move |conn| {
            let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(sql_error)?;let time=now();
            validate_job_claim(&tx,&claim,&time)?;
            if claim.job.status!="fetching" {return Err(conflict());}
            let slot:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM rss_downloader_slots WHERE downloader_id=? AND job_id=? AND owner=? AND lease_until>=?)",
                params![claim.job.downloader_id,claim.job.id,claim.owner,time],|r|r.get(0)).map_err(sql_error)?;
            if !slot {return Err(conflict());}
            if claim.job.infohash.as_deref().is_some_and(|hash|hash!=infohash) {
                finish_job_state(&tx,&claim.job,"failed",Some("来源返回的种子哈希已改变，不能按原任务重投"),None,None,true,&time)?;
                tx.commit().map_err(sql_error)?;return Ok(None);
            }
            // The check and the submitting transition share this write lock with
            // pause/cancel, source replacement, and transfer creation.
            let active:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM rss_download_jobs j JOIN rss_feeds f ON f.id=j.feed_id JOIN rss_rules r ON r.id=j.rule_id
                WHERE j.id=? AND f.enabled=1 AND r.enabled=1 AND f.archived_at IS NULL AND r.archived_at IS NULL
                AND j.feed_generation=f.generation AND j.downloader_id IS NOT NULL)",[claim.job.id],|r|r.get(0)).map_err(sql_error)?;
            if !active {
                synchronize_jobs(&tx,&time)?;tx.commit().map_err(sql_error)?;return Ok(None);
            }
            let feed_version:i64=tx.query_row("SELECT version FROM rss_feeds WHERE id=?",[claim.job.feed_id],|r|r.get(0)).map_err(sql_error)?;
            if feed_version!=claim.feed.record.version {return Err(conflict());}
            if relocation_conflict(&tx,claim.job.downloader_id,Some(&infohash),&time)? {
                finish_job_state(&tx,&claim.job,"waiting",Some("该种子正在转移，稍后重试"),Some(&after(60)),None,true,&time)?;
                tx.commit().map_err(sql_error)?;return Ok(None);
            }
            let mut item=get_item(&tx,claim.job.item_id)?;
            if item.content_revision!=claim.item.content_revision {return Err(conflict());}
            if let Some(attributes)=attributes {item.attributes=attributes;}
            item.content_revision=store_item_attributes(&tx,item.id,&item.attributes,&time)?;
            let evaluation=matcher::evaluate(&claim.job.filters_snapshot,&item);
            let needs_fresh=claim.job.filters_snapshot.free_only || claim.job.filters_snapshot.hr_policy=="require_clear";
            let recent=chrono::DateTime::parse_from_rfc3339(&item.attributes.observed_at).ok()
                .is_some_and(|t|(Utc::now()-t.with_timezone(&Utc)).num_seconds().abs()<=60);
            if !evaluation.matched || (needs_fresh && !recent) {
                let message=if needs_fresh && !recent {"免费或 H&R 证据已过期，等待重新确认".to_owned()}
                    else {evaluation.reasons.iter().map(|r|r.message.as_str()).collect::<Vec<_>>().join("；")};
                finish_job_state(&tx,&claim.job,"waiting",Some(&message),Some(&after(60)),None,true,&time)?;
                tx.commit().map_err(sql_error)?;return Ok(None);
            }
            let prior:Option<(Option<i64>,String)>=tx.query_row("SELECT job_id,outcome FROM rss_delivery_keys WHERE downloader_id=? AND key_kind='infohash' AND key_digest=?",
                params![claim.job.downloader_id,infohash],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(sql_error)?;
            if let Some((job_id,outcome))=&prior {
                if *job_id!=Some(claim.job.id) {
                    if matches!(outcome.as_str(),"submitted"|"already_present") {
                        tx.execute("UPDATE rss_download_jobs SET infohash=? WHERE id=?",params![infohash,claim.job.id]).map_err(sql_error)?;
                        finish_job_state(&tx,&claim.job,"already_present",None,None,None,true,&time)?;
                        tx.commit().map_err(sql_error)?;return Ok(None);
                    }
                    if !matches!(outcome.as_str(),"released"|"cancelled"|"failed") {
                        finish_job_state(&tx,&claim.job,"waiting",Some("另一任务正在确认同一 infohash 的投递结果"),Some(&after(60)),None,true,&time)?;
                        tx.commit().map_err(sql_error)?;return Ok(None);
                    }
                }
            }
            tx.execute("INSERT INTO rss_delivery_keys(downloader_id,key_kind,key_digest,job_id,outcome,created_at,updated_at)
                VALUES(?,'infohash',?,?,'submitting',?,?) ON CONFLICT(downloader_id,key_kind,key_digest)
                DO UPDATE SET job_id=excluded.job_id,outcome='submitting',updated_at=excluded.updated_at
                WHERE rss_delivery_keys.job_id=excluded.job_id OR rss_delivery_keys.outcome IN ('released','cancelled','failed')",
                params![claim.job.downloader_id,infohash,claim.job.id,time,time]).map_err(sql_error)?;
            tx.execute("UPDATE rss_download_jobs SET status='submitting',infohash=?,reserved_bytes=?,decision_json=?,size_bytes=?,
                attempts=attempts+1,version=version+1,last_error=NULL,updated_at=?,submission_started_at=? WHERE id=?",
                params![infohash,reserved_bytes.min(i64::MAX as u64) as i64,json(&evaluation)?,item.attributes.size_bytes.map(|v|v.min(i64::MAX as u64) as i64),time,time,claim.job.id]).map_err(sql_error)?;
            let claim=job_claim(&tx,claim.job.id,&claim.owner)?;tx.commit().map_err(sql_error)?;Ok(Some(claim))
        }).await
    }

    pub async fn rss_transition_job(
        &self,
        claim: &JobClaim,
        update: JobTransition,
    ) -> RssResult<JobRecord> {
        let claim = claim.clone();
        self.rss_read(move |conn| {
            let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(sql_error)?;let time=now();
            validate_job_claim(&tx,&claim,&time)?;
            if !allowed_transition(&claim.job.status,&update.status) {return Err(RssError::Conflict("当前下载任务不允许此状态转换".into()));}
            if matches!(claim.job.status.as_str(),"submitting"|"reconciling") && matches!(update.status.as_str(),"retry_wait"|"failed") && !update.release_reservation {
                return Err(RssError::Conflict("尚未确认远端不存在种子，不能重投".into()));
            }
            if let Some(hash)=update.infohash.as_deref() {
                let hash=normalize_hash(hash)?;
                if update.status!="already_present" && claim.job.infohash.as_deref()!=Some(&hash) {return Err(conflict());}
                tx.execute("UPDATE rss_download_jobs SET infohash=? WHERE id=?",params![hash,claim.job.id]).map_err(sql_error)?;
                if update.status=="already_present" {
                    tx.execute("INSERT INTO rss_delivery_keys(downloader_id,key_kind,key_digest,job_id,outcome,created_at,updated_at)
                        VALUES(?,'infohash',?,?,'already_present',?,?) ON CONFLICT(downloader_id,key_kind,key_digest)
                        DO UPDATE SET outcome='already_present',updated_at=excluded.updated_at",params![claim.job.downloader_id,hash,claim.job.id,time,time]).map_err(sql_error)?;
                }
            }
            finish_job_state(&tx,&claim.job,&update.status,update.last_error.as_deref(),update.next_attempt_at.as_deref(),Some(&update),update.release_reservation,&time)?;
            let result=get_job(&tx,claim.job.id)?;tx.commit().map_err(sql_error)?;Ok(result)
        }).await
    }

    pub async fn rss_job_action(
        &self,
        id: i64,
        action: &str,
        request: ActionRequest,
    ) -> RssResult<JobRecord> {
        let action = action.to_owned();
        self.rss_read(move |conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sql_error)?;
            let time = now();
            let operation = format!("job:{action}:{id}");
            let request_digest = digest(&json(&request)?);
            if let Some(id) = request_lookup(
                &tx,
                &operation,
                request.request_id.as_deref(),
                &request_digest,
            )? {
                return get_job(&tx, id);
            }
            let job = get_job(&tx, id)?;
            check_version(job.version, request.expected_version)?;
            let next = match action.as_str() {
                "cancel"
                    if matches!(
                        job.status.as_str(),
                        "queued" | "held" | "fetching" | "waiting" | "retry_wait" | "failed"
                    ) =>
                {
                    "cancelled"
                }
                "retry" if matches!(job.status.as_str(), "failed" | "waiting" | "retry_wait") => {
                    let feed = get_feed(&tx, job.feed_id)?;
                    let rule = get_rule(&tx, job.rule_id)?;
                    if !feed.record.enabled
                        || !rule.enabled
                        || feed.record.generation != job.feed_generation
                    {
                        return Err(RssError::Conflict(
                            "来源或规则已暂停或更换，请先修复配置".into(),
                        ));
                    }
                    validate_rule_target(&tx, true, Some(job.downloader_id))?;
                    "queued"
                }
                "reconcile" if matches!(job.status.as_str(), "submitting" | "reconciling") => {
                    // An in-flight add must retain its lease. The action merely
                    // makes a later poll due; it cannot fence a live add request.
                    let until: Option<String> = tx
                        .query_row(
                            "SELECT lease_until FROM rss_download_jobs WHERE id=?",
                            [id],
                            |r| r.get(0),
                        )
                        .map_err(sql_error)?;
                    if until.is_some_and(|t| t >= time) {
                        return Err(RssError::Conflict(
                            "任务正在投递或对账，请等待当前请求结束".into(),
                        ));
                    }
                    "reconciling"
                }
                _ => {
                    return Err(RssError::Conflict(
                        "当前状态不支持此操作；已发出的提交必须先对账".into(),
                    ));
                }
            };
            let release = next != "reconciling";
            finish_job_state(&tx, &job, next, None, Some(&time), None, release, &time)?;
            if action == "retry" {
                tx.execute("UPDATE rss_download_jobs SET attempts=0 WHERE id=?", [id])
                    .map_err(sql_error)?;
            }
            record_request(
                &tx,
                &operation,
                request.request_id.as_deref(),
                &request_digest,
                id,
                Some(job.feed_id),
                "job_action",
                &time,
            )?;
            let result = get_job(&tx, id)?;
            tx.commit().map_err(sql_error)?;
            Ok(result)
        })
        .await
    }

    pub async fn rss_recover(&self) -> RssResult<u64> {
        self.rss_read(move |conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sql_error)?;
            let time = now();
            let count = recover_expired(&tx, &time)?;
            synchronize_jobs(&tx, &time)?;
            refresh_backfill_runs(&tx, &time)?;
            tx.commit().map_err(sql_error)?;
            Ok(count)
        })
        .await
    }

    pub async fn rss_release_owner(&self, owner: &str) -> RssResult<()> {
        let owner = owner.to_owned();
        self.rss_read(move |conn| {
            let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(sql_error)?;let time=now();
            tx.execute("UPDATE rss_feeds SET lease_owner=NULL,lease_until=NULL,lease_version=lease_version+1 WHERE lease_owner=?",[&owner]).map_err(sql_error)?;
            tx.execute("UPDATE rss_items SET evaluation_owner=NULL,evaluation_until=NULL,evaluation_version=evaluation_version+1 WHERE evaluation_owner=?",[&owner]).map_err(sql_error)?;
            tx.execute("UPDATE rss_download_jobs SET status=CASE WHEN status IN ('submitting','reconciling') THEN 'reconciling' ELSE 'queued' END,
                lease_owner=NULL,lease_until=NULL,version=version+1,next_attempt_at=?,updated_at=?,
                reserved_bytes=CASE WHEN status IN ('submitting','reconciling') THEN reserved_bytes ELSE 0 END
                WHERE lease_owner=? AND status IN ('fetching','submitting','reconciling')",params![time,time,owner]).map_err(sql_error)?;
            tx.execute("DELETE FROM rss_downloader_slots WHERE owner=? AND job_id IN(SELECT id FROM rss_download_jobs WHERE status!='reconciling')",[&owner]).map_err(sql_error)?;
            tx.execute("UPDATE rss_delivery_keys SET outcome='released',updated_at=? WHERE key_kind='infohash' AND outcome NOT IN ('submitted','already_present') AND job_id IN(SELECT id FROM rss_download_jobs WHERE status='queued')",[&time]).map_err(sql_error)?;
            tx.commit().map_err(sql_error)?;Ok(())
        }).await
    }

    pub async fn rss_cleanup(&self) -> RssResult<u64> {
        self.rss_read(move |conn| {
            let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(sql_error)?;
            let cutoff=(Utc::now()-Duration::days(30)).to_rfc3339();let time=now();
            // Active work, uncertain outcomes and unsampled reservations retain
            // their full evidence and locators regardless of log age.
            tx.execute("UPDATE rss_decisions SET job_id=NULL WHERE job_id IN(SELECT id FROM rss_download_jobs WHERE status IN ('submitted','already_present','failed','cancelled') AND reserved_bytes=0 AND updated_at<? AND (lease_until IS NULL OR lease_until<?))",params![cutoff,time]).map_err(sql_error)?;
            let mut removed=tx.execute("DELETE FROM rss_download_jobs WHERE status IN ('submitted','already_present','failed','cancelled') AND reserved_bytes=0 AND updated_at<? AND (lease_until IS NULL OR lease_until<?)",params![cutoff,time]).map_err(sql_error)? as u64;
            removed+=tx.execute("DELETE FROM rss_items WHERE last_seen_at<?1 AND (evaluation_until IS NULL OR evaluation_until<?2)
                AND NOT EXISTS(SELECT 1 FROM rss_download_jobs j WHERE j.item_id=rss_items.id)
                AND NOT EXISTS(SELECT 1 FROM rss_decisions d JOIN rss_rules r ON r.id=d.rule_id JOIN rss_feeds f ON f.id=rss_items.feed_id
                    WHERE d.item_id=rss_items.id AND d.match_revision=r.match_revision AND r.archived_at IS NULL AND f.archived_at IS NULL
                    AND d.status IN ('pending','ready','attribute_unknown','priority_wait'))",params![cutoff,time]).map_err(sql_error)? as u64;
            removed+=tx.execute("DELETE FROM rss_runs WHERE started_at<? AND status NOT IN ('queued','running','processing')",[&cutoff]).map_err(sql_error)? as u64;
            // Successful and failed coarse identity keys survive cleanup. Hash
            // reservations that were confirmed unused carry no historical value.
            tx.execute("DELETE FROM rss_delivery_keys WHERE key_kind='infohash' AND outcome IN ('released','cancelled','failed') AND job_id IS NULL",[]).map_err(sql_error)?;
            tx.commit().map_err(sql_error)?;Ok(removed)
        }).await
    }
}

fn job_claim(conn: &Connection, id: i64, owner: &str) -> RssResult<JobClaim> {
    let job = get_job(conn, id)?;
    let item = get_item(conn, job.item_id)?;
    // Reconciliation must still work after an archive. Use the original feed
    // generation's private URL only for internal context, never to fetch it.
    let record = conn
        .query_row(
            &format!("{FEED_SELECT} WHERE f.id=?"),
            [job.feed_id],
            map_feed,
        )
        .map_err(sql_error)?;
    let url: String = conn
        .query_row(
            "SELECT url FROM rss_feed_secrets WHERE feed_id=? AND generation=?",
            params![job.feed_id, job.feed_generation],
            |r| r.get(0),
        )
        .map_err(sql_error)?;
    let feed = StoredFeed {
        record,
        url,
        etag: None,
        last_modified: None,
    };
    let locator = get_locator(conn, job.item_id)?;
    let submission_started_at = conn
        .query_row(
            "SELECT submission_started_at FROM rss_download_jobs WHERE id=?",
            [id],
            |r| r.get(0),
        )
        .map_err(sql_error)?;
    Ok(JobClaim {
        job,
        item,
        feed,
        locator,
        owner: owner.to_owned(),
        submission_started_at,
    })
}
fn validate_job_claim(conn: &Connection, claim: &JobClaim, time: &str) -> RssResult<()> {
    let valid:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM rss_download_jobs WHERE id=? AND version=? AND lease_owner=? AND lease_until>=? AND status=?)",
        params![claim.job.id,claim.job.version,claim.owner,time,claim.job.status],|r|r.get(0)).map_err(sql_error)?;
    if !valid {
        return Err(conflict());
    }
    Ok(())
}
fn normalize_hash(hash: &str) -> RssResult<String> {
    let hash = hash.trim().to_ascii_lowercase();
    if !matches!(hash.len(), 40 | 64) || !hash.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err(RssError::Invalid("种子 infohash 无效".into()));
    }
    Ok(hash)
}
fn allowed_transition(from: &str, to: &str) -> bool {
    match from {
        "fetching" => matches!(
            to,
            "waiting" | "retry_wait" | "failed" | "cancelled" | "held" | "already_present"
        ),
        "submitting" => matches!(to, "submitted" | "reconciling"),
        "reconciling" => matches!(
            to,
            "submitted" | "reconciling" | "retry_wait" | "already_present" | "failed"
        ),
        _ => false,
    }
}
fn finish_job_state(
    conn: &Connection,
    job: &JobRecord,
    status: &str,
    error: Option<&str>,
    next: Option<&str>,
    update: Option<&JobTransition>,
    release: bool,
    time: &str,
) -> RssResult<()> {
    let uncertain = matches!(status, "submitting" | "reconciling");
    let terminal_success = matches!(status, "submitted" | "already_present");
    let release = release && !uncertain;
    let attempts = if matches!(status, "retry_wait" | "failed") && job.status == "fetching" {
        job.attempts + 1
    } else {
        job.attempts
    };
    let status = if status == "retry_wait" && attempts >= 5 {
        "failed"
    } else {
        status
    };
    conn.execute("UPDATE rss_download_jobs SET status=?,last_error=?,next_attempt_at=?,lease_owner=NULL,lease_until=NULL,
        version=version+1,updated_at=?,attempts=?,reserved_bytes=CASE WHEN ? THEN 0 ELSE reserved_bytes END,
        submitted_at=CASE WHEN ? THEN COALESCE(submitted_at,?) ELSE submitted_at END,
        download_state=COALESCE(?,download_state),progress=COALESCE(?,progress),sampled_at=COALESCE(?,sampled_at) WHERE id=?",
        params![status,error,next,time,attempts,release,terminal_success,time,
            update.and_then(|u|u.download_state.as_deref()),update.and_then(|u|u.progress),update.and_then(|u|u.sampled_at.as_deref()),job.id]).map_err(sql_error)?;
    let outcome = if terminal_success {
        status
    } else if uncertain {
        "unknown"
    } else {
        status
    };
    conn.execute("UPDATE rss_delivery_keys SET outcome=CASE WHEN key_kind='infohash' AND ? AND ?=0 THEN 'released' ELSE ? END,
        updated_at=? WHERE job_id=? AND outcome NOT IN ('submitted','already_present')",params![release,terminal_success,outcome,time,job.id]).map_err(sql_error)?;
    if !uncertain {
        conn.execute("DELETE FROM rss_downloader_slots WHERE job_id=?", [job.id])
            .map_err(sql_error)?;
    }
    if terminal_success {
        conn.execute(
            "UPDATE rss_items SET status='submitted' WHERE id=?",
            [job.item_id],
        )
        .map_err(sql_error)?;
    }
    Ok(())
}
fn recover_expired(conn: &Connection, time: &str) -> RssResult<u64> {
    let mut changed = conn
        .execute(
            "UPDATE rss_download_jobs SET status='reconciling',lease_owner=NULL,lease_until=NULL,
        version=version+1,next_attempt_at=?,last_error='投递结果尚未确认，正在对账',updated_at=?
        WHERE status IN ('submitting','reconciling') AND lease_until IS NOT NULL AND lease_until<?",
            params![time, time, time],
        )
        .map_err(sql_error)? as u64;
    changed+=conn.execute("UPDATE rss_download_jobs SET status='queued',lease_owner=NULL,lease_until=NULL,version=version+1,
        reserved_bytes=0,next_attempt_at=?,updated_at=? WHERE status='fetching' AND (lease_until IS NULL OR lease_until<?)",params![time,time,time]).map_err(sql_error)? as u64;
    conn.execute("DELETE FROM rss_downloader_slots WHERE job_id IN(SELECT id FROM rss_download_jobs WHERE status NOT IN ('fetching','submitting','reconciling'))",[]).map_err(sql_error)?;
    conn.execute("UPDATE rss_delivery_keys SET outcome='released',updated_at=? WHERE key_kind='infohash' AND outcome NOT IN ('submitted','already_present') AND job_id IN(SELECT id FROM rss_download_jobs WHERE status='queued')",[time]).map_err(sql_error)?;
    conn.execute("UPDATE rss_items SET evaluation_owner=NULL,evaluation_until=NULL,evaluation_version=evaluation_version+1 WHERE evaluation_until<?",[time]).map_err(sql_error)?;
    Ok(changed)
}
fn relocation_conflict(
    conn: &Connection,
    downloader: i64,
    hash: Option<&str>,
    time: &str,
) -> RssResult<bool> {
    Ok(conn.query_row("SELECT EXISTS(SELECT 1 FROM media_relocation_jobs WHERE (downloader_id=?1 OR target_downloader_id=?1)
        AND (?2 IS NULL OR lower(infohash)=?2) AND (stage NOT IN ('completed','cancelled') OR lease_until>=?3))",params![downloader,hash,time],|r|r.get(0)).map_err(sql_error)?)
}

fn validate_feed_claim(conn: &Connection, claim: &FeedClaim, time: &str) -> RssResult<()> {
    let valid:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM rss_feeds WHERE id=? AND version=? AND generation=?
        AND lease_owner=? AND lease_version=? AND lease_until>=? AND enabled=1 AND archived_at IS NULL)",
        params![claim.feed.record.id,claim.feed.record.version,claim.feed.record.generation,claim.owner,claim.version,time],|r|r.get(0)).map_err(sql_error)?;
    if !valid {
        return Err(conflict());
    }
    Ok(())
}
fn significant_attributes(value: &str) -> serde_json::Value {
    let mut value = serde_json::from_str::<serde_json::Value>(value).unwrap_or_default();
    if let Some(object) = value.as_object_mut() {
        object.remove("observed_at");
        object.remove("hints");
    }
    value
}
fn store_item_attributes(
    conn: &Connection,
    item_id: i64,
    attributes: &ItemAttributes,
    time: &str,
) -> RssResult<i64> {
    let current: String = conn
        .query_row(
            "SELECT attributes_json FROM rss_items WHERE id=?",
            [item_id],
            |r| r.get(0),
        )
        .map_err(sql_error)?;
    let value = json(attributes)?;
    let changed = significant_attributes(&current) != significant_attributes(&value);
    conn.execute(
        "UPDATE rss_items SET attributes_json=?,content_revision=content_revision+? WHERE id=?",
        params![value, changed, item_id],
    )
    .map_err(sql_error)?;
    if changed {
        create_item_decisions(conn, item_id, time)?;
    }
    Ok(conn
        .query_row(
            "SELECT content_revision FROM rss_items WHERE id=?",
            [item_id],
            |r| r.get(0),
        )
        .map_err(sql_error)?)
}
fn evaluation_rules(conn: &Connection, item_id: i64) -> RssResult<Vec<RuleRecord>> {
    let mut stmt=conn.prepare(&format!("{RULE_SELECT} WHERE r.enabled=1 AND r.archived_at IS NULL AND EXISTS(
        SELECT 1 FROM rss_decisions d WHERE d.rule_id=r.id AND d.item_id=? AND d.match_revision=r.match_revision
        AND d.status IN ('pending','ready','attribute_unknown','priority_wait')) ORDER BY r.priority,r.id")).map_err(sql_error)?;
    let mut rules = stmt
        .query_map([item_id], map_rule)
        .map_err(sql_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(sql_error)?;
    for rule in &mut rules {
        rule.feed_ids = rule_feed_ids(conn, rule.id)?;
    }
    Ok(rules)
}
fn create_item_decisions(conn: &Connection, item_id: i64, time: &str) -> RssResult<()> {
    conn.execute("INSERT INTO rss_decisions(item_id,rule_id,match_revision,item_revision,status,next_evaluate_at,evaluation_json)
        SELECT i.id,r.id,r.match_revision,i.content_revision,'pending',?,? FROM rss_items i
        JOIN rss_rule_feeds rf ON rf.feed_id=i.feed_id JOIN rss_rules r ON r.id=rf.rule_id
        WHERE i.id=? AND r.enabled=1 AND r.archived_at IS NULL AND rf.generation=i.generation
        AND rf.baseline_pending=0 AND i.sequence>rf.activation_sequence
        ON CONFLICT(item_id,rule_id,match_revision) DO UPDATE SET
        status=CASE WHEN rss_decisions.job_id IS NULL AND rss_decisions.status NOT IN ('lower_priority','already_present','cancelled')
                    AND rss_decisions.item_revision!=excluded.item_revision THEN 'pending' ELSE rss_decisions.status END,
        next_evaluate_at=CASE WHEN rss_decisions.job_id IS NULL AND rss_decisions.status NOT IN ('lower_priority','already_present','cancelled')
                    AND rss_decisions.item_revision!=excluded.item_revision THEN excluded.next_evaluate_at ELSE rss_decisions.next_evaluate_at END",
        params![time,json(&MatchEvaluation::default())?,item_id]).map_err(sql_error)?;
    conn.execute("UPDATE rss_decisions SET status='pending',next_evaluate_at=?,version=version+1 WHERE item_id=? AND manual=1
        AND job_id IS NULL AND status NOT IN ('lower_priority','already_present','cancelled')
        AND item_revision!=(SELECT content_revision FROM rss_items WHERE id=rss_decisions.item_id)
        AND EXISTS(SELECT 1 FROM rss_rules r WHERE r.id=rule_id AND r.match_revision=rss_decisions.match_revision AND r.enabled=1 AND r.archived_at IS NULL)",
        params![time,item_id]).map_err(sql_error)?;
    refresh_item_status(conn, item_id)
}
fn reason(code: &str, message: &str) -> MatchEvaluation {
    MatchEvaluation {
        matched: false,
        needs_attributes: false,
        reasons: vec![MatchReason {
            code: code.into(),
            message: message.into(),
            field: None,
            actual: None,
            expected: None,
        }],
    }
}
fn enqueue_job(
    conn: &Connection,
    feed: &StoredFeed,
    item: &ItemRecord,
    rule: &RuleRecord,
    evaluation: &MatchEvaluation,
    time: &str,
) -> RssResult<(Option<i64>, bool, bool)> {
    let downloader = rule
        .downloader_id
        .ok_or_else(|| RssError::Invalid("未配置目标下载器".into()))?;
    validate_rule_target(conn, true, Some(downloader))?;
    let (kind, key) = if let (Some(site), Some(torrent)) =
        (feed.record.site_id, item.site_torrent_id.as_deref())
    {
        ("site", digest(&format!("{site}:{torrent}")))
    } else {
        (
            "item",
            digest(&format!(
                "{}:{}:{}",
                item.feed_id, item.generation, item.item_key
            )),
        )
    };
    if let Some((job_id,outcome))=conn.query_row("SELECT job_id,outcome FROM rss_delivery_keys WHERE downloader_id=? AND key_kind=? AND key_digest=?",params![downloader,kind,key],|r|Ok((r.get::<_,Option<i64>>(0)?,r.get::<_,String>(1)?))).optional().map_err(sql_error)? {
        return Ok((job_id,false,matches!(outcome.as_str(),"submitted"|"already_present")));
    }
    let name: String = conn
        .query_row(
            "SELECT name FROM downloaders WHERE id=?",
            [downloader],
            |r| r.get(0),
        )
        .map_err(sql_error)?;
    conn.execute("INSERT INTO rss_download_jobs(item_id,item_identity,feed_id,feed_generation,feed_name,rule_id,rule_name,downloader_id,
        downloader_identity,downloader_name,title,size_bytes,filters_json,options_json,decision_json,next_attempt_at,created_at,updated_at)
        VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",params![item.id,item.id,item.feed_id,item.generation,feed.record.name,rule.id,rule.name,downloader,downloader,name,item.title,item.attributes.size_bytes.map(|v|v.min(i64::MAX as u64) as i64),json(&rule.filters)?,json(&rule.options)?,json(evaluation)?,time,time,time]).map_err(sql_error)?;
    let job = conn.last_insert_rowid();
    conn.execute("INSERT INTO rss_delivery_keys(downloader_id,key_kind,key_digest,job_id,outcome,created_at,updated_at) VALUES(?,?,?,?,'queued',?,?)",params![downloader,kind,key,job,time,time]).map_err(sql_error)?;
    conn.execute(
        "UPDATE rss_rules SET matched_count=matched_count+1 WHERE id=?",
        [rule.id],
    )
    .map_err(sql_error)?;
    Ok((Some(job), true, false))
}
fn refresh_item_status(conn: &Connection, item_id: i64) -> RssResult<()> {
    conn.execute("UPDATE rss_items SET status=CASE
        WHEN EXISTS(SELECT 1 FROM rss_decisions WHERE item_id=?1 AND status IN ('queued','already_present')) THEN 'queued'
        WHEN EXISTS(SELECT 1 FROM rss_decisions WHERE item_id=?1 AND status='attribute_unknown') THEN 'attribute_unknown'
        WHEN EXISTS(SELECT 1 FROM rss_decisions WHERE item_id=?1 AND status='ready') THEN 'ready'
        WHEN EXISTS(SELECT 1 FROM rss_decisions WHERE item_id=?1 AND status IN ('pending','priority_wait')) THEN 'pending'
        WHEN EXISTS(SELECT 1 FROM rss_decisions WHERE item_id=?1) THEN 'skipped'
        ELSE status END WHERE id=?1",[item_id]).map_err(sql_error)?;
    Ok(())
}
fn refresh_run_counts(conn: &Connection, feed_id: i64, queued: u64, time: &str) -> RssResult<()> {
    let pending:i64=conn.query_row("SELECT count(DISTINCT d.item_id) FROM rss_decisions d JOIN rss_items i ON i.id=d.item_id
        JOIN rss_feeds f ON f.id=i.feed_id JOIN rss_rules r ON r.id=d.rule_id
        WHERE i.feed_id=? AND i.generation=f.generation AND r.match_revision=d.match_revision
        AND d.status IN ('pending','ready','attribute_unknown','priority_wait')",[feed_id],|r|r.get(0)).map_err(sql_error)?;
    conn.execute("UPDATE rss_runs SET queued_count=queued_count+?,pending_count=?,status=CASE WHEN ?=0 THEN 'completed' ELSE 'processing' END,
        finished_at=CASE WHEN ?=0 THEN ? ELSE finished_at END WHERE id=(SELECT cycle_id FROM rss_feeds WHERE id=?)",
        params![queued as i64,pending,pending,pending,time,feed_id]).map_err(sql_error)?;
    refresh_backfill_runs(conn, time)
}
fn refresh_backfill_runs(conn: &Connection, time: &str) -> RssResult<()> {
    conn.execute("UPDATE rss_runs SET pending_count=(SELECT count(*) FROM rss_run_items ri
        JOIN rss_decisions d ON d.item_id=ri.item_id AND d.rule_id=ri.rule_id AND d.match_revision=ri.match_revision
        WHERE ri.run_id=rss_runs.id AND d.status IN ('pending','ready','attribute_unknown','priority_wait')),
        queued_count=(SELECT count(DISTINCT j.id) FROM rss_run_items ri
        JOIN rss_decisions d ON d.item_id=ri.item_id AND d.rule_id=ri.rule_id AND d.match_revision=ri.match_revision
        JOIN rss_download_jobs j ON j.id=d.job_id WHERE ri.run_id=rss_runs.id AND j.created_at>=rss_runs.started_at)
        WHERE kind='backfill' AND status='processing'",[]).map_err(sql_error)?;
    conn.execute("UPDATE rss_runs SET status='completed',finished_at=? WHERE kind='backfill' AND status='processing' AND pending_count=0",[time]).map_err(sql_error)?;
    Ok(())
}

fn require_reference(conn: &Connection, table: &str, id: i64, name: &str) -> RssResult<()> {
    let exists: bool = conn
        .query_row(
            &format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE id=?)"),
            [id],
            |r| r.get(0),
        )
        .map_err(sql_error)?;
    if !exists {
        return Err(not_found(name));
    }
    Ok(())
}
fn validate_rule_target(
    conn: &Connection,
    enabled: bool,
    downloader_id: Option<i64>,
) -> RssResult<()> {
    if enabled && downloader_id.is_none() {
        return Err(RssError::Invalid("启用下载规则前请选择下载器".into()));
    }
    if let Some(id) = downloader_id {
        let kind: Option<String> = conn
            .query_row(
                "SELECT downloader_type FROM downloaders WHERE id=?",
                [id],
                |r| r.get(0),
            )
            .optional()
            .map_err(sql_error)?;
        let kind = kind.ok_or_else(|| not_found("下载器"))?;
        if !matches!(kind.to_ascii_lowercase().as_str(), "qb" | "qbittorrent") {
            return Err(RssError::Invalid("RSS 下载目前仅支持 qBittorrent".into()));
        }
    }
    Ok(())
}
fn reset_rule_boundaries(conn: &Connection, rule_id: i64, feed_ids: &[i64]) -> RssResult<()> {
    conn.execute("DELETE FROM rss_rule_feeds WHERE rule_id=?", [rule_id])
        .map_err(sql_error)?;
    for feed_id in feed_ids {
        conn.execute("INSERT INTO rss_rule_feeds(rule_id,feed_id,generation,activation_sequence,baseline_pending)
            SELECT ?,id,generation,last_sequence,initialized_at IS NULL OR resume_baseline=1 FROM rss_feeds WHERE id=? AND archived_at IS NULL",params![rule_id,feed_id]).map_err(sql_error)?;
    }
    Ok(())
}
fn request_lookup(
    conn: &Connection,
    operation: &str,
    request_id: Option<&str>,
    request_digest: &str,
) -> RssResult<Option<i64>> {
    let Some(request_id) = request_id else {
        return Ok(None);
    };
    if request_id.is_empty() || request_id.len() > 128 {
        return Err(RssError::Invalid("request_id 长度必须为 1–128 字符".into()));
    }
    let prior: Option<(String, i64)> = conn
        .query_row(
            "SELECT request_digest,result_ref FROM rss_runs WHERE operation=? AND request_id=?",
            params![operation, request_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(sql_error)?;
    match prior {
        Some((value, _)) if value != request_digest => {
            Err(RssError::Conflict("同一 request_id 已用于不同请求".into()))
        }
        Some((_, id)) => Ok(Some(id)),
        None => Ok(None),
    }
}
fn record_request(
    conn: &Connection,
    operation: &str,
    request_id: Option<&str>,
    request_digest: &str,
    result: i64,
    feed_id: Option<i64>,
    kind: &str,
    time: &str,
) -> RssResult<()> {
    if let Some(request_id) = request_id {
        conn.execute("INSERT INTO rss_runs(feed_id,kind,status,started_at,finished_at,operation,request_id,request_digest,result_ref)
            VALUES(?,?,'completed',?,?,?,?,?,?)",params![feed_id,kind,time,time,operation,request_id,request_digest,result]).map_err(sql_error)?;
    }
    Ok(())
}

/// Fence workers before changing held/cancelled work; uncertain submissions are
/// deliberately excluded, including when a source or rule has been archived.
fn synchronize_jobs(conn: &Connection, time: &str) -> RssResult<()> {
    conn.execute("UPDATE rss_download_jobs SET status='cancelled',last_error='来源或规则已归档',reserved_bytes=0,
        lease_owner=NULL,lease_until=NULL,version=version+1,updated_at=?
        WHERE status IN ('queued','fetching','waiting','retry_wait','held','failed')
        AND (EXISTS(SELECT 1 FROM rss_feeds f WHERE f.id=feed_id AND f.archived_at IS NOT NULL)
            OR EXISTS(SELECT 1 FROM rss_rules r WHERE r.id=rule_id AND r.archived_at IS NOT NULL))",[time]).map_err(sql_error)?;
    conn.execute("UPDATE rss_download_jobs SET status='held',last_error=CASE
        WHEN EXISTS(SELECT 1 FROM rss_feeds f WHERE f.id=feed_id AND f.generation!=feed_generation) THEN '来源已更换；只能查看或取消旧代次任务'
        ELSE '来源或规则已暂停' END,reserved_bytes=0,lease_owner=NULL,lease_until=NULL,version=version+1,updated_at=?
        WHERE status IN ('queued','fetching','waiting','retry_wait')
        AND (EXISTS(SELECT 1 FROM rss_feeds f WHERE f.id=feed_id AND (f.enabled=0 OR f.generation!=feed_generation))
            OR EXISTS(SELECT 1 FROM rss_rules r WHERE r.id=rule_id AND r.enabled=0))",[time]).map_err(sql_error)?;
    conn.execute("UPDATE rss_download_jobs SET status='queued',last_error=NULL,next_attempt_at=?,version=version+1,updated_at=?
        WHERE status='held' AND downloader_id IS NOT NULL
        AND EXISTS(SELECT 1 FROM rss_feeds f WHERE f.id=feed_id AND f.generation=feed_generation AND f.enabled=1 AND f.archived_at IS NULL)
        AND EXISTS(SELECT 1 FROM rss_rules r WHERE r.id=rule_id AND r.enabled=1 AND r.archived_at IS NULL)",params![time,time]).map_err(sql_error)?;
    conn.execute("UPDATE rss_delivery_keys SET outcome='released',updated_at=? WHERE key_kind='infohash' AND job_id IN
        (SELECT id FROM rss_download_jobs WHERE status IN ('held','cancelled')) AND outcome NOT IN ('submitted','already_present')",[time]).map_err(sql_error)?;
    conn.execute("UPDATE rss_delivery_keys SET outcome='cancelled',updated_at=? WHERE key_kind!='infohash' AND job_id IN (SELECT id FROM rss_download_jobs WHERE status='cancelled')",[time]).map_err(sql_error)?;
    conn.execute("DELETE FROM rss_downloader_slots WHERE job_id IN(SELECT id FROM rss_download_jobs WHERE status NOT IN ('fetching','submitting','reconciling'))",[]).map_err(sql_error)?;
    Ok(())
}

fn now() -> String {
    Utc::now().to_rfc3339()
}
fn after(seconds: u64) -> String {
    (Utc::now() + Duration::seconds(seconds.min(i64::MAX as u64) as i64)).to_rfc3339()
}
fn digest(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}
fn json<T: Serialize>(value: &T) -> RssResult<String> {
    serde_json::to_string(value).map_err(|_| RssError::Invalid("无法保存 RSS 配置".into()))
}
fn parse_json<T: DeserializeOwned>(row: &Row<'_>, col: usize) -> rusqlite::Result<T> {
    serde_json::from_str(&row.get::<_, String>(col)?).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(col, rusqlite::types::Type::Text, Box::new(error))
    })
}
fn not_found(what: &str) -> RssError {
    RssError::NotFound(format!("{what}不存在或已归档"))
}
fn conflict() -> RssError {
    RssError::Conflict("配置或任务状态已变化，请刷新后重试".into())
}
fn check_version(actual: i64, expected: Option<i64>) -> RssResult<()> {
    if expected != Some(actual) {
        return Err(conflict());
    }
    Ok(())
}

/// Paths as well as query strings may contain credentials.
fn masked_url(value: &str) -> String {
    reqwest::Url::parse(value)
        .map(|url| format!("{}/••••", url.origin().ascii_serialization()))
        .unwrap_or_else(|_| "••••".into())
}

fn map_feed(r: &Row<'_>) -> rusqlite::Result<FeedRecord> {
    Ok(FeedRecord {
        id: r.get(0)?,
        name: r.get(1)?,
        url_display: r.get(2)?,
        site_id: r.get(3)?,
        site_name: r.get(4)?,
        use_proxy: r.get::<_, Option<bool>>(5)?,
        enabled: r.get(6)?,
        interval_minutes: r.get(7)?,
        generation: r.get(8)?,
        version: r.get(9)?,
        initialized_at: r.get(10)?,
        last_sequence: r.get(11)?,
        last_checked_at: r.get(12)?,
        next_run_at: r.get(13)?,
        last_status: r.get(14)?,
        last_error: r.get(15)?,
        item_count: r.get(16)?,
        pending_count: r.get(17)?,
        created_at: r.get(18)?,
        updated_at: r.get(19)?,
    })
}
fn map_rule(r: &Row<'_>) -> rusqlite::Result<RuleRecord> {
    Ok(RuleRecord {
        id: r.get(0)?,
        name: r.get(1)?,
        enabled: r.get(2)?,
        priority: r.get(3)?,
        feed_ids: Vec::new(),
        filters: parse_json(r, 4)?,
        downloader_id: r.get(5)?,
        downloader_name: r.get(6)?,
        options: parse_json(r, 7)?,
        match_revision: r.get(8)?,
        version: r.get(9)?,
        last_error: r.get(10)?,
        matched_count: r.get(11)?,
        created_at: r.get(12)?,
        updated_at: r.get(13)?,
    })
}
fn map_item(r: &Row<'_>) -> rusqlite::Result<ItemRecord> {
    Ok(ItemRecord {
        id: r.get(0)?,
        feed_id: r.get(1)?,
        feed_name: r.get(2)?,
        generation: r.get(3)?,
        item_key: r.get(4)?,
        sequence: r.get(5)?,
        title: r.get(6)?,
        detail_url: r.get(7)?,
        site_torrent_id: r.get(8)?,
        published_at: r.get(9)?,
        categories: parse_json(r, 10)?,
        attributes: parse_json(r, 11)?,
        downloadable: r.get(12)?,
        content_revision: r.get(13)?,
        first_seen_at: r.get(14)?,
        last_seen_at: r.get(15)?,
        status: r.get(16)?,
        decisions: Vec::new(),
    })
}
fn map_job(r: &Row<'_>) -> rusqlite::Result<JobRecord> {
    Ok(JobRecord {
        id: r.get(0)?,
        item_id: r.get(1)?,
        feed_id: r.get(2)?,
        feed_generation: r.get(3)?,
        feed_name: r.get(4)?,
        rule_id: r.get(5)?,
        rule_name: r.get(6)?,
        downloader_id: r.get(7)?,
        downloader_name: r.get(8)?,
        title: r.get(9)?,
        size_bytes: r.get(10)?,
        filters_snapshot: parse_json(r, 11)?,
        options_snapshot: parse_json(r, 12)?,
        decision_snapshot: parse_json(r, 13)?,
        status: r.get(14)?,
        infohash: r.get(15)?,
        reserved_bytes: r.get(16)?,
        attempts: r.get(17)?,
        next_attempt_at: r.get(18)?,
        version: r.get(19)?,
        last_error: r.get(20)?,
        created_at: r.get(21)?,
        updated_at: r.get(22)?,
        submitted_at: r.get(23)?,
        download_state: r.get(24)?,
        progress: r.get(25)?,
        sampled_at: r.get(26)?,
    })
}
fn map_run(r: &Row<'_>) -> rusqlite::Result<RunRecord> {
    Ok(RunRecord {
        id: r.get(0)?,
        feed_id: r.get(1)?,
        kind: r.get(2)?,
        status: r.get(3)?,
        item_count: r.get(4)?,
        new_count: r.get(5)?,
        queued_count: r.get(6)?,
        pending_count: r.get(7)?,
        message: r.get(8)?,
        started_at: r.get(9)?,
        finished_at: r.get(10)?,
    })
}
fn get_feed(conn: &Connection, id: i64) -> RssResult<StoredFeed> {
    let record = conn
        .query_row(
            &format!("{FEED_SELECT} WHERE f.id=? AND f.archived_at IS NULL"),
            [id],
            map_feed,
        )
        .optional()
        .map_err(sql_error)?
        .ok_or_else(|| not_found("订阅源"))?;
    let (url,etag,last_modified) = conn.query_row("SELECT s.url,f.etag,f.last_modified FROM rss_feeds f JOIN rss_feed_secrets s ON s.feed_id=f.id AND s.generation=f.generation WHERE f.id=?",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(sql_error)?;
    Ok(StoredFeed {
        record,
        url,
        etag,
        last_modified,
    })
}
fn get_rule(conn: &Connection, id: i64) -> RssResult<RuleRecord> {
    let mut rule = conn
        .query_row(
            &format!("{RULE_SELECT} WHERE r.id=? AND r.archived_at IS NULL"),
            [id],
            map_rule,
        )
        .optional()
        .map_err(sql_error)?
        .ok_or_else(|| not_found("下载规则"))?;
    rule.feed_ids = rule_feed_ids(conn, id)?;
    Ok(rule)
}
fn rule_feed_ids(conn: &Connection, id: i64) -> RssResult<Vec<i64>> {
    let mut stmt = conn
        .prepare("SELECT feed_id FROM rss_rule_feeds WHERE rule_id=? ORDER BY feed_id")
        .map_err(sql_error)?;
    Ok(stmt
        .query_map([id], |r| r.get(0))
        .map_err(sql_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(sql_error)?)
}
fn get_item(conn: &Connection, id: i64) -> RssResult<ItemRecord> {
    let mut item = conn
        .query_row(&format!("{ITEM_SELECT} WHERE i.id=?"), [id], map_item)
        .optional()
        .map_err(sql_error)?
        .ok_or_else(|| not_found("RSS 条目"))?;
    item.decisions = get_decisions(conn, id)?;
    Ok(item)
}
fn get_locator(conn: &Connection, id: i64) -> RssResult<ItemLocator> {
    conn.query_row("SELECT l.download_url,l.detail_url,i.site_torrent_id,f.site_id FROM rss_items i JOIN rss_feeds f ON f.id=i.feed_id JOIN rss_item_locators l ON l.item_id=i.id WHERE i.id=?",[id],|r|Ok(ItemLocator {download_url:r.get(0)?,detail_url:r.get(1)?,site_torrent_id:r.get(2)?,site_id:r.get(3)?})).optional().map_err(sql_error)?.ok_or_else(||not_found("下载定位"))
}
fn get_job(conn: &Connection, id: i64) -> RssResult<JobRecord> {
    conn.query_row(&format!("{JOB_SELECT} WHERE j.id=?"), [id], map_job)
        .optional()
        .map_err(sql_error)?
        .ok_or_else(|| not_found("下载任务"))
}
fn get_run(conn: &Connection, id: i64) -> RssResult<RunRecord> {
    conn.query_row(&format!("{RUN_SELECT} WHERE id=?"), [id], map_run)
        .optional()
        .map_err(sql_error)?
        .ok_or_else(|| not_found("运行记录"))
}
fn get_decisions(conn: &Connection, id: i64) -> RssResult<Vec<DecisionRecord>> {
    let mut stmt=conn.prepare("SELECT d.id,d.item_id,d.rule_id,r.name,d.match_revision,d.status,d.evaluation_json,d.checked_at,d.next_evaluate_at,d.job_id FROM rss_decisions d JOIN rss_rules r ON r.id=d.rule_id WHERE d.item_id=? ORDER BY r.priority,r.id,d.match_revision DESC").map_err(sql_error)?;
    Ok(stmt
        .query_map([id], |r| {
            Ok(DecisionRecord {
                id: r.get(0)?,
                item_id: r.get(1)?,
                rule_id: r.get(2)?,
                rule_name: r.get(3)?,
                match_revision: r.get(4)?,
                status: r.get(5)?,
                evaluation: parse_json(r, 6)?,
                checked_at: r.get(7)?,
                next_evaluate_at: r.get(8)?,
                job_id: r.get(9)?,
            })
        })
        .map_err(sql_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(sql_error)?)
}
fn keyword_filter(
    filter: &mut String,
    values: &mut Vec<rusqlite::types::Value>,
    column: &str,
    keyword: &Option<String>,
) {
    if let Some(keyword) = keyword {
        filter.push_str(&format!(" AND instr(lower({column}),lower(?))>0"));
        values.push(keyword.clone().into());
    }
}
fn id_filter(
    filter: &mut String,
    values: &mut Vec<rusqlite::types::Value>,
    column: &str,
    id: Option<i64>,
) {
    if let Some(id) = id {
        filter.push_str(&format!(" AND {column}=?"));
        values.push(id.into());
    }
}
fn query_page<T>(
    conn: &Connection,
    select: &str,
    order: &str,
    mut values: Vec<rusqlite::types::Value>,
    q: &ListQuery,
    map: fn(&Row<'_>) -> rusqlite::Result<T>,
) -> RssResult<Page<T>> {
    let total = conn
        .query_row(
            &format!("SELECT count(*) FROM ({select})"),
            rusqlite::params_from_iter(&values),
            |r| r.get(0),
        )
        .map_err(sql_error)?;
    values.push((q.page_size as i64).into());
    values.push(
        ((q.page - 1) * q.page_size)
            .try_into()
            .map(rusqlite::types::Value::Integer)
            .unwrap_or(rusqlite::types::Value::Integer(i64::MAX)),
    );
    let mut stmt = conn
        .prepare(&format!("{select} ORDER BY {order} LIMIT ? OFFSET ?"))
        .map_err(sql_error)?;
    let items = stmt
        .query_map(rusqlite::params_from_iter(&values), map)
        .map_err(sql_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(sql_error)?;
    Ok(Page {
        items,
        total,
        page: q.page,
        page_size: q.page_size,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        _dir: tempfile::TempDir,
        db: Database,
        feed: FeedRecord,
        downloader: i64,
        site: i64,
    }

    async fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(dir.path()).await.unwrap();
        let site = db
            .create_site(
                "Tracker",
                "nexusphp",
                "https://tracker.test",
                "{\"cookie\":\"secret\"}",
                "{}",
                false,
            )
            .await
            .unwrap();
        let downloader = db
            .create_downloader("qB", "qbittorrent", "http://qb.test", "admin", "password")
            .await
            .unwrap();
        let feed = db
            .rss_save_feed(None, feed_input(Some(site)))
            .await
            .unwrap();
        Fixture {
            _dir: dir,
            db,
            feed,
            downloader,
            site,
        }
    }
    fn feed_input(site_id: Option<i64>) -> FeedInput {
        FeedInput {
            name: "RSS 来源".into(),
            url: Some("https://tracker.test/private-token/rss?passkey=secret".into()),
            site_id,
            use_proxy: Some(false),
            enabled: true,
            interval_minutes: 15,
            expected_version: None,
            request_id: None,
        }
    }
    fn rule_input(feed_id: i64, downloader: i64) -> RuleInput {
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
            downloader_id: Some(downloader),
            options: DownloadOptions {
                category: Some("documentary".into()),
                tags: vec!["rss".into()],
                ..Default::default()
            },
            expected_version: None,
            request_id: None,
        }
    }
    fn normalized(index: usize) -> NormalizedItem {
        NormalizedItem {
            item_key: digest(&format!("item:{index}")),
            guid: Some(index.to_string()),
            title: format!("Documentary {index} 1080p"),
            detail_url: Some(format!(
                "https://tracker.test/details.php?id={index}&passkey=secret"
            )),
            download_url: Some(format!(
                "https://tracker.test/download.php?id={index}&passkey=secret"
            )),
            site_torrent_id: Some(index.to_string()),
            published_at: Some("2026-01-01T00:00:00+00:00".into()),
            categories: vec!["纪录片".into()],
            description: None,
            attributes: ItemAttributes {
                size_bytes: Some(1024),
                hr: Some(false),
                observed_at: now(),
                source: "torznab".into(),
                ..Default::default()
            },
        }
    }
    async fn scan(db: &Database, feed_id: i64, items: Vec<NormalizedItem>) -> RunRecord {
        db.rss_request_check(feed_id, ActionRequest::default())
            .await
            .unwrap();
        let claim = db.rss_claim_feed("scanner", 120).await.unwrap().unwrap();
        assert_eq!(claim.feed.record.id, feed_id);
        db.rss_commit_scan(
            &claim,
            FetchedFeed {
                items,
                etag: Some("etag".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap()
    }
    async fn evaluate_all(db: &Database) -> u64 {
        let mut count = 0;
        for _ in 0..300 {
            let Some(claim) = db.rss_claim_evaluation("matcher", 120).await.unwrap() else {
                return count;
            };
            count += db.rss_commit_evaluation(&claim, None).await.unwrap();
        }
        panic!("evaluation queue did not become idle")
    }
    fn action(version: i64) -> ActionRequest {
        ActionRequest {
            expected_version: Some(version),
            request_id: None,
        }
    }
    fn run_sql(db: &Database, sql: &str) {
        open_connection(&db.path)
            .unwrap()
            .execute_batch(sql)
            .unwrap();
    }
    async fn queue_one(f: &Fixture) -> (RuleRecord, JobRecord) {
        let rule =
            f.db.rss_save_rule(None, rule_input(f.feed.id, f.downloader))
                .await
                .unwrap();
        scan(&f.db, f.feed.id, vec![]).await;
        scan(&f.db, f.feed.id, vec![normalized(1)]).await;
        assert_eq!(evaluate_all(&f.db).await, 1);
        (
            rule,
            f.db.rss_list_jobs(ListQuery::default())
                .await
                .unwrap()
                .items
                .remove(0),
        )
    }

    #[tokio::test]
    async fn first_snapshot_is_baseline_and_reordering_never_replays_history() {
        let f = fixture().await;
        let rule =
            f.db.rss_save_rule(None, rule_input(f.feed.id, f.downloader))
                .await
                .unwrap();
        let run = scan(&f.db, f.feed.id, (0..100).map(normalized).collect()).await;
        assert_eq!(run.new_count, 100);
        assert_eq!(evaluate_all(&f.db).await, 0);
        assert_eq!(
            f.db.rss_list_jobs(ListQuery::default())
                .await
                .unwrap()
                .total,
            0
        );
        let mut reordered = (0..101).rev().map(normalized).collect::<Vec<_>>();
        for item in &mut reordered {
            item.published_at = Some("2000-01-01T00:00:00Z".into());
        }
        scan(&f.db, f.feed.id, reordered).await;
        assert_eq!(evaluate_all(&f.db).await, 1);
        scan(&f.db, f.feed.id, (0..101).map(normalized).collect()).await;
        assert_eq!(evaluate_all(&f.db).await, 0);
        let jobs = f.db.rss_list_jobs(ListQuery::default()).await.unwrap();
        assert_eq!(jobs.total, 1);
        assert_eq!(jobs.items[0].rule_id, rule.id);
        assert_eq!(jobs.items[0].title, "Documentary 100 1080p");
        let feed = f.db.rss_get_feed(f.feed.id).await.unwrap();
        assert_eq!(feed.record.last_sequence, 101);
        let public = serde_json::to_string(&feed.record).unwrap();
        assert!(!public.contains("private-token"));
        assert!(!public.contains("secret"));
        let items = f.db.rss_list_items(ListQuery::default()).await.unwrap();
        assert!(!serde_json::to_string(&items).unwrap().contains("passkey"));
    }

    #[tokio::test]
    async fn snapshots_are_fenced_and_manual_checks_are_idempotent() {
        let f = fixture().await;
        let req = ActionRequest {
            request_id: Some("check-1".into()),
            ..Default::default()
        };
        let run =
            f.db.rss_request_check(f.feed.id, req.clone())
                .await
                .unwrap();
        let duplicate = f.db.rss_request_check(f.feed.id, req).await.unwrap();
        assert_eq!(run.id, duplicate.id);
        let other =
            f.db.rss_request_check(f.feed.id, ActionRequest::default())
                .await
                .unwrap();
        assert_eq!(run.id, other.id);
        let claim = f.db.rss_claim_feed("one", 120).await.unwrap().unwrap();
        assert!(f.db.rss_claim_feed("two", 120).await.unwrap().is_none());
        assert!(f.db.rss_renew_feed(&claim, 120).await.unwrap());
        let mut input = feed_input(Some(f.site));
        input.url = None;
        input.name = "重命名的订阅源".into();
        input.expected_version = Some(f.feed.version);
        let new = f.db.rss_save_feed(Some(f.feed.id), input).await.unwrap();
        assert_eq!(new.generation, f.feed.generation);
        assert!(matches!(
            f.db.rss_commit_scan(
                &claim,
                FetchedFeed {
                    items: vec![normalized(1)],
                    etag: Some("obsolete".into()),
                    ..Default::default()
                }
            )
            .await,
            Err(RssError::Conflict(_))
        ));
        assert_eq!(f.db.rss_get_feed(f.feed.id).await.unwrap().etag, None);
        assert_eq!(
            f.db.rss_list_items(ListQuery::default())
                .await
                .unwrap()
                .total,
            0
        );
        let next = f.db.rss_claim_feed("two", 120).await.unwrap().unwrap();
        f.db.rss_fail_scan(&next, "站点登录失效", None, true)
            .await
            .unwrap();
        assert!(
            f.db.rss_get_feed(f.feed.id)
                .await
                .unwrap()
                .record
                .initialized_at
                .is_none()
        );
        assert!(f.db.rss_claim_feed("two", 120).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn quota_and_pending_work_survive_restart_and_304_opens_next_cycle() {
        let f = fixture().await;
        f.db.rss_save_rule(None, rule_input(f.feed.id, f.downloader))
            .await
            .unwrap();
        scan(&f.db, f.feed.id, vec![]).await;
        let first_cycle = scan(&f.db, f.feed.id, (0..25).map(normalized).collect()).await;
        assert_eq!(evaluate_all(&f.db).await, 20);
        assert_eq!(
            f.db.rss_list_items(ListQuery {
                status: Some("ready".into()),
                ..Default::default()
            })
            .await
            .unwrap()
            .total,
            5
        );
        let reopened = Database::open(f._dir.path()).await.unwrap();
        assert_eq!(
            evaluate_all(&reopened).await,
            0,
            "polling and restart must not reset per-cycle quota"
        );
        reopened
            .rss_request_check(f.feed.id, ActionRequest::default())
            .await
            .unwrap();
        let claim = reopened
            .rss_claim_feed("scanner", 120)
            .await
            .unwrap()
            .unwrap();
        reopened
            .rss_commit_scan(
                &claim,
                FetchedFeed {
                    not_modified: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(evaluate_all(&reopened).await, 5);
        assert_eq!(
            reopened.rss_get_run(first_cycle.id).await.unwrap().status,
            "completed"
        );
        assert_eq!(
            reopened
                .rss_list_jobs(ListQuery::default())
                .await
                .unwrap()
                .total,
            25
        );
    }

    #[tokio::test]
    async fn high_priority_unknown_blocks_lower_rule_and_mutations_fence_evaluation() {
        let f = fixture().await;
        let mut high = rule_input(f.feed.id, f.downloader);
        high.priority = 1;
        high.name = "优先规则".into();
        high.filters.hr_policy = "require_clear".into();
        let high = f.db.rss_save_rule(None, high).await.unwrap();
        let low =
            f.db.rss_save_rule(None, rule_input(f.feed.id, f.downloader))
                .await
                .unwrap();
        scan(&f.db, f.feed.id, vec![]).await;
        let mut item = normalized(1);
        item.attributes.hr = None;
        scan(&f.db, f.feed.id, vec![item]).await;
        let claim =
            f.db.rss_claim_evaluation("first", 120)
                .await
                .unwrap()
                .unwrap();
        assert!(
            f.db.rss_claim_evaluation("second", 120)
                .await
                .unwrap()
                .is_none()
        );
        assert!(f.db.rss_renew_evaluation(&claim, 120).await.unwrap());
        assert_eq!(f.db.rss_commit_evaluation(&claim, None).await.unwrap(), 0);
        let item = f.db.rss_get_item(claim.item.id).await.unwrap();
        assert_eq!(
            item.decisions
                .iter()
                .find(|d| d.rule_id == high.id)
                .unwrap()
                .status,
            "attribute_unknown"
        );
        assert_eq!(
            item.decisions
                .iter()
                .find(|d| d.rule_id == low.id)
                .unwrap()
                .status,
            "priority_wait"
        );
        run_sql(&f.db, "UPDATE rss_decisions SET next_evaluate_at=NULL");
        let claim =
            f.db.rss_claim_evaluation("first", 120)
                .await
                .unwrap()
                .unwrap();
        let mut attrs = claim.item.attributes.clone();
        attrs.hr = Some(false);
        attrs.observed_at = now();
        assert_eq!(
            f.db.rss_commit_evaluation(&claim, Some(attrs))
                .await
                .unwrap(),
            1
        );
        let job =
            f.db.rss_list_jobs(ListQuery::default())
                .await
                .unwrap()
                .items
                .remove(0);
        assert_eq!(job.rule_id, high.id);
        assert_eq!(
            job.options_snapshot.category.as_deref(),
            Some("documentary")
        );
        scan(&f.db, f.feed.id, vec![normalized(2)]).await;
        let stale =
            f.db.rss_claim_evaluation("first", 120)
                .await
                .unwrap()
                .unwrap();
        f.db.rss_set_rule_enabled(high.id, false, action(high.version))
            .await
            .unwrap();
        assert!(matches!(
            f.db.rss_commit_evaluation(&stale, None).await,
            Err(RssError::Conflict(_))
        ));
    }

    #[tokio::test]
    async fn new_rules_resume_and_revisions_only_affect_future_items() {
        let f = fixture().await;
        scan(&f.db, f.feed.id, vec![normalized(0)]).await;
        let rule =
            f.db.rss_save_rule(None, rule_input(f.feed.id, f.downloader))
                .await
                .unwrap();
        scan(&f.db, f.feed.id, vec![normalized(0), normalized(1)]).await;
        assert_eq!(evaluate_all(&f.db).await, 1);
        let original =
            f.db.rss_list_jobs(ListQuery::default())
                .await
                .unwrap()
                .items
                .remove(0);
        let paused =
            f.db.rss_set_rule_enabled(rule.id, false, action(rule.version))
                .await
                .unwrap();
        assert_eq!(f.db.rss_get_job(original.id).await.unwrap().status, "held");
        scan(&f.db, f.feed.id, vec![normalized(2)]).await;
        let resumed =
            f.db.rss_set_rule_enabled(rule.id, true, action(paused.version))
                .await
                .unwrap();
        assert_eq!(
            f.db.rss_get_job(original.id).await.unwrap().status,
            "queued"
        );
        scan(&f.db, f.feed.id, vec![normalized(2)]).await;
        assert_eq!(evaluate_all(&f.db).await, 0);
        let mut rename = rule_input(f.feed.id, f.downloader);
        rename.name = "重命名".into();
        rename.expected_version = Some(resumed.version);
        let rename = f.db.rss_save_rule(Some(rule.id), rename).await.unwrap();
        assert_eq!(rename.match_revision, rule.match_revision);
        scan(&f.db, f.feed.id, vec![normalized(3)]).await;
        let mut edited = rule_input(f.feed.id, f.downloader);
        edited.filters.include.push("2160p".into());
        edited.options.category = Some("new".into());
        edited.expected_version = Some(rename.version);
        let edited = f.db.rss_save_rule(Some(rule.id), edited).await.unwrap();
        assert_eq!(edited.match_revision, rule.match_revision + 1);
        assert_eq!(evaluate_all(&f.db).await, 0);
        assert_eq!(
            f.db.rss_get_job(original.id)
                .await
                .unwrap()
                .options_snapshot
                .category
                .as_deref(),
            Some("documentary")
        );
        let feed = f.db.rss_get_feed(f.feed.id).await.unwrap().record;
        let paused =
            f.db.rss_set_feed_enabled(feed.id, false, action(feed.version))
                .await
                .unwrap();
        assert!(matches!(
            f.db.rss_request_check(feed.id, ActionRequest::default())
                .await,
            Err(RssError::Conflict(_))
        ));
        f.db.rss_set_feed_enabled(feed.id, true, action(paused.version))
            .await
            .unwrap();
        let run = scan(&f.db, feed.id, vec![normalized(4)]).await;
        assert!(run.message.unwrap().contains("基线"));
        assert_eq!(evaluate_all(&f.db).await, 0);
    }

    #[tokio::test]
    async fn creation_backfill_and_actions_are_atomic_idempotent_and_versioned() {
        let f = fixture().await;
        scan(&f.db, f.feed.id, vec![normalized(1), normalized(2)]).await;
        let mut input = rule_input(f.feed.id, f.downloader);
        input.request_id = Some("create-rule".into());
        let rule = f.db.rss_save_rule(None, input.clone()).await.unwrap();
        assert_eq!(
            f.db.rss_save_rule(None, input.clone()).await.unwrap().id,
            rule.id
        );
        input.name = "changed".into();
        assert!(matches!(
            f.db.rss_save_rule(None, input).await,
            Err(RssError::Conflict(_))
        ));
        let ids =
            f.db.rss_list_items(ListQuery::default())
                .await
                .unwrap()
                .items
                .iter()
                .map(|i| i.id)
                .collect::<Vec<_>>();
        let request = BackfillRequest {
            rule_id: rule.id,
            expected_version: rule.version,
            item_ids: ids.clone(),
            request_id: "backfill-1".into(),
        };
        let run = f.db.rss_backfill(request).await.unwrap();
        let again =
            f.db.rss_backfill(BackfillRequest {
                rule_id: rule.id,
                expected_version: rule.version,
                item_ids: ids,
                request_id: "backfill-1".into(),
            })
            .await
            .unwrap();
        assert_eq!(run.id, again.id);
        assert_eq!(evaluate_all(&f.db).await, 2);
        let run = f.db.rss_get_run(run.id).await.unwrap();
        assert_eq!(run.status, "completed");
        assert_eq!(run.queued_count, 2);
        let job =
            f.db.rss_list_jobs(ListQuery::default())
                .await
                .unwrap()
                .items
                .remove(0);
        assert!(matches!(
            f.db.rss_job_action(job.id, "cancel", action(job.version + 1))
                .await,
            Err(RssError::Conflict(_))
        ));
        let req = ActionRequest {
            expected_version: Some(job.version),
            request_id: Some("cancel-1".into()),
        };
        let cancelled =
            f.db.rss_job_action(job.id, "cancel", req.clone())
                .await
                .unwrap();
        assert_eq!(cancelled.status, "cancelled");
        assert_eq!(
            f.db.rss_job_action(job.id, "cancel", req)
                .await
                .unwrap()
                .version,
            cancelled.version
        );
    }

    #[tokio::test]
    async fn uncertain_submission_survives_restart_holds_slot_and_cannot_be_cancelled() {
        let f = fixture().await;
        let (_, job) = queue_one(&f).await;
        let first =
            f.db.rss_claim_job("worker-one", 300)
                .await
                .unwrap()
                .unwrap();
        assert_eq!(first.job.id, job.id);
        assert!(
            f.db.rss_claim_job("worker-two", 300)
                .await
                .unwrap()
                .is_none()
        );
        assert!(f.db.rss_renew_job(&first, 300).await.unwrap());
        let submitting =
            f.db.rss_prepare_submission(&first, &"a".repeat(40), 1024, None)
                .await
                .unwrap()
                .unwrap();
        assert_eq!(submitting.job.status, "submitting");
        assert!(submitting.job.submitted_at.is_none());
        assert!(submitting.submission_started_at.is_some());
        assert!(matches!(
            f.db.rss_job_action(job.id, "cancel", action(submitting.job.version))
                .await,
            Err(RssError::Conflict(_))
        ));
        run_sql(
            &f.db,
            "UPDATE rss_download_jobs SET lease_until='2000-01-01T00:00:00Z'",
        );
        let reopened = Database::open(f._dir.path()).await.unwrap();
        assert_eq!(reopened.rss_recover().await.unwrap(), 1);
        let recovered = reopened.rss_get_job(job.id).await.unwrap();
        assert_eq!(recovered.status, "reconciling");
        assert_eq!(recovered.reserved_bytes, 1024);
        assert!(matches!(
            reopened
                .rss_transition_job(
                    &submitting,
                    JobTransition {
                        status: "submitted".into(),
                        ..Default::default()
                    }
                )
                .await,
            Err(RssError::Conflict(_))
        ));
        let claim = reopened
            .rss_claim_job("worker-two", 300)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(claim.job.status, "reconciling");
        assert!(matches!(
            reopened
                .rss_transition_job(
                    &claim,
                    JobTransition {
                        status: "retry_wait".into(),
                        ..Default::default()
                    }
                )
                .await,
            Err(RssError::Conflict(_))
        ));
        let retry = reopened
            .rss_transition_job(
                &claim,
                JobTransition {
                    status: "retry_wait".into(),
                    release_reservation: true,
                    next_attempt_at: Some(now()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(retry.reserved_bytes, 0);
        let claim = reopened
            .rss_claim_job("worker-two", 300)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(claim.job.status, "fetching");
        let submitting = reopened
            .rss_prepare_submission(&claim, &"a".repeat(40), 1024, None)
            .await
            .unwrap()
            .unwrap();
        let done = reopened
            .rss_transition_job(
                &submitting,
                JobTransition {
                    status: "submitted".into(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(done.reserved_bytes, 1024);
        assert_eq!(
            reopened
                .rss_release_observed_reservations(
                    f.downloader,
                    vec!["a".repeat(40)],
                    "2000-01-01T00:00:00Z"
                )
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            reopened
                .rss_release_observed_reservations(f.downloader, vec!["b".repeat(40)], &now())
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            reopened
                .rss_release_observed_reservations(f.downloader, vec!["a".repeat(40)], &now())
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            reopened.rss_reserved_bytes(f.downloader, -1).await.unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn site_and_hash_dedupe_respect_downloader_identity_and_failed_ownership() {
        let f = fixture().await;
        let rule =
            f.db.rss_save_rule(None, rule_input(f.feed.id, f.downloader))
                .await
                .unwrap();
        let mut second = feed_input(Some(f.site));
        second.name = "第二来源".into();
        second.url = Some("https://tracker.test/rss2?secret=other".into());
        let second = f.db.rss_save_feed(None, second).await.unwrap();
        f.db.rss_save_rule(None, rule_input(second.id, f.downloader))
            .await
            .unwrap();
        // Source selection is deterministic by due timestamp; establish each
        // baseline via the explicit claim result instead of assuming scheduler order.
        for _ in 0..2 {
            let claim = f.db.rss_claim_feed("scanner", 120).await.unwrap().unwrap();
            f.db.rss_commit_scan(&claim, FetchedFeed::default())
                .await
                .unwrap();
        }
        scan(&f.db, f.feed.id, vec![normalized(1)]).await;
        scan(&f.db, second.id, vec![normalized(1)]).await;
        assert_eq!(
            evaluate_all(&f.db).await,
            1,
            "two feeds must share the site torrent admission key"
        );
        scan(&f.db, f.feed.id, vec![normalized(2)]).await;
        assert_eq!(evaluate_all(&f.db).await, 1);
        let first = f.db.rss_claim_job("first", 300).await.unwrap().unwrap();
        let sending =
            f.db.rss_prepare_submission(&first, &"b".repeat(40), 1024, None)
                .await
                .unwrap()
                .unwrap();
        f.db.rss_transition_job(
            &sending,
            JobTransition {
                status: "submitted".into(),
                release_reservation: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let second = f.db.rss_claim_job("second", 300).await.unwrap().unwrap();
        assert_eq!(second.job.rule_id, rule.id);
        assert!(
            f.db.rss_prepare_submission(&second, &"b".repeat(40), 1024, None)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            f.db.rss_get_job(second.job.id).await.unwrap().status,
            "already_present"
        );
        scan(&f.db, f.feed.id, vec![normalized(3), normalized(4)]).await;
        assert_eq!(evaluate_all(&f.db).await, 2);
        let third = f.db.rss_claim_job("third", 300).await.unwrap().unwrap();
        let sending =
            f.db.rss_prepare_submission(&third, &"c".repeat(40), 1024, None)
                .await
                .unwrap()
                .unwrap();
        f.db.rss_transition_job(
            &sending,
            JobTransition {
                status: "reconciling".into(),
                next_attempt_at: Some(now()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let recon = f.db.rss_claim_job("third", 300).await.unwrap().unwrap();
        f.db.rss_transition_job(
            &recon,
            JobTransition {
                status: "failed".into(),
                release_reservation: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let fourth = f.db.rss_claim_job("fourth", 300).await.unwrap().unwrap();
        assert!(
            f.db.rss_prepare_submission(&fourth, &"c".repeat(40), 1024, None)
                .await
                .unwrap()
                .is_some(),
            "a confirmed failure is not a success tombstone"
        );
    }

    #[tokio::test]
    async fn address_changes_are_rejected_without_disrupting_jobs_and_pauses_fence_submission() {
        let f = fixture().await;
        let (rule, job) = queue_one(&f).await;
        let claim = f.db.rss_claim_job("worker", 300).await.unwrap().unwrap();
        let paused =
            f.db.rss_set_rule_enabled(rule.id, false, action(rule.version))
                .await
                .unwrap();
        assert_eq!(f.db.rss_get_job(job.id).await.unwrap().status, "held");
        assert!(matches!(
            f.db.rss_prepare_submission(&claim, &"a".repeat(40), 1024, None)
                .await,
            Err(RssError::Conflict(_))
        ));
        f.db.rss_set_rule_enabled(rule.id, true, action(paused.version))
            .await
            .unwrap();
        let claim = f.db.rss_claim_job("worker", 300).await.unwrap().unwrap();
        let feed = f.db.rss_get_feed(f.feed.id).await.unwrap().record;
        let mut input = feed_input(Some(f.site));
        input.url = Some("https://tracker.test/replacement".into());
        input.expected_version = Some(feed.version);
        let before = f.db.rss_get_feed(feed.id).await.unwrap();
        let active_job = f.db.rss_get_job(job.id).await.unwrap();
        let error = f.db.rss_save_feed(Some(feed.id), input).await.unwrap_err();
        assert!(matches!(error, RssError::Invalid(_)));
        assert!(error.to_string().contains("RSS 地址保存后不可修改"));
        let after = f.db.rss_get_feed(feed.id).await.unwrap();
        assert_eq!(after.url, before.url);
        assert_eq!(after.record.version, before.record.version);
        assert_eq!(after.record.generation, before.record.generation);
        assert_eq!(after.record.initialized_at, before.record.initialized_at);
        assert_eq!(after.record.last_sequence, before.record.last_sequence);
        let unchanged_job = f.db.rss_get_job(job.id).await.unwrap();
        assert_eq!(unchanged_job.status, active_job.status);
        assert_eq!(unchanged_job.version, active_job.version);
        f.db.rss_set_feed_enabled(feed.id, false, action(feed.version))
            .await
            .unwrap();
        let held = f.db.rss_get_job(job.id).await.unwrap();
        assert_eq!(held.status, "held");
        assert!(f.db.rss_claim_job("other", 300).await.unwrap().is_none());
        assert!(matches!(
            f.db.rss_prepare_submission(&claim, &"a".repeat(40), 1024, None)
                .await,
            Err(RssError::Conflict(_))
        ));
        assert_eq!(
            f.db.rss_job_action(job.id, "cancel", action(held.version))
                .await
                .unwrap()
                .status,
            "cancelled"
        );
    }

    #[tokio::test]
    async fn destructive_references_and_transfers_are_checked_inside_transactions() {
        let f = fixture().await;
        let (rule, _) = queue_one(&f).await;
        assert!(f.db.delete_downloader(f.downloader).await.is_err());
        assert!(f.db.delete_site(f.site).await.is_err());
        assert!(
            f.db.update_downloader(
                f.downloader,
                "qB",
                "qb",
                "http://different.test",
                "admin",
                "password"
            )
            .await
            .is_err()
        );
        assert!(
            f.db.update_site(
                f.site,
                "Tracker",
                "nexusphp",
                "https://different.test",
                "{\"cookie\":\"new\"}",
                "{}",
                false
            )
            .await
            .is_err()
        );
        f.db.update_site(
            f.site,
            "Tracker",
            "nexusphp",
            "https://tracker.test",
            "{\"cookie\":\"new\"}",
            "{}",
            false,
        )
        .await
        .unwrap();
        let target =
            f.db.create_downloader(
                "目标",
                "qbittorrent",
                "http://target.test",
                "admin",
                "password",
            )
            .await
            .unwrap();
        let claim = f.db.rss_claim_job("worker", 300).await.unwrap().unwrap();
        assert_eq!(
            f.db.enqueue_manual_media_relocation_jobs(
                f.downloader,
                target,
                "/archive",
                "/data",
                &[("a".repeat(40), "resource".into())]
            )
            .await
            .unwrap(),
            (0, 1),
            "unknown hash fetching must block transfers on either downloader"
        );
        let cancelled =
            f.db.rss_job_action(claim.job.id, "cancel", action(claim.job.version))
                .await
                .unwrap();
        assert_eq!(cancelled.status, "cancelled");
        assert_eq!(
            f.db.enqueue_manual_media_relocation_jobs(
                target,
                f.downloader,
                "/archive",
                "/data",
                &[("a".repeat(40), "resource".into())]
            )
            .await
            .unwrap(),
            (1, 0)
        );
        scan(&f.db, f.feed.id, vec![normalized(2)]).await;
        assert_eq!(evaluate_all(&f.db).await, 1);
        assert!(
            f.db.rss_claim_job("worker", 300).await.unwrap().is_none(),
            "transfer target must block RSS before fetching an unknown hash"
        );
        // A legacy queued transfer that predates an RSS checkpoint must still
        // check the other queue on claim, closing the creation/claim window.
        run_sql(&f.db, "UPDATE media_relocation_jobs SET stage='cancelled'");
        let claim = f.db.rss_claim_job("worker", 300).await.unwrap().unwrap();
        run_sql(
            &f.db,
            "UPDATE media_relocation_jobs SET stage='waiting_download'",
        );
        assert!(
            f.db.claim_due_media_relocation_jobs("transfer", 120, 10, true)
                .await
                .unwrap()
                .is_empty()
        );
        f.db.rss_archive_rule(rule.id, action(rule.version))
            .await
            .unwrap();
        assert_eq!(
            f.db.rss_get_job(claim.job.id).await.unwrap().status,
            "cancelled"
        );
    }

    #[tokio::test]
    async fn cleanup_keeps_seen_and_success_tombstones_and_all_uncertain_work() {
        let f = fixture().await;
        let (_, job) = queue_one(&f).await;
        let claim = f.db.rss_claim_job("worker", 300).await.unwrap().unwrap();
        let send =
            f.db.rss_prepare_submission(&claim, &"d".repeat(40), 1024, None)
                .await
                .unwrap()
                .unwrap();
        f.db.rss_transition_job(
            &send,
            JobTransition {
                status: "submitted".into(),
                release_reservation: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        scan(&f.db, f.feed.id, vec![normalized(2)]).await;
        assert_eq!(evaluate_all(&f.db).await, 1);
        let claim = f.db.rss_claim_job("worker", 300).await.unwrap().unwrap();
        let uncertain =
            f.db.rss_prepare_submission(&claim, &"e".repeat(40), 1024, None)
                .await
                .unwrap()
                .unwrap();
        run_sql(
            &f.db,
            "UPDATE rss_download_jobs SET updated_at='2000-01-01T00:00:00Z';UPDATE rss_items SET last_seen_at='2000-01-01T00:00:00Z'",
        );
        f.db.rss_cleanup().await.unwrap();
        assert!(matches!(
            f.db.rss_get_job(job.id).await,
            Err(RssError::NotFound(_))
        ));
        let kept = f.db.rss_get_job(uncertain.job.id).await.unwrap();
        assert_eq!(kept.reserved_bytes, 1024);
        assert_eq!(kept.status, "submitting");
        assert!(f.db.rss_get_item_locator(kept.item_id).await.is_ok());
        let conn = open_connection(&f.db.path).unwrap();
        assert_eq!(
            conn.query_row("SELECT count(*) FROM rss_seen_keys", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            2
        );
        let outcome:(Option<i64>,String)=conn.query_row("SELECT job_id,outcome FROM rss_delivery_keys WHERE key_kind='infohash' AND key_digest=?",["d".repeat(40)],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
        assert_eq!(outcome, (None, "submitted".into()));
        drop(conn);
        scan(&f.db, f.feed.id, vec![normalized(1)]).await;
        assert_eq!(evaluate_all(&f.db).await, 0);
        assert_eq!(
            f.db.rss_get_feed(f.feed.id)
                .await
                .unwrap()
                .record
                .last_sequence,
            2
        );
    }

    #[tokio::test]
    async fn concurrent_claims_share_one_slot_but_different_downloaders_can_submit_same_hash() {
        let f = fixture().await;
        let second =
            f.db.create_downloader(
                "第二下载器",
                "qb",
                "http://second.test",
                "admin",
                "password",
            )
            .await
            .unwrap();
        f.db.rss_save_rule(None, rule_input(f.feed.id, f.downloader))
            .await
            .unwrap();
        f.db.rss_save_rule(None, rule_input(f.feed.id, second))
            .await
            .unwrap();
        scan(&f.db, f.feed.id, vec![]).await;
        scan(&f.db, f.feed.id, vec![normalized(1)]).await;
        assert_eq!(evaluate_all(&f.db).await, 2);
        let (one, two, three) = tokio::join!(
            f.db.rss_claim_job("one", 300),
            f.db.rss_claim_job("two", 300),
            f.db.rss_claim_job("three", 300)
        );
        let claims = [one.unwrap(), two.unwrap(), three.unwrap()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        assert_eq!(claims.len(), 2);
        assert_ne!(claims[0].job.downloader_id, claims[1].job.downloader_id);
        let hash = "f".repeat(40);
        let (one, two) = tokio::join!(
            f.db.rss_prepare_submission(&claims[0], &hash, 1024, None),
            f.db.rss_prepare_submission(&claims[1], &hash, 1024, None)
        );
        assert!(one.unwrap().is_some());
        assert!(two.unwrap().is_some());
    }

    #[tokio::test]
    async fn free_evidence_is_rechecked_and_ordinary_failures_have_a_finite_budget() {
        let f = fixture().await;
        let mut input = rule_input(f.feed.id, f.downloader);
        input.filters.free_only = true;
        f.db.rss_save_rule(None, input).await.unwrap();
        scan(&f.db, f.feed.id, vec![]).await;
        let mut resource = normalized(1);
        resource.attributes.download_volume_factor = Some(0.0);
        scan(&f.db, f.feed.id, vec![resource]).await;
        assert_eq!(evaluate_all(&f.db).await, 1);
        let claim = f.db.rss_claim_job("worker", 300).await.unwrap().unwrap();
        let mut stale = claim.item.attributes.clone();
        stale.observed_at = "2000-01-01T00:00:00Z".into();
        assert!(
            f.db.rss_prepare_submission(&claim, &"a".repeat(40), 1024, Some(stale))
                .await
                .unwrap()
                .is_none()
        );
        let waiting = f.db.rss_get_job(claim.job.id).await.unwrap();
        assert_eq!(waiting.status, "waiting");
        assert_eq!(waiting.attempts, 0);
        f.db.rss_job_action(waiting.id, "retry", action(waiting.version))
            .await
            .unwrap();
        let claim = f.db.rss_claim_job("worker", 300).await.unwrap().unwrap();
        let mut no_longer_free = claim.item.attributes.clone();
        no_longer_free.observed_at = now();
        no_longer_free.download_volume_factor = Some(1.0);
        assert!(
            f.db.rss_prepare_submission(&claim, &"a".repeat(40), 1024, Some(no_longer_free))
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            f.db.rss_get_job(claim.job.id).await.unwrap().status,
            "waiting"
        );
        run_sql(&f.db, "UPDATE rss_download_jobs SET next_attempt_at=NULL");
        for attempt in 1..=5 {
            let claim = f.db.rss_claim_job("worker", 300).await.unwrap().unwrap();
            let job =
                f.db.rss_transition_job(
                    &claim,
                    JobTransition {
                        status: "retry_wait".into(),
                        last_error: Some("网络暂不可达".into()),
                        next_attempt_at: Some(now()),
                        release_reservation: true,
                        ..Default::default()
                    },
                )
                .await
                .unwrap();
            assert_eq!(job.attempts, attempt);
            assert_eq!(
                job.status,
                if attempt < 5 { "retry_wait" } else { "failed" }
            );
        }
        assert!(f.db.rss_claim_job("worker", 300).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn archived_source_cancels_unsent_work_but_preserves_reconciliation() {
        let f = fixture().await;
        let (_, job) = queue_one(&f).await;
        let claim = f.db.rss_claim_job("worker", 300).await.unwrap().unwrap();
        let sending =
            f.db.rss_prepare_submission(&claim, &"a".repeat(40), 1024, None)
                .await
                .unwrap()
                .unwrap();
        scan(&f.db, f.feed.id, vec![normalized(2)]).await;
        assert_eq!(evaluate_all(&f.db).await, 1);
        let feed = f.db.rss_get_feed(f.feed.id).await.unwrap().record;
        f.db.rss_archive_feed(feed.id, action(feed.version))
            .await
            .unwrap();
        assert_eq!(f.db.rss_get_job(job.id).await.unwrap().status, "submitting");
        let jobs = f.db.rss_list_jobs(ListQuery::default()).await.unwrap();
        assert!(jobs.items.iter().any(|job| job.status == "cancelled"));
        f.db.rss_release_owner("worker").await.unwrap();
        let reconciling =
            f.db.rss_claim_job("after-restart", 300)
                .await
                .unwrap()
                .unwrap();
        assert_eq!(reconciling.job.id, sending.job.id);
        assert_eq!(reconciling.job.status, "reconciling");
        f.db.rss_transition_job(
            &reconciling,
            JobTransition {
                status: "submitted".into(),
                release_reservation: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert!(
            f.db.rss_claim_job("after-restart", 300)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn enrichment_reopens_static_rejections_before_choosing_a_lower_priority_rule() {
        let f = fixture().await;
        let mut first = rule_input(f.feed.id, f.downloader);
        first.priority = 1;
        first.filters.free_only = true;
        let first = f.db.rss_save_rule(None, first).await.unwrap();
        let mut second = rule_input(f.feed.id, f.downloader);
        second.filters.min_seeders = Some(1);
        f.db.rss_save_rule(None, second).await.unwrap();
        scan(&f.db, f.feed.id, vec![]).await;
        let mut resource = normalized(1);
        resource.attributes.download_volume_factor = Some(1.0);
        resource.attributes.seeders = None;
        scan(&f.db, f.feed.id, vec![resource]).await;
        let claim =
            f.db.rss_claim_evaluation("matcher", 120)
                .await
                .unwrap()
                .unwrap();
        let revision = claim.item.content_revision;
        assert_eq!(f.db.rss_commit_evaluation(&claim, None).await.unwrap(), 0);
        run_sql(&f.db, "UPDATE rss_decisions SET next_evaluate_at=NULL");
        let claim =
            f.db.rss_claim_evaluation("matcher", 120)
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            claim.rules.len(),
            1,
            "the free-only rule is a static rejection before enrichment"
        );
        let mut attributes = claim.item.attributes.clone();
        attributes.download_volume_factor = Some(0.0);
        attributes.seeders = Some(3);
        attributes.observed_at = now();
        assert_eq!(
            f.db.rss_commit_evaluation(&claim, Some(attributes))
                .await
                .unwrap(),
            1
        );
        let item = f.db.rss_get_item(claim.item.id).await.unwrap();
        assert_eq!(item.content_revision, revision + 1);
        let job =
            f.db.rss_list_jobs(ListQuery::default())
                .await
                .unwrap()
                .items
                .remove(0);
        assert_eq!(job.rule_id, first.id);
    }

    #[tokio::test]
    async fn retry_requires_the_original_hash_and_records_measured_torrent_size() {
        let f = fixture().await;
        let (_, job) = queue_one(&f).await;
        let claim = f.db.rss_claim_job("worker", 300).await.unwrap().unwrap();
        let mut attributes = claim.item.attributes.clone();
        attributes.size_bytes = Some(2048);
        let sending =
            f.db.rss_prepare_submission(&claim, &"a".repeat(40), 2048, Some(attributes))
                .await
                .unwrap()
                .unwrap();
        assert_eq!(sending.job.size_bytes, Some(2048));
        assert_eq!(sending.item.attributes.size_bytes, Some(2048));
        f.db.rss_transition_job(
            &sending,
            JobTransition {
                status: "reconciling".into(),
                next_attempt_at: Some(now()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let reconciling = f.db.rss_claim_job("worker", 300).await.unwrap().unwrap();
        f.db.rss_transition_job(
            &reconciling,
            JobTransition {
                status: "retry_wait".into(),
                next_attempt_at: Some(now()),
                release_reservation: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let retry = f.db.rss_claim_job("worker", 300).await.unwrap().unwrap();
        assert!(
            f.db.rss_prepare_submission(&retry, &"b".repeat(40), 2048, None)
                .await
                .unwrap()
                .is_none()
        );
        let failed = f.db.rss_get_job(job.id).await.unwrap();
        assert_eq!(failed.status, "failed");
        assert_eq!(
            failed.infohash.as_deref(),
            Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
        );
        assert_eq!(failed.reserved_bytes, 0);
        assert!(failed.last_error.unwrap().contains("哈希已改变"));
    }

    #[tokio::test]
    async fn rule_resume_keeps_its_remaining_live_source_after_another_is_archived() {
        let f = fixture().await;
        let mut input = feed_input(Some(f.site));
        input.url = Some("https://tracker.test/second".into());
        let second = f.db.rss_save_feed(None, input).await.unwrap();
        let mut input = rule_input(f.feed.id, f.downloader);
        input.feed_ids.push(second.id);
        let rule = f.db.rss_save_rule(None, input).await.unwrap();
        f.db.rss_archive_feed(second.id, action(second.version))
            .await
            .unwrap();
        let paused =
            f.db.rss_set_rule_enabled(rule.id, false, action(rule.version))
                .await
                .unwrap();
        let resumed =
            f.db.rss_set_rule_enabled(rule.id, true, action(paused.version))
                .await
                .unwrap();
        assert!(resumed.enabled);
        assert_eq!(resumed.feed_ids, vec![f.feed.id]);
    }
}
