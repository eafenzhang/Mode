/**
 * M4 adapter：PdfViewer 输入归一与页码/缩放边界（spec: docs/specs/m4-ui-foundation.md §2.18）。
 * 只测纯函数，避免在测试进程加载 react-pdf/pdfjs。
 * 运行：pnpm --filter @mode/ui test
 */
import assert from "node:assert/strict";
import test from "node:test";

import {
  capturePdfZoomAnchor,
  restorePdfZoomAnchor,
} from "../src/components/ui/usePdfZoomOverlay.js";
import {
  clampPdfPage,
  clampPdfScale,
  isPdfViewerRangeSource,
  mergePdfViewerLabels,
  normalizePdfDocumentSource,
  PDF_MAX_SCALE,
  PDF_MIN_SCALE,
  type PdfViewerRangeSource,
} from "../src/ui-foundation/pdfViewerSource.js";

test("range source 识别：有 requestRange 才算", () => {
  const range: PdfViewerRangeSource = {
    totalBytes: 10,
    requestRange: async () => new Uint8Array(),
  };
  assert.equal(isPdfViewerRangeSource(range), true);
  assert.equal(isPdfViewerRangeSource(new Uint8Array()), false);
  assert.equal(isPdfViewerRangeSource("a.pdf"), false);
});

test("ArrayBuffer/Uint8Array 进入 pdf.js 前复制，调用方可继续复用", () => {
  const buffer = new Uint8Array([1, 2, 3]).buffer;
  const normalized = normalizePdfDocumentSource(buffer) as { data: Uint8Array };
  new Uint8Array(buffer)[0] = 9;
  assert.equal(normalized.data[0], 1);

  const bytes = new Uint8Array([4, 5]);
  const normalizedBytes = normalizePdfDocumentSource(bytes) as { data: Uint8Array };
  bytes[0] = 8;
  assert.equal(normalizedBytes.data[0], 4);
});

test("labels 只覆盖传入键", () => {
  const merged = mergePdfViewerLabels({ zoomIn: "放大" });
  assert.equal(merged.zoomIn, "放大");
  assert.equal(merged.zoomOut, "Zoom out");
});

test("页码与缩放夹取到合法区间", () => {
  assert.equal(clampPdfPage(0, 5), 1);
  assert.equal(clampPdfPage(9, 5), 5);
  assert.equal(clampPdfPage(3, null), 1);
  assert.equal(clampPdfScale(0.1), PDF_MIN_SCALE);
  assert.equal(clampPdfScale(10), PDF_MAX_SCALE);
  assert.equal(clampPdfScale(1.5), 1.5);
});

function fakeElement(rect: { left: number; top: number; width: number; height: number }) {
  return {
    getBoundingClientRect: () => ({
      ...rect,
      right: rect.left + rect.width,
      bottom: rect.top + rect.height,
      x: rect.left,
      y: rect.top,
      toJSON: () => ({}),
    }),
  } as unknown as HTMLDivElement;
}

test("缩放锚点：按指针相对页面的比例记录，越界时夹取", () => {
  const container = fakeElement({ left: 0, top: 0, width: 1000, height: 800 });
  const page = fakeElement({ left: 100, top: 100, width: 500, height: 600 });
  const anchor = capturePdfZoomAnchor(container, page, { clientX: 350, clientY: 400 });
  assert.deepEqual(anchor, {
    clientX: 350,
    clientY: 400,
    pageXRatio: 0.5,
    pageYRatio: 0.5,
  });
  const clamped = capturePdfZoomAnchor(container, page, { clientX: 9999, clientY: -50 });
  assert.equal(clamped?.pageXRatio, 1);
  assert.equal(clamped?.pageYRatio, 0);
});

test("缩放锚点：恢复时按滚动容器写回偏差", () => {
  const container = {
    scrollLeft: 0,
    scrollTop: 0,
    getBoundingClientRect: () => ({ left: 0, top: 0, width: 1000, height: 800 }),
  } as unknown as HTMLDivElement;
  const page = fakeElement({ left: 120, top: 160, width: 600, height: 720 });
  restorePdfZoomAnchor(container, page, {
    clientX: 300,
    clientY: 300,
    pageXRatio: 0.5,
    pageYRatio: 0.5,
  });
  assert.equal(container.scrollLeft, 120);
  assert.equal(container.scrollTop, 220);
});
