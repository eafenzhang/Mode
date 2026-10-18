// P1.1 Button primitive（spec: docs/specs/m4-ui-foundation.md §2.1）。
// 类名规则在 buttonStyles.ts（纯函数，可单测）；本文件只负责 DOM 与可访问性属性。

import { forwardRef, type ButtonHTMLAttributes } from "react";

import { buttonClassName, type ButtonSize, type ButtonVariant } from "./buttonStyles.js";
import type { RadiusLevel } from "../lib/uiFoundation.js";

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: ButtonVariant;
  size?: ButtonSize;
  parentRadius?: RadiusLevel;
  /** 提交中：保留宽度、阻止重复点击，并标记 aria-busy。 */
  loading?: boolean;
}

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(function Button(
  { variant, size, parentRadius, loading = false, disabled, className, type, children, ...rest },
  ref,
) {
  return (
    <button
      {...rest}
      ref={ref}
      type={type ?? "button"}
      disabled={disabled || loading}
      aria-busy={loading || undefined}
      className={buttonClassName({ variant, size, parentRadius, className })}
    >
      {loading ? (
        <span
          aria-hidden="true"
          className="size-3 animate-spin rounded-full border-2 border-current border-t-transparent"
        />
      ) : null}
      {children}
    </button>
  );
});
