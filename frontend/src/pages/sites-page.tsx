import { useEffect, useMemo, useRef, useState } from "react";
import {
  Globe,
  Plus,
  Trash2,
  Activity,
  ListChecks,
  Loader2,
  Trophy,
  UploadCloud,
  DownloadCloud,
  Gauge,
  ShieldCheck,
  Copy,
  Check,
  Eye,
  EyeOff,
  ExternalLink,
  Download as DownloadIcon,
  RefreshCw,
  RotateCcw,
  Search,
  CloudCog,
  Server,
  KeyRound,
  Clock3,
  CircleCheck,
  CircleX,
  ChevronLeft,
  ChevronRight,
  ChevronDown,
  MoreHorizontal,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardHeader,
  CardDescription,
} from "@/components/ui/card";
import { Dialog } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select } from "@/components/ui/select";
import {
  Table,
  TableHeader,
  TableBody,
  TableRow,
  TableHead,
  TableCell,
} from "@/components/ui/table";
import { api } from "@/lib/api";
import type {
  PtdBackupConfig,
  PtdBackupRunResult,
  PtdBackupTestResult,
  PtdSitePreset,
  SiteCredentialsRecord,
  SiteRecord,
  SiteRequestHeader,
  SiteStatsRefreshStartResponse,
  SiteStatsRefreshStatusResponse,
  SiteStatsRecord,
  SiteTestResult,
} from "@/types";

/* ------------------------------------------------------------------ */
/*  Helpers                                                            */
/* ------------------------------------------------------------------ */

function formatBytes(bytes: number): string {
  if (bytes === 0) return "0 B";
  const k = 1024;
  const sizes = ["B", "KB", "MB", "GB", "TB", "PB"];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return `${(bytes / Math.pow(k, i)).toFixed(2)} ${sizes[i]}`;
}

function formatDateTime(value: string | null | undefined): string {
  if (!value) return "从未";
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? value : date.toLocaleString();
}

type SiteHealth = "healthy" | "failed" | "pending";
const SITE_PAGE_SIZE = 20;
const CUSTOM_SITE_PRESET = "__custom__";
const sitePrimaryButtonClassName = "h-11";

function getSiteHealth(site: SiteRecord): SiteHealth {
  if (site.stats?.last_error) return "failed";
  if (site.stats?.uploaded != null && site.stats?.downloaded != null) return "healthy";
  return "pending";
}

type AuthType = "cookie" | "passkey" | "cookie_passkey" | "api_key";
type CredentialField = "cookie" | "passkey" | "api_key";

const credentialFieldLabels: Record<CredentialField, string> = {
  cookie: "Cookie",
  passkey: "Passkey",
  api_key: "API Key",
};

function credentialFieldsFor(authType: SiteRecord["auth_type"]): CredentialField[] {
  switch (authType) {
    case "cookie":
      return ["cookie"];
    case "passkey":
      return ["passkey"];
    case "cookie_passkey":
      return ["cookie", "passkey"];
    case "api_key":
      return ["api_key"];
    default:
      return [];
  }
}

function credentialStateKey(siteId: number, field: CredentialField): string {
  return `${siteId}:${field}`;
}

function credentialActionStateKey(
  siteId: number,
  field: CredentialField,
  action: "reveal" | "copy",
): string {
  return `${credentialStateKey(siteId, field)}:${action}`;
}

async function writeClipboardText(value: string): Promise<void> {
  if (navigator.clipboard?.writeText) {
    try {
      await navigator.clipboard.writeText(value);
      return;
    } catch {
      // Fall through for browsers that expose Clipboard API but deny it on HTTP.
    }
  }

  const textarea = document.createElement("textarea");
  textarea.value = value;
  textarea.setAttribute("readonly", "");
  textarea.style.position = "fixed";
  textarea.style.opacity = "0";
  document.body.appendChild(textarea);
  textarea.select();
  const copied = document.execCommand("copy");
  textarea.remove();
  if (!copied) {
    throw new Error("当前浏览器不支持复制到剪切板");
  }
}

interface SiteForm {
  name: string;
  site_type: "nexusphp" | "mteam" | "gazelle";
  base_url: string;
  use_proxy: boolean;
  auth_type: AuthType;
  cookie: string;
  passkey: string;
  api_key: string;
  request_headers: SiteRequestHeader[];
}

interface PtdBackupForm {
  enabled: boolean;
  webdav_url: string;
  username: string;
  password: string;
  clear_password: boolean;
  use_proxy: boolean;
  backup_interval_hours: number;
}

const emptyPtdBackupForm: PtdBackupForm = {
  enabled: false,
  webdav_url: "",
  username: "",
  password: "",
  clear_password: false,
  use_proxy: false,
  backup_interval_hours: 24,
};

const defaultRequestHeaders: ReadonlyArray<Readonly<SiteRequestHeader>> = [
  {
    name: "Accept",
    value: "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,*/*;q=0.8,application/signed-exchange;v=b3;q=0.7",
  },
  { name: "Accept-Encoding", value: "gzip, deflate, br, zstd" },
  {
    name: "Accept-Language",
    value: "zh-CN,zh;q=0.9,en-US;q=0.8,en;q=0.7,zh-TW;q=0.6",
  },
  { name: "DNT", value: "1" },
  {
    name: "User-Agent",
    value: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/151.0.0.0 Safari/537.36",
  },
  {
    name: "sec-ch-ua",
    value: '"Not=A?Brand";v="99", "Google Chrome";v="151", "Chromium";v="151"',
  },
  { name: "sec-ch-ua-arch", value: '"x86"' },
  { name: "sec-ch-ua-bitness", value: '"64"' },
  { name: "sec-ch-ua-full-version", value: '"151.0.7922.109"' },
  {
    name: "sec-ch-ua-full-version-list",
    value: '"Not=A?Brand";v="99.0.0.0", "Google Chrome";v="151.0.7922.109", "Chromium";v="151.0.7922.109"',
  },
  { name: "sec-ch-ua-mobile", value: "?0" },
  { name: "sec-ch-ua-model", value: '""' },
  { name: "sec-ch-ua-platform", value: '"Windows"' },
  { name: "sec-ch-ua-platform-version", value: '"19.0.0"' },
];

function freshDefaultRequestHeaders(): SiteRequestHeader[] {
  return defaultRequestHeaders.map((header) => ({ ...header }));
}

function formatBytesCompact(bytes: number): { value: string; unit: string } {
  if (bytes === 0) return { value: "0", unit: "B" };
  const k = 1024;
  const sizes = ["B", "KB", "MB", "GB", "TB", "PB"];
  const i = Math.min(Math.floor(Math.log(bytes) / Math.log(k)), sizes.length - 1);
  return {
    value: (bytes / Math.pow(k, i)).toFixed(bytes >= Math.pow(k, 4) ? 2 : 1),
    unit: sizes[i],
  };
}

function formatRatio(uploaded: number, downloaded: number): string {
  if (downloaded <= 0) {
    return uploaded > 0 ? "∞" : "-";
  }
  return (uploaded / downloaded).toFixed(3);
}

function dateStamp(date: Date): string {
  const pad = (value: number) => value.toString().padStart(2, "0");
  return `${date.getFullYear()}${pad(date.getMonth() + 1)}${pad(date.getDate())}-${pad(date.getHours())}${pad(date.getMinutes())}`;
}

function truncateCanvasText(ctx: CanvasRenderingContext2D, text: string, maxWidth: number): string {
  if (ctx.measureText(text).width <= maxWidth) return text;
  let next = text;
  while (next.length > 0 && ctx.measureText(`${next}...`).width > maxWidth) {
    next = next.slice(0, -1);
  }
  return `${next}...`;
}

function roundedRectPath(ctx: CanvasRenderingContext2D, x: number, y: number, width: number, height: number, radius: number) {
  const r = Math.min(radius, width / 2, height / 2);
  ctx.beginPath();
  ctx.moveTo(x + r, y);
  ctx.lineTo(x + width - r, y);
  ctx.quadraticCurveTo(x + width, y, x + width, y + r);
  ctx.lineTo(x + width, y + height - r);
  ctx.quadraticCurveTo(x + width, y + height, x + width - r, y + height);
  ctx.lineTo(x + r, y + height);
  ctx.quadraticCurveTo(x, y + height, x, y + height - r);
  ctx.lineTo(x, y + r);
  ctx.quadraticCurveTo(x, y, x + r, y);
  ctx.closePath();
}

function drawRoundRect(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  width: number,
  height: number,
  radius: number,
  fill: string | CanvasGradient,
  stroke?: string,
) {
  roundedRectPath(ctx, x, y, width, height, radius);
  ctx.fillStyle = fill;
  ctx.fill();
  if (stroke) {
    ctx.strokeStyle = stroke;
    ctx.lineWidth = 1;
    ctx.stroke();
  }
}

function drawText(
  ctx: CanvasRenderingContext2D,
  text: string,
  x: number,
  y: number,
  options: {
    font: string;
    color: string;
    maxWidth?: number;
    align?: CanvasTextAlign;
  },
) {
  ctx.font = options.font;
  ctx.fillStyle = options.color;
  ctx.textAlign = options.align ?? "left";
  ctx.textBaseline = "alphabetic";
  ctx.fillText(options.maxWidth ? truncateCanvasText(ctx, text, options.maxWidth) : text, x, y);
}

function renderOverviewProofImage({
  rows,
  generatedAt,
}: {
  rows: SiteOverviewRow[];
  generatedAt: Date;
}): Promise<Blob> {
  const successfulRows = rows.filter((row) => row.stats);
  const failedRows = rows.filter((row) => row.error);
  const totalUploaded = successfulRows.reduce((sum, row) => sum + (row.stats?.uploaded ?? 0), 0);
  const totalDownloaded = successfulRows.reduce((sum, row) => sum + (row.stats?.downloaded ?? 0), 0);
  const uploaded = formatBytesCompact(totalUploaded);
  const downloaded = formatBytesCompact(totalDownloaded);
  const topRows = successfulRows
    .slice().sort((a, b) => (b.stats?.uploaded ?? 0) - (a.stats?.uploaded ?? 0))
    .slice(0, 4);
  const width = 1600;
  const tableRowHeight = 76;
  const height = 732 + 60 + rows.length * tableRowHeight + 100;
  // Keep large exports within browser canvas dimension and memory limits.
  const scale = Math.min(2, 16000 / height, Math.sqrt(24_000_000 / (width * height)));
  const canvas = document.createElement("canvas");
  canvas.width = width * scale;
  canvas.height = height * scale;
  const ctx = canvas.getContext("2d");
  if (!ctx) {
    return Promise.reject(new Error("当前浏览器不支持图片导出"));
  }

  ctx.scale(scale, scale);
  ctx.fillStyle = "#f6f3eb";
  ctx.fillRect(0, 0, width, height);

  const heroGradient = ctx.createLinearGradient(48, 48, 1552, 280);
  heroGradient.addColorStop(0, "#fffdf8");
  heroGradient.addColorStop(0.58, "#f0ece2");
  heroGradient.addColorStop(1, "#e7dfcc");
  drawRoundRect(ctx, 48, 48, 1504, 238, 36, heroGradient, "rgba(116, 109, 96, 0.18)");

  drawRoundRect(ctx, 88, 88, 92, 92, 26, "#b44332");
  drawText(ctx, "云", 111, 148, { font: "900 42px Inter, system-ui, sans-serif", color: "#ffffff" });
  drawText(ctx, "PT 账号数据", 208, 166, { font: "900 56px Inter, system-ui, sans-serif", color: "#292822" });

  drawText(ctx, "汇总包含历史数据；图片生成时间不代表账户数据刷新时间。", 88, 246, { font: "500 20px Inter, system-ui, sans-serif", color: "#746d60" });

  const generatedText = `生成时间 ${generatedAt.toLocaleString()}`;
  drawRoundRect(ctx, 1120, 88, 360, 46, 23, "rgba(255,255,255,0.62)", "rgba(116, 109, 96, 0.16)");
  drawText(ctx, generatedText, 1142, 119, { font: "800 18px Inter, system-ui, sans-serif", color: "#5b4638", maxWidth: 316 });
  drawRoundRect(ctx, 1120, 150, 170, 46, 23, "rgba(255,255,255,0.62)", "rgba(116, 109, 96, 0.16)");
  drawText(ctx, `${successfulRows.length} 个有数据`, 1142, 181, { font: "800 18px Inter, system-ui, sans-serif", color: "#5b4638" });
  drawRoundRect(ctx, 1310, 150, 170, 46, 23, "rgba(255,255,255,0.62)", "rgba(116, 109, 96, 0.16)");
  drawText(ctx, `${failedRows.length} 个失败`, 1332, 181, { font: "800 18px Inter, system-ui, sans-serif", color: failedRows.length ? "#b63232" : "#5b4638" });

  const metrics = [
    ["总上传量", successfulRows.length ? uploaded.value : "—", successfulRows.length ? uploaded.unit : ""],
    ["总下载量", successfulRows.length ? downloaded.value : "—", successfulRows.length ? downloaded.unit : ""],
    ["综合分享率", successfulRows.length ? formatRatio(totalUploaded, totalDownloaded) : "—", ""],
    ["有账户数据", `${successfulRows.length}`, `/ ${rows.length}`],
  ];
  metrics.forEach(([label, value, unit], index) => {
    const x = 48 + index * 376;
    drawRoundRect(ctx, x, 320, 352, 150, 26, "rgba(255,255,255,0.82)", "rgba(116, 109, 96, 0.16)");
    drawText(ctx, label, x + 28, 362, { font: "800 20px Inter, system-ui, sans-serif", color: "#746d60" });
    const valueWidth = unit ? 205 : 296;
    let valueSize = 48;
    ctx.font = `900 ${valueSize}px Inter, system-ui, sans-serif`;
    while (ctx.measureText(value).width > valueWidth && valueSize > 20) {
      valueSize -= 1;
      ctx.font = `900 ${valueSize}px Inter, system-ui, sans-serif`;
    }
    drawText(ctx, value, x + 28, 430, { font: ctx.font, color: "#292822" });
    drawText(ctx, unit, x + 245, 430, { font: "900 22px Inter, system-ui, sans-serif", color: "#b44332", maxWidth: 80 });
  });

  drawRoundRect(ctx, 48, 510, 1504, 154, 28, "rgba(255,255,255,0.72)", "rgba(116, 109, 96, 0.16)");
  drawText(ctx, "上传量排行", 82, 554, { font: "900 26px Inter, system-ui, sans-serif", color: "#292822" });
  if (!topRows.length) drawText(ctx, "暂无账户数据，请先刷新站点统计", 82, 614, { font: "500 22px Inter, system-ui, sans-serif", color: "#746d60" });
  topRows.forEach((row, index) => {
    const x = 82 + index * 360;
    const stats = row.stats;
    drawRoundRect(ctx, x, 584, 328, 54, 18, "rgba(245,238,255,0.78)", "rgba(116, 109, 96, 0.12)");
    drawRoundRect(ctx, x + 14, 597, 30, 30, 12, "#b44332");
    drawText(ctx, String(index + 1), x + 24, 619, { font: "900 16px Inter, system-ui, sans-serif", color: "#ffffff" });
    drawText(ctx, row.site.name, x + 56, 609, { font: "900 18px Inter, system-ui, sans-serif", color: "#292822", maxWidth: 255 });
    drawText(ctx, stats ? formatBytes(stats.uploaded) : "-", x + 56, 630, { font: "800 15px Inter, system-ui, sans-serif", color: "#746d60", maxWidth: 130 });
  });

  const tableY = 732;
  drawText(ctx, "站点明细", 48, tableY - 24, { font: "900 26px Inter, system-ui, sans-serif", color: "#292822" });
  const cols = [
    ["站点", 74, 190],
    ["UID", 282, 140],
    ["用户名", 440, 210],
    ["上传量", 668, 210],
    ["下载量", 896, 210],
    ["分享率", 1124, 120],
    ["状态", 1268, 210],
  ] as const;
  drawRoundRect(ctx, 48, tableY, 1504, 48, 18, "rgba(238,229,255,0.92)", "rgba(116, 109, 96, 0.16)");
  cols.forEach(([label, x]) =>
    drawText(ctx, label, x, tableY + 31, { font: "900 16px Inter, system-ui, sans-serif", color: "#746d60" }),
  );

  rows.forEach((row, index) => {
    const y = tableY + 60 + index * tableRowHeight;
    const stats = row.stats;
    drawRoundRect(ctx, 48, y, 1504, 66, 12, index % 2 === 0 ? "rgba(255,255,255,0.78)" : "rgba(246,240,255,0.72)", "rgba(116, 109, 96, 0.12)");
    const values = [
      row.site.name,
      stats?.uid ?? "-",
      stats?.username ?? "-",
      stats ? formatBytes(stats.uploaded) : "-",
      stats ? formatBytes(stats.downloaded) : "-",
      stats ? formatRatio(stats.uploaded, stats.downloaded) : "-",
      row.error ? (row.stats ? "拉取失败 · 保留历史数据" : "拉取失败 · 暂无数据") : (row.stats ? "最近拉取成功" : "等待刷新"),
    ];
    cols.forEach(([, x, maxWidth], colIndex) =>
      drawText(ctx, values[colIndex], x, y + 31, {
        font: colIndex === 0 || colIndex === 3 ? "900 17px Inter, system-ui, sans-serif" : "700 16px Inter, system-ui, sans-serif",
        color: row.error && colIndex === 6 ? "#b63232" : colIndex === 3 ? "#292822" : "#746d60",
        maxWidth,
      }),
    );
    drawText(ctx, row.site.stats?.last_checked_at ? `检查 ${formatDateTime(row.site.stats.last_checked_at)}` : "尚未检查", 1268, y + 53, { font: "500 13px Inter, system-ui, sans-serif", color: "#746d60", maxWidth: 250 });
  });

  drawText(ctx, "Generated by 云母", 48, height - 38, { font: "800 18px Inter, system-ui, sans-serif", color: "#b44332" });

  return new Promise((resolve, reject) => {
    canvas.toBlob((blob) => {
      if (blob) {
        resolve(blob);
      } else {
        reject(new Error("图片生成失败"));
      }
    }, "image/png", 0.95);
  });
}

type SiteOverviewRow = {
  site: SiteRecord;
  stats: (SiteStatsRecord & { uploaded: number; downloaded: number }) | null;
  error: string | null;
};

const emptySiteForm: SiteForm = {
  name: "",
  site_type: "nexusphp",
  base_url: "",
  use_proxy: true,
  auth_type: "cookie",
  cookie: "",
  passkey: "",
  api_key: "",
  request_headers: freshDefaultRequestHeaders(),
};

function buildAuthConfig(form: SiteForm): object {
  if (form.site_type === "mteam") {
    return { auth_type: "api_key", api_key: form.api_key };
  }
  switch (form.auth_type) {
    case "cookie":
      return { auth_type: "cookie", cookie: form.cookie };
    case "passkey":
      return { auth_type: "passkey", passkey: form.passkey };
    case "cookie_passkey":
      return {
        auth_type: "cookie_passkey",
        cookie: form.cookie,
        passkey: form.passkey,
      };
    case "api_key":
      return { auth_type: "api_key", api_key: form.api_key };
    default:
      return { auth_type: form.auth_type };
  }
}

function SiteCredentialList({
  site,
  credentials,
  revealedKeys,
  loadingKeys,
  copiedKey,
  compact = false,
  onToggle,
  onCopy,
}: {
  site: SiteRecord;
  credentials?: SiteCredentialsRecord;
  revealedKeys: Set<string>;
  loadingKeys: Set<string>;
  copiedKey: string | null;
  compact?: boolean;
  onToggle: (site: SiteRecord, field: CredentialField) => void;
  onCopy: (site: SiteRecord, field: CredentialField) => void;
}) {
  const fields = credentialFieldsFor(site.auth_type);
  if (!site.auth_configured || fields.length === 0) {
    return <span className="text-xs text-muted">未配置</span>;
  }

  return (
    <div className={compact ? "space-y-1.5" : "min-w-[230px] space-y-1.5"}>
      {fields.map((field) => {
        const stateKey = credentialStateKey(site.id, field);
        const revealed = revealedKeys.has(stateKey);
        const revealLoading = loadingKeys.has(credentialActionStateKey(site.id, field, "reveal"));
        const copyLoading = loadingKeys.has(credentialActionStateKey(site.id, field, "copy"));
        const loading = revealLoading || copyLoading;
        const copied = copiedKey === stateKey;
        const value = credentials?.[field] ?? "";
        const visibleValue = revealed ? value || "未配置" : "••••••••••••";

        return (
          <div key={field} className="flex h-8 min-w-0 items-center gap-1.5">
            <span className="w-[4.25rem] shrink-0 text-[11px] font-semibold text-muted">
              {credentialFieldLabels[field]}
            </span>
            <span
              className={`min-w-0 flex-1 truncate font-mono text-xs ${revealed ? "text-foreground" : "select-none text-muted"}`}
              title={revealed ? visibleValue : "凭据已隐藏"}
            >
              {visibleValue}
            </span>
            <button
              type="button"
              className="flex size-7 shrink-0 items-center justify-center rounded-lg text-muted transition-colors hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/40 disabled:cursor-wait disabled:opacity-60"
              onClick={() => onToggle(site, field)}
              disabled={loading}
              aria-label={`${revealed ? "隐藏" : "显示"}${site.name}的${credentialFieldLabels[field]}`}
              aria-pressed={revealed}
              title={revealed ? "隐藏明文" : "显示明文"}
            >
              {revealLoading ? (
                <Loader2 className="size-4 animate-spin" />
              ) : revealed ? (
                <EyeOff className="size-4" />
              ) : (
                <Eye className="size-4" />
              )}
            </button>
            <button
              type="button"
              className="flex size-7 shrink-0 items-center justify-center rounded-lg text-muted transition-colors hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/40 disabled:cursor-wait disabled:opacity-60"
              onClick={() => onCopy(site, field)}
              disabled={loading}
              aria-label={`${copied ? "已复制" : "复制"}${site.name}的${credentialFieldLabels[field]}`}
              title={copied ? "已复制" : "复制"}
            >
              {copyLoading ? (
                <Loader2 className="size-4 animate-spin" />
              ) : copied ? (
                <Check className="size-4 text-jade" />
              ) : (
                <Copy className="size-4" />
              )}
            </button>
          </div>
        );
      })}
    </div>
  );
}

function SiteRequestHeadersEditor({
  headers,
  loading,
  onChange,
  onAdd,
  onRemove,
  onRestore,
}: {
  headers: SiteRequestHeader[];
  loading: boolean;
  onChange: (index: number, field: keyof SiteRequestHeader, value: string) => void;
  onAdd: () => void;
  onRemove: (index: number) => void;
  onRestore: () => void;
}) {
  return (
    <section className="min-w-0 space-y-3" aria-labelledby="site-request-headers-label">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="flex items-baseline gap-2">
          <Label id="site-request-headers-label">自定义请求头</Label>
          <span className="text-xs text-muted">{headers.length} 项</span>
        </div>
        <div className="flex flex-wrap justify-end gap-2">
          <Button
            type="button"
            variant="outline"
            className="h-11 px-3 text-sm"
            onClick={onRestore}
            disabled={loading}
          >
            <RotateCcw className="mr-1.5 size-3.5" />
            恢复默认
          </Button>
          <Button
            type="button"
            variant="secondary"
            className="h-11 px-3 text-sm"
            onClick={onAdd}
            disabled={loading || headers.length >= 64}
          >
            <Plus className="mr-1.5 size-3.5" />
            添加请求头
          </Button>
        </div>
      </div>

      <div className="overflow-hidden rounded-2xl border border-border bg-surface-container/45">
        <div className="hidden grid-cols-[minmax(8rem,0.7fr)_minmax(12rem,1.3fr)_2.75rem] gap-2 border-b border-border bg-card/65 px-3 py-2 text-xs font-semibold text-muted sm:grid">
          <span>名称</span>
          <span>值</span>
          <span className="sr-only">操作</span>
        </div>
        {loading ? (
          <div className="flex h-40 items-center justify-center text-sm text-muted">
            <Loader2 className="mr-2 size-4 animate-spin" />
            加载请求头…
          </div>
        ) : headers.length === 0 ? (
          <div className="flex h-28 items-center justify-center text-sm text-muted">
            未配置请求头
          </div>
        ) : (
          <div className="space-y-3 p-2.5">
            {headers.map((header, index) => (
              <div
                key={index}
                className="grid grid-cols-[minmax(0,1fr)_2.75rem] gap-2 rounded-xl border border-border/70 bg-card/75 p-2 sm:grid-cols-[minmax(8rem,0.7fr)_minmax(12rem,1.3fr)_2.75rem]"
              >
                <Input
                  className="h-11 min-w-0 rounded-xl px-3 font-mono text-base"
                  value={header.name}
                  onChange={(event) => onChange(index, "name", event.target.value)}
                  placeholder="请求头名称"
                  aria-label={`第 ${index + 1} 个请求头名称`}
                  spellCheck={false}
                />
                <Input
                  className="col-start-1 row-start-2 h-11 min-w-0 rounded-xl px-3 font-mono text-base sm:col-start-2 sm:row-start-1"
                  value={header.value}
                  onChange={(event) => onChange(index, "value", event.target.value)}
                  placeholder="请求头值"
                  aria-label={`第 ${index + 1} 个请求头值`}
                  spellCheck={false}
                />
                <button
                  type="button"
                  className="col-start-2 row-span-2 row-start-1 flex size-11 cursor-pointer items-center justify-center self-center rounded-xl text-muted transition-colors duration-200 hover:bg-destructive/10 hover:text-red-700 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/40 sm:col-start-3 sm:row-span-1"
                  onClick={() => onRemove(index)}
                  aria-label={`删除第 ${index + 1} 个请求头`}
                  title="删除请求头"
                >
                  <Trash2 className="size-4" />
                </button>
              </div>
            ))}
          </div>
        )}
      </div>
    </section>
  );
}

/* ------------------------------------------------------------------ */
/*  Component                                                          */
/* ------------------------------------------------------------------ */

export function SitesPage() {
  const [sites, setSites] = useState<SiteRecord[]>([]);
  const [loading, setLoading] = useState(true);
  const [message, setMessage] = useState("");
  const [sitesError, setSitesError] = useState("");
  const [actionsTarget, setActionsTarget] = useState<SiteRecord | null>(null);
  const [credentialMessage, setCredentialMessage] = useState("");
  const [refreshAllSubmitting, setRefreshAllSubmitting] = useState(false);
  const [refreshingAll, setRefreshingAll] = useState(false);
  const [siteCredentials, setSiteCredentials] = useState<Record<number, SiteCredentialsRecord>>({});
  const [revealedCredentialKeys, setRevealedCredentialKeys] = useState<Set<string>>(() => new Set());
  const [loadingCredentialKeys, setLoadingCredentialKeys] = useState<Set<string>>(() => new Set());
  const [copiedCredentialKey, setCopiedCredentialKey] = useState<string | null>(null);
  const [credentialsTarget, setCredentialsTarget] = useState<SiteRecord | null>(null);
  const [siteQuery, setSiteQuery] = useState("");
  const [siteStatusFilter, setSiteStatusFilter] = useState<"all" | SiteHealth>("all");
  const [siteTypeFilter, setSiteTypeFilter] = useState("all");
  const [sitePage, setSitePage] = useState(1);
  const [sitePresets, setSitePresets] = useState<PtdSitePreset[]>([]);
  const [sitePresetsLoading, setSitePresetsLoading] = useState(true);
  const [sitePresetsError, setSitePresetsError] = useState("");
  const [selectedSitePreset, setSelectedSitePreset] = useState(CUSTOM_SITE_PRESET);

  // Hive PTD compatible WebDAV backup
  const [ptdConfig, setPtdConfig] = useState<PtdBackupConfig | null>(null);
  const [ptdConfigLoading, setPtdConfigLoading] = useState(true);
  const [ptdDialogOpen, setPtdDialogOpen] = useState(false);
  const [ptdForm, setPtdForm] = useState<PtdBackupForm>(emptyPtdBackupForm);
  const [ptdFormError, setPtdFormError] = useState("");
  const [ptdConfigError, setPtdConfigError] = useState("");
  const [ptdBackupMessage, setPtdBackupMessage] = useState("");
  const [ptdSaving, setPtdSaving] = useState(false);
  const [ptdTesting, setPtdTesting] = useState(false);
  const [ptdTestResult, setPtdTestResult] = useState<PtdBackupTestResult | null>(null);
  const [ptdBackingUp, setPtdBackingUp] = useState(false);

  // form dialog
  const [formOpen, setFormOpen] = useState(false);
  const [editingId, setEditingId] = useState<number | null>(null);
  const [form, setForm] = useState<SiteForm>(emptySiteForm);
  const [existingAuth, setExistingAuth] = useState<{
    siteType: SiteForm["site_type"];
    authType: AuthType;
    configured: boolean;
  } | null>(null);
  const [clearAuthConfig, setClearAuthConfig] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [formError, setFormError] = useState("");
  const [fieldErrors, setFieldErrors] = useState<{ name?: string; base_url?: string }>({});
  const [advancedOpen, setAdvancedOpen] = useState(false);
  const [visibleAuthFields, setVisibleAuthFields] = useState<Set<CredentialField>>(() => new Set());
  const [requestHeadersError, setRequestHeadersError] = useState("");
  const [requestHeadersLoading, setRequestHeadersLoading] = useState(false);
  const requestHeadersLoadRef = useRef(0);

  // delete confirmation
  const [deleteTarget, setDeleteTarget] = useState<SiteRecord | null>(null);
  const [deleting, setDeleting] = useState(false);
  const [deleteError, setDeleteError] = useState("");

  // test connection
  const [testResult, setTestResult] = useState<SiteTestResult | null>(null);
  const [testOpen, setTestOpen] = useState(false);
  const [testing, setTesting] = useState(false);
  const [testTarget, setTestTarget] = useState<SiteRecord | null>(null);
  const testRequestRef = useRef(0);

  // overview
  const [overviewRows, setOverviewRows] = useState<SiteOverviewRow[]>([]);
  const [overviewOpen, setOverviewOpen] = useState(false);
  const [overviewLoading, setOverviewLoading] = useState(false);
  const [overviewError, setOverviewError] = useState("");
  const [overviewMessage, setOverviewMessage] = useState("");
  const [overviewGeneratedAt, setOverviewGeneratedAt] = useState<Date | null>(null);
  const [overviewExporting, setOverviewExporting] = useState<"copy" | "download" | null>(null);

  const successfulOverviewRows = overviewRows.filter((row) => row.stats);
  const failedOverviewRows = overviewRows.filter((row) => row.error);
  const totalUploaded = successfulOverviewRows.reduce((sum, row) => sum + (row.stats?.uploaded ?? 0), 0);
  const totalDownloaded = successfulOverviewRows.reduce((sum, row) => sum + (row.stats?.downloaded ?? 0), 0);
  const totalUploadedCompact = formatBytesCompact(totalUploaded);
  const totalDownloadedCompact = formatBytesCompact(totalDownloaded);
  const topOverviewRows = successfulOverviewRows
    .slice().sort((a, b) => (b.stats?.uploaded ?? 0) - (a.stats?.uploaded ?? 0))
    .slice(0, 4);
  const siteCounts = useMemo(
    () => ({
      healthy: sites.filter((site) => getSiteHealth(site) === "healthy").length,
      failed: sites.filter((site) => getSiteHealth(site) === "failed").length,
      pending: sites.filter((site) => getSiteHealth(site) === "pending").length,
    }),
    [sites],
  );
  const sitePresetOptions = useMemo(
    () => [
      {
        value: CUSTOM_SITE_PRESET,
        label: "自定义站点",
        description: "手动填写名称、类型和站点地址",
        keywords: ["custom", "自定义", "手动"],
      },
      ...sitePresets.map((preset) => ({
        value: preset.ptd_id,
        label: preset.name,
        description: `${preset.site_type === "gazelle" ? "Gazelle · " : ""}${preset.aliases.join("、") || preset.ptd_id} · ${preset.base_url.replace(/^https?:\/\//, "")}`,
        keywords: [preset.ptd_id, preset.base_url, preset.site_type, ...preset.aliases],
      })),
    ],
    [sitePresets],
  );
  const filteredSites = useMemo(() => {
    const query = siteQuery.trim().toLocaleLowerCase();
    return sites.filter((site) => {
      if (siteStatusFilter !== "all" && getSiteHealth(site) !== siteStatusFilter) return false;
      if (siteTypeFilter !== "all" && site.site_type !== siteTypeFilter) return false;
      if (!query) return true;
      return [
        site.name,
        site.base_url,
        site.site_type,
        site.stats?.username,
        site.stats?.uid,
        ptdConfig?.site_identifiers[String(site.id)],
      ].some((value) => value?.toLocaleLowerCase().includes(query));
    });
  }, [ptdConfig?.site_identifiers, siteQuery, siteStatusFilter, siteTypeFilter, sites]);
  const sitePageCount = Math.max(1, Math.ceil(filteredSites.length / SITE_PAGE_SIZE));
  const pagedSites = filteredSites.slice(
    (sitePage - 1) * SITE_PAGE_SIZE,
    sitePage * SITE_PAGE_SIZE,
  );

  useEffect(() => {
    setSitePage(1);
  }, [siteQuery, siteStatusFilter, siteTypeFilter]);

  useEffect(() => {
    setSitePage((current) => Math.min(current, sitePageCount));
  }, [sitePageCount]);

  /* ---- data loading ---- */

  function loadSites() {
    setLoading(true);
    setSitesError("");
    setSiteCredentials({});
    setRevealedCredentialKeys(new Set());
    setLoadingCredentialKeys(new Set());
    setCopiedCredentialKey(null);
    api<SiteRecord[]>("/api/sites")
      .then((data) => {
        setSites(data);
      })
      .catch((error: Error) => {
        setSitesError(error.message || "无法连接服务，请稍后重试");
      })
      .finally(() => setLoading(false));
  }

  function loadPtdConfig() {
    setPtdConfigLoading(true);
    setPtdConfigError("");
    api<PtdBackupConfig>("/api/sites/ptd-backup")
      .then(setPtdConfig)
      .catch((error: Error) => setPtdConfigError(error.message || "加载备份配置失败"))
      .finally(() => setPtdConfigLoading(false));
  }

  function loadSitePresets() {
    setSitePresetsLoading(true);
    setSitePresetsError("");
    api<PtdSitePreset[]>("/api/sites/catalog")
      .then(setSitePresets)
      .catch((error: Error) => {
        setSitePresets([]);
        setSitePresetsError(error.message || "加载 PTD 站点列表失败");
      })
      .finally(() => setSitePresetsLoading(false));
  }

  useEffect(() => {
    loadSites();
    loadPtdConfig();
    loadSitePresets();
    api<SiteStatsRefreshStatusResponse>("/api/sites/refresh-all")
      .then((status) => setRefreshingAll(status.refreshing))
      .catch(() => undefined);
  }, []);

  useEffect(() => {
    if (!refreshingAll) return;

    let cancelled = false;
    let timer: number | null = null;
    let consecutiveFailures = 0;

    const scheduleStatusCheck = () => {
      timer = window.setTimeout(checkStatus, 1200);
    };

    const checkStatus = async () => {
      try {
        const status = await api<SiteStatsRefreshStatusResponse>("/api/sites/refresh-all");
        if (cancelled) return;
        consecutiveFailures = 0;
        if (status.refreshing) {
          scheduleStatusCheck();
          return;
        }

        const refreshedSites = await api<SiteRecord[]>("/api/sites");
        if (cancelled) return;
        setSites(refreshedSites);
        setSitesError("");
        setRefreshingAll(false);
        const failedCount = refreshedSites.filter((site) => site.stats?.last_error).length;
        setMessage(
          failedCount > 0
            ? `刷新完成，${failedCount} 个站点失败，可筛选“拉取失败”查看原因`
            : "全部站点刷新完成",
        );
      } catch (error) {
        if (cancelled) return;
        consecutiveFailures += 1;
        if (consecutiveFailures < 3) {
          scheduleStatusCheck();
          return;
        }
        setRefreshingAll(false);
        setMessage(`刷新任务已提交，但无法确认执行结果：${(error as Error).message}`);
      }
    };

    scheduleStatusCheck();
    return () => {
      cancelled = true;
      if (timer !== null) window.clearTimeout(timer);
    };
  }, [refreshingAll]);

  async function handleRefreshAll() {
    setRefreshAllSubmitting(true);
    try {
      const response = await api<SiteStatsRefreshStartResponse>("/api/sites/refresh-all", {
        method: "POST",
      });
      setRefreshingAll(true);
      setMessage(
        response.started
          ? "已开始在后台刷新全部站点"
          : "全部站点统计正在后台刷新",
      );
    } catch (error) {
      setMessage((error as Error).message || "启动全站刷新失败");
    } finally {
      setRefreshAllSubmitting(false);
    }
  }

  async function loadSiteCredentials(site: SiteRecord): Promise<SiteCredentialsRecord> {
    const cached = siteCredentials[site.id];
    if (cached) return cached;

    const credentials = await api<SiteCredentialsRecord>(`/api/sites/${site.id}/credentials`);
    setSiteCredentials((current) => ({ ...current, [site.id]: credentials }));
    return credentials;
  }

  function setCredentialLoading(stateKey: string, loadingState: boolean) {
    setLoadingCredentialKeys((current) => {
      const next = new Set(current);
      if (loadingState) {
        next.add(stateKey);
      } else {
        next.delete(stateKey);
      }
      return next;
    });
  }

  async function handleToggleCredential(site: SiteRecord, field: CredentialField) {
    const stateKey = credentialStateKey(site.id, field);
    if (revealedCredentialKeys.has(stateKey)) {
      setRevealedCredentialKeys((current) => {
        const next = new Set(current);
        next.delete(stateKey);
        return next;
      });
      return;
    }

    const loadingKey = credentialActionStateKey(site.id, field, "reveal");
    setCredentialLoading(loadingKey, true);
    try {
      const credentials = await loadSiteCredentials(site);
      if (!credentials[field]) {
        throw new Error(`${credentialFieldLabels[field]} 未配置`);
      }
      setRevealedCredentialKeys((current) => new Set(current).add(stateKey));
    } catch (error) {
      setCredentialMessage((error as Error).message || "读取站点凭据失败");
    } finally {
      setCredentialLoading(loadingKey, false);
    }
  }

  async function handleCopyCredential(site: SiteRecord, field: CredentialField) {
    const stateKey = credentialStateKey(site.id, field);
    const loadingKey = credentialActionStateKey(site.id, field, "copy");
    setCredentialLoading(loadingKey, true);
    try {
      const credentials = await loadSiteCredentials(site);
      const value = credentials[field];
      if (!value) {
        throw new Error(`${credentialFieldLabels[field]} 未配置`);
      }
      await writeClipboardText(value);
      setCopiedCredentialKey(stateKey);
      setCredentialMessage(`${site.name} 的 ${credentialFieldLabels[field]} 已复制`);
      window.setTimeout(() => {
        setCopiedCredentialKey((current) => (current === stateKey ? null : current));
      }, 2000);
    } catch (error) {
      setCredentialMessage((error as Error).message || "复制站点凭据失败");
    } finally {
      setCredentialLoading(loadingKey, false);
    }
  }

  function closeCredentialsDialog() {
    const siteId = credentialsTarget?.id;
    if (siteId != null) {
      setSiteCredentials((current) => {
        const next = { ...current };
        delete next[siteId];
        return next;
      });
      setRevealedCredentialKeys((current) => new Set(
        [...current].filter((key) => !key.startsWith(`${siteId}:`)),
      ));
    }
    setCredentialsTarget(null);
    setCredentialMessage("");
  }

  function handleOpenSite(site: SiteRecord) {
    try {
      const url = new URL(site.base_url.trim());
      if (url.protocol !== "http:" && url.protocol !== "https:") {
        throw new Error("unsupported protocol");
      }
      if (
        (site.site_type === "nexusphp" || site.site_type === "nexus_php") &&
        !url.pathname.endsWith("/index.php")
      ) {
        url.pathname = `${url.pathname.replace(/\/+$/, "")}/index.php`;
      }
      window.open(url.href, "_blank", "noopener,noreferrer");
    } catch {
      setActionsTarget(null);
      setMessage(`${site.name} 的站点地址无效，仅支持 HTTP 或 HTTPS`);
    }
  }

  /* ---- form helpers ---- */

  function patch(partial: Partial<SiteForm>) {
    setForm((prev) => ({ ...prev, ...partial }));
  }

  function applySitePreset(value: string) {
    if (value === CUSTOM_SITE_PRESET) {
      if (selectedSitePreset !== CUSTOM_SITE_PRESET) {
        patch({
          name: "",
          site_type: "nexusphp",
          base_url: "",
          auth_type: "cookie",
          cookie: "",
          passkey: "",
          api_key: "",
        });
      }
      setSelectedSitePreset(value);
      return;
    }

    const preset = sitePresets.find((site) => site.ptd_id === value);
    if (!preset) return;
    setSelectedSitePreset(value);
    setClearAuthConfig(false);
    patch({
      name: preset.name,
      site_type: preset.site_type,
      base_url: preset.base_url,
      auth_type: preset.site_type === "mteam" ? "api_key" : "cookie",
      cookie: "",
      passkey: "",
      api_key: "",
    });
  }

  function closeForm() {
    if (submitting) return;
    setVisibleAuthFields(new Set());
    requestHeadersLoadRef.current += 1;
    setRequestHeadersLoading(false);
    setFormError("");
    setFormOpen(false);
  }

  function updateRequestHeader(
    index: number,
    field: keyof SiteRequestHeader,
    value: string,
  ) {
    setForm((current) => ({
      ...current,
      request_headers: current.request_headers.map((header, headerIndex) =>
        headerIndex === index ? { ...header, [field]: value } : header,
      ),
    }));
  }

  function addRequestHeader() {
    setForm((current) => ({
      ...current,
      request_headers: [...current.request_headers, { name: "", value: "" }],
    }));
  }

  function removeRequestHeader(index: number) {
    setForm((current) => ({
      ...current,
      request_headers: current.request_headers.filter((_, headerIndex) => headerIndex !== index),
    }));
  }

  function restoreDefaultRequestHeaders() {
    patch({ request_headers: freshDefaultRequestHeaders() });
  }

  function openAdd() {
    requestHeadersLoadRef.current += 1;
    setEditingId(null);
    setSelectedSitePreset(CUSTOM_SITE_PRESET);
    setForm({ ...emptySiteForm, request_headers: freshDefaultRequestHeaders() });
    setExistingAuth(null);
    setClearAuthConfig(false);
    setFormError("");
    setFieldErrors({});
    setAdvancedOpen(false);
    setVisibleAuthFields(new Set());
    setRequestHeadersError("");
    setRequestHeadersLoading(false);
    setFormOpen(true);
  }

  function openEdit(site: SiteRecord) {
    const loadId = requestHeadersLoadRef.current + 1;
    requestHeadersLoadRef.current = loadId;
    setEditingId(site.id);
    setSelectedSitePreset(CUSTOM_SITE_PRESET);
    const siteType = (site.site_type as SiteForm["site_type"]) || "nexusphp";
    const authType = site.auth_type ?? (siteType === "mteam" ? "api_key" : "cookie");
    setForm({
      ...emptySiteForm,
      name: site.name,
      site_type: siteType,
      base_url: site.base_url,
      use_proxy: site.use_proxy,
      auth_type: authType,
      request_headers: [],
    });
    setExistingAuth({ siteType, authType, configured: site.auth_configured });
    setClearAuthConfig(false);
    setFormError("");
    setFieldErrors({});
    setAdvancedOpen(false);
    setVisibleAuthFields(new Set());
    setFormOpen(true);
    loadRequestHeaders(site.id, loadId);
  }

  function loadRequestHeaders(siteId: number, loadId = requestHeadersLoadRef.current) {
    setRequestHeadersLoading(true);
    setRequestHeadersError("");
    api<SiteRequestHeader[]>(`/api/sites/${siteId}/request-headers`)
      .then((requestHeaders) => {
        if (requestHeadersLoadRef.current !== loadId) return;
        patch({ request_headers: requestHeaders });
      })
      .catch((error: Error) => {
        if (requestHeadersLoadRef.current !== loadId) return;
        setRequestHeadersError(error.message || "加载站点请求头失败");
      })
      .finally(() => {
        if (requestHeadersLoadRef.current === loadId) {
          setRequestHeadersLoading(false);
        }
      });
  }

  function handleSubmit() {
    if (submitting || requestHeadersLoading || requestHeadersError) return;
    const errors: { name?: string; base_url?: string } = {};
    if (!form.name.trim()) errors.name = "请填写站点名称";
    try {
      const url = new URL(form.base_url.trim());
      if (!["http:", "https:"].includes(url.protocol)) throw new Error();
    } catch {
      errors.base_url = "请输入以 http:// 或 https:// 开头的完整站点地址";
    }
    setFieldErrors(errors);
    if (Object.keys(errors).length) {
      document.getElementById(errors.name ? "site-name" : "site-base-url")?.focus();
      return;
    }
    const requestHeaders = form.request_headers.filter(
      (header) => header.name.trim() || header.value.trim(),
    );
    const missingNameIndex = requestHeaders.findIndex((header) => !header.name.trim());
    if (missingNameIndex >= 0) {
      setAdvancedOpen(true);
      setFormError(`第 ${missingNameIndex + 1} 个请求头名称不能为空`);
      return;
    }
    const seenHeaderNames = new Set<string>();
    for (const header of requestHeaders) {
      const normalizedName = header.name.trim().toLowerCase();
      if (seenHeaderNames.has(normalizedName)) {
        setAdvancedOpen(true);
        setFormError(`请求头名称不能重复：${header.name.trim()}`);
        return;
      }
      seenHeaderNames.add(normalizedName);
    }
    setFormError("");
    setSubmitting(true);
    const body = {
      name: form.name.trim(),
      site_type: form.site_type,
      base_url: form.base_url.trim(),
      use_proxy: form.use_proxy,
      auth_config: buildAuthConfig(form),
      request_headers: requestHeaders,
      clear_auth_config: editingId != null && clearAuthConfig,
    };
    const req =
      editingId != null
        ? api<{ ok: true }>(`/api/sites/${editingId}`, {
            method: "PUT",
            body: JSON.stringify(body),
          })
        : api<{ id: number }>("/api/sites", {
            method: "POST",
            body: JSON.stringify(body),
          });
    req
      .then(() => {
        requestHeadersLoadRef.current += 1;
        setFormOpen(false);
        setVisibleAuthFields(new Set());
        setMessage(editingId != null ? "站点已更新，可测试连接或刷新账户数据" : "站点已创建，可测试连接或刷新账户数据");
        loadSites();
        loadPtdConfig();
      })
      .catch((error: Error) => setFormError(error.message || "保存站点失败"))
      .finally(() => setSubmitting(false));
  }

  /* ---- delete ---- */

  function confirmDelete() {
    if (!deleteTarget || deleting) return;
    setDeleteError("");
    setDeleting(true);
    api<{ ok: true }>(`/api/sites/${deleteTarget.id}`, { method: "DELETE" })
      .then(() => {
        setDeleteTarget(null);
        setMessage("站点已删除");
        loadSites();
        loadPtdConfig();
      })
      .catch((error: Error) => setDeleteError(error.message || "删除站点失败，请重试"))
      .finally(() => setDeleting(false));
  }

  /* ---- test connection ---- */

  function handleTest(site: SiteRecord) {
    const requestId = ++testRequestRef.current;
    setTestTarget(site);
    setTesting(true);
    setTestResult(null);
    setTestOpen(true);
    api<SiteTestResult>(`/api/sites/${site.id}/test`, { method: "POST" })
      .then((result) => { if (testRequestRef.current === requestId) setTestResult(result); })
      .catch((error: Error) => {
        if (testRequestRef.current === requestId) {
          setTestResult({ success: false, message: error.message || "连接测试失败，请重试", user_stats: null });
        }
      })
      .finally(() => { if (testRequestRef.current === requestId) setTesting(false); });
  }

  function handleOverview() {
    setOverviewOpen(true);
    setOverviewError("");
    setOverviewMessage("");
    setOverviewLoading(true);
    setOverviewRows([]);
    setOverviewGeneratedAt(null);
    api<SiteRecord[]>("/api/sites/stats-overview")
      .then((data) => {
        setSites(data);
        setSitesError("");
        return data.map((site): SiteOverviewRow => ({
          site,
          stats: site.stats?.uploaded != null && site.stats.downloaded != null
            ? { ...site.stats, uploaded: site.stats.uploaded, downloaded: site.stats.downloaded } : null,
          error: site.stats?.last_error ?? null,
        }));
      })
      .then((rows) => {
        setOverviewRows(
          rows.slice().sort((a, b) => {
            if (a.error && !b.error) return 1;
            if (!a.error && b.error) return -1;
            return a.site.id - b.site.id;
          }),
        );
      })
      .catch((error: Error) => setOverviewError(error.message || "加载站点总览失败"))
      .then(() => setOverviewGeneratedAt(new Date()))
      .finally(() => setOverviewLoading(false));
  }

  async function createOverviewImageBlob() {
    await document.fonts.ready;
    const generatedAt = overviewGeneratedAt ?? new Date();
    return renderOverviewProofImage({ rows: overviewRows, generatedAt });
  }

  function downloadOverviewBlob(blob: Blob) {
    const generatedAt = overviewGeneratedAt ?? new Date();
    const url = URL.createObjectURL(blob);
    const link = document.createElement("a");
    link.href = url;
    link.download = `yunmu-pt-proof-${dateStamp(generatedAt)}.png`;
    document.body.appendChild(link);
    link.click();
    link.remove();
    URL.revokeObjectURL(url);
  }

  async function handleCopyOverviewImage() {
    setOverviewExporting("copy");
    setOverviewMessage("");
    try {
      const blobPromise = createOverviewImageBlob();
      const ClipboardItemCtor = window.ClipboardItem;
      if (!navigator.clipboard || !ClipboardItemCtor) {
        downloadOverviewBlob(await blobPromise);
        setOverviewMessage("当前浏览器不支持图片剪切板，已改为下载 PNG");
        return;
      }
      await navigator.clipboard.write([new ClipboardItemCtor({ "image/png": blobPromise })]);
      setOverviewMessage("PT 数据证明图已复制到剪切板");
    } catch (error) {
      setOverviewMessage(`复制图片失败，请使用“下载图片”保存。${(error as Error).message || ""}`);
    } finally {
      setOverviewExporting(null);
    }
  }

  async function handleDownloadOverviewImage() {
    setOverviewExporting("download");
    setOverviewMessage("");
    try {
      const blob = await createOverviewImageBlob();
      downloadOverviewBlob(blob);
      setOverviewMessage("PT 数据证明图已下载");
    } catch (error) {
      setOverviewMessage((error as Error).message || "下载图片失败");
    } finally {
      setOverviewExporting(null);
    }
  }

  function openPtdConfig() {
    const config = ptdConfig;
    setPtdForm(
      config
        ? {
            enabled: config.enabled,
            webdav_url: config.webdav_url,
            username: config.username,
            password: "",
            clear_password: false,
            use_proxy: config.use_proxy,
            backup_interval_hours: config.backup_interval_hours,
          }
        : { ...emptyPtdBackupForm },
    );
    setPtdFormError("");
    setPtdTestResult(null);
    setPtdBackupMessage("");
    setPtdDialogOpen(true);
  }

  function ptdRequestBody() {
    return {
      ...ptdForm,
      backup_interval_hours: Number(ptdForm.backup_interval_hours),
    };
  }

  async function handleSavePtdConfig() {
    setPtdSaving(true);
    setPtdFormError("");
    try {
      const saved = await api<PtdBackupConfig>("/api/sites/ptd-backup", {
        method: "PUT",
        body: JSON.stringify(ptdRequestBody()),
      });
      setPtdConfig(saved);
      setPtdDialogOpen(false);
      setMessage(saved.enabled ? "蜂巢 PTD 自动备份已启用" : "蜂巢 PTD 配置已保存");
    } catch (error) {
      setPtdFormError((error as Error).message || "保存蜂巢 PTD 配置失败");
    } finally {
      setPtdSaving(false);
    }
  }

  async function handleTestPtdConfig() {
    setPtdTesting(true);
    setPtdFormError("");
    setPtdTestResult(null);
    try {
      const result = await api<PtdBackupTestResult>("/api/sites/ptd-backup/test", {
        method: "POST",
        body: JSON.stringify(ptdRequestBody()),
      });
      setPtdTestResult(result);
    } catch (error) {
      setPtdTestResult({
        success: false,
        message: (error as Error).message || "WebDAV 连接测试失败",
      });
    } finally {
      setPtdTesting(false);
    }
  }

  async function handlePtdBackupNow() {
    setPtdBackingUp(true);
    setPtdFormError("");
    setPtdBackupMessage("");
    try {
      const saved = await api<PtdBackupConfig>("/api/sites/ptd-backup", {
        method: "PUT",
        body: JSON.stringify(ptdRequestBody()),
      });
      setPtdConfig(saved);
      setPtdForm((current) => ({ ...current, password: "", clear_password: false }));
      const result = await api<PtdBackupRunResult>("/api/sites/ptd-backup/run", {
        method: "POST",
      });
      setPtdBackupMessage(
        `已上传 ${result.filename}，包含 ${result.site_count} 个站点（${formatBytes(result.size)}）`,
      );
      loadPtdConfig();
    } catch (error) {
      setPtdFormError((error as Error).message || "蜂巢 PTD 备份失败");
      loadPtdConfig();
    } finally {
      setPtdBackingUp(false);
    }
  }

  /* ---- auth fields ---- */

  const canPreserveAuth = Boolean(
    existingAuth?.configured &&
      existingAuth.siteType === form.site_type &&
      existingAuth.authType === form.auth_type,
  );
  const credentialPlaceholder = canPreserveAuth
    ? "留空以保留已保存的凭据"
    : "输入新的认证凭据";

  function renderAuthFields() {
    const fields = credentialFieldsFor(form.site_type === "mteam" ? "api_key" : form.auth_type);
    return (
      <div className="space-y-4">
        {form.site_type !== "mteam" ? (
          <div className="space-y-2">
            <Label htmlFor="site-auth-type">认证方式</Label>
            <Select
              id="site-auth-type"
              value={form.auth_type}
              onChange={(value) => {
                patch({ auth_type: value as AuthType });
                setVisibleAuthFields(new Set());
                setClearAuthConfig(false);
              }}
              options={[
                { value: "cookie", label: "Cookie" },
                ...(form.site_type !== "gazelle" ? [{ value: "passkey", label: "Passkey" }] : []),
                { value: "cookie_passkey", label: "Cookie + Passkey" },
                ...(form.site_type !== "gazelle" ? [{ value: "api_key", label: "API Key" }] : []),
              ]}
            />
          </div>
        ) : null}
        {fields.map((field) => {
          const revealed = visibleAuthFields.has(field);
          const label = credentialFieldLabels[field];
          const id = `site-auth-${field}`;
          return (
            <div key={field} className="space-y-2">
              <Label htmlFor={id}>{label}</Label>
              <div className="relative">
                <Input
                  id={id}
                  type={revealed ? "text" : "password"}
                  className="pr-14 text-base"
                  autoComplete="off"
                  autoCapitalize="none"
                  spellCheck={false}
                  value={form[field]}
                  onChange={(event) => {
                    patch({ [field]: event.target.value });
                    if (event.target.value) setClearAuthConfig(false);
                  }}
                  placeholder={credentialPlaceholder}
                  aria-describedby={`${id}-help`}
                  disabled={clearAuthConfig}
                />
                <button
                  type="button"
                  className="absolute inset-y-0 right-0 flex w-12 items-center justify-center rounded-r-xl text-muted hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary disabled:opacity-50"
                  disabled={clearAuthConfig}
                  aria-label={`${revealed ? "隐藏" : "显示"}${label}`}
                  aria-pressed={revealed}
                  onClick={() => setVisibleAuthFields((current) => {
                    const next = new Set(current);
                    if (next.has(field)) next.delete(field); else next.add(field);
                    return next;
                  })}
                >
                  {revealed ? <EyeOff className="size-4" /> : <Eye className="size-4" />}
                </button>
              </div>
              <p id={`${id}-help`} className="text-sm leading-6 text-muted">
                {field === "cookie" ? "登录站点后，从浏览器开发者工具的网络请求中复制 Cookie 值。"
                  : field === "passkey" ? "从站点个人设置中获取 Passkey；请勿分享给他人。"
                  : "从站点提供的 API 设置中获取密钥；请勿分享给他人。"}
              </p>
            </div>
          );
        })}
      </div>
    );
  }

  function renderSiteActions(site: SiteRecord) {
    return (
      <div className="flex flex-wrap justify-end gap-1.5">
        <Button variant="outline" className="h-11 px-3 shadow-none" onClick={() => handleTest(site)} aria-label={`测试${site.name}连接`}>测试</Button>
        <Button variant="secondary" className="h-11 px-3 shadow-none" onClick={() => openEdit(site)} aria-label={`编辑${site.name}`}>编辑</Button>
        <Button variant="outline" className="h-11 px-3 shadow-none" onClick={() => setActionsTarget(site)} aria-label={`${site.name}的更多操作`}><MoreHorizontal className="size-4" /><span className="sr-only">更多</span></Button>
      </div>
    );
  }

  /* ---- render ---- */

  return (
    <div className="space-y-6">
      <Card className="overflow-hidden rounded-2xl border-border bg-card shadow-none">
        <CardHeader className="border-0 p-4 pb-0 sm:p-6 sm:pb-0">
          <div className="flex flex-col gap-4 lg:flex-row lg:items-center lg:justify-between">
            <div className="min-w-0">
              <CardDescription>管理站点连接，查看账户数据与刷新状态</CardDescription>
            </div>
            <div className="flex flex-wrap gap-2 lg:justify-end">
              <Button
                variant="outline"
                className="h-11"
                onClick={() => void handleRefreshAll()}
                disabled={loading || sites.length === 0 || refreshAllSubmitting || refreshingAll}
                aria-busy={refreshAllSubmitting || refreshingAll}
                title="在后台刷新全部站点统计"
              >
                <RefreshCw className={`mr-2 size-4 ${refreshAllSubmitting || refreshingAll ? "motion-safe:animate-spin" : ""}`} />
                {refreshAllSubmitting ? "提交中" : refreshingAll ? "刷新中" : "刷新所有"}
              </Button>
              <Button variant="outline" className="h-11" onClick={handleOverview} disabled={loading || sites.length === 0}>
                <ListChecks className="mr-2 size-4" />
                数据总览
              </Button>
              <Button variant="outline" className="h-11" onClick={openPtdConfig} disabled={ptdConfigLoading}>
                <CloudCog className="mr-2 size-4" />
                备份与同步
                {ptdConfig?.last_error || ptdConfigError ? <span className="ml-2 text-xs text-red-700">异常</span> : null}
              </Button>
              <Button className={sitePrimaryButtonClassName} onClick={openAdd}>
                <Plus className="mr-2 size-4" />
                添加站点
              </Button>
            </div>
          </div>
        </CardHeader>

        <CardContent className="space-y-5 p-4 sm:p-6">
          {message ? (
            <div
              className="rounded-2xl border border-primary/20 bg-primary/5 px-4 py-3 text-sm"
              role="status"
              aria-live="polite"
            >
              <div className="flex items-start justify-between gap-3">
                <span>{message}</span>
                <button type="button" className="text-muted hover:text-foreground" onClick={() => setMessage("")}>
                  关闭
                </button>
              </div>
            </div>
          ) : null}

          <section className="flex flex-wrap gap-2" aria-label="站点状态概览">
            {([
              { key: "all", label: "全部站点", value: sites.length, icon: Server, tone: "text-primary bg-primary/10" },
              { key: "healthy", label: "最近拉取成功", value: siteCounts.healthy, icon: CircleCheck, tone: "text-emerald-700 bg-emerald-100" },
              { key: "failed", label: "拉取失败", value: siteCounts.failed, icon: CircleX, tone: "text-red-700 bg-red-100" },
              { key: "pending", label: "等待刷新", value: siteCounts.pending, icon: Clock3, tone: "text-amber-700 bg-amber-100" },
            ] as const).map((metric) => {
              const MetricIcon = metric.icon;
              const selected = siteStatusFilter === metric.key;
              return (
                <button
                  key={metric.key}
                  type="button"
                  className={`flex min-h-11 min-w-0 items-center gap-2 rounded-xl px-3 py-2 text-left transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary ${selected ? "bg-secondary text-secondary-foreground" : "text-muted hover:bg-accent/45"}`}
                  aria-pressed={selected}
                  onClick={() => setSiteStatusFilter(metric.key)}
                >
                  <span className={`flex size-6 shrink-0 items-center justify-center rounded-full ${metric.tone}`}>
                    <MetricIcon className="size-4" />
                  </span>
                  <span className="flex items-center gap-2 text-sm">
                    <span>{metric.label}</span>
                    <span className="font-semibold tabular-nums">{sitesError && sites.length === 0 ? "—" : metric.value}</span>
                  </span>
                </button>
              );
            })}
          </section>

          <section className="flex flex-col gap-3 sm:flex-row sm:flex-wrap sm:items-center" aria-label="筛选站点">
            <div className="relative min-w-0 flex-1 sm:basis-64">
              <Label htmlFor="site-search" className="sr-only">搜索站点</Label>
              <Search className="pointer-events-none absolute left-3.5 top-1/2 size-4 -translate-y-1/2 text-muted" />
              <Input
                id="site-search"
                value={siteQuery}
                onChange={(event) => setSiteQuery(event.target.value)}
                className="h-11 rounded-2xl pl-10"
                placeholder="搜索站点或账户"
                title="支持站点、域名、用户名、UID 和 PTD 标识"
              />
            </div>
            <Label htmlFor="site-type-filter" className="sr-only">筛选站点类型</Label>
            <Select
              id="site-type-filter"
              value={siteTypeFilter}
              onChange={setSiteTypeFilter}
              className="w-full sm:w-40"
              options={[
                { value: "all", label: "全部类型" },
                { value: "nexusphp", label: "NexusPHP" },
                { value: "mteam", label: "M-Team" },
                { value: "gazelle", label: "Gazelle" },
              ]}
            />
            <Label htmlFor="site-status-filter" className="sr-only">筛选刷新状态</Label>
            <Select
              id="site-status-filter"
              value={siteStatusFilter}
              onChange={(value) => setSiteStatusFilter(value as "all" | SiteHealth)}
              className="w-full sm:w-40"
              options={[
                { value: "all", label: "全部状态" },
                { value: "healthy", label: "最近拉取成功" },
                { value: "failed", label: "拉取失败" },
                { value: "pending", label: "等待刷新" },
              ]}
            />
            <span className="shrink-0 px-1 text-xs font-semibold text-muted">显示 {filteredSites.length} / {sites.length}</span>
          </section>

          {sitesError ? (
            <div role="alert" className="flex flex-wrap items-center justify-between gap-3 rounded-xl border border-destructive/25 bg-destructive/5 p-4">
              <div className="min-w-0 flex-1">
                <p className="font-semibold text-red-700">站点列表加载失败</p>
                <p className="mt-1 break-words text-sm">{sitesError}</p>
                {sites.length > 0 ? <p className="mt-1 text-sm text-muted">以下保留上次加载的数据，状态可能已变化。</p> : null}
              </div>
              <Button variant="outline" className="h-11" onClick={loadSites} disabled={loading}>重新加载</Button>
            </div>
          ) : null}
          {loading ? (
            <div role="status" className="flex items-center justify-center py-12 text-muted">
              <Loader2 className="mr-2 h-5 w-5 animate-spin" />
              加载中…
            </div>
          ) : sitesError && sites.length === 0 ? null : sites.length === 0 ? (
            <div className="rounded-2xl border border-dashed border-border py-14 text-center">
              <Globe className="mx-auto size-8 text-muted" />
              <p className="mt-3 font-semibold">还没有配置 PT 站点</p>
              <p className="mt-1 text-sm text-muted">添加首个站点后即可测试连接并刷新账户数据。</p>
              <Button className={`mt-4 ${sitePrimaryButtonClassName}`} onClick={openAdd}><Plus className="mr-2 size-4" />添加站点</Button>
            </div>
          ) : filteredSites.length === 0 ? (
            <div className="rounded-2xl border border-dashed border-border py-12 text-center text-sm text-muted">
              <p>没有符合当前筛选条件的站点</p>
              <Button variant="outline" className="mt-4 h-11" onClick={() => {
                setSiteQuery(""); setSiteTypeFilter("all"); setSiteStatusFilter("all");
              }}>清除筛选</Button>
            </div>
          ) : (
            <>
              <div className="hidden xl:block">
                <table className="w-full table-fixed border-collapse text-left text-sm">
                  <caption className="sr-only">站点账户数据、刷新状态与管理操作</caption>
                  <thead className="border-b border-border text-muted">
                    <tr>
                      <th scope="col" className="w-[25%] px-2 py-3 font-medium">站点 / 账户</th>
                      <th scope="col" className="w-[24%] px-2 py-3 font-medium">账户数据</th>
                      <th scope="col" className="px-2 py-3 font-medium">刷新状态</th>
                      <th scope="col" className="w-[190px] px-2 py-3 text-right font-medium">操作</th>
                    </tr>
                  </thead>
                  <tbody className="divide-y divide-border">
                    {pagedSites.map((site) => (
                      <tr key={site.id} className="align-top hover:bg-accent/25">
                        <td className="px-2 py-4">
                          <p className="break-words font-semibold">{site.name}</p>
                          <p className="mt-1 truncate text-sm text-muted" title={site.base_url}>{site.base_url}</p>
                          <p className="mt-2 break-words text-sm">{site.stats?.username ?? "账户待获取"}</p>
                          <p className="mt-1 break-words text-xs text-muted">UID {site.stats?.uid ?? "—"} · {site.site_type === "gazelle" ? "Gazelle" : site.site_type}</p>
                        </td>
                        <td className="px-2 py-4">
                          <dl className="space-y-1.5 tabular-nums">
                            <div className="flex flex-wrap gap-x-2"><dt className="text-muted">上传</dt><dd className="font-medium">{site.stats?.uploaded != null ? formatBytes(site.stats.uploaded) : "待刷新"}</dd></div>
                            <div className="flex flex-wrap gap-x-2"><dt className="text-muted">下载</dt><dd>{site.stats?.downloaded != null ? formatBytes(site.stats.downloaded) : "待刷新"}</dd></div>
                            <div className="flex flex-wrap gap-x-2"><dt className="text-muted">分享率</dt><dd>{site.stats?.uploaded != null && site.stats.downloaded != null ? formatRatio(site.stats.uploaded, site.stats.downloaded) : "—"}</dd></div>
                            <div className="flex flex-wrap gap-x-2 text-xs text-muted"><dt>魔力</dt><dd>{site.stats?.bonus != null ? site.stats.bonus.toFixed(1) : "—"}</dd></div>
                          </dl>
                        </td>
                        <td className="px-2 py-4"><SiteStatusDetail site={site} /></td>
                        <td className="px-2 py-4">{renderSiteActions(site)}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>

              <div className="grid gap-4 xl:hidden">
                {pagedSites.map((site) => (
                  <article key={site.id} aria-label={site.name} className="min-w-0 rounded-xl border border-border p-4">
                    <div className="flex flex-wrap items-start justify-between gap-2">
                      <div className="min-w-0 flex-1">
                        <h3 className="break-words text-base font-semibold">{site.name}</h3>
                        <p className="mt-1 break-all text-sm text-muted">{site.base_url}</p>
                        <p className="mt-1 break-words text-sm text-muted">{site.stats?.username ?? "账户待获取"}</p>
                      </div>
                      <SiteHealthBadge site={site} />
                    </div>
                    <dl className="my-4 grid grid-cols-3 gap-2 text-sm tabular-nums">
                      <div><dt className="text-muted">上传</dt><dd className="mt-1 break-all font-medium">{site.stats?.uploaded != null ? formatBytes(site.stats.uploaded) : "—"}</dd></div>
                      <div><dt className="text-muted">下载</dt><dd className="mt-1 break-all font-medium">{site.stats?.downloaded != null ? formatBytes(site.stats.downloaded) : "—"}</dd></div>
                      <div><dt className="text-muted">分享率</dt><dd className="mt-1 break-all font-medium">{site.stats?.uploaded != null && site.stats.downloaded != null ? formatRatio(site.stats.uploaded, site.stats.downloaded) : "—"}</dd></div>
                    </dl>
                    <SiteStatusDetail site={site} showBadge={false} />
                    <div className="mt-4 border-t border-border pt-3">{renderSiteActions(site)}</div>
                  </article>
                ))}
              </div>
              {sitePageCount > 1 ? (
                <nav className="mt-4 flex items-center justify-between gap-3 rounded-2xl border border-border bg-surface-container/40 px-3 py-2" aria-label="站点分页">
                  <span className="text-xs text-muted">
                    第 {sitePage} / {sitePageCount} 页 · 每页 {SITE_PAGE_SIZE} 个
                  </span>
                  <div className="flex gap-2">
                    <Button
                      variant="outline"
                      className="h-11 px-3 text-sm"
                      onClick={() => setSitePage((current) => Math.max(1, current - 1))}
                      disabled={sitePage <= 1}
                    >
                      <ChevronLeft className="mr-1 size-3.5" />上一页
                    </Button>
                    <Button
                      variant="outline"
                      className="h-11 px-3 text-sm"
                      onClick={() => setSitePage((current) => Math.min(sitePageCount, current + 1))}
                      disabled={sitePage >= sitePageCount}
                    >
                      下一页<ChevronRight className="ml-1 size-3.5" />
                    </Button>
                  </div>
                </nav>
              ) : null}
            </>
          )}
        </CardContent>
      </Card>

      <Dialog
        open={ptdDialogOpen}
        onClose={() => setPtdDialogOpen(false)}
        title="备份与同步"
        description="将站点账户数据备份到蜂巢或其他 WebDAV 服务，兼容 PT-Depiler。"
        panelClassName="max-w-3xl"
      >
        <div className="space-y-5 p-4 sm:p-6">
          <section className="space-y-3 rounded-2xl border border-border bg-surface-container/40 p-4" aria-label="备份状态">
            <div className="flex flex-wrap items-center justify-between gap-3">
              <div>
                <p className="text-sm font-bold">蜂巢 PTD 备份</p>
                <p className="mt-1 text-xs text-muted">{ptdConfig?.enabled ? `每 ${ptdConfig.backup_interval_hours} 小时自动备份` : ptdConfig?.configured ? "手动备份" : "填写下方连接信息后开始备份"}</p>
              </div>
              <Button variant="outline" onClick={() => void handlePtdBackupNow()} disabled={!ptdForm.webdav_url.trim() || ptdBackingUp || ptdSaving || ptdTesting || sites.length === 0}>
                {ptdBackingUp ? <Loader2 className="mr-2 size-4 motion-safe:animate-spin" /> : <UploadCloud className="mr-2 size-4" />}
                {ptdBackingUp ? "备份中" : "保存并备份"}
              </Button>
            </div>
            <p className="text-xs text-muted">上次备份：{formatDateTime(ptdConfig?.last_backup_at)}</p>
            {ptdConfig?.last_backup_filename ? <p className="break-all font-mono text-xs text-muted">{ptdConfig.last_backup_filename}</p> : null}
            {ptdConfig?.last_error ? <p className="break-words text-xs text-red-700">上次备份失败：{ptdConfig.last_error}</p> : null}
            {ptdBackupMessage ? <p className="break-words text-sm" role="status">{ptdBackupMessage}</p> : null}
            {ptdConfigError ? <p className="text-sm text-red-700" role="alert">{ptdConfigError} <button type="button" className="underline underline-offset-2" onClick={loadPtdConfig}>重试</button></p> : null}
          </section>

          {ptdFormError ? (
            <div role="alert" className="rounded-2xl border border-destructive/30 bg-destructive/10 px-4 py-3 text-sm text-red-700 lg:col-span-2">
              {ptdFormError}
            </div>
          ) : null}

          <section className="space-y-4" aria-labelledby="ptd-webdav-heading">
            <div>
              <h4 id="ptd-webdav-heading" className="flex items-center gap-2 font-black"><Server className="size-4 text-primary" />WebDAV 连接</h4>
              <p className="mt-1 text-xs leading-5 text-muted">填写服务提供的备份地址及登录凭据。</p>
            </div>
            <div className="space-y-2">
              <Label htmlFor="ptd-webdav-url">WebDAV 地址</Label>
              <Input
                id="ptd-webdav-url"
                value={ptdForm.webdav_url}
                onChange={(event) => setPtdForm((current) => ({ ...current, webdav_url: event.target.value }))}
                placeholder="https://example.com/dav/ptd"
                spellCheck={false}
              />
            </div>
            <div className="grid gap-3 sm:grid-cols-2">
              <div className="space-y-2">
                <Label htmlFor="ptd-webdav-username">用户名</Label>
                <Input
                  id="ptd-webdav-username"
                  value={ptdForm.username}
                  onChange={(event) => setPtdForm((current) => ({ ...current, username: event.target.value }))}
                  autoComplete="username"
                />
              </div>
              <div className="space-y-2">
                <Label htmlFor="ptd-webdav-password">密码</Label>
                <Input
                  id="ptd-webdav-password"
                  type="password"
                  value={ptdForm.password}
                  onChange={(event) => setPtdForm((current) => ({ ...current, password: event.target.value, clear_password: false }))}
                  placeholder={ptdConfig?.password_configured ? "留空以保留已保存密码" : "可留空（匿名 WebDAV）"}
                  disabled={ptdForm.clear_password}
                  autoComplete="new-password"
                />
              </div>
            </div>
            {ptdConfig?.password_configured ? (
              <Label className="flex cursor-pointer items-center gap-2 text-xs text-muted">
                <input
                  type="checkbox"
                  className="size-4 accent-primary"
                  checked={ptdForm.clear_password}
                  onChange={(event) => setPtdForm((current) => ({ ...current, clear_password: event.target.checked, password: "" }))}
                />
                清除已保存的 WebDAV 密码
              </Label>
            ) : null}
            <div className="space-y-2">
              <Label htmlFor="ptd-backup-interval">自动备份周期（小时）</Label>
              <Input
                id="ptd-backup-interval"
                type="number"
                min={1}
                max={720}
                value={ptdForm.backup_interval_hours}
                onChange={(event) => setPtdForm((current) => ({ ...current, backup_interval_hours: Number(event.target.value) }))}
              />
              <p className="text-[11px] leading-5 text-muted">每次站点统计刷新完成后检查周期；成功数据按日留存，并上传完整历史用户信息。</p>
            </div>
            <div className="space-y-2 rounded-2xl border border-border bg-surface-container/55 p-3">
              <Label className="flex cursor-pointer items-center justify-between gap-3">
                <span>
                  <span className="block text-sm font-bold">自动备份</span>
                  <span className="mt-0.5 block text-[11px] font-normal text-muted">到达周期后自动生成并上传</span>
                </span>
                <input
                  type="checkbox"
                  className="size-5 accent-primary"
                  checked={ptdForm.enabled}
                  onChange={(event) => setPtdForm((current) => ({ ...current, enabled: event.target.checked }))}
                />
              </Label>
              <Label className="flex cursor-pointer items-center justify-between gap-3 border-t border-border/70 pt-2">
                <span>
                  <span className="block text-sm font-bold">使用全局代理</span>
                  <span className="mt-0.5 block text-[11px] font-normal text-muted">WebDAV 连接复用系统代理配置</span>
                </span>
                <input
                  type="checkbox"
                  className="size-5 accent-primary"
                  checked={ptdForm.use_proxy}
                  onChange={(event) => setPtdForm((current) => ({ ...current, use_proxy: event.target.checked }))}
                />
              </Label>
            </div>
          </section>

          <details className="group rounded-2xl border border-border p-4">
            <summary className="flex cursor-pointer items-center justify-between gap-3 text-sm font-bold focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary">
              <span>备份范围</span>
              <span className="flex items-center gap-2 text-xs font-normal text-muted">
                {Object.values(ptdConfig?.site_identifiers ?? {}).filter(Boolean).length} / {sites.length} 个站点已识别
                <ChevronRight className="size-4 transition-transform group-open:rotate-90" />
              </span>
            </summary>
            <p className="my-3 text-xs text-muted">根据域名自动识别，展开查看可备份的站点。</p>
            <div className="max-h-[min(50dvh,32rem)] space-y-2 overflow-y-auto rounded-2xl border border-border bg-surface-container/40 p-2.5">
              {sites.length === 0 ? (
                <p className="py-10 text-center text-sm text-muted">请先添加站点</p>
              ) : sites.map((site) => {
                const identifier = ptdConfig?.site_identifiers[String(site.id)];
                return (
                  <div key={site.id} className="flex items-center justify-between gap-3 rounded-xl border border-border/70 bg-card/80 p-3">
                    <div className="min-w-0">
                      <div className="truncate text-sm font-bold">{site.name}</div>
                      <div className="mt-0.5 truncate text-[10px] text-muted">{site.base_url}</div>
                    </div>
                    {identifier ? (
                      <span className="shrink-0 rounded-lg bg-primary/10 px-2.5 py-1 font-mono text-xs font-bold text-primary">{identifier}</span>
                    ) : (
                      <span className="shrink-0 rounded-lg bg-amber-100 px-2.5 py-1 text-[10px] font-bold text-amber-800">未识别，不备份</span>
                    )}
                  </div>
                );
              })}
            </div>
          </details>
          {ptdTestResult ? (
              <div className={`flex items-start gap-2 rounded-2xl border px-3 py-2.5 text-sm ${ptdTestResult.success ? "border-emerald-200 bg-emerald-50 text-emerald-800" : "border-red-200 bg-red-50 text-red-700"}`} role="status">
                {ptdTestResult.success ? <CircleCheck className="mt-0.5 size-4 shrink-0" /> : <CircleX className="mt-0.5 size-4 shrink-0" />}
                <span>{ptdTestResult.message}</span>
              </div>
            ) : null}

          <div className="flex flex-col-reverse gap-2 border-t border-border pt-4 sm:flex-row sm:justify-end lg:col-span-2">
            <Button variant="secondary" onClick={() => setPtdDialogOpen(false)}>取消</Button>
            <Button variant="outline" onClick={() => void handleTestPtdConfig()} disabled={ptdTesting || ptdSaving || ptdBackingUp}>
              {ptdTesting ? <Loader2 className="mr-2 size-4 motion-safe:animate-spin" /> : <Activity className="mr-2 size-4" />}
              {ptdTesting ? "测试中" : "测试连接"}
            </Button>
            <Button className={sitePrimaryButtonClassName} onClick={() => void handleSavePtdConfig()} disabled={ptdSaving || ptdTesting || ptdBackingUp}>
              {ptdSaving ? <Loader2 className="mr-2 size-4 motion-safe:animate-spin" /> : null}
              {ptdSaving ? "保存中" : "保存配置"}
            </Button>
          </div>
        </div>
      </Dialog>

      <Dialog
        open={actionsTarget != null}
        onClose={() => setActionsTarget(null)}
        title={actionsTarget ? `${actionsTarget.name} · 更多操作` : "站点操作"}
        panelClassName="max-w-lg"
      >
        {actionsTarget ? (
          <div className="space-y-5 p-4 sm:p-6">
            <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-4 gap-y-2 text-sm">
              <dt className="text-muted">账户</dt><dd className="break-words">{actionsTarget.stats?.username ?? "待获取"}</dd>
              <dt className="text-muted">UID</dt><dd className="break-all">{actionsTarget.stats?.uid ?? "—"}</dd>
              <dt className="text-muted">魔力</dt><dd>{actionsTarget.stats?.bonus?.toFixed(1) ?? "—"}</dd>
              <dt className="text-muted">站点类型</dt><dd>{actionsTarget.site_type}</dd>
              <dt className="text-muted">PTD 标识</dt><dd className="break-all">{ptdConfig?.site_identifiers[String(actionsTarget.id)] ?? "未识别"}</dd>
            </dl>
            <div className="grid gap-2">
              <Button variant="outline" className="h-11 justify-start" onClick={() => { setCredentialMessage(""); setCredentialsTarget(actionsTarget); setActionsTarget(null); }}><KeyRound className="mr-2 size-4" />查看凭据</Button>
              <Button variant="outline" className="h-11 justify-start" onClick={() => handleOpenSite(actionsTarget)}><ExternalLink className="mr-2 size-4" />打开站点主页</Button>
              <Button variant="outline" className="h-11 justify-start text-red-700" onClick={() => { setDeleteError(""); setDeleteTarget(actionsTarget); setActionsTarget(null); }}><Trash2 className="mr-2 size-4" />删除站点</Button>
            </div>
          </div>
        ) : null}
      </Dialog>

      <Dialog
        open={credentialsTarget != null}
        onClose={closeCredentialsDialog}
        title="站点凭据"
        description={credentialsTarget ? `${credentialsTarget.name} · 敏感信息仅在本次查看时读取` : undefined}
      >
        {credentialsTarget ? (
          <div className="space-y-4 p-4 sm:p-6">
            {credentialMessage ? <p role="status" className="break-words text-sm">{credentialMessage}</p> : null}
            <div className="rounded-2xl border border-border bg-surface-container/55 p-4">
              <SiteCredentialList
                site={credentialsTarget}
                credentials={siteCredentials[credentialsTarget.id]}
                revealedKeys={revealedCredentialKeys}
                loadingKeys={loadingCredentialKeys}
                copiedKey={copiedCredentialKey}
                onToggle={handleToggleCredential}
                onCopy={handleCopyCredential}
              />
            </div>
            <div className="flex justify-end">
              <Button variant="secondary" onClick={closeCredentialsDialog}>关闭</Button>
            </div>
          </div>
        ) : null}
      </Dialog>

      {/* ---- add / edit dialog ---- */}
      <Dialog
        open={formOpen}
        onClose={closeForm}
        title={editingId != null ? "编辑站点" : "添加站点"}
        description={
          editingId != null
            ? "修改站点连接配置"
            : "从 PT-Depiler 站点列表快速选择，或添加自定义站点"
        }
        panelClassName="max-w-2xl"
        footer={
          <div className="space-y-2">
            {formError ? <p role="alert" className="break-words text-sm text-red-700">{formError}</p> : null}
            <div className="flex justify-end gap-2">
              <Button variant="secondary" className="h-11" onClick={closeForm} disabled={submitting}>取消</Button>
              <Button type="submit" form="site-connection-form" className={sitePrimaryButtonClassName} disabled={submitting || requestHeadersLoading || Boolean(requestHeadersError)}>
                {submitting && <Loader2 className="mr-2 size-4 animate-spin" />}
                {editingId != null ? "保存" : "添加"}
              </Button>
            </div>
          </div>
        }
      >
        <form id="site-connection-form" noValidate onSubmit={(event) => { event.preventDefault(); handleSubmit(); }} className="p-4 sm:p-6">
          <fieldset disabled={submitting} className="min-w-0 space-y-5">
          <div className="min-w-0 space-y-4">
            {editingId == null ? (
              <div className="space-y-2 rounded-2xl border border-primary/20 bg-primary/5 p-3.5">
                <div className="flex items-center justify-between gap-3">
                  <Label htmlFor="site-preset">站点来源</Label>
                  {sitePresetsLoading ? <span className="text-[11px] text-muted">加载 PTD 列表中…</span> : null}
                </div>
                <Select
                  id="site-preset"
                  value={selectedSitePreset}
                  onChange={applySitePreset}
                  options={sitePresetOptions}
                  searchable
                  searchPlaceholder="搜索站点名称、别名、域名或 PTD ID"
                  emptyMessage="没有匹配的 PTD 站点，可选择自定义站点"
                  aria-describedby="site-preset-help"
                />
                <div id="site-preset-help" className="text-[11px] leading-5 text-muted">
                  {sitePresetsError ? (
                    <span className="text-red-700">
                      {sitePresetsError}。
                      <button type="button" className="cursor-pointer font-bold underline underline-offset-2" onClick={loadSitePresets}>重新加载</button>
                    </span>
                  ) : selectedSitePreset === CUSTOM_SITE_PRESET ? (
                    "自定义模式下可手动填写全部连接信息。"
                  ) : (
                    <>已按 PTD 预设自动填充，仍可按实际入口修改。PTD ID：<span className="font-mono font-bold text-primary">{selectedSitePreset}</span></>
                  )}
                </div>
              </div>
            ) : null}

            <div className="space-y-2">
              <Label htmlFor="site-name">名称</Label>
              <Input
                id="site-name"
                className="text-base"
                required
                aria-invalid={Boolean(fieldErrors.name)}
                aria-describedby={fieldErrors.name ? "site-name-error" : undefined}
                value={form.name}
                onChange={(e) => patch({ name: e.target.value })}
                placeholder="站点名称"
              />
              {fieldErrors.name ? <p id="site-name-error" className="text-sm text-red-700">{fieldErrors.name}</p> : null}
            </div>

            <div className="space-y-2">
              <Label htmlFor="site-type">站点类型</Label>
              <Select
                id="site-type"
                value={form.site_type}
                onChange={(val) => {
                  const v = val as SiteForm["site_type"];
                  setClearAuthConfig(false);
                  patch({
                    site_type: v,
                    auth_type: v === "mteam" ? "api_key" : "cookie",
                  });
                }}
                options={[
                  { value: "nexusphp", label: "NexusPHP" },
                  { value: "mteam", label: "M-Team" },
                  { value: "gazelle", label: "Gazelle" },
                ]}
              />
            </div>

            {form.site_type === "gazelle" ? <p className="text-xs leading-5 text-muted">支持 GPW 等 Gazelle JSON API 站点的连接测试和账户统计；使用 Cookie 登录，暂不支持种子搜索。</p> : null}

            <div className="space-y-2">
              <Label htmlFor="site-base-url">基础 URL</Label>
              <Input
                id="site-base-url"
                className="text-base"
                type="url"
                required
                autoCapitalize="none"
                spellCheck={false}
                aria-invalid={Boolean(fieldErrors.base_url)}
                aria-describedby={fieldErrors.base_url ? "site-url-error" : undefined}
                value={form.base_url}
                onChange={(e) => patch({ base_url: e.target.value })}
                placeholder="https://example.com"
              />
              {fieldErrors.base_url ? <p id="site-url-error" className="text-sm text-red-700">{fieldErrors.base_url}</p> : null}
            </div>

            {renderAuthFields()}

            {canPreserveAuth ? (
              <Label className="flex cursor-pointer items-center gap-2 text-sm">
                <input
                  type="checkbox"
                  className="size-4 accent-primary"
                  checked={clearAuthConfig}
                  onChange={(event) => {
                    const checked = event.target.checked;
                    setClearAuthConfig(checked);
                    if (checked) {
                      patch({ cookie: "", passkey: "", api_key: "" });
                    }
                  }}
                />
                清除已保存的认证凭据
              </Label>
            ) : null}
          </div>

          {requestHeadersError ? (
            <div role="alert" className="space-y-2 rounded-xl border border-destructive/25 p-3 text-sm">
              <p className="font-semibold text-red-700">请求头加载失败，暂时无法保存</p>
              <p className="break-words">{requestHeadersError}</p>
              <Button type="button" variant="outline" className="h-11" onClick={() => { if (editingId != null) loadRequestHeaders(editingId); }}>重新加载请求头</Button>
            </div>
          ) : null}
          <details open={advancedOpen} onToggle={(event) => setAdvancedOpen(event.currentTarget.open)} className="group border-t border-border pt-2">
            <summary className="flex min-h-11 cursor-pointer list-none items-center justify-between gap-3 rounded-lg py-2 text-sm font-medium focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary [&::-webkit-details-marker]:hidden">
              <span>高级配置 <span className="font-normal text-muted">· 请求头 {requestHeadersLoading ? "加载中" : `${form.request_headers.length} 项`}</span></span>
              <ChevronDown className="size-4 shrink-0 transition-transform group-open:rotate-180" />
            </summary>
            <p className="mb-4 mt-1 text-sm leading-6 text-muted">默认请求头适用于常规连接，仅在站点要求特殊请求头时修改。</p>
            <SiteRequestHeadersEditor
              headers={form.request_headers}
              loading={requestHeadersLoading || Boolean(requestHeadersError)}
              onChange={updateRequestHeader}
              onAdd={addRequestHeader}
              onRemove={removeRequestHeader}
              onRestore={restoreDefaultRequestHeaders}
            />
          </details>
          </fieldset>
        </form>
      </Dialog>

      {/* ---- delete confirmation ---- */}
      <Dialog
        open={deleteTarget != null}
        onClose={() => { if (!deleting) { setDeleteTarget(null); setDeleteError(""); } }}
        title="确认删除"
        description={`确定要删除站点「${deleteTarget?.name ?? ""}」吗？此操作不可撤销。`}
      >
        {deleteError ? <p role="alert" className="px-4 pt-4 text-sm text-red-700">{deleteError}</p> : null}
        <div className="flex justify-end gap-2 p-4">
          <Button variant="secondary" disabled={deleting} onClick={() => { setDeleteTarget(null); setDeleteError(""); }}>
            取消
          </Button>
          <Button
            variant="destructive"
            onClick={confirmDelete}
            disabled={deleting}
          >
            {deleting && <Loader2 className="mr-2 h-4 w-4 animate-spin" />}
            删除
          </Button>
        </div>
      </Dialog>

      {/* ---- test result dialog ---- */}
      <Dialog
        open={testOpen}
        onClose={() => setTestOpen(false)}
        title="测试连接"
        description={testTarget ? `${testTarget.name} · 连接测试结果` : "站点连接测试结果"}
      >
        {testing ? (
          <div className="flex items-center justify-center py-8 text-muted">
            <Loader2 className="mr-2 h-5 w-5 animate-spin" />
            测试中…
          </div>
        ) : testResult ? (
          <div className="space-y-4 p-4 sm:p-6">
            <div className="flex items-center gap-2">
              <span
                className={`inline-block h-3 w-3 rounded-full ${testResult.success ? "bg-emerald-500" : "bg-red-500"}`}
              />
              <span className="font-medium">
                {testResult.success ? "连接成功" : "连接失败"}
              </span>
            </div>
            <p className="break-words text-sm text-muted" role="status">{testResult.message}</p>
            {!testResult.success && testTarget ? (
              <div className="flex flex-wrap gap-2">
                <Button className={sitePrimaryButtonClassName} onClick={() => handleTest(testTarget)}>重试测试</Button>
                <Button variant="outline" className="h-11" onClick={() => { setTestOpen(false); openEdit(testTarget); }}>编辑连接配置</Button>
              </div>
            ) : null}

            {testResult.user_stats && (
              <div className="rounded-2xl border border-border bg-surface-container/70 p-4">
                <p className="mb-3 text-sm font-medium">用户信息</p>
                <div className="grid grid-cols-2 gap-3 text-sm">
                  <div>
                    <span className="text-muted">用户名</span>
                    <p className="font-medium">
                      {testResult.user_stats.username}
                    </p>
                  </div>
                  <div>
                    <span className="text-muted">上传量</span>
                    <p className="font-medium">
                      {formatBytes(testResult.user_stats.uploaded)}
                    </p>
                  </div>
                  <div>
                    <span className="text-muted">下载量</span>
                    <p className="font-medium">
                      {formatBytes(testResult.user_stats.downloaded)}
                    </p>
                  </div>
                  <div>
                    <span className="text-muted">分享率</span>
                    <p className="font-medium">
                      {testResult.user_stats.ratio?.toFixed(3) ?? "-"}
                    </p>
                  </div>
                </div>
              </div>
            )}

            <div className="flex justify-end">
              <Button variant="secondary" onClick={() => setTestOpen(false)}>
                关闭
              </Button>
            </div>
          </div>
        ) : null}
      </Dialog>

      {/* ---- overview dialog ---- */}
      <Dialog
        open={overviewOpen}
        onClose={() => setOverviewOpen(false)}
        title="PT 数据总览"
        panelClassName="max-w-7xl"
      >
        {overviewMessage ? <p role="status" className="px-4 pt-4 text-sm">{overviewMessage}</p> : null}
        {overviewLoading ? (
          <div className="flex min-h-[420px] items-center justify-center py-8 text-muted">
            <Loader2 className="mr-2 h-5 w-5 animate-spin" />
            正在读取站点统计数据…
          </div>
        ) : overviewError ? (
          <div role="alert" className="space-y-3 p-6">
            <p className="font-semibold text-red-700">站点总览加载失败</p>
            <p className="break-words text-sm">{overviewError}</p>
            <Button variant="outline" className="h-11" onClick={handleOverview}>重新加载总览</Button>
          </div>
        ) : overviewRows.length === 0 ? (
          <p className="py-8 text-center text-muted">暂无站点统计数据</p>
        ) : (
          <div className="flex flex-col gap-5 p-4 sm:p-6">
            <section className="flex flex-col gap-4 border-b border-border pb-5 sm:flex-row sm:items-center sm:justify-between">
              <div className="min-w-0">
                <h4 className="text-xl font-bold">PT 账号数据</h4>
                <p className="mt-2 text-sm text-muted">共 {overviewRows.length} 个站点 · {failedOverviewRows.length} 个拉取失败 · {overviewRows.filter((row) => !row.stats && !row.error).length} 个待刷新</p>
                <p className="mt-1 text-xs text-muted">生成时间：{overviewGeneratedAt?.toLocaleString() ?? "—"}</p>
              </div>
              <div className="flex shrink-0 gap-2">
                <Button variant="outline" className="h-11 flex-1 sm:flex-none" onClick={() => void handleCopyOverviewImage()} disabled={overviewExporting !== null}>
                  {overviewExporting === "copy" ? <Loader2 className="mr-2 size-4 animate-spin" /> : <Copy className="mr-2 size-4" />}复制图片
                </Button>
                <Button className={`h-11 flex-1 sm:flex-none ${sitePrimaryButtonClassName}`} onClick={() => void handleDownloadOverviewImage()} disabled={overviewExporting !== null}>
                  {overviewExporting === "download" ? <Loader2 className="mr-2 size-4 animate-spin" /> : <DownloadIcon className="mr-2 size-4" />}下载图片
                </Button>
              </div>
            </section>

            <p className="text-sm text-muted">汇总包含此前获取的账户数据；请结合各站点的最近检查时间和错误状态判断时效。</p>
            <section className="grid grid-cols-2 gap-3 xl:grid-cols-4">
              <OverviewMetricCard icon={UploadCloud} label="总上传量" value={successfulOverviewRows.length ? totalUploadedCompact.value : "—"} unit={successfulOverviewRows.length ? totalUploadedCompact.unit : ""} />
              <OverviewMetricCard icon={DownloadCloud} label="总下载量" value={successfulOverviewRows.length ? totalDownloadedCompact.value : "—"} unit={successfulOverviewRows.length ? totalDownloadedCompact.unit : ""} />
              <OverviewMetricCard icon={Gauge} label="综合分享率" value={successfulOverviewRows.length ? formatRatio(totalUploaded, totalDownloaded) : "—"} unit="" />
              <OverviewMetricCard icon={ShieldCheck} label="有账户数据" value={`${successfulOverviewRows.length}`} unit={`/ ${overviewRows.length}`} />
            </section>

            {topOverviewRows.length > 0 ? (
              <section className="rounded-2xl border border-border bg-card p-4 sm:p-5">
                <div className="flex flex-wrap items-center justify-between gap-3">
                  <div>
                    <h4 className="text-base font-black">上传量排行</h4>
                    <p className="mt-1 text-xs text-muted">按上传量取前 {topOverviewRows.length} 个站点</p>
                  </div>
                  <span className="rounded-full border border-primary/15 bg-secondary px-3 py-1 text-xs font-bold text-secondary-foreground">
                    TOP {topOverviewRows.length}
                  </span>
                </div>

                <div className="mt-4 grid grid-cols-1 gap-2 lg:grid-cols-2 2xl:grid-cols-4">
                  {topOverviewRows.map((row, index) => (
                    <OverviewRankCard key={row.site.id} row={row} rank={index + 1} />
                  ))}
                </div>
              </section>
            ) : null}

            <section className="hidden lg:block">
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>站点</TableHead>
                    <TableHead>UID</TableHead>
                    <TableHead>用户名</TableHead>
                    <TableHead>上传量</TableHead>
                    <TableHead>下载量</TableHead>
                    <TableHead>分享率</TableHead>
                    <TableHead>状态 / 最近检查</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {overviewRows.map((row) => (
                    <TableRow key={row.site.id}>
                      <TableCell className="max-w-48 whitespace-normal break-words font-bold">{row.site.name}</TableCell>
                      <TableCell className="font-mono text-xs">{row.stats?.uid ?? "-"}</TableCell>
                      <TableCell className="max-w-40 whitespace-normal break-words">{row.stats?.username ?? "-"}</TableCell>
                      <TableCell className="font-semibold">
                        {row.stats ? formatBytes(row.stats.uploaded) : "-"}
                      </TableCell>
                      <TableCell className="text-muted">
                        {row.stats ? formatBytes(row.stats.downloaded) : "-"}
                      </TableCell>
                      <TableCell className="font-semibold">
                        {row.stats ? formatRatio(row.stats.uploaded, row.stats.downloaded) : "-"}
                      </TableCell>
                      <TableCell className={row.error ? "text-red-600" : "text-muted"}>
                        <div className="max-w-xs"><SiteStatusDetail site={row.site} /></div>
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </section>

            <section className="grid gap-3 lg:hidden">
              {overviewRows.map((row) => (
                <OverviewMobileCard key={row.site.id} row={row} />
              ))}
            </section>

            <div className="flex justify-end print:hidden">
              <Button variant="secondary" onClick={() => setOverviewOpen(false)}>
                关闭
              </Button>
            </div>
          </div>
        )}
      </Dialog>
    </div>
  );
}

function SiteStatusDetail({ site, showBadge = true }: { site: SiteRecord; showBadge?: boolean }) {
  return (
    <div className="space-y-2">
      {showBadge ? <SiteHealthBadge site={site} /> : null}
      {site.stats?.last_error ? <p className="break-words text-sm leading-6 text-red-700">{site.stats.last_error}</p> : null}
      <p className="text-xs leading-5 text-muted">{site.stats?.last_checked_at ? `最近检查：${formatDateTime(site.stats.last_checked_at)}` : "尚未刷新账户数据"}</p>
      {site.stats?.last_error && site.stats.uploaded != null ? <p className="text-xs text-muted">账户数值为此前获取的数据</p> : null}
    </div>
  );
}

function SiteHealthBadge({ site }: { site: SiteRecord }) {
  const health = getSiteHealth(site);
  const styles: Record<SiteHealth, string> = {
    healthy: "bg-emerald-100 text-emerald-700",
    failed: "bg-red-100 text-red-700",
    pending: "bg-amber-100 text-amber-700",
  };
  const labels: Record<SiteHealth, string> = {
    healthy: "拉取成功",
    failed: "失败",
    pending: "待刷新",
  };
  return (
    <span className={`inline-flex shrink-0 items-center rounded-full px-2.5 py-1 text-xs font-medium ${styles[health]}`}>
      {labels[health]}
    </span>
  );
}

function OverviewMetricCard({
  icon: Icon,
  label,
  value,
  unit,
}: {
  icon: typeof UploadCloud;
  label: string;
  value: string;
  unit: string;
}) {
  return (
    <div className="min-w-0 rounded-2xl border border-border bg-card p-4">
      <div className="flex items-center justify-between gap-3">
        <span className="text-sm font-bold text-muted">{label}</span>
        <span className="hidden h-9 w-9 shrink-0 items-center justify-center rounded-xl bg-secondary text-primary sm:flex">
          <Icon className="h-5 w-5" />
        </span>
      </div>
      <div className="mt-4 flex items-end gap-2">
        <span className="min-w-0 break-all text-2xl font-bold tabular-nums tracking-tight text-foreground sm:text-3xl">{value}</span>
        <span className="pb-1 text-sm font-bold text-muted">{unit}</span>
      </div>
    </div>
  );
}

function OverviewRankCard({ row, rank }: { row: SiteOverviewRow; rank: number }) {
  const stats = row.stats;
  return (
    <div className="flex min-w-0 items-center gap-3 rounded-2xl border border-border bg-surface-container/70 p-3">
      <span className="flex h-10 w-10 shrink-0 items-center justify-center rounded-2xl bg-primary text-sm font-black text-primary-foreground">
        {rank}
      </span>
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2">
          <div className="truncate font-black">{row.site.name}</div>
          <Trophy className="h-4 w-4 shrink-0 text-blossom" />
        </div>
        <div className="mt-0.5 truncate text-xs text-muted">UID {stats?.uid ?? "-"} · {stats?.username ?? "-"}</div>
      </div>
      <div className="shrink-0 text-right">
        <div className="text-[11px] font-bold text-muted">上传</div>
        <div className="text-sm font-black">{stats ? formatBytes(stats.uploaded) : "-"}</div>
      </div>
    </div>
  );
}

function OverviewMobileCard({ row }: { row: SiteOverviewRow }) {
  const stats = row.stats;
  return (
    <div className="min-w-0 rounded-2xl border border-border bg-card p-4">
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0 break-words">
          <div className="text-base font-bold">{row.site.name}</div>
          <div className="mt-0.5 text-[11px] text-muted">UID {stats?.uid ?? "-"} · {stats?.username ?? "-"}</div>
        </div>
        <SiteHealthBadge site={row.site} />
      </div>
      <div className="mt-3 grid grid-cols-3 gap-2 text-sm">
        <div className="rounded-xl bg-surface-container/70 p-2.5">
          <div className="text-[10px] font-bold text-muted">上传量</div>
          <div className="mt-0.5 text-sm font-black truncate">{stats ? formatBytes(stats.uploaded) : "-"}</div>
        </div>
        <div className="rounded-xl bg-surface-container/70 p-2.5">
          <div className="text-[10px] font-bold text-muted">下载量</div>
          <div className="mt-0.5 text-sm font-black truncate">{stats ? formatBytes(stats.downloaded) : "-"}</div>
        </div>
        <div className="rounded-xl bg-surface-container/70 p-2.5">
          <div className="text-[10px] font-bold text-muted">分享率</div>
          <div className="mt-0.5 text-sm font-black truncate">{stats ? formatRatio(stats.uploaded, stats.downloaded) : "-"}</div>
        </div>
      </div>
      <div className="mt-3 border-t border-border pt-3"><SiteStatusDetail site={row.site} showBadge={false} /></div>
    </div>
  );
}
