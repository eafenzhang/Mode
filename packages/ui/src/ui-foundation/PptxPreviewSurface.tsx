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
  resolvePptxThumbnailScale,
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
  thumbnails?: string;
  thumbnail?: (pageNumber: number) => string;
}

export interface PptxPreviewSurfaceProps extends HTMLAttributes<HTMLDivElement> {
  data: ArrayBuffer;
  labels: PptxPreviewSurfaceLabels;
  onOpenBrowserUrl?: (url: string) => void;
}

function PptxThumbnail({
  document,
  pageIndex,
  selected,
  label,
  onSelect,
}: {
  document: PresentationPreviewDocument;
  pageIndex: number;
  selected: boolean;
  label: string;
  onSelect: () => void;
}) {
  const buttonRef = useRef<HTMLButtonElement | null>(null);
  const pageRef = useRef<HTMLDivElement | null>(null);
  const [visible, setVisible] = useState(false);

  useEffect(() => {
    const node = buttonRef.current;
    if (!node || typeof IntersectionObserver === "undefined") {
      setVisible(true);
      return;
    }
    const observer = new IntersectionObserver(
      ([entry]) => setVisible(entry?.isIntersecting ?? false),
      { rootMargin: "200px 0px" },
    );
    observer.observe(node);
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    const node = pageRef.current;
    if (!visible || !node) {
      return;
    }
    node.replaceChildren();
    const handle = document.renderPage(pageIndex, node);
    void handle.ready.catch(() => undefined);
    return () => handle.dispose();
  }, [visible, document, pageIndex]);

  const scale = resolvePptxThumbnailScale(document.pageSize.width, 96);
  return (
    <button
      ref={buttonRef}
      type="button"
      aria-label={label}
      aria-current={selected ? "page" : undefined}
      data-thumbnail-index={pageIndex}
      className={[
        "w-28 shrink-0 overflow-hidden rounded-md border bg-surface p-1 text-start",
        selected ? "border-input-border-focused" : "border-border hover:border-border-hover",
      ].join(" ")}
      onClick={onSelect}
    >
      <div
        ref={pageRef}
        style={{
          width: 96,
          height: Math.ceil(document.pageSize.height * scale),
          transform: `scale(${scale})`,
          transformOrigin: "top left",
        }}
      />
    </button>
  );
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
      <div className="flex min-h-0 flex-1">
        {labels.thumbnail && document ? (
          <aside
            aria-label={labels.thumbnails}
            data-slot="pptx-preview-thumbnails"
            className="flex w-32 shrink-0 flex-col gap-2 overflow-y-auto border-e border-border p-2"
          >
            {Array.from({ length: document.pageCount }, (_, index) => (
              <PptxThumbnail
                key={index}
                document={document}
                pageIndex={index}
                selected={pageNumber === index + 1}
                label={labels.thumbnail?.(index + 1) ?? String(index + 1)}
                onSelect={() => {
                  setPageNumber(index + 1);
                  setPageInput(String(index + 1));
                }}
              />
            ))}
          </aside>
        ) : null}
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
    </div>
  );
}
