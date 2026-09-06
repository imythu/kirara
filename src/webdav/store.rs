use super::{Config, import};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub fn sql(e: rusqlite::Error) -> String {
    e.to_string()
}
pub fn config(conn: &Connection) -> Result<Config, String> {
    let json: String = conn
        .query_row(
            "SELECT config FROM webdav_sync_settings WHERE id=1",
            [],
            |r| r.get(0),
        )
        .map_err(sql)?;
    serde_json::from_str(&json).map_err(|_| "WebDAV 配置无效".into())
}
pub fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[derive(Default, Serialize, Deserialize)]
pub struct Report {
    pub created: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub skipped: usize,
    pub details: Vec<String>,
}
impl Report {
    fn skip(&mut self, name: &str, reason: &str) {
        self.skipped += 1;
        self.details.push(format!("{name}：{reason}"));
    }
}

pub fn prune(conn: &Connection) -> Result<(), String> {
    conn.execute("DELETE FROM webdav_sync_resources WHERE job_id NOT IN (SELECT job_id FROM webdav_sync_resources ORDER BY job_id DESC LIMIT 5)", []).map_err(sql)?;
    conn.execute("UPDATE webdav_sync_jobs SET body=NULL WHERE status IN ('done','failed') AND id NOT IN (SELECT job_id FROM webdav_sync_resources)", []).map_err(sql)?;
    conn.execute("DELETE FROM webdav_sync_jobs WHERE status IN ('done','failed') AND id NOT IN (SELECT job_id FROM webdav_sync_resources) AND id NOT IN (SELECT id FROM webdav_sync_jobs ORDER BY id DESC LIMIT 100)", []).map_err(sql)?;
    Ok(())
}

pub fn receive(
    conn: &mut Connection,
    name: String,
    bytes: Vec<u8>,
    source_time: Option<i64>,
    expected_hash: String,
) -> Result<bool, String> {
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(sql)?;
    let cfg = config(&tx)?;
    if !cfg.enabled || cfg.password_hash != expected_hash {
        return Err("接收服务已关闭或密码已重置".into());
    }
    let hash = digest(&bytes);
    let existing: Option<(i64, String)> = tx.query_row("SELECT j.id,j.digest FROM webdav_sync_resources r JOIN webdav_sync_jobs j ON j.id=r.job_id WHERE r.name=?", [&name], |r| Ok((r.get(0)?,r.get(1)?))).optional().map_err(sql)?;
    if existing.as_ref().is_some_and(|(_, old)| old == &hash) {
        return Ok(false);
    }
    prune(&tx)?;
    let pending: i64 = tx
        .query_row(
            "SELECT COUNT(*) FROM webdav_sync_jobs WHERE status IN ('pending','retry')",
            [],
            |r| r.get(0),
        )
        .map_err(sql)?;
    if pending >= 100 {
        return Err("接收存储已满：待处理任务达到 100 条，请等待任务处理".into());
    }
    let used: i64 = tx
        .query_row(
            "SELECT COALESCE(SUM(length(body)),0) FROM webdav_sync_jobs",
            [],
            |r| r.get(0),
        )
        .map_err(sql)?;
    if used + bytes.len() as i64 > 64 * 1024 * 1024 {
        return Err("接收存储已满，请等待任务处理或删除旧文件".into());
    }
    tx.execute(
        "INSERT INTO webdav_sync_jobs(name,body,digest,source_time,received_at) VALUES(?,?,?,?,?)",
        params![
            name,
            bytes,
            hash,
            source_time,
            chrono::Utc::now().timestamp_millis()
        ],
    )
    .map_err(sql)?;
    let id = tx.last_insert_rowid();
    tx.execute("INSERT INTO webdav_sync_resources(name,job_id) VALUES(?,?) ON CONFLICT(name) DO UPDATE SET job_id=excluded.job_id", params![name,id]).map_err(sql)?;
    prune(&tx)?;
    tx.commit().map_err(sql)?;
    Ok(existing.is_none())
}

// One worker per database; selecting and applying in the same SQLite write
// transaction makes crash recovery and retry atomic without expiring leases.
pub fn process_next(conn: &mut Connection) -> Result<bool, String> {
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(sql)?;
    let cfg = config(&tx)?;
    if !cfg.enabled {
        return Ok(false);
    }
    let job: Option<(i64,Vec<u8>,Option<i64>)> = tx.query_row("SELECT id,body,source_time FROM webdav_sync_jobs WHERE status IN ('pending','retry') AND next_retry<=? ORDER BY id LIMIT 1", [chrono::Utc::now().timestamp_millis()], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(sql)?;
    let Some((id, body, source_time)) = job else {
        return Ok(false);
    };
    let data = match import::parse(&body) {
        Ok(data) => data,
        Err(error) => {
            tx.execute(
                "UPDATE webdav_sync_jobs SET status='failed',error=? WHERE id=?",
                params![error, id],
            )
            .map_err(sql)?;
            tx.commit().map_err(sql)?;
            return Ok(true);
        }
    };
    apply_import(&tx, &cfg, &data, id, source_time)?;
    prune(&tx)?;
    tx.commit().map_err(sql)?;
    Ok(true)
}

/// Shared transactional site matching and updates for WebDAV and manual imports.
fn apply_import(
    tx: &Connection,
    cfg: &Config,
    data: &import::ImportData,
    id: i64,
    source_time: Option<i64>,
) -> Result<Report, String> {
    let (candidates, skipped) = import::normalize(data);
    let mut report = Report {
        skipped: skipped.len(),
        details: skipped,
        ..Report::default()
    };
    let mut grouped = std::collections::BTreeMap::<String, Vec<import::Candidate>>::new();
    for c in candidates {
        grouped.entry(c.ptd_id.clone()).or_default().push(c);
    }
    let sites = {
        let mut stmt = tx
            .prepare("SELECT id,base_url,auth_config FROM sites")
            .map_err(sql)?;
        stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })
        .map_err(sql)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(sql)?
    };
    for (key, candidates) in grouped {
        let first = &candidates[0];
        let matches: Vec<_> = sites
            .iter()
            .filter(|(_, url, _)| import::site_key(url).as_deref() == Some(&key))
            .collect();
        if matches.len() > 1 {
            report.skip(&first.name, "匹配到多个已有站点");
            continue;
        }
        let site_id;
        if let Some((existing_id, url, auth)) = matches.first().copied() {
            site_id = *existing_id;
            let previous: Option<(Option<i64>, i64)> = tx
                .query_row(
                    "SELECT source_time,last_job_id FROM webdav_sync_sites WHERE site_id=?",
                    [site_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(sql)?;
            if previous.is_some_and(|(time, job)| {
                job >= id || source_time.zip(time).is_some_and(|(new, old)| new < old)
            }) {
                report.skip(&first.name, "旧数据未覆盖当前 Cookie");
                continue;
            }
            if cfg.existing_policy == "skip" {
                report.skip(&first.name, "保留已有站点");
                continue;
            }
            let Ok(mut auth) = serde_json::from_str::<serde_json::Value>(auth) else {
                report.skip(&first.name, "现有认证配置无效");
                continue;
            };
            if !matches!(
                auth["auth_type"].as_str(),
                Some("cookie" | "cookie_passkey")
            ) {
                report.skip(&first.name, "现有认证方式不使用 Cookie");
                continue;
            }
            // Prefer the actual target host over alternate domains in the same backup.
            let header = candidates
                .iter()
                .filter(|c| import::host(&c.base_url) == import::host(url))
                .chain(candidates.iter())
                .find_map(|c| import::header_for(c, url));
            let Some(header) = header else {
                report.skip(&first.name, "Cookie 不适用于现有访问域名");
                continue;
            };
            if equivalent(auth["cookie"].as_str().unwrap_or(""), &header) {
                report.unchanged += 1;
            } else {
                auth["cookie"] = serde_json::Value::String(header);
                tx.execute(
                    "UPDATE sites SET auth_config=?,updated_at=? WHERE id=?",
                    params![auth.to_string(), chrono::Utc::now().to_rfc3339(), site_id],
                )
                .map_err(sql)?;
                report.updated += 1;
            }
        } else {
            if !cfg.auto_create {
                report.skip(&first.name, "未开启自动添加");
                continue;
            }
            let preset = crate::ptd_site_catalog::SITE_PRESETS
                .iter()
                .find(|p| p.ptd_id == key);
            let candidate = candidates
                .iter()
                .find(|c| {
                    preset.is_some_and(|p| import::host(p.base_url) == import::host(&c.base_url))
                })
                .unwrap_or(first);
            let auth = serde_json::json!({"auth_type":"cookie","cookie":candidate.cookie});
            let headers = serde_json::to_string(&crate::site::default_site_request_headers())
                .map_err(|e| e.to_string())?;
            let now = chrono::Utc::now().to_rfc3339();
            tx.execute("INSERT INTO sites(name,site_type,base_url,auth_config,request_headers,use_proxy,created_at,updated_at) VALUES(?,?,?,?,?,1,?,?)", params![candidate.name,candidate.site_type,candidate.base_url,auth.to_string(),headers,now,now]).map_err(sql)?;
            site_id = tx.last_insert_rowid();
            report.created += 1;
        }
        tx.execute("INSERT INTO webdav_sync_sites(site_id,source_time,last_job_id) VALUES(?,?,?) ON CONFLICT(site_id) DO UPDATE SET source_time=COALESCE(excluded.source_time,source_time),last_job_id=excluded.last_job_id", params![site_id,source_time,id]).map_err(sql)?;
    }
    let result = serde_json::to_string(&report).map_err(|e| e.to_string())?;
    tx.execute(
        "UPDATE webdav_sync_jobs SET status='done',result=?,error=NULL WHERE id=?",
        params![result, id],
    )
    .map_err(sql)?;
    Ok(report)
}

pub fn import_manual(
    conn: &mut Connection,
    data: import::ImportData,
    policy: String,
    auto_create: bool,
) -> Result<Report, String> {
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(sql)?;
    let cfg = Config {
        existing_policy: policy,
        auto_create,
        ..Config::default()
    };
    tx.execute("INSERT INTO webdav_sync_jobs(name,digest,source_time,received_at) VALUES('手动导入 PTD 配置','manual',?,?)", params![data.generated_at,chrono::Utc::now().timestamp_millis()]).map_err(sql)?;
    let id = tx.last_insert_rowid();
    let report = apply_import(&tx, &cfg, &data, id, data.generated_at)?;
    prune(&tx)?;
    tx.commit().map_err(sql)?;
    Ok(report)
}

fn equivalent(a: &str, b: &str) -> bool {
    let split = |v: &str| {
        v.split(';')
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string)
            .collect::<std::collections::BTreeSet<_>>()
    };
    split(a) == split(b)
}
