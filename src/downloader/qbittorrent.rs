use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use qbit_rs::Qbit;
use qbit_rs::model::{AddTorrentArg, TorrentFile, TorrentSource};
use reqwest::{Client, StatusCode, header::COOKIE};
use serde::Deserialize;
use tracing::debug;

use super::{
    AddTorrentOptions, DownloaderClient, DownloaderTestResult, TorrentFileInfo, TorrentInfo,
};

pub struct QBittorrentClient {
    qb: Qbit,
    http: Client,
    endpoint: String,
}

#[derive(Debug, Deserialize)]
struct TolerantTorrent {
    hash: Option<String>,
    name: Option<String>,
    size: Option<i64>,
    uploaded: Option<i64>,
    downloaded: Option<i64>,
    amount_left: Option<i64>,
    completed: Option<i64>,
    progress: Option<f64>,
    upspeed: Option<i64>,
    dlspeed: Option<i64>,
    ratio: Option<f64>,
    state: Option<String>,
    added_on: Option<i64>,
    completion_on: Option<i64>,
    num_seeds: Option<i64>,
    num_leechs: Option<i64>,
    save_path: Option<String>,
    root_path: Option<String>,
    content_path: Option<String>,
    tags: Option<String>,
    category: Option<String>,
    time_active: Option<i64>,
    last_activity: Option<i64>,
    tracker: Option<String>,
}

fn valid_qbittorrent_hash(hash: &str) -> bool {
    matches!(hash.len(), 40 | 64) && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
}

impl QBittorrentClient {
    pub fn new(
        base_url: String,
        username: String,
        password: String,
        proxy: Option<&str>,
    ) -> Result<Self, String> {
        let mut builder = Client::builder()
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none());
        if let Some(proxy_url) = proxy.map(str::trim).filter(|v| !v.is_empty()) {
            let proxy = reqwest::Proxy::all(proxy_url)
                .map_err(|e| format!("无效的代理地址 '{}': {}", proxy_url, e))?;
            builder = builder.proxy(proxy);
        }
        let client = builder
            .build()
            .map_err(|e| format!("构建 qBittorrent HTTP 客户端失败: {}", e))?;

        let endpoint = base_url.trim_end_matches('/');
        let credential = qbit_rs::model::Credential::new(username, password);
        let qb = Qbit::new_with_client(endpoint, credential, client.clone());

        Ok(Self {
            qb,
            http: client,
            endpoint: endpoint.to_string(),
        })
    }

    async fn list_torrents_tolerant_raw(
        &self,
        tag: Option<&str>,
        hashes: Option<&str>,
    ) -> Result<Vec<TolerantTorrent>, String> {
        let mut query = Vec::new();
        if let Some(tag) = tag {
            query.push(("tag", tag));
        }
        if let Some(hashes) = hashes {
            query.push(("hashes", hashes));
        }

        for attempt in 0..2 {
            self.qb
                .login(attempt > 0)
                .await
                .map_err(|error| format!("登录 qBittorrent 失败: {error}"))?;
            let cookie = self
                .qb
                .get_cookie()
                .await
                .ok_or("qBittorrent 登录成功但未返回会话 cookie")?;
            let response = self
                .http
                .get(format!("{}/api/v2/torrents/info", self.endpoint))
                .header(COOKIE, cookie)
                .query(&query)
                .send()
                .await
                .map_err(|error| format!("获取种子列表失败: {error}"))?;
            if response.status() == StatusCode::FORBIDDEN && attempt == 0 {
                continue;
            }
            let status = response.status();
            if !status.is_success() {
                let body = response.text().await.unwrap_or_default();
                return Err(format!(
                    "获取种子列表失败: HTTP {status}{}",
                    if body.trim().is_empty() {
                        String::new()
                    } else {
                        format!(" ({})", body.trim())
                    }
                ));
            }
            return response
                .json::<Vec<TolerantTorrent>>()
                .await
                .map_err(|error| format!("解析种子列表失败: {error}"));
        }

        Err("获取种子列表失败: qBittorrent 会话已失效".to_string())
    }

    async fn list_torrents_tolerant(
        &self,
        tag: Option<&str>,
        hashes: Option<&str>,
    ) -> Result<Vec<TorrentInfo>, String> {
        self.list_torrents_tolerant_raw(tag, hashes)
            .await
            .map(|torrents| torrents.into_iter().map(TorrentInfo::from).collect())
    }
}

impl DownloaderClient for QBittorrentClient {
    fn test_connection(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<DownloaderTestResult, String>> + Send + '_>> {
        Box::pin(async move {
            self.qb
                .login(false)
                .await
                .map_err(|e| format!("登录失败: {}", e))?;
            let version = self
                .qb
                .get_version()
                .await
                .map_err(|e| format!("获取版本失败: {}", e))?;
            let free_space = self.get_free_space(None).await.ok();
            Ok(DownloaderTestResult {
                success: true,
                message: format!("连接成功，版本: {}", version.trim()),
                version: Some(version.trim().to_string()),
                free_space,
            })
        })
    }

    fn add_torrent(
        &self,
        torrent_data: Vec<u8>,
        filename: &str,
        options: &AddTorrentOptions,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>> {
        let filename = filename.to_string();
        let options = options.clone();
        Box::pin(async move {
            debug!(
                "qBittorrent add_torrent: filename={} size={} save_path={:?} tags={:?} category={:?} paused={}",
                filename,
                torrent_data.len(),
                options.save_path,
                options.tags,
                options.category,
                options.paused,
            );

            let arg = AddTorrentArg {
                source: TorrentSource::TorrentFiles {
                    torrents: vec![TorrentFile {
                        filename,
                        data: torrent_data,
                    }],
                },
                savepath: options.save_path,
                tags: options.tags,
                category: options.category,
                download_limit: options.download_limit,
                up_limit: options.upload_limit,
                ratio_limit: options.ratio_limit,
                seeding_time_limit: options.inactive_seeding_time_limit,
                paused: if options.paused {
                    Some("true".to_string())
                } else {
                    None
                },
                skip_checking: if options.skip_checking {
                    Some("true".to_string())
                } else {
                    None
                },
                root_folder: options.root_folder.map(|value| value.to_string()),
                ..Default::default()
            };

            self.qb
                .add_torrent(arg)
                .await
                .map_err(|e| format!("添加种子失败: {}", e))?;
            Ok(())
        })
    }

    fn list_torrents(
        &self,
        tag: Option<&str>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<TorrentInfo>, String>> + Send + '_>> {
        let tag = tag.map(|t| t.to_string());
        Box::pin(async move { self.list_torrents_tolerant(tag.as_deref(), None).await })
    }

    fn list_torrent_tracker_urls(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<Vec<String>>, String>> + Send + '_>> {
        Box::pin(async move {
            self.list_torrents_tolerant_raw(None, None)
                .await
                .map(|torrents| {
                    torrents
                        .into_iter()
                        .map(|torrent| {
                            torrent
                                .tracker
                                .filter(|url| !url.trim().is_empty())
                                .into_iter()
                                .collect()
                        })
                        .collect()
                })
        })
    }

    fn list_torrents_by_hashes<'a>(
        &'a self,
        hashes: &'a [String],
    ) -> Pin<Box<dyn Future<Output = Result<Vec<TorrentInfo>, String>> + Send + 'a>> {
        Box::pin(async move {
            if hashes.is_empty() {
                return Ok(Vec::new());
            }
            let mut requested = Vec::with_capacity(hashes.len());
            let mut requested_set = HashSet::with_capacity(hashes.len());
            for hash in hashes {
                if !valid_qbittorrent_hash(hash) {
                    return Err(format!("qBittorrent 种子 hash 格式无效: {hash:?}"));
                }
                let normalized = hash.to_ascii_lowercase();
                if requested_set.insert(normalized.clone()) {
                    requested.push(normalized);
                }
            }

            let joined_hashes = requested.join("|");
            let torrents = self
                .list_torrents_tolerant(None, Some(&joined_hashes))
                .await?;
            let mut returned = HashSet::with_capacity(torrents.len());
            Ok(torrents
                .into_iter()
                .filter(|torrent| {
                    let normalized = torrent.hash.to_ascii_lowercase();
                    valid_qbittorrent_hash(&normalized)
                        && requested_set.contains(&normalized)
                        && returned.insert(normalized)
                })
                .collect())
        })
    }

    fn pause_torrent(
        &self,
        hash: &str,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>> {
        let hash = hash.to_string();
        Box::pin(async move {
            self.qb
                .stop_torrents(vec![hash])
                .await
                .map_err(|e| format!("暂停种子失败: {}", e))?;
            Ok(())
        })
    }

    fn delete_torrent(
        &self,
        hash: &str,
        delete_files: bool,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>> {
        let hash = hash.to_string();
        Box::pin(async move {
            self.qb
                .delete_torrents(vec![hash], delete_files)
                .await
                .map_err(|e| format!("删除种子失败: {}", e))?;
            Ok(())
        })
    }

    fn get_free_space(
        &self,
        _path: Option<&str>,
    ) -> Pin<Box<dyn Future<Output = Result<u64, String>> + Send + '_>> {
        Box::pin(async move {
            let sync_data = self
                .qb
                .sync(None)
                .await
                .map_err(|e| format!("获取同步数据失败: {}", e))?;

            let free = sync_data
                .server_state
                .as_ref()
                .and_then(|s| s.get("free_space_on_disk"))
                .and_then(|v| {
                    // serde_value::Value -> u64
                    serde_json::to_value(v).ok()?.as_u64()
                })
                .ok_or_else(|| "下载器尚未报告有效的剩余空间".to_string())?;

            Ok(free)
        })
    }

    fn get_default_save_path(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + '_>> {
        Box::pin(async move {
            let path = self
                .qb
                .get_default_save_path()
                .await
                .map_err(|e| format!("获取默认保存路径失败: {}", e))?;
            Ok(path.to_string_lossy().to_string())
        })
    }

    fn get_torrent_trackers(
        &self,
        hash: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, String>> + Send + '_>> {
        let hash = hash.to_string();
        Box::pin(async move {
            let trackers = self
                .qb
                .get_torrent_trackers(&hash)
                .await
                .map_err(|e| format!("获取种子tracker失败: {}", e))?;
            Ok(trackers.into_iter().map(|t| t.url).collect())
        })
    }

    fn add_torrent_tags(
        &self,
        hashes: Vec<String>,
        tags: Vec<String>,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>> {
        Box::pin(async move {
            self.qb
                .add_torrent_tags(hashes, tags)
                .await
                .map_err(|e| format!("添加标签失败: {}", e))?;
            Ok(())
        })
    }

    fn export_torrent(
        &self,
        hash: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, String>> + Send + '_>> {
        let hash = hash.to_string();
        Box::pin(async move {
            self.qb
                .export_torrent(hash)
                .await
                .map(|bytes| bytes.to_vec())
                .map_err(|e| format!("导出种子失败: {}", e))
        })
    }

    fn start_torrent(
        &self,
        hash: &str,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>> {
        let hash = hash.to_string();
        Box::pin(async move {
            self.qb
                .start_torrents(vec![hash])
                .await
                .map_err(|e| format!("启动种子失败: {}", e))
        })
    }

    fn recheck_torrent(
        &self,
        hash: &str,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>> {
        let hash = hash.to_string();
        Box::pin(async move {
            self.qb
                .recheck_torrents(vec![hash])
                .await
                .map_err(|e| format!("重新校验种子失败: {}", e))
        })
    }

    fn get_torrent_files(
        &self,
        hash: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<TorrentFileInfo>, String>> + Send + '_>> {
        let hash = hash.to_string();
        Box::pin(async move {
            self.qb
                .get_torrent_contents(&hash, None)
                .await
                .map(|files| files.into_iter().map(TorrentFileInfo::from).collect())
                .map_err(|e| format!("获取种子文件清单失败: {e}"))
        })
    }
}

impl From<qbit_rs::model::TorrentContent> for TorrentFileInfo {
    fn from(file: qbit_rs::model::TorrentContent) -> Self {
        let progress = file.progress;
        let skipped = file.priority == qbit_rs::model::Priority::DoNotDownload;
        Self {
            path: file.name,
            size: file.size.min(i64::MAX as u64) as i64,
            progress,
            is_seed: !skipped
                && file
                    .is_seed
                    .unwrap_or(progress.is_finite() && progress >= 0.999_999),
        }
    }
}

impl From<qbit_rs::model::Torrent> for TorrentInfo {
    fn from(t: qbit_rs::model::Torrent) -> Self {
        TorrentInfo {
            hash: t.hash.unwrap_or_default(),
            name: t.name.unwrap_or_default(),
            size: t.size.unwrap_or(0),
            uploaded: t.uploaded.unwrap_or(0),
            downloaded: t.downloaded.unwrap_or(0),
            amount_left: t.amount_left,
            completed: t.completed,
            progress: t.progress.unwrap_or(0.0),
            upload_speed: t.upspeed.unwrap_or(0),
            download_speed: t.dlspeed.unwrap_or(0),
            ratio: t.ratio.unwrap_or(0.0),
            state: t
                .state
                .and_then(|s| serde_json::to_value(&s).ok())
                .and_then(|v| v.as_str().map(|s| s.to_string()))
                .unwrap_or_default(),
            added_on: t.added_on.unwrap_or(0),
            completion_on: t.completion_on.unwrap_or(0),
            num_seeds: t.num_seeds.unwrap_or(0) as i32,
            num_leechs: t.num_leechs.unwrap_or(0) as i32,
            save_path: t.save_path.unwrap_or_default(),
            root_path: t.root_path.unwrap_or_default(),
            content_path: t.content_path.unwrap_or_default(),
            tags: t.tags.unwrap_or_default(),
            category: t.category.unwrap_or_default(),
            time_active: t.time_active.unwrap_or(0),
            last_activity: t.last_activity.unwrap_or(0),
        }
    }
}

impl From<TolerantTorrent> for TorrentInfo {
    fn from(t: TolerantTorrent) -> Self {
        Self {
            hash: t.hash.unwrap_or_default(),
            name: t.name.unwrap_or_default(),
            size: t.size.unwrap_or(0),
            uploaded: t.uploaded.unwrap_or(0),
            downloaded: t.downloaded.unwrap_or(0),
            amount_left: t.amount_left,
            completed: t.completed,
            progress: t.progress.unwrap_or(0.0),
            upload_speed: t.upspeed.unwrap_or(0),
            download_speed: t.dlspeed.unwrap_or(0),
            ratio: t.ratio.unwrap_or(0.0),
            state: t.state.unwrap_or_default(),
            added_on: t.added_on.unwrap_or(0),
            completion_on: t.completion_on.unwrap_or(0),
            num_seeds: t
                .num_seeds
                .unwrap_or(0)
                .clamp(i32::MIN as i64, i32::MAX as i64) as i32,
            num_leechs: t
                .num_leechs
                .unwrap_or(0)
                .clamp(i32::MIN as i64, i32::MAX as i64) as i32,
            save_path: t.save_path.unwrap_or_default(),
            root_path: t.root_path.unwrap_or_default(),
            content_path: t.content_path.unwrap_or_default(),
            tags: t.tags.unwrap_or_default(),
            category: t.category.unwrap_or_default(),
            time_active: t.time_active.unwrap_or(0),
            last_activity: t.last_activity.unwrap_or(0),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use axum::extract::{Query, State};
    use axum::http::{HeaderMap, HeaderValue, StatusCode, header::SET_COOKIE};
    use axum::response::{IntoResponse, Response};
    use axum::{Json, Router, routing::get, routing::post};

    use super::{QBittorrentClient, TolerantTorrent, TorrentFileInfo, TorrentInfo};
    use crate::downloader::DownloaderClient;

    async fn fake_qb_login() -> Response {
        let mut response = "Ok.".into_response();
        response.headers_mut().insert(
            SET_COOKIE,
            HeaderValue::from_static("SID=test-session; HttpOnly; path=/"),
        );
        response
    }

    async fn fake_qb_torrent_list(
        headers: HeaderMap,
        Query(query): Query<HashMap<String, String>>,
    ) -> Json<serde_json::Value> {
        assert!(
            headers
                .get("cookie")
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value.contains("SID=test-session"))
        );
        assert_eq!(
            query.get("hashes").map(String::as_str),
            Some("eadb91a4769b1fad89e0dd3a930523e7fc5814b8")
        );
        Json(serde_json::json!([{
            "hash": "eadb91a4769b1fad89e0dd3a930523e7fc5814b8",
            "state": "queuedForChecking",
            "progress": 1.0
        }]))
    }

    async fn fake_qb_untrusted_hash_list(
        Query(query): Query<HashMap<String, String>>,
    ) -> Json<serde_json::Value> {
        assert_eq!(
            query.get("hashes").map(String::as_str),
            Some(concat!(
                "eadb91a4769b1fad89e0dd3a930523e7fc5814b8|",
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
            ))
        );
        Json(serde_json::json!([
            {
                "hash": "EADB91A4769B1FAD89E0DD3A930523E7FC5814B8",
                "state": "stalledUP"
            },
            {
                "hash": "eadb91a4769b1fad89e0dd3a930523e7fc5814b8",
                "state": "pausedUP"
            },
            {
                "hash": "0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF",
                "state": "stalledUP"
            },
            {
                "hash": "ffffffffffffffffffffffffffffffffffffffff",
                "state": "stalledUP"
            },
            {
                "hash": "not-a-valid-hash",
                "state": "stalledUP"
            }
        ]))
    }

    async fn fake_qb_tracker_list(headers: HeaderMap) -> Json<serde_json::Value> {
        assert!(
            headers
                .get("cookie")
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value.contains("SID=test-session"))
        );
        Json(serde_json::json!([
            {
                "hash": "eadb91a4769b1fad89e0dd3a930523e7fc5814b8",
                "tracker": "https://tracker.example/announce?passkey=dummy-secret"
            },
            {
                "hash": "0123456789abcdef0123456789abcdef01234567",
                "tracker": ""
            }
        ]))
    }

    #[derive(Default)]
    struct ReauthState {
        login_calls: usize,
        list_calls: usize,
    }

    type SharedReauthState = Arc<Mutex<ReauthState>>;

    async fn fake_qb_reauth_login(State(state): State<SharedReauthState>) -> Response {
        let login_calls = {
            let mut state = state.lock().unwrap();
            state.login_calls += 1;
            state.login_calls
        };
        let mut response = "Ok.".into_response();
        response.headers_mut().insert(
            SET_COOKIE,
            HeaderValue::from_str(&format!("SID=session-{login_calls}; HttpOnly; path=/")).unwrap(),
        );
        response
    }

    async fn fake_qb_reauth_torrent_list(
        State(state): State<SharedReauthState>,
        headers: HeaderMap,
    ) -> Response {
        let list_calls = {
            let mut state = state.lock().unwrap();
            state.list_calls += 1;
            state.list_calls
        };
        if list_calls == 1 {
            return StatusCode::FORBIDDEN.into_response();
        }
        assert!(
            headers
                .get("cookie")
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value.contains("SID=session-2"))
        );
        Json(serde_json::json!([])).into_response()
    }

    #[test]
    fn torrent_file_completeness_is_mapped_from_qbittorrent() {
        let file = serde_json::from_value::<qbit_rs::model::TorrentContent>(serde_json::json!({
            "index": 0,
            "name": "Show/E01.mkv",
            "size": 1024,
            "progress": 0.75,
            "priority": 1,
            "is_seed": true
        }))
        .unwrap();

        assert_eq!(
            TorrentFileInfo::from(file),
            TorrentFileInfo {
                path: "Show/E01.mkv".to_string(),
                size: 1024,
                progress: 0.75,
                is_seed: true,
            }
        );
    }

    #[test]
    fn missing_seed_flag_uses_complete_progress_for_older_qbittorrent() {
        let file = serde_json::from_value::<qbit_rs::model::TorrentContent>(serde_json::json!({
            "index": 0,
            "name": "Show/E01.mkv",
            "size": 1024,
            "progress": 1.0,
            "priority": 1
        }))
        .unwrap();

        assert!(TorrentFileInfo::from(file).is_seed);
    }

    #[test]
    fn skipped_torrent_file_is_never_treated_as_complete() {
        let file = serde_json::from_value::<qbit_rs::model::TorrentContent>(serde_json::json!({
            "index": 0,
            "name": "Show/Extras.mkv",
            "size": 1024,
            "progress": 1.0,
            "priority": 0,
            "is_seed": true
        }))
        .unwrap();

        assert!(!TorrentFileInfo::from(file).is_seed);
    }

    #[test]
    fn torrent_progress_is_mapped_from_qbittorrent() {
        let torrent = serde_json::from_value::<qbit_rs::model::Torrent>(serde_json::json!({
            "progress": 0.625
        }))
        .unwrap();

        assert_eq!(TorrentInfo::from(torrent).progress, 0.625);
    }

    #[test]
    fn remaining_space_uses_current_files_not_lifetime_download_traffic() {
        for (payload, pending, incomplete, completed) in [
            (serde_json::json!({"size": 1000, "downloaded": 0, "amount_left": 0, "progress": 1.0}), 0, false, 1000),
            (serde_json::json!({"size": 1000, "downloaded": 2000, "amount_left": 600, "progress": 0.4, "completion_on": 12345}), 600, true, 400),
            (serde_json::json!({"size": 200, "downloaded": 1000, "completed": 50, "progress": 0.25}), 150, true, 50),
            (serde_json::json!({"size": 1000, "downloaded": 0, "progress": 1.0}), 0, false, 1000),
            (serde_json::json!({"size": 1000, "downloaded": 2000, "progress": 0.4}), 600, true, 400),
            (serde_json::json!({"size": 1000, "amount_left": -1, "completed": -1, "progress": 0.4}), 600, true, 400),
            (serde_json::json!({"size": 0, "amount_left": 0, "progress": 0.0, "state": "metaDL"}), 0, true, 0),
        ] {
            let torrent = TorrentInfo::from(serde_json::from_value::<TolerantTorrent>(payload.clone()).unwrap());
            assert_eq!(torrent.pending_download_bytes(), pending, "{payload}");
            assert_eq!(torrent.is_download_incomplete(), incomplete, "{payload}");
            assert_eq!(torrent.completed_bytes(), completed, "{payload}");
            assert_eq!(super::super::calculate_pending_download_bytes(&[torrent]), pending);
            let typed = TorrentInfo::from(serde_json::from_value::<qbit_rs::model::Torrent>(payload.clone()).unwrap());
            assert_eq!(typed.pending_download_bytes(), pending, "typed: {payload}");
            assert_eq!(typed.is_download_incomplete(), incomplete, "typed: {payload}");
        }
    }

    #[test]
    fn missing_torrent_progress_defaults_to_zero() {
        let torrent =
            serde_json::from_value::<qbit_rs::model::Torrent>(serde_json::json!({})).unwrap();

        assert_eq!(TorrentInfo::from(torrent).progress, 0.0);
    }

    #[test]
    fn unknown_qbittorrent_states_are_preserved_in_tolerant_list_model() {
        for state in ["queuedForChecking", "futureStateAddedByQbittorrent"] {
            let torrent = serde_json::from_value::<TolerantTorrent>(serde_json::json!({
                "hash": "eadb91a4769b1fad89e0dd3a930523e7fc5814b8",
                "state": state,
                "progress": 1.0
            }))
            .unwrap();
            let torrent = TorrentInfo::from(torrent);
            assert_eq!(torrent.state, state);
            assert_eq!(torrent.progress, 1.0);
        }
    }

    #[tokio::test]
    async fn tolerant_torrent_list_accepts_new_qbittorrent_state_over_http() {
        let app = Router::new()
            .route("/api/v2/auth/login", post(fake_qb_login))
            .route("/api/v2/torrents/info", get(fake_qb_torrent_list));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = QBittorrentClient::new(
            format!("http://{address}"),
            "user".to_string(),
            "password".to_string(),
            None,
        )
        .unwrap();

        let torrents = client
            .list_torrents_tolerant(None, Some("eadb91a4769b1fad89e0dd3a930523e7fc5814b8"))
            .await
            .unwrap();
        server.abort();

        assert_eq!(torrents.len(), 1);
        assert_eq!(torrents[0].state, "queuedForChecking");
    }

    #[tokio::test]
    async fn tolerant_torrent_list_reauthenticates_once_after_forbidden() {
        let state = Arc::new(Mutex::new(ReauthState::default()));
        let app = Router::new()
            .route("/api/v2/auth/login", post(fake_qb_reauth_login))
            .route("/api/v2/torrents/info", get(fake_qb_reauth_torrent_list))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = QBittorrentClient::new(
            format!("http://{address}"),
            "user".to_string(),
            "password".to_string(),
            None,
        )
        .unwrap();

        assert!(
            client
                .list_torrents_tolerant(None, None)
                .await
                .unwrap()
                .is_empty()
        );
        server.abort();

        let state = state.lock().unwrap();
        assert_eq!(state.login_calls, 2);
        assert_eq!(state.list_calls, 2);
    }

    #[tokio::test]
    async fn tracker_discovery_uses_the_batch_torrent_list() {
        let app = Router::new()
            .route("/api/v2/auth/login", post(fake_qb_login))
            .route("/api/v2/torrents/info", get(fake_qb_tracker_list));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = QBittorrentClient::new(
            format!("http://{address}"),
            "user".to_string(),
            "password".to_string(),
            None,
        )
        .unwrap();

        let trackers = client.list_torrent_tracker_urls().await.unwrap();
        server.abort();

        assert_eq!(
            trackers,
            vec![
                vec!["https://tracker.example/announce?passkey=dummy-secret".to_string()],
                Vec::<String>::new(),
            ]
        );
    }

    #[tokio::test]
    async fn list_by_hashes_validates_deduplicates_and_filters_untrusted_response() {
        let app = Router::new()
            .route("/api/v2/auth/login", post(fake_qb_login))
            .route("/api/v2/torrents/info", get(fake_qb_untrusted_hash_list));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = QBittorrentClient::new(
            format!("http://{address}"),
            "user".to_string(),
            "password".to_string(),
            None,
        )
        .unwrap();

        let torrents = client
            .list_torrents_by_hashes(&[
                "EADB91A4769B1FAD89E0DD3A930523E7FC5814B8".to_string(),
                "eadb91a4769b1fad89e0dd3a930523e7fc5814b8".to_string(),
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string(),
            ])
            .await
            .unwrap();
        server.abort();

        assert_eq!(torrents.len(), 2);
        assert!(torrents.iter().any(|torrent| {
            torrent
                .hash
                .eq_ignore_ascii_case("eadb91a4769b1fad89e0dd3a930523e7fc5814b8")
        }));
        assert!(torrents.iter().any(|torrent| {
            torrent.hash.eq_ignore_ascii_case(
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            )
        }));
    }

    #[tokio::test]
    async fn list_by_hashes_rejects_empty_or_invalid_hash_before_http() {
        let client = QBittorrentClient::new(
            "http://127.0.0.1:9".to_string(),
            "user".to_string(),
            "password".to_string(),
            None,
        )
        .unwrap();

        for hash in [
            "",
            "not-a-hash",
            " eadb91a4769b1fad89e0dd3a930523e7fc5814b8",
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcde",
        ] {
            let error = client
                .list_torrents_by_hashes(&[hash.to_string()])
                .await
                .unwrap_err();
            assert!(error.contains("hash 格式无效"), "{error}");
        }
    }
}
