use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use futures::stream::{FuturesUnordered, StreamExt};
use tokio::sync::{Mutex, broadcast};
use tokio::time::MissedTickBehavior;
use tracing::{debug, error, info, warn};

use crate::db::Database;
use crate::downloader::{DownloaderClientPool, DownloaderRecord, TorrentInfo};

const NEW_TORRENT_POLL_INTERVAL: Duration = Duration::from_secs(10);
const NEW_TORRENT_CHANNEL_CAPACITY: usize = 1024;

/// A single newly observed torrent. The downloader identity fields deliberately
/// exclude credentials so notifications are safe to log or inspect.
#[derive(Clone, Debug)]
pub struct NewTorrentNotification {
    pub downloader_id: i64,
    pub downloader_name: String,
    pub torrent: TorrentInfo,
    downloader_identity: DownloaderIdentity,
}

impl NewTorrentNotification {
    pub(crate) fn new(downloader: &DownloaderRecord, torrent: TorrentInfo) -> Self {
        Self {
            downloader_id: downloader.id,
            downloader_name: downloader.name.clone(),
            torrent,
            downloader_identity: DownloaderIdentity::from(downloader),
        }
    }

    pub fn matches_downloader(&self, downloader: &DownloaderRecord) -> bool {
        self.downloader_id == downloader.id
            && self.downloader_identity == DownloaderIdentity::from(downloader)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DownloaderIdentity {
    downloader_type: String,
    url: String,
}

impl From<&DownloaderRecord> for DownloaderIdentity {
    fn from(downloader: &DownloaderRecord) -> Self {
        let downloader_type = match downloader
            .downloader_type
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "qb" | "qbittorrent" => "qbittorrent".to_string(),
            other => other.to_string(),
        };
        Self {
            downloader_type,
            url: downloader.url.trim().trim_end_matches('/').to_string(),
        }
    }
}

struct DownloaderBaseline {
    identity: DownloaderIdentity,
    hashes: HashSet<String>,
}

#[derive(Default)]
struct DetectionState {
    downloaders: HashMap<i64, DownloaderBaseline>,
}

impl DetectionState {
    fn retain_downloaders(&mut self, active_ids: &HashSet<i64>) {
        self.downloaders
            .retain(|downloader_id, _| active_ids.contains(downloader_id));
    }

    fn observe(
        &mut self,
        downloader: &DownloaderRecord,
        torrents: Vec<TorrentInfo>,
    ) -> Vec<NewTorrentNotification> {
        let identity = DownloaderIdentity::from(downloader);
        let mut current_hashes = HashSet::with_capacity(torrents.len());
        let mut unique_torrents = Vec::with_capacity(torrents.len());
        for torrent in torrents {
            let normalized_hash = torrent.hash.trim().to_ascii_lowercase();
            if !normalized_hash.is_empty() && current_hashes.insert(normalized_hash.clone()) {
                unique_torrents.push((normalized_hash, torrent));
            }
        }

        let Some(baseline) = self.downloaders.get_mut(&downloader.id) else {
            self.downloaders.insert(
                downloader.id,
                DownloaderBaseline {
                    identity,
                    hashes: current_hashes,
                },
            );
            return Vec::new();
        };

        if baseline.identity != identity {
            *baseline = DownloaderBaseline {
                identity,
                hashes: current_hashes,
            };
            return Vec::new();
        }

        let mut notifications = unique_torrents
            .into_iter()
            .filter(|(hash, _)| !baseline.hashes.contains(hash))
            .map(|(_, torrent)| NewTorrentNotification::new(downloader, torrent))
            .collect::<Vec<_>>();
        baseline.hashes = current_hashes;

        notifications.sort_by(|left, right| {
            left.torrent
                .added_on
                .cmp(&right.torrent.added_on)
                .then_with(|| left.torrent.hash.cmp(&right.torrent.hash))
        });
        notifications
    }
}

/// Polls configured downloaders and broadcasts one notification per newly
/// observed torrent. Every receiver created by `subscribe` sees each event.
pub struct NewTorrentPublisher {
    db: Database,
    pool: Arc<DownloaderClientPool>,
    state: Mutex<DetectionState>,
    tx: broadcast::Sender<Arc<NewTorrentNotification>>,
    poll_interval: Duration,
}

impl NewTorrentPublisher {
    pub fn new(db: Database, pool: Arc<DownloaderClientPool>) -> Arc<Self> {
        let (tx, _) = broadcast::channel(NEW_TORRENT_CHANNEL_CAPACITY);
        Arc::new(Self {
            db,
            pool,
            state: Mutex::new(DetectionState::default()),
            tx,
            poll_interval: NEW_TORRENT_POLL_INTERVAL,
        })
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Arc<NewTorrentNotification>> {
        self.tx.subscribe()
    }

    pub async fn start(&self) {
        info!(
            interval_secs = self.poll_interval.as_secs(),
            "new torrent publisher started"
        );
        let mut interval = tokio::time::interval(self.poll_interval);
        interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            if let Err(error) = self.poll_once().await {
                error!(%error, "new torrent publisher poll failed");
            }
        }
    }

    async fn poll_once(&self) -> Result<(), String> {
        let downloaders = self
            .db
            .list_downloaders()
            .await
            .map_err(|error| format!("加载下载器列表失败: {error}"))?;
        let active_ids = downloaders
            .iter()
            .map(|downloader| downloader.id)
            .collect::<HashSet<_>>();
        self.state.lock().await.retain_downloaders(&active_ids);

        let mut pending = FuturesUnordered::new();
        for downloader in downloaders {
            let pool = Arc::clone(&self.pool);
            pending.push(async move {
                let torrents = match pool.get(&downloader).await {
                    Ok(client) => client.list_torrents(None).await,
                    Err(error) => Err(error),
                };
                (downloader, torrents)
            });
        }

        while let Some((downloader, result)) = pending.next().await {
            let torrents = match result {
                Ok(torrents) => torrents,
                Err(error) => {
                    warn!(
                        downloader_id = downloader.id,
                        downloader_name = %downloader.name,
                        %error,
                        "new torrent publisher could not read downloader"
                    );
                    continue;
                }
            };

            let notifications = self.state.lock().await.observe(&downloader, torrents);
            for notification in notifications {
                let hash = notification.torrent.hash.clone();
                let name = notification.torrent.name.clone();
                let receiver_count = self.tx.receiver_count();
                let _ = self.tx.send(Arc::new(notification));
                debug!(
                    downloader_id = downloader.id,
                    downloader_name = %downloader.name,
                    torrent_hash = %hash,
                    torrent_name = %name,
                    subscribers = receiver_count,
                    "published new torrent notification"
                );
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn downloader(id: i64, url: &str) -> DownloaderRecord {
        DownloaderRecord {
            id,
            name: format!("qB {id}"),
            downloader_type: "qbittorrent".to_string(),
            url: url.to_string(),
            username: "user".to_string(),
            password: "secret".to_string(),
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    fn torrent(hash: &str, added_on: i64) -> TorrentInfo {
        TorrentInfo {
            hash: hash.to_string(),
            name: format!("torrent-{hash}"),
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
            added_on,
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

    #[test]
    fn first_snapshot_is_a_baseline_and_later_additions_emit_once() {
        let mut state = DetectionState::default();
        let downloader = downloader(1, "http://qb:8080/");

        assert!(
            state
                .observe(&downloader, vec![torrent("AAAA", 1)])
                .is_empty()
        );
        let notifications =
            state.observe(&downloader, vec![torrent("aaaa", 1), torrent("BBBB", 2)]);
        assert_eq!(notifications.len(), 1);
        assert_eq!(notifications[0].torrent.hash, "BBBB");
        assert!(
            state
                .observe(&downloader, vec![torrent("AAAA", 1), torrent("bbbb", 2)])
                .is_empty()
        );
    }

    #[test]
    fn removed_then_readded_torrent_is_new_again() {
        let mut state = DetectionState::default();
        let downloader = downloader(1, "http://qb:8080");

        state.observe(&downloader, vec![torrent("aaaa", 1), torrent("bbbb", 2)]);
        assert!(
            state
                .observe(&downloader, vec![torrent("aaaa", 1)])
                .is_empty()
        );
        let notifications =
            state.observe(&downloader, vec![torrent("aaaa", 1), torrent("bbbb", 3)]);

        assert_eq!(notifications.len(), 1);
        assert_eq!(notifications[0].torrent.hash, "bbbb");
    }

    #[test]
    fn downloader_identity_change_creates_a_fresh_baseline() {
        let mut state = DetectionState::default();
        let original = downloader(1, "http://qb-one:8080");
        let replacement = downloader(1, "http://qb-two:8080");

        state.observe(&original, vec![torrent("aaaa", 1)]);
        assert!(
            state
                .observe(&replacement, vec![torrent("bbbb", 2)])
                .is_empty()
        );
        let notifications =
            state.observe(&replacement, vec![torrent("bbbb", 2), torrent("cccc", 3)]);

        assert_eq!(notifications.len(), 1);
        assert_eq!(notifications[0].torrent.hash, "cccc");
    }

    #[tokio::test]
    async fn broadcast_delivers_one_notification_to_every_subscriber() {
        let (tx, _) = broadcast::channel(4);
        let mut first = tx.subscribe();
        let mut second = tx.subscribe();
        let downloader = downloader(7, "http://qb:8080");
        let notification = Arc::new(NewTorrentNotification::new(&downloader, torrent("aaaa", 1)));

        tx.send(notification.clone()).unwrap();

        assert!(Arc::ptr_eq(&first.recv().await.unwrap(), &notification));
        assert!(Arc::ptr_eq(&second.recv().await.unwrap(), &notification));
    }
}
