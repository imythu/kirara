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
    if (!selectedRule) { setPreviewError("请先选择一条覆盖这些来源的下载规则。"); return; }
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
  return <Dialog open onClose={() => { if (!busy) onClose(); }} title="补下已有条目" description={`已选择 ${items.length} 条资源。确认规则与目标，再预览实际匹配结果。`} panelClassName="max-w-3xl" footer={<div className="space-y-3" aria-label="历史补下提交反馈">{error && <ErrorBox>{error}</ErrorBox>}<div className="flex flex-wrap justify-end gap-2"><Button className="h-11 px-4" variant="outline" disabled={busy} onClick={onClose}>取消</Button><Button className="h-11 px-4" disabled={busy || previewBusy || !preview || preview.matched === 0 || !selectedRule?.enabled || !selectedRule.downloader_id} onClick={submit}>{busy && <Loader2 className="motion-safe:animate-spin" />}确认补下所选条目</Button></div></div>}><div className="space-y-5 p-4 sm:p-6"><Field id="rss-backfill-rule" error={!selectedRule && previewError ? previewError : undefined} label="使用下载规则"><Select id="rss-backfill-rule" value={ruleId} onChange={setRuleId} disabled={busy || previewBusy} options={[{ value: "", label: "选择规则" }, ...applicable.map((rule) => ({ value: String(rule.id), label: `${rule.name}${rule.enabled ? "" : "（已暂停）"}` }))]} /></Field>{selectedRule ? <div className="space-y-1 text-sm leading-6"><p>目标下载器：{selectedRule.downloader_name || "尚未配置"}</p><p className="text-xs text-muted">保存目录：{selectedRule.options.save_path || "下载器默认目录"} · 规则修订 {selectedRule.match_revision}</p>{!selectedRule.enabled && <p className="text-destructive">请先启用此规则，再进行历史补下。</p>}</div> : applicable.length === 0 ? <p className="text-sm leading-6 text-muted">还没有覆盖所选来源的规则。请先创建规则，然后返回选择条目。</p> : null}<p className="text-xs leading-6 text-muted">补下只允许这些已选资源越过首次发现时间限制，仍会重新检查属性、种子有效性、空间与重复任务。提交成功表示请求已保存，实际入队结果请查看处理记录。</p><PreviewPanel preview={preview} busy={previewBusy} stale={false} onPreview={runPreview} error={previewError} /></div></Dialog>;
}
