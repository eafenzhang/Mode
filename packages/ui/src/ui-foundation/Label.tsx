// P1.1 Label primitive（spec: docs/specs/m4-ui-foundation.md §2.9）。

import { forwardRef, type LabelHTMLAttributes } from "react";
import { cn } from "@/components/lib/utils.js";

export interface LabelProps extends LabelHTMLAttributes<HTMLLabelElement> {}

export const Label = forwardRef<HTMLLabelElement, LabelProps>(function Label(
  { className, ...rest },
  ref,
) {
  return (
    <label
      {...rest}
      ref={ref}
      className={cn(["text-ui-sm font-medium text-foreground", className])}
    />
  );
});
