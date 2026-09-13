import { useEffect, useRef, useState } from "react";
import { Eye, EyeOff, Plus, Trash2, FileUp } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Select } from "@/components/ui/select";
import { api } from "@/lib/api";
import { SiteCookieInput } from "@/components/site-cookie-input";

export type Pair = { name: string; value: string };
export type Auth =
  | { type: "none" }
  | { type: "bearer"; token: string }
  | { type: "basic"; username: string; password: string }
  | {
      type: "api_key";
      name: string;
      value: string;
      location: "header" | "query";
    }
  | { type: "cookie"; value: string };
export type FilePayload = {
  name: string;
  content_type: string;
  data_base64: string;
};
export type FormField = Pair & { file: FilePayload | null };
export type HttpConfig = {
  send_via_browser: boolean;
  use_global_proxy: boolean;
  browser_use_global_proxy: boolean;
  method: string;
  url: string;
  query: Pair[];
  headers: Pair[];
  bearer_token: string;
  auth: Auth;
  body: string;
  body_type: string;
  form_fields: FormField[];
  binary: FilePayload | null;
  content_type: string;
  timeout_seconds: number;
  follow_redirects: boolean;
  expected_status: number | null;
};
export const initialHttp = (): HttpConfig => ({
  send_via_browser: false,
  use_global_proxy: false,
  browser_use_global_proxy: false,
  method: "GET",
  url: "",
  query: [],
  headers: [],
  bearer_token: "",
  auth: { type: "none" },
  body: "",
  body_type: "none",
  form_fields: [],
  binary: null,
  content_type: "application/octet-stream",
  timeout_seconds: 30,
  follow_redirects: false,
  expected_status: null,
});
export function normalizeHttp(value: HttpConfig): HttpConfig {
  return {
    ...initialHttp(),
    ...value,
    auth:
      value.auth ??
      (value.bearer_token
        ? { type: "bearer", token: value.bearer_token }
        : { type: "none" }),
    bearer_token: "",
  };
}
const errorMessage = (e: unknown) =>
  e instanceof Error ? e.message : "操作失败，请重试";
function defaultAuth(type: string): Auth {
  switch (type) {
    case "bearer":
      return { type, token: "" };
    case "basic":
      return { type, username: "", password: "" };
    case "api_key":
      return { type, name: "X-API-Key", value: "", location: "header" };
    case "cookie":
      return { type, value: "" };
    default:
      return { type: "none" };
  }
}
function authText(auth: Auth, reveal: boolean) {
  const hide = (v: string) => (reveal ? v : "••••••••");
  switch (auth.type) {
    case "bearer":
      return `Authorization: Bearer ${hide(auth.token)}`;
    case "basic": {
      const encoded = btoa(
        Array.from(
          new TextEncoder().encode(`${auth.username}:${auth.password}`),
          (b) => String.fromCharCode(b),
        ).join(""),
      );
      return `Authorization: Basic ${hide(encoded)}`;
    }
    case "api_key":
      return auth.location === "header"
        ? `${auth.name}: ${hide(auth.value)}`
        : `${new URLSearchParams([[auth.name, ""]]).toString()}${reveal ? new URLSearchParams([[auth.name, auth.value]]).toString().split("=").slice(1).join("=") : "••••••••"}`;
    case "cookie":
      return `Cookie: ${hide(auth.value)}`;
    default:
      return "不自动添加认证信息；手动填写的请求头仍会发送。";
  }
}
export function HttpAuthEditor({
  http,
  patch,
}: {
  http: HttpConfig;
  patch: (p: Partial<HttpConfig>) => void;
}) {
  const [reveal, setReveal] = useState(false);
  const auth = http.auth;
  const [drafts, setDrafts] = useState<Partial<Record<Auth["type"], Auth>>>({});
  const update = (next: Auth) => patch({ auth: next, bearer_token: "" });
  return (
    <div className="space-y-4">
      <div className="space-y-2">
        <label htmlFor="http-auth-type" className="text-sm font-medium">
          认证方式
        </label>
        <Select
          id="http-auth-type"
          value={auth.type}
          options={[
            { value: "none", label: "无认证" },
            { value: "bearer", label: "Bearer Token" },
            { value: "basic", label: "Basic Auth（用户名与密码）" },
            { value: "api_key", label: "API Key" },
            { value: "cookie", label: "Cookie" },
          ]}
          onChange={(value) => {
            setDrafts({ ...drafts, [auth.type]: auth });
            update(drafts[value as Auth["type"]] ?? defaultAuth(value));
            setReveal(false);
          }}
        />
      </div>
      {auth.type === "bearer" && (
        <div className="space-y-2">
          <label htmlFor="http-token" className="text-sm font-medium">
            Token
          </label>
          <Input
            id="http-token"
            type={reveal ? "text" : "password"}
            autoComplete="off"
            placeholder="无需 Bearer 前缀"
            value={auth.token}
            onChange={(e) => update({ ...auth, token: e.target.value })}
          />
        </div>
      )}
      {auth.type === "basic" && (
        <div className="grid gap-3 sm:grid-cols-2">
          <div className="space-y-2">
            <label htmlFor="http-auth-user" className="text-sm font-medium">
              用户名
            </label>
            <Input
              id="http-auth-user"
              autoComplete="off"
              value={auth.username}
              onChange={(e) => update({ ...auth, username: e.target.value })}
            />
          </div>
          <div className="space-y-2">
            <label htmlFor="http-auth-password" className="text-sm font-medium">
              密码
            </label>
            <Input
              id="http-auth-password"
              autoComplete="new-password"
              type={reveal ? "text" : "password"}
              value={auth.password}
              onChange={(e) => update({ ...auth, password: e.target.value })}
            />
          </div>
        </div>
      )}
      {auth.type === "api_key" && (
        <>
          <div className="space-y-2">
            <label htmlFor="http-key-location" className="text-sm font-medium">
              添加位置
            </label>
            <Select
              id="http-key-location"
              value={auth.location}
              options={[
                { value: "header", label: "请求头（Header）" },
                { value: "query", label: "查询参数（Query）" },
              ]}
              onChange={(location) =>
                update({ ...auth, location: location as "header" | "query" })
              }
            />
          </div>
          <div className="grid gap-3 sm:grid-cols-2">
            <div className="space-y-2">
              <label htmlFor="http-key-name" className="text-sm font-medium">
                参数名称
              </label>
              <Input
                id="http-key-name"
                placeholder="X-API-Key"
                value={auth.name}
                onChange={(e) => update({ ...auth, name: e.target.value })}
              />
            </div>
            <div className="space-y-2">
              <label htmlFor="http-key-value" className="text-sm font-medium">
                API Key 值
              </label>
              <Input
                id="http-key-value"
                autoComplete="off"
                type={reveal ? "text" : "password"}
                value={auth.value}
                onChange={(e) => update({ ...auth, value: e.target.value })}
              />
            </div>
          </div>
        </>
      )}
      {auth.type === "cookie" && (
        <div className="space-y-2">
          <label htmlFor="http-cookie" className="text-sm font-medium">
            Cookie 内容
          </label>
          <SiteCookieInput
            value={auth.value}
            reveal={reveal}
            onChange={(value) => update({ ...auth, value })}
          />
        </div>
      )}
      <div className="space-y-3 border-t border-border pt-4">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <h4 className="text-sm font-medium">
            最终认证信息
            {auth.type === "api_key" && auth.location === "query"
              ? " · 查询参数"
              : ""}
          </h4>
          {auth.type !== "none" && (
            <Button
              type="button"
              variant="outline"
              aria-pressed={reveal}
              onClick={() => setReveal(!reveal)}
            >
              {reveal ? <EyeOff /> : <Eye />}
              {reveal ? "隐藏认证值" : "显示认证值"}
            </Button>
          )}
        </div>
        <pre
          className="whitespace-pre-wrap break-all rounded-lg bg-background p-3 font-mono text-sm leading-6"
          aria-label="最终认证信息"
        >
          {authText(auth, reveal)}
        </pre>
        <p className="text-xs leading-6 text-muted">
          {auth.type === "basic"
            ? "用户名与密码按 UTF-8 编码，再生成 Base64 认证头。"
            : auth.type === "api_key" && auth.location === "query"
              ? "自动编码并附加到 URL，请勿在地址或查询参数中重复添加同名字段。"
              : auth.type === "none"
                ? "如需认证，可从上方选择服务要求的方式。"
                : "以上内容会自动添加到请求中，请勿在请求头中重复设置。"}
        </p>
      </div>
    </div>
  );
}
async function readFile(file: File): Promise<FilePayload> {
  if (file.size > 262144)
    throw Error("单个文件最大 256 KiB，请选择较小的文件。");
  const bytes = new Uint8Array(await file.arrayBuffer());
  return {
    name: file.name,
    content_type: file.type || "application/octet-stream",
    data_base64: btoa(
      Array.from(bytes, (b) => String.fromCharCode(b)).join(""),
    ),
  };
}
function FileField({
  id,
  value,
  onChange,
}: {
  id: string;
  value: FilePayload | null;
  onChange: (value: FilePayload | null) => void;
}) {
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);
  const generation = useRef(0);
  const inputRef = useRef<HTMLInputElement>(null);
  useEffect(
    () => () => {
      generation.current++;
    },
    [],
  );
  return (
    <div className="space-y-2">
      <label htmlFor={id} className="flex items-center gap-2 text-sm">
        <FileUp className="size-4" aria-hidden="true" />
        {value ? "替换文件" : "选择文件"}
      </label>
      <Button
        type="button"
        variant="outline"
        disabled={loading}
        onClick={() => inputRef.current?.click()}
      >
        <FileUp />
        {value ? "选择其他文件" : "浏览文件"}
      </Button>
      <input
        id={id}
        ref={inputRef}
        type="file"
        tabIndex={-1}
        disabled={loading}
        className="sr-only"
        onChange={async (e) => {
          const file = e.target.files?.[0];
          e.target.value = "";
          if (!file) return;
          const version = ++generation.current;
          setLoading(true);
          setError("");
          try {
            const next = await readFile(file);
            if (version === generation.current) onChange(next);
          } catch (e) {
            if (version === generation.current) setError(errorMessage(e));
          } finally {
            if (version === generation.current) setLoading(false);
          }
        }}
      />
      {loading && (
        <p role="status" className="text-xs text-muted">
          正在读取文件…
        </p>
      )}
      {value && (
        <div className="space-y-2">
          <p className="break-all text-xs text-muted">
            {value.name} · 文件内容随任务保存，每次执行发送同一份内容。
          </p>
          <label htmlFor={`${id}-mime`} className="text-sm">
            文件 Content-Type
          </label>
          <Input
            id={`${id}-mime`}
            value={value.content_type}
            onChange={(e) =>
              onChange({ ...value, content_type: e.target.value })
            }
          />
          <Button
            type="button"
            variant="outline"
            onClick={() => onChange(null)}
          >
            移除文件
          </Button>
        </div>
      )}
      {error && (
        <p role="alert" className="text-sm text-destructive">
          {error}
        </p>
      )}
    </div>
  );
}
export function HttpBodyEditor({
  http,
  patch,
}: {
  http: HttpConfig;
  patch: (p: Partial<HttpConfig>) => void;
}) {
  const [error, setError] = useState("");
  const form =
    http.body_type === "multipart" || http.body_type === "urlencoded";
  const fields = http.form_fields;
  const update = (index: number, value: Partial<FormField>) =>
    patch({
      form_fields: fields.map((field, i) =>
        i === index ? { ...field, ...value } : field,
      ),
    });
  return (
    <div className="space-y-4">
      <div className="space-y-2">
        <label htmlFor="http-body-type" className="text-sm font-medium">
          请求体类型
        </label>
        <Select
          id="http-body-type"
          value={http.body_type}
          disabled={["GET", "HEAD"].includes(http.method)}
          options={[
            { value: "none", label: "无请求体" },
            { value: "json", label: "JSON · application/json" },
            { value: "urlencoded", label: "表单 · x-www-form-urlencoded" },
            { value: "multipart", label: "表单与文件 · multipart/form-data" },
            { value: "text", label: "纯文本 · text/plain" },
            { value: "xml", label: "XML · application/xml" },
            { value: "html", label: "HTML · text/html" },
            { value: "binary", label: "二进制文件 · binary" },
            { value: "raw", label: "自定义 · raw" },
          ]}
          onChange={(body_type) => {
            patch({
              body_type,
              ...(body_type === "urlencoded"
                ? {
                    form_fields: http.form_fields.map((field) => ({
                      ...field,
                      file: null,
                    })),
                  }
                : {}),
            });
            setError("");
          }}
        />
      </div>
      {["GET", "HEAD"].includes(http.method) && (
        <p className="text-sm text-muted">
          GET / HEAD 使用查询参数；发送请求体请选择 POST、PUT 或 PATCH 等方法。
        </p>
      )}
      {form && (
        <div className="space-y-4">
          <p className="text-xs leading-6 text-muted">
            {http.body_type === "multipart"
              ? "支持同名字段及多个文件；Content-Type 与 boundary 自动生成。"
              : "字段自动进行 URL 编码，支持同名字段。文件仅支持 multipart 类型。"}
            内容和文件合计最大 256 KiB。
          </p>
          {fields.map((field, index) => (
            <div key={index} className="space-y-3 border-b border-border pb-4">
              <div className="grid grid-cols-[minmax(0,1fr)_auto] gap-2">
                <Input
                  aria-label={`表单字段 ${index + 1} 名称`}
                  placeholder="字段名称"
                  value={field.name}
                  onChange={(e) => update(index, { name: e.target.value })}
                />
                <Button
                  type="button"
                  variant="outline"
                  className="h-11 px-3"
                  aria-label={`移除表单字段 ${index + 1}`}
                  onClick={() =>
                    patch({ form_fields: fields.filter((_, i) => i !== index) })
                  }
                >
                  <Trash2 />
                </Button>
              </div>
              {http.body_type === "multipart" && (
                <Select
                  id={`form-kind-${index}`}
                  value={field.file !== null ? "file" : "text"}
                  options={[
                    { value: "text", label: "文本字段" },
                    { value: "file", label: "文件字段" },
                  ]}
                  onChange={(kind) =>
                    update(index, {
                      file:
                        kind === "file"
                          ? {
                              name: "",
                              content_type: "application/octet-stream",
                              data_base64: "",
                            }
                          : null,
                    })
                  }
                />
              )}
              <label htmlFor={`form-kind-${index}`} className="sr-only">
                表单字段 {index + 1} 类型
              </label>
              {field.file !== null ? (
                <FileField
                  id={`form-file-${index}`}
                  value={field.file.name ? field.file : null}
                  onChange={(file) =>
                    update(index, {
                      file: file ?? {
                        name: "",
                        content_type: "application/octet-stream",
                        data_base64: "",
                      },
                    })
                  }
                />
              ) : (
                <Input
                  aria-label={`表单字段 ${index + 1} 值`}
                  placeholder="字段值"
                  value={field.value}
                  onChange={(e) => update(index, { value: e.target.value })}
                />
              )}
            </div>
          ))}
          <Button
            type="button"
            variant="outline"
            onClick={() =>
              patch({
                form_fields: [...fields, { name: "", value: "", file: null }],
              })
            }
          >
            <Plus />
            添加表单字段
          </Button>
        </div>
      )}
      {http.body_type === "binary" && (
        <>
          <FileField
            id="http-binary-file"
            value={http.binary}
            onChange={(binary) => patch({ binary })}
          />
          <p className="text-xs text-muted">
            最大 256 KiB，原始文件字节作为请求体发送。
          </p>
        </>
      )}
      {http.body_type === "raw" && (
        <div className="space-y-2">
          <label
            htmlFor="http-custom-content-type"
            className="text-sm font-medium"
          >
            Content-Type
          </label>
          <Input
            id="http-custom-content-type"
            placeholder="application/graphql"
            value={http.content_type}
            onChange={(e) => patch({ content_type: e.target.value })}
          />
        </div>
      )}
      {!["none", "multipart", "urlencoded", "binary"].includes(
        http.body_type,
      ) && (
        <div className="space-y-3">
          <label htmlFor="http-body" className="text-sm font-medium">
            请求体内容
          </label>
          <textarea
            id="http-body"
            rows={7}
            spellCheck={false}
            className="w-full resize-y rounded-lg border border-border bg-input p-3 font-mono text-sm outline-none focus:ring-2 focus:ring-ring"
            placeholder={
              http.body_type === "json"
                ? '{\n  "message": "Hello"\n}'
                : http.body_type === "xml"
                  ? "<request>\n  <message>Hello</message>\n</request>"
                  : "填写请求正文"
            }
            value={http.body}
            onChange={(e) => patch({ body: e.target.value })}
          />
          {http.body_type === "json" && (
            <Button
              type="button"
              variant="outline"
              onClick={() => {
                try {
                  patch({
                    body: JSON.stringify(JSON.parse(http.body), null, 2),
                  });
                  setError("");
                } catch {
                  setError("JSON 格式无效，无法格式化。");
                }
              }}
            >
              格式化 JSON
            </Button>
          )}
        </div>
      )}
      {error && (
        <p role="alert" className="text-sm text-destructive">
          {error}
        </p>
      )}
    </div>
  );
}
type RequestPreview = {
  method: string;
  url: string;
  headers: Pair[];
  body: string;
};
export function HttpRequestPreview({ http }: { http: HttpConfig }) {
  const [preview, setPreview] = useState<RequestPreview | null>(null);
  const [reveal, setReveal] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const generation = useRef(0);
  useEffect(() => {
    generation.current++;
    setPreview(null);
    setError("");
    setReveal(false);
    setBusy(false);
    return () => {
      generation.current++;
    };
  }, [http]);
  function display() {
    if (!preview) return "";
    const url = new URL(preview.url);
    if (!reveal) {
      url.pathname = "/…";
      url.search = "";
      url.hash = "";
    }
    return `${preview.method} ${url.toString()}\n${preview.headers.map((p) => `${p.name}: ${reveal || p.name.toLowerCase() === "content-type" ? p.value : "••••••••"}`).join("\n")}${preview.body ? `\n\n${reveal ? preview.body : "[请求体已隐藏]"}` : ""}`;
  }
  return (
    <div className="space-y-4">
      <p className="text-sm leading-6 text-muted">
        检查合并后的地址、认证、请求头和请求体。预览不会发送 HTTP
        请求；multipart 展示字段与文件摘要，实际执行时会重新生成 boundary。
      </p>
      <div className="flex flex-wrap gap-2">
        <Button
          type="button"
          variant="outline"
          disabled={busy}
          onClick={async () => {
            const version = ++generation.current;
            setBusy(true);
            setError("");
            try {
              const result = await api<RequestPreview>(
                "/api/scheduled-tasks/request-preview",
                { method: "POST", body: JSON.stringify(http) },
              );
              if (version === generation.current) setPreview(result);
            } catch (e) {
              if (version === generation.current) setError(errorMessage(e));
            } finally {
              if (version === generation.current) setBusy(false);
            }
          }}
        >
          {busy ? "生成中…" : preview ? "刷新请求预览" : "生成请求预览"}
        </Button>
        {preview && (
          <Button
            type="button"
            variant="outline"
            aria-pressed={reveal}
            onClick={() => setReveal(!reveal)}
          >
            {reveal ? <EyeOff /> : <Eye />}
            {reveal ? "隐藏完整值" : "显示完整值"}
          </Button>
        )}
      </div>
      {error && (
        <p role="alert" className="text-sm text-destructive">
          {error}
        </p>
      )}
      {preview && (
        <pre
          aria-label="实际请求预览"
          className="max-h-96 overflow-auto whitespace-pre-wrap break-all rounded-lg bg-background p-4 font-mono text-sm leading-6"
        >
          {display()}
        </pre>
      )}
    </div>
  );
}

export function HttpDeliveryEditor({
  http,
  patch,
}: {
  http: HttpConfig;
  patch: (value: Partial<HttpConfig>) => void;
}) {
  return (
    <section
      className="space-y-3 border-t border-border pt-4"
      aria-labelledby="http-delivery-title"
    >
      <label
        id="http-delivery-title"
        htmlFor="http-delivery"
        className="text-sm font-medium"
      >
        发送方式
      </label>
      <Select
        id="http-delivery"
        value={http.send_via_browser ? "browser" : "http"}
        options={[
          { value: "http", label: "HTTP 客户端" },
          { value: "browser", label: "浏览器（Browserless）" },
        ]}
        onChange={(value) => patch({ send_via_browser: value === "browser" })}
      />
      {http.send_via_browser ? (
        <>
          <label className="flex items-center gap-2 text-sm">
            <input
              type="checkbox"
              className="size-4 accent-primary"
              checked={http.browser_use_global_proxy}
              onChange={(event) =>
                patch({ browser_use_global_proxy: event.target.checked })
              }
            />
            连接浏览器时使用全局代理
          </label>
          <p className="text-xs leading-6 text-muted">
            复用「自动签到 → 配置签到工具」中的 Browserless 地址与
            Token，需要服务支持 /function API。此开关仅控制云母到 Browserless
            的连接；浏览器访问目标网站的出口由浏览器服务配置决定。请求在独立浏览器会话中发送，不执行目标页面脚本。
          </p>
        </>
      ) : (
        <>
          <label className="flex items-center gap-2 text-sm">
            <input
              type="checkbox"
              className="size-4 accent-primary"
              checked={http.use_global_proxy}
              onChange={(event) =>
                patch({ use_global_proxy: event.target.checked })
              }
            />
            使用全局代理发送
          </label>
          <p className="text-xs leading-6 text-muted">
            {http.use_global_proxy
              ? "使用「系统设置」中的全局代理地址连接请求目标。"
              : "直接连接请求目标，不使用全局代理或环境变量代理。"}
          </p>
        </>
      )}
    </section>
  );
}
