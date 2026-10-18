/**
 * M4 P1.2 面板尺寸换算纯逻辑（spec: docs/specs/m4-ui-foundation.md §3）。
 * 运行：pnpm --filter @mode/ui test
 */
import assert from "node:assert/strict";
import test from "node:test";

import {
  clampPanelSize,
  isResizeKey,
  normalizeBounds,
  resizeFromKeyboard,
  resizeFromPointer,
  RESIZE_KEYBOARD_STEP,
} from "../src/ui-foundation/resizeMath.js";

const bounds = { min: 200, max: 480 };

test("clamp：范围内原样，越界贴边，非有限值回最小值", () => {
  assert.equal(clampPanelSize(300, bounds), 300);
  assert.equal(clampPanelSize(100, bounds), 200);
  assert.equal(clampPanelSize(999, bounds), 480);
  assert.equal(clampPanelSize(Number.NaN, bounds), 200);
});

test("bounds 归一：负数最小值归零，max < min 时取 min", () => {
  assert.deepEqual(normalizeBounds({ min: -20, max: 100 }), { min: 0, max: 100 });
  assert.deepEqual(normalizeBounds({ min: 300, max: 100 }), { min: 300, max: 300 });
});

test("指针拖拽从起始尺寸累加 delta 并 clamp", () => {
  assert.equal(resizeFromPointer(300, 40, bounds), 340);
  assert.equal(resizeFromPointer(300, -500, bounds), 200);
  assert.equal(resizeFromPointer(300, 500, bounds), 480);
});

test("键盘步进：方向键 ±16px，Home/End 到边界，其它键不处理", () => {
  assert.equal(resizeFromKeyboard(300, "ArrowRight", bounds), 300 + RESIZE_KEYBOARD_STEP);
  assert.equal(resizeFromKeyboard(300, "ArrowLeft", bounds), 300 - RESIZE_KEYBOARD_STEP);
  assert.equal(resizeFromKeyboard(210, "ArrowLeft", bounds), 200);
  assert.equal(resizeFromKeyboard(300, "Home", bounds), 200);
  assert.equal(resizeFromKeyboard(300, "End", bounds), 480);
  assert.equal(resizeFromKeyboard(300, "Enter", bounds), null);
  assert.equal(resizeFromKeyboard(300, "ArrowRight", bounds, 64), 364);
});

test("只把方向键与 Home/End 视为调整键", () => {
  for (const key of ["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End"]) {
    assert.equal(isResizeKey(key), true);
  }
  assert.equal(isResizeKey("PageUp"), false);
});
