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
  resolvePptxIntrinsicPoint,
  resolvePptxPageFromInput,
  resolvePptxPdfFileName,
  resolvePptxThumbnailScale,
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

test("缩略图缩放：按目标宽度等比，非法宽度回 1", () => {
  assert.equal(resolvePptxThumbnailScale(960, 96), 0.1);
  assert.equal(resolvePptxThumbnailScale(0, 96), 1);
  assert.equal(resolvePptxThumbnailScale(960, 0), 1);
});

test("坐标换算：客户端点按缩放换算到页面内在坐标", () => {
  assert.deepEqual(
    resolvePptxIntrinsicPoint({
      clientX: 200,
      clientY: 150,
      viewport: { left: 100, top: 50 },
      scale: 2,
    }),
    { x: 50, y: 50 },
  );
  assert.deepEqual(
    resolvePptxIntrinsicPoint({
      clientX: 120,
      clientY: 60,
      viewport: { left: 100, top: 50 },
      scale: 0,
    }),
    { x: 20, y: 10 },
  );
});

test("PDF 文件名：去扩展名/非法字符，缺省 presentation.pdf", () => {
  assert.equal(resolvePptxPdfFileName("deck.pptx"), "deck.pdf");
  assert.equal(resolvePptxPdfFileName("a/b:c.pptx"), "a-b-c.pdf");
  assert.equal(resolvePptxPdfFileName(""), "presentation.pdf");
  assert.equal(resolvePptxPdfFileName(undefined), "presentation.pdf");
});
