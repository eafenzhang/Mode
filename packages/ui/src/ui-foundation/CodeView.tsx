// P1.1 CodeView（spec: docs/specs/m4-ui-foundation.md §2.20）。
// 只读代码块：行号、markedLines、focusedRange 与异步 Shiki token。

import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type HTMLAttributes,
  type Ref,
} from "react";
import type { BundledTheme } from "shiki";

import { Button } from "./Button.js";

import type { CodeCommentPreview } from "@/lib/codeCommentContext.js";
import type { CodeCommentLabels } from "./codeViewLabels.js";
import type { TokenizedCode } from "@/lib/shikiHighlighter.js";
import {
  buildCodeViewLineModels,
  formatCodeViewLineRange,
  groupCodeViewCommentsByEndLine,
  type CodeViewRange,
} from "./codeViewLines.js";
import {
  getCodeViewSelectedText,
  isCodeViewLineSelected,
  normalizeCodeViewSelection,
  type CodeViewSelection,
} from "./codeViewSelection.js";
import { cn } from "@/components/lib/utils.js";

const MAX_HIGHLIGHT_CHARS = 200_000;

export type { CodeCommentLabels };

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
  enableLineSelection?: boolean;
  enableGutterUtility?: boolean;
  scrollContainerRef?: Ref<HTMLDivElement>;
  onSubmitCodeComment?: (params: {
    range: { startLine: number; endLine: number };
    selectedText: string;
    comment: string;
  }) => void;
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
  enableLineSelection = false,
  enableGutterUtility = false,
  scrollContainerRef,
  onSubmitCodeComment,
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
  const [selection, setSelection] = useState<CodeViewSelection | null>(null);
  const [draft, setDraft] = useState<{ selection: CodeViewSelection; comment: string } | null>(
    null,
  );
  const [dragging, setDragging] = useState(false);
  const anchorRef = useRef<number | null>(null);

  useEffect(() => {
    if (!dragging) {
      return;
    }
    const stop = (): void => setDragging(false);
    window.addEventListener("pointerup", stop);
    return () => window.removeEventListener("pointerup", stop);
  }, [dragging]);

  const assignContainerRef = (node: HTMLDivElement | null): void => {
    containerRef.current = node;
    if (typeof scrollContainerRef === "function") {
      scrollContainerRef(node);
    } else if (scrollContainerRef) {
      (scrollContainerRef as { current: HTMLDivElement | null }).current = node;
    }
  };

  const beginSelection = (lineNumber: number): void => {
    anchorRef.current = lineNumber;
    setSelection(normalizeCodeViewSelection(lineNumber, lineNumber));
    setDraft(null);
    setDragging(true);
  };

  const extendSelection = (lineNumber: number): void => {
    if (!dragging || anchorRef.current === null) {
      return;
    }
    setSelection(normalizeCodeViewSelection(anchorRef.current, lineNumber));
  };

  const openDraftForLine = (lineNumber: number): void => {
    const lineSelection = normalizeCodeViewSelection(lineNumber, lineNumber);
    setSelection(lineSelection);
    setDraft({ selection: lineSelection, comment: "" });
  };

  const commitDraft = (): void => {
    if (!draft || !onSubmitCodeComment) {
      return;
    }
    const comment = draft.comment.trim();
    if (!comment) {
      return;
    }
    onSubmitCodeComment({
      range: draft.selection,
      selectedText: getCodeViewSelectedText(lines, draft.selection, firstLineNumber),
      comment,
    });
    setDraft(null);
    setSelection(null);
  };

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
      ref={assignContainerRef}
      data-slot="code-view"
      className={cn([
        "group/code-view min-w-0 font-mono text-ui-sm",
        wrapLongLines ? "overflow-x-hidden" : "overflow-x-auto",
        className,
      ])}
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
            data-selected={isCodeViewLineSelected(selection, model.number) ? "true" : undefined}
            onPointerDown={
              enableLineSelection
                ? (event) => {
                    if (event.button !== 0) {
                      return;
                    }
                    event.preventDefault();
                    beginSelection(model.number);
                  }
                : undefined
            }
            onPointerEnter={enableLineSelection ? () => extendSelection(model.number) : undefined}
            className={cn([
              "flex min-w-0 items-start gap-2 px-3 py-px",
              model.focused || isCodeViewLineSelected(selection, model.number) ? "bg-selected" : "",
            ])}
          >
            {enableGutterUtility ? (
              <button
                type="button"
                aria-label={labels?.addComment ?? "Add comment"}
                data-slot="code-view-gutter-add"
                className="me-1 hidden w-4 shrink-0 text-ui-xs text-foreground-subtlest hover:text-foreground group-hover/code-view:inline"
                onClick={() => openDraftForLine(model.number)}
              >
                +
              </button>
            ) : null}
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
      {draft ? (
        <div
          data-slot="code-view-composer"
          className="mx-3 my-1 rounded-md border border-border bg-surface p-2"
        >
          <div className="mb-1 text-ui-xs text-foreground-subtlest">
            {formatCodeViewLineRange(draft.selection)}
          </div>
          <textarea
            autoFocus
            rows={2}
            value={draft.comment}
            placeholder={labels?.commentPlaceholder ?? "Comment"}
            className="w-full resize-none rounded-sm border border-input-border bg-input px-2 py-1 text-ui-base text-foreground outline-none focus-visible:border-input-border-focused"
            onChange={(event) =>
              setDraft((current) =>
                current ? { ...current, comment: event.target.value } : current,
              )
            }
          />
          <div className="mt-1 flex justify-end gap-2">
            <Button
              size="xs"
              variant="ghost"
              onClick={() => {
                setDraft(null);
                setSelection(null);
              }}
            >
              {labels?.cancelComment ?? "Cancel"}
            </Button>
            <Button size="xs" onClick={commitDraft} disabled={!draft.comment.trim()}>
              {labels?.submitComment ?? "Comment"}
            </Button>
          </div>
        </div>
      ) : selection && enableLineSelection ? (
        <div className="px-3 py-1">
          <Button size="xs" variant="ghost" onClick={() => setDraft({ selection, comment: "" })}>
            {labels?.addComment ?? "Add comment"}
          </Button>
        </div>
      ) : null}
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
