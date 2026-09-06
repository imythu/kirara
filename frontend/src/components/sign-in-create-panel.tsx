import { useEffect, useRef, useState } from "react";
import { Check, Loader2, Search, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select } from "@/components/ui/select";
import { api } from "@/lib/api";
import type { SiteRecord, SignInTaskRecord, SignInTaskRequest } from "@/types";

export type SignInSiteOption = { site: SiteRecord; reason: string; request: SignInTaskRequest | null };
export function SignInCreatePanel({ mode, options, tasks, onClose, onCreated, onManual }: {
  mode: "single" | "batch";
  options: SignInSiteOption[];
  tasks: SignInTaskRecord[];
  onClose: () => void;
  onCreated: (message: string) => void;
  onManual: (site: SiteRecord) => void;
}) {
  const [selected, setSelected] = useState<number[]>([]);
  const [query, setQuery] = useState("");
  const [hours, setHours] = useState("8");
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
  const [created, setCreated] = useState<number[]>([]);
  const [failures, setFailures] = useState<Record<number, string>>({});
  const [feedback, setFeedback] = useState("");
  const lock = useRef(false);
  const allRef = useRef<HTMLInputElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  useEffect(() => { searchRef.current?.focus({ preventScroll: true }); }, []);
  const reasonFor = (option: SignInSiteOption) => created.includes(option.site.id) ? "已创建" : option.reason;
  const visible = options.filter(({ site }) => `${site.name} ${site.base_url}`.toLocaleLowerCase().includes(query.trim().toLocaleLowerCase()))
    .sort((a, b) => Number(Boolean(reasonFor(a))) - Number(Boolean(reasonFor(b))));
  const available = visible.filter(option => !reasonFor(option));
  const chosen = selected.filter(id => options.some(option => option.site.id === id && !reasonFor(option)));
  const allChecked = available.length > 0 && available.every(option => chosen.includes(option.site.id));
  useEffect(() => { if (allRef.current) allRef.current.indeterminate = !allChecked && available.some(option => chosen.includes(option.site.id)); });
  function toggle(option: SignInSiteOption) {
    setFeedback("");
    if (mode === "single") { setSelected([option.site.id]); setName(option.site.name); }
    else setSelected(current => current.includes(option.site.id) ? current.filter(id => id !== option.site.id) : [...current, option.site.id]);
  }
  async function submit() {
    if (lock.current || !chosen.length) return;
    if (mode === "single" && !name.trim()) { setFeedback("请填写任务名称。"); return; }
    lock.current = true; setBusy(true); setFailures({}); setFeedback("");
    const succeeded: number[] = [], failed: Record<number, string> = {};
    const usedNames = new Set(tasks.map(task => task.name));
    for (const id of chosen) {
      const option = options.find(option => option.site.id === id)!;
      let taskName = mode === "single" ? name.trim() : option.site.name.trim();
      if (mode === "batch") {
        const base = taskName || `站点 ${id}`;
        taskName = base;
        let suffix = 1;
        while (usedNames.has(taskName)) taskName = `${base} · ${id}${suffix++ > 1 ? `-${suffix}` : ""}`;
      }
      usedNames.add(taskName);
      try {
        await api("/api/sign-in-tasks", { method: "POST", body: JSON.stringify({ ...option.request, site_id: id, name: taskName, cron_expression: `0 0 0/${hours} * * *` }) });
        succeeded.push(id);
      } catch (error) { failed[id] = error instanceof Error ? error.message : "创建失败，请重试。"; }
    }
    setCreated(current => [...current, ...succeeded]); setSelected(Object.keys(failed).map(Number)); setFailures(failed);
    const message = `已创建 ${succeeded.length} 个签到任务${Object.keys(failed).length ? `，${Object.keys(failed).length} 个创建失败，请查看站点下方原因。` : "，任务已启用。"}`;
    setFeedback(message); setBusy(false); lock.current = false;
    onCreated(message);
  }
  return <section aria-labelledby="sign-in-create-title" className="overflow-hidden rounded-2xl border border-border bg-card">
    <div className="flex items-start justify-between gap-3 border-b border-border p-4 sm:p-6">
      <div><h2 id="sign-in-create-title" className="text-lg font-semibold">{mode === "batch" ? "批量创建签到任务" : "新增签到任务"}</h2><p className="mt-1 text-sm text-muted">{mode === "batch" ? "一次选择多个站点，每个站点分别创建一个任务。" : "选择一个站点，签到方式会自动匹配。"}</p></div>
      <Button variant="outline" className="h-10 w-10 shrink-0 p-0" aria-label="收起创建面板" disabled={busy} onClick={onClose}><X className="h-4 w-4" /></Button>
    </div>
    <fieldset disabled={busy} className="m-0 min-w-0 border-0 p-0">
      <div className="grid lg:grid-cols-[minmax(0,1.2fr)_minmax(0,1fr)]">
        <div className="min-w-0 border-b border-border p-4 sm:p-6 lg:border-b-0 lg:border-r">
          <div className="mb-4 flex items-center justify-between gap-2"><h3 className="text-sm font-semibold">1. 选择站点</h3><span className="text-xs text-muted">{options.filter(option => !reasonFor(option)).length} 个可创建</span></div>
          <div className="relative"><Search className="pointer-events-none absolute left-3 top-3 h-4 w-4 text-muted" /><Input ref={searchRef} aria-label="搜索可创建的站点" className="pl-10" value={query} onChange={e => setQuery(e.target.value)} placeholder="搜索站点名称或网址" /></div>
          <div className="flex min-h-12 items-center justify-between gap-3 border-b border-border text-xs">
            {mode === "batch" ? <label className="flex cursor-pointer items-center gap-2"><input ref={allRef} type="checkbox" className="h-4 w-4 accent-primary" disabled={!available.length || busy} checked={allChecked} onChange={e => setSelected(current => e.target.checked ? [...new Set([...current, ...available.map(option => option.site.id)])] : current.filter(id => !available.some(option => option.site.id === id)))} />全选当前可创建站点</label> : <span className="text-muted">每个站点只能有一个签到任务</span>}
            <span className="shrink-0 text-muted" aria-live="polite">已选 {chosen.length} 个</span>
          </div>
          <div className="max-h-[350px] overflow-y-auto">
            {visible.map(option => { const reason = reasonFor(option); return <div key={option.site.id} className={chosen.includes(option.site.id) ? "border-b border-border bg-accent/60" : "border-b border-border"}>
              <label className={`flex min-h-16 items-center gap-3 px-2 py-3 ${reason ? "text-muted" : "cursor-pointer hover:bg-accent/60"}`}>
                <input type={mode === "batch" ? "checkbox" : "radio"} name="sign-in-create-site" className="h-4 w-4 shrink-0 accent-primary" disabled={Boolean(reason) || busy} checked={chosen.includes(option.site.id)} onChange={() => toggle(option)} />
                <span className="min-w-0 flex-1"><span className="block break-words text-sm font-medium">{option.site.name}</span><span className="mt-1 block truncate text-xs text-muted">{option.site.base_url}</span></span>
                <span className={`max-w-[42%] text-right text-xs ${reason ? "text-muted" : "text-jade"}`}>{reason || "已适配"}</span>
              </label>
              {failures[option.site.id] ? <p role="alert" className="px-3 pb-3 text-xs text-destructive">{failures[option.site.id]}</p> : null}
              {option.reason === "需手动配置" ? <Button variant="outline" className="mb-3 ml-8 h-8 text-xs" onClick={() => onManual(option.site)}>单独配置此站点</Button> : null}
            </div>; })}
            {!visible.length ? <p className="py-8 text-center text-sm text-muted">{options.length ? "没有找到站点，请换个名称试试。" : "暂无 NexusPHP 站点，请先在站点管理中添加。"}</p> : null}
          </div>
          <p className="mt-3 text-xs leading-relaxed text-muted">已有任务（包括已暂停）的站点会自动跳过，避免重复签到。</p>
        </div>
        <div className="min-w-0 space-y-5 p-4 sm:p-6">
          <h3 className="text-sm font-semibold">2. 设置执行方式</h3>
          {mode === "single" ? <div className="space-y-2"><Label htmlFor="sign-in-create-name">任务名称</Label><Input id="sign-in-create-name" value={name} onChange={e => setName(e.target.value)} placeholder="选择站点后自动填写" /></div> : null}
          <div className="space-y-2"><Label htmlFor="sign-in-create-interval">执行间隔</Label><Select id="sign-in-create-interval" disabled={busy} value={hours} onChange={setHours} options={[6, 8, 12, 16, 20, 24].map(value => ({ value: String(value), label: `每 ${value} 小时${value === 8 ? "（默认）" : ""}` }))} /><p className="text-xs leading-relaxed text-muted">按设定间隔尝试签到，执行结果可在日志中查看。</p></div>
          <div className="flex gap-2"><Check className="mt-0.5 h-4 w-4 shrink-0 text-jade" /><div><p className="text-sm font-medium">签到方式，自动匹配</p><p className="mt-1 text-sm leading-relaxed text-muted">沿用各站点已适配的配置，无需逐个填写参数。</p></div></div>
          <details className="text-xs leading-relaxed text-muted"><summary className="cursor-pointer py-2">有些站点为什么不能选择？</summary><p>已有任务的站点无需重复创建；缺少登录凭据请前往站点管理更新。浏览器未配置时，请使用页面上方的“浏览器配置”。尚未适配的站点可单独配置。</p></details>
          <div className="space-y-2 border-t border-border pt-5"><h3 className="text-sm font-semibold">创建后会怎样？</h3><p className="text-sm" aria-live="polite">{chosen.length ? `创建 ${chosen.length} 个任务，每 ${hours} 小时尝试签到一次。` : "选择站点后，这里会显示创建内容。"}</p><p className="text-sm leading-relaxed text-muted">任务将自动启用。你可以随时暂停，也可以手动执行一次。</p></div>
        </div>
      </div>
    </fieldset>
    <div className="border-t border-border bg-surface-container/40 p-4 sm:px-6">
      {feedback ? <p role="status" className="mb-3 text-sm">{feedback}</p> : null}
      <div className="flex flex-wrap items-center justify-between gap-3"><span className="text-xs text-muted">{busy ? "正在逐个创建，请稍候…" : `已选 ${chosen.length} 个站点，将分别创建独立任务`}</span><div className="flex gap-2"><Button variant="outline" onClick={onClose} disabled={busy}>{created.length ? "完成" : "取消"}</Button><Button disabled={busy || !chosen.length} onClick={() => void submit()}>{busy ? <Loader2 className="mr-2 h-4 w-4 animate-spin" /> : null}{busy ? "正在创建…" : chosen.length ? `创建 ${chosen.length} 个签到任务` : "创建签到任务"}</Button></div></div>
    </div>
  </section>;
}
