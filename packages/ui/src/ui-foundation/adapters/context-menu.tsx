// M4 adapter：ContextMenu（旧 API：ContextMenu/Trigger/Content/Item/Separator/Sub*）。
// 复用 DropdownMenu 的根状态与条目实现，只把定位锚点换成右键时的视口坐标。

import {
  cloneElement,
  createContext,
  forwardRef,
  isValidElement,
  useCallback,
  useContext,
  useId,
  useMemo,
  useRef,
  useState,
  type HTMLAttributes,
  type MouseEvent as ReactMouseEvent,
  type ReactElement,
  type ReactNode,
} from "react";

import {
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  type DropdownMenuContentProps,
  type DropdownMenuItemProps,
} from "./dropdown-menu.js";
import { DropdownMenuAdapterContext } from "./dropdownMenuContext.js";
import { childRefOf, mergeRefs } from "./domTargets.js";

type ElementProps = Record<string, unknown>;

interface ContextMenuPoint {
  x: number;
  y: number;
}

const ContextMenuPointContext = createContext<{
  point: ContextMenuPoint | null;
  setPoint: (point: ContextMenuPoint) => void;
} | null>(null);

export interface ContextMenuProps {
  open?: boolean;
  defaultOpen?: boolean;
  onOpenChange?: (open: boolean) => void;
  modal?: boolean;
  children?: ReactNode;
}

export function ContextMenu({
  open,
  defaultOpen,
  onOpenChange,
  modal: _modal,
  children,
}: ContextMenuProps): ReactNode {
  const [uncontrolled, setUncontrolled] = useState(defaultOpen ?? false);
  const [activeSubmenuId, setActiveSubmenuId] = useState<string | null>(null);
  const [point, setPoint] = useState<ContextMenuPoint | null>(null);
  const current = open ?? uncontrolled;
  const triggerRef = useRef<HTMLElement | null>(null);
  const contentId = useId();
  const setOpen = useCallback(
    (next: boolean) => {
      if (open === undefined) {
        setUncontrolled(next);
      }
      if (!next) {
        setActiveSubmenuId(null);
      }
      onOpenChange?.(next);
    },
    [open, onOpenChange],
  );
  const closeMenu = useCallback(() => {
    setOpen(false);
    triggerRef.current?.focus?.();
  }, [setOpen]);
  const state = useMemo(
    () => ({
      open: current,
      setOpen,
      closeMenu,
      triggerRef,
      contentId,
      activeSubmenuId,
      setActiveSubmenuId,
    }),
    [current, setOpen, closeMenu, contentId, activeSubmenuId],
  );
  const pointState = useMemo(() => ({ point, setPoint }), [point]);
  return (
    <DropdownMenuAdapterContext.Provider value={state}>
      <ContextMenuPointContext.Provider value={pointState}>
        {children}
      </ContextMenuPointContext.Provider>
    </DropdownMenuAdapterContext.Provider>
  );
}

export interface ContextMenuTriggerProps extends HTMLAttributes<HTMLElement> {
  asChild?: boolean;
}

export const ContextMenuTrigger = forwardRef<HTMLElement, ContextMenuTriggerProps>(
  function ContextMenuTrigger({ asChild, children, onContextMenu, ...rest }, ref) {
    const state = useContext(DropdownMenuAdapterContext);
    const pointState = useContext(ContextMenuPointContext);
    const handleContextMenu = (event: ReactMouseEvent<HTMLElement>): void => {
      onContextMenu?.(event);
      if (event.defaultPrevented) {
        return;
      }
      event.preventDefault();
      pointState?.setPoint({ x: event.clientX, y: event.clientY });
      state?.setOpen(true);
    };
    const triggerProps = {
      "aria-haspopup": "menu" as const,
      "data-state": state?.open ? "open" : "closed",
      onContextMenu: handleContextMenu,
    };
    if (asChild && isValidElement(children)) {
      const child = children as ReactElement<ElementProps>;
      const childProps = child.props;
      return cloneElement(child, {
        ...triggerProps,
        onContextMenu: (event: ReactMouseEvent<HTMLElement>) => {
          (
            childProps.onContextMenu as ((event: ReactMouseEvent<HTMLElement>) => void) | undefined
          )?.(event);
          handleContextMenu(event);
        },
        ref: mergeRefs(ref, state?.triggerRef, childRefOf<HTMLElement>(child)),
      });
    }
    return (
      <span {...rest} {...triggerProps} ref={mergeRefs(ref, state?.triggerRef)}>
        {children}
      </span>
    );
  },
);

export type ContextMenuContentProps = DropdownMenuContentProps;

export const ContextMenuContent = forwardRef<HTMLDivElement, ContextMenuContentProps>(
  function ContextMenuContent({ align = "start", sideOffset = 2, ...rest }, ref) {
    const pointState = useContext(ContextMenuPointContext);
    return (
      <DropdownMenuContent
        {...rest}
        ref={ref}
        align={align}
        sideOffset={sideOffset}
        anchorPoint={pointState?.point ?? null}
      />
    );
  },
);

export const ContextMenuSub = DropdownMenuSub;
export const ContextMenuSubTrigger = DropdownMenuSubTrigger;
export const ContextMenuSubContent = DropdownMenuSubContent;
export const ContextMenuSeparator = DropdownMenuSeparator;
export const ContextMenuItem = DropdownMenuItem;

export type ContextMenuItemProps = DropdownMenuItemProps;
export type {
  DropdownMenuSubContentProps as ContextMenuSubContentProps,
  DropdownMenuSubProps as ContextMenuSubProps,
  DropdownMenuSubTriggerProps as ContextMenuSubTriggerProps,
} from "./dropdown-menu.js";
