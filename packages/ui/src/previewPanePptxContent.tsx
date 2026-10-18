import { lazy, Suspense } from "react";
import type { PptxPreviewViewerLabels } from "@/ui-foundation/pptxPreviewLabels.js";
import type { PptxElementReferenceSource } from "@/lib/pptxElementReference.js";
import type { PptxReferencePreviewNavigation } from "@/lib/codeViewer.js";

const PptxPreviewSurface = lazy(() =>
  import("@/ui-foundation/PptxPreviewSurface.js").then((module) => ({
    default: module.PptxPreviewSurface,
  })),
);

export function PptxPreviewContent({
  data,
  fileName,
  labels,
  onOpenBrowserUrl,
  referenceSource,
  referenceNavigation,
  referenceNavigationReady,
}: {
  data: ArrayBuffer;
  labels: PptxPreviewViewerLabels;
  fileName?: string;
  referenceSource?: PptxElementReferenceSource;
  referenceNavigation?: PptxReferencePreviewNavigation;
  referenceNavigationReady?: boolean;
  onOpenBrowserUrl?: (url: string) => void;
}) {
  return (
    <Suspense
      fallback={<div className="p-3 text-ui-base text-foreground-subtle">{labels.loading}</div>}
    >
      <PptxPreviewSurface
        data={data}
        fileName={fileName}
        labels={labels}
        onOpenBrowserUrl={onOpenBrowserUrl}
        {...(referenceSource ? { referenceSource } : {})}
        {...(referenceNavigation ? { referenceNavigation } : {})}
        referenceNavigationReady={referenceNavigationReady}
        className="h-full"
      />
    </Suspense>
  );
}
