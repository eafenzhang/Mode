// PdfViewer 的纯输入归一（不依赖 react-pdf/pdfjs，便于单测）。
export interface PdfViewerRangeSource {
  totalBytes: number;
  initialData?: Uint8Array;
  requestRange: (offset: number, length: number) => Promise<Uint8Array>;
}

export type PdfViewerSource = string | Blob | ArrayBuffer | Uint8Array | PdfViewerRangeSource;

export type PdfDocumentFile = string | Blob | { data: Uint8Array };

export const PDF_VIEWER_RANGE_CHUNK_BYTES = 256 * 1024;

export function isPdfViewerRangeSource(source: PdfViewerSource): source is PdfViewerRangeSource {
  return (
    typeof source === "object" &&
    source !== null &&
    "requestRange" in source &&
    typeof (source as PdfViewerRangeSource).requestRange === "function"
  );
}

export function normalizePdfDocumentSource(
  source: Exclude<PdfViewerSource, PdfViewerRangeSource>,
): PdfDocumentFile {
  if (typeof source === "string" || source instanceof Blob) {
    return source;
  }
  if (source instanceof ArrayBuffer) {
    return { data: new Uint8Array(source.slice(0)) };
  }
  return { data: new Uint8Array(source) };
}

export interface PdfViewerLabels {
  loading: string;
  loadError: string;
  noData: string;
  previousPage: string;
  nextPage: string;
  pageInput: string;
  zoomIn: string;
  zoomOut: string;
}

export const DEFAULT_PDF_VIEWER_LABELS: PdfViewerLabels = {
  loading: "Loading PDF…",
  loadError: "Failed to load PDF",
  noData: "No PDF data",
  previousPage: "Previous page",
  nextPage: "Next page",
  pageInput: "Page number",
  zoomIn: "Zoom in",
  zoomOut: "Zoom out",
};

export function mergePdfViewerLabels(labels?: Partial<PdfViewerLabels>): PdfViewerLabels {
  return { ...DEFAULT_PDF_VIEWER_LABELS, ...labels };
}

export function clampPdfPage(page: number, numPages: number | null): number {
  if (numPages === null || numPages <= 0) {
    return 1;
  }
  return Math.min(Math.max(Math.trunc(page), 1), numPages);
}

export const PDF_ZOOM_STEP = 0.25;
export const PDF_MIN_SCALE = 0.25;
export const PDF_MAX_SCALE = 4;
export const PDF_DEFAULT_SCALE = 1;

export function clampPdfScale(scale: number): number {
  return Math.min(Math.max(scale, PDF_MIN_SCALE), PDF_MAX_SCALE);
}
