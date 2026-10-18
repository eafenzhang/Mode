/**
 * M4 UI 基础层纯函数（spec: docs/specs/m4-ui-foundation.md §5 验收 1/2）。
 * 运行：pnpm --filter @mode/ui test
 */
import assert from "node:assert/strict";
import test from "node:test";

import {
  findUiTypographyViolations,
  resolveControlRadius,
  resolveNestedRadius,
} from "../src/lib/uiFoundation.js";

test("控件半径：无父/xl 及以上 → lg，lg → md，md/sm → sm", () => {
  assert.equal(resolveControlRadius("none"), "lg");
  assert.equal(resolveControlRadius("xl"), "lg");
  assert.equal(resolveControlRadius("2xl"), "lg");
  assert.equal(resolveControlRadius("lg"), "md");
  assert.equal(resolveControlRadius("md"), "sm");
  assert.equal(resolveControlRadius("sm"), "sm");
});

test("嵌套容器：首层 xl，之后 lg→md→sm，Dialog shell 重置到 xl", () => {
  assert.equal(resolveNestedRadius("none"), "xl");
  assert.equal(resolveNestedRadius("2xl"), "xl");
  assert.equal(resolveNestedRadius("xl"), "lg");
  assert.equal(resolveNestedRadius("lg"), "md");
  assert.equal(resolveNestedRadius("md"), "sm");
  assert.equal(resolveNestedRadius("sm"), "sm");
});

test("排版守卫：内置 text-sm/base/xs 与任意字号违规，text-ui-* 与颜色类合法", () => {
  assert.deepEqual(
    findUiTypographyViolations([
      "flex text-ui-base text-foreground",
      "text-ui-sm text-foreground-subtle",
      "text-ui-2xs",
    ]),
    [],
  );
  assert.deepEqual(
    findUiTypographyViolations(["text-sm", "text-base", "text-xs", "text-[13px]", "text-[0.9rem]"]),
    ["text-sm", "text-base", "text-xs", "text-[13px]", "text-[0.9rem]"],
  );
  assert.deepEqual(findUiTypographyViolations(["text-ui-huge"]), ["text-ui-huge"]);
  assert.deepEqual(findUiTypographyViolations(["text-[var(--x)]"]), []);
});
