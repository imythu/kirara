pub fn normalize_path(value: &str) -> Result<String, String> {
    let value = value.trim().replace('\\', "/");
    if value.is_empty() {
        return Err("路径不能为空".to_string());
    }
    let absolute = value.starts_with('/');
    let mut parts = Vec::new();
    for part in value.split('/') {
        match part {
            "" | "." => {}
            ".." => return Err("路径不能包含 ..".to_string()),
            other => parts.push(other),
        }
    }
    if parts.is_empty() {
        return Ok("/".to_string());
    }
    Ok(format!(
        "{}{}",
        if absolute { "/" } else { "" },
        parts.join("/")
    ))
}

pub fn is_path_prefix(prefix: &str, path: &str) -> bool {
    prefix == "/"
        || path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|tail| tail.starts_with('/'))
}

pub fn translate_path(path: &str, from: &str, to: &str) -> Result<String, String> {
    let path = normalize_path(path)?;
    let from = normalize_path(from)?;
    let to = normalize_path(to)?;
    if !is_path_prefix(&from, &path) {
        return Err(format!("路径 {path} 不在映射根目录 {from} 下"));
    }
    let suffix = if from == "/" {
        path.trim_start_matches('/')
    } else {
        path.strip_prefix(&from)
            .unwrap_or_default()
            .trim_start_matches('/')
    };
    if suffix.is_empty() {
        Ok(to)
    } else if to == "/" {
        Ok(format!("/{suffix}"))
    } else {
        Ok(format!("{to}/{suffix}"))
    }
}

#[allow(dead_code)]
pub fn validate_non_overlapping(paths: &[String]) -> Result<(), String> {
    let mut normalized = paths
        .iter()
        .map(|path| normalize_path(path))
        .collect::<Result<Vec<_>, _>>()?;
    normalized.sort();
    for (index, left) in normalized.iter().enumerate() {
        for right in normalized.iter().skip(index + 1) {
            if is_path_prefix(left, right) || is_path_prefix(right, left) {
                return Err(format!("路径映射重叠: {left} 与 {right}"));
            }
        }
    }
    Ok(())
}

fn encode_openlist_task_ids(ids: impl IntoIterator<Item = String>) -> Option<String> {
    let ids = ids
        .into_iter()
        .filter(|id| !id.trim().is_empty())
        .collect::<Vec<_>>();
    (!ids.is_empty())
        .then(|| serde_json::to_string(&ids).ok())
        .flatten()
}

fn decode_openlist_task_ids(value: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(value)
        .unwrap_or_else(|_| vec![value.to_string()])
        .into_iter()
        .filter(|id| !id.trim().is_empty())
        .collect()
}

async fn require_openlist_tasks_terminal(
    openlist: &OpenListClient,
    job: &MediaRelocationJob,
) -> Result<(), String> {
    let task_ids = job
        .openlist_task_id
        .as_deref()
        .map(decode_openlist_task_ids)
        .unwrap_or_default();
    for task_id in task_ids {
        match openlist.task_info_if_exists(&task_id).await? {
            Some(task) if task.succeeded() || task.terminal_failure() => {}
            Some(_) => {
                return Err(format!(
                    "OpenList 任务 {task_id} 仍在运行，不能恢复源文件清理"
                ));
            }
            None => {
                return Err(format!(
                    "OpenList 任务 {task_id} 不可见，无法证明任务已终止，不能恢复源文件清理"
                ));
            }
        }
    }
    Ok(())
}

#[allow(dead_code)]
pub fn category_directory(media_type: &str, year: Option<u32>) -> String {
    match media_type.trim().to_ascii_lowercase().as_str() {
        "movie" | "电影" => "电影".to_string(),
        "anime" | "动漫" => "动漫".to_string(),
        "concert" | "演唱会" => "演唱会".to_string(),
        "year" | "年份" => year
            .map(|v| v.to_string())
            .unwrap_or_else(|| "年份".to_string()),
        "tv" | "电视" | "电视剧" => "电视剧".to_string(),
        _ => "电视剧".to_string(),
    }
}

#[allow(dead_code)]
pub fn category_year_directory(media_type: &str, year: Option<u32>) -> String {
    let category = category_directory(media_type, None);
    year.map(|year| format!("{category}/{year}"))
        .unwrap_or(category)
}

pub fn join_path(root: &str, child: &str) -> Result<String, String> {
    let root = normalize_path(root)?;
    let child = normalize_path(child)?;
    Ok(if root == "/" {
        format!("/{}", child.trim_start_matches('/'))
    } else {
        format!("{}/{}", root, child.trim_start_matches('/'))
    })
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashMap, VecDeque};
    use std::future::Future;
    use std::path::PathBuf;
    use std::pin::Pin;
    use std::sync::{Arc, Mutex};

    use axum::extract::{Query, State};
    use axum::{Json, Router, routing::post};
    use serde_json::{Value, json};
    use tempfile::TempDir;
    use tokio::sync::mpsc;

    use super::*;
    use crate::downloader::DownloaderTestResult;

    const TEST_INFOHASH: &str = "eadb91a4769b1fad89e0dd3a930523e7fc5814b8";

    #[derive(Default)]
    struct NoopDownloader {
        torrent_results: Mutex<VecDeque<Result<Vec<TorrentInfo>, String>>>,
        torrent_file_results: Mutex<VecDeque<Result<Vec<TorrentFileInfo>, String>>>,
        export_calls: Mutex<usize>,
    }

    impl NoopDownloader {
        fn with_torrents(torrents: Vec<TorrentInfo>) -> Self {
            Self::with_torrent_results([Ok(torrents)])
        }

        fn with_torrent_results(
            results: impl IntoIterator<Item = Result<Vec<TorrentInfo>, String>>,
        ) -> Self {
            Self {
                torrent_results: Mutex::new(results.into_iter().collect()),
                torrent_file_results: Mutex::new(VecDeque::new()),
                export_calls: Mutex::new(0),
            }
        }

        fn queue_torrents(&self, torrents: Vec<TorrentInfo>, count: u32) {
            self.torrent_results
                .lock()
                .unwrap()
                .extend((0..count).map(|_| Ok(torrents.clone())));
        }

        fn queue_torrent_files(&self, files: Vec<TorrentFileInfo>, count: u32) {
            self.torrent_file_results
                .lock()
                .unwrap()
                .extend((0..count).map(|_| Ok(files.clone())));
        }

        fn export_calls(&self) -> usize {
            *self.export_calls.lock().unwrap()
        }
    }

    impl DownloaderClient for NoopDownloader {
        fn test_connection(
            &self,
        ) -> Pin<Box<dyn Future<Output = Result<DownloaderTestResult, String>> + Send + '_>>
        {
            Box::pin(async { Err("unexpected downloader call".to_string()) })
        }

        fn add_torrent(
            &self,
            _torrent_data: Vec<u8>,
            _filename: &str,
            _options: &AddTorrentOptions,
        ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>> {
            Box::pin(async { Err("unexpected downloader call".to_string()) })
        }

        fn list_torrents(
            &self,
            _tag: Option<&str>,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<TorrentInfo>, String>> + Send + '_>> {
            let result = self
                .torrent_results
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Err("unexpected downloader call".to_string()));
            Box::pin(async move { result })
        }

        fn pause_torrent(
            &self,
            _hash: &str,
        ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>> {
            Box::pin(async { Err("unexpected downloader call".to_string()) })
        }

        fn delete_torrent(
            &self,
            _hash: &str,
            _delete_files: bool,
        ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>> {
            Box::pin(async { Err("unexpected downloader call".to_string()) })
        }

        fn get_free_space(
            &self,
            _path: Option<&str>,
        ) -> Pin<Box<dyn Future<Output = Result<u64, String>> + Send + '_>> {
            Box::pin(async { Err("unexpected downloader call".to_string()) })
        }

        fn get_default_save_path(
            &self,
        ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + '_>> {
            Box::pin(async { Err("unexpected downloader call".to_string()) })
        }

        fn get_torrent_trackers(
            &self,
            _hash: &str,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, String>> + Send + '_>> {
            Box::pin(async { Err("unexpected downloader call".to_string()) })
        }

        fn add_torrent_tags(
            &self,
            _hashes: Vec<String>,
            _tags: Vec<String>,
        ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>> {
            Box::pin(async { Err("unexpected downloader call".to_string()) })
        }

        fn export_torrent(
            &self,
            _hash: &str,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, String>> + Send + '_>> {
            *self.export_calls.lock().unwrap() += 1;
            Box::pin(async { Err("simulated torrent export failure".to_string()) })
        }

        fn get_torrent_files(
            &self,
            _hash: &str,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<TorrentFileInfo>, String>> + Send + '_>>
        {
            let result = self
                .torrent_file_results
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Err("unexpected torrent files call".to_string()));
            Box::pin(async move { result })
        }
    }

    struct FakeOpenListState {
        objects: BTreeMap<String, Value>,
        copy_posts: usize,
        mkdir_posts: usize,
        remove_posts: usize,
        hang_copy: bool,
        hang_mkdir: bool,
        hang_remove: bool,
        remove_applies: bool,
        task_missing: bool,
        task_state: i64,
        task_error: String,
        events: mpsc::UnboundedSender<&'static str>,
    }

    type SharedFakeOpenListState = Arc<Mutex<FakeOpenListState>>;

    fn fake_directory(name: &str) -> Value {
        json!({"name": name, "size": 0, "is_dir": true})
    }

    fn fake_file(name: &str, size: i64) -> Value {
        json!({
            "name": name,
            "size": size,
            "is_dir": false,
            "hash_info": {"sha1": "1111111111111111111111111111111111111111"}
        })
    }

    fn fake_success(data: Value) -> Json<Value> {
        Json(json!({"code": 200, "message": "success", "data": data}))
    }

    fn fake_missing() -> Json<Value> {
        Json(json!({"code": 500, "message": "object not found", "data": null}))
    }

    async fn fake_openlist_get(
        State(state): State<SharedFakeOpenListState>,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        let path = body.get("path").and_then(Value::as_str).unwrap_or_default();
        state
            .lock()
            .unwrap()
            .objects
            .get(path)
            .cloned()
            .map(fake_success)
            .unwrap_or_else(fake_missing)
    }

    async fn fake_openlist_list(
        State(state): State<SharedFakeOpenListState>,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        let parent = body.get("path").and_then(Value::as_str).unwrap_or("/");
        let prefix = if parent == "/" {
            "/".to_string()
        } else {
            format!("{}/", parent.trim_end_matches('/'))
        };
        let state = state.lock().unwrap();
        let mut direct_children = BTreeMap::new();
        for (path, value) in &state.objects {
            let Some(child) = path.strip_prefix(&prefix) else {
                continue;
            };
            if child.is_empty() {
                continue;
            }
            if let Some((directory, _)) = child.split_once('/') {
                direct_children
                    .entry(directory.to_string())
                    .or_insert_with(|| fake_directory(directory));
            } else {
                direct_children.insert(child.to_string(), value.clone());
            }
        }
        let content = direct_children.into_values().collect::<Vec<_>>();
        fake_success(json!({"total": content.len(), "content": content}))
    }

    async fn fake_openlist_copy(
        State(state): State<SharedFakeOpenListState>,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        assert_eq!(body.get("overwrite").and_then(Value::as_bool), Some(false));
        assert_eq!(
            body.get("skip_existing").and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(body.get("merge").and_then(Value::as_bool), Some(false));
        let hang = {
            let mut state = state.lock().unwrap();
            state.copy_posts += 1;
            let _ = state.events.send("copy");
            state.hang_copy
        };
        if hang {
            std::future::pending::<()>().await;
            unreachable!();
        }
        fake_success(json!({
            "tasks": [{"id": "copy-task", "state": 0}]
        }))
    }

    async fn fake_openlist_mkdir(
        State(state): State<SharedFakeOpenListState>,
        Json(_body): Json<Value>,
    ) -> Json<Value> {
        let hang = {
            let mut state = state.lock().unwrap();
            state.mkdir_posts += 1;
            let _ = state.events.send("mkdir");
            state.hang_mkdir
        };
        if hang {
            std::future::pending::<()>().await;
            unreachable!();
        }
        fake_success(Value::Null)
    }

    async fn fake_openlist_remove(
        State(state): State<SharedFakeOpenListState>,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        let directory = body
            .get("dir")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim_end_matches('/')
            .to_string();
        let names = body
            .get("names")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let hang = {
            let mut state = state.lock().unwrap();
            state.remove_posts += 1;
            if state.remove_applies {
                for name in names.iter().filter_map(Value::as_str) {
                    let path = if directory.is_empty() || directory == "/" {
                        format!("/{}", name.trim_start_matches('/'))
                    } else {
                        format!("{directory}/{}", name.trim_start_matches('/'))
                    };
                    state.objects.remove(&path);
                }
            }
            let _ = state.events.send("remove");
            state.hang_remove
        };
        if hang {
            std::future::pending::<()>().await;
            unreachable!();
        }
        fake_success(Value::Null)
    }

    async fn fake_openlist_task(
        State(state): State<SharedFakeOpenListState>,
        Query(_query): Query<HashMap<String, String>>,
    ) -> Json<Value> {
        let state = state.lock().unwrap();
        if state.task_missing {
            Json(json!({
                "code": 404,
                "message": "task not found",
                "data": null
            }))
        } else {
            fake_success(json!({
                "id": "copy-task",
                "state": state.task_state,
                "error": state.task_error.clone(),
            }))
        }
    }

    struct RelocationTestHarness {
        _directory: TempDir,
        db_path: PathBuf,
        db: Database,
        scheduler: Arc<RelocationScheduler>,
        config: OpenListConfig,
        downloader_id: i64,
        downloader: Arc<NoopDownloader>,
        state: SharedFakeOpenListState,
        events: mpsc::UnboundedReceiver<&'static str>,
        server: tokio::task::JoinHandle<()>,
    }

    impl RelocationTestHarness {
        async fn new(objects: BTreeMap<String, Value>) -> Self {
            let (event_sender, events) = mpsc::unbounded_channel();
            let state = Arc::new(Mutex::new(FakeOpenListState {
                objects,
                copy_posts: 0,
                mkdir_posts: 0,
                remove_posts: 0,
                hang_copy: false,
                hang_mkdir: false,
                hang_remove: false,
                remove_applies: false,
                task_missing: false,
                task_state: 0,
                task_error: String::new(),
                events: event_sender,
            }));
            let app = Router::new()
                .route("/api/fs/get", post(fake_openlist_get))
                .route("/api/fs/list", post(fake_openlist_list))
                .route("/api/fs/copy", post(fake_openlist_copy))
                .route("/api/fs/mkdir", post(fake_openlist_mkdir))
                .route("/api/fs/remove", post(fake_openlist_remove))
                .route("/api/task/copy/info", post(fake_openlist_task))
                .with_state(state.clone());
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

            let directory = tempfile::tempdir().unwrap();
            let db_path = directory.path().join("kirara.db");
            let db = Database::open(directory.path()).await.unwrap();
            let downloader_id = db
                .create_downloader(
                    "relocation-test",
                    "qbittorrent",
                    "http://unused.invalid",
                    "",
                    "",
                )
                .await
                .unwrap();
            let downloader = db.get_downloader(downloader_id).await.unwrap().unwrap();
            let pool = DownloaderClientPool::new(db.clone());
            let client = Arc::new(NoopDownloader::default());
            pool.insert_for_test(&downloader, None, client.clone())
                .await;
            let scheduler = RelocationScheduler::new(db.clone(), pool, true);
            let config_version = db.get_openlist_config().await.unwrap().updated_at;
            let config = OpenListConfig {
                base_url: format!("http://{address}"),
                api_key: "test-key".to_string(),
                enabled: true,
                target_directory_id: None,
                selected_target_index: None,
                scan_interval_secs: 60,
                updated_at: config_version,
                path_mappings: Vec::new(),
                target_directories: Vec::new(),
            };
            Self {
                _directory: directory,
                db_path,
                db,
                scheduler,
                config,
                downloader_id,
                downloader: client,
                state,
                events,
                server,
            }
        }

        async fn seed_job(
            &self,
            stage: &str,
            checkpoint: Option<CopyCheckpoint>,
            openlist_task_id: Option<&str>,
            manifest_path: &str,
            manifest_cursor: usize,
        ) -> MediaRelocationJob {
            let (inserted, skipped) = self
                .db
                .enqueue_manual_media_relocation_jobs(
                    self.downloader_id,
                    self.downloader_id,
                    "/dst",
                    "/target-qb",
                    &[(TEST_INFOHASH.to_string(), "episode".to_string())],
                )
                .await
                .unwrap();
            assert_eq!((inserted, skipped), (1, 0));
            let mut job = self
                .db
                .claim_due_media_relocation_jobs("fixture-setup", 120, 1, true)
                .await
                .unwrap()
                .pop()
                .unwrap();
            let expected_version = job.version;
            let expected_stage = job.stage.clone();
            job.source_qb_path = "/source-qb".to_string();
            job.source_openlist_path = "/src".to_string();
            job.source_content_openlist_path = "/src".to_string();
            job.target_openlist_path = "/dst".to_string();
            job.target_qb_path = "/target-qb".to_string();
            job.target_content_qb_path =
                format!("/target-qb/{}", manifest_path.trim_start_matches('/'));
            job.target_downloader_id = Some(self.downloader_id);
            job.copy_items_json = "[\"episode.mkv\"]".to_string();
            job.source_files_json = serde_json::to_string(&[manifest_path]).unwrap();
            job.source_manifest_json = serde_json::to_string(&[ManifestFile {
                path: manifest_path.to_string(),
                size: 10,
            }])
            .unwrap();
            job.copy_checkpoint_json = checkpoint
                .as_ref()
                .map(encode_copy_checkpoint)
                .transpose()
                .unwrap();
            job.copy_lock_acquired = true;
            job.manifest_cursor = manifest_cursor;
            job.target_root_folder = Some(false);
            job.stage = stage.to_string();
            job.openlist_task_id = openlist_task_id.map(str::to_string);
            job.next_attempt_at = None;
            assert!(
                self.db
                    .update_media_relocation_job(&job, expected_version, &expected_stage, None,)
                    .await
                    .unwrap()
            );
            self.db
                .claim_due_media_relocation_jobs("fixture-run", 120, 1, true)
                .await
                .unwrap()
                .pop()
                .unwrap()
        }

        fn make_due_after_restart(&self, job_id: i64) {
            let connection = rusqlite::Connection::open(&self.db_path).unwrap();
            connection
                .execute(
                    "UPDATE media_relocation_jobs
                     SET next_attempt_at=?, lease_owner=NULL, lease_until=NULL
                     WHERE id=?",
                    rusqlite::params![Utc::now().to_rfc3339(), job_id],
                )
                .unwrap();
        }

        async fn claim_job_after_restart(&self, job_id: i64) -> MediaRelocationJob {
            self.make_due_after_restart(job_id);
            let jobs = self
                .db
                .claim_due_media_relocation_jobs("fixture-restart", 120, 10, true)
                .await
                .unwrap();
            jobs.into_iter()
                .find(|job| job.id == job_id)
                .unwrap_or_else(|| panic!("job {job_id} was not claimable after restart"))
        }

        async fn stored_job(&self, job_id: i64) -> MediaRelocationJob {
            self.db
                .get_media_relocation_job(job_id)
                .await
                .unwrap()
                .unwrap()
        }

        async fn process(&self, job: MediaRelocationJob) {
            self.scheduler.process_one(&self.config, job).await.unwrap();
        }

        async fn abort_worker_after_event(
            &mut self,
            job: MediaRelocationJob,
            expected_event: &'static str,
        ) {
            let scheduler = self.scheduler.clone();
            let config = self.config.clone();
            let worker = tokio::spawn(async move { scheduler.process_one(&config, job).await });
            let event = tokio::time::timeout(Duration::from_secs(5), self.events.recv())
                .await
                .expect("OpenList side effect was not called")
                .expect("fake OpenList event channel closed");
            assert_eq!(event, expected_event);
            worker.abort();
            assert!(worker.await.unwrap_err().is_cancelled());
        }

        fn post_counts(&self) -> (usize, usize) {
            let state = self.state.lock().unwrap();
            (state.copy_posts, state.mkdir_posts)
        }

        fn remove_posts(&self) -> usize {
            self.state.lock().unwrap().remove_posts
        }
    }

    impl Drop for RelocationTestHarness {
        fn drop(&mut self) {
            self.server.abort();
        }
    }

    fn copy_checkpoint(
        path: &str,
        size: i64,
        operation: CopyCheckpointOperation,
        phase: CopyCheckpointPhase,
    ) -> CopyCheckpoint {
        let submitted_at =
            (phase == CopyCheckpointPhase::Uncertain).then(|| Utc::now().to_rfc3339());
        CopyCheckpoint {
            path: path.to_string(),
            size,
            operation,
            phase,
            submitted_at,
            terminal_failure_verified: false,
        }
    }

    fn flat_copy_objects() -> BTreeMap<String, Value> {
        BTreeMap::from([
            ("/src/episode.mkv".to_string(), fake_file("episode.mkv", 10)),
            ("/dst".to_string(), fake_directory("dst")),
        ])
    }

    fn torrent_with_state(state: &str, progress: f64) -> TorrentInfo {
        TorrentInfo {
            hash: TEST_INFOHASH.to_string(),
            name: "episode".to_string(),
            size: 10,
            uploaded: 0,
            downloaded: 10,
            amount_left: None,
            completed: None,
            progress,
            upload_speed: 0,
            download_speed: 0,
            ratio: 0.0,
            state: state.to_string(),
            added_on: 0,
            completion_on: 1,
            num_seeds: 0,
            num_leechs: 0,
            save_path: "/target-qb".to_string(),
            root_path: String::new(),
            content_path: "/target-qb/episode.mkv".to_string(),
            tags: String::new(),
            category: String::new(),
            time_active: 0,
            last_activity: 0,
        }
    }

    fn source_removal_objects(source_present: bool) -> BTreeMap<String, Value> {
        let mut objects = BTreeMap::from([
            ("/src".to_string(), fake_directory("src")),
            ("/dst".to_string(), fake_directory("dst")),
            ("/dst/episode.mkv".to_string(), fake_file("episode.mkv", 10)),
        ]);
        if source_present {
            objects.insert("/src/episode.mkv".to_string(), fake_file("episode.mkv", 10));
        }
        objects
    }

    fn complete_torrent_file() -> TorrentFileInfo {
        TorrentFileInfo {
            path: "episode.mkv".to_string(),
            size: 10,
            progress: 1.0,
            is_seed: true,
        }
    }

    fn queue_source_removal_observations(
        harness: &RelocationTestHarness,
        torrent: TorrentInfo,
        passes: u32,
    ) {
        // Every removal pass reads the target by hash and may separately scan the source qB.
        harness
            .downloader
            .queue_torrents(vec![torrent], passes.saturating_mul(2));
        harness
            .downloader
            .queue_torrent_files(vec![complete_torrent_file()], passes);
    }

    #[tokio::test]
    async fn manifest_recheck_waits_for_source_recovery_then_reenters_read_only_reconcile() {
        let harness = RelocationTestHarness::new(source_removal_objects(true)).await;
        let mut job = harness
            .seed_job("manifest_recheck", None, None, "episode.mkv", 0)
            .await;
        let job_id = job.id;
        job.source_manifest_json = "[]".to_string();

        harness.process(job).await;
        let blocked = harness.stored_job(job_id).await;
        assert_eq!(blocked.stage, "manifest_required");
        assert_eq!(blocked.next_attempt_at, None);
        assert!(
            blocked
                .last_error
                .as_deref()
                .is_some_and(|error| error.contains("查询源 qB"))
        );
        assert_eq!(harness.post_counts(), (0, 0));
        assert_eq!(harness.remove_posts(), 0);

        harness
            .downloader
            .queue_torrents(vec![torrent_with_state("stalledUP", 1.0)], 1);
        harness
            .downloader
            .queue_torrent_files(vec![complete_torrent_file()], 1);
        assert!(
            harness
                .db
                .recheck_media_relocation_manifest(job_id, blocked.version)
                .await
                .unwrap()
        );
        let recheck = harness.claim_job_after_restart(job_id).await;
        assert_eq!(recheck.stage, "manifest_recheck");
        harness.process(recheck).await;

        let reconcile = harness.claim_job_after_restart(job_id).await;
        assert_eq!(reconcile.stage, "copy_legacy_reconcile");
        harness.process(reconcile).await;
        let verified = harness.stored_job(job_id).await;
        assert_eq!(verified.stage, "copy_verified");
        assert!(
            decode_source_manifest(&verified.source_manifest_json)
                .unwrap()
                .is_some()
        );
        assert_eq!(verified.manifest_cursor, 1);
        assert_eq!(verified.copy_checkpoint_json, None);
        assert_eq!(harness.post_counts(), (0, 0));
        assert_eq!(harness.remove_posts(), 0);
    }

    #[tokio::test]
    async fn manifest_recheck_preserves_uncertain_remove_without_resubmitting() {
        let harness = RelocationTestHarness::new(source_removal_objects(true)).await;
        let checkpoint = copy_checkpoint(
            "episode.mkv",
            10,
            CopyCheckpointOperation::RemoveFile,
            CopyCheckpointPhase::Uncertain,
        );
        let checkpoint_json = encode_copy_checkpoint(&checkpoint).unwrap();
        let mut job = harness
            .seed_job("manifest_recheck", Some(checkpoint), None, "episode.mkv", 0)
            .await;
        let job_id = job.id;
        job.source_manifest_json = "[]".to_string();
        harness
            .downloader
            .queue_torrents(vec![torrent_with_state("stalledUP", 1.0)], 1);
        harness
            .downloader
            .queue_torrent_files(vec![complete_torrent_file()], 1);

        harness.process(job).await;
        let recovered = harness.stored_job(job_id).await;
        assert_eq!(recovered.stage, "source_removing");
        assert_eq!(
            recovered.copy_checkpoint_json.as_deref(),
            Some(checkpoint_json.as_str())
        );
        assert_eq!(recovered.manifest_cursor, 0);
        assert_eq!(harness.remove_posts(), 0);

        queue_source_removal_observations(&harness, torrent_with_state("uploading", 1.0), 1);
        let resumed = harness.claim_job_after_restart(job_id).await;
        harness.process(resumed).await;
        let observed = harness.stored_job(job_id).await;
        assert_eq!(observed.stage, "source_removing");
        assert_eq!(
            observed.copy_checkpoint_json.as_deref(),
            Some(checkpoint_json.as_str())
        );
        assert_eq!(observed.manifest_cursor, 0);
        assert_eq!(harness.remove_posts(), 0);
    }

    #[tokio::test]
    async fn manifest_recheck_never_resumes_removal_without_proven_terminal_tasks() {
        let harness = RelocationTestHarness::new(source_removal_objects(true)).await;
        let checkpoint = copy_checkpoint(
            "episode.mkv",
            10,
            CopyCheckpointOperation::RemoveFile,
            CopyCheckpointPhase::Uncertain,
        );
        let checkpoint_json = encode_copy_checkpoint(&checkpoint).unwrap();
        let job = harness
            .seed_job(
                "manifest_recheck",
                Some(checkpoint),
                Some("copy-task"),
                "episode.mkv",
                0,
            )
            .await;
        let job_id = job.id;

        harness.process(job).await;
        let blocked = harness.stored_job(job_id).await;
        assert_eq!(blocked.stage, "manifest_required");
        assert_eq!(
            blocked.copy_checkpoint_json.as_deref(),
            Some(checkpoint_json.as_str())
        );
        assert_eq!(blocked.openlist_task_id.as_deref(), Some("copy-task"));
        assert!(
            blocked
                .last_error
                .as_deref()
                .is_some_and(|error| error.contains("仍在运行"))
        );
        assert_eq!(harness.post_counts(), (0, 0));
        assert_eq!(harness.remove_posts(), 0);

        harness.state.lock().unwrap().task_missing = true;
        assert!(
            harness
                .db
                .recheck_media_relocation_manifest(job_id, blocked.version)
                .await
                .unwrap()
        );
        let missing_task = harness.claim_job_after_restart(job_id).await;
        harness.process(missing_task).await;
        let still_blocked = harness.stored_job(job_id).await;
        assert_eq!(still_blocked.stage, "manifest_required");
        assert_eq!(
            still_blocked.copy_checkpoint_json.as_deref(),
            Some(checkpoint_json.as_str())
        );
        assert!(
            still_blocked
                .last_error
                .as_deref()
                .is_some_and(|error| error.contains("无法证明任务已终止"))
        );
        assert_eq!(harness.post_counts(), (0, 0));
        assert_eq!(harness.remove_posts(), 0);
    }

    #[tokio::test]
    async fn remove_post_crash_persists_uncertain_checkpoint_and_never_resubmits() {
        let mut harness = RelocationTestHarness::new(source_removal_objects(true)).await;
        queue_source_removal_observations(&harness, torrent_with_state("uploading", 1.0), 4);
        {
            let mut state = harness.state.lock().unwrap();
            state.hang_remove = true;
            state.remove_applies = false;
        }
        let job = harness
            .seed_job("source_removing", None, None, "episode.mkv", 0)
            .await;
        let job_id = job.id;

        harness.abort_worker_after_event(job, "remove").await;

        let stored = harness.stored_job(job_id).await;
        let checkpoint =
            decode_copy_checkpoint(stored.copy_checkpoint_json.as_deref().unwrap()).unwrap();
        assert_eq!(stored.stage, "source_removing");
        assert_eq!(checkpoint.operation, CopyCheckpointOperation::RemoveFile);
        assert_eq!(checkpoint.phase, CopyCheckpointPhase::Uncertain);
        assert!(checkpoint.submitted_at.is_some());
        assert_eq!(harness.remove_posts(), 1);
        harness.state.lock().unwrap().hang_remove = false;

        for _ in 0..3 {
            let recovered = harness.claim_job_after_restart(job_id).await;
            harness.process(recovered).await;
            let stored = harness.stored_job(job_id).await;
            let checkpoint =
                decode_copy_checkpoint(stored.copy_checkpoint_json.as_deref().unwrap()).unwrap();
            assert_eq!(stored.stage, "source_removing");
            assert_eq!(stored.manifest_cursor, 0);
            assert_eq!(checkpoint.operation, CopyCheckpointOperation::RemoveFile);
            assert_eq!(checkpoint.phase, CopyCheckpointPhase::Uncertain);
            assert_eq!(harness.remove_posts(), 1);
        }
    }

    #[tokio::test]
    async fn remove_post_crash_with_remote_deletion_advances_without_resubmitting() {
        let mut harness = RelocationTestHarness::new(source_removal_objects(true)).await;
        queue_source_removal_observations(&harness, torrent_with_state("uploading", 1.0), 3);
        {
            let mut state = harness.state.lock().unwrap();
            state.hang_remove = true;
            state.remove_applies = true;
        }
        let job = harness
            .seed_job("source_removing", None, None, "episode.mkv", 0)
            .await;
        let job_id = job.id;

        harness.abort_worker_after_event(job, "remove").await;
        assert_eq!(harness.remove_posts(), 1);
        assert!(
            !harness
                .state
                .lock()
                .unwrap()
                .objects
                .contains_key("/src/episode.mkv")
        );
        harness.state.lock().unwrap().hang_remove = false;

        let recovered = harness.claim_job_after_restart(job_id).await;
        harness.process(recovered).await;
        let advanced = harness.stored_job(job_id).await;
        assert_eq!(advanced.stage, "source_removing");
        assert_eq!(advanced.manifest_cursor, 1);
        assert_eq!(advanced.copy_checkpoint_json, None);
        assert_eq!(harness.remove_posts(), 1);

        let recovered = harness.claim_job_after_restart(job_id).await;
        harness.process(recovered).await;
        let removed = harness.stored_job(job_id).await;
        assert_eq!(removed.stage, "source_removed");
        assert_eq!(removed.manifest_cursor, 1);
        assert_eq!(removed.copy_checkpoint_json, None);
        assert_eq!(harness.remove_posts(), 1);
    }

    #[tokio::test]
    async fn successful_remove_ack_without_remote_deletion_keeps_uncertain_checkpoint() {
        let harness = RelocationTestHarness::new(source_removal_objects(true)).await;
        queue_source_removal_observations(&harness, torrent_with_state("uploading", 1.0), 2);
        let job = harness
            .seed_job("source_removing", None, None, "episode.mkv", 0)
            .await;
        let job_id = job.id;

        harness.process(job).await;

        let acknowledged = harness.stored_job(job_id).await;
        let checkpoint =
            decode_copy_checkpoint(acknowledged.copy_checkpoint_json.as_deref().unwrap()).unwrap();
        assert_eq!(acknowledged.stage, "source_removing");
        assert_eq!(acknowledged.manifest_cursor, 0);
        assert_eq!(checkpoint.operation, CopyCheckpointOperation::RemoveFile);
        assert_eq!(checkpoint.phase, CopyCheckpointPhase::Uncertain);
        assert_eq!(harness.remove_posts(), 1);

        let recovered = harness.claim_job_after_restart(job_id).await;
        harness.process(recovered).await;
        let still_visible = harness.stored_job(job_id).await;
        assert_eq!(still_visible.manifest_cursor, 0);
        assert!(still_visible.copy_checkpoint_json.is_some());
        assert_eq!(harness.remove_posts(), 1);
    }

    #[tokio::test]
    async fn successful_remove_ack_advances_only_after_readback_proves_absence() {
        let harness = RelocationTestHarness::new(source_removal_objects(true)).await;
        queue_source_removal_observations(&harness, torrent_with_state("uploading", 1.0), 2);
        harness.state.lock().unwrap().remove_applies = true;
        let job = harness
            .seed_job("source_removing", None, None, "episode.mkv", 0)
            .await;
        let job_id = job.id;

        harness.process(job).await;
        let acknowledged = harness.stored_job(job_id).await;
        assert_eq!(acknowledged.manifest_cursor, 0);
        assert!(acknowledged.copy_checkpoint_json.is_some());
        assert_eq!(harness.remove_posts(), 1);

        let recovered = harness.claim_job_after_restart(job_id).await;
        harness.process(recovered).await;
        let verified = harness.stored_job(job_id).await;
        assert_eq!(verified.manifest_cursor, 1);
        assert_eq!(verified.copy_checkpoint_json, None);
        assert_eq!(harness.remove_posts(), 1);
    }

    #[tokio::test]
    async fn uncertain_remove_that_stays_visible_times_out_to_manual_review() {
        let mut harness = RelocationTestHarness::new(source_removal_objects(true)).await;
        queue_source_removal_observations(&harness, torrent_with_state("uploading", 1.0), 2);
        {
            let mut state = harness.state.lock().unwrap();
            state.hang_remove = true;
            state.remove_applies = false;
        }
        let job = harness
            .seed_job("source_removing", None, None, "episode.mkv", 0)
            .await;
        let job_id = job.id;
        harness.abort_worker_after_event(job, "remove").await;
        assert_eq!(harness.remove_posts(), 1);

        let stored = harness.stored_job(job_id).await;
        let mut checkpoint =
            decode_copy_checkpoint(stored.copy_checkpoint_json.as_deref().unwrap()).unwrap();
        checkpoint.submitted_at = Some(
            (Utc::now() - ChronoDuration::seconds(SOURCE_REMOVAL_ATTENTION_SECONDS + 1))
                .to_rfc3339(),
        );
        let checkpoint_json = encode_copy_checkpoint(&checkpoint).unwrap();
        rusqlite::Connection::open(&harness.db_path)
            .unwrap()
            .execute(
                "UPDATE media_relocation_jobs SET copy_checkpoint_json=? WHERE id=?",
                rusqlite::params![checkpoint_json, job_id],
            )
            .unwrap();
        harness.state.lock().unwrap().hang_remove = false;

        let recovered = harness.claim_job_after_restart(job_id).await;
        harness.process(recovered).await;
        let review = harness.stored_job(job_id).await;
        assert_eq!(review.stage, "source_remove_manual_review");
        assert_eq!(review.manifest_cursor, 0);
        assert!(
            review
                .last_error
                .as_deref()
                .is_some_and(|error| error.contains("删除请求结果不确定"))
        );
        assert_eq!(harness.remove_posts(), 1);
    }

    #[tokio::test]
    async fn invalid_remove_checkpoints_never_trigger_openlist_side_effects() {
        for checkpoint in [
            copy_checkpoint(
                "episode.mkv",
                10,
                CopyCheckpointOperation::RemoveFile,
                CopyCheckpointPhase::Prepared,
            ),
            copy_checkpoint(
                "episode.mkv",
                10,
                CopyCheckpointOperation::CopyFile,
                CopyCheckpointPhase::Uncertain,
            ),
        ] {
            let harness = RelocationTestHarness::new(source_removal_objects(true)).await;
            queue_source_removal_observations(&harness, torrent_with_state("uploading", 1.0), 1);
            let job = harness
                .seed_job("source_removing", Some(checkpoint), None, "episode.mkv", 0)
                .await;

            let error = harness
                .scheduler
                .process_one(&harness.config, job)
                .await
                .unwrap_err();
            assert!(error.contains("无效的副作用 checkpoint"));
            assert_eq!(harness.remove_posts(), 0);
        }
    }

    #[tokio::test]
    async fn target_qb_must_remain_complete_and_seeding_before_every_remove() {
        for (torrent, expected_error) in [
            (torrent_with_state("uploading", 0.5), "不再处于完整做种状态"),
            (torrent_with_state("pausedUP", 1.0), "不再处于完整做种状态"),
        ] {
            let harness = RelocationTestHarness::new(source_removal_objects(true)).await;
            queue_source_removal_observations(&harness, torrent, 1);
            let job = harness
                .seed_job("source_removing", None, None, "episode.mkv", 0)
                .await;
            let job_id = job.id;

            harness.process(job).await;
            let review = harness.stored_job(job_id).await;
            assert_eq!(review.stage, "source_remove_manual_review");
            assert_eq!(review.manifest_cursor, 0);
            assert!(
                review
                    .last_error
                    .as_deref()
                    .is_some_and(|error| error.contains(expected_error))
            );
            assert_eq!(harness.remove_posts(), 0);
        }
    }

    #[tokio::test]
    async fn exported_v2_torrent_validates_full_and_qb_truncated_sha256_infohash() {
        let harness = RelocationTestHarness::new(BTreeMap::new()).await;
        let mut job = harness
            .seed_job("copy_verified", None, None, "file.bin", 0)
            .await;
        job.infohash =
            "7642e218cb07f7bcd3925ba60c51885773cee91fefb492511fbe0e8f17e59602".to_string();
        job.source_manifest_json = serde_json::to_string(&[ManifestFile {
            path: "file.bin".to_string(),
            size: 12,
        }])
        .unwrap();
        let torrent =
            b"d4:infod9:file treed8:file.bind0:d6:lengthi12eeee12:meta versioni2e4:name8:file.binee";

        validate_exported_torrent(&job, torrent).unwrap();
        job.infohash = "7642e218cb07f7bcd3925ba60c51885773cee91f".to_string();
        validate_exported_torrent(&job, torrent).unwrap();
    }

    #[tokio::test]
    async fn waiting_download_hard_error_stops_without_polling_forever() {
        let harness = RelocationTestHarness::new(BTreeMap::new()).await;
        let job = harness
            .seed_job("waiting_download", None, None, "episode.mkv", 0)
            .await;
        let job_id = job.id;
        harness
            .downloader
            .queue_torrents(vec![torrent_with_state("missingFiles", 1.0)], 1);

        harness.process(job).await;

        let stored = harness.stored_job(job_id).await;
        assert_eq!(stored.stage, "qb_manual_review");
        assert!(
            stored
                .last_error
                .as_deref()
                .is_some_and(|error| error.contains("停止无限等待"))
        );
        assert!(!stored.copy_lock_acquired);
        assert_eq!(harness.post_counts(), (0, 0));

        let mut automatic = stored;
        set_waiting_download_manual_review(&mut automatic, false, "hard error".to_string());
        assert_eq!(automatic.stage, "planning_manual_review");
    }

    #[tokio::test]
    async fn waiting_download_known_state_has_a_finite_deadline() {
        let harness = RelocationTestHarness::new(BTreeMap::new()).await;
        let mut job = harness
            .seed_job("waiting_download", None, None, "episode.mkv", 0)
            .await;
        let job_id = job.id;
        job.stage_started_at = (Utc::now()
            - ChronoDuration::seconds(WAITING_DOWNLOAD_TIMEOUT_SECONDS + 1))
        .to_rfc3339();
        let mut downloading = torrent_with_state("downloading", 0.5);
        downloading.downloaded = 5;
        harness.downloader.queue_torrents(vec![downloading], 1);

        harness.process(job).await;

        let stored = harness.stored_job(job_id).await;
        assert_eq!(stored.stage, "qb_manual_review");
        assert!(
            stored
                .last_error
                .as_deref()
                .is_some_and(|error| error.contains("超过 7 天"))
        );
        assert_eq!(harness.post_counts(), (0, 0));
    }

    #[tokio::test]
    async fn copy_post_crash_recovers_uncertain_without_resubmitting() {
        let mut harness = RelocationTestHarness::new(flat_copy_objects()).await;
        let checkpoint = copy_checkpoint(
            "episode.mkv",
            10,
            CopyCheckpointOperation::CopyFile,
            CopyCheckpointPhase::Prepared,
        );
        let job = harness
            .seed_job("copy_submitting", Some(checkpoint), None, "episode.mkv", 0)
            .await;
        let job_id = job.id;
        harness.state.lock().unwrap().hang_copy = true;

        harness.abort_worker_after_event(job, "copy").await;

        let stored = harness.stored_job(job_id).await;
        let stored_checkpoint =
            decode_copy_checkpoint(stored.copy_checkpoint_json.as_deref().unwrap()).unwrap();
        assert_eq!(stored.stage, "copy_submitting");
        assert_eq!(stored_checkpoint.phase, CopyCheckpointPhase::Uncertain);
        assert!(stored_checkpoint.submitted_at.is_some());
        assert_eq!(harness.post_counts(), (1, 0));
        harness.state.lock().unwrap().hang_copy = false;

        for _ in 0..3 {
            let recovered = harness.claim_job_after_restart(job_id).await;
            harness.process(recovered).await;
            assert_eq!(harness.post_counts(), (1, 0));
            let stored = harness.stored_job(job_id).await;
            let checkpoint =
                decode_copy_checkpoint(stored.copy_checkpoint_json.as_deref().unwrap()).unwrap();
            assert_eq!(stored.stage, "copy_submitting");
            assert_eq!(checkpoint.phase, CopyCheckpointPhase::Uncertain);
        }
    }

    #[tokio::test]
    async fn mkdir_post_crash_recovers_uncertain_without_resubmitting() {
        let objects = BTreeMap::from([
            ("/src/season".to_string(), fake_directory("season")),
            (
                "/src/season/episode.mkv".to_string(),
                fake_file("episode.mkv", 10),
            ),
            ("/dst".to_string(), fake_directory("dst")),
        ]);
        let mut harness = RelocationTestHarness::new(objects).await;
        let checkpoint = copy_checkpoint(
            "/dst/season",
            0,
            CopyCheckpointOperation::CreateDirectory,
            CopyCheckpointPhase::Prepared,
        );
        let job = harness
            .seed_job(
                "copy_submitting",
                Some(checkpoint),
                None,
                "season/episode.mkv",
                0,
            )
            .await;
        let job_id = job.id;
        harness.state.lock().unwrap().hang_mkdir = true;

        harness.abort_worker_after_event(job, "mkdir").await;

        let stored = harness.stored_job(job_id).await;
        let stored_checkpoint =
            decode_copy_checkpoint(stored.copy_checkpoint_json.as_deref().unwrap()).unwrap();
        assert_eq!(stored.stage, "copy_submitting");
        assert_eq!(stored_checkpoint.phase, CopyCheckpointPhase::Uncertain);
        assert_eq!(
            stored_checkpoint.operation,
            CopyCheckpointOperation::CreateDirectory
        );
        assert_eq!(harness.post_counts(), (0, 1));
        harness.state.lock().unwrap().hang_mkdir = false;

        for _ in 0..3 {
            let recovered = harness.claim_job_after_restart(job_id).await;
            harness.process(recovered).await;
            assert_eq!(harness.post_counts(), (0, 1));
            let stored = harness.stored_job(job_id).await;
            let checkpoint =
                decode_copy_checkpoint(stored.copy_checkpoint_json.as_deref().unwrap()).unwrap();
            assert_eq!(stored.stage, "copy_submitting");
            assert_eq!(checkpoint.phase, CopyCheckpointPhase::Uncertain);
            assert_eq!(
                checkpoint.operation,
                CopyCheckpointOperation::CreateDirectory
            );
        }
    }

    #[tokio::test]
    async fn missing_openlist_task_stops_and_keeps_the_target_lock() {
        let harness = RelocationTestHarness::new(flat_copy_objects()).await;
        let checkpoint = copy_checkpoint(
            "episode.mkv",
            10,
            CopyCheckpointOperation::CopyFile,
            CopyCheckpointPhase::Uncertain,
        );
        let job = harness
            .seed_job(
                "copying",
                Some(checkpoint),
                Some("copy-task"),
                "episode.mkv",
                0,
            )
            .await;
        let job_id = job.id;
        harness.state.lock().unwrap().task_missing = true;

        harness.process(job).await;
        assert_eq!(harness.post_counts(), (0, 0));
        let stored = harness.stored_job(job_id).await;
        let checkpoint =
            decode_copy_checkpoint(stored.copy_checkpoint_json.as_deref().unwrap()).unwrap();
        assert_eq!(stored.stage, "copy_manual_review");
        assert!(stored.copy_lock_acquired);
        assert_eq!(stored.openlist_task_id.as_deref(), Some("copy-task"));
        assert_eq!(checkpoint.phase, CopyCheckpointPhase::Uncertain);
        assert!(
            stored
                .last_error
                .as_deref()
                .is_some_and(|error| error.contains("无法证明远端任务已终止"))
        );
    }

    #[tokio::test]
    async fn final_manifest_verification_never_releases_a_lock_for_a_missing_task() {
        let harness = RelocationTestHarness::new(source_removal_objects(true)).await;
        let job = harness
            .seed_job("copy_reconcile", None, Some("copy-task"), "episode.mkv", 1)
            .await;
        let job_id = job.id;
        harness.state.lock().unwrap().task_missing = true;

        harness.process(job).await;

        let stored = harness.stored_job(job_id).await;
        assert_eq!(stored.stage, "copy_manual_review");
        assert!(stored.copy_lock_acquired);
        assert_eq!(stored.openlist_task_id.as_deref(), Some("copy-task"));
        assert!(
            stored
                .last_error
                .as_deref()
                .is_some_and(|error| error.contains("任务尚未证明终止"))
        );
        assert_eq!(harness.post_counts(), (0, 0));
    }

    #[tokio::test]
    async fn terminally_failed_copy_task_records_explicit_safe_retry_evidence() {
        let harness = RelocationTestHarness::new(flat_copy_objects()).await;
        let checkpoint = copy_checkpoint(
            "episode.mkv",
            10,
            CopyCheckpointOperation::CopyFile,
            CopyCheckpointPhase::Uncertain,
        );
        let job = harness
            .seed_job(
                "copying",
                Some(checkpoint),
                Some("copy-task"),
                "episode.mkv",
                0,
            )
            .await;
        let job_id = job.id;
        {
            let mut state = harness.state.lock().unwrap();
            state.task_state = 4;
            state.task_error = "remote copy rejected".to_string();
        }

        harness.process(job).await;

        let stored = harness.stored_job(job_id).await;
        let checkpoint =
            decode_copy_checkpoint(stored.copy_checkpoint_json.as_deref().unwrap()).unwrap();
        assert_eq!(stored.stage, "copy_manual_review");
        assert!(stored.copy_lock_acquired);
        assert_eq!(stored.openlist_task_id.as_deref(), Some("copy-task"));
        assert!(checkpoint.terminal_failure_verified);
        assert!(
            stored
                .last_error
                .as_deref()
                .is_some_and(|error| error.contains("remote copy rejected"))
        );
        assert_eq!(harness.post_counts(), (0, 0));
    }

    #[tokio::test]
    async fn legacy_incomplete_manifest_uses_review_existing_and_recheck_stays_read_only() {
        let harness = RelocationTestHarness::new(flat_copy_objects()).await;
        let job = harness
            .seed_job(
                "copy_succeeded",
                None,
                Some("legacy-task"),
                "episode.mkv",
                1,
            )
            .await;
        let job_id = job.id;

        harness.process(job).await;
        let legacy = harness.claim_job_after_restart(job_id).await;
        assert_eq!(legacy.stage, "copy_legacy_reconcile");
        harness.process(legacy).await;

        let review = harness.stored_job(job_id).await;
        let checkpoint =
            decode_copy_checkpoint(review.copy_checkpoint_json.as_deref().unwrap()).unwrap();
        assert_eq!(review.stage, "copy_manual_review");
        assert_eq!(
            checkpoint.operation,
            CopyCheckpointOperation::ReviewExisting
        );
        assert_eq!(harness.post_counts(), (0, 0));

        assert!(
            harness
                .db
                .resolve_media_relocation_copy(job_id, "recheck", review.version, false)
                .await
                .unwrap()
        );
        let recheck = harness.claim_job_after_restart(job_id).await;
        assert_eq!(recheck.stage, "copy_submitting");
        harness.process(recheck).await;

        let reviewed_again = harness.stored_job(job_id).await;
        assert_eq!(reviewed_again.stage, "copy_manual_review");
        assert_eq!(harness.post_counts(), (0, 0));
    }

    #[tokio::test]
    async fn final_incomplete_manifest_uses_review_existing_and_recheck_stays_read_only() {
        let harness = RelocationTestHarness::new(flat_copy_objects()).await;
        let job = harness
            .seed_job("copy_reconcile", None, None, "episode.mkv", 1)
            .await;
        let job_id = job.id;

        harness.process(job).await;
        let review = harness.stored_job(job_id).await;
        let checkpoint =
            decode_copy_checkpoint(review.copy_checkpoint_json.as_deref().unwrap()).unwrap();
        assert_eq!(review.stage, "copy_manual_review");
        assert_eq!(
            checkpoint.operation,
            CopyCheckpointOperation::ReviewExisting
        );
        assert_eq!(harness.post_counts(), (0, 0));

        assert!(
            harness
                .db
                .resolve_media_relocation_copy(job_id, "recheck", review.version, false)
                .await
                .unwrap()
        );
        let recheck = harness.claim_job_after_restart(job_id).await;
        harness.process(recheck).await;

        let reviewed_again = harness.stored_job(job_id).await;
        assert_eq!(reviewed_again.stage, "copy_manual_review");
        assert_eq!(harness.post_counts(), (0, 0));
    }

    #[tokio::test]
    async fn verified_copy_releases_lock_before_qb_export_failures_and_retries_without_copying() {
        let mut objects = flat_copy_objects();
        objects.insert("/dst/episode.mkv".to_string(), fake_file("episode.mkv", 10));
        let harness = RelocationTestHarness::new(objects).await;
        let job = harness
            .seed_job("copy_reconcile", None, None, "episode.mkv", 1)
            .await;
        let job_id = job.id;

        harness.process(job).await;
        let verified = harness.stored_job(job_id).await;
        assert_eq!(verified.stage, "copy_verified");
        assert!(!verified.copy_lock_acquired);
        assert_eq!(verified.copy_checkpoint_json, None);
        assert_eq!(verified.openlist_task_id, None);
        assert_eq!(harness.post_counts(), (0, 0));

        let mut source_torrent = torrent_with_state("stalledUP", 1.0);
        source_torrent.save_path = "/source-qb".to_string();
        source_torrent.content_path = "/source-qb/episode.mkv".to_string();
        harness
            .downloader
            .queue_torrents(vec![source_torrent], MAX_STAGE_ERROR_ATTEMPTS);

        for attempt in 1..=MAX_STAGE_ERROR_ATTEMPTS {
            let claimed = harness.claim_job_after_restart(job_id).await;
            assert_eq!(claimed.stage, "copy_verified");
            let error = harness
                .scheduler
                .process_one(&harness.config, claimed.clone())
                .await
                .unwrap_err();
            harness.scheduler.record_retry(claimed, error).await;

            let stored = harness.stored_job(job_id).await;
            assert!(!stored.copy_lock_acquired);
            assert_eq!(stored.copy_checkpoint_json, None);
            assert_eq!(stored.openlist_task_id, None);
            assert_eq!(harness.post_counts(), (0, 0));
            assert_eq!(harness.downloader.export_calls(), attempt as usize);
            if attempt < MAX_STAGE_ERROR_ATTEMPTS {
                assert_eq!(stored.stage, "copy_verified");
            } else {
                assert_eq!(stored.stage, "qb_manual_review");
            }
        }

        let review = harness.stored_job(job_id).await;
        assert!(
            harness
                .db
                .retry_media_relocation_migration(job_id, review.version)
                .await
                .unwrap()
        );
        let retried = harness.stored_job(job_id).await;
        assert_eq!(retried.stage, "copy_verified");
        assert!(!retried.copy_lock_acquired);
        assert_eq!(retried.copy_checkpoint_json, None);
        assert_eq!(retried.openlist_task_id, None);
        assert_eq!(harness.post_counts(), (0, 0));
    }

    #[test]
    fn prefix_is_segment_aware() {
        assert!(is_path_prefix("/pt", "/pt/a"));
        assert!(is_path_prefix("/pt", "/pt"));
        assert!(!is_path_prefix("/pt", "/pt2/a"));
    }

    #[test]
    fn translation_preserves_relative_suffix() {
        assert_eq!(
            translate_path("/pt/show/file.mkv", "/pt", "/local/pt").unwrap(),
            "/local/pt/show/file.mkv"
        );
        assert!(translate_path("/pt2/file", "/pt", "/local/pt").is_err());
    }

    #[test]
    fn overlapping_ancestor_paths_are_rejected() {
        assert!(validate_non_overlapping(&["/pt".into(), "/pt/a".into()]).is_err());
        assert!(validate_non_overlapping(&["/pt".into(), "/pt2".into()]).is_ok());
    }

    #[test]
    fn relocation_rejects_identical_source_and_target_paths() {
        assert!(validate_distinct_path_pairs("/src", "/src", "/qb-a", "/qb-b").is_err());
        assert!(validate_distinct_path_pairs("/src", "/dst", "/qb", "/qb").is_err());
        assert!(validate_distinct_path_pairs("/src", "/src/archive", "/qb-a", "/qb-b").is_err());
        assert!(validate_distinct_path_pairs("/src", "/dst", "/qb", "/qb/archive").is_err());
        assert!(validate_distinct_path_pairs("/src", "/dst", "/qb-a", "/qb-b").is_ok());
        assert!(
            validate_distinct_path_pairs("/Ärchive", "/ärchive/show", "/qb-a", "/qb-b").is_err()
        );
        assert!(
            validate_distinct_path_pairs("/Café", "/Cafe\u{301}/show", "/qb-a", "/qb-b").is_err()
        );
        assert!(
            validate_distinct_path_pairs_for_mode("/src", "/dst", "/qb", "/qb/archive", false,)
                .is_ok()
        );
        assert!(
            validate_distinct_path_pairs_for_mode("/src", "/dst", "/qb", "/qb/archive", true,)
                .is_err()
        );
    }

    #[test]
    fn openlist_task_ids_round_trip_and_accept_legacy_value() {
        let encoded =
            encode_openlist_task_ids(["task-a".to_string(), "task-b".to_string()]).unwrap();
        assert_eq!(
            decode_openlist_task_ids(&encoded),
            vec!["task-a".to_string(), "task-b".to_string()]
        );
        assert_eq!(
            decode_openlist_task_ids("legacy-task"),
            vec!["legacy-task".to_string()]
        );
    }

    #[test]
    fn category_has_year_layer_when_known() {
        assert_eq!(category_year_directory("movie", Some(2026)), "电影/2026");
        assert_eq!(category_year_directory("tv", None), "电视剧");
        assert_eq!(category_year_directory("电视剧", Some(2026)), "电视剧/2026");
    }

    #[test]
    fn absolute_episode_downloads_use_the_anime_archive_category() {
        let category = media_download_category("tv:33:abs0123", false);
        assert_eq!(
            archive_relative_directory(category, "动画", Some(2025)),
            "云母/动漫/动画/2025"
        );
    }

    #[test]
    fn tmdb_archive_uses_primary_genre_and_tmdb_year() {
        assert_eq!(
            archive_relative_directory("电视剧", "动画", Some(2025)),
            "云母/电视剧/动画/2025"
        );
        assert_eq!(
            archive_relative_directory("电影", "剧情", None),
            "云母/电影/剧情/年份未知"
        );
    }

    #[test]
    fn scheduler_honors_copy_settle_deadline() {
        let before = Utc::now();
        let deadline = before + ChronoDuration::seconds(COPY_SETTLE_DELAY_SECONDS);
        assert_eq!(
            relocation_scheduler_delay(
                600,
                Some(&deadline.to_rfc3339()),
                deadline - ChronoDuration::seconds(COPY_SETTLE_DELAY_SECONDS),
            ),
            COPY_SETTLE_DELAY_SECONDS as u64
        );
        assert_eq!(relocation_scheduler_delay(600, None, before), 600);
    }

    #[test]
    fn target_qb_false_success_becomes_a_recordable_failure() {
        let infohash = "eadb91a4769b1fad89e0dd3a930523e7fc5814b8";
        let false_success = resolve_target_qb_submission(infohash, None, Ok(false)).unwrap_err();
        assert!(false_success.contains("添加接口返回成功"));
        assert!(false_success.contains(infohash));

        let explicit_failure = resolve_target_qb_submission(
            infohash,
            Some("connection refused".to_string()),
            Ok(false),
        )
        .unwrap_err();
        assert!(explicit_failure.contains("connection refused"));

        assert!(
            resolve_target_qb_submission(
                infohash,
                Some("duplicate response".to_string()),
                Ok(true),
            )
            .is_ok()
        );
    }

    #[test]
    fn historical_jobs_require_a_valid_v1_or_v2_infohash() {
        assert!(valid_infohash("eadb91a4769b1fad89e0dd3a930523e7fc5814b8"));
        assert!(valid_infohash(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        ));
        assert!(!valid_infohash(""));
        assert!(!valid_infohash(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcde"
        ));
        assert!(!valid_infohash("not-a-valid-infohash"));
    }

    #[test]
    fn directory_checkpoint_is_durable_and_distinct_from_file_copy() {
        let checkpoint = CopyCheckpoint {
            path: "/dst/show".to_string(),
            size: 0,
            operation: CopyCheckpointOperation::CreateDirectory,
            phase: CopyCheckpointPhase::Uncertain,
            submitted_at: Some(Utc::now().to_rfc3339()),
            terminal_failure_verified: false,
        };
        assert_eq!(
            decode_copy_checkpoint(&encode_copy_checkpoint(&checkpoint).unwrap()).unwrap(),
            checkpoint
        );
    }

    #[tokio::test]
    async fn directory_checkpoint_parent_chain_uses_unicode_case_identity() {
        let harness = RelocationTestHarness::new(BTreeMap::new()).await;
        let checkpoint = copy_checkpoint(
            "/archive/Cafe\u{301}/season",
            0,
            CopyCheckpointOperation::CreateDirectory,
            CopyCheckpointPhase::Uncertain,
        );
        let mut job = harness
            .seed_job(
                "copy_submitting",
                Some(checkpoint.clone()),
                None,
                "Season/E01.mkv",
                0,
            )
            .await;
        job.target_openlist_path = "/Archive/Café".to_string();

        let (index, file) = validate_copy_checkpoint(&checkpoint, &job).unwrap();

        assert_eq!(index, 0);
        assert_eq!(file.path, "Season/E01.mkv");
    }

    #[tokio::test]
    async fn directory_checkpoint_accepts_missing_ancestor_of_derived_target_root() {
        let harness = RelocationTestHarness::new(BTreeMap::new()).await;
        let checkpoint = copy_checkpoint(
            "/cmcc/Download/media/云母/动漫",
            0,
            CopyCheckpointOperation::CreateDirectory,
            CopyCheckpointPhase::Prepared,
        );
        let mut job = harness
            .seed_job(
                "copy_submitting",
                Some(checkpoint.clone()),
                None,
                "Show/E01.mkv",
                0,
            )
            .await;
        job.target_openlist_path = "/cmcc/Download/media/云母/动漫/动画/2024".to_string();

        let (index, file) = validate_copy_checkpoint(&checkpoint, &job).unwrap();

        assert_eq!(index, 0);
        assert_eq!(file.path, "Show/E01.mkv");
    }

    #[tokio::test]
    async fn directory_checkpoint_rejects_paths_outside_current_target_parent_chain() {
        let harness = RelocationTestHarness::new(BTreeMap::new()).await;
        let initial_checkpoint = copy_checkpoint(
            "/cmcc/Download/media/云母/动漫",
            0,
            CopyCheckpointOperation::CreateDirectory,
            CopyCheckpointPhase::Prepared,
        );
        let mut job = harness
            .seed_job(
                "copy_submitting",
                Some(initial_checkpoint),
                None,
                "Show/E01.mkv",
                0,
            )
            .await;
        job.target_openlist_path = "/cmcc/Download/media/云母/动漫/动画/2024".to_string();

        for path in [
            "/cmcc/Download/media/云母/电影",
            "/cmcc/Download/media/云母/动漫2",
            "/cmcc/Download/media/云母/动漫/动画/2024/Other",
            "/cmcc/Download/media/云母/动漫/动画/2024/Show/E01.mkv",
        ] {
            let checkpoint = copy_checkpoint(
                path,
                0,
                CopyCheckpointOperation::CreateDirectory,
                CopyCheckpointPhase::Prepared,
            );

            let error = validate_copy_checkpoint(&checkpoint, &job).unwrap_err();

            assert!(
                error.contains("不在当前目标文件的父目录链中"),
                "unexpected validation result for {path}: {error}"
            );
        }
    }

    #[test]
    fn qb_transfer_completion_rejects_stale_or_unstable_states() {
        assert!(!torrent_is_complete(1, 99, 100, 0.99, "downloading"));
        assert!(torrent_is_complete(1, 99, 100, 1.0, "downloading"));
        assert!(torrent_is_complete(0, 100, 100, 1.0, "stalledUP"));
        assert!(!torrent_is_complete(0, 99, 100, 0.99, "stalledDL"));
        for state in [
            "",
            "unknown",
            "futureStateAddedByQbittorrent",
            "queuedForChecking",
            "missingFiles",
            "checkingUP",
            "checkingResumeData",
            "allocating",
            "metaDL",
        ] {
            assert!(
                !torrent_is_complete(1, 100, 100, 1.0, state),
                "{state:?} must not bypass the stable source-state whitelist"
            );
        }
    }

    #[test]
    fn paused_automatic_copy_resumes_from_the_last_durable_planning_boundary() {
        let manifest = r#"[{"path":"Show/E01.mkv","size":10}]"#;
        assert!(!stage_requires_distinct_relocation_paths(
            "auto_copy_paused"
        ));
        assert_eq!(auto_copy_resume_stage("", "").unwrap(), "waiting_download");
        assert_eq!(
            auto_copy_resume_stage("/source", "[]").unwrap(),
            "waiting_download"
        );
        assert_eq!(
            auto_copy_resume_stage("/source", manifest).unwrap(),
            "copy_reconcile"
        );
        assert!(auto_copy_resume_stage("/source", "not-json").is_err());
    }

    #[test]
    fn source_is_only_removed_after_target_is_seeding() {
        assert!(target_torrent_is_seeding("uploading"));
        assert!(target_torrent_is_seeding("stalledUP"));
        assert!(target_torrent_is_seeding("forcedUP"));
        assert!(target_torrent_is_seeding("queuedUP"));
        assert!(!target_torrent_is_seeding("checkingUP"));
        assert!(!target_torrent_is_seeding("pausedUP"));
        assert!(!target_torrent_is_seeding("missingFiles"));
        assert!(!target_torrent_is_seeding("error"));
    }

    #[test]
    fn automatic_jobs_never_continue_into_qb_followup_stages() {
        for stage in [
            "manifest_recheck",
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
        ] {
            assert!(automatic_qb_followup_stage(stage), "{stage}");
        }
        for stage in [
            "waiting_download",
            "auto_copy_paused",
            "copy_reconcile",
            "copy_submitting",
            "copying",
            "copy_manual_review",
            "completed",
            "cancelled",
        ] {
            assert!(!automatic_qb_followup_stage(stage), "{stage}");
        }
    }

    #[test]
    fn target_file_completeness_is_required_only_after_recheck_is_observed() {
        assert!(!target_manifest_requires_complete(
            "target_qb_submitted",
            "pausedUP"
        ));
        assert!(!target_manifest_requires_complete(
            "target_qb_check_requested",
            "pausedUP"
        ));
        assert!(!target_manifest_requires_complete(
            "target_qb_checking",
            "checkingUP"
        ));
        assert!(target_manifest_requires_complete(
            "target_qb_checking",
            "pausedUP"
        ));
        assert!(target_manifest_requires_complete(
            "target_qb_starting",
            "stalledUP"
        ));
    }

    #[test]
    fn qb_reconcile_classifies_same_and_distinct_downloader_locations() {
        let target = torrent_with_state("pausedUP", 1.0);
        assert_eq!(
            classify_qb_reconcile(
                true,
                "/source-qb",
                "/target-qb",
                "/target-qb/episode.mkv",
                Some(&target),
            ),
            QbReconcileDecision::TargetQbSubmitted
        );
        assert_eq!(
            classify_qb_reconcile(
                false,
                "/source-qb",
                "/target-qb",
                "/target-qb/episode.mkv",
                Some(&target),
            ),
            QbReconcileDecision::TargetQbSubmitted
        );

        let mut source = target.clone();
        source.save_path = "/source-qb".to_string();
        source.content_path = "/source-qb/episode.mkv".to_string();
        assert_eq!(
            classify_qb_reconcile(
                true,
                "/source-qb",
                "/target-qb",
                "/target-qb/episode.mkv",
                Some(&source),
            ),
            QbReconcileDecision::TorrentExported
        );
        assert_eq!(
            classify_qb_reconcile(
                true,
                "/source-qb",
                "/target-qb",
                "/target-qb/episode.mkv",
                None,
            ),
            QbReconcileDecision::SourceQbRemoved
        );
        assert_eq!(
            classify_qb_reconcile(
                false,
                "/source-qb",
                "/target-qb",
                "/target-qb/episode.mkv",
                None,
            ),
            QbReconcileDecision::SourceQbRemoved
        );

        let mut unrelated = target;
        unrelated.save_path = "/somewhere-else".to_string();
        unrelated.content_path = "/somewhere-else/episode.mkv".to_string();
        assert!(matches!(
            classify_qb_reconcile(
                true,
                "/source-qb",
                "/target-qb",
                "/target-qb/episode.mkv",
                Some(&unrelated),
            ),
            QbReconcileDecision::ManualReview(_)
        ));
        assert!(matches!(
            classify_qb_reconcile(
                false,
                "/source-qb",
                "/target-qb",
                "/target-qb/episode.mkv",
                Some(&unrelated),
            ),
            QbReconcileDecision::ManualReview(_)
        ));
    }

    #[test]
    fn an_already_running_hash_check_must_not_be_requested_again() {
        assert!(target_torrent_is_hash_checking("checkingUP"));
        assert!(target_torrent_is_hash_checking("checkingDL"));
        assert!(!target_torrent_is_hash_checking("queuedForChecking"));
        assert!(!target_torrent_is_hash_checking("pausedUP"));
    }

    #[tokio::test]
    async fn target_qb_hash_check_observation_requires_explicit_hash_check_state() {
        for state in ["checkingUP", "checkingDL"] {
            let downloader = NoopDownloader::with_torrents(vec![torrent_with_state(state, 1.0)]);
            assert!(
                observe_target_qb_check_started_with_policy(
                    &downloader,
                    TEST_INFOHASH,
                    1,
                    Duration::ZERO,
                )
                .await
                .unwrap(),
                "{state} must count as hash-check evidence"
            );
        }

        for state in ["queuedUP", "queuedDL", "checkingResumeData", "pausedUP"] {
            let downloader = NoopDownloader::with_torrents(vec![torrent_with_state(state, 1.0)]);
            assert!(
                !observe_target_qb_check_started_with_policy(
                    &downloader,
                    TEST_INFOHASH,
                    1,
                    Duration::ZERO,
                )
                .await
                .unwrap(),
                "{state} must not prove that the requested hash check started"
            );
        }
    }

    #[tokio::test]
    async fn target_qb_hash_check_observation_tolerates_a_transient_api_error() {
        let downloader = NoopDownloader::with_torrent_results([
            Err("temporary qB API failure".to_string()),
            Ok(vec![torrent_with_state("checkingUP", 0.25)]),
        ]);
        assert!(
            observe_target_qb_check_started_with_policy(
                &downloader,
                TEST_INFOHASH,
                2,
                Duration::ZERO,
            )
            .await
            .unwrap()
        );
    }

    #[test]
    fn target_qb_complete_rejects_errors_and_unstable_check_states() {
        for state in [
            "error",
            "missingFiles",
            "unknown",
            "checkingUP",
            "checkingResumeData",
            "moving",
            "futureStateAddedByQbittorrent",
        ] {
            assert!(
                !target_torrent_verified_complete(&torrent_with_state(state, 1.0)),
                "{state} must not be accepted as verified complete"
            );
        }
        assert!(target_torrent_verified_complete(&torrent_with_state(
            "pausedUP", 1.0
        )));
        assert!(target_torrent_verified_complete(&torrent_with_state(
            "stoppedUP",
            1.0
        )));
    }

    #[test]
    fn qb_content_path_is_the_torrent_copy_root() {
        assert_eq!(select_torrent_content_root("/pt/Show", "/pt"), "/pt/Show");
        assert_eq!(select_torrent_content_root("", "/pt"), "/pt");
    }

    #[test]
    fn torrent_manifest_produces_unique_top_level_copy_items() {
        let files = normalize_torrent_file_paths(vec![
            TorrentFileInfo {
                path: "Show/Season 01/E01.mkv".to_string(),
                size: 10,
                progress: 1.0,
                is_seed: true,
            },
            TorrentFileInfo {
                path: "Show/Season 01/E02.mkv".to_string(),
                size: 20,
                progress: 1.0,
                is_seed: true,
            },
            TorrentFileInfo {
                path: "poster.jpg".to_string(),
                size: 1,
                progress: 1.0,
                is_seed: true,
            },
        ])
        .unwrap();
        assert_eq!(
            torrent_top_level_items(&files).unwrap(),
            vec!["Show".to_string(), "poster.jpg".to_string()]
        );
        assert!(normalize_manifest_path("../outside.mkv").is_err());
    }

    #[test]
    fn relative_content_path_is_preserved_under_the_copied_root() {
        assert_eq!(
            relative_path("/pt/Show/E01.mkv", "/pt/Show").unwrap(),
            "E01.mkv"
        );
        assert_eq!(relative_path("/pt/Show", "/pt/Show").unwrap(), "");
    }

    #[test]
    fn source_manifest_snapshot_preserves_path_and_size() {
        let manifest = normalize_torrent_manifest(vec![
            TorrentFileInfo {
                path: "Show/E02.mkv".to_string(),
                size: 20,
                progress: 1.0,
                is_seed: true,
            },
            TorrentFileInfo {
                path: "Show/E01.mkv".to_string(),
                size: 10,
                progress: 1.0,
                is_seed: true,
            },
        ])
        .unwrap();
        assert_eq!(
            manifest,
            vec![
                ManifestFile {
                    path: "Show/E01.mkv".to_string(),
                    size: 10,
                },
                ManifestFile {
                    path: "Show/E02.mkv".to_string(),
                    size: 20,
                },
            ]
        );
        let json = serde_json::to_string(&manifest).unwrap();
        assert_eq!(decode_source_manifest(&json).unwrap(), Some(manifest));
        assert_eq!(decode_source_manifest("[]").unwrap(), None);
    }

    #[test]
    fn torrent_manifest_rejects_partial_or_non_seed_files() {
        let partial = TorrentFileInfo {
            path: "Show/E01.mkv".to_string(),
            size: 10,
            progress: 0.75,
            is_seed: false,
        };
        let error = normalize_torrent_manifest(vec![partial.clone()]).unwrap_err();
        assert!(error.contains("未完整下载或被跳过"));
        assert!(validate_torrent_files_complete(&[partial.clone()]).is_err());

        let non_seed = TorrentFileInfo {
            progress: 1.0,
            ..partial.clone()
        };
        assert!(normalize_torrent_manifest(vec![non_seed]).is_err());

        assert_eq!(
            normalize_torrent_file_paths(vec![partial]).unwrap(),
            vec!["Show/E01.mkv".to_string()]
        );
    }

    #[test]
    fn torrent_manifest_rejects_duplicate_and_case_conflicting_paths() {
        let file = TorrentFileInfo {
            path: "Show/E01.mkv".to_string(),
            size: 10,
            progress: 1.0,
            is_seed: true,
        };
        let duplicate_error =
            normalize_torrent_manifest(vec![file.clone(), file.clone()]).unwrap_err();
        assert!(duplicate_error.contains("重复路径"));

        let case_conflict = TorrentFileInfo {
            path: "show/e01.mkv".to_string(),
            ..file.clone()
        };
        let case_error = normalize_torrent_manifest(vec![file, case_conflict]).unwrap_err();
        assert!(case_error.contains("大小写"));

        let unicode_file = TorrentFileInfo {
            path: "Ärchive/E01.mkv".to_string(),
            size: 10,
            progress: 1.0,
            is_seed: true,
        };
        let unicode_case_conflict = TorrentFileInfo {
            path: "ärchive/E01.mkv".to_string(),
            ..unicode_file.clone()
        };
        let unicode_error =
            normalize_torrent_manifest(vec![unicode_file, unicode_case_conflict]).unwrap_err();
        assert!(unicode_error.contains("大小写"));

        let composed = TorrentFileInfo {
            path: "Café/E01.mkv".to_string(),
            size: 10,
            progress: 1.0,
            is_seed: true,
        };
        let decomposed = TorrentFileInfo {
            path: "Cafe\u{301}/E01.mkv".to_string(),
            ..composed.clone()
        };
        let normalization_error =
            normalize_torrent_manifest(vec![composed, decomposed]).unwrap_err();
        assert!(normalization_error.contains("Unicode"));
    }

    #[tokio::test]
    async fn shared_torrent_paths_use_unicode_case_identity_before_source_removal() {
        let harness = RelocationTestHarness::new(BTreeMap::new()).await;
        let job = harness
            .seed_job("source_removing", None, None, "Show/Café.mkv", 0)
            .await;
        let mut other = torrent_with_state("uploading", 1.0);
        other.hash = "0123456789abcdef0123456789abcdef01234567".to_string();
        other.save_path = "/SOURCE-QB".to_string();
        other.content_path = "/SOURCE-QB/show/Cafe\u{301}.mkv".to_string();
        harness.downloader.queue_torrents(vec![other], 1);
        harness.downloader.queue_torrent_files(
            vec![TorrentFileInfo {
                path: "show/Cafe\u{301}.mkv".to_string(),
                size: 10,
                progress: 1.0,
                is_seed: true,
            }],
            1,
        );

        let referenced =
            source_paths_referenced_by_other_torrents(harness.downloader.as_ref(), &job)
                .await
                .unwrap();

        assert_eq!(
            referenced,
            std::collections::BTreeSet::from([openlist_identity_key("/src/Show/Café.mkv")])
        );
    }

    #[test]
    fn ambiguous_submission_never_becomes_prepared_automatically() {
        let checkpoint = CopyCheckpoint {
            path: "Show/E01.mkv".to_string(),
            size: 10,
            operation: CopyCheckpointOperation::CopyFile,
            phase: CopyCheckpointPhase::Uncertain,
            submitted_at: Some((Utc::now() - ChronoDuration::hours(1)).to_rfc3339()),
            terminal_failure_verified: false,
        };
        assert_eq!(
            uncertain_submission_next_check(&checkpoint, Utc::now()).unwrap(),
            None
        );
        assert_eq!(
            decode_copy_checkpoint(&encode_copy_checkpoint(&checkpoint).unwrap())
                .unwrap()
                .phase,
            CopyCheckpointPhase::Uncertain
        );
    }

    #[test]
    fn review_existing_checkpoint_is_locatable_and_never_submitting() {
        let file = ManifestFile {
            path: "Show/E01.mkv".to_string(),
            size: 10,
        };
        let checkpoint =
            decode_copy_checkpoint(&encode_review_existing_checkpoint(&file).unwrap()).unwrap();
        assert_eq!(checkpoint.path, file.path);
        assert_eq!(checkpoint.size, file.size);
        assert_eq!(
            checkpoint.operation,
            CopyCheckpointOperation::ReviewExisting
        );
        assert_eq!(checkpoint.phase, CopyCheckpointPhase::Prepared);
        assert_eq!(checkpoint.submitted_at, None);

        for observed in [
            ManifestFileState::Missing,
            ManifestFileState::MissingDirectory {
                path: "/archive/Show".to_string(),
            },
        ] {
            assert!(!review_existing_observation_is_verified(&observed, false));
        }
    }

    #[test]
    fn automatic_copy_never_accepts_size_only_presence_after_uncertain_submission() {
        let checkpoint = CopyCheckpoint {
            path: "Show/E01.mkv".to_string(),
            size: 10,
            operation: CopyCheckpointOperation::CopyFile,
            phase: CopyCheckpointPhase::Uncertain,
            submitted_at: Some(Utc::now().to_rfc3339()),
            terminal_failure_verified: false,
        };
        assert_eq!(checkpoint.phase, CopyCheckpointPhase::Uncertain);
        assert!(!manifest_presence_is_verified(false, false));
        assert!(manifest_presence_is_verified(false, true));
        assert!(manifest_presence_is_verified(true, false));
    }

    #[test]
    fn copy_checkpoint_must_match_authoritative_manifest() {
        let checkpoint = CopyCheckpoint {
            path: "Show/E01.mkv".to_string(),
            size: 11,
            operation: CopyCheckpointOperation::CopyFile,
            phase: CopyCheckpointPhase::Prepared,
            submitted_at: None,
            terminal_failure_verified: false,
        };
        assert!(
            validate_checkpoint_against_manifest(
                &checkpoint,
                "[{\"path\":\"Show/E01.mkv\",\"size\":10}]"
            )
            .is_err()
        );
    }

    #[test]
    fn mixed_copy_tasks_preserve_in_flight_or_uncertain_work() {
        assert_eq!(
            decide_copy_tasks(&[
                CopyTaskObservation::Failed,
                CopyTaskObservation::Pending,
                CopyTaskObservation::Missing,
            ])
            .unwrap(),
            CopyTaskDecision::Uncertain
        );
        assert_eq!(
            decide_copy_tasks(&[CopyTaskObservation::Failed, CopyTaskObservation::Missing,])
                .unwrap(),
            CopyTaskDecision::Uncertain
        );
        assert_eq!(
            decide_copy_tasks(&[CopyTaskObservation::Failed, CopyTaskObservation::Pending,])
                .unwrap(),
            CopyTaskDecision::Wait
        );
        assert_eq!(
            decide_copy_tasks(&[CopyTaskObservation::Failed, CopyTaskObservation::Failed,])
                .unwrap(),
            CopyTaskDecision::AllFailed
        );
        assert_eq!(
            decide_copy_tasks(&[CopyTaskObservation::Succeeded, CopyTaskObservation::Failed,])
                .unwrap(),
            CopyTaskDecision::VerifyTarget
        );

        let mut new_checkpoint = Some(
            encode_copy_checkpoint(&CopyCheckpoint {
                path: "Show/E01.mkv".to_string(),
                size: 10,
                operation: CopyCheckpointOperation::CopyFile,
                phase: CopyCheckpointPhase::Prepared,
                submitted_at: None,
                terminal_failure_verified: false,
            })
            .unwrap(),
        );
        assert_eq!(
            prepare_read_only_copy_reconcile(&mut new_checkpoint, Utc::now()).unwrap(),
            "copy_submitting"
        );
        assert_eq!(
            decode_copy_checkpoint(new_checkpoint.as_deref().unwrap())
                .unwrap()
                .phase,
            CopyCheckpointPhase::Uncertain
        );
        let mut legacy_checkpoint = None;
        assert_eq!(
            prepare_read_only_copy_reconcile(&mut legacy_checkpoint, Utc::now()).unwrap(),
            "copy_legacy_reconcile"
        );
        let old_pending = encode_copy_checkpoint(&CopyCheckpoint {
            path: "Show/E01.mkv".to_string(),
            size: 10,
            operation: CopyCheckpointOperation::CopyFile,
            phase: CopyCheckpointPhase::Uncertain,
            submitted_at: Some(
                (Utc::now() - ChronoDuration::seconds(COPY_TASK_PENDING_MANUAL_SECONDS + 1))
                    .to_rfc3339(),
            ),
            terminal_failure_verified: false,
        })
        .unwrap();
        assert!(checkpoint_pending_timed_out(Some(&old_pending), Utc::now()).unwrap());
        assert!(checkpoint_pending_timed_out(None, Utc::now()).unwrap());
    }

    #[test]
    fn manifest_cursor_batches_without_skipping_final_verification_boundary() {
        assert_eq!(manifest_scan_end(0, 250).unwrap(), MANIFEST_FILES_PER_PASS);
        assert_eq!(
            manifest_scan_end(MANIFEST_FILES_PER_PASS, 250).unwrap(),
            MANIFEST_FILES_PER_PASS * 2
        );
        assert_eq!(manifest_scan_end(200, 250).unwrap(), 250);
        assert_eq!(manifest_scan_end(250, 250).unwrap(), 250);
        assert!(manifest_scan_end(251, 250).is_err());
    }

    #[test]
    fn old_advanced_jobs_require_sized_manifest_recovery() {
        for stage in [
            "qb_reconcile",
            "torrent_exported",
            "source_qb_removed",
            "target_qb_submitted",
            "target_qb_starting",
        ] {
            assert!(advanced_stage_requires_manifest(stage));
        }
        assert!(migration_stage_uses_target_qb("qb_reconcile"));
        assert!(!advanced_stage_requires_manifest("source_removed"));
        assert!(
            validate_manifest_paths_snapshot(
                &[ManifestFile {
                    path: "Show/E01.mkv".to_string(),
                    size: 10,
                }],
                "[\"Show/E02.mkv\"]",
            )
            .is_err()
        );
        assert_eq!(
            manifest_from_torrent_data(b"d4:infod6:lengthi12e4:name8:file.mkvee").unwrap(),
            vec![ManifestFile {
                path: "file.mkv".to_string(),
                size: 12,
            }]
        );
    }

    #[test]
    fn checkpoint_returns_its_authoritative_manifest_cursor() {
        let checkpoint = CopyCheckpoint {
            path: "Show/E02.mkv".to_string(),
            size: 20,
            operation: CopyCheckpointOperation::CopyFile,
            phase: CopyCheckpointPhase::Prepared,
            submitted_at: None,
            terminal_failure_verified: false,
        };
        assert_eq!(
            validate_checkpoint_against_manifest(
                &checkpoint,
                "[{\"path\":\"Show/E01.mkv\",\"size\":10},{\"path\":\"Show/E02.mkv\",\"size\":20}]",
            )
            .unwrap(),
            1
        );
    }
}
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use chrono::{Duration as ChronoDuration, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;
use tracing::{error, info, warn};

use crate::db::{Database, MediaRelocationJob, OpenListConfig};
use crate::downloader::{
    AddTorrentOptions, DownloaderClient, DownloaderClientPool, TorrentFileInfo, TorrentInfo,
};
use crate::media::models::media_download_category;
use crate::media::torrent::{torrent_file_manifest, torrent_infohash_for};
use crate::openlist::{
    ManifestFileState, ManifestInspectError, OpenListClient, openlist_canonical_key,
    openlist_identity_key,
};

const COPY_SETTLE_DELAY_SECONDS: i64 = 30;
const COPY_SUBMISSION_ATTENTION_SECONDS: i64 = 300;
const SOURCE_REMOVAL_ATTENTION_SECONDS: i64 = 300;
const COPY_TASK_PENDING_MANUAL_SECONDS: i64 = 24 * 60 * 60;
const MANIFEST_FILES_PER_PASS: usize = 100;
const RELOCATION_LEASE_SECONDS: i64 = 120;
const RELOCATION_LEASE_HEARTBEAT_SECONDS: u64 = 30;
const TARGET_QB_CONFIRM_ATTEMPTS: usize = 5;
const TARGET_QB_CONFIRM_INTERVAL: Duration = Duration::from_millis(300);
const TARGET_QB_CHECK_OBSERVE_ATTEMPTS: usize = 20;
const TARGET_QB_CHECK_OBSERVE_INTERVAL: Duration = Duration::from_millis(100);
const SOURCE_QB_REMOVAL_SETTLE_SECONDS: i64 = 60;
const TARGET_QB_CHECK_START_GRACE_SECONDS: i64 = 60;
const TARGET_QB_CHECK_TIMEOUT_SECONDS: i64 = 6 * 60 * 60;
const TARGET_QB_START_TIMEOUT_SECONDS: i64 = 10 * 60;
const WAITING_DOWNLOAD_TIMEOUT_SECONDS: i64 = 7 * 24 * 60 * 60;
const MAX_STAGE_ERROR_ATTEMPTS: u32 = 8;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ManifestFile {
    path: String,
    size: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum CopyCheckpointPhase {
    Prepared,
    Uncertain,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum CopyCheckpointOperation {
    CopyFile,
    CreateDirectory,
    ReviewExisting,
    RemoveFile,
}

impl Default for CopyCheckpointOperation {
    fn default() -> Self {
        Self::CopyFile
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct CopyCheckpoint {
    path: String,
    size: i64,
    #[serde(default)]
    operation: CopyCheckpointOperation,
    phase: CopyCheckpointPhase,
    #[serde(default)]
    submitted_at: Option<String>,
    #[serde(default)]
    terminal_failure_verified: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CopyTaskObservation {
    Pending,
    Succeeded,
    Failed,
    Missing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CopyTaskDecision {
    Wait,
    AllFailed,
    VerifyTarget,
    Uncertain,
}

pub struct RelocationScheduler {
    db: Database,
    pool: Arc<DownloaderClientPool>,
    enabled_by_environment: bool,
    running: AtomicBool,
    scan_requested: Notify,
    owner: String,
}

impl RelocationScheduler {
    pub fn new(
        db: Database,
        pool: Arc<DownloaderClientPool>,
        enabled_by_environment: bool,
    ) -> Arc<Self> {
        Arc::new(Self {
            db,
            pool,
            enabled_by_environment,
            running: AtomicBool::new(true),
            scan_requested: Notify::new(),
            owner: format!(
                "relocation-{}-{}",
                std::process::id(),
                Utc::now().timestamp_nanos_opt().unwrap_or_default()
            ),
        })
    }

    pub async fn start(&self) {
        if !self.enabled_by_environment {
            info!("media relocation scheduler disabled");
            return;
        }
        while self.running.load(Ordering::Relaxed) {
            let delay = match self.tick().await {
                Ok(delay) => delay,
                Err(error) => {
                    error!("media relocation scheduler tick failed: {error}");
                    60
                }
            };
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(delay.max(5))) => {}
                _ = self.scan_requested.notified() => {}
            }
        }
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::Relaxed);
        self.scan_requested.notify_waiters();
    }

    pub fn request_scan(&self) {
        if self.enabled_by_environment {
            self.scan_requested.notify_one();
        }
    }

    async fn tick(&self) -> Result<u64, String> {
        let config = self
            .db
            .get_openlist_config()
            .await
            .map_err(|e| e.to_string())?;
        let inserted = self
            .db
            .enqueue_submitted_media_relocation_jobs(config.enabled)
            .await
            .map_err(|e| e.to_string())?;
        if inserted > 0 {
            info!("enqueued {inserted} media relocation job(s)");
        }
        if config.base_url.trim().is_empty() || config.api_key.trim().is_empty() {
            return Ok(config.scan_interval_secs);
        }
        let include_automatic = config.enabled;
        // A job has several idempotent local transitions after an OpenList copy finishes.
        // Drain those transitions now instead of making each one wait a full scan interval.
        for _ in 0..32 {
            let jobs = self
                .db
                .claim_due_media_relocation_jobs(
                    &self.owner,
                    RELOCATION_LEASE_SECONDS,
                    1,
                    include_automatic,
                )
                .await
                .map_err(|e| e.to_string())?;
            if jobs.is_empty() {
                break;
            }
            for job in jobs {
                if let Err(error) = self.process_one_with_lease(&config, job.clone()).await {
                    warn!(
                        "media relocation job {} failed at {}: {}",
                        job.id, job.stage, error
                    );
                    self.record_retry(job, error).await;
                }
            }
        }
        let next_attempt_at = self
            .db
            .next_media_relocation_attempt_at(include_automatic)
            .await
            .map_err(|e| e.to_string())?;
        Ok(relocation_scheduler_delay(
            config.scan_interval_secs,
            next_attempt_at.as_deref(),
            Utc::now(),
        ))
    }

    async fn process_one_with_lease(
        &self,
        config: &OpenListConfig,
        job: MediaRelocationJob,
    ) -> Result<(), String> {
        let lease_owner = job.lease_owner.clone().ok_or("迁移任务 lease owner 缺失")?;
        let process = self.process_one(config, job.clone());
        tokio::pin!(process);
        loop {
            tokio::select! {
                result = &mut process => return result,
                _ = tokio::time::sleep(Duration::from_secs(RELOCATION_LEASE_HEARTBEAT_SECONDS)) => {
                    let renewed = self.db
                        .renew_media_relocation_lease(
                            job.id,
                            job.version,
                            &job.stage,
                            &lease_owner,
                            RELOCATION_LEASE_SECONDS,
                        )
                        .await
                        .map_err(|error| error.to_string())?;
                    if !renewed {
                        return Err("迁移任务 lease 已变化，已停止当前处理".to_string());
                    }
                }
            }
        }
    }

    async fn process_one(
        &self,
        config: &OpenListConfig,
        mut job: MediaRelocationJob,
    ) -> Result<(), String> {
        let expected_version = job.version;
        let expected_stage = job.stage.clone();
        let manual_migration = is_manual_migration_job(&job);
        let config_guard =
            (expected_stage == "waiting_download").then_some(config.updated_at.as_str());
        if job.media_download_id.is_some() && automatic_qb_followup_stage(&expected_stage) {
            job.stage = "completed".to_string();
            job.openlist_task_id = None;
            job.copy_checkpoint_json = None;
            job.copy_lock_acquired = false;
            job.next_attempt_at = None;
            job.manual_requested_at = None;
            job.last_error = Some(
                "历史自动复制任务的 qB 后续已停止；自动追剧不会继续接管 qB 或清理源文件。旧流程可能已移除源 qB 任务，若存在 torrent_data 已保留供人工恢复"
                    .to_string(),
            );
            job.completed_at = Some(Utc::now().to_rfc3339());
            let updated = self
                .db
                .update_media_relocation_job(&job, expected_version, &expected_stage, config_guard)
                .await
                .map_err(|error| error.to_string())?;
            return updated
                .then_some(())
                .ok_or_else(|| "迁移任务状态已被其他 worker 修改".to_string());
        }
        let downloader_id = job.downloader_id.ok_or("迁移任务缺少下载器")?;
        let downloader = self
            .db
            .get_downloader(downloader_id)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("下载器 {downloader_id} 不存在"))?;
        let source_qb = self.pool.get(&downloader).await?;
        if stage_requires_distinct_relocation_paths(&expected_stage) {
            validate_distinct_relocation_paths(&job, manual_migration)?;
        }
        if advanced_stage_requires_manifest(&expected_stage)
            && decode_source_manifest(&job.source_manifest_json)?.is_none()
        {
            let torrents = source_qb
                .list_torrents_by_hashes(&[job.infohash.clone()])
                .await?;
            let recovered = if torrents.is_empty() {
                job.torrent_data
                    .as_deref()
                    .ok_or_else(|| "qB 中已无种子且导出的 torrent_data 缺失".to_string())
                    .and_then(manifest_from_torrent_data)
            } else {
                normalize_torrent_manifest(source_qb.get_torrent_files(&job.infohash).await?)
            };
            match recovered.and_then(|manifest| {
                validate_manifest_paths_snapshot(&manifest, &job.source_files_json)?;
                Ok(manifest)
            }) {
                Ok(manifest) => {
                    job.source_files_json = encode_manifest_paths(&manifest)?;
                    job.source_manifest_json = serde_json::to_string(&manifest)
                        .map_err(|error| format!("序列化恢复的种子文件大小快照失败: {error}"))?;
                    job.manifest_cursor = manifest.len();
                    job.next_attempt_at = Some(Utc::now().to_rfc3339());
                    job.last_error = None;
                }
                Err(recovery_error) => {
                    job.stage = "manifest_required".to_string();
                    job.copy_lock_acquired = false;
                    job.next_attempt_at = None;
                    job.last_error = Some(format!(
                        "旧迁移任务在阶段 {expected_stage} 无法恢复带大小的权威 manifest，已停止自动清理: {recovery_error}"
                    ));
                }
            }
            let updated = self
                .db
                .update_media_relocation_job(&job, expected_version, &expected_stage, config_guard)
                .await
                .map_err(|error| error.to_string())?;
            if !updated {
                return Err("迁移任务状态已被其他 worker 修改".to_string());
            }
            return Ok(());
        }
        if !manual_migration && !config.enabled {
            let safe_to_converge = match expected_stage.as_str() {
                "copying" | "copy_legacy_reconcile" | "copy_succeeded" => true,
                "copy_submitting" => job
                    .copy_checkpoint_json
                    .as_deref()
                    .map(decode_copy_checkpoint)
                    .transpose()?
                    .is_some_and(|checkpoint| checkpoint.phase == CopyCheckpointPhase::Uncertain),
                // These stages only exist for jobs started by older versions. They must
                // finish their qB recovery even after automatic copying is disabled.
                "qb_reconcile"
                | "torrent_exported"
                | "source_qb_removed"
                | "target_qb_submitted"
                | "target_qb_check_requested"
                | "target_qb_checking"
                | "target_qb_starting"
                | "source_removing"
                | "source_removed" => true,
                _ => false,
            };
            if !safe_to_converge {
                job.stage = "auto_copy_paused".to_string();
                job.copy_checkpoint_json = None;
                job.openlist_task_id = None;
                job.copy_lock_acquired = false;
                job.next_attempt_at = None;
                job.last_error = None;
                let updated = self
                    .db
                    .update_media_relocation_job(
                        &job,
                        expected_version,
                        &expected_stage,
                        config_guard,
                    )
                    .await
                    .map_err(|error| error.to_string())?;
                return updated
                    .then_some(())
                    .ok_or_else(|| "迁移任务状态已被其他 worker 修改".to_string());
            }
        } else if !manual_migration && expected_stage == "auto_copy_paused" {
            job.stage =
                auto_copy_resume_stage(&job.source_openlist_path, &job.source_manifest_json)?
                    .to_string();
            job.next_attempt_at = Some(Utc::now().to_rfc3339());
            let updated = self
                .db
                .update_media_relocation_job(&job, expected_version, &expected_stage, config_guard)
                .await
                .map_err(|error| error.to_string())?;
            return updated
                .then_some(())
                .ok_or_else(|| "迁移任务状态已被其他 worker 修改".to_string());
        }
        let target_qb = if !migration_stage_uses_target_qb(&expected_stage) {
            None
        } else {
            let target_downloader_id = job
                .target_downloader_id
                .or_else(|| {
                    config.target_directories.iter().find_map(|target| {
                        normalize_path(&target.openlist_path)
                            .is_ok_and(|root| {
                                is_path_prefix(
                                    &root,
                                    &normalize_path(&job.target_openlist_path).unwrap_or_default(),
                                )
                            })
                            .then_some(target.downloader_id)
                    })
                })
                .ok_or("迁移任务的目标下载器快照缺失")?;
            let record = self
                .db
                .get_downloader(target_downloader_id)
                .await
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("目标下载器 {target_downloader_id} 不存在"))?;
            Some(self.pool.get(&record).await?)
        };
        let openlist = OpenListClient::new(&config.base_url, &config.api_key)?;
        let download = match job.media_download_id {
            Some(id) => Some(
                self.db
                    .get_media_download(id)
                    .await
                    .map_err(|e| e.to_string())?
                    .ok_or("追剧下载记录不存在")?,
            ),
            None => None,
        };
        let subscription = match download.as_ref().and_then(|item| item.subscription_id) {
            Some(subscription_id) => self
                .db
                .get_subscription(subscription_id)
                .await
                .map_err(|e| e.to_string())?,
            None => None,
        };
        let tmdb_is_animation = subscription
            .as_ref()
            .is_some_and(|item| item.tmdb_is_animation);
        job.next_attempt_at = None;

        match expected_stage.as_str() {
            "waiting_download" => {
                if !valid_infohash(&job.infohash) {
                    job.stage = "cancelled".to_string();
                    job.last_error = Some("历史记录缺少有效 infohash，已跳过归档".to_string());
                    let updated = self
                        .db
                        .update_media_relocation_job(
                            &job,
                            expected_version,
                            &expected_stage,
                            config_guard,
                        )
                        .await
                        .map_err(|e| e.to_string())?;
                    if !updated {
                        return Err("迁移任务状态已被其他 worker 修改".to_string());
                    }
                    return Ok(());
                }
                let torrents = source_qb
                    .list_torrents_by_hashes(&[job.infohash.clone()])
                    .await?;
                let Some(torrent) = torrents.into_iter().next() else {
                    job.stage = "cancelled".to_string();
                    job.last_error = Some("qB 中已不存在该种子，已跳过历史归档".to_string());
                    let updated = self
                        .db
                        .update_media_relocation_job(
                            &job,
                            expected_version,
                            &expected_stage,
                            config_guard,
                        )
                        .await
                        .map_err(|e| e.to_string())?;
                    if !updated {
                        return Err("迁移任务状态已被其他 worker 修改".to_string());
                    }
                    return Ok(());
                };
                if !torrent_is_complete(
                    torrent.completion_on,
                    torrent.downloaded,
                    torrent.size,
                    torrent.progress,
                    &torrent.state,
                ) {
                    let elapsed = stage_elapsed_seconds(&job, Utc::now())?;
                    if !waiting_torrent_state_can_progress(&torrent.state) {
                        set_waiting_download_manual_review(
                            &mut job,
                            manual_migration,
                            format!(
                                "qB 种子状态 {:?} 无法证明下载仍可继续，已停止无限等待",
                                torrent.state
                            ),
                        );
                    } else if elapsed >= WAITING_DOWNLOAD_TIMEOUT_SECONDS {
                        set_waiting_download_manual_review(
                            &mut job,
                            manual_migration,
                            format!(
                                "等待 qB 下载完成已超过 {} 天（当前状态 {}，进度 {:.2}%），已停止自动轮询",
                                WAITING_DOWNLOAD_TIMEOUT_SECONDS / (24 * 60 * 60),
                                torrent.state,
                                torrent.progress * 100.0
                            ),
                        );
                    } else {
                        job.next_attempt_at =
                            Some((Utc::now() + ChronoDuration::seconds(60)).to_rfc3339());
                    }
                    let updated = self
                        .db
                        .update_media_relocation_job(
                            &job,
                            expected_version,
                            &expected_stage,
                            config_guard,
                        )
                        .await
                        .map_err(|error| error.to_string())?;
                    return updated
                        .then_some(())
                        .ok_or_else(|| "迁移任务状态已被其他 worker 修改".to_string());
                }
                let content_path = torrent_content_root(&torrent);
                let save_qb_path = normalize_path(&torrent.save_path)?;
                let mapping = config
                    .path_mappings
                    .iter()
                    .filter(|mapping| mapping.downloader_id == downloader_id)
                    .filter(|mapping| {
                        normalize_path(&mapping.qb_path)
                            .is_ok_and(|root| is_path_prefix(&root, &save_qb_path))
                    })
                    .max_by_key(|mapping| mapping.qb_path.len())
                    .ok_or_else(|| format!("种子保存路径 {save_qb_path} 没有 OpenList 映射"))?;
                let target_snapshot = if manual_migration
                    && !job.target_openlist_path.trim().is_empty()
                    && !job.target_qb_path.trim().is_empty()
                {
                    Some((
                        job.target_openlist_path.clone(),
                        job.target_qb_path.clone(),
                        job.target_downloader_id
                            .ok_or("手动迁移任务缺少目标下载器快照")?,
                    ))
                } else {
                    None
                };
                let (target_openlist_root, manual_target) = match target_snapshot {
                    Some((openlist_path, qb_path, downloader_id)) => {
                        (openlist_path, Some((qb_path, downloader_id)))
                    }
                    None => {
                        let target = config
                            .target_directories
                            .iter()
                            .find(|target| target.id == config.target_directory_id)
                            .ok_or("未选择 OpenList 目标目录")?;
                        (
                            target.openlist_path.clone(),
                            manual_migration
                                .then(|| (target.qb_path.clone(), target.downloader_id)),
                        )
                    }
                };
                let relative_dir = download.as_ref().map(|download| {
                    let primary_type =
                        media_download_category(&download.target_key, tmdb_is_animation);
                    let tmdb_year = subscription.as_ref().and_then(|item| item.year);
                    let primary_genre = subscription
                        .as_ref()
                        .and_then(|item| item.tmdb_genres.first())
                        .map(|genre| genre.name.as_str())
                        .unwrap_or("其他");
                    archive_relative_directory(primary_type, primary_genre, tmdb_year)
                });
                let content_qb_path = normalize_path(&content_path)?;
                let content_openlist_path =
                    translate_path(&content_qb_path, &mapping.qb_path, &mapping.openlist_path)?;
                let root_qb_path = normalize_optional_path(&torrent.root_path)?;
                let source_manifest =
                    normalize_torrent_manifest(source_qb.get_torrent_files(&job.infohash).await?)?;
                let source_files = source_manifest
                    .iter()
                    .map(|file| file.path.clone())
                    .collect::<Vec<_>>();
                let copy_items = torrent_top_level_items(&source_files)?;
                job.source_qb_path = save_qb_path;
                job.source_openlist_path = translate_path(
                    &job.source_qb_path,
                    &mapping.qb_path,
                    &mapping.openlist_path,
                )?;
                job.source_content_openlist_path = content_openlist_path;
                job.source_files_json = serde_json::to_string(&source_files)
                    .map_err(|e| format!("序列化种子文件清单失败: {e}"))?;
                job.source_manifest_json = serde_json::to_string(&source_manifest)
                    .map_err(|e| format!("序列化种子文件大小快照失败: {e}"))?;
                job.copy_items_json = serde_json::to_string(&copy_items)
                    .map_err(|e| format!("序列化种子顶级项目失败: {e}"))?;
                job.target_openlist_path = match relative_dir.as_deref() {
                    Some(relative_dir) => join_path(&target_openlist_root, relative_dir)?,
                    None => normalize_path(&target_openlist_root)?,
                };
                if let Some((target_qb_root, target_downloader_id)) = manual_target {
                    job.target_qb_path = match relative_dir.as_deref() {
                        Some(relative_dir) => join_path(&target_qb_root, relative_dir)?,
                        None => normalize_path(&target_qb_root)?,
                    };
                    let content_suffix = relative_path(&content_qb_path, &job.source_qb_path)?;
                    job.target_content_qb_path = if content_suffix.is_empty() {
                        job.target_qb_path.clone()
                    } else {
                        join_path(&job.target_qb_path, &content_suffix)?
                    };
                    job.target_downloader_id = Some(target_downloader_id);
                    job.target_root_folder = Some(root_qb_path.is_some());
                } else {
                    // Automatic episode copies end after OpenList verification. Keeping qB target
                    // state empty makes that ownership boundary explicit and prevents target qB
                    // settings from blocking or changing the automatic copy workflow.
                    job.target_qb_path.clear();
                    job.target_content_qb_path.clear();
                    job.target_downloader_id = None;
                    job.target_root_folder = None;
                }
                validate_distinct_relocation_paths(&job, manual_migration)?;
                job.openlist_task_id = None;
                job.copy_checkpoint_json = None;
                job.copy_lock_acquired = false;
                job.manifest_cursor = 0;
                job.stage = "copy_reconcile".to_string();
            }
            "copy_reconcile" | "copy_legacy_reconcile" => {
                if !self
                    .acquire_copy_lock(&mut job, expected_version, &expected_stage)
                    .await?
                {
                    job.next_attempt_at =
                        Some((Utc::now() + ChronoDuration::seconds(30)).to_rfc3339());
                } else if let Some(manifest) = decode_source_manifest(&job.source_manifest_json)? {
                    let end = manifest_scan_end(job.manifest_cursor, manifest.len())?;
                    let mut pending = None;
                    let mut unverified_existing = None;
                    let mut inspection_error = None;
                    for (index, file) in manifest
                        .iter()
                        .enumerate()
                        .take(end)
                        .skip(job.manifest_cursor)
                    {
                        let inspected = openlist
                            .inspect_manifest_file(
                                &job.source_openlist_path,
                                &job.target_openlist_path,
                                &file.path,
                                file.size,
                            )
                            .await;
                        let inspected = match inspected {
                            Ok(inspected) => inspected,
                            Err(ManifestInspectError::Transient(error))
                                if job.attempts.saturating_add(1) < MAX_STAGE_ERROR_ATTEMPTS =>
                            {
                                return Err(error);
                            }
                            Err(ManifestInspectError::Transient(error)) => {
                                inspection_error = Some((
                                    index,
                                    file.clone(),
                                    format!(
                                        "{error}; 连续核验失败 {} 次，已停止自动重试",
                                        job.attempts.saturating_add(1)
                                    ),
                                ));
                                break;
                            }
                            Err(ManifestInspectError::Conflict(error)) => {
                                inspection_error = Some((index, file.clone(), error));
                                break;
                            }
                        };
                        match inspected {
                            ManifestFileState::Present {
                                hash_verified: true,
                            } => {}
                            ManifestFileState::Present {
                                hash_verified: false,
                            } if manual_migration => {}
                            ManifestFileState::Present {
                                hash_verified: false,
                            } => {
                                unverified_existing = Some(file.path.clone());
                                break;
                            }
                            ManifestFileState::Missing => {
                                pending = Some((
                                    index,
                                    file.clone(),
                                    CopyCheckpointOperation::CopyFile,
                                    file.path.clone(),
                                ));
                                break;
                            }
                            ManifestFileState::MissingDirectory { path } => {
                                pending = Some((
                                    index,
                                    file.clone(),
                                    CopyCheckpointOperation::CreateDirectory,
                                    path,
                                ));
                                break;
                            }
                        }
                        job.manifest_cursor = index + 1;
                    }
                    if let Some((index, file, error)) = inspection_error {
                        set_review_existing_checkpoint(
                            &mut job,
                            index,
                            &file,
                            format!("OpenList 只读核验失败，已停止自动操作并保留目标锁: {error}"),
                        )?;
                    } else if let Some(path) = unverified_existing {
                        let cursor = job.manifest_cursor;
                        let file = manifest
                            .get(cursor)
                            .ok_or("待核验目标文件的 manifest cursor 越界")?;
                        set_review_existing_checkpoint(
                            &mut job,
                            cursor,
                            file,
                            format!(
                                "目标已存在同名同大小文件但源/目标没有可比较哈希，无法证明内容一致，已拒绝覆盖或跳过: {path}"
                            ),
                        )?;
                    } else if let Some((index, file, operation, checkpoint_path)) = pending {
                        job.manifest_cursor = index;
                        if expected_stage == "copy_legacy_reconcile" {
                            set_review_existing_checkpoint(
                                &mut job,
                                index,
                                &file,
                                format!(
                                    "旧复制任务目标结构不完整，未自动创建目录或重提复制，请人工核验: {checkpoint_path}"
                                ),
                            )?;
                        } else {
                            job.copy_checkpoint_json =
                                Some(encode_copy_checkpoint(&CopyCheckpoint {
                                    path: checkpoint_path,
                                    size: if operation == CopyCheckpointOperation::CopyFile {
                                        file.size
                                    } else {
                                        0
                                    },
                                    operation,
                                    phase: CopyCheckpointPhase::Prepared,
                                    submitted_at: None,
                                    terminal_failure_verified: false,
                                })?);
                            job.openlist_task_id = None;
                            job.stage = "copy_submitting".to_string();
                        }
                    } else if job.manifest_cursor < manifest.len() {
                        job.next_attempt_at = Some(Utc::now().to_rfc3339());
                    } else {
                        let mut final_review = None;
                        for (index, file) in manifest.iter().enumerate() {
                            match openlist
                                .inspect_manifest_file(
                                    &job.source_openlist_path,
                                    &job.target_openlist_path,
                                    &file.path,
                                    file.size,
                                )
                                .await
                            {
                                Ok(ManifestFileState::Present {
                                    hash_verified: true,
                                }) => {}
                                Ok(ManifestFileState::Present {
                                    hash_verified: false,
                                }) if manual_migration => {}
                                Ok(ManifestFileState::Present {
                                    hash_verified: false,
                                }) => {
                                    final_review = Some((
                                        index,
                                        file.clone(),
                                        format!(
                                            "最终核验无法用共同哈希证明目标文件内容一致: {}",
                                            file.path
                                        ),
                                    ));
                                    break;
                                }
                                Ok(ManifestFileState::Missing) => {
                                    final_review = Some((
                                        index,
                                        file.clone(),
                                        format!("最终核验发现目标文件不存在: {}", file.path),
                                    ));
                                    break;
                                }
                                Ok(ManifestFileState::MissingDirectory { path }) => {
                                    final_review = Some((
                                        index,
                                        file.clone(),
                                        format!("最终核验发现目标目录不存在: {path}"),
                                    ));
                                    break;
                                }
                                Err(ManifestInspectError::Transient(error))
                                    if job.attempts.saturating_add(1)
                                        < MAX_STAGE_ERROR_ATTEMPTS =>
                                {
                                    return Err(error);
                                }
                                Err(ManifestInspectError::Transient(error)) => {
                                    final_review = Some((
                                        index,
                                        file.clone(),
                                        format!(
                                            "{error}; 最终核验连续失败 {} 次，已停止自动重试",
                                            job.attempts.saturating_add(1)
                                        ),
                                    ));
                                    break;
                                }
                                Err(ManifestInspectError::Conflict(error)) => {
                                    final_review = Some((index, file.clone(), error));
                                    break;
                                }
                            }
                        }
                        if let Some((index, file, error)) = final_review {
                            set_review_existing_checkpoint(
                                &mut job,
                                index,
                                &file,
                                format!(
                                    "OpenList 最终 manifest 核验失败，已停止自动操作并保留目标锁: {error}"
                                ),
                            )?;
                        } else if let Err(error) =
                            require_openlist_tasks_terminal(&openlist, &job).await
                        {
                            job.stage = "copy_manual_review".to_string();
                            job.next_attempt_at = None;
                            job.last_error = Some(format!(
                                "OpenList manifest 已核验，但远端任务尚未证明终止；不会释放目标锁或继续迁移: {error}"
                            ));
                        } else {
                            job.copy_checkpoint_json = None;
                            job.openlist_task_id = None;
                            job.copy_lock_acquired = false;
                            if manual_migration {
                                // Persist the verified copy and release its target lock before
                                // touching qB. A qB outage must never send this job back through
                                // the OpenList copy state machine or strand the copy lock.
                                job.stage = "copy_verified".to_string();
                                job.next_attempt_at = Some(Utc::now().to_rfc3339());
                            } else {
                                job.stage = "completed".to_string();
                                job.completed_at = Some(Utc::now().to_rfc3339());
                            }
                        }
                    }
                } else {
                    let manifest = normalize_torrent_manifest(
                        source_qb.get_torrent_files(&job.infohash).await?,
                    )?;
                    validate_manifest_paths_snapshot(&manifest, &job.source_files_json)?;
                    job.source_files_json = encode_manifest_paths(&manifest)?;
                    job.source_manifest_json = serde_json::to_string(&manifest)
                        .map_err(|error| format!("序列化旧种子文件大小快照失败: {error}"))?;
                    job.manifest_cursor = 0;
                }
            }
            "copy_submitting" => {
                if !self
                    .acquire_copy_lock(&mut job, expected_version, &expected_stage)
                    .await?
                {
                    job.next_attempt_at =
                        Some((Utc::now() + ChronoDuration::seconds(30)).to_rfc3339());
                } else {
                    let mut checkpoint = decode_copy_checkpoint(
                        job.copy_checkpoint_json
                            .as_deref()
                            .ok_or("复制 checkpoint 缺失")?,
                    )?;
                    let (checkpoint_index, manifest_file) =
                        validate_copy_checkpoint(&checkpoint, &job)?;
                    if checkpoint_index != job.manifest_cursor {
                        return Err(format!(
                            "复制 checkpoint 与 manifest cursor 不一致: {checkpoint_index} != {}",
                            job.manifest_cursor
                        ));
                    }
                    let observed = match openlist
                        .inspect_manifest_file(
                            &job.source_openlist_path,
                            &job.target_openlist_path,
                            &manifest_file.path,
                            manifest_file.size,
                        )
                        .await
                    {
                        Ok(observed) => Some(observed),
                        Err(ManifestInspectError::Transient(error)) => return Err(error),
                        Err(ManifestInspectError::Conflict(error)) => {
                            if checkpoint.phase == CopyCheckpointPhase::Prepared {
                                set_review_existing_checkpoint(
                                    &mut job,
                                    checkpoint_index,
                                    &manifest_file,
                                    format!(
                                        "OpenList 只读核验发现冲突，已停止自动操作并保留目标锁: {error}"
                                    ),
                                )?;
                            } else {
                                job.stage = "copy_manual_review".to_string();
                                job.next_attempt_at = None;
                                job.last_error = Some(format!(
                                    "OpenList 操作已提交但核验发现冲突，已保留原 checkpoint 和目标锁: {error}"
                                ));
                            }
                            None
                        }
                    };
                    if let Some(observed) = observed {
                        if checkpoint.operation == CopyCheckpointOperation::ReviewExisting
                            && !manual_migration
                            && let ManifestFileState::Present {
                                hash_verified: false,
                            } = &observed
                        {
                            job.stage = "copy_manual_review".to_string();
                            job.next_attempt_at = None;
                            job.last_error = Some(format!(
                                "目标仍是同名同大小但无可比较哈希的文件，自动复制无法安全确认内容: {}",
                                checkpoint.path
                            ));
                        } else if checkpoint.operation == CopyCheckpointOperation::ReviewExisting
                            && let ManifestFileState::Missing = &observed
                        {
                            job.stage = "copy_manual_review".to_string();
                            job.next_attempt_at = None;
                            job.last_error = Some(format!(
                                "只读重新检查确认目标文件仍不存在；为避免重复提交，任务保持人工处理状态: {}",
                                checkpoint.path
                            ));
                        } else if checkpoint.operation == CopyCheckpointOperation::ReviewExisting
                            && let ManifestFileState::MissingDirectory { path } = &observed
                        {
                            job.stage = "copy_manual_review".to_string();
                            job.next_attempt_at = None;
                            job.last_error = Some(format!(
                                "只读重新检查确认目标目录仍不存在；为避免重复提交，任务保持人工处理状态: {path}"
                            ));
                        } else if checkpoint.operation == CopyCheckpointOperation::CopyFile
                            && let ManifestFileState::Present {
                                hash_verified: false,
                            } = &observed
                            && !manual_migration
                        {
                            if checkpoint.phase == CopyCheckpointPhase::Prepared {
                                set_review_existing_checkpoint(
                                    &mut job,
                                    checkpoint_index,
                                    &manifest_file,
                                    format!(
                                        "目标文件在复制提交前出现且没有可比较哈希，无法证明来源，已拒绝覆盖或提交: {}",
                                        checkpoint.path
                                    ),
                                )?;
                            } else {
                                job.stage = "copy_manual_review".to_string();
                                job.next_attempt_at = None;
                                job.last_error = Some(format!(
                                    "OpenList 复制提交后目标仅能按同名同大小核验，自动复制无法证明内容一致；已保留原 checkpoint 和目标锁: {}",
                                    checkpoint.path
                                ));
                            }
                        } else if checkpoint.operation == CopyCheckpointOperation::CopyFile
                            && let ManifestFileState::MissingDirectory { path } = &observed
                            && checkpoint.phase == CopyCheckpointPhase::Prepared
                        {
                            checkpoint.path = path.clone();
                            checkpoint.size = 0;
                            checkpoint.operation = CopyCheckpointOperation::CreateDirectory;
                            job.copy_checkpoint_json = Some(encode_copy_checkpoint(&checkpoint)?);
                            job.next_attempt_at = Some(Utc::now().to_rfc3339());
                        } else {
                            let present = match (&checkpoint.operation, &observed) {
                                (CopyCheckpointOperation::RemoveFile, _) => {
                                    return Err(
                                        "源文件删除 checkpoint 不允许进入复制核验阶段".to_string()
                                    );
                                }
                                (
                                    CopyCheckpointOperation::CopyFile,
                                    ManifestFileState::Present { hash_verified },
                                ) => {
                                    manifest_presence_is_verified(*hash_verified, manual_migration)
                                }
                                (CopyCheckpointOperation::CopyFile, ManifestFileState::Missing) => {
                                    false
                                }
                                (
                                    CopyCheckpointOperation::CopyFile,
                                    ManifestFileState::MissingDirectory { .. },
                                ) => false,
                                (CopyCheckpointOperation::ReviewExisting, observed) => {
                                    review_existing_observation_is_verified(
                                        observed,
                                        manual_migration,
                                    )
                                }
                                (
                                    CopyCheckpointOperation::CreateDirectory,
                                    ManifestFileState::MissingDirectory { path },
                                ) if path == &checkpoint.path => false,
                                (
                                    CopyCheckpointOperation::CreateDirectory,
                                    ManifestFileState::MissingDirectory { path },
                                ) if is_path_prefix(&checkpoint.path, path) => true,
                                (
                                    CopyCheckpointOperation::CreateDirectory,
                                    ManifestFileState::MissingDirectory { path },
                                ) => {
                                    return Err(format!(
                                        "目录 checkpoint 的父级结构发生变化: {} -> {path}",
                                        checkpoint.path
                                    ));
                                }
                                (CopyCheckpointOperation::CreateDirectory, _) => true,
                            };
                            if present {
                                if checkpoint.operation != CopyCheckpointOperation::CreateDirectory
                                {
                                    job.manifest_cursor = checkpoint_index + 1;
                                }
                                job.copy_checkpoint_json = None;
                                job.openlist_task_id = None;
                                job.stage = "copy_reconcile".to_string();
                            } else {
                                match checkpoint.phase {
                                    CopyCheckpointPhase::Prepared => {
                                        checkpoint.phase = CopyCheckpointPhase::Uncertain;
                                        checkpoint.submitted_at = Some(Utc::now().to_rfc3339());
                                        let checkpoint_json = encode_copy_checkpoint(&checkpoint)?;
                                        let owner = job
                                            .lease_owner
                                            .as_deref()
                                            .ok_or("迁移任务 lease owner 缺失")?;
                                        let checkpointed = self
                                            .db
                                            .checkpoint_media_relocation_copy_submission(
                                                job.id,
                                                expected_version,
                                                &expected_stage,
                                                owner,
                                                &checkpoint_json,
                                            )
                                            .await
                                            .map_err(|error| error.to_string())?;
                                        if !checkpointed {
                                            return Err(
                                        "复制提交前 checkpoint 写入失败，已拒绝调用 OpenList"
                                            .to_string(),
                                    );
                                        }
                                        job.copy_checkpoint_json = Some(checkpoint_json);
                                        let tasks = match checkpoint.operation {
                                            CopyCheckpointOperation::CopyFile => {
                                                openlist
                                                    .copy_manifest_file(
                                                        &job.source_openlist_path,
                                                        &job.target_openlist_path,
                                                        &checkpoint.path,
                                                        checkpoint.size,
                                                    )
                                                    .await?
                                            }
                                            CopyCheckpointOperation::CreateDirectory => {
                                                openlist
                                                    .create_directory_if_missing(&checkpoint.path)
                                                    .await?;
                                                Vec::new()
                                            }
                                            CopyCheckpointOperation::ReviewExisting => {
                                                return Err(
                                            "人工核验 checkpoint 不允许提交 OpenList 副作用"
                                                .to_string(),
                                        );
                                            }
                                            CopyCheckpointOperation::RemoveFile => {
                                                return Err(
                                                    "源文件删除 checkpoint 不允许进入复制提交阶段"
                                                        .to_string(),
                                                );
                                            }
                                        };
                                        job.openlist_task_id = encode_openlist_task_ids(
                                            tasks.iter().map(|task| task.id.clone()),
                                        );
                                        if tasks.is_empty() {
                                            job.next_attempt_at = Some(
                                                (Utc::now() + ChronoDuration::seconds(10))
                                                    .to_rfc3339(),
                                            );
                                        } else {
                                            job.stage = "copying".to_string();
                                            job.next_attempt_at = Some(
                                                (Utc::now() + ChronoDuration::seconds(30))
                                                    .to_rfc3339(),
                                            );
                                        }
                                    }
                                    CopyCheckpointPhase::Uncertain => {
                                        let now = Utc::now();
                                        match uncertain_submission_next_check(&checkpoint, now)? {
                                            Some(next_check) => {
                                                job.next_attempt_at = Some(next_check.to_rfc3339());
                                            }
                                            None => {
                                                job.stage = "copy_manual_review".to_string();
                                                job.next_attempt_at = None;
                                                job.last_error = Some(format!(
                                                    "OpenList 操作提交结果不确定且目标仍未出现，已拒绝自动重复提交，请人工核验: {}",
                                                    checkpoint.path
                                                ));
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            "copying" => {
                if !self
                    .acquire_copy_lock(&mut job, expected_version, &expected_stage)
                    .await?
                {
                    job.next_attempt_at =
                        Some((Utc::now() + ChronoDuration::seconds(30)).to_rfc3339());
                } else {
                    let task_ids = job
                        .openlist_task_id
                        .as_deref()
                        .map(decode_openlist_task_ids)
                        .unwrap_or_default();
                    if task_ids.is_empty() {
                        job.stage = "copy_manual_review".to_string();
                        job.next_attempt_at = None;
                        job.last_error = Some(
                            "复制任务 ID 缺失，无法证明原任务已终止；已拒绝自动重提".to_string(),
                        );
                    } else {
                        let mut observations = Vec::with_capacity(task_ids.len());
                        let mut failure_messages = Vec::new();
                        for task_id in task_ids {
                            match openlist.task_info_if_exists(&task_id).await? {
                                None => observations.push(CopyTaskObservation::Missing),
                                Some(task) if task.terminal_failure() => {
                                    observations.push(CopyTaskObservation::Failed);
                                    if !task.error.trim().is_empty() {
                                        failure_messages.push(task.error);
                                    }
                                }
                                Some(task) if task.succeeded() => {
                                    observations.push(CopyTaskObservation::Succeeded);
                                }
                                Some(_) => observations.push(CopyTaskObservation::Pending),
                            }
                        }
                        match decide_copy_tasks(&observations)? {
                            CopyTaskDecision::Uncertain => {
                                job.stage = "copy_manual_review".to_string();
                                job.next_attempt_at = None;
                                job.last_error = Some(
                                    "OpenList 复制任务不可见，无法证明远端任务已终止；已保留目标锁并停止自动处理"
                                        .to_string(),
                                );
                            }
                            CopyTaskDecision::Wait => {
                                let legacy_without_checkpoint = job.copy_checkpoint_json.is_none();
                                if checkpoint_pending_timed_out(
                                    job.copy_checkpoint_json.as_deref(),
                                    Utc::now(),
                                )? {
                                    job.stage = "copy_manual_review".to_string();
                                    job.next_attempt_at = None;
                                    job.last_error = Some(if legacy_without_checkpoint {
                                        "旧 OpenList 复制任务仍显示运行中但缺少提交 checkpoint，已停止无限轮询；请确认旧任务已终止后重试"
                                            .to_string()
                                    } else {
                                        "OpenList 复制任务长时间未终止，已停止自动轮询并保留目标锁；请人工 recheck 或确认远端任务已终止后 cancel"
                                            .to_string()
                                    });
                                } else {
                                    job.next_attempt_at = Some(
                                        (Utc::now() + ChronoDuration::seconds(30)).to_rfc3339(),
                                    );
                                }
                            }
                            CopyTaskDecision::AllFailed => {
                                if let Some(value) = job.copy_checkpoint_json.as_deref() {
                                    warn!(
                                        "all OpenList copy tasks for relocation job {} failed: {}",
                                        job.id,
                                        failure_messages.join("; ")
                                    );
                                    let mut checkpoint = decode_copy_checkpoint(value)?;
                                    if checkpoint.operation != CopyCheckpointOperation::CopyFile
                                        || checkpoint.phase != CopyCheckpointPhase::Uncertain
                                    {
                                        return Err(
                                            "终态失败的复制任务缺少 uncertain copy_file checkpoint"
                                                .to_string(),
                                        );
                                    }
                                    checkpoint.terminal_failure_verified = true;
                                    job.copy_checkpoint_json =
                                        Some(encode_copy_checkpoint(&checkpoint)?);
                                    job.stage = "copy_manual_review".to_string();
                                    job.next_attempt_at = None;
                                    job.last_error = Some(if failure_messages.is_empty() {
                                        "OpenList 复制任务失败，已停止自动重提；请核验目录后手动重试"
                                            .to_string()
                                    } else {
                                        format!(
                                            "OpenList 复制任务失败，已停止自动重提: {}",
                                            failure_messages.join("; ")
                                        )
                                    });
                                } else {
                                    job.stage = "copy_manual_review".to_string();
                                    job.next_attempt_at = None;
                                    job.last_error = Some(
                                        "旧复制任务全部失败，但缺少逐文件 checkpoint；已拒绝自动重提"
                                            .to_string(),
                                    );
                                }
                            }
                            CopyTaskDecision::VerifyTarget => {
                                job.stage = prepare_read_only_copy_reconcile(
                                    &mut job.copy_checkpoint_json,
                                    Utc::now(),
                                )?
                                .to_string();
                                job.next_attempt_at = Some(
                                    (Utc::now()
                                        + ChronoDuration::seconds(COPY_SETTLE_DELAY_SECONDS))
                                    .to_rfc3339(),
                                );
                            }
                        }
                    }
                }
            }
            "copy_succeeded" => {
                // Compatibility with jobs persisted by older versions. The authoritative
                // manifest reconciliation below is required before export and source removal.
                job.openlist_task_id = None;
                job.copy_checkpoint_json = None;
                job.manifest_cursor = 0;
                job.stage = "copy_legacy_reconcile".to_string();
            }
            "manifest_recheck" => {
                if !manual_migration {
                    return Err("自动复制任务不允许恢复 qB 迁移 manifest".to_string());
                }
                let recovery = async {
                    let manifest =
                        recover_authoritative_manifest_for_recheck(source_qb.as_ref(), &job)
                            .await?;
                    job.source_files_json = encode_manifest_paths(&manifest)?;
                    job.source_manifest_json = serde_json::to_string(&manifest)
                        .map_err(|error| format!("序列化恢复的权威 manifest 失败: {error}"))?;
                    let resume_stage = manifest_recheck_resume_stage(&job)?;
                    require_openlist_tasks_terminal(&openlist, &job).await?;
                    Ok::<_, String>(resume_stage)
                }
                .await;
                match recovery {
                    Ok(resume_stage) => {
                        if resume_stage == "copy_legacy_reconcile" {
                            job.manifest_cursor = 0;
                        }
                        job.stage = resume_stage.to_string();
                        job.copy_lock_acquired = false;
                        job.next_attempt_at = Some(Utc::now().to_rfc3339());
                        job.last_error = None;
                    }
                    Err(error) => {
                        job.stage = "manifest_required".to_string();
                        job.copy_lock_acquired = false;
                        job.next_attempt_at = None;
                        job.last_error = Some(format!(
                            "权威 manifest 重新检查未通过，未执行 OpenList 副作用或 qB 迁移: {error}"
                        ));
                    }
                }
            }
            "copy_verified" => {
                if !manual_migration {
                    return Err("自动复制任务不应进入 qB 种子导出阶段".to_string());
                }
                ensure_exported_torrent_data(source_qb.as_ref(), &mut job).await?;
                job.stage = "qb_reconcile".to_string();
            }
            "qb_reconcile" => {
                if !manual_migration {
                    return Err("自动复制任务不应进入 qB 恢复核验阶段".to_string());
                }
                let target_qb = target_qb.as_ref().ok_or("迁移任务缺少目标下载器")?;
                let target_torrents = target_qb
                    .list_torrents_by_hashes(&[job.infohash.clone()])
                    .await?;
                let target_torrent = target_torrents
                    .iter()
                    .find(|torrent| torrent.hash.eq_ignore_ascii_case(&job.infohash));
                match classify_qb_reconcile(
                    job.downloader_id == job.target_downloader_id,
                    &job.source_qb_path,
                    &job.target_qb_path,
                    &job.target_content_qb_path,
                    target_torrent,
                ) {
                    QbReconcileDecision::TargetQbSubmitted => {
                        job.stage = "target_qb_submitted".to_string();
                    }
                    QbReconcileDecision::TorrentExported => {
                        ensure_exported_torrent_data(source_qb.as_ref(), &mut job).await?;
                        job.stage = "torrent_exported".to_string();
                    }
                    QbReconcileDecision::SourceQbRemoved => {
                        if job.torrent_data.is_none() {
                            if job.downloader_id == job.target_downloader_id {
                                set_qb_manual_review(
                                    &mut job,
                                    "同一 qB 中已找不到种子，且没有已持久化的 torrent 数据；已拒绝盲目重建"
                                        .to_string(),
                                );
                            } else {
                                ensure_exported_torrent_data(source_qb.as_ref(), &mut job).await?;
                            }
                        } else if let Some(data) = job.torrent_data.as_deref() {
                            validate_exported_torrent(&job, data)?;
                        }
                        if job.stage != "qb_manual_review" {
                            job.stage = "source_qb_removed".to_string();
                        }
                    }
                    QbReconcileDecision::ManualReview(error) => {
                        set_qb_manual_review(&mut job, error);
                    }
                }
            }
            "torrent_exported" => {
                if job.downloader_id == job.target_downloader_id {
                    let existing = source_qb
                        .list_torrents_by_hashes(&[job.infohash.clone()])
                        .await?;
                    let current = existing
                        .iter()
                        .find(|torrent| torrent.hash.eq_ignore_ascii_case(&job.infohash));
                    match classify_qb_reconcile(
                        true,
                        &job.source_qb_path,
                        &job.target_qb_path,
                        &job.target_content_qb_path,
                        current,
                    ) {
                        QbReconcileDecision::TargetQbSubmitted => {
                            job.stage = "target_qb_submitted".to_string();
                        }
                        QbReconcileDecision::TorrentExported => {
                            source_qb.pause_torrent(&job.infohash).await?;
                            source_qb.delete_torrent(&job.infohash, false).await?;
                            job.stage = "source_qb_removed".to_string();
                        }
                        QbReconcileDecision::SourceQbRemoved => {
                            job.stage = "source_qb_removed".to_string();
                        }
                        QbReconcileDecision::ManualReview(error) => {
                            set_qb_manual_review(&mut job, error);
                        }
                    }
                } else {
                    job.stage = "source_qb_removed".to_string();
                }
            }
            "source_qb_removed" => {
                let target_qb = target_qb.as_ref().ok_or("迁移任务缺少目标下载器")?;
                let existing = target_qb
                    .list_torrents_by_hashes(&[job.infohash.clone()])
                    .await?;
                if let Some(torrent) = existing.first() {
                    match validate_target_torrent_location(torrent, &job) {
                        Ok(()) => job.stage = "target_qb_submitted".to_string(),
                        Err(_)
                            if job.downloader_id == job.target_downloader_id
                                && stage_elapsed_seconds(&job, Utc::now())?
                                    < SOURCE_QB_REMOVAL_SETTLE_SECONDS =>
                        {
                            job.next_attempt_at =
                                Some((Utc::now() + ChronoDuration::seconds(5)).to_rfc3339());
                        }
                        Err(location_error) => set_qb_manual_review(
                            &mut job,
                            format!(
                                "目标 qB 已存在同 infohash 但路径不匹配，已停止自动迁移: {location_error}"
                            ),
                        ),
                    }
                } else {
                    let torrent = job.torrent_data.clone().ok_or("导出的种子数据缺失")?;
                    let submission_error = target_qb
                        .add_torrent(
                            torrent,
                            &format!("{}.torrent", job.infohash),
                            &AddTorrentOptions {
                                save_path: Some(job.target_qb_path.clone()),
                                tags: Some("云母".to_string()),
                                category: download.as_ref().map(|download| {
                                    media_download_category(&download.target_key, tmdb_is_animation)
                                        .to_string()
                                }),
                                paused: true,
                                skip_checking: false,
                                root_folder: job.target_root_folder,
                                ..Default::default()
                            },
                        )
                        .await
                        .err();
                    let confirmation =
                        confirm_target_qb_torrent(target_qb.as_ref(), &job.infohash).await;
                    resolve_target_qb_submission(&job.infohash, submission_error, confirmation)?;
                    job.stage = "target_qb_submitted".to_string();
                }
            }
            "target_qb_submitted" => {
                let target_qb = target_qb.as_ref().ok_or("迁移任务缺少目标下载器")?;
                let torrents = target_qb
                    .list_torrents_by_hashes(&[job.infohash.clone()])
                    .await?;
                if let Some(torrent) = torrents.first() {
                    if let Err(error) = validate_target_torrent_location(torrent, &job) {
                        set_qb_manual_review(&mut job, error);
                    } else if let Err(error) = validate_target_torrent_manifest(
                        target_qb.as_ref(),
                        &job,
                        target_manifest_requires_complete(&expected_stage, &torrent.state),
                    )
                    .await
                    {
                        set_qb_manual_review(&mut job, error);
                    } else if target_torrent_is_hash_checking(&torrent.state) {
                        job.stage = "target_qb_checking".to_string();
                        job.next_attempt_at =
                            Some((Utc::now() + ChronoDuration::seconds(10)).to_rfc3339());
                    } else {
                        target_qb.recheck_torrent(&job.infohash).await?;
                        if observe_target_qb_check_started(target_qb.as_ref(), &job.infohash)
                            .await?
                        {
                            job.stage = "target_qb_checking".to_string();
                            job.next_attempt_at =
                                Some((Utc::now() + ChronoDuration::seconds(10)).to_rfc3339());
                        } else {
                            job.stage = "target_qb_check_requested".to_string();
                            job.next_attempt_at =
                                Some((Utc::now() + ChronoDuration::seconds(5)).to_rfc3339());
                        }
                    }
                } else {
                    set_qb_manual_review(
                        &mut job,
                        "目标 qB 中已提交并确认的种子消失，已停止自动重复提交".to_string(),
                    );
                }
            }
            "target_qb_check_requested" => {
                let target_qb = target_qb.as_ref().ok_or("迁移任务缺少目标下载器")?;
                let torrents = target_qb
                    .list_torrents_by_hashes(&[job.infohash.clone()])
                    .await?;
                match torrents.first() {
                    None => set_qb_manual_review(
                        &mut job,
                        "目标 qB 在等待完整性校验启动时丢失种子，已停止迁移".to_string(),
                    ),
                    Some(torrent) => {
                        if let Err(error) = validate_target_torrent_location(torrent, &job) {
                            set_qb_manual_review(&mut job, error);
                        } else if let Err(error) = validate_target_torrent_manifest(
                            target_qb.as_ref(),
                            &job,
                            target_manifest_requires_complete(&expected_stage, &torrent.state),
                        )
                        .await
                        {
                            set_qb_manual_review(&mut job, error);
                        } else if target_torrent_is_hash_checking(&torrent.state) {
                            job.stage = "target_qb_checking".to_string();
                            job.next_attempt_at =
                                Some((Utc::now() + ChronoDuration::seconds(10)).to_rfc3339());
                        } else if stage_elapsed_seconds(&job, Utc::now())?
                            >= TARGET_QB_CHECK_START_GRACE_SECONDS
                        {
                            set_qb_manual_review(
                                &mut job,
                                format!(
                                    "目标 qB 未进入完整性校验状态（{}），已停止迁移；未采用旧的 {:.2}% 进度",
                                    torrent.state,
                                    torrent.progress * 100.0
                                ),
                            );
                        } else {
                            job.next_attempt_at =
                                Some((Utc::now() + ChronoDuration::seconds(5)).to_rfc3339());
                        }
                    }
                }
            }
            "target_qb_checking" => {
                let target_qb = target_qb.as_ref().ok_or("迁移任务缺少目标下载器")?;
                let torrents = target_qb
                    .list_torrents_by_hashes(&[job.infohash.clone()])
                    .await?;
                match torrents.first() {
                    None => set_qb_manual_review(
                        &mut job,
                        "目标 qB 在完整性校验期间丢失种子，已停止迁移".to_string(),
                    ),
                    Some(torrent) => {
                        if let Err(error) = validate_target_torrent_location(torrent, &job) {
                            set_qb_manual_review(&mut job, error);
                        } else if let Err(error) = validate_target_torrent_manifest(
                            target_qb.as_ref(),
                            &job,
                            target_manifest_requires_complete(&expected_stage, &torrent.state),
                        )
                        .await
                        {
                            set_qb_manual_review(&mut job, error);
                        } else if target_torrent_is_check_unstable(&torrent.state) {
                            let elapsed = stage_elapsed_seconds(&job, Utc::now())?;
                            if elapsed >= TARGET_QB_CHECK_TIMEOUT_SECONDS {
                                set_qb_manual_review(
                                    &mut job,
                                    "目标 qB 完整性校验超时，已停止迁移且未清理源文件".to_string(),
                                );
                            } else {
                                job.next_attempt_at =
                                    Some((Utc::now() + ChronoDuration::seconds(10)).to_rfc3339());
                            }
                        } else if target_torrent_verified_complete(torrent) {
                            target_qb.start_torrent(&job.infohash).await?;
                            job.stage = "target_qb_starting".to_string();
                            job.next_attempt_at =
                                Some((Utc::now() + ChronoDuration::seconds(10)).to_rfc3339());
                        } else {
                            set_qb_manual_review(
                                &mut job,
                                format!(
                                    "目标 qB 完整性校验未通过（状态 {}，进度 {:.2}%），已保留源文件",
                                    torrent.state,
                                    torrent.progress * 100.0
                                ),
                            );
                        }
                    }
                }
            }
            "target_qb_starting" => {
                let target_qb = target_qb.as_ref().ok_or("迁移任务缺少目标下载器")?;
                let torrents = target_qb
                    .list_torrents_by_hashes(&[job.infohash.clone()])
                    .await?;
                let Some(torrent) = torrents.first() else {
                    set_qb_manual_review(
                        &mut job,
                        "目标 qB 在启动做种期间丢失种子，已保留源文件".to_string(),
                    );
                    job.next_attempt_at = None;
                    let updated = self
                        .db
                        .update_media_relocation_job(
                            &job,
                            expected_version,
                            &expected_stage,
                            config_guard,
                        )
                        .await
                        .map_err(|e| e.to_string())?;
                    return updated
                        .then_some(())
                        .ok_or_else(|| "迁移任务状态已被其他 worker 修改".to_string());
                };
                if let Err(error) = validate_target_torrent_location(torrent, &job) {
                    set_qb_manual_review(&mut job, error);
                } else if let Err(error) = validate_target_torrent_manifest(
                    target_qb.as_ref(),
                    &job,
                    target_manifest_requires_complete(&expected_stage, &torrent.state),
                )
                .await
                {
                    set_qb_manual_review(&mut job, error);
                } else if !target_torrent_verified_complete(torrent) {
                    set_qb_manual_review(
                        &mut job,
                        format!(
                            "目标 qB 在启动做种前不再完整（进度 {:.2}%），已保留源文件",
                            torrent.progress * 100.0
                        ),
                    );
                } else if !target_torrent_is_seeding(&torrent.state) {
                    if target_torrent_has_hard_error(&torrent.state)
                        || stage_elapsed_seconds(&job, Utc::now())?
                            >= TARGET_QB_START_TIMEOUT_SECONDS
                    {
                        set_qb_manual_review(
                            &mut job,
                            format!(
                                "目标 qB 未能进入做种状态（{}），已停止迁移且未清理源文件",
                                torrent.state
                            ),
                        );
                    } else {
                        job.next_attempt_at =
                            Some((Utc::now() + ChronoDuration::seconds(10)).to_rfc3339());
                    }
                } else {
                    // Source cleanup has its own durable, one-file-at-a-time state machine.
                    // Persisting this boundary keeps a crash from replaying a remove request.
                    job.stage = "source_removing".to_string();
                    job.openlist_task_id = None;
                    job.copy_checkpoint_json = None;
                    job.copy_lock_acquired = false;
                    job.manifest_cursor = 0;
                    job.next_attempt_at = Some(Utc::now().to_rfc3339());
                }
            }
            "source_removing" => {
                if !manual_migration {
                    return Err("自动复制任务不应进入源文件清理阶段".to_string());
                }
                let manifest = decode_source_manifest(&job.source_manifest_json)?
                    .ok_or("带大小的权威种子 manifest 为空，已拒绝自动删除源文件")?;
                validate_manifest_paths_snapshot(&manifest, &job.source_files_json)?;
                if job.manifest_cursor > manifest.len() {
                    return Err(format!(
                        "源文件清理 cursor 越界: {} > {}",
                        job.manifest_cursor,
                        manifest.len()
                    ));
                }

                let target_qb = target_qb.as_ref().ok_or("迁移任务缺少目标下载器")?;
                let target_torrents = target_qb
                    .list_torrents_by_hashes(&[job.infohash.clone()])
                    .await?;
                let Some(target_torrent) = target_torrents
                    .iter()
                    .find(|torrent| torrent.hash.eq_ignore_ascii_case(&job.infohash))
                else {
                    set_source_remove_manual_review(
                        &mut job,
                        "目标 qB 在源文件清理期间丢失种子，已停止继续删除".to_string(),
                    );
                    let updated = self
                        .db
                        .update_media_relocation_job(
                            &job,
                            expected_version,
                            &expected_stage,
                            config_guard,
                        )
                        .await
                        .map_err(|error| error.to_string())?;
                    return updated
                        .then_some(())
                        .ok_or_else(|| "迁移任务状态已被其他 worker 修改".to_string());
                };
                if let Err(error) = validate_target_torrent_location(target_torrent, &job) {
                    set_source_remove_manual_review(&mut job, error);
                } else if let Err(error) =
                    validate_target_torrent_manifest(target_qb.as_ref(), &job, true).await
                {
                    set_source_remove_manual_review(&mut job, error);
                } else if !target_torrent_verified_complete(target_torrent)
                    || !target_torrent_is_seeding(&target_torrent.state)
                {
                    set_source_remove_manual_review(
                        &mut job,
                        format!(
                            "目标 qB 在源文件清理期间不再处于完整做种状态（{}，{:.2}%），已停止继续删除",
                            target_torrent.state,
                            target_torrent.progress * 100.0
                        ),
                    );
                } else {
                    if job.downloader_id != job.target_downloader_id {
                        let source_torrents = source_qb
                            .list_torrents_by_hashes(&[job.infohash.clone()])
                            .await?;
                        if source_torrents
                            .iter()
                            .any(|torrent| torrent.hash.eq_ignore_ascii_case(&job.infohash))
                        {
                            source_qb.pause_torrent(&job.infohash).await?;
                            source_qb.delete_torrent(&job.infohash, false).await?;
                        }
                    }

                    // Revalidate the complete target immediately before every source mutation.
                    verify_openlist_manifest(&openlist, &job, &manifest, true).await?;
                    if job.manifest_cursor == manifest.len() {
                        job.openlist_task_id = None;
                        job.copy_checkpoint_json = None;
                        job.copy_lock_acquired = false;
                        job.stage = "source_removed".to_string();
                        job.next_attempt_at = Some(Utc::now().to_rfc3339());
                    } else {
                        let file = manifest[job.manifest_cursor].clone();
                        let source_path = join_path(&job.source_openlist_path, &file.path)?;
                        let referenced_by_other_torrents =
                            source_paths_referenced_by_other_torrents(source_qb.as_ref(), &job)
                                .await?;
                        if referenced_by_other_torrents
                            .contains(&openlist_identity_key(&source_path))
                        {
                            warn!(
                                "media relocation job {} retained source file referenced by another torrent: {}",
                                job.id, source_path
                            );
                            job.copy_checkpoint_json = None;
                            job.manifest_cursor += 1;
                            job.next_attempt_at = Some(Utc::now().to_rfc3339());
                        } else if let Some(value) = job.copy_checkpoint_json.as_deref() {
                            let checkpoint = decode_copy_checkpoint(value)?;
                            if checkpoint.operation != CopyCheckpointOperation::RemoveFile
                                || checkpoint.phase != CopyCheckpointPhase::Uncertain
                            {
                                return Err("源文件清理阶段包含无效的副作用 checkpoint".to_string());
                            }
                            let (checkpoint_index, checkpoint_file) =
                                validate_copy_checkpoint(&checkpoint, &job)?;
                            if checkpoint_index != job.manifest_cursor || checkpoint_file != file {
                                return Err("源文件清理 checkpoint 与 cursor 不一致".to_string());
                            }
                            let removal_needed = openlist
                                .manifest_file_removal_needed(
                                    &job.source_openlist_path,
                                    &job.target_openlist_path,
                                    &file.path,
                                    file.size,
                                )
                                .await?;
                            if !removal_needed {
                                job.copy_checkpoint_json = None;
                                job.manifest_cursor += 1;
                                job.next_attempt_at = Some(Utc::now().to_rfc3339());
                            } else if let Some(next_check) =
                                uncertain_removal_next_check(&checkpoint, Utc::now())?
                            {
                                job.next_attempt_at = Some(next_check.to_rfc3339());
                            } else {
                                set_source_remove_manual_review(
                                    &mut job,
                                    format!(
                                        "OpenList 删除请求结果不确定且源文件仍可见；为避免重复删除请求，已停止自动处理，请核验: {}",
                                        file.path
                                    ),
                                );
                            }
                        } else {
                            let removal_needed = openlist
                                .manifest_file_removal_needed(
                                    &job.source_openlist_path,
                                    &job.target_openlist_path,
                                    &file.path,
                                    file.size,
                                )
                                .await?;
                            if !removal_needed {
                                job.manifest_cursor += 1;
                                job.next_attempt_at = Some(Utc::now().to_rfc3339());
                            } else {
                                let checkpoint = CopyCheckpoint {
                                    path: file.path.clone(),
                                    size: file.size,
                                    operation: CopyCheckpointOperation::RemoveFile,
                                    phase: CopyCheckpointPhase::Uncertain,
                                    submitted_at: Some(Utc::now().to_rfc3339()),
                                    terminal_failure_verified: false,
                                };
                                let checkpoint_json = encode_copy_checkpoint(&checkpoint)?;
                                let owner = job
                                    .lease_owner
                                    .as_deref()
                                    .ok_or("迁移任务 lease owner 缺失")?;
                                let checkpointed = self
                                    .db
                                    .checkpoint_media_relocation_copy_submission(
                                        job.id,
                                        expected_version,
                                        &expected_stage,
                                        owner,
                                        &checkpoint_json,
                                    )
                                    .await
                                    .map_err(|error| error.to_string())?;
                                if !checkpointed {
                                    return Err(
                                        "删除提交前 checkpoint 写入失败，已拒绝调用 OpenList"
                                            .to_string(),
                                    );
                                }
                                job.copy_checkpoint_json = Some(checkpoint_json);
                                openlist
                                    .remove_manifest_file_if_exists(
                                        &job.source_openlist_path,
                                        &job.target_openlist_path,
                                        &file.path,
                                        file.size,
                                    )
                                    .await?;
                                // An HTTP success only acknowledges the request. Keep the durable
                                // uncertain checkpoint until a fresh listing proves the source is
                                // absent; this also covers asynchronous or no-op storage drivers.
                                let now = Utc::now();
                                job.next_attempt_at = Some(
                                    uncertain_removal_next_check(&checkpoint, now)?
                                        .unwrap_or(now)
                                        .to_rfc3339(),
                                );
                            }
                        }
                    }
                }
            }
            "source_removed" => {
                let target_qb = target_qb.as_ref().ok_or("迁移任务缺少目标下载器")?;
                // Keep this call for jobs created by older versions that reached this stage
                // before the target torrent was started.
                target_qb.start_torrent(&job.infohash).await?;
                job.stage = "completed".to_string();
                job.completed_at = Some(Utc::now().to_rfc3339());
                job.torrent_data = None;
            }
            "completed"
            | "cancelled"
            | "planning_manual_review"
            | "qb_manual_review"
            | "source_remove_manual_review" => {
                return Ok(());
            }
            stage => return Err(format!("未知迁移阶段: {stage}")),
        }
        if !matches!(
            job.stage.as_str(),
            "planning_manual_review"
                | "copy_manual_review"
                | "qb_manual_review"
                | "source_remove_manual_review"
                | "manifest_required"
        ) {
            job.last_error = None;
        }
        if job.stage != expected_stage {
            job.attempts = 0;
        }
        let updated = self
            .db
            .update_media_relocation_job(&job, expected_version, &expected_stage, config_guard)
            .await
            .map_err(|e| e.to_string())?;
        if !updated {
            return Err("迁移任务状态已被其他 worker 修改".to_string());
        }
        Ok(())
    }

    async fn acquire_copy_lock(
        &self,
        job: &mut MediaRelocationJob,
        expected_version: i64,
        expected_stage: &str,
    ) -> Result<bool, String> {
        if job.copy_lock_acquired {
            return Ok(true);
        }
        let owner = job
            .lease_owner
            .as_deref()
            .ok_or("迁移任务 lease owner 缺失")?;
        let acquired = self
            .db
            .try_acquire_media_relocation_target_lock(
                job.id,
                expected_version,
                expected_stage,
                owner,
                &job.target_openlist_path,
            )
            .await
            .map_err(|error| error.to_string())?;
        if acquired {
            job.copy_lock_acquired = true;
        }
        Ok(acquired)
    }

    async fn record_retry(&self, mut job: MediaRelocationJob, error: String) {
        job.attempts = job.attempts.saturating_add(1);
        job.last_error = Some(error);
        let backoff = 30_i64.saturating_mul(1_i64 << job.attempts.min(6));
        job.next_attempt_at = Some((Utc::now() + ChronoDuration::seconds(backoff)).to_rfc3339());
        let manual_stage = (job.attempts >= MAX_STAGE_ERROR_ATTEMPTS).then(|| {
            if job.stage == "waiting_download" && !is_manual_migration_job(&job) {
                "planning_manual_review"
            } else if job.stage == "source_removing" {
                "source_remove_manual_review"
            } else if job.stage == "copy_verified" {
                "qb_manual_review"
            } else if job.stage.starts_with("copy") {
                "copy_manual_review"
            } else if is_manual_migration_job(&job)
                || migration_stage_uses_target_qb(&job.stage)
                || matches!(job.stage.as_str(), "torrent_exported" | "source_qb_removed")
            {
                "qb_manual_review"
            } else {
                "copy_manual_review"
            }
        });
        if let Some(stage) = manual_stage {
            job.last_error = Some(format!(
                "{}；连续失败 {} 次，已停止自动重试",
                job.last_error.as_deref().unwrap_or("迁移任务失败"),
                job.attempts
            ));
            job.next_attempt_at = None;
            warn!(
                job_id = job.id,
                stage, "relocation job moved to manual review"
            );
        }
        let Some(_) = job.lease_owner.as_deref() else {
            error!(
                "failed to record media relocation job {} error because lease owner is missing",
                job.id
            );
            return;
        };
        match self
            .db
            .record_media_relocation_retry(&job, manual_stage)
            .await
        {
            Ok(true) => {}
            Ok(false) => error!(
                "failed to record media relocation job {} error because its state changed",
                job.id
            ),
            Err(record_error) => error!(
                "failed to record media relocation job {} error: {}",
                job.id, record_error
            ),
        }
    }
}

pub fn archive_relative_directory(
    primary_type: &str,
    primary_genre: &str,
    tmdb_year: Option<u32>,
) -> String {
    format!(
        "云母/{}/{}/{}",
        primary_type,
        primary_genre,
        tmdb_year
            .map(|year| year.to_string())
            .unwrap_or_else(|| "年份未知".to_string())
    )
}

fn is_manual_migration_job(job: &MediaRelocationJob) -> bool {
    job.media_download_id.is_none()
}

fn automatic_qb_followup_stage(stage: &str) -> bool {
    matches!(
        stage,
        "manifest_recheck"
            | "copy_verified"
            | "qb_reconcile"
            | "torrent_exported"
            | "source_qb_removed"
            | "target_qb_submitted"
            | "target_qb_check_requested"
            | "target_qb_checking"
            | "target_qb_starting"
            | "qb_manual_review"
            | "source_removing"
            | "source_remove_manual_review"
            | "source_removed"
    )
}

fn auto_copy_resume_stage(
    source_openlist_path: &str,
    source_manifest_json: &str,
) -> Result<&'static str, String> {
    if source_openlist_path.trim().is_empty()
        || decode_source_manifest(source_manifest_json)?.is_none()
    {
        Ok("waiting_download")
    } else {
        Ok("copy_reconcile")
    }
}

fn stage_requires_distinct_relocation_paths(stage: &str) -> bool {
    !matches!(stage, "waiting_download" | "auto_copy_paused")
}

#[derive(Debug, PartialEq, Eq)]
enum QbReconcileDecision {
    TargetQbSubmitted,
    TorrentExported,
    SourceQbRemoved,
    ManualReview(String),
}

fn classify_qb_reconcile(
    same_qb: bool,
    source_qb_path: &str,
    target_qb_path: &str,
    target_content_qb_path: &str,
    target_torrent: Option<&TorrentInfo>,
) -> QbReconcileDecision {
    let Some(torrent) = target_torrent else {
        return QbReconcileDecision::SourceQbRemoved;
    };

    if !same_qb {
        return match validate_target_torrent_location_snapshot(
            torrent,
            target_qb_path,
            target_content_qb_path,
        ) {
            Ok(()) => QbReconcileDecision::TargetQbSubmitted,
            Err(error) => QbReconcileDecision::ManualReview(format!(
                "目标 qB 已存在同 infohash 但路径不匹配，已停止自动迁移: {error}"
            )),
        };
    }

    let actual_save = match normalize_path(&torrent.save_path) {
        Ok(path) => path,
        Err(error) => {
            return QbReconcileDecision::ManualReview(format!(
                "同一 qB 中种子的当前保存路径无效，已停止自动迁移: {error}"
            ));
        }
    };
    let target_save = match normalize_path(target_qb_path) {
        Ok(path) => path,
        Err(error) => {
            return QbReconcileDecision::ManualReview(format!(
                "迁移任务的目标 qB 路径无效，已停止自动迁移: {error}"
            ));
        }
    };
    if actual_save == target_save {
        return match validate_target_torrent_location_snapshot(
            torrent,
            target_qb_path,
            target_content_qb_path,
        ) {
            Ok(()) => QbReconcileDecision::TargetQbSubmitted,
            Err(error) => QbReconcileDecision::ManualReview(format!(
                "同一 qB 中种子位于目标保存路径，但内容路径不匹配，已停止自动迁移: {error}"
            )),
        };
    }
    let source_save = match normalize_path(source_qb_path) {
        Ok(path) => path,
        Err(error) => {
            return QbReconcileDecision::ManualReview(format!(
                "迁移任务的源 qB 路径无效，已停止自动迁移: {error}"
            ));
        }
    };
    if actual_save == source_save {
        QbReconcileDecision::TorrentExported
    } else {
        QbReconcileDecision::ManualReview(format!(
            "同一 qB 中种子位于非源非目标路径，已停止自动迁移: {actual_save}"
        ))
    }
}

fn migration_stage_uses_target_qb(stage: &str) -> bool {
    matches!(
        stage,
        "qb_reconcile"
            | "source_qb_removed"
            | "target_qb_submitted"
            | "target_qb_check_requested"
            | "target_qb_checking"
            | "target_qb_starting"
            | "source_removing"
            | "source_removed"
    )
}

fn set_qb_manual_review(job: &mut MediaRelocationJob, error: String) {
    job.stage = "qb_manual_review".to_string();
    job.next_attempt_at = None;
    job.last_error = Some(error);
}

fn set_waiting_download_manual_review(
    job: &mut MediaRelocationJob,
    manual_migration: bool,
    error: String,
) {
    job.stage = if manual_migration {
        "qb_manual_review"
    } else {
        "planning_manual_review"
    }
    .to_string();
    job.openlist_task_id = None;
    job.copy_checkpoint_json = None;
    job.copy_lock_acquired = false;
    job.manifest_cursor = 0;
    job.next_attempt_at = None;
    job.last_error = Some(error);
}

fn set_source_remove_manual_review(job: &mut MediaRelocationJob, error: String) {
    job.stage = "source_remove_manual_review".to_string();
    job.next_attempt_at = None;
    job.last_error = Some(error);
}

fn stage_elapsed_seconds(
    job: &MediaRelocationJob,
    now: chrono::DateTime<Utc>,
) -> Result<i64, String> {
    let value = [
        job.stage_started_at.as_str(),
        job.updated_at.as_str(),
        job.created_at.as_str(),
    ]
    .into_iter()
    .find(|value| !value.trim().is_empty())
    .ok_or("迁移任务缺少阶段开始时间")?;
    let started_at = chrono::DateTime::parse_from_rfc3339(value)
        .map_err(|error| format!("解析迁移阶段开始时间失败: {error}"))?
        .with_timezone(&Utc);
    Ok(now.signed_duration_since(started_at).num_seconds().max(0))
}

fn target_torrent_is_check_unstable(state: &str) -> bool {
    matches!(
        state.to_ascii_lowercase().as_str(),
        "checkingdl" | "checkingup" | "checkingresumedata"
    )
}

fn target_torrent_is_hash_checking(state: &str) -> bool {
    matches!(
        state.to_ascii_lowercase().as_str(),
        "checkingdl" | "checkingup"
    )
}

fn target_manifest_requires_complete(stage: &str, state: &str) -> bool {
    stage == "target_qb_starting"
        || (stage == "target_qb_checking" && !target_torrent_is_check_unstable(state))
}

fn target_torrent_has_hard_error(state: &str) -> bool {
    matches!(
        state.to_ascii_lowercase().as_str(),
        "error" | "missingfiles" | "unknown"
    )
}

fn waiting_torrent_state_can_progress(state: &str) -> bool {
    matches!(
        state.to_ascii_lowercase().as_str(),
        "downloading"
            | "stalleddl"
            | "queueddl"
            | "forceddl"
            | "pauseddl"
            | "stoppeddl"
            | "checkingdl"
            | "checkingup"
            | "checkingresumedata"
            | "metadl"
            | "allocating"
            | "moving"
    )
}

fn target_torrent_verified_complete(torrent: &TorrentInfo) -> bool {
    !target_torrent_has_hard_error(&torrent.state)
        && !target_torrent_is_check_unstable(&torrent.state)
        && matches!(
            torrent.state.to_ascii_lowercase().as_str(),
            "uploading" | "stalledup" | "queuedup" | "forcedup" | "pausedup" | "stoppedup"
        )
        && torrent.progress.is_finite()
        && torrent.progress >= 0.999_999
}

fn relocation_scheduler_delay(
    configured_delay_secs: u64,
    next_attempt_at: Option<&str>,
    now: chrono::DateTime<Utc>,
) -> u64 {
    let configured_delay_secs = configured_delay_secs.max(5);
    let Some(next_attempt_at) = next_attempt_at else {
        return configured_delay_secs;
    };
    let Ok(next_attempt_at) = chrono::DateTime::parse_from_rfc3339(next_attempt_at) else {
        return 5;
    };
    let remaining_ms = next_attempt_at
        .with_timezone(&Utc)
        .signed_duration_since(now)
        .num_milliseconds()
        .max(0) as u64;
    let remaining_secs = remaining_ms.saturating_add(999) / 1_000;
    configured_delay_secs.min(remaining_secs.max(5))
}

async fn confirm_target_qb_torrent(
    target_qb: &dyn DownloaderClient,
    infohash: &str,
) -> Result<bool, String> {
    let hashes = [infohash.to_string()];
    let mut last_result = Ok(false);
    for attempt in 0..TARGET_QB_CONFIRM_ATTEMPTS {
        match target_qb.list_torrents_by_hashes(&hashes).await {
            Ok(torrents)
                if torrents
                    .iter()
                    .any(|torrent| torrent.hash.eq_ignore_ascii_case(infohash)) =>
            {
                return Ok(true);
            }
            Ok(_) => last_result = Ok(false),
            Err(error) => last_result = Err(error),
        }
        if attempt + 1 < TARGET_QB_CONFIRM_ATTEMPTS {
            tokio::time::sleep(TARGET_QB_CONFIRM_INTERVAL).await;
        }
    }
    last_result
}

async fn observe_target_qb_check_started(
    target_qb: &dyn DownloaderClient,
    infohash: &str,
) -> Result<bool, String> {
    observe_target_qb_check_started_with_policy(
        target_qb,
        infohash,
        TARGET_QB_CHECK_OBSERVE_ATTEMPTS,
        TARGET_QB_CHECK_OBSERVE_INTERVAL,
    )
    .await
}

async fn observe_target_qb_check_started_with_policy(
    target_qb: &dyn DownloaderClient,
    infohash: &str,
    attempts: usize,
    interval: Duration,
) -> Result<bool, String> {
    let hashes = [infohash.to_string()];
    let attempts = attempts.max(1);
    let mut observed_torrent = false;
    let mut last_error = None;
    for attempt in 0..attempts {
        match target_qb.list_torrents_by_hashes(&hashes).await {
            Ok(torrents) => {
                if let Some(torrent) = torrents
                    .iter()
                    .find(|torrent| torrent.hash.eq_ignore_ascii_case(infohash))
                {
                    observed_torrent = true;
                    if target_torrent_is_hash_checking(&torrent.state) {
                        return Ok(true);
                    }
                } else {
                    last_error = Some(format!(
                        "目标 qB 在请求完整性校验后暂未返回种子: {infohash}"
                    ));
                }
            }
            Err(error) => last_error = Some(error),
        }
        if attempt + 1 < attempts {
            tokio::time::sleep(interval).await;
        }
    }
    if observed_torrent {
        Ok(false)
    } else {
        Err(last_error
            .unwrap_or_else(|| format!("目标 qB 在请求完整性校验后未返回种子: {infohash}")))
    }
}

fn resolve_target_qb_submission(
    infohash: &str,
    submission_error: Option<String>,
    confirmation: Result<bool, String>,
) -> Result<(), String> {
    match confirmation {
        Ok(true) => Ok(()),
        Ok(false) => Err(submission_error.map_or_else(
            || format!("目标 qB 提交失败: 添加接口返回成功，但核验未发现种子 {infohash}"),
            |error| format!("目标 qB 提交失败: {error}; 核验未发现种子 {infohash}"),
        )),
        Err(confirmation_error) => Err(submission_error.map_or_else(
            || format!("目标 qB 提交结果核验失败: {confirmation_error}"),
            |error| format!("目标 qB 提交失败: {error}; 提交结果核验失败: {confirmation_error}"),
        )),
    }
}

fn find_completed_torrent(mut torrents: Vec<TorrentInfo>) -> Result<TorrentInfo, String> {
    let torrent = torrents.pop().ok_or("qB 中找不到对应种子")?;
    if !torrent_is_complete(
        torrent.completion_on,
        torrent.downloaded,
        torrent.size,
        torrent.progress,
        &torrent.state,
    ) {
        return Err("种子尚未下载完成".to_string());
    }
    Ok(torrent)
}

async fn ensure_exported_torrent_data(
    source_qb: &dyn DownloaderClient,
    job: &mut MediaRelocationJob,
) -> Result<(), String> {
    if let Some(data) = job.torrent_data.as_deref() {
        return validate_exported_torrent(job, data);
    }

    let torrents = source_qb
        .list_torrents_by_hashes(&[job.infohash.clone()])
        .await?;
    let source = torrents
        .into_iter()
        .find(|torrent| torrent.hash.eq_ignore_ascii_case(&job.infohash))
        .ok_or("源 qB 中找不到对应种子，且没有已持久化的 torrent 数据")?;
    let actual_save = normalize_path(&source.save_path)?;
    let expected_save = normalize_path(&job.source_qb_path)?;
    if actual_save != expected_save {
        return Err(format!(
            "源 qB 种子的保存路径已变化，已拒绝导出: {actual_save} != {expected_save}"
        ));
    }
    find_completed_torrent(vec![source])?;
    let torrent_data = source_qb.export_torrent(&job.infohash).await?;
    validate_exported_torrent(job, &torrent_data)?;
    job.torrent_data = Some(torrent_data);
    Ok(())
}

pub(crate) fn torrent_is_complete(
    _completion_on: i64,
    downloaded: i64,
    size: i64,
    progress: f64,
    state: &str,
) -> bool {
    let state = state.to_ascii_lowercase();
    let stable_known_state = matches!(
        state.as_str(),
        "uploading"
            | "stalledup"
            | "queuedup"
            | "forcedup"
            | "pausedup"
            | "stoppedup"
            | "downloading"
            | "stalleddl"
            | "queueddl"
            | "forceddl"
            | "pauseddl"
            | "stoppeddl"
    );
    if !stable_known_state {
        return false;
    }
    let progress_complete = progress.is_finite() && progress >= 0.999_999;
    let bytes_complete = size > 0 && downloaded >= size;
    let reliable_complete_state = matches!(
        state.as_str(),
        "uploading" | "stalledup" | "queuedup" | "forcedup" | "pausedup" | "stoppedup"
    );
    progress_complete || bytes_complete || reliable_complete_state
}

fn target_torrent_is_seeding(state: &str) -> bool {
    matches!(
        state.to_ascii_lowercase().as_str(),
        "uploading" | "stalledup" | "queuedup" | "forcedup"
    )
}

fn validate_target_torrent_location(
    torrent: &TorrentInfo,
    job: &MediaRelocationJob,
) -> Result<(), String> {
    validate_target_torrent_location_snapshot(
        torrent,
        &job.target_qb_path,
        &job.target_content_qb_path,
    )
}

fn validate_target_torrent_location_snapshot(
    torrent: &TorrentInfo,
    target_qb_path: &str,
    target_content_qb_path: &str,
) -> Result<(), String> {
    if target_content_qb_path.is_empty() {
        return Err("旧归档任务缺少目标内容路径快照，已拒绝继续自动清理".to_string());
    }
    let actual_save = normalize_path(&torrent.save_path)?;
    let expected_save = normalize_path(target_qb_path)?;
    if actual_save != expected_save {
        return Err(format!(
            "目标 qB 保存路径不匹配: {actual_save} != {expected_save}"
        ));
    }
    let actual_content = normalize_path(&torrent.content_path)?;
    let expected_content = normalize_path(target_content_qb_path)?;
    if actual_content != expected_content {
        return Err(format!(
            "目标 qB 内容路径不匹配: {actual_content} != {expected_content}"
        ));
    }
    Ok(())
}

async fn validate_target_torrent_manifest(
    target_qb: &dyn DownloaderClient,
    job: &MediaRelocationJob,
    require_complete: bool,
) -> Result<(), String> {
    let files = target_qb.get_torrent_files(&job.infohash).await?;
    let actual = if require_complete {
        normalize_torrent_manifest(files)?
    } else {
        normalize_torrent_manifest_inner(files, false)?
    };
    let expected = decode_source_manifest(&job.source_manifest_json)?
        .ok_or("目标 qB 校验缺少带大小的权威源 manifest")?;
    if actual != expected {
        return Err("目标 qB 的种子文件结构与源种子不一致，已拒绝删除源文件".to_string());
    }
    Ok(())
}

async fn source_paths_referenced_by_other_torrents(
    source_qb: &dyn DownloaderClient,
    job: &MediaRelocationJob,
) -> Result<std::collections::BTreeSet<String>, String> {
    let source_files = decode_string_list(&job.source_files_json, "源种子文件清单")?;
    let source_qb_root = normalize_path(&job.source_qb_path)?;
    let source_openlist_root = normalize_path(&job.source_openlist_path)?;
    let source_paths = source_files
        .iter()
        .map(|file| join_path(&source_openlist_root, file).map(|path| openlist_identity_key(&path)))
        .collect::<Result<std::collections::BTreeSet<_>, _>>()?;
    let mut referenced = std::collections::BTreeSet::new();
    for torrent in source_qb.list_torrents(None).await? {
        if torrent.hash.eq_ignore_ascii_case(&job.infohash) {
            continue;
        }
        let save_path = normalize_path(&torrent.save_path)?;
        if identity_relative_path(&save_path, &source_qb_root)?.is_none()
            && identity_relative_path(&source_qb_root, &save_path)?.is_none()
        {
            continue;
        }
        for file in normalize_torrent_file_paths(source_qb.get_torrent_files(&torrent.hash).await?)?
        {
            let path = join_path(&save_path, &file)?;
            let Some(relative) = identity_relative_path(&path, &source_qb_root)? else {
                continue;
            };
            let openlist_path = if relative.is_empty() {
                source_openlist_root.clone()
            } else {
                join_path(&source_openlist_root, &relative)?
            };
            let identity = openlist_identity_key(&openlist_path);
            if source_paths.contains(&identity) {
                referenced.insert(identity);
            }
        }
    }
    Ok(referenced)
}

fn identity_relative_path(path: &str, root: &str) -> Result<Option<String>, String> {
    let path = normalize_path(path)?;
    let root = normalize_path(root)?;
    if path.starts_with('/') != root.starts_with('/') {
        return Ok(None);
    }
    let path_parts = path
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    let root_parts = root
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if root_parts.len() > path_parts.len()
        || root_parts
            .iter()
            .zip(&path_parts)
            .any(|(left, right)| openlist_identity_key(left) != openlist_identity_key(right))
    {
        return Ok(None);
    }
    Ok(Some(path_parts[root_parts.len()..].join("/")))
}

fn torrent_content_root(torrent: &TorrentInfo) -> String {
    select_torrent_content_root(&torrent.content_path, &torrent.save_path)
}

fn select_torrent_content_root(content_path: &str, save_path: &str) -> String {
    [content_path, save_path]
        .into_iter()
        .find(|path| !path.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_default()
}

fn normalize_optional_path(path: &str) -> Result<Option<String>, String> {
    if path.trim().is_empty() {
        Ok(None)
    } else {
        normalize_path(path).map(Some)
    }
}

fn relative_path(path: &str, root: &str) -> Result<String, String> {
    let path = normalize_path(path)?;
    let root = normalize_path(root)?;
    if !is_path_prefix(&root, &path) {
        return Err(format!("路径 {path} 不在根目录 {root} 下"));
    }
    Ok(if path == root {
        String::new()
    } else if root == "/" {
        path.trim_start_matches('/').to_string()
    } else {
        path[root.len()..].trim_start_matches('/').to_string()
    })
}

fn normalize_torrent_file_paths(files: Vec<TorrentFileInfo>) -> Result<Vec<String>, String> {
    Ok(normalize_torrent_manifest_inner(files, false)?
        .into_iter()
        .map(|file| file.path)
        .collect())
}

fn normalize_torrent_manifest(files: Vec<TorrentFileInfo>) -> Result<Vec<ManifestFile>, String> {
    normalize_torrent_manifest_inner(files, true)
}

pub(crate) fn validate_torrent_files_complete(files: &[TorrentFileInfo]) -> Result<(), String> {
    normalize_torrent_manifest(files.to_vec()).map(|_| ())
}

fn normalize_torrent_manifest_inner(
    files: Vec<TorrentFileInfo>,
    require_complete: bool,
) -> Result<Vec<ManifestFile>, String> {
    let mut normalized = std::collections::BTreeMap::new();
    let mut case_folded_paths = std::collections::BTreeMap::new();
    for file in files {
        let path = normalize_manifest_path(&file.path)?;
        if file.size < 0 {
            return Err(format!("种子文件大小无效: {path}={}", file.size));
        }
        if require_complete
            && (!file.progress.is_finite()
                || file.progress < 0.999_999
                || file.progress > 1.000_001
                || !file.is_seed)
        {
            return Err(format!(
                "种子文件未完整下载或被跳过: {path} (progress={:.6}, is_seed={})",
                file.progress, file.is_seed
            ));
        }
        if normalized.contains_key(&path) {
            return Err(format!("种子文件清单包含重复路径: {path}"));
        }
        let folded = openlist_identity_key(&path);
        if let Some(previous) = case_folded_paths.insert(folded, path.clone()) {
            return Err(format!(
                "种子文件清单包含仅大小写或 Unicode 规范形式不同的冲突路径: {previous} 与 {path}"
            ));
        }
        normalized.insert(path, file.size);
    }
    if normalized.is_empty() {
        return Err("种子文件清单为空".to_string());
    }
    Ok(normalized
        .into_iter()
        .map(|(path, size)| ManifestFile { path, size })
        .collect())
}

fn normalize_manifest_path(path: &str) -> Result<String, String> {
    let path = path.replace('\\', "/");
    if path.is_empty()
        || path.starts_with('/')
        || path.as_bytes().get(1) == Some(&b':')
        || path.contains('\0')
    {
        return Err(format!("种子文件路径无效: {path:?}"));
    }
    let mut parts = Vec::new();
    for part in path.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return Err(format!("种子文件路径无效: {path:?}"));
        }
        parts.push(part);
    }
    Ok(parts.join("/"))
}

fn torrent_top_level_items(files: &[String]) -> Result<Vec<String>, String> {
    let items = files
        .iter()
        .map(|path| path.split('/').next().unwrap_or_default().to_string())
        .collect::<std::collections::BTreeSet<_>>();
    if items.is_empty() || items.contains("") {
        return Err("种子顶级文件/文件夹列表为空".to_string());
    }
    Ok(items.into_iter().collect())
}

fn decode_string_list(value: &str, label: &str) -> Result<Vec<String>, String> {
    let values =
        serde_json::from_str::<Vec<String>>(value).map_err(|e| format!("解析{label}失败: {e}"))?;
    values
        .into_iter()
        .map(|value| normalize_manifest_path(&value))
        .collect()
}

fn decode_source_manifest(value: &str) -> Result<Option<Vec<ManifestFile>>, String> {
    let files = serde_json::from_str::<Vec<ManifestFile>>(value)
        .map_err(|error| format!("解析种子文件大小快照失败: {error}"))?;
    if files.is_empty() {
        return Ok(None);
    }
    let normalized = normalize_torrent_manifest(
        files
            .into_iter()
            .map(|file| TorrentFileInfo {
                path: file.path,
                size: file.size,
                progress: 1.0,
                is_seed: true,
            })
            .collect(),
    )?;
    Ok(Some(normalized))
}

fn encode_manifest_paths(manifest: &[ManifestFile]) -> Result<String, String> {
    serde_json::to_string(
        &manifest
            .iter()
            .map(|file| file.path.clone())
            .collect::<Vec<_>>(),
    )
    .map_err(|error| format!("序列化种子文件路径快照失败: {error}"))
}

fn manifest_from_torrent_data(data: &[u8]) -> Result<Vec<ManifestFile>, String> {
    let files = torrent_file_manifest(data)
        .map_err(|error| format!("解析导出的 torrent_data manifest 失败: {error}"))?;
    normalize_torrent_manifest(
        files
            .into_iter()
            .map(|file| TorrentFileInfo {
                path: file.path,
                size: file.size,
                progress: 1.0,
                is_seed: true,
            })
            .collect(),
    )
}

fn manifest_from_recovery_torrent_data(
    job: &MediaRelocationJob,
) -> Result<Vec<ManifestFile>, String> {
    let data = job
        .torrent_data
        .as_deref()
        .ok_or("qB 中已无种子且导出的 torrent_data 缺失")?;
    let recovered_infohash = torrent_infohash_for(data, &job.infohash)
        .map_err(|error| format!("解析恢复用 torrent_data infohash 失败: {error}"))?;
    if !recovered_infohash.eq_ignore_ascii_case(&job.infohash) {
        return Err(format!(
            "恢复用 torrent_data infohash 与迁移任务不一致: {recovered_infohash} != {}",
            job.infohash
        ));
    }
    manifest_from_torrent_data(data)
}

async fn recover_authoritative_manifest_for_recheck(
    source_qb: &dyn DownloaderClient,
    job: &MediaRelocationJob,
) -> Result<Vec<ManifestFile>, String> {
    if let Ok(Some(manifest)) = decode_source_manifest(&job.source_manifest_json) {
        validate_manifest_paths_snapshot(&manifest, &job.source_files_json)?;
        return Ok(manifest);
    }

    let torrents = source_qb
        .list_torrents_by_hashes(&[job.infohash.clone()])
        .await;
    let manifest = match torrents {
        Ok(torrents)
            if torrents
                .iter()
                .any(|torrent| torrent.hash.eq_ignore_ascii_case(&job.infohash)) =>
        {
            match source_qb.get_torrent_files(&job.infohash).await {
                Ok(files) => normalize_torrent_manifest(files)?,
                Err(qb_error) if job.torrent_data.is_some() => {
                    manifest_from_recovery_torrent_data(job).map_err(|torrent_error| {
                        format!(
                            "从 qB 获取 manifest 失败: {qb_error}; torrent_data 恢复也失败: {torrent_error}"
                        )
                    })?
                }
                Err(error) => return Err(format!("从 qB 获取权威 manifest 失败: {error}")),
            }
        }
        Ok(_) => manifest_from_recovery_torrent_data(job)?,
        Err(qb_error) if job.torrent_data.is_some() => {
            manifest_from_recovery_torrent_data(job).map_err(|torrent_error| {
                format!(
                    "查询源 qB 失败: {qb_error}; torrent_data 恢复也失败: {torrent_error}"
                )
            })?
        }
        Err(error) => return Err(format!("查询源 qB 以恢复权威 manifest 失败: {error}")),
    };
    validate_manifest_paths_snapshot(&manifest, &job.source_files_json)?;
    Ok(manifest)
}

fn manifest_recheck_resume_stage(job: &MediaRelocationJob) -> Result<&'static str, String> {
    let Some(value) = job.copy_checkpoint_json.as_deref() else {
        return Ok("copy_legacy_reconcile");
    };
    let checkpoint = decode_copy_checkpoint(value)?;
    if checkpoint.operation != CopyCheckpointOperation::RemoveFile
        || checkpoint.phase != CopyCheckpointPhase::Uncertain
    {
        return Err(
            "manifest 恢复任务包含非删除或非 uncertain 的副作用 checkpoint，已拒绝自动恢复"
                .to_string(),
        );
    }
    let (checkpoint_index, _) = validate_copy_checkpoint(&checkpoint, job)?;
    if checkpoint_index != job.manifest_cursor {
        return Err(format!(
            "源删除 checkpoint 与 manifest cursor 不一致: {checkpoint_index} != {}",
            job.manifest_cursor
        ));
    }
    Ok("source_removing")
}

fn validate_exported_torrent(job: &MediaRelocationJob, data: &[u8]) -> Result<(), String> {
    let exported_infohash = torrent_infohash_for(data, &job.infohash)
        .map_err(|error| format!("解析导出的 torrent infohash 失败: {error}"))?;
    if !exported_infohash.eq_ignore_ascii_case(&job.infohash) {
        return Err(format!(
            "导出的 torrent infohash 与迁移任务不一致: {exported_infohash} != {}",
            job.infohash
        ));
    }
    let exported_manifest = manifest_from_torrent_data(data)?;
    let expected_manifest = decode_source_manifest(&job.source_manifest_json)?
        .ok_or("迁移任务缺少带大小的权威源 manifest")?;
    if exported_manifest != expected_manifest {
        return Err("导出的 torrent 文件结构与迁移任务源 manifest 不一致".to_string());
    }
    Ok(())
}

fn validate_manifest_paths_snapshot(
    manifest: &[ManifestFile],
    paths_json: &str,
) -> Result<(), String> {
    let paths = decode_string_list(paths_json, "种子文件路径快照")?;
    if paths.is_empty()
        || paths
            == manifest
                .iter()
                .map(|file| file.path.clone())
                .collect::<Vec<_>>()
    {
        Ok(())
    } else {
        Err("带大小的权威 manifest 与原种子文件路径快照不一致".to_string())
    }
}

async fn verify_openlist_manifest(
    openlist: &OpenListClient,
    job: &MediaRelocationJob,
    manifest: &[ManifestFile],
    allow_missing_source: bool,
) -> Result<(), String> {
    if manifest.is_empty() {
        return Err("带大小的权威 manifest 为空".to_string());
    }
    for file in manifest {
        verify_openlist_manifest_file(openlist, job, file, allow_missing_source).await?;
    }
    Ok(())
}

async fn verify_openlist_manifest_file(
    openlist: &OpenListClient,
    job: &MediaRelocationJob,
    file: &ManifestFile,
    allow_missing_source: bool,
) -> Result<bool, String> {
    let expected_name = file.path.rsplit('/').next().unwrap_or_default();
    let source_path = join_path(&job.source_openlist_path, &file.path)?;
    let source_exists = match openlist.stat_if_exists(&source_path).await? {
        Some(source) => {
            validate_openlist_manifest_object(
                &source,
                expected_name,
                file.size,
                "源",
                &source_path,
            )?;
            true
        }
        None if allow_missing_source => false,
        None => return Err(format!("OpenList 源 manifest 文件不存在: {source_path}")),
    };
    let target_path = join_path(&job.target_openlist_path, &file.path)?;
    let target = openlist
        .stat_if_exists(&target_path)
        .await?
        .ok_or_else(|| format!("OpenList 目标 manifest 文件不存在: {target_path}"))?;
    validate_openlist_manifest_object(&target, expected_name, file.size, "目标", &target_path)?;
    Ok(source_exists)
}

fn validate_openlist_manifest_object(
    object: &crate::openlist::OpenListObject,
    expected_name: &str,
    expected_size: i64,
    side: &str,
    path: &str,
) -> Result<(), String> {
    if object.is_dir {
        return Err(format!(
            "OpenList {side} manifest 路径是目录而非文件: {path}"
        ));
    }
    if openlist_canonical_key(&object.name) != openlist_canonical_key(expected_name) {
        return Err(format!(
            "OpenList {side} manifest 文件名不匹配: {:?} != {:?}",
            object.name, expected_name
        ));
    }
    if object.size != expected_size {
        return Err(format!(
            "OpenList {side} manifest 文件大小不匹配: {path} ({} != {expected_size})",
            object.size
        ));
    }
    Ok(())
}

fn advanced_stage_requires_manifest(stage: &str) -> bool {
    matches!(
        stage,
        "copy_verified"
            | "qb_reconcile"
            | "torrent_exported"
            | "source_qb_removed"
            | "target_qb_submitted"
            | "target_qb_check_requested"
            | "target_qb_checking"
            | "target_qb_starting"
            | "source_removing"
    )
}

fn validate_distinct_relocation_paths(
    job: &MediaRelocationJob,
    manual_migration: bool,
) -> Result<(), String> {
    validate_distinct_path_pairs_for_mode(
        &job.source_openlist_path,
        &job.target_openlist_path,
        &job.source_qb_path,
        &job.target_qb_path,
        manual_migration,
    )
}

#[cfg(test)]
fn validate_distinct_path_pairs(
    source_openlist: &str,
    target_openlist: &str,
    source_qb: &str,
    target_qb: &str,
) -> Result<(), String> {
    validate_distinct_path_pairs_for_mode(
        source_openlist,
        target_openlist,
        source_qb,
        target_qb,
        true,
    )
}

fn validate_distinct_path_pairs_for_mode(
    source_openlist: &str,
    target_openlist: &str,
    source_qb: &str,
    target_qb: &str,
    manual_migration: bool,
) -> Result<(), String> {
    let source_openlist = normalize_path(source_openlist)?;
    let target_openlist = normalize_path(target_openlist)?;
    let source_openlist_key = openlist_identity_key(&source_openlist);
    let target_openlist_key = openlist_identity_key(&target_openlist);
    if is_path_prefix(&source_openlist_key, &target_openlist_key)
        || is_path_prefix(&target_openlist_key, &source_openlist_key)
    {
        return Err(format!(
            "源与目标 OpenList 路径重叠，已拒绝迁移: {source_openlist} <-> {target_openlist}"
        ));
    }
    if manual_migration {
        let source_qb = normalize_path(source_qb)?;
        let target_qb = normalize_path(target_qb)?;
        if is_path_prefix(&source_qb, &target_qb) || is_path_prefix(&target_qb, &source_qb) {
            return Err(format!(
                "源与目标 qB 路径重叠，已拒绝迁移: {source_qb} <-> {target_qb}"
            ));
        }
    }
    Ok(())
}

fn manifest_scan_end(cursor: usize, manifest_len: usize) -> Result<usize, String> {
    if cursor > manifest_len {
        return Err(format!("manifest cursor 越界: {cursor} > {manifest_len}"));
    }
    Ok(cursor
        .saturating_add(MANIFEST_FILES_PER_PASS)
        .min(manifest_len))
}

fn decide_copy_tasks(observations: &[CopyTaskObservation]) -> Result<CopyTaskDecision, String> {
    if observations.is_empty() {
        return Err("OpenList 复制任务观察结果为空".to_string());
    }
    if observations.contains(&CopyTaskObservation::Missing) {
        return Ok(CopyTaskDecision::Uncertain);
    }
    if observations.contains(&CopyTaskObservation::Pending) {
        return Ok(CopyTaskDecision::Wait);
    }
    if observations
        .iter()
        .all(|state| *state == CopyTaskObservation::Failed)
    {
        return Ok(CopyTaskDecision::AllFailed);
    }
    Ok(CopyTaskDecision::VerifyTarget)
}

fn prepare_read_only_copy_reconcile(
    checkpoint_json: &mut Option<String>,
    now: chrono::DateTime<Utc>,
) -> Result<&'static str, String> {
    let Some(value) = checkpoint_json.as_deref() else {
        return Ok("copy_legacy_reconcile");
    };
    let mut checkpoint = decode_copy_checkpoint(value)?;
    checkpoint.phase = CopyCheckpointPhase::Uncertain;
    checkpoint
        .submitted_at
        .get_or_insert_with(|| now.to_rfc3339());
    *checkpoint_json = Some(encode_copy_checkpoint(&checkpoint)?);
    Ok("copy_submitting")
}

fn checkpoint_pending_timed_out(
    checkpoint_json: Option<&str>,
    now: chrono::DateTime<Utc>,
) -> Result<bool, String> {
    let Some(value) = checkpoint_json else {
        return Ok(true);
    };
    let checkpoint = decode_copy_checkpoint(value)?;
    let Some(submitted_at) = checkpoint.submitted_at.as_deref() else {
        return Ok(false);
    };
    let submitted_at = chrono::DateTime::parse_from_rfc3339(submitted_at)
        .map_err(|error| format!("解析复制 checkpoint 提交时间失败: {error}"))?
        .with_timezone(&Utc);
    Ok(now.signed_duration_since(submitted_at)
        >= ChronoDuration::seconds(COPY_TASK_PENDING_MANUAL_SECONDS))
}

fn set_review_existing_checkpoint(
    job: &mut MediaRelocationJob,
    manifest_index: usize,
    file: &ManifestFile,
    error: String,
) -> Result<(), String> {
    job.manifest_cursor = manifest_index;
    job.copy_checkpoint_json = Some(encode_review_existing_checkpoint(file)?);
    job.stage = "copy_manual_review".to_string();
    job.next_attempt_at = None;
    job.last_error = Some(error);
    Ok(())
}

fn manifest_presence_is_verified(hash_verified: bool, manual_migration: bool) -> bool {
    hash_verified || manual_migration
}

fn review_existing_observation_is_verified(
    observed: &ManifestFileState,
    manual_migration: bool,
) -> bool {
    matches!(
        observed,
        ManifestFileState::Present { hash_verified }
            if manifest_presence_is_verified(*hash_verified, manual_migration)
    )
}

fn encode_review_existing_checkpoint(file: &ManifestFile) -> Result<String, String> {
    encode_copy_checkpoint(&CopyCheckpoint {
        path: file.path.clone(),
        size: file.size,
        operation: CopyCheckpointOperation::ReviewExisting,
        phase: CopyCheckpointPhase::Prepared,
        submitted_at: None,
        terminal_failure_verified: false,
    })
}

fn encode_copy_checkpoint(checkpoint: &CopyCheckpoint) -> Result<String, String> {
    serde_json::to_string(checkpoint)
        .map_err(|error| format!("序列化 OpenList 复制 checkpoint 失败: {error}"))
}

fn decode_copy_checkpoint(value: &str) -> Result<CopyCheckpoint, String> {
    let mut checkpoint = serde_json::from_str::<CopyCheckpoint>(value)
        .map_err(|error| format!("解析 OpenList 复制 checkpoint 失败: {error}"))?;
    match checkpoint.operation {
        CopyCheckpointOperation::CopyFile
        | CopyCheckpointOperation::ReviewExisting
        | CopyCheckpointOperation::RemoveFile => {
            checkpoint.path = normalize_manifest_path(&checkpoint.path)?;
            if checkpoint.size < 0 {
                return Err(format!(
                    "OpenList 复制 checkpoint 文件大小无效: {}={}",
                    checkpoint.path, checkpoint.size
                ));
            }
        }
        CopyCheckpointOperation::CreateDirectory => {
            checkpoint.path = normalize_path(&checkpoint.path)?;
            if !checkpoint.path.starts_with('/') || checkpoint.path == "/" || checkpoint.size != 0 {
                return Err(format!(
                    "OpenList 目录 checkpoint 无效: {}={}",
                    checkpoint.path, checkpoint.size
                ));
            }
        }
    }
    Ok(checkpoint)
}

fn validate_copy_checkpoint(
    checkpoint: &CopyCheckpoint,
    job: &MediaRelocationJob,
) -> Result<(usize, ManifestFile), String> {
    let manifest = decode_source_manifest(&job.source_manifest_json)?
        .ok_or("种子文件大小快照缺失，无法校验复制 checkpoint")?;
    match checkpoint.operation {
        CopyCheckpointOperation::CopyFile
        | CopyCheckpointOperation::ReviewExisting
        | CopyCheckpointOperation::RemoveFile => {
            let index =
                validate_checkpoint_against_manifest(checkpoint, &job.source_manifest_json)?;
            Ok((index, manifest[index].clone()))
        }
        CopyCheckpointOperation::CreateDirectory => {
            let file = manifest
                .get(job.manifest_cursor)
                .cloned()
                .ok_or("目录 checkpoint 对应的 manifest cursor 越界")?;
            let target_file = join_path(&job.target_openlist_path, &file.path)?;
            let target_parent = target_file
                .rsplit_once('/')
                .map(|(parent, _)| if parent.is_empty() { "/" } else { parent })
                .ok_or("目录 checkpoint 对应的目标文件路径无效")?;
            let directory = normalize_path(&checkpoint.path)?;
            let directory_identity = openlist_identity_key(&directory);
            let target_parent_identity = openlist_identity_key(target_parent);
            if !is_path_prefix(&directory_identity, &target_parent_identity) {
                return Err(format!(
                    "目录 checkpoint 不在当前目标文件的父目录链中: {directory}"
                ));
            }
            Ok((job.manifest_cursor, file))
        }
    }
}

fn validate_checkpoint_against_manifest(
    checkpoint: &CopyCheckpoint,
    manifest_json: &str,
) -> Result<usize, String> {
    let manifest = decode_source_manifest(manifest_json)?
        .ok_or("种子文件大小快照缺失，无法校验复制 checkpoint")?;
    manifest
        .iter()
        .position(|file| file.path == checkpoint.path && file.size == checkpoint.size)
        .ok_or_else(|| {
            format!(
                "OpenList 复制 checkpoint 不在权威种子清单中: {}",
                checkpoint.path
            )
        })
}

fn copy_submission_attention_at(
    checkpoint: &CopyCheckpoint,
) -> Result<chrono::DateTime<Utc>, String> {
    let submitted_at = checkpoint
        .submitted_at
        .as_deref()
        .ok_or("uncertain 复制 checkpoint 缺少提交时间")?;
    let submitted_at = chrono::DateTime::parse_from_rfc3339(submitted_at)
        .map_err(|error| format!("解析复制 checkpoint 提交时间失败: {error}"))?
        .with_timezone(&Utc);
    Ok(submitted_at + ChronoDuration::seconds(COPY_SUBMISSION_ATTENTION_SECONDS))
}

fn uncertain_submission_next_check(
    checkpoint: &CopyCheckpoint,
    now: chrono::DateTime<Utc>,
) -> Result<Option<chrono::DateTime<Utc>>, String> {
    let attention_at = copy_submission_attention_at(checkpoint)?;
    if attention_at <= now {
        Ok(None)
    } else {
        Ok(Some(std::cmp::min(
            attention_at,
            now + ChronoDuration::seconds(30),
        )))
    }
}

fn uncertain_removal_next_check(
    checkpoint: &CopyCheckpoint,
    now: chrono::DateTime<Utc>,
) -> Result<Option<chrono::DateTime<Utc>>, String> {
    if checkpoint.operation != CopyCheckpointOperation::RemoveFile
        || checkpoint.phase != CopyCheckpointPhase::Uncertain
    {
        return Err("源文件删除恢复要求 uncertain remove_file checkpoint".to_string());
    }
    let submitted_at = checkpoint
        .submitted_at
        .as_deref()
        .ok_or("uncertain 删除 checkpoint 缺少提交时间")?;
    let submitted_at = chrono::DateTime::parse_from_rfc3339(submitted_at)
        .map_err(|error| format!("解析删除 checkpoint 提交时间失败: {error}"))?
        .with_timezone(&Utc);
    let attention_at = submitted_at + ChronoDuration::seconds(SOURCE_REMOVAL_ATTENTION_SECONDS);
    if attention_at <= now {
        Ok(None)
    } else {
        Ok(Some(std::cmp::min(
            attention_at,
            now + ChronoDuration::seconds(30),
        )))
    }
}

fn valid_infohash(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
