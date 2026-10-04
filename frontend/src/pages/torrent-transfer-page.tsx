import { useEffect, useMemo, useRef, useState } from "react";
import {
  AlertCircle,
  ArrowRight,
  CheckCircle2,
  Clock3,
  FolderTree,
  LoaderCircle,
  RefreshCw,
  RotateCcw,
  Search,
  ShieldAlert,
  X,
  XCircle,
} from "lucide-react";

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
import { SearchBox } from "@/components/list-controls";
import type { DownloaderRecord, TransferableTorrent } from "@/types";

type OpenListTargetDirectory = {
  id?: number;
  name: string;
  downloader_id: number;
  openlist_root: string;
  qb_root: string;
};

type OpenListSettingsSummary = {
  address: string;
  api_key_configured: boolean;
  enabled: boolean;
  updated_at: string;
  target_directory_id: number | null;
  source_mappings: Array<{ downloader_id: number; qb_path: string }>;
  target_directories: OpenListTargetDirectory[];
};

type TransferMode = "fixed" | "tmdb";

type TransferPlanClassification = {
  tmdb_id: number | null;
  media_type: string | null;
  title: string;
  year: number | null;
  category: string;
  genre: string;
  matched: boolean;
  source: string;
};

type TransferPlanItem = {
  hash: string;
  torrent_name: string;
  target_openlist_path: string;
  target_qb_path: string;
  classification: TransferPlanClassification;
};

type TransferPlan = {
  mode: "tmdb";
  target_directory_id: number;
  target_downloader_id: number;
  openlist_root: string;
  qb_root: string;
  directories: string[];
  items: TransferPlanItem[];
  warnings: string[];
  expected_config_updated_at: string;
  source_downloader_updated_at: string;
  target_downloader_updated_at: string;
};

type CopyCheckpoint = {
  path: string;
  size: number;
  operation: "copy_file" | "create_directory" | "review_existing" | string;
  phase: "prepared" | "uncertain" | string;
  submitted_at: string | null;
  terminal_failure_verified: boolean;
};

type TransferJob = {
  id: number;
  media_download_id: number | null;
  downloader_id: number | null;
  infohash: string;
  torrent_name: string;
  stage: string;
  workflow: "auto_copy" | "qb_migration";
  version: number;
  source_qb_path: string;
  source_openlist_path: string;
  target_openlist_path: string;
  target_qb_path: string;
  attempts: number;
  openlist_task_ids: string[];
  copy_checkpoint: CopyCheckpoint | null;
  manual_resolution_allowed: boolean;
  copy_resolution_actions: string[];
  migration_resolution_allowed: boolean;
  copy_lock_acquired: boolean;
  manifest_cursor: number;
  next_attempt_at: string | null;
  last_error: string | null;
  created_at: string;
  updated_at: string;
  stage_started_at: string;
  completed_at: string | null;
};

type TransferJobsResponse = {
  page: number;
  page_size: number;
  total: number;
  records: TransferJob[];
};

type PendingSafetyAction = {
  kind: "cancel-copy" | "retry-migration" | "abandon-migration";
  jobId: number;
};

type StageDisplay = {
  label: string;
  progress: number;
};

const STAGES: Record<string, StageDisplay> = {
  waiting_download: { label: "检查源任务", progress: 5 },
  planning_manual_review: { label: "迁移规划需要处理", progress: 5 },
  copy_reconcile: { label: "核对目标文件", progress: 12 },
  copy_legacy_reconcile: { label: "核对旧复制任务", progress: 12 },
  copy_submitting: { label: "安全提交复制", progress: 22 },
  copying: { label: "OpenList 复制中", progress: 38 },
  copy_manual_review: { label: "复制需要人工处理", progress: 38 },
  manifest_required: { label: "缺少可靠文件清单", progress: 12 },
  copy_succeeded: { label: "完成旧复制任务核验", progress: 52 },
  copy_verified: { label: "复制已确认，准备导出种子", progress: 56 },
  qb_reconcile: { label: "核对两端 qB 状态", progress: 58 },
  torrent_exported: { label: "种子已导出", progress: 60 },
  source_qb_removed: { label: "准备目标 qB", progress: 68 },
  target_qb_submitted: { label: "已提交目标 qB", progress: 74 },
  target_qb_check_requested: { label: "已请求目标 qB 校验", progress: 80 },
  target_qb_checking: { label: "目标 qB 完整性校验中", progress: 86 },
  target_qb_starting: { label: "等待目标 qB 做种", progress: 94 },
  qb_manual_review: { label: "qB 迁移需要人工处理", progress: 86 },
  source_removing: { label: "安全清理源文件", progress: 96 },
  source_remove_manual_review: { label: "源文件清理待人工核验", progress: 96 },
  source_removed: { label: "源文件已清理", progress: 98 },
  completed: { label: "转移完成", progress: 100 },
  cancelled: { label: "已停止", progress: 100 },
};

const TORRENT_PAGE_SIZE = 50;
const JOB_PAGE_SIZE = 20;
const MAX_TRANSFER_COUNT = 100;
const JOB_POLL_INTERVAL_MS = 5_000;

function formatBytes(bytes: number): string {
  if (bytes <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB", "PB"];
  const index = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  return `${(bytes / 1024 ** index).toFixed(2)} ${units[index]}`;
}

function normalizeQbPath(value: string): string | null {
  const normalized = value.trim().replace(/\\/g, "/");
  if (!normalized) return null;
  const absolute = normalized.startsWith("/");
  const parts = normalized.split("/").filter((part) => part && part !== ".");
  if (parts.some((part) => part === "..")) return null;
  if (parts.length === 0) return "/";
  return `${absolute ? "/" : ""}${parts.join("/")}`;
}

function pathIsWithin(root: string, path: string): boolean {
  return root === "/" || root === path || path.startsWith(`${root}/`);
}

function stageDisplay(job: TransferJob): StageDisplay {
  if (job.stage === "copying" && job.manual_resolution_allowed) {
    return { label: "旧复制任务待核验", progress: 32 };
  }
  if (job.stage === "copying" && !job.copy_lock_acquired) {
    return { label: "等待目标目录解锁", progress: 25 };
  }
  return STAGES[job.stage] ?? { label: job.stage, progress: 0 };
}

function copyOperationLabel(operation: string): string {
  if (operation === "create_directory") return "创建目录";
  if (operation === "review_existing") return "核对已有文件";
  if (operation === "remove_file") return "删除源文件";
  return "复制文件";
}

function copyRecheckLabel(job: TransferJob): string {
  if (job.stage === "planning_manual_review") return "重新规划";
  if (job.copy_checkpoint?.operation === "review_existing") return "重新检查";
  if (
    job.copy_checkpoint?.phase === "prepared" &&
    (job.copy_checkpoint.operation === "copy_file" ||
      job.copy_checkpoint.operation === "create_directory")
  ) {
    return "继续复制";
  }
  return "重新检查";
}

export function TorrentTransferPage() {
  const [downloaders, setDownloaders] = useState<DownloaderRecord[]>([]);
  const [settings, setSettings] = useState<OpenListSettingsSummary | null>(null);
  const [downloaderId, setDownloaderId] = useState("");
  const [targetDownloaderId, setTargetDownloaderId] = useState("");
  const [targetDirectoryId, setTargetDirectoryId] = useState("");
  const [targetMode, setTargetMode] = useState<TransferMode>("fixed");
  const [torrents, setTorrents] = useState<TransferableTorrent[]>([]);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [keyword, setKeyword] = useState("");
  const [torrentPage, setTorrentPage] = useState(1);
  const [torrentLoading, setTorrentLoading] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [planning, setPlanning] = useState(false);
  const [tmdbPlan, setTmdbPlan] = useState<TransferPlan | null>(null);
  const [confirmTransferOpen, setConfirmTransferOpen] = useState(false);
  const [notice, setNotice] = useState("");
  const [error, setError] = useState("");
  const [jobsError, setJobsError] = useState("");
  const [jobs, setJobs] = useState<TransferJob[]>([]);
  const [jobPage, setJobPage] = useState(1);
  const [jobTotal, setJobTotal] = useState(0);
  const [jobsLoading, setJobsLoading] = useState(true);
  const [jobPollNonce, setJobPollNonce] = useState(0);
  const [resolvingJobId, setResolvingJobId] = useState<number | null>(null);
  const [pendingSafetyAction, setPendingSafetyAction] = useState<PendingSafetyAction | null>(null);
  const [modalError, setModalError] = useState("");
  const [copyTaskStoppedConfirmed, setCopyTaskStoppedConfirmed] = useState(false);
  const torrentRequestGeneration = useRef(0);
  const jobPollGeneration = useRef(0);
  const jobPollController = useRef<AbortController | null>(null);

  const selectedSource = downloaders.find((item) => String(item.id) === downloaderId);
  const selectedTarget = settings?.target_directories.find(
    (target) => target.id != null && String(target.id) === targetDirectoryId,
  );
  const selectedTargetDownloader = downloaders.find((item) => String(item.id) === targetDownloaderId);
  const targetDirectories = useMemo(
    () => (settings?.target_directories ?? []).filter(
      (target) => target.id != null && (!targetDownloaderId || String(target.downloader_id) === targetDownloaderId),
    ),
    [settings?.target_directories, targetDownloaderId],
  );
  const sourceHasMapping = Boolean(
    selectedSource && settings?.source_mappings.some((mapping) => mapping.downloader_id === selectedSource.id),
  );
  const configVersionAvailable = Boolean(settings?.updated_at.trim());
  const mappedTorrentHashes = useMemo(() => {
    const roots = (settings?.source_mappings ?? [])
      .filter((mapping) => String(mapping.downloader_id) === downloaderId)
      .map((mapping) => normalizeQbPath(mapping.qb_path))
      .filter((path): path is string => path != null);
    return new Set(
      torrents
        .filter((torrent) => {
          const savePath = normalizeQbPath(torrent.save_path);
          return savePath != null && roots.some((root) => pathIsWithin(root, savePath));
        })
        .map((torrent) => torrent.hash),
    );
  }, [downloaderId, settings?.source_mappings, torrents]);
  const openListConfigured = Boolean(settings?.address.trim() && settings.api_key_configured);
  const selectedTorrents = useMemo(
    () => torrents.filter((torrent) => selected.has(torrent.hash)),
    [selected, torrents],
  );
  const selectedBytes = useMemo(
    () => selectedTorrents.reduce((total, torrent) => total + torrent.size, 0),
    [selectedTorrents],
  );
  const filteredTorrents = useMemo(() => {
    const query = keyword.trim().toLowerCase();
    return torrents.filter(
      (torrent) => !query || torrent.name.toLowerCase().includes(query) || torrent.hash.toLowerCase().includes(query),
    );
  }, [keyword, torrents]);
  const torrentPageCount = Math.max(1, Math.ceil(filteredTorrents.length / TORRENT_PAGE_SIZE));
  const pagedTorrents = filteredTorrents.slice(
    (torrentPage - 1) * TORRENT_PAGE_SIZE,
    torrentPage * TORRENT_PAGE_SIZE,
  );
  const selectablePageTorrents = pagedTorrents.filter((torrent) => mappedTorrentHashes.has(torrent.hash));
  const pageSelected = selectablePageTorrents.length > 0
    && selectablePageTorrents.every((torrent) => selected.has(torrent.hash));
  const jobPageCount = Math.max(1, Math.ceil(jobTotal / JOB_PAGE_SIZE));
  const pendingSafetyJob = pendingSafetyAction
    ? jobs.find((job) => job.id === pendingSafetyAction.jobId) ?? null
    : null;
  const canSubmit = Boolean(
    selectedSource
      && selectedTarget
      && selectedTargetDownloader
      && selectedTarget.downloader_id === selectedTargetDownloader.id
      && sourceHasMapping
      && openListConfigured
      && configVersionAvailable
      && selected.size > 0
      && selected.size <= MAX_TRANSFER_COUNT
      && selectedTorrents.length === selected.size
      && selectedTorrents.every((torrent) => mappedTorrentHashes.has(torrent.hash)),
  );

  function refreshJobs(page = jobPage) {
    jobPollController.current?.abort();
    jobPollController.current = null;
    jobPollGeneration.current += 1;
    setJobPage(page);
    setJobPollNonce((value) => value + 1);
  }

  useEffect(() => {
    let active = true;
    const controller = new AbortController();
    Promise.all([
      api<DownloaderRecord[]>("/api/downloaders", { signal: controller.signal }),
      api<OpenListSettingsSummary>("/api/media/openlist/settings", { signal: controller.signal }),
    ])
      .then(([items, loadedSettings]) => {
        if (!active) return;
        const qbDownloaders = items.filter(
          (item) => item.downloader_type === "qbittorrent" || item.downloader_type === "qb",
        );
        setDownloaders(qbDownloaders);
        setSettings(loadedSettings);
        if (qbDownloaders.length === 1) setDownloaderId(String(qbDownloaders[0].id));
        const configuredTargets = loadedSettings.target_directories.filter((target) => target.id != null);
        const preferredTarget = configuredTargets.find(
          (target) => target.id === loadedSettings.target_directory_id,
        ) ?? (configuredTargets.length === 1 ? configuredTargets[0] : undefined);
        if (preferredTarget) {
          setTargetDirectoryId(String(preferredTarget.id));
          setTargetDownloaderId(String(preferredTarget.downloader_id));
        } else if (qbDownloaders.length === 1) {
          setTargetDownloaderId(String(qbDownloaders[0].id));
        }
      })
      .catch((loadError: Error) => {
        if (active && loadError.name !== "AbortError") {
          setError(loadError.message || "加载转移配置失败");
        }
      });
    return () => {
      active = false;
      controller.abort();
    };
  }, []);

  useEffect(() => {
    if (!targetDirectoryId) return;
    const target = settings?.target_directories.find(
      (item) => item.id != null && String(item.id) === targetDirectoryId,
    );
    if (!target || (targetDownloaderId && String(target.downloader_id) !== targetDownloaderId)) {
      setTargetDirectoryId("");
      setTmdbPlan(null);
    }
  }, [settings?.target_directories, targetDirectoryId, targetDownloaderId]);

  useEffect(() => {
    if (!tmdbPlan) return;
    const planHashes = new Set(tmdbPlan.items.map((item) => item.hash));
    const currentHashes = new Set(selected);
    const sourceUpdatedAt = selectedSource?.updated_at;
    const targetUpdatedAt = selectedTargetDownloader?.updated_at;
    if (
      planHashes.size !== currentHashes.size
      || [...planHashes].some((hash) => !currentHashes.has(hash))
      || targetMode !== "tmdb"
      || tmdbPlan.expected_config_updated_at !== settings?.updated_at
      || tmdbPlan.target_directory_id !== selectedTarget?.id
      || tmdbPlan.target_downloader_id !== selectedTarget?.downloader_id
      || (sourceUpdatedAt != null && tmdbPlan.source_downloader_updated_at !== sourceUpdatedAt)
      || (targetUpdatedAt != null && tmdbPlan.target_downloader_updated_at !== targetUpdatedAt)
    ) {
      setTmdbPlan(null);
    }
  }, [selected, selectedSource?.updated_at, selectedTarget?.downloader_id, selectedTarget?.id,
    selectedTargetDownloader?.updated_at, settings?.updated_at, targetMode, tmdbPlan]);

  useEffect(() => {
    const generation = ++jobPollGeneration.current;
    let active = true;
    let timer: number | null = null;
    let controller: AbortController | null = null;

    async function poll(showLoading: boolean) {
      if (!active || generation !== jobPollGeneration.current) return;
      // 标签页在后台时跳过本轮请求，回到前台后的下一轮再刷新。
      if (!showLoading && document.hidden) {
        timer = window.setTimeout(() => void poll(false), JOB_POLL_INTERVAL_MS);
        return;
      }
      if (showLoading) setJobsLoading(true);
      controller = new AbortController();
      jobPollController.current = controller;
      try {
        const response = await api<TransferJobsResponse>(
          `/api/media/openlist/manual-jobs?page=${jobPage}&page_size=${JOB_PAGE_SIZE}`,
          { signal: controller.signal },
        );
        if (!active || generation !== jobPollGeneration.current) return;
        setJobs(response.records);
        setJobTotal(response.total);
        setJobsError("");
      } catch (loadError) {
        if (!active || generation !== jobPollGeneration.current) return;
        const requestError = loadError as Error;
        if (requestError.name !== "AbortError") {
          setJobsError(requestError.message || "加载转移任务失败");
        }
      } finally {
        if (jobPollController.current === controller) jobPollController.current = null;
        if (!active || generation !== jobPollGeneration.current) return;
        if (showLoading) setJobsLoading(false);
        timer = window.setTimeout(() => void poll(false), JOB_POLL_INTERVAL_MS);
      }
    }

    void poll(true);
    return () => {
      active = false;
      if (timer != null) window.clearTimeout(timer);
      controller?.abort();
      if (jobPollController.current === controller) jobPollController.current = null;
    };
  }, [jobPage, jobPollNonce]);

  useEffect(() => {
    if (jobPage <= jobPageCount) return;
    jobPollGeneration.current += 1;
    setJobPage(jobPageCount);
    setJobPollNonce((value) => value + 1);
  }, [jobPage, jobPageCount]);

  useEffect(() => {
    const generation = ++torrentRequestGeneration.current;
    const controller = new AbortController();
    setSelected(new Set());
    setKeyword("");
    setTorrentPage(1);
    setTorrents([]);
    if (!downloaderId) {
      setTorrentLoading(false);
      return () => controller.abort();
    }
    setTorrentLoading(true);
    setError("");
    api<TransferableTorrent[]>(`/api/downloaders/${downloaderId}/torrents`, { signal: controller.signal })
      .then((items) => {
        if (generation === torrentRequestGeneration.current) setTorrents(items);
      })
      .catch((loadError: Error) => {
        if (generation === torrentRequestGeneration.current && loadError.name !== "AbortError") {
          setError(loadError.message || "加载种子失败");
        }
      })
      .finally(() => {
        if (generation === torrentRequestGeneration.current) setTorrentLoading(false);
      });
    return () => controller.abort();
  }, [downloaderId]);

  useEffect(() => {
    if (pendingSafetyAction && !pendingSafetyJob) setPendingSafetyAction(null);
    if (
      pendingSafetyAction?.kind === "cancel-copy"
      && pendingSafetyJob
      && !pendingSafetyJob.copy_resolution_actions.includes("cancel")
    ) {
      setPendingSafetyAction(null);
    }
    if (
      ["retry-migration", "abandon-migration"].includes(pendingSafetyAction?.kind ?? "")
      && pendingSafetyJob
      && !pendingSafetyJob.migration_resolution_allowed
    ) {
      setPendingSafetyAction(null);
    }
  }, [pendingSafetyAction, pendingSafetyJob]);

  function toggle(hash: string) {
    if (!mappedTorrentHashes.has(hash)) {
      setError("该种子的保存路径不在任何 OpenList 来源映射下");
      return;
    }
    if (!selected.has(hash) && selected.size >= MAX_TRANSFER_COUNT) {
      setError(`单次最多转移 ${MAX_TRANSFER_COUNT} 个种子`);
      return;
    }
    setError("");
    setTmdbPlan(null);
    setSelected((current) => {
      const next = new Set(current);
      if (next.has(hash)) next.delete(hash);
      else next.add(hash);
      return next;
    });
  }

  function togglePage() {
    setTmdbPlan(null);
    setSelected((current) => {
      const next = new Set(current);
      const allPageSelected = selectablePageTorrents.length > 0
        && selectablePageTorrents.every((torrent) => current.has(torrent.hash));
      if (allPageSelected) {
        selectablePageTorrents.forEach((torrent) => next.delete(torrent.hash));
        return next;
      }
      const available = MAX_TRANSFER_COUNT - next.size;
      const additions = selectablePageTorrents.filter((torrent) => !next.has(torrent.hash));
      additions.slice(0, available).forEach((torrent) => next.add(torrent.hash));
      if (additions.length > available) {
        setNotice(`单次最多转移 ${MAX_TRANSFER_COUNT} 个种子，已选择前 ${MAX_TRANSFER_COUNT} 个`);
      }
      return next;
    });
  }

  async function generateTmdbPlan() {
    if (!canSubmit || !selectedSource || !selectedTarget?.id) return;
    setPlanning(true);
    setTmdbPlan(null);
    setError("");
    setNotice("");
    setModalError("");
    setConfirmTransferOpen(true);
    try {
      const plan = await api<TransferPlan>(
        `/api/downloaders/${selectedSource.id}/openlist-transfer/preview`,
        {
          method: "POST",
          body: JSON.stringify({
            hashes: [...selected],
            target_directory_id: selectedTarget.id,
            target_downloader_id: selectedTarget.downloader_id,
            expected_config_updated_at: settings?.updated_at ?? "",
          }),
        },
      );
      setTmdbPlan(plan);
    } catch (planError) {
      setModalError((planError as Error).message || "生成 TMDB 分类目录失败");
    } finally {
      setPlanning(false);
    }
  }

  function openTransferConfirmation() {
    if (!canSubmit) return;
    setError("");
    setNotice("");
    setModalError("");
    if (targetMode === "tmdb") {
      void generateTmdbPlan();
    } else {
      setConfirmTransferOpen(true);
    }
  }

  async function submit() {
    if (!canSubmit || !selectedSource || !selectedTarget?.id) return;
    if (targetMode === "tmdb" && !tmdbPlan) return;
    setSubmitting(true);
    setError("");
    setNotice("");
    setModalError("");
    try {
      const result = await api<{ created: number; skipped: number }>(
        `/api/downloaders/${selectedSource.id}/openlist-transfer`,
        {
          method: "POST",
          body: JSON.stringify({
            hashes: [...selected],
            target_directory_id: selectedTarget.id,
            target_downloader_id: selectedTarget.downloader_id,
            target_mode: targetMode,
            plan_confirmed: targetMode === "tmdb",
            planned_targets: targetMode === "tmdb"
              ? tmdbPlan?.items.map((item) => ({
                hash: item.hash,
                target_openlist_path: item.target_openlist_path,
                target_qb_path: item.target_qb_path,
              })) ?? []
              : [],
            expected_source_downloader_updated_at: targetMode === "tmdb"
              ? tmdbPlan?.source_downloader_updated_at
              : undefined,
            expected_target_downloader_updated_at: targetMode === "tmdb"
              ? tmdbPlan?.target_downloader_updated_at
              : undefined,
            expected_config_updated_at: settings?.updated_at ?? "",
          }),
        },
      );
      setSelected(new Set());
      setTmdbPlan(null);
      setConfirmTransferOpen(false);
      refreshJobs(1);
      setNotice(
        `已创建 ${result.created} 个转移任务${
          result.skipped ? `，跳过 ${result.skipped} 个进行中的同种任务` : ""
        }`,
      );
    } catch (submitError) {
      setModalError((submitError as Error).message || "创建转移任务失败");
    } finally {
      setSubmitting(false);
    }
  }

  async function resolveCopy(
    job: TransferJob,
    resolution: "recheck" | "cancel",
    confirmTaskTerminated = false,
  ) {
    setResolvingJobId(job.id);
    setError("");
    setNotice("");
    setModalError("");
    try {
      await api(`/api/media/openlist/jobs/${job.id}/resolve-copy`, {
        method: "POST",
        body: JSON.stringify({
          resolution,
          expected_version: job.version,
          confirm_task_terminated: resolution === "cancel" && confirmTaskTerminated,
        }),
      });
      setPendingSafetyAction(null);
      refreshJobs();
      setNotice(resolution === "cancel"
        ? "手动迁移任务已停止"
        : job.stage === "planning_manual_review"
          ? "已重新规划手动迁移任务"
          : job.copy_checkpoint?.operation === "review_existing"
            ? "已安排只读重新检查已有文件"
          : job.copy_checkpoint?.phase === "prepared"
            ? "已恢复复制流程；提交前会再次核验"
            : "已安排只读重新检查复制结果");
    } catch (resolveError) {
      const message = (resolveError as Error).message || "处理复制任务失败";
      if (resolution === "cancel") setModalError(message);
      else setError(message);
      refreshJobs();
    } finally {
      setResolvingJobId(null);
    }
  }

  async function resolveMigration(job: TransferJob, resolution: "retry" | "abandon") {
    setResolvingJobId(job.id);
    setError("");
    setNotice("");
    setModalError("");
    try {
      await api(`/api/media/openlist/jobs/${job.id}/resolve-migration`, {
        method: "POST",
        body: JSON.stringify({ resolution, expected_version: job.version }),
      });
      setPendingSafetyAction(null);
      refreshJobs();
      setNotice(resolution === "abandon"
        ? "已放弃迁移，系统不会继续删除"
        : job.stage === "source_remove_manual_review"
          ? "已安排重新核验源文件清理；不会重发当前不确定删除，核验通过后可能继续清理剩余源文件"
          : "已重新启动迁移核验");
    } catch (resolveError) {
      setModalError((resolveError as Error).message || "处理 qB 迁移任务失败");
      refreshJobs();
    } finally {
      setResolvingJobId(null);
    }
  }

  const targetOptions = [
    { value: "", label: targetMode === "tmdb" ? "请选择分类根目录" : "请选择固定目录" },
    ...targetDirectories
      .filter((target): target is OpenListTargetDirectory & { id: number } => target.id != null)
      .map((target) => {
        return {
          value: String(target.id),
          label: `${target.name || "未命名目标"} · ${target.openlist_root || "未设置路径"}`,
        };
      }),
  ];

  return (
    <div className="flex min-w-0 flex-col gap-6">
      <div>
        <h2 className="text-xl font-semibold">种子转移</h2>
        <p className="mt-1 text-sm text-muted">将已完成的 qBittorrent 任务安全迁移到指定目录并恢复做种</p>
      </div>

      {error ? (
        <div className="flex items-start gap-2 rounded-lg border border-destructive/30 bg-destructive/10 px-4 py-3 text-sm text-destructive" role="alert" aria-live="assertive">
          <AlertCircle className="mt-0.5 size-4 shrink-0" />
          <span className="min-w-0 break-words">{error}</span>
        </div>
      ) : null}
      {notice ? (
        <div className="flex items-start gap-2 rounded-lg border border-primary/30 bg-primary/10 px-4 py-3 text-sm text-foreground" role="status" aria-live="polite">
          <CheckCircle2 className="mt-0.5 size-4 shrink-0 text-primary" />
          <span className="min-w-0 break-words">{notice}</span>
        </div>
      ) : null}

      <Card className="rounded-2xl">
        <CardHeader>
          <CardTitle>提交手动迁移</CardTitle>
          <CardDescription>每次迁移使用本次明确选择的目标，不跟随自动复制的默认目标。</CardDescription>
        </CardHeader>
        <CardContent className="flex flex-col gap-4">
          {settings && !settings.enabled ? (
            <div className="rounded-lg border border-border bg-surface-container/45 px-4 py-3 text-sm text-muted">
              自动复制当前关闭；本页的手动 qB 迁移仍可正常创建和执行。
            </div>
          ) : null}

          <div className="grid gap-3 lg:grid-cols-[minmax(0,16rem)_minmax(0,16rem)_minmax(0,18rem)_minmax(0,1fr)] lg:items-end">
            <div className="min-w-0">
              <Label htmlFor="torrent-transfer-source" className="mb-2 block">来源下载器</Label>
              <Select
                id="torrent-transfer-source"
                value={downloaderId}
                onChange={setDownloaderId}
                disabled={downloaders.length === 0}
                options={[
                  { value: "", label: downloaders.length === 0 ? "没有可用的 qBittorrent" : "选择 qBittorrent" },
                  ...downloaders.map((item) => ({
                    value: String(item.id),
                    label: `${item.name}${
                      settings && !settings.source_mappings.some((mapping) => mapping.downloader_id === item.id)
                        ? " · 未配置来源映射"
                        : ""
                    }`,
                  })),
                ]}
              />
            </div>
            <div className="min-w-0">
              <Label htmlFor="torrent-transfer-target-qb" className="mb-2 block">目标 qBittorrent</Label>
              <Select
                id="torrent-transfer-target-qb"
                value={targetDownloaderId}
                onChange={(value) => {
                  setTargetDownloaderId(value);
                  const target = selectedTarget;
                  if (target && String(target.downloader_id) !== value) setTargetDirectoryId("");
                  setTmdbPlan(null);
                }}
                disabled={downloaders.length === 0}
                options={[
                  { value: "", label: downloaders.length === 0 ? "没有可用的 qBittorrent" : "选择目标 qBittorrent" },
                  ...downloaders.map((item) => ({ value: String(item.id), label: item.name })),
                ]}
              />
            </div>
            <div className="min-w-0">
              <Label className="mb-2 block">目标方式</Label>
              <div className="flex h-11 rounded-2xl border border-border bg-input p-1" role="group" aria-label="目标方式">
                <button
                  type="button"
                  className={cn(
                    "min-w-0 flex-1 rounded-xl px-2 text-xs font-semibold transition-colors",
                    targetMode === "fixed" ? "bg-card text-foreground shadow-sm" : "text-muted hover:text-foreground",
                  )}
                  aria-pressed={targetMode === "fixed"}
                  onClick={() => {
                    setTargetMode("fixed");
                    setTmdbPlan(null);
                  }}
                >固定目录</button>
                <button
                  type="button"
                  className={cn(
                    "min-w-0 flex-1 rounded-xl px-2 text-xs font-semibold transition-colors",
                    targetMode === "tmdb" ? "bg-card text-foreground shadow-sm" : "text-muted hover:text-foreground",
                  )}
                  aria-pressed={targetMode === "tmdb"}
                  onClick={() => {
                    setTargetMode("tmdb");
                    setTmdbPlan(null);
                  }}
                >根目录 + TMDB</button>
              </div>
            </div>
            <div className="min-w-0">
              <Label htmlFor="torrent-transfer-target" className="mb-2 block">
                {targetMode === "tmdb" ? "分类根目录" : "固定目标目录"}
              </Label>
              <Select
                id="torrent-transfer-target"
                value={targetDirectoryId}
                onChange={(value) => {
                  setTargetDirectoryId(value);
                  setTmdbPlan(null);
                }}
                disabled={!settings || targetOptions.length === 1 || !targetDownloaderId}
                options={targetOptions}
              />
            </div>
            <SearchBox
              value={keyword}
              onChange={(value) => {
                setKeyword(value);
                setTorrentPage(1);
              }}
              placeholder="搜索名称或 info hash"
              label="搜索可转移种子"
            />
          </div>

          {selectedTarget ? (
            <div className="grid min-w-0 gap-3 rounded-lg border border-border bg-surface-container/45 px-4 py-3 text-xs sm:grid-cols-2 lg:grid-cols-3">
              <div className="min-w-0">
                <div className="text-muted">目标下载器</div>
                <div className="mt-1 truncate font-medium" title={selectedTargetDownloader?.name}>
                  {selectedTargetDownloader?.name ?? "目标下载器不可用"}
                </div>
              </div>
              <div className="min-w-0">
                <div className="text-muted">OpenList 路径</div>
                <div className="mt-1 truncate font-mono" title={selectedTarget.openlist_root}>{selectedTarget.openlist_root}</div>
              </div>
              <div className="min-w-0 sm:col-span-2 lg:col-span-1">
                <div className="text-muted">qB 路径</div>
                <div className="mt-1 truncate font-mono" title={selectedTarget.qb_root}>{selectedTarget.qb_root}</div>
              </div>
              {targetMode === "tmdb" ? (
                <div className="flex min-w-0 items-start gap-2 sm:col-span-2 lg:col-span-3">
                  <FolderTree className="mt-0.5 size-4 shrink-0 text-primary" />
                  <span className="text-muted">提交前会按 TMDB 类型、主类型和年份生成目录；目录生成后需要再次确认。</span>
                </div>
              ) : null}
            </div>
          ) : null}

          {selectedSource && !sourceHasMapping ? (
            <div className="flex items-start gap-2 rounded-lg border border-destructive/25 bg-destructive/5 px-4 py-3 text-sm text-destructive">
              <AlertCircle className="mt-0.5 size-4 shrink-0" />
              <span>该来源下载器尚未配置 OpenList 来源路径映射，无法安全迁移。</span>
            </div>
          ) : null}
          {settings && !openListConfigured ? (
            <div className="flex items-start gap-2 rounded-lg border border-destructive/25 bg-destructive/5 px-4 py-3 text-sm text-destructive">
              <AlertCircle className="mt-0.5 size-4 shrink-0" />
              <span>OpenList 地址或 API Key 尚未配置。</span>
            </div>
          ) : null}
          {settings && !configVersionAvailable ? (
            <div className="flex items-start gap-2 rounded-lg border border-destructive/25 bg-destructive/5 px-4 py-3 text-sm text-destructive" role="alert">
              <AlertCircle className="mt-0.5 size-4 shrink-0" />
              <span>未取得 OpenList 配置版本，请刷新页面后再创建迁移任务。</span>
            </div>
          ) : null}
          {settings && targetDownloaderId && targetDirectories.length === 0 ? (
            <div className="flex items-start gap-2 rounded-lg border border-destructive/25 bg-destructive/5 px-4 py-3 text-sm text-destructive" role="alert">
              <AlertCircle className="mt-0.5 size-4 shrink-0" />
              <span>目标 qBittorrent 尚未配置可用的 OpenList 目标目录，请先在自动复制设置中添加根目录。</span>
            </div>
          ) : null}

          <div className="flex h-[min(34rem,60dvh)] min-h-80 min-w-0 flex-col overflow-hidden rounded-lg border border-border">
            <div className="min-h-0 min-w-0 flex-1 overflow-auto">
              {torrentLoading ? (
                <div className="flex h-full items-center justify-center gap-2 text-sm text-muted">
                  <LoaderCircle className="size-4 animate-spin" />加载种子列表
                </div>
              ) : !downloaderId ? (
                <div className="flex h-full items-center justify-center text-sm text-muted">请先选择来源下载器</div>
              ) : pagedTorrents.length === 0 ? (
                <div className="flex h-full items-center justify-center text-sm text-muted">没有可转移的已完成种子</div>
              ) : (
                <Table className="min-w-0">
                  <TableHeader className="sticky top-0 bg-card">
                    <TableRow>
                      <TableHead className="w-12 px-3">
                        <input
                          type="checkbox"
                          className="size-4 cursor-pointer accent-primary disabled:cursor-not-allowed disabled:opacity-40"
                          checked={pageSelected}
                          disabled={selectablePageTorrents.length === 0}
                          onChange={togglePage}
                          aria-label={pageSelected ? "取消全选当前页" : "全选当前页"}
                        />
                      </TableHead>
                      <TableHead>种子</TableHead>
                      <TableHead className="hidden sm:table-cell">大小</TableHead>
                      <TableHead className="hidden lg:table-cell">保存路径</TableHead>
                      <TableHead className="hidden xl:table-cell">分类 / 标签</TableHead>
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    {pagedTorrents.map((torrent) => {
                      const mapped = mappedTorrentHashes.has(torrent.hash);
                      return (
                      <TableRow key={torrent.hash} className={cn(selected.has(torrent.hash) && "bg-primary/5", !mapped && "opacity-55")}>
                        <TableCell className="px-3">
                          <input
                            type="checkbox"
                            className="size-4 cursor-pointer accent-primary disabled:cursor-not-allowed"
                            checked={selected.has(torrent.hash)}
                            disabled={!mapped}
                            onChange={() => toggle(torrent.hash)}
                            aria-label={`选择 ${torrent.name}`}
                          />
                        </TableCell>
                        <TableCell className="min-w-0">
                          <div className="max-w-[58vw] truncate font-medium sm:max-w-md" title={torrent.name}>{torrent.name}</div>
                          <div className="mt-1 text-xs text-muted">
                            <span className="font-mono">{torrent.hash.slice(0, 12)}</span>
                            {!mapped ? <span> · 保存路径未映射</span> : null}
                          </div>
                        </TableCell>
                        <TableCell className="hidden whitespace-nowrap sm:table-cell">{formatBytes(torrent.size)}</TableCell>
                        <TableCell className="hidden lg:table-cell">
                          <div className="max-w-xs truncate text-xs text-muted" title={torrent.save_path}>{torrent.save_path}</div>
                        </TableCell>
                        <TableCell className="hidden text-xs text-muted xl:table-cell">
                          {[torrent.category, torrent.tags].filter(Boolean).join(" · ") || "-"}
                        </TableCell>
                      </TableRow>
                      );
                    })}
                  </TableBody>
                </Table>
              )}
            </div>
            <div className="flex shrink-0 flex-col gap-3 border-t border-border bg-card px-3 py-3 sm:flex-row sm:items-center sm:justify-between">
              <div className="flex min-w-0 items-center justify-between gap-2 sm:justify-start">
                <Button
                  variant="outline"
                  className="h-9 shrink-0 px-3"
                  disabled={torrentPage <= 1}
                  onClick={() => setTorrentPage((page) => page - 1)}
                >上一页</Button>
                <span className="min-w-0 truncate text-xs text-muted">
                  第 {torrentPage} / {torrentPageCount} 页 · 共 {filteredTorrents.length} 个
                </span>
                <Button
                  variant="outline"
                  className="h-9 shrink-0 px-3"
                  disabled={torrentPage >= torrentPageCount}
                  onClick={() => setTorrentPage((page) => page + 1)}
                >下一页</Button>
              </div>
              <Button onClick={openTransferConfirmation} disabled={submitting || planning || !canSubmit}>
                {planning ? <LoaderCircle className="animate-spin" data-icon="inline-start" /> : targetMode === "tmdb" ? <FolderTree data-icon="inline-start" /> : <ArrowRight data-icon="inline-start" />}
                {targetMode === "tmdb" ? `生成目录并核对 ${selected.size} 个` : `核对并转移 ${selected.size} 个`}
              </Button>
            </div>
          </div>
        </CardContent>
      </Card>

      <Card className="rounded-2xl">
        <CardHeader>
          <div className="flex flex-wrap items-start justify-between gap-3">
            <div>
              <CardTitle>手动 qB 迁移任务</CardTitle>
              <CardDescription>共 {jobTotal} 条 · 自动刷新</CardDescription>
            </div>
            <Button
              variant="outline"
              className="size-10 px-0"
              disabled={jobsLoading}
              onClick={() => refreshJobs()}
              aria-label="刷新转移任务"
              title="刷新转移任务"
            >
              {jobsLoading ? <LoaderCircle className="animate-spin" /> : <RefreshCw />}
            </Button>
          </div>
        </CardHeader>
        <CardContent className="flex flex-col gap-4">
          {jobsError ? (
            <div className="flex items-start gap-2 rounded-lg border border-destructive/25 bg-destructive/5 px-4 py-3 text-sm text-destructive" role="alert" aria-live="assertive">
              <AlertCircle className="mt-0.5 size-4 shrink-0" />
              <span className="min-w-0 break-words">{jobsError}</span>
            </div>
          ) : null}

          {jobsLoading && jobs.length === 0 ? (
            <div className="flex min-h-28 items-center justify-center gap-2 text-sm text-muted">
              <LoaderCircle className="size-4 animate-spin" />加载任务
            </div>
          ) : jobs.length === 0 ? (
            <div className="rounded-lg border border-border bg-surface-container/45 px-4 py-8 text-center text-sm text-muted">
              暂无手动迁移任务
            </div>
          ) : (
            <>
              <div className="hidden lg:block">
                <Table>
                  <TableHeader>
                    <TableRow>
                      <TableHead>种子</TableHead>
                      <TableHead>当前阶段</TableHead>
                      <TableHead>路径与复制核验</TableHead>
                      <TableHead>更新时间</TableHead>
                      <TableHead className="text-right">操作</TableHead>
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    {jobs.map((job) => (
                      <TransferJobRow
                        key={job.id}
                        job={job}
                        sourceName={downloaders.find((item) => item.id === job.downloader_id)?.name}
                        resolving={resolvingJobId === job.id}
                        onCopyRecheck={() => void resolveCopy(job, "recheck")}
                        onCopyCancel={() => {
                          setModalError("");
                          setCopyTaskStoppedConfirmed(false);
                          setPendingSafetyAction({ kind: "cancel-copy", jobId: job.id });
                        }}
                        onMigrationRetry={() => {
                          setModalError("");
                          setPendingSafetyAction({ kind: "retry-migration", jobId: job.id });
                        }}
                        onMigrationAbandon={() => {
                          setModalError("");
                          setPendingSafetyAction({ kind: "abandon-migration", jobId: job.id });
                        }}
                      />
                    ))}
                  </TableBody>
                </Table>
              </div>
              <div className="grid min-w-0 gap-3 lg:hidden">
                {jobs.map((job) => (
                  <TransferJobItem
                    key={job.id}
                    job={job}
                    sourceName={downloaders.find((item) => item.id === job.downloader_id)?.name}
                    resolving={resolvingJobId === job.id}
                    onCopyRecheck={() => void resolveCopy(job, "recheck")}
                    onCopyCancel={() => {
                      setModalError("");
                      setCopyTaskStoppedConfirmed(false);
                      setPendingSafetyAction({ kind: "cancel-copy", jobId: job.id });
                    }}
                    onMigrationRetry={() => {
                      setModalError("");
                      setPendingSafetyAction({ kind: "retry-migration", jobId: job.id });
                    }}
                    onMigrationAbandon={() => {
                      setModalError("");
                      setPendingSafetyAction({ kind: "abandon-migration", jobId: job.id });
                    }}
                  />
                ))}
              </div>
            </>
          )}

          <div className="flex flex-wrap items-center justify-between gap-3">
            <span className="text-xs text-muted">第 {jobPage} / {jobPageCount} 页</span>
            <div className="flex gap-2">
              <Button variant="outline" disabled={jobPage <= 1} onClick={() => refreshJobs(jobPage - 1)}>上一页</Button>
              <Button variant="outline" disabled={jobPage >= jobPageCount} onClick={() => refreshJobs(jobPage + 1)}>下一页</Button>
            </div>
          </div>
        </CardContent>
      </Card>

      <Dialog
        open={confirmTransferOpen}
        onClose={() => {
          if (!submitting && !planning) {
            setConfirmTransferOpen(false);
            setModalError("");
          }
        }}
        title={targetMode === "tmdb" ? "确认 TMDB 分类目录" : "确认迁移目标"}
        description={targetMode === "tmdb"
          ? "目录已生成。请核对每条种子的分类路径，确认后才会创建迁移任务。"
          : "任务创建后会先复制并核验，再迁移 qB 做种任务。"}
        panelClassName={targetMode === "tmdb" ? "max-w-4xl" : "max-w-xl"}
      >
        <div className="flex flex-col gap-5 p-5 sm:p-6">
          <ModalError message={modalError} />
          {planning ? (
            <div className="flex min-h-40 items-center justify-center gap-2 rounded-lg border border-border bg-surface-container/45 text-sm text-muted">
              <LoaderCircle className="size-4 animate-spin" />正在匹配 TMDB 并生成目录
            </div>
          ) : targetMode === "tmdb" && tmdbPlan ? (
            <>
              <dl className="grid min-w-0 gap-3 rounded-lg border border-border bg-surface-container/45 p-4 text-sm sm:grid-cols-2">
                <SummaryRow label="来源下载器" value={selectedSource?.name ?? "未选择"} />
                <SummaryRow label="目标下载器" value={selectedTargetDownloader?.name ?? "不可用"} />
                <SummaryRow label="OpenList 根目录" value={tmdbPlan.openlist_root} mono />
                <SummaryRow label="qB 根目录" value={tmdbPlan.qb_root} mono />
                <SummaryRow label="已生成目录" value={`${tmdbPlan.directories.length} 个`} />
                <SummaryRow label="种子" value={`${tmdbPlan.items.length} 个 · ${formatBytes(selectedBytes)}`} />
              </dl>
              {tmdbPlan.warnings.length > 0 ? (
                <div className="rounded-lg border border-amber-500/30 bg-amber-500/10 px-4 py-3 text-sm text-foreground">
                  <div className="font-semibold">有 {tmdbPlan.warnings.length} 条需要留意的匹配</div>
                  <ul className="mt-2 max-h-28 list-disc space-y-1 overflow-auto pl-5 text-xs text-muted">
                    {tmdbPlan.warnings.map((warning, index) => <li className="break-words" key={`${index}-${warning}`}>{warning}</li>)}
                  </ul>
                </div>
              ) : null}
              <div className="max-h-[min(28rem,45dvh)] overflow-auto rounded-lg border border-border">
                <Table className="min-w-[48rem]">
                  <TableHeader className="sticky top-0 bg-card">
                    <TableRow>
                      <TableHead>种子</TableHead>
                      <TableHead>TMDB 匹配</TableHead>
                      <TableHead>生成目录</TableHead>
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    {tmdbPlan.items.map((item) => (
                      <TableRow key={item.hash}>
                        <TableCell className="max-w-[18rem]">
                          <div className="truncate font-medium" title={item.torrent_name}>{item.torrent_name}</div>
                          <div className="mt-1 font-mono text-xs text-muted">{item.hash.slice(0, 12)}</div>
                        </TableCell>
                        <TableCell className="text-xs">
                          <div className="font-medium">{item.classification.title}</div>
                          <div className="mt-1 text-muted">
                            {item.classification.category} · {item.classification.genre} · {item.classification.year ?? "年份未知"}
                            {!item.classification.matched ? " · 未匹配" : null}
                          </div>
                        </TableCell>
                        <TableCell className="max-w-[22rem] font-mono text-xs text-muted" title={item.target_openlist_path}>
                          <div className="truncate">{item.target_openlist_path}</div>
                          <div className="mt-1 truncate">{item.target_qb_path}</div>
                        </TableCell>
                      </TableRow>
                    ))}
                  </TableBody>
                </Table>
              </div>
            </>
          ) : (
            <dl className="flex min-w-0 flex-col gap-3 rounded-lg border border-border bg-surface-container/45 p-4 text-sm">
              <SummaryRow label="来源下载器" value={selectedSource?.name ?? "未选择"} />
              <SummaryRow label="目标下载器" value={selectedTargetDownloader?.name ?? "不可用"} />
              <SummaryRow label="OpenList 路径" value={selectedTarget?.openlist_root ?? "未选择"} mono />
              <SummaryRow label="qB 路径" value={selectedTarget?.qb_root ?? "未选择"} mono />
              <SummaryRow label="种子" value={`${selected.size} 个 · ${formatBytes(selectedBytes)}`} />
            </dl>
          )}
          <div className="flex flex-col-reverse gap-2 sm:flex-row sm:justify-end">
            <Button variant="outline" disabled={submitting || planning} onClick={() => setConfirmTransferOpen(false)}>返回修改</Button>
            <Button disabled={submitting || planning || !canSubmit || (targetMode === "tmdb" && !tmdbPlan)} onClick={() => void submit()}>
              {submitting ? <LoaderCircle className="animate-spin" data-icon="inline-start" /> : targetMode === "tmdb" ? <FolderTree data-icon="inline-start" /> : <ArrowRight data-icon="inline-start" />}
              {submitting ? "创建任务中" : targetMode === "tmdb" ? "确认目录并创建迁移" : "确认并创建迁移"}
            </Button>
          </div>
        </div>
      </Dialog>

      <Dialog
        open={pendingSafetyAction?.kind === "cancel-copy" && pendingSafetyJob != null}
        onClose={() => {
          if (resolvingJobId == null) {
            setPendingSafetyAction(null);
            setModalError("");
          }
        }}
        title="安全停止手动迁移任务"
        description={pendingSafetyJob?.stage === "planning_manual_review"
          ? "手动迁移任务尚未提交 OpenList 操作，可以直接停止。"
          : "仅在你已确认，且服务端确认远端任务已终止或已从 OpenList 任务列表移除后释放目标锁；状态未知时会保留锁。"}
        panelClassName="max-w-lg"
      >
        <div className="flex flex-col gap-5 p-5 sm:p-6">
          <ModalError message={modalError} />
          <div className="flex items-start gap-3 rounded-lg border border-destructive/30 bg-destructive/5 p-4 text-sm">
            <ShieldAlert className="mt-0.5 size-5 shrink-0 text-destructive" />
            <div className="min-w-0">
              <div className="break-words font-semibold">{pendingSafetyJob?.torrent_name}</div>
              <p className="mt-1 text-muted">
                {pendingSafetyJob?.stage === "planning_manual_review"
                  ? "停止只会取消这条尚未规划成功的手动迁移任务，不会操作 OpenList 或 qB。"
                  : "停止只会释放本任务的复制锁，不会删除源文件或目标文件，也不会撤销已经完成的复制。"}
              </p>
            </div>
          </div>
          {pendingSafetyJob?.stage !== "planning_manual_review" ? (
            <label className="flex cursor-pointer items-start gap-3 rounded-lg border border-border px-4 py-3 text-sm">
              <input
                type="checkbox"
                className="mt-0.5 size-4 shrink-0 accent-primary"
                checked={copyTaskStoppedConfirmed}
                onChange={(event) => setCopyTaskStoppedConfirmed(event.target.checked)}
              />
              <span>我已在 OpenList 中确认相关复制/建目录任务不再运行，允许释放目标锁。</span>
            </label>
          ) : null}
          <div className="flex flex-col-reverse gap-2 sm:flex-row sm:justify-end">
            <Button variant="outline" disabled={resolvingJobId != null} onClick={() => {
              setPendingSafetyAction(null);
              setModalError("");
            }}>返回</Button>
            <Button
              variant="destructive"
              disabled={!pendingSafetyJob
                || resolvingJobId != null
                || (pendingSafetyJob.stage !== "planning_manual_review" && !copyTaskStoppedConfirmed)}
              onClick={() => pendingSafetyJob && void resolveCopy(
                pendingSafetyJob,
                "cancel",
                copyTaskStoppedConfirmed,
              )}
            >
              {resolvingJobId != null ? <LoaderCircle className="animate-spin" data-icon="inline-start" /> : <ShieldAlert data-icon="inline-start" />}
              {pendingSafetyJob?.stage === "planning_manual_review" ? "停止任务" : "核验并停止"}
            </Button>
          </div>
        </div>
      </Dialog>

      <Dialog
        open={pendingSafetyAction?.kind === "retry-migration" && pendingSafetyJob != null}
        onClose={() => {
          if (resolvingJobId == null) {
            setPendingSafetyAction(null);
            setModalError("");
          }
        }}
        title={pendingSafetyJob?.stage === "source_remove_manual_review" ? "重新核验源文件清理" : "重新开始 qB 迁移"}
        description={pendingSafetyJob?.stage === "source_remove_manual_review"
          ? "先只读核验当前状态；确认目标仍完整后，可能继续清理剩余源文件。"
          : "任务会先重新核对两端状态，再从可证明安全的阶段继续。"}
        panelClassName="max-w-lg"
      >
        <div className="flex flex-col gap-5 p-5 sm:p-6">
          <ModalError message={modalError} />
          <div className="flex items-start gap-3 rounded-lg border border-border bg-surface-container/45 p-4 text-sm">
            <RotateCcw className="mt-0.5 size-5 shrink-0 text-primary" />
            <div className="min-w-0">
              <div className="break-words font-semibold">{pendingSafetyJob?.torrent_name}</div>
              <p className="mt-1 text-muted">
                {pendingSafetyJob?.stage === "source_remove_manual_review"
                  ? "已有的不确定删除请求不会重发。核验通过后，流程可能继续删除尚未处理且未被其他种子引用的源文件；结果仍不确定时会再次停下等待人工处理。"
                  : "重新核验通过后，流程可能移除源 qB 任务，并在目标做种确认后清理未被其他种子引用的源文件。"}
              </p>
            </div>
          </div>
          <div className="flex flex-col-reverse gap-2 sm:flex-row sm:justify-end">
            <Button variant="outline" disabled={resolvingJobId != null} onClick={() => {
              setPendingSafetyAction(null);
              setModalError("");
            }}>暂不重试</Button>
            <Button
              disabled={!pendingSafetyJob || resolvingJobId != null}
              onClick={() => pendingSafetyJob && void resolveMigration(pendingSafetyJob, "retry")}
            >
              {resolvingJobId != null ? <LoaderCircle className="animate-spin" data-icon="inline-start" /> : <RotateCcw data-icon="inline-start" />}
              {pendingSafetyJob?.stage === "source_remove_manual_review" ? "确认继续处理" : "确认重新核验"}
            </Button>
          </div>
        </div>
      </Dialog>

      <Dialog
        open={pendingSafetyAction?.kind === "abandon-migration" && pendingSafetyJob != null}
        onClose={() => {
          if (resolvingJobId == null) {
            setPendingSafetyAction(null);
            setModalError("");
          }
        }}
        title="放弃 qB 迁移"
        description="请根据两端当前实际状态决定是否放弃。"
        panelClassName="max-w-lg"
      >
        <div className="flex flex-col gap-5 p-5 sm:p-6">
          <ModalError message={modalError} />
          <div className="flex items-start gap-3 rounded-lg border border-destructive/30 bg-destructive/5 p-4 text-sm">
            <ShieldAlert className="mt-0.5 size-5 shrink-0 text-destructive" />
            <div className="min-w-0">
              <div className="break-words font-semibold">{pendingSafetyJob?.torrent_name}</div>
              <p className="mt-1 text-muted">
                放弃后系统不会继续删除；此前可能已移除源 qB 任务或部分源文件，目标 qB 任务也可能保留。
              </p>
            </div>
          </div>
          <div className="flex flex-col-reverse gap-2 sm:flex-row sm:justify-end">
            <Button variant="outline" disabled={resolvingJobId != null} onClick={() => {
              setPendingSafetyAction(null);
              setModalError("");
            }}>继续处理</Button>
            <Button
              variant="destructive"
              disabled={!pendingSafetyJob || resolvingJobId != null}
              onClick={() => pendingSafetyJob && void resolveMigration(pendingSafetyJob, "abandon")}
            >
              {resolvingJobId != null ? <LoaderCircle className="animate-spin" data-icon="inline-start" /> : <X data-icon="inline-start" />}
              确认放弃
            </Button>
          </div>
        </div>
      </Dialog>
    </div>
  );
}

function ModalError({ message }: { message: string }) {
  if (!message) return null;
  return (
    <div className="flex items-start gap-2 rounded-lg border border-destructive/30 bg-destructive/10 px-4 py-3 text-sm text-destructive" role="alert" aria-live="assertive">
      <AlertCircle className="mt-0.5 size-4 shrink-0" />
      <span className="min-w-0 break-words">{message}</span>
    </div>
  );
}

function SummaryRow({ label, value, mono = false }: { label: string; value: string; mono?: boolean }) {
  return (
    <div className="grid min-w-0 gap-1 sm:grid-cols-[7rem_minmax(0,1fr)] sm:gap-3">
      <dt className="text-muted">{label}</dt>
      <dd className={cn("min-w-0 break-all font-medium sm:text-right", mono && "font-mono text-xs")}>{value}</dd>
    </div>
  );
}

type TransferJobViewProps = {
  job: TransferJob;
  sourceName?: string;
  resolving: boolean;
  onCopyRecheck: () => void;
  onCopyCancel: () => void;
  onMigrationRetry: () => void;
  onMigrationAbandon: () => void;
};

function TransferJobRow(props: TransferJobViewProps) {
  return (
    <TableRow>
      <TableCell><JobIdentity job={props.job} sourceName={props.sourceName} /></TableCell>
      <TableCell><JobStatus job={props.job} /></TableCell>
      <TableCell><JobPathsAndSafety job={props.job} /></TableCell>
      <TableCell className="whitespace-nowrap text-xs text-muted">{formatDate(props.job.updated_at)}</TableCell>
      <TableCell><JobActions {...props} /></TableCell>
    </TableRow>
  );
}

function TransferJobItem(props: TransferJobViewProps) {
  return (
    <article className="min-w-0 rounded-lg border border-border bg-surface-container/45 p-4">
      <div className="flex min-w-0 flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
        <JobIdentity job={props.job} sourceName={props.sourceName} />
        <JobStatus job={props.job} />
      </div>
      <div className="mt-4"><JobPathsAndSafety job={props.job} /></div>
      <div className="mt-4 flex flex-col gap-3 border-t border-border pt-3 sm:flex-row sm:items-center sm:justify-between">
        <span className="text-xs text-muted">更新于 {formatDate(props.job.updated_at)}</span>
        <JobActions {...props} />
      </div>
    </article>
  );
}

function JobIdentity({ job, sourceName }: { job: TransferJob; sourceName?: string }) {
  return (
    <div className="min-w-0 max-w-72">
      <div className="line-clamp-2 text-sm font-semibold" title={job.torrent_name}>{job.torrent_name}</div>
      <div className="mt-1 truncate text-xs text-muted" title={sourceName}>
        {sourceName ?? `来源下载器 #${job.downloader_id ?? "-"}`} · {job.workflow === "qb_migration" ? "手动 qB 迁移" : "自动复制"}
      </div>
      <div className="mt-1 font-mono text-xs text-muted">{job.infohash.slice(0, 12)} · #{job.id}</div>
    </div>
  );
}

function JobStatus({ job }: { job: TransferJob }) {
  const stage = stageDisplay(job);
  const needsAttention = job.manual_resolution_allowed || job.migration_resolution_allowed
    || ["copy_manual_review", "manifest_required", "qb_manual_review"].includes(job.stage);
  const complete = job.stage === "completed";
  const stopped = job.stage === "cancelled";
  const StatusIcon = complete ? CheckCircle2 : needsAttention ? AlertCircle : stopped ? XCircle : LoaderCircle;
  return (
    <div className="min-w-44 max-w-64 text-xs" aria-live="polite">
      <div className={cn(
        "flex items-center gap-1.5 font-medium",
        complete ? "text-primary" : needsAttention ? "text-destructive" : stopped ? "text-muted" : "text-foreground",
      )}>
        <StatusIcon className={cn("size-4 shrink-0", !complete && !needsAttention && !stopped && "animate-spin")} />
        <span>{stage.label}</span>
      </div>
      <div className="mt-2 h-1.5 overflow-hidden rounded-full bg-surface-container">
        <div
          className={cn(
            "h-full rounded-full transition-[width] duration-300",
            needsAttention ? "bg-destructive" : stopped ? "bg-foreground/30" : "bg-primary",
          )}
          style={{ width: `${stage.progress}%` }}
        />
      </div>
      {job.attempts > 0 ? <div className="mt-1 text-muted">连续失败 {job.attempts} 次</div> : null}
      {job.last_error ? (
        <div className="mt-1 line-clamp-3 text-destructive" title={job.last_error}>{job.last_error}</div>
      ) : null}
    </div>
  );
}

function JobPathsAndSafety({ job }: { job: TransferJob }) {
  return (
    <div className="min-w-0 max-w-xl text-xs">
      <PathLine label="源 qB" value={job.source_qb_path} fallback="等待识别" />
      <PathLine label="源 OpenList" value={job.source_openlist_path} fallback="等待识别" />
      <PathLine label="目标 OpenList" value={job.target_openlist_path} fallback="等待规划" />
      <PathLine label="目标 qB" value={job.target_qb_path} fallback="等待规划" />
      <div className="mt-2 flex flex-wrap gap-x-3 gap-y-1 text-muted">
        {job.copy_lock_acquired ? <span>复制锁已持有</span> : null}
        {job.openlist_task_ids.length > 0 ? <span>OpenList 任务 {job.openlist_task_ids.length} 个</span> : null}
        {job.copy_checkpoint ? <span>{copyOperationLabel(job.copy_checkpoint.operation)}待核验</span> : null}
      </div>
      {job.copy_checkpoint ? (
        <div className="mt-1 truncate text-muted" title={job.copy_checkpoint.path}>
          核验点：{job.copy_checkpoint.path} · {job.copy_checkpoint.phase === "uncertain" ? "结果待确认" : "提交前已记录"}
        </div>
      ) : null}
    </div>
  );
}

function PathLine({ label, value, fallback }: { label: string; value: string; fallback: string }) {
  return (
    <div className="mt-1 flex min-w-0 gap-2 first:mt-0">
      <span className="w-20 shrink-0 text-muted">{label}</span>
      <span className="min-w-0 truncate font-mono" title={value || fallback}>{value || fallback}</span>
    </div>
  );
}

function JobActions({
  job,
  resolving,
  onCopyRecheck,
  onCopyCancel,
  onMigrationRetry,
  onMigrationAbandon,
}: TransferJobViewProps) {
  if (job.migration_resolution_allowed) {
    const reviewingSourceRemoval = job.stage === "source_remove_manual_review";
    return (
      <div className="flex flex-wrap justify-end gap-2">
        <Button variant="outline" className="h-8 px-3 text-xs" disabled={resolving} onClick={onMigrationRetry}>
          {resolving ? <LoaderCircle className="animate-spin" data-icon="inline-start" /> : <RotateCcw data-icon="inline-start" />}
          {reviewingSourceRemoval ? "核验清理结果" : "重试迁移"}
        </Button>
        <Button variant="destructive" className="h-8 px-3 text-xs" disabled={resolving} onClick={onMigrationAbandon}>
          <X data-icon="inline-start" />放弃
        </Button>
      </div>
    );
  }
  if (job.manual_resolution_allowed) {
    const canRecheck = job.copy_resolution_actions.includes("recheck");
    const canCancel = job.copy_resolution_actions.includes("cancel");
    return (
      <div className="flex flex-wrap justify-end gap-2">
        {canRecheck ? (
          <Button variant="outline" className="h-8 px-3 text-xs" disabled={resolving} onClick={onCopyRecheck}>
            {resolving ? <LoaderCircle className="animate-spin" data-icon="inline-start" /> : <RefreshCw data-icon="inline-start" />}
            {copyRecheckLabel(job)}
          </Button>
        ) : null}
        {canCancel ? (
          <Button variant="destructive" className="h-8 px-3 text-xs" disabled={resolving} onClick={onCopyCancel}>
            <ShieldAlert data-icon="inline-start" />安全停止
          </Button>
        ) : null}
      </div>
    );
  }
  if (job.stage === "completed" || job.stage === "cancelled") return <div />;
  return <div className="text-right text-xs text-muted"><Clock3 className="mr-1 inline size-3.5" />处理中</div>;
}
