import assert from "node:assert/strict";
import test from "node:test";

import { formatFileDiffDisplay } from "../src/app-tool-diff-display.js";

test("diff 展示：空输入给空行集与未截断标记", () => {
  assert.deepEqual(formatFileDiffDisplay({}), {
    diff: undefined,
    filePath: undefined,
    lines: [],
    structuredPatch: [],
    truncated: false,
  });
});

test("diff 展示：沿用 filePath，并把结构化 hunk 合成为 diff 文本", () => {
  assert.deepEqual(
    formatFileDiffDisplay({
      filePath: "a.ts",
      structuredPatch: [{ oldStart: 3, newStart: 4, lines: ["-x", "+y", " z"] }],
    }),
    {
      diff: "--- a.ts\n+++ a.ts\n@@ -3,0 +4,0 @@\n-x\n+y\n z",
      filePath: "a.ts",
      lines: [],
      structuredPatch: [],
      truncated: false,
    },
  );
});

test("diff 展示：缺 filePath 时用 file 作为头部占位", () => {
  const result = formatFileDiffDisplay({ structuredPatch: [{ lines: ["+x"] }] });
  assert.equal(result.diff, "--- file\n+++ file\n@@ -0,0 +0,0 @@\n+x");
});

test("diff 展示：truncated 为真时生成 meta 提示行", () => {
  assert.deepEqual(formatFileDiffDisplay({ filePath: "a.ts", truncated: true }), {
    diff: undefined,
    filePath: "a.ts",
    lines: [{ text: "... diff truncated", tone: "meta" }],
    structuredPatch: [],
    truncated: true,
  });
});
