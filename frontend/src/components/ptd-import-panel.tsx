import { useEffect, useRef, useState } from "react";
import { Loader2 } from "lucide-react";
import { api } from "@/lib/api";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select } from "@/components/ui/select";

interface Report {
  created: number; updated: number; unchanged: number; skipped: number; details: string[];
}
const MAX_SIZE = 8 * 1024 * 1024;
function readBase64(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result).split(",", 2)[1]);
    reader.onerror = () => reject(new Error("读取文件失败，请重新选择文件"));
    reader.readAsDataURL(file);
  });
}

export function PtdImportPanel({ onImported }: { onImported: () => void }) {
  const [file, setFile] = useState<File | null>(null);
  const [policy, setPolicy] = useState("update");
  const [autoCreate, setAutoCreate] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [report, setReport] = useState<Report | null>(null);
  const mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);

  async function submit() {
    if (!file || busy) return;
    setBusy(true); setError(""); setReport(null);
    try {
      const content = await readBase64(file);
      if (!mounted.current) return;
      const result = await api<Report>("/api/sites/ptd-import", {
        method: "POST", body: JSON.stringify({ content_base64: content, existing_policy: policy, auto_create: autoCreate }),
      });
      onImported();
      if (mounted.current) setReport(result);
    } catch (e) {
      if (mounted.current) setError((e as Error).message || "导入失败，请重试");
    } finally { if (mounted.current) setBusy(false); }
  }

  return <section className="space-y-5 p-4 sm:p-6" aria-label="PTD 配置导入">
    <div><h3 className="text-base font-bold">导入 PTD 配置</h3><p className="mt-1 text-sm leading-6 text-muted">从 PTD 备份中的站点 Cookies 添加站点或更新登录凭据。无需启用 WebDAV。</p></div>
    <fieldset disabled={busy} className="space-y-4 disabled:opacity-60">
      <legend className="sr-only">导入设置</legend>
      <div className="space-y-2"><Label htmlFor="ptd-import-file">PTD 备份文件</Label><Input id="ptd-import-file" type="file" accept=".zip,.json,application/zip,application/json" className="h-auto min-h-11 py-2" aria-describedby="ptd-import-help" onChange={event => {
        const selected = event.target.files?.[0] ?? null;
        setReport(null); setError(""); setFile(null);
        if (selected && (selected.size === 0 || selected.size > MAX_SIZE)) { setError("请选择非空且不超过 8 MiB 的文件"); return; }
        if (selected && !/\.(zip|json)$/i.test(selected.name)) { setError("请选择 PTD 导出的 ZIP 或 cookies.json 文件"); return; }
        setFile(selected);
      }} /><p id="ptd-import-help" className="text-xs leading-5 text-muted">支持未加密的 PTD_backup_*.zip 或解压得到的 cookies.json，最大 8 MiB。此处导入站点与 Cookie，不恢复下载器、任务或插件偏好。</p></div>
      <div className="space-y-2"><Label htmlFor="ptd-import-policy">遇到已有站点</Label><Select id="ptd-import-policy" disabled={busy} value={policy} onChange={setPolicy} options={[{ value: "update", label: "更新 Cookie，保留其他配置" }, { value: "skip", label: "跳过，保留现有 Cookie" }]} /></div>
      <label className="flex min-h-11 cursor-pointer items-center gap-3 text-sm"><input type="checkbox" className="size-4 accent-primary" checked={autoCreate} onChange={event => setAutoCreate(event.target.checked)} />自动添加识别到的新站点</label>
    </fieldset>
    {error && <p role="alert" className="break-words text-sm text-destructive">{error}</p>}
    <Button disabled={!file || busy} onClick={() => void submit()}>{busy && <Loader2 className="motion-safe:animate-spin" />}{busy ? "正在导入…" : "开始导入"}</Button>
    {report && <div className="space-y-2 border-t border-border pt-4" role="status"><p className="text-sm font-semibold">导入完成</p><p className="text-sm">新增 {report.created} · 更新 {report.updated} · 未变化 {report.unchanged} · 跳过 {report.skipped}</p>{report.details.length > 0 && <details className="text-sm"><summary className="cursor-pointer py-2 underline underline-offset-4">查看跳过原因</summary><ul className="space-y-2 text-xs text-muted">{report.details.map((detail, index) => <li key={index} className="break-words">{detail}</li>)}</ul></details>}</div>}
    <div className="space-y-3 border-t border-border pt-4">
      <h4 className="text-sm font-bold">PTD 怎么导出？</h4>
      <ol className="list-decimal space-y-2 pl-5 text-sm leading-6">
        <li>先在安装 PTD 的浏览器中登录需要导入的 PT 站点。</li>
        <li>打开 PTD 设置，在「常规设置 → 备份恢复」中将「备份文件加密、解密密钥」留空，导出未加密备份。</li>
        <li>进入「参数备份与恢复」，点击「本地导出」，勾选「站点 Cookies（cookies）」后导出。仅导入本站点管理时，其他项目无需勾选。</li>
        <li>回到这里选择下载的 PTD_backup_*.zip，或其中的 cookies.json，然后点击「开始导入」。</li>
      </ol>
      <p className="text-xs leading-5 text-muted">相同 Cookie 重复导入不会重复建站；多个本地站点同时匹配、过期或不适用的 Cookie 会跳过并说明原因。保留原加密密钥供旧备份恢复，导出完成后可恢复原设置。</p>
    </div>
  </section>;
}
