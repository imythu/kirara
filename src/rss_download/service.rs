use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::sync::Notify;

use crate::collector::DownloaderSnapshotCollector;
use crate::db::{
    Database,
    rss::{EvaluationClaim, FeedClaim, JobClaim, JobTransition},
};
use crate::downloader::{AddTorrentOptions, DownloaderClient, DownloaderClientPool};
use crate::indexer::IndexerPool;
use crate::media::torrent::{torrent_file_manifest, torrent_infohash};
use crate::rss::download::redact_url;

use super::{
    config::RssSchedulerConfig,
    fetcher::{RssFetcher, checked_url, supports_torrent_resolution},
    matcher,
    models::*,
};

pub struct RssService {
    pub db: Database,
    pub wake: Notify,
    pub config: RssSchedulerConfig,
    fetcher: RssFetcher,
    downloaders: Arc<DownloaderClientPool>,
    collector: Arc<DownloaderSnapshotCollector>,
}

struct PreparedDownload {
    torrent: Vec<u8>,
    infohash: String,
    attributes: ItemAttributes,
    client: Arc<dyn DownloaderClient>,
    already_present: bool,
    space_available: bool,
}

impl RssService {
    pub fn new(
        db: Database,
        downloaders: Arc<DownloaderClientPool>,
        indexers: Arc<IndexerPool>,
        collector: Arc<DownloaderSnapshotCollector>,
    ) -> Arc<Self> {
        Arc::new(Self {
            fetcher: RssFetcher::new(db.clone(), indexers),
            db,
            downloaders,
            collector,
            wake: Notify::new(),
            config: RssSchedulerConfig::default(),
        })
    }

    pub async fn test_feed(&self, request: FeedTestRequest) -> RssResult<FeedTestResponse> {
        let mut feed = if let Some(id) = request.feed_id {
            self.db.rss_get_feed(id).await?
        } else {
            draft_feed()
        };
        if let Some(url) = request.url.filter(|v| !v.trim().is_empty()) {
            feed.url = url.trim().to_string();
        }
        checked_url(&feed.url)?;
        if let Some(site_id) = request.site_id {
            feed.record.site_id = site_id;
        }
        if let Some(use_proxy) = request.use_proxy {
            feed.record.use_proxy = use_proxy;
        }
        let fetched = self.fetcher.fetch(&feed, false).await?;
        let can_resolve = self.site_can_resolve(&feed).await?;
        let items = fetched
            .items
            .iter()
            .take(20)
            .enumerate()
            .map(|(index, item)| preview_item(item, &feed, -(index as i64 + 1), can_resolve))
            .collect();
        Ok(FeedTestResponse {
            title: fetched.title,
            item_count: fetched.items.len(),
            items,
            warnings: fetched.warnings,
            sample_time: Utc::now().to_rfc3339(),
        })
    }

    async fn site_can_resolve(&self, feed: &StoredFeed) -> RssResult<bool> {
        let Some(id) = feed.record.site_id else {
            return Ok(false);
        };
        let Some(site) = self.db.get_site(id).await? else {
            return Ok(false);
        };
        Ok(supports_torrent_resolution(
            &site.site_type,
            &site.auth_config,
        ))
    }

    pub async fn preview(&self, request: PreviewRequest) -> RssResult<PreviewResponse> {
        matcher::validate_filters(&request.rule.filters).map_err(RssError::Invalid)?;
        if request.rule.feed_ids.is_empty()
            || request.rule.feed_ids.len() > 200
            || request.item_ids.len() > 200
        {
            return Err(RssError::Invalid(
                "请选择 1–200 个订阅源，单次最多预览 200 个已有条目".into(),
            ));
        }
        if request.refresh_samples && !request.item_ids.is_empty() {
            return Err(RssError::Invalid(
                "补下预览使用已保存条目，请先在源详情中重新检查".into(),
            ));
        }
        let mut samples = Vec::new();
        let mut available = 0_u64;
        let mut unvisited_feeds = false;
        if request.refresh_samples {
            for (index, id) in request.rule.feed_ids.iter().enumerate() {
                let feed = self.db.rss_get_feed(*id).await?;
                let fetched = self.fetcher.fetch(&feed, false).await?;
                let can_resolve = self.site_can_resolve(&feed).await?;
                available += fetched.items.len() as u64;
                for item in fetched
                    .items
                    .iter()
                    .take(200_usize.saturating_sub(samples.len()))
                {
                    samples.push(preview_item(
                        item,
                        &feed,
                        -(samples.len() as i64 + 1),
                        can_resolve,
                    ));
                }
                if samples.len() >= 200 {
                    unvisited_feeds = index + 1 < request.rule.feed_ids.len();
                    break;
                }
            }
        } else if !request.item_ids.is_empty() {
            for id in &request.item_ids {
                let item = self.db.rss_get_item(*id).await?;
                if !request.rule.feed_ids.contains(&item.feed_id) {
                    return Err(RssError::Invalid("所选条目不属于这条规则的订阅源".into()));
                }
                samples.push(item);
            }
            available = samples.len() as u64;
        } else {
            for id in &request.rule.feed_ids {
                let page = self
                    .db
                    .rss_list_items(
                        ListQuery {
                            feed_id: Some(*id),
                            page_size: 200,
                            ..Default::default()
                        }
                        .normalized(),
                    )
                    .await?;
                available += page.total;
                samples.extend(page.items);
            }
            samples.sort_by(|a, b| b.first_seen_at.cmp(&a.first_seen_at).then(b.id.cmp(&a.id)));
            samples.truncate(200);
        }
        let sample_time = samples
            .iter()
            .map(|item| &item.last_seen_at)
            .max()
            .cloned()
            .unwrap_or_else(|| Utc::now().to_rfc3339());
        let mut response = PreviewResponse {
            rule_version: request.rule.expected_version,
            total: samples.len() as u64,
            matched: 0,
            rejected: 0,
            unknown: 0,
            sample_limited: unvisited_feeds || available > samples.len() as u64,
            sample_time,
            items: Vec::new(),
        };
        for item in samples {
            let evaluation = matcher::evaluate(&request.rule.filters, &item);
            if evaluation.matched {
                response.matched += 1;
            } else if evaluation.needs_attributes {
                response.unknown += 1;
            } else {
                response.rejected += 1;
            }
            response.items.push(PreviewItem { item, evaluation });
        }
        Ok(response)
    }

    pub async fn scan(&self, claim: FeedClaim) -> RssResult<()> {
        let work = self.fetcher.fetch(&claim.feed, true);
        tokio::pin!(work);
        let result = loop {
            tokio::select! {
                result = &mut work => break result,
                _ = tokio::time::sleep(self.config.feed_lease / 3) => {
                    if !self.db.rss_renew_feed(&claim, self.config.feed_lease.as_secs()).await? {
                        return Err(RssError::Conflict("订阅源已变更，本次抓取结果已放弃".into()));
                    }
                }
            }
        };
        match result {
            Ok(fetched) => {
                self.db.rss_commit_scan(&claim, fetched).await?;
                self.wake.notify_waiters();
            }
            Err(error) => {
                let retry_at = match &error {
                    RssError::RateLimited { retry_at, .. } => Some(retry_at.clone()),
                    _ => None,
                };
                let requires_action = matches!(
                    error,
                    RssError::Authentication(_) | RssError::Invalid(_) | RssError::NotFound(_)
                );
                self.db
                    .rss_fail_scan(&claim, &error.to_string(), retry_at, requires_action)
                    .await?;
            }
        }
        Ok(())
    }

    pub async fn evaluate(&self, claim: EvaluationClaim) -> RssResult<()> {
        let needs_enrichment = claim
            .rules
            .iter()
            .any(|rule| matcher::evaluate(&rule.filters, &claim.item).needs_attributes);
        let mut attributes = None;
        if needs_enrichment {
            let locator = self.db.rss_get_item_locator(claim.item.id).await?;
            let work = self.fetcher.enrich(&claim.feed, &claim.item, &locator);
            tokio::pin!(work);
            let result = loop {
                tokio::select! {
                    result = &mut work => break result,
                    _ = tokio::time::sleep(self.config.evaluation_lease / 3) => {
                        if !self.db.rss_renew_evaluation(&claim, self.config.evaluation_lease.as_secs()).await? {
                            return Err(RssError::Conflict("条目或规则已变更，稍后重新匹配".into()));
                        }
                    }
                }
            };
            match result {
                Ok(value) => attributes = Some(value),
                Err(error) => {
                    // Persist the unknown evaluation; the retry is driven by the durable matcher
                    // queue instead of treating a failed lookup as a false attribute.
                    let mut unknown = claim.item.attributes.clone();
                    unknown.hints.push(error.to_string());
                    unknown.hints.truncate(12);
                    attributes = Some(unknown);
                }
            }
        }
        self.db.rss_commit_evaluation(&claim, attributes).await?;
        self.wake.notify_waiters();
        Ok(())
    }

    pub async fn process_job(&self, claim: JobClaim) -> RssResult<()> {
        if claim.job.status == "reconciling" {
            return self.reconcile(&claim).await;
        }
        let prepared = self.prepare_download(&claim);
        tokio::pin!(prepared);
        let result = loop {
            tokio::select! {
                result = &mut prepared => break result,
                _ = tokio::time::sleep(self.config.job_lease / 3) => {
                    if !self.db.rss_renew_job(&claim, self.config.job_lease.as_secs()).await? {
                        return Err(RssError::Conflict("下载任务已被暂停、取消或重新认领".into()));
                    }
                }
            }
        };
        let prepared = match result {
            Ok(prepared) => prepared,
            Err(error) => {
                self.fail_job(&claim, &error).await?;
                return Ok(());
            }
        };
        if prepared.already_present {
            self.db
                .rss_transition_job(
                    &claim,
                    JobTransition {
                        status: "already_present".into(),
                        infohash: Some(prepared.infohash),
                        release_reservation: true,
                        ..Default::default()
                    },
                )
                .await?;
            return Ok(());
        }
        if !prepared.space_available {
            self.db
                .rss_transition_job(
                    &claim,
                    JobTransition {
                        status: "waiting".into(),
                        last_error: Some(
                            "下载器报告的可用空间不足，预留其他未完成任务容量后将稍后重试".into(),
                        ),
                        next_attempt_at: Some(after_seconds(60)),
                        ..Default::default()
                    },
                )
                .await?;
            return Ok(());
        }
        let Some(submitting) = self
            .db
            .rss_prepare_submission(
                &claim,
                &prepared.infohash,
                prepared.attributes.size_bytes.unwrap_or(0),
                Some(prepared.attributes),
            )
            .await?
        else {
            return Ok(());
        };
        let options = &submitting.job.options_snapshot;
        let add_options = AddTorrentOptions {
            save_path: options.save_path.clone().filter(|v| !v.is_empty()),
            category: options.category.clone().filter(|v| !v.is_empty()),
            tags: (!options.tags.is_empty()).then(|| options.tags.join(",")),
            paused: options.paused,
            ..Default::default()
        };
        let filename = format!("rss-{}.torrent", submitting.job.id);
        let sent = tokio::time::timeout(
            self.config.submission_timeout,
            prepared
                .client
                .add_torrent(prepared.torrent, &filename, &add_options),
        )
        .await;
        let submission_error = match sent {
            Ok(Ok(())) => None,
            Ok(Err(_)) => Some("下载器提交请求失败，正在核对是否已接收".to_string()),
            Err(_) => Some("提交请求超时，正在核对下载器是否已接收".to_string()),
        };
        let presence = live_presence(
            prepared.client.as_ref(),
            &prepared.infohash,
            self.config.lookup_timeout,
        )
        .await;
        match presence {
            Ok(true) => {
                self.db
                    .rss_transition_job(
                        &submitting,
                        JobTransition {
                            status: "submitted".into(),
                            infohash: Some(prepared.infohash),
                            release_reservation: false,
                            ..Default::default()
                        },
                    )
                    .await?;
            }
            _ => {
                self.db
                    .rss_transition_job(
                        &submitting,
                        JobTransition {
                            status: "reconciling".into(),
                            infohash: Some(prepared.infohash),
                            last_error: Some(
                                submission_error
                                    .unwrap_or_else(|| "下载器暂未确认接收，正在对账".into()),
                            ),
                            next_attempt_at: Some(after_seconds(30)),
                            ..Default::default()
                        },
                    )
                    .await?;
            }
        }
        Ok(())
    }

    async fn prepare_download(&self, claim: &JobClaim) -> RssResult<PreparedDownload> {
        let downloader = self
            .db
            .get_downloader(claim.job.downloader_id)
            .await?
            .ok_or_else(|| RssError::NotFound("目标下载器已不存在，请检查规则配置".into()))?;
        let client = self
            .downloaders
            .get(&downloader)
            .await
            .map_err(|_| RssError::Unavailable("无法连接目标下载器，请检查连接配置".into()))?;
        let torrent = self
            .fetcher
            .torrent(&claim.feed, &claim.item, &claim.locator)
            .await?;
        let infohash = torrent_infohash(&torrent)
            .map_err(|_| RssError::Invalid("种子元数据无法校验".into()))?;
        if live_presence(client.as_ref(), &infohash, self.config.lookup_timeout).await? {
            return Ok(PreparedDownload {
                torrent,
                infohash,
                attributes: claim.item.attributes.clone(),
                client,
                already_present: true,
                space_available: true,
            });
        }
        let mut attributes = self
            .fetcher
            .fresh_attributes(
                &claim.feed,
                &claim.item,
                &claim.locator,
                &claim.job.filters_snapshot,
            )
            .await?;
        let files = torrent_file_manifest(&torrent)
            .map_err(|_| RssError::Invalid("种子文件清单无效，无法确认实际下载大小".into()))?;
        attributes.size_bytes = Some(
            files
                .iter()
                .try_fold(0_u64, |sum, file| {
                    u64::try_from(file.size)
                        .ok()
                        .and_then(|size| sum.checked_add(size))
                })
                .ok_or_else(|| RssError::Invalid("种子文件大小超出可处理范围".into()))?,
        );
        let sampled_at = Utc::now().to_rfc3339();
        let torrents = timed(client.list_torrents(None), self.config.lookup_timeout).await?;
        self.db
            .rss_release_observed_reservations(
                claim.job.downloader_id,
                torrents
                    .iter()
                    .map(|torrent| torrent.hash.clone())
                    .collect(),
                &sampled_at,
            )
            .await?;
        let effective = timed(
            client.get_effective_free_space(
                claim.job.options_snapshot.save_path.as_deref(),
                &torrents,
            ),
            self.config.lookup_timeout,
        )
        .await?;
        let reservations = self
            .db
            .rss_reserved_bytes(claim.job.downloader_id, claim.job.id)
            .await?;
        let required = attributes
            .size_bytes
            .unwrap_or(0)
            .saturating_add(claim.job.options_snapshot.reserve_space_bytes);
        let space_available =
            effective.effective_free_space.saturating_sub(reservations) >= required;
        Ok(PreparedDownload {
            torrent,
            infohash,
            attributes,
            client,
            already_present: false,
            space_available,
        })
    }

    async fn reconcile(&self, claim: &JobClaim) -> RssResult<()> {
        let Some(hash) = claim.job.infohash.as_deref() else {
            return Err(RssError::Conflict("对账任务缺少已保存的种子哈希".into()));
        };
        let lookup = async {
            let downloader = self
                .db
                .get_downloader(claim.job.downloader_id)
                .await?
                .ok_or_else(|| {
                    RssError::NotFound("原下载器配置不可用，尚不能确定投递结果".into())
                })?;
            let client = self
                .downloaders
                .get(&downloader)
                .await
                .map_err(|_| RssError::Unavailable("原下载器暂不可达".into()))?;
            live_presence(client.as_ref(), hash, self.config.lookup_timeout).await
        }
        .await;
        let update = match lookup {
            Ok(true) => JobTransition {
                status: "submitted".into(),
                release_reservation: false,
                ..Default::default()
            },
            Ok(false) => {
                let age = claim
                    .submission_started_at
                    .as_deref()
                    .and_then(|time| DateTime::parse_from_rfc3339(time).ok())
                    .map(|time| (Utc::now() - time.with_timezone(&Utc)).num_seconds())
                    .unwrap_or(0);
                if age < self.config.reconciliation_window.as_secs() as i64 {
                    JobTransition {
                        status: "reconciling".into(),
                        last_error: Some("等待下载器完成接收确认".into()),
                        next_attempt_at: Some(after_seconds(30)),
                        ..Default::default()
                    }
                } else if claim.job.attempts >= self.config.max_attempts {
                    JobTransition {
                        status: "failed".into(),
                        last_error: Some("已确认下载器未收到种子，自动重试次数已用尽".into()),
                        release_reservation: true,
                        ..Default::default()
                    }
                } else {
                    JobTransition {
                        status: "retry_wait".into(),
                        last_error: Some("已确认下载器未收到种子，稍后按同一任务重试".into()),
                        next_attempt_at: Some(retry_time(claim.job.attempts)),
                        release_reservation: true,
                        ..Default::default()
                    }
                }
            }
            Err(error) => JobTransition {
                status: "reconciling".into(),
                last_error: Some(format!("{}；仍在确认投递结果", error)),
                next_attempt_at: Some(after_seconds(60)),
                ..Default::default()
            },
        };
        self.db.rss_transition_job(claim, update).await?;
        Ok(())
    }

    async fn fail_job(&self, claim: &JobClaim, error: &RssError) -> RssResult<()> {
        let mut update = JobTransition {
            last_error: Some(error.to_string()),
            release_reservation: true,
            ..Default::default()
        };
        match error {
            RssError::Conflict(_) => return Ok(()),
            RssError::RateLimited { retry_at, .. } => {
                update.status = "waiting".into();
                update.next_attempt_at = Some(retry_at.clone());
            }
            RssError::Unavailable(_)
                if claim.job.attempts.saturating_add(1) < self.config.max_attempts =>
            {
                update.status = "retry_wait".into();
                update.next_attempt_at = Some(retry_time(claim.job.attempts.saturating_add(1)));
            }
            _ => {
                update.status = "failed".into();
            }
        }
        self.db.rss_transition_job(claim, update).await?;
        Ok(())
    }

    pub async fn track_reservations(&self, stop: &tokio_util::sync::CancellationToken) {
        let mut snapshots = self.collector.subscribe();
        loop {
            tokio::select! {
                _ = stop.cancelled() => break,
                snapshot = snapshots.recv() => match snapshot {
                    Ok(snapshot) => {
                        if let Err(error) = self.db.rss_release_observed_reservations(snapshot.downloader_id,
                            snapshot.torrents.iter().map(|torrent| torrent.hash.clone()).collect(), &snapshot.recorded_at).await {
                            tracing::warn!(%error, "RSS reservation update failed");
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }

    pub async fn with_download_state(&self, mut jobs: Page<JobRecord>) -> Page<JobRecord> {
        let ids: std::collections::HashSet<_> =
            jobs.items.iter().map(|job| job.downloader_id).collect();
        for id in ids {
            if let Some(snapshot) = self.collector.get_snapshot(id).await {
                let _ = self
                    .db
                    .rss_release_observed_reservations(
                        id,
                        snapshot
                            .torrents
                            .iter()
                            .map(|torrent| torrent.hash.clone())
                            .collect(),
                        &snapshot.recorded_at,
                    )
                    .await;
            }
        }
        for job in &mut jobs.items {
            self.decorate_job(job).await;
        }
        jobs
    }

    pub async fn decorate_job(&self, job: &mut JobRecord) {
        let Some(hash) = job.infohash.as_deref() else {
            return;
        };
        if let Some(snapshot) = self.collector.get_snapshot(job.downloader_id).await {
            job.sampled_at = Some(snapshot.recorded_at.clone());
            if let Some(torrent) = snapshot
                .torrents
                .iter()
                .find(|torrent| torrent.hash.eq_ignore_ascii_case(hash))
            {
                job.download_state = Some(torrent.state.clone());
                job.progress = Some(torrent.progress.clamp(0.0, 1.0));
            } else if matches!(job.status.as_str(), "submitted" | "already_present")
                && job.submitted_at.as_deref().is_some_and(|submitted| {
                    DateTime::parse_from_rfc3339(submitted)
                        .ok()
                        .zip(DateTime::parse_from_rfc3339(&snapshot.recorded_at).ok())
                        .is_some_and(|(submitted, sampled)| sampled >= submitted)
                })
            {
                job.download_state = Some("not_found".into());
                job.progress = None;
            }
        }
    }
}

async fn live_presence(
    client: &dyn DownloaderClient,
    hash: &str,
    timeout: Duration,
) -> RssResult<bool> {
    let hashes = [hash.to_string()];
    let torrents = timed(client.list_torrents_by_hashes(&hashes), timeout).await?;
    Ok(torrents
        .iter()
        .any(|torrent| torrent.hash.eq_ignore_ascii_case(hash)))
}

async fn timed<T>(
    work: impl std::future::Future<Output = Result<T, String>>,
    timeout: Duration,
) -> RssResult<T> {
    match tokio::time::timeout(timeout, work).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(_)) => Err(RssError::Unavailable(
            "下载器请求失败，请检查连接、认证或剩余空间信息".into(),
        )),
        Err(_) => Err(RssError::Unavailable("下载器请求超时，请稍后重试".into())),
    }
}
fn after_seconds(seconds: i64) -> String {
    (Utc::now() + chrono::Duration::seconds(seconds)).to_rfc3339()
}
fn retry_time(attempts: u32) -> String {
    let delay = [60, 300, 900, 3600][attempts.saturating_sub(1).min(3) as usize];
    after_seconds(delay + (Utc::now().timestamp_subsec_millis() % 11) as i64)
}

fn draft_feed() -> StoredFeed {
    let now = Utc::now().to_rfc3339();
    StoredFeed {
        url: String::new(),
        etag: None,
        last_modified: None,
        record: FeedRecord {
            id: 0,
            name: "RSS 预览".into(),
            url_display: String::new(),
            site_id: None,
            site_name: None,
            use_proxy: None,
            enabled: false,
            interval_minutes: 15,
            generation: 1,
            version: 0,
            initialized_at: None,
            last_sequence: 0,
            last_checked_at: None,
            next_run_at: None,
            last_status: "preview".into(),
            last_error: None,
            item_count: 0,
            pending_count: 0,
            created_at: now.clone(),
            updated_at: now,
        },
    }
}
fn preview_item(
    item: &NormalizedItem,
    feed: &StoredFeed,
    id: i64,
    site_can_resolve: bool,
) -> ItemRecord {
    let now = Utc::now().to_rfc3339();
    ItemRecord {
        id,
        feed_id: feed.record.id,
        feed_name: feed.record.name.clone(),
        generation: feed.record.generation,
        item_key: item.item_key.clone(),
        sequence: 0,
        title: item.title.clone(),
        detail_url: item.detail_url.as_deref().map(redact_url),
        site_torrent_id: item.site_torrent_id.clone(),
        published_at: item.published_at.clone(),
        categories: item.categories.clone(),
        attributes: item.attributes.clone(),
        downloadable: item.download_url.is_some()
            || (site_can_resolve && item.site_torrent_id.is_some()),
        content_revision: 0,
        first_seen_at: now.clone(),
        last_seen_at: now,
        status: "preview".into(),
        decisions: Vec::new(),
    }
}
