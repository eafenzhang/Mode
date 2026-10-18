// P1.3 工具调用块（plan: docs/plans/2026-10-18-m4-ui-replacement.md P1.3）。
// 五态来自 tauri-agent-runtime 的 tool.updated：scheduled/started/progress/result/error。

import { forwardRef, type HTMLAttributes, type ReactNode } from "react";

import {
  toolBlockClassName,
  toolBodyClassName,
  toolHeaderClassName,
  toolNameClassName,
  toolStatusClassName,
  type ToolBlockState,
} from "./toolStyles.js";

export interface ToolBlockProps extends HTMLAttributes<HTMLElement> {
  state: ToolBlockState;
  name: string;
  status?: ReactNode;
  children?: ReactNode;
}

export const ToolBlock = forwardRef<HTMLElement, ToolBlockProps>(function ToolBlock(
  { state, name, status, className, children, ...rest },
  ref,
) {
  return (
    <section
      {...rest}
      ref={ref}
      aria-label={name}
      data-state={state}
      className={toolBlockClassName(state, className)}
    >
      <div className={toolHeaderClassName()}>
        <span className={toolNameClassName()}>{name}</span>
        {status ? <span className={toolStatusClassName(state)}>{status}</span> : null}
      </div>
      {children ? <div className={toolBodyClassName()}>{children}</div> : null}
    </section>
  );
});
