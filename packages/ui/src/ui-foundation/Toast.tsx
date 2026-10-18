// P1.1 Toast primitive（spec: docs/specs/m4-ui-foundation.md §2.6）。

import { forwardRef, type HTMLAttributes } from "react";

import { toastClassName } from "./feedbackStyles.js";

export interface ToastProps extends HTMLAttributes<HTMLDivElement> {
  destructive?: boolean;
}

export const Toast = forwardRef<HTMLDivElement, ToastProps>(function Toast(
  { destructive, className, children, ...rest },
  ref,
) {
  return (
    <div
      {...rest}
      ref={ref}
      role={destructive ? "alert" : "status"}
      className={toastClassName({ destructive, className })}
    >
      {children}
    </div>
  );
});
