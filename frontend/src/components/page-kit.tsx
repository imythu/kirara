import type { ReactNode } from "react";
import { Card, CardContent } from "@/components/ui/card";
import { cn } from "@/lib/utils";

export function PageHero({
  eyebrow,
  title,
  description,
  icon,
  action,
  className,
}: {
  eyebrow: string;
  title: string;
  description: string;
  icon?: ReactNode;
  action?: ReactNode;
  className?: string;
}) {
  return (
    <Card className={cn("overflow-hidden border-border bg-[rgb(var(--app-shell))] text-[rgb(var(--app-shell-foreground))] shadow-lift", className)}>
      <CardContent className="relative p-6 sm:p-7">
        <div className="absolute inset-0 bg-[linear-gradient(180deg,rgba(255,255,255,0.02),transparent)]" />
        <div className="relative flex flex-col gap-5 lg:flex-row lg:items-end lg:justify-between">
          <div>
            <div className="inline-flex items-center gap-2 rounded-full border border-white/10 bg-white/5 px-3 py-1 text-[11px] font-semibold uppercase tracking-[0.18em] text-accent">
              {icon}
              {eyebrow}
            </div>
            <h3 className="mt-4 text-3xl font-semibold tracking-tight">{title}</h3>
            <p className="mt-3 max-w-2xl text-sm leading-7 text-[rgb(var(--app-shell-foreground))/0.72]">{description}</p>
          </div>
          {action ? <div className="shrink-0">{action}</div> : null}
        </div>
      </CardContent>
    </Card>
  );
}

export function MetricSurface({
  label,
  value,
  detail,
  className,
}: {
  label: string;
  value: ReactNode;
  detail?: string;
  className?: string;
}) {
  return (
    <Card className={cn("bg-card", className)}>
      <CardContent className="p-5">
        <div className="text-[11px] font-semibold uppercase tracking-[0.18em] text-muted">{label}</div>
        <div className="mt-3 text-3xl font-semibold tracking-tight">{value}</div>
        {detail ? <div className="mt-1 text-sm text-muted">{detail}</div> : null}
      </CardContent>
    </Card>
  );
}

export function SoftPanel({
  children,
  className,
}: {
  children: ReactNode;
  className?: string;
}) {
  return (
    <div
      className={cn(
        "rounded-2xl border border-border bg-surface-container/90 p-5 shadow-card",
        className,
      )}
    >
      {children}
    </div>
  );
}
