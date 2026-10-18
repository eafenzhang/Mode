// PPTX 预览的文案与 props 契约（spec: §2.22）。
import type { HTMLAttributes } from "react";

import type { PptxElementReferenceSource } from "@/lib/pptxElementReference.js";

export interface PptxPreviewViewerLabels {
  loading: string;
  loadError: string;
  noSlides: string;
  previousPage: string;
  nextPage: string;
  pageInput: string;
  zoomIn: string;
  zoomOut: string;
  thumbnails: string;
  thumbnail: (pageNumber: number) => string;
  exportPdf: string;
  exportingPdf: string;
  exportPdfSuccess: (path: string) => string;
  exportPdfFailed: string;
  selectElement: string;
  exitElementSelection: string;
  aiEdit: string;
  commentPlaceholder: string;
  cancelAiEdit: string;
  addToConversation: string;
  referencedPageMissing: (pageNumber: number) => string;
  referencedSourceChanged: (pageNumber: number) => string;
}

export interface PptxPreviewSurfaceProps extends HTMLAttributes<HTMLDivElement> {
  data: ArrayBuffer;
  labels: PptxPreviewViewerLabels;
  referenceSource?: PptxElementReferenceSource;
  referenceNavigation?: {
    requestId: string;
    pageIndex: number;
    expectedSourceFingerprint: string;
  };
  referenceNavigationReady?: boolean;
  fileName?: string;
  onOpenBrowserUrl?: (url: string) => void;
}
