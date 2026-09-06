import { ChevronLeft, ChevronRight, Loader2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import type { SearchControls } from "@/lib/server-search";

const filterLabels: Record<string, string> = {
  result: "执行结果", health: "同步状态", enabled: "启用状态", site: "站点", site_id: "站点", id: "编号", type: "类型", collection: "站点集合",
};
const valueLabels: Record<string, string> = {
  failed: "失败", success: "成功", unknown: "未知", healthy: "成功", pending: "待同步", true: "已启用", false: "已停用",
};

export function SearchFeedback({ search, onClearQuery }: { search: SearchControls; onClearQuery?: () => void }) {
  if (search.error) return (
    <div role="alert" className="my-3 flex flex-wrap items-center justify-between gap-3 rounded-xl border border-destructive/30 p-3 text-sm">
      <span className="min-w-0 break-words">搜索失败：{search.error}</span>
      <Button type="button" variant="outline" onClick={search.reload}>重新搜索</Button>
    </div>
  );
  if (search.loading || search.composing) return (
    <div role="status" className="flex items-center gap-2 py-4 text-sm text-muted">
      <Loader2 className="size-4 animate-spin" aria-hidden="true" />
      {search.composing ? "输入完成后搜索" : "正在搜索…"}
    </div>
  );
  const degraded = !["used", "not_used", "not_needed", "disabled", "unused"].includes(search.semanticStatus);
  return (
    <div aria-live="polite" className="text-sm">
      {search.parsedFilters.length > 0 ? (
        <div className="my-3 flex flex-wrap items-center gap-2">
          <span className="text-muted">已识别条件：</span>
          {search.parsedFilters.map((filter, index) => (
            <span key={`${filter.field}:${filter.value}:${index}`} className="rounded-lg border border-border px-2 py-1">
              {filterLabels[filter.field] ?? filter.field}：{valueLabels[filter.value] ?? filter.value}
            </span>
          ))}
          {onClearQuery ? <Button type="button" variant="outline" onClick={onClearQuery}>清除查询条件</Button> : null}
        </div>
      ) : null}
      {degraded ? <p className="my-3 text-muted">语义搜索暂不可用，当前显示关键词匹配结果。<button type="button" className="ml-2 rounded underline underline-offset-4 focus-visible:outline focus-visible:outline-2 focus-visible:outline-ring" onClick={search.reload}>重试</button></p> : null}
    </div>
  );
}

export function SearchPagination({ search, label = "搜索结果" }: { search: SearchControls; label?: string }) {
  if (search.loading || search.composing || search.error || search.pageCount <= 1) return null;
  return (
    <nav aria-label={`${label}分页`} className="mt-4 flex flex-wrap items-center justify-between gap-3 border-t border-border pt-4">
      <span className="text-sm text-muted">第 {search.page} / {search.pageCount} 页 · 共 {search.total} 条</span>
      <div className="flex gap-2">
        <Button type="button" variant="outline" onClick={() => search.setPage(search.page - 1)} disabled={search.page <= 1}>
          <ChevronLeft className="mr-1 size-4" aria-hidden="true" />上一页
        </Button>
        <Button type="button" variant="outline" onClick={() => search.setPage(search.page + 1)} disabled={search.page >= search.pageCount}>
          下一页<ChevronRight className="ml-1 size-4" aria-hidden="true" />
        </Button>
      </div>
    </nav>
  );
}
