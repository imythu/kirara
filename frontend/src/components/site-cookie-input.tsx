import { useEffect, useRef, useState, type CSSProperties } from "react";
import { ChevronDown } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { createPortal } from "react-dom";
import { api } from "@/lib/api";
import type { SiteRecord, SiteCredentialsRecord } from "@/types";

/** Selecting a site copies its current cookie; it does not create a live binding. */
export function SiteCookieInput({
  value,
  reveal,
  onChange,
}: {
  value: string;
  reveal: boolean;
  onChange: (value: string) => void;
}) {
  const [query, setQuery] = useState("");
  const [position, setPosition] = useState<CSSProperties>({});
  const triggerRef = useRef<HTMLButtonElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  const [open, setOpen] = useState(false);
  const [sites, setSites] = useState<SiteRecord[]>([]);
  const [loading, setLoading] = useState(false);
  const [reading, setReading] = useState(false);
  const [error, setError] = useState("");
  const [source, setSource] = useState("");
  const generation = useRef(0);
  const inputRef = useRef<HTMLInputElement>(null);
  useEffect(
    () => () => {
      generation.current++;
    },
    [],
  );
  function close(restoreFocus = false) {
    generation.current++;
    setOpen(false);
    setLoading(false);
    setReading(false);
    if (restoreFocus) triggerRef.current?.focus();
  }
  useEffect(() => {
    if (!open) return;
    function place() {
      const rect = triggerRef.current?.getBoundingClientRect();
      if (!rect) return;
      const width = Math.min(360, window.innerWidth - 24);
      const below = window.innerHeight - rect.bottom - 12;
      const above = rect.top - 12;
      const upwards = below < 240 && above > below;
      setPosition({
        position: "fixed",
        width,
        left: Math.max(
          12,
          Math.min(rect.right - width, window.innerWidth - width - 12),
        ),
        top: upwards ? undefined : rect.bottom + 6,
        bottom: upwards ? window.innerHeight - rect.top + 6 : undefined,
        maxHeight: Math.max(100, Math.min(360, (upwards ? above : below) - 6)),
      });
    }
    function outside(event: PointerEvent) {
      if (
        !panelRef.current?.contains(event.target as Node) &&
        !triggerRef.current?.contains(event.target as Node)
      )
        close();
    }
    function key(event: KeyboardEvent) {
      if (event.key === "Escape") {
        event.preventDefault();
        close(true);
      }
    }
    place();
    const frame = requestAnimationFrame(() =>
      searchRef.current?.focus({ preventScroll: true }),
    );
    window.addEventListener("resize", place);
    window.addEventListener("scroll", place, true);
    document.addEventListener("pointerdown", outside);
    document.addEventListener("keydown", key);
    return () => {
      cancelAnimationFrame(frame);
      window.removeEventListener("resize", place);
      window.removeEventListener("scroll", place, true);
      document.removeEventListener("pointerdown", outside);
      document.removeEventListener("keydown", key);
    };
  }, [open]);
  const filtered = sites.filter((site) =>
    `${site.name} ${site.base_url}`
      .toLocaleLowerCase()
      .includes(query.trim().toLocaleLowerCase()),
  );
  async function loadSites() {
    const current = ++generation.current;
    setLoading(true);
    setError("");
    try {
      const result = await api<SiteRecord[]>("/api/sites");
      if (current === generation.current)
        setSites(
          result.filter(
            (site) =>
              site.auth_configured &&
              (site.auth_type === "cookie" ||
                site.auth_type === "cookie_passkey"),
          ),
        );
    } catch (e) {
      if (current === generation.current)
        setError(e instanceof Error ? e.message : "站点加载失败，请重试。");
    } finally {
      if (current === generation.current) setLoading(false);
    }
  }
  async function choose(id: string) {
    const site = sites.find((site) => String(site.id) === id);
    if (!site) return;
    const current = ++generation.current;
    setReading(true);
    setError("");
    try {
      const credentials = await api<SiteCredentialsRecord>(
        `/api/sites/${id}/credentials`,
        { cache: "no-store" },
      );
      if (current !== generation.current) return;
      if (!credentials.cookie?.trim()) {
        setError("该站点尚未配置 Cookie，请选择其他站点或手动填写。");
        return;
      }
      onChange(credentials.cookie);
      setSource(site.name);
      setOpen(false);
      requestAnimationFrame(() => inputRef.current?.focus());
    } catch (e) {
      if (current === generation.current)
        setError(
          e instanceof Error ? e.message : "Cookie 读取失败，请重新选择站点。",
        );
    } finally {
      if (current === generation.current) setReading(false);
    }
  }
  return (
    <div className="space-y-3">
      <div className="relative">
        <Input
          id="http-cookie"
          ref={inputRef}
          autoComplete="off"
          type={reveal ? "text" : "password"}
          placeholder="session=abc; token=xyz"
          value={value}
          disabled={reading}
          aria-describedby="site-cookie-help"
          className="pr-32"
          onChange={(e) => {
            setSource("");
            onChange(e.target.value);
          }}
        />
        <button
          ref={triggerRef}
          type="button"
          aria-haspopup="dialog"
          aria-expanded={open}
          aria-controls="site-cookie-picker"
          disabled={reading}
          className="absolute inset-y-1 right-1 flex items-center gap-1 rounded-md border-l border-border px-3 text-xs font-medium text-primary hover:bg-accent focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-50"
          onClick={() => {
            if (open) {
              close();
            } else {
              setQuery("");
              setOpen(true);
              void loadSites();
            }
          }}
        >
          从站点选择
          <ChevronDown className="size-3.5" aria-hidden="true" />
        </button>
      </div>
      {open &&
        createPortal(
          <div
            ref={panelRef}
            id="site-cookie-picker"
            role="dialog"
            aria-label="从站点选择 Cookie"
            style={position}
            className="z-[100] flex flex-col overflow-hidden rounded-xl border border-border bg-card shadow-lg"
            onBlur={(event) => {
              if (
                event.relatedTarget &&
                !event.currentTarget.contains(event.relatedTarget as Node) &&
                !triggerRef.current?.contains(event.relatedTarget as Node)
              )
                close();
            }}
            onKeyDown={(event) => {
              if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
              const buttons = Array.from(
                panelRef.current?.querySelectorAll<HTMLButtonElement>(
                  "button[data-site-option]:not(:disabled)",
                ) ?? [],
              );
              if (!buttons.length) return;
              event.preventDefault();
              const index = buttons.indexOf(
                document.activeElement as HTMLButtonElement,
              );
              buttons[
                (index +
                  (event.key === "ArrowDown" ? 1 : -1) +
                  buttons.length) %
                  buttons.length
              ]?.focus();
            }}
          >
            <div className="border-b border-border p-3">
              <Input
                ref={searchRef}
                aria-label="搜索站点名称或地址"
                placeholder="搜索站点名称或地址"
                value={query}
                onChange={(event) => setQuery(event.target.value)}
              />
            </div>
            <div className="min-h-0 overflow-y-auto p-1">
              {loading ? (
                <p role="status" className="p-4 text-sm text-muted">
                  正在加载站点…
                </p>
              ) : (
                filtered.map((site) => (
                  <button
                    key={site.id}
                    data-site-option
                    type="button"
                    disabled={reading}
                    onClick={() => choose(String(site.id))}
                    className="block w-full rounded-lg px-3 py-3 text-left hover:bg-accent focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring disabled:opacity-50"
                  >
                    <span className="block truncate text-sm font-medium">
                      {site.name}
                    </span>
                    <span className="mt-1 block truncate text-xs text-muted">
                      {site.base_url}
                    </span>
                  </button>
                ))
              )}
              {!loading && !filtered.length && !error && (
                <p className="p-4 text-sm leading-6 text-muted">
                  {sites.length
                    ? "没有匹配的站点，请更换搜索词。"
                    : "暂无已配置 Cookie 的站点，可在站点管理中配置或手动填写。"}
                </p>
              )}
              {reading && (
                <p role="status" className="px-3 py-2 text-sm text-muted">
                  正在读取 Cookie…
                </p>
              )}
              {error && (
                <div className="space-y-2 p-3">
                  <p role="alert" className="text-sm text-destructive">
                    {error}
                  </p>
                  <Button
                    type="button"
                    variant="outline"
                    disabled={loading || reading}
                    onClick={loadSites}
                  >
                    重新加载站点
                  </Button>
                </div>
              )}
            </div>
          </div>,
          document.body,
        )}
      <p
        id="site-cookie-help"
        role={source ? "status" : undefined}
        className="text-xs leading-6 text-muted"
      >
        {source
          ? `已从「${source}」填入 Cookie，可继续编辑。`
          : "可手动填写，或从已有站点选择 Cookie。"}
        复制当前值，站点 Cookie 更新后不会自动同步到此任务。
      </p>
    </div>
  );
}
