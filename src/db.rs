use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use crate::brush::{BrushTaskRecord, BrushTaskRequest, BrushTorrentRecord};
use crate::config::GlobalConfig;
use crate::downloader::DownloaderRecord;
use crate::error::AppError;
use crate::sign_in::{SignInRecord, SignInResult, SignInTaskRecord, SignInTaskRequest};
use crate::site::{
    SiteRecord, SiteStatsHistoryRecord, SiteStatsRecord, SiteWithStats, UserStats,
    default_site_request_headers,
};
use crate::stats::{DownloaderSpeedSnapshot, TaskStatsSnapshot};

mod media;
mod openlist;
mod ptd_backup;
pub mod rss;
pub mod search;
mod webdav;

pub use openlist::{
    ManualMediaRelocationTarget, MediaRelocationJob, OpenListConfig, OpenListPathMapping,
    OpenListTargetDirectory,
};
pub use ptd_backup::PtdBackupConfig;

#[derive(Clone)]
pub struct Database {
    path: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
pub struct PaginatedBrushTorrents {
    pub page: usize,
    pub page_size: usize,
    pub total_records: usize,
    pub records: Vec<BrushTorrentRecord>,
}

/// 一次下载器采集快照的统计写入批次（在单事务中提交）。
pub struct SnapshotStatsBatch {
    pub downloader_id: i64,
    pub upload_speed: i64,
    pub download_speed: i64,
    pub tasks: Vec<TaskSnapshotStats>,
    pub torrents: Vec<TorrentSnapshotStats>,
}

pub struct TaskSnapshotStats {
    pub task_id: i64,
    pub total_uploaded: i64,
    pub total_downloaded: i64,
    pub torrent_count: i64,
}

pub struct TorrentSnapshotStats {
    pub task_id: i64,
    pub hash: String,
    pub uploaded: i64,
    pub downloaded: i64,
    pub download_duration_secs: i64,
    pub avg_upload_speed: f64,
    pub ratio: f64,
}

impl Database {
    pub async fn open(data_dir: &Path) -> Result<Self, AppError> {
        tokio::fs::create_dir_all(&data_dir)
            .await
            .map_err(|source| AppError::CreateDir {
                path: data_dir.display().to_string(),
                source,
            })?;
        let current_path = data_dir.join("kirara.db");
        let legacy_path = data_dir.join("rflush.db");
        let exists = |path: &Path| {
            path.try_exists().map_err(|error| AppError::Database {
                message: format!("cannot inspect {}: {error}", path.display()),
            })
        };
        // Keep existing SQLite files (including WAL/SHM companions) in place.
        // Never create a blank database just because the application was renamed.
        let path = if exists(&current_path)? || !exists(&legacy_path)? {
            current_path
        } else {
            legacy_path
        };
        let db = Self { path };
        db.init().await?;
        db.init_webdav().await?;
        db.init_rss().await?;
        Ok(db)
    }

    pub async fn get_settings(&self) -> Result<GlobalConfig, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || -> Result<GlobalConfig, AppError> {
            let conn = open_connection(&path)?;
            let settings = conn
                .query_row(
                    "SELECT log_level, proxy, use_proxy_for_lightpanda, tag_rule_scan_interval_mins,
                            ocr_api_key, lightpanda_endpoint, lightpanda_token, lightpanda_region,
                            lightpanda_browser, lightpanda_proxy, lightpanda_country,
                            browserless_address, browserless_token
                     FROM global_settings WHERE id = 1",
                    [],
                    |row| {
                        Ok(GlobalConfig {
                            log_level: row.get(0)?,
                            proxy: row.get(1)?,
                            use_proxy_for_lightpanda: row.get::<_, i32>(2).unwrap_or(1) != 0,
                            lightpanda: crate::config::LightpandaConfig {
                                endpoint: row.get(5)?,
                                token: row.get(6)?,
                                region: row.get(7)?,
                                browser: row.get(8)?,
                                proxy: row.get(9)?,
                                country: row.get(10)?,
                            },
                            browserless: crate::config::BrowserlessConfig {
                                address: row.get(11)?,
                                token: row.get(12)?,
                            },
                            tag_rule_scan_interval_mins: row.get::<_, i64>(3).unwrap_or(7) as u64,
                            ocr_api_key: row.get(4)?,
                        })
                    },
                )
                .map_err(sql_error)?;
            Ok(settings)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn update_settings(&self, settings: &GlobalConfig) -> Result<(), AppError> {
        let path = self.path.clone();
        let settings = settings.clone();
        tokio::task::spawn_blocking(move || -> Result<(), AppError> {
            let conn = open_connection(&path)?;
            conn.execute(
                "UPDATE global_settings SET
                    log_level = ?, proxy = ?,
                    use_proxy_for_lightpanda = ?, tag_rule_scan_interval_mins = ?, ocr_api_key = ?,
                    lightpanda_endpoint = ?, lightpanda_token = ?, lightpanda_region = ?,
                    lightpanda_browser = ?, lightpanda_proxy = ?, lightpanda_country = ?,
                    browserless_address = ?, browserless_token = ?
                 WHERE id = 1",
                params![
                    settings.log_level,
                    settings.proxy.as_deref().and_then(|value| {
                        let value = value.trim();
                        (!value.is_empty()).then_some(value)
                    }),
                    settings.use_proxy_for_lightpanda as i32,
                    settings.tag_rule_scan_interval_mins as i64,
                    settings.ocr_api_key.as_deref().and_then(|value| {
                        let value = value.trim();
                        (!value.is_empty()).then_some(value)
                    }),
                    settings
                        .lightpanda
                        .endpoint
                        .as_deref()
                        .and_then(non_empty_trimmed),
                    settings
                        .lightpanda
                        .token
                        .as_deref()
                        .and_then(non_empty_trimmed),
                    normalize_lightpanda_region(&settings.lightpanda.region),
                    non_empty_trimmed(&settings.lightpanda.browser).unwrap_or("lightpanda"),
                    settings
                        .lightpanda
                        .proxy
                        .as_deref()
                        .and_then(non_empty_trimmed),
                    settings
                        .lightpanda
                        .country
                        .as_deref()
                        .and_then(non_empty_trimmed),
                    settings
                        .browserless
                        .address
                        .as_deref()
                        .and_then(non_empty_trimmed),
                    settings
                        .browserless
                        .token
                        .as_deref()
                        .and_then(non_empty_trimmed),
                ],
            )
            .map_err(sql_error)?;
            Ok(())
        })
        .await
        .map_err(join_error)?
    }

    // ========== Sites ==========

    pub async fn list_sites(&self) -> Result<Vec<SiteRecord>, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let mut stmt = conn
                .prepare("SELECT id, name, site_type, base_url, auth_config, request_headers, use_proxy, created_at, updated_at FROM sites ORDER BY id")
                .map_err(sql_error)?;
            let rows = stmt
                .query_map([], |row| {
                    Ok(SiteRecord {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        site_type: row.get(2)?,
                        base_url: row.get(3)?,
                        auth_config: row.get(4)?,
                        request_headers: row.get(5)?,
                        use_proxy: row.get::<_, i32>(6).unwrap_or(1) != 0,
                        created_at: row.get(7)?,
                        updated_at: row.get(8)?,
                    })
                })
                .map_err(sql_error)?;
            let mut sites = Vec::new();
            for row in rows {
                sites.push(row.map_err(sql_error)?);
            }
            Ok(sites)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn list_sites_with_stats(&self) -> Result<Vec<SiteWithStats>, AppError> {
        self.query_sites_with_stats(None).await
    }

    async fn query_sites_with_stats(
        &self,
        id: Option<i64>,
    ) -> Result<Vec<SiteWithStats>, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let filter = if id.is_some() { "WHERE s.id = ?1" } else { "" };
            let mut stmt = conn
                .prepare(
                    &format!("SELECT s.id, s.name, s.site_type, s.base_url, s.auth_config, s.request_headers, s.use_proxy, s.created_at, s.updated_at,
                            st.site_id, st.uid, st.username, st.uploaded, st.downloaded, st.ratio, st.bonus,
                            st.seeding_count, st.leeching_count, st.updated_at, st.last_checked_at, st.last_error,
                            st.details_json
                     FROM sites s
                     LEFT JOIN site_stats st ON st.site_id = s.id
                     {filter}
                     ORDER BY s.id"),
                )
                .map_err(sql_error)?;
            let rows = stmt
                .query_map(rusqlite::params_from_iter(id), |row| {
                    let stats_site_id: Option<i64> = row.get(9)?;
                    Ok(SiteWithStats {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        site_type: row.get(2)?,
                        base_url: row.get(3)?,
                        auth_config: row.get(4)?,
                        request_headers: row.get(5)?,
                        use_proxy: row.get::<_, i32>(6).unwrap_or(1) != 0,
                        created_at: row.get(7)?,
                        updated_at: row.get(8)?,
                        stats: stats_site_id.map(|site_id| SiteStatsRecord {
                            site_id,
                            uid: row.get(10).ok().flatten(),
                            username: row.get(11).ok().flatten(),
                            uploaded: row.get::<_, Option<i64>>(12).ok().flatten().map(|v| v as u64),
                            downloaded: row.get::<_, Option<i64>>(13).ok().flatten().map(|v| v as u64),
                            ratio: row.get(14).ok().flatten(),
                            bonus: row.get(15).ok().flatten(),
                            seeding_count: row
                                .get::<_, Option<i64>>(16)
                                .ok()
                                .flatten()
                                .map(|v| v as u32),
                            leeching_count: row
                                .get::<_, Option<i64>>(17)
                                .ok()
                                .flatten()
                                .map(|v| v as u32),
                            details: row
                                .get::<_, Option<String>>(21)
                                .ok()
                                .flatten()
                                .and_then(|value| serde_json::from_str(&value).ok())
                                .unwrap_or_default(),
                            updated_at: row.get(18).ok().flatten(),
                            last_checked_at: row.get(19).unwrap_or_default(),
                            last_error: row.get(20).ok().flatten(),
                        }),
                    })
                })
                .map_err(sql_error)?;
            let mut sites = Vec::new();
            for row in rows {
                sites.push(row.map_err(sql_error)?);
            }
            Ok(sites)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn get_site_with_stats(&self, id: i64) -> Result<Option<SiteWithStats>, AppError> {
        Ok(self
            .query_sites_with_stats(Some(id))
            .await?
            .into_iter()
            .next())
    }

    pub async fn get_site(&self, id: i64) -> Result<Option<SiteRecord>, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.query_row(
                "SELECT id, name, site_type, base_url, auth_config, request_headers, use_proxy, created_at, updated_at FROM sites WHERE id = ?",
                params![id],
                |row| {
                    Ok(SiteRecord {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        site_type: row.get(2)?,
                        base_url: row.get(3)?,
                        auth_config: row.get(4)?,
                        request_headers: row.get(5)?,
                        use_proxy: row.get::<_, i32>(6).unwrap_or(1) != 0,
                        created_at: row.get(7)?,
                        updated_at: row.get(8)?,
                    })
                },
            )
            .optional()
            .map_err(sql_error)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn create_site(
        &self,
        name: &str,
        site_type: &str,
        base_url: &str,
        auth_config: &str,
        request_headers: &str,
        use_proxy: bool,
    ) -> Result<i64, AppError> {
        let path = self.path.clone();
        let now = Utc::now().to_rfc3339();
        let (name, site_type, base_url, auth_config, request_headers) = (
            name.to_string(),
            site_type.to_string(),
            base_url.to_string(),
            auth_config.to_string(),
            request_headers.to_string(),
        );
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.execute(
                "INSERT INTO sites (name, site_type, base_url, auth_config, request_headers, use_proxy, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                params![name, site_type, base_url, auth_config, request_headers, use_proxy as i32, now, now],
            )
            .map_err(sql_error)?;
            Ok(conn.last_insert_rowid())
        })
        .await
        .map_err(join_error)?
    }

    pub async fn update_site(
        &self,
        id: i64,
        name: &str,
        site_type: &str,
        base_url: &str,
        auth_config: &str,
        request_headers: &str,
        use_proxy: bool,
    ) -> Result<(), AppError> {
        let path = self.path.clone();
        let now = Utc::now().to_rfc3339();
        let (name, site_type, base_url, auth_config, request_headers) = (
            name.to_string(),
            site_type.to_string(),
            base_url.to_string(),
            auth_config.to_string(),
            request_headers.to_string(),
        );
        tokio::task::spawn_blocking(move || {
            let mut conn = open_connection(&path)?;
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(sql_error)?;
            if site_has_rss_reference(&tx, id)? {
                let current: Option<(String,String)> = tx.query_row("SELECT site_type,base_url FROM sites WHERE id=?",[id],|row|Ok((row.get(0)?,row.get(1)?))).optional().map_err(sql_error)?;
                if current.is_some_and(|(kind,url)| kind != site_type || !same_downloader_url(&url,&base_url)) {
                    return Err(AppError::InvalidConfig {message:"站点正在被启用的 RSS 规则或活动任务引用，不能更换站点类型或地址；请先解除引用".into()});
                }
                if matches!(auth_config.trim(),""|"{}"|"null") {
                    let old:String=tx.query_row("SELECT auth_config FROM sites WHERE id=?",[id],|row|row.get(0)).map_err(sql_error)?;
                    if !matches!(old.trim(),""|"{}"|"null") { return Err(AppError::InvalidConfig {message:"站点仍被 RSS 引用，不能清空凭据；可以更新有效凭据".into()}); }
                }
            }
            tx.execute(
                "UPDATE sites SET name = ?, site_type = ?, base_url = ?, auth_config = ?, request_headers = ?, use_proxy = ?, updated_at = ? WHERE id = ?",
                params![name, site_type, base_url, auth_config, request_headers, use_proxy as i32, now, id],
            )
            .map_err(sql_error)?;
            tx.commit().map_err(sql_error)?;
            Ok(())
        })
        .await
        .map_err(join_error)?
    }

    pub async fn delete_site(&self, id: i64) -> Result<(), AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = open_connection(&path)?;
            let tx=conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(sql_error)?;
            if site_has_rss_reference(&tx,id)? {
                return Err(AppError::InvalidConfig {message:"站点仍被启用的 RSS 规则或活动任务引用，请先解除或替换引用".into()});
            }
            tx.execute("UPDATE rss_feeds SET enabled=0,last_error='关联站点已删除，请检查来源认证配置',version=version+1,lease_owner=NULL,lease_until=NULL,lease_version=lease_version+1 WHERE site_id=? AND archived_at IS NULL",[id]).map_err(sql_error)?;
            tx.execute("DELETE FROM sites WHERE id = ?", params![id])
                .map_err(sql_error)?;
            tx.commit().map_err(sql_error)?;
            Ok(())
        })
        .await
        .map_err(join_error)?
    }

    pub async fn upsert_site_stats_success(
        &self,
        site_id: i64,
        stats: &UserStats,
        checked_at: &str,
    ) -> Result<(), AppError> {
        let path = self.path.clone();
        let mut stats = stats.clone();
        stats.fill_derived();
        let checked_at = checked_at.to_string();
        tokio::task::spawn_blocking(move || {
            let mut conn = open_connection(&path)?;
            let details_json =
                serde_json::to_string(&stats.details).map_err(|error| AppError::Database {
                    message: format!("failed to serialize extended site statistics: {error}"),
                })?;
            let stats_json = serde_json::to_string(&stats).map_err(|error| AppError::Database {
                message: format!("failed to serialize site statistics snapshot: {error}"),
            })?;
            let snapshot_date = DateTime::parse_from_rfc3339(&checked_at)
                .map(|value| value.with_timezone(&Utc).format("%Y-%m-%d").to_string())
                .unwrap_or_else(|_| checked_at.chars().take(10).collect());
            let tx = conn.transaction().map_err(sql_error)?;
            tx.execute(
                "INSERT INTO site_stats
                 (site_id, uid, username, uploaded, downloaded, ratio, bonus, seeding_count,
                  leeching_count, details_json, updated_at, last_checked_at, last_error)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL)
                 ON CONFLICT(site_id) DO UPDATE SET
                   uid = excluded.uid,
                   username = excluded.username,
                   uploaded = excluded.uploaded,
                   downloaded = excluded.downloaded,
                   ratio = excluded.ratio,
                   bonus = excluded.bonus,
                   seeding_count = excluded.seeding_count,
                   leeching_count = excluded.leeching_count,
                   details_json = excluded.details_json,
                   updated_at = excluded.updated_at,
                   last_checked_at = excluded.last_checked_at,
                   last_error = NULL",
                params![
                    site_id,
                    stats.uid,
                    stats.username,
                    clamp_u64_to_i64(stats.uploaded),
                    clamp_u64_to_i64(stats.downloaded),
                    stats.ratio,
                    stats.bonus,
                    stats.seeding_count.map(|v| v as i64),
                    stats.leeching_count.map(|v| v as i64),
                    details_json,
                    checked_at,
                    checked_at,
                ],
            )
            .map_err(sql_error)?;
            tx.execute(
                "INSERT INTO site_stats_history
                 (site_id, snapshot_date, updated_at, stats_json)
                 VALUES (?, ?, ?, ?)
                 ON CONFLICT(site_id, snapshot_date) DO UPDATE SET
                   updated_at = excluded.updated_at,
                   stats_json = excluded.stats_json",
                params![site_id, snapshot_date, checked_at, stats_json],
            )
            .map_err(sql_error)?;
            tx.commit().map_err(sql_error)?;
            Ok(())
        })
        .await
        .map_err(join_error)?
    }

    pub async fn list_site_stats_history(&self) -> Result<Vec<SiteStatsHistoryRecord>, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let mut stmt = conn
                .prepare(
                    "SELECT site_id, snapshot_date, updated_at, stats_json
                     FROM site_stats_history
                     ORDER BY site_id, snapshot_date",
                )
                .map_err(sql_error)?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                })
                .map_err(sql_error)?;
            let mut history = Vec::new();
            for row in rows {
                let (site_id, snapshot_date, updated_at, stats_json) = row.map_err(sql_error)?;
                let mut stats = serde_json::from_str::<UserStats>(&stats_json).map_err(|error| {
                    AppError::Database {
                        message: format!(
                            "invalid site statistics history for site {site_id} on {snapshot_date}: {error}"
                        ),
                    }
                })?;
                stats.fill_derived();
                history.push(SiteStatsHistoryRecord {
                    site_id,
                    snapshot_date,
                    updated_at,
                    stats,
                });
            }
            Ok(history)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn upsert_site_stats_error(
        &self,
        site_id: i64,
        message: &str,
        checked_at: &str,
    ) -> Result<(), AppError> {
        let path = self.path.clone();
        let message = message.to_string();
        let checked_at = checked_at.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.execute(
                "INSERT INTO site_stats (site_id, last_checked_at, last_error)
                 VALUES (?, ?, ?)
                 ON CONFLICT(site_id) DO UPDATE SET
                   last_checked_at = excluded.last_checked_at,
                   last_error = excluded.last_error",
                params![site_id, checked_at, message],
            )
            .map_err(sql_error)?;
            Ok(())
        })
        .await
        .map_err(join_error)?
    }

    // ========== Sign-in Tasks ==========

    pub async fn list_sign_in_tasks(&self) -> Result<Vec<SignInTaskRecord>, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let mut stmt = conn
                .prepare(
                    "SELECT id, name, site_id, cron_expression, browser, sign_in_method,
                     browserless_selector, browserless_cf_mode, browserless_wait_ms,
                     browserless_solve_timeout, browserless_action_timeout,
                     browserless_post_click_wait_ms, enabled,
                     last_status, last_message, last_run_at, created_at, updated_at,
                 attendance_path, captcha_selector, captcha_input_selector, already_keywords, submit_method, result_rules
                     FROM sign_in_tasks ORDER BY id",
                )
                .map_err(sql_error)?;
            let rows = stmt.query_map([], map_sign_in_task).map_err(sql_error)?;
            let mut list = Vec::new();
            for row in rows {
                list.push(row.map_err(sql_error)?);
            }
            Ok(list)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn get_sign_in_task(&self, id: i64) -> Result<Option<SignInTaskRecord>, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.query_row(
                "SELECT id, name, site_id, cron_expression, browser, sign_in_method,
                 browserless_selector, browserless_cf_mode, browserless_wait_ms,
                 browserless_solve_timeout, browserless_action_timeout,
                 browserless_post_click_wait_ms, enabled,
                 last_status, last_message, last_run_at, created_at, updated_at,
                 attendance_path, captcha_selector, captcha_input_selector, already_keywords, submit_method, result_rules
                 FROM sign_in_tasks WHERE id = ?",
                params![id],
                map_sign_in_task,
            )
            .optional()
            .map_err(sql_error)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn create_sign_in_task(&self, req: &SignInTaskRequest) -> Result<i64, AppError> {
        let path = self.path.clone();
        let req = req.clone();
        let now = Utc::now().to_rfc3339();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let browserless = req.browserless.unwrap_or_default();
            conn.execute(
                "INSERT INTO sign_in_tasks
                 (name, site_id, cron_expression, lightpanda_endpoint, lightpanda_token,
                  lightpanda_region, browser, proxy, country, sign_in_method,
                  browserless_selector, browserless_cf_mode, browserless_wait_ms,
                  browserless_solve_timeout, browserless_action_timeout,
                  browserless_post_click_wait_ms, enabled, created_at, updated_at, attendance_path, captcha_selector, captcha_input_selector, already_keywords, submit_method, result_rules)
                  VALUES (?, ?, ?, NULL, '', 'euwest', ?, 'fast_dc', NULL, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?, ?, ?, ?, ?, ?, ?)",
                params![
                    req.name,
                    req.site_id,
                    req.cron_expression,
                    req.browser.unwrap_or_else(|| "lightpanda".to_string()),
                    req.sign_in_method
                        .as_deref()
                        .unwrap_or(crate::sign_in::SIGN_IN_METHOD_OPEN_PAGE),
                    browserless.selector,
                    browserless.cf_mode,
                    browserless.wait_ms.map(|value| value as i64),
                    browserless.solve_timeout.map(|value| value as i64),
                    browserless.action_timeout.map(|value| value as i64),
                    browserless.post_click_wait_ms.map(|value| value as i64),
                    now,
                    now,
                    browserless.attendance_path,
                    browserless.captcha_selector,
                    browserless.captcha_input_selector,
                    browserless.already_keywords,
                    browserless.submit_method,
                    serde_json::to_string(&browserless.result_rules).expect("serializable result rules"),
                ],
            )
            .map_err(sign_in_write_error)?;
            Ok(conn.last_insert_rowid())
        })
        .await
        .map_err(join_error)?
    }

    pub async fn update_sign_in_task(
        &self,
        id: i64,
        req: &SignInTaskRequest,
    ) -> Result<(), AppError> {
        let path = self.path.clone();
        let req = req.clone();
        let now = Utc::now().to_rfc3339();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let browserless = req.browserless.unwrap_or_default();
            conn.execute(
                "UPDATE sign_in_tasks SET
                 name = ?, site_id = ?, cron_expression = ?, browser = ?, sign_in_method = ?,
                 browserless_selector = ?, browserless_cf_mode = ?, browserless_wait_ms = ?,
                 browserless_solve_timeout = ?, browserless_action_timeout = ?,
                 browserless_post_click_wait_ms = ?, updated_at = ?, attendance_path = ?, captcha_selector = ?, captcha_input_selector = ?, already_keywords = ?, submit_method = ?, result_rules = ?
                 WHERE id = ?",
                params![
                    req.name,
                    req.site_id,
                    req.cron_expression,
                    req.browser.unwrap_or_else(|| "lightpanda".to_string()),
                    req.sign_in_method
                        .as_deref()
                        .unwrap_or(crate::sign_in::SIGN_IN_METHOD_OPEN_PAGE),
                    browserless.selector,
                    browserless.cf_mode,
                    browserless.wait_ms.map(|value| value as i64),
                    browserless.solve_timeout.map(|value| value as i64),
                    browserless.action_timeout.map(|value| value as i64),
                    browserless.post_click_wait_ms.map(|value| value as i64),
                    now,
                    browserless.attendance_path,
                    browserless.captcha_selector,
                    browserless.captcha_input_selector,
                    browserless.already_keywords,
                    browserless.submit_method,
                    serde_json::to_string(&browserless.result_rules).expect("serializable result rules"),
                    id,
                ],
            )
            .map_err(sign_in_write_error)?;
            Ok(())
        })
        .await
        .map_err(join_error)?
    }

    pub async fn delete_sign_in_task(&self, id: i64) -> Result<(), AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.execute("DELETE FROM sign_in_tasks WHERE id = ?", params![id])
                .map_err(sql_error)?;
            Ok(())
        })
        .await
        .map_err(join_error)?
    }

    pub async fn set_sign_in_task_enabled(&self, id: i64, enabled: bool) -> Result<(), AppError> {
        let path = self.path.clone();
        let now = Utc::now().to_rfc3339();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.execute(
                "UPDATE sign_in_tasks SET enabled = ?, updated_at = ? WHERE id = ?",
                params![enabled as i32, now, id],
            )
            .map_err(sql_error)?;
            Ok(())
        })
        .await
        .map_err(join_error)?
    }

    pub async fn update_sign_in_task_result(
        &self,
        id: i64,
        status: &str,
        message: &str,
        run_at: &str,
    ) -> Result<(), AppError> {
        let path = self.path.clone();
        let (status, message, run_at) =
            (status.to_string(), message.to_string(), run_at.to_string());
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.execute(
                "UPDATE sign_in_tasks SET last_status = ?, last_message = ?, last_run_at = ?, updated_at = ? WHERE id = ?",
                params![status, message, run_at, Utc::now().to_rfc3339(), id],
            )
            .map_err(sql_error)?;
            Ok(())
        })
        .await
        .map_err(join_error)?
    }

    pub async fn insert_sign_in_record(
        &self,
        task: &SignInTaskRecord,
        site_id: i64,
        site_name: &str,
        result: &SignInResult,
    ) -> Result<i64, AppError> {
        let path = self.path.clone();
        let task = task.clone();
        let site_name = site_name.to_string();
        let result = result.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.execute(
                "INSERT INTO sign_in_records
                 (task_id, site_id, site_name, started_at, finished_at, status, message)
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
                params![
                    task.id,
                    site_id,
                    site_name,
                    result.started_at,
                    result.finished_at,
                    result.status,
                    result.message
                ],
            )
            .map_err(sql_error)?;
            Ok(conn.last_insert_rowid())
        })
        .await
        .map_err(join_error)?
    }

    pub async fn list_sign_in_records(
        &self,
        task_id: Option<i64>,
        limit: usize,
    ) -> Result<Vec<SignInRecord>, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let limit = limit.clamp(1, 500) as i64;
            let mut records = Vec::new();
            if let Some(task_id) = task_id {
                let mut stmt = conn
                    .prepare(
                        "SELECT id, task_id, site_id, site_name, started_at, finished_at, status, message
                         FROM sign_in_records WHERE task_id = ? ORDER BY id DESC LIMIT ?",
                    )
                    .map_err(sql_error)?;
                let rows = stmt
                    .query_map(params![task_id, limit], map_sign_in_record)
                    .map_err(sql_error)?;
                for row in rows {
                    records.push(row.map_err(sql_error)?);
                }
            } else {
                let mut stmt = conn
                    .prepare(
                        "SELECT id, task_id, site_id, site_name, started_at, finished_at, status, message
                         FROM sign_in_records ORDER BY id DESC LIMIT ?",
                    )
                    .map_err(sql_error)?;
                let rows = stmt
                    .query_map(params![limit], map_sign_in_record)
                    .map_err(sql_error)?;
                for row in rows {
                    records.push(row.map_err(sql_error)?);
                }
            }
            Ok(records)
        })
        .await
        .map_err(join_error)?
    }

    // ========== Downloaders ==========

    pub async fn list_downloaders(&self) -> Result<Vec<DownloaderRecord>, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let mut stmt = conn
                .prepare("SELECT id, name, downloader_type, url, username, password, created_at, updated_at FROM downloaders ORDER BY id")
                .map_err(sql_error)?;
            let rows = stmt
                .query_map([], |row| {
                    Ok(DownloaderRecord {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        downloader_type: row.get(2)?,
                        url: row.get(3)?,
                        username: row.get(4)?,
                        password: row.get(5)?,
                        created_at: row.get(6)?,
                        updated_at: row.get(7)?,
                    })
                })
                .map_err(sql_error)?;
            let mut list = Vec::new();
            for row in rows {
                list.push(row.map_err(sql_error)?);
            }
            Ok(list)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn get_downloader(&self, id: i64) -> Result<Option<DownloaderRecord>, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.query_row(
                "SELECT id, name, downloader_type, url, username, password, created_at, updated_at FROM downloaders WHERE id = ?",
                params![id],
                |row| {
                    Ok(DownloaderRecord {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        downloader_type: row.get(2)?,
                        url: row.get(3)?,
                        username: row.get(4)?,
                        password: row.get(5)?,
                        created_at: row.get(6)?,
                        updated_at: row.get(7)?,
                    })
                },
            )
            .optional()
            .map_err(sql_error)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn create_downloader(
        &self,
        name: &str,
        dtype: &str,
        url: &str,
        username: &str,
        password: &str,
    ) -> Result<i64, AppError> {
        let path = self.path.clone();
        let now = Utc::now().to_rfc3339();
        let (name, dtype, url, username, password) = (
            name.to_string(),
            dtype.to_string(),
            url.to_string(),
            username.to_string(),
            password.to_string(),
        );
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.execute(
                "INSERT INTO downloaders (name, downloader_type, url, username, password, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
                params![name, dtype, url, username, password, now, now],
            )
            .map_err(sql_error)?;
            Ok(conn.last_insert_rowid())
        })
        .await
        .map_err(join_error)?
    }

    pub async fn update_downloader(
        &self,
        id: i64,
        name: &str,
        dtype: &str,
        url: &str,
        username: &str,
        password: &str,
    ) -> Result<(), AppError> {
        let path = self.path.clone();
        let now = Utc::now().to_rfc3339();
        let (name, dtype, url, username, password) = (
            name.to_string(),
            dtype.to_string(),
            url.to_string(),
            username.to_string(),
            password.to_string(),
        );
        tokio::task::spawn_blocking(move || {
            let mut conn = open_connection(&path)?;
            let tx = conn
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(sql_error)?;
            let current = tx
                .query_row(
                    "SELECT downloader_type, url, username, password
                     FROM downloaders WHERE id = ?",
                    [id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                        ))
                    },
                )
                .optional()
                .map_err(sql_error)?;
            if let Some((current_type, current_url, current_username, current_password)) = current {
                let active = downloader_has_active_qb_work(&tx, id, &now)?;
                if active
                    && (!same_downloader_type(&current_type, &dtype)
                        || !same_downloader_url(&current_url, &url))
                {
                    return Err(AppError::InvalidConfig {
                        message: "downloader URL or type cannot change while relocation jobs are active or downloads are being processed"
                            .to_string(),
                    });
                }
                let credentials_changed =
                    current_username != username || current_password != password;
                if active
                    && credentials_changed
                    && (username.trim().is_empty() || password.is_empty())
                {
                    return Err(AppError::InvalidConfig {
                        message: "downloader credentials cannot be cleared while relocation jobs are active or downloads are being processed"
                            .to_string(),
                    });
                }
            }
            tx.execute(
                "UPDATE downloaders SET name = ?, downloader_type = ?, url = ?, username = ?, password = ?, updated_at = ? WHERE id = ?",
                params![name, dtype, url, username, password, now, id],
            )
            .map_err(sql_error)?;
            tx.commit().map_err(sql_error)?;
            Ok(())
        })
        .await
        .map_err(join_error)?
    }

    pub async fn has_active_relocation_for_downloader(&self, id: i64) -> Result<bool, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            downloader_has_active_relocation(&conn, id, &Utc::now().to_rfc3339())
        })
        .await
        .map_err(join_error)?
    }

    pub async fn delete_downloader(&self, id: i64) -> Result<(), AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = open_connection(&path)?;
            let tx = conn
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(sql_error)?;
            if downloader_has_active_qb_work(&tx, id, &Utc::now().to_rfc3339())? {
                return Err(AppError::InvalidConfig {
                    message: "downloader cannot be deleted while relocation jobs are active or downloads are being processed"
                        .to_string(),
                });
            }
            tx.execute("UPDATE rss_rules SET enabled=0,last_error='目标下载器已删除，请重新选择下载器',version=version+1 WHERE downloader_id=? AND archived_at IS NULL",[id]).map_err(sql_error)?;
            tx.execute("DELETE FROM downloaders WHERE id = ?", params![id])
                .map_err(sql_error)?;
            tx.commit().map_err(sql_error)?;
            Ok(())
        })
        .await
        .map_err(join_error)?
    }

    // ========== Brush Tasks ==========

    pub async fn list_brush_tasks(&self) -> Result<Vec<BrushTaskRecord>, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let mut stmt = conn
                .prepare(
                    "SELECT id, name, cron_expression, site_id, downloader_ids, tag, rss_url,
                     seed_volume_gb, save_dir, active_time_windows,
                     promotion, skip_hit_and_run, max_concurrent,
                     download_speed_limit, upload_speed_limit,
                     size_ranges, seeder_ranges, min_free_hours,
                     delete_mode, delete_on_free_expiry, min_seed_time_hours, hr_min_seed_time_hours,
                     target_ratio, max_upload_gb, download_timeout_hours,
                     min_avg_upload_speed_kbs, max_inactive_hours, min_disk_space_gb,
                     enabled, created_at, updated_at, downloader_ranges, last_run_info, downloader_weights
                     FROM brush_tasks ORDER BY id",
                )
                .map_err(sql_error)?;
            let rows = stmt
                .query_map([], |row| row_to_brush_task(row))
                .map_err(sql_error)?;
            let mut list = Vec::new();
            for row in rows {
                list.push(row.map_err(sql_error)?);
            }
            Ok(list)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn get_brush_task(&self, id: i64) -> Result<Option<BrushTaskRecord>, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.query_row(
                "SELECT id, name, cron_expression, site_id, downloader_ids, tag, rss_url,
                 seed_volume_gb, save_dir, active_time_windows,
                 promotion, skip_hit_and_run, max_concurrent,
                 download_speed_limit, upload_speed_limit,
                 size_ranges, seeder_ranges, min_free_hours,
                 delete_mode, delete_on_free_expiry, min_seed_time_hours, hr_min_seed_time_hours,
                 target_ratio, max_upload_gb, download_timeout_hours,
                 min_avg_upload_speed_kbs, max_inactive_hours, min_disk_space_gb,
                 enabled, created_at, updated_at, downloader_ranges, last_run_info, downloader_weights
                 FROM brush_tasks WHERE id = ?",
                params![id],
                |row| row_to_brush_task(row),
            )
            .optional()
            .map_err(sql_error)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn create_brush_task(&self, req: &BrushTaskRequest) -> Result<i64, AppError> {
        let path = self.path.clone();
        let now = Utc::now().to_rfc3339();
        let req = req.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let promotion = req.promotion.unwrap_or_else(|| "all".to_string());
            let min_free_hours = if promotion == "free" {
                req.min_free_hours
            } else {
                None
            };
            let skip_hr = req.skip_hit_and_run.unwrap_or(true) as i32;
            let max_concurrent = req.max_concurrent.unwrap_or(100);
            let delete_mode = req.delete_mode.unwrap_or_else(|| "or".to_string());
            let delete_on_free_expiry = req.delete_on_free_expiry.unwrap_or(false) as i32;
            let downloader_ids_json =
                serde_json::to_string(&req.downloader_ids).unwrap_or_else(|_| "[]".to_string());
            conn.execute(
                "INSERT INTO brush_tasks (name, cron_expression, site_id, downloader_ids, tag, rss_url,
                 seed_volume_gb, save_dir, active_time_windows,
                 promotion, skip_hit_and_run, max_concurrent,
                 download_speed_limit, upload_speed_limit,
                 size_ranges, seeder_ranges, downloader_ranges, downloader_weights, min_free_hours,
                 delete_mode, delete_on_free_expiry, min_seed_time_hours, hr_min_seed_time_hours,
                 target_ratio, max_upload_gb, download_timeout_hours,
                 min_avg_upload_speed_kbs, max_inactive_hours, min_disk_space_gb,
                 enabled, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?)",
                params![
                    req.name, req.cron_expression, req.site_id, downloader_ids_json, req.tag, req.rss_url,
                    req.seed_volume_gb, req.save_dir, req.active_time_windows,
                    promotion, skip_hr, max_concurrent,
                    req.download_speed_limit, req.upload_speed_limit,
                    req.size_ranges, req.seeder_ranges, req.downloader_ranges, req.downloader_weights, min_free_hours,
                    delete_mode, delete_on_free_expiry, req.min_seed_time_hours, req.hr_min_seed_time_hours,
                    req.target_ratio, req.max_upload_gb, req.download_timeout_hours,
                    req.min_avg_upload_speed_kbs, req.max_inactive_hours, req.min_disk_space_gb,
                    now, now
                ],
            )
            .map_err(sql_error)?;
            Ok(conn.last_insert_rowid())
        })
        .await
        .map_err(join_error)?
    }

    pub async fn update_brush_task(&self, id: i64, req: &BrushTaskRequest) -> Result<(), AppError> {
        let path = self.path.clone();
        let now = Utc::now().to_rfc3339();
        let req = req.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let promotion = req.promotion.unwrap_or_else(|| "all".to_string());
            let min_free_hours = if promotion == "free" {
                req.min_free_hours
            } else {
                None
            };
            let skip_hr = req.skip_hit_and_run.unwrap_or(true) as i32;
            let max_concurrent = req.max_concurrent.unwrap_or(100);
            let delete_mode = req.delete_mode.unwrap_or_else(|| "or".to_string());
            let delete_on_free_expiry = req.delete_on_free_expiry.unwrap_or(false) as i32;
            let downloader_ids_json =
                serde_json::to_string(&req.downloader_ids).unwrap_or_else(|_| "[]".to_string());
            conn.execute(
                "UPDATE brush_tasks SET name = ?, cron_expression = ?, site_id = ?, downloader_ids = ?, tag = ?, rss_url = ?,
                 seed_volume_gb = ?, save_dir = ?, active_time_windows = ?,
                 promotion = ?, skip_hit_and_run = ?, max_concurrent = ?,
                 download_speed_limit = ?, upload_speed_limit = ?,
                 size_ranges = ?, seeder_ranges = ?, downloader_ranges = ?, downloader_weights = ?, min_free_hours = ?,
                 delete_mode = ?, delete_on_free_expiry = ?, min_seed_time_hours = ?, hr_min_seed_time_hours = ?,
                 target_ratio = ?, max_upload_gb = ?, download_timeout_hours = ?,
                 min_avg_upload_speed_kbs = ?, max_inactive_hours = ?, min_disk_space_gb = ?,
                 updated_at = ? WHERE id = ?",
                params![
                    req.name, req.cron_expression, req.site_id, downloader_ids_json, req.tag, req.rss_url,
                    req.seed_volume_gb, req.save_dir, req.active_time_windows,
                    promotion, skip_hr, max_concurrent,
                    req.download_speed_limit, req.upload_speed_limit,
                    req.size_ranges, req.seeder_ranges, req.downloader_ranges, req.downloader_weights, min_free_hours,
                    delete_mode, delete_on_free_expiry, req.min_seed_time_hours, req.hr_min_seed_time_hours,
                    req.target_ratio, req.max_upload_gb, req.download_timeout_hours,
                    req.min_avg_upload_speed_kbs, req.max_inactive_hours, req.min_disk_space_gb,
                    now, id
                ],
            )
            .map_err(sql_error)?;
            Ok(())
        })
        .await
        .map_err(join_error)?
    }

    pub async fn delete_brush_task(&self, id: i64) -> Result<(), AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.execute("DELETE FROM brush_tasks WHERE id = ?", params![id])
                .map_err(sql_error)?;
            Ok(())
        })
        .await
        .map_err(join_error)?
    }

    pub async fn set_brush_task_enabled(&self, id: i64, enabled: bool) -> Result<(), AppError> {
        let path = self.path.clone();
        let now = Utc::now().to_rfc3339();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.execute(
                "UPDATE brush_tasks SET enabled = ?, updated_at = ? WHERE id = ?",
                params![enabled as i32, now, id],
            )
            .map_err(sql_error)?;
            Ok(())
        })
        .await
        .map_err(join_error)?
    }

    /// 更新刷流任务最后一次执行信息 (JSON)。不更新 updated_at，保留其"配置变更时间"语义。
    pub async fn update_brush_task_last_run_info(
        &self,
        id: i64,
        json: &str,
    ) -> Result<(), AppError> {
        let path = self.path.clone();
        let json = json.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.execute(
                "UPDATE brush_tasks SET last_run_info = ? WHERE id = ?",
                params![json, id],
            )
            .map_err(sql_error)?;
            Ok(())
        })
        .await
        .map_err(join_error)?
    }

    // ========== Brush Task Torrents ==========

    pub async fn list_brush_task_torrents(
        &self,
        task_id: i64,
        page: usize,
        page_size: usize,
        keyword: Option<&str>,
    ) -> Result<PaginatedBrushTorrents, AppError> {
        let path = self.path.clone();
        let keyword = keyword
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let page = page.max(1);
            let page_size = page_size.clamp(1, 100);
            let offset = (page - 1) * page_size;

            let like = keyword.as_ref().map(|value| format!("%{}%", value));
            let total_records: usize = if let Some(ref like) = like {
                conn.query_row(
                    "SELECT COUNT(*) FROM brush_task_torrents
                     WHERE task_id = ?
                       AND (torrent_name LIKE ? OR COALESCE(torrent_id, '') LIKE ?)",
                    params![task_id, like, like],
                    |row| row.get(0),
                )
                .map_err(sql_error)?
            } else {
                conn.query_row(
                    "SELECT COUNT(*) FROM brush_task_torrents WHERE task_id = ?",
                    params![task_id],
                    |row| row.get(0),
                )
                .map_err(sql_error)?
            };

            let sql = if like.is_some() {
                "SELECT id, task_id, torrent_id, torrent_link, torrent_hash, torrent_name, added_at, size_bytes, is_hr, free_end_timestamp, status, removed_at, remove_reason,
                        uploaded_bytes, downloaded_bytes, download_duration_secs, avg_upload_speed, ratio, last_stats_at, downloader_id
                 FROM brush_task_torrents
                 WHERE task_id = ?
                   AND (torrent_name LIKE ? OR COALESCE(torrent_id, '') LIKE ?)
                 ORDER BY CASE WHEN removed_at IS NULL THEN 0 ELSE 1 END, added_at DESC, id DESC
                 LIMIT ? OFFSET ?"
            } else {
                "SELECT id, task_id, torrent_id, torrent_link, torrent_hash, torrent_name, added_at, size_bytes, is_hr, free_end_timestamp, status, removed_at, remove_reason,
                        uploaded_bytes, downloaded_bytes, download_duration_secs, avg_upload_speed, ratio, last_stats_at, downloader_id
                 FROM brush_task_torrents
                 WHERE task_id = ?
                 ORDER BY CASE WHEN removed_at IS NULL THEN 0 ELSE 1 END, added_at DESC, id DESC
                 LIMIT ? OFFSET ?"
            };

            let mut stmt = conn.prepare(sql).map_err(sql_error)?;
            let mut list = Vec::new();
            if let Some(like) = like {
                let rows = stmt
                    .query_map(params![task_id, like, like, page_size as i64, offset as i64], map_brush_torrent_record)
                    .map_err(sql_error)?;
                for row in rows {
                    list.push(row.map_err(sql_error)?);
                }
            } else {
                let rows = stmt
                    .query_map(params![task_id, page_size as i64, offset as i64], map_brush_torrent_record)
                    .map_err(sql_error)?;
                for row in rows {
                    list.push(row.map_err(sql_error)?);
                }
            }
            Ok(PaginatedBrushTorrents {
                page,
                page_size,
                total_records,
                records: list,
            })
        })
        .await
        .map_err(join_error)?
    }

    pub async fn list_active_brush_torrents(
        &self,
        task_id: i64,
    ) -> Result<Vec<BrushTorrentRecord>, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let mut stmt = conn
                .prepare(
                    "SELECT id, task_id, torrent_id, torrent_link, torrent_hash, torrent_name, added_at, size_bytes, is_hr, free_end_timestamp, status, removed_at, remove_reason,
                            uploaded_bytes, downloaded_bytes, download_duration_secs, avg_upload_speed, ratio, last_stats_at, downloader_id
                     FROM brush_task_torrents WHERE task_id = ? AND status = 'active' ORDER BY id",
                )
                .map_err(sql_error)?;
            let rows = stmt
                .query_map(params![task_id], map_brush_torrent_record)
                .map_err(sql_error)?;
            let mut list = Vec::new();
            for row in rows {
                list.push(row.map_err(sql_error)?);
            }
            Ok(list)
        })
        .await
        .map_err(join_error)?
    }

    /// 返回任务下所有未移除 (status != 'removed') 种子的 torrent_id 集合 (排除 NULL)。
    pub async fn list_non_removed_brush_torrent_ids(
        &self,
        task_id: i64,
    ) -> Result<std::collections::HashSet<String>, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let mut stmt = conn
                .prepare(
                    "SELECT torrent_id FROM brush_task_torrents WHERE task_id = ? AND status != 'removed' AND torrent_id IS NOT NULL",
                )
                .map_err(sql_error)?;
            let rows = stmt
                .query_map(params![task_id], |row| row.get::<_, String>(0))
                .map_err(sql_error)?;
            let mut set = std::collections::HashSet::new();
            for row in rows {
                set.insert(row.map_err(sql_error)?);
            }
            Ok(set)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn list_non_removed_brush_torrents_with_tasks(
        &self,
    ) -> Result<Vec<(BrushTorrentRecord, BrushTaskRecord)>, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let mut stmt = conn
                .prepare(
                    "SELECT bt.id, bt.task_id, bt.torrent_id, bt.torrent_link, bt.torrent_hash, bt.torrent_name,
                            bt.added_at, bt.size_bytes, bt.is_hr, bt.free_end_timestamp, bt.status, bt.removed_at,
                            bt.remove_reason, bt.uploaded_bytes, bt.downloaded_bytes, bt.download_duration_secs,
                            bt.avg_upload_speed, bt.ratio, bt.last_stats_at, bt.downloader_id,
                            t.id, t.name, t.cron_expression, t.site_id, t.downloader_ids, t.tag, t.rss_url,
                            t.seed_volume_gb, t.save_dir, t.active_time_windows,
                            t.promotion, t.skip_hit_and_run, t.max_concurrent,
                            t.download_speed_limit, t.upload_speed_limit,
                            t.size_ranges, t.seeder_ranges, t.min_free_hours,
                            t.delete_mode, t.delete_on_free_expiry, t.min_seed_time_hours, t.hr_min_seed_time_hours,
                            t.target_ratio, t.max_upload_gb, t.download_timeout_hours,
                            t.min_avg_upload_speed_kbs, t.max_inactive_hours, t.min_disk_space_gb,
                            t.enabled, t.created_at, t.updated_at, t.downloader_ranges, t.last_run_info, t.downloader_weights
                     FROM brush_task_torrents bt
                     INNER JOIN brush_tasks t ON bt.task_id = t.id
                     WHERE bt.status != 'removed'",
                )
                .map_err(sql_error)?;
            let rows = stmt
                .query_map([], |row| {
                    let torrent = map_brush_torrent_record(row)?;
                    let task = row_to_brush_task_at(row, 20)?;
                    Ok((torrent, task))
                })
                .map_err(sql_error)?;
            let mut list = Vec::new();
            for row in rows {
                list.push(row.map_err(sql_error)?);
            }
            Ok(list)
        })
        .await
        .map_err(join_error)?
    }

    /// 返回任务下所有未移除种子的体积总和 (bytes)。无种子时返回 None。
    pub async fn sum_non_removed_torrent_size(
        &self,
        task_id: i64,
    ) -> Result<Option<i64>, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.query_row(
                "SELECT SUM(size_bytes) FROM brush_task_torrents WHERE task_id = ? AND status != 'removed'",
                params![task_id],
                |row| row.get::<_, Option<i64>>(0),
            )
            .map_err(sql_error)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn add_brush_torrent(
        &self,
        task_id: i64,
        torrent_id: Option<&str>,
        torrent_link: Option<&str>,
        hash: &str,
        name: &str,
        size_bytes: Option<i64>,
        is_hr: bool,
        free_end_timestamp: Option<i64>,
        downloader_id: i64,
    ) -> Result<i64, AppError> {
        let path = self.path.clone();
        let now = Utc::now().to_rfc3339();
        let (torrent_id, torrent_link, hash, name) = (
            torrent_id.map(|value| value.to_string()),
            torrent_link.map(|value| value.to_string()),
            hash.to_string(),
            name.to_string(),
        );
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.execute(
                "INSERT INTO brush_task_torrents (task_id, torrent_id, torrent_link, torrent_hash, torrent_name, added_at, size_bytes, is_hr, free_end_timestamp, status, downloader_id) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 'active', ?)
                 ON CONFLICT(task_id, torrent_hash) DO UPDATE SET
                    torrent_id = excluded.torrent_id,
                    torrent_link = excluded.torrent_link,
                    torrent_name = excluded.torrent_name,
                    added_at = excluded.added_at,
                    size_bytes = excluded.size_bytes,
                    is_hr = excluded.is_hr,
                    free_end_timestamp = excluded.free_end_timestamp,
                    status = 'active',
                    removed_at = NULL,
                    remove_reason = NULL,
                    downloader_id = excluded.downloader_id",
                params![task_id, torrent_id, torrent_link, hash, name, now, size_bytes, is_hr as i32, free_end_timestamp, downloader_id],
            )
            .map_err(sql_error)?;
            Ok(conn.last_insert_rowid())
        })
        .await
        .map_err(join_error)?
    }

    pub async fn update_brush_torrent_status(
        &self,
        task_id: i64,
        hash: &str,
        status: &str,
        reason: Option<&str>,
    ) -> Result<(), AppError> {
        let path = self.path.clone();
        let now = Utc::now().to_rfc3339();
        let (hash, status) = (hash.to_string(), status.to_string());
        let reason = reason.map(|s| s.to_string());
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.execute(
                "UPDATE brush_task_torrents SET status = ?, removed_at = ?, remove_reason = ? WHERE task_id = ? AND torrent_hash = ? AND status = 'active'",
                params![status, now, reason, task_id, hash],
            )
            .map_err(sql_error)?;
            Ok(())
        })
        .await
        .map_err(join_error)?
    }

    // ========== Stats Snapshots ==========

    /// 在单个连接 + 单个事务中写入一次采集快照的全部统计数据。
    /// 替代此前每个种子各开一个连接的 N+1 写法，显著降低高频采集下的开销。
    pub async fn record_snapshot_stats(&self, batch: SnapshotStatsBatch) -> Result<(), AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || -> Result<(), AppError> {
            let mut conn = open_connection(&path)?;
            let now = Utc::now().to_rfc3339();
            let tx = conn.transaction().map_err(sql_error)?;

            tx.execute(
                "INSERT INTO downloader_speed_snapshots (downloader_id, upload_speed, download_speed, recorded_at) VALUES (?, ?, ?, ?)",
                params![batch.downloader_id, batch.upload_speed, batch.download_speed, now],
            )
            .map_err(sql_error)?;

            for task in &batch.tasks {
                tx.execute(
                    "INSERT INTO task_stats_snapshots (task_id, total_uploaded, total_downloaded, torrent_count, recorded_at) VALUES (?, ?, ?, ?, ?)",
                    params![task.task_id, task.total_uploaded, task.total_downloaded, task.torrent_count, now],
                )
                .map_err(sql_error)?;
            }

            for torrent in &batch.torrents {
                tx.execute(
                    "INSERT INTO torrent_traffic (task_id, torrent_hash, uploaded_bytes, downloaded_bytes, recorded_at) VALUES (?, ?, ?, ?, ?)",
                    params![torrent.task_id, torrent.hash, torrent.uploaded, torrent.downloaded, now],
                )
                .map_err(sql_error)?;
                tx.execute(
                    "UPDATE brush_task_torrents
                     SET uploaded_bytes = ?, downloaded_bytes = ?, download_duration_secs = ?,
                         avg_upload_speed = ?, ratio = ?, last_stats_at = ?
                     WHERE task_id = ? AND torrent_hash = ?",
                    params![
                        torrent.uploaded,
                        torrent.downloaded,
                        torrent.download_duration_secs,
                        torrent.avg_upload_speed,
                        torrent.ratio,
                        now,
                        torrent.task_id,
                        torrent.hash
                    ],
                )
                .map_err(sql_error)?;
            }

            tx.commit().map_err(sql_error)?;
            Ok(())
        })
        .await
        .map_err(join_error)?
    }

    pub async fn update_brush_torrent_stats(
        &self,
        task_id: i64,
        hash: &str,
        uploaded_bytes: i64,
        downloaded_bytes: i64,
        download_duration_secs: i64,
        avg_upload_speed: f64,
        ratio: f64,
    ) -> Result<(), AppError> {
        let path = self.path.clone();
        let now = Utc::now().to_rfc3339();
        let hash = hash.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.execute(
                "UPDATE brush_task_torrents
                 SET uploaded_bytes = ?, downloaded_bytes = ?, download_duration_secs = ?,
                     avg_upload_speed = ?, ratio = ?, last_stats_at = ?
                 WHERE task_id = ? AND torrent_hash = ?",
                params![
                    uploaded_bytes,
                    downloaded_bytes,
                    download_duration_secs,
                    avg_upload_speed,
                    ratio,
                    now,
                    task_id,
                    hash
                ],
            )
            .map_err(sql_error)?;
            Ok(())
        })
        .await
        .map_err(join_error)?
    }

    pub async fn get_brush_task_transfer_totals(
        &self,
        task_id: i64,
    ) -> Result<(i64, i64, i64), AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let totals = conn
                .query_row(
                    "SELECT
                        COALESCE(SUM(uploaded_bytes), 0),
                        COALESCE(SUM(downloaded_bytes), 0),
                        COUNT(*)
                     FROM brush_task_torrents
                     WHERE task_id = ?",
                    params![task_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .map_err(sql_error)?;
            Ok(totals)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn get_downloader_speed_snapshots(
        &self,
        downloader_id: Option<i64>,
        since: &str,
        until: &str,
        bucket_secs: Option<i64>,
    ) -> Result<Vec<DownloaderSpeedSnapshot>, AppError> {
        let path = self.path.clone();
        let (since, until) = (since.to_string(), until.to_string());
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;

            // When bucket_secs is set, aggregate (AVG) within time buckets per downloader.
            // Buckets are computed by truncating recorded_at to the nearest interval.
            if let Some(bs) = bucket_secs.filter(|&bs| bs > 0) {
                // Build a strftime format string that truncates to the given interval.
                // 10s: seconds digit is 0, 60s: seconds are :00, 300s: round to 5min, 3600s: round to hour
                let bucket_expr = if bs < 60 {
                    // Sub-minute: truncate seconds to nearest bucket (e.g., 10s → floor to 0,10,20,…)
                    format!(
                        "strftime('%Y-%m-%dT%H:%M:', recorded_at) || printf('%02d', (CAST(strftime('%S', recorded_at) AS INTEGER) / {bs}) * {bs}) || 'Z'"
                    )
                } else if bs < 3600 {
                    let mins = bs / 60;
                    format!(
                        "strftime('%Y-%m-%dT%H:', recorded_at) || printf('%02d', (CAST(strftime('%M', recorded_at) AS INTEGER) / {mins}) * {mins}) || ':00Z'"
                    )
                } else {
                    let hours = bs / 3600;
                    format!(
                        "strftime('%Y-%m-%dT', recorded_at) || printf('%02d', (CAST(strftime('%H', recorded_at) AS INTEGER) / {hours}) * {hours}) || ':00:00Z'"
                    )
                };

                let (sql, params_vec): (String, Vec<Box<dyn rusqlite::types::ToSql>>) =
                    if let Some(did) = downloader_id {
                        (
                            format!(
                                "SELECT 0, downloader_id, CAST(avg(upload_speed) AS INTEGER), CAST(avg(download_speed) AS INTEGER), {bucket_expr} AS bucket
                                 FROM downloader_speed_snapshots
                                 WHERE downloader_id = ?
                                   AND datetime(recorded_at) >= datetime(?)
                                   AND datetime(recorded_at) <= datetime(?)
                                 GROUP BY downloader_id, bucket
                                 ORDER BY bucket"
                            ),
                            vec![Box::new(did), Box::new(since), Box::new(until)],
                        )
                    } else {
                        (
                            format!(
                                "SELECT 0, downloader_id, CAST(avg(upload_speed) AS INTEGER), CAST(avg(download_speed) AS INTEGER), {bucket_expr} AS bucket
                                 FROM downloader_speed_snapshots
                                 WHERE datetime(recorded_at) >= datetime(?)
                                   AND datetime(recorded_at) <= datetime(?)
                                 GROUP BY downloader_id, bucket
                                 ORDER BY bucket"
                            ),
                            vec![Box::new(since), Box::new(until)],
                        )
                    };

                let mut stmt = conn.prepare(&sql).map_err(sql_error)?;
                let params_refs: Vec<&dyn rusqlite::types::ToSql> =
                    params_vec.iter().map(|p| p.as_ref()).collect();
                let rows = stmt
                    .query_map(params_refs.as_slice(), |row| {
                        Ok(DownloaderSpeedSnapshot {
                            id: row.get(0)?,
                            downloader_id: row.get(1)?,
                            upload_speed: row.get(2)?,
                            download_speed: row.get(3)?,
                            recorded_at: row.get(4)?,
                        })
                    })
                    .map_err(sql_error)?;
                let mut list = Vec::new();
                for row in rows {
                    list.push(row.map_err(sql_error)?);
                }
                return Ok(list);
            }

            // Raw query (no aggregation)
            let (sql, params_vec): (String, Vec<Box<dyn rusqlite::types::ToSql>>) =
                if let Some(downloader_id) = downloader_id {
                    (
                        "SELECT id, downloader_id, upload_speed, download_speed, recorded_at
                         FROM downloader_speed_snapshots
                         WHERE downloader_id = ?
                           AND datetime(recorded_at) >= datetime(?)
                           AND datetime(recorded_at) <= datetime(?)
                         ORDER BY datetime(recorded_at)"
                            .to_string(),
                        vec![Box::new(downloader_id), Box::new(since), Box::new(until)],
                    )
                } else {
                    (
                        "SELECT id, downloader_id, upload_speed, download_speed, recorded_at
                         FROM downloader_speed_snapshots
                         WHERE datetime(recorded_at) >= datetime(?)
                           AND datetime(recorded_at) <= datetime(?)
                         ORDER BY datetime(recorded_at)"
                            .to_string(),
                        vec![Box::new(since), Box::new(until)],
                    )
                };
            let mut stmt = conn.prepare(&sql).map_err(sql_error)?;
            let params_refs: Vec<&dyn rusqlite::types::ToSql> =
                params_vec.iter().map(|p| p.as_ref()).collect();
            let rows = stmt
                .query_map(params_refs.as_slice(), |row| {
                    Ok(DownloaderSpeedSnapshot {
                        id: row.get(0)?,
                        downloader_id: row.get(1)?,
                        upload_speed: row.get(2)?,
                        download_speed: row.get(3)?,
                        recorded_at: row.get(4)?,
                    })
                })
                .map_err(sql_error)?;
            let mut list = Vec::new();
            for row in rows {
                list.push(row.map_err(sql_error)?);
            }
            Ok(list)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn get_task_stats_snapshots(
        &self,
        task_id: Option<i64>,
        since: &str,
        until: &str,
    ) -> Result<Vec<TaskStatsSnapshot>, AppError> {
        let path = self.path.clone();
        let (since, until) = (since.to_string(), until.to_string());
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let (sql, params_vec): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = if let Some(tid) = task_id {
                (
                    "SELECT id, task_id, total_uploaded, total_downloaded, torrent_count, recorded_at
                     FROM task_stats_snapshots
                     WHERE task_id = ?
                       AND datetime(recorded_at) >= datetime(?)
                       AND datetime(recorded_at) <= datetime(?)
                     ORDER BY datetime(recorded_at)".to_string(),
                    vec![Box::new(tid), Box::new(since), Box::new(until)],
                )
            } else {
                (
                    "SELECT id, task_id, total_uploaded, total_downloaded, torrent_count, recorded_at
                     FROM task_stats_snapshots
                     WHERE datetime(recorded_at) >= datetime(?)
                       AND datetime(recorded_at) <= datetime(?)
                     ORDER BY datetime(recorded_at)".to_string(),
                    vec![Box::new(since), Box::new(until)],
                )
            };
            let mut stmt = conn.prepare(&sql).map_err(sql_error)?;
            let params_refs: Vec<&dyn rusqlite::types::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();
            let rows = stmt
                .query_map(params_refs.as_slice(), |row| {
                    Ok(TaskStatsSnapshot {
                        id: row.get(0)?,
                        task_id: row.get(1)?,
                        total_uploaded: row.get(2)?,
                        total_downloaded: row.get(3)?,
                        torrent_count: row.get(4)?,
                        recorded_at: row.get(5)?,
                    })
                })
                .map_err(sql_error)?;
            let mut list = Vec::new();
            for row in rows {
                list.push(row.map_err(sql_error)?);
            }
            Ok(list)
        })
        .await
        .map_err(join_error)?
    }

    /// 返回指定时间范围内每天的增量上传/下载量（bytes）。
    /// 通过比较同一天内第一个和最后一个快照的累计值来计算增量。
    /// 多个任务的结果会按天聚合。返回 (date, uploaded_delta, downloaded_delta)。
    pub async fn get_daily_transfer_totals(
        &self,
        task_id: Option<i64>,
        since: &str,
        until: &str,
    ) -> Result<Vec<(String, i64, i64)>, AppError> {
        // 复用已验证可用的快照查询，在 Rust 侧按天聚合。
        // 避免在遍历游标时对同一连接执行嵌套查询。
        let snapshots = self.get_task_stats_snapshots(task_id, since, until).await?;

        // 按 (task_id, day) 分组，记录当天首个和末个快照的累计值。
        // 快照已按 recorded_at 排序，首次遇到即为当天最早一条。
        let mut task_day_first: std::collections::HashMap<(i64, String), (i64, i64)> =
            std::collections::HashMap::new();
        let mut task_day_last: std::collections::HashMap<(i64, String), (i64, i64)> =
            std::collections::HashMap::new();

        for snap in &snapshots {
            let day = match snap.recorded_at.get(..10) {
                Some(d) => d.to_string(),
                None => continue,
            };
            let key = (snap.task_id, day);
            task_day_first
                .entry(key.clone())
                .or_insert((snap.total_uploaded, snap.total_downloaded));
            task_day_last.insert(key, (snap.total_uploaded, snap.total_downloaded));
        }

        let mut day_map: std::collections::HashMap<String, (i64, i64)> =
            std::collections::HashMap::new();
        for (key, first) in &task_day_first {
            if let Some(last) = task_day_last.get(key) {
                if first == last {
                    continue;
                }
                let delta_up = (last.0 - first.0).max(0);
                let delta_down = (last.1 - first.1).max(0);
                let entry = day_map.entry(key.1.clone()).or_insert((0, 0));
                entry.0 += delta_up;
                entry.1 += delta_down;
            }
        }

        let mut result: Vec<(String, i64, i64)> = day_map
            .into_iter()
            .map(|(day, (up, down))| (day, up, down))
            .collect();
        result.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(result)
    }

    pub async fn save_torrent_traffic(
        &self,
        task_id: i64,
        hash: &str,
        uploaded: i64,
        downloaded: i64,
    ) -> Result<(), AppError> {
        let path = self.path.clone();
        let now = Utc::now().to_rfc3339();
        let hash = hash.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.execute(
                "INSERT INTO torrent_traffic (task_id, torrent_hash, uploaded_bytes, downloaded_bytes, recorded_at) VALUES (?, ?, ?, ?, ?)",
                params![task_id, hash, uploaded, downloaded, now],
            )
            .map_err(sql_error)?;
            Ok(())
        })
        .await
        .map_err(join_error)?
    }

    pub async fn get_recent_torrent_traffic(
        &self,
        task_id: i64,
        hash: &str,
        minutes: i64,
    ) -> Result<Vec<(i64, i64, String)>, AppError> {
        let path = self.path.clone();
        let hash = hash.to_string();
        let since = (Utc::now() - chrono::Duration::minutes(minutes)).to_rfc3339();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let mut stmt = conn
                .prepare(
                    "SELECT uploaded_bytes, downloaded_bytes, recorded_at FROM torrent_traffic
                     WHERE task_id = ? AND torrent_hash = ? AND datetime(recorded_at) >= datetime(?)
                     ORDER BY datetime(recorded_at)",
                )
                .map_err(sql_error)?;
            let rows = stmt
                .query_map(params![task_id, hash, since], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })
                .map_err(sql_error)?;
            let mut list = Vec::new();
            for row in rows {
                list.push(row.map_err(sql_error)?);
            }
            Ok(list)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn cleanup_old_torrent_traffic(&self, days: i64) -> Result<(), AppError> {
        let path = self.path.clone();
        let cutoff = (Utc::now() - chrono::Duration::days(days)).to_rfc3339();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            // recorded_at 全部为同格式 UTC RFC3339 字符串，字典序即时间序，
            // 直接比较裸列可命中 recorded_at 索引，避免 datetime() 包裹导致的全表扫描。
            conn.execute(
                "DELETE FROM torrent_traffic WHERE recorded_at < ?",
                params![cutoff],
            )
            .map_err(sql_error)?;
            Ok(())
        })
        .await
        .map_err(join_error)?
    }

    pub async fn cleanup_old_speed_snapshots(&self, days: i64) -> Result<(), AppError> {
        let path = self.path.clone();
        let cutoff = (Utc::now() - chrono::Duration::days(days)).to_rfc3339();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.execute(
                "DELETE FROM downloader_speed_snapshots WHERE recorded_at < ?",
                params![cutoff],
            )
            .map_err(sql_error)?;
            conn.execute(
                "DELETE FROM task_stats_snapshots WHERE recorded_at < ?",
                params![cutoff],
            )
            .map_err(sql_error)?;
            Ok(())
        })
        .await
        .map_err(join_error)?
    }

    pub async fn insert_system_snapshot(
        &self,
        snap: &crate::monitor::SystemSnapshotRecord,
    ) -> Result<(), AppError> {
        let path = self.path.clone();
        let snap = snap.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.execute(
                "INSERT INTO system_snapshots (process_cpu_usage, process_memory_bytes, system_cpu_usage, system_total_memory_bytes, system_used_memory_bytes, system_available_memory_bytes, recorded_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
                params![
                    snap.process_cpu_usage,
                    snap.process_memory_bytes,
                    snap.system_cpu_usage,
                    snap.system_total_memory_bytes,
                    snap.system_used_memory_bytes,
                    snap.system_available_memory_bytes,
                    snap.recorded_at,
                ],
            )
            .map_err(sql_error)?;
            Ok(())
        })
        .await
        .map_err(join_error)?
    }

    pub async fn get_system_snapshots(
        &self,
        since: &str,
        until: &str,
        bucket_secs: Option<i64>,
    ) -> Result<Vec<crate::monitor::SystemSnapshotRecord>, AppError> {
        let path = self.path.clone();
        let since = since.to_string();
        let until = until.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;

            if let Some(bs) = bucket_secs.filter(|&bs| bs > 0) {
                let bucket_expr = if bs < 60 {
                    format!(
                        "strftime('%Y-%m-%dT%H:%M:', recorded_at) || printf('%02d', (CAST(strftime('%S', recorded_at) AS INTEGER) / {bs}) * {bs}) || 'Z'"
                    )
                } else if bs < 3600 {
                    let mins = bs / 60;
                    format!(
                        "strftime('%Y-%m-%dT%H:', recorded_at) || printf('%02d', (CAST(strftime('%M', recorded_at) AS INTEGER) / {mins}) * {mins}) || ':00Z'"
                    )
                } else {
                    let hours = bs / 3600;
                    format!(
                        "strftime('%Y-%m-%dT', recorded_at) || printf('%02d', (CAST(strftime('%H', recorded_at) AS INTEGER) / {hours}) * {hours}) || ':00:00Z'"
                    )
                };

                let sql = format!(
                    "SELECT 0,
                            CAST(avg(process_cpu_usage) AS REAL),
                            CAST(avg(process_memory_bytes) AS INTEGER),
                            CAST(avg(system_cpu_usage) AS REAL),
                            CAST(avg(system_total_memory_bytes) AS INTEGER),
                            CAST(avg(system_used_memory_bytes) AS INTEGER),
                            CAST(avg(system_available_memory_bytes) AS INTEGER),
                            {bucket_expr} AS bucket
                     FROM system_snapshots
                     WHERE datetime(recorded_at) >= datetime(?)
                       AND datetime(recorded_at) <= datetime(?)
                     GROUP BY bucket
                     ORDER BY bucket"
                );

                let mut stmt = conn.prepare(&sql).map_err(sql_error)?;
                let rows = stmt
                    .query_map(params![since, until], |row| {
                        Ok(crate::monitor::SystemSnapshotRecord {
                            id: row.get(0)?,
                            process_cpu_usage: row.get(1)?,
                            process_memory_bytes: row.get(2)?,
                            system_cpu_usage: row.get(3)?,
                            system_total_memory_bytes: row.get(4)?,
                            system_used_memory_bytes: row.get(5)?,
                            system_available_memory_bytes: row.get(6)?,
                            recorded_at: row.get(7)?,
                        })
                    })
                    .map_err(sql_error)?;
                let mut list = Vec::new();
                for row in rows {
                    list.push(row.map_err(sql_error)?);
                }
                return Ok(list);
            }

            let mut stmt = conn
                .prepare(
                    "SELECT id, process_cpu_usage, process_memory_bytes,
                            system_cpu_usage, system_total_memory_bytes,
                            system_used_memory_bytes, system_available_memory_bytes,
                            recorded_at
                     FROM system_snapshots
                     WHERE datetime(recorded_at) >= datetime(?)
                       AND datetime(recorded_at) <= datetime(?)
                     ORDER BY datetime(recorded_at)",
                )
                .map_err(sql_error)?;
            let rows = stmt
                .query_map(params![since, until], |row| {
                    Ok(crate::monitor::SystemSnapshotRecord {
                        id: row.get(0)?,
                        process_cpu_usage: row.get(1)?,
                        process_memory_bytes: row.get(2)?,
                        system_cpu_usage: row.get(3)?,
                        system_total_memory_bytes: row.get(4)?,
                        system_used_memory_bytes: row.get(5)?,
                        system_available_memory_bytes: row.get(6)?,
                        recorded_at: row.get(7)?,
                    })
                })
                .map_err(sql_error)?;
            let mut list = Vec::new();
            for row in rows {
                list.push(row.map_err(sql_error)?);
            }
            Ok(list)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn cleanup_old_system_snapshots(&self, days: i64) -> Result<(), AppError> {
        let path = self.path.clone();
        let cutoff = (Utc::now() - chrono::Duration::days(days)).to_rfc3339();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.execute(
                "DELETE FROM system_snapshots WHERE recorded_at < ?",
                params![cutoff],
            )
            .map_err(sql_error)?;
            Ok(())
        })
        .await
        .map_err(join_error)?
    }

    // ========== Tag Rules ==========

    pub async fn list_tag_rules(&self) -> Result<Vec<crate::tag_rule::TagRuleRecord>, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let mut stmt = conn
                .prepare(
                    "SELECT id, name, tag_name, match_rules, enabled, downloader_ids, tagged_torrent_count, tagged_total_size, created_at, updated_at
                     FROM tag_rules ORDER BY id",
                )
                .map_err(sql_error)?;
            let rows = stmt
                .query_map([], |row| row_to_tag_rule(row))
                .map_err(sql_error)?;
            let mut list = Vec::new();
            for row in rows {
                list.push(row.map_err(sql_error)?);
            }
            Ok(list)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn list_enabled_tag_rules(
        &self,
    ) -> Result<Vec<crate::tag_rule::TagRuleRecord>, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let mut stmt = conn
                .prepare(
                    "SELECT id, name, tag_name, match_rules, enabled, downloader_ids, tagged_torrent_count, tagged_total_size, created_at, updated_at
                     FROM tag_rules WHERE enabled = 1 ORDER BY id",
                )
                .map_err(sql_error)?;
            let rows = stmt
                .query_map([], |row| row_to_tag_rule(row))
                .map_err(sql_error)?;
            let mut list = Vec::new();
            for row in rows {
                list.push(row.map_err(sql_error)?);
            }
            Ok(list)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn get_tag_rule(
        &self,
        id: i64,
    ) -> Result<Option<crate::tag_rule::TagRuleRecord>, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.query_row(
                "SELECT id, name, tag_name, match_rules, enabled, downloader_ids, tagged_torrent_count, tagged_total_size, created_at, updated_at
                 FROM tag_rules WHERE id = ?",
                params![id],
                |row| row_to_tag_rule(row),
            )
            .optional()
            .map_err(sql_error)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn create_tag_rule(
        &self,
        req: &crate::tag_rule::TagRuleRequest,
    ) -> Result<i64, AppError> {
        let path = self.path.clone();
        let now = Utc::now().to_rfc3339();
        let req = req.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let match_rules_json = serde_json::to_string(&req.match_rules)
                .map_err(|e| sql_error(rusqlite::Error::ToSqlConversionFailure(Box::new(e))))?;
            let downloader_ids_json = req
                .downloader_ids
                .map(|ids| serde_json::to_string(&ids))
                .transpose()
                .map_err(|e| sql_error(rusqlite::Error::ToSqlConversionFailure(Box::new(e))))?;
            let enabled = req.enabled.unwrap_or(true) as i32;
            conn.execute(
                "INSERT INTO tag_rules (name, tag_name, match_rules, enabled, downloader_ids, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
                params![req.name, req.tag_name, match_rules_json, enabled, downloader_ids_json, now, now],
            )
            .map_err(sql_error)?;
            Ok(conn.last_insert_rowid())
        })
        .await
        .map_err(join_error)?
    }

    pub async fn update_tag_rule(
        &self,
        id: i64,
        req: &crate::tag_rule::TagRuleRequest,
    ) -> Result<(), AppError> {
        let path = self.path.clone();
        let now = Utc::now().to_rfc3339();
        let req = req.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let match_rules_json = serde_json::to_string(&req.match_rules)
                .map_err(|e| sql_error(rusqlite::Error::ToSqlConversionFailure(Box::new(e))))?;
            let downloader_ids_json = req
                .downloader_ids
                .map(|ids| serde_json::to_string(&ids))
                .transpose()
                .map_err(|e| sql_error(rusqlite::Error::ToSqlConversionFailure(Box::new(e))))?;
            let enabled = req.enabled.unwrap_or(true) as i32;
            conn.execute(
                "UPDATE tag_rules SET name = ?, tag_name = ?, match_rules = ?, enabled = ?, downloader_ids = ?, updated_at = ?
                 WHERE id = ?",
                params![req.name, req.tag_name, match_rules_json, enabled, downloader_ids_json, now, id],
            )
            .map_err(sql_error)?;
            Ok(())
        })
        .await
        .map_err(join_error)?
    }

    pub async fn delete_tag_rule(&self, id: i64) -> Result<(), AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.execute("DELETE FROM tag_rules WHERE id = ?", params![id])
                .map_err(sql_error)?;
            Ok(())
        })
        .await
        .map_err(join_error)?
    }

    pub async fn update_tag_rule_stats(
        &self,
        id: i64,
        count: i64,
        total_size: i64,
    ) -> Result<(), AppError> {
        let path = self.path.clone();
        let now = Utc::now().to_rfc3339();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.execute(
                "UPDATE tag_rules SET tagged_torrent_count = ?, tagged_total_size = ?, updated_at = ? WHERE id = ?",
                params![count, total_size, now, id],
            )
            .map_err(sql_error)?;
            Ok(())
        })
        .await
        .map_err(join_error)?
    }

    async fn init(&self) -> Result<(), AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || -> Result<(), AppError> {
            let conn = open_connection(&path)?;
            conn.execute_batch(
                "
                PRAGMA journal_mode = WAL;
                PRAGMA foreign_keys = ON;

                CREATE TABLE IF NOT EXISTS global_settings (
                    id INTEGER PRIMARY KEY CHECK (id = 1),
                    log_level TEXT,
                    proxy TEXT,
                    use_proxy_for_lightpanda INTEGER NOT NULL DEFAULT 1,
                    tag_rule_scan_interval_mins INTEGER NOT NULL DEFAULT 7,
                    ocr_api_key TEXT,
                    lightpanda_endpoint TEXT,
                    lightpanda_token TEXT,
                    lightpanda_region TEXT NOT NULL DEFAULT 'euwest',
                    lightpanda_browser TEXT NOT NULL DEFAULT 'lightpanda',
                    lightpanda_proxy TEXT,
                    lightpanda_country TEXT,
                    browserless_address TEXT,
                    browserless_token TEXT,
                    sign_in_browser_config_migrated INTEGER NOT NULL DEFAULT 0
                );

                CREATE TABLE IF NOT EXISTS sites (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    name TEXT NOT NULL,
                    site_type TEXT NOT NULL,
                    base_url TEXT NOT NULL,
                    auth_config TEXT NOT NULL,
                    request_headers TEXT NOT NULL DEFAULT '[]',
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS site_search_bindings (
                    site_id INTEGER PRIMARY KEY REFERENCES sites(id) ON DELETE CASCADE,
                    mode TEXT NOT NULL DEFAULT 'auto' CHECK(mode IN ('auto','manual','none')),
                    catalog_id TEXT,
                    matched_host TEXT,
                    catalog_revision TEXT NOT NULL DEFAULT ''
                );
                INSERT OR IGNORE INTO site_search_bindings(site_id) SELECT id FROM sites;

                CREATE TABLE IF NOT EXISTS site_stats (
                    site_id INTEGER PRIMARY KEY REFERENCES sites(id) ON DELETE CASCADE,
                    uid TEXT,
                    username TEXT,
                    uploaded INTEGER,
                    downloaded INTEGER,
                    ratio REAL,
                    bonus REAL,
                    seeding_count INTEGER,
                    leeching_count INTEGER,
                    details_json TEXT NOT NULL DEFAULT '{}',
                    updated_at TEXT,
                    last_checked_at TEXT NOT NULL,
                    last_error TEXT
                );

                CREATE TABLE IF NOT EXISTS site_stats_history (
                    site_id INTEGER NOT NULL REFERENCES sites(id) ON DELETE CASCADE,
                    snapshot_date TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    stats_json TEXT NOT NULL,
                    PRIMARY KEY (site_id, snapshot_date)
                );

                CREATE TABLE IF NOT EXISTS ptd_backup_settings (
                    id INTEGER PRIMARY KEY CHECK (id = 1),
                    enabled INTEGER NOT NULL DEFAULT 0,
                    webdav_url TEXT NOT NULL DEFAULT '',
                    username TEXT NOT NULL DEFAULT '',
                    password TEXT NOT NULL DEFAULT '',
                    use_proxy INTEGER NOT NULL DEFAULT 0,
                    backup_interval_hours INTEGER NOT NULL DEFAULT 24,
                    site_mappings_json TEXT NOT NULL DEFAULT '{}',
                    last_backup_at TEXT,
                    last_backup_filename TEXT,
                    last_error TEXT,
                    updated_at TEXT NOT NULL
                );

                INSERT OR IGNORE INTO ptd_backup_settings
                    (id, enabled, webdav_url, username, password, use_proxy,
                     backup_interval_hours, site_mappings_json, updated_at)
                VALUES (1, 0, '', '', '', 0, 24, '{}', CURRENT_TIMESTAMP);

                CREATE TABLE IF NOT EXISTS downloaders (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    name TEXT NOT NULL UNIQUE,
                    downloader_type TEXT NOT NULL,
                    url TEXT NOT NULL,
                    username TEXT NOT NULL DEFAULT '',
                    password TEXT NOT NULL DEFAULT '',
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS brush_tasks (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    name TEXT NOT NULL UNIQUE,
                    cron_expression TEXT NOT NULL,
                    site_id INTEGER REFERENCES sites(id) ON DELETE SET NULL,
                    downloader_ids TEXT NOT NULL DEFAULT '[]',
                    tag TEXT NOT NULL,
                    rss_url TEXT NOT NULL,
                    seed_volume_gb REAL,
                    save_dir TEXT,
                    active_time_windows TEXT,
                    promotion TEXT NOT NULL DEFAULT 'all',
                    skip_hit_and_run INTEGER NOT NULL DEFAULT 1,
                    max_concurrent INTEGER NOT NULL DEFAULT 100,
                    download_speed_limit INTEGER,
                    upload_speed_limit INTEGER,
                    size_ranges TEXT,
                    seeder_ranges TEXT,
                    min_free_hours REAL,
                    delete_mode TEXT NOT NULL DEFAULT 'or',
                    delete_on_free_expiry INTEGER NOT NULL DEFAULT 0,
                    min_seed_time_hours REAL,
                    hr_min_seed_time_hours REAL,
                    target_ratio REAL,
                    max_upload_gb REAL,
                    download_timeout_hours REAL,
                    min_avg_upload_speed_kbs REAL,
                    max_inactive_hours REAL,
                    min_disk_space_gb REAL,
                    enabled INTEGER NOT NULL DEFAULT 1,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS brush_task_torrents (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    task_id INTEGER NOT NULL REFERENCES brush_tasks(id) ON DELETE CASCADE,
                    torrent_id TEXT,
                    torrent_link TEXT,
                    torrent_hash TEXT NOT NULL,
                    torrent_name TEXT NOT NULL,
                    added_at TEXT NOT NULL,
                    size_bytes INTEGER,
                    is_hr INTEGER NOT NULL DEFAULT 0,
                    free_end_timestamp INTEGER,
                    status TEXT NOT NULL DEFAULT 'active',
                    removed_at TEXT,
                    remove_reason TEXT,
                    uploaded_bytes INTEGER NOT NULL DEFAULT 0,
                    downloaded_bytes INTEGER NOT NULL DEFAULT 0,
                    download_duration_secs INTEGER NOT NULL DEFAULT 0,
                    avg_upload_speed REAL NOT NULL DEFAULT 0,
                    ratio REAL NOT NULL DEFAULT 0,
                    last_stats_at TEXT,
                    downloader_id INTEGER REFERENCES downloaders(id) ON DELETE SET NULL,
                    UNIQUE(task_id, torrent_hash)
                );

                CREATE TABLE IF NOT EXISTS task_stats_snapshots (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    task_id INTEGER NOT NULL REFERENCES brush_tasks(id) ON DELETE CASCADE,
                    total_uploaded INTEGER NOT NULL,
                    total_downloaded INTEGER NOT NULL,
                    torrent_count INTEGER NOT NULL,
                    recorded_at TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS torrent_traffic (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    task_id INTEGER NOT NULL,
                    torrent_hash TEXT NOT NULL,
                    uploaded_bytes INTEGER NOT NULL,
                    downloaded_bytes INTEGER NOT NULL,
                    recorded_at TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS downloader_speed_snapshots (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    downloader_id INTEGER NOT NULL REFERENCES downloaders(id) ON DELETE CASCADE,
                    upload_speed INTEGER NOT NULL,
                    download_speed INTEGER NOT NULL,
                    recorded_at TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS sign_in_tasks (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    name TEXT NOT NULL UNIQUE,
                    site_id INTEGER NOT NULL REFERENCES sites(id) ON DELETE CASCADE,
                    cron_expression TEXT NOT NULL,
                    lightpanda_endpoint TEXT,
                    lightpanda_token TEXT NOT NULL,
                    lightpanda_region TEXT NOT NULL DEFAULT 'euwest',
                    browser TEXT NOT NULL DEFAULT 'lightpanda',
                    proxy TEXT NOT NULL DEFAULT 'fast_dc',
                    country TEXT,
                    sign_in_method TEXT NOT NULL DEFAULT 'open_page',
                    browserless_selector TEXT NOT NULL DEFAULT 'input[type=''submit'']',
                    browserless_cf_mode TEXT NOT NULL DEFAULT 'auto',
                    browserless_wait_ms INTEGER,
                    browserless_solve_timeout INTEGER,
                    browserless_action_timeout INTEGER,
                    browserless_post_click_wait_ms INTEGER,
                    enabled INTEGER NOT NULL DEFAULT 1,
                    last_status TEXT,
                    last_message TEXT,
                    last_run_at TEXT,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS sign_in_records (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    task_id INTEGER NOT NULL REFERENCES sign_in_tasks(id) ON DELETE CASCADE,
                    site_id INTEGER NOT NULL,
                    site_name TEXT NOT NULL,
                    started_at TEXT NOT NULL,
                    finished_at TEXT NOT NULL,
                    status TEXT NOT NULL,
                    message TEXT NOT NULL
                );

                CREATE INDEX IF NOT EXISTS idx_brush_task_torrents_task ON brush_task_torrents(task_id, status);
                CREATE INDEX IF NOT EXISTS idx_site_stats_checked_at ON site_stats(last_checked_at);
                CREATE INDEX IF NOT EXISTS idx_site_stats_history_updated_at ON site_stats_history(updated_at);
                CREATE INDEX IF NOT EXISTS idx_task_stats_task ON task_stats_snapshots(task_id, recorded_at);
                CREATE INDEX IF NOT EXISTS idx_torrent_traffic_lookup ON torrent_traffic(task_id, torrent_hash, recorded_at);
                CREATE TABLE IF NOT EXISTS system_snapshots (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    process_cpu_usage REAL NOT NULL,
                    process_memory_bytes INTEGER NOT NULL,
                    system_cpu_usage REAL NOT NULL,
                    system_total_memory_bytes INTEGER NOT NULL,
                    system_used_memory_bytes INTEGER NOT NULL,
                    system_available_memory_bytes INTEGER NOT NULL,
                    recorded_at TEXT NOT NULL
                );

                CREATE INDEX IF NOT EXISTS idx_downloader_speed_snapshots_lookup ON downloader_speed_snapshots(downloader_id, recorded_at);
                CREATE INDEX IF NOT EXISTS idx_sign_in_records_lookup ON sign_in_records(task_id, finished_at);
                -- 保留期清理按 recorded_at 范围删除，需单列索引才能避免全表扫描。
                CREATE INDEX IF NOT EXISTS idx_torrent_traffic_recorded_at ON torrent_traffic(recorded_at);
                CREATE INDEX IF NOT EXISTS idx_task_stats_recorded_at ON task_stats_snapshots(recorded_at);
                CREATE INDEX IF NOT EXISTS idx_downloader_speed_recorded_at ON downloader_speed_snapshots(recorded_at);
                CREATE INDEX IF NOT EXISTS idx_system_snapshots_recorded_at ON system_snapshots(recorded_at);
                ",
            )
            .map_err(sql_error)?;

            ensure_column(
                &conn,
                "site_stats",
                "details_json",
                "ALTER TABLE site_stats ADD COLUMN details_json TEXT NOT NULL DEFAULT '{}'",
            )?;
            backfill_site_stats_history(&conn)?;

            ensure_column(
                &conn,
                "brush_task_torrents",
                "torrent_id",
                "ALTER TABLE brush_task_torrents ADD COLUMN torrent_id TEXT",
            )?;
            ensure_column(
                &conn,
                "brush_task_torrents",
                "torrent_link",
                "ALTER TABLE brush_task_torrents ADD COLUMN torrent_link TEXT",
            )?;
            ensure_column(
                &conn,
                "brush_task_torrents",
                "uploaded_bytes",
                "ALTER TABLE brush_task_torrents ADD COLUMN uploaded_bytes INTEGER NOT NULL DEFAULT 0",
            )?;
            ensure_column(
                &conn,
                "brush_task_torrents",
                "downloaded_bytes",
                "ALTER TABLE brush_task_torrents ADD COLUMN downloaded_bytes INTEGER NOT NULL DEFAULT 0",
            )?;
            ensure_column(
                &conn,
                "brush_task_torrents",
                "download_duration_secs",
                "ALTER TABLE brush_task_torrents ADD COLUMN download_duration_secs INTEGER NOT NULL DEFAULT 0",
            )?;
            ensure_column(
                &conn,
                "brush_task_torrents",
                "avg_upload_speed",
                "ALTER TABLE brush_task_torrents ADD COLUMN avg_upload_speed REAL NOT NULL DEFAULT 0",
            )?;
            ensure_column(
                &conn,
                "brush_task_torrents",
                "ratio",
                "ALTER TABLE brush_task_torrents ADD COLUMN ratio REAL NOT NULL DEFAULT 0",
            )?;
            ensure_column(
                &conn,
                "brush_task_torrents",
                "last_stats_at",
                "ALTER TABLE brush_task_torrents ADD COLUMN last_stats_at TEXT",
            )?;
            ensure_column(
                &conn,
                "brush_task_torrents",
                "free_end_timestamp",
                "ALTER TABLE brush_task_torrents ADD COLUMN free_end_timestamp INTEGER",
            )?;
            ensure_column(
                &conn,
                "brush_tasks",
                "min_free_hours",
                "ALTER TABLE brush_tasks ADD COLUMN min_free_hours REAL",
            )?;
            ensure_column(
                &conn,
                "brush_tasks",
                "delete_on_free_expiry",
                "ALTER TABLE brush_tasks ADD COLUMN delete_on_free_expiry INTEGER NOT NULL DEFAULT 0",
            )?;
            ensure_column(
                &conn,
                "brush_tasks",
                "site_id",
                "ALTER TABLE brush_tasks ADD COLUMN site_id INTEGER REFERENCES sites(id) ON DELETE SET NULL",
            )?;
            ensure_column(
                &conn,
                "brush_tasks",
                "downloader_ranges",
                "ALTER TABLE brush_tasks ADD COLUMN downloader_ranges TEXT",
            )?;
            ensure_column(
                &conn,
                "brush_tasks",
                "last_run_info",
                "ALTER TABLE brush_tasks ADD COLUMN last_run_info TEXT",
            )?;
            ensure_column(
                &conn,
                "downloaders",
                "weight",
                "ALTER TABLE downloaders ADD COLUMN weight INTEGER NOT NULL DEFAULT 1",
            )?;
            ensure_column(
                &conn,
                "sign_in_tasks",
                "sign_in_method",
                "ALTER TABLE sign_in_tasks ADD COLUMN sign_in_method TEXT NOT NULL DEFAULT 'open_page'",
            )?;
            ensure_column(
                &conn,
                "global_settings",
                "proxy",
                "ALTER TABLE global_settings ADD COLUMN proxy TEXT",
            )?;
            ensure_column(
                &conn,
                "global_settings",
                "use_proxy_for_lightpanda",
                "ALTER TABLE global_settings ADD COLUMN use_proxy_for_lightpanda INTEGER NOT NULL DEFAULT 1",
            )?;
            ensure_column(
                &conn,
                "global_settings",
                "tag_rule_scan_interval_mins",
                "ALTER TABLE global_settings ADD COLUMN tag_rule_scan_interval_mins INTEGER NOT NULL DEFAULT 7",
            )?;
            ensure_column(
                &conn,
                "global_settings",
                "ocr_api_key",
                "ALTER TABLE global_settings ADD COLUMN ocr_api_key TEXT",
            )?;
            for (column, sql) in [
                (
                    "lightpanda_endpoint",
                    "ALTER TABLE global_settings ADD COLUMN lightpanda_endpoint TEXT",
                ),
                (
                    "lightpanda_token",
                    "ALTER TABLE global_settings ADD COLUMN lightpanda_token TEXT",
                ),
                (
                    "lightpanda_region",
                    "ALTER TABLE global_settings ADD COLUMN lightpanda_region TEXT NOT NULL DEFAULT 'euwest'",
                ),
                (
                    "lightpanda_browser",
                    "ALTER TABLE global_settings ADD COLUMN lightpanda_browser TEXT NOT NULL DEFAULT 'lightpanda'",
                ),
                (
                    "lightpanda_proxy",
                    "ALTER TABLE global_settings ADD COLUMN lightpanda_proxy TEXT",
                ),
                (
                    "lightpanda_country",
                    "ALTER TABLE global_settings ADD COLUMN lightpanda_country TEXT",
                ),
                (
                    "browserless_address",
                    "ALTER TABLE global_settings ADD COLUMN browserless_address TEXT",
                ),
                (
                    "browserless_token",
                    "ALTER TABLE global_settings ADD COLUMN browserless_token TEXT",
                ),
                (
                    "sign_in_browser_config_migrated",
                    "ALTER TABLE global_settings ADD COLUMN sign_in_browser_config_migrated INTEGER NOT NULL DEFAULT 0",
                ),
            ] {
                ensure_column(&conn, "global_settings", column, sql)?;
            }
            for (column, sql) in [
                (
                    "browserless_selector",
                    "ALTER TABLE sign_in_tasks ADD COLUMN browserless_selector TEXT NOT NULL DEFAULT 'input[type=''submit'']'",
                ),
                (
                    "browserless_cf_mode",
                    "ALTER TABLE sign_in_tasks ADD COLUMN browserless_cf_mode TEXT NOT NULL DEFAULT 'auto'",
                ),
                (
                    "browserless_wait_ms",
                    "ALTER TABLE sign_in_tasks ADD COLUMN browserless_wait_ms INTEGER",
                ),
                (
                    "browserless_solve_timeout",
                    "ALTER TABLE sign_in_tasks ADD COLUMN browserless_solve_timeout INTEGER",
                ),
                (
                    "browserless_action_timeout",
                    "ALTER TABLE sign_in_tasks ADD COLUMN browserless_action_timeout INTEGER",
                ),
                (
                    "browserless_post_click_wait_ms",
                    "ALTER TABLE sign_in_tasks ADD COLUMN browserless_post_click_wait_ms INTEGER",
                ),
                ("attendance_path", "ALTER TABLE sign_in_tasks ADD COLUMN attendance_path TEXT NOT NULL DEFAULT '/attendance.php'"),
                ("captcha_selector", "ALTER TABLE sign_in_tasks ADD COLUMN captcha_selector TEXT NOT NULL DEFAULT ''"),
                ("captcha_input_selector", "ALTER TABLE sign_in_tasks ADD COLUMN captcha_input_selector TEXT NOT NULL DEFAULT ''"),
                ("already_keywords", "ALTER TABLE sign_in_tasks ADD COLUMN already_keywords TEXT NOT NULL DEFAULT ''"),
                ("submit_method", "ALTER TABLE sign_in_tasks ADD COLUMN submit_method TEXT NOT NULL DEFAULT 'click'"),
                ("result_rules", "ALTER TABLE sign_in_tasks ADD COLUMN result_rules TEXT NOT NULL DEFAULT '[]'"),
            ] {
                ensure_column(&conn, "sign_in_tasks", column, sql)?;
            }
            let sites_had_request_headers = column_exists(&conn, "sites", "request_headers");
            ensure_column(
                &conn,
                "sites",
                "use_proxy",
                "ALTER TABLE sites ADD COLUMN use_proxy INTEGER NOT NULL DEFAULT 1",
            )?;
            ensure_column(
                &conn,
                "sites",
                "request_headers",
                "ALTER TABLE sites ADD COLUMN request_headers TEXT NOT NULL DEFAULT '[]'",
            )?;
            if !sites_had_request_headers {
                let defaults = serde_json::to_string(&default_site_request_headers()).map_err(
                    |error| {
                        sql_error(rusqlite::Error::ToSqlConversionFailure(Box::new(error)))
                    },
                )?;
                conn.execute("UPDATE sites SET request_headers = ?1", [defaults])
                    .map_err(sql_error)?;
            }

            search::migrate_site_names(&conn)?;

            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS tag_rules (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    name TEXT NOT NULL UNIQUE,
                    tag_name TEXT NOT NULL,
                    match_rules TEXT NOT NULL,
                    enabled INTEGER NOT NULL DEFAULT 1,
                    downloader_ids TEXT,
                    tagged_torrent_count INTEGER NOT NULL DEFAULT 0,
                    tagged_total_size INTEGER NOT NULL DEFAULT 0,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );",
            )
            .map_err(sql_error)?;

            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS media_settings (
                    id INTEGER PRIMARY KEY CHECK (id = 1),
                    tmdb_token TEXT,
                    tmdb_language TEXT NOT NULL DEFAULT 'zh-CN',
                    scan_interval_mins INTEGER NOT NULL DEFAULT 30,
                    max_search_queries INTEGER NOT NULL DEFAULT 8,
                    search_concurrency INTEGER NOT NULL DEFAULT 4,
                    updated_at TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS quality_profiles (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    name TEXT NOT NULL UNIQUE,
                    resolution_order TEXT NOT NULL,
                    allowed_resolutions TEXT NOT NULL,
                    blocked_resolutions TEXT NOT NULL,
                    source_order TEXT NOT NULL,
                    allowed_sources TEXT NOT NULL,
                    codec_order TEXT NOT NULL,
                    blocked_codecs TEXT NOT NULL,
                    allow_unknown_quality INTEGER NOT NULL DEFAULT 0,
                    minimum_score INTEGER NOT NULL DEFAULT 80,
                    min_seeders INTEGER NOT NULL DEFAULT 1,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS subscriptions (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    tmdb_id INTEGER NOT NULL,
                    media_type TEXT NOT NULL CHECK (media_type IN ('tv', 'movie')),
                    tmdb_is_animation INTEGER NOT NULL DEFAULT 0,
                    tmdb_genres_json TEXT NOT NULL DEFAULT '[]',
                    title TEXT NOT NULL,
                    original_title TEXT,
                    aliases_json TEXT NOT NULL DEFAULT '[]',
                    year INTEGER,
                    poster_path TEXT,
                    season INTEGER,
                    next_episode INTEGER,
                    start_episode INTEGER,
                    absolute_episode INTEGER,
                    quality_profile_id INTEGER NOT NULL REFERENCES quality_profiles(id) ON DELETE RESTRICT,
                    downloader_id INTEGER NOT NULL REFERENCES downloaders(id) ON DELETE RESTRICT,
                    save_path TEXT,
                    enabled INTEGER NOT NULL DEFAULT 1,
                    next_run_at TEXT NOT NULL,
                    lease_owner TEXT,
                    lease_until TEXT,
                    version INTEGER NOT NULL DEFAULT 0,
                    last_status TEXT,
                    last_error TEXT,
                    last_run_at TEXT,
                    last_run_info TEXT,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS subscription_sites (
                    subscription_id INTEGER NOT NULL REFERENCES subscriptions(id) ON DELETE CASCADE,
                    site_id INTEGER NOT NULL REFERENCES sites(id) ON DELETE RESTRICT,
                    priority INTEGER NOT NULL DEFAULT 0,
                    PRIMARY KEY (subscription_id, site_id)
                );

                CREATE TABLE IF NOT EXISTS subscription_targets (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    subscription_id INTEGER NOT NULL REFERENCES subscriptions(id) ON DELETE CASCADE,
                    target_key TEXT NOT NULL,
                    season INTEGER,
                    episode INTEGER,
                    absolute_episode INTEGER,
                    air_date TEXT,
                    status TEXT NOT NULL DEFAULT 'pending',
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    UNIQUE(subscription_id, target_key)
                );

                CREATE TABLE IF NOT EXISTS media_downloads (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    subscription_id INTEGER REFERENCES subscriptions(id) ON DELETE SET NULL,
                    target_key TEXT NOT NULL,
                    dedupe_key TEXT NOT NULL UNIQUE,
                    site_id INTEGER REFERENCES sites(id) ON DELETE SET NULL,
                    downloader_id INTEGER REFERENCES downloaders(id) ON DELETE SET NULL,
                    source_site TEXT NOT NULL,
                    downloader_name TEXT NOT NULL,
                    torrent_id TEXT NOT NULL,
                    title TEXT NOT NULL,
                    size INTEGER NOT NULL DEFAULT 0,
                    release_json TEXT NOT NULL,
                    decision_json TEXT NOT NULL,
                    profile_snapshot_json TEXT NOT NULL,
                    infohash TEXT,
                    status TEXT NOT NULL DEFAULT 'queued',
                    attempts INTEGER NOT NULL DEFAULT 0,
                    next_attempt_at TEXT,
                    lease_owner TEXT,
                    lease_until TEXT,
                    version INTEGER NOT NULL DEFAULT 0,
                    last_error TEXT,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    submitted_at TEXT
                );

                CREATE UNIQUE INDEX IF NOT EXISTS idx_subscriptions_media_season
                    ON subscriptions(media_type, tmdb_id, COALESCE(season, -1));
                CREATE INDEX IF NOT EXISTS idx_subscriptions_due
                    ON subscriptions(enabled, next_run_at, lease_until);
                CREATE INDEX IF NOT EXISTS idx_subscription_targets_status
                    ON subscription_targets(subscription_id, status);
                CREATE INDEX IF NOT EXISTS idx_media_downloads_due
                    ON media_downloads(status, next_attempt_at, lease_until);
                CREATE INDEX IF NOT EXISTS idx_media_downloads_subscription
                    ON media_downloads(subscription_id, created_at);

                CREATE TABLE IF NOT EXISTS openlist_settings (
                    id INTEGER PRIMARY KEY CHECK (id = 1),
                    base_url TEXT NOT NULL DEFAULT '',
                    api_key TEXT NOT NULL DEFAULT '',
                    enabled INTEGER NOT NULL DEFAULT 0,
                    target_directory_id INTEGER,
                    scan_interval_secs INTEGER NOT NULL DEFAULT 30,
                    updated_at TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS openlist_path_mappings (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    downloader_id INTEGER NOT NULL REFERENCES downloaders(id) ON DELETE CASCADE,
                    qb_path TEXT NOT NULL,
                    openlist_path TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    UNIQUE(downloader_id, qb_path),
                    UNIQUE(downloader_id, openlist_path)
                );

                CREATE TABLE IF NOT EXISTS openlist_target_directories (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    name TEXT NOT NULL UNIQUE,
                    downloader_id INTEGER NOT NULL REFERENCES downloaders(id) ON DELETE CASCADE,
                    openlist_path TEXT NOT NULL,
                    qb_path TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    UNIQUE(downloader_id, openlist_path),
                    UNIQUE(downloader_id, qb_path)
                );

                CREATE TABLE IF NOT EXISTS media_relocation_jobs (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    media_download_id INTEGER UNIQUE REFERENCES media_downloads(id) ON DELETE CASCADE,
                    downloader_id INTEGER REFERENCES downloaders(id) ON DELETE SET NULL,
                    infohash TEXT NOT NULL,
                    source_qb_path TEXT NOT NULL,
                    source_openlist_path TEXT NOT NULL,
                    source_content_openlist_path TEXT NOT NULL DEFAULT '',
                    target_openlist_path TEXT NOT NULL,
                    target_qb_path TEXT NOT NULL,
                    target_content_qb_path TEXT NOT NULL DEFAULT '',
                    target_downloader_id INTEGER REFERENCES downloaders(id) ON DELETE SET NULL,
                    copy_items_json TEXT NOT NULL DEFAULT '[]',
                    source_files_json TEXT NOT NULL DEFAULT '[]',
                    source_manifest_json TEXT NOT NULL DEFAULT '[]',
                    copy_checkpoint_json TEXT,
                    copy_lock_acquired INTEGER NOT NULL DEFAULT 0,
                    manifest_cursor INTEGER NOT NULL DEFAULT 0,
                    target_root_folder INTEGER,
                    manual_requested_at TEXT,
                    stage_started_at TEXT NOT NULL DEFAULT '',
                    torrent_name TEXT NOT NULL,
                    stage TEXT NOT NULL DEFAULT 'waiting_download',
                    openlist_task_id TEXT,
                    torrent_data BLOB,
                    attempts INTEGER NOT NULL DEFAULT 0,
                    next_attempt_at TEXT,
                    lease_owner TEXT,
                    lease_until TEXT,
                    version INTEGER NOT NULL DEFAULT 0,
                    last_error TEXT,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    completed_at TEXT,
                    cleared_at TEXT
                );

                CREATE INDEX IF NOT EXISTS idx_media_relocation_jobs_due
                    ON media_relocation_jobs(stage, next_attempt_at, lease_until);
                ",
            )
            .map_err(sql_error)?;

            ensure_column(
                &conn,
                "media_relocation_jobs",
                "source_content_openlist_path",
                "ALTER TABLE media_relocation_jobs ADD COLUMN source_content_openlist_path TEXT NOT NULL DEFAULT ''",
            )?;
            ensure_column(
                &conn,
                "media_relocation_jobs",
                "target_content_qb_path",
                "ALTER TABLE media_relocation_jobs ADD COLUMN target_content_qb_path TEXT NOT NULL DEFAULT ''",
            )?;
            ensure_column(
                &conn,
                "media_relocation_jobs",
                "target_downloader_id",
                "ALTER TABLE media_relocation_jobs ADD COLUMN target_downloader_id INTEGER REFERENCES downloaders(id) ON DELETE SET NULL",
            )?;
            ensure_column(
                &conn,
                "media_relocation_jobs",
                "copy_items_json",
                "ALTER TABLE media_relocation_jobs ADD COLUMN copy_items_json TEXT NOT NULL DEFAULT '[]'",
            )?;
            ensure_column(
                &conn,
                "media_relocation_jobs",
                "source_files_json",
                "ALTER TABLE media_relocation_jobs ADD COLUMN source_files_json TEXT NOT NULL DEFAULT '[]'",
            )?;
            ensure_column(
                &conn,
                "media_relocation_jobs",
                "source_manifest_json",
                "ALTER TABLE media_relocation_jobs ADD COLUMN source_manifest_json TEXT NOT NULL DEFAULT '[]'",
            )?;
            ensure_column(
                &conn,
                "media_relocation_jobs",
                "copy_checkpoint_json",
                "ALTER TABLE media_relocation_jobs ADD COLUMN copy_checkpoint_json TEXT",
            )?;
            ensure_column(
                &conn,
                "media_relocation_jobs",
                "copy_lock_acquired",
                "ALTER TABLE media_relocation_jobs ADD COLUMN copy_lock_acquired INTEGER NOT NULL DEFAULT 0",
            )?;
            ensure_column(
                &conn,
                "media_relocation_jobs",
                "manifest_cursor",
                "ALTER TABLE media_relocation_jobs ADD COLUMN manifest_cursor INTEGER NOT NULL DEFAULT 0",
            )?;
            ensure_column(
                &conn,
                "media_relocation_jobs",
                "target_root_folder",
                "ALTER TABLE media_relocation_jobs ADD COLUMN target_root_folder INTEGER",
            )?;
            ensure_column(
                &conn,
                "media_relocation_jobs",
                "manual_requested_at",
                "ALTER TABLE media_relocation_jobs ADD COLUMN manual_requested_at TEXT",
            )?;
            ensure_column(
                &conn,
                "media_relocation_jobs",
                "stage_started_at",
                "ALTER TABLE media_relocation_jobs ADD COLUMN stage_started_at TEXT NOT NULL DEFAULT ''",
            )?;
            conn.execute(
                "UPDATE media_relocation_jobs
                 SET stage_started_at=COALESCE(NULLIF(updated_at, ''), created_at)
                 WHERE stage_started_at=''",
                [],
            )
            .map_err(sql_error)?;
            migrate_media_relocation_jobs_for_manual_transfers(&conn)?;
            ensure_column(
                &conn,
                "media_relocation_jobs",
                "cleared_at",
                "ALTER TABLE media_relocation_jobs ADD COLUMN cleared_at TEXT",
            )?;
            let legacy_auto_boundary_at = Utc::now().to_rfc3339();
            conn.execute(
                "UPDATE media_relocation_jobs
                 SET stage='completed', openlist_task_id=NULL,
                     copy_checkpoint_json=NULL, copy_lock_acquired=0,
                     lease_owner=NULL, lease_until=NULL, attempts=0, next_attempt_at=NULL,
                     manual_requested_at=NULL,
                     last_error='历史自动复制任务的 qB 后续已停止；自动追剧不会继续接管 qB 或清理源文件。旧流程可能已移除源 qB 任务，若存在 torrent_data 已保留供人工恢复',
                     completed_at=COALESCE(completed_at, ?), stage_started_at=?, updated_at=?,
                     version=version+1
                 WHERE media_download_id IS NOT NULL
                   AND stage IN ('copy_verified', 'qb_reconcile', 'torrent_exported',
                                 'source_qb_removed', 'target_qb_submitted',
                                 'target_qb_check_requested', 'target_qb_checking',
                                 'target_qb_starting', 'qb_manual_review',
                                 'source_removing', 'source_remove_manual_review',
                                 'source_removed')",
                params![
                    legacy_auto_boundary_at,
                    legacy_auto_boundary_at,
                    legacy_auto_boundary_at
                ],
            )
            .map_err(sql_error)?;

            migrate_media_download_infohash_uniqueness(&conn)?;

            ensure_column(
                &conn,
                "subscriptions",
                "last_run_info",
                "ALTER TABLE subscriptions ADD COLUMN last_run_info TEXT",
            )?;
            let had_tmdb_animation_classification =
                column_exists(&conn, "subscriptions", "tmdb_is_animation");
            ensure_column(
                &conn,
                "subscriptions",
                "tmdb_is_animation",
                "ALTER TABLE subscriptions ADD COLUMN tmdb_is_animation INTEGER NOT NULL DEFAULT 0",
            )?;
            if !had_tmdb_animation_classification {
                let classification_refresh_at = Utc::now().to_rfc3339();
                conn.execute(
                    "UPDATE subscriptions
                     SET next_run_at = ?, last_status = NULL
                     WHERE media_type = 'tv' AND enabled = 1",
                    [&classification_refresh_at],
                )
                .map_err(sql_error)?;
            }
            let had_tmdb_genres = column_exists(&conn, "subscriptions", "tmdb_genres_json");
            ensure_column(&conn, "subscriptions", "tmdb_genres_json",
                "ALTER TABLE subscriptions ADD COLUMN tmdb_genres_json TEXT NOT NULL DEFAULT '[]'")?;
            if !had_tmdb_genres {
                let refresh_at = Utc::now().to_rfc3339();
                conn.execute("UPDATE subscriptions SET next_run_at = ?, last_status = NULL WHERE enabled = 1", [&refresh_at]).map_err(sql_error)?;
            }

            let media_now = Utc::now().to_rfc3339();
            conn.execute(
                "INSERT OR IGNORE INTO openlist_settings
                 (id, base_url, api_key, enabled, target_directory_id, scan_interval_secs, updated_at)
                 VALUES (1, '', '', 0, NULL, 30, ?)",
                [&media_now],
            )
            .map_err(sql_error)?;
            conn.execute(
                "INSERT OR IGNORE INTO media_settings
                 (id, tmdb_token, tmdb_language, scan_interval_mins, max_search_queries, search_concurrency, updated_at)
                 VALUES (1, NULL, 'zh-CN', 30, 8, 4, ?)",
                [&media_now],
            )
            .map_err(sql_error)?;
            // Replace the legacy built-in profiles once while preserving user-created rules.
            if column_exists(&conn, "subscriptions", "quality_profile_id") {
                conn.execute(
                    "UPDATE subscriptions SET quality_profile_id = 1
                     WHERE quality_profile_id IN (
                       SELECT id FROM quality_profiles
                       WHERE name IN ('4K 高画质', '1080p 均衡', '动漫优先', '省空间 HEVC', '影音优先', '省空间', '电影推荐', '动漫推荐')
                     )",
                    [],
                )
                .map_err(sql_error)?;
            }
            conn.execute(
                "DELETE FROM quality_profiles
                 WHERE id <> 1
                   AND name IN ('4K 高画质', '1080p 均衡', '动漫优先', '省空间 HEVC', '影音优先', '省空间', '电影推荐', '动漫推荐')",
                [],
            )
            .map_err(sql_error)?;
            conn.execute(
                "UPDATE quality_profiles
                 SET name = '电视剧 · 日常', resolution_order = '[\"1080p\",\"2160p\",\"720p\"]',
                     allowed_resolutions = '[\"2160p\",\"1080p\",\"720p\"]',
                     blocked_resolutions = '[\"480p\"]',
                     source_order = '[\"WEB-DL\",\"BluRay\",\"WEBRip\"]',
                     allowed_sources = '[\"WEB-DL\",\"BluRay\",\"WEBRip\"]',
                     codec_order = '[\"H265\",\"H264\",\"AV1\"]', blocked_codecs = '[]',
                     allow_unknown_quality = 0, minimum_score = 65, min_seeders = 1, updated_at = ?
                 WHERE id = 1 AND name IN ('高清优先', '日常均衡', '电视剧推荐')",
                [&media_now],
            )
            .map_err(sql_error)?;
            conn.execute(
                "UPDATE quality_profiles
                 SET resolution_order = '[\"2160p\",\"1080p\",\"720p\"]', updated_at = ?
                 WHERE name = '动漫 · 日常'",
                [&media_now],
            )
            .map_err(sql_error)?;
            conn.execute(
                "INSERT OR IGNORE INTO quality_profiles
                 (id, name, resolution_order, allowed_resolutions, blocked_resolutions,
                  source_order, allowed_sources, codec_order, blocked_codecs,
                  allow_unknown_quality, minimum_score, min_seeders, created_at, updated_at)
                 VALUES (1, '电视剧 · 日常', '[\"1080p\",\"2160p\",\"720p\"]',
                         '[\"2160p\",\"1080p\",\"720p\"]', '[\"480p\"]',
                         '[\"WEB-DL\",\"BluRay\",\"WEBRip\"]',
                         '[\"WEB-DL\",\"BluRay\",\"WEBRip\"]',
                         '[\"H265\",\"H264\",\"AV1\"]', '[]',
                         0, 65, 1, ?, ?)",
                params![media_now, media_now],
            )
            .map_err(sql_error)?;

            let quality_presets = [
                (
                    "电视剧 · 4K",
                    r#"["2160p","1080p"]"#,
                    r#"["2160p","1080p"]"#,
                    r#"["720p","480p"]"#,
                    r#"["WEB-DL","BluRay","WEBRip"]"#,
                    r#"["WEB-DL","BluRay","WEBRip"]"#,
                    r#"["H265","AV1","H264"]"#,
                    "[]",
                    0,
                    65,
                    1,
                ),
                (
                    "电影 · 收藏",
                    r#"["2160p","1080p"]"#,
                    r#"["2160p","1080p"]"#,
                    r#"["720p","480p"]"#,
                    r#"["REMUX","BluRay","WEB-DL"]"#,
                    r#"["REMUX","BluRay","WEB-DL"]"#,
                    r#"["H265","AV1","H264"]"#,
                    "[]",
                    0,
                    70,
                    1,
                ),
                (
                    "电影 · 均衡",
                    r#"["2160p","1080p","720p"]"#,
                    r#"["2160p","1080p","720p"]"#,
                    r#"["480p"]"#,
                    r#"["BluRay","WEB-DL","WEBRip"]"#,
                    r#"["BluRay","WEB-DL","WEBRip"]"#,
                    r#"["H265","H264","AV1"]"#,
                    "[]",
                    0,
                    65,
                    1,
                ),
                (
                    "动漫 · 日常",
                    r#"["1080p","2160p","720p"]"#,
                    r#"["2160p","1080p","720p"]"#,
                    r#"["480p"]"#,
                    r#"["BluRay","WEB-DL","WEBRip"]"#,
                    r#"["BluRay","WEB-DL","WEBRip"]"#,
                    r#"["H265","H264","AV1"]"#,
                    "[]",
                    1,
                    60,
                    1,
                ),
                (
                    "动漫 · 省空间",
                    r#"["1080p","720p"]"#,
                    r#"["1080p","720p"]"#,
                    r#"["2160p","480p"]"#,
                    r#"["WEB-DL","WEBRip","BluRay"]"#,
                    r#"["WEB-DL","WEBRip","BluRay"]"#,
                    r#"["H265","AV1","H264"]"#,
                    "[]",
                    1,
                    55,
                    1,
                ),
            ];
            for preset in quality_presets {
                conn.execute(
                    "INSERT INTO quality_profiles
                     (name, resolution_order, allowed_resolutions, blocked_resolutions,
                      source_order, allowed_sources, codec_order, blocked_codecs,
                      allow_unknown_quality, minimum_score, min_seeders, created_at, updated_at)
                     SELECT ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?
                     WHERE NOT EXISTS (SELECT 1 FROM quality_profiles WHERE name = ?)",
                    params![
                        preset.0, preset.1, preset.2, preset.3, preset.4, preset.5, preset.6,
                        preset.7, preset.8, preset.9, preset.10, media_now, media_now, preset.0
                    ],
                )
                .map_err(sql_error)?;
            }

            ensure_column(
                &conn,
                "tag_rules",
                "tagged_torrent_count",
                "ALTER TABLE tag_rules ADD COLUMN tagged_torrent_count INTEGER NOT NULL DEFAULT 0",
            )?;
            ensure_column(
                &conn,
                "tag_rules",
                "tagged_total_size",
                "ALTER TABLE tag_rules ADD COLUMN tagged_total_size INTEGER NOT NULL DEFAULT 0",
            )?;

            // ONE-TIME MIGRATION — remove next release: multi-qb task support
            ensure_column(
                &conn,
                "brush_task_torrents",
                "downloader_id",
                "ALTER TABLE brush_task_torrents ADD COLUMN downloader_id INTEGER REFERENCES downloaders(id) ON DELETE SET NULL",
            )?;
            ensure_column(
                &conn,
                "brush_tasks",
                "downloader_ids",
                "ALTER TABLE brush_tasks ADD COLUMN downloader_ids TEXT NOT NULL DEFAULT '[]'",
            )?;
            // 将权重从下载器迁移到刷流任务（按 qb 维度配置）
            ensure_column(
                &conn,
                "brush_tasks",
                "downloader_weights",
                "ALTER TABLE brush_tasks ADD COLUMN downloader_weights TEXT",
            )?;
            // Backfill: 从 downloaders.weight 回填到 brush_tasks.downloader_weights
            if column_exists(&conn, "downloaders", "weight") {
                conn.execute(
                    "UPDATE brush_tasks
                     SET downloader_weights = (
                         SELECT json_group_object(CAST(d.id AS TEXT), d.weight)
                         FROM downloaders d
                         WHERE d.id IN (SELECT value FROM json_each(brush_tasks.downloader_ids))
                     )
                     WHERE downloader_weights IS NULL
                       AND downloader_ids IS NOT NULL
                       AND downloader_ids != '[]'",
                    [],
                )
                .map_err(sql_error)?;
            }
            // Backfill + drop old columns. Only runs on databases that still
            // have the legacy `downloader_id` column (created before this change).
            if column_exists(&conn, "brush_tasks", "downloader_id") {
                conn.execute(
                    "UPDATE brush_task_torrents
                     SET downloader_id = (SELECT downloader_id FROM brush_tasks WHERE id = brush_task_torrents.task_id)
                     WHERE downloader_id IS NULL",
                    [],
                )
                .map_err(sql_error)?;
                conn.execute(
                    "UPDATE brush_tasks SET downloader_ids = json_array(downloader_id)",
                    [],
                )
                .map_err(sql_error)?;
                // save_dir: legacy absolute path -> {"<downloader_id>": "<path>"}
                conn.execute(
                    "UPDATE brush_tasks
                     SET save_dir = json_object(CAST(downloader_id AS TEXT), save_dir)
                     WHERE save_dir IS NOT NULL AND save_dir NOT LIKE '{%'",
                    [],
                )
                .map_err(sql_error)?;
                conn.execute("ALTER TABLE brush_tasks DROP COLUMN downloader_id", [])
                    .map_err(sql_error)?;
                // min_disk_space_gb stays on brush_tasks (task-level config)
            }

            conn.execute(
                "INSERT OR IGNORE INTO global_settings
                 (id, log_level, proxy, use_proxy_for_lightpanda,
                  tag_rule_scan_interval_mins, ocr_api_key,
                  lightpanda_endpoint, lightpanda_token, lightpanda_region, lightpanda_browser,
                  lightpanda_proxy, lightpanda_country, browserless_address, browserless_token,
                  sign_in_browser_config_migrated)
                 VALUES (1, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0)",
                params![
                    "info",
                    Option::<String>::None,
                    1,
                    7,
                    Option::<String>::None,
                    Option::<String>::None,
                    Option::<String>::None,
                    "euwest",
                    "lightpanda",
                    "fast_dc",
                    Option::<String>::None,
                    Option::<String>::None,
                    Option::<String>::None,
                ],
            )
            .map_err(sql_error)?;
            conn.execute_batch(
                "BEGIN IMMEDIATE;

                 UPDATE global_settings
                 SET lightpanda_endpoint = (
                         SELECT NULLIF(TRIM(lightpanda_endpoint), '')
                         FROM sign_in_tasks ORDER BY id LIMIT 1
                     ),
                     lightpanda_token = (
                         SELECT NULLIF(TRIM(lightpanda_token), '')
                         FROM sign_in_tasks ORDER BY id LIMIT 1
                     ),
                     lightpanda_region = COALESCE((
                         SELECT NULLIF(TRIM(lightpanda_region), '')
                         FROM sign_in_tasks ORDER BY id LIMIT 1
                     ), 'euwest'),
                     lightpanda_browser = COALESCE((
                         SELECT NULLIF(TRIM(browser), '')
                         FROM sign_in_tasks ORDER BY id LIMIT 1
                     ), 'lightpanda'),
                     lightpanda_proxy = CASE WHEN use_proxy_for_lightpanda != 0 THEN COALESCE((
                         SELECT NULLIF(TRIM(proxy), '')
                         FROM sign_in_tasks ORDER BY id LIMIT 1
                     ), 'fast_dc') ELSE NULL END,
                     lightpanda_country = (
                         SELECT NULLIF(TRIM(country), '')
                         FROM sign_in_tasks ORDER BY id LIMIT 1
                     )
                 WHERE id = 1 AND sign_in_browser_config_migrated = 0;

                 UPDATE sign_in_tasks
                 SET lightpanda_endpoint = NULL,
                     lightpanda_token = '',
                     lightpanda_region = 'euwest',
                     browser = 'lightpanda',
                     proxy = 'fast_dc',
                     country = NULL
                 WHERE EXISTS (
                     SELECT 1 FROM global_settings
                     WHERE id = 1 AND sign_in_browser_config_migrated = 0
                 );

                 UPDATE global_settings
                 SET sign_in_browser_config_migrated = 1
                 WHERE id = 1 AND sign_in_browser_config_migrated = 0;

                 COMMIT;",
            )
            .map_err(sql_error)?;
            conn.execute(
                "UPDATE sign_in_tasks SET browser = 'lightpanda'
                 WHERE browser NOT IN ('lightpanda', 'browserless')",
                [],
            )
            .map_err(sql_error)?;
            ensure_unique_sign_in_site(&conn)?;
            for column in [
                "cloakbrowser_license_key",
                "cloakbrowser_headless",
                "cloakbrowser_humanize",
                "cloakbrowser_human_preset",
                "cloakbrowser_proxy",
                "cloakbrowser_geoip",
            ] {
                if column_exists(&conn, "global_settings", column) {
                    conn.execute(
                        &format!("ALTER TABLE global_settings DROP COLUMN {column}"),
                        [],
                    )
                    .map_err(sql_error)?;
                }
            }
            Ok(())
        })
        .await
        .map_err(join_error)?
    }
}

fn row_to_brush_task(row: &rusqlite::Row<'_>) -> rusqlite::Result<BrushTaskRecord> {
    row_to_brush_task_at(row, 0)
}

fn row_to_brush_task_at(
    row: &rusqlite::Row<'_>,
    offset: usize,
) -> rusqlite::Result<BrushTaskRecord> {
    let downloader_ids_json: String = row.get(offset + 4)?;
    let downloader_ids: Vec<i64> = serde_json::from_str(&downloader_ids_json).unwrap_or_default();
    Ok(BrushTaskRecord {
        id: row.get(offset)?,
        name: row.get(offset + 1)?,
        cron_expression: row.get(offset + 2)?,
        site_id: row.get(offset + 3)?,
        downloader_ids,
        tag: row.get(offset + 5)?,
        rss_url: row.get(offset + 6)?,
        seed_volume_gb: row.get(offset + 7)?,
        save_dir: row.get(offset + 8)?,
        active_time_windows: row.get(offset + 9)?,
        promotion: row.get(offset + 10)?,
        skip_hit_and_run: row.get::<_, i32>(offset + 11)? != 0,
        max_concurrent: row.get(offset + 12)?,
        download_speed_limit: row.get(offset + 13)?,
        upload_speed_limit: row.get(offset + 14)?,
        size_ranges: row.get(offset + 15)?,
        seeder_ranges: row.get(offset + 16)?,
        min_free_hours: row.get(offset + 17)?,
        delete_mode: row.get(offset + 18)?,
        delete_on_free_expiry: row.get::<_, i32>(offset + 19)? != 0,
        min_seed_time_hours: row.get(offset + 20)?,
        hr_min_seed_time_hours: row.get(offset + 21)?,
        target_ratio: row.get(offset + 22)?,
        max_upload_gb: row.get(offset + 23)?,
        download_timeout_hours: row.get(offset + 24)?,
        min_avg_upload_speed_kbs: row.get(offset + 25)?,
        max_inactive_hours: row.get(offset + 26)?,
        min_disk_space_gb: row.get(offset + 27)?,
        enabled: row.get::<_, i32>(offset + 28)? != 0,
        created_at: row.get(offset + 29)?,
        updated_at: row.get(offset + 30)?,
        downloader_ranges: row.get(offset + 31)?,
        last_run_info: row.get(offset + 32)?,
        downloader_weights: row.get(offset + 33)?,
    })
}

fn map_sign_in_task(row: &rusqlite::Row<'_>) -> rusqlite::Result<SignInTaskRecord> {
    Ok(SignInTaskRecord {
        id: row.get(0)?,
        name: row.get(1)?,
        site_id: row.get(2)?,
        cron_expression: row.get(3)?,
        browser: row.get(4)?,
        sign_in_method: row.get(5)?,
        browserless: crate::sign_in::BrowserlessTaskConfig {
            selector: row.get(6)?,
            cf_mode: row.get(7)?,
            wait_ms: optional_u64(row, 8)?,
            solve_timeout: optional_u64(row, 9)?,
            action_timeout: optional_u64(row, 10)?,
            post_click_wait_ms: optional_u64(row, 11)?,
            attendance_path: row.get(18)?,
            captcha_selector: row.get(19)?,
            captcha_input_selector: row.get(20)?,
            already_keywords: row.get(21)?,
            submit_method: row.get(22)?,
            result_rules: serde_json::from_str(&row.get::<_, String>(23)?).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    23,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })?,
        },
        enabled: row.get::<_, i32>(12)? != 0,
        last_status: row.get(13)?,
        last_message: row.get(14)?,
        last_run_at: row.get(15)?,
        created_at: row.get(16)?,
        updated_at: row.get(17)?,
    })
}

fn optional_u64(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<Option<u64>> {
    Ok(row
        .get::<_, Option<i64>>(index)?
        .and_then(|value| u64::try_from(value).ok()))
}

fn map_sign_in_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<SignInRecord> {
    Ok(SignInRecord {
        id: row.get(0)?,
        task_id: row.get(1)?,
        site_id: row.get(2)?,
        site_name: row.get(3)?,
        started_at: row.get(4)?,
        finished_at: row.get(5)?,
        status: row.get(6)?,
        message: row.get(7)?,
    })
}

fn map_brush_torrent_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<BrushTorrentRecord> {
    Ok(BrushTorrentRecord {
        id: row.get(0)?,
        task_id: row.get(1)?,
        torrent_id: row.get(2)?,
        torrent_link: row.get(3)?,
        torrent_hash: row.get(4)?,
        torrent_name: row.get(5)?,
        added_at: row.get(6)?,
        size_bytes: row.get(7)?,
        is_hr: row.get::<_, i32>(8)? != 0,
        free_end_timestamp: row.get(9)?,
        status: row.get(10)?,
        removed_at: row.get(11)?,
        remove_reason: row.get(12)?,
        uploaded_bytes: row.get(13)?,
        downloaded_bytes: row.get(14)?,
        download_duration_secs: row.get(15)?,
        avg_upload_speed: row.get(16)?,
        ratio: row.get(17)?,
        last_stats_at: row.get(18)?,
        downloader_id: row.get(19)?,
    })
}

fn row_to_tag_rule(row: &rusqlite::Row<'_>) -> rusqlite::Result<crate::tag_rule::TagRuleRecord> {
    Ok(crate::tag_rule::TagRuleRecord {
        id: row.get(0)?,
        name: row.get(1)?,
        tag_name: row.get(2)?,
        match_rules: row.get(3)?,
        enabled: row.get::<_, i32>(4)? != 0,
        downloader_ids: row.get(5)?,
        tagged_torrent_count: row.get(6)?,
        tagged_total_size: row.get(7)?,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
    })
}

fn downloader_has_active_relocation(
    conn: &Connection,
    id: i64,
    now: &str,
) -> Result<bool, AppError> {
    conn.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM media_relocation_jobs
             WHERE (downloader_id = ?1 OR target_downloader_id = ?1)
               AND (stage NOT IN ('completed', 'cancelled')
                    OR (lease_until IS NOT NULL AND lease_until >= ?2))
         )",
        params![id, now],
        |row| row.get(0),
    )
    .map_err(sql_error)
}

fn downloader_has_active_qb_work(conn: &Connection, id: i64, now: &str) -> Result<bool, AppError> {
    if downloader_has_active_relocation(conn, id, now)? {
        return Ok(true);
    }
    let rss_reference:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM rss_download_jobs WHERE downloader_id=?1
        AND (status NOT IN ('submitted','already_present','failed','cancelled') OR reserved_bytes>0 OR lease_until>=?2))
        OR EXISTS(SELECT 1 FROM rss_rules WHERE downloader_id=?1 AND enabled=1 AND archived_at IS NULL)",params![id,now],|row|row.get(0)).map_err(sql_error)?;
    if rss_reference {
        return Ok(true);
    }
    conn.query_row(
        "SELECT EXISTS(
             SELECT 1
             FROM media_downloads AS download
             WHERE download.downloader_id = ?1
               AND (
                   download.status NOT IN ('submitted', 'failed', 'cancelled')
                   OR (download.lease_until IS NOT NULL AND download.lease_until >= ?2)
                   OR (
                       download.status = 'failed'
                       AND length(trim(download.infohash)) > 0
                       AND download.subscription_id IS NOT NULL
                       AND EXISTS(
                           SELECT 1 FROM subscription_targets AS target
                           WHERE target.subscription_id = download.subscription_id
                             AND target.target_key = download.target_key
                             AND target.status = 'queued'
                       )
                   )
               )
         )",
        params![id, now],
        |row| row.get(0),
    )
    .map_err(sql_error)
}

fn site_has_rss_reference(conn: &Connection, id: i64) -> Result<bool, AppError> {
    conn.query_row("SELECT EXISTS(SELECT 1 FROM rss_feeds f JOIN rss_rule_feeds rf ON rf.feed_id=f.id
        JOIN rss_rules r ON r.id=rf.rule_id WHERE f.site_id=?1 AND f.archived_at IS NULL AND r.archived_at IS NULL AND r.enabled=1)
        OR EXISTS(SELECT 1 FROM rss_download_jobs j JOIN rss_feeds f ON f.id=j.feed_id WHERE f.site_id=?1
        AND (j.status NOT IN ('submitted','already_present','failed','cancelled') OR j.reserved_bytes>0 OR j.lease_until>=?2))",
        params![id,Utc::now().to_rfc3339()],|row|row.get(0)).map_err(sql_error)
}

fn same_downloader_type(current: &str, next: &str) -> bool {
    let current = current.trim();
    let next = next.trim();
    let current_is_qb =
        current.eq_ignore_ascii_case("qb") || current.eq_ignore_ascii_case("qbittorrent");
    let next_is_qb = next.eq_ignore_ascii_case("qb") || next.eq_ignore_ascii_case("qbittorrent");
    (current_is_qb && next_is_qb) || current.eq_ignore_ascii_case(next)
}

fn same_downloader_url(current: &str, next: &str) -> bool {
    current.trim().trim_end_matches('/') == next.trim().trim_end_matches('/')
}

fn open_connection(path: &Path) -> Result<Connection, AppError> {
    let conn = Connection::open(path).map_err(sql_error)?;
    // 这些 PRAGMA 是按连接生效的。由于每个操作都新开连接，必须在此逐一设置，
    // 否则 foreign_keys 会静默关闭、并发写入会立刻返回 SQLITE_BUSY。
    // - busy_timeout: 写锁争用时等待而非立即失败（高频采集 + Web 请求并发写）。
    // - foreign_keys: 启用外键级联（ON DELETE CASCADE / SET NULL）。
    // - journal_mode=WAL + synchronous=FULL: 保持读写并发，并确保每次提交在返回前
    //   同步 WAL。OpenList/qB 副作用前的 checkpoint 必须能承受主机断电，而不只是进程崩溃。
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(sql_error)?;
    conn.pragma_update(None, "foreign_keys", "ON")
        .map_err(sql_error)?;
    conn.pragma_update(None, "journal_mode", "WAL")
        .map_err(sql_error)?;
    conn.pragma_update(None, "synchronous", "FULL")
        .map_err(sql_error)?;
    Ok(conn)
}

fn join_error(error: tokio::task::JoinError) -> AppError {
    AppError::Database {
        message: format!("database task join error: {}", error),
    }
}

// Archive duplicate configurations before merging their history. The whole upgrade is atomic.
fn ensure_unique_sign_in_site(conn: &Connection) -> Result<(), AppError> {
    let tx = conn.unchecked_transaction().map_err(sql_error)?;
    let migrated: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'index' AND name = 'idx_sign_in_tasks_unique_site')",
        [], |row| row.get(0),
    ).map_err(sql_error)?;
    if migrated {
        return Ok(());
    }

    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS sign_in_tasks_duplicate_archive AS
             SELECT * FROM sign_in_tasks WHERE 0;
         INSERT INTO sign_in_tasks_duplicate_archive
             SELECT * FROM sign_in_tasks WHERE id NOT IN
                 (SELECT MIN(id) FROM sign_in_tasks GROUP BY site_id);
         UPDATE sign_in_records SET task_id = (
             SELECT MIN(kept.id) FROM sign_in_tasks kept
             WHERE kept.site_id = (SELECT old.site_id FROM sign_in_tasks old
                                   WHERE old.id = sign_in_records.task_id)
         ) WHERE task_id IN (SELECT id FROM sign_in_tasks WHERE id NOT IN
                             (SELECT MIN(id) FROM sign_in_tasks GROUP BY site_id));
         DELETE FROM sign_in_tasks WHERE id NOT IN
             (SELECT MIN(id) FROM sign_in_tasks GROUP BY site_id);
         CREATE UNIQUE INDEX IF NOT EXISTS idx_sign_in_tasks_unique_site ON sign_in_tasks(site_id);"
    ).map_err(sql_error)?;
    tx.commit().map_err(sql_error)
}

fn sign_in_write_error(error: rusqlite::Error) -> AppError {
    if let rusqlite::Error::SqliteFailure(_, Some(message)) = &error {
        if message == "UNIQUE constraint failed: sign_in_tasks.site_id" {
            return AppError::InvalidConfig {
                message: "该站点已有签到任务（包括已暂停任务），请编辑现有任务，不可重复创建。"
                    .into(),
            };
        }
        if message == "UNIQUE constraint failed: sign_in_tasks.name" {
            return AppError::InvalidConfig {
                message: "签到任务名称已存在，请使用其他名称。".into(),
            };
        }
    }
    sql_error(error)
}

fn sql_error(error: rusqlite::Error) -> AppError {
    AppError::Database {
        message: error.to_string(),
    }
}

fn column_exists(conn: &Connection, table: &str, column: &str) -> bool {
    let Ok(mut stmt) = conn.prepare(&format!("PRAGMA table_info({table})")) else {
        return false;
    };
    let Ok(rows) = stmt.query_map([], |row| row.get::<_, String>(1)) else {
        return false;
    };
    for row in rows {
        if let Ok(c) = row {
            if c == column {
                return true;
            }
        }
    }
    false
}

fn ensure_column(conn: &Connection, table: &str, column: &str, sql: &str) -> Result<(), AppError> {
    if column_exists(conn, table, column) {
        return Ok(());
    }
    conn.execute(sql, []).map_err(sql_error)?;
    Ok(())
}

/// Preserve the last pre-upgrade site snapshot before the hourly refresher overwrites it.
/// Subsequent successful refreshes maintain this table transactionally in
/// `upsert_site_stats_success`.
fn backfill_site_stats_history(conn: &Connection) -> Result<(), AppError> {
    let mut statement = conn
        .prepare(
            "SELECT site_id, uid, username, uploaded, downloaded, ratio, bonus,
                    seeding_count, leeching_count, details_json, updated_at, last_checked_at
             FROM site_stats",
        )
        .map_err(sql_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<i64>>(3)?,
                row.get::<_, Option<i64>>(4)?,
                row.get::<_, Option<f64>>(5)?,
                row.get::<_, Option<f64>>(6)?,
                row.get::<_, Option<i64>>(7)?,
                row.get::<_, Option<i64>>(8)?,
                row.get::<_, String>(9)?,
                row.get::<_, Option<String>>(10)?,
                row.get::<_, String>(11)?,
            ))
        })
        .map_err(sql_error)?;
    let mut snapshots = Vec::new();
    for row in rows {
        let (
            site_id,
            uid,
            username,
            uploaded,
            downloaded,
            ratio,
            bonus,
            seeding_count,
            leeching_count,
            details_json,
            updated_at,
            last_checked_at,
        ) = row.map_err(sql_error)?;
        let Some(username) = username.filter(|value| !value.trim().is_empty()) else {
            continue;
        };
        let (Some(uploaded), Some(downloaded)) = (
            uploaded.and_then(|value| u64::try_from(value).ok()),
            downloaded.and_then(|value| u64::try_from(value).ok()),
        ) else {
            continue;
        };
        let recorded_at = updated_at.unwrap_or(last_checked_at);
        let snapshot_date = DateTime::parse_from_rfc3339(&recorded_at)
            .map(|value| value.with_timezone(&Utc).format("%Y-%m-%d").to_string())
            .unwrap_or_else(|_| recorded_at.chars().take(10).collect());
        if snapshot_date.len() != 10 {
            continue;
        }
        let mut stats = UserStats {
            uid,
            username,
            uploaded,
            downloaded,
            ratio,
            bonus,
            seeding_count: seeding_count.and_then(|value| u32::try_from(value).ok()),
            leeching_count: leeching_count.and_then(|value| u32::try_from(value).ok()),
            details: serde_json::from_str(&details_json).unwrap_or_default(),
        };
        stats.fill_derived();
        let stats_json = serde_json::to_string(&stats).map_err(|error| AppError::Database {
            message: format!("failed to migrate site statistics history: {error}"),
        })?;
        snapshots.push((site_id, snapshot_date, recorded_at, stats_json));
    }
    drop(statement);

    for (site_id, snapshot_date, updated_at, stats_json) in snapshots {
        conn.execute(
            "INSERT OR IGNORE INTO site_stats_history
             (site_id, snapshot_date, updated_at, stats_json) VALUES (?, ?, ?, ?)",
            params![site_id, snapshot_date, updated_at, stats_json],
        )
        .map_err(sql_error)?;
    }
    Ok(())
}

fn non_empty_trimmed(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then_some(trimmed)
}

fn normalize_lightpanda_region(value: &str) -> &'static str {
    match value.trim() {
        "uswest" => "uswest",
        _ => "euwest",
    }
}

fn migrate_media_relocation_jobs_for_manual_transfers(conn: &Connection) -> Result<(), AppError> {
    let media_download_not_null = conn
        .prepare("PRAGMA table_info(media_relocation_jobs)")
        .and_then(|mut stmt| {
            stmt.query_map([], |row| {
                Ok((row.get::<_, String>(1)?, row.get::<_, i64>(3)? != 0))
            })?
            .collect::<Result<Vec<_>, _>>()
        })
        .map_err(sql_error)?
        .into_iter()
        .find_map(|(name, not_null)| (name == "media_download_id").then_some(not_null))
        .unwrap_or(false);
    if !media_download_not_null {
        return Ok(());
    }

    let tx = conn.unchecked_transaction().map_err(sql_error)?;
    tx.execute_batch(
        "ALTER TABLE media_relocation_jobs RENAME TO media_relocation_jobs_legacy;
         CREATE TABLE media_relocation_jobs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            media_download_id INTEGER UNIQUE REFERENCES media_downloads(id) ON DELETE CASCADE,
            downloader_id INTEGER REFERENCES downloaders(id) ON DELETE SET NULL,
            infohash TEXT NOT NULL, source_qb_path TEXT NOT NULL,
            source_openlist_path TEXT NOT NULL,
            source_content_openlist_path TEXT NOT NULL DEFAULT '',
            target_openlist_path TEXT NOT NULL, target_qb_path TEXT NOT NULL,
            target_content_qb_path TEXT NOT NULL DEFAULT '',
            target_downloader_id INTEGER REFERENCES downloaders(id) ON DELETE SET NULL,
            copy_items_json TEXT NOT NULL DEFAULT '[]',
            source_files_json TEXT NOT NULL DEFAULT '[]',
            source_manifest_json TEXT NOT NULL DEFAULT '[]',
            copy_checkpoint_json TEXT, copy_lock_acquired INTEGER NOT NULL DEFAULT 0,
            manifest_cursor INTEGER NOT NULL DEFAULT 0, target_root_folder INTEGER,
            manual_requested_at TEXT, stage_started_at TEXT NOT NULL DEFAULT '',
            torrent_name TEXT NOT NULL, stage TEXT NOT NULL DEFAULT 'waiting_download',
            openlist_task_id TEXT, torrent_data BLOB, attempts INTEGER NOT NULL DEFAULT 0,
            next_attempt_at TEXT, lease_owner TEXT, lease_until TEXT,
            version INTEGER NOT NULL DEFAULT 0, last_error TEXT,
            created_at TEXT NOT NULL, updated_at TEXT NOT NULL, completed_at TEXT,
            cleared_at TEXT
         );
         INSERT INTO media_relocation_jobs
           (id, media_download_id, downloader_id, infohash, source_qb_path,
            source_openlist_path, source_content_openlist_path, target_openlist_path,
            target_qb_path, target_content_qb_path, target_downloader_id,
            copy_items_json, source_files_json, source_manifest_json,
            copy_checkpoint_json, copy_lock_acquired, manifest_cursor, target_root_folder,
            manual_requested_at, stage_started_at,
            torrent_name, stage, openlist_task_id, torrent_data, attempts, next_attempt_at,
            lease_owner, lease_until, version, last_error, created_at, updated_at, completed_at,
            cleared_at)
         SELECT id, media_download_id, downloader_id, infohash, source_qb_path,
            source_openlist_path, source_content_openlist_path, target_openlist_path,
            target_qb_path, target_content_qb_path, target_downloader_id,
            copy_items_json, source_files_json, source_manifest_json,
            copy_checkpoint_json, copy_lock_acquired, manifest_cursor, target_root_folder,
            manual_requested_at, updated_at,
            torrent_name, stage, openlist_task_id, torrent_data, attempts, next_attempt_at,
            lease_owner, lease_until, version, last_error, created_at, updated_at, completed_at,
            NULL
         FROM media_relocation_jobs_legacy;
         DROP TABLE media_relocation_jobs_legacy;
         CREATE INDEX idx_media_relocation_jobs_due
           ON media_relocation_jobs(stage, next_attempt_at, lease_until);",
    )
    .map_err(sql_error)?;
    tx.commit().map_err(sql_error)?;
    Ok(())
}

fn migrate_media_download_infohash_uniqueness(conn: &Connection) -> Result<(), AppError> {
    let table_sql: String = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'media_downloads'",
            [],
            |row| row.get(0),
        )
        .map_err(sql_error)?;
    let normalized = table_sql
        .to_ascii_lowercase()
        .replace(['\n', '\r', '\t'], " ");
    let has_legacy_constraint = normalized.contains("infohash text unique")
        || normalized.contains("unique(infohash)")
        || normalized.contains("unique (infohash)");

    if has_legacy_constraint {
        let tx = conn.unchecked_transaction().map_err(sql_error)?;
        tx.execute_batch(
            "ALTER TABLE media_downloads RENAME TO media_downloads_legacy_infohash;

             CREATE TABLE media_downloads (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 subscription_id INTEGER REFERENCES subscriptions(id) ON DELETE SET NULL,
                 target_key TEXT NOT NULL,
                 dedupe_key TEXT NOT NULL UNIQUE,
                 site_id INTEGER REFERENCES sites(id) ON DELETE SET NULL,
                 downloader_id INTEGER REFERENCES downloaders(id) ON DELETE SET NULL,
                 source_site TEXT NOT NULL,
                 downloader_name TEXT NOT NULL,
                 torrent_id TEXT NOT NULL,
                 title TEXT NOT NULL,
                 size INTEGER NOT NULL DEFAULT 0,
                 release_json TEXT NOT NULL,
                 decision_json TEXT NOT NULL,
                 profile_snapshot_json TEXT NOT NULL,
                 infohash TEXT,
                 status TEXT NOT NULL DEFAULT 'queued',
                 attempts INTEGER NOT NULL DEFAULT 0,
                 next_attempt_at TEXT,
                 lease_owner TEXT,
                 lease_until TEXT,
                 version INTEGER NOT NULL DEFAULT 0,
                 last_error TEXT,
                 created_at TEXT NOT NULL,
                 updated_at TEXT NOT NULL,
                 submitted_at TEXT
             );

             INSERT INTO media_downloads
                 (id, subscription_id, target_key, dedupe_key, site_id, downloader_id,
                  source_site, downloader_name, torrent_id, title, size, release_json,
                  decision_json, profile_snapshot_json, infohash, status, attempts,
                  next_attempt_at, lease_owner, lease_until, version, last_error,
                  created_at, updated_at, submitted_at)
             SELECT id, subscription_id, target_key, dedupe_key, site_id, downloader_id,
                    source_site, downloader_name, torrent_id, title, size, release_json,
                    decision_json, profile_snapshot_json, infohash, status, attempts,
                    next_attempt_at, lease_owner, lease_until, version, last_error,
                    created_at, updated_at, submitted_at
             FROM media_downloads_legacy_infohash;

             DROP TABLE media_downloads_legacy_infohash;",
        )
        .map_err(sql_error)?;
        tx.commit().map_err(sql_error)?;
    }

    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_media_downloads_due
             ON media_downloads(status, next_attempt_at, lease_until);
         CREATE INDEX IF NOT EXISTS idx_media_downloads_subscription
             ON media_downloads(subscription_id, created_at);
         CREATE UNIQUE INDEX IF NOT EXISTS idx_media_downloads_downloader_infohash
             ON media_downloads(downloader_id, lower(infohash))
             WHERE downloader_id IS NOT NULL AND infohash IS NOT NULL;",
    )
    .map_err(sql_error)
}

fn clamp_u64_to_i64(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}

#[cfg(test)]
mod migration_tests {
    use super::*;
    use rusqlite::Connection;
    use tempfile::tempdir;

    #[tokio::test]
    async fn sign_in_site_uniqueness_covers_concurrency_updates_and_paused_tasks() {
        let dir = tempdir().unwrap();
        let db = Database::open(dir.path()).await.unwrap();
        let site = db
            .create_site("one", "nexusphp", "https://one.example", "{}", "[]", false)
            .await
            .unwrap();
        let other_site = db
            .create_site("two", "nexusphp", "https://two.example", "{}", "[]", false)
            .await
            .unwrap();
        let request = SignInTaskRequest {
            name: "first".into(),
            site_id: site,
            cron_expression: "0 0 0/8 * * *".into(),
            browser: None,
            sign_in_method: None,
            browserless: None,
        };
        let mut other = request.clone();
        other.name = "second".into();
        let (a, b) = tokio::join!(
            db.create_sign_in_task(&request),
            db.create_sign_in_task(&other)
        );
        assert_ne!(a.is_ok(), b.is_ok());
        let id = a.or(b).unwrap();
        db.set_sign_in_task_enabled(id, false).await.unwrap();
        let error = db
            .create_sign_in_task(&other)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("该站点已有签到任务"), "{error}");
        let mut edit = request.clone();
        edit.name = "renamed".into();
        db.update_sign_in_task(id, &edit).await.unwrap();
        other.site_id = other_site;
        other.name = "other".into();
        let other_id = db.create_sign_in_task(&other).await.unwrap();
        other.site_id = site;
        assert!(
            db.update_sign_in_task(other_id, &other)
                .await
                .unwrap_err()
                .to_string()
                .contains("该站点已有签到任务")
        );
        assert_eq!(
            db.get_sign_in_task(other_id)
                .await
                .unwrap()
                .unwrap()
                .site_id,
            other_site
        );
        db.delete_sign_in_task(id).await.unwrap();
        edit.name = "replacement".into();
        db.create_sign_in_task(&edit).await.unwrap();
    }

    #[tokio::test]
    async fn sign_in_duplicate_migration_archives_configuration_and_preserves_history() {
        let dir = tempdir().unwrap();
        let db = Database::open(dir.path()).await.unwrap();
        let site = db
            .create_site("one", "nexusphp", "https://one.example", "{}", "[]", false)
            .await
            .unwrap();
        let conn = open_connection(&db.path).unwrap();
        conn.execute_batch("DROP INDEX idx_sign_in_tasks_unique_site;")
            .unwrap();
        for (id, name) in [(1, "keep"), (2, "duplicate")] {
            conn.execute("INSERT INTO sign_in_tasks(id,name,site_id,cron_expression,lightpanda_token,created_at,updated_at) VALUES(?1,?2,?3,'0 0 0/8 * * *','','now','now')", params![id,name,site]).unwrap();
            conn.execute("INSERT INTO sign_in_records(task_id,site_id,site_name,started_at,finished_at,status,message) VALUES(?1,?2,'one','now','now','success','preserve me')",params![id,site]).unwrap();
        }
        drop(conn);
        drop(db);
        let reopened = Database::open(dir.path()).await.unwrap();
        let tasks = reopened.list_sign_in_tasks().await.unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].name, "keep");
        let conn = open_connection(&reopened.path).unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM sign_in_records WHERE task_id=1",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            2
        );
        assert_eq!(
            conn.query_row(
                "SELECT name FROM sign_in_tasks_duplicate_archive",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
            "duplicate"
        );
        ensure_unique_sign_in_site(&conn).unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM sign_in_tasks_duplicate_archive",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn kirara_reuses_legacy_database_without_losing_settings() {
        let dir = tempdir().unwrap();
        let legacy_path = dir.path().join("rflush.db");
        let legacy = Database {
            path: legacy_path.clone(),
        };
        legacy.init().await.unwrap();
        let mut settings = legacy.get_settings().await.unwrap();
        settings.log_level = Some("debug".into());
        legacy.update_settings(&settings).await.unwrap();

        let reopened = Database::open(dir.path()).await.unwrap();
        assert_eq!(reopened.path, legacy_path);
        assert_eq!(
            reopened.get_settings().await.unwrap().log_level.as_deref(),
            Some("debug")
        );
        assert!(!dir.path().join("kirara.db").exists());
    }

    #[tokio::test]
    async fn kirara_new_database_takes_priority_when_both_names_exist() {
        let dir = tempdir().unwrap();
        let fresh = Database::open(dir.path()).await.unwrap();
        assert_eq!(fresh.path, dir.path().join("kirara.db"));
        let legacy = Database {
            path: dir.path().join("rflush.db"),
        };
        legacy.init().await.unwrap();
        let mut settings = legacy.get_settings().await.unwrap();
        settings.log_level = Some("trace".into());
        legacy.update_settings(&settings).await.unwrap();
        let reopened = Database::open(dir.path()).await.unwrap();
        assert_eq!(reopened.path, fresh.path);
        assert_ne!(
            reopened.get_settings().await.unwrap().log_level.as_deref(),
            Some("trace")
        );
    }

    #[tokio::test]
    async fn legacy_sites_receive_default_request_headers() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("kirara.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE sites (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL UNIQUE,
                site_type TEXT NOT NULL,
                base_url TEXT NOT NULL,
                auth_config TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            INSERT INTO sites
                (name, site_type, base_url, auth_config, created_at, updated_at)
            VALUES
                ('legacy-site', 'nexusphp', 'https://tracker.example',
                 '{\"auth_type\":\"cookie\",\"cookie\":\"uid=1\"}',
                 '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');",
        )
        .unwrap();
        drop(conn);

        let db = Database::open(dir.path()).await.unwrap();
        let site = db.get_site(1).await.unwrap().unwrap();
        let headers = crate::site::parse_site_request_headers(&site.request_headers).unwrap();

        assert_eq!(headers, default_site_request_headers());
    }

    #[tokio::test]
    async fn legacy_sign_in_browser_config_moves_to_global_settings_once() {
        let dir = tempdir().unwrap();
        let db = Database::open(dir.path()).await.unwrap();
        let site_id = db
            .create_site(
                "legacy-site",
                "nexusphp",
                "https://tracker.example.com",
                r#"{"type":"cookie","cookie":"uid=1"}"#,
                "[]",
                true,
            )
            .await
            .unwrap();
        let task_id = db
            .create_sign_in_task(&SignInTaskRequest {
                name: "legacy-sign-in".to_string(),
                site_id,
                cron_expression: "0 0 0/8 * * *".to_string(),
                browser: Some("lightpanda".to_string()),
                sign_in_method: Some(crate::sign_in::SIGN_IN_METHOD_OPEN_PAGE.to_string()),
                browserless: None,
            })
            .await
            .unwrap();

        let db_path = dir.path().join("kirara.db");
        let conn = open_connection(&db_path).unwrap();
        conn.execute(
            "UPDATE sign_in_tasks
             SET lightpanda_endpoint = ?, lightpanda_token = ?, lightpanda_region = ?,
                 browser = ?, proxy = ?, country = ?
             WHERE id = ?",
            params![
                "wss://legacy.example/ws",
                "legacy-token",
                "uswest",
                "chrome",
                "datacenter",
                "DE",
                task_id,
            ],
        )
        .unwrap();
        conn.execute(
            "UPDATE global_settings
             SET lightpanda_endpoint = NULL, lightpanda_token = NULL,
                 sign_in_browser_config_migrated = 0
             WHERE id = 1",
            [],
        )
        .unwrap();
        drop(conn);

        let migrated = Database::open(dir.path()).await.unwrap();
        let settings = migrated.get_settings().await.unwrap();
        assert_eq!(
            settings.lightpanda.endpoint.as_deref(),
            Some("wss://legacy.example/ws")
        );
        assert_eq!(settings.lightpanda.token.as_deref(), Some("legacy-token"));
        assert_eq!(settings.lightpanda.region, "uswest");
        assert_eq!(settings.lightpanda.browser, "chrome");
        assert_eq!(settings.lightpanda.proxy.as_deref(), Some("datacenter"));
        assert_eq!(settings.lightpanda.country.as_deref(), Some("DE"));
        assert_eq!(
            migrated
                .get_sign_in_task(task_id)
                .await
                .unwrap()
                .unwrap()
                .browser,
            crate::sign_in::SIGN_IN_BROWSER_LIGHTPANDA
        );

        let conn = open_connection(&db_path).unwrap();
        conn.execute_batch(
            "ALTER TABLE global_settings ADD COLUMN cloakbrowser_license_key TEXT;
             ALTER TABLE global_settings ADD COLUMN cloakbrowser_headless INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE global_settings ADD COLUMN cloakbrowser_humanize INTEGER NOT NULL DEFAULT 1;
             ALTER TABLE global_settings ADD COLUMN cloakbrowser_human_preset TEXT NOT NULL DEFAULT 'careful';
             ALTER TABLE global_settings ADD COLUMN cloakbrowser_proxy TEXT;
             ALTER TABLE global_settings ADD COLUMN cloakbrowser_geoip INTEGER NOT NULL DEFAULT 1;
             UPDATE global_settings SET cloakbrowser_license_key = 'legacy-secret' WHERE id = 1;
             UPDATE sign_in_tasks SET browser = 'retired-provider' WHERE id = 1;",
        )
        .unwrap();
        drop(conn);

        let reopened = Database::open(dir.path()).await.unwrap();
        assert_eq!(
            reopened
                .get_sign_in_task(task_id)
                .await
                .unwrap()
                .unwrap()
                .browser,
            crate::sign_in::SIGN_IN_BROWSER_LIGHTPANDA
        );
        let conn = open_connection(&db_path).unwrap();
        for column in [
            "cloakbrowser_license_key",
            "cloakbrowser_headless",
            "cloakbrowser_humanize",
            "cloakbrowser_human_preset",
            "cloakbrowser_proxy",
            "cloakbrowser_geoip",
        ] {
            assert!(!column_exists(&conn, "global_settings", column));
        }
    }

    #[tokio::test]
    async fn browserless_sign_in_configuration_survives_database_reopen() {
        let dir = tempdir().unwrap();
        let db = Database::open(dir.path()).await.unwrap();
        let site_id = db
            .create_site(
                "browserless-site",
                "nexusphp",
                "https://tracker.example.com",
                r#"{"type":"cookie","cookie":"uid=1"}"#,
                "[]",
                true,
            )
            .await
            .unwrap();

        let mut settings = db.get_settings().await.unwrap();
        settings.browserless.address = Some("https://production-sfo.browserless.io".to_string());
        settings.browserless.token = Some("secret-token".to_string());
        db.update_settings(&settings).await.unwrap();

        let task_id = db
            .create_sign_in_task(&SignInTaskRequest {
                name: "browserless-sign-in".to_string(),
                site_id,
                cron_expression: "0 0 0/8 * * *".to_string(),
                browser: Some(crate::sign_in::SIGN_IN_BROWSER_BROWSERLESS.to_string()),
                sign_in_method: Some(crate::sign_in::SIGN_IN_METHOD_OPEN_PAGE.to_string()),
                browserless: Some(crate::sign_in::BrowserlessTaskConfig {
                    selector: "button.check-in".to_string(),
                    cf_mode: crate::sign_in::BROWSERLESS_CF_MODE_PAGE.to_string(),
                    wait_ms: None,
                    solve_timeout: Some(45_000),
                    action_timeout: None,
                    post_click_wait_ms: Some(2_000),
                    attendance_path: "/plugin_sign-in.php".into(),
                    captcha_selector: "#frmSignin img".into(),
                    captcha_input_selector: "#imagestring".into(),
                    already_keywords: "今日已签到\n请勿重复签到".into(),
                    submit_method: "ajax".into(),
                    result_rules: vec![crate::sign_in::SignInResultRule {
                        outcome: "success".into(),
                        kind: "json".into(),
                        selector: "".into(),
                        field: "/state".into(),
                        value: "false".into(),
                        value_type: "string".into(),
                    }],
                }),
            })
            .await
            .unwrap();
        drop(db);

        let reopened = Database::open(dir.path()).await.unwrap();
        let settings = reopened.get_settings().await.unwrap();
        assert_eq!(
            settings.browserless.address.as_deref(),
            Some("https://production-sfo.browserless.io")
        );
        assert_eq!(settings.browserless.token.as_deref(), Some("secret-token"));

        let task = reopened.get_sign_in_task(task_id).await.unwrap().unwrap();
        assert_eq!(task.browser, crate::sign_in::SIGN_IN_BROWSER_BROWSERLESS);
        assert_eq!(task.browserless.submit_method, "ajax");
        assert_eq!(task.browserless.result_rules[0].value, "false");
        assert_eq!(task.browserless.result_rules[0].value_type, "string");
        assert_eq!(task.browserless.selector, "button.check-in");
        assert_eq!(
            task.browserless.cf_mode,
            crate::sign_in::BROWSERLESS_CF_MODE_PAGE
        );
        assert_eq!(task.browserless.wait_ms, None);
        assert_eq!(task.browserless.solve_timeout, Some(45_000));
        assert_eq!(task.browserless.post_click_wait_ms, Some(2_000));
        assert_eq!(task.browserless.attendance_path, "/plugin_sign-in.php");
        assert_eq!(task.browserless.captcha_selector, "#frmSignin img");
        assert_eq!(task.browserless.captcha_input_selector, "#imagestring");
        assert_eq!(
            task.browserless.already_keywords,
            "今日已签到\n请勿重复签到"
        );
        reopened
            .update_sign_in_task(
                task_id,
                &SignInTaskRequest {
                    name: task.name,
                    site_id,
                    cron_expression: task.cron_expression,
                    browser: Some(task.browser),
                    sign_in_method: Some("ocr_captcha".into()),
                    browserless: Some(crate::sign_in::BrowserlessTaskConfig {
                        already_keywords: "已完成今日签到".into(),
                        ..task.browserless
                    }),
                },
            )
            .await
            .unwrap();
        let tasks = reopened.list_sign_in_tasks().await.unwrap();
        assert_eq!(tasks[0].browserless.submit_method, "ajax");
        assert_eq!(tasks[0].browserless.result_rules[0].field, "/state");
        assert_eq!(tasks[0].browserless.already_keywords, "已完成今日签到");
        assert_eq!(tasks[0].browserless.attendance_path, "/plugin_sign-in.php");
    }

    #[test]
    fn every_database_connection_uses_power_loss_durable_wal_commits() {
        let dir = tempdir().unwrap();
        let conn = open_connection(&dir.path().join("durability.db")).unwrap();

        let journal_mode = conn
            .query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0))
            .unwrap();
        let synchronous = conn
            .query_row("PRAGMA synchronous", [], |row| row.get::<_, i64>(0))
            .unwrap();

        assert_eq!(journal_mode.to_ascii_lowercase(), "wal");
        assert_eq!(
            synchronous, 2,
            "SQLite FULL synchronous must remain enabled"
        );
    }

    #[tokio::test]
    async fn legacy_automatic_qb_followups_are_stopped_at_database_boundary() {
        let dir = tempdir().unwrap();
        Database::open(dir.path()).await.unwrap();
        let db_path = dir.path().join("kirara.db");
        let conn = open_connection(&db_path).unwrap();
        let stages = [
            "copy_verified",
            "qb_reconcile",
            "torrent_exported",
            "source_qb_removed",
            "target_qb_submitted",
            "target_qb_check_requested",
            "target_qb_checking",
            "target_qb_starting",
            "qb_manual_review",
            "source_removing",
            "source_remove_manual_review",
            "source_removed",
        ];
        for (index, stage) in stages.iter().enumerate() {
            let now = format!("2026-01-01T00:00:{index:02}Z");
            let hash = format!("{index:040x}");
            conn.execute(
                "INSERT INTO media_downloads
                 (target_key, dedupe_key, source_site, downloader_name, torrent_id,
                  title, release_json, decision_json, profile_snapshot_json, infohash,
                  status, created_at, updated_at)
                 VALUES (?, ?, 'legacy', 'legacy-qb', ?, ?, '{}', '{}', '{}', ?,
                         'submitted', ?, ?)",
                params![
                    format!("tv:{index}"),
                    format!("legacy-auto-{index}"),
                    format!("torrent-{index}"),
                    format!("Legacy Auto {index}"),
                    hash,
                    now,
                    now
                ],
            )
            .unwrap();
            let media_download_id = conn.last_insert_rowid();
            conn.execute(
                r#"INSERT INTO media_relocation_jobs
                 (media_download_id, infohash, source_qb_path, source_openlist_path,
                  target_openlist_path, target_qb_path, torrent_name, stage,
                  openlist_task_id, torrent_data, attempts, next_attempt_at,
                  lease_owner, lease_until, last_error, copy_checkpoint_json,
                  copy_lock_acquired, manual_requested_at, stage_started_at,
                  created_at, updated_at)
                 VALUES (?, ?, '/source-qb', '/source', '/target', '/target-qb', ?, ?,
                         'legacy-task', X'0102', 3, ?, 'legacy-worker', ?, 'legacy error',
                         '{"phase":"uncertain"}', 1, ?, ?, ?, ?)"#,
                params![
                    media_download_id,
                    hash,
                    format!("Legacy Auto {index}"),
                    stage,
                    now,
                    "2099-01-01T00:00:00Z",
                    now,
                    now,
                    now,
                    now
                ],
            )
            .unwrap();
        }
        drop(conn);

        Database::open(dir.path()).await.unwrap();
        let conn = open_connection(&db_path).unwrap();
        let mut statement = conn
            .prepare(
                "SELECT stage, openlist_task_id, torrent_data, copy_checkpoint_json,
                        copy_lock_acquired, lease_owner, next_attempt_at,
                        manual_requested_at, completed_at, last_error
                 FROM media_relocation_jobs ORDER BY id",
            )
            .unwrap();
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<Vec<u8>>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, bool>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, Option<String>>(9)?,
                ))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(rows.len(), stages.len());
        for row in rows {
            assert_eq!(row.0, "completed");
            assert_eq!(row.1, None);
            assert_eq!(row.2.as_deref(), Some([1_u8, 2_u8].as_slice()));
            assert_eq!(row.3, None);
            assert!(!row.4);
            assert_eq!(row.5, None);
            assert_eq!(row.6, None);
            assert_eq!(row.7, None);
            assert!(row.8.is_some());
            assert!(row.9.as_deref().is_some_and(|error| error.contains("qB")));
        }
    }

    #[tokio::test]
    async fn creates_brush_task() {
        let dir = tempdir().unwrap();
        let db = Database::open(dir.path()).await.unwrap();
        let request = BrushTaskRequest {
            name: "test-task".to_string(),
            cron_expression: "0 */9 * * * *".to_string(),
            site_id: None,
            downloader_ids: vec![3, 1],
            tag: "test".to_string(),
            rss_url: "https://example.com/rss".to_string(),
            seed_volume_gb: None,
            save_dir: None,
            active_time_windows: None,
            promotion: Some("free".to_string()),
            skip_hit_and_run: Some(false),
            max_concurrent: Some(9),
            download_speed_limit: None,
            upload_speed_limit: Some(117_000),
            size_ranges: None,
            seeder_ranges: Some("[\"1-37\"]".to_string()),
            downloader_ranges: None,
            downloader_weights: None,
            min_free_hours: None,
            delete_mode: Some("or".to_string()),
            delete_on_free_expiry: Some(false),
            min_seed_time_hours: None,
            hr_min_seed_time_hours: None,
            target_ratio: Some(3.0),
            max_upload_gb: None,
            download_timeout_hours: Some(2.0),
            min_avg_upload_speed_kbs: Some(2048.0),
            max_inactive_hours: Some(0.1),
            min_disk_space_gb: Some(1.0),
        };

        let id = db.create_brush_task(&request).await.unwrap();
        let task = db.get_brush_task(id).await.unwrap().unwrap();

        assert_eq!(task.name, request.name);
        assert_eq!(task.downloader_ids, request.downloader_ids);
        assert_eq!(task.min_disk_space_gb, request.min_disk_space_gb);
    }

    #[tokio::test]
    async fn migrates_legacy_global_infohash_constraint_without_losing_rows() {
        let dir = tempdir().unwrap();
        let db = Database::open(dir.path()).await.unwrap();
        let downloader_a = db
            .create_downloader(
                "migration-downloader-a",
                "qbittorrent",
                "http://127.0.0.1:8080",
                "",
                "",
            )
            .await
            .unwrap();
        let downloader_b = db
            .create_downloader(
                "migration-downloader-b",
                "qbittorrent",
                "http://127.0.0.1:8081",
                "",
                "",
            )
            .await
            .unwrap();
        let db_path = dir.path().join("kirara.db");
        let conn = open_connection(&db_path).unwrap();
        conn.execute_batch(
            "DROP INDEX idx_media_downloads_downloader_infohash;
             ALTER TABLE media_downloads RENAME TO media_downloads_current;
             CREATE TABLE media_downloads (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 subscription_id INTEGER REFERENCES subscriptions(id) ON DELETE SET NULL,
                 target_key TEXT NOT NULL,
                 dedupe_key TEXT NOT NULL UNIQUE,
                 site_id INTEGER REFERENCES sites(id) ON DELETE SET NULL,
                 downloader_id INTEGER REFERENCES downloaders(id) ON DELETE SET NULL,
                 source_site TEXT NOT NULL,
                 downloader_name TEXT NOT NULL,
                 torrent_id TEXT NOT NULL,
                 title TEXT NOT NULL,
                 size INTEGER NOT NULL DEFAULT 0,
                 release_json TEXT NOT NULL,
                 decision_json TEXT NOT NULL,
                 profile_snapshot_json TEXT NOT NULL,
                 infohash TEXT UNIQUE,
                 status TEXT NOT NULL DEFAULT 'queued',
                 attempts INTEGER NOT NULL DEFAULT 0,
                 next_attempt_at TEXT,
                 lease_owner TEXT,
                 lease_until TEXT,
                 version INTEGER NOT NULL DEFAULT 0,
                 last_error TEXT,
                 created_at TEXT NOT NULL,
                 updated_at TEXT NOT NULL,
                 submitted_at TEXT
             );
             DROP TABLE media_downloads_current;",
        )
        .unwrap();
        let hash = "0123456789abcdef0123456789abcdef01234567";
        conn.execute(
            "INSERT INTO media_downloads
             (id, target_key, dedupe_key, downloader_id, source_site, downloader_name,
              torrent_id, title, size, release_json, decision_json,
              profile_snapshot_json, infohash, status, attempts, next_attempt_at,
              lease_owner, lease_until, version, last_error, created_at, updated_at,
              submitted_at)
             VALUES (77, 'movie:42', 'legacy-row', ?, 'legacy-site', 'legacy-qb',
                     'torrent-42', 'Legacy Movie 2026 1080p WEB-DL', 4242, '{\"legacy\":true}',
                     '{\"accepted\":true}', '{\"profile\":1}', ?, 'submitted', 3,
                     NULL, NULL, NULL, 4, 'preserved audit',
                     '2026-01-01T00:00:00Z', '2026-01-02T00:00:00Z',
                     '2026-01-02T00:00:00Z')",
            params![downloader_a, hash],
        )
        .unwrap();
        drop(conn);

        Database::open(dir.path()).await.unwrap();
        let conn = open_connection(&db_path).unwrap();
        let preserved: (i64, String, i64, String, i64, i64, String) = conn
            .query_row(
                "SELECT id, target_key, size, release_json, attempts, version, last_error
                 FROM media_downloads WHERE dedupe_key = 'legacy-row'",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(
            preserved,
            (
                77,
                "movie:42".to_string(),
                4242,
                "{\"legacy\":true}".to_string(),
                3,
                4,
                "preserved audit".to_string(),
            )
        );

        conn.execute(
            "INSERT INTO media_downloads
             (target_key, dedupe_key, downloader_id, source_site, downloader_name,
              torrent_id, title, release_json, decision_json, profile_snapshot_json,
              infohash, created_at, updated_at)
             VALUES ('movie:43', 'other-downloader', ?, 'legacy-site', 'other-qb',
                     'torrent-43', 'Other Movie', '{}', '{}', '{}', ?,
                     '2026-01-03T00:00:00Z', '2026-01-03T00:00:00Z')",
            params![downloader_b, hash.to_ascii_uppercase()],
        )
        .unwrap();
        let duplicate = conn.execute(
            "INSERT INTO media_downloads
             (target_key, dedupe_key, downloader_id, source_site, downloader_name,
              torrent_id, title, release_json, decision_json, profile_snapshot_json,
              infohash, created_at, updated_at)
             VALUES ('movie:44', 'same-downloader', ?, 'legacy-site', 'legacy-qb',
                     'torrent-44', 'Duplicate Movie', '{}', '{}', '{}', ?,
                     '2026-01-04T00:00:00Z', '2026-01-04T00:00:00Z')",
            params![downloader_a, hash.to_ascii_uppercase()],
        );
        assert!(duplicate.is_err());
        assert_eq!(
            conn.query_row("SELECT count(*) FROM media_downloads", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
            2
        );
        assert!(
            conn.query_row("PRAGMA foreign_key_check", [], |_| Ok(()))
                .optional()
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn test_migration_save_dir_json_conversion() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let conn = Connection::open(&db_path).unwrap();

        // Simulate a legacy brush_tasks row with downloader_id=1 and save_dir='/path'
        conn.execute_batch(
            "CREATE TABLE brush_tasks (
                id INTEGER PRIMARY KEY,
                downloader_id INTEGER NOT NULL,
                save_dir TEXT
            );
            INSERT INTO brush_tasks (downloader_id, save_dir) VALUES (1, '/downloads/brush');
            INSERT INTO brush_tasks (downloader_id, save_dir) VALUES (2, '/other/path');
            INSERT INTO brush_tasks (downloader_id, save_dir) VALUES (3, NULL);
            ",
        )
        .unwrap();

        // Run the same UPDATE as the migration
        conn.execute(
            "UPDATE brush_tasks
             SET save_dir = json_object(CAST(downloader_id AS TEXT), save_dir)
             WHERE save_dir IS NOT NULL AND save_dir NOT LIKE '{%'",
            [],
        )
        .unwrap();

        // Verify row 1
        let row1: String = conn
            .query_row(
                "SELECT save_dir FROM brush_tasks WHERE downloader_id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let json1: serde_json::Value = serde_json::from_str(&row1).unwrap();
        assert_eq!(json1["1"], "/downloads/brush");

        // Verify row 2
        let row2: String = conn
            .query_row(
                "SELECT save_dir FROM brush_tasks WHERE downloader_id = 2",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let json2: serde_json::Value = serde_json::from_str(&row2).unwrap();
        assert_eq!(json2["2"], "/other/path");

        // Verify row 3 (NULL save_dir stays NULL)
        let row3: Option<String> = conn
            .query_row(
                "SELECT save_dir FROM brush_tasks WHERE downloader_id = 3",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(row3.is_none());
    }
}
