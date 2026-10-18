// P1.2 面板尺寸换算的纯逻辑（spec: docs/specs/m4-ui-foundation.md §3）。
// 只做 clamp 与步进；尺寸归属调用方的状态层（store/settings），这里不持久化。

export interface ResizeBounds {
  min: number;
  max: number;
}

export type ResizeKey = "ArrowLeft" | "ArrowRight" | "ArrowUp" | "ArrowDown" | "Home" | "End";

export const RESIZE_KEYBOARD_STEP = 16;

export function normalizeBounds(bounds: ResizeBounds): ResizeBounds {
  const min = Math.max(0, Math.floor(bounds.min));
  const max = Math.max(min, Math.floor(bounds.max));
  return { min, max };
}

export function clampPanelSize(size: number, bounds: ResizeBounds): number {
  const { min, max } = normalizeBounds(bounds);
  if (!Number.isFinite(size)) {
    return min;
  }
  return Math.min(max, Math.max(min, Math.round(size)));
}

/** 指针拖拽：从起始尺寸累加 delta（向内容外侧为正）。 */
export function resizeFromPointer(startSize: number, delta: number, bounds: ResizeBounds): number {
  return clampPanelSize(startSize + delta, bounds);
}

/**
 * 指针位移 → 尺寸增量：
 * - 竖向分隔条（右侧 Side Pane）：向右拖增大宽度；
 * - 横向分隔条（底部终端）：向上拖增大高度（clientY 减小）。
 */
export function pointerDelta(
  orientation: "vertical" | "horizontal",
  startCoordinate: number,
  currentCoordinate: number,
): number {
  return orientation === "vertical"
    ? currentCoordinate - startCoordinate
    : startCoordinate - currentCoordinate;
}

export function isResizeKey(key: string): key is ResizeKey {
  return (
    key === "ArrowLeft" ||
    key === "ArrowRight" ||
    key === "ArrowUp" ||
    key === "ArrowDown" ||
    key === "Home" ||
    key === "End"
  );
}

/**
 * 键盘调整：Left/Up 减一步，Right/Down 加一步，Home/End 到边界。
 * 非调整键返回 null（调用方不应 preventDefault）。
 */
export function resizeFromKeyboard(
  size: number,
  key: string,
  bounds: ResizeBounds,
  step: number = RESIZE_KEYBOARD_STEP,
): number | null {
  if (!isResizeKey(key)) {
    return null;
  }
  const { min, max } = normalizeBounds(bounds);
  switch (key) {
    case "ArrowLeft":
    case "ArrowUp":
      return clampPanelSize(size - step, { min, max });
    case "ArrowRight":
    case "ArrowDown":
      return clampPanelSize(size + step, { min, max });
    case "Home":
      return min;
    case "End":
      return max;
  }
}
