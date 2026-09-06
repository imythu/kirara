import { useHashChoice } from "@/lib/navigation-state";
import { useEffect, useMemo, useState, type KeyboardEvent } from "react";
import {
  AlertTriangle,
  CalendarCheck,
  Cloud,
  ClipboardList,
  Edit,
  FlaskConical,
  History,
  Loader2,
  Pause,
  Play,
  Plus,
  RefreshCw,
  RotateCcw,
  Save,
  Search,
  Settings2,
  Sparkles,
  Trash2,
  X,
  Zap,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { SearchFeedback, SearchPagination } from "@/components/search-controls";
import { useServerSearch } from "@/lib/server-search";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Dialog } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select } from "@/components/ui/select";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { api } from "@/lib/api";
import { formatDate, statusBadge } from "@/lib/format";
import { cn } from "@/lib/utils";
import type {
  BrowserProbeResult,
  GlobalConfig,
  SignInRecord,
  SignInResultRule,
  SignInTaskRecord,
  SignInTaskRequest,
  SiteRecord,
} from "@/types";

const SIGN_IN_INTERVAL_HOURS = [6, 8, 12, 16, 20, 24] as const;
type SignInIntervalHours = (typeof SIGN_IN_INTERVAL_HOURS)[number];
const SIGN_IN_VIEWS = ["tasks", "records"] as const;
type SignInView = "tasks" | "records";
type SignInBrowser = "lightpanda" | "browserless";

const DEFAULT_BROWSERLESS_ADDRESS = "https://production-sfo.browserless.io";
function defaultLightpandaEndpoint(region: string) {
  return `wss://${region === "uswest" ? "uswest" : "euwest"}.cloud.lightpanda.io/ws`;
}

const DEFAULT_BROWSERLESS_TASK = {
  submit_method: "click" as const,
  result_rules: [] as SignInResultRule[],
  selector: "",
  attendance_path: "/attendance.php",
  captcha_selector: "",
  captcha_input_selector: "",
  already_keywords: "",
  cf_mode: "auto" as const,
  wait_ms: null,
  solve_timeout: null,
  action_timeout: null,
  post_click_wait_ms: null,
};

type SiteSigner = { site_id: number; profile: {
  id: string; label: string; browser: SignInBrowser; sign_in_method: string;
  browserless: NonNullable<SignInTaskRequest["browserless"]>;
} };

const emptyForm: SignInTaskRequest = {
  name: "",
  site_id: 0,
  cron_expression: intervalToCron(8),
  browser: "lightpanda",
  sign_in_method: "open_page",
  browserless: { ...DEFAULT_BROWSERLESS_TASK },
};

function taskToForm(task: SignInTaskRecord): SignInTaskRequest {
  return {
    name: task.name,
    site_id: task.site_id,
    cron_expression: task.cron_expression,
    browser: task.browser === "browserless" ? "browserless" : "lightpanda",
    sign_in_method: task.sign_in_method,
    browserless: { ...DEFAULT_BROWSERLESS_TASK, ...task.browserless },
  };
}

function isNexusSite(site: SiteRecord) {
  const siteType = site.site_type.trim().toLowerCase();
  return siteType === "nexusphp" || siteType === "nexus_php";
}

function displayStatus(status: string | null | undefined) {
  if (!status) return "-";
  if (status === "success") return "成功";
  if (status === "already") return "已签到";
  if (status === "failed") return "失败";
  return status;
}

function signInMethodLabel(method: string | null | undefined) {
  if (method === "cloudflare") return "CF 签到";
  if (method === "ocr_captcha") return "图片验证码签到";
  return "打开页面签到";
}

function browserLabel(browser: string | null | undefined) {
  return browser === "browserless" ? "Browserless" : "Lightpanda";
}

function browserlessCfModeLabel(mode: string | null | undefined) {
  if (mode === "page") return "页面挑战";
  if (mode === "turnstile") return "Turnstile";
  return "自动";
}

function isBrowserConfigured(settings: GlobalConfig | null, browser: SignInBrowser) {
  if (!settings) return false;
  if (browser === "lightpanda") {
    return Boolean(settings.lightpanda.endpoint?.trim() || settings.lightpanda.token?.trim());
  }
  return Boolean(settings.browserless.address?.trim() && settings.browserless.token?.trim());
}

function missingBrowserMessage(settings: GlobalConfig | null, browser: SignInBrowser) {
  if (browser === "lightpanda") return "尚未配置 Lightpanda 云浏览器，请填写 API Key 或自定义连接地址。";
  const missing = [!settings?.browserless.address?.trim() && "服务地址", !settings?.browserless.token?.trim() && "Token"].filter(Boolean).join("和");
  return `尚未配置 Browserless 云浏览器的${missing}，请先完成配置。`;
}

function intervalToCron(hours: SignInIntervalHours) {
  return `0 0 0/${hours} * * *`;
}

function cronToInterval(cron: string): SignInIntervalHours {
  const fields = cron.trim().split(/\s+/);
  const hourField = fields.length === 6 ? fields[2] : fields.length === 5 ? fields[1] : "";
  const match = hourField.match(/^0\/(\d+)$/);
  const hours = match ? Number(match[1]) : 8;
  return SIGN_IN_INTERVAL_HOURS.includes(hours as SignInIntervalHours) ? (hours as SignInIntervalHours) : 8;
}

export function SignInPage() {
  const [tasks, setTasks] = useState<SignInTaskRecord[]>([]);
  const [profiles, setProfiles] = useState<SiteSigner[]>([]);
  const [sites, setSites] = useState<SiteRecord[]>([]);
  const [searchComposing, setSearchComposing] = useState(false);
  const [settings, setSettings] = useState<GlobalConfig | null>(null);
  const [settingsDraft, setSettingsDraft] = useState<GlobalConfig | null>(null);
  const [loading, setLoading] = useState(true);
  const [message, setMessage] = useState("");
  const [activeView, setActiveView] = useHashChoice<SignInView>("view", SIGN_IN_VIEWS, "tasks");
  const [searchTerm, setSearchTerm] = useState("");
  const [siteFilter, setSiteFilter] = useState(0);
  const [searchPollUntil, setSearchPollUntil] = useState(0);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [settingsBrowser, setSettingsBrowser] = useState<SignInBrowser>("lightpanda");
  const [configFeedback, setConfigFeedback] = useState<{ tone: "success" | "error"; text: string } | null>(null);
  const [savingBrowser, setSavingBrowser] = useState(false);
  const [probingBrowser, setProbingBrowser] = useState(false);
  const [formOpen, setFormOpen] = useState(false);
  const [editingId, setEditingId] = useState<number | null>(null);
  const [form, setForm] = useState<SignInTaskRequest>({ ...emptyForm });
  const [intervalHours, setIntervalHours] = useState<SignInIntervalHours>(8);
  const [submitError, setSubmitError] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [deleteTarget, setDeleteTarget] = useState<SignInTaskRecord | null>(null);
  const [deleting, setDeleting] = useState(false);

  const nexusSites = useMemo(() => sites.filter(isNexusSite), [sites]);
  const siteNameById = useMemo(
    () => new Map(sites.map((site) => [site.id, `${site.name} · #${site.id} (${site.site_type})`])),
    [sites],
  );
  const taskNameById = useMemo(
    () => new Map(tasks.map((task) => [task.id, task.name])),
    [tasks],
  );
  const searchOptions = {
    query: searchTerm,
    composing: searchComposing,
    refreshKey: tasks,
    pollUntil: searchPollUntil,
    filters: { site_id: siteFilter || undefined },
  };
  const taskSearch = useServerSearch<SignInTaskRecord>("/api/sign-in-tasks/search", {
    ...searchOptions, enabled: !loading && activeView === "tasks",
  });
  const recordSearch = useServerSearch<SignInRecord>("/api/sign-in-records/search", {
    ...searchOptions, enabled: !loading && activeView === "records",
  });
  const activeSearch = activeView === "tasks" ? taskSearch : recordSearch;
  const filteredTasks = useMemo(() => taskSearch.records.map((task) => {
    const profile = profiles.find((item) => item.site_id === task.site_id)?.profile;
    return profile ? { ...task, browser: profile.browser, sign_in_method: profile.sign_in_method, browserless: profile.browserless } : task;
  }), [taskSearch.records, profiles]);
  const filteredRecords = recordSearch.records;
  const suggestedTaskName = editingId === null
    ? nexusSites.find((site) => site.id === form.site_id)?.name.trim() ?? ""
    : "";
  const selectedProfile = profiles.find(item => item.site_id === form.site_id)?.profile;
  const imageCaptcha = form.sign_in_method === "ocr_captcha";
  const browserlessTask = { ...DEFAULT_BROWSERLESS_TASK, ...form.browserless };
  const signerChoice = imageCaptcha ? "captcha" : form.browser === "browserless" || form.sign_in_method === "cloudflare"
    ? browserlessTask.cf_mode === "turnstile" ? "cf_turnstile" : "cf_challenge" : "nexus";

  function siteSignInPreset(site: SiteRecord | undefined) {
    return profiles.find(item => item.site_id === site?.id)?.profile;
  }

  function chooseSigner(value: string) {
    setForm(current => ({ ...current,
      browser: value === "nexus" ? "lightpanda" : "browserless",
      sign_in_method: value === "nexus" ? "open_page" : value === "captcha" ? "ocr_captcha" : "cloudflare",
      browserless: { ...DEFAULT_BROWSERLESS_TASK,
        selector: value === "captcha" ? "" : "input[type='submit']",
        cf_mode: value === "cf_turnstile" ? "turnstile" : "page",
      },
    }));
  }

  function loadData() {
    setLoading(true);
    Promise.all([
      api<SignInTaskRecord[]>("/api/sign-in-tasks"),
      api<SiteRecord[]>("/api/sites"),
      api<GlobalConfig>("/api/settings"),
      api<SiteSigner[]>("/api/sign-in-profiles"),
    ])
      .then(([nextTasks, nextSites, nextSettings, nextProfiles]) => {
        setProfiles(nextProfiles);
        setTasks(nextTasks.map(task => {
          const profile = nextProfiles.find(item => item.site_id === task.site_id)?.profile;
          return profile ? { ...task, browser: profile.browser, sign_in_method: profile.sign_in_method, browserless: profile.browserless } : task;
        }));
        setSites(nextSites);
        setSettings(nextSettings);
      })
      .catch((error: Error) => setMessage(error.message || "加载自动签到数据失败"))
      .finally(() => setLoading(false));
  }

  useEffect(() => {
    loadData();
  }, []);

  function setField<K extends keyof SignInTaskRequest>(key: K, value: SignInTaskRequest[K]) {
    setForm(current => ({ ...current, [key]: value }));
  }

  function selectSite(siteId: number) {
    const preset = siteSignInPreset(nexusSites.find(site => site.id === siteId));
    setForm(current => ({
      ...current,
      site_id: siteId,
      name: !current.name.trim() || current.name === nexusSites.find(site => site.id === current.site_id)?.name ? nexusSites.find(site => site.id === siteId)?.name ?? "" : current.name,
      browser: preset?.browser ?? "lightpanda",
      sign_in_method: preset?.sign_in_method ?? "open_page",
      browserless: { ...(preset?.browserless ?? DEFAULT_BROWSERLESS_TASK) },
    }));
    setSubmitError("");
  }

  function setLightpandaField<K extends keyof GlobalConfig["lightpanda"]>(
    key: K,
    value: GlobalConfig["lightpanda"][K],
  ) {
    setSettingsDraft((current) =>
      current ? { ...current, lightpanda: { ...current.lightpanda, [key]: value } } : current,
    );
    setConfigFeedback(null);
  }

  function setBrowserlessSettingField<K extends keyof GlobalConfig["browserless"]>(
    key: K,
    value: GlobalConfig["browserless"][K],
  ) {
    setSettingsDraft((current) =>
      current ? { ...current, browserless: { ...current.browserless, [key]: value } } : current,
    );
    setConfigFeedback(null);
  }

  function setBrowserlessTaskField<K extends keyof NonNullable<SignInTaskRequest["browserless"]>>(
    key: K,
    value: NonNullable<SignInTaskRequest["browserless"]>[K],
  ) {
    setForm((current) => ({
      ...current,
      browserless: {
        ...(current.browserless ?? DEFAULT_BROWSERLESS_TASK),
        [key]: value,
      },
    }));
  }

  function openSettings() {
    if (!settings) return;
    setSettingsDraft({
      ...settings,
      lightpanda: { ...settings.lightpanda },
      browserless: { ...settings.browserless, address: settings.browserless.address?.trim() || DEFAULT_BROWSERLESS_ADDRESS },
    });
    setConfigFeedback(null);
    setSettingsOpen(true);
  }

  function closeSettings() {
    if (savingBrowser) return;
    setSettingsOpen(false);
    setSettingsDraft(null);
    setConfigFeedback(null);
  }

  function handleViewKeyDown(event: KeyboardEvent<HTMLButtonElement>) {
    let nextView: SignInView | null = null;
    if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
      nextView = activeView === "tasks" ? "records" : "tasks";
    } else if (event.key === "Home") {
      nextView = "tasks";
    } else if (event.key === "End") {
      nextView = "records";
    }
    if (!nextView) return;
    event.preventDefault();
    setActiveView(nextView);
    requestAnimationFrame(() => document.getElementById(`sign-in-${nextView}-tab`)?.focus());
  }

  function openAdd() {
    const preset = siteSignInPreset(nexusSites[0]);
    setEditingId(null);
    setForm({
      ...emptyForm,
      site_id: nexusSites[0]?.id ?? 0,
      name: nexusSites[0]?.name ?? "",
      browser: preset?.browser ?? "lightpanda",
      sign_in_method: preset?.sign_in_method ?? "open_page",
      browserless: { ...(preset?.browserless ?? DEFAULT_BROWSERLESS_TASK) },
    });
    setIntervalHours(8);
    setSubmitError("");
    setFormOpen(true);
  }

  function openEdit(task: SignInTaskRecord) {
    setEditingId(task.id);
    const next = taskToForm(task);
    const profile = siteSignInPreset(nexusSites.find(site => site.id === task.site_id));
    if (profile) Object.assign(next, { browser: profile.browser, sign_in_method: profile.sign_in_method, browserless: profile.browserless });
    else if (next.sign_in_method === "cloudflare" || next.sign_in_method === "ocr_captcha") next.browser = "browserless";
    setForm(next);
    setIntervalHours(cronToInterval(task.cron_expression));
    setSubmitError("");
    setFormOpen(true);
  }

  function closeForm() {
    setFormOpen(false);
    setEditingId(null);
    setSubmitError("");
  }

  async function persistBrowserSettings(probe: boolean) {
    if (!settingsDraft) return;
    if (
      probe
      && settingsBrowser === "lightpanda"
      && !settingsDraft.lightpanda.endpoint?.trim()
      && !settingsDraft.lightpanda.token?.trim()
    ) {
      setConfigFeedback({ tone: "error", text: "Lightpanda endpoint 或 token 至少填写一个" });
      return;
    }
    if (
      probe
      && settingsBrowser === "browserless"
      && (!settingsDraft.browserless.address?.trim() || !settingsDraft.browserless.token?.trim())
    ) {
      setConfigFeedback({ tone: "error", text: "Browserless 地址和 Token 都不能为空" });
      return;
    }

    setSavingBrowser(true);
    setProbingBrowser(probe);
    setConfigFeedback(null);
    try {
      const saved = await api<GlobalConfig>("/api/settings", {
        method: "PUT",
        body: JSON.stringify(settingsDraft),
      });
      setSettings(saved);
      setSettingsDraft({
        ...saved,
        lightpanda: { ...saved.lightpanda },
        browserless: { ...saved.browserless },
      });
      if (!probe) {
        setConfigFeedback({ tone: "success", text: `${browserLabel(settingsBrowser)} 公共配置已保存` });
        return;
      }

      const result = await api<BrowserProbeResult>("/api/sign-in-probe-1-1-1-1", {
        method: "POST",
        body: JSON.stringify({ browser: settingsBrowser }),
      });
      if (!result.success) throw new Error(result.message);
      setConfigFeedback({
        tone: "success",
        text: `测试成功：已打开 ${result.url}${result.title ? `，标题：${result.title}` : ""}`,
      });
    } catch (error) {
      setConfigFeedback({
        tone: "error",
        text: (error as Error).message || `${browserLabel(settingsBrowser)} 配置保存失败`,
      });
    } finally {
      setSavingBrowser(false);
      setProbingBrowser(false);
    }
  }

  async function handleSubmit() {
    if (!form.name.trim()) {
      setSubmitError("名称不能为空");
      return;
    }
    if (!form.site_id) {
      setSubmitError("请选择 NexusPHP 站点");
      return;
    }
    if (!isBrowserConfigured(settings, form.browser ?? "lightpanda")) {
      setSubmitError(missingBrowserMessage(settings, form.browser ?? "lightpanda"));
      return;
    }
    if (form.browser === "browserless" && !form.browserless?.selector.trim()) {
      setSubmitError("请填写本站签到按钮的 Selector，再创建或保存任务。");
      return;
    }

    if (imageCaptcha && (!browserlessTask.captcha_selector.trim() || !browserlessTask.captcha_input_selector.trim() || !browserlessTask.attendance_path.trim())) {
      setSubmitError("请填写签到路径、验证码图片和输入框的 Selector。");
      return;
    }
    const body: SignInTaskRequest = {
      ...form,
      name: form.name.trim(),
      cron_expression: intervalToCron(intervalHours),
      browser: form.browser ?? "lightpanda",
      sign_in_method: form.sign_in_method ?? "open_page",
      browserless: form.browser !== "browserless" && !form.browserless?.selector.trim() ? null : {
        ...(form.browserless ?? DEFAULT_BROWSERLESS_TASK),
        selector: (form.browserless?.selector ?? DEFAULT_BROWSERLESS_TASK.selector).trim(),
      },
    };

    setSubmitting(true);
    setSubmitError("");
    try {
      if (editingId !== null) {
        await api(`/api/sign-in-tasks/${editingId}`, { method: "PUT", body: JSON.stringify(body) });
      } else {
        await api("/api/sign-in-tasks", { method: "POST", body: JSON.stringify(body) });
        setActiveView("tasks");
      }
      closeForm();
      setMessage(editingId !== null ? "自动签到任务已更新" : "自动签到任务已创建");
      loadData();
    } catch (error) {
      setSubmitError((error as Error).message || "保存自动签到任务失败");
    } finally {
      setSubmitting(false);
    }
  }

  async function runAction(action: Promise<unknown>, success: string, background = false) {
    try {
      await action;
      setMessage(success);
      if (background) setSearchPollUntil(Date.now() + 120_000);
      loadData();
    } catch (error) {
      setMessage((error as Error).message || "操作失败");
    }
  }

  async function confirmDelete() {
    if (!deleteTarget) return;
    setDeleting(true);
    try {
      await api(`/api/sign-in-tasks/${deleteTarget.id}`, { method: "DELETE" });
      setDeleteTarget(null);
      setMessage("自动签到任务已删除");
      loadData();
    } catch (error) {
      setMessage((error as Error).message || "删除自动签到任务失败");
    } finally {
      setDeleting(false);
    }
  }

  return (
    <div className="space-y-6">
      <Card>
        <CardHeader>
          {message ? (
            <div className="rounded-xl border border-border bg-surface-container/70 px-4 py-3 text-sm">
              <div className="flex items-start justify-between gap-3">
                <span>{message}</span>
                <button
                  type="button"
                  className="cursor-pointer text-muted transition-colors hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/40"
                  title="关闭消息"
                  aria-label="关闭消息"
                  onClick={() => setMessage("")}
                >
                  <X className="h-4 w-4" />
                </button>
              </div>
            </div>
          ) : null}
          <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
            <div>
              <CardTitle className="flex items-center gap-2">
                <CalendarCheck className="h-5 w-5" />
                自动签到
              </CardTitle>
              <CardDescription>管理 NexusPHP 站点签到任务、调度状态和最近执行结果。</CardDescription>
            </div>
            <div className="flex flex-wrap gap-2">
              <Button variant="outline" disabled={!settings || loading} onClick={openSettings}>
                <Settings2 className="mr-2 h-4 w-4" />
                浏览器配置
              </Button>
              <Button variant="outline" onClick={loadData}>
                <RefreshCw className="mr-2 h-4 w-4" />
                刷新
              </Button>
              <Button onClick={openAdd}>
                <Plus className="mr-2 h-4 w-4" />
                添加任务
              </Button>
            </div>
          </div>
        </CardHeader>
        <CardContent className="space-y-4">
          <div
            className="grid grid-cols-2 gap-2 rounded-xl bg-surface-container/70 p-1.5"
            role="tablist"
            aria-label="自动签到内容"
          >
            <button
              id="sign-in-tasks-tab"
              type="button"
              role="tab"
              aria-selected={activeView === "tasks"}
              aria-controls="sign-in-tasks-panel"
              tabIndex={activeView === "tasks" ? 0 : -1}
              className={cn(
                "flex min-h-10 cursor-pointer items-center justify-center gap-2 rounded-2xl px-3 text-sm font-semibold transition-colors duration-200 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/40",
                activeView === "tasks" ? "bg-card text-foreground shadow-sm" : "text-muted hover:bg-accent hover:text-foreground",
              )}
              onClick={() => setActiveView("tasks")}
              onKeyDown={handleViewKeyDown}
            >
              <ClipboardList className="h-4 w-4 shrink-0" aria-hidden="true" />
              <span>签到任务</span>
              <span className="rounded-full bg-secondary px-2 py-0.5 text-[11px] text-secondary-foreground">{tasks.length}</span>
            </button>
            <button
              id="sign-in-records-tab"
              type="button"
              role="tab"
              aria-selected={activeView === "records"}
              aria-controls="sign-in-records-panel"
              tabIndex={activeView === "records" ? 0 : -1}
              className={cn(
                "flex min-h-10 cursor-pointer items-center justify-center gap-2 rounded-2xl px-3 text-sm font-semibold transition-colors duration-200 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/40",
                activeView === "records" ? "bg-card text-foreground shadow-sm" : "text-muted hover:bg-accent hover:text-foreground",
              )}
              onClick={() => setActiveView("records")}
              onKeyDown={handleViewKeyDown}
            >
              <History className="h-4 w-4 shrink-0" aria-hidden="true" />
              <span>执行日志</span>

            </button>
          </div>

          <div className="grid gap-3 border-b border-border pb-4 md:grid-cols-[minmax(0,1fr)_minmax(240px,0.55fr)]">
            <div className="relative min-w-0">
              <Label htmlFor="sign-in-search" className="sr-only">快速搜索</Label>
              <Search className="pointer-events-none absolute left-3.5 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" aria-hidden="true" />
              <Input
                id="sign-in-search"
                type="search"
                autoComplete="off"
                className="pl-10"
                value={searchTerm}
                placeholder={activeView === "tasks" ? "搜索任务、站点或最近消息" : "搜索任务、站点或日志消息"}
                onChange={(event) => setSearchTerm(event.target.value)}
                onCompositionStart={() => setSearchComposing(true)}
                onCompositionEnd={(event) => { setSearchTerm(event.currentTarget.value); setSearchComposing(false); }}
              />
            </div>
            <div className="flex min-w-0 gap-2">
              <div className="min-w-0 flex-1">
                <Label htmlFor="sign-in-site-filter" className="sr-only">按站点筛选</Label>
                <Select
                  id="sign-in-site-filter"
                  value={String(siteFilter)}
                  onChange={(value) => setSiteFilter(Number(value))}
                  options={[
                    { value: "0", label: "全部站点" },
                    ...nexusSites.map((site) => ({ value: String(site.id), label: `${site.name} · #${site.id}` })),
                  ]}
                />
              </div>
              <Button
                variant="outline"
                className="h-11 w-11 shrink-0 px-0"
                disabled={!searchTerm && siteFilter === 0}
                title="重置筛选"
                aria-label="重置筛选"
                onClick={() => {
                  setSearchTerm("");
                  setSiteFilter(0);
                }}
              >
                <RotateCcw className="h-4 w-4" aria-hidden="true" />
              </Button>
            </div>
          </div>

          {!loading ? <SearchFeedback search={activeSearch} onClearQuery={() => { setSearchTerm(""); setSiteFilter(0); }} /> : null}
          {loading ? (
            <div className="flex items-center justify-center py-12 text-muted-foreground">
              <Loader2 className="mr-2 h-5 w-5 animate-spin" />
              加载中...
            </div>
          ) : activeSearch.loading || searchComposing || activeSearch.error ? null : activeView === "tasks" ? (
            <div
              id="sign-in-tasks-panel"
              role="tabpanel"
              aria-labelledby="sign-in-tasks-tab"
              tabIndex={0}
              className="focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/30"
            >
              <p className="mb-3 text-xs text-muted-foreground" aria-live="polite">
                匹配 {taskSearch.total} / {tasks.length} 个任务
              </p>
              {tasks.length === 0 ? (
                <div className="py-12 text-center text-sm text-muted-foreground">暂无自动签到任务，点击上方按钮添加。</div>
              ) : filteredTasks.length === 0 ? (
                <div className="py-12 text-center text-sm text-muted-foreground">没有匹配的任务，请更换关键词或站点筛选。</div>
              ) : (
                <div className="grid gap-3">
                  {filteredTasks.map((task) => (
                    <div key={task.id} className="rounded-xl border border-border bg-surface-container/70 p-3.5 shadow-sm">
                      <div className="flex flex-col items-stretch gap-3 sm:flex-row sm:flex-wrap sm:items-start sm:justify-between">
                        <div className="min-w-0 flex-1">
                          <div className="flex flex-wrap items-center gap-2">
                            <span className="min-w-0 break-words text-sm font-semibold sm:truncate">{task.name}</span>
                            <span className={cn("shrink-0 rounded-full px-2.5 py-0.5 text-[11px] font-medium", task.enabled ? "bg-emerald-100 text-emerald-700" : "bg-amber-100 text-amber-700")}>
                              {task.enabled ? "已启用" : "已停用"}
                            </span>
                            <span className={`shrink-0 rounded-full px-2.5 py-0.5 text-[11px] font-medium ${statusBadge(task.last_status ?? "")}`}>
                              {displayStatus(task.last_status)}
                            </span>
                          </div>
                          <div className="mt-0.5 text-[11px] text-muted-foreground">#{task.id}</div>
                        </div>
                        <div className="flex w-full flex-wrap gap-2 sm:w-auto">
                          <Button variant="outline" className="h-7 px-2.5 text-[11px]" onClick={() => void runAction(api(`/api/sign-in-tasks/${task.id}/run`, { method: "POST" }), "已触发运行一次", true)}>
                            <Zap className="mr-1.5 h-3.5 w-3.5" />运行一次
                          </Button>
                          {task.enabled ? (
                            <Button variant="outline" className="h-7 px-2.5 text-[11px]" onClick={() => void runAction(api(`/api/sign-in-tasks/${task.id}/stop`, { method: "POST" }), "自动签到任务已停用")}>
                              <Pause className="mr-1.5 h-3.5 w-3.5" />停用
                            </Button>
                          ) : (
                            <Button variant="secondary" className="h-7 px-2.5 text-[11px]" onClick={() => void runAction(api(`/api/sign-in-tasks/${task.id}/start`, { method: "POST" }), "自动签到任务已启用")}>
                              <Play className="mr-1.5 h-3.5 w-3.5" />启用
                            </Button>
                          )}
                          <Button variant="outline" className="h-7 px-2.5 text-[11px]" onClick={() => openEdit(task)}>
                            <Edit className="mr-1.5 h-3.5 w-3.5" />编辑
                          </Button>
                          <Button variant="destructive" className="h-7 px-2.5 text-[11px]" onClick={() => setDeleteTarget(task)}>
                            <Trash2 className="mr-1.5 h-3.5 w-3.5" />删除
                          </Button>
                        </div>
                      </div>

                      <div className="mt-2.5 grid gap-1.5 text-[11px] text-muted sm:grid-cols-2 xl:grid-cols-5">
                        <div className="truncate"><span className="font-medium text-foreground">站点: </span>{siteNameById.get(task.site_id) ?? `#${task.site_id}`}</div>
                        <div className="truncate"><span className="font-medium text-foreground">浏览器: </span>{browserLabel(task.browser)}</div>
                        <div className="truncate"><span className="font-medium text-foreground">间隔: </span>每 {cronToInterval(task.cron_expression)} 小时</div>
                        <div className="truncate"><span className="font-medium text-foreground">方式: </span>{task.browser === "browserless" && task.sign_in_method !== "ocr_captcha" ? `CF ${browserlessCfModeLabel(task.browserless.cf_mode)}` : signInMethodLabel(task.sign_in_method)}</div>
                        <div className="truncate"><span className="font-medium text-foreground">最近时间: </span>{formatDate(task.last_run_at)}</div>
                        <div className="truncate sm:col-span-2 xl:col-span-5"><span className="font-medium text-foreground">最近消息: </span>{task.last_message || "-"}</div>
                      </div>
                    </div>
                  ))}
                </div>
              )}
            </div>
          ) : (
            <div
              id="sign-in-records-panel"
              role="tabpanel"
              aria-labelledby="sign-in-records-tab"
              tabIndex={0}
              className="focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/30"
            >
              <p className="mb-3 text-xs text-muted-foreground" aria-live="polite">
                匹配 {recordSearch.total} 条执行日志
              </p>
              {recordSearch.total === 0 && !searchTerm && siteFilter === 0 ? (
                <div className="py-12 text-center text-sm text-muted-foreground">暂无签到执行日志。</div>
              ) : filteredRecords.length === 0 ? (
                <div className="py-12 text-center text-sm text-muted-foreground">没有匹配的执行日志，请更换关键词或站点筛选。</div>
              ) : (
                <Table>
                  <TableHeader>
                    <TableRow>
                      <TableHead>任务</TableHead>
                      <TableHead>站点</TableHead>
                      <TableHead>状态</TableHead>
                      <TableHead>消息</TableHead>
                      <TableHead>开始时间</TableHead>
                      <TableHead>结束时间</TableHead>
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    {filteredRecords.map((record) => (
                      <TableRow key={record.id}>
                        <TableCell>
                          <div className="max-w-48 truncate font-medium" title={taskNameById.get(record.task_id)}>
                            {taskNameById.get(record.task_id) ?? `任务 #${record.task_id}`}
                          </div>
                          <div className="mt-0.5 text-[11px] text-muted-foreground">#{record.task_id}</div>
                        </TableCell>
                        <TableCell>{record.site_name || siteNameById.get(record.site_id) || `#${record.site_id}`}</TableCell>
                        <TableCell><span className={`rounded-full px-3 py-1 text-xs font-medium ${statusBadge(record.status)}`}>{displayStatus(record.status)}</span></TableCell>
                        <TableCell className="max-w-[360px] truncate text-muted-foreground" title={record.message}>{record.message || "-"}</TableCell>
                        <TableCell className="text-muted-foreground">{formatDate(record.started_at)}</TableCell>
                        <TableCell className="text-muted-foreground">{formatDate(record.finished_at)}</TableCell>
                      </TableRow>
                    ))}
                  </TableBody>
                </Table>
              )}
            </div>
          )}
          <SearchPagination search={activeSearch} label={activeView === "tasks" ? "签到任务" : "签到日志"} />
        </CardContent>
      </Card>

      <Dialog
        open={settingsOpen}
        onClose={closeSettings}
        title="签到浏览器配置"
        description="所有自动签到任务共用以下浏览器连接配置。"
        escMode="double"
        panelClassName="max-w-3xl"
      >
        <div className="space-y-5 p-4 sm:p-6">
          <div className="space-y-2 text-sm leading-relaxed">
            <p className="font-medium">为什么需要云浏览器？</p>
            <p className="text-muted-foreground">部分站点需要在浏览器中加载页面、运行脚本或完成验证后才能签到。云浏览器为 Kirara 提供所需的浏览器环境，代为处理这些步骤，无需在运行 Kirara 的设备上额外安装和维护浏览器。</p>
            <p className="text-muted-foreground">选择站点后，Kirara 会自动匹配对应的服务。你只需配置该站点需要的 Lightpanda 或 Browserless，连接信息可供所有签到任务共用。</p>
          </div>
          {settingsDraft ? (
            <>
              <div
                className="grid h-11 w-full grid-cols-2 rounded-2xl border border-border bg-surface-container p-1 sm:w-[360px]"
                role="tablist"
                aria-label="签到浏览器配置"
              >
                {(["lightpanda", "browserless"] as const).map((browser) => {
                  const selected = settingsBrowser === browser;
                  const configured = isBrowserConfigured(settingsDraft, browser);
                  return (
                    <button
                      key={browser}
                      id={`sign-in-browser-settings-tab-${browser}`}
                      type="button"
                      role="tab"
                      aria-selected={selected}
                      aria-controls={`sign-in-browser-settings-panel-${browser}`}
                      onClick={() => {
                        setSettingsBrowser(browser);
                        setConfigFeedback(null);
                      }}
                      className={cn(
                        "flex min-w-0 cursor-pointer items-center justify-center gap-1.5 rounded-xl px-2 text-xs font-medium transition-colors duration-200 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/40 sm:text-sm",
                        selected ? "bg-card text-foreground shadow-sm" : "text-muted hover:text-foreground",
                      )}
                    >
                      {browser === "lightpanda" ? <Zap className="h-4 w-4 shrink-0" /> : <Cloud className="h-4 w-4 shrink-0" />}
                      <span className="truncate">{browserLabel(browser)}</span>
                      <span className={cn("hidden text-[10px] sm:inline", configured ? "text-emerald-600 dark:text-emerald-400" : "text-muted-foreground")}>
                        {configured ? "已配置" : "未配置"}
                      </span>
                    </button>
                  );
                })}
              </div>

              {settingsBrowser === "lightpanda" ? (
                <div
                  id="sign-in-browser-settings-panel-lightpanda"
                  role="tabpanel"
                  aria-labelledby="sign-in-browser-settings-tab-lightpanda"
                  className="mt-5 grid grid-cols-1 gap-3 sm:grid-cols-2 sm:gap-4"
                >
                  <div className="space-y-2 sm:col-span-2">
                    <Label htmlFor="lightpanda-endpoint">Endpoint</Label>
                    <Input
                      id="lightpanda-endpoint"
                      value={settingsDraft.lightpanda.endpoint ?? defaultLightpandaEndpoint(settingsDraft.lightpanda.region)}
                      onChange={(event) => setLightpandaField("endpoint", event.target.value.trim() === defaultLightpandaEndpoint(settingsDraft.lightpanda.region) ? null : event.target.value || null)}
                      placeholder={defaultLightpandaEndpoint(settingsDraft.lightpanda.region)}
                      aria-describedby="lightpanda-endpoint-help"
                    />
                  </div>
                  <p id="lightpanda-endpoint-help" className="text-xs leading-relaxed text-muted sm:col-span-2">默认云端地址随区域切换，填写下方 Token 即可连接。自建服务可填写自己的完整 Endpoint；清空恢复默认。</p>
                  <p id="lightpanda-token-help" className="text-xs leading-relaxed text-muted sm:col-span-2">首次使用：前往 <a href="https://console.lightpanda.io/" target="_blank" rel="noreferrer" className="text-primary underline underline-offset-4">Lightpanda 控制台注册／登录</a>，创建 API Token，复制到下方 Token，然后点击“保存并测试”。</p>
                  <div className="space-y-2">
                    <Label htmlFor="lightpanda-token">API Token</Label>
                    <Input
                      id="lightpanda-token"
                      type="password"
                      autoComplete="off"
                      value={settingsDraft.lightpanda.token ?? ""}
                      onChange={(event) => setLightpandaField("token", event.target.value || null)}
                      placeholder="粘贴控制台生成的 API Token"
                      aria-describedby="lightpanda-token-help"
                    />
                  </div>
                  <div className="space-y-2">
                    <Label htmlFor="lightpanda-region">区域</Label>
                    <Select
                      id="lightpanda-region"
                      value={settingsDraft.lightpanda.region}
                      onChange={(value) => setLightpandaField("region", value)}
                      options={[
                        { value: "euwest", label: "EU West" },
                        { value: "uswest", label: "US West" },
                      ]}
                    />
                  </div>
                  <div className="space-y-2">
                    <Label htmlFor="lightpanda-browser">云端浏览器类型</Label>
                    <Input
                      id="lightpanda-browser"
                      value={settingsDraft.lightpanda.browser}
                      onChange={(event) => setLightpandaField("browser", event.target.value)}
                      placeholder="lightpanda"
                    />
                  </div>
                  <div className="space-y-2">
                    <Label htmlFor="lightpanda-proxy">代理策略</Label>
                    <Input
                      id="lightpanda-proxy"
                      value={settingsDraft.lightpanda.proxy ?? ""}
                      disabled={!settingsDraft.use_proxy_for_lightpanda}
                      onChange={(event) => setLightpandaField("proxy", event.target.value || null)}
                      placeholder="fast_dc"
                    />
                  </div>
                  <div className="space-y-2">
                    <Label htmlFor="lightpanda-country">国家代码</Label>
                    <Input
                      id="lightpanda-country"
                      value={settingsDraft.lightpanda.country ?? ""}
                      onChange={(event) => setLightpandaField("country", event.target.value || null)}
                      placeholder="US"
                    />
                  </div>
                  <label className="flex min-h-11 cursor-pointer items-center gap-3 self-end text-sm font-medium">
                    <input
                      type="checkbox"
                      className="size-4 accent-primary"
                      checked={settingsDraft.use_proxy_for_lightpanda}
                      onChange={(event) => {
                        setSettingsDraft((current) => current
                          ? { ...current, use_proxy_for_lightpanda: event.target.checked }
                          : current);
                        setConfigFeedback(null);
                      }}
                    />
                    使用 Lightpanda 代理
                  </label>
                </div>
              ) : (
                <div
                  id="sign-in-browser-settings-panel-browserless"
                  role="tabpanel"
                  aria-labelledby="sign-in-browser-settings-tab-browserless"
                  className="mt-5 grid gap-4 sm:grid-cols-2"
                >
                  <div className="space-y-2 sm:col-span-2">
                    <Label htmlFor="browserless-address">地址</Label>
                    <Input
                      id="browserless-address"
                      type="url"
                      value={settingsDraft.browserless.address ?? ""}
                      onChange={(event) => setBrowserlessSettingField("address", event.target.value || null)}
                      placeholder={DEFAULT_BROWSERLESS_ADDRESS}
                      aria-describedby="browserless-address-help"
                    />
                    <p id="browserless-address-help" className="text-xs leading-relaxed text-muted-foreground">已提供默认云端地址，也支持自定义服务根地址或完整的 /stealth/bql 地址。</p>
                  </div>
                  <p id="browserless-token-help" className="text-xs leading-relaxed text-muted sm:col-span-2">首次使用：<a href="https://www.browserless.io/signup/email?plan=free" target="_blank" rel="noreferrer" className="text-primary underline underline-offset-4">注册 Browserless 账号</a>，登录 <a href="https://browserless.io/account/" target="_blank" rel="noreferrer" className="text-primary underline underline-offset-4">账户控制台</a>，在 API Key 区域复制 Token，填入下方后点击“保存并测试”。</p>
                  <div className="space-y-2 sm:col-span-2">
                    <Label htmlFor="browserless-token">API Key / Token</Label>
                    <Input
                      id="browserless-token"
                      type="password"
                      autoComplete="off"
                      value={settingsDraft.browserless.token ?? ""}
                      onChange={(event) => setBrowserlessSettingField("token", event.target.value || null)}
                      placeholder="粘贴控制台中的 API Key / Token"
                      aria-describedby="browserless-token-help"
                    />
                  </div>
                </div>
              )}
            </>
          ) : (
            <div className="flex items-center justify-center py-8 text-sm text-muted-foreground">
              <Loader2 className="mr-2 h-4 w-4 animate-spin" />
              加载配置中...
            </div>
          )}

          {configFeedback ? (
            <div
              role="status"
              className={cn(
                "rounded-xl border px-4 py-3 text-sm",
                configFeedback.tone === "success"
                  ? "border-emerald-500/30 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300"
                  : "border-destructive/30 bg-destructive/5 text-destructive",
              )}
            >
              {configFeedback.text}
            </div>
          ) : null}

          <div className="flex flex-wrap gap-2 border-t border-border pt-4">
            <Button disabled={!settingsDraft || savingBrowser} onClick={() => void persistBrowserSettings(false)}>
              {savingBrowser && !probingBrowser ? <Loader2 className="mr-2 h-4 w-4 animate-spin" /> : <Save className="mr-2 h-4 w-4" />}
              保存配置
            </Button>
            <Button variant="outline" disabled={!settingsDraft || savingBrowser} onClick={() => void persistBrowserSettings(true)}>
              {probingBrowser ? <Loader2 className="mr-2 h-4 w-4 animate-spin" /> : <FlaskConical className="mr-2 h-4 w-4" />}
              {probingBrowser ? "测试中..." : "保存并测试"}
            </Button>
            <Button variant="outline" disabled={savingBrowser} onClick={closeSettings}>
              <X className="mr-2 h-4 w-4" />
              关闭
            </Button>
          </div>
        </div>
      </Dialog>

      <Dialog
        open={formOpen}
        onClose={closeForm}
        title={editingId !== null ? "编辑自动签到任务" : "添加自动签到任务"}
        description="选择站点和执行间隔，已支持站点会自动匹配签到方式。"
        escMode="double"
        panelClassName="max-w-3xl"
      >
        <div className="space-y-6 p-4 sm:p-6">
          {submitError ? (
            <div className="rounded-xl border border-destructive/30 bg-destructive/5 px-4 py-3 text-sm text-destructive">{submitError}</div>
          ) : null}

          <div className="grid gap-4 sm:grid-cols-2">
            <div className="space-y-2">
              <Label htmlFor="sign-in-name">名称</Label>
              <Input
                id="sign-in-name"
                aria-describedby={suggestedTaskName ? "sign-in-name-suggestion" : undefined}
                value={form.name}
                onChange={(event) => setField("name", event.target.value)}
                placeholder="每日签到"
              />
              {suggestedTaskName ? (
                <div id="sign-in-name-suggestion" className="flex min-h-7 flex-wrap items-center gap-1.5 text-xs text-muted-foreground">
                  <span>推荐名称</span>
                  <button
                    type="button"
                    className="inline-flex max-w-full cursor-pointer items-center gap-1 rounded-md px-1.5 py-1 font-medium text-primary transition-colors duration-200 hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/40"
                    title={`使用推荐名称：${suggestedTaskName}`}
                    onClick={() => setField("name", suggestedTaskName)}
                  >
                    <Sparkles className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                    <span className="truncate">{suggestedTaskName}</span>
                  </button>
                </div>
              ) : null}
            </div>
            <div className="space-y-2">
              <Label htmlFor="sign-in-site">站点</Label>
              <Select
                id="sign-in-site"
                value={form.site_id ? String(form.site_id) : ""}
                onChange={(value) => selectSite(value === "" ? 0 : Number(value))}
                options={nexusSites.length === 0 ? [{ value: "", label: "请先添加 NexusPHP 站点" }] : nexusSites.map((site) => ({ value: String(site.id), label: `${site.name} · #${site.id} (${site.site_type})` }))}
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor="sign-in-interval">执行间隔</Label>
              <Select
                id="sign-in-interval"
                value={String(intervalHours)}
                onChange={(value) => setIntervalHours(Number(value) as SignInIntervalHours)}
                options={SIGN_IN_INTERVAL_HOURS.map((hours) => ({ value: String(hours), label: `每 ${hours} 小时` }))}
              />
            </div>
            {selectedProfile ? (
              <div className="space-y-1 sm:col-span-2" role="status">
                <p className="text-sm font-medium">{selectedProfile.label}</p>
                <p className="text-sm text-muted-foreground">已自动适配，无需配置签到参数。</p>
              </div>
            ) : (
              <div className="space-y-2 sm:col-span-2">
                <Label htmlFor="sign-in-method">签到方式</Label>
                <Select id="sign-in-method" value={signerChoice} onChange={chooseSigner} options={[
                  { value: "nexus", label: "打开页面签到" },
                  { value: "cf_challenge", label: "CF 页面挑战签到" },
                  { value: "cf_turnstile", label: "CF Turnstile 签到" },
                  { value: "captcha", label: "通用图片验证码签到" },
                ]} />
                <p className="text-xs text-muted-foreground">此站尚未自动适配，默认尝试打开页面签到。如需其他方式，请按站点实际情况选择。</p>
              </div>
            )}
            {!selectedProfile && imageCaptcha ? <>
              <p id="sign-in-image-guidance" className="text-xs leading-relaxed text-muted-foreground sm:col-span-2">
                以下五项需要了解站点页面结构。普通用户可通过 <a href="https://github.com/imythu/kirara/issues/new" target="_blank" rel="noreferrer" className="text-primary underline underline-offset-4">GitHub 联系作者适配站点</a>。
              </p>
              {([
                ["attendance_path", "签到路径", "/attendance.php"],
                ["captcha_selector", "验证码图片 Selector", "验证码图片的 CSS Selector"],
                ["captcha_input_selector", "验证码输入框 Selector", "验证码输入框的 CSS Selector"],
                ["selector", "签到按钮 Selector", "签到按钮的 CSS Selector"],
              ] as const).map(([key, label, placeholder]) => <div key={key} className="space-y-2 sm:col-span-2">
                <Label htmlFor={`browserless-${key}`}>{label}</Label>
                <Input id={`browserless-${key}`} value={browserlessTask[key]} onChange={event => setBrowserlessTaskField(key, event.target.value)} placeholder={placeholder} required spellCheck={false} maxLength={2048} aria-describedby="sign-in-image-guidance" />
              </div>)}
              <div className="space-y-2 sm:col-span-2">
                <Label htmlFor="browserless-already-keywords">已签到提示文字（可留空）</Label>
                <textarea id="browserless-already-keywords" className="min-h-24 w-full rounded-lg border border-border bg-background px-3 py-2 text-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring" value={browserlessTask.already_keywords} onChange={event => setBrowserlessTaskField("already_keywords", event.target.value)} maxLength={4096} placeholder="例如：您今天已经签到，请勿重复签到。" />
                <p className="text-xs text-muted-foreground">每行一条明确的提示，命中后跳过签到。留空不提前判断。</p>
              </div>
            </> : null}
            {isBrowserConfigured(settings, form.browser ?? "lightpanda") ? (
              <p className="text-xs text-muted-foreground sm:col-span-2">使用 {browserLabel(form.browser)} 云浏览器，连接信息已配置。</p>
            ) : (
              <div role="alert" className="space-y-2 rounded-lg border border-destructive/30 bg-destructive/5 p-3 text-sm sm:col-span-2">
                <p>{missingBrowserMessage(settings, form.browser ?? "lightpanda")}</p>
                <Button type="button" variant="outline" onClick={() => { setSettingsBrowser(form.browser ?? "lightpanda"); openSettings(); }}>配置 {browserLabel(form.browser)}</Button>
              </div>
            )}

          </div>

          <div className="flex flex-wrap gap-3 border-t border-border pt-4">
            <Button disabled={submitting} onClick={() => void handleSubmit()}>
              {submitting ? <Loader2 className="mr-2 h-4 w-4 animate-spin" /> : null}
              {submitting ? "提交中..." : editingId !== null ? "保存修改" : "创建任务"}
            </Button>
            <Button variant="outline" disabled={submitting} onClick={closeForm}>取消</Button>
          </div>
        </div>
      </Dialog>

      <Dialog
        open={deleteTarget !== null}
        onClose={() => setDeleteTarget(null)}
        title="确认删除"
        description={`确定要删除自动签到任务「${deleteTarget?.name ?? ""}」吗？此操作不可撤销。`}
      >
        <div className="flex justify-end gap-2 pt-2">
          <Button variant="secondary" onClick={() => setDeleteTarget(null)}>取消</Button>
          <Button variant="destructive" onClick={() => void confirmDelete()} disabled={deleting}>
            {deleting ? <Loader2 className="mr-2 h-4 w-4 animate-spin" /> : <Trash2 className="mr-2 h-4 w-4" />}
            删除
          </Button>
        </div>
      </Dialog>
    </div>
  );
}
