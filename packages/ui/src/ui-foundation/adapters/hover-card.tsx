// M4 adapter：HoverCard（旧 API：HoverCard/Trigger/Content）。
// 悬停/聚焦打开，内容可悬停；定位复用 Popover 数学并挂到 body。

import {
  cloneElement,
  createContext,
  forwardRef,
  isValidElement,
  useCallback,
  useContext,
  useEffect,
  useId,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type HTMLAttributes,
  type FocusEvent as ReactFocusEvent,
  type PointerEvent as ReactPointerEvent,
  type ReactElement,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";

import { Popover as FoundationPopover } from "../Popover.js";
import { childRefOf, mergeRefs } from "./domTargets.js";
import { computePopoverPoint, type PopoverAlign, type PopoverSide } from "./popoverPosition.js";

type ElementProps = Record<string, unknown>;

interface AnchorTarget {
  getBoundingClientRect(): DOMRect;
  focus?(): void;
}

interface HoverCardState {
  open: boolean;
  setOpen: (open: boolean) => void;
  scheduleOpen: () => void;
  scheduleClose: () => void;
  cancelTimers: () => void;
  anchorRef: { current: AnchorTarget | null };
  contentId: string;
}

const HoverCardContext = createContext<HoverCardState | null>(null);

const useIsomorphicLayoutEffect = typeof window === "undefined" ? useEffect : useLayoutEffect;

export interface HoverCardProps {
  open?: boolean;
  defaultOpen?: boolean;
  onOpenChange?: (open: boolean) => void;
  openDelay?: number;
  closeDelay?: number;
  children?: ReactNode;
}

export function HoverCard({
  open,
  defaultOpen,
  onOpenChange,
  openDelay = 700,
  closeDelay = 300,
  children,
}: HoverCardProps): ReactNode {
  const [uncontrolled, setUncontrolled] = useState(defaultOpen ?? false);
  const current = open ?? uncontrolled;
  const anchorRef = useRef<AnchorTarget | null>(null);
  const contentId = useId();
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const setOpen = useCallback(
    (next: boolean) => {
      if (open === undefined) {
        setUncontrolled(next);
      }
      onOpenChange?.(next);
    },
    [open, onOpenChange],
  );

  const cancelTimers = useCallback(() => {
    if (timerRef.current !== null) {
      clearTimeout(timerRef.current);
      timerRef.current = null;
    }
  }, []);

  const schedule = useCallback(
    (next: boolean, delay: number) => {
      cancelTimers();
      if (delay <= 0) {
        setOpen(next);
        return;
      }
      timerRef.current = setTimeout(() => {
        timerRef.current = null;
        setOpen(next);
      }, delay);
    },
    [cancelTimers, setOpen],
  );

  useEffect(() => cancelTimers, [cancelTimers]);

  const state = useMemo<HoverCardState>(
    () => ({
      open: current,
      setOpen,
      scheduleOpen: () => schedule(true, openDelay),
      scheduleClose: () => schedule(false, closeDelay),
      cancelTimers,
      anchorRef,
      contentId,
    }),
    [current, setOpen, schedule, openDelay, closeDelay, cancelTimers, contentId],
  );

  return <HoverCardContext.Provider value={state}>{children}</HoverCardContext.Provider>;
}

export interface HoverCardTriggerProps extends HTMLAttributes<HTMLElement> {
  asChild?: boolean;
}

export const HoverCardTrigger = forwardRef<HTMLElement, HoverCardTriggerProps>(
  function HoverCardTrigger(
    { asChild, children, onPointerEnter, onPointerLeave, onFocus, onBlur, ...rest },
    ref,
  ) {
    const state = useContext(HoverCardContext);
    const handlers = {
      onPointerEnter: (event: ReactPointerEvent<HTMLElement>) => {
        onPointerEnter?.(event);
        state?.scheduleOpen();
      },
      onPointerLeave: (event: ReactPointerEvent<HTMLElement>) => {
        onPointerLeave?.(event);
        state?.scheduleClose();
      },
      onFocus: (event: ReactFocusEvent<HTMLElement>) => {
        onFocus?.(event);
        state?.scheduleOpen();
      },
      onBlur: (event: ReactFocusEvent<HTMLElement>) => {
        onBlur?.(event);
        state?.scheduleClose();
      },
      "aria-expanded": state?.open ?? false,
      "data-state": state?.open ? "open" : "closed",
    };
    if (asChild && isValidElement(children)) {
      const child = children as ReactElement<ElementProps>;
      return cloneElement(child, {
        ...handlers,
        ref: mergeRefs(ref, state?.anchorRef, childRefOf<HTMLElement>(child)),
      });
    }
    return (
      <span
        {...rest}
        {...handlers}
        ref={mergeRefs(ref, state?.anchorRef)}
        tabIndex={0}
        data-slot="hover-card-trigger"
      >
        {children}
      </span>
    );
  },
);

export interface HoverCardContentProps extends HTMLAttributes<HTMLDivElement> {
  side?: PopoverSide;
  align?: PopoverAlign;
  sideOffset?: number;
  alignOffset?: number;
  collisionPadding?: number;
}

export const HoverCardContent = forwardRef<HTMLDivElement, HoverCardContentProps>(
  function HoverCardContent(
    {
      side = "bottom",
      align = "center",
      sideOffset = 4,
      alignOffset = 0,
      collisionPadding = 8,
      className,
      children,
      ...rest
    },
    ref,
  ) {
    const state = useContext(HoverCardContext);
    const open = state?.open ?? false;
    const contentRef = useRef<HTMLDivElement | null>(null);
    const [point, setPoint] = useState<{ top: number; left: number } | null>(null);

    useIsomorphicLayoutEffect(() => {
      if (!open) {
        setPoint(null);
        return;
      }
      const anchor = state?.anchorRef.current;
      const content = contentRef.current;
      if (!anchor || !content || typeof window === "undefined") {
        return;
      }
      const measure = (): void => {
        const rect = content.getBoundingClientRect();
        const next = computePopoverPoint({
          anchor: anchor.getBoundingClientRect(),
          content: { width: rect.width, height: rect.height },
          align,
          side,
          sideOffset,
          alignOffset,
          avoidCollisions: true,
          collisionPadding,
          viewport: { width: window.innerWidth, height: window.innerHeight },
        });
        setPoint({ top: next.top, left: next.left });
      };
      measure();
      window.addEventListener("resize", measure);
      window.addEventListener("scroll", measure, true);
      return () => {
        window.removeEventListener("resize", measure);
        window.removeEventListener("scroll", measure, true);
      };
    }, [open, state, align, side, sideOffset, alignOffset, collisionPadding]);

    useEffect(() => {
      if (!open || typeof document === "undefined") {
        return;
      }
      const handleKey = (event: KeyboardEvent): void => {
        if (event.key === "Escape") {
          state?.setOpen(false);
          state?.anchorRef.current?.focus?.();
        }
      };
      document.addEventListener("keydown", handleKey);
      return () => document.removeEventListener("keydown", handleKey);
    }, [open, state]);

    if (!open) {
      return null;
    }

    const style: CSSProperties = {
      ...rest.style,
      position: "fixed",
      top: point?.top ?? 0,
      left: point?.left ?? 0,
      visibility: point ? undefined : "hidden",
    };

    const content = (
      <FoundationPopover
        {...rest}
        ref={mergeRefs(ref, contentRef)}
        id={state?.contentId}
        data-slot="hover-card-content"
        data-state="open"
        style={style}
        className={["p-4", className].filter(Boolean).join(" ")}
        onPointerEnter={() => state?.cancelTimers()}
        onPointerLeave={() => state?.scheduleClose()}
      >
        {children}
      </FoundationPopover>
    );

    return typeof document === "undefined" || !document.body
      ? content
      : createPortal(content, document.body);
  },
);
