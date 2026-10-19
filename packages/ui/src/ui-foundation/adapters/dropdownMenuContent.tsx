// M4 adapter：DropdownMenu 内容浮层（从 dropdown-menu.tsx 拆出，控制单文件行数）。
// 与 Popover 共用定位数学；portal 到 body 并输出 Radix 内容尺寸变量。

import {
  forwardRef,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type CSSProperties,
  type HTMLAttributes,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react";
import { createPortal } from "react-dom";

import { Menu as FoundationMenu } from "../Menu.js";
import { mergeRefs } from "./domTargets.js";
import {
  isInsideMenuPortal,
  menuItemNodes,
  menuSelectEvent,
  moveMenuFocus,
  useDropdownMenuAdapterState,
  type MenuSelectEvent,
} from "./dropdownMenuContext.js";
import { computePopoverPoint, type PopoverAlign, type PopoverSide } from "./popoverPosition.js";

const useIsomorphicLayoutEffect = typeof window === "undefined" ? useEffect : useLayoutEffect;

export interface DropdownMenuContentProps extends HTMLAttributes<HTMLDivElement> {
  forceMount?: boolean;
  side?: PopoverSide;
  align?: PopoverAlign;
  sideOffset?: number;
  alignOffset?: number;
  collisionPadding?: number;
  loop?: boolean;
  /** ContextMenu 用：以视口坐标代替触发器 rect 定位。 */
  anchorPoint?: { x: number; y: number } | null;
  onOpenAutoFocus?: (event: MenuSelectEvent) => void;
  onCloseAutoFocus?: (event: MenuSelectEvent) => void;
}

export const DropdownMenuContent = forwardRef<HTMLDivElement, DropdownMenuContentProps>(
  function DropdownMenuContent(
    {
      forceMount,
      side = "bottom",
      align = "center",
      sideOffset = 2,
      alignOffset = 0,
      collisionPadding = 8,
      loop: _loop,
      anchorPoint,
      onOpenAutoFocus,
      onCloseAutoFocus,
      className,
      children,
      ...rest
    },
    ref,
  ) {
    const state = useDropdownMenuAdapterState();
    const open = state?.open ?? false;
    const contentRef = useRef<HTMLDivElement | null>(null);
    const wasOpenRef = useRef(false);
    const [point, setPoint] = useState<{
      top: number;
      left: number;
      vw: number;
      vh: number;
    } | null>(null);

    useIsomorphicLayoutEffect(() => {
      if (!open) {
        setPoint(null);
        return;
      }
      const trigger = state?.triggerRef.current;
      const content = contentRef.current;
      if (!trigger || !content || typeof window === "undefined") {
        return;
      }
      const measure = (): void => {
        const rect = content.getBoundingClientRect();
        const anchor = anchorPoint
          ? ({
              top: anchorPoint.y,
              left: anchorPoint.x,
              right: anchorPoint.x,
              bottom: anchorPoint.y,
              width: 0,
              height: 0,
            } as DOMRect)
          : trigger.getBoundingClientRect();
        const next = computePopoverPoint({
          anchor,
          content: { width: rect.width, height: rect.height },
          align,
          side,
          sideOffset,
          alignOffset,
          avoidCollisions: true,
          collisionPadding,
          viewport: { width: window.innerWidth, height: window.innerHeight },
        });
        setPoint({
          top: next.top,
          left: next.left,
          vw: window.innerWidth,
          vh: window.innerHeight,
        });
      };
      measure();
      window.addEventListener("resize", measure);
      window.addEventListener("scroll", measure, true);
      return () => {
        window.removeEventListener("resize", measure);
        window.removeEventListener("scroll", measure, true);
      };
    }, [open, state, align, side, sideOffset, alignOffset, collisionPadding, anchorPoint]);

    useEffect(() => {
      if (!open || typeof document === "undefined") {
        return;
      }
      const handlePointer = (event: globalThis.MouseEvent): void => {
        const target = event.target as Node | null;
        if (
          target &&
          (contentRef.current?.contains(target) ||
            state?.triggerRef.current?.contains(target) ||
            isInsideMenuPortal(target, state?.contentId))
        ) {
          return;
        }
        state?.setOpen(false);
      };
      document.addEventListener("mousedown", handlePointer);
      return () => document.removeEventListener("mousedown", handlePointer);
    }, [open, state]);

    useEffect(() => {
      if (!open) {
        if (!wasOpenRef.current) {
          return;
        }
        wasOpenRef.current = false;
        const event = menuSelectEvent();
        onCloseAutoFocus?.(event);
        if (!event.defaultPrevented) {
          state?.triggerRef.current?.focus();
        }
        return;
      }
      wasOpenRef.current = true;
      const event = menuSelectEvent();
      onOpenAutoFocus?.(event);
      if (event.defaultPrevented) {
        return;
      }
      menuItemNodes(contentRef.current)[0]?.focus();
    }, [open, state, onOpenAutoFocus, onCloseAutoFocus]);

    const handleKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>): void => {
      rest.onKeyDown?.(event);
      if (event.defaultPrevented) {
        return;
      }
      if (event.key === "ArrowDown" || event.key === "ArrowUp") {
        event.preventDefault();
        moveMenuFocus(contentRef.current, event.key === "ArrowDown" ? 1 : -1);
      } else if (event.key === "Home" || event.key === "End") {
        event.preventDefault();
        const items = menuItemNodes(contentRef.current);
        (event.key === "Home" ? items[0] : items[items.length - 1])?.focus();
      } else if (event.key === "Escape" || event.key === "Tab") {
        event.preventDefault();
        state?.closeMenu();
      }
    };

    if (!open && !forceMount) {
      return null;
    }

    const style: CSSProperties & Record<`--${string}`, string> = {
      ...rest.style,
      position: "fixed",
      top: point?.top ?? 0,
      left: point?.left ?? 0,
      visibility: point ? undefined : "hidden",
      "--radix-dropdown-menu-content-available-width": `${Math.max(
        0,
        (point?.vw ?? 0) - (point?.left ?? 0) - collisionPadding,
      )}px`,
      "--radix-dropdown-menu-content-available-height": `${Math.max(
        0,
        (point?.vh ?? 0) - (point?.top ?? 0) - collisionPadding,
      )}px`,
    };

    const content = (
      <FoundationMenu
        {...rest}
        ref={mergeRefs(ref, contentRef)}
        id={state?.contentId}
        data-dropdown-menu-root={state?.contentId}
        hidden={!open}
        data-state={open ? "open" : "closed"}
        style={style}
        className={className}
        onKeyDown={handleKeyDown}
      >
        {children}
      </FoundationMenu>
    );

    return typeof document === "undefined" || !document.body
      ? content
      : createPortal(content, document.body);
  },
);
