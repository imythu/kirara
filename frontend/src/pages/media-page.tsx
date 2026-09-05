import { useHashChoice, updateHashParams, readQueryDraft, writeQueryDraft } from "@/lib/navigation-state";
import {
  type Dispatch,
  type FormEvent,
  type SetStateAction,
  useCallback,
  useDeferredValue,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
} from "react";
import {
  AlertCircle,
  CheckCircle2,
  ChevronDown,
  CircleDashed,
  CircleX,
  Download,
  Film,
  FileSearch2,
  Globe2,
  HardDriveDownload,
  ImageOff,
  LoaderCircle,
  Pause,
  Pencil,
  Play,
  Plus,
  RefreshCw,
  RotateCcw,
  Search,
  Send,
  Settings2,
  ShieldCheck,
  SlidersHorizontal,
  Trash2,
  Tv,
  X,
} from "lucide-react";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Dialog } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select } from "@/components/ui/select";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import {
  OpenListAutomationPanel,
  type OpenListAutomationSettings,
} from "@/components/openlist-automation-panel";
import { api, ApiError } from "@/lib/api";
import {
  matchesLocalResourceQuery,
  parseLocalReleaseScope,
  type LocalReleaseScope,
  type LocalReleaseScopeKind,
} from "@/lib/media-release-scope";
import { cn } from "@/lib/utils";

const MEDIA_MODES = ["subscriptions", "tmdb", "resources", "settings"] as const;
type MediaMode = typeof MEDIA_MODES[number];
const SETTINGS_SECTIONS = ["media", "quality", "openlist"] as const;
const SETTINGS_LABELS = { media: "媒体设置", quality: "质量配置", openlist: "OpenList" };
type MediaType = "tv" | "movie";

type MediaSettings = {
  tmdb_token: string | null;
  tmdb_token_configured: boolean;
  tmdb_language: string;
  scan_interval_mins: number;
  max_search_queries: number;
  search_concurrency: number;
  updated_at: string;
};

type QualityProfile = {
  id: number;
  name: string;
  resolution_order: string[];
  allowed_resolutions: string[];
  blocked_resolutions: string[];
  source_order: string[];
  allowed_sources: string[];
  codec_order: string[];
  blocked_codecs: string[];
  allow_unknown_quality: boolean;
  minimum_score: number;
  min_seeders: number;
  created_at: string;
  updated_at: string;
};

type Subscription = {
  id: number;
  tmdb_id: number;
  media_type: MediaType | string;
  tmdb_is_animation: boolean;
  tmdb_genres: TmdbGenre[];
  title: string;
  original_title: string | null;
  aliases: string[];
  year: number | null;
  poster_path: string | null;
  season: number | null;
  next_episode: number | null;
  start_episode: number | null;
  absolute_episode: number | null;
  quality_profile_id: number;
  downloader_id: number;
  site_ids: number[];
  save_path: string | null;
  enabled: boolean;
  next_run_at: string;
  last_status: string | null;
  last_error: string | null;
  last_run_at: string | null;
  created_at: string;
  updated_at: string;
};

type MediaDownload = {
  id: number;
  version: number;
  subscription_id: number | null;
  target_key: string;
  site_id: number | null;
  downloader_id: number | null;
  source_site: string;
  downloader_name: string;
  torrent_id: string;
  title: string;
  size: number;
  status: string;
  attempts: number;
  infohash: string | null;
  next_attempt_at: string | null;
  last_error: string | null;
  created_at: string;
  updated_at: string;
  submitted_at: string | null;
  parsed_release: ReleaseInfo | null;
  failed_reconciliation_allowed: boolean;
};

type DeleteMediaDownloadResponse = {
  deleted_id: number;
  subscription_id: number | null;
  target_reopened: boolean;
  qb_torrent_deleted: false;
  openlist_data_deleted: false;
};

type ReconcileFailedMediaDownloadResponse = MediaDownload & {
  resolution: "submitted" | "retry_ready";
};

type SubscriptionRunResult = {
  query_count: number;
  candidate_count: number;
  accepted_count: number;
  download: unknown | null;
  site_errors: unknown[];
};

type SiteSearchError = {
  site_id: number;
  source_site: string;
  query: string;
  code: string;
  message: string;
};

type SubscriptionRunSnapshot = {
  started_at: string;
  finished_at: string;
  target_key: string;
  queries: string[];
  candidates: ResourceCandidate[];
  site_errors: SiteSearchError[];
  total_sites: number;
  successful_sites: number;
  best_candidate_id: string | null;
  error: string | null;
};

type TmdbMedia = {
  tmdb_id: number;
  media_type: MediaType;
  title: string;
  original_title: string | null;
  year: number | null;
  overview: string;
  poster_path: string | null;
  is_animation: boolean;
  genres: TmdbGenre[];
};

type TmdbGenre = { id: number; name: string };

type TmdbDetails = TmdbMedia & {
  aliases: string[];
  number_of_seasons: number | null;
  status: string | null;
  seasons?: TmdbSeasonSummary[];
};

type TmdbSeasonSummary = {
  season_number: number;
};

type TmdbEpisode = {
  episode_number: number;
};

type TmdbSeason = {
  tmdb_id: number;
  season_number: number;
  episodes: TmdbEpisode[];
};

type SeasonMetadataState = {
  tmdbId: number | null;
  season: number | null;
  status: "idle" | "loading" | "ready" | "error";
  episodes: number[];
  error: string;
};

type TvEpisodeCursor = {
  season: number;
  episode: number;
};

type Site = {
  id: number;
  name: string;
  site_type: string;
  base_url: string;
};

type Downloader = {
  id: number;
  name: string;
  downloader_type: string;
  url: string;
};

type IndexerResult = {
  site_id: number;
  source_site: string;
  torrent_id: string;
  title: string;
  detail_url: string | null;
  download_locator: string | null;
  magnet: string | null;
  size: number;
  seeders: number;
  leechers: number;
  publish_time: string | null;
};

type ReleaseInfo = {
  raw_title: string;
  title: string;
  alternate_titles: string[];
  year: number | null;
  season: number | null;
  episodes: number[];
  absolute_episodes: number[];
  full_season: boolean;
  resolution: string | null;
  codec: string | null;
  source: string | null;
  hdr_formats: string[];
  bit_depth: number | null;
  revision: string | null;
  release_group: string | null;
  matched_rule: string;
};

type MatchRejection = {
  code: string;
  message: string;
  permanent: boolean;
};

type MatchDecision = {
  accepted: boolean;
  score: number;
  breakdown: Record<string, number>;
  quality_rank: number;
  rejections: MatchRejection[];
  explanations: string[];
};

type CandidateSortKey = {
  resolution_rank: number;
  video_feature_rank: number;
  source_rank: number;
  size_fitness: number;
  size_per_item: number;
  size_target: number;
  codec_rank: number;
};

type ResourceCandidate = {
  candidateId: string;
  key: string;
  result: IndexerResult;
  release: ReleaseInfo | null;
  parseError: string | null;
  decision: MatchDecision | null;
  sortKey: CandidateSortKey | null;
  rank: number;
  raw: Record<string, unknown>;
};

type ScopedResourceCandidate = {
  candidate: ResourceCandidate;
  localScope: LocalReleaseScope;
};

type MediaTarget =
  | { target_type: "movie"; tmdb_id: number; titles: string[]; year: number | null }
  | {
      target_type: "episode";
      tmdb_id: number;
      titles: string[];
      year: number | null;
      season: number;
      episode: number;
      allow_season_pack: boolean;
    }
  | {
      target_type: "anime";
      tmdb_id: number;
      titles: string[];
      year: number | null;
      absolute_episode: number;
      season_episode: { season: number; episode: number } | null;
    };

type SubscriptionForm = {
  numberingMode: "season" | "absolute";
  season: number;
  startEpisode: number;
  absoluteEpisode: number;
  qualityProfileId: string;
  downloaderId: string;
  siteIds: number[];
  savePath: string;
};

type ResourceForm = {
  query: string;
  subscriptionId: string;
  qualityProfileId: string;
  downloaderId: string;
  siteIds: number[];
};

type QualityForm = {
  name: string;
  resolutionOrder: string;
  allowedResolutions: string;
  blockedResolutions: string;
  sourceOrder: string;
  allowedSources: string;
  codecOrder: string;
  blockedCodecs: string;
  allowUnknownQuality: boolean;
  minimumScore: number;
  minSeeders: number;
};

type QualityPreset = "tv-balanced" | "tv-4k" | "movie-collection" | "movie-balanced" | "anime-balanced" | "anime-compact" | "custom";

type Notice = { tone: "success" | "error"; text: string };
type UnknownRecord = Record<string, unknown>;

const DEFAULT_SETTINGS: MediaSettings = {
  tmdb_token: null,
  tmdb_token_configured: false,
  tmdb_language: "zh-CN",
  scan_interval_mins: 30,
  max_search_queries: 8,
  search_concurrency: 4,
  updated_at: "",
};

const DEFAULT_OPENLIST_SETTINGS: OpenListAutomationSettings = {
  address: "",
  api_key: null,
  api_key_configured: false,
  enabled: false,
  scan_interval_mins: 5,
  source_mappings: [],
  target_directories: [],
  target_directory_id: null,
  updated_at: "",
  clear_api_key: false,
};

const EMPTY_SUBSCRIPTION_FORM: SubscriptionForm = {
  numberingMode: "season",
  season: 1,
  startEpisode: 1,
  absoluteEpisode: 1,
  qualityProfileId: "",
  downloaderId: "",
  siteIds: [],
  savePath: "",
};

const EMPTY_SEASON_METADATA: SeasonMetadataState = {
  tmdbId: null,
  season: null,
  status: "idle",
  episodes: [],
  error: "",
};

const EMPTY_RESOURCE_FORM: ResourceForm = {
  query: "",
  subscriptionId: "",
  qualityProfileId: "",
  downloaderId: "",
  siteIds: [],
};

const RESOURCE_SITE_IDS_STORAGE_KEY = "kirara.media.resource-site-ids";
const DOWNLOAD_PAGE_SIZE = 20;

function storedResourceSiteIds(): number[] | null {
  if (typeof window === "undefined") return null;
  try {
    const value = window.localStorage.getItem(RESOURCE_SITE_IDS_STORAGE_KEY)
      ?? window.localStorage.getItem("rflush.media.resource-site-ids");
    if (value === null) return null;
    const parsed: unknown = JSON.parse(value);
    if (!Array.isArray(parsed)) return null;
    return [...new Set(parsed.filter((id): id is number => Number.isInteger(id) && id > 0))];
  } catch {
    return null;
  }
}

const EMPTY_QUALITY_FORM: QualityForm = {
  name: "",
  resolutionOrder: "2160p, 1080p, 720p",
  allowedResolutions: "2160p, 1080p, 720p",
  blockedResolutions: "480p",
  sourceOrder: "WEB-DL, BluRay, WEBRip",
  allowedSources: "WEB-DL, BluRay, WEBRip",
  codecOrder: "H265, H264, AV1",
  blockedCodecs: "",
  allowUnknownQuality: false,
  minimumScore: 80,
  minSeeders: 1,
};

const QUALITY_PRESETS: Array<{
  id: Exclude<QualityPreset, "custom">;
  name: string;
  description: string;
  detail: string;
  form: Omit<QualityForm, "name">;
}> = [
  {
    id: "tv-balanced",
    name: "电视剧 · 日常",
    description: "1080p WEB-DL 优先，更新快且稳定",
    detail: "兼顾画质、体积和日常追更速度",
    form: {
      resolutionOrder: "1080p, 2160p, 720p", allowedResolutions: "2160p, 1080p, 720p", blockedResolutions: "480p",
      sourceOrder: "WEB-DL, BluRay, WEBRip", allowedSources: "WEB-DL, BluRay, WEBRip", codecOrder: "H265, H264, AV1",
      blockedCodecs: "", allowUnknownQuality: false, minimumScore: 65, minSeeders: 1,
    },
  },
  {
    id: "tv-4k",
    name: "电视剧 · 4K",
    description: "4K WEB-DL 优先，适合大屏追剧",
    detail: "保留 1080p 作为备选，避免长时间等不到资源",
    form: {
      resolutionOrder: "2160p, 1080p", allowedResolutions: "2160p, 1080p", blockedResolutions: "720p, 480p",
      sourceOrder: "WEB-DL, BluRay, WEBRip", allowedSources: "WEB-DL, BluRay, WEBRip", codecOrder: "H265, AV1, H264",
      blockedCodecs: "", allowUnknownQuality: false, minimumScore: 65, minSeeders: 1,
    },
  },
  {
    id: "movie-collection",
    name: "电影 · 收藏",
    description: "优先 4K、REMUX 和 BluRay",
    detail: "适合收藏电影，文件通常较大",
    form: {
      resolutionOrder: "2160p, 1080p", allowedResolutions: "2160p, 1080p", blockedResolutions: "720p, 480p",
      sourceOrder: "REMUX, BluRay, WEB-DL", allowedSources: "REMUX, BluRay, WEB-DL", codecOrder: "H265, AV1, H264",
      blockedCodecs: "", allowUnknownQuality: false, minimumScore: 70, minSeeders: 1,
    },
  },
  {
    id: "movie-balanced",
    name: "电影 · 均衡",
    description: "1080p BluRay / WEB-DL 优先",
    detail: "画质稳定、体积适中，适合日常观影",
    form: {
      resolutionOrder: "1080p, 2160p, 720p", allowedResolutions: "2160p, 1080p, 720p", blockedResolutions: "480p",
      sourceOrder: "BluRay, WEB-DL, WEBRip", allowedSources: "BluRay, WEB-DL, WEBRip", codecOrder: "H265, H264, AV1",
      blockedCodecs: "", allowUnknownQuality: false, minimumScore: 65, minSeeders: 1,
    },
  },
  {
    id: "anime-balanced",
    name: "动漫 · 日常",
    description: "4K 优先，兼容常见字幕组命名",
    detail: "优先 BluRay / WEB-DL，并接受信息不完整的发布",
    form: {
      resolutionOrder: "2160p, 1080p, 720p", allowedResolutions: "2160p, 1080p, 720p", blockedResolutions: "480p",
      sourceOrder: "BluRay, WEB-DL, WEBRip", allowedSources: "BluRay, WEB-DL, WEBRip", codecOrder: "H265, H264, AV1",
      blockedCodecs: "", allowUnknownQuality: true, minimumScore: 60, minSeeders: 1,
    },
  },
  {
    id: "anime-compact",
    name: "动漫 · 省空间",
    description: "优先 H.265 / AV1 的 1080p 版本",
    detail: "适合长期追番和存储空间有限的设备",
    form: {
      resolutionOrder: "1080p, 720p", allowedResolutions: "1080p, 720p", blockedResolutions: "2160p, 480p",
      sourceOrder: "WEB-DL, WEBRip, BluRay", allowedSources: "WEB-DL, WEBRip, BluRay", codecOrder: "H265, AV1, H264",
      blockedCodecs: "", allowUnknownQuality: true, minimumScore: 55, minSeeders: 1,
    },
  },
];

const MODES: Array<{ value: MediaMode; label: string; icon: typeof Tv }> = [
  { value: "subscriptions", label: "订阅", icon: Tv },
  { value: "tmdb", label: "添加影视", icon: Plus },
  { value: "resources", label: "资源搜索", icon: Search },
  { value: "settings", label: "质量与设置", icon: SlidersHorizontal },
];

const LOCAL_SCOPE_FILTERS: Array<{
  value: "all" | LocalReleaseScopeKind;
  label: string;
}> = [
  { value: "all", label: "全部" },
  { value: "season_episode", label: "季 + 集" },
  { value: "season_only", label: "仅季 · 无集数" },
  { value: "season_pack", label: "全季" },
  { value: "episode_only", label: "仅集 · 无季数" },
  { value: "absolute_episode", label: "绝对集" },
  { value: "unmatched", label: "未识别" },
];

function isRecord(value: unknown): value is UnknownRecord {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function readArray<T>(payload: unknown, keys: string[]): T[] {
  if (Array.isArray(payload)) return payload as T[];
  if (!isRecord(payload)) return [];
  for (const key of keys) {
    const value = payload[key];
    if (Array.isArray(value)) return value as T[];
    if (isRecord(value)) {
      const nested = readArray<T>(value, keys);
      if (nested.length > 0) return nested;
    }
  }
  return [];
}

function readObject<T>(payload: unknown, keys: string[]): T {
  if (isRecord(payload)) {
    for (const key of keys) {
      if (isRecord(payload[key])) return payload[key] as T;
    }
  }
  return payload as T;
}

function numberValue(value: unknown, fallback = 0): number {
  if (typeof value === "number" && Number.isFinite(value)) return value;
  if (typeof value === "string") {
    const parsed = Number(value);
    if (Number.isFinite(parsed)) return parsed;
  }
  return fallback;
}

function optionalString(value: unknown): string | null {
  return typeof value === "string" && value.trim() ? value : null;
}

function stringList(value: unknown): string[] {
  if (!Array.isArray(value)) return [];
  return value
    .map((item) => {
      if (typeof item === "string") return item;
      if (isRecord(item)) {
        return optionalString(item.message) ?? optionalString(item.reason) ?? optionalString(item.code) ?? "";
      }
      return "";
    })
    .filter(Boolean);
}

function numberList(value: unknown): number[] {
  if (!Array.isArray(value)) return [];
  return value.map((item) => numberValue(item)).filter((item) => item > 0);
}

function positiveInteger(value: unknown): number | null {
  const parsed = numberValue(value);
  return Number.isInteger(parsed) && parsed > 0 ? parsed : null;
}

function nonNegativeInteger(value: unknown): number | null {
  const parsed = numberValue(value);
  return Number.isInteger(parsed) && parsed >= 0 ? parsed : null;
}

function seasonCount(details: TmdbDetails | null): number | null {
  return positiveInteger(details?.number_of_seasons);
}

function listedSeasonNumbers(details: TmdbDetails | null): number[] {
  if (!Array.isArray(details?.seasons)) return [];
  return Array.from(
    new Set(
      details.seasons
        .map((season) => nonNegativeInteger(season?.season_number))
        .filter((season): season is number => season != null),
    ),
  ).sort((left, right) => left - right);
}

function selectableSeasonNumbers(details: TmdbDetails | null): number[] {
  const listed = listedSeasonNumbers(details);
  if (listed.length > 0) return listed;
  const count = seasonCount(details);
  return count == null ? [] : Array.from({ length: count + 1 }, (_, season) => season);
}

function episodeNumbers(payload: unknown): number[] {
  const season = readObject<TmdbSeason>(payload, ["season", "data"]);
  if (!Array.isArray(season?.episodes)) return [];
  return Array.from(
    new Set(
      season.episodes
        .map((episode) => positiveInteger(episode?.episode_number))
        .filter((episode): episode is number => episode != null),
    ),
  ).sort((left, right) => left - right);
}

function metadataMatches(
  metadata: SeasonMetadataState,
  tmdbId: number,
  season: number,
): boolean {
  return metadata.tmdbId === tmdbId && metadata.season === season;
}

function tvMetadataSelectionIsValid(
  tmdbId: number,
  form: SubscriptionForm,
  details: TmdbDetails | null,
  detailsLoading: boolean,
  metadata: SeasonMetadataState,
  preservedTarget: TvEpisodeCursor | null = null,
): boolean {
  if (
    detailsLoading ||
    !Number.isInteger(form.season) ||
    form.season < 0 ||
    !Number.isInteger(form.startEpisode) ||
    form.startEpisode < 1 ||
    (form.numberingMode === "absolute" &&
      (!Number.isInteger(form.absoluteEpisode) || form.absoluteEpisode < 1))
  ) {
    return false;
  }

  const seasons = selectableSeasonNumbers(details);
  const targetIsPreserved =
    preservedTarget?.season === form.season && preservedTarget.episode === form.startEpisode;
  if (seasons.length > 0 && !seasons.includes(form.season) && preservedTarget?.season !== form.season) {
    return false;
  }
  if (!metadataMatches(metadata, tmdbId, form.season)) return false;
  if (metadata.status === "idle" || metadata.status === "loading") return false;
  return (
    metadata.status === "error" ||
    metadata.episodes.length === 0 ||
    metadata.episodes.includes(form.startEpisode) ||
    targetIsPreserved
  );
}

function describeUnknown(value: unknown): string {
  if (value instanceof Error) return value.message;
  if (typeof value === "string") return value;
  if (isRecord(value)) {
    return optionalString(value.message) ?? optionalString(value.error) ?? optionalString(value.code) ?? "请求失败";
  }
  return "请求失败";
}

function splitValues(value: string): string[] {
  return value
    .split(/[,，\n]/)
    .map((item) => item.trim())
    .filter((item, index, all) => item.length > 0 && all.indexOf(item) === index);
}

function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB", "PB"];
  const index = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  const value = bytes / 1024 ** index;
  return `${value >= 100 || index === 0 ? value.toFixed(0) : value.toFixed(1)} ${units[index]}`;
}

function formatDate(value: string | null | undefined): string {
  if (!value) return "-";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return new Intl.DateTimeFormat("zh-CN", {
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
  }).format(date);
}

function posterUrl(path: string | null | undefined): string | null {
  if (!path) return null;
  if (/^https?:\/\//i.test(path)) return path;
  return `https://image.tmdb.org/t/p/w342${path.startsWith("/") ? path : `/${path}`}`;
}

function subscriptionStatus(subscription: Subscription): { label: string; tone: "positive" | "negative" | "neutral" } {
  const status = subscription.last_status?.toLowerCase();
  if (status === "completed") return { label: "已完成", tone: "positive" };
  if (!subscription.enabled) return { label: "已暂停", tone: "neutral" };
  if (subscription.last_error) return { label: "需处理", tone: "negative" };
  if (status === "running" || status === "searching") return { label: "扫描中", tone: "positive" };
  if (status === "queued") return { label: "已入队", tone: "positive" };
  if (status === "submitted") return { label: "已提交下载器", tone: "positive" };
  if (status === "waiting_air_date") return { label: "待播出", tone: "neutral" };
  if (status === "awaiting_metadata") return { label: "等待 TMDB 更新", tone: "neutral" };
  return { label: "等待扫描", tone: "neutral" };
}

function subscriptionIsCompleted(subscription: Subscription): boolean {
  return subscription.last_status?.toLowerCase() === "completed";
}

function subscriptionTargetLabel(subscription: Subscription, mobile = false): string {
  if (subscriptionIsCompleted(subscription)) {
    return subscription.media_type === "movie" ? "电影已完成" : "季已完成";
  }
  if (subscription.media_type === "movie") return "电影";
  if (subscription.absolute_episode != null) {
    return mobile
      ? `绝对集 ${subscription.absolute_episode}`
      : `Abs ${String(subscription.absolute_episode).padStart(4, "0")}`;
  }
  const season = subscription.season ?? 1;
  const episode = subscription.next_episode ?? subscription.start_episode ?? 1;
  return mobile
    ? `第 ${season} 季 · 第 ${episode} 集`
    : `S${String(season).padStart(2, "0")}E${String(episode).padStart(2, "0")}`;
}

function downloadStatus(status: string): string {
  const labels: Record<string, string> = {
    queued: "等待处理",
    fetching: "正在取种",
    submitting: "正在提交",
    submitted: "已提交下载器",
    retry_wait: "等待重试",
    reconciling: "正在对账",
    failed: "失败",
    cancelled: "已取消",
  };
  return labels[status] ?? status;
}

function downloadTone(status: string): "positive" | "negative" | "neutral" {
  if (status === "submitted") return "positive";
  if (status === "failed" || status === "cancelled") return "negative";
  return "neutral";
}

function targetKeyLabel(targetKey: string): string {
  const seasonEpisode = targetKey.match(/:s(\d+)e(\d+)$/i);
  if (seasonEpisode) return `S${seasonEpisode[1].padStart(2, "0")}E${seasonEpisode[2].padStart(2, "0")}`;
  const absoluteEpisode = targetKey.match(/:abs(\d+)$/i);
  if (absoluteEpisode) return `绝对集 ${Number(absoluteEpisode[1])}`;
  if (targetKey.startsWith("movie:")) return "电影";
  return targetKey;
}

function releaseScopeLabel(release: ReleaseInfo | null | undefined): string | null {
  if (!release) return null;
  if (release.full_season) return `S${String(release.season ?? 1).padStart(2, "0")} 全季`;
  if (release.episodes.length > 0) {
    const season = release.season == null ? "" : `S${String(release.season).padStart(2, "0")}`;
    const episodes = compactEpisodeList(release.episodes, "E");
    return `${season}${episodes}`;
  }
  if (release.absolute_episodes.length > 0) return `绝对集 ${compactEpisodeList(release.absolute_episodes)}`;
  return null;
}

function compactEpisodeList(episodes: number[], prefix = ""): string {
  const values = Array.from(new Set(episodes)).sort((left, right) => left - right);
  const format = (value: number) => `${prefix}${prefix ? String(value).padStart(2, "0") : value}`;
  const contiguous = values.length > 1 && values.every((value, index) => index === 0 || value === values[index - 1] + 1);
  if (contiguous) return `${format(values[0])}-${format(values[values.length - 1])}`;
  return values.map(format).join(",");
}

function releaseQualityFields(release: ReleaseInfo | null | undefined): string[] {
  if (!release) return [];
  return [
    release.resolution,
    ...(release.hdr_formats ?? []),
    release.bit_depth ? `${release.bit_depth}bit` : null,
    release.source,
    release.codec,
    release.revision,
    releaseScopeLabel(release),
  ]
    .filter((field): field is string => Boolean(field));
}

function downloadMatchesCandidate(
  download: MediaDownload,
  candidate: ResourceCandidate,
  subscriptionId: number,
  targetKey: string,
): boolean {
  return download.subscription_id === subscriptionId
    && download.target_key === targetKey
    && download.site_id === candidate.result.site_id
    && download.torrent_id === candidate.result.torrent_id;
}

function downloadNotice(download: MediaDownload): { text: string; negative: boolean } | null {
  if (!download.last_error) return null;
  if (download.last_error.startsWith("torrent already submitted by media download")) {
    return { text: "检测到相同种子，已沿用此前提交记录，没有重复添加。", negative: false };
  }
  if (download.last_error.includes("response is not a bencoded torrent dictionary")) {
    return { text: "站点返回的不是有效种子文件。", negative: true };
  }
  if (download.last_error.includes("tracker returned HTTP 302")) {
    return { text: "站点下载地址发生重定向，取种失败。", negative: true };
  }
  return { text: download.last_error, negative: download.status === "failed" || download.status === "cancelled" };
}

function targetForSubscription(subscription: Subscription): MediaTarget {
  const titles = [subscription.title, subscription.original_title, ...subscription.aliases]
    .filter((value): value is string => Boolean(value?.trim()))
    .filter((value, index, all) => all.indexOf(value) === index);
  if (subscription.media_type === "movie") {
    return {
      target_type: "movie",
      tmdb_id: subscription.tmdb_id,
      titles,
      year: subscription.year,
    };
  }
  if (subscription.absolute_episode != null) {
    return {
      target_type: "anime",
      tmdb_id: subscription.tmdb_id,
      titles,
      year: subscription.year,
      absolute_episode: subscription.absolute_episode,
      season_episode:
        subscription.season != null && subscription.next_episode != null
          ? { season: subscription.season, episode: subscription.next_episode }
          : null,
    };
  }
  return {
    target_type: "episode",
    tmdb_id: subscription.tmdb_id,
    titles,
    year: subscription.year,
    season: subscription.season ?? 1,
    episode: subscription.next_episode ?? subscription.start_episode ?? 1,
    allow_season_pack: false,
  };
}

function normalizeCandidate(value: unknown, index: number): ResourceCandidate | null {
  if (!isRecord(value)) return null;
  const candidateId = optionalString(value.candidate_id);
  if (!candidateId) return null;
  const resultRaw = isRecord(value.result)
    ? value.result
    : isRecord(value.search_result)
      ? value.search_result
      : value;
  const torrentId = optionalString(resultRaw.torrent_id) ?? String(index + 1);
  const siteId = numberValue(resultRaw.site_id);
  const title = optionalString(resultRaw.title) ?? "未命名资源";
  const result: IndexerResult = {
    site_id: siteId,
    source_site: optionalString(resultRaw.source_site) ?? "未知站点",
    torrent_id: torrentId,
    title,
    detail_url: optionalString(resultRaw.detail_url),
    download_locator: optionalString(resultRaw.download_locator),
    magnet: optionalString(resultRaw.magnet),
    size: numberValue(resultRaw.size),
    seeders: numberValue(resultRaw.seeders),
    leechers: numberValue(resultRaw.leechers),
    publish_time: optionalString(resultRaw.publish_time),
  };

  let release: ReleaseInfo | null = null;
  if (isRecord(value.release)) {
    release = {
      raw_title: optionalString(value.release.raw_title) ?? title,
      title: optionalString(value.release.title) ?? title,
      alternate_titles: stringList(value.release.alternate_titles),
      year: value.release.year == null ? null : numberValue(value.release.year),
      season: value.release.season == null ? null : numberValue(value.release.season),
      episodes: numberList(value.release.episodes),
      absolute_episodes: numberList(value.release.absolute_episodes),
      full_season: Boolean(value.release.full_season),
      resolution: optionalString(value.release.resolution),
      codec: optionalString(value.release.codec),
      source: optionalString(value.release.source),
      hdr_formats: stringList(value.release.hdr_formats),
      bit_depth: value.release.bit_depth == null ? null : numberValue(value.release.bit_depth),
      revision: optionalString(value.release.revision),
      release_group: optionalString(value.release.release_group),
      matched_rule: optionalString(value.release.matched_rule) ?? "unknown",
    };
  }

  let decision: MatchDecision | null = null;
  if (isRecord(value.decision)) {
    const rejections = Array.isArray(value.decision.rejections)
      ? value.decision.rejections.map((item) => {
          const rejection = isRecord(item) ? item : {};
          return {
            code: optionalString(rejection.code) ?? "rejected",
            message: optionalString(rejection.message) ?? optionalString(rejection.code) ?? "不符合目标",
            permanent: rejection.permanent !== false,
          };
        })
      : [];
    const breakdownRaw = isRecord(value.decision.breakdown) ? value.decision.breakdown : {};
    decision = {
      accepted: value.decision.accepted === true,
      score: numberValue(value.decision.score),
      breakdown: Object.fromEntries(
        Object.entries(breakdownRaw).map(([key, item]) => [key, numberValue(item)]),
      ),
      quality_rank: numberValue(value.decision.quality_rank),
      rejections,
      explanations: stringList(value.decision.explanations),
    };
  }

  const sortKeyRaw = isRecord(value.sort_key) ? value.sort_key : null;
  const sortKey: CandidateSortKey | null = sortKeyRaw
    ? {
        resolution_rank: numberValue(sortKeyRaw.resolution_rank),
        video_feature_rank: numberValue(sortKeyRaw.video_feature_rank),
        source_rank: numberValue(sortKeyRaw.source_rank),
        size_fitness: numberValue(sortKeyRaw.size_fitness),
        size_per_item: numberValue(sortKeyRaw.size_per_item),
        size_target: numberValue(sortKeyRaw.size_target),
        codec_rank: numberValue(sortKeyRaw.codec_rank),
      }
    : null;

  return {
    candidateId,
    key: candidateId,
    result,
    release,
    parseError: value.parse_error == null ? null : describeUnknown(value.parse_error),
    decision,
    sortKey,
    rank: numberValue(value.rank, index + 1),
    raw: value,
  };
}

function normalizeRunSnapshot(value: unknown): SubscriptionRunSnapshot {
  if (!isRecord(value)) throw new Error("运行详情格式无效");
  const candidates = (Array.isArray(value.candidates) ? value.candidates : [])
    .map(normalizeCandidate)
    .filter((candidate): candidate is ResourceCandidate => candidate !== null);
  const siteErrors = (Array.isArray(value.site_errors) ? value.site_errors : []).map((item) => {
    const error = isRecord(item) ? item : {};
    return {
      site_id: numberValue(error.site_id),
      source_site: optionalString(error.source_site) ?? "未知站点",
      query: optionalString(error.query) ?? "",
      code: optionalString(error.code) ?? "unknown",
      message: optionalString(error.message) ?? describeUnknown(item),
    };
  });
  return {
    started_at: optionalString(value.started_at) ?? "",
    finished_at: optionalString(value.finished_at) ?? optionalString(value.started_at) ?? "",
    target_key: optionalString(value.target_key) ?? "-",
    queries: stringList(value.queries),
    candidates,
    site_errors: siteErrors,
    total_sites: numberValue(value.total_sites),
    successful_sites: numberValue(value.successful_sites),
    best_candidate_id: optionalString(value.best_candidate_id),
    error: optionalString(value.error),
  };
}

function candidateNeedsOverride(candidate: ResourceCandidate): boolean {
  return Boolean(candidate.parseError || (candidate.decision && !candidate.decision.accepted));
}

function extractSiteErrors(payload: unknown): string[] {
  if (!isRecord(payload)) return [];
  const raw = payload.errors ?? payload.site_errors;
  if (Array.isArray(raw)) {
    return raw.map((item) => {
      if (!isRecord(item)) return describeUnknown(item);
      const site = optionalString(item.source_site)
        ?? optionalString(item.site_name)
        ?? (item.site_id != null ? `站点 #${numberValue(item.site_id)}` : null);
      const message = describeUnknown(item);
      return site ? `${site}: ${message}` : message;
    });
  }
  if (isRecord(raw)) {
    return Object.entries(raw).map(([site, error]) => `${site}: ${describeUnknown(error)}`);
  }
  return raw ? [describeUnknown(raw)] : [];
}

export function MediaPage() {
  const [mode, setMode] = useHashChoice<MediaMode>("mode", MEDIA_MODES, "subscriptions");
  const [settings, setSettings] = useState<MediaSettings>(DEFAULT_SETTINGS);
  const [clearTmdbToken, setClearTmdbToken] = useState(false);
  const [profiles, setProfiles] = useState<QualityProfile[]>([]);
  const [subscriptions, setSubscriptions] = useState<Subscription[]>([]);
  const [downloads, setDownloads] = useState<MediaDownload[]>([]);
  const [downloadCursor, setDownloadCursor] = useState<number | null>(null);
  const [downloadsHaveMore, setDownloadsHaveMore] = useState(false);
  const [downloadsLoadingMore, setDownloadsLoadingMore] = useState(false);
  const [sites, setSites] = useState<Site[]>([]);
  const [downloaders, setDownloaders] = useState<Downloader[]>([]);
  const [openListSettings, setOpenListSettings] = useState<OpenListAutomationSettings | null>(null);
  const [initialLoading, setInitialLoading] = useState(true);
  const [loadError, setLoadError] = useState("");
  const [failedSections, setFailedSections] = useState<string[]>([]);
  const [createError, setCreateError] = useState("");
  const [qualityError, setQualityError] = useState("");
  const [notice, setNotice] = useState<Notice | null>(null);
  const [busyKey, setBusyKey] = useState("");

  const [tmdbQuery, setTmdbQuery] = useState(() => readQueryDraft("tmdb"));
  useEffect(() => writeQueryDraft("tmdb", tmdbQuery), [tmdbQuery]);
  const [tmdbType, setTmdbType] = useState<"multi" | MediaType>("multi");
  const [tmdbResults, setTmdbResults] = useState<TmdbMedia[]>([]);
  const [tmdbLoading, setTmdbLoading] = useState(false);
  const [tmdbSearched, setTmdbSearched] = useState(false);
  const [pendingMedia, setPendingMedia] = useState<TmdbMedia | null>(null);
  const [selectedMedia, setSelectedMedia] = useState<TmdbMedia | null>(null);
  const [selectedDetails, setSelectedDetails] = useState<TmdbDetails | null>(null);
  const [detailsLoading, setDetailsLoading] = useState(false);
  const [detailsError, setDetailsError] = useState("");
  const [createSeasonMetadata, setCreateSeasonMetadata] = useState<SeasonMetadataState>(EMPTY_SEASON_METADATA);
  const [subscriptionForm, setSubscriptionForm] = useState<SubscriptionForm>(EMPTY_SUBSCRIPTION_FORM);
  const [editingSubscription, setEditingSubscription] = useState<Subscription | null>(null);
  const [editingDetails, setEditingDetails] = useState<TmdbDetails | null>(null);
  const [editingDetailsLoading, setEditingDetailsLoading] = useState(false);
  const [editingDetailsError, setEditingDetailsError] = useState("");
  const [editSeasonMetadata, setEditSeasonMetadata] = useState<SeasonMetadataState>(EMPTY_SEASON_METADATA);
  const [editSubscriptionForm, setEditSubscriptionForm] = useState<SubscriptionForm>(EMPTY_SUBSCRIPTION_FORM);
  const [editSubscriptionError, setEditSubscriptionError] = useState("");
  const [resetDownloadHistory, setResetDownloadHistory] = useState(false);
  const [resetHistoryConfirmOpen, setResetHistoryConfirmOpen] = useState(false);
  const createDetailsGeneration = useRef(0);
  const createSeasonGeneration = useRef(0);
  const editDetailsGeneration = useRef(0);
  const editSeasonGeneration = useRef(0);
  const downloadListGeneration = useRef(0);
  const downloadListReloading = useRef(false);
  const downloadLoadMoreController = useRef<AbortController | null>(null);

  const rememberedResourceSiteIds = useRef<number[] | null>(null);
  const [resourceForm, setResourceForm] = useState<ResourceForm>(() => {
    rememberedResourceSiteIds.current = storedResourceSiteIds();
    return {
      ...EMPTY_RESOURCE_FORM,
      query: readQueryDraft("resources"),
      siteIds: rememberedResourceSiteIds.current ?? [],
    };
  });
  useEffect(() => writeQueryDraft("resources", resourceForm.query), [resourceForm.query]);
  const [resourceCandidates, setResourceCandidates] = useState<ResourceCandidate[]>([]);
  const [resourceErrors, setResourceErrors] = useState<string[]>([]);
  const [resourceTmdbResults, setResourceTmdbResults] = useState<TmdbMedia[]>([]);
  const [resourceLoading, setResourceLoading] = useState(false);
  const [queuedCandidateKeys, setQueuedCandidateKeys] = useState<Set<string>>(() => new Set());
  const [resourceSearched, setResourceSearched] = useState(false);
  const [overrideCandidate, setOverrideCandidate] = useState<ResourceCandidate | null>(null);
  const [overrideReason, setOverrideReason] = useState("");

  const [qualityDialogOpen, setQualityDialogOpen] = useState(false);
  const [editingQuality, setEditingQuality] = useState<QualityProfile | null>(null);
  const [qualityPreset, setQualityPreset] = useState<QualityPreset>("tv-balanced");
  const [qualityAdvancedOpen, setQualityAdvancedOpen] = useState(false);
  const [qualityForm, setQualityForm] = useState<QualityForm>(EMPTY_QUALITY_FORM);
  const [deleteQuality, setDeleteQuality] = useState<QualityProfile | null>(null);
  const [resetQualityOpen, setResetQualityOpen] = useState(false);
  const [resetQualityConfirmOpen, setResetQualityConfirmOpen] = useState(false);
  const [resetQualityError, setResetQualityError] = useState("");
  const [deleteSubscription, setDeleteSubscription] = useState<Subscription | null>(null);
  const [deleteSubscriptionError, setDeleteSubscriptionError] = useState("");
  const [deleteDownload, setDeleteDownload] = useState<MediaDownload | null>(null);
  const [deleteDownloadError, setDeleteDownloadError] = useState("");
  const [runDetailsSubscription, setRunDetailsSubscription] = useState<Subscription | null>(null);
  const [runDetails, setRunDetails] = useState<SubscriptionRunSnapshot | null>(null);
  const [runDetailsLoading, setRunDetailsLoading] = useState(false);
  const [runDetailsError, setRunDetailsError] = useState("");

  const loadData = useCallback(async () => {
    const downloadGeneration = ++downloadListGeneration.current;
    downloadListReloading.current = true;
    downloadLoadMoreController.current?.abort();
    downloadLoadMoreController.current = null;
    setDownloadsLoadingMore(false);
    setInitialLoading(true);
    setLoadError("");
    const results = await Promise.allSettled([
      api<unknown>("/api/media/settings"),
      api<unknown>("/api/media/quality-profiles"),
      api<unknown>("/api/media/subscriptions"),
      api<unknown>(`/api/media/downloads?page=1&page_size=${DOWNLOAD_PAGE_SIZE}`),
      api<unknown>("/api/sites"),
      api<unknown>("/api/downloaders"),
      api<unknown>("/api/media/openlist/settings"),
    ]);
    const errors: string[] = [];

    if (results[0].status === "fulfilled") {
      setSettings({ ...DEFAULT_SETTINGS, ...readObject<MediaSettings>(results[0].value, ["settings", "data"]) });
    } else errors.push(`媒体设置：${describeUnknown(results[0].reason)}`);

    const profileRows = results[1].status === "fulfilled"
      ? readArray<QualityProfile>(results[1].value, ["quality_profiles", "profiles", "items", "data"])
      : [];
    if (results[1].status === "fulfilled") setProfiles(profileRows);
    else errors.push(`质量配置：${describeUnknown(results[1].reason)}`);

    const subscriptionRows = results[2].status === "fulfilled"
      ? readArray<Subscription>(results[2].value, ["subscriptions", "items", "data"])
      : [];
    if (results[2].status === "fulfilled") setSubscriptions(subscriptionRows);
    else errors.push(`订阅：${describeUnknown(results[2].reason)}`);

    if (downloadGeneration === downloadListGeneration.current) {
      if (results[3].status === "fulfilled") {
        const rows = readArray<MediaDownload>(results[3].value, ["downloads", "items", "records", "data"]);
        setDownloads(rows);
        setDownloadCursor(rows[rows.length - 1]?.id ?? null);
        setDownloadsHaveMore(rows.length === DOWNLOAD_PAGE_SIZE);
      } else errors.push(`下载任务：${describeUnknown(results[3].reason)}`);
      downloadListReloading.current = false;
    }

    const siteRows = results[4].status === "fulfilled"
      ? readArray<Site>(results[4].value, ["sites", "items", "data"])
      : [];
    if (results[4].status === "fulfilled") setSites(siteRows);
    else errors.push(`站点：${describeUnknown(results[4].reason)}`);

    const downloaderRows = results[5].status === "fulfilled"
      ? readArray<Downloader>(results[5].value, ["downloaders", "items", "data"])
      : [];
    if (results[5].status === "fulfilled") setDownloaders(downloaderRows);
    else errors.push(`下载器：${describeUnknown(results[5].reason)}`);

    setSubscriptionForm((current) => ({
      ...current,
      qualityProfileId: current.qualityProfileId || String(profileRows[0]?.id ?? ""),
      downloaderId: current.downloaderId || String(downloaderRows[0]?.id ?? ""),
      siteIds: current.siteIds.length > 0 ? current.siteIds : siteRows.map((site) => site.id),
    }));
    setResourceForm((current) => ({
      ...current,
      qualityProfileId: current.qualityProfileId || String(profileRows[0]?.id ?? ""),
      downloaderId: current.downloaderId || String(downloaderRows[0]?.id ?? ""),
      siteIds: results[4].status === "rejected" ? current.siteIds : rememberedResourceSiteIds.current === null
        ? siteRows.map((site) => site.id)
        : current.siteIds.filter((id) => siteRows.some((site) => site.id === id)),
    }));

    const openListResult = results[6];
    if (openListResult.status === "fulfilled") {
      const loaded = readObject<OpenListAutomationSettings>(openListResult.value, ["settings", "data"]);
      setOpenListSettings({ ...DEFAULT_OPENLIST_SETTINGS, ...loaded, api_key: null, clear_api_key: false });
    } else {
      setOpenListSettings(null);
      if (!(openListResult.reason instanceof ApiError && openListResult.reason.status === 404)) errors.push("OpenList 设置：无法读取");
    }
    setFailedSections(results.flatMap((result, index) => result.status === "rejected" ? [["settings", "profiles", "subscriptions", "downloads", "sites", "downloaders", "openlist"][index]] : []));
    setLoadError(errors.length ? `部分数据无法读取：${errors.map((error) => error.split("：")[0]).join("、")}。请检查服务连接后重试。` : "");
    setInitialLoading(false);
  }, []);

  const reloadSubscriptions = useCallback(async () => {
    const payload = await api<unknown>("/api/media/subscriptions");
    setSubscriptions(readArray<Subscription>(payload, ["subscriptions", "items", "data"]));
  }, []);

  const reloadDownloads = useCallback(async () => {
    const generation = ++downloadListGeneration.current;
    downloadListReloading.current = true;
    downloadLoadMoreController.current?.abort();
    downloadLoadMoreController.current = null;
    setDownloadsLoadingMore(false);
    try {
      const payload = await api<unknown>(`/api/media/downloads?page=1&page_size=${DOWNLOAD_PAGE_SIZE}`);
      if (generation !== downloadListGeneration.current) return;
      const rows = readArray<MediaDownload>(payload, ["downloads", "items", "records", "data"]);
      setDownloads(rows);
      setDownloadCursor(rows[rows.length - 1]?.id ?? null);
      setDownloadsHaveMore(rows.length === DOWNLOAD_PAGE_SIZE);
    } finally {
      if (generation === downloadListGeneration.current) downloadListReloading.current = false;
    }
  }, []);

  async function loadMoreDownloads() {
    if (
      downloadsLoadingMore
      || downloadListReloading.current
      || !downloadsHaveMore
      || downloadCursor == null
    ) return;
    const generation = downloadListGeneration.current;
    const controller = new AbortController();
    downloadLoadMoreController.current?.abort();
    downloadLoadMoreController.current = controller;
    setDownloadsLoadingMore(true);
    try {
      const payload = await api<unknown>(
        `/api/media/downloads?before_id=${downloadCursor}&page_size=${DOWNLOAD_PAGE_SIZE}`,
        { signal: controller.signal },
      );
      if (controller.signal.aborted || generation !== downloadListGeneration.current) return;
      const rows = readArray<MediaDownload>(payload, ["downloads", "items", "records", "data"]);
      setDownloads((current) => {
        const existingIds = new Set(current.map((download) => download.id));
        return [...current, ...rows.filter((download) => !existingIds.has(download.id))];
      });
      if (rows.length > 0) setDownloadCursor(rows[rows.length - 1].id);
      setDownloadsHaveMore(rows.length === DOWNLOAD_PAGE_SIZE);
    } catch (error) {
      if (controller.signal.aborted || (error as Error).name === "AbortError") return;
      setNotice({ tone: "error", text: describeUnknown(error) });
    } finally {
      if (downloadLoadMoreController.current === controller) {
        downloadLoadMoreController.current = null;
        setDownloadsLoadingMore(false);
      }
    }
  }

  const reloadProfiles = useCallback(async () => {
    const payload = await api<unknown>("/api/media/quality-profiles");
    const rows = readArray<QualityProfile>(payload, ["quality_profiles", "profiles", "items", "data"]);
    setProfiles(rows);
    setResourceForm((current) => ({
      ...current,
      qualityProfileId: rows.some((profile) => String(profile.id) === current.qualityProfileId)
        ? current.qualityProfileId
        : String(rows[0]?.id ?? ""),
    }));
  }, []);

  useEffect(() => {
    void loadData();
  }, [loadData]);

  useEffect(() => {
    if (initialLoading) return;
    try {
      window.localStorage.setItem(RESOURCE_SITE_IDS_STORAGE_KEY, JSON.stringify(resourceForm.siteIds));
    } catch {
      // Storage may be unavailable in privacy-restricted browser contexts.
    }
    rememberedResourceSiteIds.current = resourceForm.siteIds;
  }, [initialLoading, resourceForm.siteIds]);


  useEffect(
    () => () => {
      createDetailsGeneration.current += 1;
      createSeasonGeneration.current += 1;
      editDetailsGeneration.current += 1;
      editSeasonGeneration.current += 1;
    },
    [],
  );

  const profileNames = useMemo(() => new Map(profiles.map((profile) => [profile.id, profile.name])), [profiles]);
  const siteNames = useMemo(() => new Map(sites.map((site) => [site.id, site.name])), [sites]);
  const downloaderNames = useMemo(
    () => new Map(downloaders.map((downloader) => [downloader.id, downloader.name])),
    [downloaders],
  );
  const selectedSearchSubscription = useMemo(
    () =>
      subscriptions.find(
        (subscription) =>
          !subscriptionIsCompleted(subscription) && String(subscription.id) === resourceForm.subscriptionId,
      ) ?? null,
    [resourceForm.subscriptionId, subscriptions],
  );

  useEffect(() => {
    if (resourceForm.subscriptionId && !selectedSearchSubscription) {
      setResourceForm((current) => ({ ...current, subscriptionId: "" }));
      setResourceCandidates([]);
    }
  }, [resourceForm.subscriptionId, selectedSearchSubscription]);

  const editingEpisode = positiveInteger(
    editingSubscription?.next_episode ?? editingSubscription?.start_episode,
  );
  const editPreservedTarget: TvEpisodeCursor | null =
    editingSubscription?.media_type === "tv" &&
    editingSubscription.season != null &&
    editingEpisode != null
      ? { season: editingSubscription.season, episode: editingEpisode }
      : null;
  const createTargetMetadataValid =
    selectedMedia?.media_type !== "tv" ||
    tvMetadataSelectionIsValid(
      selectedMedia.tmdb_id,
      subscriptionForm,
      selectedDetails,
      detailsLoading,
      createSeasonMetadata,
    );
  const editTargetMetadataValid =
    editingSubscription?.media_type !== "tv" ||
    tvMetadataSelectionIsValid(
      editingSubscription.tmdb_id,
      editSubscriptionForm,
      editingDetails,
      editingDetailsLoading,
      editSeasonMetadata,
      editPreservedTarget,
    );

  async function loadCreateDetails(media: TmdbMedia) {
    const generation = ++createDetailsGeneration.current;
    setDetailsLoading(true);
    setDetailsError("");
    try {
      const params = new URLSearchParams({ tmdb_id: String(media.tmdb_id), media_type: media.media_type });
      const payload = await api<unknown>(`/api/media/tmdb/details?${params.toString()}`);
      if (generation !== createDetailsGeneration.current) return;
      setSelectedDetails(readObject<TmdbDetails>(payload, ["details", "data"]));
    } catch (error) {
      if (generation !== createDetailsGeneration.current) return;
      setDetailsError(describeUnknown(error));
    } finally {
      if (generation === createDetailsGeneration.current) setDetailsLoading(false);
    }
  }

  async function loadCreateSeason(tmdbId: number, season: number) {
    const generation = ++createSeasonGeneration.current;
    setCreateSeasonMetadata({ tmdbId, season, status: "loading", episodes: [], error: "" });
    try {
      const payload = await api<unknown>(`/api/media/tmdb/tv/${tmdbId}/season/${season}`);
      if (generation !== createSeasonGeneration.current) return;
      const episodes = episodeNumbers(payload);
      setCreateSeasonMetadata({ tmdbId, season, status: "ready", episodes, error: "" });
      if (episodes.length > 0) {
        setSubscriptionForm((current) =>
          current.season === season
            ? {
                ...current,
                startEpisode: episodes.includes(current.startEpisode) ? current.startEpisode : episodes[0],
              }
            : current,
        );
      }
    } catch (error) {
      if (generation !== createSeasonGeneration.current) return;
      setCreateSeasonMetadata({
        tmdbId,
        season,
        status: "error",
        episodes: [],
        error: describeUnknown(error),
      });
    }
  }

  async function loadEditDetails(subscription: Subscription) {
    const generation = ++editDetailsGeneration.current;
    setEditingDetailsLoading(true);
    setEditingDetailsError("");
    try {
      const params = new URLSearchParams({ tmdb_id: String(subscription.tmdb_id), media_type: "tv" });
      const payload = await api<unknown>(`/api/media/tmdb/details?${params.toString()}`);
      if (generation !== editDetailsGeneration.current) return;
      setEditingDetails(readObject<TmdbDetails>(payload, ["details", "data"]));
    } catch (error) {
      if (generation !== editDetailsGeneration.current) return;
      setEditingDetailsError(describeUnknown(error));
    } finally {
      if (generation === editDetailsGeneration.current) setEditingDetailsLoading(false);
    }
  }

  async function loadEditSeason(tmdbId: number, season: number) {
    const generation = ++editSeasonGeneration.current;
    setEditSeasonMetadata({ tmdbId, season, status: "loading", episodes: [], error: "" });
    try {
      const payload = await api<unknown>(`/api/media/tmdb/tv/${tmdbId}/season/${season}`);
      if (generation !== editSeasonGeneration.current) return;
      const episodes = episodeNumbers(payload);
      setEditSeasonMetadata({ tmdbId, season, status: "ready", episodes, error: "" });
    } catch (error) {
      if (generation !== editSeasonGeneration.current) return;
      setEditSeasonMetadata({
        tmdbId,
        season,
        status: "error",
        episodes: [],
        error: describeUnknown(error),
      });
    }
  }

  function closeSubscriptionDialog() {
    createDetailsGeneration.current += 1;
    createSeasonGeneration.current += 1;
    setSelectedMedia(null);
    setSelectedDetails(null);
    setDetailsLoading(false);
    setDetailsError("");
    setCreateSeasonMetadata(EMPTY_SEASON_METADATA);
  }

  function closeSubscriptionEditor() {
    editDetailsGeneration.current += 1;
    editSeasonGeneration.current += 1;
    setResetHistoryConfirmOpen(false);
    setResetDownloadHistory(false);
    setEditSubscriptionError("");
    setEditingSubscription(null);
    setEditingDetails(null);
    setEditingDetailsLoading(false);
    setEditingDetailsError("");
    setEditSeasonMetadata(EMPTY_SEASON_METADATA);
  }

  function openSubscriptionEditor(subscription: Subscription) {
    editDetailsGeneration.current += 1;
    editSeasonGeneration.current += 1;
    setEditingSubscription(subscription);
    setEditingDetails(null);
    setEditingDetailsLoading(false);
    setEditingDetailsError("");
    setEditSeasonMetadata(EMPTY_SEASON_METADATA);
    setResetHistoryConfirmOpen(false);
    setResetDownloadHistory(false);
    setEditSubscriptionError("");
    setEditSubscriptionForm({
      numberingMode: subscription.absolute_episode == null ? "season" : "absolute",
      season: subscription.season ?? 1,
      startEpisode: subscription.next_episode ?? subscription.start_episode ?? 1,
      absoluteEpisode: subscription.absolute_episode ?? 1,
      qualityProfileId: String(subscription.quality_profile_id),
      downloaderId: String(subscription.downloader_id),
      siteIds: subscription.site_ids,
      savePath: subscription.save_path ?? "",
    });
    if (subscription.media_type === "tv") {
      void loadEditDetails(subscription);
      void loadEditSeason(subscription.tmdb_id, subscription.season ?? 1);
    }
  }

  async function saveSubscriptionRules(resetConfirmed = false) {
    if (!editingSubscription) return;
    if (!editTargetMetadataValid) {
      setEditSubscriptionError("请等待季集元数据加载完成，并选择有效的季与集");
      return;
    }
    if (
      !editSubscriptionForm.qualityProfileId ||
      !editSubscriptionForm.downloaderId ||
      editSubscriptionForm.siteIds.length === 0
    ) {
      setEditSubscriptionError("请选择质量配置、站点和下载器");
      return;
    }
    if (resetDownloadHistory && !resetConfirmed) {
      setResetHistoryConfirmOpen(true);
      return;
    }
    const clearingHistory = resetDownloadHistory;
    setResetHistoryConfirmOpen(false);
    setEditSubscriptionError("");
    setBusyKey(`edit-subscription:${editingSubscription.id}`);
    try {
      await api(`/api/media/subscriptions/${editingSubscription.id}`, {
        method: "PUT",
        body: JSON.stringify({
          season: editingSubscription.media_type === "tv" ? editSubscriptionForm.season : null,
          next_episode:
            editingSubscription.media_type === "tv" ? editSubscriptionForm.startEpisode : null,
          absolute_episode:
            editingSubscription.media_type === "tv" && editSubscriptionForm.numberingMode === "absolute"
              ? editSubscriptionForm.absoluteEpisode
              : null,
          quality_profile_id: Number(editSubscriptionForm.qualityProfileId),
          downloader_id: Number(editSubscriptionForm.downloaderId),
          site_ids: editSubscriptionForm.siteIds,
          save_path: editSubscriptionForm.savePath.trim() || null,
          enabled: editingSubscription.enabled,
          reset_download_history: clearingHistory,
        }),
      });
      closeSubscriptionEditor();
      const refreshResults = await Promise.allSettled([reloadSubscriptions(), reloadDownloads()]);
      if (refreshResults.some((result) => result.status === "rejected")) {
        setNotice({
          tone: "error",
          text: clearingHistory
            ? "订阅已回到所选剧集，本地历史也已清理，但页面刷新失败。qB 种子和 OpenList 文件未被删除，请手动刷新页面。"
            : "订阅规则已保存，但页面刷新失败，请手动刷新页面。",
        });
        return;
      }
      setNotice({
        tone: "success",
        text: clearingHistory
          ? "订阅已回到所选剧集，本地下载历史已清理；qB 种子和 OpenList 文件未被删除"
          : "订阅规则已更新",
      });
    } catch (error) {
      setEditSubscriptionError(
        error instanceof ApiError && error.status === 409
          ? clearingHistory
            ? "仍有下载、订阅扫描、OpenList 复制、qB 迁移任务或未确认的 qB 提交结果。请先核验/补交或在 qB 中处理异常任务，再重新清理。"
            : "所选剧集已有提交记录或关联任务仍在运行。如需重新抓取，请勾选“从所选集开始重新抓取”；否则请等待当前任务结束。"
          : describeUnknown(error),
      );
    } finally {
      setBusyKey("");
    }
  }

  async function runSubscriptionAction(subscription: Subscription, action: "run" | "pause" | "resume") {
    if (subscriptionIsCompleted(subscription) && action !== "pause") {
      setNotice({ tone: "error", text: "该季已完成；请编辑订阅并选择新的目标后再扫描" });
      return;
    }
    const key = `${action}:${subscription.id}`;
    setBusyKey(key);
    setNotice(null);
    try {
      const runResult = action === "run"
        ? await api<SubscriptionRunResult>(`/api/media/subscriptions/${subscription.id}/run`, { method: "POST" })
        : null;
      if (action !== "run") {
        await api(`/api/media/subscriptions/${subscription.id}/${action}`, { method: "POST" });
      }
      await Promise.all([reloadSubscriptions(), reloadDownloads()]);
      setNotice({
        tone: "success",
        text: runResult
          ? `扫描完成：${runResult.query_count} 条查询，${runResult.candidate_count} 个候选，${runResult.accepted_count} 个符合，${runResult.download ? "已加入下载队列" : "未入队"}${runResult.site_errors.length > 0 ? `；${runResult.site_errors.length} 个站点请求失败` : ""}`
          : action === "pause"
            ? "订阅已暂停"
            : "订阅已恢复",
      });
    } catch (error) {
      setNotice({ tone: "error", text: describeUnknown(error) });
    } finally {
      setBusyKey("");
    }
  }

  async function redeliverDownload(download: MediaDownload) {
    const key = `redeliver:${download.id}`;
    setBusyKey(key);
    setNotice(null);
    try {
      await api<MediaDownload>(`/api/media/downloads/${download.id}/redeliver`, {
        method: "POST",
      });
      await reloadDownloads();
      setNotice({ tone: "success", text: "核验完成：种子已确认存在于下载器" });
    } catch (error) {
      setNotice({ tone: "error", text: describeUnknown(error) });
    } finally {
      setBusyKey("");
    }
  }

  async function reconcileFailedDownload(download: MediaDownload) {
    const key = `reconcile-failed:${download.id}`;
    setBusyKey(key);
    setNotice(null);
    try {
      const result = await api<ReconcileFailedMediaDownloadResponse>(
        `/api/media/downloads/${download.id}/reconcile-failed?version=${download.version}`,
        { method: "POST" },
      );
      const successText = result.resolution === "submitted"
        ? "qB 已确认种子存在，订阅已推进到下一集"
        : "qB 已确认种子不存在，当前剧集已恢复扫描";
      const refreshResults = await Promise.allSettled([reloadSubscriptions(), reloadDownloads()]);
      if (refreshResults.some((refresh) => refresh.status === "rejected")) {
        setNotice({
          tone: "error",
          text: `${successText}，但页面刷新失败，请手动刷新后再操作。`,
        });
        return;
      }
      setNotice({ tone: "success", text: successText });
    } catch (error) {
      setNotice({
        tone: "error",
        text: error instanceof ApiError && error.status === 409
          ? "记录、订阅或迁移状态已变化，核验结果未写入。请刷新后重试。"
          : describeUnknown(error),
      });
    } finally {
      setBusyKey("");
    }
  }

  async function confirmDeleteDownload() {
    if (!deleteDownload) return;
    const key = `delete-download:${deleteDownload.id}`;
    setBusyKey(key);
    setDeleteDownloadError("");
    setNotice(null);
    try {
      const result = await api<DeleteMediaDownloadResponse>(
        `/api/media/downloads/${deleteDownload.id}?version=${deleteDownload.version}`,
        { method: "DELETE" },
      );
      setDeleteDownload(null);
      setDeleteDownloadError("");
      const refreshResults = await Promise.allSettled([reloadSubscriptions(), reloadDownloads()]);
      if (refreshResults.some((result) => result.status === "rejected")) {
        setNotice({
          tone: "error",
          text: "本地记录已删除，但页面刷新失败。qB 种子和 OpenList 文件未被删除，请手动刷新页面。",
        });
        return;
      }
      setNotice({
        tone: "success",
        text: result.target_reopened
          ? "本地记录已删除，订阅已回到该集；qB 种子和 OpenList 文件未被删除"
          : "本地记录已删除；qB 种子和 OpenList 文件未被删除",
      });
    } catch (error) {
      setDeleteDownloadError(
        error instanceof ApiError && error.status === 409
          ? "记录已变化，或 qB 提交结果、关联复制及迁移任务仍未确认。请先核验/补交或在 qB 中核实处理，再刷新重试。"
          : describeUnknown(error),
      );
    } finally {
      setBusyKey("");
    }
  }

  function openDeleteDownload(download: MediaDownload) {
    setDeleteDownloadError("");
    setDeleteDownload(download);
  }

  function closeDeleteDownload() {
    if (busyKey.startsWith("delete-download:")) return;
    setDeleteDownloadError("");
    setDeleteDownload(null);
  }

  async function openRunDetails(subscription: Subscription) {
    setRunDetailsSubscription(subscription);
    setRunDetails(null);
    setRunDetailsError("");
    setRunDetailsLoading(true);
    try {
      const payload = await api<unknown>(`/api/media/subscriptions/${subscription.id}/last-run`);
      setRunDetails(normalizeRunSnapshot(payload));
    } catch (error) {
      const message = describeUnknown(error);
      setRunDetailsError(
        message.includes("no recorded run details")
          ? "暂无可查看的运行详情，请先执行一次扫描。"
          : message,
      );
    } finally {
      setRunDetailsLoading(false);
    }
  }

  function closeRunDetails() {
    setRunDetailsSubscription(null);
    setRunDetails(null);
    setRunDetailsError("");
    setRunDetailsLoading(false);
  }

  async function confirmDeleteSubscription() {
    if (!deleteSubscription) return;
    setDeleteSubscriptionError("");
    setBusyKey(`delete-subscription:${deleteSubscription.id}`);
    try {
      await api(`/api/media/subscriptions/${deleteSubscription.id}`, { method: "DELETE" });
      setDeleteSubscription(null);
      await reloadSubscriptions();
      setNotice({ tone: "success", text: "订阅已删除" });
    } catch (error) {
      setDeleteSubscriptionError(
        error instanceof ApiError && error.status === 409
          ? "订阅状态已发生变化，请刷新后重试。"
          : describeUnknown(error),
      );
    } finally {
      setBusyKey("");
    }
  }

  function openDeleteSubscription(subscription: Subscription) {
    setDeleteSubscriptionError("");
    setDeleteSubscription(subscription);
  }

  function closeDeleteSubscription() {
    if (busyKey.startsWith("delete-subscription:")) return;
    setDeleteSubscriptionError("");
    setDeleteSubscription(null);
  }

  async function searchTmdb(event: FormEvent) {
    event.preventDefault();
    if (!tmdbQuery.trim()) return;
    setTmdbLoading(true);
    setTmdbSearched(true);
    setNotice(null);
    try {
      const params = new URLSearchParams({ query: tmdbQuery.trim(), media_type: tmdbType });
      const payload = await api<unknown>(`/api/media/tmdb/search?${params.toString()}`);
      setTmdbResults(readArray<TmdbMedia>(payload, ["results", "items", "data"]));
    } catch (error) {
      setTmdbResults([]);
      setNotice({ tone: "error", text: describeUnknown(error) });
    } finally {
      setTmdbLoading(false);
    }
  }

  function openSubscriptionDialog(media: TmdbMedia) {
    setCreateError("");
    createDetailsGeneration.current += 1;
    createSeasonGeneration.current += 1;
    setSelectedMedia(media);
    setSelectedDetails(null);
    setDetailsError("");
    setCreateSeasonMetadata(EMPTY_SEASON_METADATA);
    setSubscriptionForm({
      numberingMode: "season",
      season: 1,
      startEpisode: 1,
      absoluteEpisode: 1,
      qualityProfileId: String(profiles[0]?.id ?? ""),
      downloaderId: String(downloaders[0]?.id ?? ""),
      siteIds: sites.map((site) => site.id),
      savePath: "",
    });
    void loadCreateDetails(media);
    if (media.media_type === "tv") {
      void loadCreateSeason(media.tmdb_id, 1);
    }
  }

  function changeCreateSeason(season: number) {
    if (!selectedMedia || selectedMedia.media_type !== "tv") return;
    const nextSeason = Math.max(0, Math.floor(season));
    setSubscriptionForm((current) => ({ ...current, season: nextSeason }));
    void loadCreateSeason(selectedMedia.tmdb_id, nextSeason);
  }

  function changeEditSeason(season: number) {
    if (!editingSubscription || editingSubscription.media_type !== "tv") return;
    const nextSeason = Math.max(0, Math.floor(season));
    setEditSubscriptionForm((current) => ({ ...current, season: nextSeason }));
    void loadEditSeason(editingSubscription.tmdb_id, nextSeason);
  }

  async function createSubscription() {
    if (!selectedMedia) return;
    setCreateError("");
    if (!createTargetMetadataValid) {
      setCreateError("请等待季集元数据加载完成，并选择有效的季与集");
      return;
    }
    if (!subscriptionForm.qualityProfileId || !subscriptionForm.downloaderId || subscriptionForm.siteIds.length === 0) {
      setCreateError("请选择质量配置、站点和下载器");
      return;
    }
    setBusyKey("create-subscription");
    const details = selectedDetails ?? selectedMedia;
    try {
      await api("/api/media/subscriptions", {
        method: "POST",
        body: JSON.stringify({
          tmdb_id: selectedMedia.tmdb_id,
          media_type: selectedMedia.media_type,
          title: details.title,
          original_title: details.original_title,
          aliases: selectedDetails?.aliases ?? [],
          year: details.year,
          poster_path: details.poster_path,
          season: selectedMedia.media_type === "tv" ? subscriptionForm.season : null,
          start_episode: selectedMedia.media_type === "tv" ? subscriptionForm.startEpisode : null,
          absolute_episode:
            selectedMedia.media_type === "tv" && subscriptionForm.numberingMode === "absolute"
              ? subscriptionForm.absoluteEpisode
              : null,
          quality_profile_id: Number(subscriptionForm.qualityProfileId),
          downloader_id: Number(subscriptionForm.downloaderId),
          site_ids: subscriptionForm.siteIds,
          save_path: subscriptionForm.savePath.trim() || null,
          enabled: true,
        }),
      });
      closeSubscriptionDialog();
      await reloadSubscriptions();
      setMode("subscriptions");
      setNotice({ tone: "success", text: `已订阅「${details.title}」` });
    } catch (error) {
      setCreateError(`订阅未能创建：${describeUnknown(error)}。输入已保留，请重试。`);
    } finally {
      setBusyKey("");
    }
  }

  async function searchResources(event: FormEvent) {
    event.preventDefault();
    if (!resourceForm.query.trim() && !selectedSearchSubscription) {
      setNotice({ tone: "error", text: "请输入关键词或选择订阅目标" });
      return;
    }
    if (resourceForm.siteIds.length === 0) {
      setNotice({ tone: "error", text: "请至少选择一个站点" });
      return;
    }
    setResourceLoading(true);
    setResourceSearched(true);
    setResourceErrors([]);
    setQueuedCandidateKeys(new Set());
    setNotice(null);
    try {
      const tmdbQuery = resourceForm.query.trim() || selectedSearchSubscription?.title || "";
      const tmdbParams = new URLSearchParams({
        query: tmdbQuery,
        media_type: selectedSearchSubscription?.media_type ?? "multi",
      });
      const [resourceResult, tmdbResult] = await Promise.allSettled([
        api<unknown>("/api/media/resources/search", {
        method: "POST",
        body: JSON.stringify({
          query: resourceForm.query.trim() || undefined,
          site_ids: resourceForm.siteIds,
          target: selectedSearchSubscription ? targetForSubscription(selectedSearchSubscription) : undefined,
          quality_profile_id: resourceForm.qualityProfileId ? Number(resourceForm.qualityProfileId) : undefined,
          page_size: 50,
        }),
        }),
        api<unknown>(`/api/media/tmdb/search?${tmdbParams.toString()}`),
      ]);
      if (resourceResult.status === "rejected") throw resourceResult.reason;
      const payload = resourceResult.value;
      const rows = readArray<unknown>(payload, ["candidates", "results", "items", "data"])
        .map(normalizeCandidate)
        .filter((item): item is ResourceCandidate => item !== null);
      setResourceCandidates(rows);
      setResourceErrors(extractSiteErrors(payload));
      setResourceTmdbResults(
        tmdbResult.status === "fulfilled"
          ? readArray<TmdbMedia>(tmdbResult.value, ["results", "items", "data"])
          : [],
      );
    } catch (error) {
      setResourceCandidates([]);
      setResourceTmdbResults([]);
      setResourceErrors([describeUnknown(error)]);
    } finally {
      setResourceLoading(false);
    }
  }

  async function queueCandidate(candidate: ResourceCandidate, reason?: string) {
    if (!resourceForm.qualityProfileId || !resourceForm.downloaderId) {
      setNotice({ tone: "error", text: "请选择质量配置和下载器" });
      return;
    }
    setBusyKey(`queue:${candidate.key}`);
    try {
      const queued = await api<MediaDownload>("/api/media/downloads", {
        method: "POST",
        body: JSON.stringify({
          candidate_id: candidate.candidateId,
          quality_profile_id: Number(resourceForm.qualityProfileId),
          downloader_id: Number(resourceForm.downloaderId),
          subscription_id: selectedSearchSubscription?.id,
          override_reason: reason?.trim() || undefined,
        }),
      });
      setOverrideCandidate(null);
      setOverrideReason("");
      setQueuedCandidateKeys((current) => new Set(current).add(candidate.key));
      await reloadDownloads();
      setNotice({
        tone: "success",
        text: queued.status === "submitted" ? "qB 中仍存在该种子，无需重复入队" : "资源已进入下载队列",
      });
    } catch (error) {
      setNotice({ tone: "error", text: describeUnknown(error) });
    } finally {
      setBusyKey("");
    }
  }

  async function saveMediaSettings() {
    setBusyKey("save-settings");
    try {
      const saved = await api<unknown>("/api/media/settings", {
        method: "PUT",
        body: JSON.stringify({
          ...settings,
          tmdb_token: settings.tmdb_token?.trim() || null,
          clear_tmdb_token: clearTmdbToken,
          tmdb_language: settings.tmdb_language.trim() || "zh-CN",
        }),
      });
      setSettings({ ...settings, ...readObject<MediaSettings>(saved, ["settings", "data"]) });
      setClearTmdbToken(false);
      setNotice({ tone: "success", text: "媒体设置已保存" });
    } catch (error) {
      setNotice({ tone: "error", text: describeUnknown(error) });
    } finally {
      setBusyKey("");
    }
  }

  async function saveOpenListSettings() {
    if (!openListSettings) return;
    const selectedTargetIndex = openListSettings.target_directories.findIndex(
      (target) => target.id === openListSettings.target_directory_id,
    );
    const selectedTarget = selectedTargetIndex >= 0
      ? openListSettings.target_directories[selectedTargetIndex]
      : null;
    setBusyKey("save-openlist");
    try {
      const saved = await api<unknown>("/api/media/openlist/settings", {
        method: "PUT",
        body: JSON.stringify({
          ...openListSettings,
          address: openListSettings.address.trim(),
          api_key: openListSettings.api_key?.trim() || null,
          target_directory_id: selectedTarget?.id != null && selectedTarget.id > 0
            ? selectedTarget.id
            : null,
          selected_target_index: selectedTargetIndex >= 0 ? selectedTargetIndex : null,
          target_directories: openListSettings.target_directories.map((target) => ({
            ...target,
            id: target.id != null && target.id > 0 ? target.id : undefined,
          })),
        }),
      });
      const loaded = readObject<OpenListAutomationSettings>(saved, ["settings", "data"]);
      setOpenListSettings({ ...DEFAULT_OPENLIST_SETTINGS, ...loaded, api_key: null, clear_api_key: false });
      setNotice({ tone: "success", text: "OpenList 设置已保存" });
    } catch (error) {
      setNotice({ tone: "error", text: describeUnknown(error) });
    } finally {
      setBusyKey("");
    }
  }

  function openQualityEditor(profile?: QualityProfile) {
    setQualityError("");
    setEditingQuality(profile ?? null);
    setQualityForm(
      profile
        ? {
            name: profile.name,
            resolutionOrder: profile.resolution_order.join(", "),
            allowedResolutions: profile.allowed_resolutions.join(", "),
            blockedResolutions: profile.blocked_resolutions.join(", "),
            sourceOrder: profile.source_order.join(", "),
            allowedSources: profile.allowed_sources.join(", "),
            codecOrder: profile.codec_order.join(", "),
            blockedCodecs: profile.blocked_codecs.join(", "),
            allowUnknownQuality: profile.allow_unknown_quality,
            minimumScore: profile.minimum_score,
            minSeeders: profile.min_seeders,
          }
        : { name: "", ...QUALITY_PRESETS[0].form },
    );
    setQualityDialogOpen(true);
    setQualityPreset(profile ? "custom" : "tv-balanced");
    setQualityAdvancedOpen(Boolean(profile));
  }

  function applyQualityPreset(preset: (typeof QUALITY_PRESETS)[number]) {
    setQualityPreset(preset.id);
    setQualityForm((current) => ({ ...current, ...preset.form, name: current.name || preset.name }));
  }

  async function saveQualityProfile() {
    if (!qualityForm.name.trim()) return;
    setBusyKey("save-quality");
    const body = {
      name: qualityForm.name.trim(),
      resolution_order: splitValues(qualityForm.resolutionOrder),
      allowed_resolutions: splitValues(qualityForm.allowedResolutions),
      blocked_resolutions: splitValues(qualityForm.blockedResolutions),
      source_order: splitValues(qualityForm.sourceOrder),
      allowed_sources: splitValues(qualityForm.allowedSources),
      codec_order: splitValues(qualityForm.codecOrder),
      blocked_codecs: splitValues(qualityForm.blockedCodecs),
      allow_unknown_quality: qualityForm.allowUnknownQuality,
      minimum_score: qualityForm.minimumScore,
      min_seeders: qualityForm.minSeeders,
    };
    setQualityError("");
    try {
      await api(editingQuality ? `/api/media/quality-profiles/${editingQuality.id}` : "/api/media/quality-profiles", {
        method: editingQuality ? "PUT" : "POST",
        body: JSON.stringify(body),
      });
      setQualityDialogOpen(false);
      await reloadProfiles();
      setNotice({ tone: "success", text: editingQuality ? "质量配置已更新" : "质量配置已创建" });
    } catch (error) {
      setQualityError(`质量配置未能保存：${describeUnknown(error)}。输入已保留，请重试。`);
    } finally {
      setBusyKey("");
    }
  }

  async function confirmDeleteQuality() {
    if (!deleteQuality) return;
    setBusyKey(`delete-quality:${deleteQuality.id}`);
    try {
      await api(`/api/media/quality-profiles/${deleteQuality.id}`, { method: "DELETE" });
      setDeleteQuality(null);
      await reloadProfiles();
      setNotice({ tone: "success", text: "质量配置已删除" });
    } catch (error) {
      setNotice({ tone: "error", text: describeUnknown(error) });
    } finally {
      setBusyKey("");
    }
  }

  async function resetQualityProfiles() {
    setBusyKey("reset-quality");
    setResetQualityError("");
    try {
      await api("/api/media/quality-profiles/reset", { method: "POST" });
      setResetQualityOpen(false);
      setResetQualityConfirmOpen(false);
      await loadData();
      setNotice({ tone: "success", text: "已恢复 6 套默认质量配置" });
    } catch (error) {
      const message = describeUnknown(error);
      setResetQualityError(message);
      setNotice({ tone: "error", text: message });
    } finally {
      setBusyKey("");
    }
  }

  const configurationUnavailable = initialLoading || ["settings", "profiles", "sites", "downloaders"].some((key) => failedSections.includes(key));
  const prerequisites = [
    { label: "PT 站点", ready: sites.length > 0, href: "#/sites" },
    { label: "下载器", ready: downloaders.length > 0, href: "#/downloaders" },
    { label: "TMDB 密钥", ready: settings.tmdb_token_configured, href: null },
    { label: "质量配置", ready: profiles.length > 0, href: null },
  ];
  const missingPrerequisites = prerequisites.filter((item) => !item.ready);

  return (
    <div className="flex flex-col gap-6">
      <header className="flex items-center justify-between gap-3">
        <div>
          <p className="mt-1 text-sm text-muted">
            {initialLoading ? "正在读取订阅" : failedSections.includes("subscriptions") ? "订阅暂时无法读取" : `${subscriptions.length} 个订阅`}
          </p>
        </div>
        <Button variant="outline" disabled={initialLoading} onClick={() => void loadData()}>
          <RefreshCw data-icon="inline-start" />
          刷新
        </Button>
      </header>

      <div className="grid grid-cols-2 gap-1 rounded-xl border border-border bg-surface-container p-1 lg:grid-cols-4" role="tablist" aria-label="自动追剧模式">
        {MODES.map((item) => {
          const Icon = item.icon;
          const active = mode === item.value;
          return (
            <button
              key={item.value}
              type="button"
              role="tab"
              id={`media-tab-${item.value}`}
              aria-controls="media-panel"
              aria-selected={active}
              tabIndex={active ? 0 : -1}
              onKeyDown={(event) => {
                const index = MODES.findIndex((entry) => entry.value === mode);
                const next = event.key === "ArrowRight" ? (index + 1) % MODES.length : event.key === "ArrowLeft" ? (index + MODES.length - 1) % MODES.length : event.key === "Home" ? 0 : event.key === "End" ? MODES.length - 1 : -1;
                if (next < 0) return;
                event.preventDefault(); setMode(MODES[next].value);
                document.getElementById(`media-tab-${MODES[next].value}`)?.focus();
              }}
              className={cn(
                "flex min-h-11 items-center justify-center gap-2 rounded-lg px-3 text-sm font-semibold transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/40",
                active ? "bg-card text-primary shadow-sm" : "text-muted hover:bg-accent hover:text-foreground",
              )}
              onClick={() => setMode(item.value)}
            >
              <Icon className="size-4 shrink-0" aria-hidden="true" />
              <span>{item.label}</span>
            </button>
          );
        })}
      </div>

      {notice ? <NoticeBanner notice={notice} onClose={() => setNotice(null)} /> : null}
      {loadError ? (
        <div role="alert" className="flex flex-wrap items-center justify-between gap-3 rounded-xl border border-destructive/30 bg-destructive/5 p-4 text-sm">
          <p>{loadError}</p>
          <Button variant="outline" disabled={initialLoading} onClick={() => void loadData()}>重新读取</Button>
        </div>
      ) : null}

      {pendingMedia ? <div role="status" className="flex flex-wrap items-center justify-between gap-3 rounded-xl border border-border p-4 text-sm">
        <span>已保留「{pendingMedia.title}」，完成配置后可继续订阅。</span>
        <Button variant="outline" onClick={() => { openSubscriptionDialog(pendingMedia); setPendingMedia(null); }}>继续订阅</Button>
      </div> : null}
      {!configurationUnavailable && missingPrerequisites.length > 0 && mode !== "settings" ? (
        <section aria-label="订阅准备" className="rounded-xl border border-border p-4">
          <h3 className="font-semibold">开始订阅前，先完成配置</h3>
          <p className="mt-1 text-sm text-muted">完成必需配置后，即可查找影视并创建订阅。</p>
          <ul className="mt-3 flex flex-wrap gap-3">
            {prerequisites.map((item) => <li key={item.label}>
              {item.ready ? <span className="inline-flex min-h-11 items-center gap-2 text-sm"><CheckCircle2 className="size-4" />{item.label}已配置</span>
                : item.href ? <a className="inline-flex min-h-11 items-center rounded-lg border border-border px-3 text-sm underline underline-offset-4" href={item.href} target="_blank" rel="noreferrer">配置{item.label}（新标签页）</a>
                : <Button variant="outline" onClick={() => updateHashParams({ mode: "settings", section: item.label === "质量配置" ? "quality" : "media" })}>配置{item.label}</Button>}
            </li>)}
          </ul>
        </section>
      ) : null}

      <div id="media-panel" role="tabpanel" aria-labelledby={`media-tab-${mode}`}>
      {initialLoading ? (
        <LoadingState label="正在加载自动追剧数据" />
      ) : mode === "subscriptions" ? (
        <SubscriptionsPanel
          subscriptionsUnavailable={failedSections.includes("subscriptions")}
          downloadsUnavailable={failedSections.includes("downloads")}
          configurationUnavailable={configurationUnavailable}
          onRetry={() => void loadData()}
          subscriptions={subscriptions}
          downloads={downloads}
          profiles={profiles}
          sites={sites}
          downloaders={downloaders}
          profileNames={profileNames}
          siteNames={siteNames}
          downloaderNames={downloaderNames}
          busyKey={busyKey}
          downloadsHaveMore={downloadsHaveMore}
          downloadsLoadingMore={downloadsLoadingMore}
          onAdd={() => setMode("tmdb")}
          onAction={(subscription, action) => void runSubscriptionAction(subscription, action)}
          onViewRun={(subscription) => void openRunDetails(subscription)}
          onEdit={openSubscriptionEditor}
          onDelete={openDeleteSubscription}
          onRedeliver={(download) => void redeliverDownload(download)}
          onReconcileFailed={(download) => void reconcileFailedDownload(download)}
          onDeleteDownload={openDeleteDownload}
          onLoadMoreDownloads={() => void loadMoreDownloads()}
        />
      ) : mode === "tmdb" ? (
        <TmdbPanel
          query={tmdbQuery}
          mediaType={tmdbType}
          results={tmdbResults}
          loading={tmdbLoading}
          searched={tmdbSearched}
          onQueryChange={setTmdbQuery}
          onMediaTypeChange={(value) => setTmdbType(value as "multi" | MediaType)}
          onSearch={searchTmdb}
          onSubscribe={(media) => void openSubscriptionDialog(media)}
        />
      ) : mode === "resources" ? (
        <ResourcesPanel
          form={resourceForm}
          setForm={setResourceForm}
          subscriptions={subscriptions}
          profiles={profiles}
          sites={sites}
          downloaders={downloaders}
          candidates={resourceCandidates}
          tmdbResults={resourceTmdbResults}
          errors={resourceErrors}
          loading={resourceLoading}
          searched={resourceSearched}
          busyKey={busyKey}
          queuedCandidateKeys={queuedCandidateKeys}
          onSearch={searchResources}
          onQueue={(candidate) => {
            if (candidateNeedsOverride(candidate)) {
              setOverrideCandidate(candidate);
              setOverrideReason("");
            } else {
              void queueCandidate(candidate);
            }
          }}
        />
      ) : (
        <SettingsPanel
          settingsUnavailable={failedSections.includes("settings")}
          profilesUnavailable={failedSections.includes("profiles")}
          onRetry={() => void loadData()}
          settings={settings}
          setSettings={setSettings}
          clearTmdbToken={clearTmdbToken}
          setClearTmdbToken={setClearTmdbToken}
          profiles={profiles}
          downloaders={downloaders}
          openListSettings={openListSettings}
          setOpenListSettings={setOpenListSettings}
          busyKey={busyKey}
          onSaveSettings={() => void saveMediaSettings()}
          onSaveOpenList={() => void saveOpenListSettings()}
          onAddQuality={() => openQualityEditor()}
          onResetQuality={() => setResetQualityOpen(true)}
          onEditQuality={openQualityEditor}
          onDeleteQuality={setDeleteQuality}
        />
      )}

      </div>
      <Dialog
        open={runDetailsSubscription !== null}
        onClose={closeRunDetails}
        title={runDetailsSubscription ? `「${runDetailsSubscription.title}」运行详情` : "运行详情"}
        description={runDetails ? `${formatDate(runDetails.finished_at)} · ${targetKeyLabel(runDetails.target_key)}` : "最后一次订阅扫描"}
        panelClassName="max-w-7xl"
      >
        <SubscriptionRunDetails
          snapshot={runDetails}
          loading={runDetailsLoading}
          error={runDetailsError}
          subscriptionId={runDetailsSubscription?.id ?? null}
          downloads={downloads}
        />
      </Dialog>

      <Dialog
        open={selectedMedia !== null}
        onClose={closeSubscriptionDialog}
        title={selectedMedia ? `订阅「${selectedMedia.title}」` : "创建订阅"}
        description={selectedMedia?.media_type === "movie" ? "电影订阅" : "剧集订阅"}
        escMode="double"
        panelClassName="max-w-3xl"
      >
        {selectedMedia ? (
          <div className="flex flex-col gap-5 p-4 sm:p-6">
            <div className="flex gap-4">
              <Poster path={selectedDetails?.poster_path ?? selectedMedia.poster_path} title={selectedMedia.title} className="w-24 shrink-0" />
              <div className="min-w-0 flex-1">
                <div className="font-semibold">{selectedDetails?.title ?? selectedMedia.title}</div>
                <div className="mt-1 text-sm text-muted">
                  {selectedMedia.year ?? "年份未知"} · {selectedMedia.media_type === "movie" ? "电影" : "剧集"}
                </div>
                {detailsLoading ? (
                  <div className="mt-3 flex items-center gap-2 text-sm text-muted">
                    <LoaderCircle className="size-4 animate-spin" />
                    正在读取详情
                  </div>
                ) : null}
              </div>
            </div>

            {selectedMedia.media_type === "tv" ? (
              <TvTargetFields
                idPrefix="subscription"
                tmdbId={selectedMedia.tmdb_id}
                form={subscriptionForm}
                setForm={setSubscriptionForm}
                details={selectedDetails}
                detailsLoading={detailsLoading}
                detailsError={detailsError}
                metadata={createSeasonMetadata}
                seasonLabel="季"
                episodeLabel="起始集"
                absoluteLabel="绝对起始集"
                onSeasonChange={changeCreateSeason}
                onRetryDetails={() => void loadCreateDetails(selectedMedia)}
                onRetrySeason={() => void loadCreateSeason(selectedMedia.tmdb_id, subscriptionForm.season)}
              />
            ) : null}

            <div className="grid gap-4 sm:grid-cols-2">
              <FormSelect
                label="质量配置"
                value={subscriptionForm.qualityProfileId}
                onChange={(qualityProfileId) => setSubscriptionForm((current) => ({ ...current, qualityProfileId }))}
                options={profiles.map((profile) => ({ value: String(profile.id), label: profile.name }))}
              />
              <FormSelect
                label="下载器"
                value={subscriptionForm.downloaderId}
                onChange={(downloaderId) => setSubscriptionForm((current) => ({ ...current, downloaderId }))}
                options={downloaders.map((downloader) => ({ value: String(downloader.id), label: downloader.name }))}
              />
            </div>

            <SitePicker
              sites={sites}
              selected={subscriptionForm.siteIds}
              onChange={(siteIds) => setSubscriptionForm((current) => ({ ...current, siteIds }))}
            />

            <div className="flex flex-col gap-2">
              <Label htmlFor="subscription-save-path">保存路径</Label>
              <Input
                id="subscription-save-path"
                value={subscriptionForm.savePath}
                onChange={(event) => setSubscriptionForm((current) => ({ ...current, savePath: event.target.value }))}
                placeholder="使用下载器默认路径"
              />
            </div>

            {createError ? <p role="alert" className="rounded-lg border border-destructive/30 p-3 text-sm text-destructive">{createError}</p> : null}
            {configurationUnavailable || missingPrerequisites.length > 0 ? <p role="status" className="text-sm text-muted">{configurationUnavailable ? "配置读取失败，请关闭弹窗后重新读取。" : `还需配置：${missingPrerequisites.map((item) => item.label).join("、")}。可先完成配置，已选影视会保留。`}</p> : null}
            {!configurationUnavailable && missingPrerequisites.length > 0 ? <div className="flex flex-wrap gap-2">
              {missingPrerequisites.map((item) => item.href ? <a key={item.label} className="inline-flex min-h-11 items-center rounded-lg border border-border px-3 text-sm underline" href={item.href} target="_blank" rel="noreferrer">配置{item.label}（新标签页）</a> : <Button key={item.label} variant="outline" onClick={() => { setPendingMedia(selectedMedia); closeSubscriptionDialog(); setMode("settings"); }}>配置{item.label}</Button>)}
              <Button variant="outline" onClick={() => void loadData()}>已配置，重新读取</Button>
            </div> : null}
            <div className="flex flex-wrap justify-end gap-2 border-t border-border pt-4">
              <Button variant="outline" onClick={closeSubscriptionDialog}>取消</Button>
              <Button
                disabled={
                  configurationUnavailable || missingPrerequisites.length > 0 ||
                  busyKey === "create-subscription" ||
                  !createTargetMetadataValid ||
                  profiles.length === 0 ||
                  sites.length === 0 ||
                  downloaders.length === 0
                }
                onClick={() => void createSubscription()}
              >
                {busyKey === "create-subscription" ? <LoaderCircle className="animate-spin" data-icon="inline-start" /> : <Plus data-icon="inline-start" />}
                {busyKey === "create-subscription" ? "创建中" : "创建订阅"}
              </Button>
            </div>
          </div>
        ) : null}
      </Dialog>

      <Dialog
        open={editingSubscription !== null}
        onClose={closeSubscriptionEditor}
        title={editingSubscription ? `编辑「${editingSubscription.title}」` : "编辑订阅"}
        description="订阅目标与下载规则"
        escMode="double"
        panelClassName="max-w-3xl"
      >
        {editingSubscription ? (
          <div className="flex flex-col gap-5 p-4 sm:p-6">
            {editSubscriptionError ? (
              <div role="alert" aria-live="assertive" className="rounded-lg border border-destructive/30 bg-destructive/5 px-4 py-3 text-sm text-destructive">
                {editSubscriptionError}
              </div>
            ) : null}
            {editingSubscription.media_type === "tv" ? (
              <TvTargetFields
                idPrefix="edit-subscription"
                tmdbId={editingSubscription.tmdb_id}
                form={editSubscriptionForm}
                setForm={setEditSubscriptionForm}
                details={editingDetails}
                detailsLoading={editingDetailsLoading}
                detailsError={editingDetailsError}
                metadata={editSeasonMetadata}
                preservedTarget={editPreservedTarget}
                seasonLabel="季"
                episodeLabel="当前集"
                absoluteLabel="当前绝对集"
                onSeasonChange={changeEditSeason}
                onRetryDetails={() => void loadEditDetails(editingSubscription)}
                onRetrySeason={() =>
                  void loadEditSeason(editingSubscription.tmdb_id, editSubscriptionForm.season)
                }
              />
            ) : null}

            <div className="grid gap-4 sm:grid-cols-2">
              <FormSelect
                label="质量配置"
                value={editSubscriptionForm.qualityProfileId}
                onChange={(qualityProfileId) =>
                  setEditSubscriptionForm((current) => ({ ...current, qualityProfileId }))
                }
                options={profiles.map((profile) => ({ value: String(profile.id), label: profile.name }))}
              />
              <FormSelect
                label="下载器"
                value={editSubscriptionForm.downloaderId}
                onChange={(downloaderId) =>
                  setEditSubscriptionForm((current) => ({ ...current, downloaderId }))
                }
                options={downloaders.map((downloader) => ({
                  value: String(downloader.id),
                  label: downloader.name,
                }))}
              />
            </div>

            <SitePicker
              sites={sites}
              selected={editSubscriptionForm.siteIds}
              onChange={(siteIds) =>
                setEditSubscriptionForm((current) => ({ ...current, siteIds }))
              }
            />

            <div className="flex flex-col gap-2">
              <Label htmlFor="edit-subscription-save-path">保存路径</Label>
              <Input
                id="edit-subscription-save-path"
                value={editSubscriptionForm.savePath}
                onChange={(event) =>
                  setEditSubscriptionForm((current) => ({ ...current, savePath: event.target.value }))
                }
                placeholder="使用下载器默认路径"
              />
            </div>

            {editingSubscription.media_type === "tv" ? (
              <label className="flex cursor-pointer items-start gap-3 rounded-lg border border-border bg-surface-container/35 px-4 py-3 text-sm transition-colors hover:bg-accent">
                <input
                  type="checkbox"
                  className="mt-0.5 size-4 shrink-0 accent-primary"
                  checked={resetDownloadHistory}
                  onChange={(event) => setResetDownloadHistory(event.target.checked)}
                />
                <span className="min-w-0">
                  <span className="block font-medium">从所选集开始重新抓取</span>
                  <span className="mt-1 block text-xs leading-5 text-muted">
                    清理该集及后续剧集的 云母下载记录和已结束的自动复制记录，不会删除 qB 种子或 OpenList 文件。
                  </span>
                </span>
              </label>
            ) : null}

            <div className="flex flex-wrap justify-end gap-2 border-t border-border pt-4">
              <Button variant="outline" onClick={closeSubscriptionEditor}>取消</Button>
              <Button
                variant={resetDownloadHistory ? "destructive" : "default"}
                disabled={
                  busyKey === `edit-subscription:${editingSubscription.id}` || !editTargetMetadataValid
                }
                onClick={() => void saveSubscriptionRules()}
              >
                {busyKey === `edit-subscription:${editingSubscription.id}` ? (
                  <LoaderCircle className="animate-spin" data-icon="inline-start" />
                ) : null}
                {busyKey === `edit-subscription:${editingSubscription.id}`
                  ? "保存中"
                  : resetDownloadHistory
                    ? "清理记录并保存"
                    : "保存"}
              </Button>
            </div>
          </div>
        ) : null}
      </Dialog>

      <Dialog
        open={resetHistoryConfirmOpen}
        onClose={() => setResetHistoryConfirmOpen(false)}
        title="确认重新抓取"
        description={editingSubscription
          ? `将从${editSubscriptionForm.season === 0 ? "特别篇" : `第 ${editSubscriptionForm.season} 季`}第 ${editSubscriptionForm.startEpisode} 集开始清理本地历史。`
          : "确认清理本地下载历史。"}
        panelClassName="max-w-xl"
      >
        <div className="flex flex-col gap-4 p-4 sm:p-6">
          <div className="flex items-start gap-3 rounded-lg border border-destructive/30 bg-destructive/5 px-4 py-3 text-sm">
            <AlertCircle className="mt-0.5 size-4 shrink-0 text-destructive" aria-hidden="true" />
            <p className="leading-6 text-muted">
              云母会删除所选范围内的本地下载历史和已结束的自动复制记录，并允许这些剧集再次入队。qB 种子和 OpenList 文件不会被删除；仍在运行的任务会阻止本次操作。
            </p>
          </div>
          <div className="flex justify-end gap-2">
            <Button variant="outline" onClick={() => setResetHistoryConfirmOpen(false)}>取消</Button>
            <Button
              variant="destructive"
              disabled={editingSubscription == null || busyKey.startsWith("edit-subscription:")}
              onClick={() => void saveSubscriptionRules(true)}
            >
              {busyKey.startsWith("edit-subscription:") ? (
                <LoaderCircle className="animate-spin" data-icon="inline-start" />
              ) : (
                <RotateCcw data-icon="inline-start" />
              )}
              {busyKey.startsWith("edit-subscription:") ? "处理中" : "确认清理并保存"}
            </Button>
          </div>
        </div>
      </Dialog>

      <Dialog
        open={qualityDialogOpen}
        onClose={() => setQualityDialogOpen(false)}
        title={editingQuality ? "编辑质量配置" : "新建质量配置"}
        description="先选一个观看偏好，需要时再微调专业参数"
        escMode="double"
        panelClassName="max-w-3xl"
      >
        <div className="flex flex-col gap-6 p-4 sm:p-6">
          <div className="flex flex-col gap-2">
            <Label htmlFor="quality-name">名称</Label>
            <Input id="quality-name" placeholder="例如：客厅电视" value={qualityForm.name} onChange={(event) => setQualityForm((current) => ({ ...current, name: event.target.value }))} />
          </div>
          <fieldset className="flex flex-col gap-3">
            <legend className="text-sm font-medium">观看偏好</legend>
            <div className="grid gap-3 sm:grid-cols-3">
              {QUALITY_PRESETS.map((preset) => {
                const active = qualityPreset === preset.id;
                return (
                  <button key={preset.id} type="button" aria-pressed={active} onClick={() => applyQualityPreset(preset)} className={cn("cursor-pointer rounded-xl border p-4 text-left transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary", active ? "border-primary bg-primary/10" : "border-border bg-surface-container/35 hover:bg-surface-container")}>
                    <span className="flex items-center justify-between gap-2 text-sm font-semibold">{preset.name}{active ? <CheckCircle2 className="size-4 text-primary" /> : null}</span>
                    <span className="mt-2 block text-xs leading-5 text-muted">{preset.description}</span>
                    <span className="mt-2 block text-[11px] leading-4 text-muted">{preset.detail}</span>
                  </button>
                );
              })}
            </div>
          </fieldset>

          <div className="rounded-xl border border-border bg-surface-container/35 p-4">
            <div className="flex items-start gap-3">
              <ShieldCheck className="mt-0.5 size-5 shrink-0 text-primary" />
              <div className="min-w-0">
                <div className="text-sm font-semibold">当前会优先寻找</div>
                <div className="mt-2 flex flex-wrap gap-2"><TokenList values={[...splitValues(qualityForm.resolutionOrder).slice(0, 2), ...splitValues(qualityForm.sourceOrder).slice(0, 2), ...splitValues(qualityForm.codecOrder).slice(0, 1)]} /></div>
                <p className="mt-2 text-xs leading-5 text-muted">自动排序依次比较分辨率、片源、体积充足度、杜比视界 / HDR / 位深、视频编码和做种数；只有体积达到参考线后才启用视频特性加成。</p>
              </div>
            </div>
          </div>

          <div className="overflow-hidden rounded-xl border border-border">
            <button type="button" aria-expanded={qualityAdvancedOpen} className="flex w-full cursor-pointer items-center justify-between gap-3 bg-surface-container/35 px-4 py-3 text-left text-sm font-semibold hover:bg-surface-container" onClick={() => setQualityAdvancedOpen((open) => !open)}>
              高级设置 <ChevronDown className={cn("size-4 transition-transform", qualityAdvancedOpen && "rotate-180")} />
            </button>
            {qualityAdvancedOpen ? (
              <div className="flex flex-col gap-4 border-t border-border p-4">
                <p className="text-xs leading-5 text-muted">越靠前越优先；“允许”留空表示不限制。仅在你熟悉片源命名时修改。</p>
                <div className="grid gap-4 sm:grid-cols-2">
                  <DelimitedField id="quality-resolution-order" label="分辨率优先级" value={qualityForm.resolutionOrder} onChange={(resolutionOrder) => { setQualityPreset("custom"); setQualityForm((current) => ({ ...current, resolutionOrder })); }} />
                  <DelimitedField id="quality-allowed-resolution" label="允许的分辨率" value={qualityForm.allowedResolutions} onChange={(allowedResolutions) => { setQualityPreset("custom"); setQualityForm((current) => ({ ...current, allowedResolutions })); }} />
                  <DelimitedField id="quality-blocked-resolution" label="拒绝的分辨率" value={qualityForm.blockedResolutions} onChange={(blockedResolutions) => { setQualityPreset("custom"); setQualityForm((current) => ({ ...current, blockedResolutions })); }} />
                  <DelimitedField id="quality-source-order" label="片源优先级" value={qualityForm.sourceOrder} onChange={(sourceOrder) => { setQualityPreset("custom"); setQualityForm((current) => ({ ...current, sourceOrder })); }} />
                  <DelimitedField id="quality-allowed-source" label="允许的片源" value={qualityForm.allowedSources} onChange={(allowedSources) => { setQualityPreset("custom"); setQualityForm((current) => ({ ...current, allowedSources })); }} />
                  <DelimitedField id="quality-codec-order" label="视频编码优先级" value={qualityForm.codecOrder} onChange={(codecOrder) => { setQualityPreset("custom"); setQualityForm((current) => ({ ...current, codecOrder })); }} />
                  <DelimitedField id="quality-blocked-codec" label="拒绝的视频编码" value={qualityForm.blockedCodecs} onChange={(blockedCodecs) => { setQualityPreset("custom"); setQualityForm((current) => ({ ...current, blockedCodecs })); }} />
                  <FormNumber id="quality-minimum-score" label="最低匹配分" min={0} max={100} value={qualityForm.minimumScore} onChange={(minimumScore) => setQualityForm((current) => ({ ...current, minimumScore }))} />
                  <FormNumber id="quality-min-seeders" label="最低做种数" min={0} value={qualityForm.minSeeders} onChange={(minSeeders) => setQualityForm((current) => ({ ...current, minSeeders }))} />
                </div>
                <label className="flex cursor-pointer items-center gap-3 rounded-lg border border-border bg-surface-container/35 px-4 py-3 text-sm">
                  <input type="checkbox" className="size-4 accent-primary" checked={qualityForm.allowUnknownQuality} onChange={(event) => setQualityForm((current) => ({ ...current, allowUnknownQuality: event.target.checked }))} />
                  <span><span className="block font-medium">允许信息不完整的资源</span><span className="mt-1 block text-xs text-muted">可能增加结果，但也更容易下载到不符合预期的版本</span></span>
                </label>
              </div>
            ) : null}
          </div>
          <div className="flex flex-wrap justify-end gap-2 border-t border-border pt-4">
            {qualityError ? <p role="alert" className="w-full rounded-lg border border-destructive/30 p-3 text-sm text-destructive">{qualityError}</p> : null}
            <Button variant="outline" onClick={() => setQualityDialogOpen(false)}>取消</Button>
            <Button disabled={!qualityForm.name.trim() || busyKey === "save-quality"} onClick={() => void saveQualityProfile()}>
              {busyKey === "save-quality" ? <LoaderCircle className="animate-spin" data-icon="inline-start" /> : null}
              {busyKey === "save-quality" ? "保存中" : "保存"}
            </Button>
          </div>
        </div>
      </Dialog>

      <Dialog
        open={deleteSubscription !== null}
        onClose={closeDeleteSubscription}
        title="删除订阅"
        description={`确定删除「${deleteSubscription?.title ?? ""}」？云母中的相关下载记录及其自动复制、迁移任务将一并删除；qB 中的种子和文件会保留，已提交到 OpenList 的远端任务无法撤回。`}
      >
        <div className="flex flex-col gap-4 p-4 sm:p-6">
          {deleteSubscriptionError ? (
            <div role="alert" aria-live="assertive" className="rounded-lg border border-destructive/30 bg-destructive/5 px-4 py-3 text-sm text-destructive">
              {deleteSubscriptionError}
            </div>
          ) : null}
          <div className="flex justify-end gap-2">
            <Button variant="outline" disabled={busyKey.startsWith("delete-subscription:")} onClick={closeDeleteSubscription}>取消</Button>
            <Button variant="destructive" disabled={busyKey.startsWith("delete-subscription:")} onClick={() => void confirmDeleteSubscription()}>
              {busyKey.startsWith("delete-subscription:") ? <LoaderCircle className="animate-spin" data-icon="inline-start" /> : <Trash2 data-icon="inline-start" />}
              {busyKey.startsWith("delete-subscription:") ? "删除中" : "确认删除"}
            </Button>
          </div>
        </div>
      </Dialog>

      <Dialog
        open={deleteDownload !== null}
        onClose={closeDeleteDownload}
        title="删除本地下载记录"
        description={deleteDownload ? `${targetKeyLabel(deleteDownload.target_key)} · ${deleteDownload.title}` : undefined}
        panelClassName="max-w-xl"
      >
        <div className="flex flex-col gap-4 p-4 sm:p-6">
          <div className="flex items-start gap-3 rounded-lg border border-destructive/30 bg-destructive/5 px-4 py-3 text-sm">
            <AlertCircle className="mt-0.5 size-4 shrink-0 text-destructive" aria-hidden="true" />
            <p className="leading-6 text-muted">
              仅删除 云母本地记录和关联的已结束自动复制记录，不会删除 qB 中的种子、下载文件或 OpenList 数据。若这是该集当前的提交记录，关联订阅会回到该集以便重新扫描。
            </p>
          </div>
          {deleteDownloadError ? (
            <div role="alert" aria-live="assertive" className="rounded-lg border border-destructive/30 bg-destructive/5 px-4 py-3 text-sm text-destructive">
              {deleteDownloadError}
            </div>
          ) : null}
          <div className="flex justify-end gap-2">
            <Button variant="outline" disabled={busyKey.startsWith("delete-download:")} onClick={closeDeleteDownload}>取消</Button>
            <Button
              variant="destructive"
              disabled={deleteDownload == null || busyKey.startsWith("delete-download:")}
              onClick={() => void confirmDeleteDownload()}
            >
              {busyKey.startsWith("delete-download:") ? (
                <LoaderCircle className="animate-spin" data-icon="inline-start" />
              ) : (
                <Trash2 data-icon="inline-start" />
              )}
              {busyKey.startsWith("delete-download:") ? "删除中" : "确认删除本地记录"}
            </Button>
          </div>
        </div>
      </Dialog>

      <Dialog
        open={deleteQuality !== null}
        onClose={() => setDeleteQuality(null)}
        title="删除质量配置"
        description={`确定删除「${deleteQuality?.name ?? ""}」？正在使用的配置无法删除。`}
      >
        <div className="flex justify-end gap-2 p-4 sm:p-6">
          <Button variant="outline" onClick={() => setDeleteQuality(null)}>取消</Button>
          <Button variant="destructive" disabled={busyKey.startsWith("delete-quality:")} onClick={() => void confirmDeleteQuality()}>
            {busyKey.startsWith("delete-quality:") ? "删除中" : "确认删除"}
          </Button>
        </div>
      </Dialog>

      <Dialog
        open={resetQualityOpen}
        onClose={() => setResetQualityOpen(false)}
        title="恢复默认质量配置"
        description="此操作会覆盖当前质量配置，请先确认影响范围。"
      >
        <div className="flex flex-col gap-5 p-4 sm:p-6">
          <div className="rounded-xl border border-destructive/40 bg-destructive/5 p-4 text-sm">
            <div className="flex items-center gap-2 font-semibold text-destructive"><AlertCircle className="size-4" />重置将产生以下后果</div>
            <ul className="mt-3 flex list-disc flex-col gap-2 pl-5 text-muted">
              <li>删除全部现有质量配置，包括你手动创建和修改的配置。</li>
              <li>重新建立动漫、电视剧、电影共 6 套内置推荐方案。</li>
              <li>所有现有订阅统一改用“电视剧 · 日常”。</li>
            </ul>
          </div>
          <div className="flex justify-end gap-2">
            <Button variant="outline" onClick={() => setResetQualityOpen(false)}>取消</Button>
            <Button variant="destructive" onClick={() => { setResetQualityError(""); setResetQualityOpen(false); setResetQualityConfirmOpen(true); }}>
              继续
            </Button>
          </div>
        </div>
      </Dialog>

      <Dialog
        open={resetQualityConfirmOpen}
        onClose={() => setResetQualityConfirmOpen(false)}
        title="再次确认重置"
        description="质量配置及订阅关联即将被永久修改，此操作无法撤销。"
      >
        <div className="flex flex-col gap-5 p-4 sm:p-6">
          <p className="text-sm leading-6 text-muted">确认后，现有自定义规则无法找回。正在追更的动漫和电影也会先切换到“电视剧 · 日常”，需要你之后按需重新选择质量配置。</p>
          {resetQualityError ? (
            <div role="alert" className="rounded-lg border border-destructive/40 bg-destructive/5 px-4 py-3 text-sm text-destructive">重置失败：{resetQualityError}</div>
          ) : null}
          <div className="flex justify-end gap-2">
            <Button variant="outline" onClick={() => setResetQualityConfirmOpen(false)}>返回</Button>
            <Button variant="destructive" disabled={busyKey === "reset-quality"} onClick={() => void resetQualityProfiles()}>
              {busyKey === "reset-quality" ? <LoaderCircle className="animate-spin" data-icon="inline-start" /> : <RotateCcw data-icon="inline-start" />}
              {busyKey === "reset-quality" ? "重置中" : "确认重置"}
            </Button>
          </div>
        </div>
      </Dialog>

      <Dialog
        open={overrideCandidate !== null}
        onClose={() => setOverrideCandidate(null)}
        title="确认覆盖拒绝规则"
        description={overrideCandidate?.decision?.rejections.map((item) => item.message).join("；") || "该资源未通过自动匹配。"}
        panelClassName="max-w-xl"
      >
        <div className="flex flex-col gap-4 p-4 sm:p-6">
          <div className="flex flex-col gap-2">
            <Label htmlFor="override-reason">覆盖原因</Label>
            <Input id="override-reason" value={overrideReason} onChange={(event) => setOverrideReason(event.target.value)} placeholder="说明为什么仍要下载" />
          </div>
          <div className="flex justify-end gap-2">
            <Button variant="outline" onClick={() => setOverrideCandidate(null)}>取消</Button>
            <Button variant="destructive" disabled={!overrideReason.trim() || busyKey.startsWith("queue:")} onClick={() => overrideCandidate && void queueCandidate(overrideCandidate, overrideReason)}>
              {busyKey.startsWith("queue:") ? "入队中" : "覆盖并入队"}
            </Button>
          </div>
        </div>
      </Dialog>
    </div>
  );
}

function NoticeBanner({ notice, onClose }: { notice: Notice; onClose: () => void }) {
  const Icon = notice.tone === "error" ? AlertCircle : CheckCircle2;
  return (
    <div
      role={notice.tone === "error" ? "alert" : "status"}
      className={cn(
        "flex items-start justify-between gap-3 rounded-2xl border px-4 py-3 text-sm",
        notice.tone === "error"
          ? "border-destructive/25 bg-destructive/5 text-destructive"
          : "border-primary/20 bg-primary/10 text-foreground",
      )}
    >
      <div className="flex min-w-0 items-start gap-2">
        <Icon className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
        <span className="break-words">{notice.text}</span>
      </div>
      <button type="button" className="shrink-0 rounded-lg p-1 transition-colors hover:bg-accent focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/40" aria-label="关闭提示" onClick={onClose}>
        <X className="size-4" />
      </button>
    </div>
  );
}

function LoadingState({ label }: { label: string }) {
  return (
    <div className="flex min-h-56 items-center justify-center gap-3 rounded-2xl border border-border bg-card text-sm text-muted">
      <LoaderCircle className="size-5 animate-spin" aria-hidden="true" />
      {label}
    </div>
  );
}

function EmptyState({ icon: Icon, title, action }: { icon: typeof Film; title: string; action?: { label: string; onClick: () => void } }) {
  return (
    <div className="flex min-h-44 flex-col items-center justify-center gap-3 px-4 py-8 text-center">
      {title === "暂无订阅" ? <img src="/kirara-rest.svg" alt="" className="kirara-empty-art" /> : (
        <div className="flex size-11 items-center justify-center rounded-xl bg-surface-container text-muted">
          <Icon className="size-5" aria-hidden="true" />
        </div>
      )}
      <div className="text-sm font-semibold">{title}</div>
      {title === "暂无订阅" ? <p className="max-w-sm text-sm leading-6 text-muted">把想看的故事交给云母，从添加一部影视开始。</p> : null}
      {action ? <Button variant="outline" onClick={action.onClick}>{action.label}</Button> : null}
    </div>
  );
}

function StatusPill({ label, tone = "neutral" }: { label: string; tone?: "positive" | "negative" | "neutral" }) {
  return (
    <span
      className={cn(
        "inline-flex items-center rounded-full border px-2.5 py-1 text-xs font-semibold",
        tone === "positive"
          ? "border-primary/20 bg-primary/10 text-foreground"
          : tone === "negative"
            ? "border-destructive/25 bg-destructive/5 text-destructive"
            : "border-border bg-surface-container text-muted",
      )}
    >
      {label}
    </span>
  );
}

function Poster({ path, title, className }: { path: string | null | undefined; title: string; className?: string }) {
  const [failed, setFailed] = useState(false);
  const url = posterUrl(path);
  useEffect(() => setFailed(false), [url]);
  return (
    <div className={cn("aspect-[2/3] overflow-hidden rounded-lg border border-border bg-surface-container", className)}>
      {url && !failed ? (
        <img src={url} alt={`${title} 海报`} className="size-full object-cover" loading="lazy" onError={() => setFailed(true)} />
      ) : (
        <div className="flex size-full items-center justify-center text-muted">
          <ImageOff className="size-6" aria-label="暂无海报" />
        </div>
      )}
    </div>
  );
}

function TvTargetFields({
  idPrefix,
  tmdbId,
  form,
  setForm,
  details,
  detailsLoading,
  detailsError,
  metadata,
  preservedTarget = null,
  seasonLabel,
  episodeLabel,
  absoluteLabel,
  onSeasonChange,
  onRetryDetails,
  onRetrySeason,
}: {
  idPrefix: string;
  tmdbId: number;
  form: SubscriptionForm;
  setForm: Dispatch<SetStateAction<SubscriptionForm>>;
  details: TmdbDetails | null;
  detailsLoading: boolean;
  detailsError: string;
  metadata: SeasonMetadataState;
  preservedTarget?: TvEpisodeCursor | null;
  seasonLabel: string;
  episodeLabel: string;
  absoluteLabel: string;
  onSeasonChange: (season: number) => void;
  onRetryDetails: () => void;
  onRetrySeason: () => void;
}) {
  const count = seasonCount(details);
  const listedSeasons = listedSeasonNumbers(details);
  const seasons = selectableSeasonNumbers(details);
  const metadataIsCurrent =
    metadata.season === form.season && metadata.tmdbId === tmdbId;
  const currentMetadata = metadataIsCurrent
    ? metadata
    : { ...EMPTY_SEASON_METADATA, tmdbId: metadata.tmdbId, season: form.season };
  const seasonHelpId = `${idPrefix}-season-help`;
  const episodeHelpId = `${idPrefix}-episode-help`;
  const seasonOptions = seasons.map((season) => ({
    value: String(season),
    label: season === 0 ? "特别篇（第 0 季）" : `第 ${season} 季`,
  }));
  const episodeIsListed = currentMetadata.episodes.includes(form.startEpisode);
  const preservesUnlistedEpisode =
    currentMetadata.status === "ready" &&
    currentMetadata.episodes.length > 0 &&
    !episodeIsListed &&
    preservedTarget?.season === form.season &&
    preservedTarget.episode === form.startEpisode;
  const episodeOutsideList =
    currentMetadata.status === "ready" &&
    currentMetadata.episodes.length > 0 &&
    !episodeIsListed;
  const episodeOptions = [
    ...(episodeOutsideList
      ? [{
          value: String(form.startEpisode),
          label: preservesUnlistedEpisode
            ? `第 ${form.startEpisode} 集（保留当前追更位置）`
            : `第 ${form.startEpisode} 集（不在 TMDB 列表）`,
        }]
      : []),
    ...currentMetadata.episodes.map((episode) => ({
      value: String(episode),
      label: `第 ${episode} 集`,
    })),
  ];
  const seasonIsListed = seasons.includes(form.season);
  const seasonMax = seasons.length > 0 ? seasons[seasons.length - 1] : undefined;
  const useSeasonFallback = !detailsLoading && (seasons.length === 0 || !seasonIsListed);
  const useEpisodeFallback =
    currentMetadata.status === "error" ||
    (currentMetadata.status === "ready" && currentMetadata.episodes.length === 0);
  const episodeRange = currentMetadata.episodes.length > 0
    ? `${currentMetadata.episodes[0]}-${currentMetadata.episodes[currentMetadata.episodes.length - 1]}`
    : null;
  const currentSeasonLabel = form.season === 0 ? "特别篇（第 0 季）" : `第 ${form.season} 季`;

  return (
    <div className="flex flex-col gap-4">
      <div
        className="grid grid-cols-2 gap-2 rounded-lg border border-border bg-surface-container/60 p-1"
        role="group"
        aria-label="剧集编号方式"
      >
        {([
          ["season", "季 / 集"],
          ["absolute", "绝对集"],
        ] as const).map(([value, label]) => (
          <button
            key={value}
            type="button"
            aria-pressed={form.numberingMode === value}
            className={cn(
              "min-h-10 rounded-md px-3 text-sm font-medium transition-colors",
              form.numberingMode === value
                ? "bg-card text-foreground shadow-sm"
                : "text-muted hover:bg-accent",
            )}
            onClick={() => setForm((current) => ({ ...current, numberingMode: value }))}
          >
            {label}
          </button>
        ))}
      </div>

      <div
        className={cn(
          "grid gap-4 sm:grid-cols-2",
          form.numberingMode === "absolute" && "lg:grid-cols-3",
        )}
      >
        {detailsLoading ? (
          <FormSelect
            id={`${idPrefix}-season`}
            label={form.numberingMode === "absolute" ? `映射${seasonLabel}` : seasonLabel}
            value="loading"
            options={[{ value: "loading", label: "正在读取季信息" }]}
            disabled
            describedBy={seasonHelpId}
            onChange={() => undefined}
          />
        ) : useSeasonFallback ? (
          <FormNumber
            id={`${idPrefix}-season`}
            label={form.numberingMode === "absolute" ? `映射${seasonLabel}` : seasonLabel}
            value={form.season}
            min={0}
            max={seasonMax}
            describedBy={seasonHelpId}
            onChange={onSeasonChange}
          />
        ) : (
          <FormSelect
            id={`${idPrefix}-season`}
            label={form.numberingMode === "absolute" ? `映射${seasonLabel}` : seasonLabel}
            value={String(form.season)}
            options={seasonOptions}
            describedBy={seasonHelpId}
            onChange={(value) => onSeasonChange(numberValue(value, 0))}
          />
        )}

        {currentMetadata.status === "loading" || currentMetadata.status === "idle" ? (
          <FormSelect
            id={`${idPrefix}-episode`}
            label={form.numberingMode === "absolute" ? `映射季内集` : episodeLabel}
            value="loading"
            options={[
              {
                value: "loading",
                label: currentMetadata.status === "loading" ? `正在读取${currentSeasonLabel}` : "等待季信息",
              },
            ]}
            disabled
            describedBy={episodeHelpId}
            onChange={() => undefined}
          />
        ) : useEpisodeFallback ? (
          <FormNumber
            id={`${idPrefix}-episode`}
            label={form.numberingMode === "absolute" ? "映射季内集" : episodeLabel}
            value={form.startEpisode}
            min={1}
            describedBy={episodeHelpId}
            onChange={(startEpisode) =>
              setForm((current) => ({
                ...current,
                startEpisode: Math.max(1, Math.floor(startEpisode)),
              }))
            }
          />
        ) : (
          <FormSelect
            id={`${idPrefix}-episode`}
            label={form.numberingMode === "absolute" ? "映射季内集" : episodeLabel}
            value={String(form.startEpisode)}
            options={episodeOptions}
            describedBy={episodeHelpId}
            onChange={(value) =>
              setForm((current) => ({ ...current, startEpisode: numberValue(value, 1) }))
            }
          />
        )}

        {form.numberingMode === "absolute" ? (
          <FormNumber
            id={`${idPrefix}-absolute-episode`}
            label={absoluteLabel}
            value={form.absoluteEpisode}
            min={1}
            onChange={(absoluteEpisode) =>
              setForm((current) => ({
                ...current,
                absoluteEpisode: Math.max(1, Math.floor(absoluteEpisode)),
              }))
            }
          />
        ) : null}
      </div>

      <div className="flex flex-col gap-2 text-xs text-muted">
        <div id={seasonHelpId} className="flex flex-wrap items-center gap-2">
          {detailsLoading ? (
            <>
              <LoaderCircle className="size-4 animate-spin" aria-hidden="true" />
              正在读取季信息
            </>
          ) : detailsError ? (
            <>
              <span role="alert" className="text-destructive">
                季信息加载失败：{detailsError}。请确认手动季号；特别篇使用第 0 季。
              </span>
              <Button type="button" variant="outline" className="h-8 px-3" onClick={onRetryDetails}>
                <RefreshCw data-icon="inline-start" />重试
              </Button>
            </>
          ) : listedSeasons.length > 0 ? (
            <span>
              TMDB 返回 {listedSeasons.filter((season) => season > 0).length} 个常规季
              {listedSeasons.includes(0) ? "，含特别篇（第 0 季）" : ""}
              {!seasonIsListed ? `；当前${currentSeasonLabel}不在 TMDB 季列表` : ""}
            </span>
          ) : count != null ? (
            <span>
              共 {count} 季；特别篇使用第 0 季
              {!seasonIsListed ? `；当前${currentSeasonLabel}超出已知范围` : ""}
            </span>
          ) : (
            <span>TMDB 未提供季列表，请确认手动季号；特别篇使用第 0 季。</span>
          )}
        </div>

        <div id={episodeHelpId} className="flex flex-wrap items-center gap-2" aria-live="polite">
          {currentMetadata.status === "loading" ? (
            <>
              <LoaderCircle className="size-4 animate-spin" aria-hidden="true" />
              正在读取{currentSeasonLabel}集信息
            </>
          ) : currentMetadata.status === "error" ? (
            <>
              <span role="alert" className="text-destructive">
                {currentSeasonLabel}集信息加载失败：{currentMetadata.error}。请确认手动集号。
              </span>
              <Button type="button" variant="outline" className="h-8 px-3" onClick={onRetrySeason}>
                <RefreshCw data-icon="inline-start" />重试
              </Button>
            </>
          ) : preservesUnlistedEpisode && episodeRange ? (
            <span className="flex min-w-0 items-start gap-1.5 text-foreground">
              <AlertCircle className="mt-0.5 size-3.5 shrink-0" aria-hidden="true" />
              <span className="break-words">
                当前第 {form.startEpisode} 集尚未出现在 TMDB 列表，将保留该追更位置；
                {currentSeasonLabel}已知第 {episodeRange} 集 / 共 {currentMetadata.episodes.length} 集
              </span>
            </span>
          ) : episodeOutsideList && episodeRange ? (
            <span role="alert" className="break-words text-destructive">
              当前第 {form.startEpisode} 集不在 TMDB 列表，请改为有效集号；
              {currentSeasonLabel}已知第 {episodeRange} 集 / 共 {currentMetadata.episodes.length} 集
            </span>
          ) : currentMetadata.status === "ready" && episodeRange ? (
            <span>
              {currentSeasonLabel} · 第 {episodeRange} 集 / 共 {currentMetadata.episodes.length} 集
            </span>
          ) : currentMetadata.status === "ready" ? (
            <span>{currentSeasonLabel}未返回有效集号，请确认手动集号。</span>
          ) : (
            <span>等待季集元数据</span>
          )}
        </div>
      </div>
    </div>
  );
}

function FormNumber({
  id,
  label,
  value,
  onChange,
  min,
  max,
  disabled,
  describedBy,
}: {
  id: string;
  label: string;
  value: number;
  onChange: (value: number) => void;
  min?: number;
  max?: number;
  disabled?: boolean;
  describedBy?: string;
}) {
  return (
    <div className="flex flex-col gap-2">
      <Label htmlFor={id}>{label}</Label>
      <Input
        id={id}
        type="number"
        min={min}
        max={max}
        value={value}
        disabled={disabled}
        aria-describedby={describedBy}
        onChange={(event) => onChange(numberValue(event.target.value, min ?? 0))}
      />
    </div>
  );
}

function FormSelect({
  id,
  label,
  value,
  onChange,
  options,
  disabled,
  describedBy,
}: {
  id?: string;
  label: string;
  value: string;
  onChange: (value: string) => void;
  options: readonly { value: string; label: string }[];
  disabled?: boolean;
  describedBy?: string;
}) {
  const generatedId = useId();
  const controlId = id ?? generatedId;
  return (
    <div className="flex flex-col gap-2">
      <Label htmlFor={controlId}>{label}</Label>
      {options.length > 0 ? (
        <Select
          id={controlId}
          value={value}
          onChange={onChange}
          options={options}
          disabled={disabled}
          aria-describedby={describedBy}
        />
      ) : (
        <div className="flex h-11 items-center rounded-2xl border border-border bg-surface-container px-4 text-sm text-muted">
          暂无可选项
        </div>
      )}
    </div>
  );
}

function DelimitedField({ id, label, value, onChange }: { id: string; label: string; value: string; onChange: (value: string) => void }) {
  return (
    <div className="flex flex-col gap-2">
      <Label htmlFor={id}>{label}</Label>
      <Input id={id} value={value} onChange={(event) => onChange(event.target.value)} placeholder="用逗号分隔" />
    </div>
  );
}

function SitePicker({
  sites,
  selected,
  onChange,
  showSelectAll = false,
}: {
  sites: Site[];
  selected: number[];
  onChange: (ids: number[]) => void;
  showSelectAll?: boolean;
}) {
  const allSelected = sites.length > 0 && sites.every((site) => selected.includes(site.id));
  return (
    <fieldset className="flex flex-col gap-2">
      <legend className="sr-only">搜索站点</legend>
      <div className="flex items-center justify-between gap-3">
        <span className="text-sm font-medium" aria-hidden="true">搜索站点</span>
        {showSelectAll && sites.length > 0 ? (
          <label className="flex cursor-pointer items-center gap-2 text-sm text-muted">
            <input
              type="checkbox"
              className="size-4 accent-primary"
              checked={allSelected}
              onChange={() => onChange(allSelected ? [] : sites.map((site) => site.id))}
            />
            <span>{allSelected ? "取消全选" : "全选"}</span>
          </label>
        ) : null}
      </div>
      {sites.length === 0 ? (
        <div className="rounded-2xl border border-border bg-surface-container px-4 py-3 text-sm text-muted">暂无可用站点</div>
      ) : (
        <div className="grid gap-2 sm:grid-cols-2 lg:grid-cols-3">
          {sites.map((site) => {
            const checked = selected.includes(site.id);
            return (
              <label key={site.id} className="flex cursor-pointer items-center gap-3 rounded-2xl border border-border bg-surface-container/45 px-3 py-2.5 text-sm transition-colors hover:bg-accent">
                <input
                  type="checkbox"
                  className="size-4 accent-primary"
                  checked={checked}
                  onChange={() => onChange(checked ? selected.filter((id) => id !== site.id) : [...selected, site.id])}
                />
                <span className="min-w-0 truncate">{site.name}</span>
              </label>
            );
          })}
        </div>
      )}
    </fieldset>
  );
}

function SubscriptionsPanel({
  subscriptionsUnavailable, downloadsUnavailable, configurationUnavailable, onRetry,
  subscriptions,
  downloads,
  profiles,
  sites,
  downloaders,
  profileNames,
  siteNames,
  downloaderNames,
  busyKey,
  downloadsHaveMore,
  downloadsLoadingMore,
  onAdd,
  onAction,
  onViewRun,
  onEdit,
  onDelete,
  onRedeliver,
  onReconcileFailed,
  onDeleteDownload,
  onLoadMoreDownloads,
}: {
  subscriptionsUnavailable: boolean;
  downloadsUnavailable: boolean;
  configurationUnavailable: boolean;
  onRetry: () => void;
  subscriptions: Subscription[];
  downloads: MediaDownload[];
  profiles: QualityProfile[];
  sites: Site[];
  downloaders: Downloader[];
  profileNames: Map<number, string>;
  siteNames: Map<number, string>;
  downloaderNames: Map<number, string>;
  busyKey: string;
  downloadsHaveMore: boolean;
  downloadsLoadingMore: boolean;
  onAdd: () => void;
  onAction: (subscription: Subscription, action: "run" | "pause" | "resume") => void;
  onViewRun: (subscription: Subscription) => void;
  onEdit: (subscription: Subscription) => void;
  onDelete: (subscription: Subscription) => void;
  onRedeliver: (download: MediaDownload) => void;
  onReconcileFailed: (download: MediaDownload) => void;
  onDeleteDownload: (download: MediaDownload) => void;
  onLoadMoreDownloads: () => void;
}) {
  const [query, setQuery] = useState("");
  const [statusFilter, setStatusFilter] = useState("all");
  const visibleSubscriptions = subscriptions.filter((item) => {
    const matchesQuery = `${item.title} ${item.original_title ?? ""}`.toLocaleLowerCase().includes(query.trim().toLocaleLowerCase());
    const completed = subscriptionIsCompleted(item);
    return matchesQuery && (statusFilter === "all" || (statusFilter === "attention" && Boolean(item.last_error) && !completed)
      || (statusFilter === "active" && item.enabled && !completed) || (statusFilter === "paused" && !item.enabled && !completed)
      || (statusFilter === "completed" && completed));
  });
  function actions(subscription: Subscription) {
    const completed = subscriptionIsCompleted(subscription);
    return <div className="flex flex-wrap items-start gap-2 lg:justify-end">
      {subscription.last_run_at ? <Button variant="outline" className="h-11 px-3" onClick={() => onViewRun(subscription)}>详情</Button> : null}
      {!completed ? <Button variant="outline" className="h-11 px-3" disabled={busyKey === `run:${subscription.id}`} onClick={() => onAction(subscription, "run")}>
        {busyKey === `run:${subscription.id}` ? <LoaderCircle className="size-4 animate-spin" /> : <Play className="size-4" />}扫描
      </Button> : null}
      <details className="max-w-full rounded-lg border border-border">
        <summary className="flex min-h-11 cursor-pointer items-center gap-1 px-3 text-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">更多<span className="sr-only">：{subscription.title}</span><ChevronDown className="size-4" /></summary>
        <div className="flex flex-col gap-2 border-t border-border p-2">
          {!completed || subscription.media_type === "tv" ? <Button variant="outline" className="h-11 px-3" onClick={() => onEdit(subscription)}>编辑规则</Button> : null}
          {!completed ? <Button variant="outline" className="h-11 px-3" disabled={busyKey === `${subscription.enabled ? "pause" : "resume"}:${subscription.id}`} onClick={() => onAction(subscription, subscription.enabled ? "pause" : "resume")}>{subscription.enabled ? "暂停" : "恢复"}</Button> : null}
          <Button variant="outline" className="h-11 px-3 text-destructive" onClick={() => onDelete(subscription)}>删除订阅</Button>
        </div>
      </details>
    </div>;
  }
  const activeCount = subscriptions.filter((item) => item.enabled && !subscriptionIsCompleted(item)).length;
  const pausedCount = subscriptions.filter((item) => !item.enabled && !subscriptionIsCompleted(item)).length;
  const attentionCount = subscriptions.filter((item) => item.last_error && !subscriptionIsCompleted(item)).length;
  const queuedCount = downloads.filter((item) => !["submitted", "failed", "cancelled"].includes(item.status)).length;
  const submittedDownloadCount = downloads.filter((item) => item.status === "submitted").length;
  const failedDownloadCount = downloads.filter((item) => item.status === "failed").length;

  return (
    <div className="flex flex-col gap-6">
      <div className="flex flex-wrap items-center gap-2 text-sm" aria-label="订阅状态概览">
        {subscriptionsUnavailable ? <span className="text-muted">订阅状态暂不可用</span> : <>
          <span className="mr-2 text-muted">追更中 {activeCount} · 已暂停 {pausedCount}</span>
          <Button variant="outline" className="h-11" aria-pressed={statusFilter === "attention"} onClick={() => setStatusFilter(statusFilter === "attention" ? "all" : "attention")}>需处理 {attentionCount}</Button>
        </>}
      </div>

      <Card>
        <CardHeader className="flex-row items-center justify-between gap-4">
          <div>
            <CardTitle>追剧订阅</CardTitle>
            <CardDescription>{configurationUnavailable ? "配置暂时无法读取" : `${profiles.length} 套质量配置 · ${sites.length} 个站点 · ${downloaders.length} 个下载器`}</CardDescription>
          </div>
          <Button onClick={onAdd}>
            <Plus data-icon="inline-start" />
            添加
          </Button>
        </CardHeader>
        <CardContent>
          {!subscriptionsUnavailable && subscriptions.length > 0 ? <div className="mb-4 grid gap-3 sm:grid-cols-[minmax(0,1fr)_180px]">
            <div><Label htmlFor="subscription-query">查找订阅</Label><Input id="subscription-query" className="mt-2 min-h-11" placeholder="输入影视名称或原名" value={query} onChange={(event) => setQuery(event.target.value)} /></div>
            <div><Label htmlFor="subscription-status">状态筛选</Label><Select id="subscription-status" className="mt-2 min-h-11" value={statusFilter} onChange={setStatusFilter} options={[
              { value: "all", label: "全部状态" }, { value: "attention", label: "需处理" }, { value: "active", label: "追更中" }, { value: "paused", label: "已暂停" }, { value: "completed", label: "已完成" },
            ]} /></div>
            <p role="status" className="text-sm text-muted sm:col-span-2">显示 {visibleSubscriptions.length} / {subscriptions.length} 个订阅</p>
          </div> : null}
          {subscriptionsUnavailable ? <EmptyState icon={AlertCircle} title="订阅无法读取" action={{ label: "重新读取", onClick: onRetry }} /> : subscriptions.length === 0 ? (
            <EmptyState icon={Tv} title="暂无订阅" action={{ label: "添加影视", onClick: onAdd }} />
          ) : visibleSubscriptions.length === 0 ? (
            <EmptyState icon={Search} title="没有符合筛选条件的订阅" action={{ label: "清除筛选", onClick: () => { setQuery(""); setStatusFilter("all"); } }} />
          ) : (
            <>
              <div className="hidden lg:block">
                <Table>
                  <TableHeader>
                    <TableRow>
                      <TableHead>影视</TableHead>
                      <TableHead>当前目标</TableHead>
                      <TableHead>规则</TableHead>
                      <TableHead>状态</TableHead>
                      <TableHead>扫描时间</TableHead>
                      <TableHead className="text-right">操作</TableHead>
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    {visibleSubscriptions.map((subscription) => {
                      const status = subscriptionStatus(subscription);
                      const completed = subscriptionIsCompleted(subscription);
                      return (
                        <TableRow key={subscription.id}>
                          <TableCell>
                            <div className="flex min-w-[230px] items-center gap-3">
                              <Poster path={subscription.poster_path} title={subscription.title} className="w-11 shrink-0" />
                              <div className="min-w-0">
                                <div className="flex max-w-[280px] items-center gap-2">
                                  <div className="truncate font-semibold" title={subscription.title}>{subscription.title}</div>
                                  <StatusPill label={subscriptionCategoryLabel(subscription)} />
                                </div>
                                <div className="mt-0.5 text-xs text-muted">{subscription.year ?? "年份未知"}</div>
                                <GenrePills genres={subscription.tmdb_genres} />
                              </div>
                            </div>
                          </TableCell>
                          <TableCell>
                            {subscriptionTargetLabel(subscription)}
                          </TableCell>
                          <TableCell>
                            <div className="max-w-[220px] text-sm">
                              <div className="truncate">{profileNames.get(subscription.quality_profile_id) ?? `配置 #${subscription.quality_profile_id}`}</div>
                              <div className="mt-0.5 truncate text-xs text-muted">{subscription.site_ids.map((id) => siteNames.get(id) ?? `#${id}`).join("、") || "无站点"}</div>
                            </div>
                          </TableCell>
                          <TableCell>
                            <StatusPill label={status.label} tone={status.tone} />
                            {subscription.last_error ? <div className="mt-1 max-w-48 truncate text-xs text-destructive" title={subscription.last_error}>{subscription.last_error}</div> : null}
                          </TableCell>
                          <TableCell className="text-xs text-muted">
                            <div>上次 {formatDate(subscription.last_run_at)}</div>
                            <div className="mt-1">
                              {completed ? "无需继续扫描" : `下次 ${formatDate(subscription.next_run_at)}`}
                            </div>
                          </TableCell>
                          <TableCell>
                            {actions(subscription)}
                          </TableCell>
                        </TableRow>
                      );
                    })}
                  </TableBody>
                </Table>
              </div>

              <div className="grid gap-3 lg:hidden">
                {visibleSubscriptions.map((subscription) => {
                  const status = subscriptionStatus(subscription);
                  const completed = subscriptionIsCompleted(subscription);
                  return (
                    <article key={subscription.id} className="rounded-xl border border-border bg-surface-container/45 p-4">
                      <div className="flex gap-3">
                        <Poster path={subscription.poster_path} title={subscription.title} className="w-16 shrink-0" />
                        <div className="min-w-0 flex-1">
                          <div className="flex items-start justify-between gap-3">
                            <div className="min-w-0">
                              <h3 className="truncate text-sm font-semibold">{subscription.title}</h3>
                              <div className="mt-1 flex flex-wrap items-center gap-2">
                                <StatusPill label={subscriptionCategoryLabel(subscription)} />
                                <GenrePills genres={subscription.tmdb_genres} />
                                <span className="text-xs text-muted">{subscriptionTargetLabel(subscription, true)}</span>
                              </div>
                            </div>
                            <StatusPill label={status.label} tone={status.tone} />
                          </div>
                          <div className="mt-3 grid gap-1.5 text-xs text-muted sm:grid-cols-2">
                            <span>{profileNames.get(subscription.quality_profile_id) ?? `配置 #${subscription.quality_profile_id}`}</span>
                            <span>{downloaderNames.get(subscription.downloader_id) ?? `下载器 #${subscription.downloader_id}`}</span>
                            <span className="truncate sm:col-span-2">{subscription.site_ids.map((id) => siteNames.get(id) ?? `#${id}`).join("、")}</span>
                            <span>{completed ? "无需继续扫描" : `下次扫描 ${formatDate(subscription.next_run_at)}`}</span>
                          </div>
                        </div>
                      </div>
                      {subscription.last_error ? <p className="mt-3 break-words text-xs text-destructive">{subscription.last_error}</p> : null}
                      <div className="mt-4">{actions(subscription)}</div>
                    </article>
                  );
                })}
              </div>
            </>
          )}
        </CardContent>
      </Card>

      <Card>
        <CardHeader className="flex-row items-start justify-between gap-4">
          <div>
            <CardTitle>下载记录</CardTitle>
            <CardDescription>显示的是任务提交状态；“已提交下载器”不代表文件已经下载完成。状态统计仅针对已加载记录。</CardDescription>
          </div>
          <div className={cn("hidden shrink-0 flex-wrap justify-end gap-2 sm:flex", downloadsUnavailable && "!hidden")}>
            <StatusPill label={`${submittedDownloadCount} 已提交`} tone="positive" />
            <StatusPill label={`${queuedCount} 处理中`} />
            {failedDownloadCount > 0 ? <StatusPill label={`${failedDownloadCount} 失败`} tone="negative" /> : null}
          </div>
        </CardHeader>
        <CardContent>
          {downloadsUnavailable ? <EmptyState icon={AlertCircle} title="下载记录无法读取" action={{ label: "重新读取", onClick: onRetry }} /> : downloads.length === 0 ? (
            <EmptyState icon={HardDriveDownload} title="暂无下载任务" />
          ) : (
            <div className="grid gap-2">
              {downloads.map((download) => {
                const targetLabel = targetKeyLabel(download.target_key);
                const qualityFields = releaseQualityFields(download.parsed_release).filter((field) => field !== targetLabel);
                const notice = downloadNotice(download);
                const canReconcileFailed = download.failed_reconciliation_allowed;
                const reconcilingFailed = busyKey === `reconcile-failed:${download.id}`;
                return (
                  <div key={download.id} className="grid gap-3 rounded-2xl border border-border bg-surface-container/40 px-4 py-3 lg:grid-cols-[minmax(0,1fr)_minmax(220px,auto)] lg:items-center">
                    <div className="min-w-0">
                      <div className="flex flex-wrap items-center gap-2">
                        <StatusPill label={targetLabel} />
                        {qualityFields.map((field) => <StatusPill key={field} label={field} />)}
                        {qualityFields.length === 0 ? <StatusPill label="质量未识别" tone="negative" /> : null}
                      </div>
                      <div className="mt-2 line-clamp-2 text-sm font-semibold" title={download.title}>{download.title}</div>
                      <div className="mt-1 flex flex-wrap gap-x-3 gap-y-1 text-xs text-muted">
                        <span>{download.source_site}</span>
                        <span>{formatBytes(download.size)}</span>
                        <span>{formatDate(download.submitted_at ?? download.updated_at)}</span>
                        {download.attempts > 1 ? <span>尝试 {download.attempts} 次</span> : null}
                      </div>
                      {notice ? (
                        <div
                          className={cn("mt-2 line-clamp-2 text-xs", notice.negative ? "text-destructive" : "text-muted")}
                          title={notice.text}
                        >
                          {notice.text}
                        </div>
                      ) : null}
                    </div>
                    <div className="flex flex-wrap items-center justify-end gap-2">
                      <span className="text-xs text-muted">{download.downloader_name}</span>
                      <StatusPill label={downloadStatus(download.status)} tone={downloadTone(download.status)} />
                      {download.status === "submitted" ? (
                        <Button
                          variant="outline"
                          className="h-8 px-3"
                          disabled={busyKey === `redeliver:${download.id}`}
                          onClick={() => onRedeliver(download)}
                        >
                          {busyKey === `redeliver:${download.id}` ? (
                            <LoaderCircle className="animate-spin" data-icon="inline-start" />
                          ) : (
                            <RefreshCw data-icon="inline-start" />
                          )}
                          {busyKey === `redeliver:${download.id}` ? "核验中" : "核验/补交"}
                        </Button>
                      ) : null}
                      {canReconcileFailed ? (
                        <Button
                          variant="outline"
                          className="h-8 px-3"
                          disabled={reconcilingFailed}
                          onClick={() => onReconcileFailed(download)}
                        >
                          {reconcilingFailed ? (
                            <LoaderCircle className="animate-spin" data-icon="inline-start" />
                          ) : (
                            <ShieldCheck data-icon="inline-start" />
                          )}
                          {reconcilingFailed ? "核验中" : "核验 qB"}
                        </Button>
                      ) : null}
                      {["submitted", "failed", "cancelled"].includes(download.status) ? (
                        <Button
                          variant="destructive"
                          className="size-8 p-0"
                          title="删除本地下载记录"
                          aria-label={`删除${download.title}的本地下载记录`}
                          disabled={
                            busyKey === `delete-download:${download.id}`
                            || busyKey === `reconcile-failed:${download.id}`
                          }
                          onClick={() => onDeleteDownload(download)}
                        >
                          {busyKey === `delete-download:${download.id}` ? (
                            <LoaderCircle className="animate-spin" />
                          ) : (
                            <Trash2 />
                          )}
                        </Button>
                      ) : null}
                    </div>
                  </div>
                );
              })}
              <div className="flex flex-wrap items-center justify-between gap-3 px-1 pt-2 text-xs text-muted">
                <span>{downloadsHaveMore ? `已显示 ${downloads.length} 条` : `已显示全部 ${downloads.length} 条`}</span>
                {downloadsHaveMore ? (
                  <Button
                    variant="outline"
                    className="h-8 px-3"
                    disabled={downloadsLoadingMore}
                    onClick={onLoadMoreDownloads}
                  >
                    {downloadsLoadingMore ? (
                      <LoaderCircle className="animate-spin" data-icon="inline-start" />
                    ) : (
                      <ChevronDown data-icon="inline-start" />
                    )}
                    {downloadsLoadingMore ? "加载中" : "加载更多"}
                  </Button>
                ) : null}
              </div>
            </div>
          )}
        </CardContent>
      </Card>
    </div>
  );
}

function SubscriptionRunDetails({
  snapshot,
  loading,
  error,
  subscriptionId,
  downloads,
}: {
  snapshot: SubscriptionRunSnapshot | null;
  loading: boolean;
  error: string;
  subscriptionId: number | null;
  downloads: MediaDownload[];
}) {
  if (loading) return <LoadingState label="正在加载运行详情" />;
  if (error) {
    return (
      <div className="p-4 sm:p-6">
        <div role="alert" className="rounded-lg border border-destructive/25 bg-destructive/5 px-4 py-3 text-sm text-destructive">
          {error}
        </div>
      </div>
    );
  }
  if (!snapshot) return <EmptyState icon={FileSearch2} title="暂无运行详情" />;

  const acceptedCount = snapshot.candidates.filter((candidate) => candidate.decision?.accepted).length;
  const rejectedCount = snapshot.candidates.filter((candidate) => candidate.decision && !candidate.decision.accepted).length;
  const parseFailedCount = snapshot.candidates.filter((candidate) => candidate.parseError).length;
  const selectedCandidate = snapshot.candidates.find((candidate) => candidate.candidateId === snapshot.best_candidate_id) ?? null;
  const selectedDownload = selectedCandidate && subscriptionId != null
    ? downloads.find((download) => downloadMatchesCandidate(download, selectedCandidate, subscriptionId, snapshot.target_key)) ?? null
    : null;
  const selectedOutcome = selectedCandidate
    ? candidateProcessingOutcome(selectedCandidate, selectedDownload, true)
    : {
        label: "未选择资源",
        detail: snapshot.candidates.length > 0 ? "候选均未通过规则，因此没有创建下载任务。" : "搜索没有返回候选资源。",
        tone: "neutral" as const,
      };
  const targetLabel = targetKeyLabel(snapshot.target_key);
  const selectedQuality = releaseQualityFields(selectedDownload?.parsed_release ?? selectedCandidate?.release)
    .filter((field) => field !== targetLabel);

  return (
    <div className="flex flex-col gap-6 p-4 sm:p-6">
      {snapshot.error ? (
        <div role="alert" className="rounded-lg border border-destructive/25 bg-destructive/5 px-4 py-3 text-sm text-destructive">
          本次运行失败：{snapshot.error}
        </div>
      ) : null}

      <section
        aria-labelledby="run-outcome-heading"
        className={cn(
          "grid gap-4 rounded-2xl border px-4 py-4 sm:grid-cols-[auto_minmax(0,1fr)] sm:px-5",
          selectedOutcome.tone === "positive"
            ? "border-primary/25 bg-primary/10"
            : selectedOutcome.tone === "negative"
              ? "border-destructive/25 bg-destructive/5"
              : "border-border bg-surface-container/45",
        )}
      >
        <div className={cn(
          "flex size-10 items-center justify-center rounded-xl border bg-card",
          selectedOutcome.tone === "negative" ? "text-destructive" : "text-foreground",
        )}>
          {selectedOutcome.tone === "positive" ? (
            <Send className="size-5" aria-hidden="true" />
          ) : selectedOutcome.tone === "negative" ? (
            <CircleX className="size-5" aria-hidden="true" />
          ) : (
            <CircleDashed className="size-5" aria-hidden="true" />
          )}
        </div>
        <div className="min-w-0">
          <div className="flex flex-wrap items-center gap-2">
            <h4 id="run-outcome-heading" className="text-base font-semibold">{selectedOutcome.label}</h4>
            {selectedCandidate ? <StatusPill label={`第 ${selectedCandidate.rank} 名`} /> : null}
            {selectedDownload ? <StatusPill label={selectedDownload.downloader_name} /> : null}
          </div>
          {selectedCandidate ? (
            <>
              <p className="mt-1 line-clamp-2 text-sm font-medium" title={selectedCandidate.result.title}>{selectedCandidate.result.title}</p>
              <div className="mt-2 flex flex-wrap gap-1.5">
                <StatusPill label={targetLabel} />
                {selectedQuality.map((field) => <StatusPill key={field} label={field} />)}
                <StatusPill label={formatBytes(selectedCandidate.result.size)} />
                <StatusPill label={selectedCandidate.result.source_site} />
              </div>
            </>
          ) : null}
          <p className={cn("mt-2 text-xs leading-5", selectedOutcome.tone === "negative" ? "text-destructive" : "text-muted")}>{selectedOutcome.detail}</p>
          {selectedDownload?.status === "submitted" ? (
            <p className="mt-1 text-xs text-muted">已发送给下载器，文件是否完成请以下载器实时状态为准。</p>
          ) : null}
        </div>
      </section>

      <dl className="grid grid-cols-2 gap-px overflow-hidden rounded-lg border border-border bg-border lg:grid-cols-4">
        {[
          ["返回候选", snapshot.candidates.length],
          ["符合规则", acceptedCount],
          ["拒绝 / 解析失败", `${rejectedCount} / ${parseFailedCount}`],
          ["成功站点", `${snapshot.successful_sites} / ${snapshot.total_sites}`],
        ].map(([label, value]) => (
          <div
            key={label}
             className="bg-card px-4 py-3"
          >
            <dt className="text-xs text-muted">{label}</dt>
            <dd className="mt-1 text-xl font-semibold tabular-nums">{value}</dd>
          </div>
        ))}
      </dl>

      <section aria-labelledby="run-candidates-heading">
        <div className="flex items-center justify-between gap-3">
          <h4 id="run-candidates-heading" className="text-sm font-semibold">搜索结果匹配记录</h4>
          <span className="text-xs text-muted">每条记录均标明是否进入下载流程</span>
        </div>
        {snapshot.candidates.length === 0 ? (
          <div className="mt-3 rounded-lg border border-dashed border-border py-8 text-center text-sm text-muted">
            搜索没有返回候选资源。
          </div>
        ) : (
          <>
            <div className="mt-3 hidden overflow-hidden rounded-lg border border-border lg:block">
              <Table>
                <TableHeader>
                     <TableRow>
                       <TableHead>处理结果</TableHead>
                       <TableHead>资源</TableHead>
                       <TableHead>质量 / 范围</TableHead>
                       <TableHead>匹配依据</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                   {snapshot.candidates.map((candidate) => {
                     const candidateDownload = subscriptionId == null
                       ? null
                       : downloads.find((download) => downloadMatchesCandidate(download, candidate, subscriptionId, snapshot.target_key)) ?? null;
                     const selected = candidate.candidateId === snapshot.best_candidate_id;
                     const outcome = candidateProcessingOutcome(candidate, candidateDownload, selected);
                     return (
                     <TableRow key={candidate.key} aria-current={selected ? "true" : undefined}>
                       <TableCell><CandidateOutcomeSummary outcome={outcome} /></TableCell>
                       <TableCell>
                         <div className="max-w-[360px]">
                           <div className="line-clamp-2 text-sm font-semibold" title={candidate.result.title}>{candidate.result.title}</div>
                           <div className="mt-1 flex flex-wrap items-center gap-x-2 gap-y-1">
                             {selected ? <StatusPill label="最终选择" tone="positive" /> : null}
                             <span className="text-xs text-muted">{candidate.result.source_site}</span>
                             <span className="text-xs text-muted">{formatBytes(candidate.result.size)}</span>
                             <span className="text-xs text-muted">{candidate.result.seeders} 做种</span>
                           </div>
                         </div>
                       </TableCell>
                       <TableCell><ReleaseSummary candidate={candidate} /></TableCell>
                       <TableCell><DecisionSummary candidate={candidate} /></TableCell>
                     </TableRow>
                     );
                   })}
                </TableBody>
              </Table>
            </div>

            <div className="mt-3 grid gap-3 lg:hidden">
               {snapshot.candidates.map((candidate) => {
                 const candidateDownload = subscriptionId == null
                   ? null
                   : downloads.find((download) => downloadMatchesCandidate(download, candidate, subscriptionId, snapshot.target_key)) ?? null;
                 const selected = candidate.candidateId === snapshot.best_candidate_id;
                 const outcome = candidateProcessingOutcome(candidate, candidateDownload, selected);
                 return (
                 <article key={candidate.key} className="rounded-lg border border-border p-4">
                   <div className="flex items-start justify-between gap-3">
                     <h5 className="min-w-0 flex-1 break-words text-sm font-semibold">{candidate.result.title}</h5>
                     <StatusPill label={outcome.label} tone={outcome.tone} />
                   </div>
                   <div className="mt-2 flex flex-wrap gap-2 text-xs text-muted">
                     {selected ? <StatusPill label="最终选择" tone="positive" /> : null}
                     <span>{candidate.result.source_site}</span>
                    <span>{formatBytes(candidate.result.size)}</span>
                    <span>{candidate.result.seeders} 做种</span>
                  </div>
                   <div className="mt-3 grid gap-3 sm:grid-cols-2">
                     <ReleaseSummary candidate={candidate} />
                     <DecisionSummary candidate={candidate} />
                   </div>
                   <p className={cn("mt-3 text-xs leading-5", outcome.tone === "negative" ? "text-destructive" : "text-muted")}>{outcome.detail}</p>
                 </article>
                 );
               })}
            </div>
          </>
        )}
      </section>

      <div className="grid gap-6 lg:grid-cols-2">
        <section aria-labelledby="run-query-heading">
          <h4 id="run-query-heading" className="text-sm font-semibold">搜索关键词</h4>
          {snapshot.queries.length > 0 ? (
            <ol className="mt-3 flex flex-wrap gap-2">
              {snapshot.queries.map((query, index) => (
                <li key={`${query}:${index}`} className="max-w-full rounded-md border border-border bg-surface-container px-3 py-1.5 font-mono text-xs break-all">
                  {query}
                </li>
              ))}
            </ol>
          ) : (
            <p className="mt-2 text-sm text-muted">本次运行未发起资源搜索。</p>
          )}
        </section>

        {snapshot.site_errors.length > 0 ? (
          <section aria-labelledby="run-errors-heading">
            <div className="flex items-center gap-2">
              <h4 id="run-errors-heading" className="text-sm font-semibold">站点请求错误</h4>
              <StatusPill label={`${snapshot.site_errors.length} 条`} tone="negative" />
            </div>
            <div className="mt-3 grid gap-2">
              {snapshot.site_errors.map((siteError, index) => (
                <div key={`${siteError.site_id}:${siteError.query}:${index}`} className="rounded-lg border border-destructive/20 px-3 py-2 text-xs">
                  <div className="font-semibold text-destructive">{siteError.source_site} · {siteError.code}</div>
                  <div className="mt-1 break-words text-muted">关键词：{siteError.query || "-"} · {siteError.message}</div>
                </div>
              ))}
            </div>
          </section>
        ) : (
          <section aria-labelledby="run-errors-heading">
            <h4 id="run-errors-heading" className="text-sm font-semibold">站点请求</h4>
            <p className="mt-2 text-sm text-muted">本次没有站点请求错误。</p>
          </section>
        )}
      </div>
    </div>
  );
}

type CandidateProcessingOutcome = {
  label: string;
  detail: string;
  tone: "positive" | "negative" | "neutral";
};

function candidateProcessingOutcome(
  candidate: ResourceCandidate,
  download: MediaDownload | null,
  selected: boolean,
): CandidateProcessingOutcome {
  if (download) {
    if (download.status === "submitted") {
      return {
        label: "已提交下载器",
        detail: `${download.downloader_name} · ${formatDate(download.submitted_at ?? download.updated_at)}`,
        tone: "positive",
      };
    }
    if (download.status === "failed" || download.status === "cancelled") {
      return {
        label: downloadStatus(download.status),
        detail: download.last_error || "下载任务未能提交到下载器。",
        tone: "negative",
      };
    }
    return {
      label: downloadStatus(download.status),
      detail: download.last_error || `${download.downloader_name} · 已尝试 ${download.attempts} 次`,
      tone: "neutral",
    };
  }
  if (selected) {
    return {
      label: "已选中，未入队",
      detail: "规则选择了该资源，但当前没有找到对应下载任务。",
      tone: "negative",
    };
  }
  if (candidate.parseError) {
    return { label: "未下载", detail: `标题解析失败：${candidate.parseError}`, tone: "negative" };
  }
  if (candidate.decision?.accepted) {
    return { label: "符合但未选", detail: "通过全部规则，但在分辨率、片源、体积充足度、视频特性、编码或做种数的逐级排序中低于最终选择。", tone: "neutral" };
  }
  if (candidate.decision) {
    return {
      label: "未下载",
      detail: candidate.decision.rejections.map(rejectionText).join("；") || "不符合匹配规则。",
      tone: "negative",
    };
  }
  return { label: "未下载", detail: "没有关联订阅目标，未执行自动选择。", tone: "neutral" };
}

function CandidateOutcomeSummary({ outcome }: { outcome: CandidateProcessingOutcome }) {
  return (
    <div className="max-w-56 text-xs">
      <StatusPill label={outcome.label} tone={outcome.tone} />
      <div className={cn("mt-1.5 line-clamp-3 leading-5", outcome.tone === "negative" ? "text-destructive" : "text-muted")} title={outcome.detail}>
        {outcome.detail}
      </div>
    </div>
  );
}

function TmdbPanel({
  query,
  mediaType,
  results,
  loading,
  searched,
  onQueryChange,
  onMediaTypeChange,
  onSearch,
  onSubscribe,
}: {
  query: string;
  mediaType: "multi" | MediaType;
  results: TmdbMedia[];
  loading: boolean;
  searched: boolean;
  onQueryChange: (value: string) => void;
  onMediaTypeChange: (value: string) => void;
  onSearch: (event: FormEvent) => void;
  onSubscribe: (media: TmdbMedia) => void;
}) {
  return (
    <div className="flex flex-col gap-6">
      <Card>
        <CardHeader>
          <CardTitle>添加影视</CardTitle>
          <CardDescription>TMDB 影视库</CardDescription>
        </CardHeader>
        <CardContent>
          <form className="grid gap-3 md:grid-cols-[minmax(0,1fr)_180px_auto]" onSubmit={onSearch}>
            <div className="flex flex-col gap-2">
              <Label htmlFor="tmdb-query">影视名称</Label>
              <Input id="tmdb-query" value={query} onChange={(event) => onQueryChange(event.target.value)} placeholder="输入中文名或原名" autoComplete="off" />
            </div>
            <FormSelect
              label="类型"
              value={mediaType}
              onChange={onMediaTypeChange}
              options={[
                { value: "multi", label: "全部" },
                { value: "tv", label: "剧集" },
                { value: "movie", label: "电影" },
              ]}
            />
            <div className="flex items-end">
              <Button type="submit" className="w-full md:w-auto" disabled={loading || !query.trim()}>
                {loading ? <LoaderCircle className="animate-spin" data-icon="inline-start" /> : <Search data-icon="inline-start" />}
                {loading ? "搜索中" : "搜索"}
              </Button>
            </div>
          </form>
        </CardContent>
      </Card>

      {loading ? (
        <LoadingState label="正在搜索 TMDB" />
      ) : results.length > 0 ? (
        <section aria-label="TMDB 搜索结果">
          <div className="mb-3 flex items-center justify-between gap-3">
            <h3 className="text-sm font-semibold">搜索结果</h3>
            <span className="text-xs text-muted">{results.length} 项</span>
          </div>
          <div className="grid gap-4 md:grid-cols-2 xl:grid-cols-3">
            {results.map((media) => (
              <Card key={`${media.media_type}:${media.tmdb_id}`} className="rounded-xl">
                <CardContent className="flex h-full gap-4 p-4">
                  <Poster path={media.poster_path} title={media.title} className="w-24 shrink-0" />
                  <div className="flex min-w-0 flex-1 flex-col">
                    <div className="flex items-start justify-between gap-2">
                      <div className="min-w-0">
                        <h3 className="line-clamp-2 text-sm font-semibold">{media.title}</h3>
                        {media.original_title ? <p className="mt-1 truncate text-xs text-muted">{media.original_title}</p> : null}
                      </div>
                      <StatusPill label={tmdbCategoryLabel(media)} />
                    </div>
                    <GenrePills genres={media.genres} />
                    <p className="mt-2 line-clamp-3 text-xs leading-5 text-muted">{media.overview || "暂无简介"}</p>
                    <div className="mt-auto flex items-end justify-between gap-3 pt-4">
                      <span className="text-xs text-muted">{media.year ?? "年份未知"}</span>
                      <Button className="h-8 px-3" onClick={() => onSubscribe(media)}>
                        <Plus data-icon="inline-start" />订阅
                      </Button>
                    </div>
                  </div>
                </CardContent>
              </Card>
            ))}
          </div>
        </section>
      ) : searched ? (
        <EmptyState icon={Film} title="没有找到匹配影视" />
      ) : (
        <EmptyState icon={Search} title="输入影视名称开始搜索" />
      )}
    </div>
  );
}

function ResourcesPanel({
  form,
  setForm,
  subscriptions,
  profiles,
  sites,
  downloaders,
  candidates,
  tmdbResults,
  errors,
  loading,
  searched,
  busyKey,
  queuedCandidateKeys = new Set<string>(),
  onSearch,
  onQueue,
}: {
  form: ResourceForm;
  setForm: Dispatch<SetStateAction<ResourceForm>>;
  subscriptions: Subscription[];
  profiles: QualityProfile[];
  sites: Site[];
  downloaders: Downloader[];
  candidates: ResourceCandidate[];
  tmdbResults: TmdbMedia[];
  errors: string[];
  loading: boolean;
  searched: boolean;
  busyKey: string;
  queuedCandidateKeys?: ReadonlySet<string>;
  onSearch: (event: FormEvent) => void;
  onQueue: (candidate: ResourceCandidate) => void;
}) {
  const [localQuery, setLocalQuery] = useState("");
  const deferredLocalQuery = useDeferredValue(localQuery);
  const [scopeFilter, setScopeFilter] = useState<"all" | LocalReleaseScopeKind>("all");
  const scopedCandidates = useMemo<ScopedResourceCandidate[]>(
    () => candidates.map((candidate) => ({
      candidate,
      localScope: parseLocalReleaseScope(candidate.result.title),
    })),
    [candidates],
  );
  const scopeCounts = useMemo<Record<LocalReleaseScopeKind, number>>(() => {
    const counts: Record<LocalReleaseScopeKind, number> = {
      season_episode: 0,
      season_only: 0,
      season_pack: 0,
      episode_only: 0,
      absolute_episode: 0,
      unmatched: 0,
    };
    for (const item of scopedCandidates) counts[item.localScope.kind] += 1;
    return counts;
  }, [scopedCandidates]);
  const visibleCandidates = useMemo(
    () => scopedCandidates.filter(({ candidate, localScope }) => {
      if (scopeFilter !== "all" && localScope.kind !== scopeFilter) return false;
      return matchesLocalResourceQuery(
        [
          candidate.result.title,
          candidate.result.source_site,
          candidate.result.torrent_id,
          candidate.release?.title ?? "",
          ...(candidate.release?.alternate_titles ?? []),
          candidate.release?.resolution ?? "",
          candidate.release?.source ?? "",
          candidate.release?.codec ?? "",
        ],
        localScope,
        deferredLocalQuery,
      );
    }),
    [deferredLocalQuery, scopeFilter, scopedCandidates],
  );
  const acceptedCount = visibleCandidates.filter(({ candidate }) => candidate.decision?.accepted).length;
  return (
    <div className="flex flex-col gap-6">
      <Card>
        <CardHeader>
          <CardTitle>资源搜索</CardTitle>
          <CardDescription>{sites.length} 个可用搜索源</CardDescription>
        </CardHeader>
        <CardContent>
          <form className="flex flex-col gap-5" onSubmit={onSearch}>
            <div className="grid gap-4 md:grid-cols-2 xl:grid-cols-4">
              <div className="flex flex-col gap-2 md:col-span-2">
                <Label htmlFor="resource-query">关键词</Label>
                <Input id="resource-query" value={form.query} onChange={(event) => setForm((current) => ({ ...current, query: event.target.value }))} placeholder="影视名、季集或发布标题" autoComplete="off" />
              </div>
              <FormSelect
                label="关联目标"
                value={form.subscriptionId}
                onChange={(subscriptionId) => {
                  const subscription = subscriptions.find((item) => String(item.id) === subscriptionId);
                  setForm((current) => ({
                    ...current,
                    subscriptionId,
                    qualityProfileId: subscription ? String(subscription.quality_profile_id) : current.qualityProfileId,
                    downloaderId: subscription ? String(subscription.downloader_id) : current.downloaderId,
                    siteIds: subscription?.site_ids.length ? subscription.site_ids : current.siteIds,
                  }));
                }}
                options={[
                  { value: "", label: "仅按关键词" },
                  ...subscriptions.filter((subscription) => !subscriptionIsCompleted(subscription)).map((subscription) => ({
                    value: String(subscription.id),
                    label: `${subscription.title}${
                      subscription.media_type === "movie"
                        ? ""
                        : subscription.absolute_episode != null
                          ? ` Abs ${subscription.absolute_episode}`
                          : ` S${String(subscription.season ?? 1).padStart(2, "0")}E${String(subscription.next_episode ?? subscription.start_episode ?? 1).padStart(2, "0")}`
                    }`,
                  })),
                ]}
              />
              <FormSelect label="质量配置" value={form.qualityProfileId} onChange={(qualityProfileId) => setForm((current) => ({ ...current, qualityProfileId }))} options={profiles.map((profile) => ({ value: String(profile.id), label: profile.name }))} />
              <FormSelect label="下载器" value={form.downloaderId} onChange={(downloaderId) => setForm((current) => ({ ...current, downloaderId }))} options={downloaders.map((downloader) => ({ value: String(downloader.id), label: downloader.name }))} />
            </div>
            <SitePicker sites={sites} selected={form.siteIds} showSelectAll onChange={(siteIds) => setForm((current) => ({ ...current, siteIds }))} />
            <div className="flex justify-end">
              <Button type="submit" disabled={loading || (!form.query.trim() && !form.subscriptionId) || form.siteIds.length === 0}>
                {loading ? <LoaderCircle className="animate-spin" data-icon="inline-start" /> : <Search data-icon="inline-start" />}
                {loading ? "搜索中" : "搜索资源"}
              </Button>
            </div>
          </form>
        </CardContent>
      </Card>

      {errors.length > 0 ? (
        <div className="flex flex-col gap-2" role="alert">
          {errors.map((error, index) => (
            <div key={`${error}:${index}`} className="flex items-start gap-2 rounded-2xl border border-destructive/25 bg-destructive/5 px-4 py-3 text-sm text-destructive">
              <AlertCircle className="mt-0.5 size-4 shrink-0" />
              <span className="break-words">{error}</span>
            </div>
          ))}
        </div>
      ) : null}

      {resourceTmdbResults(tmdbResults)}

      {loading ? (
        <LoadingState label="正在并发搜索站点" />
      ) : candidates.length > 0 ? (
        <Card>
          <CardHeader className="flex-row items-center justify-between gap-4">
            <div>
              <CardTitle>候选资源</CardTitle>
              <CardDescription aria-live="polite">
                {visibleCandidates.length === candidates.length
                  ? `${candidates.length} 个结果`
                  : `显示 ${visibleCandidates.length} / ${candidates.length} 个结果`}
                {` · ${acceptedCount} 个通过匹配`}
              </CardDescription>
            </div>
          </CardHeader>
          <CardContent>
            <div className="mb-5 border-b border-border pb-5">
              <div className="max-w-2xl">
                <Label htmlFor="resource-local-query">本地快速查找</Label>
                <div className="relative mt-2">
                  <Search className="pointer-events-none absolute left-3.5 top-1/2 size-4 -translate-y-1/2 text-muted" aria-hidden="true" />
                  <Input
                    id="resource-local-query"
                    value={localQuery}
                    onChange={(event) => setLocalQuery(event.target.value)}
                    className="pl-10 pr-10"
                    placeholder="标题、来源站点或 S01E01"
                    autoComplete="off"
                  />
                  {localQuery ? (
                    <button
                      type="button"
                      aria-label="清空本地查找"
                      title="清空本地查找"
                      className="absolute right-2 top-1/2 flex size-8 -translate-y-1/2 items-center justify-center rounded-lg text-muted transition-colors hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/40"
                      onClick={() => setLocalQuery("")}
                    >
                      <X className="size-4" aria-hidden="true" />
                    </button>
                  ) : null}
                </div>
              </div>
              <div className="mt-3 flex flex-wrap gap-2" role="group" aria-label="季集识别类型">
                {LOCAL_SCOPE_FILTERS.map((filter) => {
                  const count = filter.value === "all" ? candidates.length : scopeCounts[filter.value];
                  const active = scopeFilter === filter.value;
                  const highlightsMissingEpisode = filter.value === "season_only" && count > 0;
                  return (
                    <button
                      key={filter.value}
                      type="button"
                      aria-pressed={active}
                      disabled={count === 0 && filter.value !== "all"}
                      onClick={() => setScopeFilter(filter.value)}
                      className={cn(
                        "inline-flex h-8 items-center gap-1.5 rounded-lg border px-2.5 text-xs font-semibold transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/40 disabled:cursor-not-allowed disabled:opacity-45",
                        active
                          ? highlightsMissingEpisode
                            ? "border-amber-400 bg-amber-100 text-amber-950"
                            : "border-primary bg-primary text-primary-foreground"
                          : highlightsMissingEpisode
                            ? "border-amber-300 bg-amber-50 text-amber-900 hover:bg-amber-100"
                            : "border-border bg-card text-muted hover:border-primary/35 hover:bg-accent hover:text-foreground",
                      )}
                    >
                      {highlightsMissingEpisode ? <AlertCircle className="size-3.5" aria-hidden="true" /> : null}
                      <span>{filter.label}</span>
                      <span className={cn("tabular-nums", active && !highlightsMissingEpisode ? "text-primary-foreground/80" : "opacity-70")}>
                        {count}
                      </span>
                    </button>
                  );
                })}
              </div>
            </div>

            {visibleCandidates.length > 0 ? (
              <>
                <div className="hidden xl:block">
                  <Table>
                    <TableHeader>
                      <TableRow>
                        <TableHead>资源</TableHead>
                        <TableHead>解析</TableHead>
                        <TableHead>匹配</TableHead>
                        <TableHead>来源站点</TableHead>
                        <TableHead>活跃度</TableHead>
                        <TableHead className="text-right">操作</TableHead>
                      </TableRow>
                    </TableHeader>
                    <TableBody>
                      {visibleCandidates.map(({ candidate, localScope }) => (
                        <TableRow key={candidate.key}>
                          <TableCell>
                            <div className="max-w-[380px]">
                              <div className="line-clamp-2 font-semibold" title={candidate.result.title}>{candidate.result.title}</div>
                              <div className="mt-1 text-xs text-muted">{formatBytes(candidate.result.size)} · {formatDate(candidate.result.publish_time)}</div>
                              <div className="mt-2"><LocalScopeBadge scope={localScope} /></div>
                            </div>
                          </TableCell>
                          <TableCell><ReleaseSummary candidate={candidate} /></TableCell>
                          <TableCell><DecisionSummary candidate={candidate} /></TableCell>
                          <TableCell><SourceSiteBadge name={candidate.result.source_site} /></TableCell>
                          <TableCell>
                            <div className="text-sm font-semibold">{candidate.result.seeders} 做种</div>
                            <div className="mt-1 text-xs text-muted">{candidate.result.leechers} 下载</div>
                          </TableCell>
                          <TableCell>
                            <div className="flex justify-end">
                              <Button
                                variant={queuedCandidateKeys.has(candidate.key) ? "outline" : candidateNeedsOverride(candidate) ? "destructive" : "default"}
                                className="h-8 px-3"
                                disabled={busyKey === `queue:${candidate.key}`}
                                onClick={() => onQueue(candidate)}
                              >
                                {busyKey === `queue:${candidate.key}` ? <LoaderCircle className="animate-spin" data-icon="inline-start" /> : queuedCandidateKeys.has(candidate.key) ? <RefreshCw data-icon="inline-start" /> : <Download data-icon="inline-start" />}
                                {queuedCandidateKeys.has(candidate.key) ? "检查并重新入队" : candidateNeedsOverride(candidate) ? "覆盖入队" : "入队"}
                              </Button>
                            </div>
                          </TableCell>
                        </TableRow>
                      ))}
                    </TableBody>
                  </Table>
                </div>

                <div className="grid gap-3 xl:hidden">
                  {visibleCandidates.map(({ candidate, localScope }) => (
                    <article key={candidate.key} className="rounded-xl border border-border bg-surface-container/45 p-4">
                      <div className="flex items-start justify-between gap-3">
                        <div className="min-w-0 flex-1">
                          <h3 className="line-clamp-2 text-sm font-semibold">{candidate.result.title}</h3>
                          <div className="mt-2 flex flex-wrap items-center gap-2 text-xs text-muted">
                            <SourceSiteBadge name={candidate.result.source_site} compact />
                            <span>{formatBytes(candidate.result.size)}</span>
                            <span>{candidate.result.seeders} 做种</span>
                          </div>
                          <div className="mt-2"><LocalScopeBadge scope={localScope} /></div>
                        </div>
                        <DecisionBadge candidate={candidate} />
                      </div>
                      <div className="mt-3 grid gap-3 sm:grid-cols-2">
                        <ReleaseSummary candidate={candidate} />
                        <DecisionSummary candidate={candidate} />
                      </div>
                      <div className="mt-4 flex justify-end">
                        <Button variant={queuedCandidateKeys.has(candidate.key) ? "outline" : candidateNeedsOverride(candidate) ? "destructive" : "default"} className="h-8 px-3" disabled={busyKey === `queue:${candidate.key}`} onClick={() => onQueue(candidate)}>
                          {busyKey === `queue:${candidate.key}` ? <LoaderCircle className="animate-spin" data-icon="inline-start" /> : queuedCandidateKeys.has(candidate.key) ? <RefreshCw data-icon="inline-start" /> : <Download data-icon="inline-start" />}
                          {queuedCandidateKeys.has(candidate.key) ? "检查并重新入队" : candidateNeedsOverride(candidate) ? "覆盖入队" : "加入下载队列"}
                        </Button>
                      </div>
                    </article>
                  ))}
                </div>
              </>
            ) : (
              <EmptyState icon={FileSearch2} title="当前本地条件没有匹配结果" />
            )}
          </CardContent>
        </Card>
      ) : searched ? (
        <EmptyState icon={Search} title="没有找到可用资源" />
      ) : (
        <EmptyState icon={Search} title="输入关键词或选择订阅目标" />
      )}
    </div>
  );
}

function SourceSiteBadge({ name, compact = false }: { name: string; compact?: boolean }) {
  return (
    <span
      className={cn(
        "inline-flex max-w-[14rem] items-center gap-1.5 border border-primary/20 bg-primary/10 font-semibold text-foreground",
        compact ? "rounded-lg px-2 py-1 text-xs" : "rounded-full px-2.5 py-1 text-xs",
      )}
      title={`来源站点：${name}`}
    >
      <Globe2 className="size-3.5 shrink-0 text-primary" aria-hidden="true" />
      <span className="truncate">{compact ? `来源 · ${name}` : name}</span>
    </span>
  );
}

function LocalScopeBadge({ scope }: { scope: LocalReleaseScope }) {
  const missingEpisode = scope.kind === "season_only";
  const incomplete = missingEpisode || scope.kind === "episode_only" || scope.kind === "unmatched";
  return (
    <span
      className={cn(
        "inline-flex max-w-full items-center gap-1.5 rounded-lg border px-2 py-1 text-xs font-semibold",
        missingEpisode
          ? "border-amber-300 bg-amber-50 text-amber-900"
          : incomplete
            ? "border-border bg-surface-container text-muted"
            : "border-jade/30 bg-jade/10 text-foreground",
      )}
      title={scope.kind === "season_only" ? "标题中识别到季号，但没有识别到集数" : scope.label}
    >
      {missingEpisode ? <AlertCircle className="size-3.5 shrink-0" aria-hidden="true" /> : null}
      <span className="truncate">{scope.label}</span>
    </span>
  );
}

function tmdbCategoryLabel(media: TmdbMedia): string {
  if (media.media_type === "movie") return "电影";
  return media.is_animation ? "动漫" : "电视剧";
}

function subscriptionCategoryLabel(subscription: Subscription): string {
  if (subscription.media_type === "movie") return "电影";
  return subscription.tmdb_is_animation ? "动漫" : "电视剧";
}

function GenrePills({ genres }: { genres: TmdbGenre[] }) {
  if (!genres?.length) return null;
  return <div className="flex flex-wrap gap-1">{genres.map((genre) => (
    <StatusPill key={genre.id} label={genre.name} />
  ))}</div>;
}

function resourceTmdbResults(results: TmdbMedia[]) {
  if (results.length === 0) return null;
  return (
    <section aria-label="TMDB 匹配结果" className="flex flex-col gap-3">
      <div className="flex items-center justify-between gap-3">
        <h3 className="text-sm font-semibold">TMDB 匹配结果</h3>
        <span className="text-xs text-muted">{results.length} 项</span>
      </div>
      <div className="grid gap-3 md:grid-cols-2 xl:grid-cols-3">
        {results.slice(0, 6).map((media) => (
          <Card key={`resource-tmdb:${media.media_type}:${media.tmdb_id}`}>
            <CardContent className="flex gap-3 p-3">
              <Poster path={media.poster_path} title={media.title} className="w-16 shrink-0" />
              <div className="flex min-w-0 flex-1 flex-col gap-1">
                <div className="flex items-start justify-between gap-2">
                  <h4 className="line-clamp-2 text-sm font-semibold">{media.title}</h4>
                  <StatusPill label={tmdbCategoryLabel(media)} />
                </div>
                <GenrePills genres={media.genres} />
                {media.original_title ? (
                  <p className="truncate text-xs text-muted" title={media.original_title}>{media.original_title}</p>
                ) : null}
                <p className="mt-auto text-xs text-muted">{media.year ?? "年份未知"} · TMDB {media.tmdb_id}</p>
              </div>
            </CardContent>
          </Card>
        ))}
      </div>
    </section>
  );
}

function ReleaseSummary({ candidate }: { candidate: ResourceCandidate }) {
  if (candidate.parseError) {
    return <div className="max-w-56 text-xs text-destructive">解析失败：{candidate.parseError}</div>;
  }
  if (!candidate.release) return <span className="text-xs text-muted">无解析数据</span>;
  const fields = releaseQualityFields(candidate.release);
  return (
    <div className="max-w-56 text-xs">
      <div className="flex flex-wrap gap-1.5">
        {fields.length > 0 ? fields.map((field) => <StatusPill key={field} label={field as string} />) : <span className="text-muted">质量未知</span>}
      </div>
      <div className="mt-1.5 truncate text-muted" title={candidate.release.title}>{candidate.release.title}</div>
    </div>
  );
}

function DecisionBadge({ candidate }: { candidate: ResourceCandidate }) {
  if (!candidate.decision) return <StatusPill label="未评估" />;
  return <StatusPill label={candidate.decision.accepted ? `${candidate.decision.score} 分` : "已拒绝"} tone={candidate.decision.accepted ? "positive" : "negative"} />;
}

function rejectionText(rejection: MatchRejection): string {
  const labels: Record<string, string> = {
    wrong_media: "资源类型与订阅目标不一致",
    wrong_title: "标题与订阅名称或别名不匹配",
    wrong_year: "年份与订阅目标不一致",
    wrong_season: "季号与当前目标不一致",
    wrong_episode: "资源不包含当前目标集数",
    ambiguous_numbering: "无法安全识别资源集数编号",
    season_pack_not_allowed: "当前目标不接受整季资源",
    quality_not_allowed: "画质、来源或编码不符合质量规则",
    unknown_quality: "无法识别资源质量且规则不允许未知质量",
    minimum_seeders: "做种人数低于质量规则要求",
    below_minimum_score: "综合匹配分低于质量规则门槛",
  };
  return labels[rejection.code] ?? rejection.message;
}

function DecisionSummary({ candidate }: { candidate: ResourceCandidate }) {
  if (!candidate.decision) return <span className="text-xs text-muted">未关联目标</span>;
  if (candidate.decision.accepted) {
    const breakdown = Object.entries(candidate.decision.breakdown)
      .filter(([, score]) => score > 0)
      .map(([name, score]) => `${decisionBreakdownLabel(name)} +${score}`)
      .join(" · ");
    return (
      <div className="max-w-64 text-xs">
        <DecisionBadge candidate={candidate} />
        {breakdown ? <div className="mt-1.5 text-muted">{breakdown}</div> : null}
        <CandidateRankingSummary candidate={candidate} />
      </div>
    );
  }
  return (
    <div className="max-w-64 text-xs">
      <DecisionBadge candidate={candidate} />
      <div className="mt-1.5 text-destructive" title={candidate.decision.rejections.map(rejectionText).join("；")}>
        {candidate.decision.rejections.map(rejectionText).join("；") || "不符合匹配规则"}
      </div>
    </div>
  );
}

function CandidateRankingSummary({ candidate }: { candidate: ResourceCandidate }) {
  const sortKey = candidate.sortKey;
  if (!sortKey || sortKey.size_target <= 0) return null;
  const fitness = Math.max(0, Math.min(100, Math.floor(sortKey.size_fitness / 10)));
  const normalized = sortKey.size_per_item === candidate.result.size ? "体积" : "单集体积";
  const hasVideoFeatures = Boolean(candidate.release?.hdr_formats.length)
    || (candidate.release?.bit_depth ?? 0) >= 10;
  const featureStatus = hasVideoFeatures
    ? ` · 视频特性加成${sortKey.video_feature_rank > 0 ? "已启用" : "未启用"}`
    : "";
  return (
    <div
      className="mt-1.5 text-muted"
      title={`${normalized} ${formatBytes(sortKey.size_per_item)}，充足参考 ${formatBytes(sortKey.size_target)}`}
    >
      {normalized}充足度 {fitness}%{featureStatus} · {candidate.result.seeders} 做种
    </div>
  );
}

function decisionBreakdownLabel(name: string): string {
  const labels: Record<string, string> = {
    title: "标题",
    year: "年份",
    season: "季号",
    episode: "集数",
    quality: "质量",
  };
  return labels[name] ?? name;
}

function SettingsPanel({
  settingsUnavailable, profilesUnavailable, onRetry,
  settings,
  setSettings,
  clearTmdbToken,
  setClearTmdbToken,
  profiles,
  downloaders,
  openListSettings,
  setOpenListSettings,
  busyKey,
  onSaveSettings,
  onSaveOpenList,
  onAddQuality,
  onResetQuality,
  onEditQuality,
  onDeleteQuality,
}: {
  settingsUnavailable: boolean;
  profilesUnavailable: boolean;
  onRetry: () => void;
  settings: MediaSettings;
  setSettings: Dispatch<SetStateAction<MediaSettings>>;
  clearTmdbToken: boolean;
  setClearTmdbToken: Dispatch<SetStateAction<boolean>>;
  profiles: QualityProfile[];
  downloaders: Downloader[];
  openListSettings: OpenListAutomationSettings | null;
  setOpenListSettings: Dispatch<SetStateAction<OpenListAutomationSettings | null>>;
  busyKey: string;
  onSaveSettings: () => void;
  onSaveOpenList: () => void;
  onAddQuality: () => void;
  onResetQuality: () => void;
  onEditQuality: (profile: QualityProfile) => void;
  onDeleteQuality: (profile: QualityProfile) => void;
}) {
  const [section, setSection] = useHashChoice("section", SETTINGS_SECTIONS, "media");
  return (
    <div className="flex flex-col gap-6">
      <div role="tablist" aria-label="设置分区" className="flex flex-wrap gap-2 border-b border-border pb-3">
        {SETTINGS_SECTIONS.map((key, index) => <button key={key} type="button" role="tab"
          id={`settings-tab-${key}`} aria-controls={`settings-panel-${key}`} aria-selected={section === key}
          tabIndex={section === key ? 0 : -1}
          onClick={() => setSection(key)}
          onKeyDown={(event) => {
            const next = event.key === "ArrowRight" ? (index + 1) % 3 : event.key === "ArrowLeft" ? (index + 2) % 3 : event.key === "Home" ? 0 : event.key === "End" ? 2 : -1;
            if (next < 0) return;
            event.preventDefault(); setSection(SETTINGS_SECTIONS[next]);
            document.getElementById(`settings-tab-${SETTINGS_SECTIONS[next]}`)?.focus();
          }}
          className={cn("min-h-11 rounded-lg px-4 text-sm font-semibold focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary", section === key ? "bg-primary text-primary-foreground" : "text-muted hover:bg-accent")}>
          {SETTINGS_LABELS[key]}
        </button>)}
      </div>
      <div id="settings-panel-media" role="tabpanel" aria-labelledby="settings-tab-media" hidden={section !== "media"}>
      {settingsUnavailable ? <EmptyState icon={AlertCircle} title="媒体设置无法读取，暂不能编辑" action={{ label: "重新读取", onClick: onRetry }} /> : <Card>
        <CardHeader>
          <CardTitle>媒体设置</CardTitle>
          <CardDescription>TMDB 与自动扫描</CardDescription>
        </CardHeader>
        <CardContent className="flex flex-col gap-5">
          <div className="grid gap-4 md:grid-cols-2 xl:grid-cols-3">
            <div className="flex flex-col gap-2 md:col-span-2 xl:col-span-3">
              <div className="flex items-center justify-between gap-3">
                <Label htmlFor="tmdb-token">TMDB API Key 或 Read Token</Label>
                {settings.tmdb_token_configured && !clearTmdbToken ? <StatusPill label="已配置" tone="positive" /> : null}
              </div>
              <div className="flex gap-2">
                <Input
                  id="tmdb-token"
                  type="password"
                  autoComplete="off"
                  value={settings.tmdb_token ?? ""}
                  placeholder={settings.tmdb_token_configured && !clearTmdbToken ? "留空以保留现有密钥" : "输入 TMDB 密钥"}
                  onChange={(event) => {
                    setClearTmdbToken(false);
                    setSettings((current) => ({ ...current, tmdb_token: event.target.value }));
                  }}
                />
                {settings.tmdb_token_configured ? (
                  <Button
                    type="button"
                    variant="outline"
                    className={cn("h-10 w-10 shrink-0 px-0", clearTmdbToken && "border-destructive text-destructive")}
                    title={clearTmdbToken ? "取消清除密钥" : "清除已配置的 TMDB 密钥"}
                    aria-label={clearTmdbToken ? "取消清除 TMDB 密钥" : "清除 TMDB 密钥"}
                    onClick={() => {
                      setSettings((current) => ({ ...current, tmdb_token: null }));
                      setClearTmdbToken((current) => !current);
                    }}
                  >
                    {clearTmdbToken ? <X className="size-4" /> : <Trash2 className="size-4" />}
                  </Button>
                ) : null}
              </div>
            </div>
            <div className="flex flex-col gap-2">
              <Label htmlFor="tmdb-language">TMDB 语言</Label>
              <Input id="tmdb-language" value={settings.tmdb_language} onChange={(event) => setSettings((current) => ({ ...current, tmdb_language: event.target.value }))} placeholder="zh-CN" />
            </div>
            <FormNumber id="media-scan-interval" label="扫描间隔（分钟）" min={1} value={settings.scan_interval_mins} onChange={(scan_interval_mins) => setSettings((current) => ({ ...current, scan_interval_mins }))} />
            <FormNumber id="media-max-queries" label="单次搜索查询上限" min={2} value={settings.max_search_queries} onChange={(max_search_queries) => setSettings((current) => ({ ...current, max_search_queries }))} />
            <FormNumber id="media-search-concurrency" label="并发搜索站点数" min={1} value={settings.search_concurrency} onChange={(search_concurrency) => setSettings((current) => ({ ...current, search_concurrency }))} />
          </div>
          <div className="flex justify-end border-t border-border pt-4">
            <Button disabled={busyKey === "save-settings"} onClick={onSaveSettings}>
              {busyKey === "save-settings" ? <LoaderCircle className="animate-spin" data-icon="inline-start" /> : <Settings2 data-icon="inline-start" />}
              {busyKey === "save-settings" ? "保存中" : "保存设置"}
            </Button>
          </div>
        </CardContent>
      </Card>}

      </div>
      <div id="settings-panel-openlist" role="tabpanel" aria-labelledby="settings-tab-openlist" hidden={section !== "openlist"}>
      {openListSettings ? (
        <OpenListAutomationPanel
          settings={openListSettings}
          setSettings={(value) => setOpenListSettings((current) => {
            if (!current) return current;
            return typeof value === "function" ? value(current) : value;
          })}
          downloaders={downloaders}
          saving={busyKey === "save-openlist"}
          onSave={onSaveOpenList}
        />
      ) : <EmptyState icon={AlertCircle} title="OpenList 设置暂不可用" action={{ label: "重新读取", onClick: onRetry }} />}
      </div>

      <div id="settings-panel-quality" role="tabpanel" aria-labelledby="settings-tab-quality" hidden={section !== "quality"}>
      {profilesUnavailable ? <EmptyState icon={AlertCircle} title="质量配置无法读取，暂不能编辑" action={{ label: "重新读取", onClick: onRetry }} /> : <Card>
        <CardHeader className="flex-row items-center justify-between gap-4">
          <div>
            <CardTitle>质量配置</CardTitle>
            <CardDescription>{profiles.length} 套下载规则</CardDescription>
          </div>
          <div className="flex flex-wrap gap-2">
            <Button variant="outline" onClick={onResetQuality}>
              <RotateCcw data-icon="inline-start" />恢复默认
            </Button>
            <Button onClick={onAddQuality}>
              <Plus data-icon="inline-start" />新建
            </Button>
          </div>
        </CardHeader>
        <CardContent>
          {profiles.length === 0 ? (
            <EmptyState icon={SlidersHorizontal} title="暂无质量配置" action={{ label: "新建配置", onClick: onAddQuality }} />
          ) : (
            <>
              <div className="hidden md:block">
                <Table>
                  <TableHeader>
                    <TableRow>
                      <TableHead>名称</TableHead>
                      <TableHead>分辨率</TableHead>
                      <TableHead>来源</TableHead>
                      <TableHead>编码</TableHead>
                      <TableHead>门槛</TableHead>
                      <TableHead className="text-right">操作</TableHead>
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    {profiles.map((profile) => (
                      <TableRow key={profile.id}>
                        <TableCell className="font-semibold">{profile.name}</TableCell>
                        <TableCell><TokenList values={profile.resolution_order} /></TableCell>
                        <TableCell><TokenList values={profile.source_order} /></TableCell>
                        <TableCell><TokenList values={profile.codec_order} /></TableCell>
                        <TableCell>
                          <div className="text-sm">{profile.minimum_score} 分</div>
                          <div className="mt-1 text-xs text-muted">至少 {profile.min_seeders} 做种</div>
                        </TableCell>
                        <TableCell>
                          <div className="flex justify-end gap-2">
                            <Button variant="outline" className="h-8 px-3" onClick={() => onEditQuality(profile)}>
                              <Pencil data-icon="inline-start" />编辑
                            </Button>
                            <Button variant="destructive" className="h-8 px-3" onClick={() => onDeleteQuality(profile)}>
                              <Trash2 data-icon="inline-start" />删除
                            </Button>
                          </div>
                        </TableCell>
                      </TableRow>
                    ))}
                  </TableBody>
                </Table>
              </div>
              <div className="grid gap-3 md:hidden">
                {profiles.map((profile) => (
                  <article key={profile.id} className="rounded-xl border border-border bg-surface-container/45 p-4">
                    <div className="flex items-start justify-between gap-3">
                      <div>
                        <h3 className="text-sm font-semibold">{profile.name}</h3>
                        <p className="mt-1 text-xs text-muted">{profile.minimum_score} 分 · 至少 {profile.min_seeders} 做种</p>
                      </div>
                      <StatusPill label={profile.allow_unknown_quality ? "允许未知" : "拒绝未知"} />
                    </div>
                    <div className="mt-3 flex flex-col gap-2 text-xs">
                      <div><span className="text-muted">分辨率：</span>{profile.resolution_order.join(" · ") || "-"}</div>
                      <div><span className="text-muted">来源：</span>{profile.source_order.join(" · ") || "-"}</div>
                      <div><span className="text-muted">编码：</span>{profile.codec_order.join(" · ") || "-"}</div>
                    </div>
                    <div className="mt-4 flex gap-2">
                      <Button variant="outline" className="h-8 px-3" onClick={() => onEditQuality(profile)}><Pencil data-icon="inline-start" />编辑</Button>
                      <Button variant="destructive" className="h-8 px-3" onClick={() => onDeleteQuality(profile)}><Trash2 data-icon="inline-start" />删除</Button>
                    </div>
                  </article>
                ))}
              </div>
            </>
          )}
        </CardContent>
      </Card>}
      </div>
    </div>
  );
}

function TokenList({ values }: { values: string[] }) {
  if (values.length === 0) return <span className="text-xs text-muted">-</span>;
  return (
    <div className="flex max-w-64 flex-wrap gap-1">
      {values.slice(0, 4).map((value) => <StatusPill key={value} label={value} />)}
      {values.length > 4 ? <StatusPill label={`+${values.length - 4}`} /> : null}
    </div>
  );
}
