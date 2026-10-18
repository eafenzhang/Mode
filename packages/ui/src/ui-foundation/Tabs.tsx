// P1.1 Tabs primitive（spec: docs/specs/m4-ui-foundation.md §2.5）。

import {
  forwardRef,
  type ButtonHTMLAttributes,
  type HTMLAttributes,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react";

import { tabClassName, tabListClassName } from "./feedbackStyles.js";
import { isTabNavigationKey, nextTabIndex } from "./tabsKeyboard.js";

export interface TabsProps extends HTMLAttributes<HTMLDivElement> {
  label?: string;
}

export const Tabs = forwardRef<HTMLDivElement, TabsProps>(function Tabs(
  { label, className, onKeyDown, children, ...rest },
  ref,
) {
  const handleKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>): void => {
    onKeyDown?.(event);
    if (event.defaultPrevented || !isTabNavigationKey(event.key)) {
      return;
    }
    const container = event.currentTarget;
    const tabs = [...container.querySelectorAll<HTMLElement>('[role="tab"]:not([disabled])')];
    const current = tabs.indexOf(document.activeElement as HTMLElement);
    const next = nextTabIndex(current, tabs.length, event.key);
    if (next === null) {
      return;
    }
    event.preventDefault();
    tabs[next]?.focus();
  };

  return (
    <div
      {...rest}
      ref={ref}
      role="tablist"
      aria-label={label}
      onKeyDown={handleKeyDown}
      className={tabListClassName(className)}
    >
      {children}
    </div>
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
