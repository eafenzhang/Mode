// P1.1 Tabs primitive（spec: docs/specs/m4-ui-foundation.md §2.5）。

import { forwardRef, type ButtonHTMLAttributes, type HTMLAttributes } from "react";

import { tabClassName, tabListClassName } from "./feedbackStyles.js";

export interface TabsProps extends HTMLAttributes<HTMLDivElement> {
  label?: string;
}

export const Tabs = forwardRef<HTMLDivElement, TabsProps>(function Tabs(
  { label, className, ...rest },
  ref,
) {
  return (
    <div {...rest} ref={ref} role="tablist" aria-label={label} className={tabListClassName(className)} />
  );
});

export interface TabProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  selected: boolean;
}

export const Tab = forwardRef<HTMLButtonElement, TabProps>(function Tab(
  { selected, className, children, type, ...rest },
  ref,
) {
  return (
    <button
      {...rest}
      ref={ref}
      type={type ?? "button"}
      role="tab"
      aria-selected={selected}
      tabIndex={selected ? 0 : -1}
      className={tabClassName({ selected, className })}
    >
      {children}
    </button>
  );
});
