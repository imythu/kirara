import { useCallback, useEffect, useRef, useState, type Dispatch, type SetStateAction } from "react";

const PREFIX = "yunmu-view:";

function read<T>(key: string, fallback: T, validate?: (value: unknown) => value is T): T {
  try {
    const raw = sessionStorage.getItem(PREFIX + key);
    if (raw == null) return fallback;
    const parsed = JSON.parse(raw) as unknown;
    if (validate ? validate(parsed) : typeof parsed === typeof fallback) return parsed as T;
  } catch {
    // Storage can be disabled or hold a value from an older version.
  }
  return fallback;
}

/**
 * 页面视图状态（搜索词、筛选、排序、子标签）：离开页面再回来时保持原样。
 * 只保存在当前浏览器标签页；不要用来保存凭据或待提交的表单内容。
 */
export function usePageState<T>(
  key: string,
  fallback: T,
  validate?: (value: unknown) => value is T,
): [T, Dispatch<SetStateAction<T>>] {
  const [value, setValue] = useState<T>(() => read(key, fallback, validate));
  const keyRef = useRef(key);
  keyRef.current = key;
  useEffect(() => {
    try {
      sessionStorage.setItem(PREFIX + keyRef.current, JSON.stringify(value));
    } catch {
      // Storage can be disabled.
    }
  }, [value]);
  return [value, setValue];
}

/** 仅接受给定选项之一，旧版本残留的无效值回落到默认值。 */
export function oneOf<T extends string>(choices: readonly T[]) {
  return (value: unknown): value is T => typeof value === "string" && (choices as readonly string[]).includes(value);
}

export type SortState<K extends string> = { key: K; direction: "asc" | "desc" };

/** 点击列头排序：首次点击按该列的默认方向，再次点击反转。 */
export function useSortState<K extends string>(storageKey: string, fallback: SortState<K>, keys: readonly K[]) {
  const [sort, setSort] = usePageState<SortState<K>>(storageKey, fallback, (value): value is SortState<K> =>
    typeof value === "object" && value !== null
    && (keys as readonly string[]).includes((value as SortState<K>).key)
    && ["asc", "desc"].includes((value as SortState<K>).direction));
  const toggle = useCallback((key: K, defaultDirection: "asc" | "desc" = "asc") => {
    setSort((current) => current.key === key
      ? { key, direction: current.direction === "asc" ? "desc" : "asc" }
      : { key, direction: defaultDirection });
  }, [setSort]);
  return [sort, toggle] as const;
}

export function compareValues(left: string | number | null | undefined, right: string | number | null | undefined) {
  if (left == null && right == null) return 0;
  if (left == null) return 1;
  if (right == null) return -1;
  if (typeof left === "number" && typeof right === "number") return left - right;
  return String(left).localeCompare(String(right), "zh-CN", { numeric: true, sensitivity: "base" });
}

/** 中文友好的包含匹配：忽略大小写与全半角差异，多个关键词以空格分隔时需全部命中。 */
export function matchesQuery(query: string, ...fields: Array<string | number | null | undefined>) {
  const terms = normalizeSearch(query).split(/\s+/).filter(Boolean);
  if (terms.length === 0) return true;
  const haystack = normalizeSearch(fields.filter((field) => field != null).join(" "));
  return terms.every((term) => haystack.includes(term));
}

export function normalizeSearch(value: string) {
  return value.normalize("NFKC").toLocaleLowerCase();
}
