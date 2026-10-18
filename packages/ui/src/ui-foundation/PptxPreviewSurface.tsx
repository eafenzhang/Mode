// P1.1 PPTX 预览渲染面（spec: §2.22，pptx viewer 第一层的前半）。
// 只负责加载、单页渲染与页码/缩放；缩略图、元素选择与导出在后续层补齐后才切换业务组件。

import { useEffect, useMemo, useRef, useState, type HTMLAttributes } from "react";
import { ChevronLeftIcon, ChevronRightIcon, ZoomInIcon, ZoomOutIcon } from "lucide-react";

import { pptxRendererPreviewEngine } from "@/presentation/pptxRendererPreviewEngine.js";
import type { PresentationPreviewDocument } from "@/presentation/types.js";

import { Button } from "./Button.js";
import {
  resolvePptxPageDisplaySize,
  resolvePptxPageFromInput,
  stepPptxZoom,
} from "./pptxViewerLayout.js";

export interface PptxPreviewSurfaceLabels {
  loading: string;
  loadError: string;
  previousPage: string;
  nextPage: string;
  pageInput: string;
  zoomIn: string;
  zoomOut: string;
}

export interface PptxPreviewSurfaceProps extends HTMLAttributes<HTMLDivElement> {
  data: ArrayBuffer;
  labels: PptxPreviewSurfaceLabels;
  onOpenBrowserUrl?: (url: string) => void;
}

export function PptxPreviewSurface({
  data,
  labels,
  onOpenBrowserUrl,
  className,
  ...rest
}: PptxPreviewSurfaceProps) {
  const [document, setDocument] = useState<PresentationPreviewDocument | null>(null);
  const [failed, setFailed] = useState(false);
  const [pageNumber, setPageNumber] = useState(1);
  const [pageInput, setPageInput] = useState("1");
  const [zoomPercent, setZoomPercent] = useState(100);
  const pageRef = useRef<HTMLDivElement | null>(null);
  const documentRef = useRef<PresentationPreviewDocument | null>(null);

  useEffect(() => {
    let cancelled = false;
    setFailed(false);
    setDocument(null);
    documentRef.current?.dispose();
    documentRef.current = null;
    // pdf/pptx 解析可能 transfer buffer；复制一份保证调用方可复用。
    void pptxRendererPreviewEngine
      .open(data.slice(0))
      .then((next) => {
        if (cancelled) {
          next.dispose();
          return;
        }
        documentRef.current = next;
        setDocument(next);
        setPageNumber(1);
        setPageInput("1");
      })
      .catch(() => {
        if (!cancelled) {
          setFailed(true);
        }
      });
    return () => {
      cancelled = true;
      documentRef.current?.dispose();
      documentRef.current = null;
    };
  }, [data]);

  useEffect(() => {
    const node = pageRef.current;
    if (!document || !node) {
      return;
    }
    node.replaceChildren();
    const handle = document.renderPage(pageNumber - 1, node, {
      onNavigate: (target) => {
        if (target.url) {
          onOpenBrowserUrl?.(target.url);
        } else if (typeof target.pageIndex === "number") {
          setPageNumber(Math.min(Math.max(1, target.pageIndex + 1), document.pageCount));
        }
      },
    });
    void handle.ready.catch(() => setFailed(true));
    return () => handle.dispose();
  }, [document, pageNumber, onOpenBrowserUrl]);

  const displaySize = useMemo(
    () => (document ? resolvePptxPageDisplaySize(document.pageSize, zoomPercent) : null),
    [document, zoomPercent],
  );

  const commitPage = (): void => {
    const next = resolvePptxPageFromInput(pageInput, document?.pageCount ?? 1, pageNumber);
    setPageNumber(next);
    setPageInput(String(next));
  };

  return (
    <div
      {...rest}
      data-slot="pptx-preview-surface"
      className={["flex h-full min-h-0 flex-col", className].filter(Boolean).join(" ")}
    >
      <div className="flex shrink-0 items-center gap-1 border-b border-border px-2 py-1">
        <Button
          aria-label={labels.previousPage}
          size="icon-xs"
          variant="ghost"
          disabled={pageNumber <= 1}
          onClick={() => {
            const next = Math.max(1, pageNumber - 1);
            setPageNumber(next);
            setPageInput(String(next));
          }}
        >
          <ChevronLeftIcon className="size-3.5" />
        </Button>
        <input
          aria-label={labels.pageInput}
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
        <span className="text-ui-sm text-foreground-subtle">/ {document?.pageCount ?? "–"}</span>
        <Button
          aria-label={labels.nextPage}
          size="icon-xs"
          variant="ghost"
          disabled={document !== null && pageNumber >= document.pageCount}
          onClick={() => {
            const next = Math.min(document?.pageCount ?? pageNumber, pageNumber + 1);
            setPageNumber(next);
            setPageInput(String(next));
          }}
        >
          <ChevronRightIcon className="size-3.5" />
        </Button>
        <span className="ms-auto flex items-center gap-1">
          <Button
            aria-label={labels.zoomOut}
            size="icon-xs"
            variant="ghost"
            onClick={() => setZoomPercent((current) => stepPptxZoom(current, "out"))}
          >
            <ZoomOutIcon className="size-3.5" />
          </Button>
          <span className="min-w-10 text-center text-ui-sm tabular-nums text-foreground-subtle">
            {zoomPercent}%
          </span>
          <Button
            aria-label={labels.zoomIn}
            size="icon-xs"
            variant="ghost"
            onClick={() => setZoomPercent((current) => stepPptxZoom(current, "in"))}
          >
            <ZoomInIcon className="size-3.5" />
          </Button>
        </span>
      </div>
      <div className="min-h-0 flex-1 overflow-auto p-3">
        {failed ? (
          <div className="p-4 text-ui-sm text-destructive">{labels.loadError}</div>
        ) : document === null ? (
          <div className="p-4 text-ui-sm text-foreground-subtle">{labels.loading}</div>
        ) : (
          <div
            ref={pageRef}
            data-slot="pptx-preview-page"
            style={
              displaySize ? { width: displaySize.width, height: displaySize.height } : undefined
            }
            className="mx-auto"
          />
        )}
      </div>
    </div>
  );
}
