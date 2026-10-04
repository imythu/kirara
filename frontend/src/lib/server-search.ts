import { useCallback, useEffect, useMemo, useState } from "react";
import { api } from "./api";

export type SearchPage<T> = {
  items: { record: T; matched_by: string[] }[];
  total: number;
  page: number;
  page_size: number;
  parsed_filters: { field: string; value: string }[];
  semantic_status: string;
};

type SearchOptions = {
  query: string;
  filters?: Record<string, string | number | boolean | undefined>;
  refreshKey?: unknown;
  enabled?: boolean;
  composing?: boolean;
  pageSize?: number;
  /** Briefly refresh after a background task is triggered, while the page is visible. */
  pollUntil?: number;
  /** Remember the current page for this query in the tab session, so returning to the list reopens it. */
  persistKey?: string;
};

function readStoredPaging(persistKey: string | undefined) {
  if (!persistKey) return null;
  try {
    const value = JSON.parse(sessionStorage.getItem(`yunmu-view:paging:${persistKey}`) ?? "null") as { key?: unknown; page?: unknown } | null;
    if (value && typeof value.key === "string" && typeof value.page === "number" && value.page >= 1) return { key: value.key, page: value.page };
  } catch {
    // Storage can be disabled.
  }
  return null;
}

/** Only manages requests. All matching, ranking, counts and pagination belong to Rust. */
export function useServerSearch<T>(endpoint: string, {
  query, filters = {}, refreshKey, enabled = true, composing = false, pageSize = 20, pollUntil = 0, persistKey,
}: SearchOptions) {
  const parameters = new URLSearchParams({ q: query, page_size: String(pageSize) });
  for (const [key, value] of Object.entries(filters).sort(([a], [b]) => a.localeCompare(b))) {
    if (value !== undefined) parameters.set(key, String(value));
  }
  const queryKey = `${endpoint}?${parameters}`;
  const [paging, setPaging] = useState(() => {
    const stored = readStoredPaging(persistKey);
    return stored?.key === queryKey ? stored : { key: queryKey, page: 1 };
  });
  useEffect(() => {
    if (!persistKey) return;
    try {
      sessionStorage.setItem(`yunmu-view:paging:${persistKey}`, JSON.stringify(paging));
    } catch {
      // Storage can be disabled.
    }
  }, [persistKey, paging]);
  const requestedPage = paging.key === queryKey ? paging.page : 1;
  const [retry, setRetry] = useState(0);
  const path = `${queryKey}&page=${requestedPage}`;
  const [response, setResponse] = useState<{
    path: string; queryKey: string; refreshKey: unknown; retry: number; data?: SearchPage<T>; error?: string;
  }>();

  useEffect(() => {
    setPaging((current) => current.key === queryKey ? current : { key: queryKey, page: 1 });
  }, [queryKey]);

  useEffect(() => {
    if (!enabled || composing) return;
    let current = true;
    const controller = new AbortController();
    let timer: number;
    const schedule = () => {
      if (current && Date.now() < pollUntil) timer = window.setTimeout(() => {
        if (document.visibilityState === "visible") void fetchPage();
        else schedule();
      }, 5000);
    };
    const fetchPage = async () => {
      try {
        const data = await api<SearchPage<T>>(path, { signal: controller.signal });
        if (!current) return;
        setResponse({ path, queryKey, refreshKey, retry, data });
        // Persist server clamping so a later refresh cannot jump back to a removed page.
        if (data.page !== requestedPage) setPaging({ key: queryKey, page: data.page });
      } catch (error) {
        if (current) setResponse({ path, queryKey, refreshKey, retry, error: (error as Error).message || "搜索失败，请重试" });
      } finally {
        schedule();
      }
    };
    timer = window.setTimeout(() => void fetchPage(), query ? 220 : 0);
    return () => {
      current = false;
      window.clearTimeout(timer);
      controller.abort();
    };
  }, [path, queryKey, requestedPage, query, refreshKey, retry, enabled, composing, pollUntil]);

  // Hide old-query records immediately, including while IME input is in progress.
  const active = enabled && !composing && response?.path === path
    && response.refreshKey === refreshKey && response.retry === retry;
  // Same query (refresh after an action, polling, or another page): keep showing the last
  // records until the new response arrives instead of flashing a loading state.
  const stale = !active && enabled && !composing && response?.queryKey === queryKey && !!response.data;
  const data = active || stale ? response?.data : undefined;
  const records = useMemo(() => data?.items.map((item) => item.record) ?? [], [data]);
  const reload = useCallback(() => setRetry((value) => value + 1), []);
  return {
    records,
    items: data?.items ?? [],
    total: data?.total ?? 0,
    page: active ? data?.page ?? requestedPage : requestedPage,
    pageCount: Math.max(1, Math.ceil((data?.total ?? 0) / pageSize)),
    loading: enabled && !composing && !active && !stale,
    refreshing: stale,
    composing,
    error: active ? response.error ?? "" : "",
    semanticStatus: data?.semantic_status ?? "not_used",
    parsedFilters: data?.parsed_filters ?? [],
    setPage: (page: number) => setPaging({ key: queryKey, page: Math.max(1, page) }),
    reload,
  };
}

export type SearchControls = Pick<ReturnType<typeof useServerSearch<unknown>>,
  "loading" | "refreshing" | "composing" | "error" | "semanticStatus" | "parsedFilters" | "reload" | "total" | "page" | "pageCount" | "setPage">;
