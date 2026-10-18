// PPTX 预览的 PDF 导出（spec: §2.22）。
import type { PrintPageToPdfResult, SaveFileRequest, SaveFileResult } from "@mode/shared";
import type { PresentationPreviewDocument } from "@/presentation/types.js";

import { resolvePptxPdfFileName } from "./pptxViewerLayout.js";

export interface PptxPdfExportPlatform {
  printPageToPdf: () => Promise<PrintPageToPdfResult>;
  saveFile: (payload: SaveFileRequest) => Promise<SaveFileResult>;
}

export type PptxPdfExportResult =
  | { status: "success"; path: string }
  | { status: "canceled" }
  | { status: "failed" };

export async function exportPptxToPdf(options: {
  document: PresentationPreviewDocument;
  platform: PptxPdfExportPlatform;
  fileName?: string;
}): Promise<PptxPdfExportResult> {
  let host: { dispose(): void } | null = null;
  try {
    const { renderPresentationToPrintHost } =
      await import("@/presentation/presentationPdfPrintExport.js");
    host = await renderPresentationToPrintHost(options.document, window.document);
    const printed = await options.platform.printPageToPdf();
    if (!printed.success || !printed.data) {
      return { status: "failed" };
    }
    const saved = await options.platform.saveFile({
      data: printed.data,
      suggestedName: resolvePptxPdfFileName(options.fileName),
    });
    if (saved.success && saved.path) {
      return { status: "success", path: saved.path };
    }
    return saved.canceled ? { status: "canceled" } : { status: "failed" };
  } catch {
    return { status: "failed" };
  } finally {
    host?.dispose();
  }
}
