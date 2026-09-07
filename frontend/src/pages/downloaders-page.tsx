import { useEffect, useRef, useState } from "react";
import {
  ArrowLeft,
  ChevronDown,
  ChevronRight,
  Clock3,
  Copy,
  Edit,
  Eye,
  FolderTree,
  HardDrive,
  KeyRound,
  Link2,
  LoaderCircle,
  Search,
  Plus,
  RefreshCw,
  Server,
  ScanSearch,
  TestTubeDiagonal,
  Trash2,
  UserRound,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Dialog } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select } from "@/components/ui/select";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { api } from "@/lib/api";
import { formatDate } from "@/lib/format";
import type { DownloaderRecord, DownloaderSpaceStats, DownloaderTestResult, TransferableTorrent } from "@/types";

function formatBytes(bytes: number): string {
  if (bytes === 0) return "0 B";
  const k = 1024;
  const sizes = ["B", "KB", "MB", "GB", "TB", "PB"];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return `${(bytes / Math.pow(k, i)).toFixed(2)} ${sizes[i]}`;
}

type FormData = {
  name: string;
  downloader_type: string;
  url: string;
  username: string;
  password: string;
};

const emptyForm: FormData = {
  name: "",
  downloader_type: "qbittorrent",
  url: "",
  username: "",
  password: "",
};

type DirectoryGroup = {
  directory: string;
  torrents: TransferableTorrent[];
  totalSize: number;
  pendingBytes: number;
  incompleteCount: number;
  lastAddedOn: number;
};

function normalizeSavePath(value: string): string {
  const raw = value.trim().replace(/\\/g, "/");
  if (!raw) return "未设置保存目录";
  const unc = raw.startsWith("//");
  const normalized = raw.replace(/\/{2,}/g, "/").replace(/\/$/, "");
  if (!normalized) return "/";
  return unc ? `/${normalized}` : normalized;
}

function groupTorrentsBySavePath(torrents: TransferableTorrent[]): DirectoryGroup[] {
  const groups = new Map<string, DirectoryGroup>();

  for (const torrent of torrents) {
    const directory = normalizeSavePath(torrent.save_path);
    const isCaseInsensitive = /^[A-Za-z]:\//.test(directory) || directory.startsWith("//");
    const key = isCaseInsensitive ? directory.toLowerCase() : directory;
    const pendingBytes = torrent.amount_left;
    const group = groups.get(key) ?? {
      directory,
      torrents: [],
      totalSize: 0,
      pendingBytes: 0,
      incompleteCount: 0,
      lastAddedOn: 0,
    };
    group.torrents.push(torrent);
    group.totalSize += Math.max(0, torrent.size);
    group.pendingBytes += pendingBytes;
    group.incompleteCount += torrent.incomplete ? 1 : 0;
    group.lastAddedOn = Math.max(group.lastAddedOn, torrent.added_on);
    groups.set(key, group);
  }

  return Array.from(groups.values()).map((group) => ({
    ...group,
    torrents: group.torrents.sort((left, right) => right.added_on - left.added_on),
  }));
}

function formatTorrentState(state: string): string {
  if (["downloading", "stalledDL", "metaDL", "forcedDL", "allocating"].includes(state)) return "下载中";
  if (["uploading", "stalledUP", "forcedUP"].includes(state)) return "做种中";
  if (["pausedDL", "stoppedDL"].includes(state)) return "已暂停下载";
  if (["pausedUP", "stoppedUP"].includes(state)) return "已暂停做种";
  if (["checkingDL", "checkingUP", "checkingResumeData"].includes(state)) return "校验中";
  if (["error", "missingFiles", "unknown"].includes(state)) return "异常";
  return state || "未知";
}

export function DownloadersPage() {
  const [downloaders, setDownloaders] = useState<DownloaderRecord[]>([]);
  const [detailId, setDetailId] = useState<number | null>(null);
  const [spaceStats, setSpaceStats] = useState<Record<number, DownloaderSpaceStats>>({});
  const [spaceStatsLoaded, setSpaceStatsLoaded] = useState(false);
  const [loading, setLoading] = useState(true);
  const [message, setMessage] = useState("");

  const [dialogOpen, setDialogOpen] = useState(false);
  const [editingId, setEditingId] = useState<number | null>(null);
  const [copySource, setCopySource] = useState<DownloaderRecord | null>(null);
  const [form, setForm] = useState<FormData>(emptyForm);
  const [existingPasswordConfigured, setExistingPasswordConfigured] = useState(false);
  const [clearPassword, setClearPassword] = useState(false);
  const [saving, setSaving] = useState(false);
  const [submitError, setSubmitError] = useState("");

  const [deleteTarget, setDeleteTarget] = useState<DownloaderRecord | null>(null);
  const [deleting, setDeleting] = useState(false);

  const [testResult, setTestResult] = useState<DownloaderTestResult | null>(null);
  const [testing, setTesting] = useState<number | null>(null);
  const [directoryGroups, setDirectoryGroups] = useState<DirectoryGroup[] | null>(null);
  const [directoryAnalysisLoading, setDirectoryAnalysisLoading] = useState(false);
  const [directoryAnalysisError, setDirectoryAnalysisError] = useState("");
  const [directoryAnalysisTime, setDirectoryAnalysisTime] = useState<Date | null>(null);
  const [expandedDirectories, setExpandedDirectories] = useState<Set<string>>(new Set());
  const [directoryQuery, setDirectoryQuery] = useState("");
  const [directorySort, setDirectorySort] = useState("size");
  const [directoryTags, setDirectoryTags] = useState<string[]>([]);
  const [directoryCategories, setDirectoryCategories] = useState<string[]>([]);
  const [filterTorrents, setFilterTorrents] = useState<TransferableTorrent[] | null>(null);
  const [filterLoading, setFilterLoading] = useState(false);
  const [filterError, setFilterError] = useState("");
  const [filterReload, setFilterReload] = useState(0);
  const directoryAnalysisController = useRef<AbortController | null>(null);

  function loadDownloaders() {
    setLoading(true);
    setSpaceStatsLoaded(false);
    api<DownloaderRecord[]>("/api/downloaders")
      .then(async (items) => {
        setDownloaders(items);
        const entries = await Promise.all(
          items.map(async (downloader) => {
            try {
              const stats = await api<DownloaderSpaceStats>(`/api/downloaders/${downloader.id}/space`);
              return [downloader.id, stats] as const;
            } catch {
              return null;
            }
          }),
        );
        const next: Record<number, DownloaderSpaceStats> = {};
        for (const entry of entries) {
          if (entry) {
            next[entry[0]] = entry[1];
          }
        }
        setSpaceStats(next);
        setSpaceStatsLoaded(true);
        setMessage("");
      })
      .catch((error: Error) => {
        setDownloaders([]);
        setSpaceStats({});
        setSpaceStatsLoaded(true);
        setMessage(error.message || "加载下载器失败");
      })
      .finally(() => setLoading(false));
  }

  useEffect(() => {
    loadDownloaders();
    return () => directoryAnalysisController.current?.abort();
  }, []);

  useEffect(() => {
    if (detailId === null) return;
    const controller = new AbortController();
    setFilterLoading(true);
    setFilterError("");
    api<TransferableTorrent[]>(`/api/downloaders/${detailId}/torrents?include_incomplete=true`, { signal: controller.signal })
      .then((torrents) => { if (!controller.signal.aborted) setFilterTorrents(torrents); })
      .catch((error: Error) => { if (!controller.signal.aborted) setFilterError(error.message || "标签和分类加载失败"); })
      .finally(() => { if (!controller.signal.aborted) setFilterLoading(false); });
    return () => controller.abort();
  }, [detailId, filterReload]);

  function openAdd() {
    setEditingId(null);
    setCopySource(null);
    setForm(emptyForm);
    setExistingPasswordConfigured(false);
    setClearPassword(false);
    setSubmitError("");
    setDialogOpen(true);
  }

  function openEdit(d: DownloaderRecord) {
    setEditingId(d.id);
    setCopySource(null);
    setForm({
      name: d.name,
      downloader_type: d.downloader_type,
      url: d.url,
      username: d.username,
      password: "",
    });
    setExistingPasswordConfigured(d.password_configured);
    setClearPassword(false);
    setSubmitError("");
    setDialogOpen(true);
  }

  function openCopy(d: DownloaderRecord) {
    setEditingId(null);
    setCopySource(d);
    setForm({
      name: `${d.name} - 副本`,
      downloader_type: d.downloader_type,
      url: d.url,
      username: d.username,
      password: "",
    });
    setExistingPasswordConfigured(false);
    setClearPassword(false);
    setSubmitError("");
    setDialogOpen(true);
  }

  function openDetail(id: number) {
    setTestResult(null);
    setMessage("");
    resetDirectoryAnalysis();
    setDetailId(id);
  }

  function resetDirectoryAnalysis() {
    directoryAnalysisController.current?.abort();
    directoryAnalysisController.current = null;
    setDirectoryAnalysisLoading(false);
    setDirectoryGroups(null);
    setDirectoryAnalysisError("");
    setDirectoryAnalysisTime(null);
    setExpandedDirectories(new Set());
    setDirectoryQuery("");
    setDirectorySort("size");
    setDirectoryTags([]);
    setDirectoryCategories([]);
    setFilterTorrents(null);
    setFilterError("");
  }

  function closeDetail() {
    resetDirectoryAnalysis();
    setDetailId(null);
    setTestResult(null);
  }

  async function analyzeSaveDirectories(id: number) {
    directoryAnalysisController.current?.abort();
    const controller = new AbortController();
    directoryAnalysisController.current = controller;
    setDirectoryAnalysisLoading(true);
    setDirectoryAnalysisError("");
    try {
      const torrents = await api<TransferableTorrent[]>(
        `/api/downloaders/${id}/torrents?include_incomplete=true`,
        { signal: controller.signal },
      );
      if (directoryAnalysisController.current !== controller) return;
      setFilterTorrents(torrents);
      const groups = groupTorrentsBySavePath(torrents.filter((torrent) => {
        const tags = torrent.tags.split(",").map((tag) => tag.trim()).filter(Boolean);
        return (directoryCategories.length === 0 || directoryCategories.includes(torrent.category))
          && (directoryTags.length === 0 || directoryTags.some((tag) => tag === "" ? tags.length === 0 : tags.includes(tag)));
      }));
      setDirectoryGroups(groups);
      setExpandedDirectories(new Set(groups[0] ? [groups[0].directory] : []));
      setDirectoryAnalysisTime(new Date());
    } catch (error) {
      if ((error as Error).name === "AbortError") return;
      setDirectoryGroups(null);
      setDirectoryAnalysisError((error as Error).message || "保存目录分析失败");
    } finally {
      if (directoryAnalysisController.current === controller) {
        directoryAnalysisController.current = null;
        setDirectoryAnalysisLoading(false);
      }
    }
  }

  function toggleDirectory(directory: string) {
    setExpandedDirectories((current) => {
      const next = new Set(current);
      if (next.has(directory)) next.delete(directory);
      else next.add(directory);
      return next;
    });
  }

  function changeDirectoryFilters(tags: string[], categories: string[]) {
    setDirectoryTags(tags);
    setDirectoryCategories(categories);
    setDirectoryGroups(null);
    setDirectoryAnalysisTime(null);
    setDirectoryAnalysisError("");
    setDirectoryQuery("");
  }

  function closeDialog() {
    setDialogOpen(false);
    setEditingId(null);
    setCopySource(null);
    setForm(emptyForm);
    setExistingPasswordConfigured(false);
    setClearPassword(false);
    setSubmitError("");
  }

  async function handleSave() {
    setSaving(true);
    setSubmitError("");
    try {
      const body = JSON.stringify(
        editingId !== null
          ? { ...form, clear_password: clearPassword }
          : { ...form, copy_from_id: copySource?.id },
      );
      if (editingId !== null) {
        await api(`/api/downloaders/${editingId}`, { method: "PUT", body });
      } else {
        await api("/api/downloaders", { method: "POST", body });
      }
      closeDialog();
      setMessage(editingId !== null ? "下载器已更新" : "下载器已创建");
      loadDownloaders();
    } catch (error) {
      setSubmitError((error as Error).message || "保存下载器失败");
    } finally {
      setSaving(false);
    }
  }

  async function handleDelete() {
    if (!deleteTarget) return;
    setDeleting(true);
    try {
      await api(`/api/downloaders/${deleteTarget.id}`, { method: "DELETE" });
      if (detailId === deleteTarget.id) {
        closeDetail();
      }
      setDeleteTarget(null);
      setMessage("下载器已删除");
      loadDownloaders();
    } catch (error) {
      setMessage((error as Error).message || "删除下载器失败");
    } finally {
      setDeleting(false);
    }
  }

  async function handleTest(id: number) {
    setTesting(id);
    setTestResult(null);
    try {
      const result = await api<DownloaderTestResult>(`/api/downloaders/${id}/test`, {
        method: "POST",
      });
      setTestResult(result);
    } catch (error) {
      setTestResult({ success: false, message: (error as Error).message || "请求失败", version: null, free_space: null });
    } finally {
      setTesting(null);
    }
  }

  const detailDownloader = downloaders.find((downloader) => downloader.id === detailId) ?? null;
  const detailSpaceStats = detailDownloader ? spaceStats[detailDownloader.id] : undefined;
  const normalizedDirectoryQuery = directoryQuery.trim().toLowerCase();
  const visibleDirectoryGroups = (directoryGroups ?? [])
    .filter(
      (group) =>
        !normalizedDirectoryQuery
        || group.directory.toLowerCase().includes(normalizedDirectoryQuery)
        || group.torrents.some(
          (torrent) =>
            torrent.name.toLowerCase().includes(normalizedDirectoryQuery)
            || torrent.hash.toLowerCase().includes(normalizedDirectoryQuery)
            || torrent.category.toLowerCase().includes(normalizedDirectoryQuery)
            || torrent.tags.toLowerCase().includes(normalizedDirectoryQuery),
        ),
    )
    .sort((left, right) => {
      if (directorySort === "pending") return right.pendingBytes - left.pendingBytes;
      if (directorySort === "count") return right.torrents.length - left.torrents.length;
      if (directorySort === "path") return left.directory.localeCompare(right.directory);
      return right.totalSize - left.totalSize;
    });
  const categoryOptions = Array.from(new Set(["", ...(filterTorrents ?? []).map((torrent) => torrent.category), ...directoryCategories]))
    .sort((left, right) => left.localeCompare(right)).map((value) => ({ value, label: value || "未分类" }));
  const tagOptions = Array.from(new Set(["", ...(filterTorrents ?? []).flatMap((torrent) => torrent.tags.split(",").map((tag) => tag.trim()).filter(Boolean)), ...directoryTags]))
    .sort((left, right) => left.localeCompare(right)).map((value) => ({ value, label: value || "无标签" }));
  const analyzedTorrentCount = directoryGroups?.reduce((total, group) => total + group.torrents.length, 0) ?? 0;
  const analyzedTotalSize = directoryGroups?.reduce((total, group) => total + group.totalSize, 0) ?? 0;
  const analyzedPendingBytes = directoryGroups?.reduce((total, group) => total + group.pendingBytes, 0) ?? 0;
  const analyzedIncompleteCount = directoryGroups?.reduce((total, group) => total + group.incompleteCount, 0) ?? 0;

  if (detailDownloader) {
    return (
      <div className="space-y-6">
        <div className="flex flex-col gap-4 sm:flex-row sm:items-center sm:justify-between">
          <div className="flex min-w-0 items-center gap-3">
            <Button
              variant="outline"
              className="h-10 w-10 shrink-0 px-0"
              onClick={closeDetail}
              aria-label="返回下载器列表"
              title="返回下载器列表"
            >
              <ArrowLeft className="h-4 w-4" />
            </Button>
            <div className="min-w-0">
              <div className="flex items-center gap-2">
                <h2 className="truncate text-xl font-semibold">{detailDownloader.name}</h2>
                <span className="shrink-0 rounded-full bg-violet-100 px-2.5 py-1 text-xs font-medium text-violet-700">
                  {detailDownloader.downloader_type}
                </span>
              </div>
              <p className="mt-1 text-sm text-muted">下载器详情 · #{detailDownloader.id}</p>
            </div>
          </div>
          <div className="flex flex-wrap gap-2">
            <Button
              variant="outline"
              disabled={testing === detailDownloader.id}
              onClick={() => handleTest(detailDownloader.id)}
            >
              <TestTubeDiagonal className="mr-2 h-4 w-4" />
              {testing === detailDownloader.id ? "测试中..." : "测试连接"}
            </Button>
            <Button variant="outline" onClick={() => openEdit(detailDownloader)}>
              <Edit className="mr-2 h-4 w-4" />
              编辑
            </Button>
            <Button variant="outline" onClick={() => openCopy(detailDownloader)}>
              <Copy className="mr-2 h-4 w-4" />
              复制
            </Button>
            <Button variant="destructive" onClick={() => setDeleteTarget(detailDownloader)}>
              <Trash2 className="mr-2 h-4 w-4" />
              删除
            </Button>
          </div>
        </div>

        {message ? (
          <div className="rounded-2xl border border-border bg-surface-container/70 px-4 py-3 text-sm">
            <div className="flex items-start justify-between gap-3">
              <span>{message}</span>
              <button type="button" className="text-muted transition-colors hover:text-foreground" onClick={() => setMessage("")}>关闭</button>
            </div>
          </div>
        ) : null}

        {testResult ? (
          <div
            className={`rounded-2xl border p-4 text-sm ${
              testResult.success
                ? "border-emerald-200 bg-emerald-50 text-emerald-800"
                : "border-red-200 bg-red-50 text-red-800"
            }`}
          >
            <div className="flex items-start justify-between gap-4">
              <div className="space-y-1">
                <div className="font-medium">{testResult.success ? "连接成功" : "连接失败"}</div>
                <div>{testResult.message}</div>
                {testResult.version ? <div>客户端版本：{testResult.version}</div> : null}
                {testResult.free_space !== null ? <div>可用空间：{formatBytes(testResult.free_space)}</div> : null}
              </div>
              <Button variant="outline" className="shrink-0" onClick={() => setTestResult(null)}>
                关闭
              </Button>
            </div>
          </div>
        ) : null}

        <div className="grid gap-4 md:grid-cols-3">
          <Card>
            <CardContent className="p-5">
              <div className="flex items-center gap-2 text-sm text-muted">
                <HardDrive className="h-4 w-4 text-primary" />
                当前空闲
              </div>
              <div className="mt-3 text-2xl font-semibold">
                {detailSpaceStats ? formatBytes(detailSpaceStats.free_space) : spaceStatsLoaded ? "获取失败" : "加载中..."}
              </div>
            </CardContent>
          </Card>
          <Card>
            <CardContent className="p-5">
              <div className="flex items-center gap-2 text-sm text-muted">
                <RefreshCw className="h-4 w-4 text-primary" />
                未完成剩余
              </div>
              <div className="mt-3 text-2xl font-semibold">
                {detailSpaceStats ? formatBytes(detailSpaceStats.pending_download_bytes) : spaceStatsLoaded ? "获取失败" : "加载中..."}
              </div>
            </CardContent>
          </Card>
          <Card>
            <CardContent className="p-5">
              <div className="flex items-center gap-2 text-sm text-muted">
                <Server className="h-4 w-4 text-primary" />
                预测可用
              </div>
              <div className="mt-3 text-2xl font-semibold">
                {detailSpaceStats ? formatBytes(detailSpaceStats.effective_free_space) : spaceStatsLoaded ? "获取失败" : "加载中..."}
              </div>
            </CardContent>
          </Card>
        </div>

        <div className="grid gap-4 lg:grid-cols-[minmax(0,1.4fr)_minmax(280px,0.6fr)]">
          <Card>
            <CardHeader>
              <CardTitle>基本信息</CardTitle>
            </CardHeader>
            <CardContent>
              <dl className="divide-y divide-border">
                <DetailRow icon={<Server className="h-4 w-4" />} label="名称" value={detailDownloader.name} />
                <DetailRow icon={<HardDrive className="h-4 w-4" />} label="类型" value={detailDownloader.downloader_type} />
                <DetailRow icon={<Link2 className="h-4 w-4" />} label="连接地址" value={detailDownloader.url} />
                <DetailRow icon={<UserRound className="h-4 w-4" />} label="用户名" value={detailDownloader.username || "未设置"} />
                <DetailRow icon={<KeyRound className="h-4 w-4" />} label="密码" value={detailDownloader.password_configured ? "已配置" : "未配置"} />
                <DetailRow icon={<Clock3 className="h-4 w-4" />} label="创建时间" value={formatDate(detailDownloader.created_at)} />
                <DetailRow icon={<Clock3 className="h-4 w-4" />} label="更新时间" value={formatDate(detailDownloader.updated_at)} />
              </dl>
            </CardContent>
          </Card>

          <Card>
            <CardHeader>
              <CardTitle>任务概况</CardTitle>
            </CardHeader>
            <CardContent className="space-y-4">
              <div className="flex items-center justify-between border-b border-border pb-4 text-sm">
                <span className="text-muted">种子总数</span>
                <span className="font-semibold">{detailSpaceStats?.torrent_count ?? "--"}</span>
              </div>
              <div className="flex items-center justify-between text-sm">
                <span className="text-muted">未完成种子</span>
                <span className="font-semibold">{detailSpaceStats?.incomplete_count ?? "--"}</span>
              </div>
            </CardContent>
          </Card>
        </div>

        <Card role="region" aria-label="保存路径占用分析">
          <CardHeader>
            <div className="flex flex-col gap-4 sm:flex-row sm:items-start sm:justify-between">
              <div>
                <CardTitle className="flex items-center gap-2">
                  <FolderTree className="h-5 w-5 text-primary" />
                  保存路径占用分析
                </CardTitle>
                <p className="mt-2 text-sm text-muted">按 qBittorrent 实际保存路径统计占用和待下载空间。</p>
              </div>

            </div>
          </CardHeader>
          <CardContent>
            <div className="mb-5 space-y-3 border-b border-border pb-5">
              <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-[minmax(0,1fr)_minmax(0,1fr)_auto] xl:items-end">
                <div className="min-w-0 space-y-2">
                  <Label htmlFor="analysis-categories">分类（多选）</Label>
                  <Select id="analysis-categories" multiple searchable value={directoryCategories}
                    onChange={(values) => changeDirectoryFilters(directoryTags, values)}
                    options={categoryOptions} placeholder="全部分类" searchPlaceholder="搜索分类"
                    disabled={filterLoading || !!filterError || filterTorrents === null || directoryAnalysisLoading}
                    aria-describedby="analysis-filter-help" />
                </div>
                <div className="min-w-0 space-y-2">
                  <Label htmlFor="analysis-tags">标签（多选）</Label>
                  <Select id="analysis-tags" multiple searchable value={directoryTags}
                    onChange={(values) => changeDirectoryFilters(values, directoryCategories)}
                    options={tagOptions} placeholder="全部标签" searchPlaceholder="搜索标签"
                    disabled={filterLoading || !!filterError || filterTorrents === null || directoryAnalysisLoading}
                    aria-describedby="analysis-filter-help" />
                </div>
                <Button className="h-11 sm:col-span-2 xl:col-span-1"
                  disabled={filterLoading || !!filterError || filterTorrents === null || directoryAnalysisLoading}
                  onClick={() => analyzeSaveDirectories(detailDownloader.id)}>
                  {directoryAnalysisLoading ? <LoaderCircle className="mr-2 size-4 animate-spin motion-reduce:animate-none" /> : <ScanSearch className="mr-2 size-4" />}
                  {directoryAnalysisLoading ? "统计中..." : directoryGroups ? "重新统计" : "开始统计"}
                </Button>
              </div>
              <div className="flex flex-wrap items-center justify-between gap-2">
                <p id="analysis-filter-help" className="text-xs leading-5 text-muted">
                  不选则统计全部；同组内匹配任一项，分类与标签同时满足。更改筛选后点击“开始统计”。
                </p>
                {directoryTags.length > 0 || directoryCategories.length > 0 ? (
                  <Button variant="outline" className="h-9 px-3 text-xs" disabled={directoryAnalysisLoading}
                    onClick={() => changeDirectoryFilters([], [])}>清空筛选</Button>
                ) : null}
              </div>
              {filterLoading ? <p role="status" className="text-xs text-muted">正在加载标签和分类...</p> : null}
              {filterError ? (
                <div role="alert" className="flex flex-wrap items-center gap-3 text-sm text-destructive">
                  <span className="min-w-0 break-words">标签和分类加载失败：{filterError}</span>
                  <Button variant="outline" onClick={() => setFilterReload((value) => value + 1)}>重试加载</Button>
                </div>
              ) : null}
            </div>
            {directoryAnalysisError ? (
              <div role="alert" className="rounded-lg border border-red-200 bg-red-50 px-4 py-3 text-sm text-red-700">
                {directoryAnalysisError}
              </div>
            ) : directoryAnalysisLoading && !directoryGroups ? (
              <div className="flex min-h-24 items-center justify-center gap-2 text-sm text-muted" aria-live="polite">
                <LoaderCircle className="h-4 w-4 animate-spin motion-reduce:animate-none" />
                正在读取种子和分析保存目录...
              </div>
            ) : directoryGroups ? (
              <div className="space-y-4">
                <div className="grid overflow-hidden rounded-lg border border-border sm:grid-cols-2 xl:grid-cols-4">
                  <AnalysisMetric label="保存路径" value={`${directoryGroups.length} 个`} />
                  <AnalysisMetric label="种子总数" value={`${analyzedTorrentCount} 个`} />
                  <AnalysisMetric label="数据总量" value={formatBytes(analyzedTotalSize)} />
                  <AnalysisMetric label="待下载" value={formatBytes(analyzedPendingBytes)} detail={`${analyzedIncompleteCount} 个未完成`} />
                </div>

                <div className="flex flex-col gap-3 lg:flex-row lg:items-center lg:justify-between">
                  <div className="relative min-w-0 flex-1 lg:max-w-xl">
                    <Search className="pointer-events-none absolute left-3.5 top-1/2 h-4 w-4 -translate-y-1/2 text-muted" />
                    <Input
                      value={directoryQuery}
                      onChange={(event) => setDirectoryQuery(event.target.value)}
                      className="pl-10"
                      placeholder="搜索路径、种子、标签或分类"
                      aria-label="搜索保存路径、种子、标签或分类"
                    />
                  </div>
                  <div className="w-full lg:w-48">
                    <Select
                      value={directorySort}
                      onChange={setDirectorySort}
                      options={[
                        { value: "size", label: "按占用空间排序" },
                        { value: "pending", label: "按待下载量排序" },
                        { value: "count", label: "按种子数量排序" },
                        { value: "path", label: "按路径名称排序" },
                      ]}
                    />
                  </div>
                </div>

                <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-muted">
                  <span>显示 {visibleDirectoryGroups.length} / {directoryGroups.length} 个路径</span>
                  {directoryAnalysisTime ? <span>实时统计于 {directoryAnalysisTime.toLocaleString()}</span> : null}
                </div>

                {directoryGroups.length === 0 ? (
                  <div className="rounded-lg border border-dashed border-border px-4 py-8 text-center text-sm text-muted">
                    {directoryTags.length > 0 || directoryCategories.length > 0 ? "所选标签和分类下暂无种子。" : "下载器中暂无种子。"}
                  </div>
                ) : visibleDirectoryGroups.length === 0 ? (
                  <div className="rounded-lg border border-dashed border-border px-4 py-8 text-center text-sm text-muted">
                    没有匹配的保存路径或种子。
                  </div>
                ) : (
                  <div className="space-y-2">
                    {visibleDirectoryGroups.map((group) => (
                      <DirectoryGroupPanel
                        key={group.directory}
                        group={group}
                        expanded={expandedDirectories.has(group.directory)}
                        onToggle={() => toggleDirectory(group.directory)}
                      />
                    ))}
                  </div>
                )}
              </div>
            ) : (
              <div className="rounded-lg border border-dashed border-border px-4 py-8 text-center text-sm text-muted">
                选择标签或分类后，点击“开始统计”查看对应保存路径的空间占用和种子明细。
              </div>
            )}
          </CardContent>
        </Card>

        {renderDialogs()}
      </div>
    );
  }

  function renderDialogs() {
    return (
      <>
        <Dialog
          open={dialogOpen}
          onClose={closeDialog}
          title={editingId !== null ? "编辑下载器" : copySource ? "复制下载器" : "添加下载器"}
          description={
            editingId !== null
              ? "修改下载器配置信息。"
              : copySource
                ? `基于「${copySource.name}」创建新下载器。`
                : "填写下载器连接信息。"
          }
          escMode="double"
        >
          <div className="space-y-4 p-4 sm:p-6">
            {submitError ? (
              <div className="rounded-2xl border border-red-200 bg-red-50 px-4 py-3 text-sm text-red-700">
                {submitError}
              </div>
            ) : null}

            {copySource?.password_configured ? (
              <div className="rounded-lg border border-amber-200 bg-amber-50 px-4 py-3 text-sm text-amber-800">
                已保存的密码会安全复制；如需更换，请在下方输入新密码。
              </div>
            ) : null}

            <div className="space-y-2">
              <Label htmlFor="dl-name">名称</Label>
              <Input id="dl-name" value={form.name} onChange={(e) => setForm((prev) => ({ ...prev, name: e.target.value }))} placeholder="例如：我的 qBittorrent" />
            </div>
            <div className="space-y-2">
              <Label>类型</Label>
              <Select value={form.downloader_type} onChange={(val) => setForm((prev) => ({ ...prev, downloader_type: val }))} options={[{ value: "qbittorrent", label: "qBittorrent" }]} />
            </div>
            <div className="space-y-2">
              <Label htmlFor="dl-url">URL</Label>
              <Input id="dl-url" value={form.url} onChange={(e) => setForm((prev) => ({ ...prev, url: e.target.value }))} placeholder="例如：http://127.0.0.1:8080" />
            </div>
            <div className="grid gap-4 sm:grid-cols-2">
              <div className="space-y-2">
                <Label htmlFor="dl-user">用户名</Label>
                <Input id="dl-user" value={form.username} onChange={(e) => setForm((prev) => ({ ...prev, username: e.target.value }))} placeholder="可选" />
              </div>
              <div className="space-y-2">
                <Label htmlFor="dl-pass">密码</Label>
                <Input
                  id="dl-pass"
                  type="password"
                  value={form.password}
                  onChange={(e) => {
                    setForm((prev) => ({ ...prev, password: e.target.value }));
                    if (e.target.value) setClearPassword(false);
                  }}
                  placeholder={
                    editingId !== null && existingPasswordConfigured
                      ? "留空以保留已保存密码"
                      : copySource?.password_configured
                        ? "留空以复制已保存密码"
                        : "可选"
                  }
                  disabled={clearPassword}
                />
              </div>
            </div>
            {editingId !== null && existingPasswordConfigured ? (
              <Label className="flex cursor-pointer items-center gap-2 text-sm">
                <input
                  type="checkbox"
                  className="size-4 accent-primary"
                  checked={clearPassword}
                  onChange={(event) => {
                    setClearPassword(event.target.checked);
                    if (event.target.checked) setForm((prev) => ({ ...prev, password: "" }));
                  }}
                />
                清除已保存密码
              </Label>
            ) : null}
            <div className="flex justify-end gap-2 border-t border-border pt-4">
              <Button variant="outline" onClick={closeDialog}>取消</Button>
              <Button disabled={saving || !form.name || !form.url} onClick={handleSave}>{saving ? "保存中..." : "保存"}</Button>
            </div>
          </div>
        </Dialog>

        <Dialog
          open={deleteTarget !== null}
          onClose={() => setDeleteTarget(null)}
          title="确认删除"
          description={`确定要删除下载器「${deleteTarget?.name ?? ""}」吗？此操作不可撤销。`}
        >
          <div className="flex justify-end gap-2 p-4 sm:p-6">
            <Button variant="outline" onClick={() => setDeleteTarget(null)}>取消</Button>
            <Button variant="destructive" disabled={deleting} onClick={handleDelete}>{deleting ? "删除中..." : "确认删除"}</Button>
          </div>
        </Dialog>
      </>
    );
  }


  return (
    <div className="space-y-6">
      {/* Header */}
      <div className="flex items-center justify-between">
        <div>
          <h2 className="text-xl font-semibold">下载器管理</h2>
          <p className="mt-1 text-sm text-muted">管理已配置的下载器实例</p>
        </div>
        <Button onClick={openAdd}>
          <Plus className="mr-2 h-4 w-4" />
          添加下载器
        </Button>
      </div>

      {/* Test result banner */}
      {testResult && (
        <div
          className={`rounded-2xl border p-4 text-sm ${
            testResult.success
              ? "border-emerald-200 bg-emerald-50 text-emerald-800"
              : "border-red-200 bg-red-50 text-red-800"
          }`}
        >
          <div className="flex items-center justify-between gap-4">
            <div className="space-y-1">
              <div className="font-medium">{testResult.success ? "连接成功" : "连接失败"}</div>
              <div>{testResult.message}</div>
              {testResult.version && <div>版本：{testResult.version}</div>}
              {testResult.free_space !== null && (
                <div>可用空间：{formatBytes(testResult.free_space)}</div>
              )}
            </div>
            <Button variant="outline" onClick={() => setTestResult(null)}>
              关闭
            </Button>
          </div>
        </div>
      )}

      {/* Table (desktop) */}
      <Card className="rounded-2xl">
        <CardHeader>
          <CardTitle>下载器列表</CardTitle>
        </CardHeader>
        <CardContent>
          {message ? (
            <div className="mb-4 rounded-2xl border border-border bg-surface-container/70 px-4 py-3 text-sm">
              <div className="flex items-start justify-between gap-3">
                <span>{message}</span>
                <button type="button" className="text-muted hover:text-foreground" onClick={() => setMessage("")}>
                  关闭
                </button>
              </div>
            </div>
          ) : null}

          {loading ? (
            <div className="text-sm text-muted">加载中...</div>
          ) : downloaders.length === 0 ? (
            <div className="text-sm text-muted">暂无下载器，请点击右上角添加。</div>
          ) : (
            <>
              {/* Desktop table */}
              <div className="hidden xl:block">
                <Table>
                  <TableHeader>
                    <TableRow>
                      <TableHead>名称</TableHead>
                      <TableHead>类型</TableHead>
                      <TableHead>URL</TableHead>
                      <TableHead>创建时间</TableHead>
                      <TableHead>空间状态</TableHead>
                      <TableHead className="text-right">操作</TableHead>
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    {downloaders.map((d) => (
                      <TableRow key={d.id}>
                        <TableCell>
                          <div className="font-medium">{d.name}</div>
                          <div className="text-xs text-muted">#{d.id}</div>
                        </TableCell>
                        <TableCell>
                          <span className="rounded-full bg-violet-100 px-3 py-1 text-xs font-medium text-violet-700">
                            {d.downloader_type}
                          </span>
                        </TableCell>
                        <TableCell className="max-w-xs truncate text-sm text-muted">
                          {d.url}
                        </TableCell>
                        <TableCell className="text-sm text-muted">
                          {formatDate(d.created_at)}
                        </TableCell>
                        <TableCell className="text-sm text-muted">
                          {spaceStats[d.id] ? (
                            <div className="space-y-1">
                              <div>当前空闲：{formatBytes(spaceStats[d.id].free_space)}</div>
                              <div>未完成剩余：{formatBytes(spaceStats[d.id].pending_download_bytes)}</div>
                              <div>预测可用：{formatBytes(spaceStats[d.id].effective_free_space)}</div>
                            </div>
                          ) : (
                            "加载中..."
                          )}
                        </TableCell>
                        <TableCell className="text-right">
                          <div className="flex items-center justify-end gap-2">
                            <Button variant="outline" className="h-8 px-2.5 text-xs" onClick={() => openDetail(d.id)}>
                              <Eye className="mr-1.5 h-3.5 w-3.5" />
                              详情
                            </Button>
                            <Button
                              variant="outline"
                              className="h-8 px-2.5 text-xs"
                              disabled={testing === d.id}
                              onClick={() => handleTest(d.id)}
                            >
                              <TestTubeDiagonal className="mr-1.5 h-3.5 w-3.5" />
                              {testing === d.id ? "测试中..." : "测试连接"}
                            </Button>
                            <Button variant="outline" className="h-8 px-2.5 text-xs" onClick={() => openEdit(d)}>
                              <Edit className="mr-1.5 h-3.5 w-3.5" />
                              编辑
                            </Button>
                            <Button variant="outline" className="h-8 px-2.5 text-xs" onClick={() => openCopy(d)}>
                              <Copy className="mr-1.5 h-3.5 w-3.5" />
                              复制
                            </Button>
                            <Button variant="destructive" className="h-8 px-2.5 text-xs" onClick={() => setDeleteTarget(d)}>
                              <Trash2 className="mr-1.5 h-3.5 w-3.5" />
                              删除
                            </Button>
                          </div>
                        </TableCell>
                      </TableRow>
                    ))}
                  </TableBody>
                </Table>
              </div>

              {/* Mobile cards */}
              <div className="grid gap-3 xl:hidden">
                {downloaders.map((d) => (
                  <div
                    key={d.id}
                    className="rounded-xl border border-border bg-surface-container/70 p-3.5 shadow-sm"
                  >
                    <div className="flex flex-wrap items-start justify-between gap-3">
                      <div className="min-w-0 flex-1">
                        <div className="font-medium text-sm truncate">{d.name}</div>
                        <div className="mt-0.5 text-[11px] text-muted">#{d.id}</div>
                      </div>
                      <span className="shrink-0 rounded-full bg-violet-100 px-2.5 py-0.5 text-[11px] font-medium text-violet-700">
                        {d.downloader_type}
                      </span>
                    </div>
                    <div className="mt-2.5 grid gap-1.5 text-[11px] text-muted sm:grid-cols-2">
                      <div className="truncate">URL: {d.url}</div>
                      <div>创建时间: {formatDate(d.created_at)}</div>
                    </div>
                    {spaceStats[d.id] ? (
                      <div className="mt-2.5 grid gap-1.5 text-[11px] text-muted sm:grid-cols-2">
                        <div>空闲: {formatBytes(spaceStats[d.id].free_space)}</div>
                        <div>未完成剩余: {formatBytes(spaceStats[d.id].pending_download_bytes)}</div>
                        <div>预测可用: {formatBytes(spaceStats[d.id].effective_free_space)}</div>
                        <div>未完成: {spaceStats[d.id].incomplete_count} / 总数: {spaceStats[d.id].torrent_count}</div>
                      </div>
                    ) : null}
                    <div className="mt-3 flex flex-wrap gap-2">
                      <Button variant="outline" className="h-7 px-2.5 text-[11px]" onClick={() => openDetail(d.id)}>
                        <Eye className="mr-1.5 h-3.5 w-3.5" />
                        详情
                      </Button>
                      <Button
                        variant="outline"
                        className="h-7 text-[11px] px-2.5"
                        disabled={testing === d.id}
                        onClick={() => handleTest(d.id)}
                      >
                        <TestTubeDiagonal className="mr-1.5 h-3.5 w-3.5" />
                        {testing === d.id ? "测试中..." : "测试连接"}
                      </Button>
                      <Button variant="outline" className="h-7 text-[11px] px-2.5" onClick={() => openEdit(d)}>
                        <Edit className="mr-1.5 h-3.5 w-3.5" />
                        编辑
                      </Button>
                      <Button variant="outline" className="h-7 text-[11px] px-2.5" onClick={() => openCopy(d)}>
                        <Copy className="mr-1.5 h-3.5 w-3.5" />
                        复制
                      </Button>
                      <Button variant="destructive" className="h-7 text-[11px] px-2.5" onClick={() => setDeleteTarget(d)}>
                        <Trash2 className="mr-1.5 h-3.5 w-3.5" />
                        删除
                      </Button>
                    </div>
                  </div>
                ))}
              </div>
            </>
          )}
        </CardContent>
      </Card>

      {renderDialogs()}
    </div>
  );
}

function DetailRow({ icon, label, value }: { icon: React.ReactNode; label: string; value: string }) {
  return (
    <div className="grid gap-1 py-3.5 sm:grid-cols-[160px_minmax(0,1fr)] sm:items-center">
      <dt className="flex items-center gap-2 text-sm text-muted">
        <span className="text-primary">{icon}</span>
        {label}
      </dt>
      <dd className="min-w-0 break-all text-sm font-medium sm:text-right">{value}</dd>
    </div>
  );
}

function AnalysisMetric({ label, value, detail }: { label: string; value: string; detail?: string }) {
  return (
    <div className="border-b border-border px-4 py-3 last:border-b-0 sm:[&:nth-child(odd)]:border-r xl:border-b-0 xl:border-r xl:last:border-r-0">
      <div className="text-xs text-muted">{label}</div>
      <div className="mt-1 text-lg font-semibold">{value}</div>
      {detail ? <div className="mt-0.5 text-xs text-muted">{detail}</div> : null}
    </div>
  );
}

function DirectoryGroupPanel({
  group,
  expanded,
  onToggle,
}: {
  group: DirectoryGroup;
  expanded: boolean;
  onToggle: () => void;
}) {
  return (
    <div className="overflow-hidden rounded-lg border border-border bg-surface-container/40">
      <button
        type="button"
        className="flex w-full items-start gap-3 px-3 py-3 text-left transition-colors hover:bg-accent focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring/40 sm:items-center sm:px-4"
        onClick={onToggle}
        aria-expanded={expanded}
      >
        {expanded ? <ChevronDown className="mt-0.5 h-4 w-4 shrink-0 text-primary sm:mt-0" /> : <ChevronRight className="mt-0.5 h-4 w-4 shrink-0 text-primary sm:mt-0" />}
        <FolderTree className="mt-0.5 h-4 w-4 shrink-0 text-muted sm:mt-0" />
        <div className="min-w-0 flex-1">
          <div className="truncate font-mono text-sm font-medium" title={group.directory}>{group.directory}</div>
          <div className="mt-1 flex flex-wrap gap-x-3 gap-y-1 text-xs text-muted">
            <span>{group.torrents.length} 个种子</span>
            <span>占用 {formatBytes(group.totalSize)}</span>
            <span>待下载 {formatBytes(group.pendingBytes)}</span>
            {group.incompleteCount > 0 ? <span>{group.incompleteCount} 个未完成</span> : <span>全部完成</span>}
            <span>最近添加 {group.lastAddedOn > 0 ? new Date(group.lastAddedOn * 1000).toLocaleString() : "未知"}</span>
          </div>
        </div>
      </button>
      {expanded ? (
        <div className="border-t border-border bg-card/60">
          {group.torrents.map((torrent) => (
            <div key={torrent.hash} className="grid gap-3 border-b border-border px-4 py-3 last:border-b-0 lg:grid-cols-[minmax(0,1fr)_minmax(0,0.8fr)_minmax(180px,260px)] lg:items-center">
              <div className="min-w-0">
                <div className="truncate text-sm font-medium" title={torrent.name}>{torrent.name}</div>
                <div className="mt-1 flex flex-wrap gap-x-3 gap-y-1 text-xs text-muted">
                  <span className="font-mono">{torrent.hash.slice(0, 12)}</span>
                  <span>{formatTorrentState(torrent.state)}</span>
                </div>
              </div>
              <dl className="min-w-0 space-y-1.5 text-xs">
                <div className="flex items-baseline gap-2">
                  <dt className="shrink-0 text-muted">分类</dt>
                  <dd className="min-w-0 break-words [overflow-wrap:anywhere]">
                    {torrent.category.trim() || <span className="text-muted">未分类</span>}
                  </dd>
                </div>
                <div className="flex items-baseline gap-2">
                  <dt className="shrink-0 text-muted">标签</dt>
                  <dd className="min-w-0 break-words [overflow-wrap:anywhere]">
                    {torrent.tags.split(",").map((tag) => tag.trim()).filter(Boolean).join(" · ") || <span className="text-muted">无标签</span>}
                  </dd>
                </div>
              </dl>
              <div className="min-w-0">
                <div className="flex items-center justify-between gap-3 text-xs">
                  <span className="text-muted">
                    {formatBytes(torrent.completed)} / {formatBytes(torrent.size)}
                  </span>
                  <span className="shrink-0 font-medium">
                    {Math.round(Math.min(1, Math.max(0, torrent.progress)) * 100)}%
                  </span>
                </div>
                <div className="mt-2 h-1.5 overflow-hidden rounded-full bg-surface-container">
                  <div
                    className="h-full rounded-full bg-primary transition-[width] duration-200 motion-reduce:transition-none"
                    style={{ width: `${Math.min(1, Math.max(0, torrent.progress)) * 100}%` }}
                  />
                </div>
                {torrent.amount_left > 0 ? (
                  <div className="mt-1 text-right text-[11px] text-muted">
                    剩余 {formatBytes(torrent.amount_left)}
                  </div>
                ) : null}
              </div>
            </div>
          ))}
        </div>
      ) : null}
    </div>
  );
}
