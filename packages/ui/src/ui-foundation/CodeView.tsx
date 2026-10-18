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

import {
  highlightCode,
  shouldUseSyntaxHighlighting,
  type TokenizedCode,
} from "@/lib/shikiHighlighter.js";
import { buildCodeViewLineModels, type CodeViewRange } from "./codeViewLines.js";

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

  useEffect(() => {
    let cancelled = false;
    if (
      !enableSyntaxHighlighting ||
      !code ||
      code.length > MAX_HIGHLIGHT_CHARS ||
      !shouldUseSyntaxHighlighting(language)
    ) {
      setTokens(null);
      return;
    }
    highlightCode(code, language, theme, (result) => {
      if (!cancelled) {
        setTokens(result);
      }
    });
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
    </div>
  );
}
