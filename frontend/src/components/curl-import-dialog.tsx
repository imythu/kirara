import { useRef, useState } from "react";
import { ClipboardPaste, Eye, EyeOff } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Dialog } from "@/components/ui/dialog";
import { api } from "@/lib/api";
import type { HttpConfig } from "./scheduled-http-panel";

type Imported = { http: HttpConfig; notes: string[] };
const bodyNames: Record<string, string> = {
  none: "无请求体",
  json: "JSON",
  text: "纯文本",
  xml: "XML",
  html: "HTML",
  raw: "自定义原文",
  multipart: "multipart 表单",
};
export function CurlImportDialog({
  onImport,
  disabled,
}: {
  onImport: (http: HttpConfig) => void;
  disabled?: boolean;
}) {
  const [open, setOpen] = useState(false);
  const [command, setCommand] = useState("");
  const [parsed, setParsed] = useState<Imported | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [reveal, setReveal] = useState(false);
  const generation = useRef(0);
  function close() {
    generation.current++;
    setOpen(false);
    setCommand("");
    setParsed(null);
    setError("");
    setBusy(false);
    setReveal(false);
  }
  async function parse() {
    const current = ++generation.current;
    setBusy(true);
    setError("");
    setParsed(null);
    setReveal(false);
    try {
      const value = await api<Imported>("/api/scheduled-tasks/import-curl", {
        method: "POST",
        body: JSON.stringify({ command }),
      });
      if (current === generation.current) setParsed(value);
    } catch (e) {
      if (current === generation.current)
        setError(
          e instanceof Error ? e.message : "解析失败，请检查命令后重试。",
        );
    } finally {
      if (current === generation.current) setBusy(false);
    }
  }
  const http = parsed?.http;
  return (
    <>
      <Button
        type="button"
        variant="outline"
        disabled={disabled}
        onClick={() => setOpen(true)}
      >
        <ClipboardPaste />
        导入 cURL
      </Button>
      <Dialog
        open={open}
        onClose={close}
        title="导入 cURL（Bash）"
        description="粘贴浏览器复制的 cURL（bash）或单条 curl 命令。仅解析文本，不执行命令、不发送请求。"
        panelClassName="max-w-3xl"
        footer={
          <div className="flex flex-wrap justify-end gap-2">
            <Button type="button" variant="outline" onClick={close}>
              取消
            </Button>
            <Button
              type="button"
              variant={parsed ? "outline" : "default"}
              disabled={busy || !command.trim()}
              onClick={parse}
            >
              {busy ? "解析中…" : "解析命令"}
            </Button>
            {parsed && (
              <Button
                type="button"
                disabled={busy}
                onClick={() => {
                  onImport(parsed.http);
                  close();
                }}
              >
                应用到请求配置
              </Button>
            )}
          </div>
        }
      >
        <div className="space-y-5 p-5 sm:p-6">
          <div className="space-y-2">
            <label htmlFor="curl-command" className="text-sm font-medium">
              cURL 命令
            </label>
            <textarea
              id="curl-command"
              rows={7}
              autoComplete="off"
              autoCorrect="off"
              autoCapitalize="none"
              spellCheck={false}
              value={command}
              maxLength={524288}
              placeholder={
                "curl 'https://example.com/api' \\\n  -H 'Content-Type: application/json' \\\n  --data-raw '{\"message\":\"Hello\"}'"
              }
              className="w-full resize-y rounded-lg border border-border bg-input p-3 font-mono text-sm leading-6 outline-none focus:ring-2 focus:ring-ring"
              onChange={(e) => {
                generation.current++;
                setCommand(e.target.value);
                setParsed(null);
                setError("");
                setBusy(false);
                setReveal(false);
              }}
              aria-describedby="curl-help"
            />
            <p id="curl-help" className="text-xs leading-6 text-muted">
              支持 Bash
              引号、多行续写、请求头、Cookie、Basic/Bearer、正文、文本表单、重定向和超时。本地文件、变量、管道及不支持的选项会提示处理。
            </p>
          </div>
          {error && (
            <p
              role="alert"
              className="rounded-lg bg-destructive/10 p-3 text-sm leading-6 text-destructive"
            >
              {error}
            </p>
          )}
          {http && (
            <section
              className="space-y-4 border-t border-border pt-4"
              aria-labelledby="curl-result-title"
            >
              <div className="flex flex-wrap items-center justify-between gap-2">
                <h3 id="curl-result-title" className="font-semibold">
                  解析结果
                </h3>
                <Button
                  type="button"
                  variant="outline"
                  aria-pressed={reveal}
                  onClick={() => setReveal(!reveal)}
                >
                  {reveal ? <EyeOff /> : <Eye />}
                  {reveal ? "隐藏完整值" : "显示完整值"}
                </Button>
              </div>
              <p role="status" className="break-all font-mono text-sm">
                {http.method} · {new URL(http.url).host}
              </p>
              <dl className="grid grid-cols-2 gap-3 text-sm">
                <div>
                  <dt className="text-muted">请求头</dt>
                  <dd>{http.headers.length} 项</dd>
                </div>
                <div>
                  <dt className="text-muted">认证</dt>
                  <dd>
                    {
                      {
                        none: "无独立认证 / 保留原始头",
                        bearer: "Bearer Token",
                        basic: "Basic Auth",
                        cookie: "Cookie",
                        api_key: "API Key",
                      }[http.auth.type]
                    }
                  </dd>
                </div>
                <div>
                  <dt className="text-muted">请求体</dt>
                  <dd>
                    {bodyNames[http.body_type] ?? http.body_type}
                    {http.form_fields.length
                      ? ` · ${http.form_fields.length} 个字段`
                      : ""}
                  </dd>
                </div>
                <div>
                  <dt className="text-muted">超时 / 重定向</dt>
                  <dd>
                    {http.timeout_seconds} 秒 /{" "}
                    {http.follow_redirects ? "跟随" : "不跟随"}
                  </dd>
                </div>
              </dl>
              {reveal && (
                <pre
                  aria-label="cURL 解析详情"
                  className="max-h-64 overflow-auto whitespace-pre-wrap break-all rounded-lg bg-background p-3 font-mono text-xs leading-6"
                >
                  {JSON.stringify(
                    {
                      url: http.url,
                      headers: http.headers,
                      auth: http.auth,
                      body: http.body,
                      form_fields: http.form_fields,
                    },
                    null,
                    2,
                  )}
                </pre>
              )}
              <ul className="list-disc space-y-2 pl-5 text-xs leading-6 text-muted">
                {parsed!.notes.map((note) => (
                  <li key={note}>{note}</li>
                ))}
              </ul>
              <p className="rounded-lg bg-accent p-3 text-sm leading-6">
                应用后替换当前请求的地址、方法、请求头、认证和请求体；任务名称、执行计划及发送方式保持不变。应用后仍需保存任务才会生效。
              </p>
            </section>
          )}
        </div>
      </Dialog>
    </>
  );
}
