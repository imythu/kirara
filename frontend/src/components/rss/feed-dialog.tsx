import { useEffect, useRef, useState } from "react";
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
  const continueRef = useRef<HTMLButtonElement>(null);
  const cancelRef = useRef<HTMLButtonElement>(null);
  useEffect(() => { if (showClose) continueRef.current?.focus({ preventScroll: true }); }, [showClose]);
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
    try { setTested(await rssApi.post<RssFeedTest>("/feeds/test", { feed_id: current?.id, url: current ? null : draft.url?.trim() || null, site_id: draft.site_id, use_proxy: draft.use_proxy })); setTestSignature(signature); }
    catch (e) { setError(rssError(e)); } finally { setBusy(null); }
  }
  async function save() {
    const invalid = validate(); setFieldErrors(invalid ? { [invalid.field]: invalid.message } : {}); if (invalid) { setError(invalid.message); return; }
    setBusy("save"); setError("");
    if (request.current.signature !== signature) request.current = { signature, id: newRequestId() };
    const body = { ...draft, name: draft.name.trim(), url: current ? null : draft.url?.trim() || null, request_id: request.current.id };
    try { onSaved(current ? await rssApi.put<RssFeed>(`/feeds/${current.id}`, body) : await rssApi.post<RssFeed>("/feeds", body)); }
    catch (e) { setError(rssError(e)); } finally { setBusy(null); }
  }
  async function reload() {
    if (!current) return;
    setBusy("reload");
    try { const fresh = await rssApi.get<RssFeed>(`/feeds/${current.id}`); setCurrent(fresh); setDraft(initial(fresh)); setTested(null); setError(""); setFieldErrors({}); } catch (e) { setError(rssError(e)); } finally { setBusy(null); }
  }
  return <Dialog open onClose={close} title={current ? "编辑订阅源" : "添加订阅源"} description="添加站点的订阅链接，自动查看新发布的资源。" panelClassName="max-w-2xl" footer={showClose ? <div className="space-y-3" role="alert" aria-label="确认放弃修改">
    <p className="text-sm leading-6">当前有未保存的修改，关闭后将丢弃。</p>
    <div className="flex flex-wrap justify-end gap-2">
      <Button ref={continueRef} type="button" className="h-11 px-3" variant="outline" onClick={() => { setShowClose(false); window.requestAnimationFrame(() => cancelRef.current?.focus({ preventScroll: true })); }}>继续编辑</Button>
      <Button type="button" className="h-11 px-3" variant="destructive" onClick={onClose}>丢弃修改并关闭</Button>
    </div>
  </div> : <div className="space-y-3" aria-label="保存订阅源反馈">{error && <ErrorBox action={current ? <Button type="button" className="h-auto min-h-11 whitespace-normal px-3 text-xs" variant="outline" disabled={!!busy} onClick={reload}>载入最新配置（替换草稿）</Button> : undefined}>{error}</ErrorBox>}<div className="flex flex-wrap justify-end gap-2"><Button ref={cancelRef} type="button" className="h-11 px-4" variant="outline" onClick={close} disabled={!!busy}>取消</Button><Button type="button" className="h-11 px-4" variant="secondary" onClick={test} disabled={!!busy}>{busy === "test" && <Loader2 className="motion-safe:animate-spin" />}测试并预览</Button><Button type="submit" className="h-11 px-4" form="rss-feed-form" disabled={!!busy}>{busy === "save" && <Loader2 className="motion-safe:animate-spin" />}保存订阅源</Button></div></div>}>
    <form noValidate id="rss-feed-form" onSubmit={(event) => { event.preventDefault(); if (!showClose) void save(); }} className="space-y-5 p-4 sm:p-6">
      <Field id="rss-feed-name" error={fieldErrors["rss-feed-name"]} label="订阅源名称" hint="起一个容易辨认的名字，方便以后选择。">
        <Input id="rss-feed-name" autoComplete="off" maxLength={100} required value={draft.name} onChange={(event) => change("name", event.target.value)} placeholder="例如：纪录片订阅" />
      </Field>
      {current ? <div className="space-y-2">
        <p className="text-sm font-semibold">RSS 地址</p>
        <p className="break-all text-sm leading-6 text-muted">{current.url_display}</p>
        <p className="text-xs leading-5 text-muted">地址保存后不可修改。如需使用其他地址，请添加订阅源。</p>
      </div> : <Field id="rss-feed-url" error={fieldErrors["rss-feed-url"]} label="RSS 地址" hint="从站点的 RSS 订阅页面复制完整链接。保存后不可修改，请先测试。" helpTitle="在哪里找到 RSS 地址？" help={<p>打开站点的 RSS 或订阅页面，按需要选择资源分类，再复制生成的订阅链接。这里填写的是订阅链接，不是站点首页或某个资源的详情页。</p>}>
        <Input id="rss-feed-url" type="url" autoComplete="off" spellCheck={false} required value={draft.url ?? ""} onChange={(event) => change("url", event.target.value)} placeholder="https://" />
      </Field>}
      <Field id="rss-feed-site" error={fieldErrors["rss-feed-site"]} label="关联站点" hint="关联对应站点，种子链接失效时可尝试用站点账号重新获取种子文件。" helpTitle="什么时候需要关联站点？" help={<><p>如果读取订阅或下载种子需要登录，请选择对应站点。云母也会尝试从该站点查询免费、做种要求等信息。</p><p>会先使用 RSS 中的种子链接；链接失效且能够识别该种子时，再用站点已保存的登录信息获取同一个种子文件。RSS 和站点 API 可以使用不同域名，无需重复填写账号。</p><p>链接本身就能访问时，可以不关联。列表中没有需要的站点，可先到「站点管理」添加。</p></>}>
        <Select id="rss-feed-site" value={draft.site_id?.toString() ?? ""} onChange={(value) => change("site_id", value ? Number(value) : null)} options={[{ value: "", label: "不关联站点" }, ...sites.map((site) => ({ value: String(site.id), label: site.name }))]} />
      </Field>
      <Field id="rss-feed-interval" error={fieldErrors["rss-feed-interval"]} label="检查间隔（分钟）" hint="多久查看一次新资源。可填 5–1440，通常保持 15 即可。">
        <Input id="rss-feed-interval" type="number" min={5} max={1440} step={1} required value={draft.interval_minutes} onChange={(event) => change("interval_minutes", Number(event.target.value))} />
      </Field>
      <details className="border-t border-border">
        <summary className="min-h-11 cursor-pointer py-3 text-sm font-medium">连接设置（代理）</summary>
        <div className="pb-2 pt-2"><Field id="rss-feed-proxy" error={fieldErrors["rss-feed-proxy"]} label="代理方式" hint={draft.use_proxy === true ? "使用「系统设置」中配置的代理访问订阅源。" : draft.use_proxy === false ? "直接访问订阅源，不经过代理。" : "跟随关联站点的代理设置；未关联站点时直接连接。"}>
          <Select id="rss-feed-proxy" value={draft.use_proxy == null ? "inherit" : draft.use_proxy ? "on" : "off"} onChange={(value) => change("use_proxy", value === "inherit" ? null : value === "on")} options={[{ value: "inherit", label: "跟随关联站点" }, { value: "on", label: "使用全局代理" }, { value: "off", label: "直接连接" }]} />
        </Field></div>
      </details>
      <CheckField checked={draft.enabled} onChange={(value) => change("enabled", value)} hint="关闭后保留订阅源，暂不检查更新。">自动检查更新</CheckField>
      {!current && <p className="border-t border-border pt-4 text-xs leading-6 text-muted">第一次检查会列出已有资源，供你按需勾选补下。创建并启用下载规则后，才会自动下载新出现的资源。</p>}
      {tested && <section className="space-y-3 border-t border-border pt-4" aria-label="订阅源测试结果" aria-live="polite"><h4 className="text-sm font-semibold">{tested.title || "已解析订阅源"} · {tested.item_count} 条</h4><p className="text-xs text-muted">测试时间：{rssDate(tested.sample_time)} · 本次仅查看资源，不会下载</p>{testSignature !== signature && <p className="text-xs text-primary">配置已更改，请重新测试。</p>}{tested.warnings.map((warning, index) => <p key={index} className="text-xs leading-5 text-muted">{warning}</p>)}<ul className="divide-y divide-border">{tested.items.slice(0, 20).map((item, index) => <li key={`${item.item_key}-${index}`} className="py-2 text-sm"><p className="break-words">{item.title}</p><p className="mt-1 text-xs text-muted">{rssSize(item.attributes.size_bytes)}{!item.downloadable && " · 缺少可用种子定位"}</p></li>)}</ul></section>}
    </form>
  </Dialog>;
}
