// P1.1 PPTX 预览渲染面（spec: §2.22，pptx viewer 第一层的前半）。
// 只负责加载、单页渲染与页码/缩放；缩略图、元素选择与导出在后续层补齐后才切换业务组件。

import { useEffect, useMemo, useRef, useState } from "react";
import {
  ChevronLeftIcon,
  ChevronRightIcon,
  FileDownIcon,
  Loader2Icon,
  MousePointer2Icon,
  ZoomInIcon,
  ZoomOutIcon,
} from "lucide-react";

import { pptxRendererPreviewEngine } from "@/presentation/pptxRendererPreviewEngine.js";
import { hitTestPresentationElement } from "@/presentation/presentationElementModel.js";
import type { PresentationPageElement, PresentationPreviewDocument } from "@/presentation/types.js";
import {
  createPptxElementReference,
  dispatchPptxElementReferenceAddToChat,
  sha256Fingerprint,
} from "@/lib/pptxElementReference.js";

import { PptxSelectionActionBar } from "./adapters/pptx-selection-action-bar.js";

import { Button } from "./Button.js";
import { PptxThumbnail } from "./PptxPreviewThumbnail.js";
import type { PptxPreviewSurfaceProps } from "./pptxPreviewLabels.js";
import { toast } from "./adapters/toast.js";
import { useOptionalPlatform } from "@/hooks/usePlatform.js";
import {
  resolvePptxIntrinsicPoint,
  resolvePptxNavigationTarget,
  resolvePptxPageDisplaySize,
  resolvePptxPageFromInput,
  stepPptxZoom,
} from "./pptxViewerLayout.js";

export function PptxPreviewSurface({
  data,
  labels,
  referenceSource,
  referenceNavigation,
  referenceNavigationReady = true,
  fileName,
  onOpenBrowserUrl,
  className,
  ...rest
}: PptxPreviewSurfaceProps) {
  const [document, setDocument] = useState<PresentationPreviewDocument | null>(null);
  const [failed, setFailed] = useState(false);
  const [pageNumber, setPageNumber] = useState(1);
  const [pageInput, setPageInput] = useState("1");
  const [zoomPercent, setZoomPercent] = useState(100);
  const [selectionMode, setSelectionMode] = useState(false);
  const [selectedElement, setSelectedElement] = useState<PresentationPageElement | null>(null);
  const [editing, setEditing] = useState(false);
  const [viewport, setViewport] = useState<HTMLElement | null>(null);
  const [sourceFingerprint, setSourceFingerprint] = useState("");
  const [exporting, setExporting] = useState(false);
  const platform = useOptionalPlatform();
  const pageRef = useRef<HTMLDivElement | null>(null);
  const pageContentRef = useRef<HTMLDivElement | null>(null);
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
    let cancelled = false;
    setSourceFingerprint("");
    void sha256Fingerprint(data.slice(0)).then((fingerprint) => {
      if (!cancelled) {
        setSourceFingerprint(fingerprint);
      }
    });
    return () => {
      cancelled = true;
    };
  }, [data]);

  useEffect(() => {
    if (!referenceNavigation || !document || !sourceFingerprint || !referenceNavigationReady) {
      return;
    }
    const target = resolvePptxNavigationTarget({
      pageIndex: referenceNavigation.pageIndex,
      expectedSourceFingerprint: referenceNavigation.expectedSourceFingerprint,
      currentSourceFingerprint: sourceFingerprint,
      pageCount: document.pageCount,
    });
    if (target.status === "source-changed") {
      toast(labels.referencedSourceChanged(referenceNavigation.pageIndex + 1));
      return;
    }
    if (target.status === "page-missing") {
      toast(labels.referencedPageMissing(target.pageIndex + 1));
      return;
    }
    setPageNumber(target.pageIndex + 1);
    setPageInput(String(target.pageIndex + 1));
  }, [referenceNavigation, referenceNavigationReady, document, sourceFingerprint, labels]);

  const exportPdf = async (): Promise<void> => {
    const printPageToPdf = platform?.printPageToPdf;
    const saveFile = platform?.saveFile;
    if (!document || exporting || !printPageToPdf || !saveFile) {
      return;
    }
    setExporting(true);
    try {
      const { exportPptxToPdf } = await import("./pptxPdfExport.js");
      const result = await exportPptxToPdf({
        document,
        platform: { printPageToPdf, saveFile },
        fileName,
      });
      if (result.status === "success") {
        toast(labels.exportPdfSuccess?.(result.path) ?? result.path);
      } else if (result.status === "failed") {
        toast(labels.exportPdfFailed ?? "PDF export failed");
      }
    } finally {
      setExporting(false);
    }
  };

  useEffect(() => {
    const node = pageContentRef.current;
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
        <Button
          aria-label={selectionMode ? labels.exitElementSelection : labels.selectElement}
          size="icon-xs"
          variant={selectionMode ? "secondary" : "ghost"}
          onClick={() => {
            setSelectionMode((current) => !current);
            setSelectedElement(null);
            setEditing(false);
          }}
        >
          <MousePointer2Icon className="size-3.5" />
        </Button>
        {platform?.printPageToPdf && platform.saveFile ? (
          <Button
            aria-label={exporting ? labels.exportingPdf : labels.exportPdf}
            title={exporting ? labels.exportingPdf : labels.exportPdf}
            size="icon-xs"
            variant="ghost"
            disabled={exporting || !document}
            onClick={() => void exportPdf()}
          >
            {exporting ? (
              <Loader2Icon className="size-3.5 animate-spin" />
            ) : (
              <FileDownIcon className="size-3.5" />
            )}
          </Button>
        ) : null}
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
        {document ? (
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
        <div ref={setViewport} className="min-h-0 flex-1 overflow-auto p-3">
          {failed ? (
            <div className="p-4 text-ui-sm text-destructive">{labels.loadError}</div>
          ) : document === null ? (
            <div className="p-4 text-ui-sm text-foreground-subtle">{labels.loading}</div>
          ) : (
            <PptxSelectionActionBar
              open={selectionMode && selectedElement !== null}
              boundary={viewport}
              label={labels.aiEdit ?? "AI edit"}
              commentPlaceholder={labels.commentPlaceholder ?? "Comment"}
              cancelLabel={labels.cancelAiEdit ?? "Cancel"}
              addToConversationLabel={labels.addToConversation ?? "Add to conversation"}
              editing={editing}
              disabled={!referenceSource || !sourceFingerprint}
              onAiEdit={() => setEditing(true)}
              onCancelAiEdit={() => setEditing(false)}
              onAddToConversation={(comment) => {
                if (!referenceSource || !selectedElement || !sourceFingerprint) {
                  return;
                }
                void createPptxElementReference({
                  element: selectedElement,
                  ...referenceSource,
                  sourceFingerprint,
                  ...(selectedElement.text ? { selectedText: selectedElement.text } : {}),
                  ...(comment ? { comment } : {}),
                }).then((reference) => {
                  dispatchPptxElementReferenceAddToChat(reference);
                  setEditing(false);
                });
              }}
              onExitSelection={() => {
                setSelectionMode(false);
                setSelectedElement(null);
                setEditing(false);
              }}
            >
              <div
                ref={pageRef}
                data-slot="pptx-preview-page"
                style={
                  displaySize ? { width: displaySize.width, height: displaySize.height } : undefined
                }
                className="mx-auto"
                onClick={(event) => {
                  if (!selectionMode || !document) {
                    return;
                  }
                  const rect = pageContentRef.current?.getBoundingClientRect();
                  if (!rect) {
                    return;
                  }
                  const point = resolvePptxIntrinsicPoint({
                    clientX: event.clientX,
                    clientY: event.clientY,
                    viewport: rect,
                    scale: zoomPercent / 100,
                  });
                  const hit = hitTestPresentationElement(
                    document.getPageElements(pageNumber - 1),
                    point,
                  );
                  setSelectedElement(hit);
                  setEditing(false);
                }}
              >
                {document ? (
                  <div
                    ref={pageContentRef}
                    style={{
                      position: "relative",
                      width: document.pageSize.width,
                      height: document.pageSize.height,
                      transform: `scale(${zoomPercent / 100})`,
                      transformOrigin: "top left",
                    }}
                  >
                    {selectedElement ? (
                      <div
                        data-slot="pptx-preview-selected-element"
                        style={{
                          position: "absolute",
                          left: selectedElement.bounds.x,
                          top: selectedElement.bounds.y,
                          width: selectedElement.bounds.width,
                          height: selectedElement.bounds.height,
                          pointerEvents: "none",
                        }}
                        className="border-2 border-brand"
                      />
                    ) : null}
                  </div>
                ) : null}
              </div>
            </PptxSelectionActionBar>
          )}
        </div>
      </div>
    </div>
  );
}
