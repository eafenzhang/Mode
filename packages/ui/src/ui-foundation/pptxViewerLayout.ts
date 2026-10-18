// PPTX 预览的纯布局/控制逻辑（spec: §2.21）。
import {
  PPTX_MAX_ZOOM_PERCENT,
  PPTX_MIN_ZOOM_PERCENT,
  PPTX_ZOOM_STEP_PERCENT,
  clampPresentationPageNumber,
  clampPresentationZoomPercent,
} from "@/presentation/presentationPreviewControls.js";
import type { PresentationPageSize } from "@/presentation/types.js";

export interface PptxPageDisplaySize {
  width: number;
  height: number;
}

export function resolvePptxPageDisplaySize(
  pageSize: PresentationPageSize,
  zoomPercent: number,
): PptxPageDisplaySize {
  const scale = clampPresentationZoomPercent(zoomPercent) / 100;
  return {
    width: Math.ceil(pageSize.width * scale),
    height: Math.ceil(pageSize.height * scale),
  };
}

export function stepPptxZoom(current: number, direction: "in" | "out"): number {
  const delta = direction === "in" ? PPTX_ZOOM_STEP_PERCENT : -PPTX_ZOOM_STEP_PERCENT;
  return clampPresentationZoomPercent(current + delta);
}

export function resolvePptxPageFromInput(
  value: string,
  pageCount: number,
  fallback: number,
): number {
  const parsed = Number.parseInt(value, 10);
  return clampPresentationPageNumber(Number.isFinite(parsed) ? parsed : fallback, pageCount);
}

export { PPTX_MAX_ZOOM_PERCENT, PPTX_MIN_ZOOM_PERCENT, PPTX_ZOOM_STEP_PERCENT };

export function resolvePptxThumbnailScale(pageWidth: number, targetWidth: number): number {
  if (!Number.isFinite(pageWidth) || pageWidth <= 0 || targetWidth <= 0) {
    return 1;
  }
  return targetWidth / pageWidth;
}

export function resolvePptxIntrinsicPoint(options: {
  clientX: number;
  clientY: number;
  viewport: Pick<DOMRect, "left" | "top">;
  scale: number;
}): { x: number; y: number } {
  const scale = options.scale > 0 ? options.scale : 1;
  return {
    x: (options.clientX - options.viewport.left) / scale,
    y: (options.clientY - options.viewport.top) / scale,
  };
}

export function resolvePptxPdfFileName(fileName?: string): string {
  const base = (fileName ?? "")
    .trim()
    .replace(/\.[^.]+$/u, "")
    .replace(/[/:*?"<>|]+/gu, "-")
    .trim();
  return `${base || "presentation"}.pdf`;
}

export type PptxNavigationResult =
  | { status: "ok"; pageIndex: number }
  | { status: "source-changed" }
  | { status: "page-missing"; pageIndex: number };

export function resolvePptxNavigationTarget(options: {
  pageIndex: number;
  expectedSourceFingerprint: string;
  currentSourceFingerprint: string;
  pageCount: number;
}): PptxNavigationResult {
  if (options.expectedSourceFingerprint !== options.currentSourceFingerprint) {
    return { status: "source-changed" };
  }
  if (options.pageIndex < 0 || options.pageIndex >= options.pageCount) {
    return { status: "page-missing", pageIndex: options.pageIndex };
  }
  return { status: "ok", pageIndex: options.pageIndex };
}
