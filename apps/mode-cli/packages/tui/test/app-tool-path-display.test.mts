import assert from "node:assert/strict";
import test from "node:test";

import { formatToolFilePath } from "../src/app-tool-path-display.js";

test("工具路径：单参数原样返回，空路径无展示值", () => {
  assert.equal(formatToolFilePath("/proj/src/a.ts"), "/proj/src/a.ts");
  assert.equal(formatToolFilePath(""), undefined);
});

test("工具路径：cwd 内的绝对路径转为相对路径（容忍尾部斜杠）", () => {
  assert.equal(formatToolFilePath("/proj/src/a.ts", "/proj"), "src/a.ts");
  assert.equal(formatToolFilePath("/proj/src/a.ts", "/proj/"), "src/a.ts");
});

test("工具路径：cwd 之外或本身是相对路径时保持原样", () => {
  assert.equal(formatToolFilePath("/proj/src/a.ts", "/other"), "/proj/src/a.ts");
  assert.equal(formatToolFilePath("src/a.ts", "/proj"), "src/a.ts");
});
