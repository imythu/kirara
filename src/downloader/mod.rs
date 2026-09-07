pub mod factory;
pub mod qbittorrent;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::debug;

use crate::db::Database;

/// 下载器类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DownloaderType {
    QBittorrent,
}

impl DownloaderType {
    fn from_str(s: &str) -> Option<Self> {
        match s {
            "qbittorrent" | "qb" => Some(DownloaderType::QBittorrent),
            _ => None,
        }
    }
}

/// 下载器数据库记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloaderRecord {
    pub id: i64,
    pub name: String,
    pub downloader_type: String,
    pub url: String,
    pub username: String,
    pub password: String,
    pub created_at: String,
    pub updated_at: String,
}

/// 添加种子的选项
#[derive(Debug, Clone, Default)]
pub struct AddTorrentOptions {
    pub save_path: Option<String>,
    pub tags: Option<String>,
    pub category: Option<String>,
    pub download_limit: Option<i64>,
    pub upload_limit: Option<i64>,
    pub ratio_limit: Option<f64>,
    pub inactive_seeding_time_limit: Option<i64>,
    pub paused: bool,
    pub skip_checking: bool,
    pub root_folder: Option<bool>,
}

/// 下载器中的种子信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TorrentInfo {
    pub hash: String,
    pub name: String,
    pub size: i64,
    pub uploaded: i64,
    pub downloaded: i64,
    #[serde(default)]
    pub amount_left: Option<i64>,
    #[serde(default)]
    pub completed: Option<i64>,
    #[serde(default)]
    pub progress: f64,
    pub upload_speed: i64,
    pub download_speed: i64,
    pub ratio: f64,
    pub state: String,
    pub added_on: i64,
    pub completion_on: i64,
    pub num_seeds: i32,
    pub num_leechs: i32,
    pub save_path: String,
    pub root_path: String,
    pub content_path: String,
    pub tags: String,
    pub category: String,
    pub time_active: i64,
    pub last_activity: i64,
}

impl TorrentInfo {
    pub fn pending_download_bytes(&self) -> u64 {
        if let Some(left) = self.amount_left.filter(|left| *left >= 0) {
            return left as u64;
        }
        let size = self.size.max(0) as u64;
        if let Some(completed) = self.completed.filter(|completed| *completed >= 0) {
            return size.saturating_sub(completed as u64);
        }
        // Older responses may lack byte counts. Progress describes the selected
        // files; downloaded is lifetime traffic and must never stand in for it.
        if self.progress.is_finite() {
            return (size as f64 * (1.0 - self.progress.clamp(0.0, 1.0))).round() as u64;
        }
        size
    }

    pub fn is_download_incomplete(&self) -> bool {
        self.pending_download_bytes() > 0 || (self.size <= 0 && self.progress < 1.0)
    }

    pub fn completed_bytes(&self) -> u64 {
        (self.size.max(0) as u64).saturating_sub(self.pending_download_bytes())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TorrentFileInfo {
    pub path: String,
    pub size: i64,
    #[serde(default)]
    pub progress: f64,
    #[serde(default)]
    pub is_seed: bool,
}

/// 下载器测试结果
#[derive(Debug, Clone, Serialize)]
pub struct DownloaderTestResult {
    pub success: bool,
    pub message: String,
    pub version: Option<String>,
    pub free_space: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DownloaderSpaceStats {
    pub free_space: u64,
    pub pending_download_bytes: u64,
    pub effective_free_space: u64,
    pub torrent_count: usize,
    pub incomplete_count: usize,
}

/// 下载器客户端 trait — 通用接口，可扩展支持不同下载器
pub trait DownloaderClient: Send + Sync {
    fn test_connection(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<DownloaderTestResult, String>> + Send + '_>>;

    fn add_torrent(
        &self,
        torrent_data: Vec<u8>,
        filename: &str,
        options: &AddTorrentOptions,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>>;

    fn list_torrents(
        &self,
        tag: Option<&str>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<TorrentInfo>, String>> + Send + '_>>;

    fn list_torrents_by_hashes<'a>(
        &'a self,
        hashes: &'a [String],
    ) -> Pin<Box<dyn Future<Output = Result<Vec<TorrentInfo>, String>> + Send + 'a>> {
        Box::pin(async move {
            if hashes.is_empty() {
                return Ok(Vec::new());
            }
            let all = self.list_torrents(None).await?;
            Ok(all
                .into_iter()
                .filter(|torrent| {
                    hashes
                        .iter()
                        .any(|hash| torrent.hash.eq_ignore_ascii_case(hash))
                })
                .collect())
        })
    }

    fn pause_torrent(
        &self,
        hash: &str,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>>;

    fn delete_torrent(
        &self,
        hash: &str,
        delete_files: bool,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>>;

    fn get_free_space(
        &self,
        path: Option<&str>,
    ) -> Pin<Box<dyn Future<Output = Result<u64, String>> + Send + '_>>;

    /// 获取下载器的默认保存路径
    fn get_default_save_path(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + '_>>;

    fn get_effective_free_space<'a>(
        &'a self,
        path: Option<&'a str>,
        torrents: &'a [TorrentInfo],
    ) -> Pin<Box<dyn Future<Output = Result<DownloaderSpaceStats, String>> + Send + 'a>> {
        Box::pin(async move {
            let free_space = self.get_free_space(path).await?;
            let pending_download_bytes = calculate_pending_download_bytes(torrents);
            let incomplete_count = torrents
                .iter()
                .filter(|torrent| torrent.is_download_incomplete())
                .count();

            Ok(DownloaderSpaceStats {
                free_space,
                pending_download_bytes,
                effective_free_space: free_space.saturating_sub(pending_download_bytes),
                torrent_count: torrents.len(),
                incomplete_count,
            })
        })
    }

    /// 获取种子的 tracker URL 列表
    fn get_torrent_trackers(
        &self,
        hash: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, String>> + Send + '_>>;

    /// 返回每个种子的 tracker URL 列表。客户端可以覆盖此方法，用批量接口避免逐个请求。
    fn list_torrent_tracker_urls(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<Vec<String>>, String>> + Send + '_>> {
        Box::pin(async move {
            let torrents = self.list_torrents(None).await?;
            let mut trackers = Vec::with_capacity(torrents.len());
            for torrent in torrents {
                trackers.push(self.get_torrent_trackers(&torrent.hash).await?);
            }
            Ok(trackers)
        })
    }

    /// 为指定种子添加标签
    fn add_torrent_tags(
        &self,
        hashes: Vec<String>,
        tags: Vec<String>,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>>;

    fn export_torrent(
        &self,
        _hash: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, String>> + Send + '_>> {
        Box::pin(async { Err("当前下载器不支持导出种子".to_string()) })
    }

    fn start_torrent(
        &self,
        _hash: &str,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>> {
        Box::pin(async { Err("当前下载器不支持启动种子".to_string()) })
    }

    fn recheck_torrent(
        &self,
        _hash: &str,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>> {
        Box::pin(async { Err("当前下载器不支持重新校验种子".to_string()) })
    }

    fn get_torrent_files(
        &self,
        _hash: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<TorrentFileInfo>, String>> + Send + '_>> {
        Box::pin(async { Err("当前下载器不支持读取种子文件清单".to_string()) })
    }
}

pub fn calculate_pending_download_bytes(torrents: &[TorrentInfo]) -> u64 {
    torrents
        .iter()
        .map(TorrentInfo::pending_download_bytes)
        .fold(0, u64::saturating_add)
}

struct CachedClient {
    record: DownloaderRecord,
    proxy: Option<String>,
    client: Arc<dyn DownloaderClient>,
}

/// 下载器客户端连接池，按 downloader_id 缓存，配置或代理变更时自动重建
pub struct DownloaderClientPool {
    db: Database,
    cache: Mutex<HashMap<i64, CachedClient>>,
}

impl DownloaderClientPool {
    pub fn new(db: Database) -> Arc<Self> {
        Arc::new(Self {
            db,
            cache: Mutex::new(HashMap::new()),
        })
    }

    /// 获取下载器客户端，配置未变时复用已有连接
    pub async fn get(
        &self,
        record: &DownloaderRecord,
    ) -> Result<Arc<dyn DownloaderClient>, String> {
        let proxy = self.db.get_settings().await.ok().and_then(|s| s.proxy);

        let mut cache = self.cache.lock().await;
        if let Some(cached) = cache.get(&record.id) {
            if cached.record.url == record.url
                && cached.record.username == record.username
                && cached.record.password == record.password
                && cached.proxy == proxy
            {
                return Ok(Arc::clone(&cached.client));
            }
            debug!(
                "downloader '{}' (id={}) config or proxy changed, recreating client",
                record.name, record.id
            );
        }

        let client: Arc<dyn DownloaderClient> =
            Arc::from(factory::create_client(record, proxy.as_deref())?);
        cache.insert(
            record.id,
            CachedClient {
                record: record.clone(),
                proxy,
                client: Arc::clone(&client),
            },
        );
        Ok(client)
    }

    #[cfg(test)]
    pub(crate) async fn insert_for_test(
        &self,
        record: &DownloaderRecord,
        proxy: Option<&str>,
        client: Arc<dyn DownloaderClient>,
    ) {
        self.cache.lock().await.insert(
            record.id,
            CachedClient {
                record: record.clone(),
                proxy: proxy.map(str::to_string),
                client,
            },
        );
    }
}
