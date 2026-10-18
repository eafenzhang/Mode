// P1.2 WorkspaceHeader（spec: docs/specs/m4-ui-foundation.md §3）。
// 标题/副标题/状态/操作槽位；文案由调用方传入（i18n 不走组件内部）。

import { forwardRef, type HTMLAttributes, type ReactNode } from "react";

import {
  workspaceHeaderActionsClassName,
  workspaceHeaderClassName,
  workspaceHeaderLeadingClassName,
  workspaceHeaderSubtitleClassName,
  workspaceHeaderTitleClassName,
} from "./headerStyles.js";

export interface WorkspaceHeaderProps extends Omit<HTMLAttributes<HTMLElement>, "title"> {
  title: ReactNode;
  subtitle?: ReactNode;
  leading?: ReactNode;
  status?: ReactNode;
  actions?: ReactNode;
}

export const WorkspaceHeader = forwardRef<HTMLElement, WorkspaceHeaderProps>(
  function WorkspaceHeader(
    { title, subtitle, leading, status, actions, className, ...rest },
    ref,
  ) {
    return (
      <header {...rest} ref={ref} className={workspaceHeaderClassName(className)}>
        {leading ? <div className={workspaceHeaderLeadingClassName()}>{leading}</div> : null}
        <div className="min-w-0 flex-1">
          <div className={workspaceHeaderTitleClassName()}>{title}</div>
          {subtitle ? (
            <div className={workspaceHeaderSubtitleClassName()}>{subtitle}</div>
          ) : null}
        </div>
        {status ? <div className="shrink-0">{status}</div> : null}
        {actions ? <div className={workspaceHeaderActionsClassName()}>{actions}</div> : null}
      </header>
    );
  },
);
