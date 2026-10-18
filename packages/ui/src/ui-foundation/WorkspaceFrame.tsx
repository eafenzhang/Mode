// P1.2 可渲染的 WorkspaceFrame：AppShell + usePanelResize + 尺寸持久化契约。
// 尺寸由本组件持有（调用方可注入 PanelSizePersistence）；键盘与指针都走同一组 bounds。

import { useEffect, useState, type ReactElement } from "react";

import { AppShell, type AppShellProps } from "./AppShell.js";
import { usePanelResize } from "./usePanelResize.js";
import type { ResizeBounds } from "./resizeMath.js";
import {
  resolveInitialPanelSize,
  type PanelSizePersistence,
} from "./panelSizePersistence.js";

const TERMINAL_BOUNDS: ResizeBounds = { min: 96, max: 480 };
const SIDE_PANE_BOUNDS: ResizeBounds = { min: 240, max: 640 };
const DEFAULT_TERMINAL_HEIGHT = 224;
const DEFAULT_SIDE_PANE_WIDTH = 320;

export interface WorkspaceFrameProps
  extends Omit<AppShellProps, "terminalResize" | "sidePaneResize"> {
  initialTerminalHeight?: number;
  terminalBounds?: ResizeBounds;
  initialSidePaneWidth?: number;
  sidePaneBounds?: ResizeBounds;
  persistence?: PanelSizePersistence;
}

export function WorkspaceFrame({
  initialTerminalHeight = DEFAULT_TERMINAL_HEIGHT,
  terminalBounds = TERMINAL_BOUNDS,
  initialSidePaneWidth = DEFAULT_SIDE_PANE_WIDTH,
  sidePaneBounds = SIDE_PANE_BOUNDS,
  persistence,
  terminal,
  sidePane,
  ...rest
}: WorkspaceFrameProps): ReactElement {
  const [terminalSize, setTerminalSize] = useState(() =>
    resolveInitialPanelSize(
      persistence?.load("terminal") ?? null,
      initialTerminalHeight,
      terminalBounds,
    ),
  );
  const [sidePaneSize, setSidePaneSize] = useState(() =>
    resolveInitialPanelSize(
      persistence?.load("sidePane") ?? null,
      initialSidePaneWidth,
      sidePaneBounds,
    ),
  );

  const terminalResize = usePanelResize({
    orientation: "horizontal",
    size: terminalSize,
    bounds: terminalBounds,
    onResize: setTerminalSize,
  });
  const sidePaneResize = usePanelResize({
    orientation: "vertical",
    size: sidePaneSize,
    bounds: sidePaneBounds,
    onResize: setSidePaneSize,
  });

  useEffect(() => {
    persistence?.save("terminal", terminalSize);
  }, [persistence, terminalSize]);
  useEffect(() => {
    persistence?.save("sidePane", sidePaneSize);
  }, [persistence, sidePaneSize]);

  return (
    <AppShell
      {...rest}
      terminal={terminal}
      sidePane={sidePane}
      terminalResize={
        terminal
          ? {
              size: terminalSize,
              bounds: terminalBounds,
              onResize: setTerminalSize,
              dragging: terminalResize.dragging,
              handleProps: terminalResize.handleProps,
            }
          : undefined
      }
      sidePaneResize={
        sidePane
          ? {
              size: sidePaneSize,
              bounds: sidePaneBounds,
              onResize: setSidePaneSize,
              dragging: sidePaneResize.dragging,
              handleProps: sidePaneResize.handleProps,
            }
          : undefined
      }
    />
  );
}
