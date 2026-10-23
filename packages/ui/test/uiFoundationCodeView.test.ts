/**
 * M4：只读 CodeView 的行模型（spec: docs/specs/m4-ui-foundation.md §2.20）。
 * 运行：pnpm --filter @mode/ui test
 */
import assert from "node:assert/strict";
import test from "node:test";

import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";

import { CodeView } from "../src/ui-foundation/CodeView.js";
import {
  buildCodeViewLineModels,
  formatCodeViewLineRange,
  groupCodeViewCommentsByEndLine,
} from "../src/ui-foundation/codeViewLines.js";
import {
  getCodeViewSelectedText,
  isCodeViewLineSelected,
  normalizeCodeViewSelection,
} from "../src/ui-foundation/codeViewSelection.js";

test("CodeView 行模型：行号从 firstLineNumber 起", () => {
  const models = buildCodeViewLineModels({ lineCount: 3, firstLineNumber: 10 });
  assert.deepEqual(
    models.map((line) => line.number),
    [10, 11, 12],
  );
});

test("CodeView 行模型：markedLines 只标指定行，focusedRange 按闭区间", () => {
  const models = buildCodeViewLineModels({
    lineCount: 5,
    firstLineNumber: 1,
    markedLines: [2, 4],
    focusedRange: { startLine: 3, endLine: 4 },
  });
  assert.deepEqual(
    models.map((line) => line.marked),
    [false, true, false, true, false],
  );
  assert.deepEqual(
    models.map((line) => line.focused),
    [false, false, true, true, false],
  );
});

test("CodeView 行模型：空代码与负行数安全", () => {
  assert.deepEqual(buildCodeViewLineModels({ lineCount: 0, firstLineNumber: 1 }), []);
  assert.deepEqual(buildCodeViewLineModels({ lineCount: -3, firstLineNumber: 1 }), []);
});

test("注释归档：按 endLine 分组且保持输入顺序", () => {
  const grouped = groupCodeViewCommentsByEndLine([
    { id: "a", endLine: 4 },
    { id: "b", endLine: 2 },
    { id: "c", endLine: 4 },
  ]);
  assert.deepEqual(
    grouped.get(4)?.map((item) => item.id),
    ["a", "c"],
  );
  assert.deepEqual(
    grouped.get(2)?.map((item) => item.id),
    ["b"],
  );
  assert.equal(grouped.get(9), undefined);
});

test("范围文案：单行 L<n>，跨行 L<n>–<m>，反向输入自动归一", () => {
  assert.equal(formatCodeViewLineRange({ startLine: 3, endLine: 3 }), "L3");
  assert.equal(formatCodeViewLineRange({ startLine: 3, endLine: 5 }), "L3–5");
  assert.equal(formatCodeViewLineRange({ startLine: 5, endLine: 3 }), "L3–5");
});

const COMMENT = {
  id: "c1",
  startLine: 2,
  endLine: 3,
  sourceTitle: "a.ts",
  selectedText: "const a = 1;",
  comment: "rename this",
};

test("CodeView 第二层：topComment 范围/提示与行后注释卡", () => {
  const html = renderToStaticMarkup(
    createElement(CodeView, {
      code: ["line1", "line2", "line3"].join(String.fromCharCode(10)),
      language: "typescript",
      enableSyntaxHighlighting: false,
      topComment: { ...COMMENT, startLine: 2, endLine: 4 },
      topCommentShowRange: true,
      topCommentNotice: "1 comment",
      comments: [COMMENT],
    }),
  );
  assert.match(html, /data-slot="code-view-top-comment"/);
  assert.match(html, /L2–4/);
  assert.match(html, /1 comment/);
  assert.match(html, /data-slot="code-view-comments"/);
  assert.match(html, /data-comment-id="c1"/);
  assert.match(html, /L2–3/);
  assert.match(html, /rename this/);
});

test("滚动容器：fillContainer 时根节点自带高度约束与纵向 overflow，长文件不被父级裁掉", () => {
  const html = renderToStaticMarkup(
    createElement(CodeView, {
      code: "line1",
      language: "typescript",
      enableSyntaxHighlighting: false,
      fillContainer: true,
    }),
  );
  const rootTag = html.slice(0, html.indexOf(">") + 1);
  assert.match(rootTag, /\bh-full\b/);
  assert.match(rootTag, /\boverflow-y-auto\b/);
});

test("聊天代码块不加高度约束：父级自适应，避免 100% 参照被内容撑高后截断", () => {
  const html = renderToStaticMarkup(
    createElement(CodeView, {
      code: "line1",
      language: "typescript",
      enableSyntaxHighlighting: false,
    }),
  );
  const rootTag = html.slice(0, html.indexOf(">") + 1);
  assert.doesNotMatch(rootTag, /\bh-full\b/);
});

test("行选拦截只落在行号槽位，代码区不再 preventDefault 复制", () => {
  const html = renderToStaticMarkup(
    createElement(CodeView, {
      code: "line1",
      language: "typescript",
      enableSyntaxHighlighting: false,
      enableLineSelection: true,
    }),
  );
  // 行选从行号槽位发起；行本身不再挂 onPointerDown，否则点代码区会 preventDefault，
  // 原生文本选区被杀掉，右侧栏无法复制。
  assert.match(html, /data-line-number-gutter="1"/);
  assert.doesNotMatch(html, /data-line-number="1"[^>]*onpointerdown/);
});

test("选区：反向拖拽归一，选中判断按闭区间", () => {
  const range = normalizeCodeViewSelection(5, 3);
  assert.deepEqual(range, { startLine: 3, endLine: 5 });
  assert.equal(isCodeViewLineSelected(range, 2), false);
  assert.equal(isCodeViewLineSelected(range, 3), true);
  assert.equal(isCodeViewLineSelected(range, 5), true);
  assert.equal(isCodeViewLineSelected(range, 6), false);
  assert.equal(isCodeViewLineSelected(null, 3), false);
});

test("选区文本：按 firstLineNumber 切片并夹取边界", () => {
  const lines = ["a", "b", "c", "d"];
  assert.equal(
    getCodeViewSelectedText(lines, { startLine: 11, endLine: 12 }, 10),
    ["b", "c"].join(String.fromCharCode(10)),
  );
  assert.equal(
    getCodeViewSelectedText(lines, { startLine: 10, endLine: 13 }, 10),
    ["a", "b", "c", "d"].join(String.fromCharCode(10)),
  );
  assert.equal(getCodeViewSelectedText(lines, null, 10), "");
});

test("空行：渲染换行文本节点，复制时不丢空行", () => {
  const html = renderToStaticMarkup(
    createElement(CodeView, {
      code: ["a", "", "b"].join(String.fromCharCode(10)),
      language: "text",
      enableSyntaxHighlighting: false,
    }),
  );
  // 浏览器复制会跳过没有任何文本的块；空行必须渲染成 "\n" 才能保留（原版 @pierre/diffs 行为）。
  assert.equal((html.match(/\n/g) ?? []).length, 1);
  assert.match(html, /<span class="[^"]*whitespace-pre">\n<\/span>/);
});

test("gutter：启用后每行有 add 按钮", () => {
  const html = renderToStaticMarkup(
    createElement(CodeView, {
      code: "x",
      language: "text",
      enableSyntaxHighlighting: false,
      enableGutterUtility: true,
      labels: { addComment: "Add comment" },
    }),
  );
  assert.match(html, /data-slot="code-view-gutter-add"/);
  assert.match(html, /aria-label="Add comment"/);
});
