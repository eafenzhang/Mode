// P1.2 工作区外壳（spec: docs/specs/m4-ui-foundation.md §3）。
// 静态结构：会话 frame + 可选终端 frame + 可选 Side Pane + 4px 分隔条。
// 拖拽/尺寸持久化留给 P1.2 的交互层；这里只提供可访问的 separator 与视觉契约。

import { forwardRef, type HTMLAttributes, type ReactNode } from "react";

import {
  conversationColumnClassName,
  conversationFrameClassName,
  mobileDrawerClassName,
  resizeHandleClassName,
  sidePaneFrameClassName,
  terminalFrameClassName,
  workspaceShellClassName,
  type ResizeOrientation,
} from "./shellStyles.js";

export interface ResizeHandleProps extends HTMLAttributes<HTMLDivElement> {
  orientation: ResizeOrientation;
  label: string;
  dragging?: boolean;
}

export const ResizeHandle = forwardRef<HTMLDivElement, ResizeHandleProps>(
  function ResizeHandle({ orientation, label, dragging = false, className, ...rest }, ref) {
    return (
      <div
        {...rest}
        ref={ref}
        role="separator"
        aria-label={label}
        aria-orientation={orientation}
        tabIndex={0}
        data-dragging={dragging || undefined}
        className={resizeHandleClassName(orientation, className)}
      />
    );
  },
);

export interface AppShellProps extends HTMLAttributes<HTMLDivElement> {
  conversation: ReactNode;
  conversationLabel: string;
  terminal?: ReactNode;
  terminalLabel?: string;
  sidePane?: ReactNode;
  sidePaneLabel?: string;
  /** 手机远控的 drawer；存在时叠加在单列布局上。 */
  mobileDrawer?: ReactNode;
}

export const AppShell = forwardRef<HTMLDivElement, AppShellProps>(function AppShell(
  {
    conversation,
    conversationLabel,
    terminal,
    terminalLabel,
    sidePane,
    sidePaneLabel,
    mobileDrawer,
    className,
    ...rest
  },
  ref,
) {
  return (
    <div {...rest} ref={ref} className={workspaceShellClassName(className)}>
      <div className={conversationColumnClassName()}>
        <section aria-label={conversationLabel} className={conversationFrameClassName()}>
          {conversation}
        </section>
        {terminal ? (
          <>
            <ResizeHandle orientation="horizontal" label={terminalLabel ?? conversationLabel} />
            <section aria-label={terminalLabel} className={terminalFrameClassName()}>
              {terminal}
            </section>
          </>
        ) : null}
      </div>
      {sidePane ? (
        <>
          <ResizeHandle orientation="vertical" label={sidePaneLabel ?? conversationLabel} />
          <aside aria-label={sidePaneLabel} className={sidePaneFrameClassName()}>
            {sidePane}
          </aside>
        </>
      ) : null}
      {mobileDrawer ? <div className={mobileDrawerClassName()}>{mobileDrawer}</div> : null}
    </div>
  );
});
