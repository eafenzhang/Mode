// P1.1 Kbd / KbdGroup primitive（spec: docs/specs/m4-ui-foundation.md §2.9）。

import { forwardRef, type HTMLAttributes } from "react";

export interface KbdProps extends HTMLAttributes<HTMLElement> {}

export const Kbd = forwardRef<HTMLElement, KbdProps>(function Kbd({ className, ...rest }, ref) {
  return (
    <kbd
      {...rest}
      ref={ref}
      className={[
        "rounded-sm border border-border bg-surface px-1 text-ui-xs text-foreground-subtle",
        className,
      ]
        .filter(Boolean)
        .join(" ")}
    />
  );
});

export interface KbdGroupProps extends HTMLAttributes<HTMLSpanElement> {}

export const KbdGroup = forwardRef<HTMLSpanElement, KbdGroupProps>(function KbdGroup(
  { className, ...rest },
  ref,
) {
  return (
    <span
      {...rest}
      ref={ref}
      className={["inline-flex items-center gap-1", className].filter(Boolean).join(" ")}
    />
  );
});
