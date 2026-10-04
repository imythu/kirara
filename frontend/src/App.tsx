import { Suspense, lazy, useEffect, useId, useLayoutEffect, useMemo, useRef, useState, type SVGProps } from "react";
import {
  BarChart3,
  BookOpen,
  ChevronDown,
  Search,
  Database,
  Download,
  FileText,
  Gift,
  HardDrive,
  FolderInput,
  LayoutDashboard,
  Menu,
  CalendarCheck,
  Clock3,
  Rss,
  Settings,
  Tag,
  Tv,
  X,
} from "lucide-react";
import { createPortal } from "react-dom";
import { Button } from "@/components/ui/button";
import { Dialog, getFocusableElements } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Notice } from "@/components/ui/notice";
import { LoadingState } from "@/components/ui/state";
import { Select } from "@/components/ui/select";
import { cn } from "@/lib/utils";
import { APP_VERSION, api, defaultSettings, subscribeLogs } from "@/lib/api";
import type { GlobalConfig } from "@/types";

const MAX_LOG_LINES = 500;
const LOG_FLUSH_INTERVAL_MS = 250;
const LOG_LEVEL_PRIORITY = {
  trace: 10,
  debug: 20,
  info: 30,
  warn: 40,
  error: 50,
} as const;

type LogLevel = keyof typeof LOG_LEVEL_PRIORITY;
const LOG_LEVELS: LogLevel[] = ["trace", "debug", "info", "warn", "error"];

type AppPage =
  | "system-overview"
  | "media"
  | "rss"
  | "sites"
  | "invite-profile"
  | "downloaders"
  | "torrent-transfer"
  | "brush-tasks"
  | "scheduled-tasks"
  | "sign-in"
  | "tag-rules"
  | "stats"
  | "system-settings";

type NavGroup = "resources" | "connections" | "automation" | "system";
const navGroups: Array<{ key: NavGroup; label: string }> = [
  { key: "resources", label: "资源与订阅" },
  { key: "connections", label: "连接配置" },
  { key: "automation", label: "自动任务" },
  { key: "system", label: "系统与监控" },
];

// 页面模块加载器：既用于 lazy，也用于悬停/空闲时预取，切换页面时无需再等待分包下载。
const pageLoaders = {
  sites: () => import("@/pages/sites-page"),
  "invite-profile": () => import("@/pages/invite-profile-page"),
  downloaders: () => import("@/pages/downloaders-page"),
  "torrent-transfer": () => import("@/pages/torrent-transfer-page"),
  "brush-tasks": () => import("@/pages/brush-tasks-page"),
  "scheduled-tasks": () => import("@/pages/scheduled-tasks-page"),
  "sign-in": () => import("@/pages/sign-in-page"),
  "tag-rules": () => import("@/pages/tag-rules-page"),
  stats: () => import("@/pages/stats-page"),
  "system-settings": () => import("@/pages/system-settings-page"),
  "system-overview": () => import("@/pages/system-overview-page"),
  media: () => import("@/pages/media-page"),
  rss: () => import("@/pages/rss-page"),
} satisfies Record<AppPage, () => Promise<unknown>>;

const prefetchedPages = new Set<AppPage>();
function prefetchPage(page: AppPage) {
  if (prefetchedPages.has(page)) return;
  prefetchedPages.add(page);
  pageLoaders[page]().catch(() => prefetchedPages.delete(page));
}

const SitesPage = lazy(() => pageLoaders.sites().then((module) => ({ default: module.SitesPage })));
const InviteProfilePage = lazy(() => pageLoaders["invite-profile"]().then((module) => ({ default: module.InviteProfilePage })));
const DownloadersPage = lazy(() => pageLoaders.downloaders().then((module) => ({ default: module.DownloadersPage })));
const TorrentTransferPage = lazy(() => pageLoaders["torrent-transfer"]().then((module) => ({ default: module.TorrentTransferPage })));
const BrushTasksPage = lazy(() => pageLoaders["brush-tasks"]().then((module) => ({ default: module.BrushTasksPage })));
const ScheduledTasksPage = lazy(() => pageLoaders["scheduled-tasks"]().then((module) => ({ default: module.ScheduledTasksPage })));
const SignInPage = lazy(() => pageLoaders["sign-in"]().then((module) => ({ default: module.SignInPage })));
const TagRulesPage = lazy(() => pageLoaders["tag-rules"]().then((module) => ({ default: module.TagRulesPage })));
const StatsPage = lazy(() => pageLoaders.stats().then((module) => ({ default: module.StatsPage })));
const SystemSettingsPage = lazy(() => pageLoaders["system-settings"]().then((module) => ({ default: module.SystemSettingsPage })));
const SystemOverviewPage = lazy(() => pageLoaders["system-overview"]().then((module) => ({ default: module.SystemOverviewPage })));
const MediaPage = lazy(() => pageLoaders.media().then((module) => ({ default: module.MediaPage })));
const RssPage = lazy(() => pageLoaders.rss().then((module) => ({ default: module.RssPage })));

const navItems: Array<{
  key: AppPage;
  label: string;
  description: string;
  icon: typeof LayoutDashboard;
  group: NavGroup;
}> = [
  { key: "system-overview", label: "系统总览", description: "CPU、内存使用率与历史趋势", icon: LayoutDashboard, group: "system" },
  {
    key: "media",
    label: "自动追剧",
    description: "TMDB 订阅、PT 聚合搜索与自动下载",
    icon: Tv,
    group: "resources",
  },
  {
    key: "sites",
    label: "站点管理",
    description: "PT站点配置、连接测试与上传下载统计",
    icon: Database,
    group: "connections",
  },
  {
    key: "rss",
    label: "RSS 下载",
    description: "订阅站点更新，自动下载符合条件的新资源",
    icon: Rss,
    group: "resources",
  },
  {
    key: "invite-profile",
    label: "求药发药",
    description: "复制站点+UID 求药信息；粘贴后逐个查询公开资料",
    icon: Gift,
    group: "resources",
  },
  {
    key: "downloaders",
    label: "下载器",
    description: "管理下载客户端与空间状态",
    icon: HardDrive,
    group: "connections",
  },
  {
    key: "torrent-transfer",
    label: "种子转移",
    description: "选择 qBittorrent 种子并跟踪 OpenList 转移进度",
    icon: FolderInput,
    group: "resources",
  },
  {
    key: "brush-tasks",
    label: "刷流任务",
    description: "自动刷流任务配置、选种与删种规则",
    icon: Download,
    group: "automation",
  },
  { key: "scheduled-tasks", label: "定时任务", description: "自定义执行计划、HTTP 请求与执行记录", icon: Clock3, group: "automation" },
  {
    key: "sign-in",
    label: "自动签到",
    description: "NexusPHP 站点自动签到任务与执行记录",
    icon: CalendarCheck,
    group: "automation",
  },
  {
    key: "tag-rules",
    label: "标签规则",
    description: "根据 Tracker URL 自动匹配并管理种子标签",
    icon: Tag,
    group: "automation",
  },
  {
    key: "stats",
    label: "数据统计",
    description: "上传下载量、种子数与下载器趋势",
    icon: BarChart3,
    group: "system",
  },
  {
    key: "system-settings",
    label: "系统设置",
    description: "全局日志级别与系统运行设置",
    icon: Settings,
    group: "system",
  },
];

function readPageFromHash(): AppPage {
  const raw = window.location.hash.replace(/^#\/?/, "").split("?")[0];
  const valid: AppPage[] = [
    "system-overview",
    "media",
    "rss",
    "sites",
    "invite-profile",
    "downloaders",
    "torrent-transfer",
    "brush-tasks",
    "scheduled-tasks",
    "sign-in",
    "tag-rules",
    "stats",
    "system-settings",
  ];
  if (valid.includes(raw as AppPage)) {
    return raw as AppPage;
  }
  return raw === "" ? "system-overview" : "brush-tasks";
}

function setHash(page: AppPage, remembered?: string) {
  const next = remembered || (page === "system-overview" ? "#/" : `#/${page}`);
  if (window.location.hash !== next) {
    window.location.hash = next;
  }
}

function extractLogLevel(line: string): LogLevel | null {
  const normalized = line.toLowerCase();
  if (normalized.includes(" trace ")) return "trace";
  if (normalized.includes(" debug ")) return "debug";
  if (normalized.includes(" info ")) return "info";
  if (normalized.includes(" warn ")) return "warn";
  if (normalized.includes(" error ")) return "error";
  return null;
}

function getEffectiveLogLevel(settings: GlobalConfig): LogLevel {
  const level = settings.log_level?.trim().toLowerCase();
  if (level && level in LOG_LEVEL_PRIORITY) {
    return level as LogLevel;
  }
  return "info";
}

export default function App() {
  const [page, setPage] = useState<AppPage>(readPageFromHash());
  const lastVisited = useRef(new Map<AppPage, string>());
  const [menuOpen, setMenuOpen] = useState(false);
  const [selfUse, setSelfUse] = useState(false);
  const [settings, setSettings] = useState<GlobalConfig>(defaultSettings);
  const [savedSettings, setSavedSettings] = useState<GlobalConfig>(defaultSettings);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [message, setMessage] = useState("");
  const [navQuery, setNavQuery] = useState("");
  const [closedGroups, setClosedGroups] = useState<NavGroup[]>([]);
  const menuPanelRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLElement>(null);
  const [logsOpen, setLogsOpen] = useState(false);

  const currentNav =
    page === "system-overview"
      ? { key: "system-overview" as AppPage, label: "系统总览", description: "CPU、内存使用率实时监控与历史趋势", icon: LayoutDashboard, group: "system" as NavGroup }
      : navItems.find((item) => item.key === page) ?? navItems[0];
  const visibleNavItems = navItems.filter((item) =>
    (item.key !== "torrent-transfer" || selfUse) && item.label.toLowerCase().includes(navQuery.trim().toLowerCase()),
  );
  const effectiveLogLevel = getEffectiveLogLevel(settings);

  async function loadSettings() {
    const loaded = await api<GlobalConfig>("/api/settings");
    setSettings(loaded);
    setSavedSettings(loaded);
  }

  useEffect(() => {
    const onHashChange = () => {
      const nextPage = readPageFromHash();
      lastVisited.current.set(nextPage, window.location.hash);
      setPage(nextPage);
    };
    window.addEventListener("hashchange", onHashChange);
    return () => window.removeEventListener("hashchange", onHashChange);
  }, []);

  useEffect(() => {
    Promise.all([
      loadSettings().catch((error: Error) => setMessage(error.message)),
      api<{ self_use?: boolean }>("/api/features")
        .then((features) => setSelfUse(features.self_use === true))
        .catch(() => setSelfUse(false)),
    ])
      .catch((error: Error) => setMessage(error.message))
      .finally(() => setLoading(false));
  }, []);

  // 进入系统设置时重新读取服务端配置，丢弃上次离开时未保存的草稿。
  const enteredSettingsRef = useRef(page === "system-settings");
  useEffect(() => {
    if (page !== "system-settings") {
      enteredSettingsRef.current = false;
      return;
    }
    if (enteredSettingsRef.current) return;
    enteredSettingsRef.current = true;
    loadSettings().catch((error: Error) => setMessage(error.message));
  }, [page]);

  // 每个页面记住自己的滚动位置：回到页面时恢复到离开时的位置，首次进入从顶部开始。
  // 页面数据异步到达，因此在内容足够高之前持续尝试，用户一旦主动滚动就停止。
  const scrollPositions = useRef(new Map<AppPage, number>());
  const scrollPageRef = useRef(page);
  const restoringScrollRef = useRef(false);
  useEffect(() => {
    const content = contentRef.current;
    const record = () => {
      if (restoringScrollRef.current) return;
      const top = content && content.scrollHeight > content.clientHeight ? content.scrollTop : window.scrollY;
      scrollPositions.current.set(scrollPageRef.current, top);
    };
    content?.addEventListener("scroll", record, { passive: true });
    window.addEventListener("scroll", record, { passive: true });
    return () => {
      content?.removeEventListener("scroll", record);
      window.removeEventListener("scroll", record);
    };
  }, [loading]);
  useLayoutEffect(() => {
    scrollPageRef.current = page;
    const content = contentRef.current;
    const target = scrollPositions.current.get(page) ?? 0;
    const apply = () => {
      content?.scrollTo({ top: target });
      window.scrollTo({ top: target });
    };
    restoringScrollRef.current = true;
    apply();
    if (target === 0) {
      restoringScrollRef.current = false;
      return;
    }
    let frame = 0;
    const startedAt = performance.now();
    const stop = () => {
      cancelAnimationFrame(frame);
      restoringScrollRef.current = false;
      for (const type of ["wheel", "touchstart", "keydown", "pointerdown"]) window.removeEventListener(type, stop, true);
    };
    const step = () => {
      apply();
      const current = content && content.scrollHeight > content.clientHeight ? content.scrollTop : window.scrollY;
      if (Math.abs(current - target) < 2 || performance.now() - startedAt > 2000) stop();
      else frame = requestAnimationFrame(step);
    };
    for (const type of ["wheel", "touchstart", "keydown", "pointerdown"]) window.addEventListener(type, stop, { capture: true, passive: true });
    frame = requestAnimationFrame(step);
    return stop;
  }, [page]);

  // 在任意位置按 “/” 聚焦当前页面的主搜索框（正在输入或打开对话框时不拦截）。
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "/" || event.ctrlKey || event.metaKey || event.altKey || event.defaultPrevented) return;
      const target = event.target as HTMLElement | null;
      if (target?.closest("input, textarea, select, [contenteditable=''], [contenteditable='true'], [role='dialog']")) return;
      if (document.querySelector("[role='dialog'][aria-modal='true']")) return;
      const search = Array.from(document.querySelectorAll<HTMLInputElement>("[data-primary-search]"))
        .find((element) => element.offsetParent !== null && !element.disabled);
      if (!search) return;
      event.preventDefault();
      search.focus();
      search.select();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  // 启动后空闲时预取其余页面分包，切换菜单时不再出现加载占位。
  useEffect(() => {
    if (loading) return;
    const pages = (Object.keys(pageLoaders) as AppPage[]).filter((key) => key !== "torrent-transfer" || selfUse);
    let index = 0;
    let handle = 0;
    const idle = typeof window.requestIdleCallback === "function";
    const schedule = (callback: () => void) => idle
      ? window.requestIdleCallback(callback, { timeout: 3000 })
      : window.setTimeout(callback, 600);
    const cancel = (id: number) => idle ? window.cancelIdleCallback(id) : window.clearTimeout(id);
    const step = () => {
      if (index >= pages.length) return;
      prefetchPage(pages[index++]);
      handle = schedule(step);
    };
    handle = schedule(step);
    return () => cancel(handle);
  }, [loading, selfUse]);

  useEffect(() => {
    if (!loading && !selfUse && page === "torrent-transfer") {
      lastVisited.current.delete("torrent-transfer");
      setPage("system-overview");
      window.history.replaceState(null, "", "#/");
    }
  }, [loading, selfUse, page]);

  useEffect(() => {
    if (!menuOpen) return;

    const previouslyFocused = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const focusFrame = requestAnimationFrame(() => {
      (menuPanelRef.current?.querySelector<HTMLElement>('[aria-label="关闭菜单"]') ?? menuPanelRef.current)?.focus();
    });
    const keepFocusInside = (event: FocusEvent) => {
      if (!menuPanelRef.current?.contains(event.target as Node)) {
        (getFocusableElements(menuPanelRef.current)[0] ?? menuPanelRef.current)?.focus();
      }
    };
    const previousBodyOverflow = document.body.style.overflow;
    const previousHtmlOverflow = document.documentElement.style.overflow;
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.defaultPrevented) return;
      if (event.key === "Tab") {
        const items = getFocusableElements(menuPanelRef.current);
        const first = items[0], last = items[items.length - 1];
        if (!first) { event.preventDefault(); menuPanelRef.current?.focus(); }
        else if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last.focus(); }
        else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first.focus(); }
      }
      if (event.key === "Escape") {
        event.preventDefault();
        setMenuOpen(false);
      }
    };
    const closeOnDesktop = () => {
      if (window.innerWidth >= 1024) {
        setMenuOpen(false);
      }
    };

    document.body.style.overflow = "hidden";
    document.documentElement.style.overflow = "hidden";
    document.addEventListener("focusin", keepFocusInside);
    window.addEventListener("keydown", closeOnEscape);
    window.addEventListener("resize", closeOnDesktop);
    closeOnDesktop();

    return () => {
      cancelAnimationFrame(focusFrame);
      document.removeEventListener("focusin", keepFocusInside);
      if (previouslyFocused?.isConnected) previouslyFocused.focus();
      document.body.style.overflow = previousBodyOverflow;
      document.documentElement.style.overflow = previousHtmlOverflow;
      window.removeEventListener("keydown", closeOnEscape);
      window.removeEventListener("resize", closeOnDesktop);
    };
  }, [menuOpen]);

  function navigate(nextPage: AppPage) {
    if (nextPage === "torrent-transfer" && !selfUse) return;
    lastVisited.current.set(readPageFromHash(), window.location.hash);
    setPage(nextPage);
    setHash(nextPage, lastVisited.current.get(nextPage));
    setMenuOpen(false);
    setNavQuery("");
    setMessage("");
  }

  /** 保存失败时抛出错误，由设置页在保存按钮旁就地显示结果。 */
  async function saveSettings() {
    setSaving(true);
    try {
      const saved = await api<GlobalConfig>("/api/settings", {
        method: "PUT",
        body: JSON.stringify(settings),
      });
      setSettings(saved);
      setSavedSettings(saved);
    } finally {
      setSaving(false);
    }
  }
  const settingsDirty = JSON.stringify(settings) !== JSON.stringify(savedSettings);

  // 页面元素只在路由或其依赖变化时重建，菜单搜索、日志弹窗等外壳状态变化不会让整页重新渲染。
  const pageContent = useMemo(() => (
    <>
      {page === "system-overview" ? <SystemOverviewPage /> : null}
      {page === "media" ? <MediaPage /> : null}
      {page === "rss" ? <RssPage /> : null}
      {page === "sites" ? <SitesPage /> : null}
      {page === "invite-profile" ? <InviteProfilePage /> : null}
      {page === "downloaders" ? <DownloadersPage /> : null}
      {page === "torrent-transfer" && selfUse ? <TorrentTransferPage /> : null}
      {page === "brush-tasks" ? <BrushTasksPage /> : null}
      {page === "scheduled-tasks" ? <ScheduledTasksPage /> : null}
      {page === "sign-in" ? <SignInPage /> : null}
      {page === "tag-rules" ? <TagRulesPage /> : null}
      {page === "stats" ? <StatsPage /> : null}
      {page === "system-settings" ? (
        <SystemSettingsPage settings={settings} setSettings={setSettings} saving={saving} dirty={settingsDirty} onSave={saveSettings} />
      ) : null}
    </>
    // saveSettings 每次渲染都是新函数，但它只读取最新的 settings，随 settings 重建即可。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  ), [page, selfUse, settings, saving, settingsDirty]);

  if (loading) {
    return <LoadingState className="min-h-[100dvh]" label="正在启动云母…" />;
  }

  const sidebar = (
    <aside className={cn(
      "kirara-sidebar relative flex h-full min-h-0 w-full flex-col gap-3 overflow-hidden rounded-2xl p-4 lg:rounded-none lg:px-4 lg:py-5",
    )}>

      <div className="relative shrink-0 border-b border-border pb-4">
        <div className="flex items-center gap-3">
          <button
            type="button"
            onClick={() => navigate("system-overview")}
            className="kirara-brand flex items-center gap-3 min-w-0 rounded-lg text-left"
          >
            <div className="flex h-14 w-14 shrink-0 items-center justify-center">
              <span aria-hidden="true" className="kirara-brand-art block h-full w-full rounded-lg" />
            </div>
            <div className="min-w-0">
              <h1 className="truncate text-2xl font-semibold tracking-[0.12em]">云母</h1>
              <p className="mt-0.5 text-xs text-muted">Kirara</p>
            </div>
          </button>
          <div className="ml-auto flex shrink-0 items-center gap-1.5">
            <a
              href="./docs/index.html"
              target="_blank"
              rel="noopener noreferrer"
              className="rounded-lg p-2 text-muted transition hover:bg-accent hover:text-foreground"
              aria-label="使用文档（新标签页打开）"
              title="使用文档（新标签页打开）"
            >
              <BookOpen className="h-4 w-4" aria-hidden="true" />
            </a>
            <a
              href="https://github.com/imythu/kirara"
              target="_blank"
              rel="noopener noreferrer"
              className="rounded-lg p-2 text-muted transition hover:bg-accent hover:text-foreground"
              aria-label="GitHub 源码"
            >
              <GithubIcon className="h-4 w-4" />
            </a>
            <button
              type="button"
              className="rounded-lg p-2 text-muted transition hover:bg-accent hover:text-foreground lg:hidden"
              onClick={() => setMenuOpen(false)}
              aria-label="关闭菜单"
            >
              <X className="h-4 w-4" />
            </button>
          </div>
        </div>
      </div>

      <div className="kirara-menu-search relative shrink-0">
        <Search className="pointer-events-none absolute left-3 top-3.5 h-4 w-4 text-muted" aria-hidden="true" />
        <input type="search" value={navQuery} onChange={(event) => setNavQuery(event.target.value)}
          aria-label="查找菜单" placeholder="查找菜单" className="h-11 w-full rounded-lg border border-border bg-card pl-9 pr-3 text-sm placeholder:text-muted" />
      </div>
      <div className="sidebar-scroll relative min-h-0 flex-1">
        <div className="flex flex-col gap-2 pb-1 pr-1">
          {navGroups.filter((group) => visibleNavItems.some((item) => item.group === group.key)).map((group) => (
            <NavSection key={group.key} title={group.label}
              open={!!navQuery.trim() || !closedGroups.includes(group.key)}
              onToggle={() => setClosedGroups((current) => current.includes(group.key)
                ? current.filter((key) => key !== group.key) : [...current, group.key])}
              items={visibleNavItems.filter((item) => item.group === group.key)}
              searching={!!navQuery.trim()}
              page={page} navigate={navigate} />
          ))}
          {visibleNavItems.length === 0 ? <p className="px-3 py-4 text-sm text-muted" role="status">没有匹配的菜单</p> : null}
        </div>
      </div>
      <div className="kirara-companion shrink-0 overflow-hidden rounded-xl" aria-hidden="true" />
      <div className="flex shrink-0 items-center justify-between border-t border-border pt-3 text-xs text-muted"><span>云母 · 影视与 PT 管理</span><span className="tabular-nums">{APP_VERSION}</span></div>
    </aside>
  );

  return (
    <main inert={menuOpen} className="min-h-[100dvh] bg-background text-foreground">
      {/* Mobile Floating Dock */}
      <div className="mobile-dock fixed left-1/2 z-50 w-[92%] max-w-[440px] -translate-x-1/2 lg:hidden">
        <div className="rounded-2xl border border-border bg-card p-1.5 shadow-lg flex items-center justify-between">
          <DockItem icon={Tv} active={page === "media"} onClick={() => navigate("media")} label="追剧" />
          <DockItem icon={BarChart3} active={page === "stats"} onClick={() => navigate("stats")} label="统计" />
          <DockItem icon={Download} active={page === "brush-tasks"} onClick={() => navigate("brush-tasks")} label="刷流" />
          <DockItem icon={Database} active={page === "sites"} onClick={() => navigate("sites")} label="站点" />
          <button
            type="button"
            onClick={() => setMenuOpen(true)}
            className="flex min-h-14 flex-1 flex-col items-center justify-center gap-1 rounded-xl text-muted hover:bg-accent transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary"
            aria-label="打开全部菜单"
            aria-expanded={menuOpen}
            aria-controls="mobile-navigation"
          >
            <Menu className="h-5 w-5" aria-hidden="true" /><span className="text-xs font-medium">菜单</span>
          </button>
        </div>
      </div>

      <div className="app-shell-grid mx-auto grid lg:grid-cols-[256px_minmax(0,1fr)]">
        <div className="hidden h-full min-h-0 lg:block">
          {sidebar}
        </div>

        {menuOpen ? createPortal(
          <div
            className="mobile-viewport fixed inset-0 z-[60] overflow-hidden bg-black/40 lg:hidden"
            onClick={() => setMenuOpen(false)}
          >
            <div
              ref={menuPanelRef}
              tabIndex={-1}
              id="mobile-navigation"
              className="mobile-menu-frame h-full w-[85vw] max-w-[320px]"
              role="dialog"
              aria-modal="true"
              aria-label="主菜单"
              onClick={(event) => event.stopPropagation()}
            >
              <div className="h-full min-h-0 animate-in slide-in-from-left duration-300">
                {sidebar}
              </div>
            </div>
          </div>, document.body
        ) : null}

        {/* Keep absolute descendants, including sr-only labels, inside this scroll container. */}
        <section ref={contentRef} className="kirara-content relative min-h-0 min-w-0 overflow-y-auto sm:p-6 lg:px-8 lg:py-7 xl:px-10">
          <header className="kirara-page-header relative pb-6">
            <div className="relative flex items-center justify-between gap-3 lg:items-start">
              <div className="flex min-w-0 items-center gap-2 lg:items-start lg:gap-3">
                <Button
                  variant="outline"
                  className="h-9 px-3 lg:hidden"
                  onClick={() => setMenuOpen(true)}
                  aria-label="打开菜单"
                  aria-expanded={menuOpen}
                  aria-controls="mobile-navigation"
                >
                  <Menu className="h-4 w-4" />
                </Button>
                <div className="min-w-0">
                  <h2 className="text-lg font-semibold leading-snug sm:text-2xl">{currentNav.label}</h2>
                  <p className="mt-1 hidden text-sm leading-6 text-muted lg:block">{currentNav.description}</p>
                </div>
              </div>

              <div className="flex shrink-0 items-center justify-end gap-2">
                <div aria-hidden="true" className="kirara-header-art hidden xl:block" />
                <HeaderClock />
                <Button variant="outline" className="h-9 px-3 lg:h-10 lg:px-5" onClick={() => setLogsOpen(true)} aria-label="实时日志">
                  <FileText className="h-4 w-4 lg:mr-2" />
                  <span className="hidden lg:inline">实时日志</span>
                </Button>
              </div>
            </div>
          </header>

          {message ? <Notice className="mt-4" onDismiss={() => setMessage("")}>{message}</Notice> : null}

          <Suspense fallback={<LoadingState className="mt-4 rounded-2xl border border-border bg-card" label="页面加载中…" />}>
            <div className="mt-4">{pageContent}</div>
          </Suspense>
        </section>
      </div>

      <LogsDialog open={logsOpen} onClose={() => setLogsOpen(false)} effectiveLogLevel={effectiveLogLevel} />
    </main>
  );
}

/** 头部时钟只显示到分钟，独立组件按分钟对齐刷新，避免每秒重渲染整个应用。 */
function HeaderClock() {
  const [now, setNow] = useState(() => new Date());
  useEffect(() => {
    let timer = 0;
    const tick = () => {
      const current = new Date();
      setNow(current);
      timer = window.setTimeout(tick, 60_000 - (current.getSeconds() * 1000 + current.getMilliseconds()) + 50);
    };
    timer = window.setTimeout(tick, 60_000 - (now.getSeconds() * 1000 + now.getMilliseconds()) + 50);
    return () => window.clearTimeout(timer);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  return (
    <div className="hidden px-3 py-2 text-xs text-muted xl:block">
      {now.toLocaleString("zh-CN", { month: "long", day: "numeric", weekday: "short", hour: "2-digit", minute: "2-digit", hour12: false })}
    </div>
  );
}

/** 距离底部小于该像素时视为“跟随最新”，新日志到达时自动滚到底部。 */
const LOG_STICK_THRESHOLD_PX = 48;

function LogsDialog({ open, onClose, effectiveLogLevel }: { open: boolean; onClose: () => void; effectiveLogLevel: LogLevel }) {
  const [logs, setLogs] = useState<string[]>([]);
  const [logsConnected, setLogsConnected] = useState(false);
  const [logLevelFilter, setLogLevelFilter] = useState<LogLevel>(effectiveLogLevel);
  const [logKeywordFilter, setLogKeywordFilter] = useState("");
  const [following, setFollowing] = useState(true);
  const logsViewportRef = useRef<HTMLDivElement | null>(null);
  const pendingLogsRef = useRef<string[]>([]);
  const followingRef = useRef(true);
  const selectableLogLevels = LOG_LEVELS.filter(
    (level) => LOG_LEVEL_PRIORITY[level] >= LOG_LEVEL_PRIORITY[effectiveLogLevel],
  );

  useEffect(() => {
    if (open) {
      setLogLevelFilter(effectiveLogLevel);
      followingRef.current = true;
      setFollowing(true);
    }
  }, [open, effectiveLogLevel]);

  useEffect(() => {
    if (!open) return;

    let closed = false;
    setLogs([]);
    pendingLogsRef.current = [];

    const flushLogs = () => {
      if (pendingLogsRef.current.length === 0) return;
      const pending = pendingLogsRef.current;
      pendingLogsRef.current = [];
      setLogs((prev) => {
        const next = prev.concat(pending);
        return next.length > MAX_LOG_LINES ? next.slice(next.length - MAX_LOG_LINES) : next;
      });
    };

    const flushTimer = window.setInterval(flushLogs, LOG_FLUSH_INTERVAL_MS);
    const source = subscribeLogs({
      onOpen: () => {
        if (!closed) setLogsConnected(true);
      },
      onLog: (data) => {
        if (closed) return;
        try {
          const payload = JSON.parse(data) as { encoded_line?: string };
          if (typeof payload.encoded_line === "string") {
            pendingLogsRef.current.push(decodeURIComponent(payload.encoded_line));
          }
        } catch {
          pendingLogsRef.current.push(data);
        }
      },
      onError: () => {
        if (!closed) setLogsConnected(false);
      },
    });

    return () => {
      closed = true;
      setLogsConnected(false);
      window.clearInterval(flushTimer);
      pendingLogsRef.current = [];
      source.close();
    };
  }, [open]);

  const keyword = logKeywordFilter.trim().toLowerCase();
  const filteredLogs = useMemo(() => logs.filter((line) => {
    const lineLevel = extractLogLevel(line);
    if (lineLevel && LOG_LEVEL_PRIORITY[lineLevel] < LOG_LEVEL_PRIORITY[logLevelFilter]) return false;
    return keyword === "" || line.toLowerCase().includes(keyword);
  }), [logs, logLevelFilter, keyword]);

  // 只有停留在底部时才跟随新日志；用户向上翻看历史时不强制拉回。
  useEffect(() => {
    if (!open || !followingRef.current) return;
    const viewport = logsViewportRef.current;
    if (viewport) viewport.scrollTop = viewport.scrollHeight;
  }, [filteredLogs, open]);

  function jumpToLatest() {
    followingRef.current = true;
    setFollowing(true);
    const viewport = logsViewportRef.current;
    if (viewport) viewport.scrollTop = viewport.scrollHeight;
  }

  return (
    <Dialog open={open} onClose={onClose} title="实时日志" description="查看后端程序的最近日志和实时输出。">
      <div className="space-y-4 p-4 sm:p-6">
        <div className="flex items-center justify-between gap-3">
          <span
            className={cn(
              "rounded-full px-3 py-1 text-xs font-medium",
              logsConnected ? "bg-emerald-100 text-emerald-700" : "bg-amber-100 text-amber-700",
            )}
          >
            {logsConnected ? "已连接" : "连接中"}
          </span>
          <div className="flex items-center gap-2">
            <span className="text-xs text-muted">最多保留 {MAX_LOG_LINES} 行</span>
            <Button
              variant="outline"
              onClick={() => {
                pendingLogsRef.current = [];
                setLogs([]);
              }}
            >
              清空视图
            </Button>
          </div>
        </div>
        <div className="grid gap-3 sm:grid-cols-[220px_minmax(0,1fr)]">
          <div className="space-y-2">
            <Select
              aria-label="日志级别筛选"
              value={logLevelFilter}
              onChange={(val) => setLogLevelFilter(val as LogLevel)}
              options={selectableLogLevels.map((level) => ({
                value: level,
                label: level.toUpperCase(),
              }))}
            />
            <p className="text-xs leading-5 text-muted">
              当前系统日志级别：{effectiveLogLevel.toUpperCase()}。更低级别的日志已被后端过滤，筛选项只显示该级别及以上。
            </p>
          </div>
          <Input
            type="search"
            aria-label="按关键词筛选日志"
            value={logKeywordFilter}
            onChange={(event) => setLogKeywordFilter(event.target.value)}
            placeholder="按关键词筛选日志"
          />
        </div>
        <div className="relative">
          <div
            ref={logsViewportRef}
            role="log"
            aria-label="实时日志输出"
            aria-live="off"
            tabIndex={0}
            onScroll={(event) => {
              const viewport = event.currentTarget;
              const atBottom = viewport.scrollHeight - viewport.scrollTop - viewport.clientHeight <= LOG_STICK_THRESHOLD_PX;
              if (atBottom !== followingRef.current) {
                followingRef.current = atBottom;
                setFollowing(atBottom);
              }
            }}
            className="h-[60vh] overflow-auto rounded-2xl border border-border bg-slate-950 p-4 font-mono text-xs leading-6 text-slate-100"
          >
            {filteredLogs.length === 0 ? (
              <div className="text-slate-400">{logs.length === 0 ? "暂无日志输出。" : "没有匹配当前筛选条件的日志。"}</div>
            ) : (
              filteredLogs.map((line, index) => (
                <div key={`${index}-${line.slice(0, 24)}`} className="whitespace-pre-wrap break-all">
                  {line}
                </div>
              ))
            )}
          </div>
          {!following && filteredLogs.length > 0 ? (
            <Button size="sm" className="absolute bottom-3 right-3 shadow-lg" onClick={jumpToLatest}>
              <ChevronDown aria-hidden="true" />
              跳到最新
            </Button>
          ) : null}
        </div>
      </div>
    </Dialog>
  );
}

function NavSection({ title, open, onToggle, items, page, navigate, searching }: {
  title: string; open: boolean; searching: boolean; onToggle: () => void;
  items: typeof navItems; page: AppPage; navigate: (page: AppPage) => void;
}) {
  const id = useId();
  const current = items.find((item) => item.key === page);
  return (
    <div>
      {searching ? <p className="flex min-h-11 items-center px-3 text-xs font-medium text-muted">{title}</p> : <button type="button" onClick={onToggle} aria-expanded={open} aria-controls={id}
        className="kirara-nav-group flex min-h-11 w-full items-center justify-between gap-2 rounded-lg px-3 text-left text-xs font-medium text-muted hover:bg-accent focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary">
        <span>{title}{!open && current ? <span className="mt-1 block text-primary">当前：{current.label}</span> : null}</span>
        <ChevronDown aria-hidden="true" className={cn("h-4 w-4 shrink-0 transition-transform", open && "rotate-180")} />
      </button>}
      <nav id={id} aria-label={title} hidden={!open} className="space-y-1">
        {items.map((item) => {
          const Icon = item.icon;
          const active = item.key === page;
          return <button key={item.key} type="button" onClick={() => navigate(item.key)}
            onPointerEnter={() => prefetchPage(item.key)} onFocus={() => prefetchPage(item.key)}
            aria-current={active ? "page" : undefined} title={item.description}
            className={cn("kirara-nav-item flex min-h-11 w-full items-center gap-3 rounded-lg px-3 py-2 text-left text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary focus-visible:ring-offset-2")}>
            <Icon aria-hidden="true" className="h-5 w-5 shrink-0" /><span>{item.label}</span>
          </button>;
        })}
      </nav>
    </div>
  );
}

function DockItem({ 
  icon: Icon, 
  active, 
  onClick, 
  label 
}: { 
  icon: typeof LayoutDashboard; 
  active: boolean; 
  onClick: () => void; 
  label: string 
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-label={label}
      aria-current={active ? "page" : undefined}
      className={cn(
        "flex flex-1 flex-col items-center justify-center min-h-14 gap-1 rounded-xl transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary",
        active ? "bg-primary text-primary-foreground" : "text-muted hover:bg-accent/50"
      )}
    >
      <Icon className="h-5 w-5" aria-hidden="true" />
      <span className="text-xs font-medium">{label}</span>
    </button>
  );
}

function GithubIcon(props: SVGProps<SVGSVGElement>) {
  return (
    <svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true" {...props}>
      <path d="M12 0C5.37 0 0 5.37 0 12c0 5.303 3.438 9.8 8.205 11.387.6.113.82-.258.82-.577 0-.285-.01-1.04-.015-2.04-3.338.724-4.042-1.61-4.042-1.61-.546-1.387-1.333-1.756-1.333-1.756-1.089-.745.083-.729.083-.729 1.205.084 1.84 1.237 1.84 1.237 1.07 1.834 2.807 1.304 3.492.997.108-.775.418-1.305.762-1.605-2.665-.3-5.466-1.332-5.466-5.93 0-1.31.468-2.381 1.236-3.221-.124-.303-.535-1.524.117-3.176 0 0 1.008-.322 3.3 1.23a11.52 11.52 0 0 1 3.003-.404c1.02.005 2.047.138 3.003.404 2.29-1.552 3.297-1.23 3.297-1.23.653 1.652.242 2.873.118 3.176.77.84 1.235 1.911 1.235 3.221 0 4.61-2.807 5.625-5.479 5.921.43.372.823 1.102.823 2.222 0 1.606-.015 2.898-.015 3.293 0 .322.216.694.825.576C20.565 21.796 24 17.3 24 12c0-6.63-5.37-12-12-12z" />
    </svg>
  );
}
