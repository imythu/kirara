use std::collections::BTreeMap;
use std::io::{Cursor, Write};

use chrono::{DateTime, Duration, Utc};
use reqwest::{Client, Method, RequestBuilder, Url};
use serde::Serialize;
use serde_json::{Map, Value, json};
use tokio::sync::Mutex;
use tracing::{info, warn};
use zip::write::SimpleFileOptions;

use crate::db::{Database, PtdBackupConfig};
use crate::net::client_factory;
use crate::site::{SiteStatsHistoryRecord, SiteWithStats, UserStats};

const MIN_BACKUP_INTERVAL_HOURS: u64 = 1;
const MAX_BACKUP_INTERVAL_HOURS: u64 = 24 * 30;
const PTD_SUCCESS_STATUS: u8 = 3;
static BACKUP_LOCK: Mutex<()> = Mutex::const_new(());

#[derive(Debug, Clone, Serialize)]
pub struct PtdBackupRunResult {
    pub filename: String,
    pub site_count: usize,
    pub size: usize,
    pub backed_up_at: String,
}

#[derive(Debug, Serialize)]
struct PtdManifestFile {
    name: &'static str,
    hash: String,
}

#[derive(Debug, Serialize)]
struct PtdManifest {
    time: i64,
    version: String,
    encryption: bool,
    files: BTreeMap<&'static str, PtdManifestFile>,
}

pub fn validate_config(config: &mut PtdBackupConfig) -> Result<(), String> {
    if config.webdav_url.trim().is_empty() {
        if config.enabled {
            return Err("启用蜂巢 PTD 备份前必须填写 WebDAV 地址".to_string());
        }
        config.webdav_url.clear();
    } else {
        config.webdav_url = normalize_webdav_url(&config.webdav_url)?;
    }
    config.username = config.username.trim().to_string();
    if !(MIN_BACKUP_INTERVAL_HOURS..=MAX_BACKUP_INTERVAL_HOURS)
        .contains(&config.backup_interval_hours)
    {
        return Err(format!(
            "备份周期必须在 {MIN_BACKUP_INTERVAL_HOURS} 到 {MAX_BACKUP_INTERVAL_HOURS} 小时之间"
        ));
    }

    Ok(())
}

pub fn ptd_site_id(site: &SiteWithStats) -> Option<&'static str> {
    if matches!(site.site_type.as_str(), "mteam" | "m_team") {
        return Some("mteam");
    }

    Url::parse(&site.base_url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_ascii_lowercase))
        .and_then(|host| crate::ptd_sites::site_id_for_host(&host))
}

pub async fn test_webdav(config: &PtdBackupConfig, proxy: Option<&str>) -> Result<(), String> {
    let url = normalize_webdav_url(&config.webdav_url)?;
    let client = build_client(config.use_proxy.then_some(proxy).flatten())?;
    let method = Method::from_bytes(b"PROPFIND").map_err(|error| error.to_string())?;
    let request = client
        .request(method, &url)
        .header("Depth", "0")
        .header("Content-Type", "application/xml; charset=utf-8")
        .body(
            r#"<?xml version="1.0" encoding="utf-8" ?><d:propfind xmlns:d="DAV:"><d:prop><d:resourcetype/></d:prop></d:propfind>"#,
        );
    let response = with_basic_auth(request, config)
        .send()
        .await
        .map_err(|error| format!("连接 WebDAV 失败: {error}"))?;
    if response.status().is_success() || response.status().as_u16() == 207 {
        return Ok(());
    }
    Err(format!("WebDAV 返回 HTTP {}", response.status()))
}

pub async fn backup_now(db: &Database) -> Result<PtdBackupRunResult, String> {
    let _guard = BACKUP_LOCK.lock().await;
    backup_locked(db).await
}

pub async fn backup_if_due(db: &Database) {
    let Ok(config) = db.get_ptd_backup_config().await else {
        warn!("failed to load Hive PTD backup config");
        return;
    };
    if !config.enabled || !is_due(&config) {
        return;
    }

    let Ok(_guard) = BACKUP_LOCK.try_lock() else {
        return;
    };
    let Ok(current) = db.get_ptd_backup_config().await else {
        return;
    };
    if !current.enabled || !is_due(&current) {
        return;
    }
    match backup_locked(db).await {
        Ok(result) => info!(
            filename = %result.filename,
            site_count = result.site_count,
            "Hive PTD automatic backup completed"
        ),
        Err(error) => warn!(%error, "Hive PTD automatic backup failed"),
    }
}

async fn backup_locked(db: &Database) -> Result<PtdBackupRunResult, String> {
    let result = perform_backup(db).await;
    if let Err(error) = result.as_ref() {
        let _ = db.record_ptd_backup_error(error).await;
    }
    result
}

async fn perform_backup(db: &Database) -> Result<PtdBackupRunResult, String> {
    let mut config = db
        .get_ptd_backup_config()
        .await
        .map_err(|error| error.to_string())?;
    let sites = db
        .list_sites_with_stats()
        .await
        .map_err(|error| error.to_string())?;
    let history = db
        .list_site_stats_history()
        .await
        .map_err(|error| error.to_string())?;
    validate_config(&mut config)?;
    if config.webdav_url.is_empty() {
        return Err("请先配置 WebDAV 地址".to_string());
    }

    let now = Utc::now();
    let (archive, site_count) = build_ptd_archive(&sites, &history, now)?;
    if site_count == 0 {
        return Err("没有可备份的已识别 PTD 站点用户信息，请检查站点域名并刷新统计".to_string());
    }

    let filename = format!("PTD_backup_{}.zip", now.format("%Y%m%dT%H%M"));
    let upload_url = webdav_file_url(&config.webdav_url, &filename)?;
    let settings = db.get_settings().await.map_err(|error| error.to_string())?;
    let proxy = config
        .use_proxy
        .then_some(settings.proxy.as_deref())
        .flatten();
    let client = build_client(proxy)?;
    let archive_size = archive.len();
    let request = client
        .put(upload_url)
        .header("Content-Type", "application/zip")
        .body(archive);
    let response = match with_basic_auth(request, &config).send().await {
        Ok(response) => response,
        Err(error) => {
            return Err(format!("上传 WebDAV 失败: {error}"));
        }
    };
    if !response.status().is_success() {
        return Err(format!("上传 WebDAV 失败: HTTP {}", response.status()));
    }

    let backed_up_at = now.to_rfc3339();
    db.record_ptd_backup_success(&filename, &backed_up_at)
        .await
        .map_err(|error| error.to_string())?;
    Ok(PtdBackupRunResult {
        filename,
        site_count,
        size: archive_size,
        backed_up_at,
    })
}

fn build_ptd_archive(
    sites: &[SiteWithStats],
    history: &[SiteStatsHistoryRecord],
    now: DateTime<Utc>,
) -> Result<(Vec<u8>, usize), String> {
    let mut user_info = BTreeMap::<String, BTreeMap<String, Value>>::new();
    for site in sites {
        let Some(ptd_site_id) = ptd_site_id(site) else {
            continue;
        };
        let snapshots = user_info.entry(ptd_site_id.to_string()).or_default();

        for record in history.iter().filter(|record| record.site_id == site.id) {
            if !has_user_stats(&record.stats) {
                continue;
            }
            let update_at = DateTime::parse_from_rfc3339(&record.updated_at)
                .map(|value| value.with_timezone(&Utc))
                .unwrap_or(now);
            snapshots.insert(
                record.snapshot_date.clone(),
                ptd_user_snapshot(ptd_site_id, &record.stats, update_at.timestamp_millis()),
            );
        }

        // 兼容升级前仅有 site_stats、尚未生成首条历史记录的数据库；同日当前数据
        // 也会覆盖较早的历史快照，与 PT-Depiler 每日保留最后一次成功刷新一致。
        if let Some((stats_record, stats)) = site
            .stats
            .as_ref()
            .and_then(|record| record.to_user_stats().map(|stats| (record, stats)))
            .filter(|(_, stats)| has_user_stats(stats))
        {
            let update_at = stats_record
                .updated_at
                .as_deref()
                .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
                .map(|value| value.with_timezone(&Utc))
                .unwrap_or(now);
            snapshots.insert(
                update_at.format("%Y-%m-%d").to_string(),
                ptd_user_snapshot(ptd_site_id, &stats, update_at.timestamp_millis()),
            );
        }
    }
    user_info.retain(|_, snapshots| !snapshots.is_empty());

    let site_count = user_info.len();
    let user_info_bytes = serde_json::to_vec(&user_info)
        .map_err(|error| format!("生成 userInfo.json 失败: {error}"))?;
    let user_info_hash = format!("{:x}", md5::compute(&user_info_bytes));
    let manifest = PtdManifest {
        time: now.timestamp_millis(),
        version: format!("PT-Depiler (kirara {})", env!("CARGO_PKG_VERSION")),
        encryption: false,
        files: BTreeMap::from([(
            "userInfo",
            PtdManifestFile {
                name: "userInfo.json",
                hash: user_info_hash,
            },
        )]),
    };
    let manifest_bytes = serde_json::to_vec(&manifest)
        .map_err(|error| format!("生成 manifest.json 失败: {error}"))?;

    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .compression_level(Some(9));
    zip.start_file("userInfo.json", options)
        .map_err(|error| format!("创建 PTD ZIP 失败: {error}"))?;
    zip.write_all(&user_info_bytes)
        .map_err(|error| format!("写入 userInfo.json 失败: {error}"))?;
    zip.start_file("manifest.json", options)
        .map_err(|error| format!("创建 PTD ZIP 失败: {error}"))?;
    zip.write_all(&manifest_bytes)
        .map_err(|error| format!("写入 manifest.json 失败: {error}"))?;
    let archive = zip
        .finish()
        .map_err(|error| format!("完成 PTD ZIP 失败: {error}"))?
        .into_inner();
    Ok((archive, site_count))
}

fn ptd_user_snapshot(site_id: &str, stats: &UserStats, update_at: i64) -> Value {
    // 自定义适配字段先写入，标准 PTD 字段随后覆盖，避免扩展数据破坏快照结构。
    let mut snapshot = Map::from_iter(stats.details.extra.clone());
    snapshot.extend([
        ("status".to_string(), json!(PTD_SUCCESS_STATUS)),
        ("updateAt".to_string(), json!(update_at)),
        ("site".to_string(), json!(site_id)),
        ("name".to_string(), json!(stats.username)),
        ("uploaded".to_string(), json!(stats.uploaded)),
        ("downloaded".to_string(), json!(stats.downloaded)),
    ]);
    if let Some(uid) = stats
        .details
        .ptd_user_id
        .as_deref()
        .or(stats.uid.as_deref())
    {
        snapshot.insert("id".to_string(), json!(uid));
    }
    if let Some(ratio) = stats.ratio.filter(|value| value.is_finite()) {
        snapshot.insert("ratio".to_string(), json!(ratio));
    }
    if let Some(bonus) = stats.bonus.filter(|value| value.is_finite()) {
        snapshot.insert("bonus".to_string(), json!(bonus));
    }
    if let Some(seeding) = stats.seeding_count {
        snapshot.insert("seeding".to_string(), json!(seeding));
    }
    if let Some(leeching) = stats.leeching_count {
        snapshot.insert("leeching".to_string(), json!(leeching));
    }

    macro_rules! insert_optional {
        ($field:literal, $value:expr) => {
            if let Some(value) = $value {
                snapshot.insert($field.to_string(), json!(value));
            }
        };
    }
    macro_rules! insert_finite {
        ($field:literal, $value:expr) => {
            if let Some(value) = $value.filter(|value| value.is_finite()) {
                snapshot.insert($field.to_string(), json!(value));
            }
        };
    }

    let details = &stats.details;
    insert_optional!("isDonor", details.is_donor);
    insert_optional!("levelId", details.level_id);
    insert_optional!("levelName", details.level_name.as_deref());
    insert_optional!("joinTime", details.join_time);
    insert_optional!("lastAccessAt", details.last_access_at);
    insert_optional!("messageCount", details.message_count);
    insert_optional!("invites", details.invites);
    insert_optional!("avatar", details.avatar.as_deref());
    insert_optional!("totalTraffic", details.total_traffic);
    insert_optional!("trueDownloaded", details.true_downloaded);
    insert_optional!("trueUploaded", details.true_uploaded);
    insert_finite!("trueRatio", details.true_ratio);
    insert_optional!("seedingSize", details.seeding_size);
    insert_optional!("seedingTime", details.seeding_time);
    insert_optional!("averageSeedingTime", details.average_seeding_time);
    insert_finite!("seedingBonus", details.seeding_bonus);
    insert_finite!("bonusPerHour", details.bonus_per_hour);
    insert_finite!("seedingBonusPerHour", details.seeding_bonus_per_hour);
    insert_optional!("uploads", details.uploads);
    insert_optional!("snatches", details.snatches);
    insert_optional!("posts", details.posts);
    insert_optional!("adoptions", details.adoptions);
    insert_optional!("hnrUnsatisfied", details.hnr_unsatisfied);
    insert_optional!("hnrPreWarning", details.hnr_pre_warning);
    Value::Object(snapshot)
}

fn has_user_stats(stats: &UserStats) -> bool {
    !stats.username.trim().is_empty()
}

fn normalize_webdav_url(value: &str) -> Result<String, String> {
    let mut url = Url::parse(value.trim()).map_err(|_| "WebDAV 地址无效".to_string())?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err("WebDAV 地址仅支持 HTTP 或 HTTPS".to_string());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("请使用独立的用户名和密码字段，不要在 WebDAV 地址中包含凭据".to_string());
    }
    url.set_query(None);
    url.set_fragment(None);
    url.path_segments_mut()
        .map_err(|_| "WebDAV 地址不能作为目录使用".to_string())?
        .pop_if_empty()
        .push("");
    Ok(url.to_string())
}

fn webdav_file_url(base: &str, filename: &str) -> Result<Url, String> {
    let base = normalize_webdav_url(base)?;
    Url::parse(&base)
        .and_then(|url| url.join(filename))
        .map_err(|_| "无法生成 WebDAV 上传地址".to_string())
}

fn build_client(proxy: Option<&str>) -> Result<Client, String> {
    client_factory::build_client(proxy).map_err(|error| format!("创建 WebDAV 客户端失败: {error}"))
}

fn with_basic_auth(request: RequestBuilder, config: &PtdBackupConfig) -> RequestBuilder {
    if config.username.is_empty() && config.password.is_empty() {
        request
    } else {
        request.basic_auth(&config.username, Some(&config.password))
    }
}

fn is_due(config: &PtdBackupConfig) -> bool {
    let Some(last_backup_at) = config
        .last_backup_at
        .as_deref()
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
    else {
        return true;
    };
    Utc::now()
        >= last_backup_at.with_timezone(&Utc)
            + Duration::hours(config.backup_interval_hours.min(i64::MAX as u64) as i64)
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;
    use crate::site::{SiteStatsRecord, UserStats, UserStatsDetails};

    fn sample_site() -> SiteWithStats {
        SiteWithStats {
            id: 9,
            name: "M-Team".to_string(),
            site_type: "mteam".to_string(),
            base_url: "https://api.m-team.cc".to_string(),
            auth_config: String::new(),
            request_headers: "[]".to_string(),
            use_proxy: false,
            created_at: String::new(),
            updated_at: String::new(),
            stats: Some(SiteStatsRecord {
                site_id: 9,
                uid: Some("123".to_string()),
                username: Some("alice".to_string()),
                uploaded: Some(1024),
                downloaded: Some(512),
                ratio: Some(2.0),
                bonus: Some(88.5),
                seeding_count: Some(7),
                leeching_count: Some(1),
                details: UserStatsDetails {
                    level_id: Some(9),
                    level_name: Some("mTorrent Master".to_string()),
                    join_time: Some(1_700_000_000_000),
                    message_count: Some(2),
                    seeding_size: Some(4096),
                    bonus_per_hour: Some(12.5),
                    ..Default::default()
                },
                updated_at: Some("2026-09-04T12:30:00Z".to_string()),
                last_checked_at: "2026-09-04T12:30:00Z".to_string(),
                last_error: None,
            }),
        }
    }

    #[test]
    fn archive_matches_ptd_user_info_and_manifest_layout() {
        let now = DateTime::parse_from_rfc3339("2026-09-04T13:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let (archive, count) = build_ptd_archive(&[sample_site()], &[], now).unwrap();
        assert_eq!(count, 1);

        let mut zip = zip::ZipArchive::new(Cursor::new(archive)).unwrap();
        let mut user_info = String::new();
        zip.by_name("userInfo.json")
            .unwrap()
            .read_to_string(&mut user_info)
            .unwrap();
        let user_info: Value = serde_json::from_str(&user_info).unwrap();
        assert_eq!(user_info["mteam"]["2026-09-04"]["site"], "mteam");
        assert_eq!(user_info["mteam"]["2026-09-04"]["status"], 3);
        assert_eq!(user_info["mteam"]["2026-09-04"]["uploaded"], 1024);
        assert_eq!(user_info["mteam"]["2026-09-04"]["totalTraffic"], 1536);
        assert_eq!(user_info["mteam"]["2026-09-04"]["levelId"], 9);
        assert_eq!(user_info["mteam"]["2026-09-04"]["messageCount"], 2);
        assert_eq!(user_info["mteam"]["2026-09-04"]["seedingSize"], 4096);
        assert_eq!(user_info["mteam"]["2026-09-04"]["seeding"], 7);
        assert_eq!(user_info["mteam"]["2026-09-04"]["leeching"], 1);
        assert_eq!(user_info["mteam"]["2026-09-04"]["bonusPerHour"], 12.5);

        let user_info_bytes = serde_json::to_vec(&user_info).unwrap();
        let mut manifest = String::new();
        zip.by_name("manifest.json")
            .unwrap()
            .read_to_string(&mut manifest)
            .unwrap();
        let manifest: Value = serde_json::from_str(&manifest).unwrap();
        assert_eq!(manifest["encryption"], false);
        assert_eq!(manifest["files"]["userInfo"]["name"], "userInfo.json");
        assert_eq!(
            manifest["files"]["userInfo"]["hash"],
            format!("{:x}", md5::compute(user_info_bytes))
        );
    }

    #[test]
    fn snapshot_exports_every_extended_field_with_ptd_names() {
        let stats = UserStats {
            uid: Some("42".to_string()),
            username: "alice".to_string(),
            uploaded: 100,
            downloaded: 50,
            ratio: Some(2.0),
            bonus: Some(10.0),
            seeding_count: Some(2),
            leeching_count: Some(1),
            details: UserStatsDetails {
                ptd_user_id: Some("account-uuid".to_string()),
                is_donor: Some(true),
                level_id: Some(3),
                level_name: Some("Elite User".to_string()),
                join_time: Some(1_700_000_000_000),
                last_access_at: Some(1_800_000_000_000),
                message_count: Some(4),
                invites: Some(5),
                avatar: Some("https://tracker.example/avatar.png".to_string()),
                total_traffic: Some(150),
                true_downloaded: Some(25),
                true_uploaded: Some(75),
                true_ratio: Some(3.0),
                seeding_size: Some(1024),
                seeding_time: Some(3600),
                average_seeding_time: Some(1800),
                seeding_bonus: Some(20.0),
                bonus_per_hour: Some(1.5),
                seeding_bonus_per_hour: Some(0.5),
                uploads: Some(6),
                snatches: Some(7),
                posts: Some(8),
                adoptions: Some(9),
                hnr_unsatisfied: Some(1),
                hnr_pre_warning: Some(2),
                extra: BTreeMap::from([("customMetric".to_string(), json!(11))]),
            },
        };

        let snapshot = ptd_user_snapshot("mteam", &stats, 1_800_000_000_000);
        assert_eq!(snapshot["id"], "account-uuid");
        assert_eq!(stats.uid.as_deref(), Some("42"));
        for field in [
            "isDonor",
            "levelId",
            "levelName",
            "joinTime",
            "lastAccessAt",
            "messageCount",
            "invites",
            "avatar",
            "totalTraffic",
            "trueDownloaded",
            "trueUploaded",
            "trueRatio",
            "seedingSize",
            "seedingTime",
            "averageSeedingTime",
            "seedingBonus",
            "bonusPerHour",
            "seedingBonusPerHour",
            "uploads",
            "snatches",
            "posts",
            "adoptions",
            "hnrUnsatisfied",
            "hnrPreWarning",
            "customMetric",
        ] {
            assert!(snapshot.get(field).is_some(), "missing PTD field {field}");
        }
        assert!(snapshot.get("message_count").is_none());
        assert_eq!(snapshot["trueRatio"], 3.0);
    }

    #[test]
    fn archive_contains_all_daily_history_and_current_snapshot() {
        let now = DateTime::parse_from_rfc3339("2026-09-05T13:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let history = vec![SiteStatsHistoryRecord {
            site_id: 9,
            snapshot_date: "2026-09-03".to_string(),
            updated_at: "2026-09-03T09:00:00Z".to_string(),
            stats: UserStats {
                uid: Some("123".to_string()),
                username: "alice".to_string(),
                uploaded: 900,
                downloaded: 450,
                ratio: Some(2.0),
                bonus: Some(80.0),
                seeding_count: Some(5),
                leeching_count: None,
                details: UserStatsDetails {
                    message_count: Some(1),
                    ..Default::default()
                },
            },
        }];

        let (archive, count) = build_ptd_archive(&[sample_site()], &history, now).unwrap();
        assert_eq!(count, 1);
        let mut zip = zip::ZipArchive::new(Cursor::new(archive)).unwrap();
        let mut user_info = String::new();
        zip.by_name("userInfo.json")
            .unwrap()
            .read_to_string(&mut user_info)
            .unwrap();
        let user_info: Value = serde_json::from_str(&user_info).unwrap();
        assert_eq!(user_info["mteam"]["2026-09-03"]["uploaded"], 900);
        assert_eq!(user_info["mteam"]["2026-09-04"]["uploaded"], 1024);
    }

    #[test]
    fn ptd_site_id_comes_from_the_official_catalog() {
        let mut site = sample_site();
        assert_eq!(ptd_site_id(&site), Some("mteam"));

        site.site_type = "nexusphp".to_string();
        site.base_url = "https://chdbits.co/".to_string();
        assert_eq!(ptd_site_id(&site), Some("chdbits"));

        site.base_url = "https://tracker.invalid/".to_string();
        assert_eq!(ptd_site_id(&site), None);
    }

    #[test]
    fn archive_skips_sites_that_ptd_does_not_recognize() {
        let mut site = sample_site();
        site.site_type = "nexusphp".to_string();
        site.base_url = "https://tracker.invalid/".to_string();
        let (_, count) = build_ptd_archive(&[site], &[], Utc::now()).unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn successful_refresh_keeps_latest_snapshot_for_each_utc_day() {
        let temp = tempfile::tempdir().unwrap();
        let db = Database::open(temp.path()).await.unwrap();
        let site_id = db
            .create_site(
                "M-Team history",
                "mteam",
                "https://api.m-team.cc",
                r#"{"auth_type":"api_key","api_key":"test"}"#,
                "[]",
                false,
            )
            .await
            .unwrap();
        let mut stats = UserStats {
            uid: Some("42".to_string()),
            username: "alice".to_string(),
            uploaded: 100,
            downloaded: 50,
            details: UserStatsDetails {
                message_count: Some(1),
                ..Default::default()
            },
            ..Default::default()
        };
        db.upsert_site_stats_success(site_id, &stats, "2026-09-04T01:00:00Z")
            .await
            .unwrap();
        stats.uploaded = 200;
        stats.details.message_count = Some(2);
        db.upsert_site_stats_success(site_id, &stats, "2026-09-04T23:00:00Z")
            .await
            .unwrap();
        stats.uploaded = 300;
        db.upsert_site_stats_success(site_id, &stats, "2026-09-05T00:00:00Z")
            .await
            .unwrap();

        let history = db.list_site_stats_history().await.unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].snapshot_date, "2026-09-04");
        assert_eq!(history[0].stats.uploaded, 200);
        assert_eq!(history[0].stats.details.message_count, Some(2));
        assert_eq!(history[1].snapshot_date, "2026-09-05");
        assert_eq!(history[1].stats.uploaded, 300);

        let current = db.list_sites_with_stats().await.unwrap();
        let current = current[0].stats.as_ref().unwrap();
        assert_eq!(current.uploaded, Some(300));
        assert_eq!(current.details.total_traffic, Some(350));
    }

    #[tokio::test]
    async fn database_reopen_backfills_the_pre_history_current_snapshot() {
        let temp = tempfile::tempdir().unwrap();
        let db = Database::open(temp.path()).await.unwrap();
        let site_id = db
            .create_site(
                "M-Team legacy snapshot",
                "mteam",
                "https://api.m-team.cc",
                r#"{"auth_type":"api_key","api_key":"test"}"#,
                "[]",
                false,
            )
            .await
            .unwrap();
        db.upsert_site_stats_success(
            site_id,
            &UserStats {
                uid: Some("42".to_string()),
                username: "alice".to_string(),
                uploaded: 100,
                downloaded: 50,
                details: UserStatsDetails {
                    level_name: Some("Power User".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            },
            "2026-09-03T12:00:00Z",
        )
        .await
        .unwrap();
        drop(db);

        let connection = rusqlite::Connection::open(temp.path().join("kirara.db")).unwrap();
        connection
            .execute("DELETE FROM site_stats_history", [])
            .unwrap();
        drop(connection);

        let reopened = Database::open(temp.path()).await.unwrap();
        let history = reopened.list_site_stats_history().await.unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].snapshot_date, "2026-09-03");
        assert_eq!(
            history[0].stats.details.level_name.as_deref(),
            Some("Power User")
        );
    }

    #[test]
    fn webdav_paths_keep_existing_percent_encoding_and_append_filename() {
        let normalized = normalize_webdav_url("https://dav.example/蜂巢 PTD").unwrap();
        assert_eq!(normalized, "https://dav.example/%E8%9C%82%E5%B7%A2%20PTD/");
        assert_eq!(
            webdav_file_url(&normalized, "PTD_backup_20260904T1200.zip")
                .unwrap()
                .as_str(),
            "https://dav.example/%E8%9C%82%E5%B7%A2%20PTD/PTD_backup_20260904T1200.zip"
        );
    }

    #[tokio::test]
    async fn backup_now_uploads_zip_with_basic_auth_and_records_success() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut content_length = None;
            let mut header_end = None;
            loop {
                let mut buffer = [0u8; 4096];
                let count = stream.read(&mut buffer).await.unwrap();
                if count == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..count]);
                if header_end.is_none()
                    && let Some(position) = request.windows(4).position(|part| part == b"\r\n\r\n")
                {
                    let end = position + 4;
                    let headers = String::from_utf8_lossy(&request[..end]);
                    content_length = headers.lines().find_map(|line| {
                        line.split_once(':').and_then(|(name, value)| {
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())
                                .flatten()
                        })
                    });
                    header_end = Some(end);
                }
                if let (Some(end), Some(length)) = (header_end, content_length)
                    && request.len() >= end + length
                {
                    break;
                }
            }
            stream
                .write_all(
                    b"HTTP/1.1 201 Created\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await
                .unwrap();
            request
        });

        let temp = tempfile::tempdir().unwrap();
        let db = Database::open(temp.path()).await.unwrap();
        let site_id = db
            .create_site(
                "M-Team",
                "mteam",
                "https://api.m-team.cc",
                r#"{"auth_type":"api_key","api_key":"test"}"#,
                "[]",
                false,
            )
            .await
            .unwrap();
        db.upsert_site_stats_success(
            site_id,
            &UserStats {
                uid: Some("123".to_string()),
                username: "alice".to_string(),
                uploaded: 1024,
                downloaded: 512,
                ratio: Some(2.0),
                bonus: Some(88.5),
                seeding_count: Some(7),
                leeching_count: Some(1),
                details: UserStatsDetails {
                    message_count: Some(3),
                    bonus_per_hour: Some(64.844),
                    ..Default::default()
                },
            },
            "2026-09-04T12:30:00Z",
        )
        .await
        .unwrap();
        let history = db.list_site_stats_history().await.unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].snapshot_date, "2026-09-04");
        assert_eq!(history[0].stats.details.message_count, Some(3));
        for (name, site_type, url, count, rate) in [
            (
                "hhanclub",
                "nexusphp",
                "https://www.hhanclub.top",
                213,
                165.614,
            ),
            (
                "greatposterwall",
                "gazelle",
                "https://greatposterwall.com",
                1,
                0.7445,
            ),
        ] {
            let id = db
                .create_site(name, site_type, url, "{}", "[]", false)
                .await
                .unwrap();
            for (date, seeding_count, bonus_per_hour) in [
                ("2026-09-03T12:30:00Z", count + 10, rate + 20.0),
                ("2026-09-04T12:30:00Z", count, rate),
            ] {
                db.upsert_site_stats_success(
                    id,
                    &UserStats {
                        username: "alice".to_string(),
                        seeding_count: Some(seeding_count),
                        details: UserStatsDetails {
                            bonus_per_hour: Some(bonus_per_hour),
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                    date,
                )
                .await
                .unwrap();
            }
        }
        let mut config = db.get_ptd_backup_config().await.unwrap();
        config.webdav_url = format!("http://{address}/dav");
        config.username = "alice".to_string();
        config.password = "secret".to_string();
        db.update_ptd_backup_config(&config).await.unwrap();

        let result = backup_now(&db).await.unwrap();
        assert_eq!(result.site_count, 3);
        assert!(result.filename.starts_with("PTD_backup_"));
        let request = server.await.unwrap();
        let header_end = request
            .windows(4)
            .position(|part| part == b"\r\n\r\n")
            .unwrap()
            + 4;
        let headers = String::from_utf8_lossy(&request[..header_end]).to_ascii_lowercase();
        assert!(headers.starts_with("put /dav/ptd_backup_"));
        assert!(headers.contains("authorization: basic ywxpy2u6c2vjcmv0"));
        assert!(request[header_end..].starts_with(b"PK"));

        // Inspect the bytes received over HTTP, including the database read,
        // historical snapshots, archive serialization and upload body.
        let mut zip = zip::ZipArchive::new(Cursor::new(&request[header_end..])).unwrap();
        let mut payload = Vec::new();
        zip.by_name("userInfo.json")
            .unwrap()
            .read_to_end(&mut payload)
            .unwrap();
        let user_info: Value = serde_json::from_slice(&payload).unwrap();
        for (site, date, count, rate) in [
            ("mteam", "2026-09-04", 7, 64.844),
            ("hhanclub", "2026-09-03", 223, 185.614),
            ("hhanclub", "2026-09-04", 213, 165.614),
            ("greatposterwall", "2026-09-03", 11, 20.7445),
            ("greatposterwall", "2026-09-04", 1, 0.7445),
        ] {
            let snapshot = &user_info[site][date];
            assert_eq!(snapshot["site"], site);
            assert_eq!(snapshot["seeding"], count, "{site}/{date}");
            assert!(
                (snapshot["bonusPerHour"].as_f64().unwrap() - rate).abs() < 1e-9,
                "{site}/{date}"
            );
        }
        let mut manifest = String::new();
        zip.by_name("manifest.json")
            .unwrap()
            .read_to_string(&mut manifest)
            .unwrap();
        let manifest: Value = serde_json::from_str(&manifest).unwrap();
        assert_eq!(
            manifest["files"]["userInfo"]["hash"],
            format!("{:x}", md5::compute(&payload))
        );

        let saved = db.get_ptd_backup_config().await.unwrap();
        assert_eq!(
            saved.last_backup_filename.as_deref(),
            Some(result.filename.as_str())
        );
        assert!(saved.last_error.is_none());
    }
}
