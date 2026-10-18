// P1.3 diff 块（plan: docs/plans/2026-10-18-m4-ui-replacement.md P1.3）。
// 只负责容器与行语义；patch 文本由调用方从事件/工具结果取得。

import { forwardRef, type HTMLAttributes, type ReactNode } from "react";

import {
  diffBlockClassName,
  diffFilePathClassName,
  diffHeaderClassName,
  diffLineClassName,
  diffMarker,
  diffStatsClassName,
  type DiffLineKind,
} from "./diffStyles.js";

export interface DiffBlockProps extends HTMLAttributes<HTMLElement> {
  filePath?: ReactNode;
  additions?: number;
  deletions?: number;
  children?: ReactNode;
}

export const DiffBlock = forwardRef<HTMLElement, DiffBlockProps>(function DiffBlock(
  { filePath, additions = 0, deletions = 0, className, children, ...rest },
  ref,
) {
  return (
    <section
      {...rest}
      ref={ref}
      aria-label={typeof filePath === "string" ? filePath : undefined}
      className={diffBlockClassName(className)}
    >
      <div className={diffHeaderClassName()}>
        <span className={diffFilePathClassName()}>{filePath}</span>
        <span className={diffStatsClassName()} aria-label={`+${additions} -${deletions}`}>
          <span className="text-diff-added-foreground">+{additions}</span>
          <span className="text-diff-removed-foreground">-{deletions}</span>
        </span>
      </div>
      <div className="overflow-x-auto">{children}</div>
    </section>
  );
});

export interface DiffLineProps extends HTMLAttributes<HTMLDivElement> {
  kind: DiffLineKind;
  children?: ReactNode;
}

export const DiffLine = forwardRef<HTMLDivElement, DiffLineProps>(function DiffLine(
  { kind, className, children, ...rest },
  ref,
) {
  return (
    <div {...rest} ref={ref} data-kind={kind} className={diffLineClassName(kind, className)}>
      <span aria-hidden="true" className="select-none">
        {diffMarker(kind)}
      </span>
      <span className="min-w-0 whitespace-pre-wrap break-words">{children}</span>
    </div>
  );
});
