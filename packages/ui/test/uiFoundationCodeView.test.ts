/**
 * M4：只读 CodeView 的行模型（spec: docs/specs/m4-ui-foundation.md §2.20）。
 * 运行：pnpm --filter @mode/ui test
 */
import assert from "node:assert/strict";
import test from "node:test";

import { buildCodeViewLineModels } from "../src/ui-foundation/codeViewLines.js";

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
