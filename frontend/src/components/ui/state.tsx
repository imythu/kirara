import type { ReactNode } from "react";
import { Loader2 } from "lucide-react";
import { cn } from "@/lib/utils";

/** 列表或面板的加载占位，带 role="status" 以便读屏播报。 */
export function LoadingState({ label = "加载中…", className }: { label?: string; className?: string }) {
  return (
    <div role="status" aria-live="polite" className={cn("flex items-center justify-center gap-2 py-10 text-sm text-muted", className)}>
      <Loader2 className="size-4 animate-spin text-primary" aria-hidden="true" />
      <span>{label}</span>
    </div>
  );
}

/** 简单的空状态：标题加一句说明与可选操作。 */
export function EmptyHint({
  title,
  children,
  action,
  className,
}: {
  title: string;
  children?: ReactNode;
  action?: ReactNode;
  className?: string;
}) {
  return (
    <div className={cn("flex flex-col items-center justify-center gap-2 rounded-xl border border-dashed border-border px-4 py-10 text-center", className)}>
      <p className="text-sm font-semibold text-foreground">{title}</p>
      {children ? <p className="max-w-md text-sm leading-6 text-muted">{children}</p> : null}
      {action ? <div className="mt-2">{action}</div> : null}
    </div>
  );
}
