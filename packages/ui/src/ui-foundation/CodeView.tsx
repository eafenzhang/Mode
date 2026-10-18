// P1.1 CodeView（spec: docs/specs/m4-ui-foundation.md §2.20）。
// 只读代码块：行号、markedLines、focusedRange 与异步 Shiki token。

import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type HTMLAttributes,
} from "react";
import type { BundledTheme } from "shiki";

import { Button } from "./Button.js";

import type { CodeCommentPreview } from "@/lib/codeCommentContext.js";
import type { CodeCommentLabels } from "@/components/ui/code-viewer.js";
import type { TokenizedCode } from "@/lib/shikiHighlighter.js";
import {
  buildCodeViewLineModels,
  formatCodeViewLineRange,
  groupCodeViewCommentsByEndLine,
  type CodeViewRange,
} from "./codeViewLines.js";

const MAX_HIGHLIGHT_CHARS = 200_000;

export interface CodeViewProps extends Omit<HTMLAttributes<HTMLDivElement>, "children"> {
  code: string;
  language: string;
  enableSyntaxHighlighting?: boolean;
  showLineNumbers?: boolean;
  theme?: BundledTheme;
  wrapLongLines?: boolean;
  fontSizePx?: number;
  firstLineNumber?: number;
  markedLines?: readonly number[];
  focusedRange?: CodeViewRange | null;
  focusRequestId?: string;
  comments?: readonly CodeCommentPreview[];
  topComment?: CodeCommentPreview | null;
  topCommentShowRange?: boolean;
  topCommentNotice?: string;
  labels?: Partial<CodeCommentLabels>;
  onDeleteCodeComment?: (commentId: string) => void;
}

export function CodeView({
  code,
  language,
  enableSyntaxHighlighting = true,
  showLineNumbers = true,
  theme,
  wrapLongLines = false,
  fontSizePx = 12,
  firstLineNumber = 1,
  markedLines,
  focusedRange,
  focusRequestId,
  comments,
  topComment,
  topCommentShowRange = false,
  topCommentNotice,
  labels,
  onDeleteCodeComment,
  className,
  style,
  ...rest
}: CodeViewProps) {
  const [tokens, setTokens] = useState<TokenizedCode | null>(null);
  const containerRef = useRef<HTMLDivElement | null>(null);
  const lines = useMemo(() => code.split("\n"), [code]);
  const models = useMemo(
    () =>
      buildCodeViewLineModels({
        lineCount: lines.length,
        firstLineNumber,
        markedLines,
        focusedRange,
      }),
    [lines.length, firstLineNumber, markedLines, focusedRange],
  );
  const commentsByLine = useMemo(() => groupCodeViewCommentsByEndLine(comments ?? []), [comments]);

  useEffect(() => {
    let cancelled = false;
    if (!enableSyntaxHighlighting || !code || code.length > MAX_HIGHLIGHT_CHARS) {
      setTokens(null);
      return;
    }
    void import("@/lib/shikiHighlighter.js").then(
      ({ highlightCode, shouldUseSyntaxHighlighting }) => {
        if (cancelled || !shouldUseSyntaxHighlighting(language)) {
          return;
        }
        highlightCode(code, language, theme, (result) => {
          if (!cancelled) {
            setTokens(result);
          }
        });
      },
    );
    return () => {
      cancelled = true;
    };
  }, [code, language, theme, enableSyntaxHighlighting]);

  useEffect(() => {
    if (!focusRequestId) {
      return;
    }
    const focused = containerRef.current?.querySelector<HTMLElement>('[data-focused="true"]');
    focused?.scrollIntoView({ block: "center" });
  }, [focusRequestId, focusedRange]);

  const containerStyle = useMemo<CSSProperties>(
    () => ({ ...style, ...(fontSizePx ? { fontSize: `${fontSizePx}px` } : {}) }),
    [style, fontSizePx],
  );

  return (
    <div
      {...rest}
      ref={containerRef}
      data-slot="code-view"
      className={[
        "min-w-0 font-mono text-ui-sm",
        wrapLongLines ? "overflow-x-hidden" : "overflow-x-auto",
        className,
      ]
        .filter(Boolean)
        .join(" ")}
      style={containerStyle}
    >
      {topComment ? (
        <div
          data-slot="code-view-top-comment"
          className="mb-1 rounded-md border border-border bg-surface px-3 py-1.5 text-ui-sm text-foreground-subtle"
        >
          {topCommentShowRange ? (
            <span className="me-2 font-mono text-ui-xs">
              {formatCodeViewLineRange({
                startLine: topComment.startLine,
                endLine: topComment.endLine,
              })}
            </span>
          ) : null}
          <span className="text-foreground">{topComment.comment}</span>
          {topCommentNotice ? (
            <div className="mt-1 text-ui-xs text-foreground-subtlest">{topCommentNotice}</div>
          ) : null}
        </div>
      ) : null}
      {lines.map((line, index) => {
        const model = models[index] ?? {
          index,
          number: firstLineNumber + index,
          marked: false,
          focused: false,
        };
        const lineTokens = tokens?.tokens[index];
        return (
          <div
            key={model.number}
            data-line-number={model.number}
            data-focused={model.focused ? "true" : undefined}
            className={[
              "flex min-w-0 items-start gap-2 px-3 py-px",
              model.focused ? "bg-selected" : "",
            ]
              .filter(Boolean)
              .join(" ")}
          >
            {showLineNumbers ? (
              <span
                aria-hidden="true"
                className={[
                  "w-8 shrink-0 select-none pe-2 text-end tabular-nums",
                  model.marked ? "text-warning" : "text-foreground-subtlest",
                ].join(" ")}
              >
                {model.number}
              </span>
            ) : null}
            <span
              className={[
                "min-w-0 flex-1",
                wrapLongLines ? "whitespace-pre-wrap break-words" : "whitespace-pre",
              ].join(" ")}
            >
              {lineTokens
                ? lineTokens.map((token, tokenIndex) => (
                    <span key={tokenIndex} style={{ color: token.color }}>
                      {token.content}
                    </span>
                  ))
                : line}
            </span>
          </div>
        );
      })}
      {Array.from(commentsByLine.entries()).map(([endLine, lineComments]) => (
        <div key={`comments-${endLine}`} data-slot="code-view-comments" className="px-3 pb-1">
          {lineComments.map((comment) => (
            <div
              key={comment.id}
              data-comment-id={comment.id}
              className="mt-1 rounded-md border border-border bg-surface px-3 py-1.5 text-ui-sm text-foreground-subtle"
            >
              <div className="flex items-center gap-2">
                <span className="font-mono text-ui-xs">
                  {formatCodeViewLineRange({
                    startLine: comment.startLine,
                    endLine: comment.endLine,
                  })}
                </span>
                <span className="min-w-0 flex-1 truncate text-foreground">{comment.comment}</span>
                {onDeleteCodeComment ? (
                  <Button
                    type="button"
                    size="icon-xs"
                    variant="ghost"
                    aria-label={labels?.deleteComment ?? "Delete comment"}
                    onClick={() => onDeleteCodeComment(comment.id)}
                  >
                    <svg
                      aria-hidden="true"
                      viewBox="0 0 16 16"
                      className="size-3"
                      fill="none"
                      stroke="currentColor"
                      strokeWidth="1.5"
                      strokeLinecap="round"
                    >
                      <path d="M4 4l8 8M12 4l-8 8" />
                    </svg>
                  </Button>
                ) : null}
              </div>
            </div>
          ))}
        </div>
      ))}
    </div>
  );
}
