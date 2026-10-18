// P1.1 高亮版 lightweight diff（spec: docs/specs/m4-ui-foundation.md §2.16）。
// 异步 Shiki token，按行覆盖内容渲染；不改变行级结构。

import type { HTMLAttributes, ReactNode } from "react";
import { useEffect, useMemo, useState } from "react";
import type { BundledTheme } from "shiki";

import type { CodePreviewSettings } from "@/store/index.js";

import { LightweightDiffPreview } from "./LightweightDiffPreview.js";
import {
  buildHighlightedLightweightDiffCode,
  getHighlightedLightweightDiffLine,
} from "./lightweightDiffTokens.js";
import {
  highlightCode,
  shouldUseSyntaxHighlighting,
  type TokenizedCode,
} from "@/lib/shikiHighlighter.js";

const MAX_HIGHLIGHT_CHARS = 120_000;

function useHighlightedTokens({
  code,
  language,
  theme,
}: {
  code: string;
  language: string;
  theme: BundledTheme;
}): TokenizedCode | null {
  const [tokens, setTokens] = useState<TokenizedCode | null>(null);

  useEffect(() => {
    let cancelled = false;
    if (!code || code.length > MAX_HIGHLIGHT_CHARS || !shouldUseSyntaxHighlighting(language)) {
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
  }, [code, language, theme]);

  return tokens;
}

export { buildHighlightedLightweightDiffCode, getHighlightedLightweightDiffLine };

export interface HighlightedLightweightDiffPreviewProps extends Omit<
  HTMLAttributes<HTMLDivElement>,
  "children"
> {
  codePreviewSettings: Pick<
    CodePreviewSettings,
    "fontSizePx" | "showLineNumbers" | "wrapLongLines"
  >;
  language: string;
  lines: readonly string[];
  path?: string;
  theme: BundledTheme;
}

export function HighlightedLightweightDiffPreview({
  codePreviewSettings,
  language,
  lines,
  path: _path,
  theme,
  ...rest
}: HighlightedLightweightDiffPreviewProps): ReactNode {
  const code = useMemo(() => buildHighlightedLightweightDiffCode(lines), [lines]);
  const tokenized = useHighlightedTokens({ code, language, theme });

  return (
    <LightweightDiffPreview
      {...rest}
      data-slot="highlighted-lightweight-diff-preview"
      codePreviewSettings={codePreviewSettings}
      lines={lines}
      renderLineContent={(parts, index) => {
        const lineTokens = tokenized?.tokens[index];
        if (!lineTokens) {
          return parts.content;
        }
        return lineTokens.map((token, tokenIndex) => (
          <span key={tokenIndex} style={{ color: token.color }}>
            {token.content}
          </span>
        ));
      }}
    />
  );
}
