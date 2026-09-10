use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::{DateTime, Duration, Utc};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

use super::{Database, join_error, open_connection, sql_error};
use crate::error::AppError;
use crate::openlist::openlist_identity_key;

static CLAIM_SEQUENCE: AtomicU64 = AtomicU64::new(1);

fn next_openlist_config_updated_at(current: &str) -> String {
    let now = Utc::now();
    let next = DateTime::parse_from_rfc3339(current)
        .ok()
        .map(|value| value.with_timezone(&Utc))
        .filter(|value| value >= &now)
        .and_then(|value| value.checked_add_signed(Duration::nanoseconds(1)))
        .unwrap_or(now);
    next.to_rfc3339()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenListPathMapping {
    pub id: Option<i64>,
    pub downloader_id: i64,
    pub qb_path: String,
    pub openlist_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenListTargetDirectory {
    pub id: Option<i64>,
    pub name: String,
    pub downloader_id: i64,
    #[serde(rename = "openlist_root")]
    pub openlist_path: String,
    #[serde(rename = "qb_root")]
    pub qb_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenListConfig {
    pub base_url: String,
    pub api_key: String,
    pub enabled: bool,
    pub target_directory_id: Option<i64>,
    pub selected_target_index: Option<usize>,
    pub scan_interval_secs: u64,
    pub updated_at: String,
    pub path_mappings: Vec<OpenListPathMapping>,
    pub target_directories: Vec<OpenListTargetDirectory>,
}

/// A manual transfer target snapshot.  The relocation worker persists these
/// paths before it starts copying so a later settings change cannot redirect
/// an already-confirmed transfer.
#[derive(Debug, Clone)]
pub struct ManualMediaRelocationTarget {
    pub infohash: String,
    pub torrent_name: String,
    pub target_openlist_path: String,
    pub target_qb_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaRelocationJob {
    pub id: i64,
    pub media_download_id: Option<i64>,
    pub downloader_id: Option<i64>,
    pub infohash: String,
    pub source_qb_path: String,
    pub source_openlist_path: String,
    pub source_content_openlist_path: String,
    pub target_openlist_path: String,
    pub target_qb_path: String,
    pub target_content_qb_path: String,
    pub target_downloader_id: Option<i64>,
    pub copy_items_json: String,
    pub source_files_json: String,
    pub source_manifest_json: String,
    pub copy_checkpoint_json: Option<String>,
    pub copy_lock_acquired: bool,
    pub manifest_cursor: usize,
    pub target_root_folder: Option<bool>,
    pub torrent_name: String,
    pub stage: String,
    pub openlist_task_id: Option<String>,
    pub torrent_data: Option<Vec<u8>>,
    pub attempts: u32,
    pub next_attempt_at: Option<String>,
    pub lease_owner: Option<String>,
    pub lease_until: Option<String>,
    pub version: i64,
    pub last_error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub completed_at: Option<String>,
    pub manual_requested_at: Option<String>,
    pub stage_started_at: String,
}

impl Database {
    pub async fn has_in_flight_openlist_operations(&self) -> Result<bool, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM media_relocation_jobs
                    WHERE stage NOT IN ('completed', 'cancelled')
                      AND (copy_lock_acquired=1 OR openlist_task_id IS NOT NULL
                           OR copy_checkpoint_json IS NOT NULL)
                )",
                [],
                |row| row.get(0),
            )
            .map_err(sql_error)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn list_manual_media_relocation_jobs(
        &self,
        page: usize,
        page_size: usize,
    ) -> Result<(Vec<MediaRelocationJob>, usize), AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = open_connection(&path)?;
            let page = page.max(1);
            let page_size = page_size.clamp(1, 100);
            let offset = i64::try_from(page.saturating_sub(1).saturating_mul(page_size))
                .unwrap_or(i64::MAX);
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Deferred)
                .map_err(sql_error)?;
            let total = tx
                .query_row(
                    "SELECT COUNT(*) FROM media_relocation_jobs
                     WHERE media_download_id IS NULL",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(sql_error)?
                .max(0) as usize;
            let mut stmt = tx
                .prepare(
                    "SELECT id, media_download_id, downloader_id, infohash, source_qb_path,
                            source_openlist_path, target_openlist_path, target_qb_path, torrent_name,
                            stage, openlist_task_id, torrent_data, attempts, next_attempt_at,
                            lease_owner, lease_until, version, last_error, created_at, updated_at,
                            completed_at, source_content_openlist_path, target_content_qb_path,
                            target_downloader_id, copy_items_json, source_files_json,
                            target_root_folder, source_manifest_json, copy_checkpoint_json,
                            copy_lock_acquired, manifest_cursor, manual_requested_at,
                            stage_started_at
                     FROM media_relocation_jobs
                     WHERE media_download_id IS NULL
                     ORDER BY CASE
                         WHEN stage IN ('copy_manual_review', 'qb_manual_review',
                                        'source_remove_manual_review', 'manifest_required') THEN 0
                         WHEN stage IN ('completed', 'cancelled') THEN 2
                         ELSE 1 END,
                         id DESC LIMIT ? OFFSET ?",
                )
                .map_err(sql_error)?;
            let records = stmt
                .query_map(
                    [page_size as i64, offset],
                    map_media_relocation_job,
                )
                .map_err(sql_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(sql_error)?;
            drop(stmt);
            tx.commit().map_err(sql_error)?;
            Ok((records, total))
        })
        .await
        .map_err(join_error)?
    }

    #[cfg(test)]
    pub async fn enqueue_manual_media_relocation_jobs(
        &self,
        downloader_id: i64,
        target_downloader_id: i64,
        target_openlist_path: &str,
        target_qb_path: &str,
        torrents: &[(String, String)],
    ) -> Result<(usize, usize), AppError> {
        self.enqueue_manual_media_relocation_jobs_inner(
            downloader_id,
            target_downloader_id,
            target_openlist_path,
            target_qb_path,
            torrents,
            None,
            None,
            None,
            None,
        )
        .await
    }

    pub async fn enqueue_manual_media_relocation_jobs_if_config_current(
        &self,
        downloader_id: i64,
        target_downloader_id: i64,
        target_openlist_path: &str,
        target_qb_path: &str,
        torrents: &[(String, String)],
        expected_openlist_config_updated_at: &str,
        expected_source_downloader_updated_at: &str,
        expected_target_downloader_updated_at: &str,
    ) -> Result<(usize, usize), AppError> {
        self.enqueue_manual_media_relocation_jobs_inner(
            downloader_id,
            target_downloader_id,
            target_openlist_path,
            target_qb_path,
            torrents,
            None,
            Some(expected_openlist_config_updated_at),
            Some(expected_source_downloader_updated_at),
            Some(expected_target_downloader_updated_at),
        )
        .await
    }

    pub async fn enqueue_manual_media_relocation_jobs_with_targets_if_config_current(
        &self,
        downloader_id: i64,
        target_downloader_id: i64,
        targets: &[ManualMediaRelocationTarget],
        expected_openlist_config_updated_at: &str,
        expected_source_downloader_updated_at: &str,
        expected_target_downloader_updated_at: &str,
    ) -> Result<(usize, usize), AppError> {
        self.enqueue_manual_media_relocation_jobs_inner(
            downloader_id,
            target_downloader_id,
            "",
            "",
            &targets
                .iter()
                .map(|target| (target.infohash.clone(), target.torrent_name.clone()))
                .collect::<Vec<_>>(),
            Some(
                targets
                    .iter()
                    .map(|target| {
                        (
                            target.infohash.clone(),
                            target.target_openlist_path.clone(),
                            target.target_qb_path.clone(),
                        )
                    })
                    .collect(),
            ),
            Some(expected_openlist_config_updated_at),
            Some(expected_source_downloader_updated_at),
            Some(expected_target_downloader_updated_at),
        )
        .await
    }

    async fn enqueue_manual_media_relocation_jobs_inner(
        &self,
        downloader_id: i64,
        target_downloader_id: i64,
        target_openlist_path: &str,
        target_qb_path: &str,
        torrents: &[(String, String)],
        target_overrides: Option<Vec<(String, String, String)>>,
        expected_openlist_config_updated_at: Option<&str>,
        expected_source_downloader_updated_at: Option<&str>,
        expected_target_downloader_updated_at: Option<&str>,
    ) -> Result<(usize, usize), AppError> {
        let path = self.path.clone();
        let target_openlist_path = target_openlist_path.to_string();
        let target_qb_path = target_qb_path.to_string();
        let torrents = torrents.to_vec();
        let target_overrides = target_overrides
            .unwrap_or_default()
            .into_iter()
            .map(|(hash, openlist, qb)| (hash.to_ascii_lowercase(), (openlist, qb)))
            .collect::<HashMap<_, _>>();
        let expected_openlist_config_updated_at =
            expected_openlist_config_updated_at.map(str::to_string);
        let expected_source_downloader_updated_at =
            expected_source_downloader_updated_at.map(str::to_string);
        let expected_target_downloader_updated_at =
            expected_target_downloader_updated_at.map(str::to_string);
        tokio::task::spawn_blocking(move || {
            let mut conn = open_connection(&path)?;
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sql_error)?;
            if let Some(expected) = expected_openlist_config_updated_at {
                let current = tx
                    .query_row(
                        "SELECT updated_at FROM openlist_settings WHERE id=1",
                        [],
                        |row| row.get::<_, String>(0),
                    )
                    .map_err(sql_error)?;
                if current != expected {
                    return Err(AppError::InvalidConfig {
                        message: "OpenList settings changed; reload migration targets before creating tasks"
                            .to_string(),
                    });
                }
            }
            for (role, id, expected_updated_at) in [
                (
                    "Source",
                    downloader_id,
                    expected_source_downloader_updated_at,
                ),
                (
                    "Target",
                    target_downloader_id,
                    expected_target_downloader_updated_at,
                ),
            ] {
                if let Some(expected_updated_at) = expected_updated_at {
                    let current = tx
                        .query_row(
                            "SELECT downloader_type, updated_at FROM downloaders WHERE id=?",
                            [id],
                            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                        )
                        .optional()
                        .map_err(sql_error)?;
                    let current = current.as_ref().filter(|(downloader_type, updated_at)| {
                        matches!(downloader_type.as_str(), "qbittorrent" | "qb")
                            && updated_at == &expected_updated_at
                    });
                    if current.is_none() {
                        return Err(AppError::InvalidConfig {
                            message: format!(
                                "{role} downloader changed; reload downloaders and migration targets before creating tasks"
                            ),
                        });
                    }
                }
            }
            let now = Utc::now().to_rfc3339();
            let mut inserted = 0;
            let mut skipped = 0;
            for (infohash, name) in torrents {
                let (active_relocation, active_download) = tx
                    .query_row(
                        "SELECT
                             EXISTS(
                                 SELECT 1 FROM media_relocation_jobs
                                 WHERE lower(infohash) = lower(?1)
                                   AND (downloader_id IN (?2, ?3)
                                        OR target_downloader_id IN (?2, ?3))
                                   AND (stage NOT IN ('completed', 'cancelled')
                                        OR (lease_until IS NOT NULL AND lease_until >= ?4))
                             ),
                             EXISTS(
                                 SELECT 1 FROM media_downloads
                                 WHERE downloader_id IN (?2, ?3)
                                   AND (
                                       status = 'fetching'
                                       OR (
                                           lower(infohash) = lower(?1)
                                           AND (status NOT IN ('submitted', 'failed', 'cancelled')
                                                OR (lease_until IS NOT NULL AND lease_until >= ?4)
                                                OR (
                                                    status = 'failed'
                                                    AND length(trim(infohash)) > 0
                                                    AND subscription_id IS NOT NULL
                                                    AND EXISTS(
                                                        SELECT 1 FROM subscription_targets AS target
                                                        WHERE target.subscription_id = media_downloads.subscription_id
                                                          AND target.target_key = media_downloads.target_key
                                                          AND target.status = 'queued'
                                                    )
                                                ))
                                       )
                                   )
                             )",
                        params![infohash, downloader_id, target_downloader_id, now],
                        |row| Ok((row.get::<_, bool>(0)?, row.get::<_, bool>(1)?)),
                    )
                    .map_err(sql_error)?;
                let active_rss:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM rss_download_jobs
                    WHERE downloader_id IN (?1,?2) AND status IN ('fetching','submitting','reconciling')
                    AND (infohash IS NULL OR lower(infohash)=lower(?3)))",
                    params![downloader_id,target_downloader_id,infohash],|row|row.get(0)).map_err(sql_error)?;
                if active_relocation || active_download || active_rss {
                    skipped += 1;
                    continue;
                }
                let (target_openlist_path, target_qb_path) = target_overrides
                    .get(&infohash.to_ascii_lowercase())
                    .cloned()
                    .unwrap_or_else(|| {
                        (target_openlist_path.clone(), target_qb_path.clone())
                    });
                tx.execute(
                    "INSERT INTO media_relocation_jobs
                     (media_download_id, downloader_id, infohash, source_qb_path,
                      source_openlist_path, target_openlist_path, target_qb_path,
                      target_downloader_id, torrent_name, stage, manual_requested_at,
                      stage_started_at, created_at, updated_at)
                      VALUES (NULL, ?, ?, '', '', ?, ?, ?, ?, 'waiting_download', ?, ?, ?, ?)",
                    params![
                        downloader_id,
                        infohash,
                        target_openlist_path,
                        target_qb_path,
                        target_downloader_id,
                        name,
                        now,
                        now,
                        now,
                        now
                    ],
                )
                .map_err(sql_error)?;
                inserted += 1;
            }
            tx.commit().map_err(sql_error)?;
            Ok((inserted, skipped))
        })
        .await
        .map_err(join_error)?
    }

    #[cfg(test)]
    pub async fn list_media_relocation_jobs(
        &self,
        limit: usize,
    ) -> Result<Vec<MediaRelocationJob>, AppError> {
        self.list_automatic_media_relocation_jobs(1, limit.clamp(1, 500))
            .await
            .map(|(records, _)| records)
    }

    pub async fn list_automatic_media_relocation_jobs(
        &self,
        page: usize,
        page_size: usize,
    ) -> Result<(Vec<MediaRelocationJob>, usize), AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = open_connection(&path)?;
            let page = page.max(1);
            let page_size = page_size.clamp(1, 500);
            let offset = i64::try_from(page.saturating_sub(1).saturating_mul(page_size))
                .unwrap_or(i64::MAX);
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Deferred)
                .map_err(sql_error)?;
            let total = tx
                .query_row(
                    "SELECT COUNT(*) FROM media_relocation_jobs
                     WHERE media_download_id IS NOT NULL
                       AND cleared_at IS NULL",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(sql_error)?
                .max(0) as usize;
            let records = {
                let mut stmt = tx
                    .prepare(
                    "SELECT id, media_download_id, downloader_id, infohash, source_qb_path,
                            source_openlist_path, target_openlist_path, target_qb_path, torrent_name,
                            stage, openlist_task_id, torrent_data, attempts, next_attempt_at,
                            lease_owner, lease_until, version, last_error, created_at, updated_at,
                            completed_at, source_content_openlist_path, target_content_qb_path,
                            target_downloader_id, copy_items_json, source_files_json,
                            target_root_folder, source_manifest_json, copy_checkpoint_json,
                            copy_lock_acquired, manifest_cursor, manual_requested_at,
                            stage_started_at
                     FROM media_relocation_jobs
                     WHERE media_download_id IS NOT NULL
                       AND cleared_at IS NULL
                     ORDER BY CASE
                         WHEN stage IN ('planning_manual_review', 'copy_manual_review',
                                        'manifest_required')
                              OR (stage='copying' AND copy_checkpoint_json IS NULL) THEN 0
                         WHEN stage NOT IN ('auto_copy_paused', 'completed', 'cancelled') THEN 1
                         WHEN stage='auto_copy_paused' THEN 2
                         ELSE 3 END,
                         id DESC LIMIT ? OFFSET ?",
                    )
                    .map_err(sql_error)?;
                stmt.query_map(params![page_size as i64, offset], map_media_relocation_job)
                    .map_err(sql_error)?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(sql_error)?
            };
            tx.commit().map_err(sql_error)?;
            Ok((records, total))
        })
        .await
        .map_err(join_error)?
    }

    /// Stop every automatic copy job and remove it from the active queue.
    ///
    /// The rows remain as local tombstones so a later scheduler scan cannot
    /// recreate the same job from its submitted media download.  Any worker
    /// that still holds a lease is invalidated by the version bump; an already
    /// submitted remote OpenList operation cannot be recalled by this local
    /// state change.
    pub async fn stop_and_clear_automatic_media_relocation_jobs(&self) -> Result<usize, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = open_connection(&path)?;
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sql_error)?;
            let now = Utc::now().to_rfc3339();
            let changed = tx
                .execute(
                    "UPDATE media_relocation_jobs
                     SET stage=CASE
                           WHEN stage IN ('completed', 'cancelled') THEN stage
                           ELSE 'cancelled'
                         END,
                         openlist_task_id=NULL,
                         copy_checkpoint_json=NULL,
                         copy_lock_acquired=0,
                         manifest_cursor=0,
                         attempts=0,
                         next_attempt_at=NULL,
                         lease_owner=NULL,
                         lease_until=NULL,
                         manual_requested_at=NULL,
                         last_error=CASE
                           WHEN stage IN ('completed', 'cancelled') THEN last_error
                           ELSE 'OpenList 自动复制任务已由用户批量停止'
                         END,
                         completed_at=CASE
                           WHEN stage IN ('completed', 'cancelled') THEN completed_at
                           ELSE COALESCE(completed_at, ?)
                         END,
                         stage_started_at=?,
                         updated_at=?,
                         cleared_at=?,
                         version=version+1
                     WHERE media_download_id IS NOT NULL
                       AND cleared_at IS NULL",
                    params![now, now, now, now],
                )
                .map_err(sql_error)?;
            tx.commit().map_err(sql_error)?;
            Ok(changed)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn enqueue_submitted_media_relocation_jobs(
        &self,
        processing_enabled: bool,
    ) -> Result<usize, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = open_connection(&path)?;
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sql_error)?;
            let now = Utc::now().to_rfc3339();
            let inserted = tx
                .execute(
                    "INSERT INTO media_relocation_jobs
                 (media_download_id, downloader_id, infohash, source_qb_path,
                  source_openlist_path, target_openlist_path, target_qb_path, torrent_name,
                   stage, stage_started_at, created_at, updated_at)
                 SELECT id, downloader_id, infohash, '', '', '', '', title,
                        CASE WHEN ? THEN 'waiting_download' ELSE 'auto_copy_paused' END,
                        ?, ?, ? FROM media_downloads m
                  WHERE status = 'submitted' AND length(trim(infohash)) = 40
                    AND downloader_id IS NOT NULL
                    AND (lease_until IS NULL OR lease_until < ?)
                    AND NOT EXISTS (SELECT 1 FROM media_relocation_jobs j
                                    WHERE j.media_download_id = m.id)
                    AND NOT EXISTS (SELECT 1 FROM media_relocation_jobs active
                                      WHERE (active.downloader_id = m.downloader_id
                                             OR active.target_downloader_id = m.downloader_id)
                                      AND lower(active.infohash) = lower(m.infohash)
                                      AND active.stage NOT IN ('completed', 'cancelled'))
                    AND NOT EXISTS (SELECT 1 FROM rss_download_jobs rss
                        WHERE rss.downloader_id=m.downloader_id AND rss.status IN ('fetching','submitting','reconciling')
                        AND (rss.infohash IS NULL OR lower(rss.infohash)=lower(m.infohash)))",
                    params![processing_enabled, now, now, now, now],
                )
                .map_err(sql_error)?;
            if !processing_enabled {
                tx.execute(
                    "UPDATE media_relocation_jobs
                     SET stage='auto_copy_paused', next_attempt_at=NULL,
                         lease_owner=NULL, lease_until=NULL, version=version+1,
                         stage_started_at=?, updated_at=?
                     WHERE stage='waiting_download'
                       AND media_download_id IS NOT NULL",
                    params![now, now],
                )
                .map_err(sql_error)?;
            }
            tx.commit().map_err(sql_error)?;
            Ok(inserted)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn claim_due_media_relocation_jobs(
        &self,
        owner: &str,
        lease_secs: i64,
        limit: usize,
        include_automatic: bool,
    ) -> Result<Vec<MediaRelocationJob>, AppError> {
        let path = self.path.clone();
        let owner = owner.to_string();
        tokio::task::spawn_blocking(move || {
            let mut conn = open_connection(&path)?;
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sql_error)?;
            let now = Utc::now();
            let now_text = now.to_rfc3339();
            let until = (now + chrono::Duration::seconds(lease_secs.max(10))).to_rfc3339();
            let claim_owner = format!(
                "{owner}#{}#{}",
                now.timestamp_nanos_opt().unwrap_or_default(),
                CLAIM_SEQUENCE.fetch_add(1, Ordering::Relaxed)
            );
            tx.execute(
                "UPDATE media_relocation_jobs SET lease_owner = ?, lease_until = ?,
                     version = version + 1, updated_at = ?
                 WHERE id IN (SELECT id FROM media_relocation_jobs relocation
                     WHERE stage NOT IN ('completed', 'cancelled', 'planning_manual_review',
                                         'copy_manual_review', 'qb_manual_review',
                                         'source_remove_manual_review', 'manifest_required')
                       AND (? OR media_download_id IS NULL
                            OR stage NOT IN ('waiting_download', 'auto_copy_paused'))
                       AND (next_attempt_at IS NULL OR next_attempt_at <= ?)
                       AND (lease_until IS NULL OR lease_until < ?)
                       AND NOT EXISTS(SELECT 1 FROM rss_download_jobs rss
                           WHERE rss.downloader_id IN (relocation.downloader_id,relocation.target_downloader_id)
                           AND rss.status IN ('fetching','submitting','reconciling')
                           AND (rss.infohash IS NULL OR lower(rss.infohash)=lower(relocation.infohash)))
                     ORDER BY id LIMIT ?)",
                params![
                    claim_owner,
                    until,
                    now_text,
                    include_automatic,
                    now_text,
                    now_text,
                    limit.max(1) as i64
                ],
            )
            .map_err(sql_error)?;
            let mut stmt = tx
                .prepare(
                    "SELECT id, media_download_id, downloader_id, infohash, source_qb_path,
                        source_openlist_path, target_openlist_path, target_qb_path, torrent_name,
                        stage, openlist_task_id, torrent_data, attempts, next_attempt_at,
                        lease_owner, lease_until, version, last_error, created_at, updated_at,
                        completed_at, source_content_openlist_path, target_content_qb_path,
                        target_downloader_id, copy_items_json, source_files_json,
                        target_root_folder, source_manifest_json, copy_checkpoint_json,
                        copy_lock_acquired, manifest_cursor, manual_requested_at,
                        stage_started_at
                        FROM media_relocation_jobs WHERE lease_owner = ? ORDER BY id",
                )
                .map_err(sql_error)?;
            let jobs = stmt
                .query_map([&claim_owner], map_media_relocation_job)
                .map_err(sql_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(sql_error)?;
            drop(stmt);
            tx.commit().map_err(sql_error)?;
            Ok(jobs)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn next_media_relocation_attempt_at(
        &self,
        include_automatic: bool,
    ) -> Result<Option<String>, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let now = Utc::now().to_rfc3339();
            conn.query_row(
                "SELECT COALESCE(next_attempt_at, ?) FROM media_relocation_jobs
                 WHERE stage NOT IN ('completed', 'cancelled', 'planning_manual_review',
                                     'copy_manual_review', 'qb_manual_review',
                                     'source_remove_manual_review', 'manifest_required')
                   AND (? OR media_download_id IS NULL
                        OR stage NOT IN ('waiting_download', 'auto_copy_paused'))
                 ORDER BY CASE WHEN next_attempt_at IS NULL THEN 0 ELSE 1 END,
                          next_attempt_at
                 LIMIT 1",
                params![now, include_automatic],
                |row| row.get(0),
            )
            .optional()
            .map_err(sql_error)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn update_media_relocation_job(
        &self,
        job: &MediaRelocationJob,
        expected_version: i64,
        expected_stage: &str,
        expected_openlist_config_updated_at: Option<&str>,
    ) -> Result<bool, AppError> {
        let path = self.path.clone();
        let job = job.clone();
        let expected_stage = expected_stage.to_string();
        let expected_openlist_config_updated_at =
            expected_openlist_config_updated_at.map(str::to_string);
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let changed = conn
                .execute(
                    "UPDATE media_relocation_jobs SET source_qb_path=?, source_openlist_path=?,
                     source_content_openlist_path=?, target_openlist_path=?, target_qb_path=?,
                     target_content_qb_path=?, target_downloader_id=?, copy_items_json=?,
                     source_files_json=?, source_manifest_json=?, copy_checkpoint_json=?,
                     copy_lock_acquired=?, manifest_cursor=?, target_root_folder=?, torrent_name=?,
                     stage_started_at=CASE WHEN stage<>? THEN ? ELSE stage_started_at END, stage=?,
                     openlist_task_id=?, torrent_data=?, attempts=?, next_attempt_at=?,
                     lease_owner=NULL, lease_until=NULL, version=version+1, last_error=?,
                     updated_at=?, completed_at=? WHERE id=? AND version=? AND stage=?
                     AND lease_owner=? AND lease_until >= ?
                     AND (? IS NULL OR EXISTS (
                         SELECT 1 FROM openlist_settings
                         WHERE id=1 AND updated_at=?
                     ))
                     AND NOT EXISTS(SELECT 1 FROM rss_download_jobs rss
                         WHERE rss.downloader_id IN (media_relocation_jobs.downloader_id,?)
                         AND rss.status IN ('fetching','submitting','reconciling')
                         AND (rss.infohash IS NULL OR lower(rss.infohash)=lower(media_relocation_jobs.infohash)))",
                    params![
                        job.source_qb_path,
                        job.source_openlist_path,
                        job.source_content_openlist_path,
                        job.target_openlist_path,
                        job.target_qb_path,
                        job.target_content_qb_path,
                        job.target_downloader_id,
                        job.copy_items_json,
                        job.source_files_json,
                        job.source_manifest_json,
                        job.copy_checkpoint_json,
                        job.copy_lock_acquired,
                        job.manifest_cursor as i64,
                        job.target_root_folder,
                        job.torrent_name,
                        job.stage,
                        Utc::now().to_rfc3339(),
                        job.stage,
                        job.openlist_task_id,
                        job.torrent_data,
                        job.attempts as i64,
                        job.next_attempt_at,
                        job.last_error,
                        Utc::now().to_rfc3339(),
                        job.completed_at,
                        job.id,
                        expected_version,
                        expected_stage,
                        job.lease_owner,
                        Utc::now().to_rfc3339(),
                        expected_openlist_config_updated_at.as_deref(),
                        expected_openlist_config_updated_at.as_deref(),
                        job.target_downloader_id,
                    ],
                )
                .map_err(sql_error)?;
            Ok(changed == 1)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn renew_media_relocation_lease(
        &self,
        id: i64,
        expected_version: i64,
        expected_stage: &str,
        owner: &str,
        lease_secs: i64,
    ) -> Result<bool, AppError> {
        let path = self.path.clone();
        let expected_stage = expected_stage.to_string();
        let owner = owner.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let now = Utc::now();
            let now_text = now.to_rfc3339();
            let lease_until = (now + chrono::Duration::seconds(lease_secs.max(10))).to_rfc3339();
            conn.execute(
                "UPDATE media_relocation_jobs SET lease_until=?, updated_at=?
                 WHERE id=? AND version=? AND stage=? AND lease_owner=?
                   AND lease_until IS NOT NULL AND lease_until >= ?",
                params![
                    lease_until,
                    now_text,
                    id,
                    expected_version,
                    expected_stage,
                    owner,
                    now_text
                ],
            )
            .map(|changed| changed == 1)
            .map_err(sql_error)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn checkpoint_media_relocation_copy_submission(
        &self,
        id: i64,
        expected_version: i64,
        expected_stage: &str,
        owner: &str,
        checkpoint_json: &str,
    ) -> Result<bool, AppError> {
        let path = self.path.clone();
        let expected_stage = expected_stage.to_string();
        let owner = owner.to_string();
        let checkpoint_json = checkpoint_json.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let now = Utc::now().to_rfc3339();
            conn.execute(
                "UPDATE media_relocation_jobs SET copy_checkpoint_json=?, updated_at=?
                 WHERE id=? AND version=? AND stage=? AND lease_owner=?
                   AND lease_until IS NOT NULL AND lease_until >= ?",
                params![
                    checkpoint_json,
                    now,
                    id,
                    expected_version,
                    expected_stage,
                    owner,
                    now
                ],
            )
            .map(|changed| changed == 1)
            .map_err(sql_error)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn try_acquire_media_relocation_target_lock(
        &self,
        id: i64,
        expected_version: i64,
        expected_stage: &str,
        owner: &str,
        target_path: &str,
    ) -> Result<bool, AppError> {
        let path = self.path.clone();
        let expected_stage = expected_stage.to_string();
        let owner = owner.to_string();
        let target_path = target_path.to_string();
        tokio::task::spawn_blocking(move || {
            let mut conn = open_connection(&path)?;
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sql_error)?;
            let now = Utc::now().to_rfc3339();
            let owned = tx
                .query_row(
                    "SELECT copy_lock_acquired FROM media_relocation_jobs
                     WHERE id=? AND version=? AND stage=? AND lease_owner=?
                       AND lease_until IS NOT NULL AND lease_until >= ?",
                    params![id, expected_version, expected_stage, owner, now],
                    |row| row.get::<_, bool>(0),
                )
                .optional()
                .map_err(sql_error)?;
            let Some(owned) = owned else {
                tx.commit().map_err(sql_error)?;
                return Ok(false);
            };
            if owned {
                tx.commit().map_err(sql_error)?;
                return Ok(true);
            }

            let mut statement = tx
                .prepare(
                    "SELECT target_openlist_path FROM media_relocation_jobs
                     WHERE id != ? AND copy_lock_acquired = 1
                       AND stage IN ('copy_reconcile', 'copy_legacy_reconcile', 'copy_submitting',
                                     'copying', 'copy_succeeded', 'copy_manual_review')",
                )
                .map_err(sql_error)?;
            let locked_paths = statement
                .query_map([id], |row| row.get::<_, String>(0))
                .map_err(sql_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(sql_error)?;
            drop(statement);
            if locked_paths
                .iter()
                .any(|locked| remote_paths_overlap(locked, &target_path))
            {
                tx.commit().map_err(sql_error)?;
                return Ok(false);
            }
            let changed = tx
                .execute(
                    "UPDATE media_relocation_jobs SET copy_lock_acquired=1, updated_at=?
                     WHERE id=? AND version=? AND stage=? AND lease_owner=?
                       AND lease_until IS NOT NULL AND lease_until >= ?",
                    params![now, id, expected_version, expected_stage, owner, now],
                )
                .map_err(sql_error)?;
            tx.commit().map_err(sql_error)?;
            Ok(changed == 1)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn record_media_relocation_retry(
        &self,
        job: &MediaRelocationJob,
        manual_stage: Option<&str>,
    ) -> Result<bool, AppError> {
        let path = self.path.clone();
        let job = job.clone();
        let manual_stage = manual_stage.map(str::to_string);
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let now = Utc::now().to_rfc3339();
            conn.execute(
                "UPDATE media_relocation_jobs SET
                     stage=COALESCE(?, stage), attempts=?,
                     next_attempt_at=CASE WHEN ? IS NULL THEN ? ELSE NULL END,
                     last_error=?,
                     lease_owner=NULL, lease_until=NULL, version=version+1, updated_at=?
                     ,stage_started_at=CASE WHEN ? IS NULL THEN stage_started_at ELSE ? END
                 WHERE id=? AND version=? AND stage=? AND lease_owner=? AND lease_until >= ?",
                params![
                    manual_stage.as_deref(),
                    job.attempts as i64,
                    manual_stage.as_deref(),
                    job.next_attempt_at,
                    job.last_error,
                    now,
                    manual_stage.as_deref(),
                    now,
                    job.id,
                    job.version,
                    job.stage,
                    job.lease_owner,
                    now
                ],
            )
            .map(|changed| changed == 1)
            .map_err(sql_error)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn resolve_media_relocation_copy(
        &self,
        id: i64,
        resolution: &str,
        expected_version: i64,
        confirm_task_terminated: bool,
    ) -> Result<bool, AppError> {
        let path = self.path.clone();
        let resolution = resolution.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let now = Utc::now().to_rfc3339();
            let (clear_checkpoint, release_lock) = match resolution.as_str() {
                "recheck" => (false, false),
                "cancel" => (true, true),
                _ => return Ok(false),
            };
            let changed = conn
                .execute(
                    "UPDATE media_relocation_jobs
                     SET stage=CASE
                           WHEN stage='planning_manual_review' AND ?='recheck'
                             THEN 'waiting_download'
                           WHEN ?='recheck' AND copy_checkpoint_json IS NULL
                             THEN 'copy_legacy_reconcile'
                           WHEN ?='recheck' THEN 'copy_submitting'
                           ELSE 'cancelled' END,
                         openlist_task_id=CASE WHEN ? THEN NULL ELSE openlist_task_id END,
                         copy_checkpoint_json=CASE WHEN ? THEN NULL ELSE copy_checkpoint_json END,
                         copy_lock_acquired=CASE WHEN ? THEN 0 ELSE copy_lock_acquired END,
                         manifest_cursor=CASE WHEN ? THEN 0 ELSE manifest_cursor END,
                         attempts=0, next_attempt_at=CASE WHEN ? = 'cancel' THEN NULL ELSE ? END,
                         lease_owner=NULL, lease_until=NULL, version=version+1,
                         last_error=NULL, stage_started_at=?, updated_at=?
                     WHERE id=? AND version=?
                       AND ((stage='planning_manual_review'
                             AND media_download_id IS NOT NULL
                             AND openlist_task_id IS NULL
                             AND copy_checkpoint_json IS NULL
                             AND copy_lock_acquired=0
                             AND ? IN ('recheck', 'cancel'))
                            OR stage='copy_manual_review'
                            OR (stage='copying' AND copy_checkpoint_json IS NULL)
                            OR (stage IN ('manifest_required', 'auto_copy_paused')
                                AND ?='cancel'))
                       AND (lease_until IS NULL OR lease_until < ?)
                       AND (? != 'cancel' OR ? OR stage='planning_manual_review')",
                    params![
                        resolution,
                        resolution,
                        resolution,
                        clear_checkpoint,
                        clear_checkpoint,
                        release_lock,
                        clear_checkpoint,
                        resolution,
                        now,
                        now,
                        now,
                        id,
                        expected_version,
                        resolution,
                        resolution,
                        now,
                        resolution,
                        confirm_task_terminated
                    ],
                )
                .map_err(sql_error)?;
            Ok(changed == 1)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn recheck_media_relocation_manifest(
        &self,
        id: i64,
        expected_version: i64,
    ) -> Result<bool, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let now = Utc::now().to_rfc3339();
            conn.execute(
                "UPDATE media_relocation_jobs
                 SET stage='manifest_recheck', attempts=0, next_attempt_at=?,
                     lease_owner=NULL, lease_until=NULL, version=version+1,
                     last_error=NULL, stage_started_at=?, updated_at=?
                 WHERE id=? AND version=? AND stage='manifest_required'
                   AND media_download_id IS NULL
                   AND (lease_until IS NULL OR lease_until < ?)",
                params![now, now, now, id, expected_version, now],
            )
            .map(|changed| changed == 1)
            .map_err(sql_error)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn retry_media_relocation_migration(
        &self,
        id: i64,
        expected_version: i64,
    ) -> Result<bool, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let now = Utc::now().to_rfc3339();
            conn.execute(
                "UPDATE media_relocation_jobs
                  SET stage=CASE
                        WHEN stage='source_remove_manual_review' THEN 'source_removing'
                        WHEN source_openlist_path='' THEN 'waiting_download'
                        WHEN torrent_data IS NULL THEN 'copy_verified'
                        ELSE 'qb_reconcile' END,
                      attempts=0, next_attempt_at=?,
                      lease_owner=NULL, lease_until=NULL, version=version+1,
                      openlist_task_id=NULL,
                      copy_checkpoint_json=CASE
                        WHEN stage='source_remove_manual_review' THEN copy_checkpoint_json
                        ELSE NULL END,
                      copy_lock_acquired=0,
                      last_error=NULL,
                      stage_started_at=?, updated_at=?
                  WHERE id=? AND version=?
                    AND stage IN ('qb_manual_review', 'source_remove_manual_review')
                   AND media_download_id IS NULL
                    AND (lease_until IS NULL OR lease_until < ?)",
                params![now, now, now, id, expected_version, now],
            )
            .map(|changed| changed == 1)
            .map_err(sql_error)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn abandon_media_relocation_migration(
        &self,
        id: i64,
        expected_version: i64,
    ) -> Result<bool, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let now = Utc::now().to_rfc3339();
            conn.execute(
                "UPDATE media_relocation_jobs
                 SET stage='cancelled', attempts=0, next_attempt_at=NULL,
                     lease_owner=NULL, lease_until=NULL, version=version+1,
                     copy_lock_acquired=0,
                     last_error='迁移已由用户放弃；系统不会继续删除文件；此前可能已移除源 qB 任务或部分源文件，目标 qB 任务也可能保留，请按当前实际状态手动核验',
                     completed_at=?, stage_started_at=?, updated_at=?
                  WHERE id=? AND version=?
                    AND stage IN ('qb_manual_review', 'source_remove_manual_review')
                   AND media_download_id IS NULL
                   AND (lease_until IS NULL OR lease_until < ?)",
                params![now, now, now, id, expected_version, now],
            )
            .map(|changed| changed == 1)
            .map_err(sql_error)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn get_openlist_config(&self) -> Result<OpenListConfig, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let (base_url, api_key, enabled, target_directory_id, scan_interval_secs, updated_at) =
                conn.query_row(
                    "SELECT base_url, api_key, enabled, target_directory_id,
                            scan_interval_secs, updated_at
                     FROM openlist_settings WHERE id = 1",
                    [],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get::<_, i64>(2)? != 0,
                            row.get::<_, Option<i64>>(3)?,
                            row.get::<_, i64>(4)?.max(1) as u64,
                            row.get(5)?,
                        ))
                    },
                )
                .map_err(sql_error)?;

            let mut mapping_stmt = conn
                .prepare(
                    "SELECT id, downloader_id, qb_path, openlist_path
                     FROM openlist_path_mappings ORDER BY downloader_id, qb_path",
                )
                .map_err(sql_error)?;
            let path_mappings = mapping_stmt
                .query_map([], |row| {
                    Ok(OpenListPathMapping {
                        id: Some(row.get(0)?),
                        downloader_id: row.get(1)?,
                        qb_path: row.get(2)?,
                        openlist_path: row.get(3)?,
                    })
                })
                .map_err(sql_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(sql_error)?;

            let mut target_stmt = conn
                .prepare(
                    "SELECT id, name, downloader_id, openlist_path, qb_path
                     FROM openlist_target_directories ORDER BY name, id",
                )
                .map_err(sql_error)?;
            let target_directories = target_stmt
                .query_map([], |row| {
                    Ok(OpenListTargetDirectory {
                        id: Some(row.get(0)?),
                        name: row.get(1)?,
                        downloader_id: row.get(2)?,
                        openlist_path: row.get(3)?,
                        qb_path: row.get(4)?,
                    })
                })
                .map_err(sql_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(sql_error)?;

            let selected_target_index = target_directory_id.and_then(|id| {
                target_directories
                    .iter()
                    .position(|target| target.id == Some(id))
            });
            Ok(OpenListConfig {
                base_url,
                api_key,
                enabled,
                target_directory_id,
                selected_target_index,
                scan_interval_secs,
                updated_at,
                path_mappings,
                target_directories,
            })
        })
        .await
        .map_err(join_error)?
    }

    pub async fn update_openlist_config(
        &self,
        config: &OpenListConfig,
    ) -> Result<OpenListConfig, AppError> {
        let path = self.path.clone();
        let mut config = config.clone();
        let expected_updated_at = config.updated_at.clone();
        config.updated_at = next_openlist_config_updated_at(&expected_updated_at);
        tokio::task::spawn_blocking(move || {
            let mut conn = open_connection(&path)?;
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sql_error)?;
            let (current_base_url, current_api_key, current_updated_at) = tx
                .query_row(
                    "SELECT base_url, api_key, updated_at FROM openlist_settings WHERE id=1",
                    [],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    },
                )
                .map_err(sql_error)?;
            if current_updated_at != expected_updated_at {
                return Err(AppError::InvalidConfig {
                    message: "OpenList settings changed; reload before saving".to_string(),
                });
            }
            let has_active_relocations = tx
                .query_row(
                    "SELECT EXISTS(
                        SELECT 1 FROM media_relocation_jobs
                        WHERE stage NOT IN ('completed', 'cancelled')
                    )",
                    [],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(sql_error)?;
            if (current_base_url != config.base_url || current_api_key != config.api_key)
                && has_active_relocations
            {
                return Err(AppError::InvalidConfig {
                    message: "OpenList address and API key cannot change while relocation jobs are active"
                        .to_string(),
                });
            }
            if has_active_relocations
                && openlist_topology_removes_or_changes_existing(&tx, &config)?
            {
                return Err(AppError::InvalidConfig {
                    message:
                        "OpenList paths cannot change while relocation jobs are active; wait for completion or stop the jobs first"
                            .to_string(),
                });
            }
            tx.execute("DELETE FROM openlist_path_mappings", [])
                .map_err(sql_error)?;
            tx.execute("DELETE FROM openlist_target_directories", [])
                .map_err(sql_error)?;

            for mapping in &mut config.path_mappings {
                tx.execute(
                    "INSERT INTO openlist_path_mappings
                     (id, downloader_id, qb_path, openlist_path, created_at, updated_at)
                     VALUES (?, ?, ?, ?, ?, ?)",
                    params![
                        mapping.id,
                        mapping.downloader_id,
                        mapping.qb_path,
                        mapping.openlist_path,
                        config.updated_at,
                        config.updated_at
                    ],
                )
                .map_err(sql_error)?;
                mapping.id = Some(tx.last_insert_rowid());
            }
            for target in &mut config.target_directories {
                tx.execute(
                    "INSERT INTO openlist_target_directories
                     (id, name, downloader_id, openlist_path, qb_path, created_at, updated_at)
                     VALUES (?, ?, ?, ?, ?, ?, ?)",
                    params![
                        target.id,
                        target.name,
                        target.downloader_id,
                        target.openlist_path,
                        target.qb_path,
                        config.updated_at,
                        config.updated_at
                    ],
                )
                .map_err(sql_error)?;
                target.id = Some(tx.last_insert_rowid());
            }

            let target_directory_id = config
                .selected_target_index
                .and_then(|index| config.target_directories.get(index))
                .and_then(|target| target.id)
                .or_else(|| {
                    config.target_directory_id.filter(|requested| {
                        config
                            .target_directories
                            .iter()
                            .any(|target| target.id == Some(*requested))
                    })
                });
            config.target_directory_id = target_directory_id;
            tx.execute(
                "UPDATE openlist_settings
                 SET base_url = ?, api_key = ?, enabled = ?, target_directory_id = ?,
                     scan_interval_secs = ?, updated_at = ? WHERE id = 1",
                params![
                    config.base_url,
                    config.api_key,
                    config.enabled as i32,
                    config.target_directory_id,
                    config.scan_interval_secs as i64,
                    config.updated_at
                ],
            )
            .map_err(sql_error)?;
            tx.commit().map_err(sql_error)?;
            Ok(config)
        })
        .await
        .map_err(join_error)?
    }

    #[allow(dead_code)]
    pub async fn get_media_relocation_job(
        &self,
        id: i64,
    ) -> Result<Option<MediaRelocationJob>, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.query_row(
                "SELECT id, media_download_id, downloader_id, infohash, source_qb_path,
                        source_openlist_path, target_openlist_path, target_qb_path, torrent_name,
                        stage, openlist_task_id, torrent_data, attempts, next_attempt_at,
                        lease_owner, lease_until, version, last_error, created_at, updated_at,
                        completed_at, source_content_openlist_path, target_content_qb_path,
                        target_downloader_id, copy_items_json, source_files_json,
                        target_root_folder, source_manifest_json, copy_checkpoint_json,
                        copy_lock_acquired, manifest_cursor, manual_requested_at,
                        stage_started_at
                 FROM media_relocation_jobs WHERE id = ?",
                [id],
                map_media_relocation_job,
            )
            .optional()
            .map_err(sql_error)
        })
        .await
        .map_err(join_error)?
    }
}

fn map_media_relocation_job(row: &rusqlite::Row<'_>) -> rusqlite::Result<MediaRelocationJob> {
    Ok(MediaRelocationJob {
        id: row.get(0)?,
        media_download_id: row.get(1)?,
        downloader_id: row.get(2)?,
        infohash: row.get(3)?,
        source_qb_path: row.get(4)?,
        source_openlist_path: row.get(5)?,
        target_openlist_path: row.get(6)?,
        target_qb_path: row.get(7)?,
        torrent_name: row.get(8)?,
        stage: row.get(9)?,
        openlist_task_id: row.get(10)?,
        torrent_data: row.get(11)?,
        attempts: row.get::<_, i64>(12)?.max(0) as u32,
        next_attempt_at: row.get(13)?,
        lease_owner: row.get(14)?,
        lease_until: row.get(15)?,
        version: row.get(16)?,
        last_error: row.get(17)?,
        created_at: row.get(18)?,
        updated_at: row.get(19)?,
        completed_at: row.get(20)?,
        source_content_openlist_path: row.get(21)?,
        target_content_qb_path: row.get(22)?,
        target_downloader_id: row.get(23)?,
        copy_items_json: row.get(24)?,
        source_files_json: row.get(25)?,
        target_root_folder: row.get(26)?,
        source_manifest_json: row.get(27)?,
        copy_checkpoint_json: row.get(28)?,
        copy_lock_acquired: row.get(29)?,
        manifest_cursor: row.get::<_, i64>(30)?.max(0) as usize,
        manual_requested_at: row.get(31)?,
        stage_started_at: row.get(32)?,
    })
}

fn openlist_topology_removes_or_changes_existing(
    conn: &rusqlite::Connection,
    config: &OpenListConfig,
) -> Result<bool, AppError> {
    let mut mapping_statement = conn
        .prepare(
            "SELECT id, downloader_id, qb_path, openlist_path
             FROM openlist_path_mappings",
        )
        .map_err(sql_error)?;
    let current_mappings = mapping_statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(sql_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(sql_error)?;
    drop(mapping_statement);
    if current_mappings
        .iter()
        .any(|(id, downloader_id, qb_path, openlist_path)| {
            !config.path_mappings.iter().any(|mapping| {
                mapping.id == Some(*id)
                    && mapping.downloader_id == *downloader_id
                    && mapping.qb_path == *qb_path
                    && mapping.openlist_path == *openlist_path
            })
        })
    {
        return Ok(true);
    }

    let mut target_statement = conn
        .prepare(
            "SELECT id, downloader_id, openlist_path, qb_path
             FROM openlist_target_directories",
        )
        .map_err(sql_error)?;
    let current_targets = target_statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(sql_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(sql_error)?;
    Ok(current_targets
        .iter()
        .any(|(id, downloader_id, openlist_path, qb_path)| {
            !config.target_directories.iter().any(|target| {
                target.id == Some(*id)
                    && target.downloader_id == *downloader_id
                    && target.openlist_path == *openlist_path
                    && target.qb_path == *qb_path
            })
        }))
}

fn remote_paths_overlap(left: &str, right: &str) -> bool {
    let left = openlist_identity_key(left);
    let right = openlist_identity_key(right);
    left == "/"
        || right == "/"
        || left == right
        || right
            .strip_prefix(left.as_str())
            .is_some_and(|suffix| suffix.starts_with('/'))
        || left
            .strip_prefix(right.as_str())
            .is_some_and(|suffix| suffix.starts_with('/'))
}

#[cfg(test)]
mod tests {
    use super::{
        Database, ManualMediaRelocationTarget, OpenListPathMapping, OpenListTargetDirectory,
        remote_paths_overlap,
    };

    #[test]
    fn remote_paths_overlap_is_unicode_case_insensitive_and_boundary_aware() {
        assert!(remote_paths_overlap(
            "/archive/Ä-show",
            "/ARCHIVE/ä-SHOW/season"
        ));
        assert!(remote_paths_overlap(
            "/archive/Café",
            "/ARCHIVE/Cafe\u{301}/season"
        ));
        assert!(!remote_paths_overlap("/archive/Ä", "/archive/äther"));
    }

    #[tokio::test]
    async fn relocation_schema_includes_safety_path_snapshots() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(dir.path()).await.unwrap();
        assert!(db.list_media_relocation_jobs(10).await.unwrap().is_empty());

        let conn = super::open_connection(&dir.path().join("kirara.db")).unwrap();
        let mut statement = conn
            .prepare("PRAGMA table_info(media_relocation_jobs)")
            .unwrap();
        let columns = statement
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(
            columns
                .iter()
                .any(|name| name == "source_content_openlist_path")
        );
        assert!(columns.iter().any(|name| name == "target_content_qb_path"));
        assert!(columns.iter().any(|name| name == "target_downloader_id"));
        assert!(columns.iter().any(|name| name == "copy_items_json"));
        assert!(columns.iter().any(|name| name == "source_files_json"));
        assert!(columns.iter().any(|name| name == "source_manifest_json"));
        assert!(columns.iter().any(|name| name == "copy_checkpoint_json"));
        assert!(columns.iter().any(|name| name == "copy_lock_acquired"));
        assert!(columns.iter().any(|name| name == "manifest_cursor"));
        assert!(columns.iter().any(|name| name == "target_root_folder"));
        assert!(columns.iter().any(|name| name == "stage_started_at"));
        assert!(columns.iter().any(|name| name == "cleared_at"));
        let media_download_not_null: i64 = conn
            .query_row(
                "SELECT \"notnull\" FROM pragma_table_info('media_relocation_jobs')
                 WHERE name='media_download_id'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(media_download_not_null, 0);

        conn.execute_batch("PRAGMA foreign_keys=OFF;").unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO media_downloads
             (id, target_key, dedupe_key, downloader_id, source_site, downloader_name,
              torrent_id, title, release_json, decision_json, profile_snapshot_json,
              infohash, status, created_at, updated_at, submitted_at)
             VALUES (101, 'episode', 'clear-test', 1, 'site', 'qB', 'torrent',
                     'automatic copy', '{}', '{}', '{}',
                     'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'submitted', ?, ?, ?)",
            rusqlite::params![now, now, now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO media_relocation_jobs
             (media_download_id, infohash, source_qb_path, source_openlist_path,
              target_openlist_path, target_qb_path, torrent_name, stage, created_at, updated_at)
             VALUES (999, '0123456789012345678901234567890123456789', '', '', '', '',
                     'schema test', 'waiting_download', ?, ?)",
            rusqlite::params![now, now],
        )
        .unwrap();
        drop(statement);
        drop(conn);

        let mut claimed = db
            .claim_due_media_relocation_jobs("schema-test", 120, 1, true)
            .await
            .unwrap()
            .pop()
            .unwrap();
        let expected_version = claimed.version;
        let expected_stage = claimed.stage.clone();
        claimed.copy_items_json = "[\"Show\"]".to_string();
        claimed.source_files_json = "[\"Show/E01.mkv\"]".to_string();
        claimed.source_manifest_json = "[{\"path\":\"Show/E01.mkv\",\"size\":123}]".to_string();
        claimed.copy_checkpoint_json =
            Some("{\"path\":\"Show/E01.mkv\",\"size\":123,\"phase\":\"prepared\"}".to_string());
        claimed.copy_lock_acquired = true;
        claimed.manifest_cursor = 1;
        claimed.target_root_folder = Some(true);
        claimed.attempts = 1;
        claimed.last_error = Some("目标 qB 提交失败: test".to_string());
        let next_attempt_at = (chrono::Utc::now() + chrono::Duration::seconds(30)).to_rfc3339();
        claimed.next_attempt_at = Some(next_attempt_at.clone());
        assert!(
            db.update_media_relocation_job(&claimed, expected_version, &expected_stage, None)
                .await
                .unwrap()
        );
        let stored = db
            .get_media_relocation_job(claimed.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.copy_items_json, "[\"Show\"]");
        assert_eq!(stored.source_files_json, "[\"Show/E01.mkv\"]");
        assert_eq!(
            stored.source_manifest_json,
            "[{\"path\":\"Show/E01.mkv\",\"size\":123}]"
        );
        assert!(stored.copy_checkpoint_json.is_some());
        assert!(stored.copy_lock_acquired);
        assert_eq!(stored.manifest_cursor, 1);
        assert_eq!(stored.target_root_folder, Some(true));
        assert_eq!(stored.attempts, 1);
        assert_eq!(stored.last_error.as_deref(), Some("目标 qB 提交失败: test"));
        assert_eq!(
            stored.next_attempt_at.as_deref(),
            Some(next_attempt_at.as_str())
        );
        assert_eq!(
            db.next_media_relocation_attempt_at(true).await.unwrap(),
            Some(next_attempt_at)
        );
    }

    #[tokio::test]
    async fn batch_clear_stops_automatic_jobs_without_touching_manual_transfers() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(dir.path()).await.unwrap();
        let conn = super::open_connection(&dir.path().join("kirara.db")).unwrap();
        conn.execute_batch("PRAGMA foreign_keys=OFF;").unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO media_relocation_jobs
             (media_download_id, infohash, source_qb_path, source_openlist_path,
              target_openlist_path, target_qb_path, torrent_name, stage,
              openlist_task_id, copy_lock_acquired, created_at, updated_at)
             VALUES (101, 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', '/src', '/src',
                     '/target', '/target', 'automatic copy', 'copying', 'remote-task', 1, ?, ?)",
            rusqlite::params![now, now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO media_relocation_jobs
             (media_download_id, infohash, source_qb_path, source_openlist_path,
              target_openlist_path, target_qb_path, torrent_name, stage,
              created_at, updated_at)
             VALUES (NULL, 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb', '/src', '/src',
                     '/target', '/target', 'manual transfer', 'copying', ?, ?)",
            rusqlite::params![now, now],
        )
        .unwrap();
        drop(conn);

        assert_eq!(
            db.stop_and_clear_automatic_media_relocation_jobs()
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            db.stop_and_clear_automatic_media_relocation_jobs()
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            db.enqueue_submitted_media_relocation_jobs(true)
                .await
                .unwrap(),
            0
        );
        let automatic = db
            .list_automatic_media_relocation_jobs(1, 20)
            .await
            .unwrap();
        assert_eq!(automatic.1, 0);
        assert!(automatic.0.is_empty());
        let manual = db.list_manual_media_relocation_jobs(1, 20).await.unwrap();
        assert_eq!(manual.1, 1);
        assert_eq!(manual.0[0].torrent_name, "manual transfer");
        let cleared = db.get_media_relocation_job(1).await.unwrap().unwrap();
        assert_eq!(cleared.stage, "cancelled");
        assert_eq!(cleared.openlist_task_id, None);
        assert!(!cleared.copy_lock_acquired);
        assert!(!db.has_in_flight_openlist_operations().await.unwrap());

        let conn = super::open_connection(&dir.path().join("kirara.db")).unwrap();
        let cleared_at: Option<String> = conn
            .query_row(
                "SELECT cleared_at FROM media_relocation_jobs WHERE id=1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(cleared_at.is_some());
    }

    #[tokio::test]
    async fn automatic_relocation_pagination_reaches_old_priority_jobs_beyond_200() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(dir.path()).await.unwrap();
        let conn = super::open_connection(&dir.path().join("kirara.db")).unwrap();
        conn.execute_batch("PRAGMA foreign_keys=OFF;").unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        let insert = |media_download_id: i64, stage: &str| {
            conn.execute(
                "INSERT INTO media_relocation_jobs
                 (media_download_id, infohash, source_qb_path, source_openlist_path,
                  target_openlist_path, target_qb_path, torrent_name, stage,
                  stage_started_at, created_at, updated_at)
                 VALUES (?, ?, '', '', '', '', ?, ?, ?, ?, ?)",
                rusqlite::params![
                    media_download_id,
                    format!("{media_download_id:040x}"),
                    format!("automatic-{media_download_id}"),
                    stage,
                    now,
                    now,
                    now,
                ],
            )
            .unwrap();
            conn.last_insert_rowid()
        };

        let old_review = insert(1, "copy_manual_review");
        let old_active = insert(2, "copy_reconcile");
        let old_paused = insert(3, "auto_copy_paused");
        for media_download_id in 4..=73 {
            insert(media_download_id, "copy_manual_review");
        }
        for media_download_id in 74..=143 {
            insert(media_download_id, "copy_reconcile");
        }
        for media_download_id in 144..=213 {
            insert(media_download_id, "auto_copy_paused");
        }
        drop(conn);

        let page_size = 50;
        let mut seen = std::collections::HashSet::new();
        for page in 1..=5 {
            let (records, total) = db
                .list_automatic_media_relocation_jobs(page, page_size)
                .await
                .unwrap();
            assert_eq!(total, 213);
            for job in records {
                assert!(
                    seen.insert(job.id),
                    "pagination returned duplicate job {}",
                    job.id
                );
            }
        }
        assert_eq!(seen.len(), 213);

        let (review_page, _) = db
            .list_automatic_media_relocation_jobs(2, page_size)
            .await
            .unwrap();
        let (active_page, _) = db
            .list_automatic_media_relocation_jobs(3, page_size)
            .await
            .unwrap();
        let (paused_page, _) = db
            .list_automatic_media_relocation_jobs(5, page_size)
            .await
            .unwrap();
        assert!(review_page.iter().any(|job| job.id == old_review));
        assert!(active_page.iter().any(|job| job.id == old_active));
        assert!(paused_page.iter().any(|job| job.id == old_paused));
    }

    #[tokio::test]
    async fn relocation_claim_heartbeat_checkpoint_and_target_lock_are_durable() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(dir.path()).await.unwrap();
        let conn = super::open_connection(&dir.path().join("kirara.db")).unwrap();
        conn.execute_batch("PRAGMA foreign_keys=OFF;").unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        for (media_download_id, target) in
            [(901, "/archive/Ä-show"), (902, "/ARCHIVE/ä-SHOW/season")]
        {
            conn.execute(
                "INSERT INTO media_relocation_jobs
                 (media_download_id, infohash, source_qb_path, source_openlist_path,
                  target_openlist_path, target_qb_path, torrent_name, stage,
                  source_files_json, source_manifest_json, created_at, updated_at)
                 VALUES (?, '0123456789012345678901234567890123456789', '/src', '/src',
                         ?, '/target', 'lock test', 'copy_reconcile',
                         '[\"Show/E01.mkv\"]',
                         '[{\"path\":\"Show/E01.mkv\",\"size\":123}]', ?, ?)",
                rusqlite::params![media_download_id, target, now, now],
            )
            .unwrap();
        }
        drop(conn);

        let first = db
            .claim_due_media_relocation_jobs("same-worker", 120, 1, true)
            .await
            .unwrap()
            .pop()
            .unwrap();
        let second_claim = db
            .claim_due_media_relocation_jobs("same-worker", 120, 1, true)
            .await
            .unwrap();
        assert_eq!(
            second_claim.len(),
            1,
            "an old lease must not be selected again"
        );
        let second = &second_claim[0];
        assert_ne!(first.id, second.id);
        assert_ne!(first.lease_owner, second.lease_owner);

        let first_owner = first.lease_owner.as_deref().unwrap();
        assert!(
            db.renew_media_relocation_lease(
                first.id,
                first.version,
                &first.stage,
                first_owner,
                120,
            )
            .await
            .unwrap()
        );
        assert!(
            !db.renew_media_relocation_lease(
                first.id,
                first.version,
                &first.stage,
                "wrong-owner",
                120,
            )
            .await
            .unwrap()
        );

        assert!(
            db.try_acquire_media_relocation_target_lock(
                first.id,
                first.version,
                &first.stage,
                first_owner,
                &first.target_openlist_path,
            )
            .await
            .unwrap()
        );
        assert!(
            !db.try_acquire_media_relocation_target_lock(
                second.id,
                second.version,
                &second.stage,
                second.lease_owner.as_deref().unwrap(),
                &second.target_openlist_path,
            )
            .await
            .unwrap()
        );

        let checkpoint = "{\"path\":\"Show/E01.mkv\",\"size\":123,\"phase\":\"uncertain\",\"submitted_at\":\"2026-01-01T00:00:00Z\"}";
        assert!(
            db.checkpoint_media_relocation_copy_submission(
                first.id,
                first.version,
                &first.stage,
                first_owner,
                checkpoint,
            )
            .await
            .unwrap()
        );
        let mut retry = first.clone();
        retry.attempts = 1;
        retry.next_attempt_at = Some(now.clone());
        retry.last_error = Some("ambiguous response".to_string());
        assert!(
            db.record_media_relocation_retry(&retry, None)
                .await
                .unwrap()
        );
        let stored = db
            .get_media_relocation_job(first.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.copy_checkpoint_json.as_deref(), Some(checkpoint));
        assert!(stored.copy_lock_acquired);
        assert_eq!(stored.last_error.as_deref(), Some("ambiguous response"));

        let conn = super::open_connection(&dir.path().join("kirara.db")).unwrap();
        conn.execute(
            "UPDATE media_relocation_jobs
             SET stage='copy_manual_review', copy_checkpoint_json=NULL,
                 source_manifest_json='[]' WHERE id=?",
            [first.id],
        )
        .unwrap();
        drop(conn);
        let before_force_retry = db
            .get_media_relocation_job(first.id)
            .await
            .unwrap()
            .unwrap();
        assert!(
            db.claim_due_media_relocation_jobs("other-worker", 120, 1, true)
                .await
                .unwrap()
                .is_empty(),
            "manual review jobs must not be claimed"
        );
        for confirm_task_terminated in [false, true] {
            assert!(
                !db.resolve_media_relocation_copy(
                    first.id,
                    "force_retry",
                    before_force_retry.version,
                    confirm_task_terminated,
                )
                .await
                .unwrap(),
                "force retry is never a supported copy resolution because it can duplicate an uncertain remote submission"
            );
        }
        let after_force_retry = db
            .get_media_relocation_job(first.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(after_force_retry.stage, "copy_manual_review");
        assert_eq!(after_force_retry.version, before_force_retry.version);
        assert_eq!(
            after_force_retry.copy_checkpoint_json,
            before_force_retry.copy_checkpoint_json
        );
        assert_eq!(
            after_force_retry.copy_lock_acquired,
            before_force_retry.copy_lock_acquired
        );
        assert_eq!(
            after_force_retry.manifest_cursor,
            before_force_retry.manifest_cursor
        );
        let retried = before_force_retry;

        let conn = super::open_connection(&dir.path().join("kirara.db")).unwrap();
        conn.execute(
            "UPDATE media_relocation_jobs
             SET stage='copying', copy_checkpoint_json=NULL,
                 copy_lock_acquired=1 WHERE id=?",
            [first.id],
        )
        .unwrap();
        drop(conn);
        assert!(
            db.resolve_media_relocation_copy(first.id, "recheck", retried.version, false)
                .await
                .unwrap()
        );
        let legacy_recheck = db
            .get_media_relocation_job(first.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(legacy_recheck.stage, "copy_legacy_reconcile");

        let conn = super::open_connection(&dir.path().join("kirara.db")).unwrap();
        conn.execute(
            "UPDATE media_relocation_jobs
             SET stage='copy_manual_review', copy_checkpoint_json=?, copy_lock_acquired=1
             WHERE id=?",
            rusqlite::params![checkpoint, first.id],
        )
        .unwrap();
        drop(conn);
        assert!(
            db.resolve_media_relocation_copy(first.id, "recheck", legacy_recheck.version, false)
                .await
                .unwrap()
        );
        let rechecking = db
            .get_media_relocation_job(first.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(rechecking.stage, "copy_submitting");
        assert_eq!(rechecking.copy_checkpoint_json.as_deref(), Some(checkpoint));

        let conn = super::open_connection(&dir.path().join("kirara.db")).unwrap();
        conn.execute(
            "UPDATE media_relocation_jobs SET stage='copy_manual_review' WHERE id=?",
            [first.id],
        )
        .unwrap();
        drop(conn);
        assert!(
            !db.resolve_media_relocation_copy(first.id, "cancel", rechecking.version, false)
                .await
                .unwrap()
        );
        assert!(
            db.resolve_media_relocation_copy(first.id, "cancel", rechecking.version, true)
                .await
                .unwrap()
        );
        let cancelled = db
            .get_media_relocation_job(first.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(cancelled.stage, "cancelled");
        assert!(!cancelled.copy_lock_acquired);

        let conn = super::open_connection(&dir.path().join("kirara.db")).unwrap();
        conn.execute(
            "UPDATE media_relocation_jobs
             SET stage='auto_copy_paused', copy_checkpoint_json=NULL,
                 openlist_task_id=NULL, copy_lock_acquired=1 WHERE id=?",
            [first.id],
        )
        .unwrap();
        drop(conn);
        assert!(
            !db.resolve_media_relocation_copy(first.id, "recheck", cancelled.version, false)
                .await
                .unwrap(),
            "paused automatic copies must not be resumed through copy resolution"
        );
        assert!(
            db.resolve_media_relocation_copy(first.id, "cancel", cancelled.version, true)
                .await
                .unwrap(),
            "paused automatic copies have no pending side effect and can be cancelled"
        );
        let paused_cancelled = db
            .get_media_relocation_job(first.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(paused_cancelled.stage, "cancelled");
        assert!(!paused_cancelled.copy_lock_acquired);
    }

    #[tokio::test]
    async fn manifest_required_recheck_is_a_manual_versioned_cas_and_preserves_state() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(dir.path()).await.unwrap();
        let downloader_id = db
            .create_downloader("qb", "qbittorrent", "http://127.0.0.1:8080", "", "")
            .await
            .unwrap();
        db.enqueue_manual_media_relocation_jobs(
            downloader_id,
            downloader_id,
            "/archive",
            "/archive",
            &[(
                "eadb91a4769b1fad89e0dd3a930523e7fc5814b8".to_string(),
                "manifest recovery".to_string(),
            )],
        )
        .await
        .unwrap();
        let (jobs, _) = db.list_manual_media_relocation_jobs(1, 10).await.unwrap();
        let job = jobs.into_iter().next().unwrap();
        let checkpoint = r#"{"path":"episode.mkv","size":10,"operation":"remove_file","phase":"uncertain","submitted_at":"2026-01-01T00:00:00Z"}"#;
        let future_lease = (chrono::Utc::now() + chrono::Duration::minutes(5)).to_rfc3339();
        let conn = super::open_connection(&dir.path().join("kirara.db")).unwrap();
        conn.execute(
            "UPDATE media_relocation_jobs
             SET stage='manifest_required', source_manifest_json='[]',
                 copy_checkpoint_json=?, manifest_cursor=3,
                 lease_owner='old-worker', lease_until=?, last_error='manifest missing'
             WHERE id=?",
            rusqlite::params![checkpoint, future_lease, job.id],
        )
        .unwrap();
        drop(conn);
        let blocked = db.get_media_relocation_job(job.id).await.unwrap().unwrap();
        assert!(
            !db.recheck_media_relocation_manifest(job.id, blocked.version)
                .await
                .unwrap(),
            "an unexpired worker lease must block manual recovery"
        );

        let conn = super::open_connection(&dir.path().join("kirara.db")).unwrap();
        conn.execute(
            "UPDATE media_relocation_jobs SET lease_owner=NULL, lease_until=NULL WHERE id=?",
            [job.id],
        )
        .unwrap();
        drop(conn);
        assert!(
            !db.recheck_media_relocation_manifest(job.id, blocked.version + 1)
                .await
                .unwrap(),
            "a stale client version must not recover the job"
        );
        assert!(
            db.recheck_media_relocation_manifest(job.id, blocked.version)
                .await
                .unwrap()
        );
        let recovered = db.get_media_relocation_job(job.id).await.unwrap().unwrap();
        assert_eq!(recovered.stage, "manifest_recheck");
        assert_eq!(recovered.version, blocked.version + 1);
        assert_eq!(recovered.copy_checkpoint_json.as_deref(), Some(checkpoint));
        assert_eq!(recovered.manifest_cursor, 3);
        assert_eq!(recovered.last_error, None);
        assert!(recovered.next_attempt_at.is_some());
        assert!(
            !db.recheck_media_relocation_manifest(job.id, recovered.version)
                .await
                .unwrap(),
            "only manifest_required may enter the recovery stage"
        );

        let conn = super::open_connection(&dir.path().join("kirara.db")).unwrap();
        conn.execute_batch("PRAGMA foreign_keys=OFF;").unwrap();
        conn.execute(
            "UPDATE media_relocation_jobs
             SET stage='manifest_required', media_download_id=999999 WHERE id=?",
            [job.id],
        )
        .unwrap();
        drop(conn);
        let automatic = db.get_media_relocation_job(job.id).await.unwrap().unwrap();
        assert!(
            !db.recheck_media_relocation_manifest(job.id, automatic.version)
                .await
                .unwrap(),
            "automatic copy jobs must never re-enter qB migration recovery"
        );
    }

    #[tokio::test]
    async fn manual_relocation_jobs_are_idempotent_while_active() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(dir.path()).await.unwrap();
        let downloader_id = db
            .create_downloader("qb", "qbittorrent", "http://127.0.0.1:8080", "", "")
            .await
            .unwrap();
        let torrents = vec![(
            "eadb91a4769b1fad89e0dd3a930523e7fc5814b8".to_string(),
            "manual torrent".to_string(),
        )];

        assert_eq!(
            db.enqueue_manual_media_relocation_jobs(
                downloader_id,
                downloader_id,
                "/archive",
                "/archive",
                &torrents,
            )
            .await
            .unwrap(),
            (1, 0)
        );
        assert_eq!(
            db.enqueue_manual_media_relocation_jobs(
                downloader_id,
                downloader_id,
                "/archive",
                "/archive",
                &torrents,
            )
            .await
            .unwrap(),
            (0, 1)
        );
        let (jobs, total) = db.list_manual_media_relocation_jobs(1, 10).await.unwrap();
        assert_eq!(total, 1);
        assert_eq!(jobs[0].media_download_id, None);
        assert_eq!(jobs[0].torrent_name, "manual torrent");
        assert_eq!(jobs[0].target_openlist_path, "/archive");

        let second = vec![(
            "0123456789012345678901234567890123456789".to_string(),
            "newer manual torrent".to_string(),
        )];
        assert_eq!(
            db.enqueue_manual_media_relocation_jobs(
                downloader_id,
                downloader_id,
                "/archive",
                "/archive",
                &second,
            )
            .await
            .unwrap(),
            (1, 0)
        );
        let (first_page, total) = db.list_manual_media_relocation_jobs(1, 1).await.unwrap();
        let (second_page, _) = db.list_manual_media_relocation_jobs(2, 1).await.unwrap();
        assert_eq!(total, 2);
        assert_eq!(first_page[0].torrent_name, "newer manual torrent");
        assert_eq!(second_page[0].torrent_name, "manual torrent");

        let (past_end, huge_total) = db
            .list_manual_media_relocation_jobs(usize::MAX, 100)
            .await
            .unwrap();
        assert!(past_end.is_empty());
        assert_eq!(huge_total, 2);
    }

    #[tokio::test]
    async fn manual_relocation_targets_are_snapshotted_per_torrent() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(dir.path()).await.unwrap();
        let source_id = db
            .create_downloader("source", "qbittorrent", "http://127.0.0.1:8080", "", "")
            .await
            .unwrap();
        let target_id = db
            .create_downloader("target", "qbittorrent", "http://127.0.0.1:8081", "", "")
            .await
            .unwrap();
        let source = db.get_downloader(source_id).await.unwrap().unwrap();
        let target = db.get_downloader(target_id).await.unwrap().unwrap();
        let config = db.get_openlist_config().await.unwrap();
        let targets = vec![
            ManualMediaRelocationTarget {
                infohash: "0123456789abcdef0123456789abcdef01234567".to_string(),
                torrent_name: "first".to_string(),
                target_openlist_path: "/archive/云母/电影/剧情/2024".to_string(),
                target_qb_path: "/data/archive/云母/电影/剧情/2024".to_string(),
            },
            ManualMediaRelocationTarget {
                infohash: "1123456789abcdef0123456789abcdef01234567".to_string(),
                torrent_name: "second".to_string(),
                target_openlist_path: "/archive/云母/电视剧/科幻/2025".to_string(),
                target_qb_path: "/data/archive/云母/电视剧/科幻/2025".to_string(),
            },
        ];
        assert_eq!(
            db.enqueue_manual_media_relocation_jobs_with_targets_if_config_current(
                source_id,
                target_id,
                &targets,
                &config.updated_at,
                &source.updated_at,
                &target.updated_at,
            )
            .await
            .unwrap(),
            (2, 0)
        );
        let (jobs, total) = db.list_manual_media_relocation_jobs(1, 10).await.unwrap();
        assert_eq!(total, 2);
        let first = jobs
            .iter()
            .find(|job| job.infohash.starts_with("01"))
            .unwrap();
        let second = jobs
            .iter()
            .find(|job| job.infohash.starts_with("11"))
            .unwrap();
        assert_eq!(first.target_openlist_path, targets[0].target_openlist_path);
        assert_eq!(first.target_qb_path, targets[0].target_qb_path);
        assert_eq!(second.target_openlist_path, targets[1].target_openlist_path);
        assert_eq!(second.target_qb_path, targets[1].target_qb_path);
    }

    #[tokio::test]
    async fn manual_relocation_enqueue_yields_to_an_active_download_on_either_qb_endpoint() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(dir.path()).await.unwrap();
        let source_id = db
            .create_downloader("source", "qbittorrent", "http://127.0.0.1:8080", "", "")
            .await
            .unwrap();
        let target_id = db
            .create_downloader("target", "qbittorrent", "http://127.0.0.1:8081", "", "")
            .await
            .unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        let lease_until = (chrono::Utc::now() + chrono::Duration::minutes(5)).to_rfc3339();
        let conn = super::open_connection(&dir.path().join("kirara.db")).unwrap();
        conn.execute(
            "INSERT INTO media_downloads
             (target_key, dedupe_key, downloader_id, source_site, downloader_name,
              torrent_id, title, release_json, decision_json, profile_snapshot_json,
              status, lease_owner, lease_until, created_at, updated_at)
             VALUES ('manual:guard', 'manual-enqueue-guard', ?, 'site', 'target',
                     'torrent', 'Active download', '{}', '{}', '{}', 'fetching',
                     'download-worker', ?, ?, ?)",
            rusqlite::params![target_id, lease_until, now, now],
        )
        .unwrap();
        let download_id = conn.last_insert_rowid();
        drop(conn);
        let selected = vec![(
            "0123456789abcdef0123456789abcdef01234567".to_string(),
            "selected torrent".to_string(),
        )];

        assert_eq!(
            db.enqueue_manual_media_relocation_jobs(
                source_id, target_id, "/archive", "/archive", &selected,
            )
            .await
            .unwrap(),
            (0, 1)
        );
        assert_eq!(
            db.list_manual_media_relocation_jobs(1, 10).await.unwrap().1,
            0
        );

        let conn = super::open_connection(&dir.path().join("kirara.db")).unwrap();
        conn.execute(
            "UPDATE media_downloads
             SET status='cancelled', lease_owner=NULL, lease_until=NULL WHERE id=?",
            [download_id],
        )
        .unwrap();
        drop(conn);
        assert_eq!(
            db.enqueue_manual_media_relocation_jobs(
                source_id, target_id, "/archive", "/archive", &selected,
            )
            .await
            .unwrap(),
            (1, 0)
        );
    }

    #[tokio::test]
    async fn stale_openlist_config_cannot_save_or_enqueue_manual_jobs() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(dir.path()).await.unwrap();
        let downloader_id = db
            .create_downloader("qb", "qbittorrent", "http://127.0.0.1:8080", "", "")
            .await
            .unwrap();
        let downloader = db.get_downloader(downloader_id).await.unwrap().unwrap();

        let stale = db.get_openlist_config().await.unwrap();
        let mut update = stale.clone();
        update.enabled = !update.enabled;
        update.scan_interval_secs += 1;
        let current = db.update_openlist_config(&update).await.unwrap();

        let mut stale_save = stale.clone();
        stale_save.base_url = "https://stale.example".to_string();
        assert!(
            db.update_openlist_config(&stale_save)
                .await
                .unwrap_err()
                .to_string()
                .contains("changed; reload")
        );
        let after_stale_save = db.get_openlist_config().await.unwrap();
        assert_eq!(after_stale_save.updated_at, current.updated_at);
        assert_eq!(after_stale_save.enabled, current.enabled);
        assert_eq!(
            after_stale_save.scan_interval_secs,
            current.scan_interval_secs
        );
        assert_eq!(after_stale_save.base_url, current.base_url);

        let error = db
            .enqueue_manual_media_relocation_jobs_if_config_current(
                downloader_id,
                downloader_id,
                "/archive",
                "/archive",
                &[(
                    "0123456789012345678901234567890123456789".to_string(),
                    "stale request".to_string(),
                )],
                &stale.updated_at,
                &downloader.updated_at,
                &downloader.updated_at,
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("changed; reload"));
        let (jobs, total) = db.list_manual_media_relocation_jobs(1, 10).await.unwrap();
        assert_eq!(total, 0);
        assert!(jobs.is_empty());
        assert_eq!(
            db.get_openlist_config().await.unwrap().updated_at,
            current.updated_at
        );
    }

    #[tokio::test]
    async fn current_downloader_snapshots_enqueue_manual_jobs() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(dir.path()).await.unwrap();
        let source_id = db
            .create_downloader(
                "source-qb",
                "qbittorrent",
                "http://127.0.0.1:8080",
                "source-user",
                "source-password",
            )
            .await
            .unwrap();
        let target_id = db
            .create_downloader(
                "target-qb",
                "qb",
                "http://127.0.0.1:8081",
                "target-user",
                "target-password",
            )
            .await
            .unwrap();
        let source = db.get_downloader(source_id).await.unwrap().unwrap();
        let target = db.get_downloader(target_id).await.unwrap().unwrap();
        let config = db.get_openlist_config().await.unwrap();

        assert_eq!(
            db.enqueue_manual_media_relocation_jobs_if_config_current(
                source_id,
                target_id,
                "/archive",
                "/data/archive",
                &[(
                    "0123456789012345678901234567890123456789".to_string(),
                    "current downloader snapshots".to_string(),
                )],
                &config.updated_at,
                &source.updated_at,
                &target.updated_at,
            )
            .await
            .unwrap(),
            (1, 0)
        );
        let (jobs, total) = db.list_manual_media_relocation_jobs(1, 10).await.unwrap();
        assert_eq!(total, 1);
        assert_eq!(jobs[0].downloader_id, Some(source_id));
        assert_eq!(jobs[0].target_downloader_id, Some(target_id));
    }

    #[tokio::test]
    async fn stale_source_downloader_snapshot_cannot_enqueue_manual_jobs() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(dir.path()).await.unwrap();
        let source_id = db
            .create_downloader("source-qb", "qbittorrent", "http://127.0.0.1:8080", "", "")
            .await
            .unwrap();
        let target_id = db
            .create_downloader("target-qb", "qbittorrent", "http://127.0.0.1:8081", "", "")
            .await
            .unwrap();
        let source = db.get_downloader(source_id).await.unwrap().unwrap();
        let target = db.get_downloader(target_id).await.unwrap().unwrap();
        let config = db.get_openlist_config().await.unwrap();
        let conn = super::open_connection(&dir.path().join("kirara.db")).unwrap();
        conn.execute(
            "UPDATE downloaders SET url=?, updated_at=? WHERE id=?",
            rusqlite::params![
                "http://127.0.0.1:9090",
                "concurrent-source-update",
                source_id
            ],
        )
        .unwrap();
        drop(conn);

        let error = db
            .enqueue_manual_media_relocation_jobs_if_config_current(
                source_id,
                target_id,
                "/archive",
                "/data/archive",
                &[(
                    "1123456789012345678901234567890123456789".to_string(),
                    "stale source".to_string(),
                )],
                &config.updated_at,
                &source.updated_at,
                &target.updated_at,
            )
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Source downloader changed; reload")
        );
        let (jobs, total) = db.list_manual_media_relocation_jobs(1, 10).await.unwrap();
        assert_eq!(total, 0);
        assert!(jobs.is_empty());
    }

    #[tokio::test]
    async fn stale_non_qb_target_downloader_cannot_enqueue_manual_jobs() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(dir.path()).await.unwrap();
        let source_id = db
            .create_downloader("source-qb", "qbittorrent", "http://127.0.0.1:8080", "", "")
            .await
            .unwrap();
        let target_id = db
            .create_downloader("target-qb", "qbittorrent", "http://127.0.0.1:8081", "", "")
            .await
            .unwrap();
        let source = db.get_downloader(source_id).await.unwrap().unwrap();
        let target = db.get_downloader(target_id).await.unwrap().unwrap();
        let config = db.get_openlist_config().await.unwrap();
        let conn = super::open_connection(&dir.path().join("kirara.db")).unwrap();
        conn.execute(
            "UPDATE downloaders SET downloader_type='other', updated_at=? WHERE id=?",
            rusqlite::params!["concurrent-target-type-update", target_id],
        )
        .unwrap();
        drop(conn);

        let error = db
            .enqueue_manual_media_relocation_jobs_if_config_current(
                source_id,
                target_id,
                "/archive",
                "/data/archive",
                &[(
                    "2123456789012345678901234567890123456789".to_string(),
                    "stale target type".to_string(),
                )],
                &config.updated_at,
                &source.updated_at,
                &target.updated_at,
            )
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Target downloader changed; reload")
        );
        let (jobs, total) = db.list_manual_media_relocation_jobs(1, 10).await.unwrap();
        assert_eq!(total, 0);
        assert!(jobs.is_empty());
    }

    #[tokio::test]
    async fn deleted_source_downloader_cannot_enqueue_manual_jobs() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(dir.path()).await.unwrap();
        let source_id = db
            .create_downloader("source-qb", "qbittorrent", "http://127.0.0.1:8080", "", "")
            .await
            .unwrap();
        let target_id = db
            .create_downloader("target-qb", "qbittorrent", "http://127.0.0.1:8081", "", "")
            .await
            .unwrap();
        let source = db.get_downloader(source_id).await.unwrap().unwrap();
        let target = db.get_downloader(target_id).await.unwrap().unwrap();
        let config = db.get_openlist_config().await.unwrap();
        db.delete_downloader(source_id).await.unwrap();

        let error = db
            .enqueue_manual_media_relocation_jobs_if_config_current(
                source_id,
                target_id,
                "/archive",
                "/data/archive",
                &[(
                    "3123456789012345678901234567890123456789".to_string(),
                    "deleted source".to_string(),
                )],
                &config.updated_at,
                &source.updated_at,
                &target.updated_at,
            )
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Source downloader changed; reload")
        );
        let (jobs, total) = db.list_manual_media_relocation_jobs(1, 10).await.unwrap();
        assert_eq!(total, 0);
        assert!(jobs.is_empty());
    }

    #[tokio::test]
    async fn active_relocations_protect_existing_topology_but_allow_additive_edits() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(dir.path()).await.unwrap();
        let downloader_id = db
            .create_downloader("qb", "qbittorrent", "http://127.0.0.1:8080", "", "")
            .await
            .unwrap();

        let mut initial = db.get_openlist_config().await.unwrap();
        initial.path_mappings = vec![OpenListPathMapping {
            id: None,
            downloader_id,
            qb_path: "/downloads/Media".to_string(),
            openlist_path: "/incoming/Ä-media".to_string(),
        }];
        initial.target_directories = vec![OpenListTargetDirectory {
            id: None,
            name: "archive".to_string(),
            downloader_id,
            openlist_path: "/archive/Ä-media".to_string(),
            qb_path: "/data/Archive".to_string(),
        }];
        initial.selected_target_index = Some(0);
        let configured = db.update_openlist_config(&initial).await.unwrap();
        let downloader = db.get_downloader(downloader_id).await.unwrap().unwrap();
        db.enqueue_manual_media_relocation_jobs_if_config_current(
            downloader_id,
            downloader_id,
            "/archive/Ä-media",
            "/data/Archive",
            &[(
                "0123456789012345678901234567890123456789".to_string(),
                "active relocation".to_string(),
            )],
            &configured.updated_at,
            &downloader.updated_at,
            &downloader.updated_at,
        )
        .await
        .unwrap();

        let mut removed_mapping = configured.clone();
        removed_mapping.path_mappings.clear();
        assert!(
            db.update_openlist_config(&removed_mapping)
                .await
                .unwrap_err()
                .to_string()
                .contains("paths cannot change")
        );

        let mut changed_mapping = configured.clone();
        changed_mapping.path_mappings[0].openlist_path = "/incoming/other".to_string();
        assert!(
            db.update_openlist_config(&changed_mapping)
                .await
                .unwrap_err()
                .to_string()
                .contains("paths cannot change")
        );

        let mut qb_case_change = configured.clone();
        qb_case_change.path_mappings[0].qb_path = "/DOWNLOADS/MEDIA".to_string();
        assert!(
            db.update_openlist_config(&qb_case_change)
                .await
                .unwrap_err()
                .to_string()
                .contains("paths cannot change"),
            "qB paths remain case-sensitive"
        );

        let mut removed_target = configured.clone();
        removed_target.target_directories.clear();
        removed_target.selected_target_index = None;
        assert!(
            db.update_openlist_config(&removed_target)
                .await
                .unwrap_err()
                .to_string()
                .contains("paths cannot change")
        );

        let mut changed_target = configured.clone();
        changed_target.target_directories[0].openlist_path = "/archive/other".to_string();
        assert!(
            db.update_openlist_config(&changed_target)
                .await
                .unwrap_err()
                .to_string()
                .contains("paths cannot change")
        );

        let mut target_qb_case_change = configured.clone();
        target_qb_case_change.target_directories[0].qb_path = "/DATA/ARCHIVE".to_string();
        assert!(
            db.update_openlist_config(&target_qb_case_change)
                .await
                .unwrap_err()
                .to_string()
                .contains("paths cannot change"),
            "target qB paths remain case-sensitive"
        );

        let mut additive = configured.clone();
        additive.enabled = !additive.enabled;
        additive.target_directories[0].name = "renamed archive".to_string();
        additive.target_directories.push(OpenListTargetDirectory {
            id: None,
            name: "second archive".to_string(),
            downloader_id,
            openlist_path: "/archive/new".to_string(),
            qb_path: "/data/new".to_string(),
        });
        additive.selected_target_index = Some(1);
        let saved = db.update_openlist_config(&additive).await.unwrap();
        assert_eq!(saved.enabled, additive.enabled);
        assert_eq!(saved.target_directories.len(), 2);
        assert_eq!(saved.target_directories[0].name, "renamed archive");
        assert_eq!(saved.target_directory_id, saved.target_directories[1].id);
        assert_eq!(saved.path_mappings[0].qb_path, "/downloads/Media");
        assert_eq!(saved.path_mappings[0].openlist_path, "/incoming/Ä-media");
    }

    #[tokio::test]
    async fn disabled_processing_enqueues_new_automatic_jobs_as_paused() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(dir.path()).await.unwrap();
        let downloader_id = db
            .create_downloader("qb", "qbittorrent", "http://127.0.0.1:8080", "", "")
            .await
            .unwrap();
        let conn = super::open_connection(&dir.path().join("kirara.db")).unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO media_downloads
             (target_key, dedupe_key, downloader_id, source_site, downloader_name,
              torrent_id, title, release_json, decision_json, profile_snapshot_json,
              infohash, status, created_at, updated_at)
             VALUES ('target', 'paused-auto', ?, 'site', 'qb', 'torrent',
                     'paused automatic copy', '{}', '{}', '{}',
                     '0123456789012345678901234567890123456789', 'submitted', ?, ?)",
            rusqlite::params![downloader_id, now, now],
        )
        .unwrap();
        drop(conn);

        assert_eq!(
            db.enqueue_submitted_media_relocation_jobs(false)
                .await
                .unwrap(),
            1
        );
        let jobs = db.list_media_relocation_jobs(10).await.unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].stage, "auto_copy_paused");
        assert_eq!(jobs[0].next_attempt_at, None);
        assert_eq!(jobs[0].lease_owner, None);
        assert_eq!(jobs[0].lease_until, None);
        assert!(
            db.claim_due_media_relocation_jobs("disabled-test", 120, 10, false)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn terminal_manual_job_does_not_block_independent_automatic_copy() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(dir.path()).await.unwrap();
        let downloader_id = db
            .create_downloader("qb", "qbittorrent", "http://127.0.0.1:8080", "", "")
            .await
            .unwrap();
        let hash = "abcdefabcdefabcdefabcdefabcdefabcdefabcd";
        assert_eq!(
            db.enqueue_manual_media_relocation_jobs(
                downloader_id,
                downloader_id,
                "/archive",
                "/archive",
                &[(hash.to_string(), "terminal manual".to_string())],
            )
            .await
            .unwrap(),
            (1, 0)
        );
        let conn = super::open_connection(&dir.path().join("kirara.db")).unwrap();
        conn.execute(
            "UPDATE media_relocation_jobs SET stage='cancelled' WHERE media_download_id IS NULL",
            [],
        )
        .unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO media_downloads
             (target_key, dedupe_key, downloader_id, source_site, downloader_name,
              torrent_id, title, release_json, decision_json, profile_snapshot_json,
              infohash, status, created_at, updated_at)
             VALUES ('target', 'auto-after-manual', ?, 'site', 'qb', 'torrent',
                     'independent automatic copy', '{}', '{}', '{}', ?, 'submitted', ?, ?)",
            rusqlite::params![downloader_id, hash, now, now],
        )
        .unwrap();
        drop(conn);

        assert_eq!(
            db.enqueue_submitted_media_relocation_jobs(true)
                .await
                .unwrap(),
            1
        );
        let automatic = db.list_media_relocation_jobs(10).await.unwrap();
        assert_eq!(automatic.len(), 1);
        assert_eq!(automatic[0].stage, "waiting_download");
        assert_eq!(automatic[0].torrent_name, "independent automatic copy");
    }

    #[tokio::test]
    async fn active_manual_target_downloader_blocks_duplicate_automatic_copy() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(dir.path()).await.unwrap();
        let source_id = db
            .create_downloader(
                "manual-source",
                "qbittorrent",
                "http://127.0.0.1:8080",
                "",
                "",
            )
            .await
            .unwrap();
        let target_id = db
            .create_downloader(
                "automatic-source",
                "qbittorrent",
                "http://127.0.0.1:8081",
                "",
                "",
            )
            .await
            .unwrap();
        let hash = "0123456789abcdef0123456789abcdef01234567";
        let now = chrono::Utc::now().to_rfc3339();
        let conn = super::open_connection(&dir.path().join("kirara.db")).unwrap();
        conn.execute(
            "INSERT INTO media_relocation_jobs
             (media_download_id, downloader_id, infohash, source_qb_path,
              source_openlist_path, target_openlist_path, target_qb_path,
              target_downloader_id, torrent_name, stage, created_at, updated_at)
             VALUES (NULL, ?, ?, '/source', '/source', '/target', '/target',
                     ?, 'manual copy', 'copying', ?, ?)",
            rusqlite::params![source_id, hash, target_id, now, now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO media_downloads
             (target_key, dedupe_key, downloader_id, source_site, downloader_name,
              torrent_id, title, release_json, decision_json, profile_snapshot_json,
              infohash, status, created_at, updated_at)
             VALUES ('target', 'automatic-copy', ?, 'site', 'qB', 'torrent',
                     'automatic copy', '{}', '{}', '{}', ?, 'submitted', ?, ?)",
            rusqlite::params![target_id, hash, now, now],
        )
        .unwrap();
        drop(conn);

        assert_eq!(
            db.enqueue_submitted_media_relocation_jobs(true)
                .await
                .unwrap(),
            0
        );
        assert!(db.list_media_relocation_jobs(10).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn migration_retry_resumes_after_the_verified_copy_boundary() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(dir.path()).await.unwrap();
        let downloader_id = db
            .create_downloader("qb", "qbittorrent", "http://127.0.0.1:8080", "", "")
            .await
            .unwrap();
        let conn = super::open_connection(&dir.path().join("kirara.db")).unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        let insert_job = |infohash: &str,
                          source_openlist_path: &str,
                          torrent_data: Option<Vec<u8>>,
                          stage: &str,
                          name: &str| {
            conn.execute(
                "INSERT INTO media_relocation_jobs
                 (downloader_id, infohash, source_qb_path, source_openlist_path,
                  target_openlist_path, target_qb_path, target_downloader_id,
                  torrent_name, stage, torrent_data, manual_requested_at,
                  copy_items_json, source_files_json, source_manifest_json,
                  manifest_cursor, openlist_task_id, copy_checkpoint_json,
                  copy_lock_acquired, attempts, last_error,
                  stage_started_at, created_at, updated_at)
                 VALUES (?, ?, '/qb/source', ?, '/archive', '/qb/archive', ?, ?, ?, ?, ?,
                         '[\"Show\"]', '[\"Show/E01.mkv\"]',
                         '[{\"path\":\"Show/E01.mkv\",\"size\":123}]',
                         7, 'remote-task', '{\"phase\":\"uncertain\"}', 1, 8,
                         'qB needs manual review', ?, ?, ?)",
                rusqlite::params![
                    downloader_id,
                    infohash,
                    source_openlist_path,
                    downloader_id,
                    name,
                    stage,
                    torrent_data,
                    now,
                    now,
                    now,
                    now
                ],
            )
            .unwrap();
            conn.last_insert_rowid()
        };
        let missing_source = insert_job(
            "1111111111111111111111111111111111111111",
            "",
            None,
            "qb_manual_review",
            "missing source",
        );
        let needs_export = insert_job(
            "2222222222222222222222222222222222222222",
            "/source/show",
            None,
            "qb_manual_review",
            "needs export",
        );
        let has_export = insert_job(
            "3333333333333333333333333333333333333333",
            "/source/show",
            Some(vec![1, 2, 3]),
            "qb_manual_review",
            "has export",
        );
        let abandon_qb_review = insert_job(
            "6666666666666666666666666666666666666666",
            "/source/show",
            Some(vec![4, 5, 6]),
            "qb_manual_review",
            "abandon qB review",
        );
        let already_verified = insert_job(
            "4444444444444444444444444444444444444444",
            "/source/show",
            None,
            "copy_verified",
            "already verified",
        );
        drop(conn);

        let abandon_before = db
            .get_media_relocation_job(abandon_qb_review)
            .await
            .unwrap()
            .unwrap();
        assert!(
            db.abandon_media_relocation_migration(abandon_qb_review, abandon_before.version)
                .await
                .unwrap()
        );
        assert_eq!(
            db.get_media_relocation_job(abandon_qb_review)
                .await
                .unwrap()
                .unwrap()
                .stage,
            "cancelled"
        );

        for (id, expected_stage) in [
            (missing_source, "waiting_download"),
            (needs_export, "copy_verified"),
            (has_export, "qb_reconcile"),
        ] {
            let before = db.get_media_relocation_job(id).await.unwrap().unwrap();
            assert!(
                db.retry_media_relocation_migration(id, before.version)
                    .await
                    .unwrap()
            );
            let after = db.get_media_relocation_job(id).await.unwrap().unwrap();
            assert_eq!(after.stage, expected_stage);
            assert_eq!(after.version, before.version + 1);
            assert_eq!(after.copy_items_json, before.copy_items_json);
            assert_eq!(after.source_files_json, before.source_files_json);
            assert_eq!(after.source_manifest_json, before.source_manifest_json);
            assert_eq!(after.manifest_cursor, before.manifest_cursor);
            assert_eq!(after.source_openlist_path, before.source_openlist_path);
            assert_eq!(after.target_openlist_path, before.target_openlist_path);
            assert_eq!(after.target_qb_path, before.target_qb_path);
            assert_eq!(after.openlist_task_id, None);
            assert_eq!(after.copy_checkpoint_json, None);
            assert!(!after.copy_lock_acquired);
        }

        let verified_before = db
            .get_media_relocation_job(already_verified)
            .await
            .unwrap()
            .unwrap();
        assert!(
            !db.retry_media_relocation_migration(already_verified, verified_before.version)
                .await
                .unwrap(),
            "retry must never move an already verified copy back into OpenList work"
        );
        let verified_after = db
            .get_media_relocation_job(already_verified)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(verified_after.stage, "copy_verified");
        assert_eq!(verified_after.version, verified_before.version);
        assert_eq!(
            verified_after.copy_checkpoint_json,
            verified_before.copy_checkpoint_json
        );
        assert_eq!(
            verified_after.source_manifest_json,
            verified_before.source_manifest_json
        );
    }

    #[tokio::test]
    async fn source_remove_manual_review_is_only_resumed_with_its_checkpoint() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(dir.path()).await.unwrap();
        let downloader_id = db
            .create_downloader("qb", "qbittorrent", "http://127.0.0.1:8080", "", "")
            .await
            .unwrap();
        db.enqueue_manual_media_relocation_jobs(
            downloader_id,
            downloader_id,
            "/archive",
            "/qb/archive",
            &[(
                "5555555555555555555555555555555555555555".to_string(),
                "uncertain source removal".to_string(),
            )],
        )
        .await
        .unwrap();
        let (jobs, _) = db.list_manual_media_relocation_jobs(1, 10).await.unwrap();
        let id = jobs[0].id;
        let conn = super::open_connection(&dir.path().join("kirara.db")).unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE media_relocation_jobs
             SET stage='source_remove_manual_review',
                 source_openlist_path='/source',
                 copy_checkpoint_json='opaque-remove-checkpoint',
                 manifest_cursor=5, openlist_task_id='remove-task',
                 copy_lock_acquired=1, next_attempt_at=?,
                 last_error='remove response was uncertain'
             WHERE id=?",
            rusqlite::params![now, id],
        )
        .unwrap();
        drop(conn);

        let review = db.get_media_relocation_job(id).await.unwrap().unwrap();
        assert_eq!(review.stage, "source_remove_manual_review");
        assert!(
            db.claim_due_media_relocation_jobs("remove-review", 120, 1, true)
                .await
                .unwrap()
                .is_empty(),
            "an uncertain source removal must wait for explicit review"
        );
        assert_eq!(
            db.next_media_relocation_attempt_at(true).await.unwrap(),
            None
        );

        assert!(
            db.retry_media_relocation_migration(id, review.version)
                .await
                .unwrap()
        );
        let resumed = db.get_media_relocation_job(id).await.unwrap().unwrap();
        assert_eq!(resumed.stage, "source_removing");
        assert_eq!(
            resumed.copy_checkpoint_json.as_deref(),
            Some("opaque-remove-checkpoint")
        );
        assert_eq!(resumed.manifest_cursor, 5);
        assert_eq!(resumed.openlist_task_id, None);
        assert!(!resumed.copy_lock_acquired);
        assert_eq!(resumed.last_error, None);

        let conn = super::open_connection(&dir.path().join("kirara.db")).unwrap();
        conn.execute(
            "UPDATE media_relocation_jobs
             SET stage='source_remove_manual_review', next_attempt_at=NULL WHERE id=?",
            [id],
        )
        .unwrap();
        drop(conn);
        assert!(
            db.abandon_media_relocation_migration(id, resumed.version)
                .await
                .unwrap(),
            "source removal review must support explicit abandonment"
        );
        let abandoned = db.get_media_relocation_job(id).await.unwrap().unwrap();
        assert_eq!(abandoned.stage, "cancelled");
        assert_eq!(
            abandoned.copy_checkpoint_json.as_deref(),
            Some("opaque-remove-checkpoint")
        );
        assert_eq!(abandoned.manifest_cursor, 5);
        assert!(!abandoned.copy_lock_acquired);
    }

    #[tokio::test]
    async fn active_relocations_protect_credentials_connection_and_version() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(dir.path()).await.unwrap();
        let mut initial = db.get_openlist_config().await.unwrap();
        initial.base_url = "https://openlist.example".to_string();
        initial.api_key = "old-key".to_string();
        let configured = db.update_openlist_config(&initial).await.unwrap();
        let stale = configured.clone();

        let downloader_id = db
            .create_downloader("qb", "qbittorrent", "http://127.0.0.1:8080", "", "")
            .await
            .unwrap();
        db.enqueue_manual_media_relocation_jobs(
            downloader_id,
            downloader_id,
            "/archive",
            "/archive",
            &[(
                "0123456789012345678901234567890123456789".to_string(),
                "active relocation".to_string(),
            )],
        )
        .await
        .unwrap();

        let mut rotation = configured.clone();
        rotation.api_key = "new-key".to_string();
        let rotation_error = db.update_openlist_config(&rotation).await.unwrap_err();
        assert!(
            rotation_error
                .to_string()
                .contains("address and API key cannot change")
        );

        let mut allowed_update = configured.clone();
        allowed_update.scan_interval_secs += 60;
        let updated = db.update_openlist_config(&allowed_update).await.unwrap();

        let stale_error = db.update_openlist_config(&stale).await.unwrap_err();
        assert!(stale_error.to_string().contains("changed; reload"));

        let mut clear_key = updated.clone();
        clear_key.api_key.clear();
        let clear_error = db.update_openlist_config(&clear_key).await.unwrap_err();
        assert!(
            clear_error
                .to_string()
                .contains("address and API key cannot change")
        );

        let mut change_address = updated;
        change_address.base_url = "https://other.example".to_string();
        let address_error = db
            .update_openlist_config(&change_address)
            .await
            .unwrap_err();
        assert!(
            address_error
                .to_string()
                .contains("address and API key cannot change")
        );
    }

    #[tokio::test]
    async fn manual_request_does_not_convert_an_active_automatic_job() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(dir.path()).await.unwrap();
        let downloader_id = db
            .create_downloader("qb", "qbittorrent", "http://127.0.0.1:8080", "", "")
            .await
            .unwrap();
        let hash = "483dcc0e0b7fd8ff3b136f496fd1ae580b421fe0";
        let conn = super::open_connection(&dir.path().join("kirara.db")).unwrap();
        conn.execute_batch("PRAGMA foreign_keys=OFF;").unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO media_relocation_jobs
             (media_download_id, downloader_id, infohash, source_qb_path,
              source_openlist_path, target_openlist_path, target_qb_path,
              torrent_name, stage, created_at, updated_at)
             VALUES (999, ?, ?, '', '', '', '', 'automatic torrent',
                     'waiting_download', ?, ?)",
            rusqlite::params![downloader_id, hash, now, now],
        )
        .unwrap();
        drop(conn);

        assert_eq!(
            db.enqueue_manual_media_relocation_jobs(
                downloader_id,
                downloader_id,
                "/archive",
                "/archive",
                &[(hash.to_string(), "selected torrent".to_string())],
            )
            .await
            .unwrap(),
            (0, 1)
        );
        let (jobs, total) = db.list_manual_media_relocation_jobs(1, 20).await.unwrap();
        assert_eq!(total, 0);
        assert!(jobs.is_empty());
        let automatic = db.list_media_relocation_jobs(20).await.unwrap();
        assert_eq!(automatic.len(), 1);
        assert_eq!(automatic[0].torrent_name, "automatic torrent");
        assert!(automatic[0].manual_requested_at.is_none());
    }

    #[tokio::test]
    async fn planning_manual_review_is_not_claimed_and_only_allows_safe_resolution() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(dir.path()).await.unwrap();
        let downloader_id = db
            .create_downloader("qb", "qbittorrent", "http://127.0.0.1:8080", "", "")
            .await
            .unwrap();
        let conn = super::open_connection(&dir.path().join("kirara.db")).unwrap();
        conn.execute_batch("PRAGMA foreign_keys=OFF;").unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO media_relocation_jobs
             (media_download_id, downloader_id, infohash, source_qb_path,
              source_openlist_path, target_openlist_path, target_qb_path,
              torrent_name, stage, stage_started_at, created_at, updated_at)
             VALUES (999, ?, '0123456789012345678901234567890123456789',
                     '', '', '', '', 'planning failure', 'waiting_download', ?, ?, ?)",
            rusqlite::params![downloader_id, now, now, now],
        )
        .unwrap();
        drop(conn);

        let mut claimed = db
            .claim_due_media_relocation_jobs("planning-test", 120, 1, true)
            .await
            .unwrap()
            .pop()
            .unwrap();
        claimed.attempts = 8;
        claimed.last_error = Some("failed to inspect source qB".to_string());
        assert!(
            db.record_media_relocation_retry(&claimed, Some("planning_manual_review"))
                .await
                .unwrap()
        );
        let review = db
            .get_media_relocation_job(claimed.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(review.stage, "planning_manual_review");
        assert_eq!(review.copy_checkpoint_json, None);
        assert_eq!(review.openlist_task_id, None);
        assert!(!review.copy_lock_acquired);
        assert_eq!(review.next_attempt_at, None);
        assert!(
            db.claim_due_media_relocation_jobs("planning-test", 120, 1, true)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            db.next_media_relocation_attempt_at(true).await.unwrap(),
            None
        );
        assert!(
            !db.resolve_media_relocation_copy(review.id, "force_retry", review.version, true)
                .await
                .unwrap(),
            "planning failures must never enter the copy retry path"
        );
        assert!(
            db.resolve_media_relocation_copy(review.id, "recheck", review.version, false)
                .await
                .unwrap()
        );
        let waiting = db
            .get_media_relocation_job(review.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(waiting.stage, "waiting_download");
        assert_eq!(waiting.attempts, 0);
        assert_eq!(waiting.last_error, None);

        let conn = super::open_connection(&dir.path().join("kirara.db")).unwrap();
        conn.execute(
            "UPDATE media_relocation_jobs
             SET stage='planning_manual_review', next_attempt_at=NULL WHERE id=?",
            [waiting.id],
        )
        .unwrap();
        drop(conn);
        assert!(
            db.resolve_media_relocation_copy(waiting.id, "cancel", waiting.version, false)
                .await
                .unwrap(),
            "planning cancellation has no remote side effect to confirm"
        );
        let cancelled = db
            .get_media_relocation_job(waiting.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(cancelled.stage, "cancelled");

        let conn = super::open_connection(&dir.path().join("kirara.db")).unwrap();
        conn.execute(
            "UPDATE media_relocation_jobs
             SET stage='planning_manual_review',
                 copy_checkpoint_json='{}' WHERE id=?",
            [cancelled.id],
        )
        .unwrap();
        drop(conn);
        assert!(
            !db.resolve_media_relocation_copy(cancelled.id, "recheck", cancelled.version, false)
                .await
                .unwrap(),
            "a corrupted planning state with a checkpoint must fail closed"
        );
    }

    #[tokio::test]
    async fn active_relocations_protect_source_and_target_downloader_identity() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(dir.path()).await.unwrap();
        let source_id = db
            .create_downloader(
                "source",
                "qbittorrent",
                "http://127.0.0.1:8080",
                "source-user",
                "source-password",
            )
            .await
            .unwrap();
        let target_id = db
            .create_downloader(
                "target",
                "qbittorrent",
                "http://127.0.0.1:8081",
                "target-user",
                "target-password",
            )
            .await
            .unwrap();
        db.enqueue_manual_media_relocation_jobs(
            source_id,
            target_id,
            "/archive",
            "/archive",
            &[(
                "0123456789012345678901234567890123456789".to_string(),
                "active transfer".to_string(),
            )],
        )
        .await
        .unwrap();

        assert!(
            db.has_active_relocation_for_downloader(source_id)
                .await
                .unwrap()
        );
        assert!(
            db.has_active_relocation_for_downloader(target_id)
                .await
                .unwrap()
        );
        db.update_downloader(
            source_id,
            "renamed source",
            "qb",
            "http://127.0.0.1:8080/",
            "rotated-user",
            "rotated-password",
        )
        .await
        .unwrap();

        let url_change = db
            .update_downloader(
                source_id,
                "renamed source",
                "qbittorrent",
                "http://127.0.0.1:9090",
                "rotated-user",
                "rotated-password",
            )
            .await
            .unwrap_err();
        assert!(
            url_change
                .to_string()
                .contains("while relocation jobs are active")
        );
        let type_change = db
            .update_downloader(
                target_id,
                "target",
                "other-client",
                "http://127.0.0.1:8081",
                "target-user",
                "target-password",
            )
            .await
            .unwrap_err();
        assert!(
            type_change
                .to_string()
                .contains("while relocation jobs are active")
        );
        let credential_clear = db
            .update_downloader(
                target_id,
                "target",
                "qbittorrent",
                "http://127.0.0.1:8081",
                "",
                "",
            )
            .await
            .unwrap_err();
        assert!(
            credential_clear
                .to_string()
                .contains("while relocation jobs are active")
        );
        assert!(db.delete_downloader(source_id).await.is_err());
        assert!(db.delete_downloader(target_id).await.is_err());
        assert_eq!(
            db.get_downloader(source_id).await.unwrap().unwrap().url,
            "http://127.0.0.1:8080/"
        );
        assert_eq!(
            db.get_downloader(target_id)
                .await
                .unwrap()
                .unwrap()
                .downloader_type,
            "qbittorrent"
        );

        let (jobs, _) = db.list_manual_media_relocation_jobs(1, 10).await.unwrap();
        let conn = super::open_connection(&dir.path().join("kirara.db")).unwrap();
        conn.execute(
            "UPDATE media_relocation_jobs SET stage='cancelled' WHERE id=?",
            [jobs[0].id],
        )
        .unwrap();
        drop(conn);
        assert!(
            !db.has_active_relocation_for_downloader(source_id)
                .await
                .unwrap()
        );
        db.update_downloader(
            source_id,
            "source after cancellation",
            "qbittorrent",
            "http://127.0.0.1:9090",
            "",
            "",
        )
        .await
        .unwrap();
        db.delete_downloader(target_id).await.unwrap();
        assert!(db.get_downloader(target_id).await.unwrap().is_none());
    }
}
