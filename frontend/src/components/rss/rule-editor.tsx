import { useEffect, useRef, useState } from "react";
import { ArrowLeft, Loader2, Pause } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Select } from "@/components/ui/select";
import { cn } from "@/lib/utils";
import { GIB, newRequestId, rssApi, rssError, ruleInput, type RssFeed, type RssJob, type RssPage, type RssPreview, type RssRule, type RssRuleInput } from "@/lib/rss-api";
import type { DownloaderRecord } from "@/types";
import { CheckField, ErrorBox, Field, OptionalSettings, PreviewPanel, type FormIssue } from "./shared";

const textareaClass = "min-h-24 w-full resize-y rounded-lg border border-border bg-card px-3 py-2 text-sm leading-6 placeholder:text-muted focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary";
const words = (value: string) => value.split(/[\n,，]/).map((word) => word.trim()).filter(Boolean);
function initial(rule: RssRule | null, feedId?: number): RssRuleInput {
  return rule ? ruleInput(rule) : { name: "", enabled: true, priority: 100, feed_ids: feedId ? [feedId] : [], filters: { include: [], include_mode: "all", exclude: [], include_regex: null, exclude_regex: null, match_all: false, min_size_bytes: null, max_size_bytes: null, min_seeders: null, free_only: false, hr_policy: "require_clear" }, downloader_id: null, options: { save_path: null, category: null, tags: [], paused: false, reserve_space_bytes: 0 } };
}

export function RuleEditor({ rule, feeds, downloaders, feedId, onBack, onSaved }: { rule: RssRule | null; feeds: RssFeed[]; downloaders: DownloaderRecord[]; feedId?: number; onBack: () => void; onSaved: (rule: RssRule) => void }) {
  const [current, setCurrent] = useState(rule);
  const [draft, setDraft] = useState(() => initial(rule, feedId));
  const [include, setInclude] = useState(rule?.filters.include.join("\n") ?? "");
  const [exclude, setExclude] = useState(rule?.filters.exclude.join("\n") ?? "");
  const [allTitles, setAllTitles] = useState(!!rule?.filters.match_all && rule.filters.include.length === 0 && !rule.filters.include_regex?.trim());
  const [tags, setTags] = useState(rule?.options.tags.join(", ") ?? "");
  const [panel, setPanel] = useState<"config" | "preview">("config");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [fieldErrors, setFieldErrors] = useState<Record<string, string>>({});
  const [previewError, setPreviewError] = useState("");
  const [previewBusy, setPreviewBusy] = useState(false);
  const [preview, setPreview] = useState<RssPreview | null>(null);
  const [previewSignature, setPreviewSignature] = useState("");
  const [showLeave, setShowLeave] = useState(false);
  const [pendingCount, setPendingCount] = useState<number | null>(null);
  const request = useRef({ signature: "", id: newRequestId() });
  const previewSequence = useRef(0);
  const autoPreviewTimer = useRef<number | undefined>(undefined);
  const saveFeedbackRef = useRef<HTMLDivElement>(null);
  const body: RssRuleInput = { ...draft, filters: { ...draft.filters, include: allTitles ? [] : words(include), include_regex: allTitles ? null : draft.filters.include_regex, exclude: words(exclude) }, options: { ...draft.options, tags: words(tags) } };
  const signature = JSON.stringify(body);
  const dirty = signature !== JSON.stringify(initial(current, feedId));
  const qb = downloaders.filter((downloader) => ["qbittorrent", "qb"].includes(downloader.downloader_type.toLowerCase()));

  useEffect(() => {
    if (!current) { setPendingCount(0); return; }
    let cancelled = false;
    Promise.all(["queued", "held", "fetching", "waiting", "retry_wait"].map((status) => rssApi.get<RssPage<RssJob>>(`/downloads?rule_id=${current.id}&status=${status}&page_size=1`))).then((pages) => { if (!cancelled) setPendingCount(pages.reduce((count, page) => count + page.total, 0)); }).catch(() => { if (!cancelled) setPendingCount(null); });
    return () => { cancelled = true; };
  }, [current?.id]);
  useEffect(() => { const handler = (event: BeforeUnloadEvent) => { if (dirty) event.preventDefault(); }; window.addEventListener("beforeunload", handler); return () => window.removeEventListener("beforeunload", handler); }, [dirty]);
  useEffect(() => {
    if (!error || document.getElementById("rss-rule-form")?.contains(document.activeElement)) return;
    const frame = window.requestAnimationFrame(() => saveFeedbackRef.current?.scrollIntoView({ block: "nearest", behavior: "instant" }));
    return () => window.cancelAnimationFrame(frame);
  }, [error]);
  useEffect(() => {
    if (validate(false)) return;
    const timeout = window.setTimeout(() => void runPreview(), 700);
    autoPreviewTimer.current = timeout;
    return () => window.clearTimeout(timeout);
  }, [signature]);
  function setField<K extends keyof RssRuleInput>(key: K, value: RssRuleInput[K]) { setDraft((previous) => ({ ...previous, [key]: value })); }
  function setFilter<K extends keyof RssRuleInput["filters"]>(key: K, value: RssRuleInput["filters"][K]) { setDraft((previous) => ({ ...previous, filters: { ...previous.filters, [key]: value } })); }
  function setOption<K extends keyof RssRuleInput["options"]>(key: K, value: RssRuleInput["options"][K]) { setDraft((previous) => ({ ...previous, options: { ...previous.options, [key]: value } })); }
  function validate(saving: boolean, enabled = false): FormIssue | null {
    if (saving && !body.name.trim()) return { field: "rss-rule-name", message: "请填写规则名称。" };
    if (body.feed_ids.length === 0) return { field: "rss-rule-feeds", message: "请至少选择一个订阅源。" };
    if (!body.filters.match_all && body.filters.include.length === 0 && body.filters.exclude.length === 0 && !body.filters.include_regex?.trim() && !body.filters.exclude_regex?.trim()) return { field: "rss-rule-include", message: "请填写关键词或正则；要接收所有标题，请勾选“接收所有标题”。" };
    if (body.filters.include.length > 50 || body.filters.include.some((word) => Array.from(word).length > 100)) return { field: "rss-rule-include", message: "包含词最多 50 个，每个最多 100 字符。" };
    if (body.filters.exclude.length > 50 || body.filters.exclude.some((word) => Array.from(word).length > 100)) return { field: "rss-rule-exclude", message: "排除词最多 50 个，每个最多 100 字符。" };
    for (const [field, value] of [["rss-min-size", body.filters.min_size_bytes], ["rss-max-size", body.filters.max_size_bytes], ["rss-reserve-space", body.options.reserve_space_bytes]] as const) {
      if (value != null && (!Number.isSafeInteger(value) || value < 0)) return { field, message: "资源大小和保留空间须为有效的非负数。" };
    }
    if (body.filters.min_seeders != null && (!Number.isInteger(body.filters.min_seeders) || body.filters.min_seeders < 0)) return { field: "rss-min-seeders", message: "最低做种数须为非负整数。" };
    if (!Number.isInteger(body.priority) || body.priority < -2147483648 || body.priority > 2147483647) return { field: "rss-rule-priority", message: "优先级须为有效整数。" };
    if (body.filters.min_size_bytes != null && body.filters.max_size_bytes != null && body.filters.min_size_bytes > body.filters.max_size_bytes) return { field: "rss-min-size", message: "最小大小不能大于最大大小。" };
    if (saving && enabled && !body.downloader_id) return { field: "rss-rule-downloader", message: "启用规则前请选择 qBittorrent 下载器，也可以先仅保存，暂不启用。" };
    return null;
  }
  async function runPreview(refresh = false) {
    if (refresh) window.clearTimeout(autoPreviewTimer.current);
    const invalid = validate(false); if (invalid) { setFieldErrors({ [invalid.field]: invalid.message }); setPreviewError(invalid.message); setPanel("preview"); return; }
    const sequence = ++previewSequence.current;
    setPreviewBusy(true); setPreviewError("");
    try { const result = await rssApi.post<RssPreview>("/rules/preview", { rule: { ...body, name: body.name.trim() || "预览规则", enabled: false }, item_ids: [], refresh_samples: refresh }); if (sequence === previewSequence.current) { setPreview(result); setPreviewSignature(signature); } }
    catch (e) { if (sequence === previewSequence.current) setPreviewError(rssError(e)); }
    finally { if (sequence === previewSequence.current) setPreviewBusy(false); }
  }
  async function save(enabled: boolean) {
    const invalid = validate(true, enabled); setFieldErrors(invalid ? { [invalid.field]: invalid.message } : {}); if (invalid) { setError(invalid.message); return; }
    setBusy(true); setError("");
    const payload = { ...body, name: body.name.trim(), enabled };
    const key = JSON.stringify(payload);
    if (request.current.signature !== key) request.current = { signature: key, id: newRequestId() };
    try { const saved = current ? await rssApi.put<RssRule>(`/rules/${current.id}`, { ...payload, request_id: request.current.id }) : await rssApi.post<RssRule>("/rules", { ...payload, request_id: request.current.id }); onSaved(saved); }
    catch (e) { setError(rssError(e)); } finally { setBusy(false); }
  }
  async function reload() {
    if (!current) return;
    setBusy(true);
    try { const fresh = await rssApi.get<RssRule>(`/rules/${current.id}`); setCurrent(fresh); setDraft(ruleInput(fresh)); setAllTitles(fresh.filters.match_all && fresh.filters.include.length === 0 && !fresh.filters.include_regex?.trim()); setInclude(fresh.filters.include.join("\n")); setExclude(fresh.filters.exclude.join("\n")); setTags(fresh.options.tags.join(", ")); setError(""); setFieldErrors({}); setPreview(null); } catch (e) { setError(rssError(e)); } finally { setBusy(false); }
  }
  async function pauseExisting() {
    if (!current) return;
    setBusy(true);
    try { const paused = await rssApi.post<RssRule>(`/rules/${current.id}/pause`, { expected_version: current.version, request_id: newRequestId() }); setCurrent(paused); setDraft((previous) => ({ ...previous, expected_version: paused.version, enabled: false })); } catch (e) { setError(rssError(e)); } finally { setBusy(false); }
  }
  return <div className="space-y-5">
    <div className="flex flex-wrap items-start justify-between gap-3"><div><Button type="button" className="-ml-3 h-11 border-transparent bg-transparent px-3" variant="outline" onClick={() => dirty ? setShowLeave(true) : onBack()} disabled={busy}><ArrowLeft />返回下载规则</Button><h2 className="mt-2 text-xl font-semibold">{current ? `编辑规则 · ${current.name}` : "新建下载规则"}</h2><p className="mt-2 text-sm leading-6 text-muted">选择想要的资源条件和下载位置。</p></div>{current && <span className="pt-4 text-xs text-muted">{current.enabled ? "已启用" : "已暂停"}</span>}</div>
    {showLeave && <div className="flex flex-wrap items-center gap-3 rounded-xl border border-border bg-card p-4" role="alert"><p className="flex-1 text-sm">当前修改尚未保存。</p><Button type="button" className="h-11 px-3" variant="outline" onClick={() => setShowLeave(false)}>继续编辑</Button><Button type="button" className="h-11 px-3" variant="destructive" onClick={onBack}>丢弃修改并返回</Button></div>}
    <div className="flex rounded-lg border border-border bg-card p-1 xl:hidden" role="tablist" aria-label="规则编辑面板">{([{ value: "config", label: "规则配置" }, { value: "preview", label: "匹配预览" }] as const).map((tab) => <button key={tab.value} className={cn("min-h-11 flex-1 rounded-md px-4 text-sm font-medium", panel === tab.value ? "bg-accent text-primary" : "text-muted")} role="tab" aria-selected={panel === tab.value} onClick={() => setPanel(tab.value)}>{tab.label}</button>)}</div>
    <div className="grid items-start gap-5 xl:grid-cols-2">
      <form noValidate id="rss-rule-form" onSubmit={(event) => { event.preventDefault(); void save(true); }} className={cn("min-w-0 space-y-7 rounded-xl border border-border bg-card p-4 sm:p-5", panel !== "config" && "hidden xl:block")}>
        <section className="space-y-5" aria-labelledby="rss-rule-source-heading">
          <h3 id="rss-rule-source-heading" className="font-semibold">从哪里找资源</h3>
          <Field id="rss-rule-name" error={fieldErrors["rss-rule-name"]} label="规则名称" hint="给这组下载条件起个名字，方便以后查找。">
            <Input id="rss-rule-name" autoComplete="off" maxLength={100} required value={draft.name} onChange={(event) => setField("name", event.target.value)} placeholder="例如：1080p 纪录片" />
          </Field>
          <Field id="rss-rule-feeds" error={fieldErrors["rss-rule-feeds"]} label="订阅源" hint="这条规则从哪些订阅中挑选资源，可以多选。">
            <Select id="rss-rule-feeds" multiple searchable value={draft.feed_ids.map(String)} onChange={(values) => setField("feed_ids", values.map(Number))} options={feeds.map((feed) => ({ value: String(feed.id), label: `${feed.name}${feed.enabled ? "" : "（已暂停）"}` }))} placeholder="选择订阅源" />
          </Field>
          {feeds.length === 0 && <p className="text-sm text-muted">请先返回订阅源页签，添加站点的 RSS 链接。</p>}
        </section>
        <section className="space-y-5 border-t border-border pt-5" aria-labelledby="rss-rule-filter-heading">
          <div><h3 id="rss-rule-filter-heading" className="font-semibold">下载哪些资源</h3><p className="mt-2 text-xs leading-5 text-muted">资源需要同时满足你设置的各项条件。</p></div>
          <CheckField checked={allTitles} onChange={(value) => { setAllTitles(value); setFilter("match_all", value); }} hint="不要求标题包含特定关键词，仍按排除词和下面的条件筛选。">接收所有标题</CheckField>
          {!allTitles && <>
            <Field id="rss-rule-include" error={fieldErrors["rss-rule-include"]} label="标题包含" hint="填写想要的片名、发布组或清晰度。每行一个词，也可用逗号分隔。" helpTitle="关键词怎么填写？" help={<><p>例如要找 1080p 纪录片，可以分两行填写「纪录片」和「1080p」。关键词只在资源标题中查找，不区分大小写。</p><p>「全部满足」要求每个词都出现在标题里；「任意满足」只需出现其中一个。排除词命中任何一个，都会跳过该资源。</p></>}>
              <textarea id="rss-rule-include" className={textareaClass} value={include} onChange={(event) => setInclude(event.target.value)} placeholder="例如：纪录片&#10;1080p" />
            </Field>
            {(words(include).length > 1 || draft.filters.include_mode === "any") && <Field id="rss-rule-include-mode" error={fieldErrors["rss-rule-include-mode"]} label="多个包含词如何匹配">
              <Select id="rss-rule-include-mode" value={draft.filters.include_mode} onChange={(value) => setFilter("include_mode", value)} options={[{ value: "all", label: "全部满足：每个词都要有" }, { value: "any", label: "任意满足：有一个就可以" }]} />
            </Field>}
          </>}
          <Field id="rss-rule-exclude" error={fieldErrors["rss-rule-exclude"]} label="标题不能包含（选填）" hint="填写不想要的内容，出现任意一个词就跳过。每行一个词或用逗号分隔。">
            <textarea id="rss-rule-exclude" className={textareaClass} value={exclude} onChange={(event) => setExclude(event.target.value)} placeholder="例如：预告片&#10;花絮" />
          </Field>
          <CheckField checked={draft.filters.free_only} onChange={(value) => setFilter("free_only", value)} hint="免费指不计入站点的下载流量。勾选后，无法确认免费的资源会等待确认。">只下载免费资源</CheckField>
          <Field id="rss-hr-policy" error={fieldErrors["rss-hr-policy"]} label="做种要求（H&R）" hint="默认跳过有 H&R 的资源；站点未提供信息时，先等待确认。" helpTitle="H&R 是什么？" help={<><p>有些资源要求下载后继续上传分享，达到规定的做种时间或分享率，否则可能被站点记为 H&R（下载后未完成做种要求）。</p><p>默认只下载明确标注无 H&R 的资源。选择「不按 H&R 筛选」后，有要求或信息未知的资源也可通过这一项，需要自行遵守站点要求。</p><p>如果预览显示信息不足，可检查订阅源是否关联了对应站点，以便查询更多信息。</p></>}>
            <Select id="rss-hr-policy" value={draft.filters.hr_policy} onChange={(value) => setFilter("hr_policy", value)} options={[{ value: "require_clear", label: "只下载确认无 H&R 的资源" }, { value: "any", label: "不按 H&R 筛选" }]} />
          </Field>
          <OptionalSettings key={`文件大小与做种人数（选填）-${current?.version ?? "new"}`} title="文件大小与做种人数（选填）" defaultOpen={draft.filters.min_size_bytes != null || draft.filters.max_size_bytes != null || draft.filters.min_seeders != null}>
              <p className="text-xs leading-5 text-muted">留空表示不限。大小按资源内所有文件的总和计算，1 GiB = 1024 MiB。</p>
              <div className="grid gap-4 sm:grid-cols-2">
                <Field id="rss-min-size" error={fieldErrors["rss-min-size"]} label="最小大小（GiB）" hint="小于这个大小的资源会跳过。"><Input id="rss-min-size" type="number" min={0} step="any" value={draft.filters.min_size_bytes == null ? "" : draft.filters.min_size_bytes / GIB} onChange={(event) => setFilter("min_size_bytes", event.target.value === "" ? null : Math.round(Number(event.target.value) * GIB))} placeholder="不限" /></Field>
                <Field id="rss-max-size" error={fieldErrors["rss-max-size"]} label="最大大小（GiB）" hint="大于这个大小的资源会跳过。"><Input id="rss-max-size" type="number" min={0} step="any" value={draft.filters.max_size_bytes == null ? "" : draft.filters.max_size_bytes / GIB} onChange={(event) => setFilter("max_size_bytes", event.target.value === "" ? null : Math.round(Number(event.target.value) * GIB))} placeholder="不限" /></Field>
              </div>
              <Field id="rss-min-seeders" error={fieldErrors["rss-min-seeders"]} label="最低做种人数" hint="做种者是拥有完整资源、正在分享的人。例如填 1，表示至少有 1 人做种才下载。">
                <Input id="rss-min-seeders" type="number" min={0} step={1} value={draft.filters.min_seeders ?? ""} onChange={(event) => setFilter("min_seeders", event.target.value === "" ? null : Number(event.target.value))} placeholder="不限" />
              </Field>
            </OptionalSettings>
          <OptionalSettings key={`高级标题匹配（正则）-${current?.version ?? "new"}`} title="高级标题匹配（正则）" defaultOpen={!!body.filters.include_regex || !!body.filters.exclude_regex}>
              <p className="text-xs leading-6 text-muted">用于更复杂的标题筛选，不熟悉可以留空。例如 1080p|2160p 表示匹配两种清晰度中的任意一种。</p>
              {!allTitles && <Field id="rss-include-regex" error={fieldErrors["rss-include-regex"]} label="包含正则" hint="标题还必须满足这个表达式；已填的包含词也要满足。"><Input id="rss-include-regex" maxLength={512} spellCheck={false} value={draft.filters.include_regex ?? ""} onChange={(event) => setFilter("include_regex", event.target.value || null)} placeholder="例如：1080p|2160p" /></Field>}
              <Field id="rss-exclude-regex" error={fieldErrors["rss-exclude-regex"]} label="排除正则" hint="标题满足这个表达式就跳过。"><Input id="rss-exclude-regex" maxLength={512} spellCheck={false} value={draft.filters.exclude_regex ?? ""} onChange={(event) => setFilter("exclude_regex", event.target.value || null)} placeholder="例如：CAMRip|HDTS" /></Field>
            </OptionalSettings>
        </section>
        <section className="space-y-5 border-t border-border pt-5" aria-labelledby="rss-rule-target-heading">
          <h3 id="rss-rule-target-heading" className="font-semibold">下载到哪里</h3>
          <Field id="rss-rule-downloader" error={fieldErrors["rss-rule-downloader"]} label="qBittorrent 下载器" hint="符合条件的资源会自动添加到这里。">
            <Select id="rss-rule-downloader" value={draft.downloader_id?.toString() ?? ""} onChange={(value) => setField("downloader_id", value ? Number(value) : null)} options={[{ value: "", label: "选择下载器" }, ...qb.map((downloader) => ({ value: String(downloader.id), label: downloader.name }))]} />
          </Field>
          {qb.length === 0 && <p className="text-sm leading-6 text-muted">还没有下载器。可先保存规则，再到<a className="text-primary underline" href="#/downloaders">下载器设置</a>添加 qBittorrent。</p>}
          <Field id="rss-save-path" error={fieldErrors["rss-save-path"]} label="保存目录（选填）" hint="留空使用下载器默认目录。填写下载器所在设备上的文件夹路径。" helpTitle="保存目录该怎么填？" help={<p>例如 qBittorrent 的下载文件夹是 /downloads，就填写 /downloads。使用 Docker 时，填写 qBittorrent 容器内的路径。拿不准时留空即可。</p>}>
            <Input id="rss-save-path" value={draft.options.save_path ?? ""} onChange={(event) => setOption("save_path", event.target.value || null)} placeholder="使用下载器默认目录" />
          </Field>
          <OptionalSettings key={`分类、标签与添加方式（选填）-${current?.version ?? "new"}`} title="分类、标签与添加方式（选填）" defaultOpen={!!draft.options.category || draft.options.tags.length > 0 || draft.options.paused}>
              <Field id="rss-category" error={fieldErrors["rss-category"]} label="分类" hint="在 qBittorrent 中给资源分组，方便统一管理；留空不指定。"><Input id="rss-category" value={draft.options.category ?? ""} onChange={(event) => setOption("category", event.target.value || null)} placeholder="例如：纪录片" /></Field>
              <Field id="rss-tags" error={fieldErrors["rss-tags"]} label="标签" hint="给资源加上便于查找的标记。可填多个，用逗号分隔。"><Input id="rss-tags" value={tags} onChange={(event) => setTags(event.target.value)} placeholder="例如：rss, 纪录片" /></Field>
              <CheckField checked={draft.options.paused} onChange={(value) => setOption("paused", value)} hint="先把种子添加到 qBittorrent，等你手动开始下载。">添加后先暂停</CheckField>
            </OptionalSettings>
        </section>
        <OptionalSettings key={`更多执行设置（优先级、磁盘空间）-${current?.version ?? "new"}`} title="更多执行设置（优先级、磁盘空间）" defaultOpen={draft.priority !== 100 || draft.options.reserve_space_bytes > 0}>
            <Field id="rss-rule-priority" error={fieldErrors["rss-rule-priority"]} label="规则优先级" hint="数字越小越优先。只有一条规则时，保持默认 100 即可。" helpTitle="多条规则都符合时，怎么选？" help={<p>同一资源发往同一下载器时，优先使用数字更小的符合规则。例如 10 优先于 100，保存目录、分类等也使用该规则的设置。这个数字不影响 qBittorrent 内部的下载顺序。</p>}>
              <Input id="rss-rule-priority" type="number" step={1} min={-2147483648} max={2147483647} required value={draft.priority} onChange={(event) => setField("priority", Number(event.target.value))} />
            </Field>
            <Field id="rss-reserve-space" error={fieldErrors["rss-reserve-space"]} label="至少保留可用空间（GiB）" hint="例如填 20，表示为磁盘留出 20 GiB。空间不够时，等待有空间再添加任务。" helpTitle="剩余空间是怎么计算的？" help={<><p>系统会从可用空间中扣除未完成任务和等待添加的任务所需空间，再计算本次下载后能否留下你设置的余量。</p><p>填 0 表示不额外预留，仍会检查空间是否足够。本项使用 qBittorrent 报告的磁盘空间；自定义目录在另一块磁盘时，需自行确认该磁盘的剩余空间。</p></>}>
              <Input id="rss-reserve-space" type="number" min={0} step="any" value={draft.options.reserve_space_bytes / GIB} onChange={(event) => setOption("reserve_space_bytes", Math.round(Number(event.target.value) * GIB))} />
            </Field>
          </OptionalSettings>
        {current && (pendingCount == null || pendingCount > 0) && <div className="space-y-3 border-t border-border pt-5 text-xs leading-6 text-muted"><p>{pendingCount == null ? "暂时无法获取等待添加的任务数量。" : `此规则还有 ${pendingCount} 个任务等待添加到下载器。`}这些任务继续使用原来的设置；本次修改用于之后的新资源。</p>{current.enabled && <Button type="button" className="h-11 px-3" variant="outline" disabled={busy} onClick={pauseExisting}><Pause />暂停此规则</Button>}</div>}
      </form>
      <div className={cn("min-w-0 xl:sticky xl:top-4", panel !== "preview" && "hidden xl:block")}><PreviewPanel preview={preview} busy={previewBusy} stale={!!preview && previewSignature !== signature} onPreview={() => void runPreview()} onRefresh={() => void runPreview(true)} error={previewError} /></div>
    </div>
    <div ref={saveFeedbackRef} role="region" aria-label="保存规则" className="scroll-mb-28 space-y-3 rounded-xl lg:scroll-mb-4 border border-border bg-card p-4">{error && <ErrorBox action={current ? <Button type="button" className="h-auto min-h-11 whitespace-normal px-3 text-xs" variant="outline" disabled={busy} onClick={reload}>载入最新配置（替换草稿）</Button> : undefined}>{error}</ErrorBox>}<div className="flex flex-wrap items-center justify-between gap-3"><p className="max-w-xl text-xs leading-6 text-muted">启用后自动下载新发现的资源。已有资源可在订阅源的资源列表中勾选补下。</p><div className="flex flex-wrap gap-2"><Button type="button" className="h-11 px-4" variant="outline" onClick={() => void save(false)} disabled={busy}>{busy && <Loader2 className="motion-safe:animate-spin" />}仅保存，暂不启用</Button><Button type="button" className="h-11 px-4" onClick={() => void save(true)} disabled={busy || !draft.downloader_id}>{busy && <Loader2 className="motion-safe:animate-spin" />}保存并启用</Button></div></div></div>
  </div>;
}
