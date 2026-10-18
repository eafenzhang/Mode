// PptxPreviewSurface 的文案契约（spec: §2.22）。
export interface PptxPreviewSurfaceLabels {
  loading: string;
  loadError: string;
  previousPage: string;
  nextPage: string;
  pageInput: string;
  zoomIn: string;
  zoomOut: string;
  thumbnails?: string;
  thumbnail?: (pageNumber: number) => string;
  selectElement?: string;
  exitElementSelection?: string;
  aiEdit?: string;
  commentPlaceholder?: string;
  cancelAiEdit?: string;
  addToConversation?: string;
  exportPdf?: string;
  exportingPdf?: string;
  exportPdfSuccess?: (path: string) => string;
  exportPdfFailed?: string;
}
