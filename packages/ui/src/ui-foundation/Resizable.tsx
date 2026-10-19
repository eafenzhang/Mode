// P1.1 Resizable panels（spec: docs/specs/m4-ui-foundation.md §2.14）。
// 持久化在 layoutId 存在时通过 useDefaultLayout；其余 props 直接映射库 API。

import { forwardRef, type ReactNode, type Ref } from "react";
import {
  Group,
  Panel,
  Separator,
  useDefaultLayout,
  type GroupProps,
  type PanelProps,
  type SeparatorProps,
} from "react-resizable-panels";
import { cn } from "@/components/lib/utils.js";

export type ResizablePanelProps = PanelProps;
export type ResizableHandleProps = SeparatorProps;

export interface ResizablePanelGroupProps extends GroupProps {
  layoutId?: string;
  panelIds?: string[];
}

function PersistedPanelGroup({
  layoutId,
  panelIds,
  ...rest
}: ResizablePanelGroupProps & { layoutId: string }): ReactNode {
  // SSR/测试没有 localStorage；给 no-op storage，避免持久化 hook 直接访问全局。
  const storage =
    typeof window === "undefined" ? { getItem: () => null, setItem: () => undefined } : undefined;
  const { defaultLayout, onLayoutChanged } = useDefaultLayout({
    id: layoutId,
    panelIds,
    ...(storage ? { storage } : {}),
  });
  return <Group {...rest} defaultLayout={defaultLayout} onLayoutChanged={onLayoutChanged} />;
}

export function ResizablePanelGroup({
  layoutId,
  panelIds,
  ...rest
}: ResizablePanelGroupProps): ReactNode {
  if (layoutId) {
    return <PersistedPanelGroup {...rest} layoutId={layoutId} panelIds={panelIds} />;
  }
  return <Group {...rest} />;
}

export function ResizablePanel(props: ResizablePanelProps): ReactNode {
  return <Panel {...props} />;
}

export const ResizableHandle = forwardRef<HTMLDivElement, ResizableHandleProps>(
  function ResizableHandle({ className, ...rest }, ref) {
    return (
      <Separator
        {...rest}
        elementRef={ref as Ref<HTMLDivElement>}
        data-slot="resizable-handle"
        className={cn([
          "relative flex items-center justify-center bg-transparent focus-visible:outline-none",
          className,
        ])}
      />
    );
  },
);
