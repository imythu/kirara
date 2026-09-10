use std::sync::Arc;
use std::time::Duration;

use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use super::{models::RssResult, service::RssService};

pub struct RssScheduler {
    service: Arc<RssService>,
    stop: CancellationToken,
    owner: String,
}

impl RssScheduler {
    pub fn new(service: Arc<RssService>) -> Arc<Self> {
        Arc::new(Self {
            service,
            stop: CancellationToken::new(),
            owner: format!(
                "rss-{}-{}",
                std::process::id(),
                chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
            ),
        })
    }
    pub fn stop(&self) {
        self.stop.cancel();
        self.service.wake.notify_waiters();
    }
    pub async fn release_owned_leases(&self) -> RssResult<()> {
        self.service.db.rss_release_owner(&self.owner).await
    }

    pub async fn start(self: Arc<Self>) {
        info!("RSS scheduler started");
        tokio::join!(
            self.feeds(),
            self.evaluations(),
            self.downloads(),
            self.recovery(),
            self.service.track_reservations(&self.stop)
        );
        info!("RSS scheduler stopped");
    }

    async fn pause(&self, duration: Duration) {
        tokio::select! {
            _ = self.stop.cancelled() => {},
            _ = self.service.wake.notified() => {},
            _ = tokio::time::sleep(duration) => {},
        }
    }

    async fn feeds(&self) {
        let mut active = JoinSet::new();
        while !self.stop.is_cancelled() {
            while active.len() < self.service.config.fetch_concurrency && !self.stop.is_cancelled()
            {
                match self
                    .service
                    .db
                    .rss_claim_feed(&self.owner, self.service.config.feed_lease.as_secs())
                    .await
                {
                    Ok(Some(claim)) => {
                        let service = Arc::clone(&self.service);
                        active.spawn(async move { service.scan(claim).await });
                    }
                    Ok(None) => break,
                    Err(error) => {
                        error!(%error, "RSS feed claim failed");
                        break;
                    }
                }
            }
            if active.is_empty() {
                self.pause(self.service.config.feed_poll).await;
            } else {
                tokio::select! {
                    _ = self.stop.cancelled() => break,
                    result = active.join_next() => report(result, "feed"),
                    _ = self.service.wake.notified() => {},
                }
            }
        }
        while let Some(result) = active.join_next().await {
            report(Some(result), "feed");
        }
    }

    async fn evaluations(&self) {
        while !self.stop.is_cancelled() {
            match self
                .service
                .db
                .rss_claim_evaluation(&self.owner, self.service.config.evaluation_lease.as_secs())
                .await
            {
                Ok(Some(claim)) => {
                    if let Err(error) = self.service.evaluate(claim).await {
                        warn!(%error, "RSS evaluation interrupted");
                    }
                }
                Ok(None) => self.pause(self.service.config.work_poll).await,
                Err(error) => {
                    error!(%error, "RSS evaluation claim failed");
                    self.pause(self.service.config.work_poll).await;
                }
            }
        }
    }

    async fn downloads(&self) {
        let mut active = JoinSet::new();
        while !self.stop.is_cancelled() {
            while active.len() < self.service.config.download_concurrency
                && !self.stop.is_cancelled()
            {
                match self
                    .service
                    .db
                    .rss_claim_job(&self.owner, self.service.config.job_lease.as_secs())
                    .await
                {
                    Ok(Some(claim)) => {
                        let service = Arc::clone(&self.service);
                        active.spawn(async move { service.process_job(claim).await });
                    }
                    Ok(None) => break,
                    Err(error) => {
                        error!(%error, "RSS download claim failed");
                        break;
                    }
                }
            }
            if active.is_empty() {
                self.pause(self.service.config.work_poll).await;
            } else {
                tokio::select! {
                    _ = self.stop.cancelled() => break,
                    result = active.join_next() => report(result, "download"),
                    _ = self.service.wake.notified() => {},
                }
            }
        }
        while let Some(result) = active.join_next().await {
            report(Some(result), "download");
        }
    }

    async fn recovery(&self) {
        let mut last_cleanup = None;
        while !self.stop.is_cancelled() {
            if let Err(error) = self.service.db.rss_recover().await {
                error!(%error, "RSS lease recovery failed");
            }
            if last_cleanup.is_none_or(|time: tokio::time::Instant| {
                time.elapsed() >= self.service.config.cleanup_interval
            }) {
                if let Err(error) = self.service.db.rss_cleanup().await {
                    error!(%error, "RSS history cleanup failed");
                }
                last_cleanup = Some(tokio::time::Instant::now());
            }
            tokio::select! {
                _ = self.stop.cancelled() => break,
                _ = tokio::time::sleep(self.service.config.recovery_poll) => {},
            }
        }
    }
}

fn report(result: Option<Result<RssResult<()>, tokio::task::JoinError>>, kind: &str) {
    match result {
        Some(Ok(Err(error))) => warn!(%error, kind, "RSS operation did not finish"),
        Some(Err(error)) => error!(%error, kind, "RSS worker interrupted"),
        _ => {}
    }
}
