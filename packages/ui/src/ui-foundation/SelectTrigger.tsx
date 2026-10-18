// P1.1 Select trigger primitive（spec: docs/specs/m4-ui-foundation.md §2.2）。
// 触发器保持输入框语义；展开面板由菜单 primitive 负责（shell rounded-lg）。

import { forwardRef, type ButtonHTMLAttributes } from "react";

import type { RadiusLevel } from "../lib/uiFoundation.js";
import { selectTriggerClassName, type FieldSize } from "./inputStyles.js";

export interface SelectTriggerProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  fieldSize?: FieldSize;
  parentRadius?: RadiusLevel;
  invalid?: boolean;
  open?: boolean;
  showChevron?: boolean;
}

export const SelectTrigger = forwardRef<HTMLButtonElement, SelectTriggerProps>(
  function SelectTrigger(
    {
      fieldSize,
      parentRadius,
      invalid,
      open = false,
      showChevron = true,
      className,
      children,
      disabled,
      ...rest
    },
    ref,
  ) {
    return (
      <button
        {...rest}
        ref={ref}
        type="button"
        disabled={disabled}
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-invalid={invalid || undefined}
        className={selectTriggerClassName({
          size: fieldSize,
          parentRadius,
          invalid,
          open,
          className,
        })}
      >
        <span className="min-w-0 truncate">{children}</span>
        {showChevron ? (
          <svg
            aria-hidden="true"
            viewBox="0 0 16 16"
            className="size-4 shrink-0 text-foreground-subtle"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.5"
            strokeLinecap="round"
            strokeLinejoin="round"
          >
            <path d="M4 6l4 4 4-4" />
          </svg>
        ) : null}
      </button>
    );
  },
);
