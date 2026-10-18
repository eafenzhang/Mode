/**
 * M4 P1.1：lightweight diff 行契约与渲染（spec: docs/specs/m4-ui-foundation.md §2.16）。
 * 运行：pnpm --filter @mode/ui test
 */
import assert from "node:assert/strict";
import test from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";

import {
  getLightweightDiffLineParts,
  LightweightDiffPreview,
} from "../src/ui-foundation/adapters/lightweight-diff-preview.js";
import {
  buildHighlightedLightweightDiffCode,
  getHighlightedLightweightDiffLine,
} from "../src/ui-foundation/lightweightDiffTokens.js";

test("行分类：added/removed/context 的 marker 与 content", () => {
  assert.deepEqual(getLightweightDiffLineParts("+next"), {
    kind: "added",
    marker: "+",
    content: "next",
    raw: "+next",
  });
  assert.deepEqual(getLightweightDiffLineParts("-prev"), {
    kind: "removed",
    marker: "-",
    content: "prev",
    raw: "-prev",
  });
  assert.deepEqual(getLightweightDiffLineParts(" same"), {
    kind: "context",
    marker: " ",
    content: "same",
    raw: " same",
  });
  assert.deepEqual(getLightweightDiffLineParts("@@ -1 +1 @@"), {
    kind: "context",
    marker: "",
    content: "@@ -1 +1 @@",
    raw: "@@ -1 +1 @@",
  });
});

test("高亮代码拼接：去掉行前缀后按行 join", () => {
  const lines = ["+a", "-b", " c"];
  assert.equal(buildHighlightedLightweightDiffCode(lines), "a\nb\nc");
  assert.deepEqual(getHighlightedLightweightDiffLine("+a"), { code: "a", marker: "+" });
});

test("渲染：行号、标记、语义色与换行策略", () => {
  const html = renderToStaticMarkup(
    createElement(LightweightDiffPreview, {
      codePreviewSettings: { fontSizePx: 12, showLineNumbers: true, wrapLongLines: false },
      lines: ["+added", "-removed", " context"],
    }),
  );
  assert.match(html, /data-slot="lightweight-diff-preview"/);
  assert.match(html, /font-size:12px/);
  assert.match(html, /data-line-kind="added"/);
  assert.match(html, /data-line-kind="removed"/);
  assert.match(html, /bg-diff-added/);
  assert.match(html, /bg-diff-removed/);
  assert.match(html, /whitespace-pre/);
  assert.match(html, />1</);
  assert.match(html, />3</);
});

test("渲染：wrapLongLines 使用 break-words", () => {
  const html = renderToStaticMarkup(
    createElement(LightweightDiffPreview, {
      codePreviewSettings: { fontSizePx: 13, showLineNumbers: false, wrapLongLines: true },
      lines: ["+long"],
    }),
  );
  assert.match(html, /whitespace-pre-wrap break-words/);
  assert.doesNotMatch(html, />1</);
});
