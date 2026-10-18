/**
 * M4 adapter：PdfViewer 输入归一与页码/缩放边界（spec: docs/specs/m4-ui-foundation.md §2.18）。
 * 只测纯函数，避免在测试进程加载 react-pdf/pdfjs。
 * 运行：pnpm --filter @mode/ui test
 */
import assert from "node:assert/strict";
import test from "node:test";

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
