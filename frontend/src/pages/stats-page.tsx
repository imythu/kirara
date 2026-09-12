import { useEffect, useRef, useState } from "react";
import {
  Activity,
  ArrowUpDown,
  BarChart3,
  Calendar,
  HardDrive,
  RefreshCw,
  TrendingUp,
} from "lucide-react";
import {
  Bar,
  BarChart,
  CartesianGrid,
  Legend,
  Line,
  LineChart,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Select } from "@/components/ui/select";
import { api } from "@/lib/api";
import type {
  DailyTransferItem,
  DownloaderRecord,
  DownloaderSpeedSnapshot,
  StatsOverview,
  TaskOverview,
  TaskStatsSnapshot,
} from "@/types";

type TimeWindow = {
  start: number;
  end: number;
};

/* ---------- helpers ---------- */

function formatBytes(bytes: number): string {
  if (bytes === 0) return "0 B";
  const k = 1024;
  const sizes = ["B", "KB", "MB", "GB", "TB", "PB"];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return `${(bytes / Math.pow(k, i)).toFixed(2)} ${sizes[i]}`;
}

function formatTime(isoString: string): string {
  const d = new Date(isoString);
  return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
}

function formatAxisTime(value: string | number, hours: number): string {
  const d = new Date(value);
  if (Number.isNaN(d.getTime())) {
    return String(value);
  }

  const mm = String(d.getMinutes()).padStart(2, "0");
  const hh = String(d.getHours()).padStart(2, "0");
  const month = d.getMonth() + 1;
  const day = d.getDate();

  if (hours <= 6) {
    return `${hh}:${mm}`;
  }
  if (hours <= 12) {
    return `${hh}:00`;
  }
  if (hours <= 24) {
    return `${hh}:00`;
  }
  if (hours <= 24 * 7) {
    return `${month}/${day}`;
  }
  return `${month}/${day}`;
}

function formatTooltipTime(value: string | number, hours: number): string {
  return formatAxisTime(typeof value === "number" ? value : Number(value) || value, hours);
}

function formatSpeed(bytesPerSec: number): string {
  return `${formatBytes(bytesPerSec)}/s`;
}

function getTimeAxisProps(hours: number) {
  if (hours <= 1) {
    return { tickCount: 7, minTickGap: 24 };
  }
  if (hours <= 6) {
    return { tickCount: 7, minTickGap: 28 };
  }
  if (hours <= 12) {
    return { tickCount: 7, minTickGap: 32 };
  }
  if (hours <= 24) {
    return { tickCount: 9, minTickGap: 36 };
  }
  return { tickCount: 8, minTickGap: 42 };
}

function buildTimeTicks(window: TimeWindow, hours: number): number[] {
  const stepMs =
    hours <= 1
      ? 10 * 60_000
      : hours <= 6
        ? 30 * 60_000
        : hours <= 24
          ? 60 * 60_000
          : 24 * 60 * 60_000;

  const ticks: number[] = [];
  const first = Math.ceil(window.start / stepMs) * stepMs;
  for (let value = first; value <= window.end; value += stepMs) {
    ticks.push(value);
  }
  return ticks.length > 0 ? ticks : [window.start, window.end];
}

function minuteBucket(isoString: string): string {
  const timestamp = new Date(isoString).getTime();
  if (Number.isNaN(timestamp)) {
    return isoString;
  }
  return new Date(Math.floor(timestamp / 60_000) * 60_000).toISOString();
}

function sortSnapshots(snapshots: TaskStatsSnapshot[]): TaskStatsSnapshot[] {
  return [...snapshots].sort(
    (a, b) =>
      new Date(a.recorded_at).getTime() - new Date(b.recorded_at).getTime(),
  );
}

function toTransferDeltaSnapshots(
  snapshots: TaskStatsSnapshot[],
): TaskStatsSnapshot[] {
  const sorted = sortSnapshots(snapshots);
  let previous: TaskStatsSnapshot | null = null;
  return sorted.map((snapshot) => {
    const total_uploaded =
      previous == null
        ? 0
        : Math.max(0, snapshot.total_uploaded - previous.total_uploaded);
    const total_downloaded =
      previous == null
        ? 0
        : Math.max(0, snapshot.total_downloaded - previous.total_downloaded);
    previous = snapshot;
    return {
      ...snapshot,
      total_uploaded,
      total_downloaded,
    };
  });
}

function toTransferGrowthSnapshots(
  snapshots: TaskStatsSnapshot[],
): TaskStatsSnapshot[] {
  let uploadGrowth = 0;
  let downloadGrowth = 0;
  return toTransferDeltaSnapshots(snapshots).map((snapshot) => {
    uploadGrowth += snapshot.total_uploaded;
    downloadGrowth += snapshot.total_downloaded;
    return {
      ...snapshot,
      total_uploaded: uploadGrowth,
      total_downloaded: downloadGrowth,
    };
  });
}

function mergeTransferSnapshotsByMinute(
  snapshotGroups: TaskStatsSnapshot[][],
): TaskStatsSnapshot[] {
  const map = new Map<
    string,
    {
      total_uploaded: number;
      total_downloaded: number;
      torrent_count: number;
    }
  >();

  for (const snapshots of snapshotGroups) {
    for (const snapshot of toTransferDeltaSnapshots(snapshots)) {
      const bucket = minuteBucket(snapshot.recorded_at);
      const existing = map.get(bucket);
      if (existing) {
        existing.total_uploaded += snapshot.total_uploaded;
        existing.total_downloaded += snapshot.total_downloaded;
        existing.torrent_count = Math.max(
          existing.torrent_count,
          snapshot.torrent_count,
        );
      } else {
        map.set(bucket, {
          total_uploaded: snapshot.total_uploaded,
          total_downloaded: snapshot.total_downloaded,
          torrent_count: snapshot.torrent_count,
        });
      }
    }
  }

  return Array.from(map.entries())
    .sort(([a], [b]) => new Date(a).getTime() - new Date(b).getTime())
    .map(([recorded_at, value], index) => ({
      id: index,
      task_id: -1,
      recorded_at,
      ...value,
    }))
    .reduce<TaskStatsSnapshot[]>((acc, snapshot, index) => {
      const previous = acc[index - 1];
      acc.push({
        ...snapshot,
        total_uploaded:
          (previous?.total_uploaded ?? 0) + snapshot.total_uploaded,
        total_downloaded:
          (previous?.total_downloaded ?? 0) + snapshot.total_downloaded,
      });
      return acc;
    }, []);
}

function mergeTorrentSnapshotsByMinute(
  snapshotGroups: TaskStatsSnapshot[][],
): TaskStatsSnapshot[] {
  const map = new Map<
    string,
    {
      total_uploaded: number;
      total_downloaded: number;
      torrent_count: number;
    }
  >();

  for (const snapshots of snapshotGroups) {
    const perMinute = new Map<string, TaskStatsSnapshot>();
    for (const snapshot of sortSnapshots(snapshots)) {
      perMinute.set(minuteBucket(snapshot.recorded_at), snapshot);
    }
    for (const [bucket, snapshot] of perMinute) {
      const existing = map.get(bucket);
      if (existing) {
        existing.torrent_count += snapshot.torrent_count;
      } else {
        map.set(bucket, {
          total_uploaded: 0,
          total_downloaded: 0,
          torrent_count: snapshot.torrent_count,
        });
      }
    }
  }

  return Array.from(map.entries())
    .sort(([a], [b]) => new Date(a).getTime() - new Date(b).getTime())
    .map(([recorded_at, value], index) => ({
      id: index,
      task_id: -1,
      recorded_at,
      ...value,
    }));
}

function ratio(up: number, down: number): string {
  if (down === 0) return up > 0 ? "∞" : "N/A";
  return (up / down).toFixed(2);
}

function withinWindow(timestamp: number, window: TimeWindow): boolean {
  return timestamp >= window.start && timestamp <= window.end;
}

/* ---------- constants ---------- */

const TIME_RANGES = [
  { label: "1h", hours: 1 },
  { label: "6h", hours: 6 },
  { label: "12h", hours: 12 },
  { label: "24h", hours: 24 },
  { label: "7d", hours: 168 },
] as const;

const REFRESH_OPTIONS = [
  { label: "不刷新", value: 0 },
  { label: "3s", value: 3 },
  { label: "5s", value: 5 },
  { label: "10s", value: 10 },
  { label: "60s", value: 60 },
] as const;

const COLORS = {
  upload: "#10b981",
  download: "#0ea5e9",
  torrent: "#99651d",
  grid: "#e5e7eb",
} as const;

/* ---------- date range helpers ---------- */

function toDateInput(ts: number): string {
  const d = new Date(ts);
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
}

function fromDateInput(value: string, endOfDay: boolean): number {
  const d = new Date(value + "T00:00:00");
  if (endOfDay) d.setHours(23, 59, 59, 999);
  return d.getTime();
}

type DateRangeMode = "quick" | "custom";

/* ---------- reusable time range controls ---------- */

function TimeRangeControls({
  mode,
  setMode,
  quickHours,
  setQuickHours,
  customStart,
  customEnd,
  setCustomStart,
  setCustomEnd,
  onApply,
  refreshSecs,
  setRefreshSecs,
  lineFilter,
  setLineFilter,
  showLineFilter,
}: {
  mode: DateRangeMode;
  setMode: (m: DateRangeMode) => void;
  quickHours: number;
  setQuickHours: (h: number) => void;
  customStart: string;
  customEnd: string;
  setCustomStart: (s: string) => void;
  setCustomEnd: (s: string) => void;
  onApply: () => void;
  refreshSecs: number;
  setRefreshSecs: (s: number) => void;
  lineFilter?: "both" | "upload" | "download";
  setLineFilter?: (f: "both" | "upload" | "download") => void;
  showLineFilter?: boolean;
}) {
  return (
    <div className="flex flex-wrap items-center justify-between gap-2 bg-surface-container/50 p-1.5 rounded-xl border border-border/50">
      <div className="flex items-center gap-1">
        <button
          className={`h-7 px-2.5 rounded-lg text-[10px] font-medium transition-colors ${
            mode === "quick" ? "bg-primary/10 text-primary" : "text-muted-foreground hover:bg-surface-container/80"
          }`}
          onClick={() => setMode("quick")}
        >
          <Calendar className="h-3 w-3 inline mr-1" />
          快捷
        </button>
        <button
          className={`h-7 px-2.5 rounded-lg text-[10px] font-medium transition-colors ${
            mode === "custom" ? "bg-primary/10 text-primary" : "text-muted-foreground hover:bg-surface-container/80"
          }`}
          onClick={() => setMode("custom")}
        >
          <Calendar className="h-3 w-3 inline mr-1" />
          日期范围
        </button>
      </div>

      {mode === "quick" ? (
        <div className="flex gap-1">
          {TIME_RANGES.map((r) => (
            <button
              key={r.label}
              className={`h-7 px-2.5 rounded-lg text-[10px] font-medium transition-colors ${
                quickHours === r.hours
                  ? "bg-primary text-primary-foreground shadow-sm"
                  : "hover:bg-surface-container/80 text-muted-foreground"
              }`}
              onClick={() => setQuickHours(r.hours)}
            >
              {r.label}
            </button>
          ))}
        </div>
      ) : (
        <div className="flex items-center gap-1.5">
          <input
            type="date"
            className="h-7 rounded-lg border border-border bg-input px-2 text-[10px]"
            value={customStart}
            onChange={(e) => setCustomStart(e.target.value)}
          />
          <span className="text-[10px] text-muted">至</span>
          <input
            type="date"
            className="h-7 rounded-lg border border-border bg-input px-2 text-[10px]"
            value={customEnd}
            onChange={(e) => setCustomEnd(e.target.value)}
          />
          <Button size="sm" className="h-7 text-[10px] px-2" onClick={onApply}>
            查询
          </Button>
        </div>
      )}

      <div className="h-4 w-[1px] bg-border/50 mx-1 hidden sm:block" />
      {showLineFilter && lineFilter && setLineFilter && (
        <div className="flex gap-1">
          {(["both", "upload", "download"] as const).map((f) => (
            <button
              key={f}
              className={`h-7 px-2.5 rounded-lg text-[10px] font-medium transition-colors ${
                lineFilter === f
                  ? "bg-surface-container-highest text-foreground shadow-sm ring-1 ring-border"
                  : "hover:bg-surface-container/80 text-muted-foreground"
              }`}
              onClick={() => setLineFilter(f)}
            >
              {f === "both" ? "全部" : f === "upload" ? "上传" : "下载"}
            </button>
          ))}
        </div>
      )}
      <div className="flex items-center gap-2">
        <Select
          value={String(refreshSecs)}
          onChange={(val) => setRefreshSecs(Number(val))}
          options={REFRESH_OPTIONS.map((o) => ({ value: String(o.value), label: o.label }))}
        />
      </div>
    </div>
  );
}

/* ---------- component ---------- */

export function StatsPage() {
  const [overview, setOverview] = useState<StatsOverview | null>(null);
  const [selectedTransferTaskId, setSelectedTransferTaskId] = useState<number | -1>(-1);
  const [transferLineFilter, setTransferLineFilter] = useState<"both" | "upload" | "download">("both");
  const [transferTrendHours, setTransferTrendHours] = useState(24);
  const [transferRefreshSecs, setTransferRefreshSecs] = useState(0);
  const [transferSnapshots, setTransferSnapshots] = useState<TaskStatsSnapshot[]>([]);
  const [transferTimeWindow, setTransferTimeWindow] = useState<TimeWindow | null>(null);
  const [transferRangeMode, setTransferRangeMode] = useState<DateRangeMode>("quick");
  const [transferCustomStart, setTransferCustomStart] = useState(toDateInput(Date.now() - 24 * 60 * 60_000));
  const [transferCustomEnd, setTransferCustomEnd] = useState(toDateInput(Date.now()));
  const [transferCustomSince, setTransferCustomSince] = useState<string | null>(null);
  const [transferCustomUntil, setTransferCustomUntil] = useState<string | null>(null);
  const [selectedTorrentTaskId, setSelectedTorrentTaskId] = useState<number | -1>(-1);
  const [torrentTrendHours, setTorrentTrendHours] = useState(24);
  const [torrentRefreshSecs, setTorrentRefreshSecs] = useState(0);
  const [torrentSnapshots, setTorrentSnapshots] = useState<TaskStatsSnapshot[]>([]);
  const [torrentTimeWindow, setTorrentTimeWindow] = useState<TimeWindow | null>(null);
  const [torrentRangeMode, setTorrentRangeMode] = useState<DateRangeMode>("quick");
  const [torrentCustomStart, setTorrentCustomStart] = useState(toDateInput(Date.now() - 24 * 60 * 60_000));
  const [torrentCustomEnd, setTorrentCustomEnd] = useState(toDateInput(Date.now()));
  const [torrentCustomSince, setTorrentCustomSince] = useState<string | null>(null);
  const [torrentCustomUntil, setTorrentCustomUntil] = useState<string | null>(null);
  const [downloaders, setDownloaders] = useState<DownloaderRecord[]>([]);
  const [selectedDownloaderId, setSelectedDownloaderId] = useState<number | -1>(-1);
  const [downloaderLineFilter, setDownloaderLineFilter] = useState<"both" | "upload" | "download">("both");
  const [downloaderTrendHours, setDownloaderTrendHours] = useState(24);
  const [downloaderRefreshSecs, setDownloaderRefreshSecs] = useState(0);
  const [downloaderSnapshots, setDownloaderSnapshots] = useState<DownloaderSpeedSnapshot[]>([]);
  const [downloaderTimeWindow, setDownloaderTimeWindow] = useState<TimeWindow | null>(null);
  const [downloaderRangeMode, setDownloaderRangeMode] = useState<DateRangeMode>("quick");
  const [downloaderCustomStart, setDownloaderCustomStart] = useState(toDateInput(Date.now() - 24 * 60 * 60_000));
  const [downloaderCustomEnd, setDownloaderCustomEnd] = useState(toDateInput(Date.now()));
  const [downloaderCustomSince, setDownloaderCustomSince] = useState<string | null>(null);
  const [downloaderCustomUntil, setDownloaderCustomUntil] = useState<string | null>(null);
  // Daily transfer chart
  const [dailyTaskId, setDailyTaskId] = useState<number | -1>(-1);
  const [dailyRangeMode, setDailyRangeMode] = useState<DateRangeMode>("quick");
  const [dailyQuickDays, setDailyQuickDays] = useState(7);
  const [dailyCustomStart, setDailyCustomStart] = useState(toDateInput(Date.now() - 7 * 24 * 60 * 60_000));
  const [dailyCustomEnd, setDailyCustomEnd] = useState(toDateInput(Date.now()));
  const [dailySince, setDailySince] = useState<string | null>(null);
  const [dailyUntil, setDailyUntil] = useState<string | null>(null);
  const [dailyData, setDailyData] = useState<DailyTransferItem[]>([]);
  const [dailyLoading, setDailyLoading] = useState(false);
  const [dailyLineFilter, setDailyLineFilter] = useState<"both" | "upload" | "download">("both");
  const [loading, setLoading] = useState(true);
  const [transferTrendLoading, setTransferTrendLoading] = useState(false);
  const [torrentTrendLoading, setTorrentTrendLoading] = useState(false);
  const [downloaderTrendLoading, setDownloaderTrendLoading] = useState(false);
  const timerRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const transferRefreshRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const torrentRefreshRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const downloaderRefreshRef = useRef<ReturnType<typeof setInterval> | null>(null);

  // Fetch overview
  const fetchOverview = async () => {
    try {
      const data = await api<StatsOverview>("/api/stats/overview");
      setOverview(data);
    } catch {
      /* silently ignore – overview card will stay empty */
    }
  };

  const fetchDownloaders = async () => {
    try {
      const data = await api<DownloaderRecord[]>("/api/downloaders");
      setDownloaders(data);
    } catch {
      setDownloaders([]);
    }
  };

  const loadData = async () => {
    setLoading(true);
    await Promise.all([fetchOverview(), fetchDownloaders()]);
    setLoading(false);
  };

  // Fetch trend data for selected task(s)
  const fetchTaskTrend = async (
    taskId: number | -1,
    h: number,
    mode: "transfer" | "torrent",
    setWindow: React.Dispatch<React.SetStateAction<TimeWindow | null>>,
    setData: React.Dispatch<React.SetStateAction<TaskStatsSnapshot[]>>,
    setLoadingState: React.Dispatch<React.SetStateAction<boolean>>,
    customSince?: string | null,
    customUntil?: string | null,
  ) => {
    setLoadingState(true);
    try {
      let end: number;
      let visibleStart: number;
      let since: string;
      let until: string;

      if (customSince && customUntil) {
        visibleStart = new Date(customSince).getTime();
        end = new Date(customUntil).getTime();
        const fetchStart = visibleStart - 2 * 60_000;
        setWindow({ start: visibleStart, end });
        since = new Date(fetchStart).toISOString();
        until = new Date(end).toISOString();
      } else {
        end = Date.now();
        visibleStart = end - h * 60 * 60_000;
        const fetchStart = visibleStart - 2 * 60_000;
        setWindow({ start: visibleStart, end });
        since = new Date(fetchStart).toISOString();
        until = new Date(end).toISOString();
      }

      if (taskId === -1) {
        if (!overview || overview.tasks.length === 0) {
          setData([]);
          return;
        }
        const allData = await Promise.all(
          overview.tasks.map((t) =>
            api<TaskStatsSnapshot[]>(
              `/api/stats/trend?task_id=${t.task_id}&since=${encodeURIComponent(since)}&until=${encodeURIComponent(until)}`,
            ),
          ),
        );
        const merged =
          mode === "transfer"
            ? mergeTransferSnapshotsByMinute(allData)
            : mergeTorrentSnapshotsByMinute(allData);
        setData(merged);
      } else {
        const data = await api<TaskStatsSnapshot[]>(
          `/api/stats/trend?task_id=${taskId}&since=${encodeURIComponent(since)}&until=${encodeURIComponent(until)}`,
        );
        setData(
          mode === "transfer"
            ? toTransferGrowthSnapshots(data)
            : sortSnapshots(data),
        );
      }
    } catch {
      setData([]);
    } finally {
      setLoadingState(false);
    }
  };

  const fetchDownloaderTrend = async (
    downloaderId: number | -1,
    h: number,
    customSince?: string | null,
    customUntil?: string | null,
  ) => {
    setDownloaderTrendLoading(true);
    try {
      let end: number;
      let visibleStart: number;
      let since: string;
      let until: string;

      if (customSince && customUntil) {
        visibleStart = new Date(customSince).getTime();
        end = new Date(customUntil).getTime();
        const fetchStart = visibleStart - 2 * 60_000;
        setDownloaderTimeWindow({ start: visibleStart, end });
        since = new Date(fetchStart).toISOString();
        until = new Date(end).toISOString();
      } else {
        end = Date.now();
        visibleStart = end - h * 60 * 60_000;
        const fetchStart = visibleStart - 2 * 60_000;
        setDownloaderTimeWindow({ start: visibleStart, end });
        since = new Date(fetchStart).toISOString();
        until = new Date(end).toISOString();
      }

      // For custom date ranges, compute hours for bucket selection
      const effectiveHours = customSince && customUntil
        ? Math.max(1, Math.ceil((end - visibleStart) / (60 * 60_000)))
        : h;

      if (downloaderId === -1) {
        if (downloaders.length === 0) {
          setDownloaderSnapshots([]);
          return;
        }
        const allData = await Promise.all(
          downloaders.map((downloader) =>
            api<DownloaderSpeedSnapshot[]>(
              `/api/stats/downloader-speed-trend?downloader_id=${downloader.id}&hours=${effectiveHours}&since=${encodeURIComponent(since)}&until=${encodeURIComponent(until)}`,
            ),
          ),
        );
        // Backend already aggregates by time bucket for > 1h ranges,
        // so we just sum across downloaders per bucket.
        // For ≤ 1h (raw data), average within same minute per downloader first.
        const needsMinuteDedup = effectiveHours <= 1;
        const perDownloaderBuckets = new Map<string, Map<string, { sum_up: number; sum_down: number; count: number }>>();
        for (const arr of allData) {
          for (const snapshot of arr) {
            const did = String(snapshot.downloader_id);
            const bucket = minuteBucket(snapshot.recorded_at);
            let byMinute = perDownloaderBuckets.get(did);
            if (!byMinute) {
              byMinute = new Map();
              perDownloaderBuckets.set(did, byMinute);
            }
            const existing = byMinute.get(bucket);
            if (existing) {
              existing.sum_up += snapshot.upload_speed;
              existing.sum_down += snapshot.download_speed;
              existing.count += 1;
            } else {
              byMinute.set(bucket, {
                sum_up: snapshot.upload_speed,
                sum_down: snapshot.download_speed,
                count: 1,
              });
            }
          }
        }
        const map = new Map<string, { upload_speed: number; download_speed: number }>();
        for (const byMinute of perDownloaderBuckets.values()) {
          for (const [bucket, agg] of byMinute) {
            const avgUp = needsMinuteDedup ? agg.sum_up / agg.count : agg.sum_up;
            const avgDown = needsMinuteDedup ? agg.sum_down / agg.count : agg.sum_down;
            const existing = map.get(bucket);
            if (existing) {
              existing.upload_speed += avgUp;
              existing.download_speed += avgDown;
            } else {
              map.set(bucket, {
                upload_speed: avgUp,
                download_speed: avgDown,
              });
            }
          }
        }
        const merged: DownloaderSpeedSnapshot[] = Array.from(map.entries())
          .sort(([a], [b]) => new Date(a).getTime() - new Date(b).getTime())
          .map(([recorded_at, value], index) => ({
            id: index,
            downloader_id: -1,
            upload_speed: value.upload_speed,
            download_speed: value.download_speed,
            recorded_at,
          }));
        setDownloaderSnapshots(merged);
      } else {
        const data = await api<DownloaderSpeedSnapshot[]>(
          `/api/stats/downloader-speed-trend?downloader_id=${downloaderId}&hours=${effectiveHours}&since=${encodeURIComponent(since)}&until=${encodeURIComponent(until)}`,
        );
        setDownloaderSnapshots(data);
      }
    } catch {
      setDownloaderSnapshots([]);
    } finally {
      setDownloaderTrendLoading(false);
    }
  };

  // Initial load & auto-refresh
  useEffect(() => {
    void loadData();

    timerRef.current = setInterval(() => {
      void fetchOverview();
    }, 30_000);

    return () => {
      if (timerRef.current) clearInterval(timerRef.current);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    if (overview) {
      const cs = transferRangeMode === "custom" ? transferCustomSince : null;
      const cu = transferRangeMode === "custom" ? transferCustomUntil : null;
      void fetchTaskTrend(selectedTransferTaskId, transferTrendHours, "transfer", setTransferTimeWindow, setTransferSnapshots, setTransferTrendLoading, cs, cu);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedTransferTaskId, transferTrendHours, overview, transferRangeMode, transferCustomSince, transferCustomUntil]);

  useEffect(() => {
    if (overview) {
      const cs = torrentRangeMode === "custom" ? torrentCustomSince : null;
      const cu = torrentRangeMode === "custom" ? torrentCustomUntil : null;
      void fetchTaskTrend(selectedTorrentTaskId, torrentTrendHours, "torrent", setTorrentTimeWindow, setTorrentSnapshots, setTorrentTrendLoading, cs, cu);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedTorrentTaskId, torrentTrendHours, overview, torrentRangeMode, torrentCustomSince, torrentCustomUntil]);

  useEffect(() => {
    if (downloaders.length > 0 || selectedDownloaderId === -1) {
      const cs = downloaderRangeMode === "custom" ? downloaderCustomSince : null;
      const cu = downloaderRangeMode === "custom" ? downloaderCustomUntil : null;
      void fetchDownloaderTrend(selectedDownloaderId, downloaderTrendHours, cs, cu);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedDownloaderId, downloaderTrendHours, downloaders, downloaderRangeMode, downloaderCustomSince, downloaderCustomUntil]);

  // Daily transfer fetch
  useEffect(() => {
    const fetchDaily = async () => {
      setDailyLoading(true);
      try {
        let since: string;
        let until: string;
        if (dailyRangeMode === "custom" && dailySince && dailyUntil) {
          since = dailySince;
          until = dailyUntil;
        } else {
          const end = new Date();
          end.setHours(23, 59, 59, 999);
          const start = new Date(end);
          start.setDate(start.getDate() - dailyQuickDays + 1);
          start.setHours(0, 0, 0, 0);
          since = start.toISOString();
          until = end.toISOString();
        }
        const params = new URLSearchParams({ since, until });
        if (dailyTaskId !== -1) params.set("task_id", String(dailyTaskId));
        const data = await api<DailyTransferItem[]>(`/api/stats/daily-transfer?${params}`);
        setDailyData(data);
      } catch {
        setDailyData([]);
      } finally {
        setDailyLoading(false);
      }
    };
    void fetchDaily();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [dailyTaskId, dailyRangeMode, dailyQuickDays, dailySince, dailyUntil]);

  useEffect(() => {
    if (transferRefreshRef.current) clearInterval(transferRefreshRef.current);
    if (transferRefreshSecs > 0 && transferRangeMode === "quick") {
      transferRefreshRef.current = setInterval(() => {
        void fetchTaskTrend(selectedTransferTaskId, transferTrendHours, "transfer", setTransferTimeWindow, setTransferSnapshots, setTransferTrendLoading);
      }, transferRefreshSecs * 1000);
    }
    return () => {
      if (transferRefreshRef.current) clearInterval(transferRefreshRef.current);
    };
  }, [selectedTransferTaskId, transferTrendHours, transferRefreshSecs, overview, transferRangeMode]);

  useEffect(() => {
    if (torrentRefreshRef.current) clearInterval(torrentRefreshRef.current);
    if (torrentRefreshSecs > 0 && torrentRangeMode === "quick") {
      torrentRefreshRef.current = setInterval(() => {
        void fetchTaskTrend(selectedTorrentTaskId, torrentTrendHours, "torrent", setTorrentTimeWindow, setTorrentSnapshots, setTorrentTrendLoading);
      }, torrentRefreshSecs * 1000);
    }
    return () => {
      if (torrentRefreshRef.current) clearInterval(torrentRefreshRef.current);
    };
  }, [selectedTorrentTaskId, torrentTrendHours, torrentRefreshSecs, overview, torrentRangeMode]);

  useEffect(() => {
    if (downloaderRefreshRef.current) clearInterval(downloaderRefreshRef.current);
    if (downloaderRefreshSecs > 0 && downloaderRangeMode === "quick") {
      downloaderRefreshRef.current = setInterval(() => {
        void fetchDownloaderTrend(selectedDownloaderId, downloaderTrendHours);
      }, downloaderRefreshSecs * 1000);
    }
    return () => {
      if (downloaderRefreshRef.current) clearInterval(downloaderRefreshRef.current);
    };
  }, [selectedDownloaderId, downloaderTrendHours, downloaderRefreshSecs, downloaders, downloaderRangeMode]);

  /* ---------- chart data ---------- */

  /* ---------- render ---------- */

  if (loading) {
    return (
      <div className="flex items-center justify-center py-24 text-sm text-muted">
        <RefreshCw className="mr-2 h-4 w-4 animate-spin" />
        加载统计数据…
      </div>
    );
  }

  const tasks = overview?.tasks ?? [];
  const transferAxisProps = getTimeAxisProps(transferTrendHours);
  const torrentAxisProps = getTimeAxisProps(torrentTrendHours);
  const downloaderAxisProps = getTimeAxisProps(downloaderTrendHours);
  const currentTransferWindow = transferTimeWindow ?? {
    start: Date.now() - transferTrendHours * 60 * 60_000,
    end: Date.now(),
  };
  const currentTorrentWindow = torrentTimeWindow ?? {
    start: Date.now() - torrentTrendHours * 60 * 60_000,
    end: Date.now(),
  };
  const currentDownloaderWindow = downloaderTimeWindow ?? {
    start: Date.now() - downloaderTrendHours * 60 * 60_000,
    end: Date.now(),
  };
  const transferTicks = buildTimeTicks(currentTransferWindow, transferTrendHours);
  const torrentTicks = buildTimeTicks(currentTorrentWindow, torrentTrendHours);
  const downloaderTicks = buildTimeTicks(currentDownloaderWindow, downloaderTrendHours);
  const transferData = transferSnapshots
    .map((s) => ({
      recordedAt: s.recorded_at,
      timestamp: new Date(s.recorded_at).getTime(),
      upload: s.total_uploaded,
      download: s.total_downloaded,
    }))
    .filter((item) => withinWindow(item.timestamp, currentTransferWindow));
  const torrentData = torrentSnapshots
    .map((s) => ({
      recordedAt: s.recorded_at,
      timestamp: new Date(s.recorded_at).getTime(),
      count: s.torrent_count,
    }))
    .filter((item) => withinWindow(item.timestamp, currentTorrentWindow));
  const downloaderSpeedData = downloaderSnapshots
    .map((s) => ({
      recordedAt: s.recorded_at,
      timestamp: new Date(s.recorded_at).getTime(),
      uploadSpeed: s.upload_speed,
      downloadSpeed: s.download_speed,
    }))
    .filter((item) => withinWindow(item.timestamp, currentDownloaderWindow));

  return (
    <div className="space-y-6">
      {/* ===== Overview Section ===== */}
      <Card className="rounded-xl border-border bg-surface-container/30 shadow-sm overflow-hidden">
        <CardHeader className="pb-2">
          <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
            <div>
              <CardTitle className="flex items-center gap-2 text-lg">
                <BarChart3 className="h-5 w-5 text-primary" />
                任务概览
              </CardTitle>
              <CardDescription className="text-[11px]">
                各刷流任务的实时汇总数据，每 30 秒自动刷新。
              </CardDescription>
            </div>
            <Button
              variant="outline"
              size="sm"
              className="h-8 text-[11px] px-3 w-fit"
              onClick={loadData}
            >
              <RefreshCw className="mr-1.5 h-3.5 w-3.5" />
              刷新全部
            </Button>
          </div>
        </CardHeader>
        <CardContent className="p-4 pt-2">
          {tasks.length === 0 ? (
            <div className="rounded-2xl border border-dashed border-border bg-surface-container/60 p-8 text-center text-[11px] text-muted">
              暂无任务统计数据。
            </div>
          ) : (
            <div className="grid gap-3 grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4">
              {tasks.map((t) => (
                <TaskOverviewCard key={t.task_id} task={t} />
              ))}
            </div>
          )}
        </CardContent>
      </Card>

      {/* ===== Global Controls for Trends ===== */}
      <div className="grid grid-cols-1 lg:grid-cols-2 gap-6">
        {/* Upload/Download Trend */}
        <Card className="rounded-xl border-border bg-surface-container/30 shadow-sm overflow-hidden">
          <CardHeader className="pb-2">
            <div className="flex flex-wrap items-center justify-between gap-3">
              <div className="flex items-center gap-2">
                <div className="p-2 rounded-xl bg-primary/10">
                  <ArrowUpDown className="h-4 w-4 text-primary" />
                </div>
                <div>
                  <CardTitle className="text-sm font-semibold">上传 / 下载趋势</CardTitle>
                  <CardDescription className="text-[10px]">增量数据视图</CardDescription>
                </div>
              </div>
              <div className="flex items-center gap-2">
                <Select
                  value={String(selectedTransferTaskId)}
                  onChange={(val) => setSelectedTransferTaskId(Number(val))}
                  options={[
                    { value: "-1", label: "全部任务" },
                    ...tasks.map((t) => ({ value: String(t.task_id), label: t.task_name })),
                  ]}
                />
              </div>
            </div>
          </CardHeader>
          <CardContent className="p-4 pt-2 space-y-4">
            <TimeRangeControls
              mode={transferRangeMode}
              setMode={setTransferRangeMode}
              quickHours={transferTrendHours}
              setQuickHours={setTransferTrendHours}
              customStart={transferCustomStart}
              customEnd={transferCustomEnd}
              setCustomStart={setTransferCustomStart}
              setCustomEnd={setTransferCustomEnd}
              onApply={() => {
                setTransferCustomSince(new Date(fromDateInput(transferCustomStart, false)).toISOString());
                setTransferCustomUntil(new Date(fromDateInput(transferCustomEnd, true)).toISOString());
              }}
              refreshSecs={transferRefreshSecs}
              setRefreshSecs={setTransferRefreshSecs}
              lineFilter={transferLineFilter}
              setLineFilter={setTransferLineFilter}
              showLineFilter
            />

            <div className="h-[280px] w-full mt-2">
              {transferTrendLoading && transferData.length === 0 ? (
                <div className="flex flex-col items-center justify-center h-full text-muted text-[11px]">
                  <RefreshCw className="mb-2 h-4 w-4 animate-spin" />
                  加载中...
                </div>
              ) : transferData.length === 0 ? (
                <div className="flex items-center justify-center h-full rounded-2xl border border-dashed border-border bg-surface-container/20 text-[11px] text-muted">
                  无数据
                </div>
              ) : (
                <ResponsiveContainer width="100%" height="100%">
                  <LineChart data={transferData}>
                    <CartesianGrid strokeDasharray="3 3" stroke={COLORS.grid} vertical={false} />
                    <XAxis
                      dataKey="timestamp"
                      type="number"
                      scale="time"
                      domain={[currentTransferWindow.start, currentTransferWindow.end]}
                      ticks={transferTicks}
                      tick={{ fontSize: 9, fill: "#746780" }}
                      tickLine={false}
                      axisLine={false}
                      tickFormatter={(value: number) => formatAxisTime(value, transferTrendHours)}
                    />
                    <YAxis
                      tick={{ fontSize: 9, fill: "#746780" }}
                      tickLine={false}
                      axisLine={false}
                      tickFormatter={(v: number) => formatBytes(v)}
                      width={45}
                    />
                    <Tooltip
                      contentStyle={{
                        backgroundColor: "rgba(255, 255, 255, 0.8)",
                        backdropFilter: "blur(8px)",
                        borderRadius: "12px",
                        border: "1px solid rgba(0,0,0,0.05)",
                        fontSize: "11px",
                        boxShadow: "0 10px 15px -3px rgba(0, 0, 0, 0.1)",
                      }}
                      formatter={(value, name) => [
                        formatBytes(Number(value)),
                        name === "upload" ? "上传" : "下载",
                      ]}
                      labelFormatter={(label) => `时间: ${formatTooltipTime(label, transferTrendHours)}`}
                    />
                    <Line
                      type="monotone"
                      dataKey="upload"
                      stroke={COLORS.upload}
                      strokeWidth={2}
                      dot={false}
                      activeDot={{ r: 4, strokeWidth: 0 }}
                      hide={transferLineFilter === "download"}
                    />
                    <Line
                      type="monotone"
                      dataKey="download"
                      stroke={COLORS.download}
                      strokeWidth={2}
                      dot={false}
                      activeDot={{ r: 4, strokeWidth: 0 }}
                      hide={transferLineFilter === "upload"}
                    />
                  </LineChart>
                </ResponsiveContainer>
              )}
            </div>
          </CardContent>
        </Card>

        {/* Torrent Count Trend */}
        <Card className="rounded-xl border-border bg-surface-container/30 shadow-sm overflow-hidden">
          <CardHeader className="pb-2">
            <div className="flex flex-wrap items-center justify-between gap-3">
              <div className="flex items-center gap-2">
                <div className="p-2 rounded-xl bg-violet-500/10">
                  <HardDrive className="h-4 w-4 text-violet-500" />
                </div>
                <div>
                  <CardTitle className="text-sm font-semibold">种子数趋势</CardTitle>
                  <CardDescription className="text-[10px]">各任务活跃数</CardDescription>
                </div>
              </div>
              <div className="flex items-center gap-2">
                <Select
                  value={String(selectedTorrentTaskId)}
                  onChange={(val) => setSelectedTorrentTaskId(Number(val))}
                  options={[
                    { value: "-1", label: "全部任务" },
                    ...tasks.map((t) => ({ value: String(t.task_id), label: t.task_name })),
                  ]}
                />
              </div>
            </div>
          </CardHeader>
          <CardContent className="p-4 pt-2 space-y-4">
            <TimeRangeControls
              mode={torrentRangeMode}
              setMode={setTorrentRangeMode}
              quickHours={torrentTrendHours}
              setQuickHours={setTorrentTrendHours}
              customStart={torrentCustomStart}
              customEnd={torrentCustomEnd}
              setCustomStart={setTorrentCustomStart}
              setCustomEnd={setTorrentCustomEnd}
              onApply={() => {
                setTorrentCustomSince(new Date(fromDateInput(torrentCustomStart, false)).toISOString());
                setTorrentCustomUntil(new Date(fromDateInput(torrentCustomEnd, true)).toISOString());
              }}
              refreshSecs={torrentRefreshSecs}
              setRefreshSecs={setTorrentRefreshSecs}
            />

            <div className="h-[280px] w-full mt-2">
              {torrentTrendLoading && torrentData.length === 0 ? (
                <div className="flex flex-col items-center justify-center h-full text-muted text-[11px]">
                  <RefreshCw className="mb-2 h-4 w-4 animate-spin" />
                  加载中...
                </div>
              ) : torrentData.length === 0 ? (
                <div className="flex items-center justify-center h-full rounded-2xl border border-dashed border-border bg-surface-container/20 text-[11px] text-muted">
                  无数据
                </div>
              ) : (
                <ResponsiveContainer width="100%" height="100%">
                  <LineChart data={torrentData}>
                    <CartesianGrid strokeDasharray="3 3" stroke={COLORS.grid} vertical={false} />
                    <XAxis
                      dataKey="timestamp"
                      type="number"
                      scale="time"
                      domain={[currentTorrentWindow.start, currentTorrentWindow.end]}
                      ticks={torrentTicks}
                      tick={{ fontSize: 9, fill: "#746780" }}
                      tickLine={false}
                      axisLine={false}
                      tickFormatter={(value: number) => formatAxisTime(value, torrentTrendHours)}
                    />
                    <YAxis
                      tick={{ fontSize: 9, fill: "#746780" }}
                      tickLine={false}
                      axisLine={false}
                      width={30}
                    />
                    <Tooltip
                      contentStyle={{
                        backgroundColor: "rgba(255, 255, 255, 0.8)",
                        backdropFilter: "blur(8px)",
                        borderRadius: "12px",
                        border: "1px solid rgba(0,0,0,0.05)",
                        fontSize: "11px",
                        boxShadow: "0 10px 15px -3px rgba(0, 0, 0, 0.1)",
                      }}
                      formatter={(value) => [String(value), "种子数"]}
                      labelFormatter={(label) => `时间: ${formatTooltipTime(label, torrentTrendHours)}`}
                    />
                    <Line
                      type="monotone"
                      dataKey="count"
                      stroke={COLORS.torrent}
                      strokeWidth={2}
                      dot={false}
                      activeDot={{ r: 4, strokeWidth: 0 }}
                    />
                  </LineChart>
                </ResponsiveContainer>
              )}
            </div>
          </CardContent>
        </Card>
      </div>

      {/* ===== Downloader Speed Trend ===== */}
      <Card className="rounded-xl border-border bg-surface-container/30 shadow-sm overflow-hidden">
        <CardHeader className="pb-2">
          <div className="flex flex-wrap items-center justify-between gap-3">
            <div className="flex items-center gap-2">
              <div className="p-2 rounded-xl bg-sky-500/10">
                <Activity className="h-5 w-5 text-sky-500" />
              </div>
              <div>
                <CardTitle className="text-sm font-semibold">下载器实时速度</CardTitle>
                <CardDescription className="text-[10px]">各下载器总宽带占用</CardDescription>
              </div>
            </div>
            <div className="flex items-center gap-2">
              <Select
                value={String(selectedDownloaderId)}
                onChange={(val) => setSelectedDownloaderId(Number(val))}
                options={[
                  { value: "-1", label: "全部下载器" },
                  ...downloaders.map((d) => ({ value: String(d.id), label: d.name })),
                ]}
              />
            </div>
          </div>
        </CardHeader>
        <CardContent className="p-4 pt-2 space-y-4">
          <TimeRangeControls
            mode={downloaderRangeMode}
            setMode={setDownloaderRangeMode}
            quickHours={downloaderTrendHours}
            setQuickHours={setDownloaderTrendHours}
            customStart={downloaderCustomStart}
            customEnd={downloaderCustomEnd}
            setCustomStart={setDownloaderCustomStart}
            setCustomEnd={setDownloaderCustomEnd}
            onApply={() => {
              setDownloaderCustomSince(new Date(fromDateInput(downloaderCustomStart, false)).toISOString());
              setDownloaderCustomUntil(new Date(fromDateInput(downloaderCustomEnd, true)).toISOString());
            }}
            refreshSecs={downloaderRefreshSecs}
            setRefreshSecs={setDownloaderRefreshSecs}
            lineFilter={downloaderLineFilter}
            setLineFilter={setDownloaderLineFilter}
            showLineFilter
          />

          <div className="h-[320px] w-full">
            {downloaderTrendLoading && downloaderSpeedData.length === 0 ? (
              <div className="flex flex-col items-center justify-center h-full text-muted text-[11px]">
                <RefreshCw className="mb-2 h-4 w-4 animate-spin" />
                加载中...
              </div>
            ) : downloaderSpeedData.length === 0 ? (
              <div className="flex items-center justify-center h-full rounded-2xl border border-dashed border-border bg-surface-container/20 text-[11px] text-muted">
                无数据
              </div>
            ) : (
              <ResponsiveContainer width="100%" height="100%">
                <LineChart data={downloaderSpeedData}>
                  <CartesianGrid strokeDasharray="3 3" stroke={COLORS.grid} vertical={false} />
                  <XAxis
                    dataKey="timestamp"
                    type="number"
                    scale="time"
                    domain={[currentDownloaderWindow.start, currentDownloaderWindow.end]}
                    ticks={downloaderTicks}
                    tick={{ fontSize: 9, fill: "#746780" }}
                    tickLine={false}
                    axisLine={false}
                    tickFormatter={(value: number) => formatAxisTime(value, downloaderTrendHours)}
                  />
                  <YAxis
                    tick={{ fontSize: 9, fill: "#746780" }}
                    tickLine={false}
                    axisLine={false}
                    tickFormatter={(v: number) => formatSpeed(v)}
                    width={55}
                  />
                  <Tooltip
                    contentStyle={{
                      backgroundColor: "rgba(255, 255, 255, 0.8)",
                      backdropFilter: "blur(8px)",
                      borderRadius: "12px",
                      border: "1px solid rgba(0,0,0,0.05)",
                      fontSize: "11px",
                      boxShadow: "0 10px 15px -3px rgba(0, 0, 0, 0.1)",
                    }}
                    formatter={(value, name) => [
                      formatSpeed(Number(value)),
                      name === "uploadSpeed" ? "上传速度" : "下载速度",
                    ]}
                    labelFormatter={(label) => `时间: ${formatTooltipTime(label, downloaderTrendHours)}`}
                  />
                  <Line
                    type="monotone"
                    dataKey="uploadSpeed"
                    stroke={COLORS.upload}
                    strokeWidth={2}
                    dot={false}
                    activeDot={{ r: 4, strokeWidth: 0 }}
                    hide={downloaderLineFilter === "download"}
                  />
                  <Line
                    type="monotone"
                    dataKey="downloadSpeed"
                    stroke={COLORS.download}
                    strokeWidth={2}
                    dot={false}
                    activeDot={{ r: 4, strokeWidth: 0 }}
                    hide={downloaderLineFilter === "upload"}
                  />
                </LineChart>
              </ResponsiveContainer>
            )}
          </div>
        </CardContent>
      </Card>

      {/* ===== Daily Transfer Chart ===== */}
      <Card className="rounded-xl border-border bg-surface-container/30 shadow-sm overflow-hidden">
        <CardHeader className="pb-2">
          <div className="flex flex-wrap items-center justify-between gap-3">
            <div className="flex items-center gap-2">
              <div className="p-2 rounded-xl bg-emerald-500/10">
                <BarChart3 className="h-4 w-4 text-emerald-500" />
              </div>
              <div>
                <CardTitle className="text-sm font-semibold">每日上传 / 下载量</CardTitle>
                <CardDescription className="text-[10px]">按天聚合的增量数据</CardDescription>
              </div>
            </div>
            <div className="flex items-center gap-2">
              <Select
                value={String(dailyTaskId)}
                onChange={(val) => setDailyTaskId(Number(val))}
                options={[
                  { value: "-1", label: "全部任务" },
                  ...tasks.map((t) => ({ value: String(t.task_id), label: t.task_name })),
                ]}
              />
            </div>
          </div>
        </CardHeader>
        <CardContent className="p-4 pt-2 space-y-4">
          <div className="flex flex-wrap items-center justify-between gap-2 bg-surface-container/50 p-1.5 rounded-xl border border-border/50">
            <div className="flex items-center gap-1">
              <button
                className={`h-7 px-2.5 rounded-lg text-[10px] font-medium transition-colors ${
                  dailyRangeMode === "quick" ? "bg-primary/10 text-primary" : "text-muted-foreground hover:bg-surface-container/80"
                }`}
                onClick={() => setDailyRangeMode("quick")}
              >
                快捷
              </button>
              <button
                className={`h-7 px-2.5 rounded-lg text-[10px] font-medium transition-colors ${
                  dailyRangeMode === "custom" ? "bg-primary/10 text-primary" : "text-muted-foreground hover:bg-surface-container/80"
                }`}
                onClick={() => setDailyRangeMode("custom")}
              >
                日期范围
              </button>
            </div>
            {dailyRangeMode === "quick" ? (
              <div className="flex gap-1">
                {[{ label: "7天", days: 7 }, { label: "14天", days: 14 }, { label: "30天", days: 30 }].map((r) => (
                  <button
                    key={r.label}
                    className={`h-7 px-2.5 rounded-lg text-[10px] font-medium transition-colors ${
                      dailyQuickDays === r.days
                        ? "bg-primary text-primary-foreground shadow-sm"
                        : "hover:bg-surface-container/80 text-muted-foreground"
                    }`}
                    onClick={() => setDailyQuickDays(r.days)}
                  >
                    {r.label}
                  </button>
                ))}
              </div>
            ) : (
              <div className="flex items-center gap-1.5">
                <input
                  type="date"
                  className="h-7 rounded-lg border border-border bg-input px-2 text-[10px]"
                  value={dailyCustomStart}
                  onChange={(e) => setDailyCustomStart(e.target.value)}
                />
                <span className="text-[10px] text-muted">至</span>
                <input
                  type="date"
                  className="h-7 rounded-lg border border-border bg-input px-2 text-[10px]"
                  value={dailyCustomEnd}
                  onChange={(e) => setDailyCustomEnd(e.target.value)}
                />
                <Button size="sm" className="h-7 text-[10px] px-2" onClick={() => {
                  setDailySince(new Date(fromDateInput(dailyCustomStart, false)).toISOString());
                  setDailyUntil(new Date(fromDateInput(dailyCustomEnd, true)).toISOString());
                }}>
                  查询
                </Button>
              </div>
            )}
            <div className="h-4 w-[1px] bg-border/50 mx-1 hidden sm:block" />
            <div className="flex gap-1">
              {(["both", "upload", "download"] as const).map((f) => (
                <button
                  key={f}
                  className={`h-7 px-2.5 rounded-lg text-[10px] font-medium transition-colors ${
                    dailyLineFilter === f
                      ? "bg-surface-container-highest text-foreground shadow-sm ring-1 ring-border"
                      : "hover:bg-surface-container/80 text-muted-foreground"
                  }`}
                  onClick={() => setDailyLineFilter(f)}
                >
                  {f === "both" ? "全部" : f === "upload" ? "上传" : "下载"}
                </button>
              ))}
            </div>
          </div>

          <div className="h-[300px] w-full">
            {dailyLoading && dailyData.length === 0 ? (
              <div className="flex flex-col items-center justify-center h-full text-muted text-[11px]">
                <RefreshCw className="mb-2 h-4 w-4 animate-spin" />
                加载中...
              </div>
            ) : dailyData.length === 0 ? (
              <div className="flex items-center justify-center h-full rounded-2xl border border-dashed border-border bg-surface-container/20 text-[11px] text-muted">
                无数据
              </div>
            ) : (
              <ResponsiveContainer width="100%" height="100%">
                <BarChart data={dailyData}>
                  <CartesianGrid strokeDasharray="3 3" stroke={COLORS.grid} vertical={false} />
                  <XAxis
                    dataKey="date"
                    tick={{ fontSize: 9, fill: "#746780" }}
                    tickLine={false}
                    axisLine={false}
                    tickFormatter={(v: string) => {
                      const parts = v.split("-");
                      return `${parts[1]}/${parts[2]}`;
                    }}
                  />
                  <YAxis
                    tick={{ fontSize: 9, fill: "#746780" }}
                    tickLine={false}
                    axisLine={false}
                    tickFormatter={(v: number) => formatBytes(v)}
                    width={50}
                  />
                  <Tooltip
                    contentStyle={{
                      backgroundColor: "rgba(255, 255, 255, 0.8)",
                      backdropFilter: "blur(8px)",
                      borderRadius: "12px",
                      border: "1px solid rgba(0,0,0,0.05)",
                      fontSize: "11px",
                      boxShadow: "0 10px 15px -3px rgba(0, 0, 0, 0.1)",
                    }}
                    formatter={(value, name) => [
                      formatBytes(Number(value)),
                      name === "uploaded" ? "上传" : "下载",
                    ]}
                    labelFormatter={(label) => `日期: ${label}`}
                  />
                  <Legend formatter={(value) => (value === "uploaded" ? "上传" : "下载")} wrapperStyle={{ fontSize: 11 }} />
                  <Bar dataKey="uploaded" fill={COLORS.upload} radius={[4, 4, 0, 0]} maxBarSize={36} hide={dailyLineFilter === "download"} />
                  <Bar dataKey="downloaded" fill={COLORS.download} radius={[4, 4, 0, 0]} maxBarSize={36} hide={dailyLineFilter === "upload"} />
                </BarChart>
              </ResponsiveContainer>
            )}
          </div>
        </CardContent>
      </Card>
    </div>
  );
}

/* ---------- sub-components ---------- */

function TaskOverviewCard({ task }: { task: TaskOverview }) {
  return (
    <div className="rounded-xl border border-border bg-surface-container/50 p-3 hover:bg-surface-container/80 transition-colors shadow-sm">
      <div className="flex items-center justify-between gap-2">
        <div className="text-[13px] font-semibold truncate text-foreground">{task.task_name}</div>
        <span
          className={`shrink-0 rounded-full px-2 py-0.5 text-[10px] font-medium ${
            task.enabled
              ? "bg-emerald-500/10 text-emerald-600 ring-1 ring-emerald-500/20"
              : "bg-neutral-500/10 text-neutral-500 ring-1 ring-neutral-500/20"
          }`}
        >
          {task.enabled ? "运行中" : "已停用"}
        </span>
      </div>

      <div className="mt-2.5 grid grid-cols-2 gap-2">
        <MetricItem
          icon={<TrendingUp className="h-3 w-3 text-emerald-500" />}
          label="累计上传"
          value={formatBytes(task.total_uploaded)}
        />
        <MetricItem
          icon={<TrendingUp className="h-3 w-3 text-sky-500" />}
          label="累计下载"
          value={formatBytes(task.total_downloaded)}
        />
        <MetricItem
          icon={<HardDrive className="h-3 w-3 text-violet-500" />}
          label="活跃种子"
          value={task.torrent_count.toString()}
        />
        <MetricItem
          icon={<Activity className="h-3 w-3 text-amber-500" />}
          label="实时分享率"
          value={ratio(task.total_uploaded, task.total_downloaded)}
        />
      </div>
    </div>
  );
}

function MetricItem({
  icon,
  label,
  value,
}: {
  icon: React.ReactNode;
  label: string;
  value: string;
}) {
  return (
    <div className="space-y-0.5 bg-surface-container-low/40 p-1.5 rounded-lg border border-border/30">
      <div className="flex items-center gap-1 text-[10px] text-muted-foreground">
        {icon}
        {label}
      </div>
      <div className="text-[12px] font-bold tracking-tight text-foreground">{value}</div>
    </div>
  );
}
