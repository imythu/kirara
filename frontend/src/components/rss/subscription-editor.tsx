import { useEffect, useRef, useState } from "react";
import { ArrowLeft, ArrowRight, Check, Loader2, RefreshCw } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Select } from "@/components/ui/select";
import { Dialog } from "@/components/ui/dialog";
import { CheckField, ErrorBox, Evidence, Field, OptionalSettings } from "./shared";
import { GIB, newRequestId, rssApi, rssError, rssSize, type RssFeed, type RssFeedInput, type RssPreview, type RssRule, type RssRuleInput } from "@/lib/rss-api";
import type { DownloaderRecord, SiteRecord } from "@/types";
import { cn } from "@/lib/utils";

type Subscription = { feed: RssFeed; rule: RssRule | null };
const words = (value: string) => value.split(/[,，\n]/).map(word => word.trim()).filter(Boolean);
const area = "min-h-24 w-full rounded-lg border border-border bg-background px-3 py-2 text-sm focus:outline-none focus:ring-2 focus:ring-primary";

export function SubscriptionEditor({ initial, sites, downloaders, onBack, onSaved }: { initial: Subscription | null; sites: SiteRecord[]; downloaders: DownloaderRecord[]; onBack: () => void; onSaved: (record: Subscription) => void }) {
  const [feed, setFeed] = useState<RssFeedInput>(() => ({ name: initial?.feed.name ?? "", url: null, site_id: initial?.feed.site_id ?? null, use_proxy: initial?.feed.use_proxy ?? null, enabled: initial?.feed.enabled ?? true, interval_minutes: initial?.feed.interval_minutes ?? 15, expected_version: initial?.feed.version }));
  const qb = downloaders.filter(d => ["qb", "qbittorrent"].includes(d.downloader_type.toLowerCase()));
  const [rule, setRule] = useState<RssRuleInput>(() => initial?.rule ? { ...initial.rule, expected_version: initial.rule.version } : { name: "", enabled: true, priority: 100, feed_ids: [], downloader_id: qb.length === 1 ? qb[0].id : null, filters: { include: [], exclude: [], include_mode: "all", include_regex: null, exclude_regex: null, match_all: true, min_size_bytes: null, max_size_bytes: null, min_seeders: null, free_only: false, hr_policy: "require_clear" }, options: { save_path: null, category: null, tags: [], paused: false, reserve_space_bytes: 0 } });
  const [include, setInclude] = useState(rule.filters.include.join("\n"));
  const [exclude, setExclude] = useState(rule.filters.exclude.join("\n"));
  const [tags, setTags] = useState(rule.options.tags.join(", "));
  const [step, setStep] = useState(0);
  const [busy, setBusy] = useState<"preview" | "save" | null>(null);
  const [error, setError] = useState("");
  const [issues, setIssues] = useState<Record<string, string>>({});
  const [preview, setPreview] = useState<RssPreview | null>(null);
  const [previewKey, setPreviewKey] = useState("");
  const [leave, setLeave] = useState(false);
  const heading = useRef<HTMLHeadingElement>(null);
  const request = useRef({ key: "", id: newRequestId() });
  const filters = { ...rule.filters, include: rule.filters.match_all ? [] : words(include), include_regex: rule.filters.match_all ? null : rule.filters.include_regex, exclude: words(exclude) };
  const body = { feed: { ...feed, name: feed.name.trim(), url: initial ? null : feed.url?.trim() }, rule: { ...rule, filters, options: { ...rule.options, tags: words(tags) } } };
  const signature = JSON.stringify(body);
  const original = useRef(signature);
  const dirty = original.current !== signature;
  const sampleKey = JSON.stringify({ url: body.feed.url, site: feed.site_id, proxy: feed.use_proxy, filters });
  const stale = sampleKey !== previewKey;
  useEffect(() => { const handler = (event: BeforeUnloadEvent) => { if (dirty) event.preventDefault(); }; window.addEventListener("beforeunload", handler); return () => window.removeEventListener("beforeunload", handler); }, [dirty]);
  function changeFeed<K extends keyof RssFeedInput>(key: K, value: RssFeedInput[K]) { setFeed(previous => ({ ...previous, [key]: value })); }
  function filter<K extends keyof RssRuleInput["filters"]>(key: K, value: RssRuleInput["filters"][K]) { setRule(previous => ({ ...previous, filters: { ...previous.filters, [key]: value } })); }
  function option<K extends keyof RssRuleInput["options"]>(key: K, value: RssRuleInput["options"][K]) { setRule(previous => ({ ...previous, options: { ...previous.options, [key]: value } })); }
  function validate(part: number) {
    const found: Record<string, string> = {};
    if (part === 0) {
      if (!feed.name.trim()) found["subscription-name"] = "给订阅起个名字。";
      if (!initial) { try { const url = new URL(feed.url ?? ""); if (!["http:", "https:"].includes(url.protocol) || url.username || url.password) throw new Error(); } catch { found["subscription-url"] = "请输入完整的 HTTP 或 HTTPS RSS 链接。"; } }
      if (!Number.isInteger(feed.interval_minutes) || feed.interval_minutes < 5 || feed.interval_minutes > 1440) found["subscription-interval"] = "检查间隔须为 5–1440 分钟的整数。";
    } else {
      if (!rule.downloader_id) found["subscription-downloader"] = "请选择 qBittorrent 下载器。";
      if (!filters.match_all && !filters.include.length && !filters.include_regex) found["subscription-include"] = "填写包含词，或选择全部标题。";
      if (filters.min_size_bytes != null && filters.max_size_bytes != null && filters.min_size_bytes > filters.max_size_bytes) found["subscription-min"] = "最小大小不能超过最大大小。";
      for (const [id, value] of [["subscription-min", filters.min_size_bytes], ["subscription-max", filters.max_size_bytes], ["subscription-seeders", filters.min_seeders], ["subscription-reserve", rule.options.reserve_space_bytes]] as const) if (value != null && (!Number.isSafeInteger(value) || value < 0)) found[id] = "请输入有效的非负数。";
    }
    setIssues(found);
    if (Object.keys(found).length) { setError("请检查标出的设置。输入内容已保留。"); requestAnimationFrame(() => document.getElementById(Object.keys(found)[0])?.focus()); return false; }
    setError(""); return true;
  }
  function move(next: number) { if (next > step && !validate(step)) return; setStep(next); requestAnimationFrame(() => { heading.current?.focus(); heading.current?.scrollIntoView({ block: "start" }); }); }
  async function runPreview() {
    setBusy("preview"); setError("");
    try { const result = await rssApi.post<RssPreview>("/subscriptions/preview", { source: { feed_id: initial?.feed.id, url: body.feed.url, site_id: feed.site_id, use_proxy: feed.use_proxy }, filters }); setPreview(result); setPreviewKey(sampleKey); }
    catch (e) { setError(rssError(e)); } finally { setBusy(null); }
  }
  async function save() {
    for (const part of [0, 1]) if (!validate(part)) { setStep(part); return; }
    setBusy("save");
    if (request.current.key !== signature) request.current = { key: signature, id: newRequestId() };
    try { const payload = { ...body, request_id: request.current.id }; onSaved(initial ? await rssApi.put<Subscription>(`/subscriptions/${initial.feed.id}`, payload) : await rssApi.post<Subscription>("/subscriptions", payload)); }
    catch (e) { setError(rssError(e)); } finally { setBusy(null); }
  }
  return <div className="space-y-6" data-subscription-editor>
    <Button variant="outline" className="h-11" disabled={!!busy} onClick={() => dirty ? setLeave(true) : onBack()}><ArrowLeft />返回我的订阅</Button>
    <div><h2 ref={heading} tabIndex={-1} className="text-xl font-semibold outline-none">{initial ? `设置订阅 · ${initial.feed.name}` : "添加 RSS 订阅"}</h2><p className="mt-2 text-sm text-muted">一个订阅，管理来源、下载条件和保存位置。</p></div>
    <ol className="grid grid-cols-3 border-b border-border" aria-label="添加订阅步骤">{["RSS 来源", "下载条件", "预览与确认"].map((label, index) => <li key={label}><button disabled={!!busy || index > step} aria-current={step === index ? "step" : undefined} onClick={() => move(index)} className={cn("flex min-h-14 w-full items-center justify-center gap-2 border-b-2 px-1 text-sm", step === index ? "border-primary font-semibold text-primary" : "border-transparent text-muted")}><span className="tabular-nums">{index < step ? <Check className="size-4" /> : index + 1}</span>{label}</button></li>)}</ol>
    <div className="grid items-start gap-8 xl:grid-cols-[minmax(0,1fr)_280px]">
      <div className="min-w-0 rounded-xl border border-border bg-card p-5 sm:p-7">
        {step === 0 && <div className="space-y-6">
          <Field id="subscription-url" label="RSS 链接" error={issues["subscription-url"]} hint={initial ? "地址已保存并隐藏敏感参数。更换来源时请新建订阅。" : "在 PT 站点的 RSS 页面选择分类，复制生成的完整订阅链接。"}>{initial ? <p className="break-all text-sm leading-6">{initial.feed.url_display}</p> : <Input id="subscription-url" type="url" value={feed.url ?? ""} onChange={e => changeFeed("url", e.target.value)} placeholder="https://站点地址/rss…" autoComplete="off" spellCheck={false} />}</Field>
          <Field id="subscription-name" label="订阅名称" error={issues["subscription-name"]}><Input id="subscription-name" value={feed.name} maxLength={100} onChange={e => changeFeed("name", e.target.value)} placeholder="例如：纪录片精选" /></Field>
          <Field id="subscription-site" label="关联站点（选填）" hint="复用站点登录信息，并补充免费、H&R 等资源属性。"><Select id="subscription-site" value={feed.site_id ? String(feed.site_id) : ""} onChange={value => changeFeed("site_id", value ? Number(value) : null)} options={[{ value: "", label: "不关联，直接使用 RSS 链接" }, ...sites.map(site => ({ value: String(site.id), label: site.name }))]} /></Field>
          <OptionalSettings title="检查频率与连接设置" defaultOpen={!!initial}>
            <Field id="subscription-interval" label="每隔多少分钟检查" hint="默认 15 分钟。可设置 5–1440 分钟。" error={issues["subscription-interval"]}><Input id="subscription-interval" type="number" min={5} max={1440} value={feed.interval_minutes} onChange={e => changeFeed("interval_minutes", Number(e.target.value))} /></Field>
            <Field id="subscription-proxy" label="访问代理"><Select id="subscription-proxy" value={feed.use_proxy == null ? "inherit" : feed.use_proxy ? "on" : "off"} onChange={v => changeFeed("use_proxy", v === "inherit" ? null : v === "on")} options={[{ value: "inherit", label: "跟随站点设置（未关联时直连）" }, { value: "on", label: "使用全局代理" }, { value: "off", label: "直接连接" }]} /></Field>
          </OptionalSettings>
        </div>}
        {step === 1 && <div className="space-y-6">
          <section className="space-y-4"><h3 className="font-semibold">下载哪些资源</h3>
            <Field id="subscription-mode" label="标题范围"><Select id="subscription-mode" value={filters.match_all ? "all" : "keywords"} onChange={v => filter("match_all", v === "all")} options={[{ value: "all", label: "全部标题，按下面的条件筛选" }, { value: "keywords", label: "只下载包含关键词的资源" }]} /></Field>
            {!filters.match_all && <><Field id="subscription-include" label="标题包含" hint="每行一个词，或用逗号分隔。不区分大小写。" error={issues["subscription-include"]}><textarea id="subscription-include" className={area} value={include} onChange={e => setInclude(e.target.value)} placeholder={'纪录片\n1080p'} /></Field><Field id="subscription-match" label="多个关键词"><Select id="subscription-match" value={filters.include_mode} onChange={v => filter("include_mode", v)} options={[{ value: "all", label: "全部都要包含" }, { value: "any", label: "包含任意一个" }]} /></Field></>}
            <Field id="subscription-exclude" label="排除关键词（选填）" hint="包含任意一个词就跳过。"><Input id="subscription-exclude" value={exclude} onChange={e => setExclude(e.target.value)} placeholder="预告片, 花絮" /></Field>
            <CheckField checked={filters.free_only} onChange={v => filter("free_only", v)} hint="只下载确认不计下载流量的资源；未知时等待确认。">只下载免费资源</CheckField>
            <Field id="subscription-hr" label="做种要求（H&R）" hint="默认只接收确认无 H&R 的资源。信息未知时等待站点补充，不会直接下载。"><Select id="subscription-hr" value={filters.hr_policy} onChange={v => filter("hr_policy", v)} options={[{ value: "require_clear", label: "只下载确认无 H&R 的资源" }, { value: "any", label: "不筛选 H&R，自行遵守做种要求" }]} /></Field>
          </section>
          <section className="space-y-4 border-t border-border pt-6"><h3 className="font-semibold">下载到哪里</h3>
            <Field id="subscription-downloader" label="下载器" error={issues["subscription-downloader"]}><Select id="subscription-downloader" value={rule.downloader_id ? String(rule.downloader_id) : ""} onChange={v => setRule(previous => ({ ...previous, downloader_id: v ? Number(v) : null }))} options={[{ value: "", label: "选择 qBittorrent" }, ...qb.map(d => ({ value: String(d.id), label: d.name }))]} /></Field>
            {!qb.length && <p className="text-sm leading-6 text-muted">还没有 qBittorrent。<a className="text-primary underline" href="#/downloaders" target="_blank" rel="noreferrer">打开下载器配置</a>，添加后返回并刷新页面。</p>}
            <Field id="subscription-path" label="保存目录（选填）" hint="留空使用下载器的默认目录。路径是下载器所在机器的目录。"><Input id="subscription-path" value={rule.options.save_path ?? ""} onChange={e => option("save_path", e.target.value || null)} placeholder="例如：/downloads/documentary" /></Field>
          </section>
          <OptionalSettings title="更多筛选：大小、做种人数与正则" defaultOpen={filters.min_size_bytes != null || filters.max_size_bytes != null || filters.min_seeders != null || !!filters.include_regex || !!filters.exclude_regex}>
            <div className="grid gap-4 sm:grid-cols-2">{([['subscription-min', '最小大小（GiB）', 'min_size_bytes'], ['subscription-max', '最大大小（GiB）', 'max_size_bytes']] as const).map(([id, label, key]) => <Field key={id} id={id} label={label} error={issues[id]}><Input id={id} type="number" min={0} step="any" placeholder="不限" value={filters[key] == null ? "" : filters[key] / GIB} onChange={e => filter(key, e.target.value === "" ? null : Math.round(Number(e.target.value) * GIB))} /></Field>)}</div>
            <Field id="subscription-seeders" label="最低做种人数" error={issues["subscription-seeders"]}><Input id="subscription-seeders" type="number" min={0} placeholder="不限" value={filters.min_seeders ?? ""} onChange={e => filter("min_seeders", e.target.value === "" ? null : Number(e.target.value))} /></Field>
            {!filters.match_all && <Field id="subscription-regex" label="包含正则（同时满足关键词）"><Input id="subscription-regex" value={filters.include_regex ?? ""} onChange={e => filter("include_regex", e.target.value || null)} placeholder="1080p|2160p" /></Field>}
            <Field id="subscription-exclude-regex" label="排除正则"><Input id="subscription-exclude-regex" value={filters.exclude_regex ?? ""} onChange={e => filter("exclude_regex", e.target.value || null)} /></Field>
          </OptionalSettings>
          <OptionalSettings title="下载选项：分类、标签与空间" defaultOpen={!!rule.options.category || !!tags || rule.options.paused || rule.options.reserve_space_bytes > 0}>
            <Field id="subscription-category" label="分类"><Input id="subscription-category" value={rule.options.category ?? ""} onChange={e => option("category", e.target.value || null)} /></Field>
            <Field id="subscription-tags" label="标签（逗号分隔）"><Input id="subscription-tags" value={tags} onChange={e => setTags(e.target.value)} /></Field>
            <Field id="subscription-reserve" label="保留可用空间（GiB）" hint="空间不足时等待，0 表示不额外预留。" error={issues["subscription-reserve"]}><Input id="subscription-reserve" type="number" min={0} value={rule.options.reserve_space_bytes / GIB} onChange={e => option("reserve_space_bytes", Math.round(Number(e.target.value) * GIB))} /></Field>
            <CheckField checked={rule.options.paused} onChange={v => option("paused", v)}>添加到下载器后先暂停种子</CheckField>
          </OptionalSettings>
        </div>}
        {step === 2 && <div className="space-y-6">
          <div className="flex flex-wrap items-center justify-between gap-3"><div><h3 className="font-semibold">先看看会选中什么</h3><p className="mt-2 text-sm leading-6 text-muted">读取当前 RSS，逐条解释匹配结果。预览不会下载。</p></div><Button variant="outline" className="h-11" disabled={!!busy} onClick={runPreview}>{busy === "preview" ? <Loader2 className="motion-safe:animate-spin" /> : <RefreshCw />}测试链接并预览</Button></div>
          {!preview ? <p className="border-y border-border py-8 text-sm leading-6 text-muted">点击上方按钮，检查链接是否可用、筛选是否符合预期。</p> : <div className="space-y-4">{stale && <p role="status" className="text-sm text-destructive">条件已修改，请重新预览。下面是修改前的结果。</p>}<p className="border-y border-border py-3 text-sm" role="status">读取 {preview.total} 条 · <span className="font-semibold text-jade">符合 {preview.matched}</span> · 不符合 {preview.rejected} · 待确认 {preview.unknown}</p>{preview.total === 0 && <p className="text-sm text-muted">链接可读取，但当前没有资源。可以保存，等待下次更新。</p>}{preview.sample_limited && <p className="text-xs text-muted">仅展示部分样本，结果不代表全部资源。</p>}<div className="max-h-96 divide-y divide-border overflow-y-auto">{preview.items.map(({ item, evaluation }) => <details key={item.id} className="py-3"><summary className="cursor-pointer text-sm leading-6"><span className="mr-2 font-medium">{evaluation.matched ? "符合" : evaluation.needs_attributes ? "待确认" : "跳过"}</span>{item.title}<span className="ml-2 text-xs text-muted">{rssSize(item.attributes.size_bytes)}</span></summary><div className="pt-3"><Evidence evaluation={evaluation} /></div></details>)}</div></div>}
          <section className="space-y-3 border-t border-border pt-5"><h3 className="font-semibold">保存后如何运行</h3><CheckField checked={feed.enabled} onChange={v => changeFeed("enabled", v)}>保存后开启订阅</CheckField><p className="text-sm leading-6 text-muted">首次检查只收集已有资源，之后发现的新资源才会自动下载。已有资源可在订阅内勾选补下。</p>{!feed.enabled && <p className="text-sm text-muted">将保存为已暂停，不会检查或下载；准备好后在列表中开启。</p>}</section>
        </div>}
        <div className="mt-7 space-y-4 border-t border-border pt-5">{error && <ErrorBox>{error}</ErrorBox>}<div className="flex flex-wrap justify-between gap-3"><Button variant="outline" disabled={!!busy} className="h-11" onClick={() => step > 0 ? move(step - 1) : dirty ? setLeave(true) : onBack()}>{step > 0 ? "上一步" : "取消"}</Button>{step < 2 ? <Button className="h-11" disabled={!!busy} onClick={() => move(step + 1)}>下一步：{step === 0 ? "下载条件" : "预览与确认"}<ArrowRight /></Button> : <Button className="h-11" disabled={!!busy} onClick={save}>{busy === "save" && <Loader2 className="motion-safe:animate-spin" />}{feed.enabled ? "保存并开启订阅" : "保存为已暂停"}</Button>}</div></div>
      </div>
      <aside className={cn("space-y-5 text-sm xl:sticky xl:top-6", step !== 2 && "hidden xl:block")} aria-label="订阅设置摘要"><h3 className="font-semibold">你的订阅</h3><dl className="space-y-5"><div><dt className="text-xs text-muted">来源</dt><dd className="mt-1 break-words font-medium">{feed.name || "尚未命名"}</dd><dd className="mt-1 text-xs text-muted">每 {feed.interval_minutes} 分钟检查</dd></div><div><dt className="text-xs text-muted">下载条件</dt><dd className="mt-1 break-words leading-6">{filters.match_all ? "全部标题" : words(include).join(filters.include_mode === "all" ? " + " : " / ") || "待填写关键词"}{filters.free_only && " · 仅免费"}</dd><dd className="mt-1 text-xs leading-5 text-muted">{filters.hr_policy === "require_clear" ? "仅确认无 H&R，未知时等待" : "不筛选 H&R"}</dd></div><div><dt className="text-xs text-muted">下载到</dt><dd className="mt-1">{qb.find(d => d.id === rule.downloader_id)?.name || "待选择下载器"}</dd><dd className="mt-1 break-all text-xs text-muted">{rule.options.save_path || "下载器默认目录"}</dd></div></dl><p className="border-t border-border pt-4 text-xs leading-6 text-muted">只自动下载新发现的资源。每一次筛选和添加结果都可以回看。</p></aside>
    </div>
    {leave && <Dialog open title="放弃本次修改？" description="未保存的订阅设置将丢失。" onClose={() => setLeave(false)} panelClassName="max-w-lg" footer={<div className="flex justify-end gap-2"><Button variant="outline" onClick={() => setLeave(false)}>继续编辑</Button><Button variant="destructive" onClick={onBack}>放弃修改</Button></div>}><div /></Dialog>}
  </div>;
}
