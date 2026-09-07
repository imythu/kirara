use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration as StdDuration, Instant};

use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use tracing::{Instrument, warn};

use crate::db::Database;
use crate::downloader::{AddTorrentOptions, DownloaderClient, DownloaderClientPool};
use crate::error::AppError;
use crate::indexer::{
    IndexerAggregator, IndexerError, IndexerPool, SearchRequest, SearchResult, SiteSearchError,
};
use crate::net::client_factory;

use super::domain::{
    DecisionEngine, MatchDecision, MediaTarget, QualityProfile, QueryGenerator, RejectCode,
    ReleaseInfo, ReleaseParser, SearchCriteria, SeasonEpisode, SortKey, stable_release_key,
};
use super::lease::process_owner_id;
use super::models::{
    MEDIA_DOWNLOAD_RECONCILIATION_GRACE_SECONDS, MediaDownloadCoverage, MediaDownloadDeleteOutcome,
    MediaDownloadDeletion, MediaDownloadRecord, NewMediaDownload, NewSubscription,
    QualityProfileRecord, SubscriptionRecord, SubscriptionTargetRecord, media_download_category,
};
use super::progression::{
    ProgressionError, SubscriptionTargetSeedStatus, TargetReadiness, air_date_eligible_at,
    next_run_at, plan_tv_targets, target_readiness,
};
use super::tmdb::{TmdbClient, TmdbDetails, TmdbError, TmdbMedia, TmdbMediaType, TmdbSeason};
use super::torrent::{TorrentMetadataError, torrent_infohash};

const SUBSCRIPTION_LEASE_SECONDS: i64 = 10 * 60;
const DOWNLOAD_LEASE_SECONDS: i64 = 5 * 60;
const DOWNLOAD_CONFIRM_ATTEMPTS: usize = 5;
const DOWNLOAD_CONFIRM_INTERVAL: StdDuration = StdDuration::from_millis(300);
const CANDIDATE_CACHE_TTL: StdDuration = StdDuration::from_secs(10 * 60);
const CANDIDATE_CACHE_CAPACITY: usize = 2_048;
const RESOURCE_SEARCH_RESULT_LIMIT: usize = 512;
const _: () = assert!(RESOURCE_SEARCH_RESULT_LIMIT <= CANDIDATE_CACHE_CAPACITY);

#[derive(Debug, thiserror::Error)]
pub enum MediaServiceError {
    #[error(transparent)]
    App(#[from] AppError),
    #[error(transparent)]
    Tmdb(#[from] TmdbError),
    #[error(transparent)]
    Progression(#[from] ProgressionError),
    #[error(transparent)]
    Indexer(#[from] IndexerError),
    #[error(transparent)]
    Torrent(#[from] TorrentMetadataError),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("invalid request: {0}")]
    Invalid(String),
    #[error("serialization error: {0}")]
    Serialization(String),
    #[error("downloader error: {0}")]
    Downloader(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceSearchRequest {
    pub query: Option<String>,
    #[serde(default)]
    pub site_ids: Vec<i64>,
    pub target: Option<MediaTarget>,
    pub quality_profile_id: Option<i64>,
    pub page_size: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceCandidate {
    pub candidate_id: String,
    pub result: SearchResult,
    pub release: Option<ReleaseInfo>,
    pub parse_error: Option<String>,
    pub decision: Option<MatchDecision>,
    pub sort_key: Option<SortKey>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceSearchResponse {
    pub queries: Vec<String>,
    pub candidates: Vec<ResourceCandidate>,
    pub errors: Vec<SiteSearchError>,
    pub total_sites: usize,
    pub successful_sites: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSubscriptionRequest {
    pub tmdb_id: i64,
    pub media_type: String,
    pub season: Option<u32>,
    pub start_episode: Option<u32>,
    pub absolute_episode: Option<u32>,
    pub quality_profile_id: i64,
    pub downloader_id: i64,
    #[serde(default)]
    pub site_ids: Vec<i64>,
    pub save_path: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueueDownloadRequest {
    pub candidate_id: String,
    pub quality_profile_id: i64,
    pub downloader_id: i64,
    pub subscription_id: Option<i64>,
    pub override_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubscriptionRunResult {
    pub subscription_id: i64,
    pub target_key: String,
    pub query_count: usize,
    pub candidate_count: usize,
    pub accepted_count: usize,
    pub download: Option<MediaDownloadRecord>,
    pub site_errors: Vec<SiteSearchError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubscriptionRunSnapshot {
    pub started_at: String,
    pub finished_at: String,
    pub target_key: String,
    pub queries: Vec<String>,
    pub candidates: Vec<ResourceCandidate>,
    pub site_errors: Vec<SiteSearchError>,
    pub total_sites: usize,
    pub successful_sites: usize,
    pub best_candidate_id: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailedDownloadReconciliation {
    pub resolution: String,
    pub download: MediaDownloadRecord,
}

#[derive(Clone)]
pub struct MediaService {
    db: Database,
    indexers: Arc<IndexerPool>,
    downloaders: Arc<DownloaderClientPool>,
    parser: ReleaseParser,
    candidate_cache: Arc<tokio::sync::Mutex<CandidateCache>>,
}

#[derive(Clone)]
struct CachedCandidate {
    result: SearchResult,
    target: Option<MediaTarget>,
    quality_profile_id: Option<i64>,
    expires_at: Instant,
}

#[derive(Default)]
struct CandidateCache {
    entries: HashMap<String, CachedCandidate>,
    insertion_order: VecDeque<String>,
}

impl CandidateCache {
    fn insert(
        &mut self,
        candidate_id: String,
        result: SearchResult,
        target: Option<MediaTarget>,
        quality_profile_id: Option<i64>,
    ) {
        self.prune_expired();
        while self.entries.len() >= CANDIDATE_CACHE_CAPACITY {
            let Some(oldest) = self.insertion_order.pop_front() else {
                break;
            };
            self.entries.remove(&oldest);
        }
        self.insertion_order.push_back(candidate_id.clone());
        self.entries.insert(
            candidate_id,
            CachedCandidate {
                result,
                target,
                quality_profile_id,
                expires_at: Instant::now() + CANDIDATE_CACHE_TTL,
            },
        );
    }

    fn get(&mut self, candidate_id: &str) -> Option<CachedCandidate> {
        self.prune_expired();
        self.entries.get(candidate_id).cloned()
    }

    fn prune_expired(&mut self) {
        let now = Instant::now();
        self.entries.retain(|_, entry| entry.expires_at > now);
        self.insertion_order
            .retain(|candidate_id| self.entries.contains_key(candidate_id));
    }
}

enum PreparedSubscriptionTarget {
    Due(MediaTarget),
    Deferred {
        target_key: String,
        next_run_at: String,
        status: &'static str,
    },
    Completed {
        target_key: String,
    },
}

enum TorrentSubmissionError {
    ConfirmedAbsent(String),
    Unknown(String),
}

impl MediaService {
    pub fn new(db: Database, downloaders: Arc<DownloaderClientPool>) -> Arc<Self> {
        Arc::new(Self {
            db,
            indexers: IndexerPool::new(),
            downloaders,
            parser: ReleaseParser::default(),
            candidate_cache: Arc::new(tokio::sync::Mutex::new(CandidateCache::default())),
        })
    }

    pub fn database(&self) -> &Database {
        &self.db
    }

    pub async fn tmdb_search(
        &self,
        query: &str,
        media_type: Option<&str>,
    ) -> Result<Vec<TmdbMedia>, MediaServiceError> {
        self.tmdb_client()
            .await?
            .search(query, media_type)
            .await
            .map_err(Into::into)
    }

    pub async fn tmdb_details(
        &self,
        tmdb_id: i64,
        media_type: TmdbMediaType,
    ) -> Result<TmdbDetails, MediaServiceError> {
        self.tmdb_client()
            .await?
            .details(tmdb_id, media_type)
            .await
            .map_err(Into::into)
    }

    pub async fn tmdb_season(
        &self,
        tmdb_id: i64,
        season: u32,
    ) -> Result<TmdbSeason, MediaServiceError> {
        self.tmdb_client()
            .await?
            .season(tmdb_id, season)
            .await
            .map_err(Into::into)
    }

    pub async fn create_subscription(
        &self,
        request: &CreateSubscriptionRequest,
    ) -> Result<SubscriptionRecord, MediaServiceError> {
        if request.site_ids.is_empty() {
            return Err(MediaServiceError::Invalid(
                "at least one PT site must be selected".to_string(),
            ));
        }
        let media_type = TmdbMediaType::parse(&request.media_type)?;
        if media_type == TmdbMediaType::Tv && request.season.is_none() {
            return Err(MediaServiceError::Invalid(
                "season is required for a TV subscription".to_string(),
            ));
        }
        self.ensure_references(
            request.quality_profile_id,
            request.downloader_id,
            &request.site_ids,
        )
        .await?;
        let (details, target_plan) = if media_type == TmdbMediaType::Tv {
            let season_number = request.season.expect("TV season was validated");
            let client = self.tmdb_client().await?;
            let (details, season) = tokio::try_join!(
                client.details(request.tmdb_id, media_type.clone()),
                client.season(request.tmdb_id, season_number),
            )?;
            let plan = plan_tv_targets(
                &details,
                &season,
                request.start_episode.unwrap_or(1),
                request.absolute_episode,
            )?;
            (details, Some(plan))
        } else {
            (
                self.tmdb_details(request.tmdb_id, media_type.clone())
                    .await?,
                None,
            )
        };
        let mut aliases = details.aliases.clone();
        if let Some(original) = &details.media.original_title {
            push_case_insensitive(&mut aliases, original);
        }
        let subscription = NewSubscription {
            tmdb_id: details.media.tmdb_id,
            media_type: media_type.as_str().to_string(),
            tmdb_is_animation: details.media.is_animation,
            tmdb_genres: details.media.genres.clone(),
            title: details.media.title,
            original_title: details.media.original_title,
            aliases,
            year: details.media.year,
            poster_path: details.media.poster_path,
            season: request.season,
            start_episode: request
                .start_episode
                .or((media_type == TmdbMediaType::Tv).then_some(1)),
            absolute_episode: request.absolute_episode,
            quality_profile_id: request.quality_profile_id,
            downloader_id: request.downloader_id,
            site_ids: request.site_ids.clone(),
            save_path: request.save_path.clone(),
            enabled: request.enabled,
        };
        if let Some(plan) = target_plan {
            let settings = self.db.get_media_settings().await?;
            let now = Utc::now();
            let current = plan
                .targets
                .iter()
                .find(|target| target.status != SubscriptionTargetSeedStatus::Skipped)
                .ok_or_else(|| {
                    MediaServiceError::Invalid(
                        "TMDB season has no episode at or after the requested start".to_string(),
                    )
                })?;
            let readiness = target_readiness(
                current.status,
                current.air_date.as_deref(),
                plan.terminal,
                now,
            );
            let scheduled_at = next_run_at(
                &readiness,
                now,
                Duration::minutes(settings.scan_interval_mins as i64),
            )?
            .to_rfc3339();
            let initial_status = readiness_status(&readiness);
            self.db
                .create_subscription_with_targets(
                    &subscription,
                    &plan.targets,
                    Some(&scheduled_at),
                    Some(initial_status),
                )
                .await
                .map_err(Into::into)
        } else {
            self.db
                .create_subscription(&subscription)
                .await
                .map_err(Into::into)
        }
    }

    pub async fn search_resources(
        &self,
        request: &ResourceSearchRequest,
    ) -> Result<ResourceSearchResponse, MediaServiceError> {
        self.search_resources_internal(request, true).await
    }

    async fn search_resources_uncached(
        &self,
        request: &ResourceSearchRequest,
    ) -> Result<ResourceSearchResponse, MediaServiceError> {
        self.search_resources_internal(request, false).await
    }

    async fn search_resources_internal(
        &self,
        request: &ResourceSearchRequest,
        cache_candidates: bool,
    ) -> Result<ResourceSearchResponse, MediaServiceError> {
        let media_settings = self.db.get_media_settings().await?;
        let profile = match (request.target.as_ref(), request.quality_profile_id) {
            (Some(_), id) => Some(
                self.db
                    .get_quality_profile(id.unwrap_or(1))
                    .await?
                    .ok_or_else(|| {
                        MediaServiceError::NotFound(format!("quality profile {}", id.unwrap_or(1)))
                    })?,
            ),
            (None, _) => None,
        };
        let (mut queries, fallback_queries) = if let Some(query) = request
            .query
            .as_deref()
            .map(str::trim)
            .filter(|query| !query.is_empty())
        {
            (vec![query.to_string()], Vec::new())
        } else if let Some(target) = &request.target {
            let plan = QueryGenerator::new(media_settings.max_search_queries)
                .generate_plan(&SearchCriteria::from(target));
            (
                plan.primary.into_iter().map(|query| query.query).collect(),
                plan.fallback.into_iter().map(|query| query.query).collect(),
            )
        } else {
            return Err(MediaServiceError::Invalid(
                "query or target is required".to_string(),
            ));
        };
        let page_size = request.page_size.unwrap_or(30).clamp(1, 100);
        let search_requests: Vec<_> = queries
            .iter()
            .map(|query| SearchRequest {
                query: query.clone(),
                page: 1,
                page_size,
            })
            .collect();
        let (indexers, mut setup_errors, priorities) =
            self.resolve_indexers(&request.site_ids).await?;
        let aggregator = IndexerAggregator::new(media_settings.search_concurrency);
        let mut aggregate = aggregator.search(&indexers, &search_requests).await;

        let should_fallback =
            !fallback_queries.is_empty()
                && aggregate.successful_requests > 0
                && request.target.as_ref().zip(profile.as_ref()).is_some_and(
                    |(target, profile)| {
                        !self.has_accepted_search_result(&aggregate.results, target, profile)
                    },
                );
        if should_fallback {
            let fallback_requests: Vec<_> = fallback_queries
                .into_iter()
                .map(|query| SearchRequest {
                    query,
                    page: 1,
                    page_size,
                })
                .collect();
            queries.extend(
                fallback_requests
                    .iter()
                    .map(|request| request.query.clone()),
            );
            let fallback = aggregator.search(&indexers, &fallback_requests).await;
            aggregate = aggregate.merge(fallback);
        }
        setup_errors.extend(aggregate.errors.clone());
        let candidates = self
            .finalize_resource_candidates(
                aggregate.results,
                request,
                profile.as_ref(),
                &priorities,
                cache_candidates,
            )
            .await?;
        Ok(ResourceSearchResponse {
            queries,
            candidates,
            errors: setup_errors,
            total_sites: aggregate.total_sites,
            successful_sites: aggregate.successful_sites,
        })
    }

    fn has_accepted_search_result(
        &self,
        results: &[SearchResult],
        target: &MediaTarget,
        profile: &QualityProfileRecord,
    ) -> bool {
        let profile = profile_to_domain(profile);
        results.iter().any(|result| {
            self.parser.parse(&result.title).is_ok_and(|release| {
                DecisionEngine::evaluate(target, &release, &profile, result.seeders).accepted
            })
        })
    }

    async fn finalize_resource_candidates(
        &self,
        results: Vec<SearchResult>,
        request: &ResourceSearchRequest,
        profile: Option<&QualityProfileRecord>,
        priorities: &HashMap<i64, u32>,
        cache_candidates: bool,
    ) -> Result<Vec<ResourceCandidate>, MediaServiceError> {
        let domain_profile = profile.map(profile_to_domain);
        let mut candidates = Vec::with_capacity(results.len().min(RESOURCE_SEARCH_RESULT_LIMIT));
        for result in results {
            match self.parser.parse(&result.title) {
                Ok(release) => {
                    let decision = request.target.as_ref().zip(domain_profile.as_ref()).map(
                        |(target, profile)| {
                            DecisionEngine::evaluate(target, &release, profile, result.seeders)
                        },
                    );
                    let sort_key = decision
                        .as_ref()
                        .zip(request.target.as_ref())
                        .zip(domain_profile.as_ref())
                        .map(|((decision, target), profile)| {
                            SortKey::from_candidate(
                                decision,
                                profile,
                                target,
                                &release,
                                result.size,
                                result.seeders,
                                result.publish_time,
                                priorities.get(&result.site_id).copied().unwrap_or(u32::MAX),
                                stable_release_key(
                                    &result.source_site,
                                    &result.torrent_id,
                                    &result.title,
                                ),
                            )
                        });
                    candidates.push(ResourceCandidate {
                        candidate_id: String::new(),
                        result,
                        release: Some(release),
                        parse_error: None,
                        decision,
                        sort_key,
                    });
                }
                Err(error) => candidates.push(ResourceCandidate {
                    candidate_id: String::new(),
                    result,
                    release: None,
                    parse_error: Some(error.to_string()),
                    decision: None,
                    sort_key: None,
                }),
            }
        }

        candidates.sort_by(compare_candidates);
        candidates.truncate(RESOURCE_SEARCH_RESULT_LIMIT);
        for candidate in &mut candidates {
            candidate.candidate_id = new_candidate_id()?;
        }

        if cache_candidates {
            let mut cache = self.candidate_cache.lock().await;
            for candidate in &candidates {
                cache.insert(
                    candidate.candidate_id.clone(),
                    candidate.result.clone(),
                    request.target.clone(),
                    profile.map(|profile| profile.id),
                );
            }
        }
        Ok(candidates)
    }

    pub async fn run_subscription(
        &self,
        subscription_id: i64,
    ) -> Result<SubscriptionRunResult, MediaServiceError> {
        let owner = process_owner_id("manual-subscription");
        let record = self
            .db
            .claim_subscription(subscription_id, &owner, SUBSCRIPTION_LEASE_SECONDS)
            .await?
            .ok_or_else(|| {
                MediaServiceError::Conflict(
                    "subscription is already running or does not exist".to_string(),
                )
            })?;
        let service = self.clone();
        let task_owner = owner.clone();
        let shutdown = crate::runtime_shutdown_token();
        let task = tokio::spawn(
            async move {
                tokio::select! {
                    biased;
                    _ = shutdown.cancelled() => {
                        service.db.recover_subscription_leases_for_owners(&[task_owner.clone()]).await?;
                        Err(MediaServiceError::App(AppError::Server {
                            message: "manual subscription interrupted by server shutdown".into(),
                        }))
                    }
                    result = service.run_claimed_subscription(record, &task_owner, SUBSCRIPTION_LEASE_SECONDS) => result,
                }
            }
            .in_current_span(),
        );
        match task.await {
            Ok(result) => result,
            Err(error) => {
                let _ = self
                    .db
                    .recover_subscription_leases_for_owners(&[owner])
                    .await;
                Err(MediaServiceError::App(AppError::Server {
                    message: format!("manual subscription scan task terminated: {error}"),
                }))
            }
        }
    }

    pub async fn get_subscription_last_run(
        &self,
        subscription_id: i64,
    ) -> Result<Option<SubscriptionRunSnapshot>, MediaServiceError> {
        self.db
            .get_subscription_last_run_info(subscription_id)
            .await?
            .map(|info| {
                serde_json::from_str(&info).map_err(|error| {
                    MediaServiceError::Serialization(format!(
                        "invalid subscription run snapshot: {error}"
                    ))
                })
            })
            .transpose()
    }

    pub async fn run_claimed_subscription(
        &self,
        subscription: SubscriptionRecord,
        owner: &str,
        lease_seconds: i64,
    ) -> Result<SubscriptionRunResult, MediaServiceError> {
        let subscription_id = subscription.id;
        let claimed_target_key = subscription_target(&subscription)?.target_key();
        let started_at = Utc::now().to_rfc3339();
        let result = self
            .run_claimed_subscription_inner(subscription, owner, lease_seconds, &started_at)
            .await;
        if let Err(error) = &result {
            let current_snapshot = self
                .get_subscription_last_run(subscription_id)
                .await
                .ok()
                .flatten();
            let snapshot = match current_snapshot {
                Some(mut snapshot) if snapshot.started_at == started_at => {
                    snapshot.finished_at = Utc::now().to_rfc3339();
                    snapshot.error = Some(error.to_string());
                    snapshot
                }
                _ => failed_run_snapshot(&started_at, &claimed_target_key, error.to_string()),
            };
            if let Ok(info) = to_json(&snapshot) {
                let _ = self
                    .db
                    .save_claimed_subscription_last_run_info(subscription_id, owner, &info)
                    .await;
            }
            let retry_minutes = self
                .db
                .get_media_settings()
                .await
                .map(|settings| settings.scan_interval_mins as i64)
                .unwrap_or(30)
                .max(1);
            let next_run_at = (Utc::now() + Duration::minutes(retry_minutes)).to_rfc3339();
            let _ = self
                .db
                .release_claimed_subscription_after_error(
                    subscription_id,
                    owner,
                    &claimed_target_key,
                    &next_run_at,
                    &error.to_string(),
                )
                .await;
        }
        result
    }

    async fn run_claimed_subscription_inner(
        &self,
        mut subscription: SubscriptionRecord,
        owner: &str,
        lease_seconds: i64,
        started_at: &str,
    ) -> Result<SubscriptionRunResult, MediaServiceError> {
        let settings = self.db.get_media_settings().await?;
        let scan_interval = Duration::minutes(settings.scan_interval_mins as i64);
        let fallback_next_run_at = (Utc::now() + scan_interval).to_rfc3339();
        let prepared = match self
            .prepare_claimed_subscription_target(
                &mut subscription,
                owner,
                scan_interval,
                started_at,
            )
            .await
        {
            Ok(prepared) => prepared,
            Err(error) => {
                let snapshot = failed_run_snapshot(
                    started_at,
                    &subscription_target(&subscription)?.target_key(),
                    error.to_string(),
                );
                self.save_claimed_run_snapshot(subscription.id, owner, &snapshot)
                    .await?;
                let _ = self
                    .db
                    .finish_subscription_scan(
                        subscription.id,
                        subscription.version,
                        owner,
                        &fallback_next_run_at,
                        "error",
                        Some(&error.to_string()),
                    )
                    .await;
                return Err(error);
            }
        };
        let mut target = match prepared {
            PreparedSubscriptionTarget::Due(target) => target,
            PreparedSubscriptionTarget::Deferred {
                target_key,
                next_run_at,
                status,
            } => {
                let snapshot = empty_run_snapshot(started_at, &target_key);
                self.save_claimed_run_snapshot(subscription.id, owner, &snapshot)
                    .await?;
                if !self
                    .db
                    .finish_subscription_scan(
                        subscription.id,
                        subscription.version,
                        owner,
                        &next_run_at,
                        status,
                        None,
                    )
                    .await?
                {
                    return Err(MediaServiceError::Conflict(
                        "subscription lease expired while deferring the scan".to_string(),
                    ));
                }
                return Ok(empty_subscription_run(subscription.id, target_key));
            }
            PreparedSubscriptionTarget::Completed { target_key } => {
                return Ok(empty_subscription_run(subscription.id, target_key));
            }
        };
        let profile = self
            .db
            .get_quality_profile(subscription.quality_profile_id)
            .await?
            .ok_or_else(|| {
                MediaServiceError::NotFound(format!(
                    "quality profile {}",
                    subscription.quality_profile_id
                ))
            })?;
        let (search, mut lease_version) = self
            .search_resources_with_subscription_heartbeat(
                &subscription,
                owner,
                lease_seconds,
                ResourceSearchRequest {
                    query: None,
                    site_ids: subscription.site_ids.clone(),
                    target: Some(target.clone()),
                    quality_profile_id: Some(profile.id),
                    page_size: Some(50),
                },
            )
            .await?;
        let next_run_at = (Utc::now() + scan_interval).to_rfc3339();

        let mut search = match search {
            Ok(search) => search,
            Err(error) => {
                let snapshot =
                    failed_run_snapshot(started_at, &target.target_key(), error.to_string());
                self.save_claimed_run_snapshot(subscription.id, owner, &snapshot)
                    .await?;
                let _ = self
                    .db
                    .finish_subscription_scan(
                        subscription.id,
                        lease_version,
                        owner,
                        &next_run_at,
                        "error",
                        Some(&error.to_string()),
                    )
                    .await;
                return Err(error);
            }
        };
        lease_version = self
            .refresh_aliases_after_title_only_rejection(
                &mut subscription,
                owner,
                lease_version,
                &mut target,
                &mut search,
                &profile,
            )
            .await?;
        let accepted_count = search
            .candidates
            .iter()
            .filter(|candidate| candidate.decision.as_ref().is_some_and(|d| d.accepted))
            .count();
        let best = search.candidates.iter().find(|candidate| {
            candidate
                .decision
                .as_ref()
                .is_some_and(|decision| decision.accepted)
        });
        let snapshot = SubscriptionRunSnapshot {
            started_at: started_at.to_string(),
            finished_at: Utc::now().to_rfc3339(),
            target_key: target.target_key(),
            queries: search.queries.clone(),
            candidates: search.candidates.clone(),
            site_errors: search.errors.clone(),
            total_sites: search.total_sites,
            successful_sites: search.successful_sites,
            best_candidate_id: best.map(|candidate| candidate.candidate_id.clone()),
            error: None,
        };
        self.save_claimed_run_snapshot(subscription.id, owner, &snapshot)
            .await?;
        let download = if let Some(best) = best {
            let downloader = self
                .db
                .get_downloader(subscription.downloader_id)
                .await?
                .ok_or_else(|| {
                    MediaServiceError::NotFound(format!(
                        "downloader {}",
                        subscription.downloader_id
                    ))
                })?;
            let decision = best
                .decision
                .as_ref()
                .expect("accepted candidate has decision");
            let target_key = target.target_key();
            let request = NewMediaDownload {
                subscription_id: Some(subscription.id),
                target_key: target_key.clone(),
                dedupe_key: format!("subscription:{}:{target_key}", subscription.id),
                site_id: Some(best.result.site_id),
                downloader_id: Some(subscription.downloader_id),
                source_site: best.result.source_site.clone(),
                downloader_name: downloader.name,
                torrent_id: best.result.torrent_id.clone(),
                title: best.result.title.clone(),
                size: best.result.size,
                release_json: to_json(&best.result)?,
                decision_json: to_json(decision)?,
                profile_snapshot_json: to_json(&profile)?,
            };
            let download = self
                .db
                .enqueue_claimed_subscription_media_download(lease_version, owner, &request)
                .await?
                .ok_or_else(|| {
                    MediaServiceError::Conflict(
                        "subscription lease or target changed before queueing".to_string(),
                    )
                })?;
            Some(download)
        } else {
            None
        };
        let status = if download.is_some() {
            "queued"
        } else {
            "waiting"
        };
        let error_summary = if download.is_none() && !search.errors.is_empty() {
            Some(
                search
                    .errors
                    .iter()
                    .map(|error| format!("{}: {}", error.source_site, error.message))
                    .take(3)
                    .collect::<Vec<_>>()
                    .join("; "),
            )
        } else {
            None
        };
        if !self
            .db
            .finish_subscription_scan(
                subscription.id,
                lease_version,
                owner,
                &next_run_at,
                status,
                error_summary.as_deref(),
            )
            .await?
        {
            return Err(MediaServiceError::Conflict(
                "subscription lease expired while finishing the scan".to_string(),
            ));
        }
        Ok(SubscriptionRunResult {
            subscription_id: subscription.id,
            target_key: target.target_key(),
            query_count: search.queries.len(),
            candidate_count: search.candidates.len(),
            accepted_count,
            download,
            site_errors: search.errors,
        })
    }

    async fn save_claimed_run_snapshot(
        &self,
        subscription_id: i64,
        owner: &str,
        snapshot: &SubscriptionRunSnapshot,
    ) -> Result<(), MediaServiceError> {
        let info = to_json(snapshot)?;
        if self
            .db
            .save_claimed_subscription_last_run_info(subscription_id, owner, &info)
            .await?
        {
            Ok(())
        } else {
            Err(MediaServiceError::Conflict(
                "subscription lease expired while saving run details".to_string(),
            ))
        }
    }

    async fn refresh_aliases_after_title_only_rejection(
        &self,
        subscription: &mut SubscriptionRecord,
        owner: &str,
        lease_version: i64,
        target: &mut MediaTarget,
        search: &mut ResourceSearchResponse,
        profile: &QualityProfileRecord,
    ) -> Result<i64, MediaServiceError> {
        let title_only_rejection = search.candidates.iter().any(|candidate| {
            candidate.decision.as_ref().is_some_and(|decision| {
                !decision.accepted
                    && decision.rejections.len() == 1
                    && decision.rejections[0].code == RejectCode::WrongTitle
            })
        });
        if !title_only_rejection {
            return Ok(lease_version);
        }

        let media_type = TmdbMediaType::parse(&subscription.media_type)?;
        let details = match self.tmdb_details(subscription.tmdb_id, media_type).await {
            Ok(details) => details,
            Err(error) => {
                warn!(
                    subscription_id = subscription.id,
                    %error,
                    "could not refresh TMDB aliases after title-only rejection"
                );
                return Ok(lease_version);
            }
        };
        let mut aliases = subscription.aliases.clone();
        push_case_insensitive(&mut aliases, &details.media.title);
        if let Some(original_title) = &details.media.original_title {
            push_case_insensitive(&mut aliases, original_title);
        }
        for alias in &details.aliases {
            push_case_insensitive(&mut aliases, alias);
        }
        if aliases.len() == subscription.aliases.len() {
            return Ok(lease_version);
        }

        subscription.aliases = aliases;
        let refreshed_target = subscription_target(subscription)?;
        reevaluate_resource_candidates(&refreshed_target, &mut search.candidates, profile);
        *target = refreshed_target;

        let refreshed_version = self
            .db
            .refresh_claimed_subscription_aliases(
                subscription.id,
                lease_version,
                owner,
                &subscription.aliases,
            )
            .await?
            .ok_or_else(|| {
                MediaServiceError::Conflict(
                    "subscription lease changed while refreshing TMDB aliases".to_string(),
                )
            })?;
        subscription.version = refreshed_version;
        Ok(refreshed_version)
    }

    async fn prepare_claimed_subscription_target(
        &self,
        subscription: &mut SubscriptionRecord,
        owner: &str,
        scan_interval: Duration,
        started_at: &str,
    ) -> Result<PreparedSubscriptionTarget, MediaServiceError> {
        let target = subscription_target(subscription)?;
        let target_key = target.target_key();
        let now = Utc::now();
        let mut target_record = self
            .db
            .get_subscription_target(subscription.id, &target_key)
            .await?;
        let mut terminal = subscription.media_type == "movie"
            || target_record
                .as_ref()
                .is_some_and(|record| record.status == "skipped");

        if subscription.media_type == "tv"
            && target_needs_tmdb_refresh(
                target_record.as_ref(),
                subscription.last_status.as_deref(),
                now,
            )
        {
            let season_number = subscription.season.ok_or_else(|| {
                MediaServiceError::Invalid("TV subscription has no season".to_string())
            })?;
            let start_episode = subscription.start_episode.unwrap_or(1);
            let absolute_anchor = subscription_absolute_anchor(subscription)?;
            let client = self.tmdb_client().await?;
            let (details, season) = tokio::try_join!(
                client.details(subscription.tmdb_id, TmdbMediaType::Tv),
                client.season(subscription.tmdb_id, season_number),
            )?;
            let plan = plan_tv_targets(&details, &season, start_episode, absolute_anchor)?;
            terminal = plan.terminal;
            let synced = self
                .db
                .sync_claimed_subscription_targets(
                    subscription.id,
                    subscription.version,
                    owner,
                    &plan.targets,
                    details.media.is_animation,
                    &details.media.genres,
                )
                .await?
                .ok_or_else(|| {
                    MediaServiceError::Conflict(
                        "subscription changed while refreshing TMDB targets".to_string(),
                    )
                })?;
            subscription.version = synced.version;
            subscription.tmdb_is_animation = details.media.is_animation;
            subscription.tmdb_genres = details.media.genres;
            target_record = synced.current;
        }

        let target_record = target_record.ok_or_else(|| {
            MediaServiceError::Conflict(format!(
                "subscription target {target_key} is missing after metadata refresh"
            ))
        })?;
        match target_record.status.as_str() {
            "skipped" => {
                if !terminal {
                    return Err(MediaServiceError::Conflict(
                        "a non-terminal season has a skipped current target".to_string(),
                    ));
                }
                let snapshot = empty_run_snapshot(started_at, &target_key);
                self.save_claimed_run_snapshot(subscription.id, owner, &snapshot)
                    .await?;
                if !self
                    .db
                    .complete_claimed_subscription(
                        subscription.id,
                        subscription.version,
                        owner,
                        Some(&target_key),
                    )
                    .await?
                {
                    return Err(MediaServiceError::Conflict(
                        "subscription changed while completing the season".to_string(),
                    ));
                }
                Ok(PreparedSubscriptionTarget::Completed { target_key })
            }
            "queued" => Ok(PreparedSubscriptionTarget::Deferred {
                target_key,
                next_run_at: (now + scan_interval).to_rfc3339(),
                status: "queued",
            }),
            "metadata_pending" => Ok(PreparedSubscriptionTarget::Deferred {
                target_key,
                next_run_at: next_run_at(&TargetReadiness::AwaitingMetadata, now, scan_interval)?
                    .to_rfc3339(),
                status: "awaiting_metadata",
            }),
            "pending" => {
                let readiness = target_readiness(
                    SubscriptionTargetSeedStatus::Pending,
                    target_record.air_date.as_deref(),
                    terminal,
                    now,
                );
                match readiness {
                    TargetReadiness::Due => Ok(PreparedSubscriptionTarget::Due(target)),
                    TargetReadiness::Future(_) => Ok(PreparedSubscriptionTarget::Deferred {
                        target_key,
                        next_run_at: next_run_at(&readiness, now, scan_interval)?.to_rfc3339(),
                        status: "waiting_air_date",
                    }),
                    TargetReadiness::AwaitingMetadata => Ok(PreparedSubscriptionTarget::Deferred {
                        target_key,
                        next_run_at: next_run_at(&readiness, now, scan_interval)?.to_rfc3339(),
                        status: "awaiting_metadata",
                    }),
                }
            }
            "submitted" => Err(MediaServiceError::Conflict(
                "subscription cursor still points at a submitted target".to_string(),
            )),
            status => Err(MediaServiceError::Invalid(format!(
                "unsupported subscription target status: {status}"
            ))),
        }
    }

    async fn search_resources_with_subscription_heartbeat(
        &self,
        subscription: &SubscriptionRecord,
        owner: &str,
        lease_seconds: i64,
        request: ResourceSearchRequest,
    ) -> Result<(Result<ResourceSearchResponse, MediaServiceError>, i64), MediaServiceError> {
        let lease_seconds = lease_seconds.max(10);
        let heartbeat_seconds = (lease_seconds / 3).max(1) as u64;
        let mut version = subscription.version;
        let search = self.search_resources_uncached(&request);
        tokio::pin!(search);

        loop {
            tokio::select! {
                result = &mut search => return Ok((result, version)),
                _ = tokio::time::sleep(std::time::Duration::from_secs(heartbeat_seconds)) => {
                    version = self.db
                        .renew_subscription_lease(
                            subscription.id,
                            version,
                            owner,
                            lease_seconds,
                        )
                        .await?
                        .ok_or_else(|| MediaServiceError::Conflict(
                            "subscription lease changed during the scan".to_string()
                        ))?;
                }
            }
        }
    }

    pub async fn queue_download(
        &self,
        request: &QueueDownloadRequest,
    ) -> Result<MediaDownloadRecord, MediaServiceError> {
        let candidate_id = request.candidate_id.trim();
        if candidate_id.is_empty() {
            return Err(MediaServiceError::Invalid(
                "candidate_id is required".to_string(),
            ));
        }
        let cached = self
            .candidate_cache
            .lock()
            .await
            .get(candidate_id)
            .ok_or_else(|| {
                MediaServiceError::Invalid(
                    "candidate_id is unknown or expired; search again".to_string(),
                )
            })?;
        let override_reason = request
            .override_reason
            .as_deref()
            .map(str::trim)
            .filter(|reason| !reason.is_empty());
        let mut result = cached.result;
        let (
            quality_profile_id,
            downloader_id,
            target_key,
            dedupe_key,
            decision,
            subscription_version,
        ) = if let Some(subscription_id) = request.subscription_id {
            let subscription = self
                .db
                .get_subscription(subscription_id)
                .await?
                .ok_or_else(|| {
                    MediaServiceError::NotFound(format!("subscription {subscription_id}"))
                })?;
            if request.quality_profile_id != subscription.quality_profile_id
                || request.downloader_id != subscription.downloader_id
            {
                return Err(MediaServiceError::Invalid(
                    "quality profile and downloader must match the subscription".to_string(),
                ));
            }
            if !subscription.site_ids.contains(&result.site_id) {
                return Err(MediaServiceError::Invalid(
                    "download site is not configured for the subscription".to_string(),
                ));
            }
            let target = subscription_target(&subscription)?;
            if cached
                .target
                .as_ref()
                .map(MediaTarget::target_key)
                .as_deref()
                != Some(target.target_key().as_str())
                || cached.quality_profile_id != Some(subscription.quality_profile_id)
            {
                return Err(MediaServiceError::Invalid(
                        "candidate was not issued for the subscription's current target and quality profile"
                            .to_string(),
                    ));
            }
            let profile = self
                .db
                .get_quality_profile(subscription.quality_profile_id)
                .await?
                .ok_or_else(|| {
                    MediaServiceError::NotFound(format!(
                        "quality profile {}",
                        subscription.quality_profile_id
                    ))
                })?;
            let release = self.parser.parse(&result.title).map_err(|error| {
                MediaServiceError::Invalid(format!(
                    "release title could not be matched to the subscription: {error}"
                ))
            })?;
            let decision = DecisionEngine::evaluate(
                &target,
                &release,
                &profile_to_domain(&profile),
                result.seeders,
            );
            if !decision.accepted && override_reason.is_none() {
                return Err(MediaServiceError::Invalid(
                    "override_reason is required for a rejected candidate".to_string(),
                ));
            }
            let target_key = target.target_key();
            (
                subscription.quality_profile_id,
                subscription.downloader_id,
                target_key.clone(),
                format!("subscription:{subscription_id}:{target_key}"),
                Some(decision),
                Some(subscription.version),
            )
        } else {
            if cached
                .quality_profile_id
                .is_some_and(|profile_id| profile_id != request.quality_profile_id)
            {
                return Err(MediaServiceError::Invalid(
                    "quality profile does not match the candidate search context".to_string(),
                ));
            }
            let decision = if let Some(target) = cached.target.as_ref() {
                let profile = self
                    .db
                    .get_quality_profile(request.quality_profile_id)
                    .await?
                    .ok_or_else(|| {
                        MediaServiceError::NotFound(format!(
                            "quality profile {}",
                            request.quality_profile_id
                        ))
                    })?;
                let release = self.parser.parse(&result.title).map_err(|error| {
                    MediaServiceError::Invalid(format!(
                        "release title could not be matched to the search target: {error}"
                    ))
                })?;
                Some(DecisionEngine::evaluate(
                    target,
                    &release,
                    &profile_to_domain(&profile),
                    result.seeders,
                ))
            } else {
                None
            };
            if decision.as_ref().is_some_and(|decision| !decision.accepted)
                && override_reason.is_none()
            {
                return Err(MediaServiceError::Invalid(
                    "override_reason is required for a rejected candidate".to_string(),
                ));
            }
            let target_key = cached.target.as_ref().map_or_else(
                || format!("manual:{}:{}", result.site_id, result.torrent_id),
                MediaTarget::target_key,
            );
            (
                request.quality_profile_id,
                request.downloader_id,
                target_key,
                format!(
                    "manual:{}:{}:{}",
                    result.site_id, result.torrent_id, request.downloader_id
                ),
                decision,
                None,
            )
        };

        self.ensure_references(quality_profile_id, downloader_id, &[result.site_id])
            .await?;
        let profile = self
            .db
            .get_quality_profile(quality_profile_id)
            .await?
            .ok_or_else(|| {
                MediaServiceError::NotFound(format!("quality profile {quality_profile_id}"))
            })?;
        let downloader = self
            .db
            .get_downloader(downloader_id)
            .await?
            .ok_or_else(|| MediaServiceError::NotFound(format!("downloader {downloader_id}")))?;
        let site = self
            .db
            .get_site(result.site_id)
            .await?
            .ok_or_else(|| MediaServiceError::NotFound(format!("site {}", result.site_id)))?;
        result.source_site = site.name;
        let decision_json = serde_json::json!({
            "decision": decision,
            "override_reason": override_reason,
        });
        let new_download = NewMediaDownload {
            subscription_id: request.subscription_id,
            target_key,
            dedupe_key,
            site_id: Some(result.site_id),
            downloader_id: Some(downloader_id),
            source_site: result.source_site.clone(),
            downloader_name: downloader.name.clone(),
            torrent_id: result.torrent_id.clone(),
            title: result.title.clone(),
            size: result.size,
            release_json: to_json(&result)?,
            decision_json: decision_json.to_string(),
            profile_snapshot_json: to_json(&profile)?,
        };
        if let Some(expected_version) = subscription_version {
            return self
                .db
                .enqueue_subscription_media_download(expected_version, &new_download)
                .await?
                .ok_or_else(|| {
                    MediaServiceError::Conflict(
                        "subscription target is no longer ready to queue".to_string(),
                    )
                });
        }
        let queued = self
            .db
            .enqueue_media_download(&new_download)
            .await
            .map_err(MediaServiceError::from)?;
        if queued.status == "submitted"
            && let Some(infohash) = queued.infohash.as_deref()
        {
            let client = self
                .downloaders
                .get(&downloader)
                .await
                .map_err(MediaServiceError::Downloader)?;
            let exists = !client
                .list_torrents_by_hashes(&[infohash.to_string()])
                .await
                .map_err(MediaServiceError::Downloader)?
                .is_empty();
            if !exists
                && self
                    .db
                    .requeue_missing_manual_media_download(queued.id, queued.version)
                    .await?
            {
                return self
                    .db
                    .get_media_download(queued.id)
                    .await?
                    .ok_or_else(|| MediaServiceError::NotFound(format!("download {}", queued.id)));
            }
        }
        Ok(queued)
    }

    pub async fn redeliver_download(
        &self,
        download_id: i64,
    ) -> Result<MediaDownloadRecord, MediaServiceError> {
        let download = self
            .db
            .get_media_download(download_id)
            .await?
            .ok_or_else(|| MediaServiceError::NotFound(format!("download {download_id}")))?;
        if download.status != "submitted" {
            return Err(MediaServiceError::Conflict(
                "only submitted downloads can be verified or redelivered".to_string(),
            ));
        }
        let infohash = download.infohash.clone().ok_or_else(|| {
            MediaServiceError::Conflict("submitted download has no infohash".to_string())
        })?;
        if !is_valid_infohash(&infohash) {
            return Err(MediaServiceError::Conflict(
                "submitted download has an invalid infohash".to_string(),
            ));
        }
        let downloader_id = download.downloader_id.ok_or_else(|| {
            MediaServiceError::NotFound("download client was deleted".to_string())
        })?;
        let lease_owner = format!("redelivery:{}:{}", download.id, new_candidate_id()?);
        if !self
            .db
            .reserve_media_download_redelivery(
                download.id,
                download.version,
                downloader_id,
                &infohash,
                &lease_owner,
                DOWNLOAD_LEASE_SECONDS,
            )
            .await?
        {
            self.ensure_download_identity_not_relocating(download.id, downloader_id, &infohash)
                .await?;
            return Err(MediaServiceError::Conflict(
                "download changed or is already being verified; reload and try again".to_string(),
            ));
        }

        let result = async {
            let downloader = self
                .db
                .get_downloader(downloader_id)
                .await?
                .ok_or_else(|| {
                    MediaServiceError::NotFound(format!("downloader {downloader_id}"))
                })?;
            let client = self
                .downloaders
                .get(&downloader)
                .await
                .map_err(MediaServiceError::Downloader)?;
            if torrent_is_present(client.as_ref(), &infohash)
                .await
                .map_err(MediaServiceError::Downloader)?
            {
                return Ok(download.clone());
            }

            let (search_result, torrent) = self.fetch_torrent_for_download(&download).await?;
            let fetched_infohash = torrent_infohash(&torrent)?;
            if !fetched_infohash.eq_ignore_ascii_case(&infohash) {
                return Err(MediaServiceError::Conflict(
                    "the stored torrent locator now returns different torrent content".to_string(),
                ));
            }
            let options = self.add_torrent_options(&download).await?;
            let filename = media_download_filename(&search_result);
            if !self
                .db
                .renew_media_download_redelivery(
                    download.id,
                    download.version,
                    downloader_id,
                    &infohash,
                    &lease_owner,
                    DOWNLOAD_LEASE_SECONDS,
                )
                .await?
            {
                return Err(MediaServiceError::Conflict(
                    "redelivery reservation expired or a migration started; qBittorrent submission was not attempted"
                        .to_string(),
                ));
            }
            match add_torrent_and_confirm(client.as_ref(), torrent, &filename, &options, &infohash)
                .await
            {
                Ok(()) => Ok(download.clone()),
                Err(TorrentSubmissionError::ConfirmedAbsent(message)) => {
                    Err(MediaServiceError::Downloader(message))
                }
                Err(TorrentSubmissionError::Unknown(message)) => Err(
                    MediaServiceError::Downloader(format!("submission result is unknown: {message}")),
                ),
            }
        }
        .await;
        if !self
            .db
            .release_media_download_redelivery(download.id, download.version, &lease_owner)
            .await?
        {
            return Err(MediaServiceError::Conflict(
                "download changed while releasing the redelivery reservation".to_string(),
            ));
        }
        result
    }

    pub async fn delete_download_record(
        &self,
        download_id: i64,
        expected_version: i64,
    ) -> Result<MediaDownloadDeletion, MediaServiceError> {
        match self
            .db
            .delete_media_download_record(download_id, expected_version)
            .await?
        {
            MediaDownloadDeleteOutcome::Deleted {
                download,
                target_reopened,
            } => Ok(MediaDownloadDeletion {
                deleted_id: download.id,
                subscription_id: download.subscription_id,
                target_reopened,
            }),
            MediaDownloadDeleteOutcome::NotFound => Err(MediaServiceError::NotFound(format!(
                "download {download_id}"
            ))),
            MediaDownloadDeleteOutcome::VersionChanged => Err(MediaServiceError::Conflict(
                "download record changed; reload and try again".to_string(),
            )),
            MediaDownloadDeleteOutcome::DownloadActive => Err(MediaServiceError::Conflict(
                "download record is still being processed and cannot be deleted".to_string(),
            )),
            MediaDownloadDeleteOutcome::SubscriptionActive => Err(MediaServiceError::Conflict(
                "subscription has active scan or download work; retry after it finishes"
                    .to_string(),
            )),
            MediaDownloadDeleteOutcome::RelocationActive => Err(MediaServiceError::Conflict(
                "the related OpenList copy or qB migration is still active; resolve it before deleting the record"
                    .to_string(),
            )),
        }
    }

    pub async fn reconcile_failed_download(
        &self,
        download_id: i64,
        expected_version: i64,
    ) -> Result<FailedDownloadReconciliation, MediaServiceError> {
        let download = self
            .db
            .get_media_download(download_id)
            .await?
            .ok_or_else(|| MediaServiceError::NotFound(format!("download {download_id}")))?;
        if download.version != expected_version {
            return Err(MediaServiceError::Conflict(
                "download record changed; reload and try again".to_string(),
            ));
        }
        if download.status != "failed" {
            return Err(MediaServiceError::Conflict(
                "only failed downloads with an unresolved qB submission can be reconciled"
                    .to_string(),
            ));
        }
        let subscription_id = download.subscription_id.ok_or_else(|| {
            MediaServiceError::Conflict(
                "only subscription downloads can release an unresolved target".to_string(),
            )
        })?;
        let target = self
            .db
            .get_subscription_target(subscription_id, &download.target_key)
            .await?
            .ok_or_else(|| {
                MediaServiceError::Conflict("subscription target no longer exists".to_string())
            })?;
        if target.status != "queued" {
            return Err(MediaServiceError::Conflict(
                "subscription target is not awaiting qB reconciliation".to_string(),
            ));
        }
        let infohash = download.infohash.clone().ok_or_else(|| {
            MediaServiceError::Conflict(
                "failed download has no infohash to verify in qB".to_string(),
            )
        })?;
        if !is_valid_infohash(&infohash) {
            return Err(MediaServiceError::Conflict(
                "failed download has an invalid infohash".to_string(),
            ));
        }
        let downloader_id = download.downloader_id.ok_or_else(|| {
            MediaServiceError::NotFound("download client was deleted".to_string())
        })?;
        self.ensure_download_identity_not_relocating(download.id, downloader_id, &infohash)
            .await?;
        let downloader = self
            .db
            .get_downloader(downloader_id)
            .await?
            .ok_or_else(|| MediaServiceError::NotFound(format!("downloader {downloader_id}")))?;
        let client = self
            .downloaders
            .get(&downloader)
            .await
            .map_err(MediaServiceError::Downloader)?;
        let torrent_present = !client
            .list_torrents_by_hashes(&[infohash])
            .await
            .map_err(MediaServiceError::Downloader)?
            .is_empty();
        let resolved = self
            .db
            .resolve_verified_failed_media_download(
                download.id,
                download.version,
                downloader_id,
                &downloader.updated_at,
                torrent_present,
            )
            .await?
            .ok_or_else(|| {
                MediaServiceError::Conflict(
                    "download or subscription changed during qB reconciliation".to_string(),
                )
            })?;
        Ok(FailedDownloadReconciliation {
            resolution: if torrent_present {
                "submitted".to_string()
            } else {
                "retry_ready".to_string()
            },
            download: resolved,
        })
    }

    async fn fetch_torrent_for_download(
        &self,
        download: &MediaDownloadRecord,
    ) -> Result<(SearchResult, Vec<u8>), MediaServiceError> {
        let result: SearchResult = serde_json::from_str(&download.release_json)
            .map_err(|error| MediaServiceError::Serialization(error.to_string()))?;
        let site_id = download
            .site_id
            .ok_or_else(|| MediaServiceError::NotFound("download site was deleted".to_string()))?;
        let site = self
            .db
            .get_site(site_id)
            .await?
            .ok_or_else(|| MediaServiceError::NotFound(format!("site {site_id}")))?;
        let proxy = self.db.get_settings().await?.proxy;
        let adapter = self.indexers.get_or_create(&site, proxy.as_deref()).await?;
        let torrent = adapter.fetch_torrent(&result).await?;
        Ok((result, torrent))
    }

    async fn add_torrent_options(
        &self,
        download: &MediaDownloadRecord,
    ) -> Result<AddTorrentOptions, MediaServiceError> {
        let subscription = match download.subscription_id {
            Some(subscription_id) => self.db.get_subscription(subscription_id).await?,
            None => None,
        };
        Ok(AddTorrentOptions {
            save_path: subscription
                .as_ref()
                .and_then(|subscription| subscription.save_path.clone()),
            tags: Some("云母".to_string()),
            category: Some(
                media_download_category(
                    &download.target_key,
                    subscription
                        .as_ref()
                        .is_some_and(|subscription| subscription.tmdb_is_animation),
                )
                .to_string(),
            ),
            ..Default::default()
        })
    }

    async fn ensure_download_identity_not_relocating(
        &self,
        download_id: i64,
        downloader_id: i64,
        infohash: &str,
    ) -> Result<(), MediaServiceError> {
        if self
            .db
            .media_download_identity_has_active_relocation(download_id, downloader_id, infohash)
            .await?
        {
            return Err(MediaServiceError::Conflict(
                "the torrent is being copied or migrated; qBittorrent submission is blocked"
                    .to_string(),
            ));
        }
        Ok(())
    }

    pub async fn process_download(
        &self,
        download: MediaDownloadRecord,
        owner: &str,
    ) -> Result<(), MediaServiceError> {
        let download_id = download.id;
        let result = self.process_download_attempt(download, owner).await;
        if let Err(error) = &result {
            self.db
                .release_media_download_after_error(download_id, owner, &error.to_string(), false)
                .await?;
        }
        result
    }

    async fn process_download_attempt(
        &self,
        download: MediaDownloadRecord,
        owner: &str,
    ) -> Result<(), MediaServiceError> {
        if download.status == "reconciling" {
            return self.reconcile_download(download, owner).await;
        }
        if download.status != "fetching" {
            return Err(MediaServiceError::Conflict(format!(
                "download {} is in unexpected state {}",
                download.id, download.status
            )));
        }
        let coverage = media_download_coverage(&download, &self.parser);
        let (result, torrent) = self.fetch_torrent_for_download(&download).await?;
        let downloader_id = download.downloader_id.ok_or_else(|| {
            MediaServiceError::NotFound("download client was deleted".to_string())
        })?;
        let downloader = self
            .db
            .get_downloader(downloader_id)
            .await?
            .ok_or_else(|| MediaServiceError::NotFound(format!("downloader {downloader_id}")))?;
        let infohash = torrent_infohash(&torrent)?;
        let client = self
            .downloaders
            .get(&downloader)
            .await
            .map_err(MediaServiceError::Downloader)?;
        if torrent_is_present(client.as_ref(), &infohash)
            .await
            .map_err(MediaServiceError::Downloader)?
        {
            if !self
                .db
                .mark_media_download_qb_present(
                    download.id,
                    download.version,
                    owner,
                    &infohash,
                    coverage.clone(),
                )
                .await?
            {
                return Err(MediaServiceError::Conflict(
                    "download lease changed while recording an existing qBittorrent torrent"
                        .to_string(),
                ));
            }
            return Ok(());
        }
        self.ensure_download_identity_not_relocating(download.id, downloader_id, &infohash)
            .await?;
        if let Some(existing) = self
            .db
            .get_media_download_by_infohash(downloader_id, &infohash)
            .await?
        {
            if existing.id != download.id && existing.status == "submitted" {
                let options = self.add_torrent_options(&download).await?;
                let filename = media_download_filename(&result);
                self.ensure_download_identity_not_relocating(download.id, downloader_id, &infohash)
                    .await?;
                match add_torrent_and_confirm(
                    client.as_ref(),
                    torrent,
                    &filename,
                    &options,
                    &infohash,
                )
                .await
                {
                    Ok(()) => {}
                    Err(TorrentSubmissionError::ConfirmedAbsent(message)) => {
                        return Err(MediaServiceError::Downloader(message));
                    }
                    Err(TorrentSubmissionError::Unknown(message)) => {
                        return Err(MediaServiceError::Downloader(format!(
                            "submission result is unknown: {message}"
                        )));
                    }
                }
                if !self
                    .db
                    .mark_media_download_qb_present(
                        download.id,
                        download.version,
                        owner,
                        &infohash,
                        coverage.clone(),
                    )
                    .await?
                {
                    return Err(MediaServiceError::Conflict(
                        "download lease changed while recording a restored qBittorrent torrent"
                            .to_string(),
                    ));
                }
                return Ok(());
            }
        }
        if !self
            .db
            .mark_media_download_submitting(
                download.id,
                download.version,
                owner,
                &infohash,
                DOWNLOAD_LEASE_SECONDS,
            )
            .await?
        {
            self.ensure_download_identity_not_relocating(download.id, downloader_id, &infohash)
                .await?;
            return Err(MediaServiceError::Conflict(
                "download lease changed before qBittorrent submission".to_string(),
            ));
        }
        let submitting = self
            .db
            .get_media_download(download.id)
            .await?
            .ok_or_else(|| MediaServiceError::NotFound(format!("download {}", download.id)))?;
        let options = self.add_torrent_options(&submitting).await?;
        let filename = media_download_filename(&result);
        match add_torrent_and_confirm(client.as_ref(), torrent, &filename, &options, &infohash)
            .await
        {
            Ok(()) => {
                if !self
                    .db
                    .mark_media_download_submitted(
                        submitting.id,
                        submitting.version,
                        owner,
                        &infohash,
                        coverage,
                    )
                    .await?
                {
                    return Err(MediaServiceError::Conflict(
                        "qBittorrent contains the torrent but the outbox state changed".to_string(),
                    ));
                }
                Ok(())
            }
            Err(TorrentSubmissionError::ConfirmedAbsent(message)) => {
                self.db
                    .release_media_download_after_error(submitting.id, owner, &message, true)
                    .await?;
                Err(MediaServiceError::Downloader(message))
            }
            Err(TorrentSubmissionError::Unknown(message)) => {
                let next = (Utc::now()
                    + Duration::seconds(MEDIA_DOWNLOAD_RECONCILIATION_GRACE_SECONDS))
                .to_rfc3339();
                if !self
                    .db
                    .transition_media_download(
                        submitting.id,
                        submitting.version,
                        owner,
                        "submitting",
                        "reconciling",
                        Some(&infohash),
                        Some(&message),
                        Some(&next),
                    )
                    .await?
                {
                    return Err(MediaServiceError::Conflict(
                        "download lease changed while recording an unknown submission".to_string(),
                    ));
                }
                Err(MediaServiceError::Downloader(format!(
                    "submission result is unknown: {message}"
                )))
            }
        }
    }

    async fn reconcile_download(
        &self,
        download: MediaDownloadRecord,
        owner: &str,
    ) -> Result<(), MediaServiceError> {
        let coverage = media_download_coverage(&download, &self.parser);
        let infohash = download.infohash.clone().ok_or_else(|| {
            MediaServiceError::Invalid("reconciling download has no infohash".to_string())
        })?;
        let downloader_id = download.downloader_id.ok_or_else(|| {
            MediaServiceError::NotFound("download client was deleted".to_string())
        })?;
        let downloader = self
            .db
            .get_downloader(downloader_id)
            .await?
            .ok_or_else(|| MediaServiceError::NotFound(format!("downloader {downloader_id}")))?;
        let client = self
            .downloaders
            .get(&downloader)
            .await
            .map_err(MediaServiceError::Downloader)?;
        match torrent_is_present(client.as_ref(), &infohash).await {
            Ok(true) => {
                if !self
                    .db
                    .mark_media_download_qb_present(
                        download.id,
                        download.version,
                        owner,
                        &infohash,
                        coverage,
                    )
                    .await?
                {
                    return Err(MediaServiceError::Conflict(
                        "download lease changed during reconciliation".to_string(),
                    ));
                }
                Ok(())
            }
            Ok(false) => {
                self.ensure_download_identity_not_relocating(download.id, downloader_id, &infohash)
                    .await?;
                self.db
                    .release_media_download_after_error(
                        download.id,
                        owner,
                        "torrent not found during reconciliation",
                        true,
                    )
                    .await?;
                Ok(())
            }
            Err(error) => Err(MediaServiceError::Downloader(error)),
        }
    }

    async fn resolve_indexers(
        &self,
        requested_site_ids: &[i64],
    ) -> Result<
        (
            Vec<Arc<dyn crate::indexer::IndexerAdapter>>,
            Vec<SiteSearchError>,
            HashMap<i64, u32>,
        ),
        MediaServiceError,
    > {
        let all_sites = self.db.list_sites().await?;
        let selected: Vec<_> =
            if requested_site_ids.is_empty() {
                all_sites
            } else {
                let mut by_id: HashMap<_, _> =
                    all_sites.into_iter().map(|site| (site.id, site)).collect();
                let mut seen = HashSet::new();
                let mut selected = Vec::with_capacity(requested_site_ids.len());
                for site_id in requested_site_ids {
                    if !seen.insert(*site_id) {
                        return Err(MediaServiceError::Invalid(
                            "selected PT sites must not contain duplicates".to_string(),
                        ));
                    }
                    selected.push(by_id.remove(site_id).ok_or_else(|| {
                        MediaServiceError::NotFound(format!("PT site {site_id}"))
                    })?);
                }
                selected
            };
        if selected.is_empty() {
            return Err(MediaServiceError::Invalid(
                "no PT sites are configured".to_string(),
            ));
        }
        let proxy = self.db.get_settings().await?.proxy;
        let priorities: HashMap<_, _> = selected
            .iter()
            .enumerate()
            .map(|(priority, site)| (site.id, priority as u32))
            .collect();
        let mut adapters = Vec::new();
        let mut errors = Vec::new();
        for site in selected {
            match self.indexers.get_or_create(&site, proxy.as_deref()).await {
                Ok(adapter) => adapters.push(adapter),
                Err(error) => errors.push(SiteSearchError {
                    site_id: site.id,
                    source_site: site.name,
                    query: String::new(),
                    code: error.code().to_string(),
                    message: error.to_string(),
                }),
            }
        }
        Ok((adapters, errors, priorities))
    }

    async fn ensure_references(
        &self,
        quality_profile_id: i64,
        downloader_id: i64,
        site_ids: &[i64],
    ) -> Result<(), MediaServiceError> {
        if self
            .db
            .get_quality_profile(quality_profile_id)
            .await?
            .is_none()
        {
            return Err(MediaServiceError::NotFound(format!(
                "quality profile {quality_profile_id}"
            )));
        }
        if self.db.get_downloader(downloader_id).await?.is_none() {
            return Err(MediaServiceError::NotFound(format!(
                "downloader {downloader_id}"
            )));
        }
        let configured: HashSet<_> = self
            .db
            .list_sites()
            .await?
            .into_iter()
            .map(|site| site.id)
            .collect();
        if site_ids.is_empty() || site_ids.iter().any(|id| !configured.contains(id)) {
            return Err(MediaServiceError::NotFound(
                "one or more selected PT sites do not exist".to_string(),
            ));
        }
        Ok(())
    }

    async fn tmdb_client(&self) -> Result<TmdbClient, MediaServiceError> {
        let media = self.db.get_media_settings().await?;
        let token = media.tmdb_token.ok_or(TmdbError::MissingToken)?;
        let proxy = self.db.get_settings().await?.proxy;
        let client = client_factory::build_client(proxy.as_deref())?;
        TmdbClient::new(client, token, media.tmdb_language).map_err(Into::into)
    }
}

fn media_download_filename(result: &SearchResult) -> String {
    format!(
        "kirara-media-{}-{}.torrent",
        result.site_id, result.torrent_id
    )
}

fn media_download_coverage(
    download: &MediaDownloadRecord,
    parser: &ReleaseParser,
) -> MediaDownloadCoverage {
    if download.subscription_id.is_none() {
        return MediaDownloadCoverage::default();
    }
    let accepted = serde_json::from_str::<serde_json::Value>(&download.decision_json)
        .ok()
        .and_then(|decision| {
            decision
                .get("accepted")
                .and_then(serde_json::Value::as_bool)
                .or_else(|| {
                    decision
                        .get("decision")
                        .and_then(|value| value.get("accepted"))
                        .and_then(serde_json::Value::as_bool)
                })
        })
        .unwrap_or(false);
    if !accepted {
        return MediaDownloadCoverage::default();
    }
    let Ok(release) = parser.parse(&download.title) else {
        return MediaDownloadCoverage::default();
    };
    let episodes = if release.episodes.len() > 1 {
        release.episodes
    } else {
        Vec::new()
    };
    let absolute_episodes = if release.absolute_episodes.len() > 1 {
        release.absolute_episodes
    } else {
        Vec::new()
    };
    MediaDownloadCoverage {
        season: release.season,
        episodes,
        absolute_episodes,
    }
}

fn is_valid_infohash(infohash: &str) -> bool {
    matches!(infohash.len(), 40 | 64) && infohash.bytes().all(|byte| byte.is_ascii_hexdigit())
}

async fn torrent_is_present(client: &dyn DownloaderClient, infohash: &str) -> Result<bool, String> {
    let hashes = [infohash.to_string()];
    client
        .list_torrents_by_hashes(&hashes)
        .await
        .map(|torrents| {
            torrents
                .iter()
                .any(|torrent| torrent.hash.eq_ignore_ascii_case(infohash))
        })
}

async fn confirm_torrent_present(
    client: &dyn DownloaderClient,
    infohash: &str,
) -> Result<bool, String> {
    let mut last_result = Ok(false);
    for attempt in 0..DOWNLOAD_CONFIRM_ATTEMPTS {
        match torrent_is_present(client, infohash).await {
            Ok(true) => return Ok(true),
            result => last_result = result,
        }
        if attempt + 1 < DOWNLOAD_CONFIRM_ATTEMPTS {
            tokio::time::sleep(DOWNLOAD_CONFIRM_INTERVAL).await;
        }
    }
    last_result
}

async fn add_torrent_and_confirm(
    client: &dyn DownloaderClient,
    torrent: Vec<u8>,
    filename: &str,
    options: &AddTorrentOptions,
    infohash: &str,
) -> Result<(), TorrentSubmissionError> {
    let submission_error = client.add_torrent(torrent, filename, options).await.err();
    match confirm_torrent_present(client, infohash).await {
        Ok(true) => Ok(()),
        Ok(false) => Err(TorrentSubmissionError::ConfirmedAbsent(
            submission_error
                .unwrap_or_else(|| "qBittorrent did not contain the submitted torrent".to_string()),
        )),
        Err(confirmation_error) => {
            let message = submission_error.map_or(confirmation_error.clone(), |submission_error| {
                format!("{submission_error}; qBittorrent verification failed: {confirmation_error}")
            });
            Err(TorrentSubmissionError::Unknown(message))
        }
    }
}

fn readiness_status(readiness: &TargetReadiness) -> &'static str {
    match readiness {
        TargetReadiness::Due => "waiting",
        TargetReadiness::Future(_) => "waiting_air_date",
        TargetReadiness::AwaitingMetadata => "awaiting_metadata",
    }
}

fn target_needs_tmdb_refresh(
    target: Option<&SubscriptionTargetRecord>,
    last_status: Option<&str>,
    now: chrono::DateTime<Utc>,
) -> bool {
    let Some(target) = target else {
        return true;
    };
    match target.status.as_str() {
        "metadata_pending" => true,
        "pending" => match target.air_date.as_deref().and_then(air_date_eligible_at) {
            None => true,
            Some(eligible_at) => {
                matches!(last_status, Some("waiting_air_date" | "submitted")) && eligible_at <= now
            }
        },
        _ => false,
    }
}

fn subscription_absolute_anchor(
    subscription: &SubscriptionRecord,
) -> Result<Option<u32>, MediaServiceError> {
    let Some(current_absolute) = subscription.absolute_episode else {
        return Ok(None);
    };
    let start_episode = subscription.start_episode.unwrap_or(1);
    let current_episode = subscription.next_episode.ok_or_else(|| {
        MediaServiceError::Invalid(
            "absolute-numbered subscription has no episode cursor".to_string(),
        )
    })?;
    let offset = current_episode.checked_sub(start_episode).ok_or_else(|| {
        MediaServiceError::Invalid(
            "subscription episode cursor precedes its configured start".to_string(),
        )
    })?;
    current_absolute
        .checked_sub(offset)
        .map(Some)
        .ok_or_else(|| {
            MediaServiceError::Invalid(
                "subscription absolute episode cursor precedes its configured anchor".to_string(),
            )
        })
}

fn empty_subscription_run(subscription_id: i64, target_key: String) -> SubscriptionRunResult {
    SubscriptionRunResult {
        subscription_id,
        target_key,
        query_count: 0,
        candidate_count: 0,
        accepted_count: 0,
        download: None,
        site_errors: Vec::new(),
    }
}

fn empty_run_snapshot(started_at: &str, target_key: &str) -> SubscriptionRunSnapshot {
    SubscriptionRunSnapshot {
        started_at: started_at.to_string(),
        finished_at: Utc::now().to_rfc3339(),
        target_key: target_key.to_string(),
        queries: Vec::new(),
        candidates: Vec::new(),
        site_errors: Vec::new(),
        total_sites: 0,
        successful_sites: 0,
        best_candidate_id: None,
        error: None,
    }
}

fn failed_run_snapshot(
    started_at: &str,
    target_key: &str,
    error: String,
) -> SubscriptionRunSnapshot {
    SubscriptionRunSnapshot {
        error: Some(error),
        ..empty_run_snapshot(started_at, target_key)
    }
}

fn subscription_target(
    subscription: &SubscriptionRecord,
) -> Result<MediaTarget, MediaServiceError> {
    let mut titles = vec![subscription.title.clone()];
    if let Some(original) = &subscription.original_title {
        push_case_insensitive(&mut titles, original);
    }
    for alias in &subscription.aliases {
        push_case_insensitive(&mut titles, alias);
    }
    if subscription.media_type == "movie" {
        return Ok(MediaTarget::Movie {
            tmdb_id: subscription.tmdb_id,
            titles,
            year: subscription.year,
        });
    }
    if let Some(absolute_episode) = subscription.absolute_episode {
        return Ok(MediaTarget::Anime {
            tmdb_id: subscription.tmdb_id,
            titles,
            year: subscription.year,
            absolute_episode,
            season_episode: subscription
                .season
                .zip(subscription.next_episode)
                .map(|(season, episode)| SeasonEpisode { season, episode }),
        });
    }
    Ok(MediaTarget::Episode {
        tmdb_id: subscription.tmdb_id,
        titles,
        year: subscription.year,
        season: subscription.season.ok_or_else(|| {
            MediaServiceError::Invalid("TV subscription has no season".to_string())
        })?,
        episode: subscription.next_episode.ok_or_else(|| {
            MediaServiceError::Invalid("TV subscription has no next episode".to_string())
        })?,
        allow_season_pack: false,
    })
}

fn profile_to_domain(profile: &QualityProfileRecord) -> QualityProfile {
    QualityProfile {
        id: Some(profile.id),
        name: profile.name.clone(),
        resolution_order: profile.resolution_order.clone(),
        allowed_resolutions: profile.allowed_resolutions.clone(),
        blocked_resolutions: profile.blocked_resolutions.clone(),
        source_order: profile.source_order.clone(),
        allowed_sources: profile.allowed_sources.clone(),
        codec_order: profile.codec_order.clone(),
        blocked_codecs: profile.blocked_codecs.clone(),
        allow_unknown_quality: profile.allow_unknown_quality,
        minimum_score: profile.minimum_score.clamp(0, 100) as u32,
        min_seeders: profile.min_seeders,
    }
}

fn compare_candidates(left: &ResourceCandidate, right: &ResourceCandidate) -> std::cmp::Ordering {
    match (&left.sort_key, &right.sort_key) {
        (Some(left), Some(right)) => SortKey::compare_best_first(left, right),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => right
            .result
            .seeders
            .cmp(&left.result.seeders)
            .then_with(|| right.result.publish_time.cmp(&left.result.publish_time))
            .then_with(|| left.result.site_id.cmp(&right.result.site_id))
            .then_with(|| left.result.torrent_id.cmp(&right.result.torrent_id)),
    }
}

fn reevaluate_resource_candidates(
    target: &MediaTarget,
    candidates: &mut [ResourceCandidate],
    profile: &QualityProfileRecord,
) {
    let domain_profile = profile_to_domain(profile);
    for candidate in candidates.iter_mut() {
        let Some(release) = candidate.release.as_ref() else {
            continue;
        };
        let decision =
            DecisionEngine::evaluate(target, release, &domain_profile, candidate.result.seeders);
        let site_priority = candidate
            .sort_key
            .as_ref()
            .map(|sort_key| sort_key.site_priority)
            .unwrap_or(u32::MAX);
        candidate.sort_key = Some(SortKey::from_candidate(
            &decision,
            &domain_profile,
            target,
            release,
            candidate.result.size,
            candidate.result.seeders,
            candidate.result.publish_time,
            site_priority,
            stable_release_key(
                &candidate.result.source_site,
                &candidate.result.torrent_id,
                &candidate.result.title,
            ),
        ));
        candidate.decision = Some(decision);
    }
    candidates.sort_by(compare_candidates);
}

fn push_case_insensitive(values: &mut Vec<String>, value: &str) {
    let value = value.trim();
    if !value.is_empty()
        && !values
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(value))
    {
        values.push(value.to_string());
    }
}

fn to_json<T: Serialize>(value: &T) -> Result<String, MediaServiceError> {
    serde_json::to_string(value)
        .map_err(|error| MediaServiceError::Serialization(error.to_string()))
}

fn new_candidate_id() -> Result<String, MediaServiceError> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut random = [0_u8; 24];
    getrandom::fill(&mut random).map_err(|error| {
        MediaServiceError::Serialization(format!("failed to generate candidate id: {error}"))
    })?;
    let mut candidate_id = String::with_capacity(5 + random.len() * 2);
    candidate_id.push_str("cand_");
    for byte in random {
        candidate_id.push(HEX[(byte >> 4) as usize] as char);
        candidate_id.push(HEX[(byte & 0x0f) as usize] as char);
    }
    Ok(candidate_id)
}

fn default_true() -> bool {
    true
}

impl From<reqwest::Error> for MediaServiceError {
    fn from(error: reqwest::Error) -> Self {
        Self::Invalid(format!("failed to create HTTP client: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, VecDeque};
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Mutex as StdMutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::downloader::{
        DownloaderClient, DownloaderClientPool, DownloaderTestResult, TorrentInfo,
    };
    use crate::indexer::{IndexerAdapter, IndexerCapabilities, IndexerFuture};
    use tempfile::tempdir;

    use super::*;

    fn coverage_test_download(
        subscription_id: Option<i64>,
        title: &str,
        decision_json: &str,
    ) -> MediaDownloadRecord {
        MediaDownloadRecord {
            id: 1,
            subscription_id,
            target_key: "tv:42:s01e03".to_string(),
            dedupe_key: "coverage-test".to_string(),
            site_id: Some(1),
            downloader_id: Some(1),
            source_site: "test-site".to_string(),
            downloader_name: "test-downloader".to_string(),
            torrent_id: "coverage-torrent".to_string(),
            title: title.to_string(),
            size: 1,
            release_json: "{}".to_string(),
            decision_json: decision_json.to_string(),
            profile_snapshot_json: "{}".to_string(),
            infohash: None,
            status: "fetching".to_string(),
            attempts: 1,
            next_attempt_at: None,
            lease_owner: None,
            lease_until: None,
            version: 1,
            last_error: None,
            created_at: String::new(),
            updated_at: String::new(),
            submitted_at: None,
        }
    }

    #[test]
    fn accepted_subscription_download_recovers_multi_episode_coverage() {
        let parser = ReleaseParser::default();
        let raw = coverage_test_download(
            Some(42),
            "Example.Show.S01E03-E05.1080p.WEB-DL",
            r#"{"accepted":true}"#,
        );
        assert_eq!(
            media_download_coverage(&raw, &parser),
            MediaDownloadCoverage {
                season: Some(1),
                episodes: vec![3, 4, 5],
                absolute_episodes: Vec::new(),
            }
        );

        let envelope = coverage_test_download(
            Some(42),
            "Example.Show.S01E03E04E05.1080p.WEB-DL",
            r#"{"decision":{"accepted":true}}"#,
        );
        assert_eq!(
            media_download_coverage(&envelope, &parser).episodes,
            vec![3, 4, 5]
        );
    }

    #[test]
    fn unaccepted_manual_or_single_episode_download_has_no_multi_episode_coverage() {
        let parser = ReleaseParser::default();
        let rejected = coverage_test_download(
            Some(42),
            "Example.Show.S01E03-E05.1080p.WEB-DL",
            r#"{"accepted":false}"#,
        );
        assert_eq!(
            media_download_coverage(&rejected, &parser),
            MediaDownloadCoverage::default()
        );

        let manual = coverage_test_download(
            None,
            "Example.Show.S01E03-E05.1080p.WEB-DL",
            r#"{"accepted":true}"#,
        );
        assert_eq!(
            media_download_coverage(&manual, &parser),
            MediaDownloadCoverage::default()
        );

        let single = coverage_test_download(
            Some(42),
            "Example.Show.S01E03.1080p.WEB-DL",
            r#"{"accepted":true}"#,
        );
        assert_eq!(
            media_download_coverage(&single, &parser),
            MediaDownloadCoverage {
                season: Some(1),
                episodes: Vec::new(),
                absolute_episodes: Vec::new(),
            }
        );
    }

    struct StaticIndexer {
        site_id: i64,
        site_name: String,
        results: Vec<SearchResult>,
    }

    struct QueryAwareIndexer {
        site_id: i64,
        site_name: String,
        outcomes: HashMap<String, Result<Vec<SearchResult>, IndexerError>>,
        default_outcome: Result<Vec<SearchResult>, IndexerError>,
        calls: Arc<StdMutex<Vec<String>>>,
    }

    struct TorrentIndexer {
        site_id: i64,
        site_name: String,
        torrent: Vec<u8>,
        fetch_count: Arc<AtomicUsize>,
    }

    impl IndexerAdapter for TorrentIndexer {
        fn site_id(&self) -> i64 {
            self.site_id
        }

        fn site_name(&self) -> &str {
            &self.site_name
        }

        fn capabilities(&self) -> IndexerCapabilities {
            IndexerCapabilities {
                search: false,
                fetch_torrent: true,
                api_search: false,
                html_search: false,
            }
        }

        fn search<'a>(
            &'a self,
            _request: &'a SearchRequest,
        ) -> IndexerFuture<'a, Vec<SearchResult>> {
            Box::pin(async { Ok(Vec::new()) })
        }

        fn fetch_torrent<'a>(&'a self, _result: &'a SearchResult) -> IndexerFuture<'a, Vec<u8>> {
            Box::pin(async move {
                self.fetch_count.fetch_add(1, Ordering::SeqCst);
                Ok(self.torrent.clone())
            })
        }
    }

    struct ScriptedDownloader {
        lookups: StdMutex<VecDeque<Result<Vec<TorrentInfo>, String>>>,
        fallback_lookup: Result<Vec<TorrentInfo>, String>,
        add_result: Result<(), String>,
        add_calls: AtomicUsize,
    }

    impl ScriptedDownloader {
        fn new(
            lookups: Vec<Result<Vec<TorrentInfo>, String>>,
            fallback_lookup: Result<Vec<TorrentInfo>, String>,
            add_result: Result<(), String>,
        ) -> Self {
            Self {
                lookups: StdMutex::new(lookups.into()),
                fallback_lookup,
                add_result,
                add_calls: AtomicUsize::new(0),
            }
        }
    }

    impl DownloaderClient for ScriptedDownloader {
        fn test_connection(
            &self,
        ) -> Pin<Box<dyn Future<Output = Result<DownloaderTestResult, String>> + Send + '_>>
        {
            Box::pin(async { Err("unused test operation".to_string()) })
        }

        fn add_torrent(
            &self,
            _torrent_data: Vec<u8>,
            _filename: &str,
            _options: &AddTorrentOptions,
        ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>> {
            self.add_calls.fetch_add(1, Ordering::SeqCst);
            let result = self.add_result.clone();
            Box::pin(async move { result })
        }

        fn list_torrents(
            &self,
            _tag: Option<&str>,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<TorrentInfo>, String>> + Send + '_>> {
            let result = self.fallback_lookup.clone();
            Box::pin(async move { result })
        }

        fn list_torrents_by_hashes<'a>(
            &'a self,
            _hashes: &'a [String],
        ) -> Pin<Box<dyn Future<Output = Result<Vec<TorrentInfo>, String>> + Send + 'a>> {
            let result = self
                .lookups
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| self.fallback_lookup.clone());
            Box::pin(async move { result })
        }

        fn pause_torrent(
            &self,
            _hash: &str,
        ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>> {
            Box::pin(async { Err("unused test operation".to_string()) })
        }

        fn delete_torrent(
            &self,
            _hash: &str,
            _delete_files: bool,
        ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>> {
            Box::pin(async { Err("unused test operation".to_string()) })
        }

        fn get_free_space(
            &self,
            _path: Option<&str>,
        ) -> Pin<Box<dyn Future<Output = Result<u64, String>> + Send + '_>> {
            Box::pin(async { Ok(0) })
        }

        fn get_default_save_path(
            &self,
        ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + '_>> {
            Box::pin(async { Ok(String::new()) })
        }

        fn get_torrent_trackers(
            &self,
            _hash: &str,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, String>> + Send + '_>> {
            Box::pin(async { Ok(Vec::new()) })
        }

        fn add_torrent_tags(
            &self,
            _hashes: Vec<String>,
            _tags: Vec<String>,
        ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>> {
            Box::pin(async { Ok(()) })
        }
    }

    impl IndexerAdapter for StaticIndexer {
        fn site_id(&self) -> i64 {
            self.site_id
        }

        fn site_name(&self) -> &str {
            &self.site_name
        }

        fn capabilities(&self) -> IndexerCapabilities {
            IndexerCapabilities {
                search: true,
                fetch_torrent: false,
                api_search: true,
                html_search: false,
            }
        }

        fn search<'a>(
            &'a self,
            _request: &'a SearchRequest,
        ) -> IndexerFuture<'a, Vec<SearchResult>> {
            Box::pin(async move { Ok(self.results.clone()) })
        }

        fn fetch_torrent<'a>(&'a self, _result: &'a SearchResult) -> IndexerFuture<'a, Vec<u8>> {
            Box::pin(async move {
                Err(IndexerError::Configuration(
                    "static test indexer cannot fetch torrents".to_string(),
                ))
            })
        }
    }

    impl IndexerAdapter for QueryAwareIndexer {
        fn site_id(&self) -> i64 {
            self.site_id
        }

        fn site_name(&self) -> &str {
            &self.site_name
        }

        fn capabilities(&self) -> IndexerCapabilities {
            IndexerCapabilities {
                search: true,
                fetch_torrent: false,
                api_search: true,
                html_search: false,
            }
        }

        fn search<'a>(
            &'a self,
            request: &'a SearchRequest,
        ) -> IndexerFuture<'a, Vec<SearchResult>> {
            let query = request.query.clone();
            self.calls.lock().unwrap().push(query.clone());
            let outcome = self
                .outcomes
                .get(&query)
                .cloned()
                .unwrap_or_else(|| self.default_outcome.clone());
            Box::pin(async move { outcome })
        }

        fn fetch_torrent<'a>(&'a self, _result: &'a SearchResult) -> IndexerFuture<'a, Vec<u8>> {
            Box::pin(async move {
                Err(IndexerError::Configuration(
                    "query-aware test indexer cannot fetch torrents".to_string(),
                ))
            })
        }
    }

    async fn test_service() -> (
        tempfile::TempDir,
        Arc<MediaService>,
        Database,
        i64,
        i64,
        i64,
    ) {
        let dir = tempdir().unwrap();
        let db = Database::open(dir.path()).await.unwrap();
        let first_site = db
            .create_site(
                "service-site-a",
                "nexusphp",
                "https://a.example",
                r#"{"auth_type":"cookie","cookie":"test=1"}"#,
                "[]",
                false,
            )
            .await
            .unwrap();
        let second_site = db
            .create_site(
                "service-site-b",
                "nexusphp",
                "https://b.example",
                r#"{"auth_type":"cookie","cookie":"test=1"}"#,
                "[]",
                false,
            )
            .await
            .unwrap();
        let downloader_id = db
            .create_downloader(
                "service-downloader",
                "qbittorrent",
                "http://127.0.0.1:8080",
                "",
                "",
            )
            .await
            .unwrap();
        let pool = DownloaderClientPool::new(db.clone());
        let service = MediaService::new(db.clone(), pool);
        (dir, service, db, first_site, second_site, downloader_id)
    }

    fn resource_search_result(index: usize) -> SearchResult {
        SearchResult {
            site_id: 1,
            source_site: "test-site".to_string(),
            torrent_id: format!("torrent-{index}"),
            title: format!("Example.Show.S01E01.1080p.WEB-DL-{index}"),
            detail_url: None,
            download_locator: Some(format!("torrent-{index}")),
            magnet: None,
            size: 1_024,
            seeders: u32::try_from(index).unwrap_or(u32::MAX),
            leechers: 0,
            publish_time: None,
        }
    }

    fn resource_search_request(site_id: i64) -> ResourceSearchRequest {
        ResourceSearchRequest {
            query: Some("Example Show".to_string()),
            site_ids: vec![site_id],
            target: None,
            quality_profile_id: None,
            page_size: Some(100),
        }
    }

    async fn install_static_indexer(
        service: &MediaService,
        db: &Database,
        site_id: i64,
        results: Vec<SearchResult>,
    ) {
        let site = db.get_site(site_id).await.unwrap().unwrap();
        service
            .indexers
            .insert_for_test(
                &site,
                None,
                Arc::new(StaticIndexer {
                    site_id,
                    site_name: site.name.clone(),
                    results,
                }),
            )
            .await;
    }

    async fn install_query_aware_indexer(
        service: &MediaService,
        db: &Database,
        site_id: i64,
        outcomes: HashMap<String, Result<Vec<SearchResult>, IndexerError>>,
        default_outcome: Result<Vec<SearchResult>, IndexerError>,
    ) -> Arc<StdMutex<Vec<String>>> {
        let site = db.get_site(site_id).await.unwrap().unwrap();
        let calls = Arc::new(StdMutex::new(Vec::new()));
        service
            .indexers
            .insert_for_test(
                &site,
                None,
                Arc::new(QueryAwareIndexer {
                    site_id,
                    site_name: site.name.clone(),
                    outcomes,
                    default_outcome,
                    calls: Arc::clone(&calls),
                }),
            )
            .await;
        calls
    }

    fn episode_search_result(site_id: i64, torrent_id: &str, title: &str) -> SearchResult {
        SearchResult {
            site_id,
            source_site: format!("test-site-{site_id}"),
            torrent_id: torrent_id.to_string(),
            title: title.to_string(),
            detail_url: None,
            download_locator: Some(torrent_id.to_string()),
            magnet: None,
            size: 146_374_137,
            seeders: 31,
            leechers: 0,
            publish_time: None,
        }
    }

    fn episode_search_request(
        site_ids: Vec<i64>,
        titles: Vec<&str>,
        query: Option<&str>,
    ) -> ResourceSearchRequest {
        ResourceSearchRequest {
            query: query.map(str::to_string),
            site_ids,
            target: Some(MediaTarget::Episode {
                tmdb_id: 42,
                titles: titles.into_iter().map(str::to_string).collect(),
                year: None,
                season: 1,
                episode: 3,
                allow_season_pack: false,
            }),
            quality_profile_id: Some(1),
            page_size: Some(50),
        }
    }

    fn test_torrent(name: &str) -> Vec<u8> {
        format!("d4:infod4:name{}:{}ee", name.len(), name).into_bytes()
    }

    fn test_torrent_info(infohash: &str) -> TorrentInfo {
        TorrentInfo {
            hash: infohash.to_string(),
            name: "test torrent".to_string(),
            size: 1,
            uploaded: 0,
            downloaded: 0,
            amount_left: None,
            completed: None,
            progress: 0.0,
            upload_speed: 0,
            download_speed: 0,
            ratio: 0.0,
            state: "downloading".to_string(),
            added_on: 0,
            completion_on: 0,
            num_seeds: 0,
            num_leechs: 0,
            save_path: String::new(),
            root_path: String::new(),
            content_path: String::new(),
            tags: String::new(),
            category: String::new(),
            time_active: 0,
            last_activity: 0,
        }
    }

    fn test_download_result(site_id: i64, torrent_id: &str) -> SearchResult {
        SearchResult {
            site_id,
            source_site: "test site".to_string(),
            torrent_id: torrent_id.to_string(),
            title: "Example.Show.S01E01.1080p.WEB-DL.H264".to_string(),
            detail_url: None,
            download_locator: Some(torrent_id.to_string()),
            magnet: None,
            size: 1_024,
            seeders: 10,
            leechers: 0,
            publish_time: None,
        }
    }

    async fn enqueue_test_download(
        db: &Database,
        site_id: i64,
        downloader_id: i64,
        torrent_id: &str,
    ) -> MediaDownloadRecord {
        let site = db.get_site(site_id).await.unwrap().unwrap();
        let downloader = db.get_downloader(downloader_id).await.unwrap().unwrap();
        let result = test_download_result(site_id, torrent_id);
        db.enqueue_media_download(&NewMediaDownload {
            subscription_id: None,
            target_key: format!("manual:{site_id}:{torrent_id}"),
            dedupe_key: format!("test:{site_id}:{downloader_id}:{torrent_id}"),
            site_id: Some(site_id),
            downloader_id: Some(downloader_id),
            source_site: site.name,
            downloader_name: downloader.name,
            torrent_id: torrent_id.to_string(),
            title: result.title.clone(),
            size: result.size,
            release_json: to_json(&result).unwrap(),
            decision_json: "{}".to_string(),
            profile_snapshot_json: "{}".to_string(),
        })
        .await
        .unwrap()
    }

    async fn enqueue_failed_linked_download(
        db: &Database,
        site_id: i64,
        downloader_id: i64,
        tmdb_id: i64,
        infohash: &str,
    ) -> (SubscriptionRecord, MediaDownloadRecord, NewMediaDownload) {
        let target_key = crate::media::models::target_key("tv", tmdb_id, Some(1), Some(3), None);
        let subscription = db
            .create_subscription_with_targets(
                &NewSubscription {
                    tmdb_id,
                    media_type: "tv".to_string(),
                    tmdb_is_animation: false,
                    tmdb_genres: Vec::new(),
                    title: format!("Example Show {tmdb_id}"),
                    original_title: None,
                    aliases: Vec::new(),
                    year: Some(2026),
                    poster_path: None,
                    season: Some(1),
                    start_episode: Some(3),
                    absolute_episode: None,
                    quality_profile_id: 1,
                    downloader_id,
                    site_ids: vec![site_id],
                    save_path: None,
                    enabled: true,
                },
                &[crate::media::progression::SubscriptionTargetSeed {
                    target_key: target_key.clone(),
                    season: 1,
                    episode: 3,
                    absolute_episode: None,
                    air_date: Some("2020-01-01".to_string()),
                    status: SubscriptionTargetSeedStatus::Pending,
                }],
                None,
                Some("waiting"),
            )
            .await
            .unwrap();
        let request = NewMediaDownload {
            subscription_id: Some(subscription.id),
            target_key: target_key.clone(),
            dedupe_key: format!("subscription:{}:{target_key}", subscription.id),
            site_id: Some(site_id),
            downloader_id: Some(downloader_id),
            source_site: "service-site-a".to_string(),
            downloader_name: "service-downloader".to_string(),
            torrent_id: format!("failed-{tmdb_id}"),
            title: format!("Example.Show.{tmdb_id}.S01E03.1080p.WEB-DL"),
            size: 1_024,
            release_json: "{}".to_string(),
            decision_json: "{}".to_string(),
            profile_snapshot_json: "{}".to_string(),
        };
        let queued = db
            .enqueue_subscription_media_download(subscription.version, &request)
            .await
            .unwrap()
            .unwrap();
        let owner = format!("failed-reconcile-{tmdb_id}");
        let fetching = db
            .claim_due_media_downloads(&owner, 60, 1)
            .await
            .unwrap()
            .into_iter()
            .find(|download| download.id == queued.id)
            .unwrap();
        assert!(
            db.transition_media_download(
                fetching.id,
                fetching.version,
                &owner,
                "fetching",
                "failed",
                Some(infohash),
                Some("submission outcome is unknown"),
                None,
            )
            .await
            .unwrap()
        );
        let failed = db.get_media_download(queued.id).await.unwrap().unwrap();
        (subscription, failed, request)
    }

    async fn install_download_dependencies(
        service: &MediaService,
        db: &Database,
        site_id: i64,
        downloader_id: i64,
        torrent: Vec<u8>,
        client: Arc<ScriptedDownloader>,
    ) -> Arc<AtomicUsize> {
        let site = db.get_site(site_id).await.unwrap().unwrap();
        let fetch_count = Arc::new(AtomicUsize::new(0));
        service
            .indexers
            .insert_for_test(
                &site,
                None,
                Arc::new(TorrentIndexer {
                    site_id,
                    site_name: site.name.clone(),
                    torrent,
                    fetch_count: Arc::clone(&fetch_count),
                }),
            )
            .await;
        let downloader = db.get_downloader(downloader_id).await.unwrap().unwrap();
        let injected: Arc<dyn DownloaderClient> = client;
        service
            .downloaders
            .insert_for_test(&downloader, None, injected)
            .await;
        fetch_count
    }

    async fn mark_test_download_submitted(
        db: &Database,
        queued: &MediaDownloadRecord,
        owner: &str,
        infohash: &str,
    ) -> MediaDownloadRecord {
        let fetching = db
            .claim_due_media_downloads(owner, 60, 1)
            .await
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(fetching.id, queued.id);
        assert!(
            db.mark_media_download_submitting(fetching.id, fetching.version, owner, infohash, 60,)
                .await
                .unwrap()
        );
        let submitting = db.get_media_download(queued.id).await.unwrap().unwrap();
        assert!(
            db.mark_media_download_submitted(
                submitting.id,
                submitting.version,
                owner,
                infohash,
                MediaDownloadCoverage::default(),
            )
            .await
            .unwrap()
        );
        db.get_media_download(queued.id).await.unwrap().unwrap()
    }

    #[tokio::test]
    async fn successful_add_without_visible_infohash_is_not_marked_submitted() {
        let (_dir, service, db, site_id, _other_site, downloader_id) = test_service().await;
        let torrent = test_torrent("missing-after-add");
        let client = Arc::new(ScriptedDownloader::new(Vec::new(), Ok(Vec::new()), Ok(())));
        install_download_dependencies(
            &service,
            &db,
            site_id,
            downloader_id,
            torrent,
            Arc::clone(&client),
        )
        .await;
        let queued = enqueue_test_download(&db, site_id, downloader_id, "missing").await;
        let owner = "missing-hash-worker";
        let fetching = db
            .claim_due_media_downloads(owner, 60, 1)
            .await
            .unwrap()
            .pop()
            .unwrap();

        let error = service.process_download(fetching, owner).await.unwrap_err();

        assert!(matches!(error, MediaServiceError::Downloader(_)));
        let stored = db.get_media_download(queued.id).await.unwrap().unwrap();
        assert_eq!(stored.status, "retry_wait");
        assert!(stored.submitted_at.is_none());
        assert_eq!(client.add_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn delayed_infohash_visibility_is_confirmed_before_marking_submitted() {
        let (_dir, service, db, site_id, _other_site, downloader_id) = test_service().await;
        let torrent = test_torrent("delayed-after-add");
        let infohash = torrent_infohash(&torrent).unwrap();
        let present = vec![test_torrent_info(&infohash)];
        let client = Arc::new(ScriptedDownloader::new(
            vec![Ok(Vec::new()), Ok(present.clone())],
            Ok(present),
            Ok(()),
        ));
        install_download_dependencies(
            &service,
            &db,
            site_id,
            downloader_id,
            torrent,
            Arc::clone(&client),
        )
        .await;
        let queued = enqueue_test_download(&db, site_id, downloader_id, "delayed").await;
        let owner = "delayed-hash-worker";
        let fetching = db
            .claim_due_media_downloads(owner, 60, 1)
            .await
            .unwrap()
            .pop()
            .unwrap();

        service.process_download(fetching, owner).await.unwrap();

        let stored = db.get_media_download(queued.id).await.unwrap().unwrap();
        assert_eq!(stored.status, "submitted");
        assert_eq!(stored.infohash.as_deref(), Some(infohash.as_str()));
        assert!(stored.submitted_at.is_some());
        assert_eq!(client.add_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn qb_present_torrent_is_submitted_without_adding_it_again() {
        let (_dir, service, db, site_id, _other_site, downloader_id) = test_service().await;
        let torrent = test_torrent("externally-present");
        let infohash = torrent_infohash(&torrent).unwrap();
        let present = vec![test_torrent_info(&infohash)];
        let client = Arc::new(ScriptedDownloader::new(
            Vec::new(),
            Ok(present),
            Err("duplicate torrent".to_string()),
        ));
        install_download_dependencies(
            &service,
            &db,
            site_id,
            downloader_id,
            torrent,
            Arc::clone(&client),
        )
        .await;
        let queued = enqueue_test_download(&db, site_id, downloader_id, "externally-present").await;
        let owner = "externally-present-worker";
        let fetching = db
            .claim_due_media_downloads(owner, 60, 1)
            .await
            .unwrap()
            .pop()
            .unwrap();

        service.process_download(fetching, owner).await.unwrap();

        let stored = db.get_media_download(queued.id).await.unwrap().unwrap();
        assert_eq!(stored.status, "submitted");
        assert_eq!(stored.infohash.as_deref(), Some(infohash.as_str()));
        assert_eq!(client.add_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn qb_present_duplicate_bypasses_active_relocation_without_duplicate_add() {
        let (_dir, service, db, site_id, _other_site, downloader_id) = test_service().await;
        let torrent = test_torrent("relocating-present");
        let infohash = torrent_infohash(&torrent).unwrap();
        let original =
            enqueue_test_download(&db, site_id, downloader_id, "relocating-original").await;
        let original =
            mark_test_download_submitted(&db, &original, "relocating-original-worker", &infohash)
                .await;
        assert_eq!(
            db.enqueue_submitted_media_relocation_jobs(true)
                .await
                .unwrap(),
            1
        );
        let client = Arc::new(ScriptedDownloader::new(
            Vec::new(),
            Ok(vec![test_torrent_info(&infohash)]),
            Err("duplicate torrent".to_string()),
        ));
        install_download_dependencies(
            &service,
            &db,
            site_id,
            downloader_id,
            torrent,
            Arc::clone(&client),
        )
        .await;
        let duplicate =
            enqueue_test_download(&db, site_id, downloader_id, "relocating-duplicate").await;
        let owner = "relocating-duplicate-worker";
        let fetching = db
            .claim_due_media_downloads(owner, 60, 1)
            .await
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(fetching.id, duplicate.id);

        service.process_download(fetching, owner).await.unwrap();

        let stored = db.get_media_download(duplicate.id).await.unwrap().unwrap();
        assert_eq!(stored.status, "submitted");
        assert_eq!(stored.infohash, None);
        assert!(
            stored
                .last_error
                .as_deref()
                .is_some_and(|message| message.contains(&original.id.to_string()))
        );
        assert_eq!(client.add_calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            db.enqueue_submitted_media_relocation_jobs(true)
                .await
                .unwrap(),
            0
        );
        let jobs = db.list_media_relocation_jobs(10).await.unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].media_download_id, Some(original.id));
    }

    #[tokio::test]
    async fn redelivery_is_idempotent_when_infohash_already_exists() {
        let (_dir, service, db, site_id, _other_site, downloader_id) = test_service().await;
        let torrent = test_torrent("already-present");
        let infohash = torrent_infohash(&torrent).unwrap();
        let queued = enqueue_test_download(&db, site_id, downloader_id, "already-present").await;
        let submitted =
            mark_test_download_submitted(&db, &queued, "submit-existing", &infohash).await;
        let client = Arc::new(ScriptedDownloader::new(
            Vec::new(),
            Ok(vec![test_torrent_info(&infohash)]),
            Ok(()),
        ));
        let fetch_count = install_download_dependencies(
            &service,
            &db,
            site_id,
            downloader_id,
            torrent,
            Arc::clone(&client),
        )
        .await;

        let verified = service.redeliver_download(submitted.id).await.unwrap();

        assert_eq!(verified.version, submitted.version);
        assert_eq!(client.add_calls.load(Ordering::SeqCst), 0);
        assert_eq!(fetch_count.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn redelivery_restores_missing_torrent_without_mutating_outbox_state() {
        let (_dir, service, db, site_id, _other_site, downloader_id) = test_service().await;
        let torrent = test_torrent("redelivered");
        let infohash = torrent_infohash(&torrent).unwrap();
        let queued = enqueue_test_download(&db, site_id, downloader_id, "redelivered").await;
        let submitted =
            mark_test_download_submitted(&db, &queued, "submit-missing", &infohash).await;
        let present = vec![test_torrent_info(&infohash)];
        let client = Arc::new(ScriptedDownloader::new(
            vec![Ok(Vec::new()), Ok(present.clone())],
            Ok(present),
            Ok(()),
        ));
        let fetch_count = install_download_dependencies(
            &service,
            &db,
            site_id,
            downloader_id,
            torrent,
            Arc::clone(&client),
        )
        .await;

        service.redeliver_download(submitted.id).await.unwrap();

        let stored = db.get_media_download(submitted.id).await.unwrap().unwrap();
        assert_eq!(stored.status, "submitted");
        assert_eq!(stored.version, submitted.version);
        assert_eq!(stored.submitted_at, submitted.submitted_at);
        assert_eq!(client.add_calls.load(Ordering::SeqCst), 1);
        assert_eq!(fetch_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn redelivery_rejects_downloads_that_are_not_submitted() {
        let (_dir, service, db, site_id, _other_site, downloader_id) = test_service().await;
        let queued = enqueue_test_download(&db, site_id, downloader_id, "still-queued").await;

        let error = service.redeliver_download(queued.id).await.unwrap_err();

        assert!(matches!(error, MediaServiceError::Conflict(_)));
    }

    #[tokio::test]
    async fn redelivery_is_blocked_by_target_side_manual_migration_before_qb_access() {
        let (_dir, service, db, site_id, _other_site, downloader_id) = test_service().await;
        let torrent = test_torrent("redelivery-migration-guard");
        let infohash = torrent_infohash(&torrent).unwrap();
        let queued = enqueue_test_download(&db, site_id, downloader_id, "guarded-redelivery").await;
        let submitted =
            mark_test_download_submitted(&db, &queued, "guarded-redelivery-worker", &infohash)
                .await;
        let source_downloader_id = db
            .create_downloader(
                "migration-source",
                "qbittorrent",
                "http://127.0.0.1:9090",
                "",
                "",
            )
            .await
            .unwrap();
        assert_eq!(
            db.enqueue_manual_media_relocation_jobs(
                source_downloader_id,
                downloader_id,
                "/archive",
                "/archive",
                &[(infohash.clone(), "guarded torrent".to_string())],
            )
            .await
            .unwrap(),
            (1, 0)
        );
        let client = Arc::new(ScriptedDownloader::new(Vec::new(), Ok(Vec::new()), Ok(())));
        let fetch_count = install_download_dependencies(
            &service,
            &db,
            site_id,
            downloader_id,
            torrent,
            Arc::clone(&client),
        )
        .await;

        let error = service.redeliver_download(submitted.id).await.unwrap_err();

        assert!(matches!(error, MediaServiceError::Conflict(_)));
        assert_eq!(client.add_calls.load(Ordering::SeqCst), 0);
        assert_eq!(fetch_count.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn process_download_does_not_add_when_manual_migration_owns_the_identity() {
        let (_dir, service, db, site_id, _other_site, downloader_id) = test_service().await;
        let torrent = test_torrent("process-migration-guard");
        let infohash = torrent_infohash(&torrent).unwrap();
        assert_eq!(
            db.enqueue_manual_media_relocation_jobs(
                downloader_id,
                downloader_id,
                "/archive",
                "/archive",
                &[(infohash, "guarded process torrent".to_string())],
            )
            .await
            .unwrap(),
            (1, 0)
        );
        let client = Arc::new(ScriptedDownloader::new(Vec::new(), Ok(Vec::new()), Ok(())));
        install_download_dependencies(
            &service,
            &db,
            site_id,
            downloader_id,
            torrent,
            Arc::clone(&client),
        )
        .await;
        let queued = enqueue_test_download(&db, site_id, downloader_id, "guarded-process").await;
        let owner = "guarded-process-worker";
        let fetching = db
            .claim_due_media_downloads(owner, 60, 1)
            .await
            .unwrap()
            .pop()
            .unwrap();

        let error = service.process_download(fetching, owner).await.unwrap_err();

        assert!(matches!(error, MediaServiceError::Conflict(_)));
        assert_eq!(client.add_calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            db.get_media_download(queued.id)
                .await
                .unwrap()
                .unwrap()
                .status,
            "retry_wait"
        );
    }

    #[tokio::test]
    async fn failed_reconciliation_confirms_present_torrent_without_adding() {
        let (_dir, service, db, site_id, _other_site, downloader_id) = test_service().await;
        let infohash = "0123456789abcdef0123456789abcdef01234567";
        let (subscription, failed, request) =
            enqueue_failed_linked_download(&db, site_id, downloader_id, 301, infohash).await;
        let client = Arc::new(ScriptedDownloader::new(
            Vec::new(),
            Ok(vec![test_torrent_info(infohash)]),
            Ok(()),
        ));
        install_download_dependencies(
            &service,
            &db,
            site_id,
            downloader_id,
            test_torrent("unused-present-reconcile"),
            Arc::clone(&client),
        )
        .await;
        assert!(
            db.media_download_failed_reconciliation_allowed(failed.id, failed.version)
                .await
                .unwrap()
        );

        let resolved = service
            .reconcile_failed_download(failed.id, failed.version)
            .await
            .unwrap();

        assert_eq!(resolved.resolution, "submitted");
        assert_eq!(resolved.download.status, "submitted");
        assert_eq!(client.add_calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            db.get_subscription_target(subscription.id, &request.target_key)
                .await
                .unwrap()
                .unwrap()
                .status,
            "submitted"
        );
        let advanced = db.get_subscription(subscription.id).await.unwrap().unwrap();
        assert_eq!(advanced.next_episode, Some(4));
        assert_eq!(advanced.last_status.as_deref(), Some("submitted"));
        assert!(
            !db.media_download_failed_reconciliation_allowed(failed.id, resolved.download.version)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn failed_reconciliation_confirms_absence_and_allows_requeue() {
        let (_dir, service, db, site_id, _other_site, downloader_id) = test_service().await;
        let infohash = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
        let (subscription, failed, request) =
            enqueue_failed_linked_download(&db, site_id, downloader_id, 302, infohash).await;
        let client = Arc::new(ScriptedDownloader::new(Vec::new(), Ok(Vec::new()), Ok(())));
        install_download_dependencies(
            &service,
            &db,
            site_id,
            downloader_id,
            test_torrent("unused-absent-reconcile"),
            Arc::clone(&client),
        )
        .await;
        assert!(
            db.media_download_failed_reconciliation_allowed(failed.id, failed.version)
                .await
                .unwrap()
        );

        let resolved = service
            .reconcile_failed_download(failed.id, failed.version)
            .await
            .unwrap();

        assert_eq!(resolved.resolution, "retry_ready");
        assert_eq!(resolved.download.status, "failed");
        assert_eq!(client.add_calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            db.get_subscription_target(subscription.id, &request.target_key)
                .await
                .unwrap()
                .unwrap()
                .status,
            "pending"
        );
        assert!(
            !db.media_download_failed_reconciliation_allowed(failed.id, resolved.download.version)
                .await
                .unwrap()
        );
        let retry_subscription = db.get_subscription(subscription.id).await.unwrap().unwrap();
        let requeued = db
            .enqueue_subscription_media_download(retry_subscription.version, &request)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(requeued.id, failed.id);
        assert_eq!(requeued.status, "queued");
        assert!(
            !db.media_download_failed_reconciliation_allowed(failed.id, requeued.version)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn failed_reconciliation_qb_error_leaves_all_state_unchanged() {
        let (_dir, service, db, site_id, _other_site, downloader_id) = test_service().await;
        let infohash = "23456789abcdef0123456789abcdef0123456789";
        let (subscription, failed, request) =
            enqueue_failed_linked_download(&db, site_id, downloader_id, 303, infohash).await;
        let client = Arc::new(ScriptedDownloader::new(
            Vec::new(),
            Err("qB lookup unavailable".to_string()),
            Ok(()),
        ));
        install_download_dependencies(
            &service,
            &db,
            site_id,
            downloader_id,
            test_torrent("unused-error-reconcile"),
            Arc::clone(&client),
        )
        .await;
        assert!(
            db.media_download_failed_reconciliation_allowed(failed.id, failed.version)
                .await
                .unwrap()
        );
        let before_download = serde_json::to_value(&failed).unwrap();
        let before_target = serde_json::to_value(
            db.get_subscription_target(subscription.id, &request.target_key)
                .await
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        let before_subscription =
            serde_json::to_value(db.get_subscription(subscription.id).await.unwrap().unwrap())
                .unwrap();

        let error = service
            .reconcile_failed_download(failed.id, failed.version)
            .await
            .unwrap_err();

        assert!(matches!(error, MediaServiceError::Downloader(_)));
        assert_eq!(client.add_calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            serde_json::to_value(db.get_media_download(failed.id).await.unwrap().unwrap()).unwrap(),
            before_download
        );
        assert_eq!(
            serde_json::to_value(
                db.get_subscription_target(subscription.id, &request.target_key)
                    .await
                    .unwrap()
                    .unwrap()
            )
            .unwrap(),
            before_target
        );
        assert_eq!(
            serde_json::to_value(db.get_subscription(subscription.id).await.unwrap().unwrap())
                .unwrap(),
            before_subscription
        );
        assert!(
            db.media_download_failed_reconciliation_allowed(failed.id, failed.version)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn deleting_download_history_is_versioned_and_does_not_contact_the_downloader() {
        let (_dir, service, db, site_id, _other_site, downloader_id) = test_service().await;
        let queued = enqueue_test_download(&db, site_id, downloader_id, "delete-history").await;
        let active = service
            .delete_download_record(queued.id, queued.version)
            .await
            .unwrap_err();
        assert!(matches!(active, MediaServiceError::Conflict(_)));
        let submitted = mark_test_download_submitted(
            &db,
            &queued,
            "delete-history-worker",
            "0123456789abcdef0123456789abcdef01234567",
        )
        .await;

        let stale = service
            .delete_download_record(submitted.id, submitted.version - 1)
            .await
            .unwrap_err();
        assert!(matches!(stale, MediaServiceError::Conflict(_)));
        assert!(db.get_media_download(submitted.id).await.unwrap().is_some());

        // No downloader client is installed in the pool. A successful deletion therefore also
        // proves this operation is local database cleanup rather than a qB mutation.
        let deleted = service
            .delete_download_record(submitted.id, submitted.version)
            .await
            .unwrap();
        assert_eq!(deleted.deleted_id, submitted.id);
        assert_eq!(deleted.subscription_id, None);
        assert!(!deleted.target_reopened);
        assert!(db.get_media_download(submitted.id).await.unwrap().is_none());
    }

    #[test]
    fn persistent_profile_maps_to_domain_policy() {
        let record = QualityProfileRecord {
            id: 9,
            name: "Test".to_string(),
            resolution_order: vec!["1080p".to_string()],
            allowed_resolutions: vec!["1080p".to_string()],
            blocked_resolutions: vec!["480p".to_string()],
            source_order: vec!["web-dl".to_string()],
            allowed_sources: Vec::new(),
            codec_order: vec!["h265".to_string()],
            blocked_codecs: Vec::new(),
            allow_unknown_quality: false,
            minimum_score: 80,
            min_seeders: 2,
            created_at: String::new(),
            updated_at: String::new(),
        };
        let domain = profile_to_domain(&record);
        assert_eq!(domain.id, Some(9));
        assert_eq!(domain.minimum_score, 80);
        assert_eq!(domain.min_seeders, 2);
    }

    #[test]
    fn translated_alias_turns_title_only_rejection_into_an_accepted_candidate() {
        let profile = QualityProfileRecord {
            id: 1,
            name: "Test".to_string(),
            resolution_order: vec!["1080p".to_string()],
            allowed_resolutions: vec!["1080p".to_string()],
            blocked_resolutions: Vec::new(),
            source_order: vec!["web-dl".to_string()],
            allowed_sources: vec!["web-dl".to_string()],
            codec_order: vec!["h264".to_string()],
            blocked_codecs: Vec::new(),
            allow_unknown_quality: false,
            minimum_score: 80,
            min_seeders: 1,
            created_at: String::new(),
            updated_at: String::new(),
        };
        let release = ReleaseParser::default()
            .parse("Crowned in a Hundred Days 2026 S01E06 1080p WEB-DL H.264 AAC-UBWEB")
            .unwrap();
        let original_target = MediaTarget::Episode {
            tmdb_id: 326844,
            titles: vec!["百日成王".to_string(), "Bai Ri Cheng Wang".to_string()],
            year: Some(2026),
            season: 1,
            episode: 6,
            allow_season_pack: false,
        };
        let original_decision =
            DecisionEngine::evaluate(&original_target, &release, &profile_to_domain(&profile), 31);
        assert_eq!(original_decision.rejections.len(), 1);
        assert_eq!(original_decision.rejections[0].code, RejectCode::WrongTitle);

        let translated_target = MediaTarget::Episode {
            tmdb_id: 326844,
            titles: vec![
                "百日成王".to_string(),
                "Bai Ri Cheng Wang".to_string(),
                "Crowned in a Hundred Days".to_string(),
            ],
            year: Some(2026),
            season: 1,
            episode: 6,
            allow_season_pack: false,
        };
        let mut candidates = vec![ResourceCandidate {
            candidate_id: "candidate".to_string(),
            result: SearchResult {
                site_id: 1,
                source_site: "site".to_string(),
                torrent_id: "1207293".to_string(),
                title: release.raw_title.clone(),
                detail_url: None,
                download_locator: Some("1207293".to_string()),
                magnet: None,
                size: 146_374_137,
                seeders: 31,
                leechers: 0,
                publish_time: None,
            },
            release: Some(release),
            parse_error: None,
            decision: Some(original_decision),
            sort_key: None,
        }];

        reevaluate_resource_candidates(&translated_target, &mut candidates, &profile);

        assert!(candidates[0].decision.as_ref().unwrap().accepted);
        assert_eq!(candidates[0].decision.as_ref().unwrap().score, 100);
    }

    #[test]
    fn subscription_target_uses_absolute_episode_when_configured() {
        let mut subscription = SubscriptionRecord {
            id: 1,
            tmdb_id: 2,
            media_type: "tv".to_string(),
            tmdb_is_animation: true,
            tmdb_genres: Vec::new(),
            title: "Anime".to_string(),
            original_title: None,
            aliases: Vec::new(),
            year: None,
            poster_path: None,
            season: Some(1),
            next_episode: Some(3),
            start_episode: Some(3),
            absolute_episode: Some(123),
            quality_profile_id: 1,
            downloader_id: 1,
            site_ids: vec![1],
            save_path: None,
            enabled: true,
            next_run_at: String::new(),
            lease_owner: None,
            lease_until: None,
            version: 0,
            last_status: None,
            last_error: None,
            last_run_at: None,
            created_at: String::new(),
            updated_at: String::new(),
        };
        let target = subscription_target(&subscription).unwrap();
        assert_eq!(target.target_key(), "tv:2:abs0123");

        subscription.next_episode = Some(5);
        subscription.absolute_episode = Some(125);
        assert_eq!(
            subscription_absolute_anchor(&subscription).unwrap(),
            Some(123)
        );
    }

    #[test]
    fn tmdb_refresh_is_required_only_at_metadata_or_air_date_checkpoints() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-07-15T10:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let mut target = SubscriptionTargetRecord {
            id: 1,
            subscription_id: 1,
            target_key: "tv:2:s01e03".to_string(),
            season: Some(1),
            episode: Some(3),
            absolute_episode: None,
            air_date: None,
            status: "metadata_pending".to_string(),
            created_at: String::new(),
            updated_at: String::new(),
        };
        assert!(target_needs_tmdb_refresh(Some(&target), None, now));

        target.status = "pending".to_string();
        target.air_date = Some("2026-07-20".to_string());
        assert!(!target_needs_tmdb_refresh(
            Some(&target),
            Some("waiting_air_date"),
            now,
        ));

        target.air_date = Some("2026-07-15".to_string());
        assert!(target_needs_tmdb_refresh(
            Some(&target),
            Some("waiting_air_date"),
            now,
        ));
        assert!(target_needs_tmdb_refresh(
            Some(&target),
            Some("submitted"),
            now,
        ));

        target.status = "queued".to_string();
        assert!(!target_needs_tmdb_refresh(Some(&target), None, now));
    }

    #[tokio::test]
    async fn subscription_queue_uses_the_cached_title_instead_of_forged_client_fields() {
        let (_dir, service, db, site_id, _other_site, downloader_id) = test_service().await;
        let subscription = db
            .create_subscription(&NewSubscription {
                tmdb_id: 42,
                media_type: "tv".to_string(),
                tmdb_is_animation: false,
                tmdb_genres: Vec::new(),
                title: "Example Show".to_string(),
                original_title: None,
                aliases: Vec::new(),
                year: Some(2026),
                poster_path: None,
                season: Some(1),
                start_episode: Some(3),
                absolute_episode: None,
                quality_profile_id: 1,
                downloader_id,
                site_ids: vec![site_id],
                save_path: None,
                enabled: true,
            })
            .await
            .unwrap();
        let target = subscription_target(&subscription).unwrap();
        let candidate_id = new_candidate_id().unwrap();
        service.candidate_cache.lock().await.insert(
            candidate_id.clone(),
            SearchResult {
                site_id,
                source_site: "service-site-a".to_string(),
                torrent_id: "authoritative-unrelated-3".to_string(),
                title: "Other.Show.S01E03.1080p.WEB-DL".to_string(),
                detail_url: None,
                download_locator: Some("authoritative-unrelated-3".to_string()),
                magnet: None,
                size: 1024,
                seeders: 100,
                leechers: 0,
                publish_time: None,
            },
            Some(target),
            Some(1),
        );
        let request = QueueDownloadRequest {
            candidate_id: candidate_id.clone(),
            quality_profile_id: 1,
            downloader_id,
            subscription_id: Some(subscription.id),
            override_reason: None,
        };

        let error = service.queue_download(&request).await.unwrap_err();
        assert!(matches!(error, MediaServiceError::Invalid(_)));
        assert!(error.to_string().contains("override_reason is required"));
        assert!(
            db.list_media_downloads(None, None, 100, 0)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            db.get_subscription(subscription.id)
                .await
                .unwrap()
                .unwrap()
                .next_episode,
            Some(3)
        );

        let forged = serde_json::json!({
            "candidate_id": candidate_id,
            "quality_profile_id": 1,
            "downloader_id": downloader_id,
            "subscription_id": subscription.id,
            "result": {
                "site_id": site_id,
                "torrent_id": "forged-torrent",
                "title": "Example.Show.S01E03.1080p.WEB-DL"
            }
        });
        assert!(serde_json::from_value::<QueueDownloadRequest>(forged).is_err());
    }

    #[tokio::test]
    async fn subscription_queue_rejects_candidates_from_another_target_or_indexer() {
        let (_dir, service, db, site_id, other_site, downloader_id) = test_service().await;
        let subscription = db
            .create_subscription(&NewSubscription {
                tmdb_id: 42,
                media_type: "tv".to_string(),
                tmdb_is_animation: false,
                tmdb_genres: Vec::new(),
                title: "Example Show".to_string(),
                original_title: None,
                aliases: Vec::new(),
                year: Some(2026),
                poster_path: None,
                season: Some(1),
                start_episode: Some(3),
                absolute_episode: None,
                quality_profile_id: 1,
                downloader_id,
                site_ids: vec![site_id],
                save_path: None,
                enabled: true,
            })
            .await
            .unwrap();
        let other_target = MediaTarget::Episode {
            tmdb_id: 42,
            titles: vec!["Example Show".to_string()],
            year: Some(2026),
            season: 1,
            episode: 4,
            allow_season_pack: false,
        };
        let candidate_id = new_candidate_id().unwrap();
        service.candidate_cache.lock().await.insert(
            candidate_id.clone(),
            SearchResult {
                site_id: other_site,
                source_site: "service-site-b".to_string(),
                torrent_id: "other-context".to_string(),
                title: "Example.Show.S01E03.1080p.WEB-DL".to_string(),
                detail_url: None,
                download_locator: Some("other-context".to_string()),
                magnet: None,
                size: 1024,
                seeders: 100,
                leechers: 0,
                publish_time: None,
            },
            Some(other_target),
            Some(1),
        );

        let error = service
            .queue_download(&QueueDownloadRequest {
                candidate_id,
                quality_profile_id: 1,
                downloader_id,
                subscription_id: Some(subscription.id),
                override_reason: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(error, MediaServiceError::Invalid(_)));
        assert!(
            error.to_string().contains("site is not configured")
                || error.to_string().contains("current target")
        );
    }

    #[tokio::test]
    async fn manual_queue_persists_only_the_cached_search_result() {
        let (_dir, service, _db, site_id, _other_site, downloader_id) = test_service().await;
        let candidate_id = new_candidate_id().unwrap();
        service.candidate_cache.lock().await.insert(
            candidate_id.clone(),
            SearchResult {
                site_id,
                source_site: "stale-client-name".to_string(),
                torrent_id: "authoritative-torrent".to_string(),
                title: "Authoritative.Release.1080p.WEB-DL".to_string(),
                detail_url: None,
                download_locator: Some("authoritative-torrent".to_string()),
                magnet: None,
                size: 2048,
                seeders: 20,
                leechers: 1,
                publish_time: None,
            },
            None,
            None,
        );

        let queued = service
            .queue_download(&QueueDownloadRequest {
                candidate_id,
                quality_profile_id: 1,
                downloader_id,
                subscription_id: None,
                override_reason: None,
            })
            .await
            .unwrap();
        assert_eq!(queued.torrent_id, "authoritative-torrent");
        assert_eq!(queued.title, "Authoritative.Release.1080p.WEB-DL");
        assert_eq!(queued.source_site, "service-site-a");
        let persisted: SearchResult = serde_json::from_str(&queued.release_json).unwrap();
        assert_eq!(persisted.torrent_id, "authoritative-torrent");
        assert_eq!(persisted.seeders, 20);
    }

    #[test]
    fn candidate_ids_are_random_opaque_tokens() {
        let first = new_candidate_id().unwrap();
        let second = new_candidate_id().unwrap();
        assert_ne!(first, second);
        assert_eq!(first.len(), 53);
        assert!(
            first
                .strip_prefix("cand_")
                .unwrap()
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        );
    }

    #[test]
    fn candidate_cache_discards_expired_and_oldest_entries() {
        let result = SearchResult {
            site_id: 1,
            source_site: "site".to_string(),
            torrent_id: "torrent".to_string(),
            title: "Title".to_string(),
            detail_url: None,
            download_locator: Some("torrent".to_string()),
            magnet: None,
            size: 1,
            seeders: 1,
            leechers: 0,
            publish_time: None,
        };
        let mut cache = CandidateCache::default();
        cache.insert("expired".to_string(), result.clone(), None, None);
        cache.entries.get_mut("expired").unwrap().expires_at = Instant::now();
        assert!(cache.get("expired").is_none());

        for index in 0..=CANDIDATE_CACHE_CAPACITY {
            cache.insert(format!("candidate-{index}"), result.clone(), None, None);
        }
        assert_eq!(cache.entries.len(), CANDIDATE_CACHE_CAPACITY);
        assert!(cache.get("candidate-0").is_none());
        assert!(
            cache
                .get(&format!("candidate-{CANDIDATE_CACHE_CAPACITY}"))
                .is_some()
        );
    }

    #[tokio::test]
    async fn target_search_falls_back_when_numbered_results_do_not_match_the_episode() {
        let (_dir, service, db, first_site, _second_site, _downloader_id) = test_service().await;
        let repeated_wrong = episode_search_result(
            first_site,
            "wrong-episode",
            "Example.Show.S01E02.1080p.WEB-DL.H264",
        );
        let mut outcomes = HashMap::new();
        outcomes.insert(
            "Example Show".to_string(),
            Ok(vec![
                repeated_wrong.clone(),
                episode_search_result(
                    first_site,
                    "wrong-season",
                    "Example.Show.S02E03.1080p.WEB-DL.H264",
                ),
                episode_search_result(
                    first_site,
                    "matching-episode",
                    "Example.Show.S01E03.1080p.WEB-DL.H264",
                ),
            ]),
        );
        let calls = install_query_aware_indexer(
            &service,
            &db,
            first_site,
            outcomes,
            Ok(vec![repeated_wrong]),
        )
        .await;

        let response = service
            .search_resources(&episode_search_request(
                vec![first_site],
                vec!["Example Show"],
                None,
            ))
            .await
            .unwrap();

        assert_eq!(
            response.queries,
            vec![
                "Example Show S01E03",
                "Example Show S1E3",
                "Example Show 03",
                "Example Show",
            ]
        );
        let accepted: Vec<_> = response
            .candidates
            .iter()
            .filter(|candidate| {
                candidate
                    .decision
                    .as_ref()
                    .is_some_and(|decision| decision.accepted)
            })
            .collect();
        assert_eq!(accepted.len(), 1);
        assert_eq!(accepted[0].result.torrent_id, "matching-episode");
        assert_eq!(
            response.candidates.len(),
            3,
            "cross-stage duplicates must collapse"
        );
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 4);
        assert_eq!(calls.last().map(String::as_str), Some("Example Show"));
    }

    #[tokio::test]
    async fn subscription_scan_queues_exactly_one_episode_found_by_title_only_fallback() {
        let (_dir, service, db, first_site, _second_site, downloader_id) = test_service().await;
        let target_key = crate::media::models::target_key("tv", 42, Some(1), Some(3), None);
        let subscription = db
            .create_subscription_with_targets(
                &NewSubscription {
                    tmdb_id: 42,
                    media_type: "tv".to_string(),
                    tmdb_is_animation: false,
                    tmdb_genres: Vec::new(),
                    title: "Example Show".to_string(),
                    original_title: None,
                    aliases: Vec::new(),
                    year: None,
                    poster_path: None,
                    season: Some(1),
                    start_episode: Some(3),
                    absolute_episode: None,
                    quality_profile_id: 1,
                    downloader_id,
                    site_ids: vec![first_site],
                    save_path: None,
                    enabled: true,
                },
                &[crate::media::progression::SubscriptionTargetSeed {
                    target_key: target_key.clone(),
                    season: 1,
                    episode: 3,
                    absolute_episode: None,
                    air_date: Some("2020-01-01".to_string()),
                    status: SubscriptionTargetSeedStatus::Pending,
                }],
                None,
                None,
            )
            .await
            .unwrap();
        let mut outcomes = HashMap::new();
        outcomes.insert(
            "Example Show".to_string(),
            Ok(vec![episode_search_result(
                first_site,
                "fallback-subscription-episode",
                "Example.Show.S01E03.1080p.WEB-DL.H264",
            )]),
        );
        install_query_aware_indexer(
            &service,
            &db,
            first_site,
            outcomes,
            Ok(vec![episode_search_result(
                first_site,
                "wrong-subscription-episode",
                "Example.Show.S01E02.1080p.WEB-DL.H264",
            )]),
        )
        .await;

        let run = service.run_subscription(subscription.id).await.unwrap();
        let snapshot = service
            .get_subscription_last_run(subscription.id)
            .await
            .unwrap()
            .unwrap();
        let downloads = db
            .list_media_downloads(None, Some("queued"), 100, 0)
            .await
            .unwrap();

        assert_eq!(run.target_key, target_key);
        assert_eq!(run.query_count, 4);
        assert_eq!(run.accepted_count, 1);
        assert_eq!(
            run.download
                .as_ref()
                .map(|download| download.torrent_id.as_str()),
            Some("fallback-subscription-episode")
        );
        assert_eq!(downloads.len(), 1);
        assert_eq!(downloads[0].torrent_id, "fallback-subscription-episode");
        assert_eq!(snapshot.queries.len(), run.query_count);
        assert_eq!(
            snapshot.best_candidate_id,
            run.download
                .as_ref()
                .and_then(|download| snapshot
                    .candidates
                    .iter()
                    .find(|candidate| { candidate.result.torrent_id == download.torrent_id }))
                .map(|candidate| candidate.candidate_id.clone())
        );
    }

    #[tokio::test]
    async fn target_search_does_not_fall_back_after_a_numbered_candidate_is_accepted() {
        let (_dir, service, db, first_site, _second_site, _downloader_id) = test_service().await;
        let calls = install_query_aware_indexer(
            &service,
            &db,
            first_site,
            HashMap::new(),
            Ok(vec![episode_search_result(
                first_site,
                "matching-numbered",
                "Example.Show.S01E03.1080p.WEB-DL.H264",
            )]),
        )
        .await;

        let response = service
            .search_resources(&episode_search_request(
                vec![first_site],
                vec!["Example Show"],
                None,
            ))
            .await
            .unwrap();

        assert_eq!(response.queries.len(), 3);
        assert!(!response.queries.iter().any(|query| query == "Example Show"));
        assert!(response.candidates.iter().any(|candidate| {
            candidate
                .decision
                .as_ref()
                .is_some_and(|decision| decision.accepted)
        }));
        assert!(
            !calls
                .lock()
                .unwrap()
                .iter()
                .any(|query| query == "Example Show")
        );
    }

    #[tokio::test]
    async fn explicit_resource_query_is_never_expanded_or_given_a_fallback() {
        let (_dir, service, db, first_site, _second_site, _downloader_id) = test_service().await;
        let calls =
            install_query_aware_indexer(&service, &db, first_site, HashMap::new(), Ok(Vec::new()))
                .await;

        let response = service
            .search_resources(&episode_search_request(
                vec![first_site],
                vec!["Example Show"],
                Some(" Custom S01E03 "),
            ))
            .await
            .unwrap();

        assert_eq!(response.queries, vec!["Custom S01E03"]);
        assert_eq!(*calls.lock().unwrap(), vec!["Custom S01E03"]);
    }

    #[tokio::test]
    async fn fallback_respects_the_total_query_budget_and_the_two_query_minimum() {
        let (_dir, service, db, first_site, _second_site, _downloader_id) = test_service().await;
        let mut settings = db.get_media_settings().await.unwrap();
        settings.max_search_queries = 4;
        db.update_media_settings(&settings).await.unwrap();
        let calls =
            install_query_aware_indexer(&service, &db, first_site, HashMap::new(), Ok(Vec::new()))
                .await;

        let response = service
            .search_resources(&episode_search_request(
                vec![first_site],
                vec!["Example Show", "Original Title", "Third Alias"],
                None,
            ))
            .await
            .unwrap();

        assert_eq!(
            response.queries,
            vec![
                "Example Show S01E03",
                "Example Show S1E3",
                "Example Show",
                "Original Title",
            ]
        );
        assert_eq!(calls.lock().unwrap().len(), 4);
        assert_eq!(
            response.queries.iter().collect::<HashSet<_>>().len(),
            response.queries.len()
        );

        settings.max_search_queries = 1;
        let normalized = db.update_media_settings(&settings).await.unwrap();
        assert_eq!(normalized.max_search_queries, 2);
    }

    #[tokio::test]
    async fn failed_numbered_requests_do_not_trigger_a_title_only_fallback() {
        let (_dir, service, db, first_site, _second_site, _downloader_id) = test_service().await;
        let mut outcomes = HashMap::new();
        outcomes.insert(
            "Example Show".to_string(),
            Ok(vec![episode_search_result(
                first_site,
                "would-match",
                "Example.Show.S01E03.1080p.WEB-DL.H264",
            )]),
        );
        let calls = install_query_aware_indexer(
            &service,
            &db,
            first_site,
            outcomes,
            Err(IndexerError::Http("tracker unavailable".to_string())),
        )
        .await;

        let response = service
            .search_resources(&episode_search_request(
                vec![first_site],
                vec!["Example Show"],
                None,
            ))
            .await
            .unwrap();

        assert_eq!(response.queries.len(), 3);
        assert_eq!(response.errors.len(), 3);
        assert_eq!(response.successful_sites, 0);
        assert!(
            !calls
                .lock()
                .unwrap()
                .iter()
                .any(|query| query == "Example Show")
        );
    }

    #[tokio::test]
    async fn staged_search_counts_sites_that_succeed_in_different_phases_once_each() {
        let (_dir, service, db, first_site, second_site, _downloader_id) = test_service().await;
        let mut first_outcomes = HashMap::new();
        first_outcomes.insert(
            "Example Show".to_string(),
            Err(IndexerError::Http("fallback failed".to_string())),
        );
        install_query_aware_indexer(&service, &db, first_site, first_outcomes, Ok(Vec::new()))
            .await;
        let mut second_outcomes = HashMap::new();
        second_outcomes.insert(
            "Example Show".to_string(),
            Ok(vec![episode_search_result(
                second_site,
                "matching-fallback",
                "Example.Show.S01E03.1080p.WEB-DL.H264",
            )]),
        );
        install_query_aware_indexer(
            &service,
            &db,
            second_site,
            second_outcomes,
            Err(IndexerError::Http("numbered search failed".to_string())),
        )
        .await;

        let response = service
            .search_resources(&episode_search_request(
                vec![first_site, second_site],
                vec!["Example Show"],
                None,
            ))
            .await
            .unwrap();

        assert_eq!(response.successful_sites, 2);
        assert_eq!(response.errors.len(), 4);
        assert!(response.candidates.iter().any(|candidate| {
            candidate.result.torrent_id == "matching-fallback"
                && candidate
                    .decision
                    .as_ref()
                    .is_some_and(|decision| decision.accepted)
        }));
    }

    #[tokio::test]
    async fn oversized_public_search_returns_only_immediately_resolvable_candidate_ids() {
        let (_dir, service, db, first_site, _second_site, _downloader_id) = test_service().await;
        let results = (0..RESOURCE_SEARCH_RESULT_LIMIT + 37)
            .map(resource_search_result)
            .collect();
        install_static_indexer(&service, &db, first_site, results).await;

        let response = service
            .search_resources(&resource_search_request(first_site))
            .await
            .unwrap();

        assert_eq!(response.candidates.len(), RESOURCE_SEARCH_RESULT_LIMIT);
        let mut ids = HashSet::new();
        let mut cache = service.candidate_cache.lock().await;
        for candidate in response.candidates {
            assert!(ids.insert(candidate.candidate_id.clone()));
            let cached = cache
                .get(&candidate.candidate_id)
                .expect("every returned candidate must still be cached");
            assert_eq!(cached.result.torrent_id, candidate.result.torrent_id);
        }
    }

    #[tokio::test]
    async fn subscription_heartbeat_search_does_not_evict_manual_candidates() {
        let (_dir, service, db, first_site, _second_site, downloader_id) = test_service().await;
        let subscription = db
            .create_subscription(&NewSubscription {
                tmdb_id: 42,
                media_type: "movie".to_string(),
                tmdb_is_animation: false,
                tmdb_genres: Vec::new(),
                title: "Example Show".to_string(),
                original_title: None,
                aliases: Vec::new(),
                year: Some(2026),
                poster_path: None,
                season: None,
                start_episode: None,
                absolute_episode: None,
                quality_profile_id: 1,
                downloader_id,
                site_ids: vec![first_site],
                save_path: None,
                enabled: true,
            })
            .await
            .unwrap();
        let manual_id = new_candidate_id().unwrap();
        {
            let mut cache = service.candidate_cache.lock().await;
            cache.insert(manual_id.clone(), resource_search_result(0), None, None);
            for index in 1..CANDIDATE_CACHE_CAPACITY {
                cache.insert(
                    format!("existing-{index}"),
                    resource_search_result(index),
                    None,
                    None,
                );
            }
            assert_eq!(cache.entries.len(), CANDIDATE_CACHE_CAPACITY);
        }

        let results = (0..RESOURCE_SEARCH_RESULT_LIMIT + 1)
            .map(|index| resource_search_result(index + CANDIDATE_CACHE_CAPACITY))
            .collect();
        install_static_indexer(&service, &db, first_site, results).await;
        let (automatic_search, _version) = service
            .search_resources_with_subscription_heartbeat(
                &subscription,
                "test-owner",
                30,
                resource_search_request(first_site),
            )
            .await
            .unwrap();
        let automatic_candidates = automatic_search.unwrap().candidates;

        assert_eq!(automatic_candidates.len(), RESOURCE_SEARCH_RESULT_LIMIT);
        let mut cache = service.candidate_cache.lock().await;
        assert_eq!(cache.entries.len(), CANDIDATE_CACHE_CAPACITY);
        assert!(cache.get(&manual_id).is_some());
        assert!(
            automatic_candidates
                .iter()
                .all(|candidate| cache.get(&candidate.candidate_id).is_none())
        );
    }

    #[tokio::test]
    async fn subscription_scan_persists_queries_and_candidate_decisions() {
        let (_dir, service, db, first_site, _second_site, downloader_id) = test_service().await;
        let subscription = db
            .create_subscription(&NewSubscription {
                tmdb_id: 42,
                media_type: "movie".to_string(),
                tmdb_is_animation: false,
                tmdb_genres: Vec::new(),
                title: "Example Show".to_string(),
                original_title: None,
                aliases: Vec::new(),
                year: Some(2026),
                poster_path: None,
                season: None,
                start_episode: None,
                absolute_episode: None,
                quality_profile_id: 1,
                downloader_id,
                site_ids: vec![first_site],
                save_path: None,
                enabled: true,
            })
            .await
            .unwrap();
        install_static_indexer(&service, &db, first_site, vec![resource_search_result(1)]).await;

        let run = service.run_subscription(subscription.id).await.unwrap();
        let snapshot = service
            .get_subscription_last_run(subscription.id)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(snapshot.target_key, run.target_key);
        assert_eq!(snapshot.queries.len(), run.query_count);
        assert_eq!(snapshot.candidates.len(), run.candidate_count);
        assert_eq!(snapshot.candidates.len(), 1);
        assert!(snapshot.candidates[0].decision.is_some());
        assert!(snapshot.error.is_none());
    }

    #[tokio::test]
    async fn requested_indexer_order_defines_site_priority() {
        let (_dir, service, _db, first_site, second_site, _downloader_id) = test_service().await;
        let (adapters, errors, priorities) = service
            .resolve_indexers(&[second_site, first_site])
            .await
            .unwrap();
        assert_eq!(adapters.len(), 2);
        assert!(errors.is_empty());
        assert_eq!(priorities.get(&second_site), Some(&0));
        assert_eq!(priorities.get(&first_site), Some(&1));

        let duplicate = match service.resolve_indexers(&[first_site, first_site]).await {
            Ok(_) => panic!("duplicate site ids must be rejected"),
            Err(error) => error,
        };
        assert!(matches!(duplicate, MediaServiceError::Invalid(_)));
    }
}
