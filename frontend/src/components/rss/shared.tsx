import { AlertCircle, Check, ChevronDown, ChevronLeft, ChevronRight, CircleHelp, RefreshCw } from "lucide-react";
import { Children, cloneElement, isValidElement, useId, useState, type ReactNode } from "react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { rssDate, rssSize, rssStatus, type RssAttributes, type RssEvaluation, type RssPreview } from "@/lib/rss-api";

export type FormIssue = { field: string; message: string };

export function Help({ title, children }: { title: string; children: ReactNode }) {
  return <details className="group text-xs leading-6"><summary className="flex min-h-11 w-fit cursor-pointer list-none items-center gap-2 rounded-md text-primary [&::-webkit-details-marker]:hidden"><CircleHelp className="size-4 shrink-0" aria-hidden="true" /><span>{title}</span><ChevronDown className="size-4 shrink-0 transition-transform group-open:rotate-180" aria-hidden="true" /></summary><div className="max-w-prose space-y-2 pb-2 text-muted">{children}</div></details>;
}

export function OptionalSettings({ title, defaultOpen = false, children }: { title: string; defaultOpen?: boolean; children: ReactNode }) {
  const [open, setOpen] = useState(defaultOpen);
  return <details className="border-t border-border" open={open} onToggle={(event) => setOpen(event.currentTarget.open)}><summary className="min-h-11 cursor-pointer py-3 text-sm font-medium">{title}</summary><div className="space-y-5 pb-2 pt-2">{children}</div></details>;
}

export function Field({ id, label, hint, help, helpTitle, error, children }: { id: string; label: string; hint?: string; help?: ReactNode; helpTitle?: string; error?: string; children: ReactNode }) {
  const controls = Children.map(children, (child) => {
    if (!isValidElement<Record<string, unknown>>(child) || child.props.id !== id) return child;
    const describedBy = [...new Set([child.props["aria-describedby"], hint ? `${id}-hint` : null, error ? `${id}-error` : null].filter(Boolean))].join(" ") || undefined;
    return cloneElement(child, { "aria-invalid": error ? true : child.props["aria-invalid"], "aria-describedby": describedBy });
  });
  return <div className="min-w-0 space-y-2"><label className="block text-sm font-semibold" htmlFor={id}>{label}</label>{controls}{hint && <p id={`${id}-hint`} className="text-xs leading-5 text-muted">{hint}</p>}{error && <p id={`${id}-error`} className="text-xs leading-5 text-destructive">{error}</p>}{help && <Help title={helpTitle ?? `了解${label}`}>{help}</Help>}</div>;
}

export function CheckField({ checked, onChange, children, hint, disabled = false }: { checked: boolean; onChange: (checked: boolean) => void; children: ReactNode; hint?: string; disabled?: boolean }) {
  const id = useId();
  return <label className={cn("flex min-h-11 cursor-pointer items-start gap-3 py-2 text-sm leading-6", disabled && "cursor-not-allowed opacity-60")}><input className="mt-1 size-4 shrink-0 accent-primary" type="checkbox" checked={checked} onChange={(event) => onChange(event.target.checked)} disabled={disabled} aria-labelledby={`${id}-label`} aria-describedby={hint ? `${id}-hint` : undefined} /><span className="min-w-0"><span id={`${id}-label`} className="block">{children}</span>{hint && <span id={`${id}-hint`} className="mt-1 block text-xs leading-5 text-muted">{hint}</span>}</span></label>;
}

export function ErrorBox({ children, action }: { children: ReactNode; action?: ReactNode }) {
  return <div role="alert" className="flex flex-wrap items-start gap-3 rounded-xl border border-destructive/30 bg-card p-4 text-sm"><AlertCircle className="mt-0.5 size-4 shrink-0 text-destructive" /><div className="min-w-0 flex-1 break-words leading-6">{children}</div>{action}</div>;
}

export function Status({ status }: { status: string }) {
  const positive = ["success", "ok", "completed", "submitted", "matched", "not_modified", "already_present"].includes(status);
  const problem = ["failed", "error", "source_changed"].includes(status);
  return <span className={cn("inline-flex items-center gap-1.5 whitespace-nowrap text-xs font-medium", positive ? "text-jade" : problem ? "text-destructive" : "text-muted")}><span className="size-1.5 rounded-full bg-current" aria-hidden="true" />{rssStatus(status)}</span>;
}

export function Empty({ title, children, action }: { title: string; children: ReactNode; action?: ReactNode }) {
  return <div className="px-4 py-12 text-center sm:py-16"><h3 className="text-base font-semibold">{title}</h3><p className="mx-auto mt-2 max-w-lg text-sm leading-6 text-muted">{children}</p>{action && <div className="mt-5 flex justify-center">{action}</div>}</div>;
}

export function ListSkeleton() {
  return <div role="status" aria-label="正在加载 RSS 数据" className="divide-y divide-border rounded-xl border border-border bg-card">{[0, 1, 2].map((row) => <div key={row} className="space-y-3 p-5 motion-safe:animate-pulse"><div className="h-4 w-2/5 rounded bg-accent" /><div className="h-3 w-3/5 rounded bg-accent" /></div>)}</div>;
}

export function Pager({ page, total, size, onChange }: { page: number; total: number; size: number; onChange: (page: number) => void }) {
  if (total === 0) return null;
  return <div className="flex flex-wrap items-center justify-between gap-2 py-3 text-xs text-muted"><span className="tabular-nums">共 {total} 条 · 第 {page} / {Math.max(1, Math.ceil(total / size))} 页</span><div className="flex gap-2"><Button className="h-11 px-3" variant="outline" disabled={page <= 1} onClick={() => onChange(page - 1)} aria-label="上一页"><ChevronLeft /></Button><Button className="h-11 px-3" variant="outline" disabled={page * size >= total} onClick={() => onChange(page + 1)} aria-label="下一页"><ChevronRight /></Button></div></div>;
}

const attributeSources: Record<string, string> = { rss: "RSS 订阅", torznab: "RSS 订阅", site: "关联站点", nexusphp: "关联站点", mteam: "关联站点", unknown: "暂未取得" };

export function AttributeDetails({ attributes }: { attributes: RssAttributes }) {
  return <div className="space-y-2 text-xs leading-5 text-muted"><dl className="grid grid-cols-2 gap-x-4 gap-y-1 sm:grid-cols-3"><div><dt className="inline">大小：</dt><dd className="inline text-foreground">{rssSize(attributes.size_bytes)}</dd></div><div><dt className="inline">做种：</dt><dd className="inline text-foreground">{attributes.seeders ?? "未知"}</dd></div><div><dt className="inline">H&amp;R：</dt><dd className="inline text-foreground">{attributes.hr == null ? "未知" : attributes.hr ? "有 H&R" : "确认无 H&R"}</dd></div><div><dt className="inline">下载流量：</dt><dd className="inline text-foreground">{attributes.download_volume_factor == null ? "未知" : attributes.download_volume_factor === 0 ? "免费" : `${attributes.download_volume_factor} 倍`}</dd></div><div><dt className="inline">信息来自：</dt><dd className="inline text-foreground">{attributeSources[attributes.source] ?? "未知"}</dd></div><div><dt className="inline">更新于：</dt><dd className="inline text-foreground">{rssDate(attributes.observed_at)}</dd></div></dl>{attributes.free_until != null && <p>免费截止：{rssDate(new Date(attributes.free_until * 1000).toISOString())}</p>}{attributes.hints.length > 0 && <p>站点说明：{attributes.hints.join("；")}。免费与做种要求仍以站点确认的信息为准。</p>}</div>;
}

export function Evidence({ evaluation }: { evaluation: RssEvaluation }) {
  return <ul className="space-y-2 text-xs leading-5">{evaluation.reasons.length === 0 ? <li className="text-muted">暂时没有筛选结果。</li> : evaluation.reasons.map((reason, index) => <li key={`${reason.code}-${index}`}><p className="break-words">{reason.message}</p>{(reason.actual != null || reason.expected != null) && <p className="break-words text-muted">{reason.actual != null && `实际：${reason.actual}`}{reason.actual != null && reason.expected != null && " · "}{reason.expected != null && `要求：${reason.expected}`}</p>}</li>)}</ul>;
}

export function PreviewPanel({ preview, busy, stale, onPreview, onRefresh, error }: { preview: RssPreview | null; busy: boolean; stale: boolean; onPreview: () => void; onRefresh?: () => void; error?: string }) {
  return <section aria-labelledby="rss-preview-heading" className="min-w-0 rounded-xl border border-border bg-card"><div className="border-b border-border p-4 sm:p-5"><h3 id="rss-preview-heading" className="font-semibold">匹配预览</h3><p className="mt-1 text-xs leading-5 text-muted">用已发现的资源试一试条件，不会开始下载。</p><div className="mt-4 flex flex-wrap gap-2"><Button className="h-11 px-4" variant="secondary" disabled={busy} onClick={onPreview}><RefreshCw className={busy ? "motion-safe:animate-spin" : ""} />{busy ? "正在查看" : "查看匹配结果"}</Button>{onRefresh && <Button className="h-11 px-4" variant="outline" disabled={busy} onClick={onRefresh}>读取最新资源</Button>}</div></div>{error && <div className="p-4"><ErrorBox>{error}</ErrorBox></div>}<div aria-live="polite" aria-atomic="true" className="px-4 pt-4 text-sm sm:px-5">{preview && <><p className="flex flex-wrap gap-x-4 gap-y-1 tabular-nums"><span>资源 {preview.total}</span><span className="text-jade">符合 {preview.matched}</span><span>被排除 {preview.rejected}</span><span>信息不足 {preview.unknown}</span></p><p className="mt-2 text-xs leading-5 text-muted">读取于：{rssDate(preview.sample_time)}{preview.sample_limited ? " · 仅展示部分资源" : ""}</p>{preview.unknown > 0 && <p className="mt-2 text-xs leading-5 text-muted">部分资源缺少规则需要的信息，展开资源可查看具体原因。</p>}{stale && <p className="mt-2 text-xs font-medium text-primary">条件已修改，正在显示上一次的结果。</p>}</>}</div>{!preview ? <Empty title="先看看哪些资源符合条件">选择订阅源并填写条件后，这里会说明哪些资源符合，以及其他资源被跳过的原因。</Empty> : preview.items.length === 0 ? <Empty title="还没有可以预览的资源">可以读取最新资源，或等待订阅源完成首次检查。</Empty> : <div className="mt-4 divide-y divide-border">{preview.items.map(({ item, evaluation }) => <details key={`${item.feed_id}-${item.id}-${item.item_key}`} className="group px-4 py-3 sm:px-5"><summary className="flex min-h-11 cursor-pointer list-none items-start gap-2 rounded-md py-1 [&::-webkit-details-marker]:hidden"><span className="mt-0.5 shrink-0">{evaluation.needs_attributes ? <CircleHelp className="size-4 text-muted" /> : evaluation.matched ? <Check className="size-4 text-jade" /> : <AlertCircle className="size-4 text-muted" />}</span><span className="min-w-0 flex-1"><span className="block break-words text-sm font-medium">{item.title}</span><span className="mt-1 block text-xs text-muted">{evaluation.needs_attributes ? "信息不足" : evaluation.matched ? "符合规则" : "被排除"} · {rssSize(item.attributes.size_bytes)}</span></span><ChevronDown className="mt-1 size-4 shrink-0 text-muted transition-transform group-open:rotate-180" /></summary><div className="space-y-4 pb-2 pl-6 pt-3"><Evidence evaluation={evaluation} /><AttributeDetails attributes={item.attributes} /></div></details>)}</div>}</section>;
}
