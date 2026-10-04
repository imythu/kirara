import { forwardRef, useMemo, type CompositionEvent, type ReactNode } from "react";
import { ArrowDown, ArrowUp, ArrowUpDown, Loader2, Search, X } from "lucide-react";
import { TableHead } from "@/components/ui/table";
import { cn } from "@/lib/utils";
import type { SortState } from "@/lib/page-state";

/**
 * 列表搜索框：输入即筛选，Esc 清空（再按一次离开输入框），右侧一键清空。
 * primary 为 true 时，在页面任意位置按 “/” 可直接聚焦到这里。
 */
export const SearchBox = forwardRef<HTMLInputElement, {
  value: string;
  onChange: (value: string) => void;
  placeholder: string;
  label?: string;
  id?: string;
  primary?: boolean;
  busy?: boolean;
  className?: string;
  inputClassName?: string;
  onEnter?: () => void;
  onCompositionStart?: () => void;
  onCompositionEnd?: (value: string) => void;
}>(function SearchBox({
  value, onChange, placeholder, label, id, primary = true, busy = false, className, inputClassName, onEnter,
  onCompositionStart, onCompositionEnd,
}, ref) {
  return (
    <div className={cn("relative min-w-0", className)}>
      {busy
        ? <Loader2 className="pointer-events-none absolute left-3.5 top-1/2 size-4 -translate-y-1/2 animate-spin text-muted" aria-hidden="true" />
        : <Search className="pointer-events-none absolute left-3.5 top-1/2 size-4 -translate-y-1/2 text-muted" aria-hidden="true" />}
      <input
        ref={ref}
        id={id}
        type="search"
        autoComplete="off"
        spellCheck={false}
        enterKeyHint="search"
        data-primary-search={primary ? "true" : undefined}
        aria-label={label ?? placeholder}
        aria-keyshortcuts={primary ? "/" : undefined}
        value={value}
        placeholder={placeholder}
        onChange={(event) => onChange(event.target.value)}
        onCompositionStart={onCompositionStart}
        onCompositionEnd={(event: CompositionEvent<HTMLInputElement>) => onCompositionEnd?.(event.currentTarget.value)}
        onKeyDown={(event) => {
          if (event.nativeEvent.isComposing) return;
          if (event.key === "Escape") {
            // 有内容时先清空，不让外层对话框跟着关闭；已为空时离开输入框。
            if (value) {
              event.preventDefault();
              event.stopPropagation();
              onChange("");
            } else {
              event.currentTarget.blur();
            }
          } else if (event.key === "Enter" && onEnter) {
            event.preventDefault();
            onEnter();
          }
        }}
        className={cn(
          "kirara-search-input flex h-11 w-full rounded-lg border border-border bg-input py-2 pl-10 text-sm text-foreground placeholder:text-muted transition-colors focus-visible:border-primary focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-card",
          value ? "pr-10" : primary ? "pr-10 lg:pr-12" : "pr-3",
          inputClassName,
        )}
      />
      {value ? (
        <button
          type="button"
          aria-label="清空搜索"
          title="清空搜索（Esc）"
          className="absolute right-1 top-1/2 flex size-9 -translate-y-1/2 items-center justify-center rounded-md text-muted transition-colors hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          onClick={(event) => {
            onChange("");
            (event.currentTarget.previousElementSibling as HTMLInputElement | null)?.focus();
          }}
        >
          <X className="size-4" aria-hidden="true" />
        </button>
      ) : primary ? (
        <kbd aria-hidden="true" className="pointer-events-none absolute right-3 top-1/2 hidden h-6 -translate-y-1/2 items-center rounded border border-border bg-surface-container px-1.5 font-mono text-[11px] text-muted lg:flex">/</kbd>
      ) : null}
    </div>
  );
});

export type FilterChipOption<T extends string> = { value: T; label: string; count?: number; tone?: "danger" };

/** 单选筛选标签：带实时数量，再次点击当前标签回到“全部”。 */
export function FilterChips<T extends string>({
  options, value, onChange, label, resetValue, className,
}: {
  options: readonly FilterChipOption<T>[];
  value: T;
  onChange: (value: T) => void;
  label: string;
  resetValue?: T;
  className?: string;
}) {
  const reset = resetValue ?? options[0]?.value;
  return (
    <div role="group" aria-label={label} className={cn("flex flex-wrap gap-1.5", className)}>
      {options.map((option) => {
        const active = option.value === value;
        return (
          <button
            key={option.value}
            type="button"
            aria-pressed={active}
            onClick={() => onChange(active && reset !== undefined ? reset : option.value)}
            className={cn(
              "inline-flex min-h-9 items-center gap-1.5 rounded-full border px-3 text-xs font-medium transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1",
              active
                ? "border-primary bg-primary text-primary-foreground"
                : "border-border bg-card text-muted hover:border-primary/35 hover:bg-accent hover:text-foreground",
            )}
          >
            {option.label}
            {option.count !== undefined ? (
              <span className={cn(
                "tabular-nums",
                active ? "text-primary-foreground/85" : option.tone === "danger" && option.count > 0 ? "font-semibold text-destructive" : "text-muted",
              )}>
                {option.count}
              </span>
            ) : null}
          </button>
        );
      })}
    </div>
  );
}

/** 结果计数与“清除筛选”：只要有任何条件生效就提供一键复原。 */
export function FilterSummary({
  shown, total, noun, active, onClear, className, children,
}: {
  shown: number;
  total: number;
  noun: string;
  active: boolean;
  onClear: () => void;
  className?: string;
  children?: ReactNode;
}) {
  return (
    <div className={cn("flex min-h-9 flex-wrap items-center gap-x-3 gap-y-1 text-xs text-muted", className)}>
      <span role="status" aria-live="polite" className="tabular-nums">
        {active ? `显示 ${shown} / ${total} ${noun}` : `共 ${total} ${noun}`}
      </span>
      {active ? (
        <button
          type="button"
          onClick={onClear}
          className="rounded font-medium text-primary underline-offset-4 hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
        >
          清除筛选
        </button>
      ) : null}
      {children}
    </div>
  );
}

/** 高亮搜索命中的文字；多个关键词以空格分隔。 */
export function Highlight({ text, query }: { text: string | null | undefined; query: string }) {
  const parts = useMemo(() => {
    const value = text ?? "";
    const terms = query.trim().split(/\s+/).filter(Boolean).map((term) => term.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"));
    if (!value || terms.length === 0) return [value];
    return value.split(new RegExp(`(${terms.join("|")})`, "gi"));
  }, [text, query]);
  if (parts.length === 1) return <>{parts[0]}</>;
  return (
    <>
      {parts.map((part, index) => index % 2 === 1
        ? <mark key={index} className="rounded-sm bg-primary/15 px-0.5 text-inherit">{part}</mark>
        : part)}
    </>
  );
}

/** 可点击排序的表头；再次点击切换升降序。 */
export function SortableHead<K extends string>({
  label, sortKey, sort, onSort, defaultDirection = "asc", className, align = "left",
}: {
  label: string;
  sortKey: K;
  sort: SortState<K>;
  onSort: (key: K, defaultDirection?: "asc" | "desc") => void;
  defaultDirection?: "asc" | "desc";
  className?: string;
  align?: "left" | "right";
}) {
  const active = sort.key === sortKey;
  const Icon = !active ? ArrowUpDown : sort.direction === "asc" ? ArrowUp : ArrowDown;
  return (
    <TableHead className={className} aria-sort={active ? (sort.direction === "asc" ? "ascending" : "descending") : "none"}>
      <button
        type="button"
        onClick={() => onSort(sortKey, defaultDirection)}
        title={active ? (sort.direction === "asc" ? "当前升序，点击改为降序" : "当前降序，点击改为升序") : `按${label}排序`}
        className={cn(
          "-mx-1.5 inline-flex h-8 items-center gap-1 rounded-md px-1.5 transition-colors hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
          active && "text-foreground",
          align === "right" && "flex-row-reverse",
        )}
      >
        {label}
        <Icon className={cn("size-3.5", !active && "opacity-40")} aria-hidden="true" />
      </button>
    </TableHead>
  );
}
