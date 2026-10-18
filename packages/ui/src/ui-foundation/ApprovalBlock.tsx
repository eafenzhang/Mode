// P1.3 审批/提问块（plan: docs/plans/2026-10-18-m4-ui-replacement.md P1.3）。
// 等待态统一 confirmation 绿色；允许/拒绝使用语义状态色。

import { forwardRef, type HTMLAttributes, type ReactNode } from "react";

import {
  approvalActionsClassName,
  approvalBlockClassName,
  approvalDescriptionClassName,
  approvalStatusClassName,
  approvalTitleClassName,
  type ApprovalStatus,
} from "./toolStyles.js";

export interface ApprovalBlockProps extends Omit<HTMLAttributes<HTMLElement>, "title"> {
  title: ReactNode;
  description?: ReactNode;
  status: ApprovalStatus;
  /** 状态文案（i18n 由调用方提供）。 */
  statusLabel?: string;
  actions?: ReactNode;
}

export const ApprovalBlock = forwardRef<HTMLElement, ApprovalBlockProps>(
  function ApprovalBlock(
    { title, description, status, statusLabel, actions, className, ...rest },
    ref,
  ) {
    return (
      <section
        {...rest}
        ref={ref}
        aria-label={typeof title === "string" ? title : undefined}
        data-status={status}
        className={approvalBlockClassName(className)}
      >
        <div className="flex min-w-0 items-start gap-2">
          <div className="min-w-0 flex-1">
            <div className={approvalTitleClassName()}>{title}</div>
            {description ? (
              <div className={approvalDescriptionClassName()}>{description}</div>
            ) : null}
          </div>
          {statusLabel ? (
            <span aria-live="polite" className={approvalStatusClassName(status)}>
              {statusLabel}
            </span>
          ) : null}
        </div>
        {actions ? <div className={approvalActionsClassName()}>{actions}</div> : null}
      </section>
    );
  },
);
