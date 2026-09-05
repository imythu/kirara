import { useEffect, useState } from "react";
import { Activity, ChevronLeft, ChevronRight, Edit, Eye, Pause, Play, Plus, Search, Trash2, Zap } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Dialog } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select } from "@/components/ui/select";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { api } from "@/lib/api";
import { formatDate } from "@/lib/format";
import { cn } from "@/lib/utils";
import type {
  BrushTaskLastRunInfo,
  BrushTaskRecord,
  BrushTaskRequest,
  BrushTaskTorrentsResponse,
  BrushTorrentRecord,
  DownloaderRecord,
  SiteRecord,
} from "@/types";

function formatBytes(bytes: number): string {
  if (bytes === 0) return "0 B";
  const k = 1024;
  const sizes = ["B", "KB", "MB", "GB", "TB", "PB"];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return `${(bytes / Math.pow(k, i)).toFixed(2)} ${sizes[i]}`;
}

function formatSpeed(bytesPerSec: number): string {
  if (!Number.isFinite(bytesPerSec) || bytesPerSec <= 0) return "0 B/s";
  return `${formatBytes(bytesPerSec)}/s`;
}

function formatDuration(totalSeconds: number): string {
  if (!Number.isFinite(totalSeconds) || totalSeconds <= 0) return "0s";
  const seconds = Math.floor(totalSeconds);
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  const secs = seconds % 60;
  if (hours > 0) return `${hours}h ${minutes}m`;
  if (minutes > 0) return `${minutes}m ${secs}s`;
  return `${secs}s`;
}

function formatHoursLabel(hours: number | null): string {
  if (hours == null || !Number.isFinite(hours) || hours <= 0) return "";
  const totalMinutes = Math.round(hours * 60);
  const h = Math.floor(totalMinutes / 60);
  const m = totalMinutes % 60;
  if (h > 0 && m > 0) return `= ${h}小时${m}分钟`;
  if (h > 0) return `= ${h}小时`;
  return `= ${m}分钟`;
}

function joinPath(base: string, sub: string): string {
  if (!sub) return base;
  const b = base.replace(/\/+$/, "");
  const s = sub.replace(/^\/+/, "");
  return s ? `${b}/${s}` : b;
}

function extractSubPath(
  saveDir: string | null,
  downloaderIds: number[],
  defaultPaths: Record<number, string>,
): string {
  if (!saveDir) return "";
  let obj: Record<string, string>;
  try {
    obj = JSON.parse(saveDir);
  } catch {
    return "";
  }
  for (const dlId of downloaderIds) {
    const full = obj[String(dlId)];
    const base = defaultPaths[dlId] ?? "";
    if (full && base && full.startsWith(base)) {
      return full.slice(base.length).replace(/^\/+/, "");
    }
    if (full && !base) return full;
  }
  return "";
}

function parseDownloaderWeights(json: string | null): Record<number, number> {
  if (!json) return {};
  try {
    const obj = JSON.parse(json) as Record<string, number>;
    const result: Record<number, number> = {};
    for (const [key, value] of Object.entries(obj)) {
      result[Number(key)] = value;
    }
    return result;
  } catch {
    return {};
  }
}

function buildDownloaderWeightsJson(
  ids: number[],
  weights: Record<number, number>,
): string | null {
  const map: Record<string, number> = {};
  for (const id of ids) {
    map[String(id)] = weights[id] ?? 1;
  }
  return Object.keys(map).length > 0 ? JSON.stringify(map) : null;
}

function parseLastRunInfo(info: string | null): BrushTaskLastRunInfo | null {
  if (!info) return null;
  try {
    return JSON.parse(info) as BrushTaskLastRunInfo;
  } catch {
    return null;
  }
}

const EARLY_EXIT_LABELS: Record<string, string> = {
  no_downloaders: "无可用下载器",
  concurrency_limit: "并发已满",
  seed_volume_limit: "保种体积已满",
};

const SKIP_REASON_LABELS: Record<string, string> = {
  not_exist: "不存在",
  client_create_failed: "客户端创建失败",
  free_space_fetch_failed: "空间查询失败",
  space_insufficient: "空间不足",
};

const FAIL_REASON_LABELS: Record<string, string> = {
  download_failed: "下载失败",
  invalid_torrent: "无效种子",
  all_downloaders_failed: "全部下载器添加失败",
  detail_fetch_failed: "详情获取失败",
};

function LastRunPanel({ info }: { info: BrushTaskLastRunInfo }) {
  const statusLabel =
    info.status === "success" ? "成功" : info.status === "failed" ? "失败" : info.status === "skipped" ? "跳过" : info.status;
  const statusColor =
    info.status === "success"
      ? "bg-emerald-100 text-emerald-700"
      : info.status === "failed"
        ? "bg-red-100 text-red-700"
        : "bg-amber-100 text-amber-700";
  const addedCap = info.added_torrents.slice(0, 200);
  const failedCap = info.failed_torrents.slice(0, 200);
  return (
    <div className="space-y-3 text-sm">
      <div className="flex flex-wrap items-center gap-2 text-xs">
        <span className={`rounded-full px-2.5 py-0.5 text-[11px] font-medium ${statusColor}`}>{statusLabel}</span>
        <span className="text-muted">{info.trigger_type === "manual" ? "手动触发" : "定时触发"}</span>
        <span className="text-muted">·</span>
        <span className="text-muted">{formatDate(info.started_at)}</span>
        <span className="text-muted">·</span>
        <span className="text-muted">耗时 {info.duration_secs.toFixed(1)}s</span>
        {info.early_exit_reason ? (
          <span className="rounded-full bg-amber-100 px-2 py-0.5 text-[11px] font-medium text-amber-700">
            {EARLY_EXIT_LABELS[info.early_exit_reason] ?? info.early_exit_reason}
          </span>
        ) : null}
      </div>

      {info.error ? (
        <div className="rounded-xl border border-red-200 bg-red-50 px-3 py-2 text-xs text-red-700">{info.error}</div>
      ) : null}

      <div className="rounded-xl border border-border bg-surface-container/50 p-3">
        <div className="mb-2 text-xs font-semibold text-foreground">概览</div>
        <div className="grid gap-x-4 gap-y-1.5 text-xs text-muted sm:grid-cols-2 xl:grid-cols-4">
          <div>
            数据源: {info.source.type === "u2_shoutbox" ? "U2" : "RSS"} ({info.source.items_parsed} 条)
          </div>
          <div>
            管理种子: {info.sync.managed_before}（标记移除 {info.sync.missing_marked_removed}）
          </div>
          <div>
            并发: {info.concurrency.active_count}/{info.concurrency.max_concurrent}（可加 {info.concurrency.can_add}）
          </div>
          <div>
            保种: {info.seed_volume.current_gb.toFixed(2)} GB
            {info.seed_volume.limit_gb != null ? ` / ${info.seed_volume.limit_gb} GB` : ""}
          </div>
        </div>
        <div className="mt-2 flex flex-wrap gap-x-3 gap-y-1 text-xs text-muted">
          <span>检查 {info.selection.checked}</span>
          <span className="text-emerald-600">新增 {info.selection.added}</span>
          <span className="text-red-600">失败 {info.selection.failed}</span>
          <span>已存在 {info.selection.skipped_existing}</span>
          <span>预筛 {info.selection.skipped_pre_filter}</span>
          <span>二筛 {info.selection.skipped_post_filter}</span>
          <span>详情失败 {info.selection.skipped_detail_failure}</span>
          <span>空间不足 {info.selection.skipped_no_space}</span>
        </div>
      </div>

      <div className="rounded-xl border border-border bg-surface-container/50 p-3">
        <div className="mb-2 text-xs font-semibold text-foreground">下载器</div>
        <div className="text-xs text-muted">
          <span className="font-medium text-foreground">候选: </span>
          {info.downloaders.candidates.map((d) => `${d.name} (${d.free_space_gb.toFixed(1)}GB)`).join(", ") || "无"}
        </div>
        {info.downloaders.skipped.length > 0 ? (
          <div className="mt-1 text-xs text-muted">
            <span className="font-medium text-foreground">排除: </span>
            {info.downloaders.skipped
              .map((d) => `${d.name}(${SKIP_REASON_LABELS[d.reason] ?? d.reason})`)
              .join(", ")}
          </div>
        ) : null}
      </div>

      {addedCap.length > 0 ? (
        <div className="rounded-xl border border-border bg-surface-container/50 p-3">
          <div className="mb-2 text-xs font-semibold text-foreground">新增种子 ({info.added_torrents.length})</div>
          <div className="space-y-1">
            {addedCap.map((t, i) => (
              <div key={i} className="flex flex-wrap items-center gap-2 text-xs text-muted">
                <span className="max-w-[360px] truncate text-foreground/80" title={t.title}>
                  {t.title}
                </span>
                {t.size_bytes != null ? <span className="text-muted/70">{formatBytes(t.size_bytes)}</span> : null}
                <span>→ {t.downloader_name}</span>
                {t.is_free ? <span className="text-emerald-600">免费</span> : null}
                {t.is_hr ? <span className="text-amber-600">H&amp;R</span> : null}
              </div>
            ))}
            {info.added_torrents.length > addedCap.length ? (
              <div className="text-xs text-muted">+{info.added_torrents.length - addedCap.length} 更多…</div>
            ) : null}
          </div>
        </div>
      ) : null}

      {failedCap.length > 0 ? (
        <div className="rounded-xl border border-border bg-surface-container/50 p-3">
          <div className="mb-2 text-xs font-semibold text-foreground">失败明细 ({info.failed_torrents.length})</div>
          <div className="space-y-1">
            {failedCap.map((t, i) => (
              <div key={i} className="flex flex-wrap items-center gap-2 text-xs text-muted">
                <span className="max-w-[340px] truncate text-foreground/80" title={t.title}>
                  {t.title}
                </span>
                <span className="text-red-600">{FAIL_REASON_LABELS[t.reason] ?? t.reason}</span>
                {t.detail ? <span className="truncate text-muted/70" title={t.detail}>{t.detail}</span> : null}
              </div>
            ))}
            {info.failed_torrents.length > failedCap.length ? (
              <div className="text-xs text-muted">+{info.failed_torrents.length - failedCap.length} 更多…</div>
            ) : null}
          </div>
        </div>
      ) : null}

      <details className="text-xs text-muted/60">
        <summary className="cursor-pointer select-none">原始 JSON</summary>
        <pre className="mt-1 max-h-64 overflow-auto rounded-xl bg-surface-container/70 p-2 text-[10px]">
          {JSON.stringify(info, null, 2)}
        </pre>
      </details>
    </div>
  );
}

const emptyForm: BrushTaskRequest = {
  name: "",
  cron_expression: "",
  site_id: null,
  downloader_ids: [],
  tag: "",
  rss_url: "",
  seed_volume_gb: null,
  save_dir: null,
  active_time_windows: null,
  promotion: "all",
  skip_hit_and_run: false,
  max_concurrent: 100,
  download_speed_limit: null,
  upload_speed_limit: null,
  size_ranges: null,
  seeder_ranges: null,
  downloader_ranges: null,
  downloader_weights: null,
  min_free_hours: null,
  delete_mode: "or",
  delete_on_free_expiry: false,
  min_seed_time_hours: null,
  hr_min_seed_time_hours: null,
  target_ratio: null,
  max_upload_gb: null,
  download_timeout_hours: null,
  min_avg_upload_speed_kbs: null,
  max_inactive_hours: null,
  min_disk_space_gb: null,
};

function taskToForm(task: BrushTaskRecord): BrushTaskRequest {
  return {
    name: task.name,
    cron_expression: task.cron_expression,
    site_id: task.site_id,
    downloader_ids: task.downloader_ids,
    tag: task.tag,
    rss_url: task.rss_url,
    seed_volume_gb: task.seed_volume_gb,
    save_dir: task.save_dir,
    active_time_windows: task.active_time_windows,
    promotion: task.promotion,
    skip_hit_and_run: task.skip_hit_and_run,
    max_concurrent: task.max_concurrent,
    download_speed_limit: task.download_speed_limit,
    upload_speed_limit: task.upload_speed_limit,
    size_ranges: task.size_ranges,
    seeder_ranges: task.seeder_ranges,
    downloader_ranges: task.downloader_ranges,
    downloader_weights: task.downloader_weights,
    min_free_hours: task.min_free_hours,
    delete_mode: task.delete_mode,
    delete_on_free_expiry: task.delete_on_free_expiry,
    min_seed_time_hours: task.min_seed_time_hours,
    hr_min_seed_time_hours: task.hr_min_seed_time_hours,
    target_ratio: task.target_ratio,
    max_upload_gb: task.max_upload_gb,
    download_timeout_hours: task.download_timeout_hours,
    min_avg_upload_speed_kbs: task.min_avg_upload_speed_kbs,
    max_inactive_hours: task.max_inactive_hours,
    min_disk_space_gb: task.min_disk_space_gb,
  };
}

const selectClass =
  "flex h-11 w-full rounded-2xl border border-border bg-input px-4 py-2 text-sm shadow-sm focus:outline-none focus:ring-2 focus:ring-ring/30";

const checkboxClass = "h-4 w-4 rounded border border-border accent-[hsl(var(--primary))]";

export function BrushTasksPage() {
  const [tasks, setTasks] = useState<BrushTaskRecord[]>([]);
  const [sites, setSites] = useState<SiteRecord[]>([]);
  const [downloaders, setDownloaders] = useState<DownloaderRecord[]>([]);
  const [form, setForm] = useState<BrushTaskRequest>({ ...emptyForm });
  const [editingId, setEditingId] = useState<number | null>(null);
  const [formOpen, setFormOpen] = useState(false);
  const [torrentsOpen, setTorrentsOpen] = useState(false);
  const [torrentsTask, setTorrentsTask] = useState<BrushTaskRecord | null>(null);
  const [lastRunTask, setLastRunTask] = useState<BrushTaskRecord | null>(null);
  const [torrents, setTorrents] = useState<BrushTorrentRecord[]>([]);
  const [torrentsPage, setTorrentsPage] = useState(1);
  const [torrentsPageSize] = useState(20);
  const [torrentsTotal, setTorrentsTotal] = useState(0);
  const [torrentKeyword, setTorrentKeyword] = useState("");
  const [loadingTorrents, setLoadingTorrents] = useState(false);
  const [deleteConfirmId, setDeleteConfirmId] = useState<number | null>(null);
  const [submitError, setSubmitError] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [message, setMessage] = useState("");
  const [defaultPaths, setDefaultPaths] = useState<Record<number, string>>({});
  const [saveSubPath, setSaveSubPath] = useState("");
  const [downloaderWeights, setDownloaderWeights] = useState<Record<number, number>>({});

  function reload() {
    api<BrushTaskRecord[]>("/api/brush-tasks")
      .then(setTasks)
      .catch((error: Error) => setMessage(error.message || "加载刷流任务失败"));
  }

  useEffect(() => {
    reload();
    api<SiteRecord[]>("/api/sites")
      .then(setSites)
      .catch((error: Error) => setMessage(error.message || "加载站点列表失败"));
    api<DownloaderRecord[]>("/api/downloaders")
      .then(setDownloaders)
      .catch((error: Error) => setMessage(error.message || "加载下载器列表失败"));
  }, []);

  useEffect(() => {
    for (const dlId of form.downloader_ids) {
      if (defaultPaths[dlId] === undefined) {
        api<{ path: string }>(`/api/downloaders/${dlId}/default-path`)
          .then((res) => setDefaultPaths((prev) => ({ ...prev, [dlId]: res.path })))
          .catch(() => setDefaultPaths((prev) => ({ ...prev, [dlId]: "" })));
      }
    }
  }, [form.downloader_ids, defaultPaths]);

  function openAdd() {
    const initialIds = downloaders[0] ? [downloaders[0].id] : [];
    setForm({
      ...emptyForm,
      site_id: sites[0]?.id ?? null,
      downloader_ids: initialIds,
      downloader_weights: buildDownloaderWeightsJson(initialIds, {}),
    });
    const weights: Record<number, number> = {};
    for (const id of initialIds) weights[id] = 1;
    setDownloaderWeights(weights);
    setSaveSubPath("");
    setEditingId(null);
    setSubmitError("");
    setFormOpen(true);
  }

  function openEdit(task: BrushTaskRecord) {
    setForm(taskToForm(task));
    setSaveSubPath(extractSubPath(task.save_dir, task.downloader_ids, defaultPaths));
    setDownloaderWeights(parseDownloaderWeights(task.downloader_weights));
    setEditingId(task.id);
    setSubmitError("");
    setFormOpen(true);
  }

  function closeForm() {
    setFormOpen(false);
    setEditingId(null);
    setSubmitError("");
  }

  function toggleDownloader(id: number) {
    const isCurrentlySelected = form.downloader_ids.includes(id);
    const newIds = isCurrentlySelected
      ? form.downloader_ids.filter((d) => d !== id)
      : [...form.downloader_ids, id];
    const newWeights = { ...downloaderWeights };
    if (isCurrentlySelected) {
      delete newWeights[id];
    } else {
      newWeights[id] = 1;
    }
    setDownloaderWeights(newWeights);
    setForm((prev) => ({
      ...prev,
      downloader_ids: newIds,
      downloader_weights: buildDownloaderWeightsJson(newIds, newWeights),
    }));
  }

  function updateDownloaderWeight(id: number, weight: number) {
    const newWeights = { ...downloaderWeights, [id]: weight };
    setDownloaderWeights(newWeights);
    setForm((prev) => ({
      ...prev,
      downloader_weights: buildDownloaderWeightsJson(prev.downloader_ids, newWeights),
    }));
  }

  async function handleSubmit() {
    if (form.site_id == null) {
      setSubmitError("必须选择一个具体站点");
      return;
    }
    setSubmitting(true);
    setSubmitError("");
    try {
      if (editingId !== null) {
        await api(`/api/brush-tasks/${editingId}`, { method: "PUT", body: JSON.stringify(form) });
      } else {
        await api("/api/brush-tasks", { method: "POST", body: JSON.stringify(form) });
      }
      closeForm();
      setMessage(editingId !== null ? "刷流任务已更新" : "刷流任务已创建");
      reload();
    } catch (error) {
      setSubmitError((error as Error).message || "提交失败");
    } finally {
      setSubmitting(false);
    }
  }

  async function handleDelete(id: number) {
    try {
      await api(`/api/brush-tasks/${id}`, { method: "DELETE" });
      setDeleteConfirmId(null);
      setMessage("刷流任务已删除");
      reload();
    } catch (error) {
      setMessage((error as Error).message || "删除刷流任务失败");
    }
  }

  async function handleStart(id: number) {
    try {
      await api(`/api/brush-tasks/${id}/start`, { method: "POST" });
      setMessage("刷流任务已启动");
      reload();
    } catch (error) {
      setMessage((error as Error).message || "启动刷流任务失败");
    }
  }

  async function handleStop(id: number) {
    try {
      await api(`/api/brush-tasks/${id}/stop`, { method: "POST" });
      setMessage("刷流任务已停止");
      reload();
    } catch (error) {
      setMessage((error as Error).message || "停止刷流任务失败");
    }
  }

  async function handleRunOnce(id: number) {
    try {
      await api(`/api/brush-tasks/${id}/run`, { method: "POST" });
      setMessage("刷流任务已触发执行");
      reload();
    } catch (error) {
      setMessage((error as Error).message || "触发刷流任务失败");
    }
  }

  function openTorrents(task: BrushTaskRecord) {
    setTorrentsTask(task);
    setTorrentsPage(1);
    setTorrentKeyword("");
    setTorrentsOpen(true);
  }

  function closeTorrents() {
    setTorrentsOpen(false);
    setTorrentsTask(null);
    setTorrents([]);
    setTorrentsPage(1);
    setTorrentsTotal(0);
    setTorrentKeyword("");
  }

  function setField<K extends keyof BrushTaskRequest>(key: K, value: BrushTaskRequest[K]) {
    setForm((prev) => ({ ...prev, [key]: value }));
  }

  function numOrNull(value: string): number | null {
    const n = Number(value);
    return value === "" || Number.isNaN(n) ? null : n;
  }

  useEffect(() => {
    if (!torrentsOpen || !torrentsTask) {
      return;
    }

    setLoadingTorrents(true);
    const params = new URLSearchParams({
      page: String(torrentsPage),
      page_size: String(torrentsPageSize),
    });
    const keyword = torrentKeyword.trim();
    if (keyword) {
      params.set("keyword", keyword);
    }

    api<BrushTaskTorrentsResponse>(`/api/brush-tasks/${torrentsTask.id}/torrents?${params.toString()}`)
      .then((data) => {
        setTorrents(data.records);
        setTorrentsTotal(data.total_records);
      })
      .catch((error: Error) => setMessage(error.message || "加载种子列表失败"))
      .finally(() => setLoadingTorrents(false));
  }, [torrentKeyword, torrentsOpen, torrentsPage, torrentsPageSize, torrentsTask]);

  const torrentsTotalPages = Math.max(1, Math.ceil(torrentsTotal / torrentsPageSize));

  return (
    <>
      <div className="grid gap-4 xl:gap-6">
        <Card>
          <CardHeader>
            {message ? (
              <div className="rounded-2xl border border-border bg-surface-container/70 px-4 py-3 text-sm">
                <div className="flex items-start justify-between gap-3">
                  <span>{message}</span>
                  <button type="button" className="text-muted hover:text-foreground" onClick={() => setMessage("")}>
                    关闭
                  </button>
                </div>
              </div>
            ) : null}
            <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
              <div>
                <CardTitle>刷流任务管理</CardTitle>
                <CardDescription>管理 PT 刷流任务，配置选种与删种规则，查看种子状态。</CardDescription>
              </div>
              <Button className="w-full sm:w-auto" onClick={openAdd}>
                <Plus className="mr-2 h-4 w-4" />
                添加任务
              </Button>
            </div>
          </CardHeader>
          <CardContent>
            {tasks.length === 0 ? (
              <div className="py-12 text-center text-sm text-muted">暂无刷流任务，点击上方按钮添加。</div>
            ) : (
              <div className="grid gap-3">
                {tasks.map((task) => (
                  <div key={task.id} className="rounded-xl border border-border bg-surface-container/70 p-3.5 shadow-sm">
                    <div className="flex flex-wrap items-start justify-between gap-3">
                      <div className="min-w-0 flex-1">
                        <div className="flex items-center gap-2">
                          <span className="font-semibold text-sm truncate">{task.name}</span>
                          <span
                            className={`shrink-0 rounded-full px-2.5 py-0.5 text-[11px] font-medium ${
                              task.enabled ? "bg-emerald-100 text-emerald-700" : "bg-amber-100 text-amber-700"
                            }`}
                          >
                            {task.enabled ? "运行中" : "已停止"}
                          </span>
                        </div>
                        <div className="mt-0.5 text-[11px] text-muted">#{task.id}</div>
                      </div>
                      <div className="flex flex-wrap gap-2">
                        {task.enabled ? (
                          <Button variant="outline" className="h-7 text-[11px] px-2.5" onClick={() => void handleStop(task.id)}>
                            <Pause className="mr-1.5 h-3.5 w-3.5" />
                            停止
                          </Button>
                        ) : (
                          <Button variant="secondary" className="h-7 text-[11px] px-2.5" onClick={() => void handleStart(task.id)}>
                            <Play className="mr-1.5 h-3.5 w-3.5" />
                            启动
                          </Button>
                        )}
                        <Button variant="outline" className="h-7 text-[11px] px-2.5" onClick={() => void handleRunOnce(task.id)}>
                          <Zap className="mr-1.5 h-3.5 w-3.5" />
                          立即执行一次
                        </Button>
                        <Button variant="outline" className="h-7 text-[11px] px-2.5" onClick={() => openEdit(task)}>
                          <Edit className="mr-1.5 h-3.5 w-3.5" />
                          编辑
                        </Button>
                        <Button variant="outline" className="h-7 text-[11px] px-2.5" onClick={() => openTorrents(task)}>
                          <Eye className="mr-1.5 h-3.5 w-3.5" />
                          查看种子
                        </Button>
                        <Button
                          variant="outline"
                          className="h-7 text-[11px] px-2.5"
                          onClick={() => setLastRunTask(task)}
                        >
                          <Activity className="mr-1.5 h-3.5 w-3.5" />
                          最近执行
                        </Button>
                        <Button variant="destructive" className="h-7 text-[11px] px-2.5" onClick={() => setDeleteConfirmId(task.id)}>
                          <Trash2 className="mr-1.5 h-3.5 w-3.5" />
                          删除
                        </Button>
                      </div>
                    </div>

                    <div className="mt-2.5 grid gap-1.5 text-[11px] text-muted sm:grid-cols-2 xl:grid-cols-4">
                      <div className="truncate">
                        <span className="font-medium text-foreground">Cron: </span>
                        {task.cron_expression}
                      </div>
                      <div className="truncate">
                        <span className="font-medium text-foreground">站点: </span>
                        {sites.find((site) => site.id === task.site_id)?.name ?? (task.site_id ? `#${task.site_id}` : "未绑定")}
                      </div>
                      <div className="truncate">
                        <span className="font-medium text-foreground">下载器: </span>
                        {task.downloader_ids.map((id) => downloaders.find((d) => d.id === id)?.name ?? `#${id}`).join(", ") || "未绑定"}
                      </div>
                      <div className="truncate">
                        <span className="font-medium text-foreground">标签: </span>
                        {task.tag}
                      </div>
                      <div className="sm:col-span-2 truncate">
                        <span className="font-medium text-foreground">RSS: </span>
                        {task.rss_url}
                      </div>
                    </div>

                    <div className="mt-2 text-[11px] text-muted">
                      创建: {formatDate(task.created_at)} · 更新: {formatDate(task.updated_at)}
                    </div>
                  </div>
                ))}
              </div>
            )}
          </CardContent>
        </Card>
      </div>

      {/* 添加/编辑任务对话框 */}
      <Dialog
        open={formOpen}
        onClose={closeForm}
        title={editingId !== null ? "编辑刷流任务" : "添加刷流任务"}
        description="配置任务的选种规则、删种策略和其他参数。"
        escMode="double"
      >
        <div className="space-y-6 p-4 sm:p-6">
          {submitError ? (
            <div className="rounded-2xl border border-red-200 bg-red-50 px-4 py-3 text-sm text-red-700">
              {submitError}
            </div>
          ) : null}

          {/* 基本设置 */}
          <section>
            <h4 className="mb-3 text-sm font-semibold">基本设置</h4>
            <div className="grid gap-4 sm:grid-cols-2">
              <div className="space-y-2">
                <Label>任务名称</Label>
                <Input
                  placeholder="我的刷流任务"
                  value={form.name}
                  onChange={(e) => setField("name", e.target.value)}
                />
              </div>
              <div className="space-y-2">
                <Label>Cron 表达式</Label>
                <Input
                  placeholder="0 */5 * * * *"
                  value={form.cron_expression}
                  onChange={(e) => setField("cron_expression", e.target.value)}
                />
                <p className="text-xs text-muted">例如：0 */5 * * * *（每 5 分钟）</p>
              </div>
              <div className="space-y-2">
                <Label>站点</Label>
                <Select
                  value={form.site_id ? String(form.site_id) : ""}
                  onChange={(val) => setField("site_id", val === "" ? null : Number(val))}
                  options={
                    sites.length === 0
                      ? [{ value: "", label: "请先添加站点" }]
                      : sites.map((site) => ({ value: String(site.id), label: `${site.name} (${site.site_type})` }))
                  }
                />
              </div>
              <div className="space-y-2 sm:col-span-2">
                <Label>下载器（可多选，加权随机分配）</Label>
                <div className="flex flex-wrap gap-2">
                  {downloaders.length === 0 && (
                    <span className="text-xs text-muted">无可用下载器</span>
                  )}
                  {downloaders.map((d) => {
                    const selected = form.downloader_ids.includes(d.id);
                    return (
                      <button
                        key={d.id}
                        type="button"
                        onClick={() => toggleDownloader(d.id)}
                        className={`rounded-full px-3 py-1.5 text-xs font-medium transition-colors ${
                          selected
                            ? "bg-primary text-primary-foreground shadow-glow"
                            : "bg-surface-container text-muted hover:bg-accent"
                        }`}
                      >
                        {d.name}
                      </button>
                    );
                  })}
                </div>
                {form.downloader_ids.length > 0 && (
                  <div className="mt-2 space-y-1.5">
                    <p className="text-xs text-muted">优先级（数值越大越优先被选中）</p>
                    {form.downloader_ids.map((dlId) => {
                      const dl = downloaders.find((d) => d.id === dlId);
                      return (
                        <div key={dlId} className="flex items-center gap-2">
                          <span className="text-xs text-muted w-28 truncate">{dl?.name ?? `#${dlId}`}</span>
                          <Input
                            type="number"
                            min="1"
                            className="h-8 w-24 text-xs"
                            value={downloaderWeights[dlId] ?? 1}
                            onChange={(e) => updateDownloaderWeight(dlId, Number(e.target.value) || 1)}
                          />
                        </div>
                      );
                    })}
                  </div>
                )}
              </div>
              <div className="space-y-2">
                <Label>标签</Label>
                <Input
                  placeholder="brush"
                  value={form.tag}
                  onChange={(e) => setField("tag", e.target.value)}
                />
              </div>
              <div className="space-y-2 sm:col-span-2">
                <Label>RSS 地址</Label>
                <Input
                  placeholder="https://example.com/rss"
                  value={form.rss_url}
                  onChange={(e) => setField("rss_url", e.target.value)}
                />
              </div>
            </div>
          </section>

          {/* 可选设置 */}
          <section>
            <h4 className="mb-3 text-sm font-semibold">可选设置</h4>
            <div className="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
              <div className="space-y-2">
                <Label>做种体积上限 (GB)</Label>
                <Input
                  type="number"
                  placeholder="不限"
                  value={form.seed_volume_gb ?? ""}
                  onChange={(e) => setField("seed_volume_gb", numOrNull(e.target.value))}
                />
              </div>
              <div className="space-y-2">
                <Label>最低剩余磁盘空间 (GB)</Label>
                <Input
                  type="number"
                  step="0.1"
                  placeholder="不限"
                  value={form.min_disk_space_gb ?? ""}
                  onChange={(e) => setField("min_disk_space_gb", numOrNull(e.target.value))}
                />
                <p className="text-xs text-muted">低于此值的下载器不参与选种</p>
              </div>
              <div className="space-y-2 sm:col-span-2">
                <Label>保存子路径（留空使用各下载器默认目录）</Label>
                <Input
                  placeholder="如 brush/task1"
                  value={saveSubPath}
                  onChange={(e) => {
                    const sub = e.target.value;
                    setSaveSubPath(sub);
                    const map: Record<string, string> = {};
                    for (const dlId of form.downloader_ids) {
                      const base = defaultPaths[dlId] ?? "";
                      map[String(dlId)] = joinPath(base, sub.trim());
                    }
                    setField("save_dir", Object.keys(map).length > 0 ? JSON.stringify(map) : null);
                  }}
                />
                {form.downloader_ids.length > 0 && (
                  <div className="space-y-1">
                    {form.downloader_ids.map((dlId) => {
                      const dl = downloaders.find((d) => d.id === dlId);
                      const base = defaultPaths[dlId] ?? "";
                      const finalPath = joinPath(base, saveSubPath.trim());
                      return (
                        <p key={dlId} className="text-xs text-muted">
                          {dl?.name ?? `#${dlId}`}: <code className="text-foreground/70">{finalPath || "（默认目录）"}</code>
                        </p>
                      );
                    })}
                  </div>
                )}
              </div>
              <div className="space-y-2">
                <Label>活动时间窗口</Label>
                <Input
                  placeholder='["00:00-09:00"]'
                  value={form.active_time_windows ?? ""}
                  onChange={(e) => setField("active_time_windows", e.target.value || null)}
                />
                <p className="text-xs text-muted">JSON 数组格式，例如：[&quot;00:00-09:00&quot;]</p>
              </div>
            </div>
          </section>

          {/* 选种规则 */}
          <section>
            <h4 className="mb-3 text-sm font-semibold">选种规则</h4>
            <div className="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
              <div className="space-y-2">
                <Label>促销类型</Label>
                <Select
                  value={form.promotion ?? "all"}
                  onChange={(val) => {
                    setField("promotion", val);
                    if (val !== "free") {
                      setField("min_free_hours", null);
                    }
                  }}
                  options={[
                    { value: "all", label: "全部" },
                    { value: "free", label: "免费" },
                    { value: "normal", label: "普通" },
                  ]}
                />
              </div>
              <div className="space-y-2">
                <Label>最大并发数</Label>
                <Input
                  type="number"
                  min={1}
                  value={form.max_concurrent ?? 100}
                  onChange={(e) => setField("max_concurrent", Number(e.target.value) || 100)}
                />
              </div>
              <div className="space-y-2">
                <Label>下载限速 (KB/s)</Label>
                <Input
                  type="number"
                  placeholder="不限"
                  value={form.download_speed_limit ?? ""}
                  onChange={(e) => setField("download_speed_limit", numOrNull(e.target.value))}
                />
              </div>
              <div className="space-y-2">
                <Label>上传限速 (KB/s)</Label>
                <Input
                  type="number"
                  placeholder="不限"
                  value={form.upload_speed_limit ?? ""}
                  onChange={(e) => setField("upload_speed_limit", numOrNull(e.target.value))}
                />
              </div>
              <div className="space-y-2">
                <Label>体积范围</Label>
                <Input
                  placeholder='["0-10","10-50"]'
                  value={form.size_ranges ?? ""}
                  onChange={(e) => setField("size_ranges", e.target.value || null)}
                />
                <p className="text-xs text-muted">JSON 数组，单位 GB</p>
              </div>
              <div className="space-y-2">
                <Label>做种人数范围</Label>
                <Input
                  placeholder='["1-100"]'
                  value={form.seeder_ranges ?? ""}
                  onChange={(e) => setField("seeder_ranges", e.target.value || null)}
                />
                <p className="text-xs text-muted">JSON 数组</p>
              </div>
              <div className="space-y-2">
                <Label>下载人数范围</Label>
                <Input
                  placeholder='["0-10"]'
                  value={form.downloader_ranges ?? ""}
                  onChange={(e) => setField("downloader_ranges", e.target.value || null)}
                />
                <p className="text-xs text-muted">JSON 数组，默认不校验</p>
              </div>
              <div className="space-y-2">
                <Label>最少 free 时长 (小时)</Label>
                <Input
                  type="number"
                  placeholder="不限"
                  disabled={(form.promotion ?? "all") !== "free"}
                  value={(form.promotion ?? "all") === "free" ? (form.min_free_hours ?? "") : ""}
                  onChange={(e) => setField("min_free_hours", numOrNull(e.target.value))}
                />
                <p className="text-xs text-muted">仅免费种可设置，表示剩余 free 时长至少多少小时。</p>
              </div>
              <div className="flex items-end pb-2">
                <label className="flex items-center gap-3 text-sm text-muted">
                  <input
                    type="checkbox"
                    className={checkboxClass}
                    checked={form.skip_hit_and_run ?? false}
                    onChange={(e) => setField("skip_hit_and_run", e.target.checked)}
                  />
                  跳过 Hit and Run
                </label>
              </div>
            </div>
          </section>

          {/* 删种规则 */}
          <section>
            <h4 className="mb-3 text-sm font-semibold">删种规则</h4>
            <div className="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
              <div className="space-y-2">
                <Label>删除模式</Label>
                <Select
                  value={form.delete_mode ?? "or"}
                  onChange={(val) => setField("delete_mode", val)}
                  options={[
                    { value: "or", label: "或（满足任一条件即删除）" },
                    { value: "and", label: "与（满足所有条件才删除）" },
                  ]}
                />
              </div>
              <div className="space-y-2">
                <Label>最小做种时间 (小时)</Label>
                <Input
                  type="number"
                  step="0.1"
                  placeholder="不限"
                  value={form.min_seed_time_hours ?? ""}
                  onChange={(e) => setField("min_seed_time_hours", numOrNull(e.target.value))}
                />
                <p className="text-xs text-muted">{formatHoursLabel(form.min_seed_time_hours)}</p>
              </div>
              <div className="space-y-2">
                <Label>HR 最小做种时间 (小时)</Label>
                <Input
                  type="number"
                  step="0.1"
                  placeholder="不限"
                  value={form.hr_min_seed_time_hours ?? ""}
                  onChange={(e) => setField("hr_min_seed_time_hours", numOrNull(e.target.value))}
                />
                <p className="text-xs text-muted">{formatHoursLabel(form.hr_min_seed_time_hours)}</p>
              </div>
              <div className="flex items-end pb-2">
                <label className="flex items-center gap-3 text-sm text-muted">
                  <input
                    type="checkbox"
                    className={checkboxClass}
                    checked={form.delete_on_free_expiry ?? false}
                    onChange={(e) => setField("delete_on_free_expiry", e.target.checked)}
                  />
                  free到期删除种子
                </label>
              </div>
              <div className="space-y-2">
                <Label>目标分享率</Label>
                <Input
                  type="number"
                  step="0.1"
                  placeholder="不限"
                  value={form.target_ratio ?? ""}
                  onChange={(e) => setField("target_ratio", numOrNull(e.target.value))}
                />
              </div>
              <div className="space-y-2">
                <Label>最大上传量 (GB)</Label>
                <Input
                  type="number"
                  placeholder="不限"
                  value={form.max_upload_gb ?? ""}
                  onChange={(e) => setField("max_upload_gb", numOrNull(e.target.value))}
                />
              </div>
              <div className="space-y-2">
                <Label>下载超时 (小时)</Label>
                <Input
                  type="number"
                  step="0.1"
                  placeholder="不限"
                  value={form.download_timeout_hours ?? ""}
                  onChange={(e) => setField("download_timeout_hours", numOrNull(e.target.value))}
                />
                <p className="text-xs text-muted">{formatHoursLabel(form.download_timeout_hours)}</p>
              </div>
              <div className="space-y-2">
                <Label>最低平均上传速度 (KB/s)</Label>
                <Input
                  type="number"
                  placeholder="不限"
                  value={form.min_avg_upload_speed_kbs ?? ""}
                  onChange={(e) => setField("min_avg_upload_speed_kbs", numOrNull(e.target.value))}
                />
              </div>
              <div className="space-y-2">
                <Label>最大不活跃时间 (小时)</Label>
                <Input
                  type="number"
                  step="0.1"
                  placeholder="不限"
                  value={form.max_inactive_hours ?? ""}
                  onChange={(e) => setField("max_inactive_hours", numOrNull(e.target.value))}
                />
                <p className="text-xs text-muted">{formatHoursLabel(form.max_inactive_hours)}</p>
              </div>
            </div>
          </section>

          {/* 操作按钮 */}
          <div className="flex gap-3 border-t border-border pt-4">
            <Button disabled={submitting} onClick={() => void handleSubmit()}>
              {submitting ? "提交中..." : editingId !== null ? "保存修改" : "创建任务"}
            </Button>
            <Button variant="outline" disabled={submitting} onClick={closeForm}>
              取消
            </Button>
          </div>
        </div>
      </Dialog>

      {/* 查看种子对话框 */}
      <Dialog
        open={torrentsOpen}
        onClose={closeTorrents}
        title={torrentsTask ? `${torrentsTask.name} 的种子列表` : "种子列表"}
        description="支持分页、关键字检索，并优先展示未移除种子。"
      >
        <div className="space-y-4 p-4 sm:p-6">
          <div className="flex flex-col gap-3 rounded-3xl border border-border/70 bg-surface-container/60 p-4 sm:flex-row sm:items-center sm:justify-between">
            <div className="flex items-center gap-3">
              <div className="rounded-full border border-border bg-card px-3 py-1 text-xs font-medium text-muted">
                第 {torrentsPage} / {torrentsTotalPages} 页
              </div>
              <div className="rounded-full border border-emerald-200 bg-emerald-50 px-3 py-1 text-xs font-medium text-emerald-700">
                共 {torrentsTotal} 条
              </div>
            </div>
            <div className="relative w-full sm:max-w-sm">
              <Search className="pointer-events-none absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted" />
              <Input
                className="h-11 rounded-2xl border-border/70 bg-card pl-9 shadow-sm"
                placeholder="搜索名称或种子ID"
                value={torrentKeyword}
                onChange={(e) => {
                  setTorrentKeyword(e.target.value);
                  setTorrentsPage(1);
                }}
              />
            </div>
          </div>

          {loadingTorrents ? (
            <div className="text-sm text-muted">加载中...</div>
          ) : torrents.length === 0 ? (
            <div className="py-8 text-center text-sm text-muted">暂无种子记录。</div>
          ) : (
            <div className="grid gap-3">
              <Table className="table-fixed">
                <TableHeader className="sticky top-0 z-10 backdrop-blur">
                  <TableRow>
                    <TableHead className="w-40">名称</TableHead>
                    <TableHead className="w-28">种子ID</TableHead>
                    <TableHead className="w-20">大小</TableHead>
                    <TableHead className="w-20">上传量</TableHead>
                    <TableHead className="w-20">下载量</TableHead>
                    <TableHead className="w-20">状态</TableHead>
                    <TableHead className="w-14">HR</TableHead>
                    <TableHead className="w-28">添加时间</TableHead>
                    <TableHead className="w-28">移除时间</TableHead>
                    <TableHead className="w-20">下载耗时</TableHead>
                    <TableHead className="w-24">平均上传速度</TableHead>
                    <TableHead className="w-14">分享率</TableHead>
                    <TableHead className="w-24">移除原因</TableHead>
                    <TableHead className="w-28">信息Hash</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {torrents.map((t) => (
                    <TableRow key={t.id}>
                      <TableCell className="p-4 text-xs">
                        <div className="truncate font-medium text-foreground" title={t.torrent_name}>
                          {t.torrent_name}
                        </div>
                      </TableCell>
                      <TableCell className="p-4 text-xs font-mono">
                        {t.torrent_id ? (
                          <a
                            href={t.torrent_link ?? "#"}
                            target="_blank"
                            rel="noopener noreferrer"
                            title={t.torrent_id}
                            className={cn(
                              "block truncate font-medium underline-offset-4 hover:underline",
                              t.torrent_link ? "text-blue-500 hover:text-blue-700" : "pointer-events-none text-muted",
                            )}
                          >
                            {t.torrent_id}
                          </a>
                        ) : (
                          <span className="text-muted">-</span>
                        )}
                      </TableCell>
                      <TableCell className="truncate p-4 text-xs text-muted" title={t.size_bytes != null ? formatBytes(t.size_bytes) : "-"}>
                        {t.size_bytes != null ? formatBytes(t.size_bytes) : "-"}
                      </TableCell>
                      <TableCell className="truncate p-4 text-xs text-muted" title={formatBytes(t.uploaded_bytes)}>
                        {formatBytes(t.uploaded_bytes)}
                      </TableCell>
                      <TableCell className="truncate p-4 text-xs text-muted" title={formatBytes(t.downloaded_bytes)}>
                        {formatBytes(t.downloaded_bytes)}
                      </TableCell>
                      <TableCell className="p-4 text-xs">
                        <span
                          className={`rounded-full px-3 py-1 text-xs font-medium ${
                            t.status === "seeding"
                              ? "bg-emerald-100 text-emerald-700"
                              : t.status === "downloading"
                                ? "bg-sky-100 text-sky-700"
                                : t.status === "removed"
                                  ? "bg-red-100 text-red-700"
                                  : "bg-violet-100 text-violet-700"
                          }`}
                        >
                          {t.status}
                        </span>
                      </TableCell>
                      <TableCell className="p-4 text-xs">
                        {t.is_hr ? (
                          <span className="rounded-full border border-amber-200 bg-amber-50 px-3 py-1 text-xs font-medium text-amber-700">
                            HR
                          </span>
                        ) : (
                          <span className="text-xs text-muted">否</span>
                        )}
                      </TableCell>
                      <TableCell className="truncate p-4 text-xs text-muted" title={t.added_at}>
                        {formatDate(t.added_at)}
                      </TableCell>
                      <TableCell className="truncate p-4 text-xs text-muted" title={t.removed_at ?? "-"}>
                        {t.removed_at ? formatDate(t.removed_at) : "-"}
                      </TableCell>
                      <TableCell className="truncate p-4 text-xs text-muted" title={formatDuration(t.download_duration_secs)}>
                        {formatDuration(t.download_duration_secs)}
                      </TableCell>
                      <TableCell className="truncate p-4 text-xs text-muted" title={formatSpeed(t.avg_upload_speed)}>
                        {formatSpeed(t.avg_upload_speed)}
                      </TableCell>
                      <TableCell className="truncate p-4 text-xs font-semibold text-foreground" title={t.ratio.toFixed(2)}>
                        {t.ratio.toFixed(2)}
                      </TableCell>
                      <TableCell className="p-4 text-xs text-muted">
                        <div className="truncate" title={t.remove_reason ?? "-"}>
                          {t.remove_reason ?? "-"}
                        </div>
                      </TableCell>
                      <TableCell className="p-4 text-xs font-mono text-muted">
                        <div className="truncate" title={t.torrent_hash || "-"}>
                          {t.torrent_hash || "-"}
                        </div>
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </div>
          )}

          <div className="flex flex-col gap-3 rounded-3xl border border-border/70 bg-surface-container/50 p-4 sm:flex-row sm:items-center sm:justify-between">
            <div className="text-sm text-muted">
              第 {torrentsPage} / {torrentsTotalPages} 页，共 {torrentsTotal} 条
            </div>
            <div className="flex gap-2">
              <Button variant="outline" disabled={torrentsPage <= 1} onClick={() => setTorrentsPage((prev) => Math.max(1, prev - 1))}>
                <ChevronLeft className="mr-2 h-4 w-4" />
                上一页
              </Button>
              <Button
                variant="outline"
                disabled={torrentsPage >= torrentsTotalPages}
                onClick={() => setTorrentsPage((prev) => Math.min(torrentsTotalPages, prev + 1))}
              >
                下一页
                <ChevronRight className="ml-2 h-4 w-4" />
              </Button>
            </div>
          </div>
        </div>
      </Dialog>

      {/* 删除确认对话框 */}
      <Dialog
        open={deleteConfirmId !== null}
        onClose={() => setDeleteConfirmId(null)}
        title="确认删除"
        description="删除后任务配置和相关种子记录将无法恢复。"
      >
        <div className="space-y-4 p-4 sm:p-6">
          <p className="text-sm text-muted">确定要删除该刷流任务吗？此操作不可撤销。</p>
          <div className="flex gap-3">
            <Button variant="destructive" onClick={() => deleteConfirmId !== null && void handleDelete(deleteConfirmId)}>
              <Trash2 className="mr-2 h-4 w-4" />
              确认删除
            </Button>
            <Button variant="outline" onClick={() => setDeleteConfirmId(null)}>
              取消
            </Button>
          </div>
        </div>
      </Dialog>

      {/* 最近执行信息对话框 */}
      <Dialog
        open={lastRunTask !== null}
        onClose={() => setLastRunTask(null)}
        title={lastRunTask ? `${lastRunTask.name} · 最近执行` : "最近执行"}
        description="展示该刷流任务上一次执行采集的详细信息。"
        escMode="double"
        panelClassName="max-w-3xl"
      >
        <div className="p-4 sm:p-6">
          {lastRunTask ? (
            (() => {
              const lastRun = parseLastRunInfo(lastRunTask.last_run_info);
              return lastRun ? (
                <LastRunPanel info={lastRun} />
              ) : (
                <div className="py-8 text-center text-sm text-muted">暂无执行记录。</div>
              );
            })()
          ) : null}
        </div>
      </Dialog>
    </>
  );
}
