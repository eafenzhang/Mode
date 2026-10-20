// M4 adapter：DropdownMenu 子菜单（旧 Radix API：Sub/SubTrigger/SubContent）。

import {
  cloneElement,
  forwardRef,
  isValidElement,
  useCallback,
  useEffect,
  useId,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ButtonHTMLAttributes,
  type CSSProperties,
  type HTMLAttributes,
  type KeyboardEvent as ReactKeyboardEvent,
  type MouseEvent as ReactMouseEvent,
  type ReactElement,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";

import { MenuItem } from "../Menu.js";
import { menuClassName } from "../surfaceStyles.js";
import { childRefOf, mergeRefs } from "./domTargets.js";
import {
  DropdownSubAdapterContext,
  isInsideMenuPortal,
  isSiblingSubmenuActive,
  menuItemNodes,
  moveMenuFocus,
  useDropdownMenuAdapterState,
  useDropdownSubAdapterState,
} from "./dropdownMenuContext.js";
import { computePopoverPoint } from "./popoverPosition.js";
import { cn } from "@/components/lib/utils.js";

type ElementProps = Record<string, unknown>;

const useIsomorphicLayoutEffect = typeof window === "undefined" ? useEffect : useLayoutEffect;

export interface DropdownMenuSubProps {
  open?: boolean;
  defaultOpen?: boolean;
  onOpenChange?: (open: boolean) => void;
  children?: ReactNode;
}

export function DropdownMenuSub({
  open,
  defaultOpen,
  onOpenChange,
  children,
}: DropdownMenuSubProps): ReactNode {
  const parent = useDropdownMenuAdapterState();
  const [uncontrolled, setUncontrolled] = useState(defaultOpen ?? false);
  const current = open ?? uncontrolled;
  const triggerRef = useRef<HTMLElement | null>(null);
  const contentId = useId();
  const setOpen = useCallback(
    (next: boolean) => {
      if (open === undefined) {
        setUncontrolled(next);
      }
      onOpenChange?.(next);
    },
    [open, onOpenChange],
  );

  useEffect(() => {
    if (parent && !parent.open) {
      setOpen(false);
    }
  }, [parent, setOpen]);

  // 兄弟互斥：根菜单只允许一个子菜单打开。指针进入另一个子菜单时关闭自己，
  // 否则旧子菜单的 portal 内容会继续覆盖在新子菜单上（表现为悬停内容不切换）。
  useEffect(() => {
    if (isSiblingSubmenuActive(parent?.activeSubmenuId, contentId)) {
      setOpen(false);
    }
  }, [parent?.activeSubmenuId, contentId, setOpen]);

  const state = useMemo(
    () => ({ open: current, setOpen, triggerRef, contentId }),
    [current, setOpen, contentId],
  );
  return (
    <DropdownSubAdapterContext.Provider value={state}>
      {children}
    </DropdownSubAdapterContext.Provider>
  );
}

export interface DropdownMenuSubTriggerProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  inset?: boolean;
  asChild?: boolean;
}

export const DropdownMenuSubTrigger = forwardRef<HTMLButtonElement, DropdownMenuSubTriggerProps>(
  function DropdownMenuSubTrigger({ inset, asChild, className, children, onClick, ...rest }, ref) {
    const state = useDropdownSubAdapterState();
    const root = useDropdownMenuAdapterState();
    const merged = cn([className, inset ? "ps-7.5" : ""]);
    const handleClick = (event: ReactMouseEvent<HTMLElement>): void => {
      onClick?.(event as ReactMouseEvent<HTMLButtonElement>);
      if (!event.defaultPrevented) {
        root?.setActiveSubmenuId(state?.contentId ?? null);
        state?.setOpen(!state?.open);
      }
    };
    const itemProps = {
      "data-state": state?.open ? "open" : "closed",
      "data-inset": inset ? "" : undefined,
      "aria-haspopup": "menu" as const,
      "aria-expanded": state?.open ?? false,
      "aria-controls": state?.open ? state.contentId : undefined,
      onPointerEnter: () => {
        root?.setActiveSubmenuId(state?.contentId ?? null);
        state?.setOpen(true);
      },
      onClick: handleClick,
    };
    if (asChild && isValidElement(children)) {
      const child = children as ReactElement<ElementProps>;
      const childProps = child.props;
      return cloneElement(child, {
        ...itemProps,
        onClick: (event: ReactMouseEvent<HTMLElement>) => {
          (childProps.onClick as ((event: ReactMouseEvent<HTMLElement>) => void) | undefined)?.(
            event,
          );
          handleClick(event);
        },
        ref: mergeRefs(ref, state?.triggerRef, childRefOf<HTMLButtonElement>(child)),
      });
    }
    return (
      <MenuItem
        {...rest}
        {...itemProps}
        ref={mergeRefs(ref, state?.triggerRef)}
        data-inset={inset ? "" : undefined}
        className={merged}
      >
        {children}
        <svg
          aria-hidden="true"
          viewBox="0 0 16 16"
          className="ms-auto size-3.5 shrink-0 text-foreground-subtle"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.5"
          strokeLinecap="round"
          strokeLinejoin="round"
        >
          <path d="M6 4l4 4-4 4" />
        </svg>
      </MenuItem>
    );
  },
);

export interface DropdownMenuSubContentProps extends HTMLAttributes<HTMLDivElement> {
  forceMount?: boolean;
  sideOffset?: number;
  alignOffset?: number;
  collisionPadding?: number;
}

export const DropdownMenuSubContent = forwardRef<HTMLDivElement, DropdownMenuSubContentProps>(
  function DropdownMenuSubContent(
    {
      forceMount,
      sideOffset = 0,
      alignOffset = 0,
      collisionPadding = 8,
      className,
      children,
      ...rest
    },
    ref,
  ) {
    const state = useDropdownSubAdapterState();
    const root = useDropdownMenuAdapterState();
    const open = state?.open ?? false;
    const contentRef = useRef<HTMLDivElement | null>(null);
    const closeTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
    const [point, setPoint] = useState<{
      top: number;
      left: number;
      vw: number;
      vh: number;
    } | null>(null);

    const cancelClose = useCallback(() => {
      if (closeTimerRef.current !== null) {
        clearTimeout(closeTimerRef.current);
        closeTimerRef.current = null;
      }
    }, []);

    useEffect(() => cancelClose, [cancelClose]);

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
        const next = computePopoverPoint({
          anchor: trigger.getBoundingClientRect(),
          content: { width: rect.width, height: rect.height },
          align: "start",
          side: "right",
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
    }, [open, state, sideOffset, alignOffset, collisionPadding]);

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
            isInsideMenuPortal(target, root?.contentId))
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
        return;
      }
      menuItemNodes(contentRef.current)[0]?.focus();
    }, [open]);

    const handleKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>): void => {
      rest.onKeyDown?.(event);
      if (event.defaultPrevented) {
        return;
      }
      if (event.key === "ArrowDown" || event.key === "ArrowUp") {
        event.preventDefault();
        moveMenuFocus(contentRef.current, event.key === "ArrowDown" ? 1 : -1);
      } else if (event.key === "ArrowLeft" || event.key === "Escape") {
        event.preventDefault();
        state?.setOpen(false);
        state?.triggerRef.current?.focus();
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
      <div
        {...rest}
        ref={mergeRefs(ref, contentRef)}
        id={state?.contentId}
        data-dropdown-menu-root={root?.contentId}
        role="menu"
        hidden={!open}
        data-state={open ? "open" : "closed"}
        data-slot="dropdown-menu-sub-content"
        style={style}
        className={menuClassName(className)}
        onKeyDown={handleKeyDown}
        onPointerEnter={cancelClose}
        onPointerLeave={() => {
          closeTimerRef.current = setTimeout(() => {
            closeTimerRef.current = null;
            state?.setOpen(false);
          }, 150);
        }}
      >
        {children}
      </div>
    );

    return typeof document === "undefined" || !document.body
      ? content
      : createPortal(content, document.body);
  },
);
