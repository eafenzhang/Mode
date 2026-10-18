/**
 * M4 P1.1 Tabs 键盘导航纯逻辑（spec: docs/specs/m4-ui-foundation.md §2.5）。
 * 运行：pnpm --filter @mode/ui test
 */
import assert from "node:assert/strict";
import test from "node:test";

import { isTabNavigationKey, nextTabIndex } from "../src/ui-foundation/tabsKeyboard.js";

test("Arrow 左右循环，Home/End 到首尾", () => {
  assert.equal(nextTabIndex(1, 3, "ArrowLeft"), 0);
  assert.equal(nextTabIndex(0, 3, "ArrowLeft"), 2);
  assert.equal(nextTabIndex(2, 3, "ArrowRight"), 0);
  assert.equal(nextTabIndex(1, 3, "ArrowRight"), 2);
  assert.equal(nextTabIndex(1, 3, "Home"), 0);
  assert.equal(nextTabIndex(1, 3, "End"), 2);
});

test("越界下标按第一个处理；空 tablist 返回 null", () => {
  assert.equal(nextTabIndex(-1, 3, "ArrowRight"), 1);
  assert.equal(nextTabIndex(9, 3, "ArrowLeft"), 2);
  assert.equal(nextTabIndex(0, 0, "End"), null);
});

test("只把四个导航键视为 Tabs 导航键", () => {
  for (const key of ["ArrowLeft", "ArrowRight", "Home", "End"]) {
    assert.equal(isTabNavigationKey(key), true);
  }
  for (const key of ["ArrowUp", "ArrowDown", "Enter", "a"]) {
    assert.equal(isTabNavigationKey(key), false);
  }
});
