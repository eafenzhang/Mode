/**
 * M4：PPTX viewer 第一层纯布局/控制逻辑（spec §2.21）。
 * 运行：pnpm --filter @mode/ui test
 */
import assert from "node:assert/strict";
import test from "node:test";

import {
  PPTX_MAX_ZOOM_PERCENT,
  PPTX_MIN_ZOOM_PERCENT,
  resolvePptxPageDisplaySize,
  resolvePptxPageFromInput,
  stepPptxZoom,
} from "../src/ui-foundation/pptxViewerLayout.js";

test("页面显示尺寸：按缩放百分比放大并向上取整", () => {
  assert.deepEqual(resolvePptxPageDisplaySize({ width: 960, height: 540 }, 100), {
    width: 960,
    height: 540,
  });
  assert.deepEqual(resolvePptxPageDisplaySize({ width: 960, height: 540 }, 125), {
    width: 1200,
    height: 675,
  });
  // 25% 下限夹取
  assert.deepEqual(resolvePptxPageDisplaySize({ width: 100, height: 50 }, 1), {
    width: 25,
    height: 13,
  });
});

test("缩放步进：25% 步进并夹在 25%–300%", () => {
  assert.equal(stepPptxZoom(100, "in"), 125);
  assert.equal(stepPptxZoom(100, "out"), 75);
  assert.equal(stepPptxZoom(PPTX_MAX_ZOOM_PERCENT, "in"), PPTX_MAX_ZOOM_PERCENT);
  assert.equal(stepPptxZoom(PPTX_MIN_ZOOM_PERCENT, "out"), PPTX_MIN_ZOOM_PERCENT);
});

test("页码输入：非法回退当前页，合法夹在 1..pageCount", () => {
  assert.equal(resolvePptxPageFromInput("abc", 5, 3), 3);
  assert.equal(resolvePptxPageFromInput("2", 5, 3), 2);
  assert.equal(resolvePptxPageFromInput("0", 5, 3), 1);
  assert.equal(resolvePptxPageFromInput("99", 5, 3), 5);
});
