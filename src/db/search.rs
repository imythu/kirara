use super::*;
use std::collections::HashMap;

fn snapshot_sites(conn: &Connection) -> Result<Vec<SiteWithStats>, AppError> {
    let filter = "";
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
        .query_map([], |row| {
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
                    uploaded: row
                        .get::<_, Option<i64>>(12)
                        .ok()
                        .flatten()
                        .map(|v| v as u64),
                    downloaded: row
                        .get::<_, Option<i64>>(13)
                        .ok()
                        .flatten()
                        .map(|v| v as u64),
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
}
fn snapshot_sign_in_tasks(conn: &Connection) -> Result<Vec<SignInTaskRecord>, AppError> {
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
}
fn snapshot_brush_tasks(conn: &Connection) -> Result<Vec<BrushTaskRecord>, AppError> {
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
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchBinding {
    pub site_id: i64,
    pub mode: String,
    pub catalog_id: Option<String>,
    pub matched_host: Option<String>,
    pub catalog_revision: String,
}

pub struct SearchSnapshot {
    pub sites: Vec<SiteWithStats>,
    pub bindings: HashMap<i64, SearchBinding>,
    pub sign_tasks: Vec<SignInTaskRecord>,
    pub brush_tasks: Vec<BrushTaskRecord>,
    pub records: Vec<SignInRecord>,
}

fn resolve_binding(site_id: i64, url: &str, mode: String, manual: Option<String>) -> SearchBinding {
    let catalog_id = if mode == "manual" {
        manual
    } else {
        crate::search::resolve_catalog(url, &mode, None)
    };
    SearchBinding {
        site_id,
        matched_host: if mode == "auto" && catalog_id.is_some() {
            crate::search::normalize_host(url)
        } else {
            None
        },
        mode,
        catalog_id,
        catalog_revision: crate::search::catalog_revision().to_owned(),
    }
}

fn local_search_snapshot(conn: &Connection, scope: &str) -> Result<SearchSnapshot, AppError> {
    let sites = snapshot_sites(conn)?;
    let mut bindings = HashMap::new();
    {
        let mut stmt = conn.prepare("SELECT s.id,s.base_url,COALESCE(b.mode,'auto'),b.catalog_id FROM sites s LEFT JOIN site_search_bindings b ON b.site_id=s.id").map_err(sql_error)?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<String>>(3)?,
                ))
            })
            .map_err(sql_error)?;
        for row in rows {
            let (id, url, mode, catalog) = row.map_err(sql_error)?;
            bindings.insert(id, resolve_binding(id, &url, mode, catalog));
        }
    }
    let sign_tasks = if matches!(scope, "sign" | "records") {
        snapshot_sign_in_tasks(conn)?
    } else {
        Vec::new()
    };
    let brush_tasks = if scope == "brush" {
        snapshot_brush_tasks(conn)?
    } else {
        Vec::new()
    };
    Ok(SearchSnapshot {
        sites,
        bindings,
        sign_tasks,
        brush_tasks,
        records: Vec::new(),
    })
}

impl Database {
    /// Capture current identity, names and status together; release SQLite before matching.
    pub fn search_snapshot_blocking(&self, scope: &str) -> Result<SearchSnapshot, AppError> {
        let mut conn = open_connection(&self.path)?;
        let tx = conn.transaction().map_err(sql_error)?;
        let snapshot = local_search_snapshot(&tx, scope)?;
        tx.commit().map_err(sql_error)?;
        Ok(snapshot)
    }

    /// Full history exists only as a test oracle for streaming equivalence.
    #[cfg(test)]
    pub async fn search_snapshot(
        &self,
        scope: &str,
        task_id: Option<i64>,
        site_id: Option<i64>,
    ) -> Result<SearchSnapshot, AppError> {
        let path = self.path.clone();
        let scope = scope.to_owned();
        tokio::task::spawn_blocking(move || {
            let mut conn = open_connection(&path)?;
            let tx = conn.transaction().map_err(sql_error)?;
            let mut snapshot = local_search_snapshot(&tx, &scope)?;
            if scope == "records" {
                let mut stmt = tx.prepare("SELECT id,task_id,site_id,site_name,started_at,finished_at,status,message FROM sign_in_records WHERE (?1 IS NULL OR task_id=?1) AND (?2 IS NULL OR site_id=?2) ORDER BY id DESC").map_err(sql_error)?;
                let rows = stmt.query_map(params![task_id,site_id],map_sign_in_record).map_err(sql_error)?;
                for row in rows { snapshot.records.push(row.map_err(sql_error)?); }
            }
            tx.commit().map_err(sql_error)?;
            Ok(snapshot)
        }).await.map_err(join_error)?
    }

    pub async fn search_binding(
        &self,
        site_id: i64,
        update: Option<(String, Option<String>)>,
    ) -> Result<Option<SearchBinding>, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn=open_connection(&path)?;
            let tx=conn.transaction().map_err(sql_error)?;
            let url:Option<String>=tx.query_row("SELECT base_url FROM sites WHERE id=?",[site_id],|r|r.get(0)).optional().map_err(sql_error)?;
            let Some(url)=url else { return Ok(None) };
            let (mode,catalog)=match update { Some(value)=>value,None=>tx.query_row("SELECT mode,catalog_id FROM site_search_bindings WHERE site_id=?",[site_id],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(sql_error)?.unwrap_or(("auto".to_owned(),None)) };
            let binding=resolve_binding(site_id,&url,mode,catalog);
            tx.execute("INSERT INTO site_search_bindings(site_id,mode,catalog_id,matched_host,catalog_revision) VALUES (?1,?2,?3,?4,?5) ON CONFLICT(site_id) DO UPDATE SET mode=excluded.mode,catalog_id=excluded.catalog_id,matched_host=excluded.matched_host,catalog_revision=excluded.catalog_revision",params![binding.site_id,binding.mode,binding.catalog_id,binding.matched_host,binding.catalog_revision]).map_err(sql_error)?;
            tx.commit().map_err(sql_error)?;
            Ok(Some(binding))
        }).await.map_err(join_error)?
    }
}

/// Remove only the legacy display-name uniqueness constraint, preserving FK targets.
pub(super) fn migrate_site_names(conn: &Connection) -> Result<(), AppError> {
    let schema: String = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name='sites'",
            [],
            |r| r.get(0),
        )
        .map_err(sql_error)?;
    if !schema.contains("name TEXT NOT NULL UNIQUE") {
        return Ok(());
    }
    let replacement = schema
        .replacen(
            "CREATE TABLE sites",
            "CREATE TABLE sites_search_migration",
            1,
        )
        .replace("name TEXT NOT NULL UNIQUE", "name TEXT NOT NULL");
    let sequence: Option<i64> = conn
        .query_row(
            "SELECT seq FROM sqlite_sequence WHERE name='sites'",
            [],
            |r| r.get(0),
        )
        .optional()
        .map_err(sql_error)?;
    let objects: Vec<String> = {
        let mut stmt=conn.prepare("SELECT sql FROM sqlite_master WHERE tbl_name='sites' AND type IN ('index','trigger') AND sql IS NOT NULL").map_err(sql_error)?;
        let rows = stmt.query_map([], |r| r.get(0)).map_err(sql_error)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(sql_error)?
    };
    // Keep the original table name in all dependent FK definitions. Never rename the old table.
    conn.execute_batch("PRAGMA foreign_keys=OFF; BEGIN IMMEDIATE")
        .map_err(sql_error)?;
    let migration = (|| {
        conn.execute_batch(&replacement).map_err(sql_error)?;
        conn.execute_batch("INSERT INTO sites_search_migration SELECT * FROM sites; DROP TABLE sites; ALTER TABLE sites_search_migration RENAME TO sites;").map_err(sql_error)?;
        for sql in &objects {
            conn.execute_batch(sql).map_err(sql_error)?;
        }
        if let Some(sequence) = sequence {
            conn.execute(
                "UPDATE sqlite_sequence SET seq=MAX(seq,?1) WHERE name='sites'",
                [sequence],
            )
            .map_err(sql_error)?;
        }
        let violations: i64 = conn
            .query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| {
                r.get(0)
            })
            .map_err(sql_error)?;
        if violations != 0 {
            return Err(AppError::Database {
                message: "site-name migration would violate foreign keys".to_owned(),
            });
        }
        conn.execute_batch("COMMIT").map_err(sql_error)
    })();
    if migration.is_err() {
        let _ = conn.execute_batch("ROLLBACK");
    }
    conn.execute_batch("PRAGMA foreign_keys=ON")
        .map_err(sql_error)?;
    migration
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn search_snapshot_preserves_history_and_current_task_truth() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(dir.path()).await.unwrap();
        let conn = open_connection(&db.path).unwrap();
        conn.execute("INSERT INTO sites(id,name,site_type,base_url,auth_config,created_at,updated_at) VALUES(1,'Same','nexusphp','https://unknown.invalid','secret','now','now'),(2,'Same','nexusphp','https://other.invalid','secret','now','now')",[]).unwrap();
        conn.execute("INSERT INTO sign_in_tasks(id,name,site_id,cron_expression,lightpanda_token,enabled,last_status,created_at,updated_at) VALUES(1,'Current',1,'0 * * * * *','',1,'success','now','now')",[]).unwrap();
        for index in 0..520 {
            conn.execute("INSERT INTO sign_in_records(task_id,site_id,site_name,started_at,finished_at,status,message) VALUES(1,1,'Historical','now','now','failed',?1)",[format!("history {index}")]).unwrap();
        }
        let snapshot = db
            .search_snapshot("records", Some(1), Some(1))
            .await
            .unwrap();
        assert_eq!(snapshot.records.len(), 520);
        assert_eq!(snapshot.sites.len(), 2);
        assert_eq!(
            db.list_sign_in_records(None, 1000).await.unwrap().len(),
            500
        );
        let snapshot = db.search_snapshot("sign", None, None).await.unwrap();
        assert_eq!(
            snapshot.sign_tasks[0].last_status.as_deref(),
            Some("success")
        );
        assert!(snapshot.sign_tasks[0].enabled);
        let binding = db
            .search_binding(1, Some(("none".into(), None)))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(binding.mode, "none");
        conn.execute(
            "UPDATE sites SET base_url='https://new.invalid',name='Renamed' WHERE id=1",
            [],
        )
        .unwrap();
        let reopened = Database::open(dir.path()).await.unwrap();
        assert_eq!(
            reopened
                .search_binding(1, None)
                .await
                .unwrap()
                .unwrap()
                .mode,
            "none"
        );
        assert_eq!(
            reopened
                .search_snapshot("sites", None, None)
                .await
                .unwrap()
                .sites[0]
                .name,
            "Renamed"
        );
        conn.execute("DELETE FROM sites WHERE id=2", []).unwrap();
        assert_eq!(
            db.search_snapshot("sites", None, None)
                .await
                .unwrap()
                .sites
                .len(),
            1
        );
    }
    #[tokio::test]
    async fn automatic_binding_tracks_domain_and_manual_binding_survives_edits() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(dir.path()).await.unwrap();
        let entry = crate::search::catalog()
            .iter()
            .find(|entry| {
                crate::search::resolve_catalog(&entry.url, "auto", None).as_deref()
                    == Some(entry.id.as_str())
            })
            .unwrap();
        let id = db
            .create_site("Custom", "nexusphp", &entry.url, "{}", "[]", false)
            .await
            .unwrap();
        let first = db.search_binding(id, None).await.unwrap().unwrap();
        assert_eq!(first.catalog_id.as_deref(), Some(entry.id.as_str()));
        assert!(first.matched_host.is_some());
        let conn = open_connection(&db.path).unwrap();
        conn.execute(
            "UPDATE sites SET base_url='https://unknown.invalid',name='Renamed' WHERE id=?1",
            [id],
        )
        .unwrap();
        assert!(
            db.search_binding(id, None)
                .await
                .unwrap()
                .unwrap()
                .catalog_id
                .is_none()
        );
        db.search_binding(id, Some(("manual".into(), Some(entry.id.clone()))))
            .await
            .unwrap();
        conn.execute(
            "UPDATE sites SET base_url='https://another.invalid' WHERE id=?1",
            [id],
        )
        .unwrap();
        let reopened = Database::open(dir.path()).await.unwrap();
        let manual = reopened.search_binding(id, None).await.unwrap().unwrap();
        assert_eq!(manual.catalog_id.as_deref(), Some(entry.id.as_str()));
        assert_eq!(manual.mode, "manual");
        assert!(manual.matched_host.is_none());
        conn.execute("DELETE FROM sites WHERE id=?1", [id]).unwrap();
        assert!(reopened.search_binding(id, None).await.unwrap().is_none());
    }

    #[test]
    fn legacy_name_migration_keeps_foreign_keys_and_id_high_water() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE sites(id INTEGER PRIMARY KEY AUTOINCREMENT,name TEXT NOT NULL UNIQUE); CREATE TABLE tasks(id INTEGER PRIMARY KEY,site_id INTEGER REFERENCES sites(id) ON DELETE CASCADE); CREATE TABLE stats(site_id INTEGER PRIMARY KEY REFERENCES sites(id) ON DELETE CASCADE, uploaded INTEGER);INSERT INTO sites VALUES(1,'original'),(99,'deleted');DELETE FROM sites WHERE id=99;INSERT INTO tasks VALUES(5,1);INSERT INTO stats VALUES(1,42);").unwrap();
        migrate_site_names(&conn).unwrap();
        migrate_site_names(&conn).unwrap();
        conn.execute("INSERT INTO sites(name) VALUES('original')", [])
            .unwrap();
        assert_eq!(conn.last_insert_rowid(), 100);
        assert_eq!(
            conn.query_row("SELECT site_id FROM tasks WHERE id=5", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            conn.query_row("SELECT uploaded FROM stats WHERE site_id=1", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            42
        );
        assert!(conn.execute("INSERT INTO tasks VALUES(6,999)", []).is_err());
        conn.execute("DELETE FROM sites WHERE id=1", []).unwrap();
        assert_eq!(
            conn.query_row("SELECT count(*) FROM tasks", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
}

/// A bounded history scan shares one read transaction and uses a private, disk-backed
/// SQLite TEMP table for global ranking. Closing this connection removes that table.
pub struct HistorySearchSession<'a> {
    conn: &'a Connection,
    pub snapshot: SearchSnapshot,
    task_id: Option<i64>,
    site_id: Option<i64>,
}

impl Database {
    pub fn with_history_search<T, E: From<AppError>>(
        &self,
        task_id: Option<i64>,
        site_id: Option<i64>,
        deadline: std::time::Instant,
        operation: impl FnOnce(HistorySearchSession<'_>) -> Result<T, E>,
    ) -> Result<T, E> {
        let mut conn = open_connection(&self.path)?;
        conn.progress_handler(1000, Some(move || std::time::Instant::now() >= deadline));
        conn.execute_batch("PRAGMA temp_store=FILE; PRAGMA temp.cache_size=-2048; CREATE TEMP TABLE search_hits(id INTEGER PRIMARY KEY, rank INTEGER NOT NULL, similarity REAL NOT NULL, reasons TEXT NOT NULL)").map_err(sql_error)?;
        let tx = conn.transaction().map_err(sql_error)?;
        let snapshot = local_search_snapshot(&tx, "records")?;
        let result = operation(HistorySearchSession {
            conn: &tx,
            snapshot,
            task_id,
            site_id,
        })?;
        tx.commit().map_err(sql_error)?;
        Ok(result)
    }
}

impl HistorySearchSession<'_> {
    pub fn store_all_scoped_hits(&self) -> Result<(), AppError> {
        self.conn.execute("INSERT INTO temp.search_hits(id,rank,similarity,reasons) SELECT id,0,0,'[]' FROM sign_in_records WHERE (?1 IS NULL OR task_id=?1) AND (?2 IS NULL OR site_id=?2)",params![self.task_id,self.site_id]).map_err(sql_error)?;
        Ok(())
    }

    pub fn batch(
        &self,
        before: Option<i64>,
        names_only: bool,
    ) -> Result<Vec<SignInRecord>, AppError> {
        let columns = if names_only {
            "id,task_id,site_id,site_name,'','','',''"
        } else {
            "id,task_id,site_id,site_name,started_at,finished_at,status,message"
        };
        let task_scope = if self.task_id.is_some() {
            "task_id=?1"
        } else {
            "1"
        };
        let site_scope = if self.site_id.is_some() {
            "site_id=?2"
        } else {
            "1"
        };
        let mut stmt=self.conn.prepare(&format!("SELECT {columns} FROM sign_in_records WHERE {task_scope} AND {site_scope} AND (?3 IS NULL OR id<?3) ORDER BY id DESC LIMIT 256")).map_err(sql_error)?;
        let rows = stmt
            .query_map(
                params![self.task_id, self.site_id, before],
                map_sign_in_record,
            )
            .map_err(sql_error)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(sql_error)
    }

    pub fn store_hits(&self, hits: &[crate::search::SearchHit]) -> Result<(), AppError> {
        let mut stmt = self
            .conn
            .prepare_cached(
                "INSERT INTO temp.search_hits(id,rank,similarity,reasons) VALUES(?1,?2,?3,?4)",
            )
            .map_err(sql_error)?;
        for hit in hits {
            let reasons =
                serde_json::to_string(&hit.matched_by).map_err(|error| AppError::Database {
                    message: error.to_string(),
                })?;
            stmt.execute(params![hit.id, hit.rank, hit.similarity, reasons])
                .map_err(sql_error)?;
        }
        Ok(())
    }

    pub fn page(
        &self,
        requested: usize,
        page_size: usize,
        empty_query: bool,
    ) -> Result<(usize, usize, Vec<(SignInRecord, Vec<String>)>), AppError> {
        let total: i64 = self
            .conn
            .query_row("SELECT count(*) FROM temp.search_hits", [], |r| r.get(0))
            .map_err(sql_error)?;
        let total = total as usize;
        let page = requested.min(total.div_ceil(page_size).max(1));
        let order = if empty_query {
            "h.id DESC"
        } else {
            "h.rank,h.similarity DESC,h.id"
        };
        let mut stmt=self.conn.prepare(&format!("WITH selected AS MATERIALIZED (SELECT h.* FROM temp.search_hits h ORDER BY {order} LIMIT ?1 OFFSET ?2) SELECT r.id,r.task_id,r.site_id,r.site_name,r.started_at,r.finished_at,r.status,r.message,h.reasons FROM selected h JOIN sign_in_records r ON r.id=h.id ORDER BY {order}")).map_err(sql_error)?;
        let rows = stmt
            .query_map(
                params![page_size as i64, ((page - 1) * page_size) as i64],
                |row| {
                    let record = map_sign_in_record(row)?;
                    let reasons: String = row.get(8)?;
                    Ok((record, serde_json::from_str(&reasons).unwrap_or_default()))
                },
            )
            .map_err(sql_error)?;
        Ok((
            total,
            page,
            rows.collect::<Result<Vec<_>, _>>().map_err(sql_error)?,
        ))
    }
}
