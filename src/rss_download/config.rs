use std::time::Duration;

/// All worker timing is shared by the scheduler and its lease heartbeats.
/// The database owns the fixed per-feed cycle quota (20) as a transactional invariant.
#[derive(Clone, Debug)]
pub struct RssSchedulerConfig {
    pub feed_poll: Duration,
    pub work_poll: Duration,
    pub recovery_poll: Duration,
    pub cleanup_interval: Duration,
    pub feed_lease: Duration,
    pub evaluation_lease: Duration,
    pub job_lease: Duration,
    pub fetch_concurrency: usize,
    pub download_concurrency: usize,
    pub lookup_timeout: Duration,
    pub submission_timeout: Duration,
    pub reconciliation_window: Duration,
    pub max_attempts: u32,
}

impl Default for RssSchedulerConfig {
    fn default() -> Self {
        Self {
            feed_poll: Duration::from_secs(15),
            work_poll: Duration::from_secs(2),
            recovery_poll: Duration::from_secs(30),
            cleanup_interval: Duration::from_secs(3600),
            feed_lease: Duration::from_secs(120),
            evaluation_lease: Duration::from_secs(120),
            job_lease: Duration::from_secs(300),
            fetch_concurrency: 4,
            download_concurrency: 4,
            lookup_timeout: Duration::from_secs(30),
            submission_timeout: Duration::from_secs(45),
            reconciliation_window: Duration::from_secs(30),
            max_attempts: 5,
        }
    }
}
