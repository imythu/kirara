import { useEffect, useRef, useState, type FormEvent } from "react";
import {
  ArrowLeft,
  Clock3,
  Globe,
  History,
  Pause,
  Pencil,
  Play,
  Plus,
  Send,
  Trash2,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Select } from "@/components/ui/select";
import { Dialog } from "@/components/ui/dialog";
import { api } from "@/lib/api";
import { cn } from "@/lib/utils";

import {
  HttpAuthEditor,
  HttpDeliveryEditor,
  HttpBodyEditor,
  HttpRequestPreview,
  initialHttp,
  normalizeHttp,
  type HttpConfig,
  type Pair,
} from "@/components/scheduled-http-panel";

type Timing =
  | { mode: "interval"; minutes: number }
  | { mode: "cron"; expression: string; utc_offset_minutes: number };
type Run = {
  id: number;
  trigger: string;
  started_at: string;
  finished_at: string | null;
  status: string;
  status_code: number | null;
  duration_ms: number | null;
  message: string;
};
type Task = {
  id: number;
  name: string;
  task_type: string;
  timing: Timing;
  enabled: boolean;
  request_summary: string;
  next_run_at: string | null;
  running: boolean;
  last_run: Run | null;
};
const root = "/api/scheduled-tasks";
const methods = [
  "GET",
  "POST",
  "PUT",
  "PATCH",
  "DELETE",
  "HEAD",
  "OPTIONS",
].map((value) => ({ value, label: value }));
const intervals = [
  { value: "5", label: "每 5 分钟" },
  { value: "15", label: "每 15 分钟" },
  { value: "30", label: "每 30 分钟" },
  { value: "60", label: "每 1 小时" },
  { value: "360", label: "每 6 小时" },
  { value: "720", label: "每 12 小时" },
  { value: "1440", label: "每 1 天" },
  { value: "10080", label: "每 1 周" },
  { value: "custom", label: "自定义间隔…" },
];
const messageOf = (error: unknown) =>
  error instanceof Error ? error.message : "操作失败，请重试";
const date = (value: string | null) =>
  value ? new Date(value).toLocaleString("zh-CN", { hour12: false }) : "—";
function timingLabel(timing: Timing) {
  if (timing.mode === "cron") return timing.expression;
  const mins = timing.minutes;
  return mins % 10080 === 0
    ? `每 ${mins / 10080} 周`
    : mins % 1440 === 0
      ? `每 ${mins / 1440} 天`
      : mins % 60 === 0
        ? `每 ${mins / 60} 小时`
        : `每 ${mins} 分钟`;
}
function Status({ run, running }: { run: Run | null; running?: boolean }) {
  const value = running ? "running" : run?.status;
  return (
    <span
      className={cn(
        "inline-flex items-center rounded-md px-2 py-1 text-xs font-medium",
        value === "success"
          ? "bg-accent text-foreground"
          : value === "failed" || value === "interrupted"
            ? "bg-destructive/10 text-destructive"
            : "bg-secondary text-secondary-foreground",
      )}
    >
      {value === "running"
        ? "执行中"
        : value === "success"
          ? "成功"
          : value === "failed"
            ? "失败"
            : value === "interrupted"
              ? "已中断"
              : "尚未执行"}
    </span>
  );
}

export function ScheduledTasksPage() {
  const [tasks, setTasks] = useState<Task[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [busy, setBusy] = useState<number | null>(null);
  const [editor, setEditor] = useState<Task | "new" | null>(null);
  const [history, setHistory] = useState<Task | null>(null);
  const [runs, setRuns] = useState<Run[]>([]);
  const [historyError, setHistoryError] = useState("");
  const [historyLoading, setHistoryLoading] = useState(false);
  const [deleting, setDeleting] = useState<Task | null>(null);
  const [deleteError, setDeleteError] = useState("");
  const createRef = useRef<HTMLButtonElement>(null);
  const [query, setQuery] = useState("");
  async function refresh() {
    try {
      setTasks(await api<Task[]>(root));
      setError("");
    } catch (e) {
      setError(messageOf(e));
    } finally {
      setLoading(false);
    }
  }
  useEffect(() => {
    void refresh();
    const timer = window.setInterval(() => {
      if (!document.hidden) void refresh();
    }, 3000);
    return () => clearInterval(timer);
  }, []);
  useEffect(() => {
    if (!history) return;
    let current = true;
    setRuns([]);
    setHistoryError("");
    setHistoryLoading(true);
    const load = async () => {
      try {
        const data = await api<Run[]>(`${root}/${history.id}/records`);
        if (current) {
          setRuns(data);
          setHistoryError("");
        }
      } catch (e) {
        if (current) setHistoryError(messageOf(e));
      } finally {
        if (current) setHistoryLoading(false);
      }
    };
    void load();
    const timer = window.setInterval(load, 3000);
    return () => {
      current = false;
      clearInterval(timer);
    };
  }, [history]);
  async function action(task: Task, kind: "run" | "enabled") {
    setBusy(task.id);
    setNotice("");
    try {
      await api(`${root}/${task.id}/${kind}`, {
        method: kind === "run" ? "POST" : "PUT",
        ...(kind === "enabled"
          ? { body: JSON.stringify({ enabled: !task.enabled }) }
          : {}),
      });
      setNotice(
        kind === "run"
          ? `「${task.name}」已开始执行，可在执行记录查看结果。`
          : task.enabled
            ? `「${task.name}」已暂停，正在执行的请求会继续完成。`
            : `「${task.name}」已启用。`,
      );
      await refresh();
    } catch (e) {
      setNotice(`${task.name}：${messageOf(e)}`);
    } finally {
      setBusy(null);
    }
  }
  function closeEditor() {
    setEditor(null);
    requestAnimationFrame(() => createRef.current?.focus());
  }
  const filtered = tasks.filter((t) =>
    `${t.name} ${t.request_summary}`
      .toLowerCase()
      .includes(query.toLowerCase()),
  );
  return (
    <div className="space-y-5">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <p className="max-w-2xl text-sm leading-6 text-muted">
          把重复请求交给时间。设置执行频率，随时暂停，并追踪每次结果。
        </p>
        {!editor && (
          <Button
            ref={createRef}
            onClick={() => {
              setEditor("new");
              setNotice("");
            }}
          >
            <Plus />
            新增任务
          </Button>
        )}
      </div>
      {error && (
        <div
          role="alert"
          className="flex flex-wrap items-center justify-between gap-3 rounded-xl border border-destructive/30 bg-card p-4 text-sm text-destructive"
        >
          任务加载失败：{error}
          <Button variant="outline" onClick={refresh}>
            重新加载
          </Button>
        </div>
      )}
      {notice && (
        <p
          role="status"
          className="rounded-xl border border-border bg-accent p-4 text-sm"
        >
          {notice}
        </p>
      )}
      {editor ? (
        <TaskEditor
          key={editor === "new" ? "new" : editor.id}
          task={editor === "new" ? null : editor}
          onCancel={closeEditor}
          onSaved={async () => {
            closeEditor();
            setNotice("任务已保存。执行计划已从保存时刻重新计算。");
            await refresh();
          }}
        />
      ) : (
        <>
          {loading ? (
            <p role="status" className="py-12 text-center text-muted">
              正在加载定时任务…
            </p>
          ) : !tasks.length && !error ? (
            <div className="rounded-2xl border border-dashed border-border bg-card px-6 py-16 text-center">
              <Clock3
                className="mx-auto mb-5 size-9 text-primary"
                aria-hidden="true"
              />
              <h2 className="text-lg font-semibold">让第一个请求按时发生</h2>
              <p className="mx-auto mb-6 mt-2 max-w-md text-sm leading-6 text-muted">
                定期检查服务、调用 API，或发送
                Webhook。选择执行间隔后，云母会在后台为你运行。
              </p>
              <Button onClick={() => setEditor("new")}>
                <Plus />
                创建 HTTP 任务
              </Button>
            </div>
          ) : (
            <section
              className="overflow-hidden rounded-2xl border border-border bg-card"
              aria-label="任务列表"
            >
              <div className="flex flex-wrap items-center justify-between gap-3 border-b border-border p-4 sm:px-5">
                <span className="text-sm text-muted">
                  {tasks.length} 个任务 ·{" "}
                  {tasks.filter((t) => t.enabled).length} 个已启用
                </span>
                <Input
                  aria-label="搜索任务"
                  placeholder="搜索名称或主机"
                  value={query}
                  onChange={(e) => setQuery(e.target.value)}
                  className="sm:max-w-64"
                />
              </div>
              {!filtered.length && (
                <p className="p-10 text-center text-sm text-muted">
                  没有匹配的任务，试试其他名称或主机。
                </p>
              )}
              {filtered.map((task) => (
                <article
                  key={task.id}
                  className="space-y-4 border-b border-border p-4 last:border-0 sm:p-5"
                >
                  <div className="flex flex-wrap items-start justify-between gap-3">
                    <div className="min-w-0">
                      <div className="flex flex-wrap items-center gap-2">
                        <h2 className="break-all font-semibold">{task.name}</h2>
                        <span className="text-xs text-muted">
                          {task.enabled ? "已启用" : "已暂停"}
                        </span>
                      </div>
                      <p className="mt-1 break-all text-sm text-muted">
                        HTTP 请求 · {task.request_summary}
                      </p>
                    </div>
                    <Status run={task.last_run} running={task.running} />
                  </div>
                  <div className="grid gap-2 text-sm sm:grid-cols-3">
                    <p>
                      <span className="text-muted">执行频率 </span>
                      <span
                        className={
                          task.timing.mode === "cron" ? "font-mono" : ""
                        }
                      >
                        {timingLabel(task.timing)}
                      </span>
                    </p>
                    <p>
                      <span className="text-muted">下次执行 </span>
                      <span className="tabular-nums">
                        {task.enabled ? date(task.next_run_at) : "暂停中"}
                      </span>
                    </p>
                    <p>
                      <span className="text-muted">上次结果 </span>
                      {task.last_run?.message ?? "等待首次执行"}
                    </p>
                  </div>
                  <div className="flex flex-wrap gap-2">
                    <Button
                      variant="outline"
                      disabled={busy === task.id || task.running}
                      onClick={() => action(task, "run")}
                    >
                      <Play />
                      立即执行
                    </Button>
                    <Button
                      variant="outline"
                      disabled={busy === task.id}
                      onClick={() => action(task, "enabled")}
                    >
                      {task.enabled ? <Pause /> : <Play />}
                      {task.enabled ? "暂停" : "启用"}
                    </Button>
                    <Button
                      variant="outline"
                      disabled={task.running || busy === task.id}
                      onClick={() => setEditor(task)}
                    >
                      <Pencil />
                      编辑
                    </Button>
                    <Button variant="outline" onClick={() => setHistory(task)}>
                      <History />
                      记录
                    </Button>
                    <Button
                      variant="outline"
                      aria-label={`删除 ${task.name}`}
                      disabled={task.running || busy === task.id}
                      onClick={() => {
                        setDeleteError("");
                        setDeleting(task);
                      }}
                      className="px-3 text-destructive"
                    >
                      <Trash2 />
                    </Button>
                  </div>
                </article>
              ))}
            </section>
          )}
          <p className="text-xs leading-6 text-muted">
            服务需保持运行；停机错过的计划恢复后最多补执行一次。每个任务保留最近
            100 条记录，不自动重试失败请求。
          </p>
        </>
      )}
      <Dialog
        open={!!history}
        onClose={() => setHistory(null)}
        title={`${history?.name ?? "任务"} · 执行记录`}
        description="最近 100 次执行；只记录状态和耗时，不保存响应正文。"
        panelClassName="max-w-3xl"
      >
        <div className="px-5 py-2 sm:px-6">
          {historyError && (
            <p role="alert" className="text-sm text-destructive">
              {historyError}
            </p>
          )}
          {historyLoading ? (
            <p role="status" className="py-8 text-center text-muted">
              正在加载记录…
            </p>
          ) : !runs.length && !historyError ? (
            <p className="py-8 text-center text-muted">
              还没有执行记录。可点击「立即执行」进行首次请求。
            </p>
          ) : (
            <div className="divide-y divide-border">
              {runs.map((run) => (
                <div key={run.id} className="space-y-2 py-4">
                  <div className="flex flex-wrap items-center justify-between gap-2">
                    <span className="text-sm tabular-nums">
                      {date(run.started_at)}
                    </span>
                    <Status run={run} />
                  </div>
                  <p className="text-sm">{run.message}</p>
                  <p className="text-xs text-muted">
                    {run.trigger === "manual" ? "手动执行" : "定时执行"} ·{" "}
                    {run.duration_ms == null
                      ? "耗时待定"
                      : `${run.duration_ms.toLocaleString()} ms`}
                    {run.status_code && ` · HTTP ${run.status_code}`}
                  </p>
                </div>
              ))}
            </div>
          )}
        </div>
      </Dialog>
      <Dialog
        open={!!deleting}
        onClose={() => {
          if (busy === null) setDeleting(null);
        }}
        title="删除定时任务"
        description={`删除「${deleting?.name ?? ""}」及其全部执行记录，此操作无法撤销。`}
        footer={
          <>
            <Button
              variant="outline"
              disabled={busy !== null}
              onClick={() => setDeleting(null)}
            >
              取消
            </Button>
            <Button
              variant="destructive"
              disabled={busy !== null}
              onClick={async () => {
                if (!deleting) return;
                setBusy(deleting.id);
                try {
                  await api(`${root}/${deleting.id}`, { method: "DELETE" });
                  setDeleting(null);
                  await refresh();
                } catch (e) {
                  setDeleteError(messageOf(e));
                } finally {
                  setBusy(null);
                }
              }}
            >
              {busy !== null ? "删除中…" : "删除任务"}
            </Button>
          </>
        }
      >
        <p
          role={deleteError ? "alert" : undefined}
          className="text-sm text-destructive"
        >
          {deleteError}
        </p>
      </Dialog>
    </div>
  );
}

function PairEditor({
  title,
  pairs,
  onChange,
  secret = false,
}: {
  title: string;
  pairs: Pair[];
  onChange: (next: Pair[]) => void;
  secret?: boolean;
}) {
  return (
    <div className="space-y-3">
      <p className="text-sm text-muted">
        {title === "查询参数"
          ? "参数会自动编码并附加到请求地址，支持同名参数。"
          : "无需手动设置 Content-Type；选择请求体格式后会自动添加。"}
      </p>
      {pairs.map((pair, index) => (
        <div
          key={index}
          className="grid grid-cols-[minmax(0,1fr)_minmax(0,1fr)_auto] gap-2"
        >
          <Input
            aria-label={`${title} ${index + 1} 名称`}
            placeholder="名称"
            value={pair.name}
            onChange={(e) =>
              onChange(
                pairs.map((p, i) =>
                  i === index ? { ...p, name: e.target.value } : p,
                ),
              )
            }
          />
          <Input
            aria-label={`${title} ${index + 1} 值`}
            type={secret ? "password" : "text"}
            autoComplete="off"
            placeholder="值"
            value={pair.value}
            onChange={(e) =>
              onChange(
                pairs.map((p, i) =>
                  i === index ? { ...p, value: e.target.value } : p,
                ),
              )
            }
          />
          <Button
            type="button"
            variant="outline"
            className="h-11 px-3"
            aria-label={`移除${title} ${index + 1}`}
            onClick={() => onChange(pairs.filter((_, i) => i !== index))}
          >
            <Trash2 />
          </Button>
        </div>
      ))}
      <Button
        type="button"
        variant="outline"
        onClick={() => onChange([...pairs, { name: "", value: "" }])}
      >
        <Plus />
        添加{title}
      </Button>
    </div>
  );
}

function TaskEditor({
  task,
  onSaved,
  onCancel,
}: {
  task: Task | null;
  onSaved: () => Promise<void>;
  onCancel: () => void;
}) {
  const [name, setName] = useState(task?.name ?? "");
  const [enabled, setEnabled] = useState(task?.enabled ?? true);
  const [mode, setMode] = useState<"interval" | "cron">(
    task?.timing.mode ?? "interval",
  );
  const initialMinutes =
    task?.timing.mode === "interval" ? task.timing.minutes : 60;
  const [preset, setPreset] = useState(
    intervals.some((i) => i.value === String(initialMinutes))
      ? String(initialMinutes)
      : "custom",
  );
  const [amount, setAmount] = useState(String(initialMinutes));
  const [unit, setUnit] = useState("1");
  const [cron, setCron] = useState(
    task?.timing.mode === "cron" ? task.timing.expression : "0 9 * * *",
  );
  const [offset, setOffset] = useState(
    task?.timing.mode === "cron"
      ? task.timing.utc_offset_minutes
      : -new Date().getTimezoneOffset(),
  );
  const [http, setHttp] = useState(initialHttp);
  const [replaceHttp, setReplaceHttp] = useState(!task);
  const [loadingHttp, setLoadingHttp] = useState(false);
  const [tab, setTab] = useState("query");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const [preview, setPreview] = useState<string[]>([]);
  const [previewError, setPreviewError] = useState("");
  const nameRef = useRef<HTMLInputElement>(null);
  useEffect(() => {
    nameRef.current?.focus();
  }, []);
  const minutes =
    preset === "custom" ? Number(amount) * Number(unit) : Number(preset);
  const timing: Timing =
    mode === "interval"
      ? { mode, minutes }
      : { mode, expression: cron, utc_offset_minutes: offset };
  const timingJson = JSON.stringify(timing);
  useEffect(() => {
    let current = true;
    setPreview([]);
    setPreviewError("");
    const timer = setTimeout(async () => {
      try {
        const result = await api<string[]>(`${root}/preview`, {
          method: "POST",
          body: timingJson,
        });
        if (current) setPreview(result);
      } catch (e) {
        if (current) setPreviewError(messageOf(e));
      }
    }, 350);
    return () => {
      current = false;
      clearTimeout(timer);
    };
  }, [timingJson]);
  function patch(value: Partial<HttpConfig>) {
    setHttp((current) => ({ ...current, ...value }));
  }
  async function save(e: FormEvent) {
    e.preventDefault();
    setError("");
    if (!name.trim()) {
      setError("请填写任务名称。");
      nameRef.current?.focus();
      return;
    }
    if (
      mode === "interval" &&
      (!Number.isInteger(minutes) || minutes < 1 || minutes > 525600)
    ) {
      setError("执行间隔应为 1 分钟至 365 天，且为整数分钟。");
      return;
    }
    if (replaceHttp && http.body_type === "json") {
      try {
        JSON.parse(http.body);
      } catch {
        setError("请求体不是有效的 JSON，请检查后保存。");
        setTab("body");
        return;
      }
    }
    setSaving(true);
    try {
      await api(task ? `${root}/${task.id}` : root, {
        method: task ? "PUT" : "POST",
        body: JSON.stringify({
          name,
          enabled,
          task_type: "http",
          timing,
          http: replaceHttp ? http : null,
        }),
      });
      await onSaved();
    } catch (e) {
      setError(messageOf(e));
    } finally {
      setSaving(false);
    }
  }
  const tabs = [
    {
      value: "query",
      label: `查询参数${http.query.length ? ` (${http.query.length})` : ""}`,
    },
    {
      value: "headers",
      label: `请求头${http.headers.length ? ` (${http.headers.length})` : ""}`,
    },
    { value: "auth", label: "认证" },
    { value: "body", label: "请求体" },
    { value: "preview", label: "请求预览" },
  ];
  return (
    <form
      onSubmit={save}
      className="overflow-hidden rounded-2xl border border-border bg-card"
    >
      <div className="flex items-center gap-3 border-b border-border p-4 sm:px-6">
        <Button
          type="button"
          variant="outline"
          className="px-3"
          aria-label="返回任务列表"
          disabled={saving}
          onClick={onCancel}
        >
          <ArrowLeft />
        </Button>
        <h2 className="text-lg font-semibold">
          {task ? "编辑定时任务" : "新增定时任务"}
        </h2>
      </div>
      <fieldset disabled={saving} className="min-w-0">
        <div className="grid min-w-0 lg:grid-cols-[minmax(0,1fr)_320px]">
          <div className="min-w-0 space-y-7 p-4 sm:p-6">
            <div className="grid gap-4 sm:grid-cols-[minmax(0,1fr)_180px]">
              <div className="space-y-2">
                <label htmlFor="scheduled-name" className="text-sm font-medium">
                  任务名称 <span className="text-muted">必填</span>
                </label>
                <Input
                  id="scheduled-name"
                  ref={nameRef}
                  maxLength={100}
                  placeholder="例如：检查媒体服务"
                  value={name}
                  onChange={(e) => setName(e.target.value)}
                />
              </div>
              <div className="space-y-2">
                <label htmlFor="scheduled-type" className="text-sm font-medium">
                  任务类型
                </label>
                <Select
                  id="scheduled-type"
                  value="http"
                  onChange={() => {}}
                  options={[{ value: "http", label: "HTTP 请求" }]}
                />
              </div>
            </div>
            <section className="space-y-4" aria-labelledby="request-title">
              <div>
                <h3
                  id="request-title"
                  className="flex items-center gap-2 font-semibold"
                >
                  <Globe className="size-4 text-primary" aria-hidden="true" />
                  请求配置
                </h3>
                <p className="mt-1 text-sm leading-6 text-muted">
                  配置请求地址与内容，并在下方选择发送方式。
                </p>
              </div>
              {task && (
                <div className="space-y-3 rounded-xl bg-accent p-4">
                  <p className="break-all text-sm">
                    已保存：{task.request_summary}
                  </p>
                  <p className="text-xs leading-6 text-muted">
                    调整名称或时间会保留原请求。载入后可查看、修改已保存的认证与请求体，认证值默认隐藏。
                  </p>
                  <div className="flex flex-wrap gap-2">
                    <Button
                      type="button"
                      variant="outline"
                      disabled={loadingHttp || replaceHttp}
                      onClick={async () => {
                        setLoadingHttp(true);
                        setError("");
                        try {
                          const value = await api<HttpConfig>(
                            `${root}/${task.id}/http-config`,
                            { method: "POST" },
                          );
                          setHttp(normalizeHttp(value));
                          setReplaceHttp(true);
                        } catch (e) {
                          setError(messageOf(e));
                        } finally {
                          setLoadingHttp(false);
                        }
                      }}
                    >
                      {loadingHttp
                        ? "载入中…"
                        : replaceHttp
                          ? "已载入，保存时更新请求"
                          : "载入已保存配置"}
                    </Button>
                    {replaceHttp && (
                      <Button
                        type="button"
                        variant="outline"
                        onClick={() => {
                          setReplaceHttp(false);
                          setHttp(initialHttp());
                        }}
                      >
                        取消请求修改
                      </Button>
                    )}
                  </div>
                </div>
              )}
              {replaceHttp && (
                <>
                  <div className="grid gap-3 sm:grid-cols-[116px_minmax(0,1fr)]">
                    <div className="space-y-2">
                      <label
                        htmlFor="http-method"
                        className="text-sm font-medium"
                      >
                        方法
                      </label>
                      <Select
                        id="http-method"
                        value={http.method}
                        options={methods}
                        onChange={(method) =>
                          patch({
                            method,
                            ...(["GET", "HEAD"].includes(method)
                              ? { body_type: "none", body: "" }
                              : {}),
                          })
                        }
                      />
                    </div>
                    <div className="space-y-2">
                      <label htmlFor="http-url" className="text-sm font-medium">
                        请求地址 <span className="text-muted">必填</span>
                      </label>
                      <Input
                        id="http-url"
                        type="url"
                        required
                        autoComplete="off"
                        placeholder="https://example.com/api/health"
                        value={http.url}
                        onChange={(e) => patch({ url: e.target.value })}
                        className="font-mono text-sm"
                      />
                    </div>
                  </div>
                  <div
                    className="flex flex-wrap gap-1 border-b border-border pb-2"
                    aria-label="请求配置分区"
                  >
                    {tabs.map((item) => (
                      <button
                        type="button"
                        key={item.value}
                        aria-pressed={tab === item.value}
                        onClick={() => setTab(item.value)}
                        className={cn(
                          "min-h-11 rounded-lg px-3 text-sm transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
                          tab === item.value
                            ? "bg-secondary font-semibold text-secondary-foreground"
                            : "text-muted hover:bg-accent",
                        )}
                      >
                        {item.label}
                      </button>
                    ))}
                  </div>
                  <div className="min-h-40">
                    {tab === "query" && (
                      <PairEditor
                        title="查询参数"
                        pairs={http.query}
                        onChange={(query) => patch({ query })}
                      />
                    )}
                    {tab === "headers" && (
                      <PairEditor
                        title="请求头"
                        pairs={http.headers}
                        secret
                        onChange={(headers) => patch({ headers })}
                      />
                    )}
                    {tab === "auth" && (
                      <HttpAuthEditor http={http} patch={patch} />
                    )}
                    {tab === "body" && (
                      <HttpBodyEditor http={http} patch={patch} />
                    )}
                    {tab === "preview" && <HttpRequestPreview http={http} />}
                  </div>
                  <HttpDeliveryEditor http={http} patch={patch} />
                  <details className="border-t border-border pt-4">
                    <summary className="cursor-pointer py-2 text-sm font-medium focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">
                      高级选项 · 超时、成功条件与重定向
                    </summary>
                    <div className="mt-4 grid gap-4 sm:grid-cols-2">
                      <div className="space-y-2">
                        <label htmlFor="http-timeout" className="text-sm">
                          超时（秒）
                        </label>
                        <Input
                          id="http-timeout"
                          type="number"
                          min={1}
                          max={300}
                          required
                          value={http.timeout_seconds}
                          onChange={(e) =>
                            patch({ timeout_seconds: Number(e.target.value) })
                          }
                        />
                      </div>
                      <div className="space-y-2">
                        <label htmlFor="http-status" className="text-sm">
                          预期状态码
                        </label>
                        <Input
                          id="http-status"
                          type="number"
                          min={100}
                          max={599}
                          placeholder="留空：任意 2xx"
                          value={http.expected_status ?? ""}
                          onChange={(e) =>
                            patch({
                              expected_status: e.target.value
                                ? Number(e.target.value)
                                : null,
                            })
                          }
                        />
                      </div>
                      <label className="flex items-center gap-2 text-sm sm:col-span-2">
                        <input
                          type="checkbox"
                          className="size-4 accent-primary"
                          checked={http.follow_redirects}
                          onChange={(e) =>
                            patch({ follow_redirects: e.target.checked })
                          }
                        />
                        跟随重定向（最多 5 次）
                      </label>
                      <p className="text-xs leading-6 text-muted sm:col-span-2">
                        失败不自动重试，避免重复提交。
                      </p>
                    </div>
                  </details>
                </>
              )}
            </section>
          </div>
          <aside
            className="min-w-0 space-y-5 border-t border-border bg-background/60 p-4 sm:p-6 lg:border-l lg:border-t-0"
            aria-labelledby="timing-title"
          >
            <h3
              id="timing-title"
              className="flex items-center gap-2 font-semibold"
            >
              <Clock3 className="size-4 text-primary" aria-hidden="true" />
              执行计划
            </h3>
            <div className="space-y-2">
              <label htmlFor="timing-mode" className="text-sm font-medium">
                设置方式
              </label>
              <Select
                id="timing-mode"
                value={mode}
                options={[
                  { value: "interval", label: "指定间隔" },
                  { value: "cron", label: "高级CRON表达式" },
                ]}
                onChange={(value) => setMode(value as typeof mode)}
              />
            </div>
            {mode === "interval" ? (
              <div className="space-y-4">
                <div className="space-y-2">
                  <label
                    htmlFor="timing-preset"
                    className="text-sm font-medium"
                  >
                    多久执行一次
                  </label>
                  <Select
                    id="timing-preset"
                    value={preset}
                    options={intervals}
                    onChange={setPreset}
                  />
                </div>
                {preset === "custom" && (
                  <div className="grid grid-cols-2 gap-2">
                    <div className="space-y-2">
                      <label htmlFor="timing-amount" className="text-sm">
                        每隔
                      </label>
                      <Input
                        id="timing-amount"
                        type="number"
                        min={1}
                        required
                        value={amount}
                        onChange={(e) => setAmount(e.target.value)}
                      />
                    </div>
                    <div className="space-y-2">
                      <label htmlFor="timing-unit" className="text-sm">
                        单位
                      </label>
                      <Select
                        id="timing-unit"
                        value={unit}
                        options={[
                          { value: "1", label: "分钟" },
                          { value: "60", label: "小时" },
                          { value: "1440", label: "天" },
                          { value: "10080", label: "周" },
                        ]}
                        onChange={setUnit}
                      />
                    </div>
                  </div>
                )}
                <p className="text-xs leading-6 text-muted">
                  从保存或启用时开始计时。首次执行会等待一个间隔；指定每天几点执行，请切换
                  Cron。
                </p>
              </div>
            ) : (
              <div className="space-y-4">
                <div className="space-y-2">
                  <label htmlFor="timing-cron" className="text-sm font-medium">
                    Cron 表达式
                  </label>
                  <Input
                    id="timing-cron"
                    required
                    spellCheck={false}
                    value={cron}
                    onChange={(e) => setCron(e.target.value)}
                    className="font-mono"
                    aria-describedby="cron-help"
                  />
                  <p id="cron-help" className="text-xs leading-6 text-muted">
                    分 · 时 · 日 · 月 · 星期
                    <br />
                    例如 0 9 * * MON-FRI：工作日 09:00。星期建议使用 MON–SUN。
                  </p>
                </div>
                <div className="space-y-2">
                  <label
                    htmlFor="timing-offset"
                    className="text-sm font-medium"
                  >
                    时区（固定 UTC 偏移）
                  </label>
                  <Select
                    id="timing-offset"
                    value={String(offset)}
                    options={Array.from(
                      new Set([
                        offset,
                        0,
                        480,
                        ...Array.from({ length: 27 }, (_, i) => (i - 12) * 60),
                      ]),
                    )
                      .sort((a, b) => a - b)
                      .map((value) => ({
                        value: String(value),
                        label: `UTC${value < 0 ? "−" : "+"}${String(Math.floor(Math.abs(value) / 60)).padStart(2, "0")}:${String(Math.abs(value) % 60).padStart(2, "0")}${value === 480 ? " · 北京时间" : ""}`,
                      }))}
                    onChange={(v) => setOffset(Number(v))}
                  />
                  <p className="text-xs text-muted">
                    固定偏移，不随夏令时变化。
                  </p>
                </div>
              </div>
            )}
            <div className="space-y-3 border-t border-border pt-5">
              <label className="flex items-center gap-2 text-sm font-medium">
                <input
                  type="checkbox"
                  className="size-4 accent-primary"
                  checked={enabled}
                  onChange={(e) => setEnabled(e.target.checked)}
                />
                保存后启用
              </label>
              <p className="text-xs leading-6 text-muted">
                {enabled
                  ? "启用后自动运行；立即执行不会改变计划。"
                  : "保存为暂停状态，可随时手动执行或启用。"}
              </p>
            </div>
            <div className="border-t border-border pt-5">
              <h4 className="text-sm font-medium">
                {enabled ? "预计接下来 3 次" : "启用后的计划预览"}
              </h4>
              <p className="mt-1 text-xs text-muted">
                以下时间按浏览器本地时区显示
              </p>
              <div aria-live="polite" className="mt-3">
                {previewError ? (
                  <p className="text-sm leading-6 text-destructive">
                    {previewError}
                  </p>
                ) : preview.length ? (
                  <ol className="space-y-2 text-sm tabular-nums">
                    {preview.map((value) => (
                      <li key={value}>{date(value)}</li>
                    ))}
                  </ol>
                ) : (
                  <p className="text-sm text-muted">正在计算…</p>
                )}
              </div>
            </div>
          </aside>
        </div>
      </fieldset>
      <div className="space-y-3 border-t border-border p-4 sm:px-6">
        {error && (
          <p role="alert" className="text-sm text-destructive">
            {error}
          </p>
        )}
        <div className="flex flex-wrap items-center justify-between gap-3">
          <p className="text-xs leading-6 text-muted">
            保存后可通过「立即执行」发送真实请求并查看记录。
          </p>
          <div className="flex gap-2">
            <Button
              type="button"
              variant="outline"
              disabled={saving}
              onClick={onCancel}
            >
              取消
            </Button>
            <Button
              type="submit"
              disabled={saving || loadingHttp || !!previewError}
            >
              <Send />
              {saving ? "保存中…" : "保存任务"}
            </Button>
          </div>
        </div>
      </div>
    </form>
  );
}
