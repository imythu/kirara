export type GlobalConfig = {
  log_level: string | null;
  proxy: string | null;
  use_proxy_for_lightpanda: boolean;
  lightpanda: {
    endpoint: string | null;
    token: string | null;
    region: "euwest" | "uswest" | string;
    browser: string;
    proxy: string | null;
    country: string | null;
  };
  browserless: {
    address: string | null;
    token: string | null;
  };
  tag_rule_scan_interval_mins?: number;
  vision_llm: { base_url: string; model: string; api_key: string | null; api_standard: "claude" | "openai_compatible" | "openai_responses"; api_key_configured: boolean; clear_api_key: boolean };
};

export type ProxyTestRequest = {
  proxy: string;
  test_url: string;
};

export type ProxyTestResult = {
  success: boolean;
  status_code: number | null;
  elapsed_ms: number;
  message: string;
};

// ========== PT 刷流模块类型 ==========

export type SiteRecord = {
  id: number;
  name: string;
  site_type: string;
  base_url: string;
  auth_type: "cookie" | "passkey" | "cookie_passkey" | "api_key" | null;
  auth_configured: boolean;
  use_proxy: boolean;
  created_at: string;
  updated_at: string;
  stats: SiteStatsRecord | null;
};

export type PtdSitePreset = {
  ptd_id: string;
  name: string;
  site_type: "nexusphp" | "mteam" | "gazelle";
  base_url: string;
  aliases: string[];
};

export type SiteRequestHeader = {
  name: string;
  value: string;
};

export type SiteCredentialsRecord = {
  auth_type: "cookie" | "passkey" | "cookie_passkey" | "api_key";
  cookie: string | null;
  passkey: string | null;
  api_key: string | null;
};

export type UserStatsDetails = {
  is_donor?: boolean | null;
  level_id?: number | null;
  level_name?: string | null;
  join_time?: number | null;
  last_access_at?: number | null;
  message_count?: number | null;
  invites?: number | null;
  avatar?: string | null;
  total_traffic?: number | null;
  true_downloaded?: number | null;
  true_uploaded?: number | null;
  true_ratio?: number | null;
  seeding_size?: number | null;
  seeding_time?: number | null;
  average_seeding_time?: number | null;
  seeding_bonus?: number | null;
  bonus_per_hour?: number | null;
  seeding_bonus_per_hour?: number | null;
  uploads?: number | null;
  snatches?: number | null;
  posts?: number | null;
  adoptions?: number | null;
  hnr_unsatisfied?: number | null;
  hnr_pre_warning?: number | null;
  email?: string | null;
};

export type SiteStatsRecord = UserStatsDetails & {
  site_id: number;
  uid: string | null;
  username: string | null;
  uploaded: number | null;
  downloaded: number | null;
  ratio: number | null;
  bonus: number | null;
  seeding_count: number | null;
  leeching_count: number | null;
  updated_at: string | null;
  last_checked_at: string;
  last_error: string | null;
};

export type SiteSyncResult = {
  success: boolean;
  message: string;
  user_stats: UserStats | null;
};

export type SiteStatsRefreshStartResponse = {
  started: boolean;
  refreshing: boolean;
};

export type SiteStatsRefreshStatusResponse = {
  refreshing: boolean;
};

export type PtdBackupConfig = {
  enabled: boolean;
  webdav_url: string;
  username: string;
  password_configured: boolean;
  use_proxy: boolean;
  backup_interval_hours: number;
  site_identifiers: Record<string, string | null>;
  configured: boolean;
  last_backup_at: string | null;
  last_backup_filename: string | null;
  last_error: string | null;
  updated_at: string;
};

export type PtdBackupTestResult = {
  success: boolean;
  message: string;
};

export type PtdBackupRunResult = {
  filename: string;
  site_count: number;
  size: number;
  backed_up_at: string;
};

export type UserStats = UserStatsDetails & {
  uid: string | null;
  username: string;
  uploaded: number;
  downloaded: number;
  ratio: number | null;
  bonus: number | null;
  seeding_count: number | null;
  leeching_count: number | null;
};

export type DownloaderRecord = {
  id: number;
  name: string;
  downloader_type: string;
  url: string;
  username: string;
  password_configured: boolean;
  created_at: string;
  updated_at: string;
};

export type DownloaderTestResult = {
  success: boolean;
  message: string;
  version: string | null;
  free_space: number | null;
};

export type DownloaderSpaceStats = {
  free_space: number;
  pending_download_bytes: number;
  effective_free_space: number;
  torrent_count: number;
  incomplete_count: number;
};

export type TransferableTorrent = {
  hash: string;
  name: string;
  size: number;
  downloaded: number;
  amount_left: number;
  completed: number;
  incomplete: boolean;
  save_path: string;
  category: string;
  tags: string;
  added_on: number;
  progress: number;
  state: string;
};

export type BrushTaskRecord = {
  id: number;
  name: string;
  cron_expression: string;
  site_id: number | null;
  downloader_ids: number[];
  tag: string;
  rss_url: string;
  seed_volume_gb: number | null;
  save_dir: string | null;
  active_time_windows: string | null;
  promotion: string;
  skip_hit_and_run: boolean;
  max_concurrent: number;
  download_speed_limit: number | null;
  upload_speed_limit: number | null;
  size_ranges: string | null;
  seeder_ranges: string | null;
  downloader_ranges: string | null;
  downloader_weights: string | null;
  min_free_hours: number | null;
  delete_mode: string;
  delete_on_free_expiry: boolean;
  min_seed_time_hours: number | null;
  hr_min_seed_time_hours: number | null;
  target_ratio: number | null;
  max_upload_gb: number | null;
  download_timeout_hours: number | null;
  min_avg_upload_speed_kbs: number | null;
  max_inactive_hours: number | null;
  min_disk_space_gb: number | null;
  enabled: boolean;
  created_at: string;
  updated_at: string;
  last_run_info: string | null;
};

export type BrushTaskLastRunInfo = {
  trigger_type: string;
  started_at: string;
  finished_at: string;
  duration_secs: number;
  status: string;
  error?: string | null;
  early_exit_reason?: string | null;
  downloaders: {
    candidates: { id: number; name: string; free_space_gb: number; weight: number }[];
    skipped: { id: number; name: string; reason: string; detail?: string | null }[];
  };
  sync: { managed_before: number; missing_marked_removed: number };
  concurrency: { active_count: number; max_concurrent: number; can_add: number };
  seed_volume: { current_gb: number; limit_gb?: number | null };
  source: { type: string; items_parsed: number };
  selection: {
    checked: number;
    added: number;
    failed: number;
    skipped_detail_failure: number;
    skipped_existing: number;
    skipped_pre_filter: number;
    skipped_post_filter: number;
    skipped_no_space: number;
  };
  added_torrents: {
    title: string;
    hash: string;
    size_bytes: number | null;
    downloader_id: number;
    downloader_name: string;
    is_hr: boolean;
    is_free: boolean;
  }[];
  failed_torrents: { title: string; reason: string; detail?: string | null }[];
};

export type BrushTaskRequest = {
  name: string;
  cron_expression: string;
  site_id?: number | null;
  downloader_ids: number[];
  tag: string;
  rss_url: string;
  seed_volume_gb?: number | null;
  save_dir?: string | null;
  active_time_windows?: string | null;
  promotion?: string | null;
  skip_hit_and_run?: boolean | null;
  max_concurrent?: number | null;
  download_speed_limit?: number | null;
  upload_speed_limit?: number | null;
  size_ranges?: string | null;
  seeder_ranges?: string | null;
  downloader_ranges?: string | null;
  downloader_weights?: string | null;
  min_free_hours?: number | null;
  delete_mode?: string | null;
  delete_on_free_expiry?: boolean | null;
  min_seed_time_hours?: number | null;
  hr_min_seed_time_hours?: number | null;
  target_ratio?: number | null;
  max_upload_gb?: number | null;
  download_timeout_hours?: number | null;
  min_avg_upload_speed_kbs?: number | null;
  max_inactive_hours?: number | null;
  min_disk_space_gb?: number | null;
};

export type SignInTaskRecord = {
  id: number;
  name: string;
  site_id: number;
  cron_expression: string;
  browser: string;
  sign_in_method: string;
  browserless: BrowserlessTaskConfig;
  enabled: boolean;
  last_status: string | null;
  last_message: string | null;
  last_run_at: string | null;
  created_at: string;
  updated_at: string;
};

export type SignInTaskRequest = {
  name: string;
  site_id: number;
  cron_expression: string;
  browser?: "lightpanda" | "browserless" | null;
  sign_in_method?: string | null;
  browserless?: BrowserlessTaskConfig | null;
};

export type SignInResultRule = {
  outcome: "success" | "already" | "failed";
  kind: "text" | "selector" | "json";
  selector: string;
  field: string;
  value: string;
  value_type: "string" | "number" | "boolean" | "null";
};

export type BrowserlessTaskConfig = {
  submit_method: "click" | "form" | "ajax";
  result_rules: SignInResultRule[];
  attendance_path: string;
  captcha_selector: string;
  captcha_input_selector: string;
  already_keywords: string;
  selector: string;
  cf_mode: "auto" | "page" | "turnstile";
  wait_ms: number | null;
  solve_timeout: number | null;
  action_timeout: number | null;
  post_click_wait_ms: number | null;
};

export type SignInRecord = {
  id: number;
  task_id: number;
  site_id: number;
  site_name: string;
  started_at: string;
  finished_at: string;
  status: string;
  message: string;
};

export type BrowserProbeResult = {
  success: boolean;
  url: string;
  message: string;
  title: string | null;
};

export type BrushTorrentRecord = {
  id: number;
  task_id: number;
  torrent_id: string | null;
  torrent_link: string | null;
  torrent_hash: string;
  torrent_name: string;
  added_at: string;
  size_bytes: number | null;
  is_hr: boolean;
  free_end_timestamp: number | null;
  status: string;
  removed_at: string | null;
  remove_reason: string | null;
  uploaded_bytes: number;
  downloaded_bytes: number;
  download_duration_secs: number;
  avg_upload_speed: number;
  ratio: number;
  last_stats_at: string | null;
  downloader_id: number | null;
};

export type BrushTaskTorrentsResponse = {
  task: BrushTaskRecord;
  page: number;
  page_size: number;
  total_records: number;
  records: BrushTorrentRecord[];
};

export type TaskStatsSnapshot = {
  id: number;
  task_id: number;
  total_uploaded: number;
  total_downloaded: number;
  torrent_count: number;
  recorded_at: string;
};

export type DownloaderSpeedSnapshot = {
  id: number;
  downloader_id: number;
  upload_speed: number;
  download_speed: number;
  recorded_at: string;
};

export type StatsOverview = {
  tasks: TaskOverview[];
};

// ========== 系统监控 ==========

export type SystemSnapshot = {
  recorded_at: string;
  process_cpu_usage: number;
  process_memory_bytes: number;
  process_memory_mb: number;
  system_cpu_usage: number;
  system_total_memory_bytes: number;
  system_used_memory_bytes: number;
  system_available_memory_bytes: number;
  system_memory_usage_percent: number;
};

export type SystemSnapshotRecord = {
  id: number;
  process_cpu_usage: number;
  process_memory_bytes: number;
  system_cpu_usage: number;
  system_total_memory_bytes: number;
  system_used_memory_bytes: number;
  system_available_memory_bytes: number;
  recorded_at: string;
};

export type TaskOverview = {
  task_id: number;
  task_name: string;
  total_uploaded: number;
  total_downloaded: number;
  torrent_count: number;
  enabled: boolean;
};

export type DailyTransferItem = {
  date: string;
  uploaded: number;
  downloaded: number;
};

// ========== 标签规则 ==========

export type TagMatchCriteria = {
  match_type: "prefix" | "suffix" | "contains" | "exact" | "regex";
  pattern: string;
};

export type TagRuleRecord = {
  id: number;
  name: string;
  tag_name: string;
  match_rules: string; // JSON string of TagMatchCriteria[]
  enabled: boolean;
  downloader_ids: string | null; // JSON string of number[] | null
  tagged_torrent_count: number;
  tagged_total_size: number;
  created_at: string;
  updated_at: string;
};

export type TagRuleRequest = {
  name: string;
  tag_name: string;
  match_rules: TagMatchCriteria[];
  enabled?: boolean;
  downloader_ids?: number[] | null;
};

export type TagRuleTrackerOption = {
  domain: string;
  torrent_count: number;
  downloader_ids: number[];
};

export type TagRuleTrackerDiscovery = {
  trackers: TagRuleTrackerOption[];
  failed_downloaders: string[];
};
