// P1.7 quickpick 静态结构（plan: docs/plans/2026-10-18-m4-ui-replacement.md P1.7）。

import {
  forwardRef,
  type ButtonHTMLAttributes,
  type HTMLAttributes,
  type InputHTMLAttributes,
  type ReactNode,
} from "react";

import {
  quickpickEmptyClassName,
  quickpickInputClassName,
  quickpickItemClassName,
  quickpickListClassName,
  quickpickPanelClassName,
} from "./quickpickStyles.js";

export const QuickpickPanel = forwardRef<HTMLDivElement, HTMLAttributes<HTMLDivElement>>(
  function QuickpickPanel({ className, ...rest }, ref) {
    return <div {...rest} ref={ref} className={quickpickPanelClassName(className)} />;
  },
);

export const QuickpickInput = forwardRef<HTMLInputElement, InputHTMLAttributes<HTMLInputElement>>(
  function QuickpickInput({ className, ...rest }, ref) {
    return <input {...rest} ref={ref} type="search" className={quickpickInputClassName(className)} />;
  },
);

export interface QuickpickListProps extends HTMLAttributes<HTMLUListElement> {
  label: string;
}

export const QuickpickList = forwardRef<HTMLUListElement, QuickpickListProps>(
  function QuickpickList({ label, className, ...rest }, ref) {
    return (
      <ul {...rest} ref={ref} role="listbox" aria-label={label} className={quickpickListClassName(className)} />
    );
  },
);

export interface QuickpickItemProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  selected?: boolean;
}

export const QuickpickItem = forwardRef<HTMLButtonElement, QuickpickItemProps>(
  function QuickpickItem({ selected, className, type, children, ...rest }, ref) {
    return (
      <button
        {...rest}
        ref={ref}
        type={type ?? "button"}
        role="option"
        aria-selected={selected}
        className={quickpickItemClassName({ selected, className })}
      >
        {children}
      </button>
    );
  },
);

export function QuickpickEmpty({
  className,
  children,
}: HTMLAttributes<HTMLDivElement>): ReactNode {
  return <div className={quickpickEmptyClassName(className)}>{children}</div>;
}
