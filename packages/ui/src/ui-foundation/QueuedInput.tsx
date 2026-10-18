// P1.3 排队输入（plan: docs/plans/2026-10-18-m4-ui-replacement.md P1.3）。
// 只展示已接受但尚未执行的输入；队列语义归 CommandInbox。

import { forwardRef, type HTMLAttributes, type ReactNode } from "react";

import {
  queuedInputActionsClassName,
  queuedInputItemClassName,
  queuedInputListClassName,
  queuedInputTextClassName,
} from "./diffStyles.js";

export interface QueuedInputListProps extends HTMLAttributes<HTMLUListElement> {
  /** 列表可访问名称（i18n）。 */
  label: string;
}

export const QueuedInputList = forwardRef<HTMLUListElement, QueuedInputListProps>(
  function QueuedInputList({ label, className, ...rest }, ref) {
    return (
      <ul {...rest} ref={ref} aria-label={label} className={queuedInputListClassName(className)} />
    );
  },
);

export interface QueuedInputItemProps extends HTMLAttributes<HTMLLIElement> {
  text: ReactNode;
  actions?: ReactNode;
}

export const QueuedInputItem = forwardRef<HTMLLIElement, QueuedInputItemProps>(
  function QueuedInputItem({ text, actions, className, ...rest }, ref) {
    return (
      <li {...rest} ref={ref} className={queuedInputItemClassName(className)}>
        <span className={queuedInputTextClassName()}>{text}</span>
        {actions ? <div className={queuedInputActionsClassName()}>{actions}</div> : null}
      </li>
    );
  },
);
