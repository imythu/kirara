mod brush;
pub mod cli;
mod collector;
mod config;
mod db;
mod downloader;
mod error;
mod indexer;
mod listener;
mod logging;
mod media;
mod monitor;
mod net;
mod openlist;
mod ptd_backup;
mod ptd_site_catalog;
mod ptd_sites;
mod relocation;
mod rss;
mod sign_in;
mod site;
mod site_stats;
mod stats;
mod tag_rule;
mod torrent_watcher;
mod web;

use std::sync::Arc;
use std::time::Duration;

use std::path::PathBuf;
use tokio_util::sync::CancellationToken;

pub use error::AppError;
pub use listener::{ListenEndpoint, Stream};
use net::http::AppHttpClient;
use net::rate_limiter::{RateLimitPolicy, SharedRateLimiter};
use tracing::info;

/// Configuration shared by the CLI and desktop application.
#[derive(Debug, Clone)]
pub struct ServerOptions {
    pub base_dir: PathBuf,
    pub db_dir: PathBuf,
    pub listen: ListenEndpoint,
}

/// An in-process server. Dropping the handle requests shutdown; `shutdown`
/// also waits for background tasks and database lease cleanup to finish.
pub struct ServerHandle {
    endpoint: ListenEndpoint,
    shutdown: CancellationToken,
    task: tokio::sync::oneshot::Receiver<Result<(), AppError>>,
}

impl ServerHandle {
    pub fn endpoint(&self) -> &ListenEndpoint {
        &self.endpoint
    }

    pub async fn wait(&mut self) -> Result<(), AppError> {
        (&mut self.task).await.map_err(|error| AppError::Server {
            message: format!("server task failed: {error}"),
        })?
    }

    pub async fn shutdown(mut self) -> Result<(), AppError> {
        self.shutdown.cancel();
        self.wait().await
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

/// Initialize the database and bind the listener before returning a running
/// server. No process-global signal handlers or child processes are installed.
pub async fn start(options: ServerOptions) -> Result<ServerHandle, AppError> {
    let shutdown = CancellationToken::new();
    let guard = shutdown.clone().drop_guard();
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let mut task = spawn_runtime(run(options, shutdown.clone(), ready_tx), shutdown.clone())?;
    let endpoint = match ready_rx.await {
        Ok(endpoint) => endpoint,
        Err(_) => {
            return match (&mut task).await {
                Ok(Err(error)) => Err(error),
                _ => Err(AppError::Server {
                    message: "server stopped during startup".into(),
                }),
            };
        }
    };
    guard.disarm();
    Ok(ServerHandle {
        endpoint,
        shutdown,
        task,
    })
}

thread_local! {
    static RUNTIME_SHUTDOWN: std::cell::RefCell<Option<CancellationToken>> = const { std::cell::RefCell::new(None) };
}

// Also available on this server's blocking workers, so synchronous browser
// sessions can close their sockets before the owned runtime joins its threads.
pub(crate) fn runtime_shutdown_token() -> CancellationToken {
    RUNTIME_SHUTDOWN.with(|slot| slot.borrow().clone().unwrap_or_default())
}

// Existing schedulers also spawn detached child tasks. Owning their runtime
// ensures every descendant is cancelled, and blocking DB operations finish,
// before shutdown reports completion to an embedding application.
fn spawn_runtime(
    future: impl std::future::Future<Output = Result<(), AppError>> + Send + 'static,
    shutdown: CancellationToken,
) -> Result<tokio::sync::oneshot::Receiver<Result<(), AppError>>, AppError> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    std::thread::Builder::new()
        .name("kirara-server".into())
        .spawn(move || {
            let result = match tokio::runtime::Builder::new_multi_thread()
                .on_thread_start(move || {
                    RUNTIME_SHUTDOWN.with(|slot| *slot.borrow_mut() = Some(shutdown.clone()));
                })
                .enable_all()
                .build()
            {
                Ok(runtime) => {
                    let result = runtime.block_on(future);
                    drop(runtime);
                    result
                }
                Err(error) => Err(AppError::Server {
                    message: format!("failed to create server runtime: {error}"),
                }),
            };
            let _ = sender.send(result);
        })
        .map_err(|error| AppError::Server {
            message: format!("failed to create server thread: {error}"),
        })?;
    Ok(receiver)
}

async fn run(
    options: ServerOptions,
    shutdown: CancellationToken,
    ready: tokio::sync::oneshot::Sender<ListenEndpoint>,
) -> Result<(), AppError> {
    let ServerOptions {
        base_dir,
        db_dir,
        listen,
    } = options;
    let listener = listen.bind().await.map_err(|error| AppError::Server {
        message: format!("failed to bind {listen}: {error}"),
    })?;
    let endpoint = listener.endpoint().map_err(|error| AppError::Server {
        message: format!("failed to read listener address: {error}"),
    })?;
    let db = db::Database::open(&db_dir).await?;
    let self_use = self_use_enabled(std::env::var("SELF_USE").ok().as_deref());
    let settings = db.get_settings().await?;
    let log_filter = logging::build_log_filter(settings.log_level.as_deref())?;
    logging::init_logging(log_filter)?;
    info!(
        "startup configuration: listen_addr={} data_dir={} database_dir={}",
        endpoint,
        base_dir.display(),
        db_dir.display()
    );

    let pool = downloader::DownloaderClientPool::new(db.clone());
    let media_service = media::service::MediaService::new(db.clone(), pool.clone());
    let media_scheduler = media::scheduler::MediaScheduler::new(media_service.clone());

    let collector = std::sync::Arc::new(collector::DownloaderSnapshotCollector::new(
        db.clone(),
        pool.clone(),
    ));
    let stats_db = db.clone();
    let stats_rx = collector.subscribe();

    // 构建共享 HTTP 客户端（代理 + 限流），供刷流调度器使用
    let proxy = settings.proxy.as_deref();
    let limiter = Arc::new(SharedRateLimiter::new());
    let policy = RateLimitPolicy::new(5, Duration::from_secs(1), Duration::from_secs(60));
    let http = Arc::new(
        AppHttpClient::new(limiter.clone(), policy, proxy).map_err(|e| {
            AppError::InvalidConfig {
                message: format!("failed to build HTTP client: {}", e),
            }
        })?,
    );

    let scheduler = std::sync::Arc::new(brush::scheduler::BrushScheduler::new(
        db.clone(),
        collector.clone(),
        pool.clone(),
        http,
    ));

    let sign_in_scheduler = std::sync::Arc::new(sign_in::scheduler::SignInScheduler::new(
        db.clone(),
        base_dir.clone(),
    ));

    let site_stats_refresher = std::sync::Arc::new(site_stats::SiteStatsRefresher::new(db.clone()));

    let monitor = std::sync::Arc::new(monitor::SystemMonitor::new(db.clone()));

    let tag_rule_scheduler = tag_rule::scheduler::TagRuleScheduler::new(db.clone(), pool.clone());
    let new_torrent_publisher = torrent_watcher::NewTorrentPublisher::new(db.clone(), pool.clone());
    let new_torrent_notifications = new_torrent_publisher.subscribe();
    let relocation_scheduler =
        relocation::RelocationScheduler::new(db.clone(), pool.clone(), self_use);

    let _ = ready.send(endpoint);
    let media_scheduler_ref = media_scheduler.clone();
    let mut media_scheduler_handle = tokio::spawn(async move {
        media_scheduler_ref.start().await;
    });
    let collector_ref = collector.clone();
    let collector_handle = tokio::spawn(async move {
        collector_ref.start().await;
    });
    let stats_handle = tokio::spawn(async move {
        stats::start_stats_consumer(stats_db, stats_rx).await;
    });
    let scheduler_ref = scheduler.clone();
    let scheduler_handle = tokio::spawn(async move {
        scheduler_ref.start().await;
    });
    let sign_in_scheduler_ref = sign_in_scheduler.clone();
    let sign_in_scheduler_handle = tokio::spawn(async move {
        sign_in_scheduler_ref.start().await;
    });
    let site_stats_refresher_ref = site_stats_refresher.clone();
    let site_stats_handle = tokio::spawn(async move {
        site_stats_refresher_ref.start().await;
    });
    let monitor_ref = monitor.clone();
    let monitor_handle = tokio::spawn(async move {
        monitor_ref.start().await;
    });
    let tag_rule_scheduler_ref = tag_rule_scheduler.clone();
    let tag_rule_scheduler_handle = tokio::spawn(async move {
        tag_rule_scheduler_ref.start().await;
    });
    let tag_rule_subscriber_ref = tag_rule_scheduler.clone();
    let tag_rule_subscriber_handle = tokio::spawn(async move {
        tag_rule_subscriber_ref
            .start_new_torrent_subscriber(new_torrent_notifications)
            .await;
    });
    let new_torrent_publisher_ref = new_torrent_publisher.clone();
    let new_torrent_publisher_handle = tokio::spawn(async move {
        new_torrent_publisher_ref.start().await;
    });
    let relocation_scheduler_ref = relocation_scheduler.clone();
    let relocation_scheduler_handle = tokio::spawn(async move {
        relocation_scheduler_ref.start().await;
    });

    let web_result = web::serve(
        listener,
        db,
        scheduler,
        sign_in_scheduler,
        site_stats_refresher,
        collector,
        pool,
        media_service,
        media_scheduler.clone(),
        monitor,
        tag_rule_scheduler,
        relocation_scheduler.clone(),
        self_use,
        shutdown,
    )
    .await;

    media_scheduler.stop();
    relocation_scheduler.stop();
    collector_handle.abort();
    stats_handle.abort();
    scheduler_handle.abort();
    sign_in_scheduler_handle.abort();
    site_stats_handle.abort();
    monitor_handle.abort();
    tag_rule_scheduler_handle.abort();
    tag_rule_subscriber_handle.abort();
    new_torrent_publisher_handle.abort();
    relocation_scheduler_handle.abort();

    if tokio::time::timeout(Duration::from_secs(10), &mut media_scheduler_handle)
        .await
        .is_err()
    {
        media_scheduler_handle.abort();
        let _ = media_scheduler_handle.await;
    }
    match media_scheduler.release_owned_leases().await {
        Ok((0, 0)) => {}
        Ok((subscriptions, downloads)) => info!(
            recovered_subscriptions = subscriptions,
            recovered_downloads = downloads,
            "released interrupted media leases during shutdown"
        ),
        Err(error) => {
            tracing::error!(%error, "failed to release media leases during shutdown")
        }
    }

    let _ = collector_handle.await;
    let _ = stats_handle.await;
    let _ = scheduler_handle.await;
    let _ = sign_in_scheduler_handle.await;
    let _ = site_stats_handle.await;
    let _ = monitor_handle.await;
    let _ = tag_rule_scheduler_handle.await;
    let _ = tag_rule_subscriber_handle.await;
    let _ = new_torrent_publisher_handle.await;
    let _ = relocation_scheduler_handle.await;

    web_result
}

fn self_use_enabled(value: Option<&str>) -> bool {
    value == Some("true")
}

#[cfg(test)]
mod feature_gate_tests {
    use super::self_use_enabled;

    #[test]
    fn self_use_requires_exact_true_literal() {
        assert!(self_use_enabled(Some("true")));
        for value in [None, Some("TRUE"), Some("1"), Some(" true"), Some("")] {
            assert!(!self_use_enabled(value));
        }
    }
}

#[cfg(test)]
mod runtime_tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[tokio::test]
    async fn completion_waits_for_detached_descendants_to_be_dropped() {
        struct Guard(Arc<AtomicBool>);
        impl Drop for Guard {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let dropped = Arc::new(AtomicBool::new(false));
        let guard = Guard(dropped.clone());
        let finished = spawn_runtime(
            async move {
                let (ready, started) = tokio::sync::oneshot::channel();
                tokio::spawn(async move {
                    tokio::spawn(async move {
                        let _guard = guard;
                        let _ = ready.send(());
                        std::future::pending::<()>().await;
                    });
                });
                started.await.unwrap();
                Ok(())
            },
            CancellationToken::new(),
        )
        .unwrap();
        finished.await.unwrap().unwrap();
        assert!(dropped.load(Ordering::SeqCst));
    }
}
