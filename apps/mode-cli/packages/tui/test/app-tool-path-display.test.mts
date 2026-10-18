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

test("工具路径：Windows 分隔符归一化为反斜杠并折叠重复分隔符", () => {
  assert.equal(formatToolFilePath("C:\\proj\\src\\a.ts"), "C:\\proj\\src\\a.ts");
  assert.equal(formatToolFilePath("C:\\\\proj\\\\src\\\\a.ts"), "C:\\proj\\src\\a.ts");
  assert.equal(formatToolFilePath("C:/proj/src/a.ts"), "C:\\proj\\src\\a.ts");
  assert.equal(formatToolFilePath("\\\\server\\share\\a.ts"), "\\\\server\\share\\a.ts");
});

test("工具路径：Windows cwd 前缀剔除（容忍尾部/混合分隔符与盘符大小写）", () => {
  assert.equal(formatToolFilePath("C:\\proj\\src\\a.ts", "C:\\proj"), "src\\a.ts");
  assert.equal(formatToolFilePath("C:\\proj\\src\\a.ts", "C:\\proj\\"), "src\\a.ts");
  assert.equal(formatToolFilePath("C:\\proj\\src\\a.ts", "C:/proj"), "src\\a.ts");
  assert.equal(formatToolFilePath("C:/proj/src/a.ts", "C:\\proj/src"), "a.ts");
  assert.equal(formatToolFilePath("C:\\proj\\a.ts", "c:\\proj"), "a.ts");
});

test("工具路径：cwd 外的 Windows 路径仍归一化但不转相对", () => {
  assert.equal(formatToolFilePath("C:/proj/src/a.ts", "/other"), "C:\\proj\\src\\a.ts");
  assert.equal(
    formatToolFilePath("\\\\server\\share\\dir\\a.ts", "\\\\server\\share"),
    "dir\\a.ts",
  );
});
