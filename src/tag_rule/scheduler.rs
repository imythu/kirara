use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use futures::stream::{FuturesUnordered, StreamExt};
use regex::Regex;
use tokio::sync::{Mutex, broadcast};
use tracing::{debug, error, info, warn};

use crate::db::Database;
use crate::downloader::{DownloaderClient, DownloaderClientPool, TorrentInfo};
use crate::tag_rule::{
    TagMatchCriteria, TagRuleRecord, TagRuleTrackerDiscovery, TagRuleTrackerOption,
    extract_tracker_domain,
};
use crate::torrent_watcher::NewTorrentNotification;

/// 标签规则执行器：周期性全量扫描，并实时消费单种子新增通知。
pub struct TagRuleScheduler {
    db: Database,
    pool: Arc<DownloaderClientPool>,
    execution_lock: Mutex<()>,
}

impl TagRuleScheduler {
    pub fn new(db: Database, pool: Arc<DownloaderClientPool>) -> Arc<Self> {
        Arc::new(Self {
            db,
            pool,
            execution_lock: Mutex::new(()),
        })
    }

    pub async fn start(&self) {
        // 首次使用默认值，后续每次循环从数据库读取最新配置
        let mut interval_secs = self
            .db
            .get_settings()
            .await
            .map(|s| s.tag_rule_scan_interval_mins.saturating_mul(60).max(60))
            .unwrap_or(420);
        info!(
            "tag rule scheduler started, scanning every {}s",
            interval_secs
        );
        let mut interval = tokio::time::interval(Duration::from_secs(interval_secs));
        loop {
            interval.tick().await;
            // 每次循环重新读取间隔配置
            match self.db.get_settings().await {
                Ok(s) => {
                    let new_secs = s.tag_rule_scan_interval_mins.saturating_mul(60).max(60);
                    if new_secs != interval_secs {
                        info!(
                            "tag rule scan interval changed: {}s -> {}s",
                            interval_secs, new_secs
                        );
                        interval_secs = new_secs;
                        interval = tokio::time::interval(Duration::from_secs(interval_secs));
                    }
                }
                Err(e) => {
                    warn!("tag scheduler: 读取设置失败，沿用上次间隔: {}", e);
                }
            }
            if let Err(e) = self.run_once().await {
                error!("tag rule scheduler error: {}", e);
            }
        }
    }

    pub async fn start_new_torrent_subscriber(
        &self,
        mut notifications: broadcast::Receiver<Arc<NewTorrentNotification>>,
    ) {
        info!("tag rule new torrent subscriber started");
        loop {
            match notifications.recv().await {
                Ok(notification) => {
                    if let Err(error) = self.run_for_new_torrent(&notification).await {
                        warn!(
                            downloader_id = notification.downloader_id,
                            downloader_name = %notification.downloader_name,
                            torrent_hash = %notification.torrent.hash,
                            %error,
                            "tag rule subscriber failed to process new torrent"
                        );
                    }
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    warn!(
                        skipped,
                        "tag rule subscriber lagged; running a full reconciliation scan"
                    );
                    if let Err(error) = self.run_once().await {
                        error!(%error, "tag rule subscriber reconciliation scan failed");
                    }
                }
                Err(broadcast::error::RecvError::Closed) => {
                    info!("tag rule new torrent subscriber stopped");
                    return;
                }
            }
        }
    }

    pub async fn discover_trackers(&self) -> Result<TagRuleTrackerDiscovery, String> {
        let downloaders = self
            .db
            .list_downloaders()
            .await
            .map_err(|e| format!("加载下载器列表失败: {e}"))?;
        let mut pending = FuturesUnordered::new();

        for downloader in downloaders {
            let pool = Arc::clone(&self.pool);
            pending.push(async move {
                let result = match pool.get(&downloader).await {
                    Ok(client) => client.list_torrent_tracker_urls().await,
                    Err(error) => Err(error),
                };
                (downloader.id, downloader.name, result)
            });
        }

        let mut discovered = HashMap::<String, (usize, HashSet<i64>)>::new();
        let mut failed_downloaders = Vec::new();
        while let Some((downloader_id, downloader_name, result)) = pending.next().await {
            let torrents = match result {
                Ok(torrents) => torrents,
                Err(error) => {
                    warn!(
                        "tag rule tracker discovery: 读取下载器 '{}' 失败: {}",
                        downloader_name, error
                    );
                    failed_downloaders.push(downloader_name);
                    continue;
                }
            };

            for tracker_urls in torrents {
                let domains = tracker_urls
                    .iter()
                    .filter_map(|url| extract_tracker_domain(url))
                    .collect::<HashSet<_>>();
                for domain in domains {
                    let (torrent_count, downloader_ids) = discovered
                        .entry(domain)
                        .or_insert_with(|| (0, HashSet::new()));
                    *torrent_count += 1;
                    downloader_ids.insert(downloader_id);
                }
            }
        }

        let mut trackers = discovered
            .into_iter()
            .map(|(domain, (torrent_count, downloader_ids))| {
                let mut downloader_ids = downloader_ids.into_iter().collect::<Vec<_>>();
                downloader_ids.sort_unstable();
                TagRuleTrackerOption {
                    domain,
                    torrent_count,
                    downloader_ids,
                }
            })
            .collect::<Vec<_>>();
        trackers.sort_by(|left, right| {
            right
                .torrent_count
                .cmp(&left.torrent_count)
                .then_with(|| left.domain.cmp(&right.domain))
        });
        failed_downloaders.sort();
        failed_downloaders.dedup();

        Ok(TagRuleTrackerDiscovery {
            trackers,
            failed_downloaders,
        })
    }

    pub async fn run_once(&self) -> Result<(), String> {
        let _execution_guard = self.execution_lock.lock().await;
        self.run_once_inner().await
    }

    async fn run_once_inner(&self) -> Result<(), String> {
        let rules = self
            .db
            .list_enabled_tag_rules()
            .await
            .map_err(|e| format!("加载标签规则失败: {}", e))?;

        if rules.is_empty() {
            debug!("no enabled tag rules, skipping");
            return Ok(());
        }

        let parsed_rules = self.parse_rules(&rules)?;
        debug!("tag scheduler: === 开始扫描 ===");
        debug!("tag scheduler: 已加载 {} 条启用规则", parsed_rules.len());
        for (rule, compiled) in &parsed_rules {
            let ids_desc = match rule
                .downloader_ids
                .as_ref()
                .and_then(|s| serde_json::from_str::<Vec<i64>>(s).ok())
            {
                None => "所有".to_string(),
                Some(ids) if ids.is_empty() => "无".to_string(),
                Some(ids) => ids
                    .iter()
                    .map(|id| id.to_string())
                    .collect::<Vec<_>>()
                    .join(","),
            };
            let criteria_desc: Vec<String> = compiled
                .iter()
                .map(|c| match c {
                    CompiledCriteria::Prefix(p) => format!("前缀:{}", p),
                    CompiledCriteria::Suffix(s) => format!("后缀:{}", s),
                    CompiledCriteria::Contains(c) => format!("包含:{}", c),
                    CompiledCriteria::Exact(e) => format!("精确:{}", e),
                    CompiledCriteria::Regex(r) => format!("正则:{}", r.as_str()),
                })
                .collect();
            debug!(
                "tag scheduler:   规则[{}] → 标签 '{}' | 匹配条件: [{}] | 实例: {}",
                rule.name,
                rule.tag_name,
                criteria_desc.join(", "),
                ids_desc
            );
        }

        let downloaders = self
            .db
            .list_downloaders()
            .await
            .map_err(|e| format!("加载下载器列表失败: {}", e))?;

        let mut totals = RuleApplicationTotals::default();

        for downloader in &downloaders {
            let applicable_rules: Vec<_> = parsed_rules
                .iter()
                .filter(|(rule, _)| rule_applies_to_downloader(rule, downloader.id))
                .collect();
            if applicable_rules.is_empty() {
                debug!(
                    "tag scheduler: 下载器 '{}' 无适用规则，跳过",
                    downloader.name
                );
                continue;
            }
            debug!(
                "tag scheduler: 下载器 '{}' 有 {} 条适用规则",
                downloader.name,
                applicable_rules.len()
            );

            let client = match self.pool.get(downloader).await {
                Ok(c) => c,
                Err(e) => {
                    warn!(
                        "tag scheduler: 获取 '{}' 客户端失败: {}",
                        downloader.name, e
                    );
                    continue;
                }
            };

            let torrents = match client.list_torrents(None).await {
                Ok(t) => t,
                Err(e) => {
                    warn!("tag scheduler: 列出 '{}' 种子失败: {}", downloader.name, e);
                    continue;
                }
            };

            debug!(
                "tag scheduler: 下载器 '{}' 共 {} 个种子",
                downloader.name,
                torrents.len()
            );
            totals.torrents += torrents.len();

            for torrent in &torrents {
                self.apply_rules_to_torrent(
                    client.as_ref(),
                    torrent,
                    &applicable_rules,
                    &mut totals,
                )
                .await;
            }
        }

        debug!(
            "tag scheduler: === 扫描完成 === 种子:{} 已有标签跳过:{} 无tracker跳过:{} 命中:{} 标签成功:{}",
            totals.torrents, totals.already, totals.no_tracker, totals.matched, totals.tagged
        );

        // 统计每条规则的标签种子数和总体积，写入数据库
        for (rule, _) in &parsed_rules {
            let target_ids: Option<Vec<i64>> = rule
                .downloader_ids
                .as_ref()
                .and_then(|s| serde_json::from_str(s).ok());

            let mut count: i64 = 0;
            let mut total_size: i64 = 0;
            for downloader in &downloaders {
                if let Some(ref ids) = target_ids {
                    if !ids.contains(&downloader.id) {
                        continue;
                    }
                }
                if let Ok(client) = self.pool.get(downloader).await {
                    if let Ok(torrents) = client.list_torrents(Some(&rule.tag_name)).await {
                        count += torrents.len() as i64;
                        total_size += torrents.iter().map(|t| t.size).sum::<i64>();
                    }
                }
            }
            if let Err(e) = self
                .db
                .update_tag_rule_stats(rule.id, count, total_size)
                .await
            {
                warn!("tag scheduler: 更新规则 '{}' 统计失败: {}", rule.name, e);
            } else {
                debug!(
                    "tag scheduler: 规则 '{}' 统计已更新: 种子数={}, 总体积={}B",
                    rule.name, count, total_size
                );
            }
        }

        Ok(())
    }

    async fn run_for_new_torrent(
        &self,
        notification: &NewTorrentNotification,
    ) -> Result<(), String> {
        let _execution_guard = self.execution_lock.lock().await;
        let Some(downloader) = self
            .db
            .get_downloader(notification.downloader_id)
            .await
            .map_err(|error| format!("加载下载器失败: {error}"))?
        else {
            debug!(
                downloader_id = notification.downloader_id,
                "tag rule subscriber ignored a notification for a deleted downloader"
            );
            return Ok(());
        };
        if !notification.matches_downloader(&downloader) {
            debug!(
                downloader_id = notification.downloader_id,
                torrent_hash = %notification.torrent.hash,
                "tag rule subscriber ignored a stale downloader notification"
            );
            return Ok(());
        }

        let rules = self
            .db
            .list_enabled_tag_rules()
            .await
            .map_err(|error| format!("加载标签规则失败: {error}"))?;
        if rules.is_empty() {
            debug!("tag rule subscriber found no enabled rules");
            return Ok(());
        }
        let parsed_rules = self.parse_rules(&rules)?;
        let applicable_rules = parsed_rules
            .iter()
            .filter(|(rule, _)| rule_applies_to_downloader(rule, downloader.id))
            .collect::<Vec<_>>();
        if applicable_rules.is_empty() {
            debug!(
                downloader_id = downloader.id,
                torrent_hash = %notification.torrent.hash,
                "tag rule subscriber found no applicable rules"
            );
            return Ok(());
        }

        let client = self
            .pool
            .get(&downloader)
            .await
            .map_err(|error| format!("获取下载器 '{}' 客户端失败: {error}", downloader.name))?;
        let requested_hashes = vec![notification.torrent.hash.clone()];
        let current_torrent = client
            .list_torrents_by_hashes(&requested_hashes)
            .await
            .map_err(|error| {
                format!(
                    "读取下载器 '{}' 的新种子 '{}' 失败: {error}",
                    downloader.name, notification.torrent.hash
                )
            })?
            .into_iter()
            .find(|torrent| {
                torrent
                    .hash
                    .eq_ignore_ascii_case(&notification.torrent.hash)
            });
        let Some(current_torrent) = current_torrent else {
            debug!(
                downloader_id = downloader.id,
                torrent_hash = %notification.torrent.hash,
                "tag rule subscriber ignored a torrent that is no longer present"
            );
            return Ok(());
        };
        let mut totals = RuleApplicationTotals {
            torrents: 1,
            ..RuleApplicationTotals::default()
        };
        self.apply_rules_to_torrent(
            client.as_ref(),
            &current_torrent,
            &applicable_rules,
            &mut totals,
        )
        .await;
        debug!(
            downloader_id = downloader.id,
            torrent_hash = %notification.torrent.hash,
            already_tagged = totals.already,
            no_tracker = totals.no_tracker,
            matched_rules = totals.matched,
            tags_added = totals.tagged,
            "tag rule subscriber processed new torrent"
        );
        Ok(())
    }

    async fn apply_rules_to_torrent(
        &self,
        client: &dyn DownloaderClient,
        torrent: &TorrentInfo,
        applicable_rules: &[&ParsedRule],
        totals: &mut RuleApplicationTotals,
    ) {
        let trackers = match client.get_torrent_trackers(&torrent.hash).await {
            Ok(trackers) => trackers,
            Err(error) => {
                debug!(
                    "tag scheduler: 种子 '{}' (hash={}...) 获取tracker失败: {}",
                    torrent.name,
                    &torrent.hash[..8.min(torrent.hash.len())],
                    error
                );
                return;
            }
        };

        let tracker_domains = trackers
            .iter()
            .filter(|url| !url.is_empty() && !url.starts_with("**"))
            .filter_map(|url| extract_tracker_domain(url).map(|domain| (url.as_str(), domain)))
            .collect::<Vec<_>>();
        if tracker_domains.is_empty() {
            totals.no_tracker += 1;
            debug!(
                "tag scheduler: 种子 '{}' (hash={}...) 无可用tracker域名，跳过",
                torrent.name,
                &torrent.hash[..8.min(torrent.hash.len())]
            );
            return;
        }

        let existing_tags = torrent
            .tags
            .split(',')
            .map(str::trim)
            .filter(|tag| !tag.is_empty())
            .collect::<Vec<_>>();

        for (rule, compiled) in applicable_rules.iter().copied() {
            if existing_tags.contains(&rule.tag_name.as_str()) {
                totals.already += 1;
                debug!(
                    "tag scheduler: 种子 '{}' 已有标签 '{}'，跳过规则 '{}'",
                    torrent.name, rule.tag_name, rule.name
                );
                continue;
            }

            let mut matched = false;
            for (raw_url, domain) in &tracker_domains {
                match match_tracker_detailed(domain, compiled) {
                    Some((criteria_idx, criteria)) => {
                        totals.matched += 1;
                        debug!(
                            "tag scheduler: ✓ 种子 '{}' | 域名 '{}' (tracker: {}) | 规则 '{}' 第{}条命中 → 标签 '{}'",
                            torrent.name,
                            domain,
                            raw_url,
                            rule.name,
                            criteria_idx + 1,
                            rule.tag_name
                        );
                        debug!(
                            "tag scheduler:   匹配详情: {} ~ '{}'",
                            criteria_desc(criteria),
                            domain
                        );
                        matched = true;

                        match client
                            .add_torrent_tags(
                                vec![torrent.hash.clone()],
                                vec![rule.tag_name.clone()],
                            )
                            .await
                        {
                            Ok(()) => {
                                totals.tagged += 1;
                                info!(
                                    "tag scheduler: 已为种子 '{}' (hash={}...) 添加标签 '{}'",
                                    torrent.name,
                                    &torrent.hash[..8.min(torrent.hash.len())],
                                    rule.tag_name
                                );
                            }
                            Err(error) => {
                                warn!(
                                    "tag scheduler: 为种子 '{}' 添加标签 '{}' 失败: {}",
                                    torrent.name, rule.tag_name, error
                                );
                            }
                        }
                        break;
                    }
                    None => {
                        debug!(
                            "tag scheduler:   种子 '{}' | 域名 '{}' | 规则 '{}' 未命中任何条件",
                            torrent.name, domain, rule.name
                        );
                    }
                }
            }
            if !matched {
                debug!(
                    "tag scheduler: ✗ 种子 '{}' | 规则 '{}' 全部域名均未匹配 (共{}条): [{}]",
                    torrent.name,
                    rule.name,
                    tracker_domains.len(),
                    tracker_domains
                        .iter()
                        .map(|(_, domain)| domain.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
        }
    }

    /// 解析规则中的匹配条件，预编译正则表达式
    fn parse_rules(&self, rules: &[TagRuleRecord]) -> Result<Vec<ParsedRule>, String> {
        let mut result = Vec::new();
        for rule in rules {
            let criteria: Vec<TagMatchCriteria> = serde_json::from_str(&rule.match_rules)
                .map_err(|e| format!("解析规则 '{}' 的匹配条件失败: {}", rule.name, e))?;
            let compiled: Vec<CompiledCriteria> = criteria
                .into_iter()
                .map(|c| compile_criteria(&c))
                .collect::<Result<Vec<_>, _>>()?;
            result.push((rule.clone(), compiled));
        }
        Ok(result)
    }
}

type ParsedRule = (TagRuleRecord, Vec<CompiledCriteria>);

#[derive(Default)]
struct RuleApplicationTotals {
    torrents: usize,
    matched: usize,
    tagged: usize,
    already: usize,
    no_tracker: usize,
}

fn rule_applies_to_downloader(rule: &TagRuleRecord, downloader_id: i64) -> bool {
    match &rule.downloader_ids {
        Some(ids_json) => serde_json::from_str::<Vec<i64>>(ids_json)
            .unwrap_or_default()
            .contains(&downloader_id),
        None => true,
    }
}

/// 编译后的匹配条件
enum CompiledCriteria {
    Prefix(String),
    Suffix(String),
    Contains(String),
    Exact(String),
    Regex(Regex),
}

fn compile_criteria(criteria: &TagMatchCriteria) -> Result<CompiledCriteria, String> {
    match criteria.match_type.as_str() {
        "prefix" => Ok(CompiledCriteria::Prefix(criteria.pattern.clone())),
        "suffix" => Ok(CompiledCriteria::Suffix(criteria.pattern.clone())),
        "contains" => Ok(CompiledCriteria::Contains(criteria.pattern.clone())),
        "exact" => Ok(CompiledCriteria::Exact(criteria.pattern.clone())),
        "regex" => {
            let re = Regex::new(&criteria.pattern)
                .map_err(|e| format!("无效的正则表达式 '{}': {}", criteria.pattern, e))?;
            Ok(CompiledCriteria::Regex(re))
        }
        other => Err(format!("未知的匹配类型: {}", other)),
    }
}

fn match_tracker_detailed<'a>(
    domain: &str,
    criteria: &'a [CompiledCriteria],
) -> Option<(usize, &'a CompiledCriteria)> {
    if criteria.is_empty() {
        return None;
    }
    criteria.iter().enumerate().find_map(|(i, c)| {
        let hit = match c {
            CompiledCriteria::Prefix(prefix) => domain.starts_with(prefix),
            CompiledCriteria::Suffix(suffix) => domain.ends_with(suffix),
            CompiledCriteria::Contains(needle) => domain.contains(needle),
            CompiledCriteria::Exact(expected) => domain == expected,
            CompiledCriteria::Regex(re) => re.is_match(domain),
        };
        if hit { Some((i, c)) } else { None }
    })
}

fn criteria_desc(c: &CompiledCriteria) -> String {
    match c {
        CompiledCriteria::Prefix(p) => format!("前缀匹配 '{}'", p),
        CompiledCriteria::Suffix(s) => format!("后缀匹配 '{}'", s),
        CompiledCriteria::Contains(n) => format!("包含 '{}'", n),
        CompiledCriteria::Exact(e) => format!("精确匹配 '{}'", e),
        CompiledCriteria::Regex(r) => format!("正则 '{}'", r.as_str()),
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex as StdMutex};

    use tempfile::tempdir;

    use super::*;
    use crate::downloader::{
        AddTorrentOptions, DownloaderSpaceStats, DownloaderTestResult, TorrentInfo,
    };
    use crate::tag_rule::TagRuleRequest;

    struct RecordingDownloader {
        list_calls: AtomicUsize,
        lookup_hashes: StdMutex<Vec<Vec<String>>>,
        tracker_hashes: StdMutex<Vec<String>>,
        tag_calls: StdMutex<Vec<(Vec<String>, Vec<String>)>>,
    }

    impl RecordingDownloader {
        fn new() -> Self {
            Self {
                list_calls: AtomicUsize::new(0),
                lookup_hashes: StdMutex::new(Vec::new()),
                tracker_hashes: StdMutex::new(Vec::new()),
                tag_calls: StdMutex::new(Vec::new()),
            }
        }
    }

    impl DownloaderClient for RecordingDownloader {
        fn test_connection(
            &self,
        ) -> Pin<Box<dyn Future<Output = Result<DownloaderTestResult, String>> + Send + '_>>
        {
            Box::pin(async { Err("not used".to_string()) })
        }

        fn add_torrent(
            &self,
            _torrent_data: Vec<u8>,
            _filename: &str,
            _options: &AddTorrentOptions,
        ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>> {
            Box::pin(async { Err("not used".to_string()) })
        }

        fn list_torrents(
            &self,
            _tag: Option<&str>,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<TorrentInfo>, String>> + Send + '_>> {
            self.list_calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(Vec::new()) })
        }

        fn list_torrents_by_hashes<'a>(
            &'a self,
            hashes: &'a [String],
        ) -> Pin<Box<dyn Future<Output = Result<Vec<TorrentInfo>, String>> + Send + 'a>> {
            let hashes = hashes.to_vec();
            Box::pin(async move {
                self.lookup_hashes.lock().unwrap().push(hashes.clone());
                Ok(hashes.into_iter().map(|hash| torrent(&hash)).collect())
            })
        }

        fn pause_torrent(
            &self,
            _hash: &str,
        ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>> {
            Box::pin(async { Err("not used".to_string()) })
        }

        fn delete_torrent(
            &self,
            _hash: &str,
            _delete_files: bool,
        ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>> {
            Box::pin(async { Err("not used".to_string()) })
        }

        fn get_free_space(
            &self,
            _path: Option<&str>,
        ) -> Pin<Box<dyn Future<Output = Result<u64, String>> + Send + '_>> {
            Box::pin(async { Err("not used".to_string()) })
        }

        fn get_default_save_path(
            &self,
        ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + '_>> {
            Box::pin(async { Err("not used".to_string()) })
        }

        fn get_effective_free_space<'a>(
            &'a self,
            _path: Option<&'a str>,
            _torrents: &'a [TorrentInfo],
        ) -> Pin<Box<dyn Future<Output = Result<DownloaderSpaceStats, String>> + Send + 'a>>
        {
            Box::pin(async { Err("not used".to_string()) })
        }

        fn get_torrent_trackers(
            &self,
            hash: &str,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, String>> + Send + '_>> {
            let hash = hash.to_string();
            Box::pin(async move {
                self.tracker_hashes.lock().unwrap().push(hash);
                Ok(vec!["https://tracker.example/announce".to_string()])
            })
        }

        fn add_torrent_tags(
            &self,
            hashes: Vec<String>,
            tags: Vec<String>,
        ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>> {
            Box::pin(async move {
                self.tag_calls.lock().unwrap().push((hashes, tags));
                Ok(())
            })
        }
    }

    fn torrent(hash: &str) -> TorrentInfo {
        TorrentInfo {
            hash: hash.to_string(),
            name: "new torrent".to_string(),
            size: 1024,
            uploaded: 0,
            downloaded: 0,
            amount_left: None,
            completed: None,
            progress: 0.0,
            upload_speed: 0,
            download_speed: 0,
            ratio: 0.0,
            state: "downloading".to_string(),
            added_on: 1,
            completion_on: 0,
            num_seeds: 0,
            num_leechs: 0,
            save_path: "/downloads".to_string(),
            root_path: String::new(),
            content_path: String::new(),
            tags: String::new(),
            category: String::new(),
            time_active: 0,
            last_activity: 0,
        }
    }

    #[tokio::test]
    async fn new_torrent_handler_applies_rules_only_to_notified_torrent() {
        let directory = tempdir().unwrap();
        let db = Database::open(directory.path()).await.unwrap();
        let downloader_id = db
            .create_downloader("qB", "qbittorrent", "http://qb:8080", "user", "secret")
            .await
            .unwrap();
        db.create_tag_rule(&TagRuleRequest {
            name: "example tracker".to_string(),
            tag_name: "example".to_string(),
            match_rules: vec![TagMatchCriteria {
                match_type: "exact".to_string(),
                pattern: "tracker.example".to_string(),
            }],
            enabled: Some(true),
            downloader_ids: Some(vec![downloader_id]),
        })
        .await
        .unwrap();
        let downloader = db.get_downloader(downloader_id).await.unwrap().unwrap();
        let pool = DownloaderClientPool::new(db.clone());
        let client = Arc::new(RecordingDownloader::new());
        pool.insert_for_test(&downloader, None, client.clone())
            .await;
        let scheduler = TagRuleScheduler::new(db, pool);
        let notification = NewTorrentNotification::new(&downloader, torrent("new-hash"));

        scheduler.run_for_new_torrent(&notification).await.unwrap();

        assert_eq!(client.list_calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            *client.lookup_hashes.lock().unwrap(),
            vec![vec!["new-hash".to_string()]]
        );
        assert_eq!(
            *client.tracker_hashes.lock().unwrap(),
            vec!["new-hash".to_string()]
        );
        assert_eq!(
            *client.tag_calls.lock().unwrap(),
            vec![(vec!["new-hash".to_string()], vec!["example".to_string()])]
        );
    }
}
