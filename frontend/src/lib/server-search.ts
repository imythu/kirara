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
};

/** Only manages requests. All matching, ranking, counts and pagination belong to Rust. */
export function useServerSearch<T>(endpoint: string, {
  query, filters = {}, refreshKey, enabled = true, composing = false, pageSize = 20, pollUntil = 0,
}: SearchOptions) {
  const parameters = new URLSearchParams({ q: query, page_size: String(pageSize) });
  for (const [key, value] of Object.entries(filters).sort(([a], [b]) => a.localeCompare(b))) {
    if (value !== undefined) parameters.set(key, String(value));
  }
  const queryKey = `${endpoint}?${parameters}`;
  const [paging, setPaging] = useState({ key: queryKey, page: 1 });
  const requestedPage = paging.key === queryKey ? paging.page : 1;
  const [retry, setRetry] = useState(0);
  const path = `${queryKey}&page=${requestedPage}`;
  const [response, setResponse] = useState<{
    path: string; refreshKey: unknown; retry: number; data?: SearchPage<T>; error?: string;
  }>();

  useEffect(() => {
    setPaging((current) => current.key === queryKey ? current : { key: queryKey, page: 1 });
  }, [queryKey]);

  useEffect(() => {
    if (!enabled || composing) return;
    setResponse(undefined);
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
        setResponse({ path, refreshKey, retry, data });
        // Persist server clamping so a later refresh cannot jump back to a removed page.
        if (data.page !== requestedPage) setPaging({ key: queryKey, page: data.page });
      } catch (error) {
        if (current) setResponse({ path, refreshKey, retry, error: (error as Error).message || "搜索失败，请重试" });
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
  const data = active ? response.data : undefined;
  const records = useMemo(() => data?.items.map((item) => item.record) ?? [], [data]);
  const reload = useCallback(() => setRetry((value) => value + 1), []);
  return {
    records,
    items: data?.items ?? [],
    total: data?.total ?? 0,
    page: data?.page ?? requestedPage,
    pageCount: Math.max(1, Math.ceil((data?.total ?? 0) / pageSize)),
    loading: enabled && !composing && !active,
    composing,
    error: active ? response.error ?? "" : "",
    semanticStatus: data?.semantic_status ?? "not_used",
    parsedFilters: data?.parsed_filters ?? [],
    setPage: (page: number) => setPaging({ key: queryKey, page: Math.max(1, page) }),
    reload,
  };
}

export type SearchControls = Pick<ReturnType<typeof useServerSearch<unknown>>,
  "loading" | "composing" | "error" | "semanticStatus" | "parsedFilters" | "reload" | "total" | "page" | "pageCount" | "setPage">;
