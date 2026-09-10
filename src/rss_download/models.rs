use serde::{Deserialize, Serialize};

pub type RssResult<T> = Result<T, RssError>;

#[derive(Debug, thiserror::Error)]
pub enum RssError {
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    Unavailable(String),
    #[error("{0}")]
    Authentication(String),
    #[error("{message}")]
    RateLimited { message: String, retry_at: String },
    #[error("RSS 数据库操作失败")]
    Database(#[from] crate::error::AppError),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ItemAttributes {
    pub size_bytes: Option<u64>,
    pub seeders: Option<u32>,
    pub leechers: Option<u32>,
    pub download_volume_factor: Option<f64>,
    pub upload_volume_factor: Option<f64>,
    pub hr: Option<bool>,
    pub minimum_ratio: Option<f64>,
    pub minimum_seed_time: Option<u64>,
    pub free_until: Option<i64>,
    pub observed_at: String,
    pub source: String,
    pub hints: Vec<String>,
}

// Raw locations are intentionally not serializable or Debug: they may contain credentials.
#[derive(Clone)]
pub struct NormalizedItem {
    pub item_key: String,
    pub guid: Option<String>,
    pub title: String,
    pub detail_url: Option<String>,
    pub download_url: Option<String>,
    pub site_torrent_id: Option<String>,
    pub published_at: Option<String>,
    pub categories: Vec<String>,
    pub description: Option<String>,
    pub attributes: ItemAttributes,
}

#[derive(Clone, Default)]
pub struct FetchedFeed {
    pub not_modified: bool,
    pub title: Option<String>,
    pub items: Vec<NormalizedItem>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedRecord {
    pub id: i64,
    pub name: String,
    pub url_display: String,
    pub site_id: Option<i64>,
    pub site_name: Option<String>,
    pub use_proxy: Option<bool>,
    pub enabled: bool,
    pub interval_minutes: u32,
    pub generation: i64,
    pub version: i64,
    pub initialized_at: Option<String>,
    pub last_sequence: i64,
    pub last_checked_at: Option<String>,
    pub next_run_at: Option<String>,
    pub last_status: String,
    pub last_error: Option<String>,
    pub item_count: u64,
    pub pending_count: u64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone)]
pub struct StoredFeed {
    pub record: FeedRecord,
    pub url: String,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct FeedInput {
    pub name: String,
    pub url: Option<String>,
    pub site_id: Option<i64>,
    pub use_proxy: Option<bool>,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default = "default_interval")]
    pub interval_minutes: u32,
    pub expected_version: Option<i64>,
    pub request_id: Option<String>,
}

#[derive(Clone, Deserialize)]
pub struct FeedTestRequest {
    pub feed_id: Option<i64>,
    pub url: Option<String>,
    // Omitted keeps the saved setting; explicit null clears it in an edit draft.
    #[serde(default, deserialize_with = "present_option")]
    pub site_id: Option<Option<i64>>,
    #[serde(default, deserialize_with = "present_option")]
    pub use_proxy: Option<Option<bool>>,
}

fn present_option<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct RuleFilters {
    pub include: Vec<String>,
    pub include_mode: String,
    pub exclude: Vec<String>,
    pub include_regex: Option<String>,
    pub exclude_regex: Option<String>,
    pub match_all: bool,
    pub min_size_bytes: Option<u64>,
    pub max_size_bytes: Option<u64>,
    pub min_seeders: Option<u32>,
    pub free_only: bool,
    pub hr_policy: String,
}

impl Default for RuleFilters {
    fn default() -> Self {
        Self {
            include: Vec::new(),
            include_mode: "all".into(),
            exclude: Vec::new(),
            include_regex: None,
            exclude_regex: None,
            match_all: false,
            min_size_bytes: None,
            max_size_bytes: None,
            min_seeders: None,
            free_only: false,
            hr_policy: "require_clear".into(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct DownloadOptions {
    pub save_path: Option<String>,
    pub category: Option<String>,
    pub tags: Vec<String>,
    pub paused: bool,
    pub reserve_space_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleRecord {
    pub id: i64,
    pub name: String,
    pub enabled: bool,
    pub priority: i32,
    pub feed_ids: Vec<i64>,
    pub filters: RuleFilters,
    pub downloader_id: Option<i64>,
    pub downloader_name: Option<String>,
    pub options: DownloadOptions,
    pub match_revision: i64,
    pub version: i64,
    pub last_error: Option<String>,
    pub matched_count: u64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleInput {
    pub name: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default = "default_priority")]
    pub priority: i32,
    pub feed_ids: Vec<i64>,
    #[serde(default)]
    pub filters: RuleFilters,
    pub downloader_id: Option<i64>,
    #[serde(default)]
    pub options: DownloadOptions,
    pub expected_version: Option<i64>,
    pub request_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchReason {
    pub code: String,
    pub message: String,
    pub field: Option<String>,
    pub actual: Option<String>,
    pub expected: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MatchEvaluation {
    pub matched: bool,
    pub needs_attributes: bool,
    pub reasons: Vec<MatchReason>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionRecord {
    pub id: i64,
    pub item_id: i64,
    pub rule_id: i64,
    pub rule_name: String,
    pub match_revision: i64,
    pub status: String,
    pub evaluation: MatchEvaluation,
    pub checked_at: Option<String>,
    pub next_evaluate_at: Option<String>,
    pub job_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemRecord {
    pub id: i64,
    pub feed_id: i64,
    pub feed_name: String,
    pub generation: i64,
    pub item_key: String,
    pub sequence: i64,
    pub title: String,
    pub detail_url: Option<String>,
    pub site_torrent_id: Option<String>,
    pub published_at: Option<String>,
    pub categories: Vec<String>,
    pub attributes: ItemAttributes,
    pub downloadable: bool,
    pub content_revision: i64,
    pub first_seen_at: String,
    pub last_seen_at: String,
    pub status: String,
    pub decisions: Vec<DecisionRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobRecord {
    pub id: i64,
    pub item_id: i64,
    pub feed_id: i64,
    pub feed_generation: i64,
    pub feed_name: String,
    pub rule_id: i64,
    pub rule_name: String,
    pub downloader_id: i64,
    pub downloader_name: String,
    pub title: String,
    pub size_bytes: Option<u64>,
    pub filters_snapshot: RuleFilters,
    pub options_snapshot: DownloadOptions,
    pub decision_snapshot: MatchEvaluation,
    pub status: String,
    pub infohash: Option<String>,
    pub reserved_bytes: u64,
    pub attempts: u32,
    pub next_attempt_at: Option<String>,
    pub version: i64,
    pub last_error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub submitted_at: Option<String>,
    pub download_state: Option<String>,
    pub progress: Option<f64>,
    pub sampled_at: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RunRecord {
    pub id: i64,
    pub feed_id: Option<i64>,
    pub kind: String,
    pub status: String,
    pub item_count: u64,
    pub new_count: u64,
    pub queued_count: u64,
    pub pending_count: u64,
    pub message: Option<String>,
    pub started_at: String,
    pub finished_at: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RssSummary {
    pub feeds_total: u64,
    pub running: u64,
    pub paused: u64,
    pub needs_attention: u64,
    pub rules_enabled: u64,
    pub queued: u64,
    pub submitted: u64,
    pub failed: u64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ListQuery {
    pub page: usize,
    pub page_size: usize,
    pub keyword: Option<String>,
    pub status: Option<String>,
    pub feed_id: Option<i64>,
    pub rule_id: Option<i64>,
    pub site_id: Option<i64>,
    pub downloader_id: Option<i64>,
}

impl ListQuery {
    pub fn normalized(mut self) -> Self {
        self.page = self.page.clamp(1, 1_000_000);
        self.page_size = if self.page_size == 0 {
            50
        } else {
            self.page_size.clamp(1, 200)
        };
        self.keyword = self
            .keyword
            .map(|s| s.trim().chars().take(200).collect())
            .filter(|s: &String| !s.is_empty());
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub total: u64,
    pub page: usize,
    pub page_size: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ActionRequest {
    pub expected_version: Option<i64>,
    pub request_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PreviewRequest {
    pub rule: RuleInput,
    #[serde(default)]
    pub item_ids: Vec<i64>,
    #[serde(default)]
    pub refresh_samples: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PreviewItem {
    pub item: ItemRecord,
    pub evaluation: MatchEvaluation,
}

#[derive(Debug, Clone, Serialize)]
pub struct PreviewResponse {
    pub rule_version: Option<i64>,
    pub total: u64,
    pub matched: u64,
    pub rejected: u64,
    pub unknown: u64,
    pub sample_limited: bool,
    pub sample_time: String,
    pub items: Vec<PreviewItem>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FeedTestResponse {
    pub title: Option<String>,
    pub item_count: usize,
    pub items: Vec<ItemRecord>,
    pub warnings: Vec<String>,
    pub sample_time: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BackfillRequest {
    pub rule_id: i64,
    pub expected_version: i64,
    pub item_ids: Vec<i64>,
    pub request_id: String,
}

fn yes() -> bool {
    true
}
fn default_interval() -> u32 {
    15
}
fn default_priority() -> i32 {
    100
}
