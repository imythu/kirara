import { useRef, useState } from "react";
import { Loader2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Dialog } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Select } from "@/components/ui/select";
import { newRequestId, rssApi, rssDate, rssError, rssSize, type RssFeed, type RssFeedInput, type RssFeedTest } from "@/lib/rss-api";
import type { SiteRecord } from "@/types";
import { CheckField, ErrorBox, Field, type FormIssue } from "./shared";

function initial(feed: RssFeed | null): RssFeedInput {
  return { name: feed?.name ?? "", url: "", site_id: feed?.site_id ?? null, use_proxy: feed?.use_proxy ?? null, enabled: feed?.enabled ?? true, interval_minutes: feed?.interval_minutes ?? 15, expected_version: feed?.version };
}

export function FeedDialog({ feed, sites, onClose, onSaved }: { feed: RssFeed | null; sites: SiteRecord[]; onClose: () => void; onSaved: (feed: RssFeed) => void }) {
  const [draft, setDraft] = useState(() => initial(feed));
  const [current, setCurrent] = useState(feed);
  const [busy, setBusy] = useState<"save" | "test" | "reload" | null>(null);
  const [error, setError] = useState("");
  const [fieldErrors, setFieldErrors] = useState<Record<string, string>>({});
  const [tested, setTested] = useState<RssFeedTest | null>(null);
  const [testSignature, setTestSignature] = useState("");
  const [showClose, setShowClose] = useState(false);
  const request = useRef({ signature: "", id: newRequestId() });
  const signature = JSON.stringify(draft);
  const dirty = signature !== JSON.stringify(initial(current));
  function change<K extends keyof RssFeedInput>(key: K, value: RssFeedInput[K]) { setDraft((previous) => ({ ...previous, [key]: value })); }
  function close() { if (busy) return; if (dirty) setShowClose(true); else onClose(); }
  function validate(): FormIssue | null {
    if (!draft.name.trim()) return { field: "rss-feed-name", message: "请填写订阅源名称。" };
    if (!current && !draft.url?.trim()) return { field: "rss-feed-url", message: "请填写 RSS 地址。" };
    if (draft.url?.trim()) { try { const url = new URL(draft.url.trim()); if (!["http:", "https:"].includes(url.protocol) || url.username || url.password) return { field: "rss-feed-url", message: "RSS 地址须为 HTTP 或 HTTPS，且不能包含用户名和密码。" }; } catch { return { field: "rss-feed-url", message: "请输入完整的 RSS 地址，例如 https://站点地址/rss。" }; } }
    if (!Number.isInteger(draft.interval_minutes) || draft.interval_minutes < 5 || draft.interval_minutes > 1440) return { field: "rss-feed-interval", message: "检查间隔应为 5–1440 分钟的整数。" };
    return null;
  }
  async function test() {
    const invalid = validate(); setFieldErrors(invalid ? { [invalid.field]: invalid.message } : {}); if (invalid) { setError(invalid.message); return; }
    setBusy("test"); setError("");
    try { setTested(await rssApi.post<RssFeedTest>("/feeds/test", { feed_id: current?.id, url: draft.url?.trim() || null, site_id: draft.site_id, use_proxy: draft.use_proxy })); setTestSignature(signature); }
    catch (e) { setError(rssError(e)); } finally { setBusy(null); }
  }
  async function save() {
    const invalid = validate(); setFieldErrors(invalid ? { [invalid.field]: invalid.message } : {}); if (invalid) { setError(invalid.message); return; }
    setBusy("save"); setError("");
    if (request.current.signature !== signature) request.current = { signature, id: newRequestId() };
    const body = { ...draft, name: draft.name.trim(), url: draft.url?.trim() || null, request_id: request.current.id };
    try { onSaved(current ? await rssApi.put<RssFeed>(`/feeds/${current.id}`, body) : await rssApi.post<RssFeed>("/feeds", body)); }
    catch (e) { setError(rssError(e)); } finally { setBusy(null); }
  }
  async function reload() {
    if (!current) return;
    setBusy("reload");
    try { const fresh = await rssApi.get<RssFeed>(`/feeds/${current.id}`); setCurrent(fresh); setDraft(initial(fresh)); setTested(null); setError(""); setFieldErrors({}); } catch (e) { setError(rssError(e)); } finally { setBusy(null); }
  }
  return <Dialog open onClose={close} title={current ? "编辑订阅源" : "添加订阅源"} description="收集 RSS 中的新种子，再由下载规则决定哪些资源需要下载。" panelClassName="max-w-2xl" footer={<div className="space-y-3" aria-label="保存订阅源反馈">{error && <ErrorBox action={current ? <Button type="button" className="h-auto min-h-11 whitespace-normal px-3 text-xs" variant="outline" disabled={!!busy} onClick={reload}>载入最新配置（替换草稿）</Button> : undefined}>{error}</ErrorBox>}<div className="flex flex-wrap justify-end gap-2"><Button type="button" className="h-11 px-4" variant="outline" onClick={close} disabled={!!busy}>取消</Button><Button type="button" className="h-11 px-4" variant="secondary" onClick={test} disabled={!!busy}>{busy === "test" && <Loader2 className="motion-safe:animate-spin" />}测试并预览</Button><Button type="submit" className="h-11 px-4" form="rss-feed-form" disabled={!!busy}>{busy === "save" && <Loader2 className="motion-safe:animate-spin" />}保存订阅源</Button></div></div>}>
    <form noValidate id="rss-feed-form" onSubmit={(event) => { event.preventDefault(); void save(); }} className="space-y-5 p-4 sm:p-6">
      {showClose && <div role="alert" className="rounded-xl border border-border bg-accent p-4 text-sm"><p>当前有未保存的修改，关闭后将丢弃。</p><div className="mt-3 flex gap-2"><Button type="button" className="h-11 px-3" variant="outline" onClick={() => setShowClose(false)}>继续编辑</Button><Button type="button" className="h-11 px-3" variant="destructive" onClick={onClose}>丢弃修改并关闭</Button></div></div>}
      <Field id="rss-feed-name" error={fieldErrors["rss-feed-name"]} label="订阅源名称"><Input id="rss-feed-name" autoComplete="off" maxLength={100} required value={draft.name} onChange={(event) => change("name", event.target.value)} /></Field>
      <Field id="rss-feed-url" error={fieldErrors["rss-feed-url"]} label={current ? "替换 RSS 地址（留空保留）" : "RSS 地址"} hint="支持 RSS 2.0 / Torznab 种子订阅；首版不处理 Atom 或磁力链接。"><Input id="rss-feed-url" type="url" autoComplete="off" spellCheck={false} required={!current} aria-describedby="rss-feed-url-hint" value={draft.url ?? ""} onChange={(event) => change("url", event.target.value)} placeholder={current ? "留空使用已保存地址" : "https://"} />{current && <p className="break-all text-xs leading-5 text-muted">已保存：{current.url_display}</p>}</Field>
      {current && draft.url?.trim() && <p className="rounded-lg bg-accent p-3 text-sm leading-6">更换地址后将重新建立历史基线。当前待处理 {current.pending_count} 条；旧来源的未提交任务将挂起，只能查看或取消。</p>}
      <div className="grid gap-5 sm:grid-cols-2"><Field id="rss-feed-site" error={fieldErrors["rss-feed-site"]} label="关联站点" hint="凭据来自站点配置，地址须与站点同源。"><Select id="rss-feed-site" value={draft.site_id?.toString() ?? ""} onChange={(value) => change("site_id", value ? Number(value) : null)} options={[{ value: "", label: "不关联站点" }, ...sites.map((site) => ({ value: String(site.id), label: site.name }))]} /></Field><Field id="rss-feed-interval" error={fieldErrors["rss-feed-interval"]} label="检查间隔（分钟）"><Input id="rss-feed-interval" type="number" min={5} max={1440} step={1} required value={draft.interval_minutes} onChange={(event) => change("interval_minutes", Number(event.target.value))} /></Field></div>
      <details className="rounded-lg border border-border px-4"><summary className="min-h-11 cursor-pointer py-3 text-sm font-medium">高级设置与首次处理</summary><div className="space-y-4 pb-4"><Field id="rss-feed-proxy" error={fieldErrors["rss-feed-proxy"]} label="代理方式"><Select id="rss-feed-proxy" value={draft.use_proxy == null ? "inherit" : draft.use_proxy ? "on" : "off"} onChange={(value) => change("use_proxy", value === "inherit" ? null : value === "on")} options={[{ value: "inherit", label: "继承站点 / 全局配置" }, { value: "on", label: "使用全局代理" }, { value: "off", label: "直接连接" }]} /></Field><p className="text-xs leading-6 text-muted">首次成功检查仅建立历史基线，已有条目不会自动下载。保存后可创建规则，历史内容需在条目列表中单独选择补下。</p></div></details>
      <CheckField checked={draft.enabled} onChange={(value) => change("enabled", value)}>保存后启用订阅源检查</CheckField>
      {tested && <section className="space-y-3 border-t border-border pt-4" aria-label="订阅源测试结果" aria-live="polite"><h4 className="text-sm font-semibold">{tested.title || "已解析订阅源"} · {tested.item_count} 条</h4><p className="text-xs text-muted">测试时间：{rssDate(tested.sample_time)} · 未写入历史或下载队列</p>{testSignature !== signature && <p className="text-xs text-primary">配置已更改，请重新测试。</p>}{tested.warnings.map((warning, index) => <p key={index} className="text-xs leading-5 text-muted">{warning}</p>)}<ul className="divide-y divide-border">{tested.items.slice(0, 20).map((item, index) => <li key={`${item.item_key}-${index}`} className="py-2 text-sm"><p className="break-words">{item.title}</p><p className="mt-1 text-xs text-muted">{rssSize(item.attributes.size_bytes)}{!item.downloadable && " · 缺少可用种子定位"}</p></li>)}</ul></section>}
    </form>
  </Dialog>;
}
