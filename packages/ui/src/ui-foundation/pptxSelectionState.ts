// PptxSelectionActionBar 的纯计算（spec: docs/specs/m4-ui-foundation.md §2.19）。
export interface PptxActionBarPosition {
  left: number;
  top: number;
}

const EDGE_PADDING = 16;
const VERTICAL_OFFSET = 8;

export function resolvePptxActionBarPosition(
  boundary: Pick<DOMRect, "left" | "top" | "width">,
  viewportWidth: number,
): PptxActionBarPosition {
  const center = boundary.left + boundary.width / 2;
  const minLeft = EDGE_PADDING;
  const maxLeft = Math.max(minLeft, viewportWidth - EDGE_PADDING);
  return {
    left: Math.min(Math.max(center, minLeft), maxLeft),
    top: boundary.top + VERTICAL_OFFSET,
  };
}

export function normalizePptxComment(comment: string): string {
  return comment.trim();
}

export function shouldSubmitPptxComment(comment: string, disabled: boolean): boolean {
  return !disabled && normalizePptxComment(comment).length > 0;
}
