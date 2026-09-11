import { useEffect, useRef, useState } from "react";
import { Loader2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Dialog } from "@/components/ui/dialog";
import { Select } from "@/components/ui/select";
import { newRequestId, rssApi, rssError, ruleInput, type RssItem, type RssPreview, type RssRule, type RssRun } from "@/lib/rss-api";
import { ErrorBox, Field, PreviewPanel } from "./shared";

export function BackfillDialog({ items, rules, onClose, onQueued }: { items: RssItem[]; rules: RssRule[]; onClose: () => void; onQueued: (run: RssRun) => void }) {
  const applicable = rules.filter((rule) => items.every((item) => rule.feed_ids.includes(item.feed_id)));
  const [ruleId, setRuleId] = useState(applicable.length === 1 ? String(applicable[0].id) : "");
  const selectedRule = applicable.find((rule) => rule.id === Number(ruleId));
  const [preview, setPreview] = useState<RssPreview | null>(null);
  const [previewBusy, setPreviewBusy] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [previewError, setPreviewError] = useState("");
  const request = useRef(newRequestId());
  const sequence = useRef(0);
  useEffect(() => { setPreview(null); setPreviewError(""); request.current = newRequestId(); sequence.current += 1; }, [ruleId, selectedRule?.version]);
  async function runPreview() {
    if (!selectedRule) { setPreviewError("请选择一条包含这些订阅源的下载规则。"); return; }
    const seq = ++sequence.current;
    setPreviewBusy(true); setPreviewError("");
    try { const result = await rssApi.post<RssPreview>("/rules/preview", { rule: ruleInput(selectedRule), item_ids: items.map((item) => item.id), refresh_samples: false }); if (sequence.current === seq) setPreview(result); }
    catch (e) { if (sequence.current === seq) setPreviewError(rssError(e)); } finally { setPreviewBusy(false); }
  }
  async function submit() {
    if (!selectedRule || !preview) return;
    setBusy(true); setError("");
    try { onQueued(await rssApi.post<RssRun>("/backfills", { rule_id: selectedRule.id, expected_version: selectedRule.version, item_ids: items.map((item) => item.id), request_id: request.current })); }
    catch (e) { setError(rssError(e)); } finally { setBusy(false); }
  }
  return <Dialog open onClose={() => { if (!busy) onClose(); }} title="补下已有条目" description={`已选择 ${items.length} 条资源。选择一条规则，先看看哪些资源符合下载条件。`} panelClassName="max-w-3xl" footer={<div className="space-y-3" aria-label="历史补下提交反馈">{error && <ErrorBox>{error}</ErrorBox>}<div className="flex flex-wrap justify-end gap-2"><Button className="h-11 px-4" variant="outline" disabled={busy} onClick={onClose}>取消</Button><Button className="h-11 px-4" disabled={busy || previewBusy || !preview || preview.matched === 0 || !selectedRule?.enabled || !selectedRule.downloader_id} onClick={submit}>{busy && <Loader2 className="motion-safe:animate-spin" />}确认补下所选条目</Button></div></div>}><div className="space-y-5 p-4 sm:p-6"><Field id="rss-backfill-rule" error={!selectedRule && previewError ? previewError : undefined} label="使用下载规则"><Select id="rss-backfill-rule" value={ruleId} onChange={setRuleId} disabled={busy || previewBusy} options={[{ value: "", label: "选择规则" }, ...applicable.map((rule) => ({ value: String(rule.id), label: `${rule.name}${rule.enabled ? "" : "（已暂停）"}` }))]} /></Field>{selectedRule ? <div className="space-y-1 text-sm leading-6"><p>目标下载器：{selectedRule.downloader_name || "尚未配置"}</p><p className="text-xs text-muted">保存目录：{selectedRule.options.save_path || "下载器默认目录"}</p>{!selectedRule.enabled && <p className="text-destructive">请先启用此规则，再进行历史补下。</p>}</div> : applicable.length === 0 ? <p className="text-sm leading-6 text-muted">还没有适用于这些订阅源的规则。请先创建规则，再回来补下。</p> : null}<p className="text-xs leading-6 text-muted">补下会按所选规则检查这些已有资源，只下载符合条件的内容。已经添加过的资源不会重复添加。</p><PreviewPanel preview={preview} busy={previewBusy} stale={false} onPreview={runPreview} error={previewError} /></div></Dialog>;
}
