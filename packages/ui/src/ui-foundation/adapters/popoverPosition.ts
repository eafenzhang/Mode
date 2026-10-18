// M4 adapter：Popover 定位数学（与 Radix 的 align/side/collisionPadding 语义一致）。
export type PopoverSide = "top" | "bottom" | "left" | "right";
export type PopoverAlign = "start" | "center" | "end";

export interface PopoverPoint {
  top: number;
  left: number;
  availableHeight: number;
  triggerWidth: number;
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(Math.max(value, min), max);
}

export function computePopoverPoint(options: {
  anchor: DOMRect;
  content: { width: number; height: number };
  align: PopoverAlign;
  side: PopoverSide;
  sideOffset: number;
  alignOffset: number;
  avoidCollisions: boolean;
  collisionPadding: number;
  viewport: { width: number; height: number };
}): PopoverPoint {
  const {
    anchor,
    content,
    align,
    side,
    sideOffset,
    alignOffset,
    avoidCollisions,
    collisionPadding,
  } = options;
  const { width: vw, height: vh } = options.viewport;
  let top = 0;
  let left = 0;

  if (side === "top" || side === "bottom") {
    top = side === "bottom" ? anchor.bottom + sideOffset : anchor.top - content.height - sideOffset;
    if (align === "start") {
      left = anchor.left + alignOffset;
    } else if (align === "end") {
      left = anchor.right - content.width - alignOffset;
    } else {
      left = anchor.left + anchor.width / 2 - content.width / 2 + alignOffset;
    }
  } else {
    left = side === "right" ? anchor.right + sideOffset : anchor.left - content.width - sideOffset;
    if (align === "start") {
      top = anchor.top + alignOffset;
    } else if (align === "end") {
      top = anchor.bottom - content.height - alignOffset;
    } else {
      top = anchor.top + anchor.height / 2 - content.height / 2 + alignOffset;
    }
  }

  if (avoidCollisions) {
    top = clamp(
      top,
      collisionPadding,
      Math.max(collisionPadding, vh - content.height - collisionPadding),
    );
    left = clamp(
      left,
      collisionPadding,
      Math.max(collisionPadding, vw - content.width - collisionPadding),
    );
  }

  const availableHeight =
    side === "top"
      ? anchor.top - collisionPadding * 2 - sideOffset
      : vh - anchor.bottom - collisionPadding * 2 - sideOffset;

  return { top, left, availableHeight, triggerWidth: anchor.width };
}
