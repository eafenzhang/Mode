// P1.7 快捷键清单静态结构（plan: docs/plans/2026-10-18-m4-ui-replacement.md P1.7）。

import { forwardRef, type HTMLAttributes, type ReactNode } from "react";

import {
  shortcutKeyClassName,
  shortcutKeysClassName,
  shortcutLabelClassName,
  shortcutListClassName,
  shortcutRowClassName,
} from "./shortcutStyles.js";

export interface ShortcutListProps extends HTMLAttributes<HTMLDListElement> {
  label?: string;
}

export const ShortcutList = forwardRef<HTMLDListElement, ShortcutListProps>(
  function ShortcutList({ label, className, ...rest }, ref) {
    return <dl {...rest} ref={ref} aria-label={label} className={shortcutListClassName(className)} />;
  },
);

export interface ShortcutRowProps {
  label: ReactNode;
  keys: string[];
}

export function ShortcutRow({ label, keys }: ShortcutRowProps): ReactNode {
  return (
    <div className={shortcutRowClassName()}>
      <dt className={shortcutLabelClassName()}>{label}</dt>
      <dd className={shortcutKeysClassName()}>
        {keys.map((key) => (
          <kbd key={key} className={shortcutKeyClassName()}>
            {key}
          </kbd>
        ))}
      </dd>
    </div>
  );
}
