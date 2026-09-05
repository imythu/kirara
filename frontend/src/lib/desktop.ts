import { Channel, invoke, isTauri } from "@tauri-apps/api/core";

export const isDesktop = isTauri();

interface DesktopResponse {
  status: number;
  statusText: string;
  body: string;
}

function abortError(signal: AbortSignal) {
  return signal.reason ?? new DOMException("The operation was aborted", "AbortError");
}

export async function desktopFetch(path: string, init?: RequestInit): Promise<Response> {
  const signal = init?.signal;
  if (signal?.aborted) throw abortError(signal);
  if (init?.body != null && typeof init.body !== "string") {
    throw new TypeError("桌面 API 仅支持 JSON 请求体");
  }
  const response = invoke<DesktopResponse>("api_request", {
    request: { path, method: (init?.method ?? "GET").toUpperCase(), body: init?.body ?? null },
  }).catch((error: unknown) => {
    throw error instanceof Error ? error : new Error(String(error));
  });
  const result = signal
    ? await new Promise<DesktopResponse>((resolve, reject) => {
      const onAbort = () => reject(abortError(signal));
      signal.addEventListener("abort", onAbort, { once: true });
      response.then(resolve, reject).finally(() => signal.removeEventListener("abort", onAbort));
      if (signal.aborted) onAbort();
    })
    : await response;
  return new Response(result.status === 204 || result.status === 205 || result.status === 304 ? null : result.body, {
    status: result.status,
    statusText: result.statusText,
    headers: { "Content-Type": "application/json" },
  });
}

type LogEvent =
  | { type: "open" }
  | { type: "data"; data: string }
  | { type: "error"; message: string }
  | { type: "end" };

export interface LogHandlers {
  onOpen: () => void;
  onLog: (data: string) => void;
  onError: () => void;
}

export function desktopLogs(handlers: LogHandlers): { close: () => void } {
  let closed = false;
  let reconnectTimer: ReturnType<typeof setTimeout> | undefined;
  let current: { id?: string; ended: boolean } | undefined;

  const cancel = (id: string) => {
    void invoke("logs_close", { id }).catch(() => undefined);
  };
  const reconnect = (attempt: { id?: string; ended: boolean }) => {
    if (closed || attempt.ended || current !== attempt) return;
    attempt.ended = true;
    if (attempt.id) cancel(attempt.id);
    handlers.onError();
    reconnectTimer = setTimeout(connect, 2000);
  };
  const connect = () => {
    if (closed) return;
    const attempt: { id?: string; ended: boolean } = { ended: false };
    current = attempt;
    const channel = new Channel<LogEvent>();
    channel.onmessage = (event) => {
      if (closed || attempt.ended || current !== attempt) return;
      switch (event.type) {
        case "open": handlers.onOpen(); break;
        case "data": handlers.onLog(event.data); break;
        case "error":
        case "end": reconnect(attempt); break;
      }
    };
    void invoke<string>("logs_open", { onEvent: channel }).then((id) => {
      attempt.id = id;
      // Closing the dialog can race the initial IPC response.
      if (closed || attempt.ended || current !== attempt) cancel(id);
    }).catch(() => reconnect(attempt));
  };
  connect();
  return {
    close() {
      closed = true;
      clearTimeout(reconnectTimer);
      if (current?.id) cancel(current.id);
    },
  };
}
