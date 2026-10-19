// M4 adapter：PdfViewer（旧 API：source/labels/onLoadError）。
// 渲染交给 react-pdf；这里只做 source 归一、页码/缩放工具条与错误文案。

import {
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type HTMLAttributes,
  type ReactNode,
} from "react";
import { ChevronLeftIcon, ChevronRightIcon, ZoomInIcon, ZoomOutIcon } from "lucide-react";
import { Document, Page, pdfjs } from "react-pdf";
import type { DocumentProps, PageProps } from "react-pdf";
import pdfWorkerSrc from "pdfjs-dist/build/pdf.worker.min.mjs?url";

import { Button } from "../Button.js";
import { createPdfJsDocumentOptions } from "@/lib/pdfJsAssets.js";
import {
  capturePdfZoomAnchor,
  restorePdfZoomAnchor,
  type PdfZoomAnchor,
} from "@/components/ui/usePdfZoomOverlay.js";
import {
  clampPdfPage,
  clampPdfScale,
  isPdfViewerRangeSource,
  mergePdfViewerLabels,
  normalizePdfDocumentSource,
  PDF_DEFAULT_SCALE,
  PDF_MAX_SCALE,
  PDF_MIN_SCALE,
  PDF_VIEWER_RANGE_CHUNK_BYTES,
  PDF_ZOOM_STEP,
  type PdfViewerLabels,
  type PdfViewerRangeSource,
  type PdfViewerSource,
} from "../pdfViewerSource.js";
import { cn } from "@/components/lib/utils.js";

pdfjs.GlobalWorkerOptions.workerSrc = pdfWorkerSrc;

const BASE_URL = typeof import.meta.env?.BASE_URL === "string" ? import.meta.env.BASE_URL : "./";
const DOCUMENT_OPTIONS = createPdfJsDocumentOptions(
  BASE_URL,
  globalThis.location?.href ?? "http://localhost/",
);
const RANGE_DOCUMENT_OPTIONS = {
  ...DOCUMENT_OPTIONS,
  disableAutoFetch: true,
  disableStream: true,
  rangeChunkSize: PDF_VIEWER_RANGE_CHUNK_BYTES,
};

class PdfViewerRangeTransport extends pdfjs.PDFDataRangeTransport {
  private readonly rangeSource: PdfViewerRangeSource;
  private readonly onRequestError: (error: Error) => void;

  constructor(rangeSource: PdfViewerRangeSource, onRequestError: (error: Error) => void) {
    super(rangeSource.totalBytes, rangeSource.initialData ?? null);
    this.rangeSource = rangeSource;
    this.onRequestError = onRequestError;
  }

  override requestDataRange(begin: number, end: number): void {
    this.rangeSource
      .requestRange(begin, end - begin)
      .then((chunk) => this.onDataRange(begin, chunk))
      .catch((error: unknown) => {
        this.onRequestError(error instanceof Error ? error : new Error(String(error)));
      });
  }
}

export interface PdfViewerProps extends HTMLAttributes<HTMLDivElement> {
  source: PdfViewerSource;
  labels?: Partial<PdfViewerLabels>;
  onLoadError?: (error: Error) => void;
}

export function PdfViewer({
  source,
  labels,
  onLoadError,
  className,
  ...rest
}: PdfViewerProps): ReactNode {
  const mergedLabels = useMemo(() => mergePdfViewerLabels(labels), [labels]);
  const [numPages, setNumPages] = useState<number | null>(null);
  const [pageNumber, setPageNumber] = useState(1);
  const [pageInput, setPageInput] = useState("1");
  const [scale, setScale] = useState(PDF_DEFAULT_SCALE);
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const pageViewportRef = useRef<HTMLDivElement | null>(null);
  const scaleRef = useRef(scale);
  const [pendingAnchor, setPendingAnchor] = useState<PdfZoomAnchor | null>(null);

  const isRange = isPdfViewerRangeSource(source);
  const file = useMemo<DocumentProps["file"]>(() => {
    if (isRange) {
      return {
        range: new PdfViewerRangeTransport(source, (error) => onLoadError?.(error)),
      } as unknown as DocumentProps["file"];
    }
    return normalizePdfDocumentSource(source) as unknown as DocumentProps["file"];
  }, [isRange, source, onLoadError]);

  const commitPage = (): void => {
    const parsed = Number.parseInt(pageInput, 10);
    const next = Number.isFinite(parsed) ? clampPdfPage(parsed, numPages) : pageNumber;
    setPageNumber(next);
    setPageInput(String(next));
  };

  const gotoPage = (next: number): void => {
    const clamped = clampPdfPage(next, numPages);
    setPageNumber(clamped);
    setPageInput(String(clamped));
  };

  const changeScale = (next: number, pointer?: { clientX: number; clientY: number }): void => {
    const container = scrollRef.current;
    const viewport = pageViewportRef.current;
    if (container && viewport) {
      const anchor = capturePdfZoomAnchor(container, viewport, pointer);
      if (anchor) {
        setPendingAnchor(anchor);
      }
    }
    setScale(clampPdfScale(next));
  };

  useEffect(() => {
    scaleRef.current = scale;
  }, [scale]);

  useLayoutEffect(() => {
    if (!pendingAnchor) {
      return;
    }
    const container = scrollRef.current;
    const viewport = pageViewportRef.current;
    if (container && viewport) {
      restorePdfZoomAnchor(container, viewport, pendingAnchor);
    }
    setPendingAnchor(null);
  }, [pendingAnchor]);

  useEffect(() => {
    const node = scrollRef.current;
    if (!node) {
      return;
    }
    const handleWheel = (event: WheelEvent): void => {
      if (!event.ctrlKey && !event.metaKey) {
        return;
      }
      event.preventDefault();
      changeScale(scaleRef.current + (event.deltaY < 0 ? PDF_ZOOM_STEP : -PDF_ZOOM_STEP), {
        clientX: event.clientX,
        clientY: event.clientY,
      });
    };
    node.addEventListener("wheel", handleWheel, { passive: false });
    return () => node.removeEventListener("wheel", handleWheel);
  }, []);

  return (
    <div
      {...rest}
      data-slot="pdf-viewer"
      className={cn(["flex h-full min-h-0 flex-col", className])}
    >
      <div className="flex shrink-0 items-center gap-1 border-b border-border px-2 py-1">
        <Button
          aria-label={mergedLabels.previousPage}
          size="icon-xs"
          variant="ghost"
          disabled={pageNumber <= 1}
          onClick={() => gotoPage(pageNumber - 1)}
        >
          <ChevronLeftIcon className="size-3.5" />
        </Button>
        <input
          aria-label={mergedLabels.pageInput}
          className="h-5 w-10 rounded-sm border border-input-border bg-input px-1 text-center text-ui-base tabular-nums outline-none focus-visible:border-input-border-focused"
          value={pageInput}
          onChange={(event) => setPageInput(event.target.value)}
          onBlur={commitPage}
          onKeyDown={(event) => {
            if (event.key === "Enter") {
              event.preventDefault();
              commitPage();
            }
          }}
        />
        <span className="text-ui-sm text-foreground-subtle">/ {numPages ?? "–"}</span>
        <Button
          aria-label={mergedLabels.nextPage}
          size="icon-xs"
          variant="ghost"
          disabled={numPages !== null && pageNumber >= numPages}
          onClick={() => gotoPage(pageNumber + 1)}
        >
          <ChevronRightIcon className="size-3.5" />
        </Button>
        <span className="ms-auto flex items-center gap-1">
          <Button
            aria-label={mergedLabels.zoomOut}
            size="icon-xs"
            variant="ghost"
            disabled={scale <= PDF_MIN_SCALE}
            onClick={() => changeScale(scale - PDF_ZOOM_STEP)}
          >
            <ZoomOutIcon className="size-3.5" />
          </Button>
          <span className="min-w-10 text-center text-ui-sm text-foreground-subtle tabular-nums">
            {Math.round(scale * 100)}%
          </span>
          <Button
            aria-label={mergedLabels.zoomIn}
            size="icon-xs"
            variant="ghost"
            disabled={scale >= PDF_MAX_SCALE}
            onClick={() => changeScale(scale + PDF_ZOOM_STEP)}
          >
            <ZoomInIcon className="size-3.5" />
          </Button>
        </span>
      </div>
      <div ref={scrollRef} className="min-h-0 flex-1 overflow-auto p-3">
        <Document
          file={file}
          options={isRange ? RANGE_DOCUMENT_OPTIONS : DOCUMENT_OPTIONS}
          loading={
            <div className="p-4 text-ui-sm text-foreground-subtle">{mergedLabels.loading}</div>
          }
          error={<div className="p-4 text-ui-sm text-destructive">{mergedLabels.loadError}</div>}
          noData={
            <div className="p-4 text-ui-sm text-foreground-subtle">{mergedLabels.noData}</div>
          }
          onLoadSuccess={(document: { numPages: number }) => {
            setNumPages(document.numPages);
            setPageNumber((current) => clampPdfPage(current, document.numPages));
          }}
          onLoadError={(error: Error) => onLoadError?.(error)}
        >
          <div ref={pageViewportRef} className="mx-auto w-fit">
            <Page
              pageNumber={pageNumber}
              scale={scale}
              renderTextLayer
              renderAnnotationLayer
              className="shadow-md"
              loading=""
            />
          </div>
        </Document>
      </div>
    </div>
  );
}

export type { PdfViewerLabels, PdfViewerRangeSource, PdfViewerSource };

export type { DocumentProps as PdfDocumentProps, PageProps as PdfPageProps };
