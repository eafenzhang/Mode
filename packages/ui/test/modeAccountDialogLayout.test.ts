import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const read = (relativePath: string): string =>
  readFileSync(new URL(relativePath, import.meta.url), "utf8");

// M4 Dialog 适配器只给 shell（圆角/边框/阴影），内边距由调用方提供；
// 漏传 p-* 时标题与内容会直接压在圆角边框上（2026-10-21 Mode 账号弹窗真机截图）。
test("ModeAccountDialog 自带内边距与纵向间距", () => {
  const source = read("../src/ModeAccountDialog.tsx");
  assert.match(
    source,
    /flex max-w-2xl flex-col gap-4 p-4/,
    "账号弹窗 shell 必须有 p-4 + 纵向 gap，避免内容贴着圆角边缘",
  );
});

test("ModeAccountDialog 标题区给右上角关闭按钮留位", () => {
  const source = read("../src/ModeAccountDialog.tsx");
  assert.match(source, /<DialogHeader className="pe-10">/, "header 需预留关闭按钮空间");
});
