// PptxPreviewSurface 的懒加载缩略图（spec: §2.22）。
import { useEffect, useRef, useState } from "react";

import type { PresentationPreviewDocument } from "@/presentation/types.js";

import { resolvePptxThumbnailScale } from "./pptxViewerLayout.js";

export function PptxThumbnail({
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
