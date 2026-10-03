import type { ReactNode } from "react";
import { AlertTriangle, CheckCircle2, Info, X, XCircle } from "lucide-react";
import { cn } from "@/lib/utils";

export type NoticeTone = "info" | "success" | "warning" | "error";

const ERROR_PATTERN = /失败|错误|无法|异常|拒绝|超时|不存在|未找到|不能为空|必须|请先|invalid|error|failed|timeout/i;
const SUCCESS_PATTERN = /成功|已保存|已创建|已更新|已删除|已启动|已停止|已触发|已暂停|已恢复|已启用|已停用|已完成|已导入|已复制|已同步|已提交|已清空|已重置|已归档|已添加/;

/** 根据提示文本推断语气，用于沿用旧的纯文本 message 状态。 */
export function inferNoticeTone(text: string): NoticeTone {
  if (ERROR_PATTERN.test(text)) return "error";
  if (SUCCESS_PATTERN.test(text)) return "success";
  return "info";
}

const toneStyles: Record<NoticeTone, { box: string; icon: string; Icon: typeof Info }> = {
  info: { box: "border-border bg-surface-container/70 text-foreground", icon: "text-primary", Icon: Info },
  success: { box: "border-jade/25 bg-jade/5 text-foreground", icon: "text-jade", Icon: CheckCircle2 },
  warning: { box: "border-amber-300/70 bg-amber-50 text-amber-900", icon: "text-amber-600", Icon: AlertTriangle },
  error: { box: "border-destructive/30 bg-destructive/5 text-destructive", icon: "text-destructive", Icon: XCircle },
};

/**
 * 页面或表单内的就地反馈。错误使用 role="alert"，其余使用 role="status"，
 * 图标只作辅助，语义由文本承担。
 */
export function Notice({
  tone,
  children,
  onDismiss,
  dismissLabel = "关闭提示",
  className,
}: {
  tone?: NoticeTone;
  children: ReactNode;
  onDismiss?: () => void;
  dismissLabel?: string;
  className?: string;
}) {
  const resolvedTone = tone ?? (typeof children === "string" ? inferNoticeTone(children) : "info");
  const { box, icon, Icon } = toneStyles[resolvedTone];
  return (
    <div
      role={resolvedTone === "error" ? "alert" : "status"}
      aria-live={resolvedTone === "error" ? "assertive" : "polite"}
      className={cn("flex items-start gap-3 rounded-xl border px-4 py-3 text-sm leading-6", box, className)}
    >
      <Icon className={cn("mt-1 size-4 shrink-0", icon)} aria-hidden="true" />
      <div className="min-w-0 flex-1 break-words">{children}</div>
      {onDismiss ? (
        <button
          type="button"
          className="-my-1 -mr-2 shrink-0 rounded-lg p-1.5 text-muted transition-colors hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          aria-label={dismissLabel}
          title={dismissLabel}
          onClick={onDismiss}
        >
          <X className="size-4" aria-hidden="true" />
        </button>
      ) : null}
    </div>
  );
}
