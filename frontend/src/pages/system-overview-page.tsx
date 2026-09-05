import { useCallback, useEffect, useRef, useState } from "react";
import {
  Activity,
  Cpu,
  MemoryStick,
  Monitor,
  Server,
} from "lucide-react";
import {
  Area,
  AreaChart,
  CartesianGrid,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Select } from "@/components/ui/select";
import { api } from "@/lib/api";
import { cn } from "@/lib/utils";
import type { SystemSnapshot, SystemSnapshotRecord } from "@/types";

/* ---------- helpers ---------- */

function formatBytes(bytes: number): string {
  if (bytes === 0) return "0 B";
  const k = 1024;
  const sizes = ["B", "KB", "MB", "GB", "TB"];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return `${(bytes / Math.pow(k, i)).toFixed(1)} ${sizes[i]}`;
}

function formatAxisTime(value: string | number, hours: number): string {
  const d = new Date(value);
  if (Number.isNaN(d.getTime())) return String(value);
  const mm = String(d.getMinutes()).padStart(2, "0");
  const hh = String(d.getHours()).padStart(2, "0");
  if (hours <= 6) return `${hh}:${mm}`;
  if (hours <= 24) return `${hh}:00`;
  const month = d.getMonth() + 1;
  const day = d.getDate();
  return `${month}/${day}`;
}

/** 根据百分比返回内存颜色：绿 → 黄 → 橙 → 红 */
function memoryColor(percent: number): string {
  const p = Math.min(100, Math.max(0, percent));
  if (p <= 50) return "#477564";       // green
  if (p <= 75) return "#926515";       // yellow
  if (p <= 90) return "#ac582e";       // orange
  return "#b63232";                    // red
}

/** 根据百分比返回内存渐变色（偏柔和） */
function memoryGradientStops(percent: number): { start: string; end: string } {
  const c = memoryColor(percent);
  return { start: c, end: c };
}

/* ---------- chart source filter ---------- */

type SourceFilter = "process" | "system" | "all";

function SourceToggle({
  value,
  onChange,
}: {
  value: SourceFilter;
  onChange: (v: SourceFilter) => void;
}) {
  const options: { key: SourceFilter; label: string }[] = [
    { key: "all", label: "全部" },
    { key: "process", label: "本进程" },
    { key: "system", label: "系统" },
  ];
  return (
    <div className="flex rounded-xl border border-border bg-surface-container/60 p-0.5">
      {options.map((opt) => (
        <button
          key={opt.key}
          type="button"
          onClick={() => onChange(opt.key)}
          className={cn(
            "rounded-lg px-3 py-1 text-xs font-semibold transition-all",
            value === opt.key
              ? "bg-primary text-primary-foreground shadow-sm"
              : "text-muted hover:text-foreground",
          )}
        >
          {opt.label}
        </button>
      ))}
    </div>
  );
}

/* ---------- circular gauge ---------- */

function Gauge({
  value,
  size = 120,
  strokeWidth = 10,
  color,
  label,
  sub,
}: {
  value: number;
  size?: number;
  strokeWidth?: number;
  color: string;
  label: string;
  sub?: string;
}) {
  const radius = (size - strokeWidth) / 2;
  const circumference = 2 * Math.PI * radius;
  const clamped = Math.min(100, Math.max(0, value));
  const offset = circumference - (clamped / 100) * circumference;

  return (
    <div className="flex flex-col items-center gap-2">
      <svg width={size} height={size} className="-rotate-90">
        <circle
          cx={size / 2}
          cy={size / 2}
          r={radius}
          fill="none"
          stroke="currentColor"
          strokeWidth={strokeWidth}
          className="text-border"
        />
        <circle
          cx={size / 2}
          cy={size / 2}
          r={radius}
          fill="none"
          stroke={color}
          strokeWidth={strokeWidth}
          strokeDasharray={circumference}
          strokeDashoffset={offset}
          strokeLinecap="round"
          className="transition-all duration-700 ease-out"
        />
      </svg>
      <div className="absolute flex flex-col items-center justify-center" style={{ width: size, height: size }}>
        <span className="text-2xl font-black tracking-tight" style={{ color }}>
          {clamped.toFixed(1)}%
        </span>
        {sub ? <span className="text-[10px] text-muted mt-0.5">{sub}</span> : null}
      </div>
      <span className="text-xs font-semibold text-muted">{label}</span>
    </div>
  );
}

/* ---------- metric card ---------- */

function MetricCard({
  icon: Icon,
  label,
  value,
  detail,
  color,
}: {
  icon: typeof Cpu;
  label: string;
  value: string;
  detail?: string;
  color: string;
}) {
  return (
    <Card className="bg-card">
      <CardContent className="p-4 sm:p-5">
        <div className="flex items-center gap-3">
          <div
            className="flex h-10 w-10 shrink-0 items-center justify-center rounded-xl"
            style={{ backgroundColor: `${color}15`, color }}
          >
            <Icon className="h-5 w-5" />
          </div>
          <div className="min-w-0">
            <div className="text-[11px] font-semibold uppercase tracking-[0.14em] text-muted">{label}</div>
            <div className="mt-1 text-xl font-black tracking-tight">{value}</div>
            {detail ? <div className="mt-0.5 text-xs text-muted">{detail}</div> : null}
          </div>
        </div>
      </CardContent>
    </Card>
  );
}

/* ---------- time window options ---------- */

const TIME_WINDOWS = [
  { label: "1 小时", hours: 1 },
  { label: "6 小时", hours: 6 },
  { label: "24 小时", hours: 24 },
  { label: "3 天", hours: 72 },
  { label: "7 天", hours: 168 },
];

const REFRESH_OPTIONS = [
  { label: "关闭", value: 0 },
  { label: "10 秒", value: 10 },
  { label: "30 秒", value: 30 },
  { label: "60 秒", value: 60 },
];

/* ---------- main page ---------- */

export function SystemOverviewPage() {
  const [snapshot, setSnapshot] = useState<SystemSnapshot | null>(null);
  const [history, setHistory] = useState<SystemSnapshotRecord[]>([]);
  const [hours, setHours] = useState(24);
  const [refreshSec, setRefreshSec] = useState(10);
  const [cpuSource, setCpuSource] = useState<SourceFilter>("all");
  const [memSource, setMemSource] = useState<SourceFilter>("all");
  const refreshTimerRef = useRef<number | null>(null);

  const fetchSnapshot = useCallback(async () => {
    try {
      const data = await api<SystemSnapshot>("/api/system/stats");
      setSnapshot(data);
    } catch {
      // 静默失败，快照可能尚未就绪
    }
  }, []);

  const fetchHistory = useCallback(async (h: number) => {
    try {
      const data = await api<SystemSnapshotRecord[]>(
        `/api/system/stats/history?hours=${h}`,
      );
      setHistory(data);
    } catch {
      // 静默失败
    }
  }, []);

  // 初始加载 + 实时轮询
  useEffect(() => {
    fetchSnapshot();
    fetchHistory(hours);

    if (refreshTimerRef.current !== null) {
      window.clearInterval(refreshTimerRef.current);
    }

    if (refreshSec > 0) {
      refreshTimerRef.current = window.setInterval(() => {
        fetchSnapshot();
        fetchHistory(hours);
      }, refreshSec * 1000);
    }

    return () => {
      if (refreshTimerRef.current !== null) {
        window.clearInterval(refreshTimerRef.current);
      }
    };
  }, [refreshSec, hours, fetchSnapshot, fetchHistory]);

  // 切换时间窗口时重新拉取
  useEffect(() => {
    fetchHistory(hours);
  }, [hours, fetchHistory]);

  // 图表数据
  const chartData = history.map((r) => ({
    time: new Date(r.recorded_at).getTime(),
    processCpu: r.process_cpu_usage,
    systemCpu: r.system_cpu_usage,
    processMemMb: r.process_memory_bytes / 1024 / 1024,
    systemMemPercent: r.system_total_memory_bytes > 0
      ? (r.system_used_memory_bytes / r.system_total_memory_bytes) * 100
      : 0,
    systemUsedGb: r.system_used_memory_bytes / 1024 / 1024 / 1024,
    systemTotalGb: r.system_total_memory_bytes / 1024 / 1024 / 1024,
  }));

  const processMemMb = snapshot?.process_memory_mb ?? 0;
  const sysMemPercent = snapshot?.system_memory_usage_percent ?? 0;
  const sysTotalGb = snapshot ? snapshot.system_total_memory_bytes / 1024 / 1024 / 1024 : 0;
  const sysUsedGb = snapshot ? snapshot.system_used_memory_bytes / 1024 / 1024 / 1024 : 0;
  const memColor = memoryColor(sysMemPercent);

  return (
    <div className="space-y-4">
      {/* 实时指标卡片 */}
      <div className="grid grid-cols-2 gap-3 lg:grid-cols-4">
        <MetricCard
          icon={Cpu}
          label="进程 CPU"
          value={`${(snapshot?.process_cpu_usage ?? 0).toFixed(1)}%`}
          detail="当前进程 CPU 占用"
          color="#b44332"
        />
        <MetricCard
          icon={MemoryStick}
          label="进程内存"
          value={`${processMemMb.toFixed(1)} MB`}
          detail={snapshot ? formatBytes(snapshot.process_memory_bytes) : "-"}
          color="#aa7143"
        />
        <MetricCard
          icon={Server}
          label="系统 CPU"
          value={`${(snapshot?.system_cpu_usage ?? 0).toFixed(1)}%`}
          detail="全部核心平均"
          color="#477564"
        />
        <MetricCard
          icon={Monitor}
          label="系统内存"
          value={`${sysMemPercent.toFixed(1)}%`}
          detail={snapshot ? `${sysUsedGb.toFixed(1)} / ${sysTotalGb.toFixed(1)} GB` : "-"}
          color={memColor}
        />
      </div>

      {/* 仪表盘 */}
      <Card className="bg-card">
        <CardHeader className="pb-2">
          <CardTitle className="flex items-center gap-2 text-base">
            <Activity className="h-4 w-4 text-primary" />
            实时仪表盘
          </CardTitle>
        </CardHeader>
        <CardContent className="flex flex-wrap items-center justify-center gap-8 py-6 sm:gap-12">
          <div className="relative">
            <Gauge
              value={snapshot?.process_cpu_usage ?? 0}
              color="#b44332"
              label="进程 CPU"
              sub={snapshot ? `${snapshot.process_cpu_usage.toFixed(1)}%` : undefined}
            />
          </div>
          <div className="relative">
            <Gauge
              value={snapshot?.system_cpu_usage ?? 0}
              size={140}
              strokeWidth={12}
              color="#477564"
              label="系统 CPU"
              sub={snapshot ? `${snapshot.system_cpu_usage.toFixed(1)}%` : undefined}
            />
          </div>
          <div className="relative">
            <Gauge
              value={processMemMb / (sysTotalGb * 1024 || 1) * 100}
              color="#aa7143"
              label="进程内存"
              sub={`${processMemMb.toFixed(0)} MB`}
            />
          </div>
          <div className="relative">
            <Gauge
              value={sysMemPercent}
              size={140}
              strokeWidth={12}
              color={memColor}
              label="系统内存"
              sub={`${sysUsedGb.toFixed(1)} / ${sysTotalGb.toFixed(1)} GB`}
            />
          </div>
        </CardContent>
      </Card>

      {/* 历史趋势 */}
      <Card className="bg-card">
        <CardHeader className="pb-2">
          <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
            <CardTitle className="flex items-center gap-2 text-base">
              <Activity className="h-4 w-4 text-primary" />
              历史趋势
            </CardTitle>
            <div className="flex items-center gap-2">
              <Select
                value={String(hours)}
                onChange={(val) => setHours(Number(val))}
                options={TIME_WINDOWS.map((tw) => ({
                  value: String(tw.hours),
                  label: tw.label,
                }))}
              />
              <Select
                value={String(refreshSec)}
                onChange={(val) => setRefreshSec(Number(val))}
                options={REFRESH_OPTIONS.map((ro) => ({
                  value: String(ro.value),
                  label: `刷新: ${ro.label}`,
                }))}
              />
            </div>
          </div>
        </CardHeader>
        <CardContent className="space-y-6 pt-2">
          {/* CPU 趋势 */}
          <div>
            <div className="mb-3 flex items-center justify-between">
              <h4 className="text-sm font-semibold text-foreground">CPU 使用率</h4>
              <SourceToggle value={cpuSource} onChange={setCpuSource} />
            </div>
            <div className="h-[240px]">
              <ResponsiveContainer width="100%" height="100%">
                <AreaChart data={chartData}>
                  <defs>
                    <linearGradient id="gradProcessCpu" x1="0" y1="0" x2="0" y2="1">
                      <stop offset="0%" stopColor="#b44332" stopOpacity={0.3} />
                      <stop offset="100%" stopColor="#b44332" stopOpacity={0} />
                    </linearGradient>
                    <linearGradient id="gradSystemCpu" x1="0" y1="0" x2="0" y2="1">
                      <stop offset="0%" stopColor="#477564" stopOpacity={0.3} />
                      <stop offset="100%" stopColor="#477564" stopOpacity={0} />
                    </linearGradient>
                  </defs>
                  <CartesianGrid strokeDasharray="3 3" stroke="var(--border)" />
                  <XAxis
                    dataKey="time"
                    type="number"
                    domain={["dataMin", "dataMax"]}
                    tickFormatter={(v) => formatAxisTime(v, hours)}
                    tick={{ fontSize: 11, fill: "var(--muted)" }}
                    stroke="var(--border)"
                  />
                  <YAxis
                    tickFormatter={(v) => `${v}%`}
                    tick={{ fontSize: 11, fill: "var(--muted)" }}
                    stroke="var(--border)"
                    domain={[0, "auto"]}
                  />
                  <Tooltip
                    labelFormatter={(v) => new Date(v as number).toLocaleString()}
                    formatter={(v: number) => [`${v.toFixed(1)}%`]}
                    contentStyle={{
                      borderRadius: 12,
                      border: "1px solid var(--border)",
                      background: "var(--card)",
                      fontSize: 12,
                    }}
                  />
                  {(cpuSource === "all" || cpuSource === "system") && (
                    <Area
                      type="monotone"
                      dataKey="systemCpu"
                      name="系统 CPU"
                      stroke="#477564"
                      fill="url(#gradSystemCpu)"
                      strokeWidth={2}
                      dot={false}
                      isAnimationActive={false}
                    />
                  )}
                  {(cpuSource === "all" || cpuSource === "process") && (
                    <Area
                      type="monotone"
                      dataKey="processCpu"
                      name="进程 CPU"
                      stroke="#b44332"
                      fill="url(#gradProcessCpu)"
                      strokeWidth={2}
                      dot={false}
                      isAnimationActive={false}
                    />
                  )}
                </AreaChart>
              </ResponsiveContainer>
            </div>
          </div>

          {/* 内存趋势 */}
          <div>
            <div className="mb-3 flex items-center justify-between">
              <h4 className="text-sm font-semibold text-foreground">内存使用</h4>
              <SourceToggle value={memSource} onChange={setMemSource} />
            </div>
            <div className="h-[240px]">
              <ResponsiveContainer width="100%" height="100%">
                <AreaChart data={chartData}>
                  <defs>
                    <linearGradient id="gradProcessMem" x1="0" y1="0" x2="0" y2="1">
                      <stop offset="0%" stopColor="#aa7143" stopOpacity={0.3} />
                      <stop offset="100%" stopColor="#aa7143" stopOpacity={0} />
                    </linearGradient>
                    <linearGradient id="gradSystemMem" x1="0" y1="0" x2="0" y2="1">
                      <stop offset="0%" stopColor={memoryGradientStops(sysMemPercent).start} stopOpacity={0.3} />
                      <stop offset="100%" stopColor={memoryGradientStops(sysMemPercent).end} stopOpacity={0} />
                    </linearGradient>
                  </defs>
                  <CartesianGrid strokeDasharray="3 3" stroke="var(--border)" />
                  <XAxis
                    dataKey="time"
                    type="number"
                    domain={["dataMin", "dataMax"]}
                    tickFormatter={(v) => formatAxisTime(v, hours)}
                    tick={{ fontSize: 11, fill: "var(--muted)" }}
                    stroke="var(--border)"
                  />
                  {(memSource === "all" || memSource === "system") && (
                    <YAxis
                      yAxisId="sys"
                      orientation="left"
                      tickFormatter={(v) => `${v.toFixed(1)}%`}
                      tick={{ fontSize: 11, fill: "var(--muted)" }}
                      stroke="var(--border)"
                      domain={[0, 100]}
                    />
                  )}
                  {(memSource === "all" || memSource === "process") && (
                    <YAxis
                      yAxisId="proc"
                      orientation={memSource === "process" ? "left" : "right"}
                      tickFormatter={(v) => `${v.toFixed(0)} MB`}
                      tick={{ fontSize: 11, fill: "var(--muted)" }}
                      stroke="var(--border)"
                      domain={[0, "auto"]}
                    />
                  )}
                  <Tooltip
                    labelFormatter={(v) => new Date(v as number).toLocaleString()}
                    formatter={(v: number, name: string) => {
                      if (name === "系统内存") return [`${v.toFixed(1)}%`];
                      return [`${v.toFixed(1)} MB`];
                    }}
                    contentStyle={{
                      borderRadius: 12,
                      border: "1px solid var(--border)",
                      background: "var(--card)",
                      fontSize: 12,
                    }}
                  />
                  {(memSource === "all" || memSource === "system") && (
                    <Area
                      yAxisId="sys"
                      type="monotone"
                      dataKey="systemMemPercent"
                      name="系统内存"
                      stroke={memColor}
                      fill="url(#gradSystemMem)"
                      strokeWidth={2}
                      dot={false}
                      isAnimationActive={false}
                    />
                  )}
                  {(memSource === "all" || memSource === "process") && (
                    <Area
                      yAxisId="proc"
                      type="monotone"
                      dataKey="processMemMb"
                      name="进程内存"
                      stroke="#aa7143"
                      fill="url(#gradProcessMem)"
                      strokeWidth={2}
                      dot={false}
                      isAnimationActive={false}
                    />
                  )}
                </AreaChart>
              </ResponsiveContainer>
            </div>
          </div>
        </CardContent>
      </Card>
    </div>
  );
}
