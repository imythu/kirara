use super::{Database, open_connection};

impl Database {
    pub(crate) async fn dav<T: Send + 'static>(
        &self,
        work: impl FnOnce(&mut rusqlite::Connection) -> Result<T, String> + Send + 'static,
    ) -> Result<T, String> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn =
                open_connection(&path).map_err(|_| "无法打开 WebDAV 数据库".to_string())?;
            work(&mut conn)
        })
        .await
        .map_err(|_| "WebDAV 数据库任务中断".to_string())?
    }

    pub(super) async fn init_webdav(&self) -> Result<(), crate::error::AppError> {
        self.dav(|conn| {
            conn.execute_batch("CREATE TABLE IF NOT EXISTS webdav_sync_settings (
                id INTEGER PRIMARY KEY CHECK(id=1), config TEXT NOT NULL,
                running INTEGER NOT NULL DEFAULT 0, runtime_error TEXT);
                CREATE TABLE IF NOT EXISTS webdav_sync_jobs (
                    id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL,
                    body BLOB, digest TEXT NOT NULL, source_time INTEGER,
                    received_at INTEGER NOT NULL, status TEXT NOT NULL DEFAULT 'pending',
                    attempts INTEGER NOT NULL DEFAULT 0, next_retry INTEGER NOT NULL DEFAULT 0,
                    result TEXT, error TEXT);
                CREATE TABLE IF NOT EXISTS webdav_sync_resources (
                    name TEXT PRIMARY KEY, job_id INTEGER NOT NULL REFERENCES webdav_sync_jobs(id));
                CREATE TABLE IF NOT EXISTS webdav_sync_sites (
                    site_id INTEGER PRIMARY KEY REFERENCES sites(id) ON DELETE CASCADE,
                    source_time INTEGER, last_job_id INTEGER NOT NULL);
                CREATE INDEX IF NOT EXISTS webdav_sync_pending ON webdav_sync_jobs(status, next_retry);")
                .map_err(|e| e.to_string())?;
            let config = serde_json::to_string(&crate::webdav::Config::default()).map_err(|e| e.to_string())?;
            conn.execute("INSERT OR IGNORE INTO webdav_sync_settings(id,config) VALUES(1,?)", [config]).map_err(|e| e.to_string())?;
            Ok(())
        }).await.map_err(|message| crate::error::AppError::Database { message })
    }
}
