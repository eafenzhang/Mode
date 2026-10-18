// P1.1 Lightweight diff preview（spec: docs/specs/m4-ui-foundation.md §2.16）。
// 逐行渲染，不解析完整 patch；用于超大/多文件 patch 的降级路径。

import type { CSSProperties, HTMLAttributes, ReactNode } from "react";
import { useMemo } from "react";

import type { CodePreviewSettings } from "@/store/index.js";

export type LightweightDiffLineKind = "added" | "removed" | "context";

export interface LightweightDiffLineParts {
  content: string;
  kind: LightweightDiffLineKind;
  marker: string;
  raw: string;
}

export function getLightweightDiffLineParts(line: string): LightweightDiffLineParts {
  if (line.startsWith("+")) {
    return { kind: "added", marker: "+", content: line.slice(1), raw: line };
  }
  if (line.startsWith("-")) {
    return { kind: "removed", marker: "-", content: line.slice(1), raw: line };
  }
  if (line.startsWith(" ")) {
    return { kind: "context", marker: " ", content: line.slice(1), raw: line };
  }
  return { kind: "context", marker: "", content: line, raw: line };
}

const ROW_BY_KIND: Record<LightweightDiffLineKind, string> = {
  added: "bg-diff-added text-diff-added-foreground",
  removed: "bg-diff-removed text-diff-removed-foreground",
  context: "text-foreground",
};

export interface LightweightDiffPreviewProps extends Omit<
  HTMLAttributes<HTMLDivElement>,
  "children"
> {
  codePreviewSettings: Pick<
    CodePreviewSettings,
    "fontSizePx" | "showLineNumbers" | "wrapLongLines"
  >;
  lines: readonly string[];
  renderLineContent?: (line: LightweightDiffLineParts, index: number) => ReactNode;
}

export function LightweightDiffPreview({
  codePreviewSettings,
  lines,
  renderLineContent,
  className,
  style,
  ...rest
}: LightweightDiffPreviewProps): ReactNode {
  const { fontSizePx, showLineNumbers, wrapLongLines } = codePreviewSettings;
  const containerStyle = useMemo<CSSProperties>(
    () => ({ ...style, ...(fontSizePx ? { fontSize: `${fontSizePx}px` } : {}) }),
    [style, fontSizePx],
  );
  return (
    <div
      {...rest}
      data-slot="lightweight-diff-preview"
      className={[
        "min-w-0 font-mono text-ui-sm",
        wrapLongLines ? "overflow-x-hidden" : "overflow-x-auto",
        className,
      ]
        .filter(Boolean)
        .join(" ")}
      style={containerStyle}
    >
      {lines.map((line, index) => {
        const parts = getLightweightDiffLineParts(line);
        return (
          <div
            key={`${index}-${parts.kind}`}
            data-line-kind={parts.kind}
            className={["flex min-w-0 items-start gap-2 px-3 py-px", ROW_BY_KIND[parts.kind]].join(
              " ",
            )}
          >
            {showLineNumbers ? (
              <span
                aria-hidden="true"
                className="w-8 shrink-0 select-none pe-2 text-end text-foreground-subtlest tabular-nums"
              >
                {index + 1}
              </span>
            ) : null}
            <span aria-hidden="true" className="w-2 shrink-0 select-none opacity-70">
              {parts.marker}
            </span>
            <span
              className={[
                "min-w-0 flex-1",
                wrapLongLines ? "whitespace-pre-wrap break-words" : "whitespace-pre",
              ].join(" ")}
            >
              {renderLineContent ? renderLineContent(parts, index) : parts.content}
            </span>
          </div>
        );
      })}
    </div>
  );
}
