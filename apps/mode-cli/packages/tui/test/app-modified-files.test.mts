import assert from "node:assert/strict";
import test from "node:test";

import { applyModifiedFileStats } from "../src/app-modified-files.js";

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
