import { useEffect, useMemo, useState } from "react";
import { Activity, CheckCircle2, XCircle } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select } from "@/components/ui/select";
import { api } from "@/lib/api";
import type { GlobalConfig, ProxyTestResult } from "@/types";

const COMMON_LOG_LEVELS = ["trace", "debug", "info", "warn", "error"];

const PROXY_PROTOCOLS = [
  { value: "", label: "不使用代理" },
  { value: "http", label: "HTTP" },
  { value: "https", label: "HTTPS" },
  { value: "socks5", label: "SOCKS5" },
  { value: "socks5h", label: "SOCKS5H（远程 DNS）" },
] as const;

/** Parse a proxy URL string like "http://1.2.3.4:7890" into parts. */
function parseProxy(raw: string | null): { protocol: string; host: string; port: string } {
  if (!raw) return { protocol: "", host: "", port: "" };
  const trimmed = raw.trim();
  if (!trimmed) return { protocol: "", host: "", port: "" };

  const match = trimmed.match(/^(https?|socks5h?):\/\/(.*?)(?::(\d+))?$/);
  if (match) {
    return { protocol: match[1], host: match[2], port: match[3] ?? "" };
  }
  // Fallback: treat the whole thing as host (for unusual values)
  return { protocol: "http", host: trimmed, port: "" };
}

/** Build a proxy URL string from parts. Returns null when no proxy is configured. */
function buildProxy(protocol: string, host: string, port: string): string | null {
  if (!protocol) return null;
  const h = host.trim();
  const p = port.trim();
  return p ? `${protocol}://${h}:${p}` : `${protocol}://${h}`;
}


export function SystemSettingsPage({
  settings,
  setSettings,
  saving,
  onSave,
}: {
  settings: GlobalConfig;
  setSettings: React.Dispatch<React.SetStateAction<GlobalConfig>>;
  saving: boolean;
  onSave: () => Promise<void>;
}) {
  useEffect(() => {
    if (!COMMON_LOG_LEVELS.includes(settings.log_level ?? "")) {
      setSettings((prev) => ({ ...prev, log_level: "info" }));
    }
  }, [settings.log_level, setSettings]);

  const proxyParts = useMemo(() => parseProxy(settings.proxy), [settings.proxy]);
  const [testUrl, setTestUrl] = useState("https://www.google.com");
  const [testingProxy, setTestingProxy] = useState(false);
  const [testResult, setTestResult] = useState<ProxyTestResult | null>(null);

  const updateProxy = (patch: Partial<{ protocol: string; host: string; port: string }>) => {
    const next = { ...proxyParts, ...patch };
    setSettings((prev) => ({ ...prev, proxy: buildProxy(next.protocol, next.host, next.port) }));
  };

  const handleTestProxy = async () => {
    if (!settings.proxy) return;
    setTestingProxy(true);
    setTestResult(null);
    try {
      const res = await api<ProxyTestResult>("/api/proxy/test", {
        method: "POST",
        body: JSON.stringify({
          proxy: settings.proxy,
          test_url: testUrl,
        }),
      });
      setTestResult(res);
    } catch (e: any) {
      setTestResult({
        success: false,
        status_code: null,
        elapsed_ms: 0,
        message: String(e.message || e),
      });
    } finally {
      setTestingProxy(false);
    }
  };

  return (
    <Card>
      <CardHeader>
        <CardTitle>系统设置</CardTitle>
        <CardDescription>全局后端程序配置。日志级别保存后会立即作用到整个后端进程。</CardDescription>
      </CardHeader>
      <CardContent className="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
        <div className="space-y-2">
          <Label>全局日志级别</Label>
          <Select
            value={COMMON_LOG_LEVELS.includes(settings.log_level ?? "") ? settings.log_level ?? "info" : "info"}
            onChange={(val) =>
              setSettings((prev) => ({
                ...prev,
                log_level: val,
              }))
            }
            options={COMMON_LOG_LEVELS.map((level) => ({ value: level, label: level }))}
          />
        </div>

        {/* ---- 全局代理（小白友好版） ---- */}
        <div className="space-y-3 sm:col-span-2 xl:col-span-3">
          <Label>全局代理</Label>
          <p className="text-xs text-muted">
            配置后所有 HTTP 请求（站点抓取、RSS 拉取等）都会走此代理。不需要代理就选"不使用代理"。
          </p>

          <div className="grid gap-3 sm:grid-cols-3">
            {/* 协议 */}
            <div className="space-y-1.5">
              <Label className="text-xs text-muted">协议类型</Label>
              <Select
                value={proxyParts.protocol}
                onChange={(val) => updateProxy({ protocol: val })}
                options={[...PROXY_PROTOCOLS]}
              />
            </div>

            {/* 地址 & 端口 (仅在选择代理时展示) */}
            {proxyParts.protocol && (
              <>
                <div className="space-y-1.5">
              <Label className="text-xs text-muted">地址</Label>
              <Input
                value={proxyParts.host}
                onChange={(e) => updateProxy({ host: e.target.value })}
                placeholder="127.0.0.1"
                disabled={!proxyParts.protocol}
              />
            </div>

            {/* 端口 */}
            <div className="space-y-1.5">
              <Label className="text-xs text-muted">端口</Label>
              <Input
                value={proxyParts.port}
                onChange={(e) => {
                  const v = e.target.value.replace(/\D/g, "").slice(0, 5);
                  updateProxy({ port: v });
                }}
                placeholder="7890"
                disabled={!proxyParts.protocol}
                inputMode="numeric"
              />
                </div>
              </>
            )}
          </div>

          {proxyParts.protocol && proxyParts.host && (
            <div className="mt-2 space-y-3 rounded-2xl border border-border bg-card p-4 shadow-sm">
              <p className="text-sm font-medium">代理测试</p>
              <div className="flex flex-col gap-3 sm:flex-row sm:items-end">
                <div className="flex-1 space-y-1.5">
                  <Label className="text-xs text-muted">测试 URL</Label>
                  <Input
                    value={testUrl}
                    onChange={(e) => setTestUrl(e.target.value)}
                    placeholder="https://www.google.com"
                  />
                </div>
                <Button 
                  variant="outline" 
                  onClick={() => void handleTestProxy()} 
                  disabled={testingProxy || !settings.proxy}
                  className="w-full sm:w-auto"
                >
                  {testingProxy ? (
                    <Activity className="mr-2 h-4 w-4 animate-spin" />
                  ) : null}
                  测试连接
                </Button>
              </div>

              {testResult && (
                <div
                  className={`flex items-start gap-2 rounded-xl border p-3 text-sm ${
                    testResult.success
                      ? "border-green-500/30 bg-green-500/10 text-green-600 dark:text-green-400"
                      : "border-red-500/30 bg-red-500/10 text-red-600 dark:text-red-400"
                  }`}
                >
                  {testResult.success ? (
                    <CheckCircle2 className="mt-0.5 h-4 w-4 shrink-0" />
                  ) : (
                    <XCircle className="mt-0.5 h-4 w-4 shrink-0" />
                  )}
                  <div className="flex-1 space-y-1">
                    <p className="font-medium">
                      {testResult.success ? "测试通过" : "测试失败"}
                      {testResult.elapsed_ms > 0 && ` (${testResult.elapsed_ms}ms)`}
                      {testResult.status_code && ` • HTTP ${testResult.status_code}`}
                    </p>
                    <p className="text-xs opacity-80">{testResult.message}</p>
                  </div>
                </div>
              )}
            </div>
          )}
        </div>

        <div className="sm:col-span-2 xl:col-span-3">
          <Button onClick={() => void onSave()} disabled={saving}>
            {saving ? "保存中..." : "保存系统设置"}
          </Button>
        </div>
      </CardContent>
    </Card>
  );
}
