// P1.1 Label primitive（spec: docs/specs/m4-ui-foundation.md §2.9）。

import { forwardRef, type LabelHTMLAttributes } from "react";

export interface LabelProps extends LabelHTMLAttributes<HTMLLabelElement> {}

export const Label = forwardRef<HTMLLabelElement, LabelProps>(function Label(
  { className, ...rest },
  ref,
) {
  return (
    <label
      {...rest}
      ref={ref}
      className={["text-ui-sm font-medium text-foreground", className].filter(Boolean).join(" ")}
    />
  );
});
