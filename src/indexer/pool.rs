use std::collections::{HashMap, HashSet, hash_map::DefaultHasher};
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::Duration;

use futures::stream::{FuturesUnordered, StreamExt};
use reqwest::redirect::Policy;
use reqwest::{Client, Proxy};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::site::SiteRecord;

use super::access::{IndexerAccessPolicy, OriginAccessGate, default_indexer_access_policy};
use super::{
    IndexerAdapter, IndexerCapabilities, IndexerError, IndexerFuture, SearchRequest, SearchResult,
    create_indexer, normalize_base_url,
};

struct CachedIndexer {
    config_digest: u64,
    adapter: Arc<dyn IndexerAdapter>,
}

// Wrappers for equivalent origins share state owned by IndexerPool, so rebuilding an adapter
// cannot bypass an in-flight request, the minimum interval, or a 429 cooldown.
struct RateLimitedIndexer {
    inner: Arc<dyn IndexerAdapter>,
    access_gate: Arc<OriginAccessGate>,
    access_key: String,
}

impl IndexerAdapter for RateLimitedIndexer {
    fn site_id(&self) -> i64 {
        self.inner.site_id()
    }

    fn site_name(&self) -> &str {
        self.inner.site_name()
    }

    fn capabilities(&self) -> IndexerCapabilities {
        self.inner.capabilities()
    }

    fn access_key(&self) -> Option<&str> {
        Some(&self.access_key)
    }

    fn search<'a>(&'a self, request: &'a SearchRequest) -> IndexerFuture<'a, Vec<SearchResult>> {
        Box::pin(async move {
            let _operation = self.access_gate.lock_operation().await;
            match self.inner.search(request).await {
                Err(IndexerError::RateLimited(limit)) => {
                    let limit = self.access_gate.observe_rate_limit(limit).await;
                    Err(IndexerError::RateLimited(limit))
                }
                result => result,
            }
        })
    }

    fn fetch_torrent<'a>(&'a self, result: &'a SearchResult) -> IndexerFuture<'a, Vec<u8>> {
        Box::pin(async move {
            let _operation = self.access_gate.lock_operation().await;
            match self.inner.fetch_torrent(result).await {
                Err(IndexerError::RateLimited(limit)) => {
                    let limit = self.access_gate.observe_rate_limit(limit).await;
                    Err(IndexerError::RateLimited(limit))
                }
                result => result,
            }
        })
    }
}

/// Reuses HTTP clients and adapters until any site, credential or effective proxy setting changes.
pub struct IndexerPool {
    cache: Mutex<HashMap<i64, CachedIndexer>>,
    access_gates: Mutex<HashMap<String, Arc<OriginAccessGate>>>,
    access_policy: IndexerAccessPolicy,
}

#[allow(dead_code)]
impl IndexerPool {
    pub fn new() -> Arc<Self> {
        Self::new_with_policy(default_indexer_access_policy())
    }

    fn new_with_policy(access_policy: IndexerAccessPolicy) -> Arc<Self> {
        Arc::new(Self {
            cache: Mutex::new(HashMap::new()),
            access_gates: Mutex::new(HashMap::new()),
            access_policy,
        })
    }

    pub async fn get_or_create(
        &self,
        record: &SiteRecord,
        global_proxy: Option<&str>,
    ) -> Result<Arc<dyn IndexerAdapter>, IndexerError> {
        let effective_proxy = record
            .use_proxy
            .then_some(global_proxy)
            .flatten()
            .map(str::trim)
            .filter(|proxy| !proxy.is_empty());
        let digest = config_digest(record, effective_proxy);
        let rate_limit_key = site_rate_limit_key(record)?;
        let access_gate = self.get_or_create_access_gate(&rate_limit_key).await;
        let mut cache = self.cache.lock().await;
        if let Some(cached) = cache.get(&record.id)
            && cached.config_digest == digest
        {
            return Ok(Arc::clone(&cached.adapter));
        }

        let client = build_indexer_client(record, effective_proxy)?;
        let inner = create_indexer(record, client, Arc::clone(&access_gate))?;
        let adapter: Arc<dyn IndexerAdapter> = Arc::new(RateLimitedIndexer {
            inner,
            access_gate,
            access_key: rate_limit_key,
        });
        cache.insert(
            record.id,
            CachedIndexer {
                config_digest: digest,
                adapter: Arc::clone(&adapter),
            },
        );
        Ok(adapter)
    }

    pub async fn invalidate(&self, site_id: i64) {
        self.cache.lock().await.remove(&site_id);
    }

    pub async fn clear(&self) {
        self.cache.lock().await.clear();
    }

    pub async fn len(&self) -> usize {
        self.cache.lock().await.len()
    }

    pub async fn is_empty(&self) -> bool {
        self.len().await == 0
    }

    async fn get_or_create_access_gate(&self, key: &str) -> Arc<OriginAccessGate> {
        let mut gates = self.access_gates.lock().await;
        gates
            .entry(key.to_string())
            .or_insert_with(|| Arc::new(OriginAccessGate::new(self.access_policy)))
            .clone()
    }

    /// RSS requests share the exact origin gates used by search and torrent retrieval.
    pub(crate) async fn access_gate_for_url(&self, url: &reqwest::Url) -> Arc<OriginAccessGate> {
        self.get_or_create_access_gate(&url.origin().ascii_serialization())
            .await
    }

    #[cfg(test)]
    pub(crate) async fn insert_for_test(
        &self,
        record: &SiteRecord,
        global_proxy: Option<&str>,
        adapter: Arc<dyn IndexerAdapter>,
    ) {
        let effective_proxy = record
            .use_proxy
            .then_some(global_proxy)
            .flatten()
            .map(str::trim)
            .filter(|proxy| !proxy.is_empty());
        let rate_limit_key = site_rate_limit_key(record).expect("test site URL must be valid");
        let access_gate = self.get_or_create_access_gate(&rate_limit_key).await;
        let adapter: Arc<dyn IndexerAdapter> = Arc::new(RateLimitedIndexer {
            inner: adapter,
            access_gate,
            access_key: rate_limit_key,
        });
        self.cache.lock().await.insert(
            record.id,
            CachedIndexer {
                config_digest: config_digest(record, effective_proxy),
                adapter,
            },
        );
    }
}

fn site_rate_limit_key(record: &SiteRecord) -> Result<String, IndexerError> {
    Ok(normalize_base_url(&record.base_url)?
        .origin()
        .ascii_serialization())
}

fn build_indexer_client(
    record: &SiteRecord,
    effective_proxy: Option<&str>,
) -> Result<Client, IndexerError> {
    normalize_base_url(&record.base_url)?;
    let mut builder = Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .redirect(Policy::none());
    if let Some(proxy) = effective_proxy {
        builder = builder.proxy(Proxy::all(proxy).map_err(|error| {
            IndexerError::Configuration(format!("invalid indexer proxy URL: {error}"))
        })?);
    }
    builder.build().map_err(|error| {
        IndexerError::Configuration(format!("failed to build indexer HTTP client: {error}"))
    })
}

fn config_digest(record: &SiteRecord, effective_proxy: Option<&str>) -> u64 {
    let mut hasher = DefaultHasher::new();
    record.id.hash(&mut hasher);
    record.name.hash(&mut hasher);
    record.site_type.hash(&mut hasher);
    record.base_url.hash(&mut hasher);
    record.auth_config.hash(&mut hasher);
    record.request_headers.hash(&mut hasher);
    record.use_proxy.hash(&mut hasher);
    effective_proxy.hash(&mut hasher);
    hasher.finish()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SiteSearchError {
    pub site_id: i64,
    pub source_site: String,
    pub query: String,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregateSearchResult {
    pub results: Vec<SearchResult>,
    pub errors: Vec<SiteSearchError>,
    pub total_sites: usize,
    pub total_queries: usize,
    pub successful_sites: usize,
    pub successful_requests: usize,
    #[serde(skip)]
    successful_site_ids: HashSet<i64>,
}

impl AggregateSearchResult {
    pub(crate) fn merge(mut self, other: Self) -> Self {
        self.results.extend(other.results);
        self.results = deduplicate_results(self.results);

        self.errors.extend(other.errors);
        self.errors.sort_by(|left, right| {
            left.site_id
                .cmp(&right.site_id)
                .then_with(|| left.query.cmp(&right.query))
                .then_with(|| left.code.cmp(&right.code))
                .then_with(|| left.message.cmp(&right.message))
        });
        self.errors.dedup();

        self.total_sites = self.total_sites.max(other.total_sites);
        self.total_queries += other.total_queries;
        self.successful_requests += other.successful_requests;
        let reported_successful_sites = self.successful_sites.max(other.successful_sites);
        self.successful_site_ids.extend(other.successful_site_ids);
        self.successful_sites = self
            .successful_site_ids
            .len()
            .max(reported_successful_sites);
        self
    }
}

pub struct IndexerAggregator {
    max_concurrency: usize,
}

impl IndexerAggregator {
    pub fn new(max_concurrency: usize) -> Self {
        Self {
            max_concurrency: max_concurrency.max(1),
        }
    }

    /// Search different sites concurrently while keeping every query for one site sequential.
    /// A failed pair is recorded and never cancels successful work from another site.
    pub async fn search(
        &self,
        indexers: &[Arc<dyn IndexerAdapter>],
        requests: &[SearchRequest],
    ) -> AggregateSearchResult {
        let requests = unique_requests(requests);
        let total_sites = indexers.len();
        let total_queries = requests.len();
        let jobs = group_indexers_by_access_key(indexers)
            .into_iter()
            .map(|adapters| (adapters, requests.clone()))
            .collect::<Vec<_>>();

        let mut jobs = jobs.into_iter();
        let mut pending = FuturesUnordered::new();
        for _ in 0..self.max_concurrency {
            if let Some((adapters, requests)) = jobs.next() {
                pending.push(run_origin_search_jobs(adapters, requests));
            }
        }
        let mut completed = Vec::new();
        while let Some(results) = pending.next().await {
            completed.extend(results);
            if let Some((adapters, requests)) = jobs.next() {
                pending.push(run_origin_search_jobs(adapters, requests));
            }
        }

        let mut deduplicated = HashMap::<String, SearchResult>::new();
        let mut errors = Vec::new();
        let mut successful_requests = 0;
        let mut successful_sites = HashSet::new();
        for (site_id, source_site, query, outcome) in completed {
            match outcome {
                Ok(results) => {
                    successful_requests += 1;
                    successful_sites.insert(site_id);
                    for result in results {
                        let key = dedupe_key(&result);
                        match deduplicated.get_mut(&key) {
                            Some(current) if result_is_preferred(&result, current) => {
                                *current = result;
                            }
                            None => {
                                deduplicated.insert(key, result);
                            }
                            _ => {}
                        }
                    }
                }
                Err(error) => errors.push(SiteSearchError {
                    site_id,
                    source_site,
                    query,
                    code: error.code().to_string(),
                    message: error.to_string(),
                }),
            }
        }

        let mut results: Vec<_> = deduplicated.into_values().collect();
        results.sort_by(|left, right| {
            right
                .publish_time
                .cmp(&left.publish_time)
                .then_with(|| right.seeders.cmp(&left.seeders))
                .then_with(|| left.title.to_lowercase().cmp(&right.title.to_lowercase()))
                .then_with(|| left.site_id.cmp(&right.site_id))
                .then_with(|| left.torrent_id.cmp(&right.torrent_id))
        });
        errors.sort_by(|left, right| {
            left.site_id
                .cmp(&right.site_id)
                .then_with(|| left.query.cmp(&right.query))
        });

        AggregateSearchResult {
            results,
            errors,
            total_sites,
            total_queries,
            successful_sites: successful_sites.len(),
            successful_requests,
            successful_site_ids: successful_sites,
        }
    }
}

fn group_indexers_by_access_key(
    indexers: &[Arc<dyn IndexerAdapter>],
) -> Vec<Vec<Arc<dyn IndexerAdapter>>> {
    let mut positions = HashMap::<String, usize>::new();
    let mut groups: Vec<Vec<Arc<dyn IndexerAdapter>>> = Vec::new();
    for (index, adapter) in indexers.iter().enumerate() {
        let key = adapter
            .access_key()
            .map(str::to_string)
            .unwrap_or_else(|| format!("unscoped:{index}"));
        if let Some(position) = positions.get(&key).copied() {
            groups[position].push(Arc::clone(adapter));
        } else {
            positions.insert(key, groups.len());
            groups.push(vec![Arc::clone(adapter)]);
        }
    }
    groups
}

async fn run_origin_search_jobs(
    adapters: Vec<Arc<dyn IndexerAdapter>>,
    requests: Vec<SearchRequest>,
) -> Vec<(i64, String, String, Result<Vec<SearchResult>, IndexerError>)> {
    let mut completed = Vec::with_capacity(adapters.len().saturating_mul(requests.len()));
    let mut adapters = adapters.into_iter();
    while let Some(adapter) = adapters.next() {
        let site_results = run_site_search_jobs(adapter, requests.clone()).await;
        let rate_limit = site_results
            .iter()
            .find_map(|(_, _, _, result)| match result {
                Err(error @ IndexerError::RateLimited(_)) => Some(error.clone()),
                _ => None,
            });
        completed.extend(site_results);
        if let Some(error) = rate_limit {
            for adapter in adapters {
                let site_id = adapter.site_id();
                let site_name = adapter.site_name().to_string();
                completed.extend(requests.iter().map(|request| {
                    (
                        site_id,
                        site_name.clone(),
                        request.query.clone(),
                        Err(error.clone()),
                    )
                }));
            }
            break;
        }
    }
    completed
}

async fn run_site_search_jobs(
    adapter: Arc<dyn IndexerAdapter + 'static>,
    requests: Vec<SearchRequest>,
) -> Vec<(i64, String, String, Result<Vec<SearchResult>, IndexerError>)> {
    let site_id = adapter.site_id();
    let site_name = adapter.site_name().to_string();
    let mut completed = Vec::with_capacity(requests.len());
    let mut requests = requests.into_iter();
    while let Some(request) = requests.next() {
        let query = request.query.clone();
        let result = adapter.search(&request).await;
        let rate_limit = match &result {
            Err(error @ IndexerError::RateLimited(_)) => Some(error.clone()),
            _ => None,
        };
        completed.push((site_id, site_name.clone(), query, result));
        if let Some(error) = rate_limit {
            completed.extend(requests.map(|request| {
                (
                    site_id,
                    site_name.clone(),
                    request.query,
                    Err(error.clone()),
                )
            }));
            break;
        }
    }
    completed
}

impl Default for IndexerAggregator {
    fn default() -> Self {
        Self::new(4)
    }
}

fn unique_requests(requests: &[SearchRequest]) -> Vec<SearchRequest> {
    let mut seen = HashSet::new();
    requests
        .iter()
        .filter(|request| {
            seen.insert((
                request.query.trim().to_lowercase(),
                request.page,
                request.page_size,
            ))
        })
        .cloned()
        .collect()
}

fn dedupe_key(result: &SearchResult) -> String {
    if let Some(info_hash) = result.magnet.as_deref().and_then(magnet_info_hash) {
        return format!("hash:{info_hash}");
    }

    if result.size > 0 {
        let normalized_title: String = result
            .title
            .chars()
            .flat_map(char::to_lowercase)
            .filter(|character| character.is_alphanumeric())
            .collect();
        if !normalized_title.is_empty() {
            return format!("release:{normalized_title}:{}", result.size);
        }
    }
    format!("site:{}:{}", result.site_id, result.torrent_id)
}

fn deduplicate_results(results: Vec<SearchResult>) -> Vec<SearchResult> {
    let mut deduplicated = HashMap::<String, SearchResult>::new();
    for result in results {
        let key = dedupe_key(&result);
        match deduplicated.get_mut(&key) {
            Some(current) if result_is_preferred(&result, current) => *current = result,
            None => {
                deduplicated.insert(key, result);
            }
            _ => {}
        }
    }
    let mut results: Vec<_> = deduplicated.into_values().collect();
    results.sort_by(|left, right| {
        right
            .publish_time
            .cmp(&left.publish_time)
            .then_with(|| right.seeders.cmp(&left.seeders))
            .then_with(|| left.title.to_lowercase().cmp(&right.title.to_lowercase()))
            .then_with(|| left.site_id.cmp(&right.site_id))
            .then_with(|| left.torrent_id.cmp(&right.torrent_id))
    });
    results
}

fn magnet_info_hash(magnet: &str) -> Option<String> {
    let url = reqwest::Url::parse(magnet).ok()?;
    url.query_pairs().find_map(|(key, value)| {
        if key != "xt" {
            return None;
        }
        value
            .to_ascii_lowercase()
            .strip_prefix("urn:btih:")
            .filter(|hash| !hash.is_empty())
            .map(str::to_string)
    })
}

fn result_is_preferred(candidate: &SearchResult, current: &SearchResult) -> bool {
    candidate.seeders > current.seeders
        || (candidate.seeders == current.seeders
            && (candidate.publish_time > current.publish_time
                || (candidate.publish_time == current.publish_time
                    && (candidate.site_id, &candidate.torrent_id)
                        < (current.site_id, &current.torrent_id))))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex as StdMutex};
    use std::time::{Duration, Instant};

    use super::{
        IndexerAccessPolicy, IndexerAggregator, IndexerPool, config_digest, site_rate_limit_key,
    };
    use crate::indexer::{
        IndexerAdapter, IndexerCapabilities, IndexerError, IndexerFuture, IndexerRateLimit,
        SearchRequest, SearchResult,
    };
    use crate::site::SiteRecord;

    struct FakeIndexer {
        site_id: i64,
        site_name: String,
        seeders: u32,
        fail_query: Option<String>,
    }

    struct ProbeIndexer {
        site_id: i64,
        site_name: String,
        delay: Duration,
        rate_limit_first: bool,
        calls: AtomicUsize,
        active: AtomicUsize,
        max_active: AtomicUsize,
        global_active: Arc<AtomicUsize>,
        max_global_active: Arc<AtomicUsize>,
        starts: Arc<StdMutex<Vec<Instant>>>,
    }

    impl ProbeIndexer {
        fn new(
            site_id: i64,
            delay: Duration,
            rate_limit_first: bool,
            global_active: Arc<AtomicUsize>,
            max_global_active: Arc<AtomicUsize>,
        ) -> Arc<Self> {
            Arc::new(Self {
                site_id,
                site_name: format!("Site {site_id}"),
                delay,
                rate_limit_first,
                calls: AtomicUsize::new(0),
                active: AtomicUsize::new(0),
                max_active: AtomicUsize::new(0),
                global_active,
                max_global_active,
                starts: Arc::new(StdMutex::new(Vec::new())),
            })
        }
    }

    impl IndexerAdapter for ProbeIndexer {
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
            Box::pin(async move {
                let call = self.calls.fetch_add(1, Ordering::SeqCst);
                let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
                self.max_active.fetch_max(active, Ordering::SeqCst);
                let global = self.global_active.fetch_add(1, Ordering::SeqCst) + 1;
                self.max_global_active.fetch_max(global, Ordering::SeqCst);
                self.starts.lock().unwrap().push(Instant::now());
                tokio::time::sleep(self.delay).await;
                self.active.fetch_sub(1, Ordering::SeqCst);
                self.global_active.fetch_sub(1, Ordering::SeqCst);

                if self.rate_limit_first && call == 0 {
                    return Err(IndexerError::RateLimited(IndexerRateLimit::new(None)));
                }
                Ok(vec![SearchResult {
                    site_id: self.site_id,
                    source_site: self.site_name.clone(),
                    torrent_id: format!("{}-{}", self.site_id, request.query),
                    title: format!("{}.S01E01", request.query),
                    detail_url: None,
                    download_locator: Some("id".to_string()),
                    magnet: None,
                    size: 1024,
                    seeders: 1,
                    leechers: 0,
                    publish_time: None,
                }])
            })
        }

        fn fetch_torrent<'a>(&'a self, _result: &'a SearchResult) -> IndexerFuture<'a, Vec<u8>> {
            Box::pin(async move {
                let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
                self.max_active.fetch_max(active, Ordering::SeqCst);
                let global = self.global_active.fetch_add(1, Ordering::SeqCst) + 1;
                self.max_global_active.fetch_max(global, Ordering::SeqCst);
                self.starts.lock().unwrap().push(Instant::now());
                tokio::time::sleep(self.delay).await;
                self.active.fetch_sub(1, Ordering::SeqCst);
                self.global_active.fetch_sub(1, Ordering::SeqCst);
                Ok(vec![1, 2, 3])
            })
        }
    }

    fn site_record(id: i64, base_url: &str) -> SiteRecord {
        SiteRecord {
            id,
            name: format!("Site {id}"),
            site_type: "nexusphp".to_string(),
            base_url: base_url.to_string(),
            auth_config: r#"{"auth_type":"api_key","api_key":"test"}"#.to_string(),
            request_headers: "[]".to_string(),
            use_proxy: false,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    impl IndexerAdapter for FakeIndexer {
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
            Box::pin(async move {
                if self.fail_query.as_deref() == Some(request.query.as_str()) {
                    return Err(IndexerError::Http("isolated failure".to_string()));
                }
                Ok(vec![SearchResult {
                    site_id: self.site_id,
                    source_site: self.site_name.clone(),
                    torrent_id: format!("{}-{}", self.site_id, request.query),
                    title: "Same.Release.S01E01".to_string(),
                    detail_url: None,
                    download_locator: Some("id".to_string()),
                    magnet: None,
                    size: 1024,
                    seeders: self.seeders,
                    leechers: 0,
                    publish_time: None,
                }])
            })
        }

        fn fetch_torrent<'a>(&'a self, _result: &'a SearchResult) -> IndexerFuture<'a, Vec<u8>> {
            Box::pin(async move {
                Err(IndexerError::Configuration(
                    "fake fetch is unsupported".to_string(),
                ))
            })
        }
    }

    #[tokio::test]
    async fn aggregates_concurrently_deduplicates_and_isolates_errors() {
        let indexers: Vec<Arc<dyn IndexerAdapter>> = vec![
            Arc::new(FakeIndexer {
                site_id: 1,
                site_name: "One".to_string(),
                seeders: 3,
                fail_query: None,
            }),
            Arc::new(FakeIndexer {
                site_id: 2,
                site_name: "Two".to_string(),
                seeders: 9,
                fail_query: Some("second".to_string()),
            }),
        ];
        let requests = vec![SearchRequest::new("first"), SearchRequest::new("second")];
        let aggregate = IndexerAggregator::new(2).search(&indexers, &requests).await;

        assert_eq!(aggregate.total_sites, 2);
        assert_eq!(aggregate.total_queries, 2);
        assert_eq!(aggregate.successful_requests, 3);
        assert_eq!(aggregate.errors.len(), 1);
        assert_eq!(aggregate.errors[0].site_id, 2);
        assert_eq!(aggregate.results.len(), 1);
        assert_eq!(aggregate.results[0].site_id, 2);
        assert_eq!(aggregate.results[0].seeders, 9);
    }

    #[tokio::test]
    async fn aggregator_serializes_each_site_while_different_sites_still_overlap() {
        let global_active = Arc::new(AtomicUsize::new(0));
        let max_global_active = Arc::new(AtomicUsize::new(0));
        let first = ProbeIndexer::new(
            1,
            Duration::from_millis(20),
            false,
            Arc::clone(&global_active),
            Arc::clone(&max_global_active),
        );
        let second = ProbeIndexer::new(
            2,
            Duration::from_millis(20),
            false,
            Arc::clone(&global_active),
            Arc::clone(&max_global_active),
        );
        let indexers: Vec<Arc<dyn IndexerAdapter>> = vec![first.clone(), second.clone()];
        let requests = vec![
            SearchRequest::new("first"),
            SearchRequest::new("second"),
            SearchRequest::new("third"),
        ];

        let aggregate = IndexerAggregator::new(8).search(&indexers, &requests).await;

        assert_eq!(aggregate.successful_requests, 6);
        assert_eq!(first.max_active.load(Ordering::SeqCst), 1);
        assert_eq!(second.max_active.load(Ordering::SeqCst), 1);
        assert_eq!(max_global_active.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn operation_gate_survives_adapter_rebuilds_and_serializes_concurrent_searches() {
        let policy = IndexerAccessPolicy {
            min_request_interval: Duration::from_millis(30),
            default_cooldown: Duration::from_millis(40),
        };
        let pool = IndexerPool::new_with_policy(policy);
        let record = site_record(1, "https://tracker.example/base/");
        let global_active = Arc::new(AtomicUsize::new(0));
        let max_global_active = Arc::new(AtomicUsize::new(0));
        let first = ProbeIndexer::new(
            1,
            Duration::from_millis(5),
            false,
            Arc::clone(&global_active),
            Arc::clone(&max_global_active),
        );
        pool.insert_for_test(&record, None, first.clone()).await;
        let old_adapter = pool.get_or_create(&record, None).await.unwrap();

        pool.invalidate(record.id).await;
        let replacement = ProbeIndexer::new(
            1,
            Duration::from_millis(5),
            false,
            Arc::clone(&global_active),
            Arc::clone(&max_global_active),
        );
        pool.insert_for_test(&record, None, replacement.clone())
            .await;
        let new_adapter = pool.get_or_create(&record, None).await.unwrap();
        let first_request = SearchRequest::new("first");
        let second_request = SearchRequest::new("second");

        let (first_result, second_result) = tokio::join!(
            old_adapter.search(&first_request),
            new_adapter.search(&second_request)
        );

        assert!(first_result.is_ok());
        assert!(second_result.is_ok());
        assert_eq!(max_global_active.load(Ordering::SeqCst), 1);
        assert_eq!(first.calls.load(Ordering::SeqCst), 1);
        assert_eq!(replacement.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn rate_limit_response_stops_the_remaining_queries_for_that_site() {
        let global_active = Arc::new(AtomicUsize::new(0));
        let max_global_active = Arc::new(AtomicUsize::new(0));
        let indexer = ProbeIndexer::new(1, Duration::ZERO, true, global_active, max_global_active);
        let indexers: Vec<Arc<dyn IndexerAdapter>> = vec![indexer.clone()];
        let requests = vec![
            SearchRequest::new("first"),
            SearchRequest::new("second"),
            SearchRequest::new("third"),
        ];

        let aggregate = IndexerAggregator::new(8).search(&indexers, &requests).await;

        assert_eq!(indexer.calls.load(Ordering::SeqCst), 1);
        assert_eq!(aggregate.successful_requests, 0);
        assert_eq!(aggregate.errors.len(), 3);
        assert!(
            aggregate
                .errors
                .iter()
                .all(|error| error.code == "rate_limited")
        );
    }

    #[tokio::test]
    async fn pool_records_rate_limit_errors_returned_by_an_adapter() {
        let pool = IndexerPool::new_with_policy(IndexerAccessPolicy {
            min_request_interval: Duration::ZERO,
            default_cooldown: Duration::from_secs(30),
        });
        let record = site_record(1, "https://tracker.example/");
        let indexer = ProbeIndexer::new(
            1,
            Duration::ZERO,
            true,
            Arc::new(AtomicUsize::new(0)),
            Arc::new(AtomicUsize::new(0)),
        );
        pool.insert_for_test(&record, None, indexer.clone()).await;
        let adapter = pool.get_or_create(&record, None).await.unwrap();

        let error = adapter
            .search(&SearchRequest::new("limited"))
            .await
            .unwrap_err();
        assert!(matches!(error, IndexerError::RateLimited(_)));
        assert_eq!(indexer.calls.load(Ordering::SeqCst), 1);

        let key = site_rate_limit_key(&record).unwrap();
        let gate = pool.get_or_create_access_gate(&key).await;
        assert!(matches!(
            gate.send(reqwest::Client::new().get("http://127.0.0.1:1/blocked"))
                .await,
            Err(IndexerError::RateLimited(_))
        ));
    }

    #[tokio::test]
    async fn search_and_torrent_fetch_share_the_same_origin_operation_gate() {
        let policy = IndexerAccessPolicy {
            min_request_interval: Duration::from_millis(25),
            default_cooldown: Duration::from_millis(40),
        };
        let pool = IndexerPool::new_with_policy(policy);
        let record = site_record(1, "https://tracker.example/");
        let indexer = ProbeIndexer::new(
            1,
            Duration::from_millis(5),
            false,
            Arc::new(AtomicUsize::new(0)),
            Arc::new(AtomicUsize::new(0)),
        );
        pool.insert_for_test(&record, None, indexer.clone()).await;
        let adapter = pool.get_or_create(&record, None).await.unwrap();
        let request = SearchRequest::new("search");
        let result = SearchResult {
            site_id: 1,
            source_site: "Site 1".to_string(),
            torrent_id: "torrent".to_string(),
            title: "Example.Show.S01E01".to_string(),
            detail_url: None,
            download_locator: Some("torrent".to_string()),
            magnet: None,
            size: 1,
            seeders: 1,
            leechers: 0,
            publish_time: None,
        };

        let (search, fetch) =
            tokio::join!(adapter.search(&request), adapter.fetch_torrent(&result));

        assert!(search.is_ok());
        assert!(fetch.is_ok());
        assert_eq!(indexer.max_active.load(Ordering::SeqCst), 1);
        assert_eq!(indexer.starts.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn duplicate_origins_share_one_slot_while_another_origin_runs_in_parallel() {
        let pool = IndexerPool::new_with_policy(IndexerAccessPolicy {
            min_request_interval: Duration::ZERO,
            default_cooldown: Duration::ZERO,
        });
        let first_record = site_record(1, "https://tracker-a.example/one/");
        let duplicate_record = site_record(2, "https://tracker-a.example/two/");
        let other_record = site_record(3, "https://tracker-b.example/");
        let global_active = Arc::new(AtomicUsize::new(0));
        let max_global_active = Arc::new(AtomicUsize::new(0));
        let first = ProbeIndexer::new(
            1,
            Duration::from_millis(20),
            false,
            Arc::clone(&global_active),
            Arc::clone(&max_global_active),
        );
        let duplicate = ProbeIndexer::new(
            2,
            Duration::from_millis(20),
            false,
            Arc::clone(&global_active),
            Arc::clone(&max_global_active),
        );
        let other = ProbeIndexer::new(
            3,
            Duration::from_millis(20),
            false,
            Arc::clone(&global_active),
            Arc::clone(&max_global_active),
        );
        pool.insert_for_test(&first_record, None, first.clone())
            .await;
        pool.insert_for_test(&duplicate_record, None, duplicate.clone())
            .await;
        pool.insert_for_test(&other_record, None, other.clone())
            .await;
        let indexers = vec![
            pool.get_or_create(&first_record, None).await.unwrap(),
            pool.get_or_create(&duplicate_record, None).await.unwrap(),
            pool.get_or_create(&other_record, None).await.unwrap(),
        ];

        let aggregate = IndexerAggregator::new(2)
            .search(&indexers, &[SearchRequest::new("query")])
            .await;

        assert_eq!(aggregate.successful_requests, 3);
        assert_eq!(max_global_active.load(Ordering::SeqCst), 2);
        let first_start = first.starts.lock().unwrap()[0];
        let duplicate_start = duplicate.starts.lock().unwrap()[0];
        let other_start = other.starts.lock().unwrap()[0];
        assert!(first_start < duplicate_start);
        assert!(other_start < duplicate_start);
    }

    #[tokio::test]
    async fn pool_preserves_origin_cooldown_state_when_adapters_are_cleared() {
        let policy = IndexerAccessPolicy {
            min_request_interval: Duration::ZERO,
            default_cooldown: Duration::from_secs(30),
        };
        let pool = IndexerPool::new_with_policy(policy);
        let record = site_record(1, "https://tracker.example/");
        let key = site_rate_limit_key(&record).unwrap();
        let original = pool.get_or_create_access_gate(&key).await;

        original
            .observe_rate_limit(IndexerRateLimit::new(None))
            .await;
        pool.clear().await;
        let rebuilt = pool.get_or_create_access_gate(&key).await;

        assert!(Arc::ptr_eq(&original, &rebuilt));
        // Measure the cooldown gate, not the HTTP client's one-time TLS setup.
        let blocked_request = reqwest::Client::new().get("http://127.0.0.1:1/blocked");
        let fail_fast_started = Instant::now();
        assert!(matches!(
            rebuilt.send(blocked_request).await,
            Err(IndexerError::RateLimited(_))
        ));
        assert!(fail_fast_started.elapsed() < Duration::from_millis(20));
    }

    #[test]
    fn equivalent_site_origins_share_the_same_rate_limit_key() {
        let first = site_record(1, "https://Tracker.Example:443/one/");
        let second = site_record(2, "https://tracker.example/two/");

        assert_eq!(
            site_rate_limit_key(&first).unwrap(),
            site_rate_limit_key(&second).unwrap()
        );
    }

    #[test]
    fn configuration_digest_changes_with_credentials_and_proxy() {
        let mut record = SiteRecord {
            id: 1,
            name: "Site".to_string(),
            site_type: "nexusphp".to_string(),
            base_url: "https://tracker.example".to_string(),
            auth_config: r#"{"auth_type":"api_key","api_key":"one"}"#.to_string(),
            request_headers: "[]".to_string(),
            use_proxy: true,
            created_at: String::new(),
            updated_at: String::new(),
        };
        let initial = config_digest(&record, Some("http://proxy-one:8080"));
        record.auth_config = r#"{"auth_type":"api_key","api_key":"two"}"#.to_string();
        assert_ne!(
            initial,
            config_digest(&record, Some("http://proxy-one:8080"))
        );
        assert_ne!(
            initial,
            config_digest(&record, Some("http://proxy-two:8080"))
        );
        let before_headers = config_digest(&record, Some("http://proxy-one:8080"));
        record.request_headers = r#"[{"name":"X-Browser-Profile","value":"desktop"}]"#.to_string();
        assert_ne!(
            before_headers,
            config_digest(&record, Some("http://proxy-one:8080"))
        );
    }
}
