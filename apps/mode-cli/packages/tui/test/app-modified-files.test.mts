import assert from "node:assert/strict";
import test from "node:test";

import {
  applyModifiedFileStats,
  modifiedFileStatsFromToolResultPayload,
} from "../src/app-modified-files.js";

test("修改文件统计：同路径累加并移到最前，新路径前置", () => {
  const first = applyModifiedFileStats([], [{ filePath: "a", additions: 1, deletions: 2 }]);
  assert.deepEqual(first, [{ additions: 1, deletions: 2, filePath: "a" }]);

  const merged = applyModifiedFileStats(first, [
    { filePath: "a", additions: 3, deletions: 1 },
    { filePath: "b", additions: 0, deletions: 0 },
  ]);
  assert.deepEqual(merged, [
    { additions: 0, deletions: 0, filePath: "b" },
    { additions: 4, deletions: 3, filePath: "a" },
  ]);
});

test("修改文件统计：最多保留最近 100 个路径", () => {
  const many = Array.from({ length: 105 }, (_, index) => ({
    filePath: `f${index}`,
    additions: 1,
    deletions: 0,
  }));
  const capped = applyModifiedFileStats([], many);
  assert.equal(capped.length, 100);
  assert.equal(capped[0]?.filePath, "f104");
  assert.equal(capped[capped.length - 1]?.filePath, "f5");
});

test("工具结果提取：单文件 diff 按 workspaceDirectory 转相对路径", () => {
  const stats = modifiedFileStatsFromToolResultPayload(
    {
      result: {
        display: { kind: "file_diff", filePath: "/w/a.ts", additions: 2, deletions: 1 },
      },
    },
    "/w",
  );
  assert.deepEqual(stats, [{ additions: 2, deletions: 1, filePath: "a.ts" }]);

  const noWorkspace = modifiedFileStatsFromToolResultPayload(
    {
      result: {
        display: { kind: "file_diff", filePath: "/w/a.ts", additions: 1, deletions: 1 },
      },
    },
    undefined,
  );
  assert.deepEqual(noWorkspace, [{ additions: 1, deletions: 1, filePath: "/w/a.ts" }]);
});

test("工具结果提取：多文件 diff 丢弃空路径并把负数/小数归一", () => {
  const stats = modifiedFileStatsFromToolResultPayload(
    {
      result: {
        display: {
          kind: "file_diffs",
          files: [
            { filePath: "/w/a.ts", additions: 1, deletions: 2 },
            { filePath: "", additions: 9, deletions: 9 },
            { filePath: "/w/b.ts", additions: -3, deletions: 1.7 },
          ],
        },
      },
    },
    "/w",
  );
  assert.deepEqual(stats, [
    { additions: 1, deletions: 2, filePath: "a.ts" },
    { additions: 0, deletions: 1, filePath: "b.ts" },
  ]);
});

test("工具结果提取：未知 kind 或缺 display 返回空数组", () => {
  assert.deepEqual(
    modifiedFileStatsFromToolResultPayload(
      { result: { display: { kind: "other", filePath: "/w/a.ts" } } },
      "/w",
    ),
    [],
  );
  assert.deepEqual(modifiedFileStatsFromToolResultPayload({}, "/w"), []);
});
