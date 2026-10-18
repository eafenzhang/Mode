/**
 * M4：PPTX viewer 的纯控制逻辑（页码/缩放/命中测试），为后续 adapter 替换钉住行为。
 * 运行：pnpm --filter @mode/ui test
 */
import assert from "node:assert/strict";
import test from "node:test";

import { hitTestPresentationElement } from "../src/presentation/presentationElementModel.js";
import {
  PPTX_MAX_ZOOM_PERCENT,
  PPTX_MIN_ZOOM_PERCENT,
  clampPresentationPageNumber,
  clampPresentationZoomPercent,
} from "../src/presentation/presentationPreviewControls.js";
import type { PresentationPageElement } from "../src/presentation/types.js";

function element(
  overrides: Partial<PresentationPageElement> & Pick<PresentationPageElement, "nodeId">,
): PresentationPageElement {
  return {
    slideIndex: 0,
    slidePart: "ppt/slides/slide1.xml",
    nodeName: overrides.nodeId,
    nodeType: "shape",
    bounds: { x: 0, y: 0, width: 100, height: 100 },
    zIndex: 1,
    ...overrides,
  };
}

test("页码夹取：非法值回 1，越界夹到页数", () => {
  assert.equal(clampPresentationPageNumber(Number.NaN, 5), 1);
  assert.equal(clampPresentationPageNumber(0, 5), 1);
  assert.equal(clampPresentationPageNumber(3.6, 5), 4);
  assert.equal(clampPresentationPageNumber(9, 5), 5);
  assert.equal(clampPresentationPageNumber(2, 0), 1);
});

test("缩放夹取：25%–300%，非法值回 100%", () => {
  assert.equal(clampPresentationZoomPercent(Number.NaN), 100);
  assert.equal(clampPresentationZoomPercent(1), PPTX_MIN_ZOOM_PERCENT);
  assert.equal(clampPresentationZoomPercent(999), PPTX_MAX_ZOOM_PERCENT);
  assert.equal(clampPresentationZoomPercent(126.4), 126);
});

test("命中测试：zIndex 高的优先，表格单元压过普通形状", () => {
  const low = element({ nodeId: "low", zIndex: 1 });
  const high = element({ nodeId: "high", zIndex: 5 });
  assert.equal(hitTestPresentationElement([low, high], { x: 50, y: 50 })?.nodeId, "high");

  const cell = element({
    nodeId: "cell",
    nodeType: "table-cell",
    zIndex: 1,
    bounds: { x: 10, y: 10, width: 30, height: 30 },
  });
  assert.equal(hitTestPresentationElement([high, cell], { x: 20, y: 20 })?.nodeId, "cell");
});

test("命中测试：边界外返回 null", () => {
  const only = element({ nodeId: "only" });
  assert.equal(hitTestPresentationElement([only], { x: 200, y: 200 }), null);
  assert.equal(hitTestPresentationElement([], { x: 1, y: 1 }), null);
});
