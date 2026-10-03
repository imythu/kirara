import * as React from "react";
import { cva, type VariantProps } from "class-variance-authority";
import { Loader2 } from "lucide-react";
import { cn } from "@/lib/utils";

const buttonVariants = cva(
  "inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-lg text-sm font-semibold transition-colors duration-150 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-card disabled:pointer-events-none disabled:opacity-50 [&_svg]:size-4 [&_svg]:shrink-0 [&>svg]:m-0 [&[aria-busy=true]>svg:not(.animate-spin)]:hidden",
  {
    variants: {
      variant: {
        default: "bg-primary text-primary-foreground hover:bg-[#603c8e] active:bg-[#4e2f77]",
        secondary: "border border-primary/10 bg-secondary text-secondary-foreground hover:bg-accent",
        destructive: "bg-destructive text-white hover:brightness-105",
        outline: "border border-border bg-card text-foreground hover:border-primary/35 hover:bg-accent"
      },
      size: {
        default: "h-10 px-5 py-2",
        sm: "h-9 px-3",
        icon: "size-10 p-0",
      },
    },
    defaultVariants: {
      variant: "default",
      size: "default",
    },
  },
);

export interface ButtonProps
  extends React.ButtonHTMLAttributes<HTMLButtonElement>,
    VariantProps<typeof buttonVariants> {
  /** 异步操作进行中：禁用按钮、显示旋转图标并设置 aria-busy。 */
  loading?: boolean;
}

const Button = React.forwardRef<HTMLButtonElement, ButtonProps>(
  ({ className, variant, size, loading = false, disabled, children, ...props }, ref) => (
    <button
      ref={ref}
      className={cn(buttonVariants({ variant, size }), className)}
      disabled={disabled || loading}
      aria-busy={loading || undefined}
      {...props}
    >
      {loading ? <Loader2 className="animate-spin" aria-hidden="true" /> : null}
      {children}
    </button>
  ),
);
Button.displayName = "Button";

export { Button, buttonVariants };
