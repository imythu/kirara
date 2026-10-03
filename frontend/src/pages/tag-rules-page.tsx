import { useEffect, useMemo, useState } from "react";
import { Check, Clock, Edit, Loader2, Plus, RadioTower, RefreshCw, Tag, Trash2, Zap } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Dialog } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Notice } from "@/components/ui/notice";
import { EmptyHint, LoadingState } from "@/components/ui/state";
import { Select } from "@/components/ui/select";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { api } from "@/lib/api";
import { formatDate } from "@/lib/format";
import type {
  DownloaderRecord,
  TagMatchCriteria,
  TagRuleRecord,
  TagRuleRequest,
  TagRuleTrackerDiscovery,
  TagRuleTrackerOption,
} from "@/types";

const MATCH_TYPES = [
  { value: "prefix", label: "前缀匹配" },
  { value: "suffix", label: "后缀匹配" },
  { value: "contains", label: "包含匹配" },
  { value: "exact", label: "完全匹配" },
  { value: "regex", label: "正则匹配" },
] as const;

const emptyRule: TagMatchCriteria = { match_type: "contains", pattern: "" };

const emptyForm: TagRuleRequest = {
  name: "",
  tag_name: "",
  match_rules: [{ ...emptyRule }],
  enabled: true,
  downloader_ids: null,
};

const COMMON_SECOND_LEVEL_SUFFIXES = new Set([
  "ac", "co", "com", "edu", "gov", "net", "org",
]);

type TrackerMatchCandidate = TagMatchCriteria & {
  key: string;
  title: string;
  detail: string;
  recommended?: boolean;
};

function trackerMatchCandidates(domain: string): TrackerMatchCandidate[] {
  const normalized = domain.trim().replace(/^\.+|\.+$/g, "").toLowerCase();
  if (!normalized) return [];

  const exact: TrackerMatchCandidate = {
    key: `exact:${normalized}`,
    match_type: "exact",
    pattern: normalized,
    title: "仅此域名",
    detail: "只命中当前 Tracker 域名",
    recommended: true,
  };
  const labels = normalized.split(".").filter(Boolean);
  const isIpv4 = /^(?:\d{1,3}\.){3}\d{1,3}$/.test(normalized);
  if (labels.length < 2 || isIpv4 || normalized.includes(":")) return [exact];

  let registrableStart = Math.max(0, labels.length - 2);
  if (
    labels.length >= 3
    && COMMON_SECOND_LEVEL_SUFFIXES.has(labels[labels.length - 2])
  ) {
    registrableStart = labels.length - 3;
  }
  const siteDomain = labels.slice(registrableStart).join(".");
  const siteKeyword = labels[registrableStart];
  const candidates: TrackerMatchCandidate[] = [
    exact,
    {
      key: `suffix:${siteDomain}`,
      match_type: "suffix",
      pattern: siteDomain,
      title: "同站点域名",
      detail: `${siteDomain} 及其子域名`,
    },
  ];

  if (siteKeyword.length >= 3 && !/^\d+$/.test(siteKeyword)) {
    candidates.push({
      key: `contains:${siteKeyword}`,
      match_type: "contains",
      pattern: siteKeyword,
      title: "站点关键词",
      detail: "命中范围较宽",
    });
  }
  return candidates;
}

function ruleToForm(rule: TagRuleRecord): TagRuleRequest {
  let matchRules: TagMatchCriteria[] = [];
  try {
    matchRules = JSON.parse(rule.match_rules);
  } catch {
    matchRules = [{ ...emptyRule }];
  }
  let downloaderIds: number[] | null = null;
  if (rule.downloader_ids) {
    try {
      downloaderIds = JSON.parse(rule.downloader_ids);
    } catch {
      downloaderIds = null;
    }
  }
  return {
    name: rule.name,
    tag_name: rule.tag_name,
    match_rules: matchRules,
    enabled: rule.enabled,
    downloader_ids: downloaderIds,
  };
}

function formatBytes(bytes: number): string {
  if (bytes <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const i = Math.floor(Math.log(bytes) / Math.log(1024));
  const val = bytes / Math.pow(1024, i);
  return `${val.toFixed(i > 1 ? 1 : 0)} ${units[i]}`;
}

function matchTypeLabel(type: string) {
  return MATCH_TYPES.find((m) => m.value === type)?.label ?? type;
}

export function TagRulesPage() {
  const [rules, setRules] = useState<TagRuleRecord[]>([]);
  const [downloaders, setDownloaders] = useState<DownloaderRecord[]>([]);
  const [loading, setLoading] = useState(true);
  const [message, setMessage] = useState("");
  const [formOpen, setFormOpen] = useState(false);
  const [editingId, setEditingId] = useState<number | null>(null);
  const [form, setForm] = useState<TagRuleRequest>({ ...emptyForm, match_rules: [{ ...emptyRule }] });
  const [submitError, setSubmitError] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [deleteTarget, setDeleteTarget] = useState<TagRuleRecord | null>(null);
  const [deleting, setDeleting] = useState(false);
  const [deleteError, setDeleteError] = useState("");
  const [scanning, setScanning] = useState(false);
  const [scanInterval, setScanInterval] = useState<number>(7);
  const [savingInterval, setSavingInterval] = useState(false);
  const [trackerOptions, setTrackerOptions] = useState<TagRuleTrackerOption[]>([]);
  const [trackersLoading, setTrackersLoading] = useState(false);
  const [trackersError, setTrackersError] = useState("");
  const [failedDownloaders, setFailedDownloaders] = useState<string[]>([]);
  const [selectedTrackerDomain, setSelectedTrackerDomain] = useState("");
  const [generatedRuleIndex, setGeneratedRuleIndex] = useState<number | null>(null);

  const downloaderNameById = useMemo(
    () => new Map(downloaders.map((d) => [d.id, d.name])),
    [downloaders],
  );
  const trackerByDomain = useMemo(
    () => new Map(trackerOptions.map((tracker) => [tracker.domain, tracker])),
    [trackerOptions],
  );
  const trackerSelectOptions = useMemo(
    () => trackerOptions.map((tracker) => ({
      value: tracker.domain,
      label: `${tracker.domain} · ${tracker.torrent_count} 个种子`,
    })),
    [trackerOptions],
  );
  const selectedTracker = trackerByDomain.get(selectedTrackerDomain);
  const generatedCandidates = useMemo(
    () => trackerMatchCandidates(selectedTrackerDomain),
    [selectedTrackerDomain],
  );

  function loadData() {
    setLoading(true);
    Promise.all([
      api<TagRuleRecord[]>("/api/tag-rules"),
      api<DownloaderRecord[]>("/api/downloaders"),
      api<import("@/types").GlobalConfig>("/api/settings"),
    ])
      .then(([rulesData, downloadersData, settings]) => {
        setRules(rulesData);
        setDownloaders(downloadersData);
        setScanInterval(settings.tag_rule_scan_interval_mins ?? 7);
      })
      .catch((err: Error) => setMessage(err.message))
      .finally(() => setLoading(false));
  }

  async function saveScanInterval(mins: number) {
    setSavingInterval(true);
    try {
      const settings = await api<import("@/types").GlobalConfig>("/api/settings");
      await api("/api/settings", {
        method: "PUT",
        body: JSON.stringify({ ...settings, tag_rule_scan_interval_mins: mins }),
      });
      setScanInterval(mins);
      setMessage(`扫描间隔已设为 ${mins} 分钟`);
    } catch (err) {
      setMessage((err as Error).message);
    } finally {
      setSavingInterval(false);
    }
  }

  useEffect(() => {
    loadData();
  }, []);

  async function loadTrackerOptions() {
    setTrackersLoading(true);
    setTrackersError("");
    try {
      const discovery = await api<TagRuleTrackerDiscovery>("/api/tag-rules/trackers");
      setTrackerOptions(discovery.trackers);
      setFailedDownloaders(discovery.failed_downloaders);
      setSelectedTrackerDomain((current) => (
        discovery.trackers.some((tracker) => tracker.domain === current) ? current : ""
      ));
    } catch (err) {
      setTrackerOptions([]);
      setFailedDownloaders([]);
      setSelectedTrackerDomain("");
      setTrackersError((err as Error).message);
    } finally {
      setTrackersLoading(false);
    }
  }

  function openCreate() {
    setEditingId(null);
    setForm({ ...emptyForm, match_rules: [{ ...emptyRule }] });
    setSubmitError("");
    setSelectedTrackerDomain("");
    setGeneratedRuleIndex(null);
    setFormOpen(true);
    void loadTrackerOptions();
  }

  function openEdit(rule: TagRuleRecord) {
    setEditingId(rule.id);
    setForm(ruleToForm(rule));
    setSubmitError("");
    setSelectedTrackerDomain("");
    setGeneratedRuleIndex(null);
    setFormOpen(true);
  }

  async function handleSubmit() {
    setSubmitting(true);
    setSubmitError("");
    try {
      const payload: TagRuleRequest = {
        ...form,
        name: form.tag_name,
        match_rules: form.match_rules.filter((r) => r.pattern.trim() !== ""),
      };
      if (payload.match_rules.length === 0) {
        setSubmitError("至少需要一条匹配规则");
        setSubmitting(false);
        return;
      }
      if (editingId) {
        await api(`/api/tag-rules/${editingId}`, {
          method: "PUT",
          body: JSON.stringify(payload),
        });
        setMessage("标签规则已更新");
      } else {
        await api("/api/tag-rules", {
          method: "POST",
          body: JSON.stringify(payload),
        });
        setMessage("标签规则已创建");
      }
      setFormOpen(false);
      loadData();
    } catch (err) {
      setSubmitError((err as Error).message || "保存标签规则失败，请重试");
    } finally {
      setSubmitting(false);
    }
  }

  async function handleDelete() {
    if (!deleteTarget) return;
    setDeleting(true);
    try {
      await api(`/api/tag-rules/${deleteTarget.id}`, { method: "DELETE" });
      setMessage("标签规则已删除");
      setDeleteTarget(null);
      loadData();
    } catch (err) {
      setDeleteError((err as Error).message || "删除标签规则失败，请重试");
    } finally {
      setDeleting(false);
    }
  }

  async function handleScan() {
    setScanning(true);
    try {
      await api("/api/tag-rules/scan", { method: "POST" });
      setMessage("扫描完成");
    } catch (err) {
      setMessage((err as Error).message);
    } finally {
      setScanning(false);
    }
  }

  function updateMatchRule(index: number, field: keyof TagMatchCriteria, value: string) {
    setForm((prev) => {
      const next = [...prev.match_rules];
      next[index] = { ...next[index], [field]: value };
      return { ...prev, match_rules: next };
    });
  }

  function addMatchRule() {
    setForm((prev) => ({
      ...prev,
      match_rules: [...prev.match_rules, { ...emptyRule }],
    }));
  }

  function removeMatchRule(index: number) {
    setForm((prev) => ({
      ...prev,
      match_rules: prev.match_rules.filter((_, i) => i !== index),
    }));
    setGeneratedRuleIndex((current) => {
      if (current === null) return null;
      if (current === index) return null;
      return current > index ? current - 1 : current;
    });
  }

  function applyTrackerCandidate(candidate: TrackerMatchCandidate) {
    const blankIndex = form.match_rules.findIndex((rule) => rule.pattern.trim() === "");
    const targetIndex = generatedRuleIndex !== null && form.match_rules[generatedRuleIndex]
      ? generatedRuleIndex
      : blankIndex >= 0
        ? blankIndex
        : form.match_rules.length;
    setForm((prev) => {
      const next = [...prev.match_rules];
      next[targetIndex] = { match_type: candidate.match_type, pattern: candidate.pattern };
      return { ...prev, match_rules: next };
    });
    setGeneratedRuleIndex(targetIndex);
  }

  function toggleDownloader(id: number) {
    setForm((prev) => {
      const current = prev.downloader_ids ?? [];
      const next = current.includes(id) ? current.filter((x) => x !== id) : [...current, id];
      return { ...prev, downloader_ids: next };
    });
  }

  function setAllDownloaders() {
    setForm((prev) => ({ ...prev, downloader_ids: null }));
  }

  if (loading) {
    return (
      <Card>
        <CardContent>
          <LoadingState className="py-12" label="正在加载标签规则…" />
        </CardContent>
      </Card>
    );
  }

  const isAllDownloaders = form.downloader_ids === null || form.downloader_ids === undefined;

  return (
    <div className="space-y-4">
      {message ? <Notice onDismiss={() => setMessage("")}>{message}</Notice> : null}

      <Card>
        <CardHeader>
          <div className="flex items-center justify-between">
            <div>
              <CardTitle className="flex items-center gap-2">
                <Tag className="h-5 w-5 text-primary" />
                标签规则
              </CardTitle>
              <CardDescription>
                根据种子的 Tracker 域名自动匹配并添加标签。
              </CardDescription>
              <div className="mt-2 flex items-center gap-2 text-xs text-muted">
                <Clock className="h-3.5 w-3.5" aria-hidden="true" />
                <label htmlFor="tag-rule-scan-interval">扫描间隔</label>
                <select
                  id="tag-rule-scan-interval"
                  value={scanInterval}
                  onChange={(e) => saveScanInterval(Number(e.target.value))}
                  disabled={savingInterval}
                  className="h-8 rounded-lg border border-border bg-input px-2 text-xs text-foreground focus:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                >
                  {[3, 5, 7, 10, 15, 20, 30, 60].map((m) => (
                    <option key={m} value={m}>{m} 分钟</option>
                  ))}
                </select>
                {savingInterval ? <Loader2 className="h-3 w-3 animate-spin" /> : null}
              </div>
            </div>
            <div className="flex items-center gap-2">
              <Button variant="outline" onClick={handleScan} disabled={scanning} className="gap-2">
                {scanning ? <Loader2 className="h-4 w-4 animate-spin" /> : <Zap className="h-4 w-4" />}
                立即扫描
              </Button>
              <Button onClick={openCreate} className="gap-2">
                <Plus className="h-4 w-4" />
                新增规则
              </Button>
            </div>
          </div>
        </CardHeader>
        <CardContent>
          {rules.length === 0 ? (
            <EmptyHint title="暂无标签规则" action={<Button onClick={openCreate}><Plus className="h-4 w-4" />新增规则</Button>}>
              按 Tracker 域名或正则为下载器中的种子自动添加标签。
            </EmptyHint>
          ) : (
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>标签名</TableHead>
                  <TableHead>种子数</TableHead>
                  <TableHead>匹配规则</TableHead>
                  <TableHead>生效实例</TableHead>
                  <TableHead>状态</TableHead>
                  <TableHead>更新时间</TableHead>
                  <TableHead className="text-right">操作</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {rules.map((rule) => {
                  let criteria: TagMatchCriteria[] = [];
                  try {
                    criteria = JSON.parse(rule.match_rules);
                  } catch {
                    /* ignore */
                  }
                  let downloaderIds: number[] | null = null;
                  if (rule.downloader_ids) {
                    try {
                      downloaderIds = JSON.parse(rule.downloader_ids);
                    } catch {
                      /* ignore */
                    }
                  }
                  return (
                    <TableRow key={rule.id}>
                      <TableCell>
                        <span className="inline-flex items-center gap-1 rounded-full bg-primary/10 px-2.5 py-0.5 text-xs font-semibold text-primary">
                          {rule.tag_name}
                        </span>
                      </TableCell>
                      <TableCell>
                        <span className="text-sm tabular-nums">{rule.tagged_torrent_count}</span>
                        {rule.tagged_total_size > 0 ? (
                          <span className="ml-1 text-xs text-muted">
                            ({formatBytes(rule.tagged_total_size)})
                          </span>
                        ) : null}
                      </TableCell>
                      <TableCell>
                        <div className="flex flex-wrap gap-1">
                          {criteria.map((c, i) => (
                            <span
                              key={i}
                              className="inline-flex items-center gap-1 rounded-full bg-surface-container px-2 py-0.5 text-xs text-foreground"
                            >
                              <span className="text-muted">{matchTypeLabel(c.match_type)}</span>
                              <span className="max-w-[140px] truncate font-mono">{c.pattern}</span>
                            </span>
                          ))}
                        </div>
                      </TableCell>
                      <TableCell>
                        <span className="text-xs text-muted">
                          {downloaderIds === null
                            ? "所有实例"
                            : downloaderIds.map((id) => downloaderNameById.get(id) ?? `#${id}`).join(", ")}
                        </span>
                      </TableCell>
                      <TableCell>
                        <span
                          className={`inline-flex items-center rounded-full px-2.5 py-0.5 text-xs font-medium ${
                            rule.enabled ? "bg-emerald-100 text-emerald-700" : "bg-slate-100 text-slate-500"
                          }`}
                        >
                          {rule.enabled ? "启用" : "禁用"}
                        </span>
                      </TableCell>
                      <TableCell className="text-xs text-muted">{formatDate(rule.updated_at)}</TableCell>
                      <TableCell className="text-right">
                        <div className="flex items-center justify-end gap-1">
                          <button
                            type="button"
                            onClick={() => openEdit(rule)}
                            title="编辑"
                            aria-label={`编辑标签规则 ${rule.tag_name}`}
                            className="rounded-lg p-2 text-muted transition hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                          >
                            <Edit className="h-4 w-4" />
                          </button>
                          <button
                            type="button"
                            onClick={() => { setDeleteError(""); setDeleteTarget(rule); }}
                            title="删除"
                            aria-label={`删除标签规则 ${rule.tag_name}`}
                            className="rounded-lg p-2 text-muted transition hover:bg-destructive/10 hover:text-destructive focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                          >
                            <Trash2 className="h-4 w-4" />
                          </button>
                        </div>
                      </TableCell>
                    </TableRow>
                  );
                })}
              </TableBody>
            </Table>
          )}
        </CardContent>
      </Card>

      {/* 新增/编辑对话框 */}
      <Dialog
        open={formOpen}
        onClose={() => setFormOpen(false)}
        title={editingId ? "编辑标签规则" : "新增标签规则"}
        description="设置 Tracker 匹配规则，匹配成功的种子将自动添加对应标签。"
      >
        <div className="space-y-5 p-4 sm:p-6">

          {/* 基本信息 */}
          <div className="space-y-2">
            <Label htmlFor="tag-rule-name">标签名</Label>
            <Input
              id="tag-rule-name"
              value={form.tag_name}
              onChange={(e) => setForm((prev) => ({ ...prev, tag_name: e.target.value }))}
              placeholder="例如：mteam"
            />
          </div>

          {/* 启用状态 */}
          <div className="flex items-center gap-3">
            <button
              type="button"
              role="switch"
              id="tag-rule-enabled"
              aria-checked={form.enabled ?? true}
              onClick={() => setForm((prev) => ({ ...prev, enabled: !(prev.enabled ?? true) }))}
              className={`relative inline-flex h-6 w-11 shrink-0 cursor-pointer rounded-full border-2 border-transparent transition-colors ${
                form.enabled ?? true ? "bg-primary" : "bg-border"
              }`}
            >
              <span
                className={`pointer-events-none inline-block h-5 w-5 transform rounded-full bg-white shadow-sm ring-0 transition-transform ${
                  form.enabled ?? true ? "translate-x-5" : "translate-x-0"
                }`}
              />
            </button>
            <Label htmlFor="tag-rule-enabled">启用</Label>
          </div>

          {/* 从已有 Tracker 生成 */}
          {editingId === null ? (
            <div className="space-y-3 border-y border-border py-4">
              <div className="flex items-center justify-between gap-3">
                <Label htmlFor="existing-tracker" className="flex items-center gap-2">
                  <RadioTower className="h-4 w-4 text-primary" aria-hidden="true" />
                  已有 Tracker
                </Label>
                <button
                  type="button"
                  onClick={() => void loadTrackerOptions()}
                  disabled={trackersLoading}
                  aria-label="刷新已有 Tracker"
                  title="刷新"
                  className="cursor-pointer rounded-lg p-2 text-muted transition-colors hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary disabled:cursor-not-allowed disabled:opacity-50"
                >
                  <RefreshCw className={`h-4 w-4 ${trackersLoading ? "animate-spin" : ""}`} />
                </button>
              </div>
              <Select
                id="existing-tracker"
                value={selectedTrackerDomain}
                onChange={setSelectedTrackerDomain}
                options={trackerSelectOptions}
                disabled={trackersLoading || trackerSelectOptions.length === 0}
                aria-describedby="existing-tracker-status"
              />
              <div id="existing-tracker-status" aria-live="polite">
                {trackersLoading ? (
                  <p className="flex items-center gap-2 text-xs text-muted">
                    <Loader2 className="h-3.5 w-3.5 animate-spin" aria-hidden="true" />
                    正在读取 Tracker
                  </p>
                ) : trackersError ? (
                  <p role="alert" className="text-xs text-destructive">读取失败：{trackersError}</p>
                ) : trackerOptions.length === 0 ? (
                  <p className="text-xs text-muted">未发现可用 Tracker</p>
                ) : null}
                {failedDownloaders.length > 0 ? (
                  <p className="mt-1 text-xs text-amber-700 dark:text-amber-300">
                    未能读取：{failedDownloaders.join("、")}
                  </p>
                ) : null}
                {selectedTracker ? (
                  <p className="mt-1 text-xs text-muted">
                    {selectedTracker.torrent_count} 个种子 · {selectedTracker.downloader_ids
                      .map((id) => downloaderNameById.get(id) ?? `#${id}`)
                      .join("、")}
                  </p>
                ) : null}
              </div>

              {generatedCandidates.length > 0 ? (
                <div className="space-y-2">
                  <Label>候选匹配</Label>
                  <div className="grid gap-2 sm:grid-cols-3">
                    {generatedCandidates.map((candidate) => {
                      const generatedRule = generatedRuleIndex === null
                        ? null
                        : form.match_rules[generatedRuleIndex];
                      const selected = generatedRule?.match_type === candidate.match_type
                        && generatedRule.pattern === candidate.pattern;
                      return (
                        <button
                          key={candidate.key}
                          type="button"
                          aria-pressed={selected}
                          onClick={() => applyTrackerCandidate(candidate)}
                          className={`min-w-0 cursor-pointer rounded-lg border px-3 py-3 text-left transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary ${
                            selected
                              ? "border-primary bg-primary/10 text-foreground"
                              : "border-border bg-surface-container/35 text-foreground hover:border-primary/40 hover:bg-accent"
                          }`}
                        >
                          <span className="flex items-center justify-between gap-2 text-sm font-semibold">
                            <span>{candidate.title}</span>
                            {selected ? (
                              <Check className="h-4 w-4 shrink-0 text-primary" aria-hidden="true" />
                            ) : candidate.recommended ? (
                              <span className="shrink-0 text-[10px] font-medium text-primary">推荐</span>
                            ) : null}
                          </span>
                          <code className="mt-1 block truncate text-xs text-primary">{candidate.pattern}</code>
                          <span className="mt-1 block text-xs leading-5 text-muted">{candidate.detail}</span>
                        </button>
                      );
                    })}
                  </div>
                </div>
              ) : null}
            </div>
          ) : null}

          {/* 匹配规则 */}
          <div className="space-y-3">
            <div className="flex items-center justify-between">
              <Label>匹配规则（任一匹配即生效）</Label>
              <button
                type="button"
                onClick={addMatchRule}
                className="inline-flex items-center gap-1 rounded-full border border-border bg-card/80 px-3 py-1.5 text-xs font-semibold text-foreground transition hover:border-primary/35 hover:bg-accent"
              >
                <Plus className="h-3 w-3" />
                添加
              </button>
            </div>
            <p className="text-xs leading-5 text-muted">
              匹配目标为 tracker 的<b>域名</b>（如 <code className="rounded bg-surface-container px-1">kp.m-team.xyz</code>），不含路径和参数。
            </p>
            {form.match_rules.map((rule, index) => (
              <div key={index} className="flex items-center gap-2">
                <Select
                  className="w-[140px] shrink-0"
                  aria-label={`第 ${index + 1} 条规则的匹配方式`}
                  value={rule.match_type}
                  onChange={(val) => updateMatchRule(index, "match_type", val as TagMatchCriteria["match_type"])}
                  options={MATCH_TYPES}
                />
                <Input
                  className="min-w-0 flex-1"
                  aria-label={`第 ${index + 1} 条规则的匹配内容`}
                  value={rule.pattern}
                  onChange={(e) => updateMatchRule(index, "pattern", e.target.value)}
                  placeholder={rule.match_type === "regex" ? "正则表达式，如 m-team\\.xyz" : "域名关键词，如 m-team.xyz"}
                />
                {form.match_rules.length > 1 ? (
                  <button
                    type="button"
                    onClick={() => removeMatchRule(index)}
                    className="shrink-0 rounded-lg p-2 text-muted transition hover:bg-destructive/10 hover:text-destructive"
                    title="删除此规则"
                    aria-label={`删除第 ${index + 1} 条匹配规则`}
                  >
                    <Trash2 className="h-4 w-4" />
                  </button>
                ) : null}
              </div>
            ))}
          </div>

          {/* 生效实例 */}
          <div className="space-y-3">
            <Label>生效下载器实例</Label>
            <div className="flex flex-wrap gap-2">
              <button
                type="button"
                onClick={setAllDownloaders}
                className={`rounded-full px-3 py-1.5 text-xs font-medium transition-colors ${
                  isAllDownloaders
                    ? "bg-primary text-primary-foreground shadow-glow"
                    : "bg-surface-container text-muted hover:bg-accent"
                }`}
              >
                所有实例
              </button>
              {downloaders.map((d) => {
                const selected = !isAllDownloaders && (form.downloader_ids ?? []).includes(d.id);
                return (
                  <button
                    key={d.id}
                    type="button"
                    onClick={() => toggleDownloader(d.id)}
                    className={`rounded-full px-3 py-1.5 text-xs font-medium transition-colors ${
                      selected
                        ? "bg-primary text-primary-foreground shadow-glow"
                        : "bg-surface-container text-muted hover:bg-accent"
                    }`}
                  >
                    {d.name}
                  </button>
                );
              })}
            </div>
          </div>

          {/* 提交 */}
          {submitError ? <Notice tone="error">{submitError}</Notice> : null}
          <div className="flex items-center justify-end gap-3 pt-2">
            <Button variant="outline" disabled={submitting} onClick={() => setFormOpen(false)}>
              取消
            </Button>
            <Button onClick={handleSubmit} loading={submitting}>
              {submitting ? "保存中…" : editingId ? "保存" : "创建"}
            </Button>
          </div>
        </div>
      </Dialog>

      {/* 删除确认 */}
      <Dialog
        open={!!deleteTarget}
        onClose={() => { setDeleteTarget(null); setDeleteError(""); }}
        title="确认删除"
        description={`确定要删除标签规则「${deleteTarget?.name ?? ""}」吗？此操作不可撤销。`}
      >
        {deleteError ? <Notice tone="error" className="mx-4 mt-4 sm:mx-6">{deleteError}</Notice> : null}
        <div className="flex items-center justify-end gap-3 p-4 sm:p-6">
          <Button variant="outline" disabled={deleting} onClick={() => { setDeleteTarget(null); setDeleteError(""); }}>
            取消
          </Button>
          <Button variant="destructive" onClick={handleDelete} loading={deleting}>
            {deleting ? "删除中…" : "确认删除"}
          </Button>
        </div>
      </Dialog>
    </div>
  );
}
