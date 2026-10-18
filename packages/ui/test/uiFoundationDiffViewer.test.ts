/**
 * M4 adapter：DiffViewer 主题/样式计算契约（spec: docs/specs/m4-ui-foundation.md §2.15）。
 * 只测纯函数，避免在测试进程加载 @pierre/diffs 的重型运行时。
 * 运行：pnpm --filter @mode/ui test
 */
import assert from "node:assert/strict";
import test from "node:test";

import {
  buildDiffViewerOptions,
  buildDiffViewerStyle,
  diffViewerContainerClassName,
} from "../src/ui-foundation/diffViewerStyles.js";

test("DiffViewer：容器类与 --diffs-* 变量", () => {
  assert.equal(diffViewerContainerClassName("max-h-40"), "min-w-0 overflow-auto max-h-40");
  const style = buildDiffViewerStyle({ fontSizePx: 12 });
  assert.equal(style["--diffs-font-size" as keyof typeof style], "12px");
  assert.equal(style["--diffs-font-family" as keyof typeof style], "var(--font-mono)");
  assert.equal(style["--diffs-bg" as keyof typeof style], "var(--color-background)");
});

test("DiffViewer：明暗主题组合成 options.theme，themeType 透传", () => {
  const options = buildDiffViewerOptions({
    lightTheme: "github-light" as never,
    darkTheme: "github-dark" as never,
    themeType: "dark",
  });
  assert.deepEqual(options?.theme, { light: "github-light", dark: "github-dark" });
  assert.equal(options?.themeType, "dark");
});

test("DiffViewer：未传主题时保持原 options 引用", () => {
  const original = { diffStyle: "unified" } as never;
  assert.equal(buildDiffViewerOptions({ options: original }), original);
});
