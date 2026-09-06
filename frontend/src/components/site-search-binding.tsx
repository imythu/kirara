import { useEffect, useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select } from "@/components/ui/select";
import { SearchFeedback, SearchPagination } from "@/components/search-controls";
import { api } from "@/lib/api";
import { useServerSearch } from "@/lib/server-search";

type Binding = { mode: "auto" | "manual" | "none"; catalog_id: string | null };
type CatalogSite = { id: string; name: string; url: string; aka: string[] };

export function useSiteSearchBinding(siteId: number | null, open: boolean) {
  const [value, setValue] = useState<Binding>({ mode: "auto", catalog_id: null });
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const [retry, setRetry] = useState(0);
  useEffect(() => {
    if (!open) return;
    let current = true;
    setError("");
    setValue({ mode: "auto", catalog_id: null });
    setLoading(siteId !== null);
    if (siteId !== null) {
      api<Binding>(`/api/sites/${siteId}/search-binding`).then(
        (binding) => { if (current) setValue(binding); },
        (reason: Error) => { if (current) setError(reason.message || "目录关联加载失败"); },
      ).finally(() => { if (current) setLoading(false); });
    }
    return () => { current = false; };
  }, [siteId, open, retry]);
  return {
    value, setValue, loading, error,
    retry: () => setRetry((version) => version + 1),
    save: (id: number) => api(`/api/sites/${id}/search-binding`, {
      method: "PUT",
      body: JSON.stringify({ mode: value.mode, catalog_id: value.mode === "manual" ? value.catalog_id : null }),
    }),
  };
}

export function SiteSearchBindingField({ binding, open }: { binding: ReturnType<typeof useSiteSearchBinding>; open: boolean }) {
  const [query, setQuery] = useState("");
  const [composing, setComposing] = useState(false);
  const [selected, setSelected] = useState<CatalogSite | null>(null);
  const search = useServerSearch<CatalogSite>("/api/site-catalog/search", {
    query, composing, enabled: open && binding.value.mode === "manual", pageSize: 5,
  });
  useEffect(() => {
    if (!open) { setQuery(""); setSelected(null); }
  }, [open]);
  return (
    <div className="space-y-3 sm:col-span-2">
      <div className="space-y-1">
        <Label htmlFor="site-search-binding-mode">关联目录站点</Label>
        <p className="text-sm text-muted">用于按官方别名和站点特色搜索，保留你填写的站点名称。</p>
      </div>
      {binding.error ? (
        <div role="alert" className="flex flex-wrap items-center gap-3 text-sm">
          <span>{binding.error}</span><Button type="button" variant="outline" onClick={binding.retry}>重新加载</Button>
        </div>
      ) : null}
      <Select id="site-search-binding-mode" value={binding.value.mode} disabled={binding.loading || Boolean(binding.error)}
        onChange={(mode) => binding.setValue({ mode: mode as Binding["mode"], catalog_id: null })}
        options={[{ value: "auto", label: "按站点地址自动识别" }, { value: "manual", label: "手动指定" }, { value: "none", label: "不关联目录" }]} />
      {binding.loading ? <p role="status" className="text-sm text-muted">正在加载关联信息…</p> : null}
      {binding.value.mode === "auto" && !binding.loading ? <p className="text-sm text-muted">
        {binding.value.catalog_id ? `当前关联：${binding.value.catalog_id}。保存时会按站点地址重新识别。` : "保存后按站点地址识别；自定义域名或反代地址可以手动指定。"}
      </p> : null}
      {binding.value.mode === "manual" ? (
        <div className="space-y-3">
          {binding.value.catalog_id ? <p className="text-sm">已选择：{selected?.id === binding.value.catalog_id ? selected.name : binding.value.catalog_id}</p> : <p className="text-sm text-muted">请选择对应的公开站点。</p>}
          <Label htmlFor="site-catalog-search" className="sr-only">查找目录站点</Label>
          <Input id="site-catalog-search" type="search" value={query} placeholder="搜索官方名称、别名或域名"
            onChange={(event) => setQuery(event.target.value)}
            onKeyDown={(event) => { if (event.key === "Enter") event.preventDefault(); }}
            onCompositionStart={() => setComposing(true)}
            onCompositionEnd={(event) => { setQuery(event.currentTarget.value); setComposing(false); }} />
          <SearchFeedback search={search} onClearQuery={() => setQuery("")} />
          <div role="group" aria-label="目录站点搜索结果" className="divide-y divide-border">
            {search.records.map((site) => (
              <label key={site.id} className="flex cursor-pointer items-start gap-3 rounded-lg p-3 hover:bg-accent">
                <input type="radio" name="site-catalog-choice" value={site.id} className="mt-1 accent-[hsl(var(--primary))]"
                  checked={binding.value.catalog_id === site.id} onChange={() => { setSelected(site); binding.setValue({ mode: "manual", catalog_id: site.id }); }} />
                <span className="min-w-0 text-sm"><span className="block font-medium">{site.name}</span><span className="block break-all text-muted">{site.aka.join("、") || site.id} · {site.url}</span></span>
              </label>
            ))}
          </div>
          {!search.loading && !search.composing && !search.error && search.total === 0 ? <p className="text-sm text-muted">没有匹配的目录站点，可更换关键词或选择不关联。</p> : null}
          <SearchPagination search={search} label="目录站点" />
        </div>
      ) : null}
    </div>
  );
}
