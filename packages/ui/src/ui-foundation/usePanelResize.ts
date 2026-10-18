// P1.2 指针拖拽 hook（spec: docs/specs/m4-ui-foundation.md §3）。
// 只负责指针捕获、增量换算与拖拽态；尺寸仍由调用方状态层持有。

import { useCallback, useRef, useState, type PointerEvent as ReactPointerEvent } from "react";

import type { ResizeOrientation } from "./shellStyles.js";
import { pointerDelta, resizeFromPointer, type ResizeBounds } from "./resizeMath.js";

export interface UsePanelResizeOptions {
  orientation: ResizeOrientation;
  /** 当前尺寸（拖拽开始时快照）。 */
  size: number;
  bounds: ResizeBounds;
  onResize: (size: number) => void;
  onDraggingChange?: (dragging: boolean) => void;
}

export interface PanelResizeHandleProps {
  onPointerDown: (event: ReactPointerEvent<HTMLElement>) => void;
  onPointerMove: (event: ReactPointerEvent<HTMLElement>) => void;
  onPointerUp: (event: ReactPointerEvent<HTMLElement>) => void;
  onPointerCancel: (event: ReactPointerEvent<HTMLElement>) => void;
  onLostPointerCapture: (event: ReactPointerEvent<HTMLElement>) => void;
}

export interface PanelResize {
  dragging: boolean;
  handleProps: PanelResizeHandleProps;
}

interface DragState {
  pointerId: number;
  startCoordinate: number;
  startSize: number;
}

export function usePanelResize(options: UsePanelResizeOptions): PanelResize {
  const { orientation, size, bounds, onResize, onDraggingChange } = options;
  const [dragging, setDragging] = useState(false);
  const dragRef = useRef<DragState | null>(null);

  const finish = useCallback((): void => {
    if (!dragRef.current) {
      return;
    }
    dragRef.current = null;
    setDragging(false);
    onDraggingChange?.(false);
  }, [onDraggingChange]);

  const onPointerDown = useCallback(
    (event: ReactPointerEvent<HTMLElement>): void => {
      if (event.button !== 0) {
        return;
      }
      event.preventDefault();
      event.currentTarget.setPointerCapture?.(event.pointerId);
      dragRef.current = {
        pointerId: event.pointerId,
        startCoordinate: orientation === "vertical" ? event.clientX : event.clientY,
        startSize: size,
      };
      setDragging(true);
      onDraggingChange?.(true);
    },
    [orientation, size, onDraggingChange],
  );

  const onPointerMove = useCallback(
    (event: ReactPointerEvent<HTMLElement>): void => {
      const drag = dragRef.current;
      if (!drag || drag.pointerId !== event.pointerId) {
        return;
      }
      const coordinate = orientation === "vertical" ? event.clientX : event.clientY;
      onResize(resizeFromPointer(drag.startSize, pointerDelta(orientation, drag.startCoordinate, coordinate), bounds));
    },
    [orientation, bounds, onResize],
  );

  const onPointerUp = useCallback(
    (event: ReactPointerEvent<HTMLElement>): void => {
      if (dragRef.current?.pointerId === event.pointerId) {
        event.currentTarget.releasePointerCapture?.(event.pointerId);
        finish();
      }
    },
    [finish],
  );

  const onPointerCancel = useCallback(
    (event: ReactPointerEvent<HTMLElement>): void => {
      if (dragRef.current?.pointerId === event.pointerId) {
        finish();
      }
    },
    [finish],
  );

  const onLostPointerCapture = useCallback((): void => {
    finish();
  }, [finish]);

  return {
    dragging,
    handleProps: { onPointerDown, onPointerMove, onPointerUp, onPointerCancel, onLostPointerCapture },
  };
}
