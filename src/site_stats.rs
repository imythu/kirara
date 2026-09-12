use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use futures::stream::{self, StreamExt};
use tokio::sync::Mutex;
use tracing::{error, info};

use crate::db::Database;
use crate::error::AppError;
use crate::net::client_factory;
use crate::site::{SiteAdapter, SiteWithStats, UserStats};
use crate::site::factory as site_factory;

const REFRESH_INTERVAL: Duration = Duration::from_secs(60 * 60);
// PT-Depiler 使用队列限制用户信息刷新并发。这里也限制同时访问的站点数，
// 避免大量站点经同一代理请求时触发限流或连接资源耗尽。
const REFRESH_CONCURRENCY: usize = 4;

#[derive(Clone)]
pub struct SiteStatsRefresher {
    db: Database,
    refresh_lock: Arc<Mutex<()>>,
}

impl SiteStatsRefresher {
    pub fn new(db: Database) -> Self {
        Self {
            db,
            refresh_lock: Arc::new(Mutex::new(())),
        }
    }

    pub async fn start(&self) {
        info!("site stats refresher started (interval: 1h)");
        if let Err(error) = self.refresh_all().await {
            error!("initial site stats refresh failed: {}", error);
        }

        let mut interval = tokio::time::interval(REFRESH_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            if let Err(error) = self.refresh_all().await {
                error!("site stats refresh failed: {}", error);
            }
        }
    }

    pub async fn refresh_all(&self) -> Result<Vec<SiteWithStats>, AppError> {
        let _refresh_guard = self.refresh_lock.lock().await;
        self.refresh_all_unlocked().await
    }

    pub fn refresh_all_in_background(self: &Arc<Self>) -> bool {
        let Ok(refresh_guard) = Arc::clone(&self.refresh_lock).try_lock_owned() else {
            return false;
        };
        let refresher = Arc::clone(self);
        tokio::spawn(async move {
            let _refresh_guard = refresh_guard;
            match refresher.refresh_all_unlocked().await {
                Ok(sites) => info!(
                    site_count = sites.len(),
                    "manual site stats refresh completed"
                ),
                Err(error) => error!("manual site stats refresh failed: {}", error),
            }
        });
        true
    }

    pub fn is_refreshing(&self) -> bool {
        self.refresh_lock.try_lock().is_err()
    }

    async fn refresh_all_unlocked(&self) -> Result<Vec<SiteWithStats>, AppError> {
        // Timestamp the configuration snapshot, not a later queued request. Otherwise a Cookie
        // edit while this batch is running could make an old uid appear newer than the edit.
        let checked_at = Utc::now().to_rfc3339();
        let sites = self.db.list_sites_with_stats().await?;
        let settings = self.db.get_settings().await?;
        let proxy = settings.proxy.as_deref();
        let db = self.db.clone();
        stream::iter(sites.into_iter().map(|site| {
            let db = db.clone();
            let checked_at = checked_at.clone();
            async move {
                let cached_user_id = site.reusable_user_id();
                let site_record = site.site_record();
                let prepared = match client_factory::resolve_site_client(proxy, site.use_proxy) {
                    Ok(client) => {
                        site_factory::create_adapter_with_cached_user_id(
                            &site_record,
                            client,
                            cached_user_id,
                        )
                    }
                    Err(error) => Err(format!("创建 HTTP 客户端失败: {}", error)),
                };

                match prepared {
                    Ok(adapter) => match adapter.get_user_stats().await {
                        Ok(mut stats) => {
                            apply_own_email(adapter.as_ref(), &site, &mut stats).await;
                            db.upsert_site_stats_success(site.id, &stats, &checked_at)
                                .await
                        }
                        Err(error) => {
                            db.upsert_site_stats_error(site.id, &error, &checked_at)
                                .await
                        }
                    },
                    Err(error) => {
                        db.upsert_site_stats_error(site.id, &error, &checked_at)
                            .await
                    }
                }
            }
        }))
        .buffer_unordered(REFRESH_CONCURRENCY)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;

        let sites = self.db.list_sites_with_stats().await?;
        crate::ptd_backup::backup_if_due(&self.db).await;
        Ok(sites)
    }
}

/// 统计刷新成功后补全本人邮箱。
///
/// 已保存非空邮箱则跳过（站点侧邮箱不可改，避免重复请求）；
/// 需要抓取时失败或未公开则不写入，保留历史结果。
pub(crate) async fn apply_own_email(
    adapter: &dyn SiteAdapter,
    previous: &SiteWithStats,
    stats: &mut UserStats,
) {
    if let Some(existing) = previous
        .stats
        .as_ref()
        .and_then(|record| record.details.email.as_deref())
        .map(str::trim)
        .filter(|email| !email.is_empty())
    {
        stats.details.email = Some(existing.to_string());
        return;
    }
    if stats
        .details
        .email
        .as_deref()
        .map(str::trim)
        .is_some_and(|email| !email.is_empty())
    {
        return;
    }
    let Some(uid) = stats
        .uid
        .as_deref()
        .map(str::trim)
        .filter(|uid| !uid.is_empty())
    else {
        return;
    };
    let lookup = adapter.fetch_user_profile(uid).await;
    let email = lookup.email().trim();
    if !email.is_empty() {
        stats.details.email = Some(email.to_string());
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::Router;
    use axum::http::StatusCode;
    use axum::response::Html;
    use axum::routing::get;

    use super::*;
    use crate::site::{
        SiteAdapter, TorrentAttributes, UserStats, UserStatsDetails,
        user_email::{UserProfileInfo, UserProfileLookup},
    };
    use std::future::Future;
    use std::pin::Pin;

    struct StubAdapter {
        profile: UserProfileLookup,
        fetch_calls: Arc<AtomicUsize>,
    }

    impl SiteAdapter for StubAdapter {
        fn get_user_stats(
            &self,
        ) -> Pin<Box<dyn Future<Output = Result<UserStats, String>> + Send + '_>> {
            Box::pin(async { unreachable!("stats not used in this unit test") })
        }

        fn get_torrent_attributes(
            &self,
            _detail_url: &str,
        ) -> Pin<Box<dyn Future<Output = Result<TorrentAttributes, String>> + Send + '_>> {
            Box::pin(async { unreachable!("torrents not used in this unit test") })
        }

        fn fetch_user_profile(
            &self,
            _user_id: &str,
        ) -> Pin<Box<dyn Future<Output = UserProfileLookup> + Send + '_>> {
            self.fetch_calls.fetch_add(1, Ordering::SeqCst);
            let profile = self.profile.clone();
            Box::pin(async move { profile })
        }
    }

    fn lookup_with_email(email: &str) -> UserProfileLookup {
        UserProfileLookup::ok(UserProfileInfo {
            email: Some(email.to_string()),
            ..Default::default()
        })
    }

    fn site_with_email(email: Option<&str>) -> SiteWithStats {
        SiteWithStats {
            id: 1,
            name: "tracker".into(),
            site_type: "nexusphp".into(),
            base_url: "https://pt.example".into(),
            auth_config: "{}".into(),
            request_headers: "[]".into(),
            use_proxy: false,
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
            stats: Some(crate::site::SiteStatsRecord {
                site_id: 1,
                details: UserStatsDetails {
                    email: email.map(str::to_string),
                    ..Default::default()
                },
                ..Default::default()
            }),
        }
    }

    fn stats_with_uid() -> UserStats {
        UserStats {
            uid: Some("42".into()),
            username: "Alice".into(),
            uploaded: 1,
            downloaded: 1,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn apply_own_email_skips_fetch_when_already_present() {
        let calls = Arc::new(AtomicUsize::new(0));
        let adapter = StubAdapter {
            profile: lookup_with_email("new@example.com"),
            fetch_calls: calls.clone(),
        };
        let previous = site_with_email(Some("kept@example.com"));
        let mut stats = stats_with_uid();
        apply_own_email(&adapter, &previous, &mut stats).await;
        assert_eq!(stats.details.email.as_deref(), Some("kept@example.com"));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn apply_own_email_saves_successful_fetch() {
        let calls = Arc::new(AtomicUsize::new(0));
        let adapter = StubAdapter {
            profile: lookup_with_email("fetched@example.com"),
            fetch_calls: calls.clone(),
        };
        let previous = site_with_email(None);
        let mut stats = stats_with_uid();
        apply_own_email(&adapter, &previous, &mut stats).await;
        assert_eq!(stats.details.email.as_deref(), Some("fetched@example.com"));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn apply_own_email_ignores_failed_or_empty_lookup() {
        let calls = Arc::new(AtomicUsize::new(0));
        let adapter = StubAdapter {
            profile: UserProfileLookup::failed(
                crate::site::user_email::UserProfileFailureKind::Intercepted,
                "blocked",
            ),
            fetch_calls: calls.clone(),
        };
        let previous = site_with_email(None);
        let mut stats = stats_with_uid();
        apply_own_email(&adapter, &previous, &mut stats).await;
        assert_eq!(stats.details.email, None);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn background_refresh_is_deduplicated_while_another_refresh_is_running() {
        let temp = tempfile::tempdir().unwrap();
        let db = Database::open(temp.path()).await.unwrap();
        let refresher = Arc::new(SiteStatsRefresher::new(db));
        let refresh_guard = Arc::clone(&refresher.refresh_lock).lock_owned().await;

        assert!(refresher.is_refreshing());
        assert!(!refresher.refresh_all_in_background());

        drop(refresh_guard);
        assert!(!refresher.is_refreshing());
        assert!(refresher.refresh_all_in_background());

        tokio::time::timeout(Duration::from_secs(1), async {
            while refresher.is_refreshing() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn scheduled_refresh_reuses_persisted_nexusphp_user_id() {
        let homepage_hits = Arc::new(AtomicUsize::new(0));
        let homepage_hits_for_route = Arc::clone(&homepage_hits);
        let api_hits = Arc::new(AtomicUsize::new(0));
        let api_hits_for_route = Arc::clone(&api_hits);
        let app = Router::new()
            .route(
                "/index.php",
                get(move || async move {
                    homepage_hits_for_route.fetch_add(1, Ordering::SeqCst);
                    StatusCode::SERVICE_UNAVAILABLE
                }),
            )
            .route(
                "/userdetails.php",
                get(|| async {
                    Html(
                        r#"<div id="info_block">
                              <a class="User_Name" href="/userdetails.php?id=42">Alice</a>
                           </div>
                           <table><tr><td class="rowhead">传输</td>
                             <td>上传量 4 GiB 下载量 2 GiB 分享率 2.0</td>
                           </tr></table>"#,
                    )
                }),
            )
            .route(
                "/api/user",
                get(move || async move {
                    api_hits_for_route.fetch_add(1, Ordering::SeqCst);
                    StatusCode::NOT_FOUND
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let temp = tempfile::tempdir().unwrap();
        let db = Database::open(temp.path()).await.unwrap();
        let site_id = db
            .create_site(
                "tracker",
                "nexusphp",
                &format!("http://{address}"),
                r#"{"auth_type":"cookie","cookie":"session=valid"}"#,
                "[]",
                false,
            )
            .await
            .unwrap();
        let checked_at = Utc::now().to_rfc3339();
        assert!(
            db.get_site_with_stats(site_id)
                .await
                .unwrap()
                .unwrap()
                .stats
                .is_none()
        );
        assert!(db.get_site_with_stats(site_id + 1).await.unwrap().is_none());
        db.upsert_site_stats_success(
            site_id,
            &UserStats {
                uid: Some("42".to_string()),
                username: "Alice".to_string(),
                uploaded: 1,
                downloaded: 1,
                ratio: Some(1.0),
                bonus: None,
                seeding_count: None,
                leeching_count: None,
                details: UserStatsDetails::default(),
            },
            &checked_at,
        )
        .await
        .unwrap();

        let refreshed = SiteStatsRefresher::new(db.clone())
            .refresh_all()
            .await
            .unwrap();
        let stats = refreshed[0].stats.as_ref().unwrap();
        assert_eq!(stats.uid.as_deref(), Some("42"));
        assert_eq!(stats.uploaded, Some(4_294_967_296));
        assert_eq!(stats.downloaded, Some(2_147_483_648));
        let stored = db.get_site_with_stats(site_id).await.unwrap().unwrap();
        assert_eq!(stored.reusable_user_id(), Some("42"));
        assert_eq!(stored.stats.as_ref().unwrap().uploaded, stats.uploaded);
        assert_eq!(homepage_hits.load(Ordering::SeqCst), 0);
        assert_eq!(api_hits.load(Ordering::SeqCst), 0);
        server.abort();
    }
}
