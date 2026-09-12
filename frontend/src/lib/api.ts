import type { GlobalConfig } from "@/types";
import { desktopFetch, desktopLogs, isDesktop, type LogHandlers } from "./desktop";

export const API_BASE = "";

export const defaultSettings: GlobalConfig = {
  log_level: "info",
  proxy: null,
  use_proxy_for_lightpanda: true,
  lightpanda: {
    endpoint: null,
    token: null,
    region: "euwest",
    browser: "lightpanda",
    proxy: "fast_dc",
    country: null,
  },
  browserless: {
    address: null,
    token: null,
  },
  tag_rule_scan_interval_mins: 7,
  vision_llm: { base_url: "https://openrouter.ai/api/v1", model: "", api_key: null, api_standard: "openai_responses", api_key_configured: false, clear_api_key: false },
};

export async function api<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await (isDesktop ? desktopFetch : fetch)(`${API_BASE}${path}`, {
    headers: {
      "Content-Type": "application/json",
      ...(init?.headers ?? {}),
    },
    ...init,
  });

  if (!response.ok) {
    const body = await response.json().catch(() => ({ error: response.statusText }));
    throw new ApiError(body.error ?? response.statusText, response.status);
  }

  if (response.status === 204) {
    return undefined as T;
  }

  return response.json() as Promise<T>;
}

export function subscribeLogs(handlers: LogHandlers): { close: () => void } {
  if (isDesktop) return desktopLogs(handlers);
  const source = new EventSource(`${API_BASE}/api/system/logs/stream`);
  source.onopen = handlers.onOpen;
  source.onerror = handlers.onError;
  source.addEventListener("log", (event) => handlers.onLog((event as MessageEvent<string>).data));
  return { close: () => source.close() };
}

export const APP_VERSION = import.meta.env.VITE_APP_VERSION as string | undefined;

export class ApiError extends Error {
  constructor(
    message: string,
    public readonly status: number,
  ) {
    super(message);
    this.name = "ApiError";
  }
}
