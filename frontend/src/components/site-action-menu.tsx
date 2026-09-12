import { useEffect, useId, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { ChevronDown } from "lucide-react";
import { Button } from "@/components/ui/button";

interface SiteAction {
  label: string;
  description: string;
  icon: ReactNode;
  disabled?: boolean;
  onSelect: () => void;
}

export function SiteActionMenu({ onSync, disabled, hasError, actions }: {
  onSync: () => void;
  disabled: boolean;
  hasError: boolean;
  actions: SiteAction[];
}) {
  const [open, setOpen] = useState(false);
  const [position, setPosition] = useState({ top: 0, left: 0, width: 288, maxHeight: 400 });
  const id = useId();
  const anchor = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const menu = useRef<HTMLDivElement>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const hoverOpened = useRef(false);
  const pendingFocus = useRef<"first" | "last" | null>(null);
  function cancelClose() { if (timer.current) clearTimeout(timer.current); }
  function close(restoreFocus = false) {
    cancelClose();
    hoverOpened.current = false;
    setOpen(false);
    if (restoreFocus) trigger.current?.focus();
  }
  function leave() {
    cancelClose();
    timer.current = setTimeout(() => {
      if (!menu.current?.contains(document.activeElement)) close();
    }, 180);
  }
  function items() { return Array.from(menu.current?.querySelectorAll<HTMLButtonElement>('[role="menuitem"]:not(:disabled)') ?? []); }
  useLayoutEffect(() => {
    if (!open) return;
    function reposition() {
      const rect = anchor.current?.getBoundingClientRect();
      if (!rect) return;
      if (!rect.width) { setOpen(false); return; }
      const width = Math.min(288, window.innerWidth - 24);
      const height = menu.current?.offsetHeight ?? 280;
      const below = window.innerHeight - rect.bottom - 12;
      const above = rect.top - 12;
      const top = below < height && above > below ? Math.max(12, rect.top - height) : rect.bottom;
      setPosition({ top, left: Math.max(12, Math.min(rect.right - width, window.innerWidth - width - 12)), width, maxHeight: Math.max(80, top < rect.top ? above : below) });
    }
    reposition();
    const target = pendingFocus.current;
    if (target) {
      const options = items();
      (target === "last" ? options[options.length - 1] : options[0])?.focus();
      pendingFocus.current = null;
    }
    window.addEventListener("resize", reposition);
    window.addEventListener("scroll", reposition, true);
    return () => { window.removeEventListener("resize", reposition); window.removeEventListener("scroll", reposition, true); };
  }, [open]);
  useEffect(() => {
    if (!open) return;
    function outside(event: PointerEvent) {
      const target = event.target as Node;
      if (!anchor.current?.contains(target) && !menu.current?.contains(target)) close();
    }
    function escape(event: KeyboardEvent) {
      if (event.key === "Escape") { event.preventDefault(); close(Boolean(menu.current?.contains(document.activeElement))); }
    }
    function focusOutside(event: FocusEvent) {
      const target = event.target as Node;
      if (!anchor.current?.contains(target) && !menu.current?.contains(target)) close();
    }
    document.addEventListener("pointerdown", outside);
    document.addEventListener("keydown", escape);
    document.addEventListener("focusin", focusOutside);
    return () => {
      document.removeEventListener("pointerdown", outside);
      document.removeEventListener("keydown", escape);
      document.removeEventListener("focusin", focusOutside);
    };
  }, [open]);
  useEffect(() => () => cancelClose(), []);

  return <>
    <div ref={anchor} className="inline-flex shrink-0 items-stretch" onPointerEnter={cancelClose} onPointerLeave={leave}>
      <Button variant="secondary" className="h-11 rounded-r-none border-primary/20 px-3 text-primary focus-visible:relative focus-visible:z-10" disabled={disabled} aria-describedby="hive-sync-help" onClick={() => { close(); onSync(); }}>
        同步到蜂巢{hasError && <span className="sr-only">（上次备份异常）</span>}
      </Button>
      <Button ref={trigger} variant="outline" className="h-11 min-w-11 gap-1 rounded-l-none border-l-0 border-primary/20 px-2 focus-visible:relative focus-visible:z-10 aria-expanded:bg-accent sm:px-3" aria-label="更多站点操作" aria-haspopup="menu" aria-expanded={open} aria-controls={open ? id : undefined}
        onPointerEnter={event => {
          cancelClose();
          if (event.pointerType === "mouse" && !open) { hoverOpened.current = true; setOpen(true); }
        }}
        onClick={() => {
          cancelClose();
          if (open && !hoverOpened.current) close();
          else {
            hoverOpened.current = false;
            pendingFocus.current = "first";
            if (open) { items()[0]?.focus(); pendingFocus.current = null; }
            else setOpen(true);
          }
        }}
        onKeyDown={event => {
          if (event.key === "ArrowDown" || event.key === "ArrowUp") {
            event.preventDefault();
            const target = event.key === "ArrowUp" ? "last" : "first";
            if (open) { const options = items(); (target === "last" ? options[options.length - 1] : options[0])?.focus(); }
            else { pendingFocus.current = target; setOpen(true); }
          }
        }}>
        <span>更多</span>
        <ChevronDown aria-hidden="true" className={`size-4 transition-transform duration-150 motion-reduce:transition-none ${open ? "rotate-180" : ""}`} />
      </Button>
    </div>
    {open && createPortal(<div ref={menu} id={id} role="menu" aria-label="更多站点操作" className="fixed z-50 overflow-y-auto rounded-xl bg-card p-1.5 shadow-lg ring-1 ring-border" style={position} onPointerEnter={cancelClose} onPointerLeave={leave}
      onKeyDown={event => {
        const options = items();
        const index = options.indexOf(document.activeElement as HTMLButtonElement);
        if (["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) {
          event.preventDefault();
          const next = event.key === "Home" ? 0 : event.key === "End" ? options.length - 1 : (index + (event.key === "ArrowDown" ? 1 : -1) + options.length) % options.length;
          options[next]?.focus();
        } else if (event.key === "Tab") close(true);
      }}>
      {actions.map(action => <button key={action.label} type="button" role="menuitem" tabIndex={-1} disabled={action.disabled} className="flex min-h-11 w-full items-start gap-3 rounded-lg px-3 py-3 text-left text-sm transition-colors hover:bg-accent focus:bg-accent focus:outline-none disabled:cursor-not-allowed disabled:opacity-50" onClick={() => { close(true); action.onSelect(); }}>
        <span aria-hidden="true" className="mt-0.5 shrink-0 text-muted [&_svg]:size-4">{action.icon}</span>
        <span className="min-w-0"><span className="block font-semibold">{action.label}</span><span className="mt-0.5 block text-xs leading-5 text-muted">{action.description}</span></span>
      </button>)}
    </div>, document.body)}
  </>;
}
