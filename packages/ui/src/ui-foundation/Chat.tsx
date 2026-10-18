// P1.3 会话核心静态结构（plan: docs/plans/2026-10-18-m4-ui-replacement.md P1.3）。
// 只做布局与可访问性；消息数据/流式事件由 P1.3 的接线层注入。

import { forwardRef, type HTMLAttributes, type ReactNode } from "react";

import {
  chatInputShellClassName,
  chatInputToolbarClassName,
  emptyDescriptionClassName,
  emptyStateClassName,
  emptyTitleClassName,
  messageBubbleClassName,
  messageListClassName,
  messageRowClassName,
  streamingIndicatorClassName,
  type ChatRole,
} from "./chatStyles.js";

export const MessageList = forwardRef<HTMLDivElement, HTMLAttributes<HTMLDivElement>>(
  function MessageList({ className, ...rest }, ref) {
    return (
      <div
        {...rest}
        ref={ref}
        role="log"
        aria-live="polite"
        aria-relevant="additions text"
        className={messageListClassName(className)}
      />
    );
  },
);

export interface MessageRowProps extends Omit<HTMLAttributes<HTMLDivElement>, "role"> {
  role: ChatRole;
  children?: ReactNode;
}

export const MessageRow = forwardRef<HTMLDivElement, MessageRowProps>(function MessageRow(
  { role, className, children, ...rest },
  ref,
) {
  return (
    <div {...rest} ref={ref} className={messageRowClassName(role, className)}>
      <div className={messageBubbleClassName(role)}>{children}</div>
    </div>
  );
});

export interface EmptyChatStateProps extends Omit<HTMLAttributes<HTMLDivElement>, "title"> {
  title: ReactNode;
  description?: ReactNode;
  action?: ReactNode;
}

export const EmptyChatState = forwardRef<HTMLDivElement, EmptyChatStateProps>(
  function EmptyChatState({ title, description, action, className, ...rest }, ref) {
    return (
      <div {...rest} ref={ref} className={emptyStateClassName(className)}>
        <div className={emptyTitleClassName()}>{title}</div>
        {description ? <div className={emptyDescriptionClassName()}>{description}</div> : null}
        {action}
      </div>
    );
  },
);

export interface StreamingIndicatorProps extends HTMLAttributes<HTMLDivElement> {
  /** 读屏文本（由调用方 i18n）。 */
  label: string;
}

export function StreamingIndicator({
  label,
  className,
  ...rest
}: StreamingIndicatorProps): ReactNode {
  return (
    <div
      {...rest}
      role="status"
      aria-label={label}
      className={streamingIndicatorClassName(className)}
    >
      {[0, 1, 2].map((index) => (
        <span
          key={index}
          aria-hidden="true"
          className="size-1 animate-pulse rounded-full bg-current"
        />
      ))}
    </div>
  );
}

export interface ChatInputShellProps extends HTMLAttributes<HTMLDivElement> {
  toolbar?: ReactNode;
  children?: ReactNode;
}

export const ChatInputShell = forwardRef<HTMLDivElement, ChatInputShellProps>(
  function ChatInputShell({ toolbar, className, children, ...rest }, ref) {
    return (
      <div {...rest} ref={ref} className={chatInputShellClassName(className)}>
        {children}
        {toolbar ? <div className={chatInputToolbarClassName()}>{toolbar}</div> : null}
      </div>
    );
  },
);
