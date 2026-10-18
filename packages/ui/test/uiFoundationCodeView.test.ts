/**
 * M4：只读 CodeView 的行模型（spec: docs/specs/m4-ui-foundation.md §2.20）。
 * 运行：pnpm --filter @mode/ui test
 */
import assert from "node:assert/strict";
import test from "node:test";

import {
  buildCodeViewLineModels,
  formatCodeViewLineRange,
  groupCodeViewCommentsByEndLine,
} from "../src/ui-foundation/codeViewLines.js";

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
