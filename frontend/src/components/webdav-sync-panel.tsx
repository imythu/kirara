import { useEffect, useRef, useState } from "react";
import { Check, Copy, Loader2, RefreshCw } from "lucide-react";
import { api } from "@/lib/api";
import { isDesktop } from "@/lib/desktop";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select } from "@/components/ui/select";

interface Config {
  enabled: boolean; username: string;
  password_configured: boolean; existing_policy: string; auto_create: boolean;
  running: boolean; runtime_error: string | null; last_received_at: number | null;
  new_password?: string;
}
interface Run {
  id: number; name: string; received_at: number; status: string; error: string | null; can_retry: boolean;
  result: { created: number; updated: number; unchanged: number; skipped: number; details: string[] } | null;
}
type Form = Pick<Config, "enabled" | "username" | "existing_policy" | "auto_create">;
const endpoint = "/api/sites/webdav-sync";
const formatTime = (value: number | null) => value == null ? "尚未接收" : new Date(value).toLocaleString();
const statuses: Record<string, string> = { pending: "等待同步", retry: "等待自动重试", done: "已处理", failed: "同步失败" };

export function WebdavSyncPanel() {
  const [config, setConfig] = useState<Config | null>(null);
  const [form, setForm] = useState<Form | null>(null);
  const [runs, setRuns] = useState<Run[]>([]);
  const [error, setError] = useState("");
  const [loadError, setLoadError] = useState("");
  const [message, setMessage] = useState("");
  const [saving, setSaving] = useState(false);
  const [retrying, setRetrying] = useState<number | null>(null);
  const [retryError, setRetryError] = useState<{ id: number; message: string } | null>(null);
  const [password, setPassword] = useState("");
  const [copied, setCopied] = useState("");
  const mounted = useRef(true);
  const version = useRef(0);

  async function refresh(initial = false) {
    const current = version.current;
    try {
      const [settings, history] = await Promise.all([api<Config>(endpoint), api<Run[]>(`${endpoint}/runs`)]);
      if (!mounted.current || current !== version.current) return;
      setConfig(settings); setRuns(history); setLoadError("");
      if (initial) setForm({ enabled: settings.enabled, username: settings.username, existing_policy: settings.existing_policy, auto_create: settings.auto_create });
    } catch (e) {
      if (mounted.current && current === version.current) setLoadError((e as Error).message || "加载接收服务失败，请重试");
    }
  }
  useEffect(() => {
    mounted.current = true;
    void refresh(true);
    const timer = window.setInterval(() => { if (!document.hidden) void refresh(); }, 5000);
    return () => { mounted.current = false; window.clearInterval(timer); version.current += 1; };
  }, []);
  useEffect(() => {
    if (!copied) return;
    const timer = window.setTimeout(() => setCopied(""), 2000);
    return () => window.clearTimeout(timer);
  }, [copied]);

  async function save(rotate = false) {
    if (!form || saving) return;
    setSaving(true); setError(""); setMessage(""); version.current += 1;
    try {
      const next = await api<Config>(endpoint, { method: "PUT", body: JSON.stringify({ ...form, rotate_password: rotate || !config?.password_configured }) });
      if (!mounted.current) return;
      setConfig(next);
      setForm({ enabled: next.enabled, username: next.username, existing_policy: next.existing_policy, auto_create: next.auto_create });
      if (next.new_password) setPassword(next.new_password);
      setMessage(next.new_password ? "已保存，请复制新密码并更新 PTD 中的连接配置。" : "设置已保存，运行状态将在几秒内更新。");
      void refresh();
    } catch (e) { if (mounted.current) setError((e as Error).message || "保存失败，请重试"); }
    finally { if (mounted.current) setSaving(false); }
  }
  async function copy(value: string, label: string) {
    try { await navigator.clipboard.writeText(value); setCopied(label); }
    catch { setError("无法访问剪贴板，请选中文本手动复制。"); }
  }
  async function retry(id: number) {
    setRetrying(id); setRetryError(null);
    try { await api(`${endpoint}/runs/${id}/retry`, { method: "POST" }); await refresh(); }
    catch (e) { if (mounted.current) setRetryError({ id, message: (e as Error).message || "重新同步失败，请重试" }); }
    finally { if (mounted.current) setRetrying(null); }
  }
  if (isDesktop) return <section className="space-y-3 p-4 sm:p-6" aria-label="WebDAV 接收"><h3 className="text-base font-bold">Cookie 自动同步</h3><p className="text-sm text-muted">桌面版通过本地进程通道通信，没有可供 PTD 连接的 Web 端口。请使用 Web 服务版，在其站点管理中配置自动同步。</p></section>;
  const address = new URL("/dav/ptd/", window.location.origin).href;
  const runningText = config?.enabled ? config.runtime_error ? "启动失败" : config.running ? "正在接收" : "正在启动" : config?.running ? "正在停止" : "未启用";

  return <section className="space-y-4 p-4 sm:space-y-6 sm:p-6" aria-label="WebDAV 接收">
    <div className="flex flex-wrap items-start justify-between gap-3">
      <div><h3 className="text-base font-bold">Cookie 自动同步</h3><p className="mt-1 text-sm text-muted">PTD 定时推送后，自动更新站点登录凭据。</p></div>
      {config && <span className="text-sm font-semibold" role="status">{runningText}</span>}
    </div>
    {loadError && <div role="alert" className="text-sm text-destructive">{loadError} <button type="button" className="underline underline-offset-2" onClick={() => void refresh(!form)}>重新加载</button></div>}
    {!form ? !loadError && <p role="status" className="flex items-center gap-2 text-sm text-muted"><Loader2 className="size-4 motion-safe:animate-spin" />正在加载接收设置…</p> : <>
      <fieldset disabled={saving} className="space-y-3 disabled:opacity-60 sm:space-y-4">
        <legend className="sr-only">接收服务设置</legend>
        <label className="flex min-h-11 cursor-pointer items-center gap-3 text-sm font-semibold"><input type="checkbox" className="size-4 accent-primary" checked={form.enabled} onChange={e => setForm({ ...form, enabled: e.target.checked })} />启用 WebDAV 接收</label>
        <p className="text-xs leading-5 text-muted">与当前 Web 页面共用地址和端口，接收路径为 /dav/ptd/，无需额外映射端口。</p>
        <div className="space-y-2"><Label htmlFor="dav-username">接收用户名</Label><Input id="dav-username" value={form.username} onChange={e => setForm({ ...form, username: e.target.value })} autoComplete="off" maxLength={128} /></div>
        <div className="space-y-2"><Label htmlFor="dav-policy">已有站点</Label><Select id="dav-policy" disabled={saving} value={form.existing_policy} onChange={value => setForm({ ...form, existing_policy: value })} options={[{ value: "update", label: "更新 Cookie，保留其他配置" }, { value: "skip", label: "跳过，保留现有 Cookie" }]} /></div>
        <label className="flex min-h-11 cursor-pointer items-center gap-3 text-sm"><input type="checkbox" className="size-4 accent-primary" checked={form.auto_create} onChange={e => setForm({ ...form, auto_create: e.target.checked })} />自动添加识别到的新站点</label>
      </fieldset>
      {error && <p role="alert" className="break-words text-sm text-destructive">{error}</p>}
      {message && <p role="status" className="text-sm">{message}</p>}
      <div className="flex flex-wrap gap-3"><Button disabled={saving} onClick={() => void save()}>{saving && <Loader2 className="motion-safe:animate-spin" />}保存接收设置</Button>{config?.password_configured && <Button variant="outline" disabled={saving} onClick={() => void save(true)}>生成新密码并保存</Button>}</div>
      {config?.password_configured && <p className="text-xs text-muted">生成新密码会立即替换原密码，并保存当前表单设置；请同步修改 PTD 配置。</p>}
      {password && <div className="space-y-2"><Label htmlFor="dav-password">本次生成的密码</Label><div className="flex gap-2"><Input id="dav-password" value={password} readOnly autoComplete="off" spellCheck={false} onFocus={e => e.target.select()} /><Button variant="outline" aria-label="复制接收密码" onClick={() => void copy(password, "密码")}>{copied === "密码" ? <Check /> : <Copy />}</Button></div><p className="text-xs text-muted">密码仅在本次显示，离开此面板后无法找回，可重新生成。</p></div>}
      <div className="space-y-3 border-t border-border pt-5">
        <h4 className="text-sm font-bold">连接 PTD</h4>
        <div className="flex items-center gap-2"><code className="min-w-0 flex-1 break-all text-sm">{address}</code><Button variant="outline" aria-label="复制 WebDAV 地址" onClick={() => void copy(address, "地址")}>{copied === "地址" ? <Check /> : <Copy />}</Button></div>
        <p className="text-xs leading-5 text-muted">在 PTD 的备份设置中添加 WebDAV，填写地址、用户名和密码，关闭 Digest，勾选 Cookie 并设置自动备份周期。当前支持未加密的备份。</p>
        {config?.runtime_error && <p role="alert" className="text-sm text-destructive">{config.runtime_error}</p>}
        <p className="text-xs text-muted">最近接收：{formatTime(config?.last_received_at ?? null)}</p>
      </div>
    </>}
    <div className="space-y-3 border-t border-border pt-5">
      <div className="flex items-center justify-between gap-3"><h4 className="text-sm font-bold">同步记录</h4><Button variant="outline" aria-label="刷新同步记录" onClick={() => void refresh(!form)}><RefreshCw />刷新</Button></div>
      <p className="text-xs leading-5 text-muted">PTD 上传成功表示数据已接收；站点更新结果以此处为准。</p>
      {!loadError && runs.length === 0 && <p className="py-3 text-sm text-muted">还没有同步记录。在 PTD 中测试连接并推送一次，即可查看结果。</p>}
      <ul className="divide-y divide-border">{runs.map(run => <li key={run.id} className="space-y-2 py-4">
        <div className="flex flex-wrap justify-between gap-2 text-sm"><time dateTime={new Date(run.received_at).toISOString()}>{formatTime(run.received_at)}</time><span className="font-semibold">{statuses[run.status] ?? run.status}</span></div>
        <p className="break-all text-xs text-muted">{run.name}</p>
        {run.result && <p className="text-sm">新增 {run.result.created} · 更新 {run.result.updated} · 未变化 {run.result.unchanged} · 跳过 {run.result.skipped}</p>}
        {run.error && <p className="text-sm text-destructive">{run.error}</p>}
        {retryError?.id === run.id && <p role="alert" className="text-sm text-destructive">{retryError.message}</p>}
        {run.result && run.result.details.length > 0 && <details className="text-sm"><summary className="cursor-pointer py-1 underline underline-offset-4">查看跳过原因</summary><ul className="mt-2 space-y-1 text-xs text-muted">{run.result.details.map((detail, i) => <li key={i} className="break-words">{detail}</li>)}</ul></details>}
        {run.status === "failed" && run.can_retry && <Button variant="outline" disabled={retrying !== null} onClick={() => void retry(run.id)}>{retrying === run.id && <Loader2 className="motion-safe:animate-spin" />}重新同步</Button>}
      </li>)}</ul>
    </div>
  </section>;
}
