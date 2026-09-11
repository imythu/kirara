import { useCallback, useEffect, useRef, useState } from "react";
import { Archive, ArrowLeft, ArrowUpRight, Ban, ChevronDown, Clock3, Download, ListFilter, Loader2, Pause, Pencil, Play, Plus, RefreshCw, RotateCcw, Rss, Search, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Dialog } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Select } from "@/components/ui/select";
import { FeedDialog } from "@/components/rss/feed-dialog";
import { RuleEditor } from "@/components/rss/rule-editor";
import { BackfillDialog } from "@/components/rss/backfill-dialog";
import { JobDialog } from "@/components/rss/job-dialog";
import { AttributeDetails, Empty, ErrorBox, Evidence, ListSkeleton, Pager, Status } from "@/components/rss/shared";
import { api } from "@/lib/api";
import { allRssRecords, canCancelJob, canReconcileJob, canRetryJob, downloadState, newRequestId, rssApi, rssDate, rssError, rssSize, type RssFeed, type RssItem, type RssJob, type RssPage, type RssRule, type RssRun, type RssSummary } from "@/lib/rss-api";
import { cn } from "@/lib/utils";
import type { DownloaderRecord, SiteRecord } from "@/types";

type Tab = "feeds" | "rules" | "downloads";
type Route = { tab: Tab; feed: number | null; job: number | null; edit: string; q: string; status: string; site: string; source: string; target: string; page: number };
type Confirmation = { title: string; description: string; label: string; dangerous?: boolean; execute: () => Promise<void> };
const blankPage = <T,>(): RssPage<T> => ({ items: [], total: 0, page: 1, page_size: 20 });
function readRoute(): Route {
  const search = new URLSearchParams(window.location.hash.split("?")[1] ?? "");
  const number = (key: string) => { const value = Number(search.get(key)); return Number.isSafeInteger(value) && value > 0 ? value : null; };
  const tab = search.get("tab");
  return { tab: tab === "rules" || tab === "downloads" ? tab : "feeds", feed: number("feed"), job: number("job"), edit: search.get("edit") ?? "", q: search.get("q") ?? "", status: search.get("status") ?? "", site: search.get("site") ?? "", source: search.get("source") ?? "", target: search.get("target") ?? "", page: number("page") ?? 1 };
}
const feedStatuses = [{ value: "", label: "全部状态" }, { value: "running", label: "运行中" }, { value: "paused", label: "已暂停" }, { value: "needs_attention", label: "需处理" }];
const itemStatuses = [{ value: "", label: "全部条目" }, { value: "pending", label: "待处理" }, { value: "queued", label: "已入队" }, { value: "skipped", label: "已跳过" }, { value: "baseline", label: "已有资源" }, { value: "needs_attention", label: "需处理" }];
const jobStatuses = [{ value: "", label: "全部添加状态" }, { value: "queued", label: "已加入队列" }, { value: "held", label: "已暂停等待" }, { value: "waiting", label: "等待条件" }, { value: "retry_wait", label: "等待重试" }, { value: "reconciling", label: "正在确认添加结果" }, { value: "submitted", label: "已添加到下载器" }, { value: "already_present", label: "已存在" }, { value: "needs_attention", label: "需处理" }, { value: "failed", label: "添加失败" }, { value: "cancelled", label: "已取消" }];

export function RssPage() {
  const [route, setRoute] = useState(readRoute);
  const [summary, setSummary] = useState<RssSummary | null>(null);
  const [feedPage, setFeedPage] = useState<RssPage<RssFeed>>(blankPage);
  const [rulePage, setRulePage] = useState<RssPage<RssRule>>(blankPage);
  const [jobPage, setJobPage] = useState<RssPage<RssJob>>(blankPage);
  const [itemPage, setItemPage] = useState<RssPage<RssItem>>(blankPage);
  const [feeds, setFeeds] = useState<RssFeed[]>([]);
  const [rules, setRules] = useState<RssRule[]>([]);
  const [sites, setSites] = useState<SiteRecord[]>([]);
  const [downloaders, setDownloaders] = useState<DownloaderRecord[]>([]);
  const [selectedFeed, setSelectedFeed] = useState<RssFeed | null>(null);
  const [selectedJob, setSelectedJob] = useState<RssJob | null>(null);
  const [editRule, setEditRule] = useState<RssRule | null>(null);
  const [editLoading, setEditLoading] = useState(false);
  const [feedDialog, setFeedDialog] = useState<{ record: RssFeed | null } | null>(null);
  const [selectedItems, setSelectedItems] = useState<Map<number, RssItem>>(new Map());
  const [backfillOpen, setBackfillOpen] = useState(false);
  const [runsOpen, setRunsOpen] = useState(false);
  const [runs, setRuns] = useState<RssPage<RssRun>>(blankPage);
  const [runPage, setRunPage] = useState(1);
  const [watchedRun, setWatchedRun] = useState<RssRun | null>(null);
  const [confirmation, setConfirmation] = useState<Confirmation | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [refreshing, setRefreshing] = useState(false);
  const [loaded, setLoaded] = useState(false);
  const [updatedAt, setUpdatedAt] = useState<string | null>(null);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [search, setSearch] = useState(route.q);
  const requestIds = useRef(new Map<string, string>());
  const loadSequence = useRef(0);
  const loadRef = useRef<() => Promise<void>>(async () => {});
  const mounted = useRef(true);

  useEffect(() => { mounted.current = true; const listener = () => setRoute(readRoute()); window.addEventListener("hashchange", listener); return () => { mounted.current = false; window.removeEventListener("hashchange", listener); }; }, []);
  function go(patch: Partial<Route>) {
    const next = { ...readRoute(), ...patch };
    const params = new URLSearchParams();
    for (const [key, value] of Object.entries(next)) if (value !== null && value !== "" && !(key === "page" && value === 1)) params.set(key, String(value));
    window.location.hash = `/rss?${params.toString()}`;
    setRoute(next);
  }
  useEffect(() => { setSearch(route.q); }, [route.q]);
  useEffect(() => { if (route.edit) setNotice(""); }, [route.edit]);
  useEffect(() => { if (search === route.q) return; const timeout = window.setTimeout(() => go({ q: search, page: 1 }), 300); return () => window.clearTimeout(timeout); }, [search, route.q]);
  useEffect(() => { setSelectedItems(new Map()); setRunPage(1); }, [route.feed]);

  const refreshOptions = useCallback(async () => {
    const results = await Promise.allSettled([allRssRecords<RssFeed>("/feeds"), allRssRecords<RssRule>("/rules"), api<SiteRecord[]>("/api/sites"), api<DownloaderRecord[]>("/api/downloaders")]);
    if (!mounted.current) return;
    const [feedResult, ruleResult, siteResult, downloaderResult] = results;
    if (feedResult.status === "fulfilled") setFeeds(feedResult.value);
    if (ruleResult.status === "fulfilled") setRules(ruleResult.value);
    if (siteResult.status === "fulfilled") setSites(siteResult.value);
    if (downloaderResult.status === "fulfilled") setDownloaders(downloaderResult.value);
    const failed = results.find((result) => result.status === "rejected");
    if (failed?.status === "rejected") setError(rssError(failed.reason));
  }, []);
  useEffect(() => { void refreshOptions(); }, [refreshOptions]);

  useEffect(() => {
    if (!route.edit || route.edit === "new") { setEditRule(null); setEditLoading(false); return; }
    let cancelled = false;
    setEditRule(null);
    setEditLoading(true);
    rssApi.get<RssRule>(`/rules/${encodeURIComponent(route.edit)}`).then((record) => { if (!cancelled) setEditRule(record); }).catch((e) => { if (!cancelled) setError(rssError(e)); }).finally(() => { if (!cancelled) setEditLoading(false); });
    return () => { cancelled = true; };
  }, [route.edit]);

  const load = useCallback(async () => {
    const sequence = ++loadSequence.current;
    setRefreshing(true);
    const query = new URLSearchParams({ page: String(route.page), page_size: "20" });
    if (route.q) query.set("keyword", route.q);
    if (route.status) query.set("status", route.status);
    if (route.site) query.set("site_id", route.site);
    if (route.source) query.set("feed_id", route.source);
    if (route.target) query.set("downloader_id", route.target);
    if (route.feed && route.tab === "feeds") query.set("feed_id", String(route.feed));
    const listPath = route.tab === "feeds" ? route.feed ? "/items" : "/feeds" : route.tab === "rules" ? "/rules" : "/downloads";
    const tasks: Promise<unknown>[] = [rssApi.get<RssSummary>("/summary"), rssApi.get(`${listPath}?${query.toString()}`)];
    const feedIndex = route.feed && route.tab === "feeds" ? tasks.push(rssApi.get<RssFeed>(`/feeds/${route.feed}`)) - 1 : -1;
    const jobIndex = route.job && route.tab === "downloads" ? tasks.push(rssApi.get<RssJob>(`/downloads/${route.job}`)) - 1 : -1;
    const runsIndex = runsOpen ? tasks.push(rssApi.get<RssPage<RssRun>>(`/runs?page=${runPage}&page_size=20${route.feed ? `&feed_id=${route.feed}` : ""}`)) - 1 : -1;
    const watchedIndex = watchedRun?.id && (!watchedRun.finished_at || watchedRun.pending_count > 0) ? tasks.push(rssApi.get<RssRun>(`/runs/${watchedRun.id}`)) - 1 : -1;
    const results = await Promise.allSettled(tasks);
    if (!mounted.current || sequence !== loadSequence.current) return;
    const summaryResult = results[0]; if (summaryResult.status === "fulfilled") setSummary(summaryResult.value as RssSummary);
    const listResult = results[1];
    if (listResult.status === "fulfilled") {
      if (route.tab === "feeds" && route.feed) setItemPage(listResult.value as RssPage<RssItem>);
      else if (route.tab === "feeds") setFeedPage(listResult.value as RssPage<RssFeed>);
      else if (route.tab === "rules") setRulePage(listResult.value as RssPage<RssRule>);
      else setJobPage(listResult.value as RssPage<RssJob>);
      setUpdatedAt(new Date().toISOString()); setLoaded(true);
    }
    const feedResult = results[feedIndex]; if (feedResult?.status === "fulfilled") setSelectedFeed(feedResult.value as RssFeed);
    const jobResult = results[jobIndex]; if (jobResult?.status === "fulfilled") setSelectedJob(jobResult.value as RssJob);
    const runsResult = results[runsIndex]; if (runsResult?.status === "fulfilled") setRuns(runsResult.value as RssPage<RssRun>);
    const watchedResult = results[watchedIndex]; if (watchedResult?.status === "fulfilled") setWatchedRun(watchedResult.value as RssRun);
    const failed = results.find((result) => result.status === "rejected");
    if (failed?.status === "rejected") setError(rssError(failed.reason)); else setError("");
    setLoading(false); setRefreshing(false);
  }, [route, runsOpen, runPage, watchedRun?.id, watchedRun?.finished_at, watchedRun?.pending_count]);
  loadRef.current = load;
  useEffect(() => { void load(); }, [load]);
  const active = (summary?.queued ?? 0) > 0 || !!(watchedRun && (!watchedRun.finished_at || watchedRun.pending_count > 0)) || jobPage.items.some((job) => ["fetching", "submitting", "reconciling"].includes(job.status) || (job.status === "submitted" && job.progress != null && job.progress < 1));
  useEffect(() => {
    let timer: number;
    const schedule = () => { timer = window.setTimeout(async () => { if (!document.hidden) await loadRef.current(); schedule(); }, active ? 5000 : 30000); };
    const visible = () => { window.clearTimeout(timer); if (!document.hidden) { void loadRef.current(); schedule(); } };
    schedule(); document.addEventListener("visibilitychange", visible);
    return () => { window.clearTimeout(timer); document.removeEventListener("visibilitychange", visible); };
  }, [active]);

  function requestId(key: string) { let id = requestIds.current.get(key); if (!id) { id = newRequestId(); requestIds.current.set(key, id); } return id; }
  async function mutation(key: string, action: (requestId: string) => Promise<unknown>, message: string) {
    if (busy) return;
    setBusy(key); setError("");
    try { await action(requestId(key)); requestIds.current.delete(key); setNotice(message); setConfirmation(null); await Promise.all([loadRef.current(), refreshOptions()]); }
    catch (e) { setError(rssError(e)); } finally { setBusy(null); }
  }
  function openRule(id: string, feedId?: number) { setSelectedJob(null); go({ tab: "rules", edit: id, feed: feedId ?? null, job: null, q: "", status: "", source: "", target: "", site: "", page: 1 }); }
  function selectTab(tab: Tab) { setSelectedJob(null); setSelectedFeed(null); go({ tab, feed: null, job: null, edit: "", q: "", status: "", site: "", source: "", target: "", page: 1 }); }
  function toggleFeed(feed: RssFeed) {
    const action = feed.enabled ? "pause" : "resume";
    const execute = () => mutation(`feed-${feed.id}-${action}-${feed.version}`, (id) => rssApi.post(`/feeds/${feed.id}/${action}`, { expected_version: feed.version, request_id: id }), feed.enabled ? "订阅源已暂停，等待添加的任务也已暂停。" : "订阅源已恢复，将继续检查更新。暂停期间的资源可按需勾选补下。");
    if (feed.enabled) void execute(); else setConfirmation({ title: "恢复订阅源", description: "恢复后继续处理等待添加的任务，并检查新资源。暂停期间出现的资源不会自动下载，可在资源列表中勾选补下。", label: "恢复检查", execute });
  }
  function toggleRule(rule: RssRule) {
    const action = rule.enabled ? "pause" : "resume";
    const execute = () => mutation(`rule-${rule.id}-${action}-${rule.version}`, (id) => rssApi.post(`/rules/${rule.id}/${action}`, { expected_version: rule.version, request_id: id }), rule.enabled ? "规则已暂停，等待添加的任务也已暂停。" : "规则已启用，将自动下载之后发现的符合条件的资源。");
    if (rule.enabled) void execute(); else setConfirmation({ title: "启用下载规则", description: "启用后继续处理等待添加的任务，并自动下载之后发现的新资源。已有资源可在订阅源的资源列表中勾选补下。", label: "启用规则", execute });
  }
  function archive(kind: "feeds" | "rules", record: RssFeed | RssRule) {
    setConfirmation({ title: `归档${kind === "feeds" ? "订阅源" : "下载规则"}`, description: `归档“${record.name}”将停止检查或匹配，并取消尚未添加到下载器的任务。已经发出的请求会继续确认结果。历史记录、下载器中的种子和文件会保留。`, label: "确认归档", dangerous: true, execute: () => mutation(`${kind}-${record.id}-archive-${record.version}`, async (id) => { await rssApi.archive(`/${kind}/${record.id}`, { expected_version: record.version, request_id: id }); if (kind === "feeds" && route.feed === record.id) { setSelectedFeed(null); go({ feed: null, page: 1 }); } }, "已归档，下载历史仍可查询。") });
  }
  async function check(feed: RssFeed) {
    await mutation(`feed-${feed.id}-check-${feed.version}`, async (id) => { const run = await rssApi.post<RssRun>(`/feeds/${feed.id}/check`, { expected_version: feed.version, request_id: id }); setWatchedRun(run); }, "正在检查更新，符合已启用规则的新资源会自动添加到下载器。");
  }
  function jobAction(action: "retry" | "reconcile" | "cancel", job: RssJob) {
    const execute = () => mutation(`job-${job.id}-${action}-${job.version}`, (id) => rssApi.post(`/downloads/${job.id}/${action}`, { expected_version: job.version, request_id: id }), action === "retry" ? "重试请求已保存。" : action === "reconcile" ? "正在查询下载器，确认任务是否已添加。" : "未提交任务已取消。");
    if (action === "cancel") setConfirmation({ title: "取消未提交任务", description: `取消“${job.title}”后将停止添加此资源。已经添加到下载器的种子和文件不会被删除。`, label: "确认取消任务", dangerous: true, execute }); else void execute();
  }
  function toggleItem(item: RssItem) { setSelectedItems((previous) => { const next = new Map(previous); if (next.has(item.id)) next.delete(item.id); else if (next.size < 200) next.set(item.id, item); return next; }); }
  const currentList = route.tab === "feeds" ? route.feed ? itemPage : feedPage : route.tab === "rules" ? rulePage : jobPage;
  const hasFilters = !!(route.q || route.status || route.site || route.source || route.target);
  function clearFilters() { setSearch(""); go({ q: "", status: "", site: "", source: "", target: "", page: 1 }); }
  const feed = selectedFeed?.id === route.feed ? selectedFeed : null;
  const editing = route.tab === "rules" && !!route.edit;

  return <div className="space-y-5" data-rss-page="true">
    {!editing && <>
      <div className="flex flex-wrap items-center justify-between gap-3"><div role="tablist" aria-label="RSS 下载页面" className="flex min-w-0 gap-1 rounded-xl border border-border bg-card p-1">{([{ tab: "feeds", label: "订阅源" }, { tab: "rules", label: "下载规则" }, { tab: "downloads", label: "下载记录" }] as const).map((tab) => <button type="button" role="tab" key={tab.tab} id={`rss-tab-${tab.tab}`} aria-selected={route.tab === tab.tab} aria-controls="rss-tab-panel" className={cn("min-h-11 whitespace-nowrap rounded-lg px-3 text-sm font-semibold transition-colors sm:px-5", route.tab === tab.tab ? "bg-accent text-primary" : "text-muted hover:bg-accent/60 hover:text-foreground")} onClick={() => selectTab(tab.tab)}>{tab.label}</button>)}</div><div className="flex flex-wrap gap-2">{route.tab === "feeds" && !route.feed && <Button className="h-11 px-4" onClick={() => setFeedDialog({ record: null })}><Plus />添加订阅源</Button>}{route.tab === "rules" && <Button className="h-11 px-4" onClick={() => openRule("new")}><Plus />新建规则</Button>}<Button className="h-11 px-3" variant="outline" onClick={() => { setRunsOpen(true); setRunPage(1); }}><Clock3 /><span className="hidden sm:inline">检查记录</span><span className="sm:hidden">记录</span></Button></div></div>
      <p className="text-sm leading-6 text-muted">{route.tab === "feeds" ? "订阅源用来查看站点的新资源，下载规则决定哪些资源自动下载。" : route.tab === "rules" ? "为订阅源设置筛选条件，符合的新资源会自动添加到指定下载器。" : "这里记录资源是否已添加到下载器，以及最新的下载进度。"}</p>
      {summary && <div className="flex flex-wrap items-center justify-between gap-2 text-xs"><div className="flex flex-wrap items-center gap-x-4"><button className="min-h-11 text-muted hover:text-foreground" onClick={() => go({ tab: "feeds", feed: null, status: "running", page: 1 })}>运行中 <span className="ml-1 font-semibold tabular-nums text-foreground">{summary.running}</span></button><button className="min-h-11 text-muted hover:text-foreground" onClick={() => go({ tab: "feeds", feed: null, status: "paused", page: 1 })}>已暂停 <span className="ml-1 font-semibold tabular-nums text-foreground">{summary.paused}</span></button><span className="text-muted">需处理 <span className={cn("ml-1 font-semibold tabular-nums", summary.needs_attention ? "text-destructive" : "text-foreground")}>{summary.needs_attention}</span></span><button className="min-h-11 text-primary hover:underline" onClick={() => go({ status: "needs_attention", page: 1 })}>查看本页异常</button><span className="text-muted">启用规则 {summary.rules_enabled} · 队列 {summary.queued}</span></div><div className="flex items-center gap-2 text-muted"><span>{updatedAt ? `更新于 ${rssDate(updatedAt)}` : "等待数据"}</span><button className="flex size-11 items-center justify-center rounded-lg hover:bg-accent disabled:opacity-50" onClick={() => void loadRef.current()} disabled={refreshing} aria-label="刷新列表"><RefreshCw className={cn("size-4", refreshing && "motion-safe:animate-spin")} /></button></div></div>}
    </>}
    {notice && <div className="flex flex-wrap items-start gap-2 rounded-xl border border-border bg-card px-4 py-3 text-sm" role="status"><p className="min-w-0 flex-1 leading-6">{notice}</p><button className="flex size-11 shrink-0 items-center justify-center rounded-lg text-muted hover:bg-accent" aria-label="关闭 RSS 提示" onClick={() => setNotice("")}><X className="size-4" /></button></div>}
    {error && <ErrorBox action={<Button className="h-11 px-3" variant="outline" onClick={() => { void loadRef.current(); void refreshOptions(); }}>重新加载</Button>}>{error}{updatedAt && <p className="mt-1 text-xs text-muted">保留上次成功读取的数据，时间：{rssDate(updatedAt)}。</p>}</ErrorBox>}
    {watchedRun && !editing && <div className="flex flex-wrap items-center gap-3 rounded-xl border border-border bg-card p-4 text-xs"><Status status={watchedRun.status} /><span className="flex-1 leading-6">检查 #{watchedRun.id} · 发现 {watchedRun.new_count} 条新资源 · 安排下载 {watchedRun.queued_count} 条 · 待处理 {watchedRun.pending_count} 条{watchedRun.message ? ` · ${watchedRun.message}` : ""}</span><Button className="h-11 px-3" variant="outline" onClick={() => setRunsOpen(true)}>查看处理记录</Button></div>}
    {editing ? editLoading ? <ListSkeleton /> : route.edit !== "new" && !editRule ? <Empty title="无法载入此规则" action={<Button className="h-11" variant="outline" onClick={() => selectTab("rules")}>返回下载规则</Button>}>规则可能已归档，或暂时无法连接服务。</Empty> : <RuleEditor key={route.edit} rule={editRule} feeds={feeds} downloaders={downloaders} feedId={route.feed ?? undefined} onBack={() => selectTab("rules")} onSaved={() => { setNotice("规则已保存。已有资源可在订阅源的资源列表中勾选补下。"); void refreshOptions(); selectTab("rules"); }} /> : <div role="tabpanel" id="rss-tab-panel" aria-labelledby={`rss-tab-${route.tab}`} className="space-y-4">
      {route.tab === "feeds" && route.feed && <div className="space-y-3"><Button className="-ml-3 h-11 border-transparent bg-transparent px-3" variant="outline" onClick={() => { setSelectedFeed(null); go({ feed: null, q: "", status: "", page: 1 }); }}><ArrowLeft />返回订阅源</Button>{feed && <><div className="flex flex-wrap items-start justify-between gap-3"><div className="min-w-0"><h2 className="break-words text-xl font-semibold">{feed.name}</h2><p className="mt-2 text-xs leading-6 text-muted">{feed.site_name || "独立订阅源"} · {feed.enabled ? "已启用" : "已暂停"} · 每 {feed.interval_minutes} 分钟检查 · {feed.item_count} 条已收集</p></div><div className="flex flex-wrap gap-2"><Button className="h-11 px-3" variant="outline" disabled={!!busy || !feed.enabled} onClick={() => void check(feed)}><RefreshCw />立即检查</Button><Button className="h-11 px-3" onClick={() => openRule("new", feed.id)}><Plus />为此源创建规则</Button></div></div><p className="break-all text-xs text-muted">{feed.url_display}</p>{!feed.initialized_at && <p className="rounded-lg bg-accent p-3 text-sm leading-6">等待第一次检查。已有资源会列在这里，供你按需勾选补下。</p>}{!feed.enabled && <p className="text-sm text-muted">订阅源和等待添加的任务已暂停。<button className="min-h-11 px-2 font-medium text-primary underline" onClick={() => toggleFeed(feed)}>恢复订阅源</button></p>}{feed.last_error && <ErrorBox action={<a className="inline-flex min-h-11 items-center text-primary underline" href="#/sites">查看站点配置<ArrowUpRight className="ml-1 size-4" /></a>}>{feed.last_error}</ErrorBox>}{!rules.some((rule) => rule.feed_ids.includes(feed.id) && rule.enabled) && <p className="text-xs leading-6 text-muted">还没有启用下载规则。点击「为此源创建规则」，选择想自动下载的资源。</p>}</>}</div>}
      <div className="grid gap-3 sm:grid-cols-2 xl:flex xl:items-end"><div className="min-w-0 xl:flex-1"><label className="mb-2 block text-xs font-medium" htmlFor="rss-search">{route.tab === "feeds" && !route.feed ? "搜索订阅源" : route.tab === "rules" ? "搜索规则" : "搜索资源"}</label><div className="relative"><Search className="pointer-events-none absolute left-3 top-3.5 size-4 text-muted" /><Input id="rss-search" className="pl-9" value={search} onChange={(event) => setSearch(event.target.value)} placeholder={route.tab === "feeds" && !route.feed ? "按名称查找" : route.tab === "rules" ? "按规则名称查找" : "按资源标题查找"} /></div></div>{route.tab === "feeds" && !route.feed ? <div className="min-w-0 xl:w-44"><label className="mb-2 block text-xs font-medium" htmlFor="rss-site-filter">关联站点</label><Select id="rss-site-filter" value={route.site} onChange={(value) => go({ site: value, page: 1 })} options={[{ value: "", label: "全部站点" }, ...sites.map((site) => ({ value: String(site.id), label: site.name }))]} /></div> : route.tab !== "feeds" ? <div className="min-w-0 xl:w-44"><label className="mb-2 block text-xs font-medium" htmlFor="rss-source-filter">订阅源</label><Select id="rss-source-filter" value={route.source} onChange={(value) => go({ source: value, page: 1 })} options={[{ value: "", label: "全部来源" }, ...feeds.map((record) => ({ value: String(record.id), label: record.name }))]} /></div> : null}{route.tab === "downloads" && <div className="min-w-0 xl:w-44"><label className="mb-2 block text-xs font-medium" htmlFor="rss-target-filter">下载器</label><Select id="rss-target-filter" value={route.target} onChange={(value) => go({ target: value, page: 1 })} options={[{ value: "", label: "全部下载器" }, ...downloaders.map((record) => ({ value: String(record.id), label: record.name }))]} /></div>}<div className="min-w-0 xl:w-48"><label className="mb-2 block text-xs font-medium" htmlFor="rss-status-filter">{route.tab === "downloads" ? "添加状态" : "状态"}</label><Select id="rss-status-filter" value={route.status} onChange={(value) => go({ status: value, page: 1 })} options={route.tab === "downloads" ? jobStatuses : route.feed && route.tab === "feeds" ? itemStatuses : feedStatuses} /></div>{hasFilters && <Button className="h-11 px-3" variant="outline" onClick={clearFilters}><ListFilter />清除筛选</Button>}</div>
      {route.tab === "feeds" && route.feed && itemPage.items.length > 0 && <div className="flex flex-wrap items-center justify-between gap-2 border-b border-border pb-3"><label className="flex min-h-11 cursor-pointer items-center gap-2 text-sm"><input type="checkbox" className="size-4 accent-primary" checked={itemPage.items.filter((item) => item.downloadable).length > 0 && itemPage.items.filter((item) => item.downloadable).every((item) => selectedItems.has(item.id))} onChange={(event) => setSelectedItems((previous) => { const next = new Map(previous); for (const item of itemPage.items) { if (!event.target.checked) next.delete(item.id); else if (item.downloadable && next.size < 200) next.set(item.id, item); } return next; })} />选择本页可下载条目</label><div className="flex flex-wrap items-center gap-2"><span className="text-xs text-muted">已选 {selectedItems.size} / 200</span>{selectedItems.size > 0 && <Button className="h-11 px-3" variant="outline" onClick={() => setSelectedItems(new Map())}>清空</Button>}<Button className="h-11 px-3" disabled={selectedItems.size === 0 || !feed?.enabled} onClick={() => setBackfillOpen(true)}><Download />补下所选条目</Button></div></div>}
      {loading ? <ListSkeleton /> : !loaded && error ? null : currentList.items.length === 0 ? <div className="rounded-xl border border-border bg-card"><Empty title={hasFilters ? "没有符合筛选条件的记录" : route.tab === "feeds" ? route.feed ? "订阅源尚未收集到条目" : "开始收取 RSS 中的新种子" : route.tab === "rules" ? "尚未开启自动下载" : "还没有下载记录"} action={hasFilters ? <Button className="h-11" variant="outline" onClick={clearFilters}>清除筛选</Button> : route.tab === "feeds" && !route.feed ? <Button className="h-11" onClick={() => setFeedDialog({ record: null })}><Plus />添加订阅源</Button> : route.tab === "rules" ? <Button className="h-11" onClick={() => openRule("new")}><Plus />创建下载规则</Button> : undefined}>{hasFilters ? "调整关键词或状态，查看其他记录。" : route.tab === "feeds" ? route.feed ? "第一次检查后，资源会出现在这里。已有资源可勾选补下，新资源按启用的规则自动下载。" : "添加站点的 RSS 链接，再创建规则，选择你想自动下载的新资源。" : route.tab === "rules" ? "选择订阅源、填写关键词并指定下载器，之后符合条件的新资源就会自动下载。" : "自动下载和手动补下的任务都会显示在这里，可以查看添加结果和下载进度。"}</Empty></div> : route.tab === "feeds" && route.feed ? <div className="divide-y divide-border rounded-xl border border-border bg-card">{itemPage.items.map((item) => <article className="p-4 sm:p-5" key={item.id}><div className="flex items-start gap-3"><label className="flex min-h-11 w-5 shrink-0 items-start pt-1.5"><input className="size-4 accent-primary" type="checkbox" aria-label={`选择 ${item.title}`} checked={selectedItems.has(item.id)} disabled={!item.downloadable || (!selectedItems.has(item.id) && selectedItems.size >= 200)} onChange={() => toggleItem(item)} /></label><div className="min-w-0 flex-1"><div className="flex flex-wrap items-start justify-between gap-2"><h3 className="min-w-0 flex-1 break-words text-sm font-semibold">{item.title}</h3><Status status={item.status} /></div><p className="mt-2 flex flex-wrap gap-x-3 gap-y-1 text-xs text-muted"><span>{rssSize(item.attributes.size_bytes)}</span><span>做种 {item.attributes.seeders ?? "未知"}</span><span>发现于 {rssDate(item.first_seen_at)}</span>{item.published_at && <span>发布于 {rssDate(item.published_at)}</span>}</p>{!item.downloadable && <p className="mt-2 text-xs text-muted">缺少可用种子定位，暂不能自动下载。</p>}<details className="group mt-2"><summary className="flex min-h-11 cursor-pointer list-none items-center gap-1 text-xs font-medium text-primary [&::-webkit-details-marker]:hidden">查看筛选结果与资源信息<ChevronDown className="size-4 transition-transform group-open:rotate-180" /></summary><div className="space-y-4 border-t border-border pt-3"><AttributeDetails attributes={item.attributes} />{item.decisions.length === 0 ? <p className="text-xs leading-6 text-muted">已有资源不会自动下载。想要这条资源，可以勾选后点击「补下所选条目」。</p> : item.decisions.map((decision) => <section key={decision.id} className="space-y-2 border-t border-border pt-3"><div className="flex flex-wrap items-center justify-between gap-2 text-xs"><button className="min-h-11 text-sm font-medium text-primary underline" onClick={() => openRule(String(decision.rule_id))}>{decision.rule_name}</button><Status status={decision.status} /></div><Evidence evaluation={decision.evaluation} /><p className="text-xs text-muted">判定：{rssDate(decision.checked_at)} · 使用当时的规则设置{decision.next_evaluate_at && ` · 下次判断 ${rssDate(decision.next_evaluate_at)}`}</p>{decision.job_id && <Button className="h-11 px-3" variant="outline" onClick={() => go({ tab: "downloads", feed: null, job: decision.job_id, status: "", q: "", page: 1 })}>查看下载任务</Button>}</section>)}</div></details></div></div></article>)}</div> : route.tab === "feeds" ? <div className="divide-y divide-border rounded-xl border border-border bg-card">{feedPage.items.map((record) => <article key={record.id} className="p-4 sm:p-5"><div className="grid items-start gap-4 xl:grid-cols-[minmax(0,1.25fr)_minmax(0,1fr)_auto]"><div className="min-w-0"><button className="min-h-11 text-left text-base font-semibold text-foreground hover:text-primary" onClick={() => { setSelectedFeed(record); go({ feed: record.id, status: "", q: "", page: 1 }); }}>{record.name}</button><p className="text-xs leading-6 text-muted">{record.site_name || "独立订阅源"} · {record.item_count} 条资源 · 待处理 {record.pending_count}</p><p className="mt-1 break-all text-xs text-muted">{record.url_display}</p></div><div className="min-w-0 space-y-2 pt-2"><div className="flex flex-wrap gap-3 text-xs"><span className={record.enabled ? "text-foreground" : "text-muted"}>{record.enabled ? "运行中" : "已暂停"}</span><Status status={record.last_status} /></div><p className="text-xs leading-5 text-muted">最近检查：{rssDate(record.last_checked_at)}<br />下次检查：{record.enabled ? record.next_run_at ? rssDate(record.next_run_at) : "等待调度" : "已暂停"}</p></div><div className="flex flex-wrap gap-1 xl:justify-end"><Button className="h-11 px-3" variant="outline" disabled={!!busy || !record.enabled} onClick={() => void check(record)}><RefreshCw />检查</Button><button className="flex size-11 items-center justify-center rounded-lg text-muted hover:bg-accent hover:text-foreground disabled:opacity-50" disabled={!!busy} aria-label={`${record.enabled ? "暂停" : "恢复"}${record.name}`} onClick={() => toggleFeed(record)}>{record.enabled ? <Pause className="size-4" /> : <Play className="size-4" />}</button><button className="flex size-11 items-center justify-center rounded-lg text-muted hover:bg-accent hover:text-foreground" aria-label={`编辑${record.name}`} onClick={() => setFeedDialog({ record })}><Pencil className="size-4" /></button><button className="flex size-11 items-center justify-center rounded-lg text-muted hover:bg-accent hover:text-destructive disabled:opacity-50" aria-label={`归档${record.name}`} disabled={!!busy} onClick={() => archive("feeds", record)}><Archive className="size-4" /></button></div></div>{record.last_error && <p className="mt-3 break-words text-xs leading-6 text-destructive">{record.last_error} · <a className="underline" href="#/sites">查看站点配置</a></p>}{!record.initialized_at && <p className="mt-3 text-xs text-muted">第一次检查仅列出已有资源，供你按需勾选补下。</p>}</article>)}</div> : route.tab === "rules" ? <div className="divide-y divide-border rounded-xl border border-border bg-card">{rulePage.items.map((record) => <article key={record.id} className="p-4 sm:p-5"><div className="flex flex-wrap items-start justify-between gap-3"><div className="min-w-0 flex-1"><button className="min-h-11 text-left text-base font-semibold hover:text-primary" onClick={() => openRule(String(record.id))}>{record.name}</button><p className="text-xs leading-6 text-muted">{record.feed_ids.map((id) => feeds.find((source) => source.id === id)?.name ?? "已归档来源").join("、") || "未选择来源"} → {record.downloader_name || "未选择下载器"}</p></div><div className="flex flex-wrap items-center gap-1"><span className="mr-2 text-xs text-muted">{record.enabled ? "已启用" : "已暂停"}</span><Button className="h-11 px-3" variant="outline" onClick={() => openRule(String(record.id))}><Pencil />编辑与预览</Button><button className="flex size-11 items-center justify-center rounded-lg text-muted hover:bg-accent disabled:opacity-50" disabled={!!busy} aria-label={`${record.enabled ? "暂停" : "启用"}${record.name}`} onClick={() => toggleRule(record)}>{record.enabled ? <Pause className="size-4" /> : <Play className="size-4" />}</button><button className="flex size-11 items-center justify-center rounded-lg text-muted hover:bg-accent hover:text-destructive disabled:opacity-50" disabled={!!busy} aria-label={`归档${record.name}`} onClick={() => archive("rules", record)}><Archive className="size-4" /></button></div></div><p className="mt-3 break-words text-sm leading-6">{record.filters.match_all ? "匹配全部标题" : `包含${record.filters.include_mode === "any" ? "任意" : "全部"}：${record.filters.include.join("、") || record.filters.include_regex || "尚未设置"}`}{record.filters.exclude.length > 0 && ` · 排除 ${record.filters.exclude.join("、")}`}</p><p className="mt-2 flex flex-wrap gap-x-4 gap-y-1 text-xs text-muted"><span>优先级 {record.priority}</span><span>累计命中 {record.matched_count}</span><span>{record.filters.free_only ? "仅确认免费" : "免费不限"}</span><span>{record.filters.hr_policy === "require_clear" ? "仅确认无 H&R" : "H&R 不限"}</span></p>{record.last_error && <p className="mt-3 text-xs leading-6 text-destructive">{record.last_error}</p>}</article>)}</div> : <div className="divide-y divide-border rounded-xl border border-border bg-card">{jobPage.items.map((record) => <article key={record.id} className="p-4 sm:p-5"><div className="flex flex-wrap items-start justify-between gap-3"><div className="min-w-0 flex-1"><button className="min-h-11 break-words text-left text-sm font-semibold hover:text-primary" onClick={() => { setSelectedJob(record); go({ job: record.id }); }}>{record.title}</button><p className="mt-1 break-words text-xs leading-6 text-muted">{record.feed_name} · {record.rule_name} → {record.downloader_name}</p></div><Status status={record.status} /></div><div className="mt-3 flex flex-wrap items-center justify-between gap-3"><div className="min-w-0 space-y-1 text-xs text-muted"><p>{rssSize(record.size_bytes)} · 入队 {rssDate(record.created_at)}</p><p>下载器：{downloadState(record)}{record.progress != null ? ` · ${(Math.max(0, Math.min(1, record.progress)) * 100).toFixed(1)}%` : ""} · 采集 {rssDate(record.sampled_at)}</p>{record.next_attempt_at && <p>下次重试 {rssDate(record.next_attempt_at)}</p>}</div><div className="flex flex-wrap gap-2"><Button className="h-11 px-3" variant="outline" onClick={() => { setSelectedJob(record); go({ job: record.id }); }}>查看详情</Button>{canRetryJob(record) && <Button className="h-11 px-3" variant="outline" disabled={!!busy} onClick={() => jobAction("retry", record)}><RotateCcw />重试</Button>}{canReconcileJob(record) && <Button className="h-11 px-3" variant="outline" disabled={!!busy} onClick={() => jobAction("reconcile", record)}><RefreshCw />确认添加结果</Button>}{canCancelJob(record) && <button className="flex size-11 items-center justify-center rounded-lg text-muted hover:bg-accent disabled:opacity-50" aria-label={`取消任务 ${record.title}`} disabled={!!busy} onClick={() => jobAction("cancel", record)}><Ban className="size-4" /></button>}</div></div>{record.last_error && <p className="mt-3 break-words text-xs leading-6 text-destructive">{record.last_error}</p>}</article>)}</div>}
      <Pager page={route.page} total={currentList.total} size={currentList.page_size} onChange={(page) => go({ page })} />
    </div>}
    {feedDialog && <FeedDialog feed={feedDialog.record} sites={sites} onClose={() => setFeedDialog(null)} onSaved={(record) => { const created = !feedDialog.record; setFeedDialog(null); setNotice(created ? "订阅源已保存。接着创建下载规则，选择想自动下载的资源。" : "订阅源已保存。"); void refreshOptions(); setSelectedFeed(record); go({ tab: "feeds", feed: record.id, edit: "", q: "", status: "", page: 1 }); void loadRef.current(); }} />}
    {backfillOpen && <BackfillDialog items={[...selectedItems.values()]} rules={rules} onClose={() => setBackfillOpen(false)} onQueued={(run) => { setWatchedRun(run); setBackfillOpen(false); setSelectedItems(new Map()); setNotice("补下请求已收到，可在下载记录中查看符合条件的任务。"); void loadRef.current(); }} />}
    {route.tab === "downloads" && route.job && selectedJob?.id === route.job && !confirmation && <JobDialog job={selectedJob} busy={!!busy} error={error} onClose={() => { setSelectedJob(null); go({ job: null }); }} onAction={jobAction} onRule={(id) => openRule(String(id))} />}
    {runsOpen && <Dialog open title="检查与处理记录" description="查看每次发现了多少新资源、安排了多少下载，以及还有多少资源等待处理。" onClose={() => setRunsOpen(false)} panelClassName="max-w-3xl"><div className="p-4 sm:p-6">{runs.items.length === 0 ? <Empty title="暂无检查记录">添加并启用订阅源后，检查结果会保存在这里。</Empty> : <div className="divide-y divide-border">{runs.items.map((run) => <article key={run.id} className="space-y-2 py-4"><div className="flex flex-wrap items-center justify-between gap-2"><h4 className="text-sm font-semibold">{run.kind === "backfill" ? "历史补下" : "订阅源检查"} #{run.id}{run.feed_id ? ` · ${feeds.find((record) => record.id === run.feed_id)?.name ?? "已归档来源"}` : ""}</h4><Status status={run.status} /></div><p className="text-xs leading-6 text-muted">读取 {run.item_count} 条 · 新发现 {run.new_count} 条 · 安排下载 {run.queued_count} 条 · 待处理 {run.pending_count} 条</p><p className="text-xs text-muted">开始 {rssDate(run.started_at)} · {run.finished_at ? `结束 ${rssDate(run.finished_at)}` : "仍在处理"}</p>{run.message && <p className="break-words text-sm leading-6">{run.message}</p>}</article>)}</div>}<Pager page={runPage} total={runs.total} size={runs.page_size} onChange={setRunPage} /></div></Dialog>}
    {confirmation && <Dialog open title={confirmation.title} description={confirmation.description} onClose={() => { if (!busy) setConfirmation(null); }} panelClassName="max-w-lg" footer={<div className="flex justify-end gap-2"><Button className="h-11 px-4" variant="outline" disabled={!!busy} onClick={() => setConfirmation(null)}>返回</Button><Button className="h-11 px-4" variant={confirmation.dangerous ? "destructive" : "default"} disabled={!!busy} onClick={() => void confirmation.execute()}>{busy && <Loader2 className="motion-safe:animate-spin" />}{confirmation.label}</Button></div>}><div className="px-4 py-2 sm:px-6">{error && <ErrorBox>{error}</ErrorBox>}</div></Dialog>}
  </div>;
}
