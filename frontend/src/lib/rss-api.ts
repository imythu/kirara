import { api, ApiError } from "@/lib/api";

export type RssPage<T> = { items: T[]; total: number; page: number; page_size: number };
export type RssAttributes = {
  size_bytes: number | null; seeders: number | null; leechers: number | null;
  download_volume_factor: number | null; upload_volume_factor: number | null;
  hr: boolean | null; minimum_ratio: number | null; minimum_seed_time: number | null;
  free_until: number | null; observed_at: string; source: string; hints: string[];
};
export type RssFeed = {
  id: number; name: string; url_display: string; site_id: number | null; site_name: string | null;
  use_proxy: boolean | null; enabled: boolean; interval_minutes: number; generation: number; version: number;
  initialized_at: string | null; last_sequence: number; last_checked_at: string | null; next_run_at: string | null;
  last_status: string; last_error: string | null; item_count: number; pending_count: number;
  created_at: string; updated_at: string;
};
export type RssFeedInput = Pick<RssFeed, "name" | "site_id" | "use_proxy" | "enabled" | "interval_minutes"> & {
  url: string | null; expected_version?: number; request_id?: string;
};
export type RssFilters = {
  include: string[]; include_mode: string; exclude: string[]; include_regex: string | null;
  exclude_regex: string | null; match_all: boolean; min_size_bytes: number | null; max_size_bytes: number | null;
  min_seeders: number | null; free_only: boolean; hr_policy: string;
};
export type RssOptions = { save_path: string | null; category: string | null; tags: string[]; paused: boolean; reserve_space_bytes: number };
export type RssRuleInput = {
  name: string; enabled: boolean; priority: number; feed_ids: number[]; filters: RssFilters;
  downloader_id: number | null; options: RssOptions; expected_version?: number; request_id?: string;
};
export type RssRule = RssRuleInput & {
  id: number; downloader_name: string | null; match_revision: number; version: number; last_error: string | null;
  matched_count: number; created_at: string; updated_at: string;
};
export type RssReason = { code: string; message: string; field: string | null; actual: string | null; expected: string | null };
export type RssEvaluation = { matched: boolean; needs_attributes: boolean; reasons: RssReason[] };
export type RssDecision = {
  id: number; item_id: number; rule_id: number; rule_name: string; match_revision: number; status: string;
  evaluation: RssEvaluation; checked_at: string | null; next_evaluate_at: string | null; job_id: number | null;
};
export type RssItem = {
  id: number; feed_id: number; feed_name: string; generation: number; item_key: string; sequence: number; title: string;
  detail_url: string | null; site_torrent_id: string | null; published_at: string | null; categories: string[];
  attributes: RssAttributes; downloadable: boolean; content_revision: number; first_seen_at: string; last_seen_at: string;
  status: string; decisions: RssDecision[];
};
export type RssJob = {
  id: number; item_id: number; feed_id: number; feed_generation: number; feed_name: string; rule_id: number;
  rule_name: string; downloader_id: number; downloader_name: string; title: string; size_bytes: number | null;
  filters_snapshot: RssFilters; options_snapshot: RssOptions; decision_snapshot: RssEvaluation;
  status: string; infohash: string | null; reserved_bytes: number; attempts: number; next_attempt_at: string | null;
  version: number; last_error: string | null; created_at: string; updated_at: string; submitted_at: string | null;
  download_state: string | null; progress: number | null; sampled_at: string | null;
};
export type RssRun = {
  id: number; feed_id: number | null; kind: string; status: string; item_count: number; new_count: number;
  queued_count: number; pending_count: number; message: string | null; started_at: string; finished_at: string | null;
};
export type RssSummary = {
  feeds_total: number; running: number; paused: number; needs_attention: number;
  rules_enabled: number; queued: number; submitted: number; failed: number;
};
export type RssPreview = {
  rule_version: number | null;
  total: number; matched: number; rejected: number; unknown: number; sample_limited: boolean;
  sample_time: string; items: { item: RssItem; evaluation: RssEvaluation }[];
};
export type RssFeedTest = { title: string | null; item_count: number; items: RssItem[]; warnings: string[]; sample_time: string };

export const rssApi = {
  get: <T,>(path: string) => api<T>(`/api/rss${path}`),
  post: <T,>(path: string, body: unknown) => api<T>(`/api/rss${path}`, { method: "POST", body: JSON.stringify(body) }),
  put: <T,>(path: string, body: unknown) => api<T>(`/api/rss${path}`, { method: "PUT", body: JSON.stringify(body) }),
  archive: (path: string, body: unknown) => api<void>(`/api/rss${path}`, { method: "DELETE", body: JSON.stringify(body) }),
};

export async function allRssRecords<T>(path: string): Promise<T[]> {
  const items: T[] = [];
  for (let page = 1; ; page += 1) {
    const data = await rssApi.get<RssPage<T>>(`${path}${path.includes("?") ? "&" : "?"}page=${page}&page_size=200`);
    items.push(...data.items);
    if (items.length >= data.total || data.items.length === 0) return items;
  }
}

export function newRequestId() {
  return typeof crypto.randomUUID === "function" ? crypto.randomUUID() : `${Date.now()}-${Math.random().toString(36).slice(2)}`;
}

export function rssError(error: unknown): string {
  if (error instanceof ApiError && error.status === 409) return `${error.message}。配置或状态已变化，请载入最新数据后重试；当前草稿已保留。`;
  if (error instanceof TypeError) return "暂时无法连接服务，请检查网络后重试。当前输入已保留。";
  return error instanceof Error ? error.message : "操作未完成，请重试。";
}

export function ruleInput(rule: RssRule): RssRuleInput {
  return {
    name: rule.name, enabled: rule.enabled, priority: rule.priority, feed_ids: [...rule.feed_ids],
    filters: { ...rule.filters }, downloader_id: rule.downloader_id, options: { ...rule.options }, expected_version: rule.version,
  };
}

export const GIB = 1024 ** 3;

export function rssDate(value?: string | null) {
  if (!value) return "尚无记录";
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? "时间未知" : date.toLocaleString("zh-CN", { month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit", hour12: false });
}

export function rssSize(bytes?: number | null): string {
  if (bytes == null) return "大小未知";
  if (bytes >= GIB) return `${(bytes / GIB).toFixed(2)} GiB`;
  if (bytes >= 1024 ** 2) return `${(bytes / 1024 ** 2).toFixed(1)} MiB`;
  if (bytes >= 1024) return `${(bytes / 1024).toFixed(1)} KiB`;
  return `${bytes} B`;
}

export const rssStatusLabels: Record<string, string> = {
  active: "运行中", enabled: "运行中", paused: "已暂停", disabled: "已暂停", uninitialized: "等待首次检查", idle: "等待检查",
  pending: "待处理", pending_attributes: "待补充属性", attribute_unknown: "属性待确认", baseline: "已有资源", skipped: "已跳过", rejected: "未符合",
  matched: "符合规则", queued: "已加入队列", held: "已暂停等待", fetching: "正在准备种子", waiting: "等待条件", retry_wait: "等待重试",
  submitting: "正在提交", reconciling: "正在确认添加结果", submitted: "已添加到下载器", already_present: "下载器已存在", failed: "需处理",
  cancelled: "已取消", source_changed: "来源已更换", lower_priority: "低优先级规则", already_queued: "已加入队列",
  observed: "已收集", ready: "待入队", priority_wait: "等待优先规则", unavailable: "无法下载", ignored: "未命中规则", processing: "正在处理", scheduled: "等待检查",
  running: "检查中", completed: "处理完成", success: "检查成功", ok: "检查成功", not_modified: "内容未更新", error: "检查失败", archived: "已归档",
};

export function rssStatus(status: string) { return rssStatusLabels[status] ?? "状态待确认"; }

export function canCancelJob(job: RssJob) { return ["queued", "held", "fetching", "waiting", "retry_wait", "failed", "source_changed"].includes(job.status); }
export function canRetryJob(job: RssJob) { return ["failed", "waiting", "retry_wait"].includes(job.status); }
export function canReconcileJob(job: RssJob) { return ["submitting", "reconciling"].includes(job.status); }

export function downloadState(job: RssJob): string {
  const state = job.download_state;
  if (!state) return "尚未采集";
  const labels: Record<string, string> = { missing: "下载器中未找到", not_found: "下载器中未找到", error: "下载器错误", missingFiles: "文件丢失", pausedDL: "已暂停", stoppedDL: "已暂停", pausedUP: "已暂停（已完成）", stoppedUP: "已暂停（已完成）", checkingDL: "校验中", checkingUP: "校验中", queuedDL: "等待下载", queuedUP: "等待做种", stalledDL: "等待连接", downloading: "下载中", forcedDL: "下载中", metaDL: "获取元数据", uploading: "做种中", forcedUP: "做种中", stalledUP: "做种中", completed: "已完成", moving: "移动文件中", checkingResumeData: "检查恢复数据" };
  return labels[state] ?? `下载器状态：${state}`;
}
