use chrono::Utc;
use tracing::info;

use crate::brush::BrushTaskRecord;
use crate::brush::BrushTorrentRecord;
use crate::db::Database;
use crate::downloader::TorrentInfo;

/// 评估删种规则，返回需要删除的种子 (hash, reason)
pub async fn evaluate_delete_rules(
    task: &BrushTaskRecord,
    managed: &[BrushTorrentRecord],
    downloader_torrents: &[TorrentInfo],
    db: &Database,
) -> Vec<(String, String)> {
    let is_and_mode = task.delete_mode == "and";
    let mut to_remove = Vec::new();

    for record in managed {
        if record.status != "active" {
            continue;
        }

        let short_hash = &record.torrent_hash[..8.min(record.torrent_hash.len())];
        let torrent_name = &record.torrent_name;

        // 查找对应的下载器种子信息
        let Some(dl_info) = find_matching_downloader_torrent(record, downloader_torrents) else {
            info!(
                "[删种评估][{}] hash={} name={} → 下载器中不存在，标记移除",
                task.name, short_hash, torrent_name
            );
            to_remove.push((record.torrent_hash.clone(), "下载器中不存在".to_string()));
            continue;
        };

        let mut reasons = Vec::new();
        let mut rule_details = Vec::new();
        let mut rule_results = Vec::new();

        // 规则 1: 最小做种时长
        if let Some(min_hours) = task.min_seed_time_hours {
            let seed_hours = dl_info.time_active as f64 / 3600.0;
            let passed = seed_hours >= min_hours;
            rule_results.push(passed);
            rule_details.push(format!(
                "做种时间 {:.1}h {} {:.1}h",
                seed_hours,
                if passed { ">=" } else { "<" },
                min_hours
            ));
            if passed {
                reasons.push(format!("做种时间 {:.1}h >= {:.1}h", seed_hours, min_hours));
            }
        }

        // 规则 1.5: free 到期删除
        if task.delete_on_free_expiry {
            if let Some(free_end_timestamp) = record.free_end_timestamp {
                let now_secs = Utc::now().timestamp();
                let passed = now_secs >= free_end_timestamp;
                rule_results.push(passed);
                rule_details.push(if passed {
                    "free已到期".to_string()
                } else {
                    let remaining_hours = (free_end_timestamp - now_secs) as f64 / 3600.0;
                    format!("free未到期，剩余 {:.1}h", remaining_hours)
                });
                if passed {
                    reasons.push("free已到期".to_string());
                }
            }
        }

        // 规则 2: H&R 种子最小做种时长
        if record.is_hr {
            if let Some(hr_min_hours) = task.hr_min_seed_time_hours {
                let seed_hours = dl_info.time_active as f64 / 3600.0;
                let passed = seed_hours >= hr_min_hours;
                rule_results.push(passed);
                rule_details.push(format!(
                    "H&R做种 {:.1}h {} {:.1}h",
                    seed_hours,
                    if passed { ">=" } else { "<" },
                    hr_min_hours
                ));
                if passed {
                    reasons.push(format!(
                        "H&R做种时间 {:.1}h >= {:.1}h",
                        seed_hours, hr_min_hours
                    ));
                }
            }
        }

        // 规则 3: 分享率（仅针对已完成的种子）
        if let Some(target_ratio) = task.target_ratio {
            if is_torrent_incomplete(dl_info) {
                rule_details.push("未完成下载，跳过分享率规则".to_string());
            } else {
                // qBittorrent 在下载量为 0 时用 ratio = -1 表示「无限分享率」。
                // 直接比较会让这类种子永远无法满足规则，需归一化为无穷大。
                let effective_ratio = if dl_info.ratio < 0.0 {
                    f64::INFINITY
                } else {
                    dl_info.ratio
                };
                let passed = effective_ratio >= target_ratio;
                rule_results.push(passed);
                let ratio_text = if effective_ratio.is_infinite() {
                    "∞".to_string()
                } else {
                    format!("{:.2}", effective_ratio)
                };
                rule_details.push(format!(
                    "分享率 {} {} {:.2}",
                    ratio_text,
                    if passed { ">=" } else { "<" },
                    target_ratio
                ));
                if passed {
                    reasons.push(format!("分享率 {} >= {:.2}", ratio_text, target_ratio));
                }
            }
        }

        // 规则 3.5: 未完成种子的上传量阈值
        // 对于未完成种子，分享率无法直接计算，改用 上传量 >= 目标分享率 × 种子大小 判断。
        // 注意：这里比较的是上传量与 threshold（target_ratio * size），而非实际分享率。
        if let Some(target_ratio) = task.target_ratio {
            if is_torrent_incomplete(dl_info) && dl_info.size > 0 {
                let threshold_bytes = (target_ratio * dl_info.size as f64) as i64;
                let passed = dl_info.uploaded >= threshold_bytes;
                rule_results.push(passed);
                let uploaded_gb = dl_info.uploaded as f64 / (1024.0 * 1024.0 * 1024.0);
                let threshold_gb = threshold_bytes as f64 / (1024.0 * 1024.0 * 1024.0);
                rule_details.push(format!(
                    "未完成上传量 {:.2}GB {} {:.2}GB (分享率{:.2}×大小)",
                    uploaded_gb,
                    if passed { ">=" } else { "<" },
                    threshold_gb,
                    target_ratio
                ));
                if passed {
                    reasons.push(format!(
                        "未完成种子上传量 {:.2}GB >= {:.2}GB (分享率{:.2}×大小)",
                        uploaded_gb, threshold_gb, target_ratio
                    ));
                }
            }
        }

        // 规则 4: 上传量
        if let Some(max_gb) = task.max_upload_gb {
            let uploaded_gb = dl_info.uploaded as f64 / (1024.0 * 1024.0 * 1024.0);
            let passed = uploaded_gb >= max_gb;
            rule_results.push(passed);
            rule_details.push(format!(
                "上传量 {:.2}GB {} {:.2}GB",
                uploaded_gb,
                if passed { ">=" } else { "<" },
                max_gb
            ));
            if passed {
                reasons.push(format!("上传量 {:.2}GB >= {:.2}GB", uploaded_gb, max_gb));
            }
        }

        // 规则 5: 下载耗时（仅针对未完成的卡死下载）
        if let Some(timeout_hours) = task.download_timeout_hours {
            // 仅凭 completion_on 判断是否完成不可靠：不同 qBittorrent / libtorrent
            // 版本对未完成种子的 completion_on 取值不一（-1、0，甚至 u32::MAX 这类
            // 很大的正数），后者会被误判为「已完成」从而跳过本规则。
            // 这里以 已下载字节 < 种子大小 作为「未完成」的可靠判据。
            let is_incomplete = is_torrent_incomplete(dl_info);
            if is_incomplete {
                let added_secs = dl_info.added_on;
                let now_secs = Utc::now().timestamp();
                let elapsed_hours = (now_secs - added_secs) as f64 / 3600.0;
                // added_on 缺失 (<=0) 时无法计算存活时长，跳过以免误删。
                if added_secs > 0 {
                    let passed = elapsed_hours >= timeout_hours;
                    rule_results.push(passed);
                    rule_details.push(format!(
                        "下载耗时 {:.1}h {} {:.1}h (未完成)",
                        elapsed_hours,
                        if passed { ">=" } else { "<" },
                        timeout_hours
                    ));
                    if passed {
                        reasons.push(format!(
                            "下载耗时 {:.1}h >= {:.1}h，未完成",
                            elapsed_hours, timeout_hours
                        ));
                    }
                } else {
                    rule_details.push("缺少添加时间，跳过超时规则".to_string());
                }
            } else {
                rule_details.push("下载已完成，跳过超时规则".to_string());
            }
        }

        // 规则 6: 最近10分钟与1分钟平均上传速度均低于阈值
        if let Some(min_speed) = task.min_avg_upload_speed_kbs {
            if is_torrent_incomplete(dl_info) {
                rule_details.push("未完成下载，跳过速度规则".to_string());
            } else {
                let active_enough = dl_info.time_active > 600;
                let avg_10min =
                    get_recent_avg_upload_speed(db, task.id, &record.torrent_hash, 10).await;
                let avg_1min =
                    get_recent_avg_upload_speed(db, task.id, &record.torrent_hash, 1).await;
                match (avg_10min, avg_1min) {
                    // 仅在有足够流量样本时才评估该规则，避免重启/采集滞后导致
                    // 速度被当成 0 从而误删正常做种的种子。
                    (Some(avg_10min), Some(avg_1min)) if active_enough => {
                        let avg_10min_kbs = avg_10min / 1024.0;
                        let avg_1min_kbs = avg_1min / 1024.0;
                        let passed_10min = avg_10min_kbs < min_speed;
                        let passed_1min = avg_1min_kbs < min_speed;
                        let passed = passed_10min && passed_1min;
                        rule_results.push(passed);
                        rule_details.push(format!(
                            "近10min上传 {:.1}KB/s {} {:.1}KB/s，近1min上传 {:.1}KB/s {} {:.1}KB/s",
                            avg_10min_kbs,
                            if passed_10min { "<" } else { ">=" },
                            min_speed,
                            avg_1min_kbs,
                            if passed_1min { "<" } else { ">=" },
                            min_speed
                        ));
                        if passed {
                            reasons.push(format!(
                                "近10分钟平均上传 {:.1}KB/s < {:.1}KB/s 且近1分钟平均上传 {:.1}KB/s < {:.1}KB/s",
                                avg_10min_kbs, min_speed, avg_1min_kbs, min_speed
                            ));
                        }
                    }
                    (Some(_), Some(_)) => {
                        rule_details.push(format!(
                            "活跃 {:.0}s < 600s，跳过速度规则",
                            dl_info.time_active
                        ));
                    }
                    (None, Some(_)) => {
                        rule_details.push("近10分钟流量样本不足，跳过速度规则".to_string());
                    }
                    (Some(_), None) => {
                        rule_details.push("近1分钟流量样本不足，跳过速度规则".to_string());
                    }
                    (None, None) => {
                        rule_details
                            .push("近10分钟和近1分钟流量样本不足，跳过速度规则".to_string());
                    }
                }
            }
        }

        // 规则 7: 最大未活跃时长
        if let Some(max_hours) = task.max_inactive_hours {
            if dl_info.last_activity > 0 {
                let now_secs = Utc::now().timestamp();
                let inactive_hours = (now_secs - dl_info.last_activity) as f64 / 3600.0;
                let passed = inactive_hours >= max_hours;
                rule_results.push(passed);
                rule_details.push(format!(
                    "未活跃 {:.1}h {} {:.1}h",
                    inactive_hours,
                    if passed { ">=" } else { "<" },
                    max_hours
                ));
                if passed {
                    reasons.push(format!(
                        "未活跃 {:.1}h >= {:.1}h",
                        inactive_hours, max_hours
                    ));
                }
            } else {
                rule_details.push("无活跃记录，跳过不活跃规则".to_string());
            }
        }

        // 根据模式判断是否需要删除
        if rule_results.is_empty() {
            info!(
                "[删种评估][{}] hash={} name={} → 未配置删种规则，跳过",
                task.name, short_hash, torrent_name
            );
            continue;
        }

        let should_remove = if is_and_mode {
            rule_results.iter().all(|&r| r)
        } else {
            rule_results.iter().any(|&r| r)
        };

        if should_remove && !reasons.is_empty() {
            info!(
                "[删种评估][{}] hash={} name={} → 删除 [{}] {}",
                task.name,
                short_hash,
                torrent_name,
                if is_and_mode { "AND" } else { "OR" },
                reasons.join("; ")
            );
            // 使用下载器中的实际 hash，避免数据库 hash 与下载器 hash 不一致导致暂停/删除 404
            to_remove.push((dl_info.hash.clone(), reasons.join("; ")));
        } else {
            info!(
                "[删种评估][{}] hash={} name={} → 保留 [{}] {}",
                task.name,
                short_hash,
                torrent_name,
                if is_and_mode { "AND" } else { "OR" },
                rule_details.join("; ")
            );
        }
    }

    to_remove
}

/// 判断下载器中的种子是否「未完成下载」。
/// 不同 qBittorrent/libtorrent 版本对未完成种子的 `completion_on` 取值不一致
/// （-1、0 或很大的正数），因此以「已下载 < 大小」作为可靠判据，
/// 仅当大小未知 (<=0) 时回退到 `completion_on`。
pub(crate) fn is_torrent_incomplete(dl_info: &TorrentInfo) -> bool {
    if dl_info.size > 0 {
        dl_info.downloaded < dl_info.size
    } else {
        dl_info.completion_on <= 0
    }
}

fn find_matching_downloader_torrent<'a>(
    record: &BrushTorrentRecord,
    downloader_torrents: &'a [TorrentInfo],
) -> Option<&'a TorrentInfo> {
    downloader_torrents
        .iter()
        .find(|torrent| torrent.hash.eq_ignore_ascii_case(&record.torrent_hash))
        .or_else(|| {
            downloader_torrents
                .iter()
                .find(|torrent| torrent.name == record.torrent_name)
        })
}

/// 获取最近指定分钟数的平均上传速度 (bytes/s)。
/// 流量样本不足 (< 2) 或时间戳无法解析时返回 `None`，调用方应跳过该规则而非按 0 处理。
async fn get_recent_avg_upload_speed(
    db: &Database,
    task_id: i64,
    hash: &str,
    minutes: i64,
) -> Option<f64> {
    match db.get_recent_torrent_traffic(task_id, hash, minutes).await {
        Ok(snapshots) if snapshots.len() >= 2 => {
            let first = &snapshots[0];
            let last = &snapshots[snapshots.len() - 1];
            let bytes_diff = (last.0 - first.0).max(0) as f64;

            // 解析时间差
            let first_time = chrono::DateTime::parse_from_rfc3339(&first.2).ok()?;
            let last_time = chrono::DateTime::parse_from_rfc3339(&last.2).ok()?;
            let secs = (last_time - first_time).num_seconds().max(1) as f64;
            Some(bytes_diff / secs)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::is_torrent_incomplete;
    use crate::downloader::TorrentInfo;

    fn torrent(size: i64, downloaded: i64, completion_on: i64) -> TorrentInfo {
        TorrentInfo {
            hash: "h".to_string(),
            name: "n".to_string(),
            size,
            uploaded: 0,
            downloaded,
            amount_left: None,
            completed: None,
            progress: if size > 0 {
                downloaded as f64 / size as f64
            } else {
                0.0
            },
            upload_speed: 0,
            download_speed: 0,
            ratio: 0.0,
            state: String::new(),
            added_on: 0,
            completion_on,
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

    #[test]
    fn incomplete_by_downloaded_less_than_size() {
        // 即便 completion_on 是很大的正数（某些 qB 版本对未完成种子的取值），
        // 只要 已下载 < 大小 就应判为未完成，确保超时规则能生效。
        assert!(is_torrent_incomplete(&torrent(1000, 400, 4_294_967_295)));
        assert!(is_torrent_incomplete(&torrent(1000, 999, -1)));
    }

    #[test]
    fn complete_when_downloaded_reaches_size() {
        assert!(!is_torrent_incomplete(&torrent(1000, 1000, 0)));
        assert!(!is_torrent_incomplete(&torrent(1000, 1200, 0)));
    }

    #[test]
    fn falls_back_to_completion_on_when_size_unknown() {
        assert!(is_torrent_incomplete(&torrent(0, 0, -1)));
        assert!(!is_torrent_incomplete(&torrent(0, 0, 12345)));
    }
}
